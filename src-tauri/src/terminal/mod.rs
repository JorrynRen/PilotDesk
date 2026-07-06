//  Terminal Module — Persistent Shell Terminal (ConPTY + xterm.js)
//  ──────────────────────────────────────────────────────
//  Provides real terminal experience: user types any command, ConPTY provides
//  full console environment, stdout/stderr streamed to frontend xterm.js
//  via Tauri events.

pub mod commands;

use std::collections::HashMap;
use tauri::Emitter;

use crate::agent::session_process::{spawn_with_conpty, ConptyProcess};

/// Single terminal session state
pub struct TerminalSession {
    pub id: String,
    pub shell_type: String,
    pub process: ConptyProcess,
    /// stdout pipe (moved out when read_loop starts via terminal_attach)
    pub stdout: Option<tokio::fs::File>,
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
        let (process, stdout, _stderr, _conpty_mode) = tokio::task::spawn_blocking(move || {
            spawn_with_conpty(&cmdline_owned, &cwd_owned, init_cols, init_rows)
        }).await.map_err(|e| format!("Spawn blocking failed: {}", e))?.map_err(|e| {
            format!("Failed to start terminal: {}", e)
        })?;

        let pid = process.id();


        let mut session = TerminalSession {
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

        unsafe {
            use windows_sys::Win32::Storage::FileSystem::WriteFile;
            use windows_sys::Win32::Foundation::GetLastError;

            let handle = session.process.stdin_handle();
            let data_bytes = data.as_bytes();
            let mut bytes_written: u32 = 0;
            let result = WriteFile(
                handle,
                data_bytes.as_ptr() as *const _,
                data_bytes.len() as u32,
                &mut bytes_written,
                std::ptr::null_mut(),
            );

            if result == 0 {
                let err = GetLastError();
                return Err(format!("Failed to write to terminal: system error {}", err));
            }
        }

        Ok(())
    }

    /// Resize terminal session (sync ConPTY window size)
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let session = self.sessions.get(id).ok_or_else(|| {
            format!("terminal session {} not found", id)
        })?;
        session.process.resize(cols, rows)
    }

    /// Close terminal session
    pub fn close_session(&mut self, id: &str) -> Result<(), String> {
        let session = self
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

        let stdout = session.stdout.take()
            .ok_or_else(|| format!("terminal session {} already attached or stdout unavailable", id))?;

        let pid = session.process.id();
        let session_id = id.to_string();
        let app = app_handle.clone();

        tauri::async_runtime::spawn(async move {
            terminal_read_loop(stdout, &session_id, &app, pid).await;
        });

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
    let mut read_count: usize = 0;
    let mut total_bytes: usize = 0;


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
                read_count += 1;
                total_bytes += n;
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
