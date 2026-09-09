use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use rusqlite::OptionalExtension;
use serde_json::Value;
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

/// 角色 → 会话事件 kind + 模型可见性（system 不进模型上下文，user/assistant/tool 可见）。
fn role_event_kind(role: &str) -> (&'static str, bool) {
    match role {
        "user" => ("user/message", true),
        "assistant" => ("assistant/message", true),
        "tool" => ("tool/result", true),
        _ => ("system/message", false),
    }
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_string())
}

/// 从会话事件 payload 反解 Message。payload 键为 camelCase；attachments 兼容
/// JSON 字符串（历史回放/存量）与数组两种形态。
fn message_from_payload(raw: &str) -> Result<Message, AppError> {
    let v: Value = serde_json::from_str(raw)?;
    let attachments = match &v["attachments"] {
        Value::String(s) => serde_json::from_str::<Vec<Attachment>>(s).ok(),
        Value::Array(_) => serde_json::from_value(v["attachments"].clone()).ok(),
        _ => None,
    };
    Ok(Message {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        session_id: v["sessionId"].as_str().unwrap_or_default().to_string(),
        role: v["role"].as_str().unwrap_or_default().to_string(),
        content: v["content"].as_str().unwrap_or_default().to_string(),
        mode: v["mode"].as_str().unwrap_or_default().to_string(),
        timestamp: v["timestamp"].as_i64().unwrap_or(0),
        tool_calls: opt_str(&v["toolCalls"]),
        tool_call_id: opt_str(&v["toolCallId"]),
        tool_name: opt_str(&v["toolName"]),
        attachments,
    })
}

/// 构建会话消息事件 payload（与 `message_from_payload` 键一一对应）。
fn build_msg_payload(
    id: &str,
    session_id: &str,
    role: &str,
    content: &str,
    mode: &str,
    timestamp: i64,
    tool_calls: &Option<String>,
    tool_call_id: &Option<String>,
    tool_name: &Option<String>,
    attachments: &str,
) -> Value {
    serde_json::json!({
        "id": id,
        "sessionId": session_id,
        "role": role,
        "content": content,
        "mode": mode,
        "timestamp": timestamp,
        "toolCalls": tool_calls,
        "toolCallId": tool_call_id,
        "toolName": tool_name,
        "attachments": attachments,
    })
}

/// 从 `session_events` 派生会话全部消息：同 id 多事件（创建 + 编辑）取最新一条，
/// 按原 timestamp 升序返回（编辑保留原 timestamp 以维持消息位置）。
/// 仅取消息类事件：summary/result（滚动摘要）与 todo/state（todo_write 状态快照，
/// 非模型可见，跨轮任务列表改由 lib.rs 读 latest_session_todos 注入）不入消息投影。
fn load_session_messages(conn: &Connection, session_id: &str) -> Result<Vec<Message>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM session_events
         WHERE session_id = ?1 AND kind NOT IN ('summary/result', 'todo/state')
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![session_id], |r| r.get::<_, String>(0))?;
    let mut by_id: BTreeMap<String, Message> = BTreeMap::new();
    for row in rows {
        let msg = message_from_payload(&row?)?;
        by_id.insert(msg.id.clone(), msg);
    }
    let mut out: Vec<Message> = by_id.into_values().collect();
    out.sort_by_key(|m| m.timestamp);
    Ok(out)
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
    temperature: Option<f64>,
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

    let offset = offset.unwrap_or(0).max(0) as usize;
    let limit = limit.unwrap_or(100).max(0) as usize;

    let all = load_session_messages(&conn, &session_id)?;
    Ok(all.into_iter().skip(offset).take(limit).collect())
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

    // 会话消息事件化（唯一写入口）：消息只追加 session_events，不再写 messages 旧表。
    // kind 按角色归类；model_visible 仅对会进入模型上下文的 user/assistant/tool 记 true。
    let (ev_kind, model_visible) = role_event_kind(&role);
    let payload = build_msg_payload(
        &id,
        &session_id,
        &role,
        &content,
        &mode,
        now,
        &tool_calls,
        &tool_call_id,
        &tool_name,
        attachments_json.as_deref().unwrap_or("[]"),
    );
    crate::eventlog::append_session_event(&conn, &session_id, ev_kind, &payload, model_visible)?;
    
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
    // 事件/用量无外键级联，删除会话时一并显式清理；滚动摘要存于 summary/result 事件中随之删除。
    conn.execute("DELETE FROM session_events WHERE session_id = ?1", params![session_id])?;
    conn.execute("DELETE FROM api_usage_log WHERE session_id = ?1", params![session_id])?;

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
    load_session_messages(conn, session_id)
}

pub fn get_session_inner(conn: &Connection, session_id: &str) -> Result<Option<Session>, AppError> {
    // sessions 为 v1 终态列，直接查询含 agent_session_id / temperature / max_tokens。
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
    conn.execute("DELETE FROM session_events WHERE session_id = ?1", params![session_id])?;
    conn.execute("DELETE FROM api_usage_log WHERE session_id = ?1", params![session_id])?;
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

/// 更新会话的工作目录（项目根）：对后续消息生效（每次发消息动态读取 cwd）。
#[tauri::command]
pub fn update_session_cwd(
    state: State<'_, DbState>,
    session_id: String,
    cwd: String,
) -> Result<(), AppError> {
    let cwd = cwd.trim().to_string();
    let conn = state.get_conn()?;
    conn.execute(
        "UPDATE sessions SET cwd = ?1, updated_at = ?2 WHERE id = ?3",
        params![cwd, crate::utils::now(), session_id],
    )?;
    Ok(())
}

/// 从 `session_events` 取指定消息的最新快照（事件为唯一事实源，按 seq 取最新）。
fn session_message_by_id(conn: &Connection, message_id: &str) -> Result<Option<Message>, AppError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT payload FROM session_events WHERE json_extract(payload, '$.id') = ?1 ORDER BY seq DESC LIMIT 1",
            params![message_id],
            |r| r.get(0),
        )
        .optional()?;
    match raw {
        Some(r) => message_from_payload(&r).map(Some),
        None => Ok(None),
    }
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

    // 编辑 = 追加一条同 id 的事件（保留原 timestamp 以维持消息位置），投影取最新。
    let Some(mut msg) = session_message_by_id(&conn, &message_id)? else {
        return Err(AppError::Db(format!("消息不存在: {}", message_id)));
    };
    msg.content = content;

    let attachments_json = msg
        .attachments
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));
    let (ev_kind, model_visible) = role_event_kind(&msg.role);
    let payload = build_msg_payload(
        &msg.id,
        &msg.session_id,
        &msg.role,
        &msg.content,
        &msg.mode,
        msg.timestamp,
        &msg.tool_calls,
        &msg.tool_call_id,
        &msg.tool_name,
        attachments_json.as_deref().unwrap_or("[]"),
    );
    crate::eventlog::append_session_event(&conn, &msg.session_id, ev_kind, &payload, model_visible)?;

    // Update session timestamp
    let now = crate::utils::now();
    conn.execute(
        "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
        params![now, msg.session_id],
    )?;

    Ok(msg)
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
    let limit = limit.unwrap_or(50) as usize;
    let pattern = query.to_string();

    // 事件为唯一事实源：在派生消息上做内容过滤（数据量小，逐会话扫描足够）。
    let mut hits: Vec<Message> = Vec::new();
    match &session_id {
        Some(sid) => {
            for m in load_session_messages(&conn, sid)? {
                if m.content.contains(&pattern) {
                    hits.push(m);
                }
            }
        }
        None => {
            let mut stmt = conn.prepare("SELECT id FROM sessions")?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for sid in ids {
                for m in load_session_messages(&conn, &sid)? {
                    if m.content.contains(&pattern) {
                        hits.push(m);
                    }
                }
            }
        }
    }
    hits.sort_by_key(|m| m.timestamp);
    hits.truncate(limit);
    Ok(hits)
}

