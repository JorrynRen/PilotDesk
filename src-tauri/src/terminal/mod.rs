//  Terminal Module — Persistent Shell Terminal (PTY + xterm.js)
//  ──────────────────────────────────────────────────────
//  Provides real terminal experience: user types any command, PTY provides
//  full console environment, stdout/stderr streamed to frontend xterm.js
//  via Tauri events.
//  Windows 使用 ConPTY（conpty_process.rs），Linux/macOS 使用 portable-pty（unix_pty.rs），
//  经 pty.rs 统一抽象分发。

pub mod commands;
pub mod conpty_process;
pub mod console_bridge;
pub mod pty;
pub mod unix_pty;

use std::collections::HashMap;
use tauri::Emitter;

use crate::terminal::pty::{StdoutStream, TerminalProcess};

/// Single terminal session state
pub struct TerminalSession {
    pub id: String,
    pub shell_type: String,
    pub process: TerminalProcess,
    /// stdout stream (moved out when read_loop starts via terminal_attach)
    pub stdout: Option<StdoutStream>,
}

/// Terminal manager: manages all active terminal sessions
pub struct TerminalManager {
    sessions: HashMap<String, TerminalSession>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Create a new terminal session (launch persistent shell)
    pub async fn create_session(
        &mut self,
        id: String,
        shell_type: &str,
        cwd: &str,
        app_handle: tauri::AppHandle,
        initial_cols: Option<u16>,
        initial_rows: Option<u16>,
    ) -> Result<(), String> {
        if self.sessions.contains_key(&id) {
            return Err(format!("terminal session {} already exists", id));
        }

        let cmdline = match shell_type {
            "powershell" | "pwsh" => {
                if std::process::Command::new("pwsh")
                    .arg("-Version")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .is_ok()
                {
                    "pwsh -NoProfile".to_string()
                } else {
                    "powershell -NoProfile".to_string()
                }
            }
            _ => "cmd /K".to_string(),
        };

        let cmdline_owned = cmdline.clone();
        let cwd_owned = cwd.to_string();
        let init_cols = initial_cols.unwrap_or(80);
        let init_rows = initial_rows.unwrap_or(30);
        let (process, stdout) = tokio::task::spawn_blocking(move || {
            TerminalProcess::spawn(&cmdline_owned, &cwd_owned, init_cols, init_rows)
        })
        .await
        .map_err(|e| format!("Spawn blocking failed: {}", e))?
        .map_err(|e| format!("Failed to start terminal: {}", e))?;

        let pid = process.id();

        let session = TerminalSession {
            id: id.clone(),
            shell_type: shell_type.to_string(),
            process,
            stdout: Some(stdout),
        };
        self.sessions.insert(id.clone(), session);

        // Notify frontend that terminal is created
        let _ = app_handle.emit(
            "terminal://created",
            serde_json::json!({
                "session_id": id,
                "shell_type": shell_type,
                "pid": pid,
            }),
        );

        Ok(())
    }

    /// Write data to terminal session (user keyboard input)
    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let session = self
            .sessions
            .get(id)
            .ok_or_else(|| format!("terminal session {} not found", id))?;

        session.process.write(data)
    }

    /// Resize terminal session (sync PTY window size)
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let session = self
            .sessions
            .get(id)
            .ok_or_else(|| format!("terminal session {} not found", id))?;
        session.process.resize(cols, rows)
    }

    /// Close terminal session
    pub fn close_session(&mut self, id: &str) -> Result<(), String> {
        let mut session = self
            .sessions
            .remove(id)
            .ok_or_else(|| format!("terminal session {} not found", id))?;

        session.process.kill();
        session.process.close_stdout_pipe();

        Ok(())
    }

    /// Start reading stdout for a terminal session (call after frontend registers listeners)
    pub fn attach(&mut self, id: &str, app_handle: &tauri::AppHandle) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(id)
            .ok_or_else(|| format!("terminal session {} not found", id))?;

        let stdout = session.stdout.take().ok_or_else(|| {
            format!(
                "terminal session {} already attached or stdout unavailable",
                id
            )
        })?;

        let pid = session.process.id();
        let session_id = id.to_string();
        let app = app_handle.clone();

        match stdout {
            #[cfg(target_os = "windows")]
            StdoutStream::Windows(file) => {
                tauri::async_runtime::spawn(async move {
                    terminal_read_loop(file, &session_id, &app, pid).await;
                });
            }
            #[cfg(not(target_os = "windows"))]
            StdoutStream::Unix(reader) => {
                tauri::async_runtime::spawn(async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        terminal_unix_read_loop(reader, &session_id, &app, pid);
                    })
                    .await;
                });
            }
        }

        Ok(())
    }

    /// List all active terminal sessions
    pub fn list_sessions(&self) -> Vec<TerminalSessionInfo> {
        self.sessions
            .values()
            .map(|s| TerminalSessionInfo {
                id: s.id.clone(),
                shell_type: s.shell_type.clone(),
                pid: s.process.id(),
            })
            .collect()
    }
}

/// Terminal session info (visible to frontend)
#[derive(serde::Serialize, Clone)]
pub struct TerminalSessionInfo {
    pub id: String,
    pub shell_type: String,
    pub pid: u32,
}

/// Background read loop: read from ConPTY stdout and push to frontend
async fn terminal_read_loop(
    mut stdout: tokio::fs::File,
    session_id: &str,
    app_handle: &tauri::AppHandle,
    pid: u32,
) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 4096];
    let event_name = format!("terminal://output/{}", session_id);

    loop {
        match stdout.read(&mut buf).await {
            Ok(0) => {
                let _ = app_handle.emit(
                    "terminal://exited",
                    serde_json::json!({
                        "session_id": session_id,
                        "pid": pid,
                    }),
                );
                break;
            }
            Ok(n) => {
                let data = String::from_utf8_lossy(&buf[..n]).to_string();
                let _ = app_handle.emit(&event_name, data);
            }
            Err(e) => {
                let _ = app_handle.emit(
                    "terminal://exited",
                    serde_json::json!({
                        "session_id": session_id,
                        "pid": pid,
                        "error": format!("{}", e),
                    }),
                );
                break;
            }
        }
    }
}

/// Unix 终端读取循环：同步读 portable-pty master reader，推送前端。
/// 运行在 spawn_blocking 中（master reader 是同步 Read）。
#[cfg(not(target_os = "windows"))]
fn terminal_unix_read_loop(
    mut reader: Box<dyn std::io::Read + Send>,
    session_id: &str,
    app_handle: &tauri::AppHandle,
    pid: u32,
) {
    use std::io::Read;
    let event_name = format!("terminal://output/{}", session_id);
    let mut buf = [0u8; 4096];

    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                let _ = app_handle.emit(
                    "terminal://exited",
                    serde_json::json!({
                        "session_id": session_id,
                        "pid": pid,
                    }),
                );
                break;
            }
            Ok(n) => {
                let data = String::from_utf8_lossy(&buf[..n]).to_string();
                let _ = app_handle.emit(&event_name, data);
            }
            Err(e) => {
                let _ = app_handle.emit(
                    "terminal://exited",
                    serde_json::json!({
                        "session_id": session_id,
                        "pid": pid,
                        "error": format!("{}", e),
                    }),
                );
                break;
            }
        }
    }
}
