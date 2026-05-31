use std::fs;
use crate::utils::paths::app_data_dir;
use crate::utils::errors::AppError;

/// Get the persisted theme setting
pub fn get_theme() -> String {
    let path = app_data_dir().join("theme.txt");
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            let trimmed = content.trim().to_string();
            if matches!(trimmed.as_str(), "light" | "dark" | "system") {
                return trimmed;
            }
        }
    }
    "system".to_string()
}

/// Save theme setting to disk
pub fn set_theme(theme: String) -> Result<String, AppError> {
    if !matches!(theme.as_str(), "light" | "dark" | "system") {
        return Err(AppError {
            code: "ERR_INVALID_THEME".to_string(),
            message: format!("无效的主题值: {}", theme),
            details: None,
        });
    }
    let dir = app_data_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join("theme.txt");
    fs::write(&path, &theme)?;
    Ok(theme)
}
