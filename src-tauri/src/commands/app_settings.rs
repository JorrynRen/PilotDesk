use rusqlite::{params, OptionalExtension};
use crate::utils::errors::AppError;

/// Get a setting value by key
pub fn get_setting(conn: &rusqlite::Connection, key: &str) -> Result<Option<String>, AppError> {
    let value: Option<String> = conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?",
        params![key],
        |row| row.get("value"),
    ).optional()?;
    Ok(value)
}

/// Set a setting value (insert or update)
pub fn set_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = ?3",
        params![key, value, now],
    )?;
    Ok(())
}

/// Delete a setting by key
#[allow(dead_code)]
pub fn delete_setting(conn: &rusqlite::Connection, key: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM app_settings WHERE key = ?", params![key])?;
    Ok(())
}
