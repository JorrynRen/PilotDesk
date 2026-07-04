use tauri::Emitter;
use std::process::{Command, Stdio};
use std::thread;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use crate::db::models::EnvInfo;
use crate::utils::errors::AppError;
use crate::commands::agents;

/// Debug logging for env detection
macro_rules! log_env {
    ($($arg:tt)*) => {{
        log::debug!($($arg)*);
    }};
}

// ──────────────────────────────────────────────
//  工具函数
// ──────────────────────────────────────────────

/// Run a command and capture stdout, with timeout.
/// For .cmd/.bat scripts on Windows, wraps in `cmd /C`.
fn run_cmd(cmd: &str, arg: &str, use_cmd_wrapper: bool) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    let cmd_s = cmd.to_string();
    let arg_s = arg.to_string();
    thread::spawn(move || {
        let mut command = if use_cmd_wrapper {
            let mut c = Command::new("cmd");
            c.args(["/C", &cmd_s, &arg_s]);
            c
        } else {
            let mut c = Command::new(&cmd_s);
            c.arg(&arg_s);
            c
        };

        let output = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let _ = tx.send(output);
    });

    let output = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            log_env!("[env] Spawn failed for {}: {}", cmd, e);
            return None;
        }
        Err(_) => {
            log_env!("[env] Timeout (15s) for: {} {}", cmd, arg);
            return None;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Some tools (e.g. python on Windows) output version to stderr
    let output_text = if stdout.trim().is_empty() && !stderr.trim().is_empty() {
        stderr
    } else {
        stdout
    };

    let first_line = output_text.lines().next()?;
    if output.status.success() {
        Some(first_line.trim().to_string())
    } else {
        None
    }
}

/// Run a full shell command string via cmd /C (for install/uninstall/update commands from DB)
pub fn run_shell_cmd(cmd_str: &str) -> Result<String, String> {
    let mut child = Command::new("cmd");
    child.args(["/C", cmd_str])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = child.spawn()
        .map_err(|e| format!("执行命令失败: {}", e))?;

    let output = child.wait_with_output()
        .map_err(|e| format!("命令执行过程出错: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("命令执行失败 (exit code {:?}): {}", output.status.code(), stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim().to_string())
}

/// 探测工具版本：先尝试动态查找（用 cmd /C 处理 .cmd 脚本），再回退 PATH
fn probe_tool_version(name: &str, version_flag: &str) -> Option<String> {
    if let Some(path) = crate::utils::paths::resolve_in_path(name) {
        if let Some(v) = run_cmd(&path, version_flag, true) {
            return Some(v);
        }
    }
    run_cmd(name, version_flag, false)
}

/// Clean version string: keep only the semver part
fn clean_version_string(raw: &str) -> String {
    let trimmed = raw.trim();
    let segments: Vec<&str> = trimmed
        .split(|c: char| c == ' ' || c == '(')
        .collect();

    // Priority 1: "v" + digit (e.g. "v0.16.0")
    if let Some(ver) = segments.iter().find(|s| {
        s.starts_with('v') && s.len() > 1
            && s[1..].chars().next().map_or(false, |c| c.is_ascii_digit())
    }) {
        return ver.trim_start_matches('v').trim().to_string();
    }

    // Priority 2: starts with digit (e.g. "2.1.177", "0.140.0-alpha.2")
    if let Some(ver) = segments.iter().find(|s| {
        !s.is_empty() && s.chars().next().map_or(false, |c| c.is_ascii_digit())
    }) {
        return ver.trim().to_string();
    }

    trimmed.to_string()
}

// ──────────────────────────────────────────────
//  环境检测
// ──────────────────────────────────────────────

/// 简单缓存：仅用于避免高频重复检测
static LAST_DETECT: std::sync::RwLock<Option<(Instant, EnvInfo)>> = std::sync::RwLock::new(None);
const CACHE_TTL: Duration = Duration::from_secs(30);

fn detect_env_inner(state: &crate::DbState) -> Result<EnvInfo, AppError> {
    log_env!("[env] Detecting environment...");

    let node_version = probe_tool_version("node", "--version");
    let git_version = probe_tool_version("git", "--version");
    let python_version = probe_tool_version("python", "--version")
        .or_else(|| probe_tool_version("python3", "--version"))
        .or_else(|| probe_tool_version("py", "--version"));

    // Read enabled agents from DB and detect versions
    let conn = state.get_conn()?;
    let db_agents = agents::list_agents_inner(&conn).unwrap_or_default();

    let mut agent_versions = std::collections::HashMap::new();

    for agent in &db_agents {
        if !agent.is_enabled {
            continue;
        }
        let version = run_shell_cmd(&agent.version_cmd).ok()
            .map(|v| clean_version_string(&v));
        agent_versions.insert(agent.agent_type.clone(), version);
    }

    log_env!("[env] node={:?} git={:?} python={:?} agents={:?}",
        node_version, git_version, python_version, agent_versions);

    Ok(EnvInfo {
        node_version,
        git_version,
        python_version,
        agent_versions,
    })
}

#[tauri::command]
pub async fn detect_env(state: tauri::State<'_, crate::DbState>) -> Result<EnvInfo, AppError> {
    {
        let cache = LAST_DETECT.read().unwrap();
        if let Some((fetched, ref info)) = *cache {
            if fetched.elapsed() < CACHE_TTL {
                return Ok(info.clone());
            }
        }
    }

    let state_ref: crate::DbState = crate::DbState { pool: state.pool.clone() };
    let result = tokio::task::spawn_blocking(move || detect_env_inner(&state_ref)).await
        .map_err(|e| AppError::External(format!("环境检测任务失败: {}", e)))?;

    if let Ok(ref info) = result {
        let mut cache = LAST_DETECT.write().unwrap();
        *cache = Some((Instant::now(), info.clone()));
    }

    result
}

/// 清除环境检测缓存（安装/更新后调用）
pub fn clear_env_cache() {
    if let Ok(mut cache) = LAST_DETECT.write() {
        *cache = None;
    }
}

#[tauri::command]
pub async fn clear_env_detect_cache() -> Result<(), AppError> {
    clear_env_cache();
    Ok(())
}

// ──────────────────────────────────────────────
//  安装/卸载/更新 — 从 DB 读取命令并执行
// ──────────────────────────────────────────────

/// 根据 agent_type 从 DB 读取 install_cmd 并执行
#[tauri::command]
pub async fn install_agent(app: tauri::AppHandle, state: tauri::State<'_, crate::DbState>, agent_type: String) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    let config = agents::get_agent_inner(&conn, &agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型 '{}' 不存在", agent_type)))?;

    let cmd = config.install_cmd;
    if cmd.is_empty() {
        return Err(AppError::Config(format!("{} 未配置安装命令", agent_type)));
    }

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent_type,
        "message": format!("正在安装 {}...", config.display_name),
        "progress": 50
    }));

    run_shell_cmd(&cmd).map_err(|e| AppError::External(format!("安装 {} 失败: {}", agent_type, e)))?;

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent_type,
        "message": format!("{} 安装完成", config.display_name),
        "progress": 100
    }));

    clear_env_cache();
    Ok(())
}

/// 根据 agent_type 从 DB 读取 uninstall_cmd 并执行
#[tauri::command]
pub async fn uninstall_agent(app: tauri::AppHandle, state: tauri::State<'_, crate::DbState>, agent_type: String) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    let config = agents::get_agent_inner(&conn, &agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型 '{}' 不存在", agent_type)))?;

    let cmd = config.uninstall_cmd;
    if cmd.is_empty() {
        return Err(AppError::Config(format!("{} 未配置卸载命令", agent_type)));
    }

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent_type,
        "message": format!("正在卸载 {}...", config.display_name),
        "progress": 50
    }));

    run_shell_cmd(&cmd).map_err(|e| AppError::External(format!("卸载 {} 失败: {}", agent_type, e)))?;

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent_type,
        "message": format!("{} 卸载完成", config.display_name),
        "progress": 100
    }));

    clear_env_cache();
    Ok(())
}

// ──────────────────────────────────────────────
//  目录校验
// ──────────────────────────────────────────────

/// Validate a directory path and create it if it doesn't exist.
/// Returns the normalized path on success, or an error message on failure.
#[tauri::command]
pub fn ensure_dir(path: String) -> Result<String, String> {
    if path.trim().is_empty() {
        return Err("路径不能为空".into());
    }

    let trimmed = path.trim();

    // Expand ~ to user home directory
    let expanded = if trimmed.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            trimmed.replacen("~", &home.to_string_lossy(), 1)
        } else {
            return Err("无法解析用户主目录路径".into());
        }
    } else {
        trimmed.to_string()
    };

    let p = std::path::Path::new(&expanded);

    #[cfg(target_os = "windows")]
    {
        // Reject bare drive letters like "C:" or "D:"
        if trimmed.len() <= 2 && trimmed.ends_with(':') {
            return Err("路径不完整，请输入完整目录路径（如 C:\\Users\\work）".into());
        }

        // Check for invalid path characters on Windows
        let invalid_chars = ['<', '>', '"', '|', '?', '*'];
        for c in invalid_chars {
            if trimmed.contains(c) {
                return Err(format!("路径包含非法字符 '{}'", c));
            }
        }

        // Ensure drive letter is valid (A-Z)
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            let drive = bytes[0].to_ascii_uppercase();
            if drive < b'A' || drive > b'Z' {
                return Err("无效的盘符，请输入有效的驱动器号（如 C:、D:）".into());
            }
        }
    }

    // Check if path exists
    if p.exists() {
        if p.is_dir() {
            Ok(expanded.to_string())
        } else {
            Err(format!("路径已存在但不是一个目录: {}", expanded))
        }
    } else {
        // Create directory (including parent directories)
        std::fs::create_dir_all(p)
            .map_err(|e| format!("创建目录失败: {}", e))?;
        Ok(expanded.to_string())
    }
}
