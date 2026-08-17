//! 进程执行原语（会话模式与群聊参与者共用）。
//!
//! - `run_command`：执行 Windows Shell 命令（cmd.exe /C），30s 超时 + 输出解码 + 32KB 截断
//! - `run_python`：执行 Python 内联代码或 .py 脚本（file/code 二选一）
//!
//! 自 `api_agent/exec.rs`（run_python）与 `lib.rs`（execute_command 内联逻辑）合并迁移
//! （工具架构统一 v1.0，轮 4）。

use std::sync::{Mutex, OnceLock};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Python 环境可用性缓存（全局，首次检测后复用）。
static PY_AVAILABLE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

fn python_available() -> Result<bool, String> {
    let cache = PY_AVAILABLE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap();
    if let Some(v) = *guard {
        return Ok(v);
    }
    let ok = std::process::Command::new("python")
        .args(["--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    *guard = Some(ok);
    Ok(ok)
}

/// 执行 Windows Shell 命令（cmd.exe /C），返回 stdout/stderr 与退出码。
/// 30 秒超时强制终止 + Windows 输出解码 + 32KB head/tail 截断。
pub fn run_command(cwd: &str, command: &str) -> Result<String, String> {
    let mut child = std::process::Command::new("cmd.exe")
        .args(["/C", command])
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW，不弹出 cmd 窗口
        .spawn()
        .map_err(|e| format!("启动命令失败: {}", e))?;

    // ── 30 秒超时轮询（非阻塞，让 tokio 可调度其他任务）──
    let timeout_dur = std::time::Duration::from_secs(30);
    let deadline = std::time::Instant::now() + timeout_dur;
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    log::warn!("[execute_command] 超时，强制终止: {}", command);
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("命令执行超时（已强制终止，30秒限制）".to_string());
                }
                // 短暂休眠，避免忙等消耗 CPU
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                log::error!("[execute_command] 进程异常: {}", e);
                return Err(format!("命令执行异常: {}", e));
            }
        }
    };

    // ── 收集输出 ──
    let output = match exit_status {
        Some(_) => child.wait_with_output().map_err(|e| format!("读取输出失败: {}", e))?,
        None => unreachable!(),
    };

    let stdout = crate::decode_windows_output(&output.stdout).trim().to_string();
    let stderr = crate::decode_windows_output(&output.stderr).trim().to_string();

    // ── 组装结果 ──
    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() { result.push('\n'); }
        result.push_str("[stderr]\n");
        result.push_str(&stderr);
    }

    // ── 智能截断：head + tail 策略（保留开头和结尾，丢弃中间冗余）──
    // 32KB 上限足够覆盖绝大部分命令输出，超出时保留前 16KB + 后 16KB
    const MAX_OUTPUT: usize = 32_768; // 32KB
    if result.len() > MAX_OUTPUT {
        let head_size = MAX_OUTPUT / 2;
        let tail_size = MAX_OUTPUT - head_size;
        // 安全找到字符边界，避免在多字节字符中间截断导致 panic
        let head_end = result
            .char_indices()
            .take_while(|&(i, _)| i < head_size)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        let tail_start = result
            .char_indices()
            .filter(|&(i, _)| i >= result.len() - tail_size)
            .next()
            .map_or(result.len(), |(i, _)| i);
        let head = &result[..head_end];
        let tail = &result[tail_start..];
        result = format!(
            "{}\n\n... (中间 {} 字节已省略，总输出 {} 字节) ...\n\n{}",
            head,
            result.len() - head_end - (result.len() - tail_start),
            result.len(),
            tail
        );
    }

    let exit_code = output.status.code().unwrap_or(-1);

    if result.is_empty() {
        Ok(format!("命令执行完成（退出码: {}），无输出", exit_code))
    } else {
        Ok(format!("退出码: {}\n{}", exit_code, result))
    }
}

/// 执行 Python：`code` 与 `file` 二选一（file 优先），返回 stdout/stderr 与退出码。
pub fn run_python(cwd: &str, code: Option<&str>, file: Option<&str>) -> Result<String, String> {
    if !python_available()? {
        return Err(
            "当前系统未检测到 Python 环境。请安装 Python 后重试。\n\
             安装时请勾选 \"Add Python to PATH\"。\n\
             版本号请根据实际需求选择（建议使用当前最新稳定版）。"
                .to_string(),
        );
    }

    // ── 确定脚本源：优先 file（脚本文件），否则 code（内联代码写临时文件）──
    let (target, tmp_cleanup): (std::path::PathBuf, bool) = match (file, code) {
        (Some(f), _) if !f.trim().is_empty() => {
            let p = std::path::PathBuf::from(f.trim());
            let abs = if p.is_absolute() { p } else { std::path::Path::new(cwd).join(&p) };
            if !abs.exists() {
                return Err(format!("Python 脚本文件不存在: {}", abs.display()));
            }
            (abs, false)
        }
        (_, Some(c)) if !c.trim().is_empty() => {
            let tmp_dir = std::env::temp_dir().join("pilotdesk_py");
            let _ = std::fs::create_dir_all(&tmp_dir);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let tmp_file = tmp_dir.join(format!("script_{}.py", ts));
            std::fs::write(&tmp_file, c)
                .map_err(|e| format!("写入临时脚本失败: {}", e))?;
            (tmp_file, true)
        }
        _ => return Err("execute_python 缺少参数：请提供 file（脚本路径）或 code（内联代码）".to_string()),
    };

    // ── 执行（spawn + 30 秒超时轮询，避免同步阻塞与无限运行）──
    let mut child = std::process::Command::new("python")
        .arg(&target)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|e| {
            if tmp_cleanup { let _ = std::fs::remove_file(&target); }
            format!("启动 Python 失败: {}", e)
        })?;

    let timeout_dur = std::time::Duration::from_secs(30);
    let deadline = std::time::Instant::now() + timeout_dur;
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    log::warn!("[run_python] 超时，强制终止脚本");
                    let _ = child.kill();
                    let _ = child.wait();
                    if tmp_cleanup { let _ = std::fs::remove_file(&target); }
                    return Err("Python 执行超时（已强制终止，30秒限制）".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                if tmp_cleanup { let _ = std::fs::remove_file(&target); }
                return Err(format!("Python 执行异常: {}", e));
            }
        }
    };

    let output = match exit_status {
        Some(_) => child.wait_with_output().map_err(|e| format!("读取输出失败: {}", e))?,
        None => unreachable!(),
    };

    if tmp_cleanup {
        let _ = std::fs::remove_file(&target);
    }

    let stdout = crate::decode_windows_output(&output.stdout).trim().to_string();
    let stderr = crate::decode_windows_output(&output.stderr).trim().to_string();

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(&stderr);
    }

    let exit_code = output.status.code().unwrap_or(-1);
    if result.is_empty() {
        Ok(format!("执行完毕（退出码: {}），无输出", exit_code))
    } else {
        Ok(format!("退出码: {}\n{}", exit_code, result))
    }
}
