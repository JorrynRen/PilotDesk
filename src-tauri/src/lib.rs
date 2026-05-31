mod agent_config;
mod commands;
mod db;
mod sidecar;
mod utils;

use db::init::init_db;
use std::sync::Mutex;
use rusqlite::Connection;
use tauri::Manager;

pub struct DbState {
    pub conn: Mutex<Connection>,
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! Welcome to PilotDesk.", name)
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
        ])
        .setup(|_app| {
            println!("PilotDesk initialized successfully.");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
