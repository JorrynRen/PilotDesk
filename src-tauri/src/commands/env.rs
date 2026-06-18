use tauri::Emitter;
use std::process::{Command, Stdio};
use std::thread;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use crate::db::models::EnvInfo;
use crate::utils::errors::AppError;

/// Debug logging for env detection
macro_rules! log_env {
    ($($arg:tt)*) => {{
        log::debug!($($arg)*);
    }};
}

// ──────────────────────────────────────────────
//  Agent 集中配置表
// ──────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AgentConfig {
    /// Agent 类型标识（claude / hermes / codex）
    pub agent_type: &'static str,
    /// npm 包名（None 表示非 npm 安装）
    pub npm_package: Option<&'static str>,
    /// pip 包名（None 表示非 pip 安装）
    pub pip_package: Option<&'static str>,
    /// CLI 命令名
    pub cli_command: &'static str,
    /// 版本检测参数
    pub version_flag: &'static str,
}

pub const AGENTS: &[AgentConfig] = &[
    AgentConfig {
        agent_type: "claude",
        npm_package: Some("@anthropic-ai/claude-code"),
        pip_package: None,
        cli_command: "claude",
        version_flag: "--version",
    },
    AgentConfig {
        agent_type: "hermes",
        npm_package: None,
        pip_package: Some("hermes-agent"),
        cli_command: "hermes",
        version_flag: "--version",
    },
    AgentConfig {
        agent_type: "codex",
        npm_package: Some("@openai/codex"),
        pip_package: None,
        cli_command: "codex",
        version_flag: "--version",
    },
];

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
        let output = if use_cmd_wrapper {
            Command::new("cmd")
                .args(["/C", &cmd_s, &arg_s])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
        } else {
            Command::new(&cmd_s).arg(&arg_s)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
        };
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

fn detect_env_inner() -> Result<EnvInfo, AppError> {
    log_env!("[env] Detecting environment...");

    let node_version = probe_tool_version("node", "--version");
    let git_version = probe_tool_version("git", "--version");
    let python_version = probe_tool_version("python", "--version")
        .or_else(|| probe_tool_version("python3", "--version"));

    let claude_code_version = probe_tool_version("claude", "--version")
        .as_deref().map(clean_version_string);
    let hermes_version = probe_tool_version("hermes", "--version")
        .as_deref().map(clean_version_string);
    let codex_version = probe_tool_version("codex", "--version")
        .as_deref().map(clean_version_string);

    log_env!("[env] node={:?} git={:?} python={:?} claude={:?} hermes={:?} codex={:?}",
        node_version, git_version, python_version,
        claude_code_version, hermes_version, codex_version);

    Ok(EnvInfo {
        node_version,
        git_version,
        python_version,
        claude_code_version,
        hermes_version,
        codex_version,
    })
}

#[tauri::command]
pub async fn detect_env() -> Result<EnvInfo, AppError> {
    {
        let cache = LAST_DETECT.read().unwrap();
        if let Some((fetched, ref info)) = *cache {
            if fetched.elapsed() < CACHE_TTL {
                return Ok(info.clone());
            }
        }
    }

    let result = tokio::task::spawn_blocking(detect_env_inner).await
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

// ──────────────────────────────────────────────
//  泛化安装函数
// ──────────────────────────────────────────────

enum PackageManager { Npm, Pip }

fn run_install(app: &tauri::AppHandle, manager: PackageManager, package: &str, agent: &str) -> Result<(), AppError> {
    let cmd = match manager {
        PackageManager::Npm => format!("npm install -g {}", package),
        PackageManager::Pip => format!("pip install {}", package),
    };

    let child = Command::new("cmd")
        .args(["/C", &cmd])
        .spawn()
        .map_err(|e| AppError::External(format!("安装 {} 失败: {}", agent, e)))?;

    let output = child.wait_with_output().map_err(|e| AppError::External(format!("安装 {} 过程出错: {}", agent, e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::External(format!("{} 安装失败: {}", agent, stderr)));
    }

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent,
        "message": format!("{} 安装完成", agent),
        "progress": 100
    }));

    clear_env_cache();
    Ok(())
}

/// 根据 AGENTS 配置表泛化安装命令
#[tauri::command]
pub async fn install_agent(app: tauri::AppHandle, agent_type: String) -> Result<(), AppError> {
    let config = AGENTS.iter().find(|a| a.agent_type == agent_type)
        .ok_or_else(|| AppError::InvalidInput(format!("未知 Agent 类型: {}", agent_type)))?;

    let (manager, package) = match (config.npm_package, config.pip_package) {
        (Some(pkg), _) => (PackageManager::Npm, pkg),
        (_, Some(pkg)) => (PackageManager::Pip, pkg),
        _ => return Err(AppError::Config(format!("{} 无可用的包管理器", agent_type))),
    };

    run_install(&app, manager, package, &agent_type)
}

// 保留旧命令作为别名（兼容前端调用）
#[tauri::command]
pub async fn install_claude_code(app: tauri::AppHandle) -> Result<(), AppError> {
    install_agent(app, "claude".to_string()).await
}

#[tauri::command]
pub async fn install_hermes(app: tauri::AppHandle) -> Result<(), AppError> {
    install_agent(app, "hermes".to_string()).await
}

#[tauri::command]
pub async fn install_codex(app: tauri::AppHandle) -> Result<(), AppError> {
    install_agent(app, "codex".to_string()).await
}
