use tauri::Emitter;
use std::process::{Command, Stdio};
use std::thread;
use std::sync::mpsc;
use std::time::Duration;
use crate::db::models::EnvInfo;
use crate::utils::errors::AppError;

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
            eprintln!("[env] Spawn failed for {}: {}", exe_path, e);
            return None;
        }
        Err(_) => {
            eprintln!("[env] Timeout (15s) for: {} {}", exe_path, arg);
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

fn detect_env_inner() -> Result<EnvInfo, AppError> {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| r"C:\Users\Administrator".into());
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| format!(r"{}\AppData\Roaming", home));
    let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| format!(r"{}\AppData\Local", home));

    println!("[env] Detecting environment...");

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

    let claude_code_version = probe_version(&claude_paths, "--version");
    println!("[env] claude: {:?}", claude_code_version);

    let hermes_version = probe_version(&hermes_paths, "version")
        .map(|v| {
            let first_line = v.lines().next().unwrap_or(&v);
            first_line.trim_start_matches("Hermes Agent ").to_string()
        });
    println!("[env] hermes: {:?}", hermes_version);

    // Try known node paths (Tauri process may not inherit full PATH)
    let node_version = get_version_bat(r"F:\soft\nodejs\node.exe", "--version")
        .or_else(|| get_version("node", "--version"));
    let git_version = get_version("git", "--version");
    let python_version = get_version("python", "--version")
        .or_else(|| get_version("python3", "--version"));

    println!("[env] node={:?} git={:?} python={:?}", node_version, git_version, python_version);

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
    tokio::task::spawn_blocking(detect_env_inner).await.map_err(|e| AppError {
        code: "ENV_DETECT_FAILED".into(),
        message: format!("环境检测任务失败: {}", e),
        details: None,
    })?
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

    Ok(())
}
