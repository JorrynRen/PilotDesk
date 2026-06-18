use rusqlite::params;
use tauri::State;
use tauri::Emitter;
use crate::DbState;
use crate::utils::errors::AppError;
use crate::db::models::LogEntry;

/// Insert a log entry into the install_logs table
#[tauri::command]
pub fn insert_log(app: tauri::AppHandle, conn: State<'_, DbState>, message: String, level: Option<String>) -> Result<i64, AppError> {
    let level = level.unwrap_or_else(|| "info".to_string());
    let ts = crate::utils::now_millis();

    let conn = conn.get_conn()?;

    conn.execute(
        "INSERT INTO install_logs (timestamp, message, level) VALUES (?1, ?2, ?3)",
        params![ts, message, level],
    )?;

    // Delete logs older than 7 days
    let cutoff = crate::utils::now_millis() - 7 * 24 * 60 * 60 * 1000;
    let _ = conn.execute("DELETE FROM install_logs WHERE timestamp < ?1", params![cutoff]);

    // Emit event so InstallLog can refresh
    let _ = app.emit("log-updated", format!("log:{}", message));

    Ok(ts)
}

/// List recent log entries (most recent first, default max 200)
#[tauri::command]
pub fn list_logs(conn: State<'_, DbState>, limit: Option<i64>) -> Result<Vec<LogEntry>, AppError> {
    let limit = limit.unwrap_or(200);
    let conn = conn.get_conn()?;

    let mut stmt = conn.prepare(
        "SELECT id, timestamp, message, level FROM install_logs ORDER BY timestamp DESC LIMIT ?1"
    )?;

    let rows = stmt.query_map(params![limit], |row| {
        Ok(LogEntry {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            message: row.get(2)?,
            level: row.get(3)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }

    Ok(entries)
}

/// Clear all log entries
#[tauri::command]
pub fn clear_logs(conn: State<'_, DbState>) -> Result<(), AppError> {
    let conn = conn.get_conn()?;
    conn.execute("DELETE FROM install_logs", [])?;
    Ok(())
}
