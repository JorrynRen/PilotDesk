use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use super::PluginHost;

/// 索引文件 schema 版本
const INDEX_SCHEMA_VERSION: &str = "1.0";

/// 在线商店插件信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlinePluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    #[serde(rename = "minAppVersion")]
    pub min_app_version: String,
    pub permissions: Vec<String>,
    pub entry: OnlinePluginEntry,
    pub contributes: Option<OnlinePluginContributes>,
    pub path: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub icon: Option<String>,
    pub size: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlinePluginEntry {
    pub main: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlinePluginContributes {
    pub panels: Option<Vec<serde_json::Value>>,
    pub commands: Option<Vec<serde_json::Value>>,
    pub hooks: Option<Vec<serde_json::Value>>,
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

fn fetch_url(url: &str) -> Result<String, String> {
    let response = reqwest::blocking::get(url)
        .map_err(|e| format!("HTTP 请求失败: {}", e))?;

    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("HTTP {}: {}", status, url));
    }

    response.text()
        .map_err(|e| format!("读取响应失败: {}", e))
}

/// 从 raw URL 下载文件内容（二进制）
fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let response = reqwest::blocking::get(url)
        .map_err(|e| format!("HTTP 请求失败: {}", e))?;

    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("HTTP {}: {}", status, url));
    }

    response.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("读取响应失败: {}", e))
}

#[tauri::command]
pub fn plugin_store_fetch_index(
    host: tauri::State<'_, Mutex<PluginHost>>,
    force_refresh: Option<bool>,
) -> Result<IndexFetchResult, String> {
    let _host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;

    let force = force_refresh.unwrap_or(false);

    if force {
        let content = fetch_url(INDEX_RAW_URL)?;
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
        let (content, source) = match fetch_url(INDEX_CDN_URL) {
            Ok(c) => (c, "cdn"),
            Err(_) => {
                let c = fetch_url(INDEX_RAW_URL)?;
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

/// 从在线商店安装插件（文件夹模式）
/// 从 raw.githubusercontent.com 逐个下载插件文件到本地插件目录
#[tauri::command]
pub fn plugin_store_install(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
) -> Result<InstallResult, String> {
    // 获取索引找到插件信息
    let content = fetch_url(INDEX_RAW_URL)?;
    let index: PluginIndex = serde_json::from_str(&content)
        .map_err(|e| format!("解析索引失败: {}", e))?;

    let online_plugin = index.plugins.iter()
        .find(|p| p.id == plugin_id)
        .ok_or_else(|| format!("在线商店中未找到插件: {}", plugin_id))?;

    // 检查是否已安装
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

    // 确定插件目录
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

    let base_url = &online_plugin.base_url;

    // 下载文件列表（从 manifest.json 推断需要下载的文件）
    let manifest_url = format!("{}/manifest.json", base_url);
    let manifest_content = fetch_url(&manifest_url)?;

    // 验证 manifest.json
    let manifest: super::PluginManifest = serde_json::from_str(&manifest_content)
        .map_err(|e| format!("解析 manifest.json 失败: {}", e))?;

    // 写入 manifest.json
    std::fs::write(target_dir.join("manifest.json"), &manifest_content)
        .map_err(|e| format!("写入 manifest.json 失败: {}", e))?;

    // 下载入口文件
    let entry_main = &manifest.entry.main;
    let entry_url = format!("{}/{}", base_url, entry_main);
    match fetch_bytes(&entry_url) {
        Ok(data) => {
            let entry_path = target_dir.join(entry_main);
            if let Some(parent) = entry_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&entry_path, &data)
                .map_err(|e| format!("写入入口文件失败: {}", e))?;
        }
        Err(e) => {
            // 入口文件可能不存在于 raw（开发阶段），忽略
            log::warn!("[PluginInstall] 入口文件下载失败 (可能不存在): {}", e);
        }
    }

    // 下载图标（如果存在）
    if let Some(icon) = &online_plugin.icon {
        if !icon.is_empty() {
            match fetch_bytes(icon) {
                Ok(data) => {
                    // 从 icon URL 提取文件名
                    let icon_name = icon.rsplit('/').next().unwrap_or("icon.png");
                    std::fs::write(target_dir.join(icon_name), &data)
                        .map_err(|e| format!("写入图标文件失败: {}", e))?;
                }
                Err(_) => {
                    // 图标不存在是正常的
                    log::info!("[PluginInstall] 图标不存在 (可选): {}", icon);
                }
            }
        }
    }

    // 通过 PluginHost 加载插件
    let manifest_path = target_dir.join("manifest.json");
    let instance = {
        let guard = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        guard.load_and_validate_plugin(&target_dir, &manifest_path)?
    };

    let id = instance.manifest.id.clone();

    // 重新 discover 加载新插件
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
