mod agent_config;
mod commands;
mod db;
mod sidecar;
mod utils;

use tauri::Manager;
use db::init::init_db;
use std::sync::Mutex;
use rusqlite::Connection;
use commands::bot as bot_cmds;
use sidecar::manager::SidecarManager;

pub struct DbState {
    pub conn: Mutex<Connection>,
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! Welcome to PilotDesk.", name)
}

// --- Inspiration commands ---
#[tauri::command]
fn list_inspirations(conn: tauri::State<'_, DbState>, tag: Option<String>, favorite_only: Option<bool>) -> Result<Vec<commands::inspiration::Inspiration>, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    commands::inspiration::list_inspirations(&conn, tag, favorite_only.unwrap_or(false))
}

#[tauri::command]
fn get_inspiration(conn: tauri::State<'_, DbState>, id: String) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    commands::inspiration::get_inspiration(&conn, id)
}

#[tauri::command]
fn create_inspiration(conn: tauri::State<'_, DbState>, payload: commands::inspiration::CreateInspirationPayload) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    commands::inspiration::create_inspiration(&conn, payload)
}

#[tauri::command]
fn update_inspiration(conn: tauri::State<'_, DbState>, payload: commands::inspiration::UpdateInspirationPayload) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    commands::inspiration::update_inspiration(&conn, payload)
}

#[tauri::command]
fn delete_inspiration(conn: tauri::State<'_, DbState>, id: String) -> Result<(), crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    commands::inspiration::delete_inspiration(&conn, id)
}

#[tauri::command]
fn search_inspirations(conn: tauri::State<'_, DbState>, query: String, limit: Option<u32>) -> Result<Vec<commands::inspiration::Inspiration>, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    commands::inspiration::search_inspirations(&conn, query, limit.unwrap_or(50))
}

#[tauri::command]
fn list_tags(conn: tauri::State<'_, DbState>) -> Result<Vec<String>, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    commands::inspiration::list_tags(&conn)
}

// --- Bot channel commands ---
#[tauri::command]
fn list_bot_channels(conn: tauri::State<'_, DbState>) -> Result<Vec<bot_cmds::BotChannel>, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    bot_cmds::list_bot_channels(&conn)
}

#[tauri::command]
fn save_bot_channel(conn: tauri::State<'_, DbState>, payload: bot_cmds::SaveBotChannelPayload) -> Result<bot_cmds::BotChannel, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    bot_cmds::save_bot_channel(&conn, payload)
}

#[tauri::command]
fn delete_bot_channel(conn: tauri::State<'_, DbState>, id: String) -> Result<(), crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError { code: "ERR_LOCK".into(), message: "数据库锁获取失败".into(), details: Some(e.to_string()) })?;
    bot_cmds::delete_bot_channel(&conn, id)
}

// --- Theme commands ---
#[tauri::command]
fn get_theme() -> String {
    commands::theme::get_theme()
}

#[tauri::command]
fn set_theme_cmd(theme: String) -> Result<String, crate::utils::errors::AppError> {
    commands::theme::set_theme(theme)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let conn = init_db().expect("Failed to initialize database");
    
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(DbState { conn: Mutex::new(conn) })
        .invoke_handler(tauri::generate_handler![
            greet,
            commands::env::detect_env,
            commands::env::install_claude_code,
            commands::env::install_hermes,
            commands::session::list_sessions,
            commands::session::list_archived_sessions,
            commands::session::create_session,
            commands::session::get_session,
            commands::session::get_session_messages,
            commands::session::rename_session,
            commands::session::archive_session,
            commands::session::delete_session,
            commands::session::save_message,
            commands::config::get_config,
            commands::config::save_claude_config,
            commands::config::save_hermes_config,
            commands::config::test_api_connection,
            list_inspirations,
            get_inspiration,
            create_inspiration,
            update_inspiration,
            delete_inspiration,
            search_inspirations,
            list_tags,
            list_bot_channels,
            save_bot_channel,
            delete_bot_channel,
            get_theme,
            set_theme_cmd,
        ])
        .setup(|app| {
            // Start sidecar WebSocket server
            let app_handle = app.handle().clone();
            let mut sidecar = SidecarManager::new(19830);
            match sidecar.start(app_handle) {
                Ok(port) => println!("[Sidecar] WebSocket server started on port {}", port),
                Err(e) => println!("[Sidecar] Failed to start: {} — WebSocket features will be unavailable", e),
            }
            // Store sidecar as managed state so it gets dropped on app exit
            app.manage(std::sync::Mutex::new(sidecar));

            println!("PilotDesk initialized successfully.");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
