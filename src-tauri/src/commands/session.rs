use rusqlite::{params, Connection};
use crate::db::models::{Session, Message};
use crate::utils::errors::AppError;
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
        agent_session_id: row.get(11).ok(),
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
        reasoning_content: row.get(6).ok(),
        tool_calls: row.get(7).ok(),
        tool_call_id: row.get(8).ok(),
        tool_name: row.get(9).ok(),
    })
}

#[tauri::command]
pub fn list_sessions(state: State<'_, DbState>) -> Result<Vec<Session>, AppError> {
    let conn = state.get_conn()?;
    
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id
         FROM sessions WHERE status = 'active' ORDER BY updated_at DESC"
    )?;
    
    let sessions = stmt.query_map([], row_to_session)?
        .collect::<Result<Vec<_>, _>>()?;
    
    Ok(sessions)
}

#[tauri::command]
pub fn list_archived_sessions(state: State<'_, DbState>) -> Result<Vec<Session>, AppError> {
    let conn = state.get_conn()?;
    
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id
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
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    let title = title.unwrap_or_default();
    let cwd = cwd.unwrap_or_default();
    
    let conn = state.get_conn()?;
    
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
        agent_session_id: None,
    })
}

#[tauri::command]
pub fn get_session(state: State<'_, DbState>, session_id: String) -> Result<Session, AppError> {
    let conn = state.get_conn()?;
    
    let session = conn.query_row(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id
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
    let conn = state.get_conn()?;
    
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(100);
    
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, content, mode, timestamp, reasoning_content, tool_calls, tool_call_id, tool_name 
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
    reasoning_content: Option<String>,
    tool_calls: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
) -> Result<Message, AppError> {
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    
    let conn = state.get_conn()?;
    
    conn.execute(
        "INSERT INTO messages (id, session_id, role, content, mode, timestamp, reasoning_content, tool_calls, tool_call_id, tool_name) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![id, session_id, role, content, mode, now, reasoning_content, tool_calls, tool_call_id, tool_name],
    )?;
    
    // Update session preview and count (UTF-8 safe truncation)
    let preview = if content.chars().count() > 100 {
        format!("{}...", content.chars().take(100).collect::<String>())
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
        reasoning_content,
        tool_calls,
        tool_call_id,
        tool_name,
    })
}

#[tauri::command]
pub fn rename_session(
    state: State<'_, DbState>,
    session_id: String,
    new_title: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    
    let now = crate::utils::now();
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
    let conn = state.get_conn()?;
    
    let now = crate::utils::now();
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
    let conn = state.get_conn()?;
    
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    // CASCADE will delete related messages
    
    Ok(())
}

// ──────────────────────────────────────────────
//  内部函数（可被其他模块调用）
// ──────────────────────────────────────────────

pub fn list_sessions_inner(conn: &Connection) -> Result<Vec<Session>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id
         FROM sessions WHERE status = 'active' ORDER BY updated_at DESC"
    )?;
    
    let sessions = stmt.query_map([], row_to_session)?
        .collect::<Result<Vec<_>, _>>()?;
    
    Ok(sessions)
}

pub fn get_session_messages_inner(conn: &Connection, session_id: &str) -> Result<Vec<Message>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, content, mode, timestamp, reasoning_content, tool_calls, tool_call_id, tool_name 
         FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC"
    )?;
    
    let messages = stmt.query_map(params![session_id], row_to_message)?
        .collect::<Result<Vec<_>, _>>()?;
    
    Ok(messages)
}

pub fn delete_session_inner(conn: &Connection, session_id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    Ok(())
}

/// Update the agent_session_id for a session
#[tauri::command]
pub fn update_session_agent_id(
    state: State<'_, DbState>,
    session_id: String,
    agent_session_id: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    conn.execute(
        "UPDATE sessions SET agent_session_id = ?1 WHERE id = ?2",
        params![agent_session_id, session_id],
    )?;
    Ok(())
}

/// Update an existing message's content
#[tauri::command]
pub fn update_message(
    state: State<'_, DbState>,
    message_id: String,
    content: String,
) -> Result<Message, AppError> {
    let conn = state.get_conn()?;

    // Get existing message
    let msg = conn.query_row(
        "SELECT id, session_id, role, content, mode, timestamp, reasoning_content, tool_calls, tool_call_id, tool_name
         FROM messages WHERE id = ?1",
        params![message_id],
        row_to_message,
    )?;

    // Update content
    let now = crate::utils::now();
    conn.execute(
        "UPDATE messages SET content = ?1 WHERE id = ?2",
        params![content, message_id],
    )?;

    // Update session timestamp
    conn.execute(
        "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
        params![now, msg.session_id],
    )?;

    Ok(Message {
        content,
        ..msg
    })
}

/// Search sessions by title (fuzzy match)
#[tauri::command]
pub fn search_sessions(
    state: State<'_, DbState>,
    query: String,
) -> Result<Vec<Session>, AppError> {
    let conn = state.get_conn()?;

    let pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id
         FROM sessions WHERE status = 'active' AND title LIKE ?1 ORDER BY updated_at DESC LIMIT 50"
    )?;

    let sessions = stmt.query_map(params![pattern], row_to_session)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(sessions)
}

/// Search messages by content (LIKE fuzzy match)
#[tauri::command]
pub fn search_messages(
    state: State<'_, DbState>,
    session_id: Option<String>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<Message>, AppError> {
    let conn = state.get_conn()?;
    let limit = limit.unwrap_or(50) as i64;
    let pattern = format!("%{}%", query);

    let sql = match session_id {
        Some(_) => "SELECT id, session_id, role, content, mode, timestamp, reasoning_content, tool_calls, tool_call_id, tool_name
                    FROM messages WHERE session_id = ?1 AND content LIKE ?2
                    ORDER BY timestamp DESC LIMIT ?3",
        None => "SELECT id, session_id, role, content, mode, timestamp, reasoning_content, tool_calls, tool_call_id, tool_name
                 FROM messages WHERE content LIKE ?1
                 ORDER BY timestamp DESC LIMIT ?2",
    };

    let mut stmt = conn.prepare(sql)?;

    let messages = match &session_id {
        Some(sid) => stmt.query_map(params![sid, pattern, limit], row_to_message)?
            .collect::<Result<Vec<_>, _>>()?,
        None => stmt.query_map(params![pattern, limit], row_to_message)?
            .collect::<Result<Vec<_>, _>>()?,
    };

    Ok(messages)
}

