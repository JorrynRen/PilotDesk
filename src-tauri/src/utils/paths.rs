use std::path::PathBuf;
use std::process::Command;

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("Cannot determine app data directory")
        .join("PilotDesk")
}

/// 获取应用资源目录（内置 Agent 配置、图标、用户上传资源等）
/// 打包后通过 tauri::path::BaseDirectory::Resource 解析
pub fn resources_dir() -> PathBuf {
    // 开发阶段直接使用项目目录下的 resources 文件夹
    // 打包后 Tauri 会自动处理资源路径
    let mut dir = std::env::current_exe()
        .expect("Cannot determine executable path");
    dir.pop(); // 移除 exe 文件名
    // 开发模式: target/debug/ -> 项目根目录/src-tauri/resources
    // 打包后: 资源会被复制到 exe 同级目录下的 resources/
    dir.join("resources")
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
