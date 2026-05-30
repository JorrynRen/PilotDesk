use crate::db::models::EnvInfo;
use std::process::Command;

#[tauri::command]
pub fn detect_env() -> EnvInfo {
    EnvInfo {
        node_version: get_cmd_version("node", &["--version"]),
        git_version: get_cmd_version("git", &["--version"]),
        python_version: get_cmd_version("python", &["--version"])
            .or_else(|| get_cmd_version("python3", &["--version"])),
        claude_code_version: get_cmd_version("claude", &["--version"]),
        hermes_version: None,
    }
}

fn get_cmd_version(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
