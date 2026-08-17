use rusqlite::{params, Connection};
use rusqlite::OptionalExtension;
use crate::db::models::{Session, Message, Attachment};
use crate::utils::errors::AppError;
use tauri::State;

use crate::DbState;

/// 会话消息正文落库前的归一化：把连续空行折叠为单个空行，并去除首尾空白。
/// 保留单个空行以维持 Markdown 的段落/代码块结构，仅消除冗余的连续空行。
fn normalize_message_content(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut pending_blank = false;
    let mut started = false;

    for line in content.lines() {
        if line.trim().is_empty() {
            if started {
                pending_blank = true;
            }
        } else {
            if pending_blank {
                out.push('\n');
                pending_blank = false;
            }
            out.push_str(line);
            out.push('\n');
            started = true;
        }
    }

    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// 工具调用链 JSON（ThinkingChainStep[]）落库前的归一化：对其中的文本字段
/// （content/result/fileDiff）逐项折叠空行。工具输出、文件 diff、推理文本可能含
/// 连续空行，若不处理，前端 whitespace-pre-wrap 直出时会原样显示多空行。
fn normalize_tool_calls(tool_calls: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(tool_calls) {
        if let Some(items) = value.as_array_mut() {
            for item in items.iter_mut() {
                if let Some(obj) = item.as_object_mut() {
                    for key in ["content", "result", "fileDiff"] {
                        if let Some(serde_json::Value::String(s)) = obj.get_mut(key) {
                            let normalized = normalize_message_content(s);
                            *s = normalized;
                        }
                    }
                }
            }
        }
        if let Ok(serialized) = serde_json::to_string(&value) {
            return serialized;
        }
    }
    tool_calls.to_string()
}

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        agent_type: row.get("agent_type")?,
        title: row.get("title")?,
        cwd: row.get("cwd")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        last_message_preview: row.get("last_message_preview")?,
        message_count: row.get("message_count")?,
        status: row.get("status")?,
        api_provider: row.get("api_provider").ok(),
        api_model: row.get("api_model").ok(),
        agent_session_id: row.get("agent_session_id").ok(),
        temperature: row.get("temperature").ok(),
        max_tokens: row.get("max_tokens").ok(),
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
        tool_calls: row.get(6).ok(),
        tool_call_id: row.get(7).ok(),
        tool_name: row.get(8).ok(),
        attachments: row
            .get::<_, Option<String>>(9)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Vec<Attachment>>(&s).ok()),
    })
}

#[tauri::command]
pub fn list_sessions(state: State<'_, DbState>) -> Result<Vec<Session>, AppError> {
    let conn = state.get_conn()?;
    
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id, temperature, max_tokens
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
    agent_session_id: Option<String>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> Result<Session, AppError> {
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    let title = title.unwrap_or_default();
    let cwd = cwd.unwrap_or_default();
    
    let conn = state.get_conn()?;
    
    conn.execute(
        "INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at, api_provider, api_model, agent_session_id, temperature, max_tokens) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![id, agent_type, title, cwd, now, now, api_provider, api_model, agent_session_id, temperature, max_tokens],
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
        temperature,
        max_tokens,
    })
}

#[tauri::command]
pub fn get_session(state: State<'_, DbState>, session_id: String) -> Result<Session, AppError> {
    let conn = state.get_conn()?;
    
    let session = conn.query_row(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id, temperature, max_tokens
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
        "SELECT id, session_id, role, content, mode, timestamp, tool_calls, tool_call_id, tool_name, attachments 
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
    tool_calls: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    attachments: Option<Vec<Attachment>>,
) -> Result<Message, AppError> {
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    let content = normalize_message_content(&content);
    let tool_calls = tool_calls.map(|s| normalize_tool_calls(&s));

    let conn = state.get_conn()?;
    
    let attachments_json = attachments
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));
    
    conn.execute(
        "INSERT INTO messages (id, session_id, role, content, mode, timestamp, tool_calls, tool_call_id, tool_name, attachments) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![id, session_id, role, content, mode, now, tool_calls, tool_call_id, tool_name, attachments_json],
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
        tool_calls,
        tool_call_id,
        tool_name,
        attachments,
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

/// 删除会话前清理其附件落盘目录（尽力而为，失败不阻断删除）
fn cleanup_session_attachments(conn: &Connection, session_id: &str) {
    let cwd: Option<String> = conn
        .query_row(
            "SELECT cwd FROM sessions WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let dir = crate::utils::paths::resolve_attachments_dir(conn, cwd.as_deref(), session_id);
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            log::warn!("[session] 清理附件目录失败: {:?}, error={}", dir, e);
        }
    }
}

#[tauri::command]
pub fn delete_session(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    
    cleanup_session_attachments(&conn, &session_id);
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    // CASCADE will delete related messages
    
    Ok(())
}

// ──────────────────────────────────────────────
//  内部函数（可被其他模块调用）
// ──────────────────────────────────────────────

pub fn list_sessions_inner(conn: &Connection) -> Result<Vec<Session>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id, temperature, max_tokens
         FROM sessions WHERE status = 'active' ORDER BY updated_at DESC"
    )?;
    
    let sessions = stmt.query_map([], row_to_session)?
        .collect::<Result<Vec<_>, _>>()?;
    
    Ok(sessions)
}

pub fn get_session_messages_inner(conn: &Connection, session_id: &str) -> Result<Vec<Message>, AppError> {
    // 优先尝试包含扩展列的查询（migration v6+）
    let sql = "SELECT id, session_id, role, content, mode, timestamp, tool_calls, tool_call_id, tool_name, attachments 
         FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC";
    match conn.prepare(sql) {
        Ok(mut stmt) => {
            let messages = stmt.query_map(params![session_id], row_to_message)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(messages)
        }
        Err(_) => {
            // 扩展列不存在，回退到基础列查询
            let mut stmt = conn.prepare(
                "SELECT id, session_id, role, content, mode, timestamp
                 FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC"
            )?;
            fn row_to_message_v0(row: &rusqlite::Row) -> rusqlite::Result<Message> {
                Ok(Message {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    mode: row.get(4)?,
                    timestamp: row.get(5)?,
                    tool_calls: None,
                    tool_call_id: None,
                    tool_name: None,
                    attachments: None,
                })
            }
            let messages = stmt.query_map(params![session_id], row_to_message_v0)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(messages)
        }
    }
}

pub fn get_session_inner(conn: &Connection, session_id: &str) -> Result<Option<Session>, AppError> {
    // 优先尝试包含 agent_session_id 的查询（migration v7+）
    let sql = "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id, temperature, max_tokens
         FROM sessions WHERE id = ?1";
    match conn.prepare(sql) {
        Ok(mut stmt) => {
            match stmt.query_row(params![session_id], row_to_session).optional() {
                Ok(session) => return Ok(session),
                Err(e) => return Err(AppError::Db(format!("查询会话失败: {}", e))),
            }
        }
        Err(_) => {
            // agent_session_id 列不存在，回退到不含该列的查询
            let mut stmt = conn.prepare(
                "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model
                 FROM sessions WHERE id = ?1"
            )?;
            // 使用临时 row mapper（少了第 11 列 agent_session_id）
            fn row_to_session_v1(row: &rusqlite::Row) -> rusqlite::Result<Session> {
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
                    agent_session_id: None,
                    temperature: None,
                    max_tokens: None,
                })
            }
            let session = stmt.query_row(params![session_id], row_to_session_v1).optional()?;
            Ok(session)
        }
    }
}

pub fn delete_session_inner(conn: &Connection, session_id: &str) -> Result<(), AppError> {
    cleanup_session_attachments(conn, session_id);
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
    let content = normalize_message_content(&content);
    let conn = state.get_conn()?;

    // Get existing message
    let msg = conn.query_row(
        "SELECT id, session_id, role, content, mode, timestamp, tool_calls, tool_call_id, tool_name, attachments
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
        Some(_) => "SELECT id, session_id, role, content, mode, timestamp, tool_calls, tool_call_id, tool_name, attachments
                    FROM messages WHERE session_id = ?1 AND content LIKE ?2
                    ORDER BY timestamp ASC LIMIT ?3",
        None => "SELECT id, session_id, role, content, mode, timestamp, tool_calls, tool_call_id, tool_name, attachments
                 FROM messages WHERE content LIKE ?1
                 ORDER BY timestamp ASC LIMIT ?2",
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

