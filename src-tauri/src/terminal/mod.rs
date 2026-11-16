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
#[cfg(target_os = "windows")]
use std::collections::HashSet;
#[cfg(target_os = "windows")]
use std::sync::Mutex;
use std::sync::OnceLock;
use tauri::Emitter;

use crate::terminal::pty::{StdoutStream, TerminalProcess};
use crate::utils::process::hidden_command;

#[cfg(target_os = "windows")]
static LIVE_TERMINAL_PIDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn track_terminal_process(pid: u32) {
    if pid == 0 {
        return;
    }
    match LIVE_TERMINAL_PIDS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        Ok(mut pids) => {
            pids.insert(pid);
        }
        Err(_) => log::warn!("[terminal] 无法登记终端进程 {}", pid),
    }
}

#[cfg(target_os = "windows")]
fn untrack_terminal_process(pid: u32) {
    if pid == 0 {
        return;
    }
    if let Ok(mut pids) = LIVE_TERMINAL_PIDS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        pids.remove(&pid);
    }
}

#[cfg(not(target_os = "windows"))]
fn track_terminal_process(_pid: u32) {}

#[cfg(not(target_os = "windows"))]
fn untrack_terminal_process(_pid: u32) {}

/// Last-resort shutdown cleanup for Windows. The async manager lock can still
/// be held by an in-flight command when the app exits, so tracked process trees
/// are terminated independently of the manager.
pub fn terminate_registered_processes() {
    #[cfg(target_os = "windows")]
    {
        let pids = match LIVE_TERMINAL_PIDS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            Ok(mut pids) => pids.drain().collect::<Vec<_>>(),
            Err(_) => {
                log::warn!("[terminal] 退出清理无法读取终端进程登记表");
                return;
            }
        };

        for pid in pids {
            let _ = hidden_command("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

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

fn pwsh_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        hidden_command("pwsh")
            .args(["-NoLogo", "-NoProfile", "-Command", "exit 0"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
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
        initial_cols: Option<u16>,
        initial_rows: Option<u16>,
    ) -> Result<u32, String> {
        if self.sessions.contains_key(&id) {
            return Err(format!("terminal session {} already exists", id));
        }

        let cmdline = match shell_type {
            "powershell" | "pwsh" => {
                if pwsh_available() {
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
        track_terminal_process(pid);

        Ok(pid)
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

        let pid = session.process.id();
        session.process.kill();
        untrack_terminal_process(pid);

        Ok(())
    }

    /// Terminate every active session. Used when the application exits, where
    /// no frontend command will get a chance to close tabs one by one.
    pub fn close_all(&mut self) {
        let sessions = std::mem::take(&mut self.sessions);
        let count = sessions.len();
        for (_, mut session) in sessions {
            let pid = session.process.id();
            session.process.kill();
            untrack_terminal_process(pid);
        }
        if count > 0 {
            log::info!("[terminal] 应用退出：关闭 {} 个终端会话", count);
        }
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

/// Preserves incomplete UTF-8 sequences when PTY output is split across reads.
#[derive(Default)]
struct Utf8ChunkDecoder {
    pending: Vec<u8>,
}

impl Utf8ChunkDecoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        let mut consumed = 0;

        while consumed < self.pending.len() {
            match std::str::from_utf8(&self.pending[consumed..]) {
                Ok(valid) => {
                    output.push_str(valid);
                    consumed = self.pending.len();
                }
                Err(error) => {
                    let valid_end = consumed + error.valid_up_to();
                    if valid_end > consumed {
                        output.push_str(
                            std::str::from_utf8(&self.pending[consumed..valid_end])
                                .expect("valid UTF-8 prefix"),
                        );
                        consumed = valid_end;
                    }
                    match error.error_len() {
                        Some(length) => {
                            output.push(char::REPLACEMENT_CHARACTER);
                            consumed += length;
                        }
                        None => break,
                    }
                }
            }
        }

        self.pending.drain(..consumed);
        output
    }

    fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            return String::new();
        }
        let pending = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&pending).into_owned()
    }
}

/// Background read loop: read from ConPTY stdout and push to frontend
#[cfg(target_os = "windows")]
async fn terminal_read_loop(
    mut stdout: tokio::fs::File,
    session_id: &str,
    app_handle: &tauri::AppHandle,
    pid: u32,
) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 4096];
    let event_name = format!("terminal://output/{}", session_id);
    let mut decoder = Utf8ChunkDecoder::default();

    loop {
        match stdout.read(&mut buf).await {
            Ok(0) => {
                untrack_terminal_process(pid);
                let tail = decoder.finish();
                if !tail.is_empty() {
                    let _ = app_handle.emit(&event_name, tail);
                }
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
                let data = decoder.push(&buf[..n]);
                if !data.is_empty() {
                    let _ = app_handle.emit(&event_name, data);
                }
            }
            Err(e) => {
                untrack_terminal_process(pid);
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
    let mut decoder = Utf8ChunkDecoder::default();

    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                untrack_terminal_process(pid);
                let tail = decoder.finish();
                if !tail.is_empty() {
                    let _ = app_handle.emit(&event_name, tail);
                }
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
                let data = decoder.push(&buf[..n]);
                if !data.is_empty() {
                    let _ = app_handle.emit(&event_name, data);
                }
            }
            Err(e) => {
                untrack_terminal_process(pid);
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

#[cfg(test)]
mod tests {
    use super::Utf8ChunkDecoder;

    #[test]
    fn utf8_decoder_preserves_split_multibyte_characters() {
        let bytes = "中文输出".as_bytes();
        let mut decoder = Utf8ChunkDecoder::default();

        assert_eq!(decoder.push(&bytes[..1]), "");
        assert_eq!(decoder.push(&bytes[1..4]), "中");
        assert_eq!(decoder.push(&bytes[4..]), "文输出");
    }

    #[test]
    fn utf8_decoder_replaces_invalid_sequences_without_losing_following_text() {
        let mut decoder = Utf8ChunkDecoder::default();
        assert_eq!(decoder.push(&[0xff, b'o', b'k']), "\u{fffd}ok");
    }

    #[test]
    fn utf8_decoder_flushes_incomplete_tail_at_eof() {
        let mut decoder = Utf8ChunkDecoder::default();
        assert_eq!(decoder.push(&"中".as_bytes()[..2]), "");
        assert_eq!(decoder.finish(), "\u{fffd}");
        assert_eq!(decoder.finish(), "");
    }
}
