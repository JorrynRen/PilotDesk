//  Terminal Tauri Commands
//  ─────────────────────────

use tauri::AppHandle;
use tokio::sync::Mutex as AsyncMutex;

use super::TerminalManager;

/// Create a new terminal session
#[tauri::command]
pub async fn terminal_create(
    app: AppHandle,
    terminal_mgr: tauri::State<'_, AsyncMutex<TerminalManager>>,
    session_id: Option<String>,
    shell_type: Option<String>,
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<serde_json::Value, String> {
    let id = session_id.unwrap_or_else(|| format!("term-{}", uuid::Uuid::new_v4().to_string()[..8].to_string()));
    let shell = shell_type.unwrap_or_else(|| {
        "cmd".to_string()
    });
    let cwd_dir = cwd.unwrap_or_else(|| {
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "C:\\".to_string())
    });

    let mut mgr = terminal_mgr.lock().await;
    mgr.create_session(id.clone(), &shell, &cwd_dir, app, cols, rows).await?;

    Ok(serde_json::json!({
        "session_id": id,
        "shell_type": shell,
    }))
}

/// Write data to a terminal session (user input)
#[tauri::command]
pub async fn terminal_write(
    terminal_mgr: tauri::State<'_, AsyncMutex<TerminalManager>>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let mgr = terminal_mgr.lock().await;
    mgr.write(&session_id, &data)
}

/// Resize a terminal session
#[tauri::command]
pub async fn terminal_resize(
    terminal_mgr: tauri::State<'_, AsyncMutex<TerminalManager>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let mgr = terminal_mgr.lock().await;
    mgr.resize(&session_id, cols, rows)
}

/// Close a terminal session
#[tauri::command]
pub async fn terminal_close(
    terminal_mgr: tauri::State<'_, AsyncMutex<TerminalManager>>,
    session_id: String,
) -> Result<(), String> {
    let mut mgr = terminal_mgr.lock().await;
    mgr.close_session(&session_id)
}


/// Start reading stdout for a terminal session (call after frontend registers listeners)
#[tauri::command]
pub async fn terminal_attach(
    app: AppHandle,
    terminal_mgr: tauri::State<'_, AsyncMutex<TerminalManager>>,
    session_id: String,
) -> Result<(), String> {
    let mut mgr = terminal_mgr.lock().await;
    mgr.attach(&session_id, &app)
}

/// List all active terminal sessions
#[tauri::command]
pub async fn terminal_list(
    terminal_mgr: tauri::State<'_, AsyncMutex<TerminalManager>>,
) -> Result<Vec<super::TerminalSessionInfo>, String> {
    let mgr = terminal_mgr.lock().await;
    Ok(mgr.list_sessions())
}

/// 操作系统检测（原 virtual_console::config::detect_os_type 迁移而来）
///
/// 返回与前端 console_type 约定一致的字符串（serde 枚举名）。
fn detect_os_type() -> &'static str {
    #[cfg(target_os = "windows")]
    return "Windows";

    #[cfg(target_os = "macos")]
    return "MacOS";

    #[cfg(target_os = "linux")]
    return "Linux";

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    panic!("Unsupported operating system");
}

/// Get console configuration info for status bar
#[tauri::command]
pub async fn terminal_get_config() -> Result<serde_json::Value, String> {
    // 原 ConsoleConfig::default() 的常量值（virtual_console 模块已删除，直接内联）
    Ok(serde_json::json!({
        "console_type": detect_os_type(),
        "buffer_size": 1024 * 1024,
        "timeout_ms": 30000,
        "event_driven": true,
        "output_encoding": "utf-8",
        "merge_output": false,
        "max_lines": 10000,
    }))
}
