//! 事件日志（append-only）——规划中的会话/房间/工作流单一事实源。
//!
//! 借鉴 deepseek-harness 的 SessionEvent 思想：所有持久化事实以只追加事件落库，
//! 消息/任务/摘要/标题等可重建投影一律从事件流派生，模型可见内容 ⟺ 已记录。
//! 本模块当前只落地 `room_events`（群聊域）的表结构与读写原语，作为 P0 地基；
//! `session_events` / `workflow_events` 在各自域接线时按同一模式追加。
//!
//! 接线说明：P0 完成前生产代码尚未调用本模块，公开原语暂标 `#[allow(dead_code)]`，
//! 由 `#[cfg(test)]` 单测驱动，避免未接线即编译告警。

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::utils::errors::AppError;

/// `room_events` 建表 + 索引 SQL（测试用；终态建表已固化进 `db::init::FINAL_SCHEMA_SQL`）。
#[allow(dead_code)]
pub const ROOM_EVENTS_SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS room_events (
    seq            INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id        TEXT NOT NULL,
    kind           TEXT NOT NULL,
    payload        TEXT NOT NULL DEFAULT '{}',
    model_visible  INTEGER NOT NULL DEFAULT 1,
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_room_events_room_seq ON room_events (room_id, seq);";

/// `session_events` 建表 + 索引 SQL（测试用；终态建表已固化进 `db::init::FINAL_SCHEMA_SQL`）。
#[allow(dead_code)]
pub const SESSION_EVENTS_SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS session_events (
    seq            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id     TEXT NOT NULL,
    kind           TEXT NOT NULL,
    payload        TEXT NOT NULL DEFAULT '{}',
    model_visible  INTEGER NOT NULL DEFAULT 1,
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_session_events_session_seq ON session_events (session_id, seq);";

/// `workflow_events` 建表 + 索引 SQL（测试用；终态建表已固化进 `db::init::FINAL_SCHEMA_SQL`）。
#[allow(dead_code)]
pub const WORKFLOW_EVENTS_SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS workflow_events (
    seq            INTEGER PRIMARY KEY AUTOINCREMENT,
    execution_id   TEXT NOT NULL,
    kind           TEXT NOT NULL,
    payload        TEXT NOT NULL DEFAULT '{}',
    model_visible  INTEGER NOT NULL DEFAULT 1,
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_workflow_events_exec ON workflow_events (execution_id, seq);";

/// 单条房间事件（seq 按房间内追加顺序单调递增）。
#[derive(Debug, Clone)]
#[allow(dead_code)] // P0 接线（list_room_events 投影调用）前字段仅测试读取。
pub struct RoomEvent {
    pub seq: i64,
    pub room_id: String,
    pub kind: String,
    pub payload: Value,
    pub model_visible: bool,
    pub created_at: i64,
}

/// 追加一条房间事件，返回新事件的 seq。唯一写入口，先于业务投影落库。
/// 生产调用方：groupchat/store 写路径。
pub fn append_room_event(
    conn: &Connection,
    room_id: &str,
    kind: &str,
    payload: &Value,
    model_visible: bool,
) -> Result<i64, AppError> {
    insert_room_event_at(conn, room_id, kind, payload, model_visible, crate::utils::now())
}

/// 以显式时间追加一条房间事件（历史一次性回放/测试专用，保留原始时间语义）。
/// 生产写路径一律走 [`append_room_event`]。
pub fn insert_room_event_at(
    conn: &Connection,
    room_id: &str,
    kind: &str,
    payload: &Value,
    model_visible: bool,
    created_at: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO room_events (room_id, kind, payload, model_visible, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            room_id,
            kind,
            payload.to_string(),
            i64::from(model_visible),
            created_at,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 读取某房间的全部事件（seq 升序）。
#[allow(dead_code)] // 同上；P0 接线后供派生投影使用。
pub fn list_room_events(conn: &Connection, room_id: &str) -> Result<Vec<RoomEvent>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT seq, room_id, kind, payload, model_visible, created_at
         FROM room_events WHERE room_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![room_id], |r| {
        let payload: String = r.get(3)?;
        Ok(RoomEvent {
            seq: r.get(0)?,
            room_id: r.get(1)?,
            kind: r.get(2)?,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            model_visible: r.get::<_, i64>(4)? != 0,
            created_at: r.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// 某房间的事件总数（派生投影与一致性校验用）。
#[allow(dead_code)] // 同上。
pub fn room_event_count(conn: &Connection, room_id: &str) -> Result<i64, AppError> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM room_events WHERE room_id = ?1",
        params![room_id],
        |r| r.get::<_, i64>(0),
    )?;
    Ok(count)
}

/// 最近事件的 kind（用于双写一致性对比等调试场景）。
#[allow(dead_code)] // 同上。
pub fn last_room_event_kind(conn: &Connection, room_id: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT kind FROM room_events WHERE room_id = ?1 ORDER BY seq DESC LIMIT 1",
        params![room_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}

/// 单条会话事件（API Agent 域；seq 按会话内追加顺序单调递增）。
#[derive(Debug, Clone)]
#[allow(dead_code)] // API agent 域接线（agent_loop 双写收敛）前无生产读取。
pub struct SessionEvent {
    pub seq: i64,
    pub session_id: String,
    pub kind: String,
    pub payload: Value,
    pub model_visible: bool,
    pub created_at: i64,
}

/// 追加一条会话事件，返回新事件的 seq。
#[allow(dead_code)] // API agent 域接线（agent_loop 写路径收敛）前暂无生产调用，单测覆盖。
pub fn append_session_event(
    conn: &Connection,
    session_id: &str,
    kind: &str,
    payload: &Value,
    model_visible: bool,
) -> Result<i64, AppError> {
    insert_session_event_at(conn, session_id, kind, payload, model_visible, crate::utils::now())
}

/// 以显式时间追加一条会话事件（存量回放专用；生产写路径走 [`append_session_event`]）。
#[allow(dead_code)] // 同上。
pub fn insert_session_event_at(
    conn: &Connection,
    session_id: &str,
    kind: &str,
    payload: &Value,
    model_visible: bool,
    created_at: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO session_events (session_id, kind, payload, model_visible, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            session_id,
            kind,
            payload.to_string(),
            i64::from(model_visible),
            created_at,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 读取某会话的全部事件（seq 升序）。
#[allow(dead_code)] // 同上。
pub fn list_session_events(conn: &Connection, session_id: &str) -> Result<Vec<SessionEvent>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT seq, session_id, kind, payload, model_visible, created_at
         FROM session_events WHERE session_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![session_id], |r| {
        let payload: String = r.get(3)?;
        Ok(SessionEvent {
            seq: r.get(0)?,
            session_id: r.get(1)?,
            kind: r.get(2)?,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            model_visible: r.get::<_, i64>(4)? != 0,
            created_at: r.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// 某会话的事件总数（派生投影与一致性校验用）。
#[allow(dead_code)] // 同上。
pub fn session_event_count(conn: &Connection, session_id: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COUNT(*) FROM session_events WHERE session_id = ?1",
        params![session_id],
        |r| r.get::<_, i64>(0),
    )
    .map_err(Into::into)
}

/// 读取会话最新滚动摘要（`summary/result` 事件，seq 降序取首条）；无摘要返回 `None`。
pub fn latest_session_summary(conn: &Connection, session_id: &str) -> Result<Option<String>, AppError> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM session_events WHERE session_id = ?1 AND kind = 'summary/result' ORDER BY seq DESC LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?;
    match payload {
        Some(p) => Ok(serde_json::from_str::<Value>(&p)
            .ok()
            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string))),
        None => Ok(None),
    }
}

/// 读取某会话最近一张任务列表快照（`todo/state` 事件，seq 降序取首条）的 `todos` 数组。
/// todo_write 每次执行都落整表快照（last-write-wins），跨轮消息组装据此重建模型可见的
/// 任务列表；无任何 `todo/state` 事件时返回空数组。
pub fn latest_session_todos(conn: &Connection, session_id: &str) -> Result<Vec<Value>, AppError> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM session_events WHERE session_id = ?1 AND kind = 'todo/state' ORDER BY seq DESC LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?;
    match payload {
        Some(p) => Ok(serde_json::from_str::<Value>(&p)
            .ok()
            .and_then(|v| v.get("todos").and_then(|t| t.as_array()).cloned())
            .unwrap_or_default()),
        None => Ok(Vec::new()),
    }
}

/// 单条工作流事件（seq 按执行实例内追加顺序单调递增）。
#[derive(Debug, Clone)]
#[allow(dead_code)] // 工作流域接线（engine.rs 收敛）前无生产读取。
pub struct WorkflowEvent {
    pub seq: i64,
    pub execution_id: String,
    pub kind: String,
    pub payload: Value,
    pub model_visible: bool,
    pub created_at: i64,
}

/// 追加一条工作流事件，返回新事件的 seq。
#[allow(dead_code)] // 工作流域接线（engine.rs 写路径收敛）前暂无生产调用，单测覆盖。
pub fn append_workflow_event(
    conn: &Connection,
    execution_id: &str,
    kind: &str,
    payload: &Value,
    model_visible: bool,
) -> Result<i64, AppError> {
    insert_workflow_event_at(conn, execution_id, kind, payload, model_visible, crate::utils::now())
}

/// 以显式时间追加一条工作流事件（存量回放专用；生产写路径走 [`append_workflow_event`]）。
#[allow(dead_code)] // 同上。
pub fn insert_workflow_event_at(
    conn: &Connection,
    execution_id: &str,
    kind: &str,
    payload: &Value,
    model_visible: bool,
    created_at: i64,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO workflow_events (execution_id, kind, payload, model_visible, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            execution_id,
            kind,
            payload.to_string(),
            i64::from(model_visible),
            created_at,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 读取某执行实例的全部事件（seq 升序）。
#[allow(dead_code)] // 同上。
pub fn list_workflow_events(conn: &Connection, execution_id: &str) -> Result<Vec<WorkflowEvent>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT seq, execution_id, kind, payload, model_visible, created_at
         FROM workflow_events WHERE execution_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![execution_id], |r| {
        let payload: String = r.get(3)?;
        Ok(WorkflowEvent {
            seq: r.get(0)?,
            execution_id: r.get(1)?,
            kind: r.get(2)?,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            model_visible: r.get::<_, i64>(4)? != 0,
            created_at: r.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// 某执行实例的事件总数（派生投影与一致性校验用）。
#[allow(dead_code)] // 同上。
pub fn workflow_event_count(conn: &Connection, execution_id: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COUNT(*) FROM workflow_events WHERE execution_id = ?1",
        params![execution_id],
        |r| r.get::<_, i64>(0),
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(ROOM_EVENTS_SCHEMA).unwrap();
        conn
    }

    #[test]
    fn append_and_list_ascending() {
        let conn = mem_conn();
        let s1 = append_room_event(&conn, "r1", "task/created", &serde_json::json!({"no": 1}), true).unwrap();
        let s2 = append_room_event(&conn, "r1", "task/status", &serde_json::json!({"status": "running"}), true).unwrap();
        assert!(s1 < s2);

        let events = list_room_events(&conn, "r1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, s1);
        assert_eq!(events[0].kind, "task/created");
        assert_eq!(events[0].payload["no"], 1);
        assert!(events[0].model_visible);
        assert_eq!(events[1].kind, "task/status");
    }

    #[test]
    fn rooms_are_isolated() {
        let conn = mem_conn();
        append_room_event(&conn, "r1", "a", &serde_json::json!({}), true).unwrap();
        append_room_event(&conn, "r1", "b", &serde_json::json!({}), true).unwrap();
        append_room_event(&conn, "r2", "x", &serde_json::json!({}), false).unwrap();

        assert_eq!(room_event_count(&conn, "r1").unwrap(), 2);
        assert_eq!(room_event_count(&conn, "r2").unwrap(), 1);
        let r1 = list_room_events(&conn, "r1").unwrap();
        assert!(!r1.iter().any(|e| e.kind == "x"));
        let r2 = list_room_events(&conn, "r2").unwrap();
        assert_eq!(r2[0].model_visible, false);
        assert_eq!(last_room_event_kind(&conn, "r1").unwrap().as_deref(), Some("b"));
        assert_eq!(last_room_event_kind(&conn, "r3").unwrap(), None);
    }

    #[test]
    fn session_append_list_and_isolate() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SESSION_EVENTS_SCHEMA).unwrap();
        let s1 = append_session_event(&conn, "s1", "user/message", &serde_json::json!({"role": "user"}), true).unwrap();
        let s2 = append_session_event(&conn, "s1", "assistant/message", &serde_json::json!({"role": "assistant"}), true).unwrap();
        append_session_event(&conn, "s2", "title", &serde_json::json!({}), false).unwrap();
        assert!(s1 < s2);
        assert_eq!(session_event_count(&conn, "s1").unwrap(), 2);
        let events = list_session_events(&conn, "s1").unwrap();
        assert_eq!(events[0].kind, "user/message");
        assert_eq!(events[1].payload["role"], "assistant");
        assert!(events[0].model_visible);
        // 会话隔离：s2 的事件不进 s1 列表。
        assert_eq!(session_event_count(&conn, "s2").unwrap(), 1);
    }

    #[test]
    fn latest_session_todos_takes_newest_snapshot() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SESSION_EVENTS_SCHEMA).unwrap();
        // 空：无 todo/state 事件 → 空数组
        assert!(latest_session_todos(&conn, "s1").unwrap().is_empty());

        // 单写：返回该快照的 todos
        let p1 = serde_json::json!({
            "todos": [{"id": "t1", "content": "a", "status": "pending", "priority": "high"}],
            "toolCallId": "c1", "ts": 1,
        });
        append_session_event(&conn, "s1", "todo/state", &p1, false).unwrap();
        let single = latest_session_todos(&conn, "s1").unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0]["content"], "a");

        // 覆盖写取最新：中间混插其它 kind 不影响按 kind 取末条
        append_session_event(&conn, "s1", "user/message", &serde_json::json!({"role": "user"}), true).unwrap();
        let p2 = serde_json::json!({
            "todos": [
                {"id": "t1", "content": "a", "status": "completed", "priority": "high"},
                {"id": "t2", "content": "b", "status": "in_progress", "priority": "low"},
            ],
            "toolCallId": "c2", "ts": 2,
        });
        append_session_event(&conn, "s1", "todo/state", &p2, false).unwrap();
        let latest = latest_session_todos(&conn, "s1").unwrap();
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[0]["status"], "completed");
        assert_eq!(latest[1]["content"], "b");

        // 会话隔离：s2 无事件 → 空数组
        assert!(latest_session_todos(&conn, "s2").unwrap().is_empty());
    }

    #[test]
    fn workflow_append_list_and_count() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(WORKFLOW_EVENTS_SCHEMA).unwrap();
        let e1 = append_workflow_event(&conn, "wf1", "node/start", &serde_json::json!({"node": "a"}), false).unwrap();
        let e2 = append_workflow_event(&conn, "wf1", "node/end", &serde_json::json!({"node": "a"}), false).unwrap();
        append_workflow_event(&conn, "wf2", "node/start", &serde_json::json!({}), false).unwrap();
        assert!(e1 < e2);
        assert_eq!(workflow_event_count(&conn, "wf1").unwrap(), 2);
        let events = list_workflow_events(&conn, "wf1").unwrap();
        assert_eq!(events[0].kind, "node/start");
        assert_eq!(events[0].payload["node"], "a");
        assert_eq!(events[1].kind, "node/end");
    }
}
