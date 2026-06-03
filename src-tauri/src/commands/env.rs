use tauri::Emitter;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use crate::db::models::EnvInfo;
use crate::utils::errors::AppError;
/// Safe logging macro that silently ignores stderr write errors
/// (avoids panic when stderr pipe is closed by the OS).
macro_rules! log_env {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}



/// Cached env detection result with TTL
struct EnvCache {
    value: Option<EnvInfo>,
    fetched_at: Option<Instant>,
    in_progress: bool,
}

static ENV_CACHE: std::sync::LazyLock<Mutex<EnvCache>> = std::sync::LazyLock::new(|| {
    Mutex::new(EnvCache {
        value: None,
        fetched_at: None,
        in_progress: false,
    })
});

const ENV_CACHE_TTL: Duration = Duration::from_secs(30);

fn get_version(cmd: &str, arg: &str) -> Option<String> {
    let output = Command::new(cmd).arg(arg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?;
    if output.status.success() {
        Some(first_line.trim().to_string())
    } else {
        None
    }
}

/// Run a .cmd/.bat script with absolute path on Windows, with 15s timeout.
fn get_version_bat(exe_path: &str, arg: &str) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    let exe = exe_path.to_string();
    let arg_s = arg.to_string();
    thread::spawn(move || {
        let output = Command::new("cmd")
            .args(["/C", &exe, &arg_s])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let _ = tx.send(output);
    });

    let output = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            log_env!("[env] Spawn failed for {}: {}", exe_path, e);
            return None;
        }
        Err(_) => {
            log_env!("[env] Timeout (15s) for: {} {}", exe_path, arg);
            return None;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?;
    if output.status.success() {
        Some(first_line.trim().to_string())
    } else {
        None
    }
}

fn probe_version(paths: &[&str], arg: &str) -> Option<String> {
    for p in paths {
        if let Some(v) = get_version_bat(p, arg) {
            return Some(v);
        }
    }
    get_version(paths.last().unwrap_or(&""), arg)
}

/// Clean version string: keep only the semver part (e.g. "2.1.157 (Claude Code)" → "2.1.157")
/// Also strips leading 'v'.
fn clean_version_string(raw: &str) -> String {
    let trimmed = raw.trim();
    // Take only up to first space or '('
    trimmed
        .trim_start_matches('v')
        .split(|c: char| c == ' ' || c == '(')
        .next()
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

fn detect_env_inner() -> Result<EnvInfo, AppError> {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| r"C:\Users\Administrator".into());
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| format!(r"{}\AppData\Roaming", home));
    let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| format!(r"{}\AppData\Local", home));

    log_env!("[env] Detecting environment...");

    let claude_paths = [
        &format!(r"{}\npm\claude.cmd", appdata),
        "claude",
    ];

    let hermes_paths = [
        &format!(r"{}\Programs\Python\Python313\Scripts\hermes.bat", localappdata),
        &format!(r"{}\Programs\Python\Python312\Scripts\hermes.bat", localappdata),
        &format!(r"{}\Programs\Python\Python311\Scripts\hermes.bat", localappdata),
        "hermes",
    ];

    let claude_code_version = probe_version(&claude_paths, "--version")
        .as_deref()
        .map(clean_version_string);
    log_env!("[env] claude: {:?}", claude_code_version);

    let hermes_version = probe_version(&hermes_paths, "version")
        .map(|v| {
            let first_line = v.lines().next().unwrap_or(&v);
            clean_version_string(first_line.trim_start_matches("Hermes Agent "))
        });
    log_env!("[env] hermes: {:?}", hermes_version);

    // Try known node paths (Tauri process may not inherit full PATH)
    let node_version = get_version_bat(r"F:\soft\nodejs\node.exe", "--version")
        .or_else(|| get_version("node", "--version"));

    // Git: try common install paths first, fall back to PATH
    let git_paths = [
        &format!(r"{}\Program Files\Git\bin\git.exe", std::env::var("PROGRAMFILES").unwrap_or_else(|_| r"C:\Program Files".into())),
        &format!(r"{}\Program Files\Git\cmd\git.exe", std::env::var("PROGRAMFILES").unwrap_or_else(|_| r"C:\Program Files".into())),
        &format!(r"{}\Program Files (x86)\Git\bin\git.exe", std::env::var("PROGRAMFILES").unwrap_or_else(|_| r"C:\Program Files".into())),
        "git",
    ];
    let git_version = probe_version(&git_paths, "--version");

    // Python: try common install paths first, fall back to PATH
    let python_paths = [
        &format!(r"{}\Programs\Python\Python313\python.exe", localappdata),
        &format!(r"{}\Programs\Python\Python312\python.exe", localappdata),
        &format!(r"{}\Programs\Python\Python311\python.exe", localappdata),
        &format!(r"C:\Python313\python.exe"),
        &format!(r"C:\Python312\python.exe"),
        "python",
        "python3",
    ];
    let python_version = probe_version(&python_paths, "--version");

    log_env!("[env] node={:?} git={:?} python={:?}", node_version, git_version, python_version);

    Ok(EnvInfo {
        node_version,
        git_version,
        python_version,
        claude_code_version,
        hermes_version,
    })
}

#[tauri::command]
pub async fn detect_env() -> Result<EnvInfo, AppError> {
    // Return cached result if fresh (within TTL)
    {
        let cache = ENV_CACHE.lock().unwrap();
        if let (Some(ref info), Some(fetched)) = (&cache.value, &cache.fetched_at) {
            if fetched.elapsed() < ENV_CACHE_TTL {
                log_env!("[env] Returning cached result ({}ms old)", fetched.elapsed().as_millis());
                return Ok(info.clone());
            }
        }
    }

    // Prevent concurrent detections — wait or return cached
    let need_detect = {
        let mut cache = ENV_CACHE.lock().unwrap();
        if cache.in_progress {
            if cache.value.is_some() {
                return Ok(cache.value.clone().unwrap());
            }
            drop(cache);
            log_env!("[env] Waiting for in-progress detection...");
            for _ in 0..300 {
                std::thread::sleep(Duration::from_millis(100));
                let cache = ENV_CACHE.lock().unwrap();
                if let Some(ref info) = cache.value {
                    if let Some(fetched) = &cache.fetched_at {
                        if fetched.elapsed() < Duration::from_secs(5) {
                            return Ok(info.clone());
                        }
                    }
                }
                drop(cache);
            }
            false // give up
        } else {
            cache.in_progress = true;
            true
        }
    };
    if !need_detect {
        { let mut c = ENV_CACHE.lock().unwrap(); c.in_progress = false; }
        return Err(AppError {
            code: "ENV_DETECT_BUSY".into(),
            message: "环境检测繁忙，请稍后重试".into(),
            details: None,
        });
    }

    let result = tokio::task::spawn_blocking(detect_env_inner).await;

    let mut cache = ENV_CACHE.lock().unwrap();
    cache.in_progress = false;
    match result {
        Ok(Ok(info)) => {
            cache.value = Some(info.clone());
            cache.fetched_at = Some(Instant::now());
            Ok(info)
        }
        Ok(Err(e)) => Err(e),
        Err(e) => Err(AppError {
            code: "ENV_DETECT_FAILED".into(),
            message: format!("环境检测任务失败: {}", e),
            details: None,
        }),
    }
}

#[tauri::command]
pub async fn install_claude_code(app: tauri::AppHandle) -> Result<(), AppError> {
    let child = Command::new("cmd").args(["/C", "npm", "install", "-g", "@anthropic-ai/claude-code"]).spawn().map_err(|e| AppError {
        code: "FATAL_INSTALL_FAILED".into(),
        message: format!("安装 Claude Code 失败: {}", e),
        details: None,
    })?;

    let output = child.wait_with_output().map_err(|e| AppError {
        code: "FATAL_INSTALL_FAILED".into(),
        message: format!("安装 Claude Code 过程出错: {}", e),
        details: None,
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError {
            code: "ERR_INSTALL_FAILED".into(),
            message: "Claude Code 安装失败".into(),
            details: Some(stderr.to_string()),
        });
    }

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": "claude",
        "message": "Claude Code 安装完成",
        "progress": 100
    }));

    // Clear cache so next detect_env picks up the new installation
    { let mut c = ENV_CACHE.lock().unwrap(); c.value = None; c.fetched_at = None; }

    Ok(())
}

#[tauri::command]
pub async fn install_hermes(app: tauri::AppHandle) -> Result<(), AppError> {
    let child = Command::new("cmd").args(["/C", "pip", "install", "hermes-cli"]).spawn().map_err(|e| AppError {
        code: "FATAL_INSTALL_FAILED".into(),
        message: format!("安装 Hermes Agent 失败: {}", e),
        details: None,
    })?;

    let output = child.wait_with_output().map_err(|e| AppError {
        code: "FATAL_INSTALL_FAILED".into(),
        message: format!("安装 Hermes Agent 过程出错: {}", e),
        details: None,
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError {
            code: "ERR_INSTALL_FAILED".into(),
            message: "Hermes Agent 安装失败".into(),
            details: Some(stderr.to_string()),
        });
    }

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": "hermes",
        "message": "Hermes Agent 安装完成",
        "progress": 100
    }));

    { let mut c = ENV_CACHE.lock().unwrap(); c.value = None; c.fetched_at = None; }

    Ok(())
}
