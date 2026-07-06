//! 虚拟控制台 Tauri 命令

use std::io;
use tauri::State;
use tokio::sync::Mutex;

use crate::virtual_console::factory::ConsoleFactory;
use crate::virtual_console::traits::VirtualConsole;
use crate::virtual_console::events::EventSystem;

/// 虚拟控制台状态（由 Tauri 管理）
pub struct VirtualConsoleState {
    pub console: Mutex<Option<Box<dyn VirtualConsole>>>,
    pub event_system: Mutex<Option<EventSystem>>,
    pub sessions: Mutex<std::collections::HashMap<String, SessionInfoData>>,
}

impl Default for VirtualConsoleState {
    fn default() -> Self {
        Self {
            console: Mutex::new(None),
            event_system: Mutex::new(None),
            sessions: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[tauri::command]
pub async fn connect_virtual_console(
    state: State<'_, VirtualConsoleState>,
) -> Result<(), String> {
    let mut console_guard = state.console.lock().await;
    let console = ConsoleFactory::auto_create().map_err(|e| format!("创建虚拟控制台失败: {}", e))?;
    *console_guard = Some(console);

    let event_system = EventSystem::new();
    *state.event_system.lock().await = Some(event_system);

    log::info!("[VirtualConsole] Connected");
    Ok(())
}

#[tauri::command]
pub async fn disconnect_virtual_console(
    state: State<'_, VirtualConsoleState>,
) -> Result<(), String> {
    let mut console_guard = state.console.lock().await;
    if let Some(mut console) = console_guard.take() {
        let _ = console.close();
    }
    *state.event_system.lock().await = None;
    state.sessions.lock().await.clear();

    log::info!("[VirtualConsole] Disconnected");
    Ok(())
}

#[tauri::command]
pub async fn spawn_session(
    state: State<'_, VirtualConsoleState>,
    session_id: String,
    command: String,
    args: Vec<String>,
    working_dir: String,
) -> Result<u32, String> {
    let mut console_guard = state.console.lock().await;

    if let Some(ref mut console) = *console_guard {
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let handle = console.spawn(&command, &args_refs, &working_dir)
            .map_err(|e| format!("启动进程失败: {}", e))?;

        let session_data = SessionInfoData {
            session_id: session_id.clone(),
            console_type: "auto".to_string(),
            command: command.clone(),
            args,
            cwd: working_dir.clone(),
            status: "running".to_string(),
        };
        state.sessions.lock().await.insert(session_id.clone(), session_data);

        log::info!("[VirtualConsole] Session '{}' spawned: pid={}", session_id, handle.pid);
        Ok(handle.pid)
    } else {
        Err("Virtual console not connected".to_string())
    }
}

#[tauri::command]
pub async fn send_session_input(
    state: State<'_, VirtualConsoleState>,
    _session_id: String,
    input: String,
) -> Result<(), String> {
    let mut console_guard = state.console.lock().await;
    if let Some(ref mut console) = *console_guard {
        console.write(input.as_bytes())
            .map_err(|e| format!("写入失败: {}", e))?;
        Ok(())
    } else {
        Err("Virtual console not connected".to_string())
    }
}

#[tauri::command]
pub async fn get_session_output(
    state: State<'_, VirtualConsoleState>,
    _session_id: String,
) -> Result<String, String> {
    let mut console_guard = state.console.lock().await;
    if let Some(ref mut console) = *console_guard {
        let output = console.read()
            .map_err(|e| format!("读取失败: {}", e))?;
        Ok(String::from_utf8_lossy(&output).to_string())
    } else {
        Err("Virtual console not connected".to_string())
    }
}

#[tauri::command]
pub async fn terminate_session(
    state: State<'_, VirtualConsoleState>,
    session_id: String,
) -> Result<(), String> {
    let mut console_guard = state.console.lock().await;
    let mut sessions_guard = state.sessions.lock().await;

    if let Some(ref mut console) = *console_guard {
        console.terminate().map_err(|e| format!("终止失败: {}", e))?;
        if let Some(session) = sessions_guard.get_mut(&session_id) {
            session.status = "terminated".to_string();
        }
        Ok(())
    } else {
        Err("Virtual console not connected".to_string())
    }
}

#[tauri::command]
pub async fn remove_session(
    state: State<'_, VirtualConsoleState>,
    session_id: String,
) -> Result<(), String> {
    state.sessions.lock().await.remove(&session_id);
    Ok(())
}

#[tauri::command]
pub async fn list_virtual_console_sessions(
    state: State<'_, VirtualConsoleState>,
) -> Result<Vec<SessionInfoData>, String> {
    let sessions = state.sessions.lock().await;
    Ok(sessions.values().cloned().collect())
}

#[tauri::command]
pub async fn get_session_info(
    state: State<'_, VirtualConsoleState>,
    session_id: String,
) -> Result<Option<SessionInfoData>, String> {
    let sessions = state.sessions.lock().await;
    Ok(sessions.get(&session_id).cloned())
}

#[tauri::command]
pub async fn get_console_status(
    state: State<'_, VirtualConsoleState>,
) -> Result<ConsoleStatus, String> {
    let console_guard = state.console.lock().await;
    let connected = console_guard.is_some();
    let running = state.sessions.lock().await.len();

    Ok(ConsoleStatus {
        connected,
        running_sessions: running,
        console_type: if connected { "auto".to_string() } else { "none".to_string() },
    })
}

/// 会话数据（可序列化，用于 Tauri 命令返回）
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionInfoData {
    pub session_id: String,
    pub console_type: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub status: String,
}

/// 控制台状态（可序列化）
#[derive(Clone, serde::Serialize)]
pub struct ConsoleStatus {
    pub connected: bool,
    pub running_sessions: usize,
    pub console_type: String,
}
