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
    #[serde(rename = "downloadUrl")]
    pub download_url: String,
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

#[tauri::command]
pub fn plugin_store_fetch_index(
    host: tauri::State<'_, Mutex<PluginHost>>,
    force_refresh: Option<bool>,
) -> Result<IndexFetchResult, String> {
    let _host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;

    let force = force_refresh.unwrap_or(false);

    if force {
        // 强制刷新：直接走 raw
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
        // 优先 CDN，失败降级 raw
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

#[tauri::command]
pub fn plugin_store_install(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
) -> Result<InstallResult, String> {
    // 先获取索引找到插件信息
    let content = fetch_url(INDEX_RAW_URL)?;
    let index: PluginIndex = serde_json::from_str(&content)
        .map_err(|e| format!("解析索引失败: {}", e))?;

    let online_plugin = index.plugins.iter()
        .find(|p| p.id == plugin_id)
        .ok_or_else(|| format!("在线商店中未找到插件: {}", plugin_id))?;

    // 检查是否已安装
    {
        let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        let local_plugins = host.list_plugins();
        if local_plugins.iter().any(|p| p.manifest.id == plugin_id) {
            return Ok(InstallResult {
                plugin_id: plugin_id.clone(),
                plugin_name: online_plugin.name.clone(),
                version: online_plugin.version.clone(),
                already_installed: true,
            });
        }
    }

    // 下载 zipball
    let download_url = &online_plugin.download_url;
    let response = reqwest::blocking::get(download_url)
        .map_err(|e| format!("下载插件失败: {}", e))?;

    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("下载插件失败: HTTP {}", status));
    }

    let body = response.bytes()
        .map_err(|e| format!("读取下载数据失败: {}", e))?;

    // 写入临时文件
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("pilotdesk_plugin_{}.zip", plugin_id));
    std::fs::write(&temp_path, &body)
        .map_err(|e| format!("写入临时文件失败: {}", e))?;

    // 通过 PluginHost 安装
    let mut host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let instance = host.install_from_zip(&temp_path.to_string_lossy())?;

    // 清理临时文件
    let _ = std::fs::remove_file(&temp_path);

    Ok(InstallResult {
        plugin_id: instance.manifest.id.clone(),
        plugin_name: instance.manifest.name.clone(),
        version: instance.manifest.version.clone(),
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
