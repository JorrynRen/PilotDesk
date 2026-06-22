use std::path::PathBuf;
use std::process::Command;

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("Cannot determine app data directory")
        .join("PilotDesk")
}

/// 获取内置资源目录（打包携带的 Agent 配置、默认图标等，只读）
/// 注意：此函数仅用于开发模式快速获取路径。
/// 生产环境应通过 app.path().resource_dir() 获取（由 Tauri 框架管理路径）。
/// 路径规则：
///   - 开发模式: src-tauri/resources/
///   - Windows MSI: %LOCALAPPDATA%\com.pilotdesk.app\resources\
///   - Windows NSIS: %APPDATA%\com.pilotdesk.app\resources\
#[allow(dead_code)]
pub fn builtin_resources_dir() -> PathBuf {
    // 开发模式使用项目目录下的 resources 文件夹
    // 生产模式通过 Tauri 的 resource_dir 解析
    let mut dir = std::env::current_exe()
        .expect("Cannot determine executable path");
    dir.pop(); // 移除 exe 文件名
    // 开发模式: target/debug/ -> 项目根目录/src-tauri/resources
    // 生产模式: 资源会被 Tauri 复制到 resource_dir
    dir.join("resources")
}

/// 获取用户资源目录（自定义图标、用户上传文件等，可读写）
#[allow(dead_code)]
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
