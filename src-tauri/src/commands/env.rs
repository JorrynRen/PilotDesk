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

#[tauri::command]
pub fn detect_env() -> Result<EnvInfo, AppError> {
    Ok(EnvInfo {
        node_version: get_version("node", "--version"),
        git_version: get_version("git", "--version"),
        python_version: get_version("python", "--version")
            .or_else(|| get_version("python3", "--version")),
        claude_code_version: get_version("claude", "--version"),
        hermes_version: None,
    })
}

#[tauri::command]
pub async fn install_claude_code(app: tauri::AppHandle) -> Result<(), AppError> {
    let child = Command::new("npm")
        .args(["install", "-g", "@anthropic-ai/claude-code"])
        .spawn()
        .map_err(|e| AppError {
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
    let child = Command::new("pip")
        .args(["install", "hermes-agent"])
        .spawn()
        .map_err(|e| AppError {
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
