mod commands;
mod db;
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
        ])
        .setup(|app| {
            println!("PilotDesk initialized successfully.");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
