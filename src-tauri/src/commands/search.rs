//! 联网搜索配置持久化
//!
//! 配置以 JSON 形式存储在 `app_settings` 表的 `search_config` key 下，
//! 由 AgentLoop 在构建 WebSearchTool 时加载。

use crate::api_agent::web::SearchConfig;
use crate::utils::errors::AppError;
use tauri::State;

pub const SEARCH_CONFIG_KEY: &str = "search_config";

/// 从 app_settings 加载搜索配置（不存在或解析失败时返回默认值）
pub fn load_search_config(conn: &rusqlite::Connection) -> SearchConfig {
    let raw = crate::commands::app_settings::get_setting(conn, SEARCH_CONFIG_KEY)
        .ok()
        .flatten();
    raw.and_then(|s| serde_json::from_str::<SearchConfig>(&s).ok())
        .unwrap_or_default()
}

/// 将搜索配置保存到 app_settings
pub fn save_search_config(conn: &rusqlite::Connection, cfg: &SearchConfig) -> Result<(), AppError> {
    let json = serde_json::to_string(cfg).unwrap_or_else(|_| "{}".to_string());
    crate::commands::app_settings::set_setting(conn, SEARCH_CONFIG_KEY, &json)
}

#[tauri::command]
pub fn get_search_config(state: State<'_, crate::DbState>) -> Result<SearchConfig, AppError> {
    let conn = state.get_conn()?;
    Ok(load_search_config(&conn))
}

#[tauri::command]
pub fn set_search_config(
    state: State<'_, crate::DbState>,
    config: SearchConfig,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    save_search_config(&conn, &config)
}
