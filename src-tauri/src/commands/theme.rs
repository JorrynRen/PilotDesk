use crate::utils::errors::AppError;
use crate::commands::app_settings;

/// Get the persisted theme setting from app_settings table
pub fn get_theme(conn: &rusqlite::Connection) -> Result<String, AppError> {
    // Try reading from app_settings first
    if let Some(value) = app_settings::get_setting(conn, "theme")? {
        if matches!(value.as_str(), "light" | "dark" | "system") {
            return Ok(value);
        }
    }

    // Fallback: migrate from legacy theme.txt
    let legacy_path = crate::utils::paths::app_data_dir().join("theme.txt");
    if legacy_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&legacy_path) {
            let trimmed = content.trim().to_string();
            if matches!(trimmed.as_str(), "light" | "dark" | "system") {
                // Migrate to app_settings
                app_settings::set_setting(conn, "theme", &trimmed)?;
                // Remove legacy file
                let _ = std::fs::remove_file(&legacy_path);
                return Ok(trimmed);
            }
        }
    }

    Ok("system".to_string())
}

/// Save theme setting to app_settings table
pub fn set_theme(conn: &rusqlite::Connection, theme: String) -> Result<String, AppError> {
    if !matches!(theme.as_str(), "light" | "dark" | "system") {
        return Err(AppError::InvalidInput(format!("无效的主题值: {}", theme)));
    }
    app_settings::set_setting(conn, "theme", &theme)?;
    Ok(theme)
}
