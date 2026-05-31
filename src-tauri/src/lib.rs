mod agent_config;
mod commands;
mod db;
mod sidecar;
mod utils;

use db::init::init_db;
use std::sync::Mutex;
use rusqlite::Connection;
use tauri::Manager;
use commands::inspiration;

pub struct DbState {
    pub conn: Mutex<Connection>,
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! Welcome to PilotDesk.", name)
}

#[tauri::command]
fn list_inspirations(conn: tauri::State<'_, DbState>, tag: Option<String>, favorite_only: Option<bool>) -> Result<Vec<inspiration::Inspiration>, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError {
        code: "ERR_LOCK".to_string(),
        message: "数据库锁获取失败".to_string(),
        details: Some(e.to_string()),
    })?;
    inspiration::list_inspirations(&conn, tag, favorite_only.unwrap_or(false))
}

#[tauri::command]
fn get_inspiration(conn: tauri::State<'_, DbState>, id: String) -> Result<inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError {
        code: "ERR_LOCK".to_string(),
        message: "数据库锁获取失败".to_string(),
        details: Some(e.to_string()),
    })?;
    inspiration::get_inspiration(&conn, id)
}

#[tauri::command]
fn create_inspiration(conn: tauri::State<'_, DbState>, payload: inspiration::CreateInspirationPayload) -> Result<inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError {
        code: "ERR_LOCK".to_string(),
        message: "数据库锁获取失败".to_string(),
        details: Some(e.to_string()),
    })?;
    inspiration::create_inspiration(&conn, payload)
}

#[tauri::command]
fn update_inspiration(conn: tauri::State<'_, DbState>, payload: inspiration::UpdateInspirationPayload) -> Result<inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError {
        code: "ERR_LOCK".to_string(),
        message: "数据库锁获取失败".to_string(),
        details: Some(e.to_string()),
    })?;
    inspiration::update_inspiration(&conn, payload)
}

#[tauri::command]
fn delete_inspiration(conn: tauri::State<'_, DbState>, id: String) -> Result<(), crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError {
        code: "ERR_LOCK".to_string(),
        message: "数据库锁获取失败".to_string(),
        details: Some(e.to_string()),
    })?;
    inspiration::delete_inspiration(&conn, id)
}

#[tauri::command]
fn search_inspirations(conn: tauri::State<'_, DbState>, query: String, limit: Option<u32>) -> Result<Vec<inspiration::Inspiration>, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError {
        code: "ERR_LOCK".to_string(),
        message: "数据库锁获取失败".to_string(),
        details: Some(e.to_string()),
    })?;
    inspiration::search_inspirations(&conn, query, limit.unwrap_or(50))
}

#[tauri::command]
fn list_tags(conn: tauri::State<'_, DbState>) -> Result<Vec<String>, crate::utils::errors::AppError> {
    let conn = conn.conn.lock().map_err(|e| crate::utils::errors::AppError {
        code: "ERR_LOCK".to_string(),
        message: "数据库锁获取失败".to_string(),
        details: Some(e.to_string()),
    })?;
    inspiration::list_tags(&conn)
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
        ])
        .setup(|_app| {
            println!("PilotDesk initialized successfully.");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
