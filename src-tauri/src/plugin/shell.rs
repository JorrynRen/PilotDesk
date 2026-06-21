use serde::{Deserialize, Serialize};
use std::sync::Mutex;

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
pub struct ShellExecOptions {
    pub timeout_ms: Option<u64>,
    pub working_dir: Option<String>,
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
    
    // 沙箱启用时拒绝
    if sandbox_info.sandbox_enabled {
        return Err("沙箱已启用，Shell 命令执行被拒绝".to_string());
    }
    
    // 验证插件存在
    let plugins = host.list_plugins();
    let _plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;
    
    // 释放锁后再执行命令，避免死锁
    drop(host);
    
    let timeout = options.as_ref().and_then(|o| o.timeout_ms).unwrap_or(30000);
    
    // 在 Windows 上使用 cmd /c，其他系统使用 sh -c
    let (shell, flag) = if cfg!(target_os = "windows") {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    
    let mut child = std::process::Command::new(shell)
        .arg(flag)
        .arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动进程失败: {}", e))?;
    
    // 等待进程完成（带超时）
    let start = std::time::Instant::now();
    let status = loop {
        if start.elapsed().as_millis() as u64 > timeout {
            let _ = child.kill();
            return Err(format!("命令执行超时 ({}ms)", timeout));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(format!("等待进程失败: {}", e)),
        }
    };
    
    use std::io::Read;
    let mut stdout = String::new();
    let mut stderr = String::new();
    child.stdout.take().unwrap().read_to_string(&mut stdout).ok();
    child.stderr.take().unwrap().read_to_string(&mut stderr).ok();
    
    let exit_code = status.code().unwrap_or(-1);
    
    Ok(ShellResult {
        stdout,
        stderr,
        exit_code,
    })
}
