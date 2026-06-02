use tauri::Emitter;
use std::process::Command;
use crate::db::models::EnvInfo;
use crate::utils::errors::AppError;

fn get_version(cmd: &str, arg: &str) -> Option<String> {
    let output = Command::new(cmd).arg(arg).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?;
    if output.status.success() {
        Some(first_line.trim().to_string())
    } else {
        None
    }
}

/// Run a .cmd/.bat script with full absolute path, avoiding PATH lookup issues
/// in Tauri's limited environment on Windows.
fn get_version_absolute(exe_path: &str, arg: &str) -> Option<String> {
    let output = Command::new("cmd")
        .args(["/C", &format!("\"{}\" {}", exe_path, arg)])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?;
    if output.status.success() {
        Some(first_line.trim().to_string())
    } else {
        None
    }
}

/// Try multiple paths to find and run a CLI tool, returning its version.
fn probe_version(paths: &[&str], arg: &str) -> Option<String> {
    for p in paths {
        if let Some(v) = get_version_absolute(p, arg) {
            return Some(v);
        }
    }
    // Fallback: try bare command name via shell
    let shell_cmd = format!("{} {}", paths[0], arg);
    if let Some(v) = get_version(&paths[0].split('\\').last().unwrap_or(paths[0]), arg) {
        return Some(v);
    }
    None
}

#[tauri::command]
pub fn detect_env() -> Result<EnvInfo, AppError> {
    // Known install locations on Windows
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| r"C:\Users\Administrator".into());
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| format!(r"{}\AppData\Roaming", home));
    let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| format!(r"{}\AppData\Local", home));

    // Claude Code: installed via npm -g, lives in %APPDATA%\npm\claude.cmd
    let claude_paths = [
        &format!(r"{}\npm\claude.cmd", appdata),
        "claude",
    ];

    // Hermes: installed via pip, lives in Python313\Scripts\hermes.bat
    let hermes_paths = [
        &format!(r"{}\Programs\Python\Python313\Scripts\hermes.bat", localappdata),
        &format!(r"{}\Programs\Python\Python312\Scripts\hermes.bat", localappdata),
        &format!(r"{}\Programs\Python\Python311\Scripts\hermes.bat", localappdata),
        "hermes",
    ];

    let claude_code_version = probe_version(&claude_paths, "--version");

    let hermes_version = probe_version(&hermes_paths, "--version")
        .map(|v| {
            // Trim prefix "Hermes Agent " -> "v0.15.1 (2026.5.29)"
            v.trim_start_matches("Hermes Agent ").to_string()
        });

    Ok(EnvInfo {
        node_version: get_version("node", "--version"),
        git_version: get_version("git", "--version"),
        python_version: get_version("python", "--version")
            .or_else(|| get_version("python3", "--version")),
        claude_code_version,
        hermes_version,
    })
}

#[tauri::command]
pub async fn install_claude_code(app: tauri::AppHandle) -> Result<(), AppError> {
    let child = Command::new("cmd").args(["/C", "npm install -g @anthropic-ai/claude-code"]).spawn().map_err(|e| AppError {
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
    let child = Command::new("cmd").args(["/C", "pip install hermes-cli"]).spawn().map_err(|e| AppError {
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
