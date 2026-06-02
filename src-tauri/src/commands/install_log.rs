use rusqlite::params;
use tauri::State;
use crate::DbState;
use crate::utils::errors::AppError;
use serde::{Deserialize, Serialize};

/// Log entry returned from the database
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: i64,
    pub timestamp: i64,
    pub message: String,
    pub level: String,
}

/// Insert a log entry into the install_logs table
#[tauri::command]
pub fn insert_log(conn: State<'_, DbState>, message: String, level: Option<String>) -> Result<i64, AppError> {
    let level = level.unwrap_or_else(|| "info".to_string());
    let ts = chrono::Utc::now().timestamp_millis();

    let conn = conn.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "数据库锁获取失败".into(),
        details: Some(e.to_string()),
    })?;

    conn.execute(
        "INSERT INTO install_logs (timestamp, message, level) VALUES (?1, ?2, ?3)",
        params![ts, message, level],
    ).map_err(|e| AppError {
        code: "ERR_INSERT_LOG".into(),
        message: "写入日志失败".into(),
        details: Some(e.to_string()),
    })?;

    // Delete logs older than 7 days
    let cutoff = chrono::Utc::now().timestamp_millis() - 7 * 24 * 60 * 60 * 1000;
    let _ = conn.execute("DELETE FROM install_logs WHERE timestamp < ?1", params![cutoff]);

    Ok(ts)
}

/// List recent log entries (most recent first, default max 200)
#[tauri::command]
pub fn list_logs(conn: State<'_, DbState>, limit: Option<i64>) -> Result<Vec<LogEntry>, AppError> {
    let limit = limit.unwrap_or(200);
    let conn = conn.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "数据库锁获取失败".into(),
        details: Some(e.to_string()),
    })?;

    let mut stmt = conn.prepare(
        "SELECT id, timestamp, message, level FROM install_logs ORDER BY timestamp DESC LIMIT ?1"
    ).map_err(|e| AppError {
        code: "ERR_QUERY_LOGS".into(),
        message: "查询日志失败".into(),
        details: Some(e.to_string()),
    })?;

    let rows = stmt.query_map(params![limit], |row| {
        Ok(LogEntry {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            message: row.get(2)?,
            level: row.get(3)?,
        })
    }).map_err(|e| AppError {
        code: "ERR_QUERY_LOGS".into(),
        message: "查询日志失败".into(),
        details: Some(e.to_string()),
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row.map_err(|e| AppError {
            code: "ERR_QUERY_LOGS".into(),
            message: "查询日志失败".into(),
            details: Some(e.to_string()),
        })?);
    }

    // Reverse so oldest first for display
    entries.reverse();
    Ok(entries)
}

/// Clear all log entries
#[tauri::command]
pub fn clear_logs(conn: State<'_, DbState>) -> Result<(), AppError> {
    let conn = conn.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "数据库锁获取失败".into(),
        details: Some(e.to_string()),
    })?;

    conn.execute("DELETE FROM install_logs", []).map_err(|e| AppError {
        code: "ERR_CLEAR_LOGS".into(),
        message: "清空日志失败".into(),
        details: Some(e.to_string()),
    })?;

    Ok(())
}
