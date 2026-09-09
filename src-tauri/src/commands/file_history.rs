//! Agent 文件修改历史与撤销
//!
//! write_file / edit_file 在覆盖文件前会记录旧内容快照，供用户撤销到上一个版本。

use rusqlite::{params, OptionalExtension};
use rusqlite::types::ToSql;
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

/// 历史分页结果（总数 + 当前页条目）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryPage {
    pub total: i64,
    pub items: Vec<FileHistoryEntry>,
}

/// 构造查询条件（session_id / path_keyword），返回 (WHERE 子句, 动态参数)。
fn build_filters(session_id: &Option<String>, path_keyword: &Option<String>) -> (String, Vec<Box<dyn ToSql>>) {
    let mut conds: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(sid) = session_id.as_deref() {
        if !sid.is_empty() {
            conds.push("session_id = ?".to_string());
            args.push(Box::new(sid.to_string()));
        }
    }
    if let Some(kw) = path_keyword.as_deref() {
        let kw = kw.trim();
        if !kw.is_empty() {
            conds.push("file_path LIKE ?".to_string());
            args.push(Box::new(format!("%{}%", kw)));
        }
    }
    let where_sql = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    (where_sql, args)
}

#[tauri::command]
pub fn list_file_history(
    state: State<'_, crate::DbState>,
    session_id: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    path_keyword: Option<String>,
) -> Result<FileHistoryPage, AppError> {
    let conn = state.get_conn()?;
    let limit = limit.unwrap_or(100).clamp(1, 500);
    let offset = offset.unwrap_or(0).max(0);

    let (where_sql, filter_args) = build_filters(&session_id, &path_keyword);
    let params_ref: Vec<&dyn ToSql> = filter_args.iter().map(|b| b.as_ref()).collect();

    // 总数（供前端展示/触底判断）
    let total = {
        let sql = format!("SELECT COUNT(*) FROM file_history{}", where_sql);
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_row(params_ref.as_slice(), |r| r.get::<_, i64>(0))?
    };

    // 当前页条目
    let items = {
        let mut item_args = filter_args;
        item_args.push(Box::new(limit));
        item_args.push(Box::new(offset));
        let item_params: Vec<&dyn ToSql> = item_args.iter().map(|b| b.as_ref()).collect();
        let sql = format!(
            "SELECT id, session_id, file_path, file_existed, created_at
             FROM file_history{} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
            where_sql
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(item_params.as_slice(), row_to_entry)?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    Ok(FileHistoryPage { total, items })
}

/// 清理文件修改历史：scope = "one"（按 history_id）/"session"（按 session_id）/"all"（清空全部）。
/// 返回删除的记录数。
#[tauri::command]
pub fn delete_file_history(
    state: State<'_, crate::DbState>,
    scope: String,
    history_id: Option<i64>,
    session_id: Option<String>,
) -> Result<i64, AppError> {
    let conn = state.get_conn()?;
    let deleted = match scope.as_str() {
        "one" => {
            let id = history_id.ok_or_else(|| AppError::Config("缺少 history_id".to_string()))?;
            conn.execute("DELETE FROM file_history WHERE id = ?1", params![id])?
        }
        "session" => {
            let sid = session_id.ok_or_else(|| AppError::Config("缺少 session_id".to_string()))?;
            conn.execute("DELETE FROM file_history WHERE session_id = ?1", params![sid])?
        }
        "all" => conn.execute("DELETE FROM file_history", [])?,
        other => return Err(AppError::Config(format!("未知清理范围: {}", other))),
    };
    Ok(deleted as i64)
}

/// 历史会话项（区分会话/群聊房间，供前端筛选下拉展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistorySession {
    pub id: String,
    /// "session"（正式会话）/"room"（群聊房间）。
    pub kind: String,
}

/// 列出所有出现过文件修改历史的会话/房间 id（按最近修改时间倒序，供前端筛选下拉）。
/// 通过 `sessions` 表区分：存在记录为会话，否则为群聊房间（room_id 不落 sessions 表）。
#[tauri::command]
pub fn list_file_history_sessions(state: State<'_, crate::DbState>) -> Result<Vec<FileHistorySession>, AppError> {
    let conn = state.get_conn()?;
    let mut stmt = conn.prepare(
        "SELECT fh.session_id, EXISTS(SELECT 1 FROM sessions s WHERE s.id = fh.session_id) AS is_session
         FROM file_history fh GROUP BY fh.session_id ORDER BY MAX(fh.created_at) DESC",
    )?;
    let sessions = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let is_session: bool = row.get(1)?;
            Ok(FileHistorySession {
                id,
                kind: if is_session { "session".to_string() } else { "room".to_string() },
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(sessions)
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
