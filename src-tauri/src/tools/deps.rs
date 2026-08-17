//! 工具层共享复用函数（路径解析、glob 转换等）。
//!
//! 从 `lib.rs` / `groupchat/adapter/tools.rs` 抽出的公共原语，供各工具文件使用：
//! - `expand_user_path`：展开 `~/`、`~\\`、`%USERPROFILE%` 前缀

/// 路径预处理：展开 `~/`、`~\\`、`%USERPROFILE%` 前缀为完整用户目录。
pub fn expand_user_path(raw_path: &str) -> String {
    if raw_path.starts_with("~/") || raw_path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&raw_path[2..]).to_string_lossy().to_string();
        }
    } else if raw_path.to_lowercase().starts_with("%userprofile%") {
        if let Some(home) = dirs::home_dir() {
            let rest = &raw_path[14..];
            return home
                .join(rest.trim_start_matches('\\').trim_start_matches('/'))
                .to_string_lossy()
                .to_string();
        }
    }
    raw_path.to_string()
}
