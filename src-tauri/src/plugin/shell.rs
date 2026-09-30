//! 插件 Shell 执行模块
//!
//! 提供插件 Shell 命令执行能力。
//! 命令通过 tokio::process::Command 执行（原 virtual_console 同步路径已随模块删除）。

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

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

#[tauri::command]
pub async fn plugin_shell_exec(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    command: String,
    options: Option<ShellExecOptions>,
) -> Result<ShellResult, String> {
    // 校验权限与沙箱（块作用域内持有 guard，块结束即释放，避免 !Send 的 MutexGuard 跨 await）
    //
    // 先按 manifest 声明的 shell:exec 判定（与沙箱解耦：关闭沙箱不再等于授予权限），
    // 再叠加沙箱开关作为额外的用户确认层。
    {
        let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;

        host.require_permission(&plugin_id, "shell:exec")?;

        if host.sandbox_enabled() {
            return Err("沙箱已启用，Shell 命令执行被拒绝（需先在设置中禁用沙箱）".to_string());
        }
    }

    let work_dir = options
        .as_ref()
        .and_then(|o| o.working_dir.clone())
        .unwrap_or_else(|| {
            // 默认使用应用根目录（配置与数据同根）作为工作区
            crate::utils::paths::app_root_dir()
                .to_string_lossy()
                .to_string()
        });

    let timeout_ms = options.as_ref().and_then(|o| o.timeout_ms).unwrap_or(30000);

    // Windows: cmd /C 包装；其余平台：sh -c。
    // Windows 下必须把命令行"原样"交给 cmd.exe：argv 传递会为含引号的参数插入 `\"` 转义，
    // 而 cmd.exe 不识别该转义（含引号的命令会被误解析为以 `\\` 开头的 UNC 路径）。
    #[cfg(target_os = "windows")]
    let exe = "cmd";
    #[cfg(not(target_os = "windows"))]
    let exe = "sh";

    log::info!("[Plugin/Shell] exec cmd='{}' cwd='{}'", command, work_dir);

    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let mut cmd = tokio::process::Command::new(exe);
        #[cfg(target_os = "windows")]
        {
            cmd.raw_arg(format!("/C {}", command));
        }
        #[cfg(not(target_os = "windows"))]
        {
            cmd.args(["-c", &command]);
        }
        cmd.current_dir(&work_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env_remove("PYTHONHOME")
            .kill_on_drop(true);
        cmd.output().await
    })
    .await;

    match result {
        Ok(Ok(output)) => {
            let exit_code = output.status.code().unwrap_or(-1);
            log::info!("[Plugin/Shell] command completed: exit_code={}", exit_code);
            Ok(ShellResult {
                stdout: crate::decode_windows_output(&output.stdout)
                    .trim()
                    .to_string(),
                stderr: crate::decode_windows_output(&output.stderr)
                    .trim()
                    .to_string(),
                exit_code,
            })
        }
        Ok(Err(e)) => Err(format!("启动进程失败: {}", e)),
        Err(_) => Err(format!(
            "plugin_shell: 超时 ({:.1}s)，命令: {}",
            timeout_ms as f64 / 1000.0,
            // 命令里可能含中文参数，必须按字符截断（原先的 &command[..100] 会切进字符内部 panic）
            crate::utils::text::elide_head(&command, 100)
        )),
    }
}
