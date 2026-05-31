use rusqlite::{params, Connection};
use crate::db::models::{Session, Message};
use crate::utils::errors::AppError;
use std::sync::Mutex;
use tauri::State;

use crate::DbState;

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get(0)?,
        agent_type: row.get(1)?,
        title: row.get(2)?,
        cwd: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        last_message_preview: row.get(6)?,
        message_count: row.get(7)?,
        status: row.get(8)?,
        api_provider: row.get(9).ok(),
        api_model: row.get(10).ok(),
    })
}

fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        session_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        mode: row.get(4)?,
        timestamp: row.get(5)?,
    })
}

#[tauri::command]
pub fn list_sessions(state: State<'_, DbState>) -> Result<Vec<Session>, AppError> {
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model 
         FROM sessions WHERE status = 'active' ORDER BY updated_at DESC"
    )?;
    
    let sessions = stmt.query_map([], row_to_session)?
        .collect::<Result<Vec<_>, _>>()?;
    
    Ok(sessions)
}

#[tauri::command]
pub fn list_archived_sessions(state: State<'_, DbState>) -> Result<Vec<Session>, AppError> {
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model 
         FROM sessions WHERE status = 'archived' ORDER BY updated_at DESC"
    )?;
    
    let sessions = stmt.query_map([], row_to_session)?
        .collect::<Result<Vec<_>, _>>()?;
    
    Ok(sessions)
}

#[tauri::command]
pub fn create_session(
    state: State<'_, DbState>,
    agent_type: String,
    cwd: Option<String>,
    title: Option<String>,
    api_provider: Option<String>,
    api_model: Option<String>,
) -> Result<Session, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let title = title.unwrap_or_else(|| {
        match agent_type.as_str() {
            "claude" => "Claude Code 新会话".into(),
            "hermes" => "Hermes Agent 新会话".into(),
            "api" => "API 直连会话".into(),
            _ => "新会话".into(),
        }
    });
    let cwd = cwd.unwrap_or_default();
    
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    conn.execute(
        "INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at, api_provider, api_model) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, agent_type, title, cwd, now, now, api_provider, api_model],
    )?;
    
    Ok(Session {
        id,
        agent_type,
        title,
        cwd,
        created_at: now,
        updated_at: now,
        last_message_preview: String::new(),
        message_count: 0,
        status: "active".into(),
        api_provider,
        api_model,
    })
}

#[tauri::command]
pub fn get_session(state: State<'_, DbState>, session_id: String) -> Result<Session, AppError> {
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    let session = conn.query_row(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model 
         FROM sessions WHERE id = ?1",
        params![session_id],
        row_to_session,
    )?;
    
    Ok(session)
}

#[tauri::command]
pub fn get_session_messages(
    state: State<'_, DbState>,
    session_id: String,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<Message>, AppError> {
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(100);
    
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, content, mode, timestamp 
         FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC LIMIT ?2 OFFSET ?3"
    )?;
    
    let messages = stmt.query_map(params![session_id, limit, offset], row_to_message)?
        .collect::<Result<Vec<_>, _>>()?;
    
    Ok(messages)
}

/// Save a message to the database and update the session's last_message_preview and message_count.
#[tauri::command]
pub fn save_message(
    state: State<'_, DbState>,
    session_id: String,
    role: String,
    content: String,
    mode: String,
) -> Result<Message, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    conn.execute(
        "INSERT INTO messages (id, session_id, role, content, mode, timestamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, session_id, role, content, mode, now],
    )?;
    
    // Update session preview and count
    let preview = if content.len() > 100 {
        format!("{}...", &content[..100])
    } else {
        content.clone()
    };
    
    conn.execute(
        "UPDATE sessions SET last_message_preview = ?1, message_count = message_count + 1, updated_at = ?2 WHERE id = ?3",
        params![preview, now, session_id],
    )?;
    
    Ok(Message {
        id,
        session_id,
        role,
        content,
        mode,
        timestamp: now,
    })
}

#[tauri::command]
pub fn rename_session(
    state: State<'_, DbState>,
    session_id: String,
    new_title: String,
) -> Result<(), AppError> {
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![new_title, now, session_id],
    )?;
    
    Ok(())
}

#[tauri::command]
pub fn archive_session(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<(), AppError> {
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE sessions SET status = 'archived', updated_at = ?1 WHERE id = ?2",
        params![now, session_id],
    )?;
    
    Ok(())
}

#[tauri::command]
pub fn delete_session(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<(), AppError> {
    let conn = state.conn.lock().map_err(|e| AppError {
        code: "ERR_LOCK".into(),
        message: "获取数据库锁失败".into(),
        details: Some(e.to_string()),
    })?;
    
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    // CASCADE will delete related messages
    
    Ok(())
}
