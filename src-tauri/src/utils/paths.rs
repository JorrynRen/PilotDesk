use std::path::PathBuf;
use std::process::Command;

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("Cannot determine app data directory")
        .join("PilotDesk")
}

pub fn db_path() -> PathBuf {
    app_data_dir().join("pilotdesk.db")
}

/// 通过 `where` 命令动态查找可执行文件路径（Windows）
pub fn resolve_in_path(name: &str) -> Option<String> {
    let output = Command::new("where")
        .arg(name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|s| s.trim().to_string())
}
