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
/// 完整清单（permissions/entry/contributes）安装时从 baseUrl/manifest.json 读取
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlinePluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    #[serde(rename = "minAppVersion")]
    pub min_app_version: String,
    pub path: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub icon: Option<String>,
    pub size: Option<String>,
    pub readme: Option<String>,
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

const INDEX_CDN_URL: &str = "https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main/server/market/plugins/index.json";
const INDEX_RAW_URL: &str = "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/index.json";

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
        let response = http_client()
            .get(url)
            .send()
            .await;

        match response {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    return resp.text()
                        .await
                        .map_err(|e| format!("读取响应失败: {}", e));
                } else if status >= 500 && attempt < MAX_RETRIES {
                    // 5xx 错误可重试
                    last_err = format!("HTTP {}: {}", status, url);
                    log::warn!("[HTTP] 第 {} 次请求失败 ({}), {}ms 后重试...", attempt, last_err, RETRY_DELAY_MS);
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
                    log::warn!("[HTTP] 第 {} 次请求失败 ({}), {}ms 后重试...", attempt, last_err, RETRY_DELAY_MS);
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
        let response = http_client()
            .get(url)
            .send()
            .await;

        match response {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    return resp.bytes()
                        .await
                        .map(|b| b.to_vec())
                        .map_err(|e| format!("读取响应失败: {}", e));
                } else if status >= 500 && attempt < MAX_RETRIES {
                    last_err = format!("HTTP {}: {}", status, url);
                    log::warn!("[HTTP] 第 {} 次请求失败 ({}), {}ms 后重试...", attempt, last_err, RETRY_DELAY_MS);
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
                    log::warn!("[HTTP] 第 {} 次请求失败 ({}), {}ms 后重试...", attempt, last_err, RETRY_DELAY_MS);
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

#[tauri::command]
pub async fn plugin_store_fetch_index(
    _host: tauri::State<'_, Mutex<PluginHost>>,
    force_refresh: Option<bool>,
) -> Result<IndexFetchResult, String> {
    let force = force_refresh.unwrap_or(false);

    if force {
        let content = fetch_url(INDEX_RAW_URL).await?;
        let index: PluginIndex = serde_json::from_str(&content)
            .map_err(|e| format!("解析索引失败: {}", e))?;

        if index.schema_version != INDEX_SCHEMA_VERSION {
            return Err(format!("索引 schema 版本不兼容: {} (期望: {})", index.schema_version, INDEX_SCHEMA_VERSION));
        }

        Ok(IndexFetchResult {
            plugins: index.plugins,
            updated_at: index.updated_at,
            source: "raw".to_string(),
        })
    } else {
        let (content, source) = match fetch_url(INDEX_CDN_URL).await {
            Ok(c) => (c, "cdn"),
            Err(_) => {
                let c = fetch_url(INDEX_RAW_URL).await?;
                (c, "raw")
            }
        };

        let index: PluginIndex = serde_json::from_str(&content)
            .map_err(|e| format!("解析索引失败: {}", e))?;

        if index.schema_version != INDEX_SCHEMA_VERSION {
            return Err(format!("索引 schema 版本不兼容: {} (期望: {})", index.schema_version, INDEX_SCHEMA_VERSION));
        }

        Ok(IndexFetchResult {
            plugins: index.plugins,
            updated_at: index.updated_at,
            source: source.to_string(),
        })
    }
}

/// 从在线商店安装插件（文件夹模式，异步）
/// 从 raw.githubusercontent.com 逐个下载插件文件到本地插件目录
#[tauri::command]
pub async fn plugin_store_install(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
) -> Result<InstallResult, String> {
    // 获取索引找到插件信息（CDN 优先，raw 降级）
    let index_content = match fetch_url(INDEX_CDN_URL).await {
        Ok(c) => c,
        Err(_) => fetch_url(INDEX_RAW_URL).await?,
    };
    let index: PluginIndex = serde_json::from_str(&index_content)
        .map_err(|e| format!("解析索引失败: {}", e))?;

    let online_plugin = index.plugins.iter()
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
        std::fs::remove_dir_all(&target_dir)
            .map_err(|e| format!("清理旧插件目录失败: {}", e))?;
    }
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("创建插件目录失败: {}", e))?;

    let base_url = online_plugin.base_url.clone();

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
    match fetch_bytes(&entry_url).await {
        Ok(data) => {
            let entry_path = target_dir.join(&entry_main);
            if let Some(parent) = entry_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&entry_path, &data)
                .map_err(|e| format!("写入入口文件失败: {}", e))?;
        }
        Err(e) => {
            log::warn!("[PluginInstall] 入口文件下载失败 (可能不存在): {}", e);
        }
    }

    // 下载图标（如果存在，支持 .png / .ico）
    if let Some(icon) = &online_plugin.icon {
        if !icon.is_empty() {
            match fetch_bytes(icon).await {
                Ok(data) => {
                    let icon_name = icon.rsplit('/').next().unwrap_or("icon.png");
                    std::fs::write(target_dir.join(icon_name), &data)
                        .map_err(|e| format!("写入图标文件失败: {}", e))?;
                }
                Err(_) => {
                    // 尝试备选扩展名（.png → .ico）
                    let alt_icon = if icon.ends_with(".png") {
                        icon.replace(".png", ".ico")
                    } else if icon.ends_with(".ico") {
                        icon.replace(".ico", ".png")
                    } else {
                        String::new()
                    };
                    if !alt_icon.is_empty() {
                        if let Ok(data) = fetch_bytes(&alt_icon).await {
                            let icon_name = alt_icon.rsplit('/').next().unwrap_or("icon.png");
                            std::fs::write(target_dir.join(icon_name), &data)
                                .map_err(|e| format!("写入图标文件失败: {}", e))?;
                        }
                    }
                }
            }
        }
    }

    // 下载 README.md（如果存在）
    if let Some(readme) = &online_plugin.readme {
        if !readme.is_empty() {
            match fetch_bytes(readme).await {
                Ok(data) => {
                    std::fs::write(target_dir.join("README.md"), &data)
                        .map_err(|e| format!("写入 README.md 失败: {}", e))?;
                }
                Err(_) => {
                    log::info!("[PluginInstall] README.md 不存在 (可选): {}", readme);
                }
            }
        }
    }

    // 通过 PluginHost 加载插件（短暂锁定，不跨 await）
    let manifest_path = target_dir.join("manifest.json");
    let instance = {
        let guard = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        guard.load_and_validate_plugin(&target_dir, &manifest_path)?
    };

    let id = instance.manifest.id.clone();

    // 重新 discover 加载新插件（短暂锁定，不跨 await）
    {
        let mut guard = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        guard.discover();
    }

    Ok(InstallResult {
        plugin_id: id,
        plugin_name: online_plugin.name.clone(),
        version: online_plugin.version.clone(),
        already_installed: false,
    })
}

#[tauri::command]
pub fn plugin_store_get_local_versions(
    host: tauri::State<'_, Mutex<PluginHost>>,
) -> Result<Vec<LocalPluginVersion>, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let plugins = host.list_plugins();

    Ok(plugins.iter().map(|p| LocalPluginVersion {
        id: p.manifest.id.clone(),
        version: p.manifest.version.clone(),
    }).collect())
}
