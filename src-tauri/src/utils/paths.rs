use std::path::PathBuf;
use std::process::Command;

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("Cannot determine app data directory")
        .join("PilotDesk")
}

/// 获取内置资源目录（打包携带的 Agent 配置、默认图标等，只读）
pub fn builtin_resources_dir() -> PathBuf {
    let mut dir = std::env::current_exe()
        .expect("Cannot determine executable path");
    dir.pop();
    dir.join("resources")
}

/// 获取用户资源目录（自定义图标、用户上传文件等，可读写）
pub fn user_resources_dir() -> PathBuf {
    app_data_dir().join("resources")
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
