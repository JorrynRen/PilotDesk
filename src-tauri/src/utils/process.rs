//! 进程存活检测工具
//! 提供跨平台的进程存活性检查，用于智能超时机制：
//! 超时后检查子进程是否仍然存活，若存活则视为"假超时"继续等待。

#[cfg(target_os = "windows")]
pub fn is_process_alive(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let wait_result = WaitForSingleObject(handle, 0);
        windows_sys::Win32::Foundation::CloseHandle(handle);
        wait_result == 258 // WAIT_TIMEOUT = still running
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

use std::time::Duration;

// ── Unified Smart Timeout ──────────────────────────────
//  Strategy is fixed here, not configurable by callers.
//  is_process_alive() distinguishes "still working" from "process died".
//
//  Sync (wait_channel_with_alive) — commands are fast, threads are scarce
//    check_interval: 30s,  max_wait: 2min
//  Async (wait_future_with_alive) — LLM inference is slow, coroutines are cheap
//    check_interval: 30s,  max_wait: 10min

const SYNC_CHECK_INTERVAL_MS: u64 = 30_000;   // sync polling interval
const SYNC_MAX_WAIT_MS: u64 = 120_000;         // sync upper limit (2 min)
const ASYNC_CHECK_INTERVAL_MS: u64 = 30_000;   // async polling interval
const ASYNC_MAX_WAIT_MS: u64 = 600_000;        // async upper limit (10 min)

/// Smart timeout error types
#[derive(Debug)]
pub enum SmartTimeoutError {
    ProcessDead(String),
    StillAlive(String),
    ChannelDisconnected(String),
}

impl std::fmt::Display for SmartTimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SmartTimeoutError::ProcessDead(msg) => write!(f, "{}", msg),
            SmartTimeoutError::StillAlive(msg) => write!(f, "{}", msg),
            SmartTimeoutError::ChannelDisconnected(msg) => write!(f, "{}", msg),
        }
    }
}

/// Wait for a channel message with smart timeout detection (sync).
/// Used by execute_command and plugin/shell.
/// The channel can carry any message type T — the caller matches on the result.
pub fn wait_channel_with_alive<T>(
    pid: u32,
    rx: &std::sync::mpsc::Receiver<T>,
    operation_name: &str,
) -> Result<T, SmartTimeoutError> {
    let mut remaining_ms = SYNC_MAX_WAIT_MS;

    loop {
        let wait_ms = SYNC_CHECK_INTERVAL_MS.min(remaining_ms);
        match rx.recv_timeout(Duration::from_millis(wait_ms)) {
            Ok(msg) => return Ok(msg),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                remaining_ms = remaining_ms.saturating_sub(wait_ms);
                if pid > 0 && is_process_alive(pid) {
                    if remaining_ms == 0 {
                        return Err(SmartTimeoutError::StillAlive(
                            format!("{} 超过最大等待时间 ({:.0}s)，进程仍存活",
                                operation_name, SYNC_MAX_WAIT_MS as f64 / 1000.0)
                        ));
                    }
                    continue;
                } else {
                    return Err(SmartTimeoutError::ProcessDead(
                        format!("{} 超时 ({:.0}s)，进程已退出",
                            operation_name, (SYNC_MAX_WAIT_MS - remaining_ms) as f64 / 1000.0)
                    ));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(SmartTimeoutError::ChannelDisconnected(
                    format!("{} 执行线程异常终止", operation_name)
                ));
            }
        }
    }
}

/// Wait for an async future with smart timeout detection (async).
/// Used by send_message_with_config.
pub async fn wait_future_with_alive<T>(
    pid: u32,
    mut future: std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send>>,
    operation_name: &str,
) -> Result<T, SmartTimeoutError>
where
    T: Send + 'static,
{
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ASYNC_MAX_WAIT_MS);
    let mut elapsed = Duration::ZERO;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            if pid > 0 && is_process_alive(pid) {
                return Err(SmartTimeoutError::StillAlive(
                    format!("{} 超过最大等待时间 ({:.0}s)，进程仍存活",
                        operation_name, ASYNC_MAX_WAIT_MS as f64 / 1000.0)
                ));
            } else {
                return Err(SmartTimeoutError::ProcessDead(
                    format!("{} 超时，进程已退出", operation_name)
                ));
            }
        }

        let check = std::cmp::min(Duration::from_millis(ASYNC_CHECK_INTERVAL_MS), remaining);

        match tokio::time::timeout(check, future.as_mut()).await {
            Ok(Ok(result)) => return Ok(result),
            Ok(Err(e)) => return Err(SmartTimeoutError::ProcessDead(
                format!("{} 进程异常: {}", operation_name, e)
            )),
            Err(_) => {
                elapsed += check;
                if pid > 0 && is_process_alive(pid) {
                    continue;
                } else {
                    return Err(SmartTimeoutError::ProcessDead(
                        format!("{} 超时 ({:.0}s)，进程已退出",
                            operation_name, elapsed.as_secs_f64())
                    ));
                }
            }
        }
    }
}
