//! 插件 Shell 执行模块
//!
//! 提供插件 Shell 命令执行能力。
//! 所有命令均通过虚拟控制台（VirtualConsole trait）统一执行，
//! 不使用裸 std::process::Command，确保执行通道的一致性与可追溯性。

use serde::{Deserialize, Serialize};
use std::sync::{Mutex, Arc};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::virtual_console::factory::ConsoleFactory;

use super::PluginHost;

/// Shell 执行结果
#[derive(Debug, Clone, Serialize)]
pub struct ShellResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Shell 执行选项
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ShellExecOptions {
    pub timeout_ms: Option<u64>,
    pub working_dir: Option<String>,
}

/// 插件 Shell 统一虚拟工作台方法
///
/// 通过 VirtualConsole trait 统一执行，返回 (pid, ShellResult)。
fn execute_via_virtual_console(
    command: &str,
    options: &Option<ShellExecOptions>,
) -> Result<(u32, ShellResult), String> {
    let work_dir = options
        .as_ref()
        .and_then(|o| o.working_dir.clone())
        .unwrap_or_else(|| {
            // 默认使用用户数据根目录作为工作区
            crate::utils::paths::app_data_dir()
                .to_string_lossy()
                .to_string()
        });

    let mut console = ConsoleFactory::auto_create()
        .map_err(|e| format!("创建虚拟控制台失败: {}", e))?;

    #[cfg(target_os = "windows")]
    let (exe, args): (&str, &[&str]) = ("cmd", &["/C", command]);
    #[cfg(not(target_os = "windows"))]
    let (exe, args): (&str, &[&str]) = ("sh", &["-c", command]);

    let mut handle = console
        .spawn(exe, args, &work_dir)
        .map_err(|e| format!("虚拟控制台 spawn 失败: {}", e))?;

    let pid = handle.pid;
    log::info!("[Plugin/Shell] virtual console spawned: pid={} cmd='{}' cwd='{}'", pid, command, work_dir);

    // 读取 stdout
    let mut stdout_bytes = Vec::new();
    { let mut buf = [0u8; 4096]; loop {
        match handle.stdout.read(&mut buf) {
            Ok(0) => break, Ok(n) => stdout_bytes.extend_from_slice(&buf[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }}

    // 读取 stderr
    let mut stderr_bytes = Vec::new();
    { let mut buf = [0u8; 4096]; loop {
        match handle.stderr.read(&mut buf) {
            Ok(0) => break, Ok(n) => stderr_bytes.extend_from_slice(&buf[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }}

    let _ = handle.stdin.close();
    let status = console.wait(None).map_err(|e| format!("虚拟控制台 wait 失败: {}", e))?;
    let exit_code = status.code().unwrap_or(-1);

    log::info!("[Plugin/Shell] command completed: pid={} exit_code={}", pid, exit_code);

    Ok((pid, ShellResult {
        stdout: String::from_utf8_lossy(&stdout_bytes).trim().to_string(),
        stderr: String::from_utf8_lossy(&stderr_bytes).trim().to_string(),
        exit_code,
    }))
}

#[tauri::command]
pub fn plugin_shell_exec(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    command: String,
    options: Option<ShellExecOptions>,
) -> Result<ShellResult, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let sandbox_info = host.get_sandbox_info();

    if sandbox_info.sandbox_enabled {
        return Err("沙箱已启用，Shell 命令执行被拒绝".to_string());
    }

    let plugins = host.list_plugins();
    let _plugin = plugins
        .iter()
        .find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    drop(host);

    // 智能超时等待
    let timeout_ms = options.as_ref().and_then(|o| o.timeout_ms).unwrap_or(30000);
    let max_wait = Duration::from_millis(timeout_ms);
    let check_interval = Duration::from_secs(5);

    let shared_pid = Arc::new(AtomicU32::new(0));
    let cmd = command.clone();
    let opts = options.clone();
    let shared_pid_clone = Arc::clone(&shared_pid);
    let (tx, rx) = std::sync::mpsc::channel::<Result<(u32, ShellResult), String>>();

    std::thread::spawn(move || {
        let result = execute_via_virtual_console(&cmd, &opts);
        if let Ok((pid, _)) = &result {
            shared_pid_clone.store(*pid, Ordering::Relaxed);
        }
        let _ = tx.send(result);
    });

    // 智能超时轮询（与 agent execute_sync 一致的模式）
    let start = Instant::now();
    loop {
        match rx.try_recv() {
            Ok(Ok((_, shell_result))) => return Ok(shell_result),
            Ok(Err(e)) => return Err(e),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                let elapsed = start.elapsed();
                if elapsed >= max_wait {
                    return Err(format!(
                        "plugin_shell: 超时 ({:.1}s)，命令: {}",
                        elapsed.as_secs_f64(),
                        &command[..command.len().min(100)]
                    ));
                }
                std::thread::sleep(check_interval);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err("plugin_shell: 执行通道异常断开".to_string());
            }
        }
    }
}
