//! 统一路径管理模块
//!
//! 职责范围：本地文件系统路径的统一管理与派生
//! 所有路径派生自用户数据根目录（app_data_dir）
//!
//! 工作区路径解析规则：调用方自有配置 → 全局工作区（app_settings）→ 用户数据根目录
//!
//! 不包含：远程在线商店 URL 配置
//!   → Rust 端：见 market.rs（SERVER_SOURCES / build_urls / PLUGINS_INDEX_PATH 等）
//!   → 前端：  见 src/utils/market.ts（与 market.rs 保持镜像同步）
//!   注意：两端的 SERVER_SOURCES 和路径常量必须手动保持一致

use std::path::PathBuf;
use std::process::Command;

/// 获取用户数据根目录
pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("Cannot determine app data directory")
        .join("PilotDesk")
}

/// 数据库文件路径
pub fn db_path() -> PathBuf {
    app_data_dir().join("pilotdesk.db")
}

/// 加密密钥文件路径
pub fn encryption_key_path() -> PathBuf {
    app_data_dir().join(".key")
}

/// 用户资源目录
pub fn user_resources_dir() -> PathBuf {
    app_data_dir().join("resources")
}

/// Agent 图标目录
pub fn agent_icons_dir() -> PathBuf {
    user_resources_dir().join("icons")
}

/// Agent 自定义配置目录
pub fn agent_configs_dir() -> PathBuf {
    user_resources_dir().join("agents")
}

/// 其他资源目录
pub fn assets_dir() -> PathBuf {
    user_resources_dir().join("assets")
}

/// 插件安装目录
pub fn plugins_dir() -> PathBuf {
    app_data_dir().join("plugins")
}

/// 工作流模板目录（预留）
pub fn templates_dir() -> PathBuf {
    app_data_dir().join("templates")
}

/// 用户系统资源目录（预留）
pub fn users_dir() -> PathBuf {
    app_data_dir().join("users")
}

/// 从 app_settings 表读取全局工作区路径
/// 键名: pilotdesk-workspace
pub fn get_global_workspace(conn: &rusqlite::Connection) -> Option<String> {
    use rusqlite::OptionalExtension;
    let value: Option<String> = conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?",
        rusqlite::params!["pilotdesk-workspace"],
        |row| row.get("value"),
    ).optional().ok()?;
    value.and_then(|v| {
        if v.is_empty() { None } else { Some(v) }
    })
}

/// 解析工作区路径：调用方自有配置 -> 全局工作区 -> 用户数据根目录
pub fn resolve_workspace_path(
    caller_workspace: Option<&str>,
    sub_path: &str,
    conn: &rusqlite::Connection,
) -> PathBuf {
    match caller_workspace {
        Some(dir) if !dir.is_empty() => {
            let resolved = resolve_tilde(dir);
            PathBuf::from(resolved).join(sub_path)
        }
        _ => {
            match get_global_workspace(conn) {
                Some(global) => {
                    let resolved = resolve_tilde(&global);
                    PathBuf::from(resolved).join(sub_path)
                }
                None => {
                    app_data_dir().join(sub_path)
                }
            }
        }
    }
}

/// 解析路径中的 ~ 为用户 home 目录
fn resolve_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            let rest = &path[2..];
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

/// 获取内置资源目录（开发模式）
#[allow(dead_code)]
pub fn builtin_resources_dir() -> PathBuf {
    let mut dir = std::env::current_exe()
        .expect("Cannot determine executable path");
    dir.pop();
    dir.join("resources")
}

/// 通过 where 命令动态查找可执行文件路径（Windows）
///
/// 查找顺序：
/// 1. 使用 `where` 命令（Windows 原生，能找到 .exe/.cmd/.bat/.com 等）
/// 2. 回退到手动遍历 PATH 环境变量，依次查找 .exe -> .cmd -> .bat
///
/// 回退机制解决以下场景：
/// - Tauri 打包进程的 PATH 不完整，`where` 找不到 npm 全局命令
/// - `where` 对某些 PATHEXT 扩展名不敏感
pub fn resolve_in_path(name: &str) -> Option<String> {
    // 策略1：使用 where 命令
    if let Ok(output) = Command::new("where")
        .arg(name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = stdout.lines().next() {
                let trimmed = first_line.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    // 策略2：手动遍历 PATH 环境变量查找
    let path_var = std::env::var("PATH").ok()?;
    let extensions = [".exe", ".cmd", ".bat"];
    let name_lower = name.to_lowercase();
    let name_has_ext = name_lower.ends_with(".exe")
        || name_lower.ends_with(".cmd")
        || name_lower.ends_with(".bat");

    for dir in path_var.split(';') {
        let dir = dir.trim();
        if dir.is_empty() {
            continue;
        }

        let candidates = if name_has_ext {
            // 命令已带扩展名，直接查找
            vec![name.to_string()]
        } else {
            // 按优先级尝试各扩展名
            extensions.iter().map(|ext| format!("{}{}", name, ext)).collect()
        };

        for candidate in &candidates {
            let full_path = std::path::Path::new(dir).join(candidate);
            if full_path.is_file() {
                return Some(full_path.to_string_lossy().to_string());
            }
        }
    }

    None
}
