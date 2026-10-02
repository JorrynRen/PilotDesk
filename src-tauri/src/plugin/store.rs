use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::sync::OnceLock;

use super::PluginHost;

/// 索引文件 schema 版本
const INDEX_SCHEMA_VERSION: &str = "1.0";

/// HTTP 请求超时配置
const CONNECT_TIMEOUT_SECS: u64 = 10;
const READ_TIMEOUT_SECS: u64 = 30;

/// HTTP 重试配置
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY_MS: u64 = 1000;

/// 在线商店插件信息（精简版，仅存储浏览/搜索所需字段）
///
/// 索引里**只存相对路径**（`path` + 图标/README 的**文件名**），不出现任何绝对 URL；
/// 绝对地址由 `fetch_plugin_index` 在运行时用「实际命中的源 + `path`」算出来再填回
/// `base_url` / `icon` / `readme` —— 所以换镜像不需要重新生成索引。
/// 完整清单（permissions/entry/contributes）安装时从 `<base_url>/manifest.json` 读取。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlinePluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    #[serde(rename = "minAppVersion")]
    pub min_app_version: String,
    /// **相对** `server/market/` 的插件目录，如 `plugins/hello-world/1.0.0`。唯一权威字段。
    pub path: String,
    /// 绝对地址，由归一化填入 —— 索引里**不存在**该字段，故必须 `default`。
    /// 保留它是为了前端契约不变（前端用 `baseUrl` 展示与兜底）。
    ///
    /// ⚠️ 必须显式 rename：本结构体没有 `rename_all`，字段名 `base_url` 会原样序列化成
    /// `base_url`，而前端读的是 `baseUrl` —— 少了这行，前端拿到 `undefined`，
    /// 传给 `read_plugin_readme` 的 `pluginId` 会被 JSON 序列化丢掉，
    /// 后端就报 `missing required key pluginId`（"查看 README"必挂）。
    #[serde(default, rename = "baseUrl")]
    pub base_url: String,
    /// 索引里是图标**文件名**（如 `favicon.png`），归一化后变成绝对地址。
    #[serde(default)]
    pub icon: Option<String>,
    pub size: Option<String>,
    /// 索引里是 README **文件名**（`README.md`），归一化后变成绝对地址。
    #[serde(default)]
    pub readme: Option<String>,
    /// 入口文件的 sha256（十六进制）。远程安装时的完整性锚点；
    /// 索引缺失该字段时安装会被拒绝，不做静默放行。
    #[serde(default)]
    pub sha256: Option<String>,
}

/// 插件索引
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndex {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub plugins: Vec<OnlinePluginInfo>,
}

/// 索引获取结果
#[derive(Debug, Clone, Serialize)]
pub struct IndexFetchResult {
    pub plugins: Vec<OnlinePluginInfo>,
    pub updated_at: String,
    pub source: String,
}

/// 安装结果
#[derive(Debug, Clone, Serialize)]
pub struct InstallResult {
    pub plugin_id: String,
    pub plugin_name: String,
    pub version: String,
    pub already_installed: bool,
}

/// 本地已安装的插件版本信息
#[derive(Debug, Clone, Serialize)]
pub struct LocalPluginVersion {
    pub id: String,
    pub version: String,
}

// ── CDN / Raw URL ──
// 服务器源定义见 market.rs

/// 获取或创建共享的异步 HTTP 客户端（带超时）
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
            .build()
            .expect("创建 HTTP 客户端失败")
    })
}

/// 从 URL 获取文本内容（异步，带超时，自动重试）
async fn fetch_url(url: &str) -> Result<String, String> {
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRIES {
        let response = http_client().get(url).send().await;

        match response {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    return resp
                        .text()
                        .await
                        .map_err(|e| format!("读取响应失败: {}", e));
                } else if status >= 500 && attempt < MAX_RETRIES {
                    // 5xx 错误可重试
                    last_err = format!("HTTP {}: {}", status, url);
                    log::warn!(
                        "[HTTP] 第 {} 次请求失败 ({}), {}ms 后重试...",
                        attempt,
                        last_err,
                        RETRY_DELAY_MS
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                } else {
                    return Err(format!("HTTP {}: {}", status, url));
                }
            }
            Err(e) => {
                if attempt < MAX_RETRIES && (e.is_timeout() || e.is_connect()) {
                    last_err = if e.is_timeout() {
                        format!("请求超时 ({}s): {}", READ_TIMEOUT_SECS, url)
                    } else {
                        format!("连接失败 ({}s): {}", CONNECT_TIMEOUT_SECS, url)
                    };
                    log::warn!(
                        "[HTTP] 第 {} 次请求失败 ({}), {}ms 后重试...",
                        attempt,
                        last_err,
                        RETRY_DELAY_MS
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
                return Err(if e.is_timeout() {
                    format!("请求超时 ({}s): {}", READ_TIMEOUT_SECS, url)
                } else if e.is_connect() {
                    format!("连接失败 ({}s): {}", CONNECT_TIMEOUT_SECS, url)
                } else if e.is_request() {
                    format!("请求被取消: {}", url)
                } else {
                    format!("HTTP 请求失败: {} - {}", e, url)
                });
            }
        }
    }
    Err(format!("重试 {} 次后仍失败: {}", MAX_RETRIES, last_err))
}

/// 从 URL 下载文件内容（二进制，异步，带超时，自动重试）
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRIES {
        let response = http_client().get(url).send().await;

        match response {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    return resp
                        .bytes()
                        .await
                        .map(|b| b.to_vec())
                        .map_err(|e| format!("读取响应失败: {}", e));
                } else if status >= 500 && attempt < MAX_RETRIES {
                    last_err = format!("HTTP {}: {}", status, url);
                    log::warn!(
                        "[HTTP] 第 {} 次请求失败 ({}), {}ms 后重试...",
                        attempt,
                        last_err,
                        RETRY_DELAY_MS
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                } else {
                    return Err(format!("HTTP {}: {}", status, url));
                }
            }
            Err(e) => {
                if attempt < MAX_RETRIES && (e.is_timeout() || e.is_connect()) {
                    last_err = if e.is_timeout() {
                        format!("请求超时 ({}s): {}", READ_TIMEOUT_SECS, url)
                    } else {
                        format!("连接失败 ({}s): {}", CONNECT_TIMEOUT_SECS, url)
                    };
                    log::warn!(
                        "[HTTP] 第 {} 次请求失败 ({}), {}ms 后重试...",
                        attempt,
                        last_err,
                        RETRY_DELAY_MS
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
                return Err(if e.is_timeout() {
                    format!("请求超时 ({}s): {}", READ_TIMEOUT_SECS, url)
                } else if e.is_connect() {
                    format!("连接失败 ({}s): {}", CONNECT_TIMEOUT_SECS, url)
                } else if e.is_request() {
                    format!("请求被取消: {}", url)
                } else {
                    format!("HTTP 请求失败: {} - {}", e, url)
                });
            }
        }
    }
    Err(format!("重试 {} 次后仍失败: {}", MAX_RETRIES, last_err))
}

/// 把索引里的绝对文件地址重新挂到 `base_url` 下（只取文件名），
/// 保证 icon / readme 与 baseUrl 来自同一个源，不会出现跨源混搭。
fn rehost_file(url: Option<&str>, base_url: &str) -> Option<String> {
    let file = url?.trim().rsplit('/').next().filter(|s| !s.is_empty())?;
    Some(format!("{}/{}", base_url.trim_end_matches('/'), file))
}

#[tauri::command]
/// 从所有服务器源尝试获取并解析插件索引。
///
/// 索引里的 `baseUrl` / `icon` / `readme` 是"兼容字段"（生成器写的是主源地址）。
/// 这里一律丢弃它们的 host，改用「**实际命中的源** + 条目内相对 `path`」重算，
/// 使前端拿到的绝对地址与本次索引来源、后续安装来源保持一致。
async fn fetch_plugin_index(_force: bool) -> Result<IndexFetchResult, String> {
    let (json, source) = crate::utils::market::fetch_market_json_with_source(
        crate::utils::market::PLUGINS_INDEX_PATH,
    )
    .await?;
    let mut index: PluginIndex =
        serde_json::from_value(json).map_err(|e| format!("解析索引失败: {}", e))?;

    if index.schema_version != INDEX_SCHEMA_VERSION {
        return Err(format!(
            "索引 schema 版本不兼容: {} (期望: {})",
            index.schema_version, INDEX_SCHEMA_VERSION
        ));
    }

    // 归一化：相对 `path` 才是唯一权威，绝对字段只是给老客户端兜底的兼容字段。
    for plugin in &mut index.plugins {
        let base_url = crate::utils::market::build_market_url(&source, &plugin.path);
        plugin.base_url = base_url.clone();
        plugin.icon = rehost_file(plugin.icon.as_deref(), &base_url);
        plugin.readme = rehost_file(plugin.readme.as_deref(), &base_url);
    }

    Ok(IndexFetchResult {
        plugins: index.plugins,
        updated_at: index.updated_at,
        source,
    })
}

#[tauri::command]
pub async fn plugin_store_fetch_index(
    _host: tauri::State<'_, Mutex<PluginHost>>,
    force_refresh: Option<bool>,
) -> Result<IndexFetchResult, String> {
    fetch_plugin_index(force_refresh.unwrap_or(false)).await
}

/// 从在线商店安装插件（文件夹模式，异步）
/// 从当前可用镜像源逐个下载插件文件到本地插件目录（同插件所有文件同源）
#[tauri::command]
pub async fn plugin_store_install(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
) -> Result<InstallResult, String> {
    // 获取索引找到插件信息（复用公共 fetch 函数）
    let index_result = fetch_plugin_index(false).await?;
    let online_plugin = index_result
        .plugins
        .iter()
        .find(|p| p.id == plugin_id)
        .ok_or_else(|| format!("在线商店中未找到插件: {}", plugin_id))?
        .clone();

    // 检查是否已安装（短暂锁定，不跨 await）
    {
        let guard = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        let local_plugins = guard.list_plugins();
        if local_plugins.iter().any(|p| p.manifest.id == plugin_id) {
            return Ok(InstallResult {
                plugin_id: plugin_id.clone(),
                plugin_name: online_plugin.name.clone(),
                version: online_plugin.version.clone(),
                already_installed: true,
            });
        }
    }

    // 确定插件目录（短暂锁定，不跨 await）
    let plugins_dir = {
        let guard = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        let info = guard.get_sandbox_info();
        std::path::PathBuf::from(info.plugins_dir)
    };

    let target_dir = plugins_dir.join(&plugin_id);
    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).map_err(|e| format!("清理旧插件目录失败: {}", e))?;
    }
    std::fs::create_dir_all(&target_dir).map_err(|e| format!("创建插件目录失败: {}", e))?;

    // ── 安装过程：任何步骤失败则回滚删除目录 ──
    let install_result = install_plugin_from_sources(&target_dir, &online_plugin, &*host).await;

    match install_result {
        Ok(instance_id) => {
            // 重新 discover 加载新插件（短暂锁定，不跨 await）
            {
                let mut guard = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
                guard.discover();
            }

            Ok(InstallResult {
                plugin_id: instance_id,
                plugin_name: online_plugin.name.clone(),
                version: online_plugin.version.clone(),
                already_installed: false,
            })
        }
        Err(e) => {
            // 安装失败，回滚：删除残留的插件目录
            log::warn!(
                "[PluginInstall] 安装失败，回滚删除目录: {}",
                target_dir.display()
            );
            if target_dir.exists() {
                let _ = std::fs::remove_dir_all(&target_dir);
            }
            Err(e)
        }
    }
}

/// 按源优先级逐个尝试安装：**同一个插件的所有文件必须来自同一个源**。
///
/// 先选定一个源，再一次性下载 manifest/入口/图标/README；只有该源整体失败时，
/// 才清空目录、整体降级到下一个源重来 —— 避免出现"一半来自 CDN、一半来自 GitHub"
/// 的跨源混搭状态。目录回滚由调用方负责。
async fn install_plugin_from_sources(
    target_dir: &std::path::Path,
    online_plugin: &OnlinePluginInfo,
    host: &Mutex<PluginHost>,
) -> Result<String, String> {
    let mut last_err = String::new();
    for source in crate::utils::market::SERVER_SOURCES {
        // base_url 由「源 + 相对 path」动态解析，与源固定绑定
        let base_url = crate::utils::market::build_market_url(source, &online_plugin.path);

        // 降级换源前清掉上一个源可能残留的半成品文件
        if target_dir.exists() {
            let _ = std::fs::remove_dir_all(target_dir);
        }
        std::fs::create_dir_all(target_dir).map_err(|e| format!("创建插件目录失败: {}", e))?;

        match install_plugin_files(target_dir, &base_url, online_plugin, host).await {
            Ok(id) => return Ok(id),
            Err(e) => {
                log::warn!(
                    "[PluginInstall] 源 {} 安装失败，降级到下一个源: {}",
                    source,
                    e
                );
                last_err = e;
            }
        }
    }
    Err(format!("所有服务器源均安装失败: {}", last_err))
}

/// 执行插件文件下载和加载（内部函数，失败时由调用方回滚）
async fn install_plugin_files(
    target_dir: &std::path::Path,
    base_url: &str,
    online_plugin: &OnlinePluginInfo,
    host: &Mutex<PluginHost>,
) -> Result<String, String> {
    // 下载 manifest.json
    let manifest_url = format!("{}/manifest.json", base_url);
    let manifest_content = fetch_url(&manifest_url).await?;

    // 验证 manifest.json
    let manifest: super::PluginManifest = serde_json::from_str(&manifest_content)
        .map_err(|e| format!("解析 manifest.json 失败: {}", e))?;

    // 写入 manifest.json
    std::fs::write(target_dir.join("manifest.json"), &manifest_content)
        .map_err(|e| format!("写入 manifest.json 失败: {}", e))?;

    // 下载入口文件
    let entry_main = manifest.entry.main.clone();
    let entry_url = format!("{}/{}", base_url, entry_main);

    // 完整性校验锚点：索引必须提供入口文件的 sha256，缺失即拒绝安装（不静默放行）
    let expected_sha = online_plugin
        .sha256
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "插件 '{}' 的索引缺少完整性信息（sha256），已拒绝安装",
                online_plugin.id
            )
        })?;

    let entry_data = fetch_bytes(&entry_url)
        .await
        .map_err(|e| format!("入口文件下载失败: {}", e))?;

    let actual_sha = crate::api_agent::knowledge::sha256_hex(&entry_data);
    if !actual_sha.eq_ignore_ascii_case(expected_sha) {
        return Err(format!(
            "插件 '{}' 入口文件完整性校验失败：期望 sha256={}，实际={}。请稍后重试；若持续失败，可能是市场缓存未刷新",
            online_plugin.id, expected_sha, actual_sha
        ));
    }

    let entry_path = target_dir.join(&entry_main);
    if let Some(parent) = entry_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&entry_path, &entry_data).map_err(|e| format!("写入入口文件失败: {}", e))?;

    // 下载图标（如果存在，支持 .png / .ico）
    // 文件名取自索引，host 一律换成当前源的 base_url，保证与其它文件同源
    if let Some(icon_name) = online_plugin
        .icon
        .as_deref()
        .map(str::trim)
        .and_then(|u| u.rsplit('/').next())
        .filter(|s| !s.is_empty())
    {
        let icon_url = format!("{}/{}", base_url.trim_end_matches('/'), icon_name);
        match fetch_bytes(&icon_url).await {
            Ok(data) => {
                std::fs::write(target_dir.join(icon_name), &data)
                    .map_err(|e| format!("写入图标文件失败: {}", e))?;
            }
            Err(_) => {
                // 尝试备选扩展名（.png → .ico）
                let alt_name = if icon_name.ends_with(".png") {
                    icon_name.replace(".png", ".ico")
                } else if icon_name.ends_with(".ico") {
                    icon_name.replace(".ico", ".png")
                } else {
                    String::new()
                };
                if !alt_name.is_empty() {
                    let alt_url = format!("{}/{}", base_url.trim_end_matches('/'), alt_name);
                    if let Ok(data) = fetch_bytes(&alt_url).await {
                        std::fs::write(target_dir.join(&alt_name), &data)
                            .map_err(|e| format!("写入图标文件失败: {}", e))?;
                    }
                }
            }
        }
    }

    // 下载 README.md（可选，硬编码路径，不依赖索引字段）
    let readme_url = format!("{}/README.md", base_url);
    match fetch_bytes(&readme_url).await {
        Ok(data) => {
            let _ = std::fs::write(target_dir.join("README.md"), &data);
        }
        Err(_) => {
            // README.md 可选，下载失败不阻塞安装
            log::info!("[PluginInstall] README.md 不存在 (可选): {}", readme_url);
        }
    }

    // 通过 PluginHost 加载插件（短暂锁定，不跨 await）
    let manifest_path = target_dir.join("manifest.json");
    let instance = {
        let guard = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        guard.load_and_validate_plugin(target_dir, &manifest_path)?
    };

    let id = instance.manifest.id.clone();

    Ok(id)
}

#[tauri::command]
pub fn plugin_store_get_local_versions(
    host: tauri::State<'_, Mutex<PluginHost>>,
) -> Result<Vec<LocalPluginVersion>, String> {
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    // 先刷新缓存，确保手动删除的插件不会残留
    host.discover();
    let plugins = host.list_plugins();

    Ok(plugins
        .iter()
        .map(|p| LocalPluginVersion {
            id: p.manifest.id.clone(),
            version: p.manifest.version.clone(),
        })
        .collect())
}

/// 读取插件 README.md 内容
/// - 本地插件：从插件目录读取 README.md 文件
/// - 远程插件：从 remote_url/README.md 下载（带超时）
#[tauri::command]
pub async fn read_plugin_readme(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    is_remote: Option<bool>,
    remote_url: Option<String>,
) -> Result<String, String> {
    if is_remote.unwrap_or(false) {
        // 远程模式：从 remote_url 下载 README.md
        let url = remote_url.ok_or_else(|| "远程模式需要提供 remote_url")?;
        let readme_url = format!("{}/README.md", url.trim_end_matches('/'));

        // 带超时的 HTTP 请求（10 秒超时）
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

        let resp = client.get(&readme_url).send().await.map_err(|e| {
            if e.is_timeout() {
                format!("请求超时（10秒）: {}", readme_url)
            } else if e.is_connect() {
                format!("连接失败: {}", readme_url)
            } else {
                format!("请求失败: {} - {}", e, readme_url)
            }
        })?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}: {}", resp.status().as_u16(), readme_url));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;
        Ok(text)
    } else {
        // 本地模式：从插件目录读取 README.md 文件
        let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        let plugins = host.list_plugins();
        let plugin = plugins
            .iter()
            .find(|p| p.manifest.id == plugin_id)
            .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

        let readme_path = std::path::Path::new(&plugin.path).join("README.md");
        if !readme_path.exists() {
            return Err("NOT_FOUND".to_string());
        }

        let content = std::fs::read_to_string(&readme_path)
            .map_err(|e| format!("读取 README.md 失败: {}", e))?;
        Ok(content)
    }
}
