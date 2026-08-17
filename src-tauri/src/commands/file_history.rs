//! Agent 文件修改历史与撤销
//!
//! write_file / edit_file 在覆盖文件前会记录旧内容快照，供用户撤销到上一个版本。

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::db::init::DbPool;
use crate::utils::errors::AppError;

/// 超过该字节数不备份（避免数据库膨胀）
const MAX_BACKUP_BYTES: usize = 2_000_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryEntry {
    pub id: i64,
    pub session_id: String,
    pub file_path: String,
    pub file_existed: bool,
    pub created_at: i64,
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<FileHistoryEntry> {
    Ok(FileHistoryEntry {
        id: row.get(0)?,
        session_id: row.get(1)?,
        file_path: row.get(2)?,
        file_existed: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
    })
}

/// 记录一次文件修改（同步，供 write_file / edit_file 工具内部调用）。
/// 内容超过阈值时跳过备份。
pub fn record_file_change(
    pool: &DbPool,
    session_id: &str,
    file_path: &str,
    backup_content: &str,
    file_existed: bool,
) {
    if backup_content.len() > MAX_BACKUP_BYTES {
        return;
    }
    if let Ok(conn) = pool.get() {
        let now = crate::utils::now();
        let _ = conn.execute(
            "INSERT INTO file_history (session_id, file_path, backup_content, file_existed, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, file_path, backup_content, if file_existed { 1 } else { 0 }, now],
        );
    }
}

#[tauri::command]
pub fn list_file_history(
    state: State<'_, crate::DbState>,
    session_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<FileHistoryEntry>, AppError> {
    let conn = state.get_conn()?;
    let limit = limit.unwrap_or(100).clamp(1, 500);

    let sql = match session_id.as_deref() {
        Some(_) => "SELECT id, session_id, file_path, file_existed, created_at
                    FROM file_history WHERE session_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
        None => "SELECT id, session_id, file_path, file_existed, created_at
                 FROM file_history ORDER BY created_at DESC, id DESC LIMIT ?1",
    };

    let mut stmt = conn.prepare(sql)?;

    let entries = match session_id.as_deref() {
        Some(sid) => stmt.query_map(params![sid, limit], row_to_entry)?
            .collect::<Result<Vec<_>, _>>()?,
        None => stmt.query_map(params![limit], row_to_entry)?
            .collect::<Result<Vec<_>, _>>()?,
    };
    Ok(entries)
}

#[tauri::command]
pub fn undo_file_history(state: State<'_, crate::DbState>, history_id: i64) -> Result<String, AppError> {
    let conn = state.get_conn()?;

    let (file_path, backup_content, file_existed) = conn
        .query_row(
            "SELECT file_path, backup_content, file_existed FROM file_history WHERE id = ?1",
            params![history_id],
            |row| {
                let path: String = row.get(0)?;
                let backup: Option<String> = row.get(1)?;
                let existed: i64 = row.get(2)?;
                Ok((path, backup, existed != 0))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::Config("未找到该历史记录".to_string()))?;

    if file_existed {
        std::fs::write(&file_path, backup_content.unwrap_or_default())
            .map_err(|e| AppError::Io(format!("恢复文件失败 ({}): {}", file_path, e)))?;
    } else {
        // 该文件在修改前不存在，撤销即删除
        match std::fs::remove_file(&file_path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::Io(format!("删除文件失败 ({}): {}", file_path, e))),
        }
    }

    // 撤销后删除该条历史记录
    conn.execute("DELETE FROM file_history WHERE id = ?1", params![history_id])?;

    Ok(format!("已撤销对文件的修改: {}", file_path))
}
