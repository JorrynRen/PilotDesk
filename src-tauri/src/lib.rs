mod agent;
mod commands;
mod db;
mod plugin;
mod utils;

use db::init::{init_db, DbPool};
use std::sync::Mutex;
use tokio::sync::Mutex as AsyncMutex;
use agent::AgentManager;

pub struct DbState {
    pub pool: DbPool,
}

impl DbState {
    pub fn get_conn(&self) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, crate::utils::errors::AppError> {
        self.pool.get().map_err(|e| crate::utils::errors::AppError::Lock(format!("数据库连接获取失败: {}", e)))
    }
}

// ── 数据库命令 ──

#[tauri::command]
fn list_tags(state: tauri::State<'_, DbState>) -> Result<Vec<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::list_tags(&conn)
}

#[tauri::command]
fn list_api_providers(state: tauri::State<'_, DbState>) -> Result<Vec<commands::api_provider::ApiProvider>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::list_api_providers(&conn)
}

#[tauri::command]
fn get_inspiration(state: tauri::State<'_, DbState>, id: String) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::get_inspiration(&conn, id)
}

#[tauri::command]
fn create_inspiration(state: tauri::State<'_, DbState>, payload: commands::inspiration::CreateInspirationPayload) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::create_inspiration(&conn, payload)
}

#[tauri::command]
fn update_inspiration(state: tauri::State<'_, DbState>, payload: commands::inspiration::UpdateInspirationPayload) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::update_inspiration(&conn, payload)
}

#[tauri::command]
fn delete_inspiration(state: tauri::State<'_, DbState>, id: String) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::delete_inspiration(&conn, id)
}

#[tauri::command]
fn list_inspirations(
    state: tauri::State<'_, DbState>,
    tag: Option<String>,
    favorite_only: Option<bool>,
) -> Result<Vec<commands::inspiration::Inspiration>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::list_inspirations(&conn, tag, favorite_only.unwrap_or(false))
}

#[tauri::command]
fn search_inspirations(
    state: tauri::State<'_, DbState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<commands::inspiration::Inspiration>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::search_inspirations(&conn, query, limit.unwrap_or(50))
}

#[tauri::command]
fn get_api_provider(state: tauri::State<'_, DbState>, id: String) -> Result<Option<commands::api_provider::ApiProvider>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::get_api_provider(&conn, &id)
}

#[tauri::command]
fn upsert_api_provider(state: tauri::State<'_, DbState>, payload: commands::api_provider::CreateOrUpdateProvider) -> Result<commands::api_provider::ApiProvider, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::upsert_api_provider(&conn, &payload)
}

#[tauri::command]
fn delete_api_provider(state: tauri::State<'_, DbState>, id: String) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::delete_api_provider(&conn, &id)
}

#[tauri::command]
fn get_api_key(state: tauri::State<'_, DbState>, id: String) -> Result<Option<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::get_api_key(&conn, &id)
}

#[tauri::command]
fn reorder_api_providers(state: tauri::State<'_, DbState>, ids: Vec<String>) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::reorder_api_providers(&conn, &ids)
}

#[tauri::command]
fn get_app_setting(state: tauri::State<'_, DbState>, key: String) -> Result<Option<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::app_settings::get_setting(&conn, &key)
}

#[tauri::command]
fn set_app_setting(state: tauri::State<'_, DbState>, key: String, value: String) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::app_settings::set_setting(&conn, &key, &value)
}

#[tauri::command]
fn get_theme(state: tauri::State<'_, DbState>) -> Result<String, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::theme::get_theme(&conn)
}

#[tauri::command]
fn set_theme_cmd(state: tauri::State<'_, DbState>, theme: String) -> Result<String, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::theme::set_theme(&conn, theme)
}

// ── Agent 命令 ──

/// 使用 AgentConfig 元信息驱动的 send_message
/// 从 DB 加载 AgentConfig，通过 StdioHandler 驱动进程交互
#[tauri::command]
async fn agent_send_message_with_config(
    app: tauri::AppHandle,
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    state: tauri::State<'_, DbState>,
    session_id: String,
    agent_type: String,
    message: String,
    mode: String,
    cwd: Option<String>,
    system_prompt: Option<String>,
    agent_session_id: Option<String>,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let config = commands::agents::get_agent_inner(&conn, &agent_type)
        .map_err(|e| format!("查询 Agent 配置失败: {}", e))?
        .ok_or_else(|| format!("未知 Agent 类型: {}", agent_type))?;
    let mut mgr = agent_mgr.lock().await;
    mgr.send_message_with_config(app, session_id, config, message, mode, cwd, system_prompt, agent_session_id).await
}

#[tauri::command]
async fn agent_stop_generation(
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    session_id: String,
) -> Result<(), String> {
    let mut mgr = agent_mgr.lock().await;
    mgr.stop_generation(&session_id);
    Ok(())
}

#[tauri::command]
async fn agent_create_session(
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    session_id: String,
    agent_type: String,
    cwd: Option<String>,
) -> Result<(), String> {
    let mut mgr = agent_mgr.lock().await;
    mgr.create_session(&session_id, &agent_type, cwd.as_deref());
    Ok(())
}

#[tauri::command]
async fn agent_close_session(
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    session_id: String,
) -> Result<(), String> {
    let mut mgr = agent_mgr.lock().await;
    mgr.close_session(&session_id);
    Ok(())
}

#[tauri::command]
async fn agent_list_skills(state: tauri::State<'_, DbState>, agent_type: String) -> Result<Vec<crate::db::models::SkillInfo>, String> {
    let config = state.get_conn()
        .ok()
        .and_then(|conn| commands::agents::get_agent_inner(&conn, &agent_type).ok()?)
        ;
    Ok(agent::AgentManager::list_skills(&agent_type, config.as_ref()).await)
}

#[tauri::command]
async fn agent_detect_installed(state: tauri::State<'_, DbState>) -> Result<Vec<String>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let agents = commands::agents::list_agents_inner(&conn)
        .map_err(|e| format!("查询 Agent 列表失败: {}", e))?;
    Ok(agent::detect_installed_agents(&agents))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let pool = init_db().expect("Failed to initialize database");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(DbState { pool })
        .manage(AsyncMutex::new(AgentManager::new()))
        .manage(Mutex::new(plugin::PluginHost::new()))
        .invoke_handler(tauri::generate_handler![
            commands::env::detect_env,
            commands::env::clear_env_detect_cache,
            commands::env::install_agent,
            commands::env::uninstall_agent,
            commands::install_log::insert_log,
            commands::install_log::list_logs,
            commands::install_log::clear_logs,
            commands::update::check_pilotdesk_update,
            commands::update::check_agent_update,
            commands::session::list_sessions,
            commands::session::list_archived_sessions,
            commands::session::create_session,
            commands::session::get_session,
            commands::session::get_session_messages,
            commands::session::rename_session,
            commands::session::archive_session,
            commands::session::delete_session,
            commands::session::save_message,
            commands::session::update_message,
            commands::session::update_session_agent_id,
            commands::session::search_sessions,
            commands::session::search_messages,
            commands::env::ensure_dir,
            list_inspirations,
            get_inspiration,
            create_inspiration,
            update_inspiration,
            delete_inspiration,
            search_inspirations,
            list_tags,
            list_api_providers,
            get_api_provider,
            upsert_api_provider,
            delete_api_provider,
            get_api_key,
            reorder_api_providers,
            get_app_setting,
            set_app_setting,
            get_theme,
            set_theme_cmd,
            agent_send_message_with_config,
            agent_stop_generation,
            agent_create_session,
            agent_close_session,
            agent_list_skills,
            agent_detect_installed,
            plugin::plugin_discover,
            plugin::plugin_list,
            plugin::plugin_enable,
            plugin::plugin_disable,
            plugin::plugin_get_sandbox_info,
            plugin::plugin_install_zip,
            plugin::plugin_uninstall,
    plugin::plugin_set_sandbox_enabled,
    plugin::plugin_get_panel_content,
    plugin::plugin_read_entry,
    plugin::plugin_read_icon_file,
            commands::agents::list_agents,
            commands::agents::get_agent,
            commands::agents::add_agent,
            commands::agents::update_agent,
            commands::agents::delete_agent,
            commands::agents::export_agents_json,
            commands::agents::import_agents_json,
            commands::agents::list_agent_market,
        ])
        .setup(|app| {
            log::info!("PilotDesk initialized successfully (AgentManager mode, r2d2 pool).");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
