//! Agent 权限规则（allow/deny 列表）持久化
//!
//! 规则以 JSON 形式存储在 `app_settings` 表的 `agent_permission_rules` key 下，
//! 由 AgentLoop 在工具审批前加载并执行确定性拦截/放行。

use crate::api_agent::agent_loop::PermissionRules;
use crate::utils::errors::AppError;
use tauri::State;

pub const PERMISSION_RULES_KEY: &str = "agent_permission_rules";

/// 从 app_settings 加载权限规则（不存在或解析失败时返回默认空规则）
pub fn load_rules(conn: &rusqlite::Connection) -> Result<PermissionRules, AppError> {
    let raw = crate::commands::app_settings::get_setting(conn, PERMISSION_RULES_KEY)?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<PermissionRules>(&s).ok())
        .unwrap_or_default())
}

/// 将权限规则保存到 app_settings
pub fn save_rules(conn: &rusqlite::Connection, rules: &PermissionRules) -> Result<(), AppError> {
    let json = serde_json::to_string(rules).unwrap_or_else(|_| "{}".to_string());
    crate::commands::app_settings::set_setting(conn, PERMISSION_RULES_KEY, &json)
}

#[tauri::command]
pub fn get_permission_rules(
    state: State<'_, crate::DbState>,
) -> Result<PermissionRules, AppError> {
    let conn = state.get_conn()?;
    load_rules(&conn)
}

#[tauri::command]
pub fn set_permission_rules(
    state: State<'_, crate::DbState>,
    rules: PermissionRules,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    save_rules(&conn, &rules)
}
