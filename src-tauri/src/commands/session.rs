use std::collections::BTreeMap;

use crate::db::models::{Attachment, Message, Session};
use crate::utils::errors::AppError;
use rusqlite::OptionalExtension;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::DbState;

/// 会话消息正文落库前的归一化：把连续空行折叠为单个空行，并去除首尾空白。
/// 保留单个空行以维持 Markdown 的段落/代码块结构，仅消除冗余的连续空行。
///
/// 同一份模型正文在工作流 Agent 节点里也会作为节点执行结果保存
/// （见 [`crate::workflow::executors::agent_executor`]），两侧必须同口径：
/// 否则会话里看不到的多余空行，会在节点执行结果里冒出来。
pub(crate) fn normalize_message_content(content: &str) -> String {
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
        origin: row.get("origin").ok(),
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
pub(crate) fn load_session_messages(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<Message>, AppError> {
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
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id, origin, temperature, max_tokens
         FROM sessions WHERE status = 'active' ORDER BY updated_at DESC"
    )?;

    let sessions = stmt
        .query_map([], row_to_session)?
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

    let sessions = stmt
        .query_map([], row_to_session)?
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
    let conn = state.get_conn()?;
    create_session_inner(
        &conn,
        agent_type,
        cwd,
        title,
        api_provider,
        api_model,
        agent_session_id,
        temperature,
        max_tokens,
        None,
    )
}

/// 会话行写入（`create_session` 命令与工作流 Agent 节点共用）。
/// `agent_session_id` 仅作为入参保留签名对称，落库后仍由首次运行生成（见 lib.rs run_api_agent_inner）。
/// `origin` 标记会话来源：工作流节点传 `Some("workflow:{工作流定义id}")`（derive 失败回落
/// `Some("workflow")`），用户会话传 `None`；判据一律按前缀 `workflow` 匹配。
pub fn create_session_inner(
    conn: &Connection,
    agent_type: String,
    cwd: Option<String>,
    title: Option<String>,
    api_provider: Option<String>,
    api_model: Option<String>,
    agent_session_id: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    origin: Option<String>,
) -> Result<Session, AppError> {
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    let title = title.unwrap_or_default();
    let cwd = cwd.unwrap_or_default();

    conn.execute(
        "INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at, api_provider, api_model, agent_session_id, temperature, max_tokens, origin) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![id, agent_type, title, cwd, now, now, api_provider, api_model, agent_session_id, temperature, max_tokens, origin],
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
        origin,
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
    let conn = state.get_conn()?;
    save_message_inner(
        &conn,
        &session_id,
        &role,
        &content,
        &mode,
        tool_calls,
        tool_call_id,
        tool_name,
        attachments,
    )
}

/// 会话消息写入（`save_message` 命令与后端自行落库的路径共用）。
/// 会话模式由前端 `save_message` 落库；工作流 Agent 节点没有前端调用方，
/// 由后端以同一 payload 形态落库（见 lib.rs run_api_agent_inner 的 persist_turn）。
pub fn save_message_inner(
    conn: &Connection,
    session_id: &str,
    role: &str,
    content: &str,
    mode: &str,
    tool_calls: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    attachments: Option<Vec<Attachment>>,
) -> Result<Message, AppError> {
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    let content = normalize_message_content(content);
    let tool_calls = tool_calls.map(|s| normalize_tool_calls(&s));

    let attachments_json = attachments
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));

    // 会话消息事件化（唯一写入口）：消息只追加 session_events，不再写 messages 旧表。
    // kind 按角色归类；model_visible 仅对会进入模型上下文的 user/assistant/tool 记 true。
    let (ev_kind, model_visible) = role_event_kind(role);
    let payload = build_msg_payload(
        &id,
        session_id,
        role,
        &content,
        mode,
        now,
        &tool_calls,
        &tool_call_id,
        &tool_name,
        attachments_json.as_deref().unwrap_or("[]"),
    );
    crate::eventlog::append_session_event(conn, session_id, ev_kind, &payload, model_visible)?;

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
        session_id: session_id.to_string(),
        role: role.to_string(),
        content,
        mode: mode.to_string(),
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
pub fn archive_session(state: State<'_, DbState>, session_id: String) -> Result<(), AppError> {
    let conn = state.get_conn()?;

    let now = crate::utils::now();
    conn.execute(
        "UPDATE sessions SET status = 'archived', updated_at = ?1 WHERE id = ?2",
        params![now, session_id],
    )?;

    Ok(())
}

#[tauri::command]
pub fn unarchive_session(state: State<'_, DbState>, session_id: String) -> Result<(), AppError> {
    let conn = state.get_conn()?;

    let now = crate::utils::now();
    conn.execute(
        "UPDATE sessions SET status = 'active', updated_at = ?1 WHERE id = ?2",
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
pub fn delete_session(state: State<'_, DbState>, session_id: String) -> Result<(), AppError> {
    let conn = state.get_conn()?;

    cleanup_session_attachments(&conn, &session_id);
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    // 事件/用量无外键级联，删除会话时一并显式清理；滚动摘要存于 summary/result 事件中随之删除。
    conn.execute(
        "DELETE FROM session_events WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute(
        "DELETE FROM api_usage_log WHERE session_id = ?1",
        params![session_id],
    )?;
    // 自动沉淀的"沉淀水位"是 app_settings 里的 KV（没有外键），同样随会话一起清
    conn.execute(
        "DELETE FROM app_settings WHERE key = ?1",
        params![crate::commands::knowledge::kb_sink_cursor_key(&session_id)],
    )?;

    Ok(())
}

// ──────────────────────────────────────────────
//  内部函数（可被其他模块调用）
// ──────────────────────────────────────────────

pub fn list_sessions_inner(conn: &Connection) -> Result<Vec<Session>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id, origin, temperature, max_tokens
         FROM sessions WHERE status = 'active' ORDER BY updated_at DESC"
    )?;

    let sessions = stmt
        .query_map([], row_to_session)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(sessions)
}

pub fn get_session_messages_inner(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<Message>, AppError> {
    load_session_messages(conn, session_id)
}

pub fn get_session_inner(conn: &Connection, session_id: &str) -> Result<Option<Session>, AppError> {
    // sessions 为 v1 终态列，直接查询含 agent_session_id / temperature / max_tokens。
    let sql = "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id, origin, temperature, max_tokens
         FROM sessions WHERE id = ?1";
    match conn.prepare(sql) {
        Ok(mut stmt) => {
            match stmt
                .query_row(params![session_id], row_to_session)
                .optional()
            {
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
                    origin: None,
                    temperature: None,
                    max_tokens: None,
                })
            }
            let session = stmt
                .query_row(params![session_id], row_to_session_v1)
                .optional()?;
            Ok(session)
        }
    }
}

pub fn delete_session_inner(conn: &Connection, session_id: &str) -> Result<(), AppError> {
    cleanup_session_attachments(conn, session_id);
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    conn.execute(
        "DELETE FROM session_events WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute(
        "DELETE FROM api_usage_log WHERE session_id = ?1",
        params![session_id],
    )?;
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
///
/// **不更新 `updated_at`**：该列只表示"最后一次消息时间"（列表分组、排序、相对时间都据此），
/// 改配置不属于会话活动——否则切一次目录会把老会话顶到列表最前、时间显示成"刚刚"。
#[tauri::command]
pub fn update_session_cwd(
    state: State<'_, DbState>,
    session_id: String,
    cwd: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    update_session_cwd_inner(&conn, &session_id, &cwd)
}

/// `update_session_cwd` 的库层实现（独立出来便于单测，同 `list_sessions_inner`）。
pub fn update_session_cwd_inner(
    conn: &Connection,
    session_id: &str,
    cwd: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE sessions SET cwd = ?1 WHERE id = ?2",
        params![cwd.trim(), session_id],
    )?;
    Ok(())
}

/// 切换会话模型的结果：CAS 未命中时不写入，回传库中当前值供前端对齐提示。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelSwitch {
    /// 是否真的写入了新值（false = 期望值与库中不一致，未覆盖）
    pub applied: bool,
    pub api_provider: String,
    pub api_model: String,
}

/// 切换会话的 API 提供商 / 模型（会话模式）：对**后续**消息生效。
///
/// 运行期每轮从会话行取 provider/model（见 `run_api_agent_body`），所以改这里即改"下一轮用谁"。
/// 比较后再写：`expected_*` 与库中当前值不一致时**不覆盖**，直接回传当前值——用户在别的窗口
/// （工作流节点、群聊房间）也可能改过同一会话，静默覆盖会让对方的改动凭空消失。
///
/// **不更新 `updated_at`**：理由同 `update_session_cwd`（时间只由消息驱动）。
#[tauri::command]
pub fn update_session_model(
    state: State<'_, DbState>,
    session_id: String,
    api_provider: String,
    api_model: String,
    expected_provider: Option<String>,
    expected_model: Option<String>,
) -> Result<SessionModelSwitch, AppError> {
    let conn = state.get_conn()?;
    update_session_model_inner(
        &conn,
        &session_id,
        &api_provider,
        &api_model,
        expected_provider.as_deref().unwrap_or(""),
        expected_model.as_deref().unwrap_or(""),
    )
}

/// `update_session_model` 的库层实现（独立出来便于单测，同 `list_sessions_inner`）。
pub fn update_session_model_inner(
    conn: &Connection,
    session_id: &str,
    api_provider: &str,
    api_model: &str,
    expected_provider: &str,
    expected_model: &str,
) -> Result<SessionModelSwitch, AppError> {
    let api_provider = api_provider.trim().to_string();
    let api_model = api_model.trim().to_string();
    if api_provider.is_empty() || api_model.is_empty() {
        return Err(AppError::InvalidInput("提供商与模型都不能为空".into()));
    }

    let current: Option<(String, String, String)> = conn
        .query_row(
            "SELECT agent_type, COALESCE(api_provider, ''), COALESCE(api_model, '') FROM sessions WHERE id = ?1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (agent_type, cur_provider, cur_model) =
        current.ok_or_else(|| AppError::NotFound(format!("会话不存在: {}", session_id)))?;

    // CLI 会话的"模型"由 agent_type（二进制 + 续聊令牌）决定，改这两列没有运行期含义
    if agent_type != "api" {
        return Err(AppError::InvalidInput(
            "只有 API 会话可以切换模型（CLI 会话请新建会话）".into(),
        ));
    }

    // CAS：期望值与库中不一致说明期间被别处改过，不覆盖，回传当前值
    if cur_provider != expected_provider || cur_model != expected_model {
        return Ok(SessionModelSwitch {
            applied: false,
            api_provider: cur_provider,
            api_model: cur_model,
        });
    }

    // 目标提供商必须存在且已配 Key：否则切换成功但下一条消息必报错（错误信息还离现场很远）
    let provider = crate::commands::api_provider::get_api_provider(conn, &api_provider)?
        .ok_or_else(|| AppError::NotFound(format!("提供商不存在: {}", api_provider)))?;
    if !provider.api_key_set {
        return Err(AppError::Config(format!(
            "提供商「{}」未配置 API Key",
            provider.name
        )));
    }
    if !provider.models.iter().any(|m| m == &api_model) {
        return Err(AppError::InvalidInput(format!(
            "提供商「{}」下没有模型「{}」",
            provider.name, api_model
        )));
    }

    conn.execute(
        "UPDATE sessions SET api_provider = ?1, api_model = ?2 WHERE id = ?3",
        params![api_provider, api_model, session_id],
    )?;

    Ok(SessionModelSwitch {
        applied: true,
        api_provider,
        api_model,
    })
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
        return Err(AppError::NotFound(format!("消息不存在: {}", message_id)));
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
    crate::eventlog::append_session_event(
        &conn,
        &msg.session_id,
        ev_kind,
        &payload,
        model_visible,
    )?;

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
pub fn search_sessions(state: State<'_, DbState>, query: String) -> Result<Vec<Session>, AppError> {
    let conn = state.get_conn()?;

    let pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model, agent_session_id, origin
         FROM sessions WHERE status = 'active' AND title LIKE ?1 ORDER BY updated_at DESC LIMIT 50"
    )?;

    let sessions = stmt
        .query_map(params![pattern], row_to_session)?
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

// ════════════════════════════════════════════════════════════
// 会话 → 工作流（确定性转换）
// ════════════════════════════════════════════════════════════

/// 任务来源（决定从会话的哪一部分提取任务清单）
#[derive(Debug, Clone, Copy, PartialEq)]
enum PlanSource {
    /// 会话计划（todo_write 的 todo/state）
    Todos,
    /// 会话消息（用户发言，每条一个候选）
    Messages,
    /// 前端弹窗里勾选/编辑后的最终清单
    Edited,
}

impl PlanSource {
    fn as_str(self) -> &'static str {
        match self {
            PlanSource::Todos => "todos",
            PlanSource::Messages => "messages",
            PlanSource::Edited => "edited",
        }
    }
}

/// 前端确认后的任务条目（勾选 + 编辑 + 新增后的最终清单）
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTaskInput {
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// 依赖：本次提交数组里的 **1-based 位置**；
    /// `None` = 未提供（按提交顺序串成链，兼容旧前端），`Some(vec![])` = 显式无依赖（可并行）
    #[serde(default)]
    pub deps: Option<Vec<i64>>,
    /// 预览时的默认勾选状态（仅前端回传，后端不读取；保留字段以固定线上格式）
    #[serde(default)]
    #[allow(dead_code)]
    pub suggested: bool,
    /// 确认类任务（需要用户拍板）：转换时生成「人工交互」节点而不是 Agent 节点。
    /// 模型提炼会按需标记，用户也可在弹窗里逐行切换。
    #[serde(default)]
    pub confirm: bool,
    /// 确认项的默认值（仅 confirm 有意义）：无人应答时交互节点按它继续，自动运行不空转。
    #[serde(default)]
    pub default_value: Option<String>,
}

/// 覆盖路径里的单条任务（标题已过滤空行、文本已按字符截断），供依赖换算与节点配置使用。
struct RawPlanTask {
    title: String,
    detail: String,
    /// `None` = 未提供依赖（旧前端），`Some(vec![])` = 显式无依赖
    deps: Option<Vec<i64>>,
    confirm: bool,
    default_value: Option<String>,
}

/// 候选任务：供前端勾选/编辑，并可原样回传（前端清单是唯一事实来源）。
///
/// 确认类标记与默认值必须"带着走"：来源清单里被自动识别出的信息收集类任务，
/// 若在往返中丢掉标记，转换出来又会变成空转的 Agent 节点。
#[derive(Clone)]
struct PlanCandidateItem {
    title: String,
    detail: String,
    /// 默认勾选（false = 被自动忽略的确认/寒暄/追问，用户可勾回来）
    suggested: bool,
    /// 来源分组 todos / messages / edited
    origin: String,
    /// 信息/需求收集类（工作流里落成「人工交互」节点）
    confirm: bool,
    /// 确认项的默认值（无人应答时采用）
    default_value: Option<String>,
}

/// 转换来源的解析结果：最终计划 + 供前端勾选/编辑的候选清单
struct SessionPlanResolution {
    plan: crate::workflow::plan::PlanGraph,
    candidates: Vec<PlanCandidateItem>,
    source: PlanSource,
    /// 默认未勾选（明显不是任务）的候选数，用于前端提示"已忽略 N 条"
    ignored: usize,
    /// 会话里已被用户确认过的需求条数（已并入总目标，运行期无需再问）
    confirmed: usize,
}

/// 用户发言是否默认勾选为任务。
///
/// 只排除"明显不是任务"的（确认、寒暄、催prompt、过短、纯追问）；语义判断交给用户在弹窗里勾选 ——
/// 被排除的候选仍会返回（suggested=false），用户可勾回来，不丢信息。
fn suggest_task_from_message(text: &str) -> bool {
    let t = text.trim();
    if t.chars().count() < 6 {
        return false;
    }
    let norm: String = t
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '。' | '，' | ',' | '.' | '！' | '!' | '?' | '？' | ' ' | '\n' | '\t' | '~' | '～'
            )
        })
        .collect::<String>()
        .to_lowercase();
    const STOP: &[&str] = &[
        "好的",
        "好",
        "行",
        "可以",
        "收到",
        "嗯",
        "是",
        "对",
        "谢谢",
        "多谢",
        "继续",
        "接着",
        "再来",
        "再来一次",
        "再试一次",
        "重试",
        "ok",
        "okay",
        "yes",
        "no",
        "thanks",
        "thankyou",
        "goahead",
        "pleasecontinue",
        "继续吧",
        "接着做",
    ];
    if STOP.contains(&norm.as_str()) {
        return false;
    }
    if norm.starts_with("继续") || norm.starts_with("接着") {
        return false;
    }
    if t.chars().count() < 12 && (t.ends_with('?') || t.ends_with('？')) {
        return false;
    }
    true
}

/// 会话的用户消息 → 任务候选（按时间升序）
fn plan_candidates_from_messages(
    conn: &Connection,
    session: &Session,
) -> Result<Vec<(String, String, bool)>, AppError> {
    let msgs = load_session_messages(conn, &session.id)
        .map_err(|e| AppError::Db(format!("读取会话消息失败: {}", e)))?;
    let mut out = Vec::new();
    for m in msgs.iter().filter(|m| m.role == "user") {
        let text = m.content.trim();
        if text.is_empty() {
            continue;
        }
        let suggested = suggest_task_from_message(text);
        out.push((
            crate::utils::text::head_chars(text, 60),
            crate::utils::text::head_chars(text, 400),
            suggested,
        ));
    }
    Ok(out)
}

/// 来源候选（会话计划 / 用户消息）→ 带确认标记的候选。
///
/// 工作流的 Agent 节点不能与用户交互，所以"收集/确认用户需求"这类任务在候选阶段就定标为确认类，
/// 并同步预填默认值（用户可在弹窗里改写或清空）。群聊端不使用该标记（主持人本来就能向用户确认）。
fn candidate_from_source(
    title: String,
    detail: String,
    suggested: bool,
    origin: &str,
) -> PlanCandidateItem {
    let confirm = looks_like_user_input_task(&title);
    PlanCandidateItem {
        title,
        detail,
        suggested,
        origin: origin.to_string(),
        confirm,
        default_value: confirm.then(|| CONFIRM_DEFAULT_FALLBACK.to_string()),
    }
}

/// 候选 → PlanGraph（依赖按提交顺序串成链；会话计划本身也不含依赖信息）
fn plan_from_candidates(
    session: &Session,
    params: &Value,
    goal: &str,
    source: PlanSource,
    items: &[PlanCandidateItem],
) -> crate::workflow::plan::PlanGraph {
    let mut tasks: Vec<crate::workflow::plan::PlanTask> = Vec::new();
    for (i, c) in items.iter().enumerate() {
        tasks.push(crate::workflow::plan::PlanTask {
            key: format!("t{}", i + 1),
            no: (i + 1) as i64,
            title: if c.title.trim().is_empty() {
                format!("任务 {}", i + 1)
            } else {
                c.title.trim().to_string()
            },
            detail: c.detail.trim().to_string(),
            deps: if i == 0 {
                Vec::new()
            } else {
                vec![format!("t{}", i)]
            },
            params: params.clone(),
            // 信息/需求收集类候选在候选阶段就已定标（见 candidates_from_sources），这里原样带下去
            confirm: c.confirm,
            default_value: c.default_value.clone(),
        });
    }
    let from = match source {
        PlanSource::Todos => "会话计划（todo_write）",
        PlanSource::Messages => "会话消息（用户发言）",
        PlanSource::Edited => "已编辑的任务清单",
    };
    crate::workflow::plan::PlanGraph {
        name: format!("{}（来自会话）", session.title),
        description: format!(
            "由会话「{}」的{}生成（会话 id: {}；任务 {} 个，按顺序串行，依赖可在编辑器中调整）。",
            session.title,
            from,
            session.id,
            tasks.len()
        ),
        goal: goal.to_string(),
        stage_name: "会话任务".to_string(),
        tasks,
    }
}

/// 应用前端回传的名称/描述覆盖（用户可在弹窗里改名与写描述）。
fn apply_plan_naming(
    mut plan: crate::workflow::plan::PlanGraph,
    name: Option<&str>,
    description: Option<&str>,
) -> crate::workflow::plan::PlanGraph {
    if let Some(n) = name {
        let n = n.trim();
        if !n.is_empty() {
            plan.name = n.to_string();
        }
    }
    if let Some(d) = description {
        plan.description = d.to_string();
    }
    plan
}

/// 依赖成环校验（Kahn）：任务 key 与 deps 都为 `t{n}`。
///
/// 工作流侧 `build_definition` 也会查一遍，但群聊侧是直接落库，所以这里统一先挡住。
fn check_no_cycle(tasks: &[crate::workflow::plan::PlanTask]) -> Result<(), AppError> {
    let mut indeg: std::collections::HashMap<&str, usize> = tasks
        .iter()
        .map(|t| (t.key.as_str(), t.deps.len()))
        .collect();
    let mut ready: Vec<&str> = indeg
        .iter()
        .filter(|(_, v)| **v == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut done = 0usize;
    while let Some(k) = ready.pop() {
        done += 1;
        for t in tasks {
            if t.deps.iter().any(|d| d == k) {
                let e = indeg.get_mut(t.key.as_str()).unwrap();
                *e = e.saturating_sub(1);
                if *e == 0 {
                    ready.push(t.key.as_str());
                }
            }
        }
    }
    if done != tasks.len() {
        // 拿不到具体涉及的任务（此处的 key 与 UI 序号不同源），按退化句式。
        return Err(AppError::InvalidInput(
            "任务依赖存在环，请调整依赖后重试".to_string(),
        ));
    }
    Ok(())
}

/// 从消息序列里配对出「ask_user 问答对」（纯函数，便于单测）。
///
/// 工具实现把用户答复作为 tool_result 回流（见 tools/ask_user.rs），所以：
/// 问题取自 assistant 消息里 ask_user 工具调用的 title/prompt，答复取自对应 tool 消息正文。
/// 超时占位文案（工具在无人回复时返回）不算确认。
fn pair_confirmations(msgs: &[Message]) -> Vec<(String, String)> {
    let mut q_by_call: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for m in msgs.iter().filter(|m| m.role == "assistant") {
        let Some(raw) = m.tool_calls.as_deref() else {
            continue;
        };
        let Ok(arr) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let Some(list) = arr.as_array() else { continue };
        for tc in list {
            let name = tc["function"]["name"]
                .as_str()
                .or_else(|| tc["name"].as_str())
                .unwrap_or("");
            if name != "ask_user" {
                continue;
            }
            let args_raw = tc["function"]["arguments"]
                .as_str()
                .or_else(|| tc["arguments"].as_str())
                .unwrap_or("");
            let args: Value = serde_json::from_str(args_raw).unwrap_or(Value::Null);
            let title = args["title"].as_str().unwrap_or("").trim();
            let prompt = args["prompt"].as_str().unwrap_or("").trim();
            let q = if !title.is_empty() {
                title.to_string()
            } else {
                crate::utils::text::head_chars(prompt, 40)
            };
            let id = tc["id"].as_str().unwrap_or("");
            if !id.is_empty() && !q.is_empty() {
                q_by_call.insert(id.to_string(), q);
            }
        }
    }

    let mut out: Vec<(String, String)> = Vec::new();
    for m in msgs
        .iter()
        .filter(|m| m.role == "tool" && m.tool_name.as_deref() == Some("ask_user"))
    {
        let ans = m.content.trim();
        if ans.is_empty() || ans.contains("确认请求已超时") {
            continue;
        }
        let q = m
            .tool_call_id
            .as_deref()
            .and_then(|id| q_by_call.get(id))
            .cloned()
            .unwrap_or_else(|| "需求确认".to_string());
        out.push((q, crate::utils::text::head_chars(ans, 200)));
    }
    out
}

/// 会话里已确认过的需求（ask_user 问答对）。
///
/// 这些答复属于**已知约束**：转换时并入总目标，避免生成的工作流在运行期再问一遍
/// （无人值守时会挂起等人），也避免用户把同一个问题答第二次。
fn collect_confirmed_requirements(conn: &Connection, session: &Session) -> Vec<(String, String)> {
    match load_session_messages(conn, &session.id) {
        Ok(msgs) => pair_confirmations(&msgs),
        Err(e) => {
            log::warn!("[Session] 读取会话消息以提取确认答复失败: {}", e);
            Vec::new()
        }
    }
}

/// 解析转换来源（分层）：
/// 1) 前端已确认/编辑的任务清单（`override_tasks`）；
/// 2) 会话计划里未完成的项；
/// 3) 退化到用户消息（每条一个候选，明显非任务的默认不勾选）。
fn resolve_session_plan(
    conn: &Connection,
    session: &Session,
    override_tasks: Option<&[PlanTaskInput]>,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<SessionPlanResolution, AppError> {
    let summary = crate::eventlog::latest_session_summary(conn, &session.id)
        .map_err(|e| AppError::Db(format!("读取会话摘要失败: {}", e)))?
        .unwrap_or_default();
    let base_goal = if summary.trim().is_empty() {
        session.title.clone()
    } else {
        format!("{}\n\n【会话结论/摘要】\n{}", session.title, summary.trim())
    };
    // 会话里已确认过的需求 = 已知约束：并入总目标，工作流运行期不必再问（无人值守也不会挂起）
    let confirmations = collect_confirmed_requirements(conn, session);
    let confirmed_count = confirmations.len();
    let goal = if confirmations.is_empty() {
        base_goal
    } else {
        let lines = confirmations
            .iter()
            .map(|(q, a)| format!("- {}：{}", q, a))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{}\n\n【已确认的需求（会话中已确认，视为已知约束，运行期无需再向用户确认）】\n{}",
            base_goal, lines
        )
    };

    let mut p = serde_json::Map::new();
    match (
        session.api_provider.as_deref(),
        session.api_model.as_deref(),
    ) {
        (Some(pr), Some(m)) if !pr.trim().is_empty() && !m.trim().is_empty() => {
            p.insert("agent_type".to_string(), serde_json::json!("api"));
            p.insert("api_provider".to_string(), serde_json::json!(pr));
            p.insert("api_model".to_string(), serde_json::json!(m));
        }
        _ => {
            p.insert(
                "agent_type".to_string(),
                serde_json::json!(session.agent_type),
            );
        }
    }
    let params = Value::Object(p);

    // 1) 前端回传的最终清单（含显式依赖）
    if let Some(list) = override_tasks {
        // 位置映射：过滤掉空标题后下标会变，而 deps 按提交数组的 1-based 位置给出
        let mut idx_map: Vec<Option<usize>> = Vec::with_capacity(list.len());
        let mut raw: Vec<RawPlanTask> = Vec::new();
        for t in list.iter() {
            if t.title.trim().is_empty() {
                idx_map.push(None);
                continue;
            }
            idx_map.push(Some(raw.len()));
            let title = crate::utils::text::head_chars(t.title.trim(), 60);
            let detail = if t.detail.trim().is_empty() {
                crate::utils::text::head_chars(t.title.trim(), 400)
            } else {
                crate::utils::text::head_chars(t.detail.trim(), 400)
            };
            raw.push(RawPlanTask {
                title,
                detail,
                deps: t.deps.clone(),
                confirm: t.confirm,
                // 默认值只对确认类任务有意义，其余一律丢弃（避免脏值流进节点参数）
                default_value: if t.confirm {
                    t.default_value
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| crate::utils::text::head_chars(s, 200))
                } else {
                    None
                },
            });
        }
        if raw.is_empty() {
            return Err(AppError::InvalidInput(
                "任务清单为空：请至少保留一个任务再转换".to_string(),
            ));
        }
        let total = raw.len();
        // 只要有任何一行显式给了依赖，就按"显式依赖"解释（未给的行 = 无依赖，可并行）；
        // 全部未给（旧前端）时退回串行链，保持既有行为。
        let any_deps = raw.iter().any(|r| r.deps.is_some());
        let mut tasks: Vec<crate::workflow::plan::PlanTask> = Vec::new();
        for (i, rt) in raw.iter().enumerate() {
            let dep_keys: Vec<String> = match &rt.deps {
                Some(ds) => {
                    let mut out: Vec<String> = Vec::new();
                    for &d in ds {
                        if d < 1 || d as usize > list.len() {
                            return Err(AppError::InvalidInput(format!(
                                "任务「{}」的前置序号 {} 超出范围",
                                rt.title, d
                            )));
                        }
                        // 依赖项已被取消勾选 → 丢弃该条依赖
                        if let Some(mi) = idx_map[(d - 1) as usize] {
                            if mi == i {
                                return Err(AppError::InvalidInput(format!(
                                    "任务「{}」依赖了自己",
                                    rt.title
                                )));
                            }
                            out.push(format!("t{}", mi + 1));
                        }
                    }
                    out.sort();
                    out.dedup();
                    out
                }
                None => {
                    if any_deps || i == 0 {
                        Vec::new()
                    } else {
                        vec![format!("t{}", i)]
                    }
                }
            };
            tasks.push(crate::workflow::plan::PlanTask {
                key: format!("t{}", i + 1),
                no: (i + 1) as i64,
                title: rt.title.clone(),
                detail: rt.detail.clone(),
                deps: dep_keys,
                params: params.clone(),
                confirm: rt.confirm,
                default_value: rt.default_value.clone(),
            });
        }
        check_no_cycle(&tasks)?;
        let candidates: Vec<PlanCandidateItem> = raw
            .iter()
            .map(|r| PlanCandidateItem {
                title: r.title.clone(),
                detail: r.detail.clone(),
                suggested: true,
                origin: "edited".to_string(),
                // 前端清单是唯一事实来源：用户勾选/切换的确认标记与默认值原样带下去
                confirm: r.confirm,
                default_value: r.default_value.clone(),
            })
            .collect();
        let plan = crate::workflow::plan::PlanGraph {
            name: format!("{}（来自会话）", session.title),
            description: format!(
                "由会话「{}」的已编辑任务清单生成（会话 id: {}；任务 {} 个，依赖以清单中的设置为准）。",
                session.title, session.id, total
            ),
            goal: goal.clone(),
            stage_name: "会话任务".to_string(),
            tasks,
        };
        let plan = apply_plan_naming(plan, name, description);
        return Ok(SessionPlanResolution {
            plan,
            candidates,
            source: PlanSource::Edited,
            ignored: 0,
            confirmed: confirmed_count,
        });
    }

    // 2) 候选清单 = 会话计划（未完成项）∪ 用户消息 —— 两者是 **and** 关系：都列出，不做二选一
    let todos = crate::eventlog::latest_session_todos(conn, &session.id)
        .map_err(|e| AppError::Db(format!("读取会话计划失败: {}", e)))?;
    let pending: Vec<&Value> = todos
        .iter()
        .filter(|t| t["status"].as_str().unwrap_or("pending") != "completed")
        .collect();
    let has_todos = !pending.is_empty();

    let mut candidates: Vec<PlanCandidateItem> = Vec::new();
    for t in &pending {
        let c = t["content"].as_str().unwrap_or("").trim();
        candidates.push(candidate_from_source(
            crate::utils::text::head_chars(c, 60),
            crate::utils::text::head_chars(c, 400),
            true,
            "todos",
        ));
    }
    for (title, detail, suggested) in plan_candidates_from_messages(conn, session)? {
        // 有计划时消息默认不勾选（避免与计划项重复），但依然列出，用户可勾选回来
        let checked = if has_todos { false } else { suggested };
        candidates.push(candidate_from_source(title, detail, checked, "messages"));
    }
    if candidates.is_empty() {
        return Err(AppError::InvalidInput(
            "这个会话既没有计划、也没有用户消息，无法转换".to_string(),
        ));
    }
    let ignored = candidates.iter().filter(|c| !c.suggested).count();
    let selected: Vec<PlanCandidateItem> =
        candidates.iter().filter(|c| c.suggested).cloned().collect();
    if selected.is_empty() {
        return Err(AppError::InvalidInput(
            "这个会话里没有识别到可执行任务：请在弹窗里手动勾选或编辑后再转换".to_string(),
        ));
    }
    let source = if has_todos {
        PlanSource::Todos
    } else {
        PlanSource::Messages
    };
    let plan = apply_plan_naming(
        plan_from_candidates(session, &params, &goal, source, &selected),
        name,
        description,
    );
    Ok(SessionPlanResolution {
        plan,
        candidates,
        source,
        ignored,
        confirmed: confirmed_count,
    })
}

/// 会话 → 工作流：返回转换后的定义 JSON（与落库所用定义逐字一致，供前端预览）。
#[tauri::command]
pub fn session_export_workflow(
    state: State<'_, DbState>,
    session_id: String,
    tasks: Option<Vec<PlanTaskInput>>,
    name: Option<String>,
    description: Option<String>,
) -> Result<serde_json::Value, String> {
    let conn = state.get_conn()?;
    let session = get_session_inner(&conn, &session_id)?
        .ok_or_else(|| AppError::NotFound(format!("会话不存在: {}", session_id)))?;
    let r = resolve_session_plan(
        &conn,
        &session,
        tasks.as_deref(),
        name.as_deref(),
        description.as_deref(),
    )?;
    let def = crate::workflow::plan::build_definition(&r.plan)?;
    Ok(serde_json::json!({
        "definition": serde_json::to_value(&def).map_err(|e| AppError::Json(e.to_string()))?,
        "goal": r.plan.goal,
        "source": r.source.as_str(),
        "ignoredCount": r.ignored,
        "confirmedCount": r.confirmed,
        "tasks": r.candidates.iter().map(|c| serde_json::json!({
            "title": c.title, "detail": c.detail, "suggested": c.suggested, "origin": c.origin,
            // 确认类标记与默认值随候选下发：弹窗据此显示「需确认」并预填默认值，回传时原样带上
            "confirm": c.confirm, "defaultValue": c.default_value
        })).collect::<Vec<_>>(),
    }))
}

/// 会话 → 工作流：把会话计划落库为**可直接运行**的工作流定义（工作流列表可见、无需再编辑）。
#[tauri::command]
pub fn session_promote_workflow(
    state: State<'_, DbState>,
    session_id: String,
    tasks: Option<Vec<PlanTaskInput>>,
    name: Option<String>,
    description: Option<String>,
) -> Result<crate::workflow::WorkflowDefinition, String> {
    let conn = state.get_conn()?;
    let session = get_session_inner(&conn, &session_id)?
        .ok_or_else(|| AppError::NotFound(format!("会话不存在: {}", session_id)))?;
    let r = resolve_session_plan(
        &conn,
        &session,
        tasks.as_deref(),
        name.as_deref(),
        description.as_deref(),
    )?;
    let def = crate::workflow::plan::build_definition(&r.plan)?;
    crate::workflow::create_definition(&conn, &def)?;
    Ok(def)
}

// ════════════════════════════════════════════════════════════
// 会话 → 群聊（确定性转换：房间 + 参与者 + 任务）
// ════════════════════════════════════════════════════════════

/// 会话 → 群聊：预览将要创建的房间（议题 + 任务清单），供前端确认与配置成员。
#[tauri::command]
pub fn session_export_room_plan(
    state: State<'_, DbState>,
    session_id: String,
    tasks: Option<Vec<PlanTaskInput>>,
) -> Result<serde_json::Value, String> {
    let conn = state.get_conn()?;
    let session = get_session_inner(&conn, &session_id)?
        .ok_or_else(|| AppError::NotFound(format!("会话不存在: {}", session_id)))?;
    let r = resolve_session_plan(&conn, &session, tasks.as_deref(), None, None)?;
    Ok(serde_json::json!({
        "sessionId": session.id,
        "title": session.title,
        "topic": r.plan.goal,
        "source": r.source.as_str(),
        "ignoredCount": r.ignored,
        "confirmedCount": r.confirmed,
        "tasks": r.candidates.iter().enumerate().map(|(i, c)| serde_json::json!({
            "no": i + 1,
            "title": c.title,
            "detail": c.detail,
            "suggested": c.suggested,
            "origin": c.origin,
            "deps": if i == 0 { Vec::<String>::new() } else { vec![format!("t{}", i)] },
        })).collect::<Vec<_>>(),
    }))
}

/// 会话 → 群聊：创建房间 + 参与者 + 任务（依赖按计划顺序串成链），并启动房间 Actor。
///
/// 与群聊建房间走同一条落库路径（store::insert_room/insert_participant/insert_task）；
/// 成员与负责人由前端配置面板传入 —— 会话里没有"多 Agent"信息，程序化决定不了谁来做。
#[tauri::command]
pub fn session_promote_to_room(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, crate::groupchat::room::RoomRegistry>,
    session_id: String,
    input: crate::commands::groupchat::CreateRoomInput,
    assignee: Option<String>,
    tasks: Option<Vec<PlanTaskInput>>,
) -> Result<serde_json::Value, String> {
    let conn = state.get_conn()?;
    let session = get_session_inner(&conn, &session_id)?
        .ok_or_else(|| AppError::NotFound(format!("会话不存在: {}", session_id)))?;
    let r = resolve_session_plan(&conn, &session, tasks.as_deref(), None, None)?;
    let plan = r.plan;

    // 负责人必须落在本次成员里的 api/cli 参与者上（与 groupchat_task_add 同口径）
    let assignee = match assignee.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => {
            let ok = input
                .participants
                .iter()
                .any(|p| p.id == id && matches!(p.participant_type.as_str(), "api" | "cli"));
            if !ok {
                return Err(AppError::InvalidInput(
                    "负责人必须是本次创建的成员里的 Agent 参与者（API/CLI）".to_string(),
                )
                .into());
            }
            Some(id.to_string())
        }
        None => None,
    };

    let now = crate::utils::now();
    let room = crate::groupchat::models::Room {
        id: uuid::Uuid::new_v4().to_string(),
        title: input.title.clone(),
        topic: input.topic.clone(),
        status: "idle".into(),
        strategy: "round_robin".into(),
        max_rounds: 5,
        max_parallel: 2,
        director_id: Some(input.director_id.clone()),
        current_task_id: None,
        created_at: now,
        updated_at: now,
        goal_notes: "[]".into(),
        allow_auto_cli: input.allow_auto_cli,
        output_dir: input.output_dir.clone(),
        // 派生字段：只有 list_rooms 会填
        current_round: None,
        task_total: None,
        task_finished: None,
    };
    crate::groupchat::store::insert_room(&conn, &room)?;
    for p in &input.participants {
        let row = crate::groupchat::models::ParticipantRow {
            id: p.id.clone(),
            room_id: room.id.clone(),
            participant_type: p.participant_type.clone(),
            agent_config: p.agent_config.clone(),
            display_name: p.display_name.clone(),
            system_role: p.system_role.clone(),
            status: "active".into(),
        };
        crate::groupchat::store::insert_participant(&conn, &row)?;
    }

    // 任务：计划清单本身没有依赖信息，按顺序串成链（依赖下标交给既有换算函数，不自己造下标）
    // 未指定统一执行者时，给每个任务预置一个默认执行者（在提交成员里按轮转挑 api/cli 参与者）。
    // 否则任务会以「负责人：-」进入讨论：review 阶段没有指派能力，任务既推不进讨论，
    // 又可能被主持人误判为已完成而整批跳过。主持人仍可在重排环节改派。
    let default_assignees: Vec<String> = input
        .participants
        .iter()
        .filter(|p| matches!(p.participant_type.as_str(), "api" | "cli"))
        .map(|p| p.id.clone())
        .collect();
    if assignee.is_none() && default_assignees.is_empty() {
        return Err(AppError::InvalidInput(
            "至少要有一个 API/CLI 参与者来执行任务：请至少添加一个 Agent 成员，或显式指定统一执行者"
                .to_string(),
        )
        .into());
    }

    let mut acc: Vec<crate::groupchat::models::TaskRow> = Vec::new();
    let mut prev_id: Option<String> = None;
    for (ti, t) in plan.tasks.iter().enumerate() {
        let want: Vec<String> = prev_id.clone().into_iter().collect();
        let deps = crate::groupchat::task::resolve_dep_indices(&acc, "", &want)?;
        let task = crate::groupchat::models::TaskRow {
            id: uuid::Uuid::new_v4().to_string(),
            room_id: room.id.clone(),
            task_no: t.no,
            description: if t.detail.trim().is_empty() {
                t.title.clone()
            } else {
                t.detail.clone()
            },
            assignee: assignee.clone().or_else(|| {
                default_assignees
                    .get(ti % default_assignees.len().max(1))
                    .cloned()
            }),
            depends_on: serde_json::to_string(&deps).unwrap_or_else(|_| "[]".to_string()),
            status: "discussing".to_string(),
            result_summary: None,
            error: None,
            started_at: None,
            completed_at: None,
        };
        crate::groupchat::store::insert_task(&conn, &task)?;
        prev_id = Some(task.id.clone());
        acc.push(task);
    }

    registry.spawn(state.pool.clone(), app.clone(), &room.id)?;
    Ok(serde_json::json!({ "roomId": room.id, "taskCount": acc.len() }))
}

#[cfg(test)]
mod convert_tests {
    use super::*;

    #[test]
    fn confirmations_and_courtesies_are_not_suggested() {
        for s in [
            "好的",
            "好",
            "继续",
            "嗯嗯",
            "OK",
            "okay",
            "谢谢！",
            "再试一次",
            "继续吧",
            "接着做",
        ] {
            assert!(!suggest_task_from_message(s), "不该默认勾选: {}", s);
        }
    }

    #[test]
    fn real_requests_are_suggested() {
        for s in [
            "帮我实现会话转工作流的转换",
            "把之前的调研结论整理成一份文档",
            "修复下拉在弹窗底部被裁掉的问题",
            "为什么这条链路的产出会落空，帮我查一下",
        ] {
            assert!(suggest_task_from_message(s), "应当默认勾选: {}", s);
        }
    }

    #[test]
    fn too_short_or_pure_question_is_not_suggested() {
        assert!(!suggest_task_from_message("这是什么?"));
        assert!(!suggest_task_from_message("这样?"));
    }

    fn pt(key: &str, deps: &[&str]) -> crate::workflow::plan::PlanTask {
        crate::workflow::plan::PlanTask {
            key: key.to_string(),
            no: 1,
            title: key.to_string(),
            detail: String::new(),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            params: serde_json::json!({}),
            confirm: false,
            default_value: None,
        }
    }

    #[test]
    fn dependency_cycle_is_rejected_and_parallel_is_allowed() {
        assert!(check_no_cycle(&[pt("t1", &["t2"]), pt("t2", &["t1"])]).is_err());
        assert!(check_no_cycle(&[pt("t1", &[]), pt("t2", &["t1"])]).is_ok());
        // 并行：两条都无依赖
        assert!(check_no_cycle(&[pt("t1", &[]), pt("t2", &[])]).is_ok());
        // 汇合：t3 依赖 t1、t2
        assert!(check_no_cycle(&[pt("t1", &[]), pt("t2", &[]), pt("t3", &["t1", "t2"])]).is_ok());
    }

    #[test]
    fn extract_parser_tolerates_fence_and_filters_bad_deps() {
        let raw = "```json\n{\"tasks\":[{\"title\":\"A\",\"detail\":\"做A\",\"deps\":[]},{\"title\":\"B\",\"detail\":\"做B\",\"deps\":[1,9]}]}\n```";
        let v = parse_extracted_tasks(raw, ExtractTarget::Workflow).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1]["deps"], serde_json::json!([1]));
        // 没写 kind（或写了 task）一律按普通任务处理
        assert_eq!(v[0]["confirm"], serde_json::json!(false));
    }

    /// kind=confirm 的任务要带出 confirm 标记与 default 默认值（后者进人工交互节点的默认值）；
    /// 模型没给 default 时**自动预填兜底值**，保证确认节点开箱即用（不留空 → 超时不会是空串）。
    #[test]
    fn extract_parser_marks_confirm_kind() {
        let raw = "{\"tasks\":[{\"title\":\"确认部署方案\",\"detail\":\"用蓝绿还是灰度？\",\"kind\":\"confirm\",\"default\":\"采用蓝绿\"},{\"title\":\"写代码\",\"detail\":\"实现功能\",\"kind\":\"task\",\"default\":\"不该保留\"},{\"title\":\"确认密钥\",\"detail\":\"请提供测试环境密钥\",\"kind\":\"confirm\",\"default\":\"\"},{\"title\":\"确认回滚窗口\",\"detail\":\"何时可以停服？\",\"kind\":\"confirm\"}]}";
        let v = parse_extracted_tasks(raw, ExtractTarget::Workflow).unwrap();
        assert_eq!(v[0]["confirm"], serde_json::json!(true));
        assert_eq!(v[0]["defaultValue"], serde_json::json!("采用蓝绿"));
        // 普通任务带 default 也丢弃（默认值只对确认节点有意义）
        assert_eq!(v[1]["confirm"], serde_json::json!(false));
        assert_eq!(v[1]["defaultValue"], serde_json::Value::Null);
        // 空串与完全缺失 → 都预填兜底值（而不是留空）
        for idx in [2usize, 3] {
            assert_eq!(v[idx]["confirm"], serde_json::json!(true));
            assert_eq!(
                v[idx]["defaultValue"],
                serde_json::json!(CONFIRM_DEFAULT_FALLBACK)
            );
        }
    }

    /// 「向用户收集信息」特征词判定：只认"索取动作 + 需求类名词"，且不误伤可自主完成的任务。
    #[test]
    fn user_input_tasks_are_detected() {
        // 命中的（工作流里必须落成人工交互节点）
        for t in [
            "收集用户需求",
            "向用户确认偏好",
            "澄清验收目标",
            "征求用户意见",
            "确认需求口径",
        ] {
            assert!(looks_like_user_input_task(t), "应识别为信息收集类: {}", t);
        }
        // 不该命中的：可自主完成的、或已明确"自动/无需"的
        for t in [
            "收集竞品公开资料",   // 无需求类名词：Agent 自己能做
            "自动生成需求目标",   // 明确自动推断，不向用户收集
            "确认接口数据一致性", // Agent 自己能校验
            "运行单元测试",
        ] {
            assert!(
                !looks_like_user_input_task(t),
                "不应识别为信息收集类: {}",
                t
            );
        }
    }

    /// 同一份模型输出：工作流端把信息收集类任务(含模型漏标的)升级为确认类并预填默认值，
    /// 群聊端一律普通任务（主持人本来就能向用户确认）。
    #[test]
    fn confirm_mapping_differs_by_target() {
        let raw = "{\"tasks\":[{\"title\":\"收集用户需求\",\"detail\":\"先问清交付范围\",\"kind\":\"task\"},{\"title\":\"实现登录\",\"detail\":\"写代码\"}]}";
        let wf = parse_extracted_tasks(raw, ExtractTarget::Workflow).unwrap();
        assert_eq!(
            wf[0]["confirm"],
            serde_json::json!(true),
            "模型漏标时也要兜底升级"
        );
        assert_eq!(
            wf[0]["defaultValue"],
            serde_json::json!(CONFIRM_DEFAULT_FALLBACK)
        );
        assert_eq!(wf[1]["confirm"], serde_json::json!(false));

        let room = parse_extracted_tasks(raw, ExtractTarget::Room).unwrap();
        assert_eq!(room[0]["confirm"], serde_json::json!(false));
        assert_eq!(room[0]["defaultValue"], serde_json::Value::Null);
    }

    /// 两端提示词必须有区别：工作流讲"Agent 不能交互 + 人工交互节点 + default 必填"，
    /// 群聊讲"主持人可以向用户确认 + 统一 kind=task"；两头都要求写清任务边界（防越界导致下游空转）。
    #[test]
    fn extract_prompt_differs_by_target() {
        let wf = build_extract_prompt("目标", "发言", ExtractTarget::Workflow);
        let room = build_extract_prompt("目标", "发言", ExtractTarget::Room);
        assert_ne!(wf, room);
        assert!(
            wf.contains("不能与用户交互") && wf.contains("人工交互"),
            "{}",
            wf
        );
        assert!(wf.contains("default") && wf.contains("必填"));
        assert!(room.contains("群聊") && room.contains("主持人"), "{}", room);
        assert!(room.contains("统一写 \"kind\":\"task\""));
        assert!(!room.contains("人工交互"));
        // 任务边界（流水线型任务：准备类节点只交付中间结果，别把最终产物也做了）
        for p in [&wf, &room] {
            assert!(p.contains("任务边界"), "{}", p);
            assert!(p.contains("不要产出图片/视频/音频"), "{}", p);
        }
    }

    fn msg(
        role: &str,
        content: &str,
        tool_calls: Option<&str>,
        tool_name: Option<&str>,
        tool_call_id: Option<&str>,
    ) -> Message {
        Message {
            id: format!("m_{}", content),
            session_id: "s1".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            mode: "api".to_string(),
            timestamp: 0,
            tool_calls: tool_calls.map(|s| s.to_string()),
            tool_call_id: tool_call_id.map(|s| s.to_string()),
            tool_name: tool_name.map(|s| s.to_string()),
            attachments: None,
        }
    }

    #[test]
    fn confirmations_are_paired_and_timeouts_are_ignored() {
        let calls = r#"[{"id":"c1","function":{"name":"ask_user","arguments":"{\"title\":\"需求确认\",\"prompt\":\"用哪种方案\"}"}}]"#;
        let msgs = vec![
            msg("assistant", "我来确认一下", Some(calls), None, None),
            msg("tool", "用方案 B", None, Some("ask_user"), Some("c1")),
            msg(
                "tool",
                "用户未在 60 秒内回复，确认请求已超时，请根据已有信息自行决定如何继续或收尾。",
                None,
                Some("ask_user"),
                Some("c9"),
            ),
            msg("tool", "普通工具结果", None, Some("read_file"), Some("c2")),
        ];
        let got = pair_confirmations(&msgs);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "需求确认");
        assert_eq!(got[0].1, "用方案 B");
    }
}

/// 确认类任务没拿到模型默认值时的**自动预填兜底值**。
///
/// 人工交互节点的默认值就是"无人应答时下游拿到的答复"：留空会让下游拿到空串（语义断裂），
/// 而"没人答复"本身也是信息——这条文案直接作为默认响应下发，告诉下游按会话已确认的结论继续，
/// 不必再等待确认。用户仍可在弹窗里改写或清空（清空即回到超时取空串的默认行为）。
const CONFIRM_DEFAULT_FALLBACK: &str =
    "用户未答复：请按会话中已确认的结论自行判断并继续推进，不要再等待确认";

/// 提炼目标端：**两端运行期能力不同，提示词必须有所区别**——
/// 工作流的 Agent 节点不能与用户交互（只能靠「人工交互」节点停下来问），
/// 群聊的主持人可以随时向用户发起确认，因此"信息/需求收集"类任务的落地方式不同。
#[derive(Clone, Copy, PartialEq, Debug)]
enum ExtractTarget {
    /// 会话 → 工作流
    Workflow,
    /// 会话 → 群聊
    Room,
}

impl ExtractTarget {
    fn from_opt(s: Option<&str>) -> Self {
        match s.map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("room") => ExtractTarget::Room,
            _ => ExtractTarget::Workflow,
        }
    }
}

/// 任务标题是否属于「要向用户收集/确认信息」。
///
/// 工作流里 Agent 节点无法与用户交互，这类任务若落成 Agent 节点只会空转，
/// 所以在工作流端把它们自动升级为确认类任务（→「人工交互」节点）。
///
/// 判据：标题里**同时**出现索取动作与需求类名词，且不含"自动/无需"这类自给自足字样。
/// 只在标题上判定：任务详情常带"不要向用户收集确认"这类反向说明，按详情判会误伤。
fn looks_like_user_input_task(title: &str) -> bool {
    const ASK: &[&str] = &[
        "收集", "获取", "询问", "确认", "澄清", "征集", "调研", "征求", "了解",
    ];
    const NEED: &[&str] = &[
        "用户", "需求", "要求", "期望", "偏好", "意见", "反馈", "目标", "口径", "喜好",
    ];
    const AUTO: &[&str] = &["自动", "自行", "无需", "不必", "内部", "预设"];
    let t = title.trim();
    if t.is_empty() || AUTO.iter().any(|k| t.contains(k)) {
        return false;
    }
    ASK.iter().any(|k| t.contains(k)) && NEED.iter().any(|k| t.contains(k))
}

/// 组装"从会话提炼任务"的提示词（单次专用调用，走会话自己的模型）。
///
/// 公共部分只讲格式与依赖；**kind/default 的规则按目标端分叉**（见各自的第 4 条）。
fn build_extract_prompt(goal: &str, user_lines: &str, target: ExtractTarget) -> String {
    // 第 4 条：两端的能力差异就体现在这里
    let kind_rule = match target {
        ExtractTarget::Workflow => "4. 本清单将转成**工作流**执行，而工作流的 Agent 节点**不能与用户交互**，因此：\n\
              - 凡是要向用户收集/确认信息、需求、偏好或资料的任务（标题常以 收集/确认/澄清/询问 开头），\
             必须写 \"kind\":\"confirm\"——它会生成「人工交互」节点：运行到此处停下问用户，\
             并给出 \"default\"（**必填，不能为空**）：用户没答复时可直接采用的取值，要具体、能落地\
             （如 \"采用方案 B\"）；优先按会话已有倾向推断，推断不出就给最保守、可直接执行的取值，\
             不要写\"待用户确认\"这类无效值；\n\
              - 或者把它改写成 Agent 能自主完成的子任务：detail 写成\"自动生成需求目标\"——\
             自行推断需求、在 detail 里写清推断依据与假设，**不要向用户收集确认**；\n",
        ExtractTarget::Room => "4. 本清单将交给**群聊**多 Agent 协作执行，主持人（Director）**可以**向用户发起确认，因此：\n\
              - 统一写 \"kind\":\"task\"，不要产出 confirm；\n\
              - 需要用户拍板或补充的信息，直接在 detail 里写明（例如\"方案取舍不确定时由主持人向用户发起确认\"），\
             由主持人按需向用户确认，不必为此单独建任务；\n\
              - 也可以把任务写成\"自动生成需求目标\"：自行推断需求并在 detail 里写清假设，不要为了等确认而停下；\n",
    };
    format!(
        "你是任务规划助手。下面给出一个会话的目标与用户发言摘录。\n\
         请把它们整理成一份可执行的任务清单，供后续交给多个 Agent 协作执行。\n\n\
         输出要求（必须严格遵守）：\n\
         1. 只输出一个 JSON 对象，形如：{{\"tasks\":[{{\"title\":\"简短任务名（不超过 20 字）\",\"detail\":\"交给执行者的完整说明：目标、约束与验收标准\",\"kind\":\"task\",\"default\":\"\",\"deps\":[1,2]}}]}}；\n\
         2. 任务数量控制在 3–12 条，按执行先后顺序排列；\n\
         3. 把同一件事的多次追问合并为一条；忽略纯确认、寒暄与和任务无关的对话；\n\
         {}会话中已经确认过的需求（见【已确认的需求】）不要再写成任务，也不要产出\"向用户确认需求\"这类泛泛的确认项；\n\
         5. **任务边界要写清、各干各的**（流水线型任务尤其重要，例如\"生成提示词 → 确定调用的 API/参数 → 生成图片/视频\"）：\n\
         - 准备类任务（写提示词、选模型与参数、定数据结构、写脚本）只产出**中间结果**（文字、参数、配置），\n\
         不要调用生成类工具、不要产出图片/视频/音频等最终产物；\n\
         - detail 里必须写明边界，例如\"只输出最终提示词文本，不要生成图片\"、\"只确定并交付 API 与参数，不要调用它\"；\n\
         - 只有真正产出最终产物的那一步，才在 detail 里要求产出产物并落盘；\n\
         6. deps 写**本数组中必须先完成的任务的 1-based 序号**；确实可并行的任务留空数组 []，不要为并列的任务硬造依赖；\n\
         7. 不要输出任何解释文字，不要用 markdown 代码块包裹 JSON。\n\n\
         【总目标】\n{}\n\n【用户发言（按时间顺序）】\n{}",
        kind_rule,
        goal,
        if user_lines.trim().is_empty() { "（无）" } else { user_lines }
    )
}

/// 宽松解析模型输出：允许被 ```json 包裹，取第一个 `{` 到最后一个 `}` 之间的内容。
///
/// 确认类任务的判定 = 模型给的 kind=confirm **或**（工作流端）标题命中"向用户收集信息"特征词：
/// 模型常把"收集用户需求"这类任务写成普通任务，而工作流里它只会空转，故做一次兜底升级。
fn parse_extracted_tasks(
    raw: &str,
    target: ExtractTarget,
) -> Result<Vec<serde_json::Value>, AppError> {
    let s = raw.trim();
    let (start, end) = match (s.find('{'), s.rfind('}')) {
        (Some(a), Some(b)) if b > a => (a, b),
        _ => {
            return Err(AppError::Json("模型没有返回内容，请重试".to_string()));
        }
    };
    let v: Value = serde_json::from_str(&s[start..=end])
        .map_err(|e| AppError::Json(format!("模型返回的不是合法 JSON（{}），请重试", e)))?;
    let arr = v["tasks"]
        .as_array()
        .cloned()
        .ok_or_else(|| AppError::Json("模型返回的内容里没有任务列表，请重试".to_string()))?;
    let mut out: Vec<serde_json::Value> = Vec::new();
    let arr_len = arr.len() as i64;
    for (self_idx, t) in arr.into_iter().enumerate() {
        let title_raw = t["title"].as_str().unwrap_or("").trim().to_string();
        let detail_raw = t["detail"].as_str().unwrap_or("").trim().to_string();
        if title_raw.is_empty() && detail_raw.is_empty() {
            continue;
        }
        let title = if title_raw.is_empty() {
            crate::utils::text::head_chars(&detail_raw, 60)
        } else {
            crate::utils::text::head_chars(&title_raw, 60)
        };
        let detail = if detail_raw.is_empty() {
            title.clone()
        } else {
            crate::utils::text::head_chars(&detail_raw, 400)
        };
        // deps：本数组内的 1-based 序号；越界与自依赖直接丢弃（后续还有范围与成环校验）
        let n = arr_len as i64;
        let mut deps: Vec<i64> = Vec::new();
        if let Some(ds) = t["deps"].as_array() {
            for d in ds.iter() {
                if let Some(x) = d.as_i64() {
                    // 自依赖与越界一律丢弃（成环另有 check_no_cycle 兜底）
                    if x >= 1 && x <= n && x != self_idx as i64 + 1 {
                        deps.push(x);
                    }
                }
            }
        }
        deps.sort();
        deps.dedup();
        // kind=confirm → 确认类任务（工作流端生成「人工交互」节点，运行期真正停下来问用户）。
        // 群聊端一律按普通任务处理：主持人本来就能向用户确认，不需要（也没有）交互节点映射。
        let confirm = target == ExtractTarget::Workflow
            && (t["kind"]
                .as_str()
                .map(|k| k.trim().eq_ignore_ascii_case("confirm"))
                .unwrap_or(false)
                || looks_like_user_input_task(&title));
        // default：仅确认类任务有意义 —— 无人应答时交互节点按它继续（自动运行不空转）。
        // 模型没给（或给了空串）时**自动预填兜底值**：模型提炼这条路必须保证确认节点开箱即用，
        // 不能出现"默认值空 → 超时拿到空串 → 下游语义断裂"。
        let default_value = if confirm {
            t["default"]
                .as_str()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| crate::utils::text::head_chars(s, 200))
                .or_else(|| Some(CONFIRM_DEFAULT_FALLBACK.to_string()))
        } else {
            None
        };
        out.push(serde_json::json!({
            "title": title, "detail": detail, "suggested": true, "origin": "ai", "deps": deps,
            "confirm": confirm,
            "defaultValue": default_value
        }));
    }
    if out.is_empty() {
        return Err(AppError::External(
            "模型没有给出可用任务，请重试".to_string(),
        ));
    }
    Ok(out)
}

/// 会话标题契约：用**第一条用户消息**让模型起一个标题（一次性、低开销的专用调用，不进会话对话）。
///
/// 与旧的"机械截取消息前 30 字"相比，模型标题更贴语义；机械标题仍由前端先写入做占位，
/// 本命令返回后再覆盖，因此列表不会出现空标题。
///
/// 覆盖规则（比较后再写，避免踩掉用户/其它来源的改动）：
/// - `expected_title = Some(t)`：仅当会话当前标题仍等于 `t` 时才覆盖（用户手动改过就不动）；
/// - `expected_title = None`：仅当当前标题为空时填。
///
/// 返回最终标题；生成失败或不可用（没有可用的 API 提供商/模型）时返回空串，调用方保留原占位。
#[tauri::command]
pub async fn session_suggest_title(
    state: State<'_, DbState>,
    session_id: String,
    expected_title: Option<String>,
) -> Result<String, String> {
    // 1) 同步段：判定是否可覆盖 + 取第一条用户消息 + 解析要用的 provider/model/key（连接不跨 await）
    let (endpoint, api_key, is_anthropic, model, prompt, current_title) = {
        let conn = state.get_conn()?;
        let session = get_session_inner(&conn, &session_id)?
            .ok_or_else(|| AppError::NotFound(format!("会话不存在: {}", session_id)))?;
        let current = session.title.clone();

        // 覆盖判定：与调用方看到的标题不一致 → 说明期间被改过（用户重命名/已被生成），不做覆盖
        match expected_title.as_deref() {
            Some(expect) if current != expect => {
                log::info!(
                    "[Title] 会话 {} 标题已变更（当前 {:?}），不覆盖",
                    session_id,
                    current
                );
                return Ok(current);
            }
            None if !current.is_empty() => return Ok(current),
            _ => {}
        }

        // 素材：会话里第一条用户消息（标题的依据就是它）
        let msgs = load_session_messages(&conn, &session_id)?;
        let first_user = msgs
            .iter()
            .find(|m| m.role == "user")
            .map(|m| m.content.trim().to_string())
            .unwrap_or_default();
        if first_user.is_empty() {
            // 调用方是"首条消息落库后"才来的，走到这里说明消息没写进来（或会话无用户消息）
            log::warn!(
                "[Title] 会话 {} 取不到第一条用户消息，保留占位标题",
                session_id
            );
            return Ok(current);
        }

        // 用哪个模型：优先会话自己的（API 会话）；CLI 会话没有 API 配置，
        // 则借"第一个已配 Key 的提供商 + 它的第一个模型"来起标题（纯文本小任务）
        let (pid, model) = match (session.api_provider.clone(), session.api_model.clone()) {
            (Some(p), Some(m)) if !p.trim().is_empty() && !m.trim().is_empty() => (p, m),
            _ => {
                let providers = crate::commands::api_provider::list_api_providers(&conn)?;
                let Some(p) = providers
                    .into_iter()
                    .find(|p| p.api_key_set && !p.models.is_empty())
                else {
                    log::info!(
                        "[Title] 会话 {} 没有可用的 API 提供商，保留占位标题",
                        session_id
                    );
                    return Ok(current); // 没有可用的 API 提供商：保留机械标题
                };
                let model = p.models[0].clone();
                (p.id, model)
            }
        };
        let p = crate::commands::api_provider::get_api_provider(&conn, &pid)?
            .ok_or_else(|| AppError::NotFound(format!("提供商不存在: {}", pid)))?;
        let raw_key = crate::commands::api_provider::get_api_key(&conn, &pid)?.unwrap_or_default();
        let key = if raw_key.is_empty() {
            log::warn!(
                "[Title] 会话 {} 的提供商 {} 没有可用 Key，保留占位标题",
                session_id,
                pid
            );
            return Ok(current);
        } else {
            crate::utils::crypto::decrypt(&raw_key).unwrap_or(raw_key)
        };

        let prompt = build_title_prompt(&first_user);
        let is_anthropic = p.api_format.eq_ignore_ascii_case("anthropic");
        (
            p.api_endpoint.clone(),
            key,
            is_anthropic,
            model,
            prompt,
            current,
        )
    };

    // 2) 一次专用调用（低温、非流式、极短输出）
    let client = if is_anthropic {
        crate::api_agent::client::ApiClient::new(
            endpoint,
            api_key,
            crate::api_agent::types::ApiFormat::Anthropic,
        )
    } else {
        crate::api_agent::client::ApiClient::new_openai(endpoint, api_key)
    };
    let request = crate::api_agent::types::ChatRequest {
        model,
        messages: vec![crate::api_agent::types::ChatMessage::user(&prompt)],
        tools: None,
        tool_choice: None,
        stream: false,
        // 不发 temperature：deepseek-reasoner 等推理模型不支持该参数，带了直接 400
        //（与会话记忆的意图路由同一口径）。
        temperature: None,
        // 预算不收紧：思考型模型会把预算烧在 reasoning 上，48 时标题会整段落空。
        max_tokens: Some(256),
        response_format: None,
    };
    let resp = {
        // 免费/限流档位的提供商常在"主请求刚结束"时对本请求限流，而客户端内建的 429 重试
        // 只覆盖 ~2.4s（0.8s + 1.6s）；这里对**限流类**失败再补一次延迟重试，其余错误直接返回
        // （Key 无效、模型不存在之类重试也没用，白等 5 秒）。
        const RETRY_DELAY_SECS: u64 = 5;
        let mut attempt = 1;
        loop {
            let call = if is_anthropic {
                client.chat_anthropic(&request).await
            } else {
                client.chat(&request).await
            };
            match call {
                Ok(r) => break r,
                Err(e) if attempt == 1 && looks_like_rate_limit(&e) => {
                    log::warn!(
                        "[Title] 会话 {} 起标题被限流，{}s 后重试一次: {}",
                        session_id,
                        RETRY_DELAY_SECS,
                        e
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
                    attempt += 1;
                }
                Err(e) => {
                    // 标题只是辅助信息：失败不打扰用户，但要在日志里留下确切原因（模型名/HTTP 码）
                    log::warn!("[Title] 会话 {} 起标题调用失败: {}", session_id, e);
                    return Err(e);
                }
            }
        }
    };
    let title = sanitize_generated_title(&resp.content);
    if title.is_empty() {
        // finish_reason=length 说明预算仍被吃光（思考型模型）；空串则多为模型只回了包裹符号
        log::warn!(
            "[Title] 会话 {} 模型未给出可用标题（finish_reason={}，正文 {} 字），保留占位标题",
            session_id,
            resp.finish_reason,
            resp.content.chars().count()
        );
        return Ok(current_title);
    }
    if title == current_title {
        return Ok(current_title);
    }

    // 3) 条件写入（再校验一次：期间若被改过就放弃）
    {
        let conn = state.get_conn()?;
        let now_title = get_session_inner(&conn, &session_id)?
            .map(|s| s.title)
            .unwrap_or_default();
        if now_title != current_title {
            log::info!(
                "[Title] 会话 {} 标题已被改写为 {:?}，放弃本次生成结果",
                session_id,
                now_title
            );
            return Ok(now_title);
        }
        conn.execute(
            "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![title, crate::utils::now(), session_id],
        )
        .map_err(AppError::from)?;
        log::info!("[Title] 会话 {} 标题已生成: {:?}", session_id, title);
    }
    Ok(title)
}

/// 失败信息是否属于"限流/额度"类（值得延迟重试一次）：
/// 覆盖客户端的 HTTP 429 文案与常见限流措辞；其余错误（Key 无效、模型不存在、网络中断）
/// 重试没有意义，白等延迟。
fn looks_like_rate_limit(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("429")
        || e.contains("rate limit")
        || e.contains("too many requests")
        || e.contains("限流")
}

/// 起标题的提示词：只要标题本身，避免模型带解释/引号
fn build_title_prompt(first_user_message: &str) -> String {
    format!(
        "为下面这条用户需求起一个会话标题。\n\
         要求：不超过 14 个汉字（中文）或 8 个单词（英文）；\n\
         概括主题即可，不要复述原句、不要加引号/句号/前缀（如\"标题：\"）、不要解释。\n\
         只输出标题本身。\n\n【用户第一条消息】\n{}",
        crate::utils::text::head_chars(first_user_message, 300)
    )
}

/// 清洗模型返回的标题：去掉引号/包裹符号、行首标签、结尾标点，并按码点截断。
fn sanitize_generated_title(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // 只取第一行（模型偶尔会附一行解释）
    if let Some(pos) = s.find('\n') {
        s = s[..pos].trim().to_string();
    }
    // 去掉常见的包裹与标签
    for prefix in [
        "标题：",
        "标题:",
        "会话标题：",
        "会话标题:",
        "Title:",
        "title:",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
        }
    }
    let s = s
        .trim_matches(|c| {
            matches!(
                c,
                '"' | '\'' | '“' | '”' | '‘' | '’' | '《' | '》' | '「' | '」' | '`' | '#' | '*'
            )
        })
        .trim()
        .trim_end_matches(|c| {
            matches!(
                c,
                '。' | '.' | '！' | '!' | '?' | '？' | '，' | ',' | '；' | ';'
            )
        })
        .trim()
        .to_string();
    crate::utils::text::head_chars(&s, 24)
}

/// 会话 → 任务清单（**模型提炼**）：由后端发起一次专用模型调用，不作为会话消息参与对话。
///
/// 与三层来源（前端清单 / 会话计划 / 用户消息）是互斥的两条路：前端二选一展示，两侧都可自由编辑。
///
/// `target` 决定提示词与确认类任务口径：`"workflow"`（默认，Agent 节点不能与用户交互 → 信息收集
/// 类任务落成人工交互节点）或 `"room"`（群聊主持人可随时向用户确认 → 一律普通任务）。
#[tauri::command]
pub async fn session_extract_tasks(
    state: State<'_, DbState>,
    session_id: String,
    target: Option<String>,
) -> Result<serde_json::Value, String> {
    let target = ExtractTarget::from_opt(target.as_deref());

    // 1) 同步段：读会话/提供商/素材（连接不跨 await）
    let (endpoint, api_key, is_anthropic, model, prompt) = {
        let conn = state.get_conn()?;
        let session = get_session_inner(&conn, &session_id)?
            .ok_or_else(|| AppError::NotFound(format!("会话不存在: {}", session_id)))?;
        let pid = session
            .api_provider
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| AppError::InvalidInput(
                "这个会话不是 API 会话（没有配置提供商），无法让模型提炼任务；请在来源清单里手动勾选"
                    .to_string(),
            ))?;
        let model = session
            .api_model
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "这个会话没有配置模型，无法让模型提炼任务；请在来源清单里手动勾选".to_string(),
                )
            })?;
        let p = crate::commands::api_provider::get_api_provider(&conn, &pid)?
            .ok_or_else(|| AppError::NotFound(format!("提供商不存在: {}", pid)))?;
        let raw = crate::commands::api_provider::get_api_key(&conn, &pid)?.unwrap_or_default();
        let key = if raw.is_empty() {
            String::new()
        } else {
            crate::utils::crypto::decrypt(&raw).unwrap_or(raw)
        };

        let summary =
            crate::eventlog::latest_session_summary(&conn, &session_id)?.unwrap_or_default();
        let msgs = load_session_messages(&conn, &session_id)?;
        let mut lines = String::new();
        for m in msgs.iter().filter(|m| m.role == "user") {
            let t = m.content.trim();
            if t.is_empty() {
                continue;
            }
            lines.push_str(&format!("- {}\n", crate::utils::text::head_chars(t, 400)));
        }
        let goal = if summary.trim().is_empty() {
            session.title.clone()
        } else {
            format!("{}\n\n【会话结论/摘要】\n{}", session.title, summary.trim())
        };
        // 已确认的需求一并进提示词：模型才知道哪些已经拍过板，不必再写成确认任务
        let confirmations = collect_confirmed_requirements(&conn, &session);
        let goal = if confirmations.is_empty() {
            goal
        } else {
            let lines = confirmations
                .iter()
                .map(|(q, a)| format!("- {}：{}", q, a))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}\n\n【已确认的需求（视为已知约束，不要再写成任务）】\n{}",
                goal, lines
            )
        };
        let prompt = build_extract_prompt(&goal, &lines, target);
        let is_anthropic = p.api_format.eq_ignore_ascii_case("anthropic");
        (p.api_endpoint.clone(), key, is_anthropic, model, prompt)
    };

    // 2) 一次专用调用（温度低、非流式、要求纯 JSON）
    let client = if is_anthropic {
        crate::api_agent::client::ApiClient::new(
            endpoint,
            api_key,
            crate::api_agent::types::ApiFormat::Anthropic,
        )
    } else {
        crate::api_agent::client::ApiClient::new_openai(endpoint, api_key)
    };
    let request = crate::api_agent::types::ChatRequest {
        model,
        messages: vec![crate::api_agent::types::ChatMessage::user(&prompt)],
        tools: None,
        tool_choice: None,
        stream: false,
        temperature: Some(0.2),
        max_tokens: Some(1600),
        response_format: None,
    };
    let resp = if is_anthropic {
        client.chat_anthropic(&request).await?
    } else {
        client.chat(&request).await?
    };
    let tasks = parse_extracted_tasks(&resp.content, target)?;
    Ok(serde_json::json!({ "tasks": tasks }))
}

#[cfg(test)]
mod model_switch_tests {
    use super::*;

    /// 只建本次断言用到的列/表（与迁移后的线上结构对齐）。
    /// `models` 用对象数组 `[{name,note}]`——与线上写入格式一致（见 `parse_model_entries`）。
    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                agent_type TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                cwd TEXT DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_message_preview TEXT DEFAULT '',
                message_count INTEGER DEFAULT 0,
                status TEXT DEFAULT 'active',
                api_provider TEXT,
                api_model TEXT,
                agent_session_id TEXT,
                origin TEXT,
                temperature REAL,
                max_tokens INTEGER);
             CREATE TABLE api_providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                api_endpoint TEXT NOT NULL DEFAULT '',
                api_key TEXT DEFAULT '',
                api_key_masked TEXT DEFAULT '',
                api_key_set INTEGER DEFAULT 0,
                models TEXT NOT NULL DEFAULT '[]',
                sort_order INTEGER DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                api_format TEXT NOT NULL DEFAULT 'openai');
             INSERT INTO api_providers (id, name, api_key_set, models, created_at, updated_at)
                VALUES ('p1', '甲商', 1, '[{\"name\":\"m-a\"},{\"name\":\"m-b\"}]', 1, 1),
                       ('p2', '乙商', 0, '[{\"name\":\"m-c\"}]', 1, 1);
             INSERT INTO sessions (id, agent_type, api_provider, api_model, created_at, updated_at)
                VALUES ('s-api', 'api', 'p1', 'm-a', 1, 100),
                       ('s-cli', 'claude-code', NULL, NULL, 1, 100);",
        )
        .unwrap();
        conn
    }

    fn session_row(conn: &Connection, id: &str) -> (String, String, i64) {
        conn.query_row(
            "SELECT COALESCE(api_provider, ''), COALESCE(api_model, ''), updated_at FROM sessions WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    }

    /// 切换成功后 `updated_at` 必须保持原值：该列只表示"最后消息时间"，
    /// 否则改一次模型会把老会话顶到列表最前、时间显示成"刚刚"。
    #[test]
    fn switch_model_writes_columns_but_keeps_updated_at() {
        let conn = mem_conn();
        let out = update_session_model_inner(&conn, "s-api", "p1", "m-b", "p1", "m-a").unwrap();
        assert!(out.applied);
        assert_eq!(
            session_row(&conn, "s-api"),
            ("p1".to_string(), "m-b".to_string(), 100)
        );
    }

    /// 期望值与库中不一致（期间被别处改过）→ 不覆盖，回传当前值由前端对齐。
    #[test]
    fn switch_model_is_compare_and_swap() {
        let conn = mem_conn();
        let out = update_session_model_inner(&conn, "s-api", "p1", "m-b", "p2", "m-c").unwrap();
        assert!(!out.applied);
        assert_eq!(out.api_provider, "p1");
        assert_eq!(out.api_model, "m-a");
        assert_eq!(
            session_row(&conn, "s-api"),
            ("p1".to_string(), "m-a".to_string(), 100)
        );
    }

    /// CLI 会话的模型由 agent_type 决定（二进制 + 续聊令牌），不接受切换。
    #[test]
    fn switch_model_rejects_cli_session() {
        let conn = mem_conn();
        let err = update_session_model_inner(&conn, "s-cli", "p1", "m-b", "", "").unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));
    }

    /// 目标提供商没配 Key 或没有该模型 → 直接拒绝，避免"切换成功但下一条消息必报错"。
    #[test]
    fn switch_model_validates_provider_and_model() {
        let conn = mem_conn();
        assert!(update_session_model_inner(&conn, "s-api", "p2", "m-c", "p1", "m-a").is_err());
        assert!(update_session_model_inner(&conn, "s-api", "p1", "m-x", "p1", "m-a").is_err());
        assert!(update_session_model_inner(&conn, "s-api", "p9", "m-a", "p1", "m-a").is_err());
        // 全失败后原值不动
        assert_eq!(
            session_row(&conn, "s-api"),
            ("p1".to_string(), "m-a".to_string(), 100)
        );
    }

    /// 切目录同样不该动 `updated_at`（同一条口径：时间只由消息驱动）。
    #[test]
    fn update_session_cwd_keeps_updated_at() {
        let conn = mem_conn();
        update_session_cwd_inner(&conn, "s-api", "E:\\ws").unwrap();
        let cwd: String = conn
            .query_row(
                "SELECT COALESCE(cwd, '') FROM sessions WHERE id = 's-api'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cwd, "E:\\ws");
        let (_, _, ts) = session_row(&conn, "s-api");
        assert_eq!(ts, 100);
    }

    /// 只有限流/额度类失败值得延迟重试；认证、模型不存在等重试没意义。
    #[test]
    fn rate_limit_detection_matches_observed_wording() {
        assert!(looks_like_rate_limit(
            "API 返回错误 (HTTP 429)：模型可用但触发服务商限流/免费额度上限（已自动重试仍失败）"
        ));
        assert!(looks_like_rate_limit(
            "You've reached the API rate limit for free users"
        ));
        assert!(!looks_like_rate_limit(
            "认证失败 (HTTP 401)：API Key 无效或已过期。"
        ));
        assert!(!looks_like_rate_limit(
            "API 请求失败: connection reset by peer"
        ));
    }
}
