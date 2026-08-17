//! MCP 服务器配置持久化
//!
//! 配置以 JSON 数组形式存储在 `app_settings` 表的 `mcp_servers` key 下。

use crate::tools::mcp::McpServerConfig;
use crate::utils::errors::AppError;
use tauri::State;

pub const MCP_SERVERS_KEY: &str = "mcp_servers";

/// 从 app_settings 加载 MCP 服务器配置（不存在或解析失败时返回空列表）
pub fn load_servers(conn: &rusqlite::Connection) -> Result<Vec<McpServerConfig>, AppError> {
    let raw = crate::commands::app_settings::get_setting(conn, MCP_SERVERS_KEY)?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<Vec<McpServerConfig>>(&s).ok())
        .unwrap_or_default())
}

/// 将 MCP 服务器配置保存到 app_settings
pub fn save_servers(
    conn: &rusqlite::Connection,
    servers: &[McpServerConfig],
) -> Result<(), AppError> {
    let json = serde_json::to_string(servers).unwrap_or_else(|_| "[]".to_string());
    crate::commands::app_settings::set_setting(conn, MCP_SERVERS_KEY, &json)
}

#[tauri::command]
pub fn get_mcp_servers(state: State<'_, crate::DbState>) -> Result<Vec<McpServerConfig>, AppError> {
    let conn = state.get_conn()?;
    load_servers(&conn)
}

#[tauri::command]
pub fn set_mcp_servers(
    state: State<'_, crate::DbState>,
    servers: Vec<McpServerConfig>,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    save_servers(&conn, &servers)
}
