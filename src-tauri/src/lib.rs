mod config;
mod db;
mod sidecar_manager;

use std::sync::Mutex;
use tauri::Manager;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! Welcome to PilotDesk.", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![greet])
        .setup(|app| {
            // Initialize database
            let app_dir = app.path().app_data_dir().expect("Failed to get app data dir");
            std::fs::create_dir_all(&app_dir).expect("Failed to create app data dir");
            
            let db_path = app_dir.join("pilotdesk.db");
            let conn = db::init_db(&db_path).expect("Failed to initialize database");
            app.manage(db::AppState { conn: Mutex::new(conn) });
            
            println!("PilotDesk initialized successfully.");
            println!("App data directory: {}", app_dir.display());
            
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
