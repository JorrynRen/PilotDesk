//! 群聊数据访问：rooms/participants/stances 仍为关系表；
//! **messages/tasks 已事件化**——事实源是 `room_events`（kind=message/task/created/task/status），
//! 本文件的读写函数只是事件 append + 投影派生（含旧表迁移期保留的兼容签名）。
//! 规则见数据表处理规范：事件为唯一事实源，旧表已删除；任何新增写入口必须先追加事件。

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension};

use crate::utils::errors::AppError;

use super::models::{MessageRow, ParticipantRow, Room, StanceRow, TaskRow};
use super::participant::{Attitude, resolve_attitude};

// ── Room ──

pub fn insert_room(conn: &Connection, room: &Room) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO groupchat_rooms
            (id, title, topic, status, strategy, max_rounds, max_parallel,
             director_id, current_task_id, created_at, updated_at, goal_notes, allow_auto_cli, output_dir)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            room.id,
            room.title,
            room.topic,
            room.status,
            room.strategy,
            room.max_rounds,
            room.max_parallel,
            room.director_id,
            room.current_task_id,
            room.created_at,
            room.updated_at,
            room.goal_notes,
            room.allow_auto_cli,
            room.output_dir,
        ],
    )?;
    Ok(())
}

pub fn get_room(conn: &Connection, room_id: &str) -> Result<Option<Room>, AppError> {
    conn.query_row(
        "SELECT id, title, topic, status, strategy, max_rounds, max_parallel,
                director_id, current_task_id, created_at, updated_at, goal_notes, allow_auto_cli, output_dir
         FROM groupchat_rooms WHERE id = ?1",
        params![room_id],
        row_to_room,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_rooms(conn: &Connection) -> Result<Vec<Room>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, topic, status, strategy, max_rounds, max_parallel,
                director_id, current_task_id, created_at, updated_at, goal_notes, allow_auto_cli, output_dir
         FROM groupchat_rooms ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], row_to_room)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 删除房间及其全部关联数据：事件（room_events 已事件化消息/任务）+ 立场/参与者/房间。
pub fn delete_room(conn: &Connection, room_id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM room_events WHERE room_id = ?1", params![room_id])?;
    conn.execute("DELETE FROM groupchat_stances WHERE room_id = ?1", params![room_id])?;
    conn.execute("DELETE FROM groupchat_participants WHERE room_id = ?1", params![room_id])?;
    conn.execute("DELETE FROM groupchat_rooms WHERE id = ?1", params![room_id])?;
    Ok(())
}

pub fn update_room_status(conn: &Connection, room_id: &str, status: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_rooms SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status, crate::utils::now(), room_id],
    )?;
    Ok(())
}

pub fn update_room_director(conn: &Connection, room_id: &str, director_id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_rooms SET director_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![director_id, crate::utils::now(), room_id],
    )?;
    Ok(())
}

pub fn update_room_current_task(conn: &Connection, room_id: &str, task_id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_rooms SET current_task_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![task_id, crate::utils::now(), room_id],
    )?;
    Ok(())
}

pub fn update_room_topic(conn: &Connection, room_id: &str, topic: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_rooms SET topic = ?1, updated_at = ?2 WHERE id = ?3",
        params![topic, crate::utils::now(), room_id],
    )?;
    Ok(())
}

pub fn update_room_goal_notes(conn: &Connection, room_id: &str, goal_notes: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_rooms SET goal_notes = ?1, updated_at = ?2 WHERE id = ?3",
        params![goal_notes, crate::utils::now(), room_id],
    )?;
    Ok(())
}

pub fn update_room_output_dir(conn: &Connection, room_id: &str, output_dir: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_rooms SET output_dir = ?1, updated_at = ?2 WHERE id = ?3",
        params![output_dir, crate::utils::now(), room_id],
    )?;
    Ok(())
}

fn row_to_room(row: &rusqlite::Row<'_>) -> rusqlite::Result<Room> {
    Ok(Room {
        id: row.get(0)?,
        title: row.get(1)?,
        topic: row.get(2)?,
        status: row.get(3)?,
        strategy: row.get(4)?,
        max_rounds: row.get(5)?,
        max_parallel: row.get(6)?,
        director_id: row.get(7)?,
        current_task_id: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        goal_notes: row.get(11)?,
        allow_auto_cli: row.get(12)?,
        output_dir: row.get(13)?,
    })
}

// ── Participant ──

pub fn insert_participant(conn: &Connection, p: &ParticipantRow) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO groupchat_participants
            (id, room_id, participant_type, agent_config, display_name, system_role, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            p.id,
            p.room_id,
            p.participant_type,
            p.agent_config,
            p.display_name,
            p.system_role,
            p.status,
        ],
    )?;
    Ok(())
}

pub fn get_participant(
    conn: &Connection,
    room_id: &str,
    participant_id: &str,
) -> Result<Option<ParticipantRow>, AppError> {
    conn.query_row(
        "SELECT id, room_id, participant_type, agent_config, display_name, system_role, status
         FROM groupchat_participants WHERE room_id = ?1 AND id = ?2",
        params![room_id, participant_id],
        row_to_participant,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_participants(conn: &Connection, room_id: &str) -> Result<Vec<ParticipantRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, room_id, participant_type, agent_config, display_name, system_role, status
         FROM groupchat_participants WHERE room_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![room_id], row_to_participant)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 删除参与者及其立场快照。
pub fn delete_participant(
    conn: &Connection,
    room_id: &str,
    participant_id: &str,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM groupchat_participants WHERE room_id = ?1 AND id = ?2",
        params![room_id, participant_id],
    )?;
    conn.execute(
        "DELETE FROM groupchat_stances WHERE room_id = ?1 AND participant_id = ?2",
        params![room_id, participant_id],
    )?;
    Ok(())
}

pub fn update_participant_role(
    conn: &Connection,
    room_id: &str,
    participant_id: &str,
    role: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_participants SET system_role = ?1 WHERE room_id = ?2 AND id = ?3",
        params![role, room_id, participant_id],
    )?;
    Ok(())
}

/// 更新参与者类型（api/cli/user/director）——主持人热替换时在参与者间交换结构身份。
pub fn update_participant_type(
    conn: &Connection,
    room_id: &str,
    participant_id: &str,
    participant_type: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_participants SET participant_type = ?1 WHERE room_id = ?2 AND id = ?3",
        params![participant_type, room_id, participant_id],
    )?;
    Ok(())
}

fn row_to_participant(row: &rusqlite::Row<'_>) -> rusqlite::Result<ParticipantRow> {
    Ok(ParticipantRow {
        id: row.get(0)?,
        room_id: row.get(1)?,
        participant_type: row.get(2)?,
        agent_config: row.get(3)?,
        display_name: row.get(4)?,
        system_role: row.get(5)?,
        status: row.get(6)?,
    })
}

// ── Message（事件源：room_events kind='message'，消息不可变只追加）──

/// 追加一条消息事件（唯一写入口）。事件 append 成功后即持久；旧表已删除。
pub fn insert_message(conn: &Connection, m: &MessageRow) -> Result<(), AppError> {
    let value = serde_json::to_value(m)?;
    crate::eventlog::append_room_event(conn, &m.room_id, "message", &value, true).map(|_| ())
}

/// 从 `room_events(kind='message')` 派生消息投影，按消息 `seq` 升序返回。
/// payload 为整行消息 JSON（camelCase），反序列化为 `MessageRow`。
/// 排序取消息自身 seq：事件追加序与业务 seq 在恢复/回放场景可能不一致。
pub fn derive_messages_from_events(conn: &Connection, room_id: &str) -> Result<Vec<MessageRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM room_events WHERE room_id = ?1 AND kind = 'message' ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![room_id], |r| r.get::<_, String>(0))?;
    let mut out: Vec<MessageRow> = Vec::new();
    for row in rows {
        out.push(serde_json::from_str::<MessageRow>(&row?)?);
    }
    out.sort_by(|a, b| a.seq.cmp(&b.seq));
    Ok(out)
}

/// 读取某房间任务事件（task/created + task/status）的 payload（seq 升序）。
fn load_task_payloads(conn: &Connection, room_id: &str) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM room_events WHERE room_id = ?1 AND kind IN ('task/created', 'task/status') ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![room_id], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn list_messages(
    conn: &Connection,
    room_id: &str,
    after_seq: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<MessageRow>, AppError> {
    let after = after_seq.unwrap_or(0);
    let limit = limit.unwrap_or(-1);
    let mut all = derive_messages_from_events(conn, room_id)?;
    all.retain(|m| m.seq > after);
    if limit >= 0 {
        all.truncate(limit as usize);
    }
    Ok(all)
}

/// 房间消息总数（供前端分页判断 hasMore 与虚拟列表 firstItemIndex）。
pub fn count_messages(conn: &Connection, room_id: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COUNT(*) FROM room_events WHERE room_id = ?1 AND kind = 'message'",
        params![room_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// 分页拉取消息（向上加载）：按 `seq` 升序返回最多 `limit` 条。
/// - `before_seq` 为 `None`：最近 `limit` 条（首屏）；
/// - `before_seq` 为 `Some(b)`：`seq < b` 的最近 `limit` 条。
pub fn list_messages_page(
    conn: &Connection,
    room_id: &str,
    before_seq: Option<i64>,
    limit: i64,
) -> Result<Vec<MessageRow>, AppError> {
    let all = derive_messages_from_events(conn, room_id)?;
    let filtered: Vec<MessageRow> = match before_seq {
        Some(b) => all.into_iter().filter(|m| m.seq < b).collect(),
        None => all,
    };
    let start = filtered.len().saturating_sub(limit.max(0) as usize);
    Ok(filtered.into_iter().skip(start).collect())
}

/// 房间当前最大消息序号，用于 Actor 重启后延续单调递增的 seq。
pub fn max_message_seq(conn: &Connection, room_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(json_extract(payload, '$.seq')), 0) FROM room_events WHERE room_id = ?1 AND kind = 'message'",
        params![room_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// 房间当前最大消息轮次，用于 Actor 重启后延续 round。
pub fn max_message_round(conn: &Connection, room_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(json_extract(payload, '$.round')), 0) FROM room_events WHERE room_id = ?1 AND kind = 'message'",
        params![room_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

// ── Stance ──

pub fn upsert_stance(
    conn: &Connection,
    room_id: &str,
    participant_id: &str,
    stance: &str,
    attitude: Attitude,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO groupchat_stances (room_id, participant_id, stance, attitude, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(room_id, participant_id) DO UPDATE SET
            stance = excluded.stance, attitude = excluded.attitude, updated_at = excluded.updated_at",
        params![room_id, participant_id, stance, attitude.as_str(), crate::utils::now()],
    )?;
    Ok(())
}

pub fn list_stances(conn: &Connection, room_id: &str) -> Result<Vec<StanceRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT room_id, participant_id, stance, attitude, updated_at
         FROM groupchat_stances WHERE room_id = ?1",
    )?;
    let rows = stmt.query_map(params![room_id], |row| {
        let stance: String = row.get(2)?;
        let attitude: String = row.get(3)?;
        Ok(StanceRow {
            room_id: row.get(0)?,
            participant_id: row.get(1)?,
            // 空值（迁移前旧行）回退到文本分类。
            attitude: if attitude.is_empty() {
                resolve_attitude(&stance).as_str().to_string()
            } else {
                attitude
            },
            stance,
            updated_at: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ── Task（事件源：task/created + task/status 投影任务终态）──

/// 任务创建事件（唯一写入口）。
pub fn insert_task(conn: &Connection, t: &TaskRow) -> Result<(), AppError> {
    let value = serde_json::to_value(t)?;
    crate::eventlog::append_room_event(conn, &t.room_id, "task/created", &value, true).map(|_| ())
}

/// 任务状态/字段更新事件（唯一写入口）。payload 为本次更新后的行快照。
pub fn update_task(conn: &Connection, t: &TaskRow) -> Result<(), AppError> {
    let value = serde_json::to_value(t)?;
    crate::eventlog::append_room_event(conn, &t.room_id, "task/status", &value, true).map(|_| ())
}

/// 把一次任务事件快照叠加到当前投影上：必选字段（description/status/depends_on/task_no）
/// 直接覆盖；可选字段仅在事件快照里为 Some 时覆盖（None=未携带，保留既有值）。
/// 该语义对齐旧表行为：调用方总是携带完整行，只有真正赋值时才带上可选字段。
fn apply_task_patch(base: &mut TaskRow, patch: &TaskRow) {
    base.description = patch.description.clone();
    base.status = patch.status.clone();
    base.depends_on = patch.depends_on.clone();
    base.task_no = patch.task_no;
    if let Some(v) = &patch.assignee {
        base.assignee = Some(v.clone());
    }
    if let Some(v) = &patch.result_summary {
        base.result_summary = Some(v.clone());
    }
    if let Some(v) = &patch.error {
        base.error = Some(v.clone());
    }
    if let Some(v) = patch.started_at {
        base.started_at = Some(v);
    }
    if let Some(v) = patch.completed_at {
        base.completed_at = Some(v);
    }
}

/// 从事件流投影某房间全部任务的终态快照（task/created 建基，task/status 依序叠加）。
fn derive_task_snapshots(conn: &Connection, room_id: &str) -> Result<Vec<TaskRow>, AppError> {
    let payloads = load_task_payloads(conn, room_id)?;
    let mut map: BTreeMap<String, TaskRow> = BTreeMap::new();
    for raw in payloads {
        let patch: TaskRow = serde_json::from_str(&raw)?;
        match map.get_mut(&patch.id) {
            Some(base) => apply_task_patch(base, &patch),
            None => {
                map.insert(patch.id.clone(), patch);
            }
        }
    }
    let mut out: Vec<TaskRow> = map.into_values().collect();
    out.sort_by(|a, b| a.task_no.cmp(&b.task_no));
    Ok(out)
}

pub fn list_tasks(conn: &Connection, room_id: &str) -> Result<Vec<TaskRow>, AppError> {
    derive_task_snapshots(conn, room_id)
}

pub fn get_task(conn: &Connection, room_id: &str, task_id: &str) -> Result<Option<TaskRow>, AppError> {
    Ok(derive_task_snapshots(conn, room_id)?
        .into_iter()
        .find(|t| t.id == task_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eventlog::ROOM_EVENTS_SCHEMA;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(ROOM_EVENTS_SCHEMA).unwrap();
        conn
    }

    fn msg(id: &str, seq: i64) -> MessageRow {
        MessageRow {
            id: id.to_string(),
            room_id: "r1".to_string(),
            round: 1,
            seq,
            sender: "u1".to_string(),
            recipients: "[]".to_string(),
            kind: "message".to_string(),
            reply_to: None,
            content: format!("content-{id}"),
            attachments: "[]".to_string(),
            tool_calls: "[]".to_string(),
            reasoning_content: String::new(),
            extra: "{}".to_string(),
            timestamp: 0,
        }
    }

    #[test]
    fn messages_roundtrip_list_page_count_max() {
        let conn = mem_conn();
        for (i, id) in ["m1", "m2", "m3"].iter().enumerate() {
            insert_message(&conn, &msg(id, i as i64 + 1)).unwrap();
        }
        assert_eq!(count_messages(&conn, "r1").unwrap(), 3);
        assert_eq!(max_message_seq(&conn, "r1"), 3);
        assert_eq!(max_message_round(&conn, "r1"), 1);

        // list_messages: after_seq 过滤 + limit
        let after = list_messages(&conn, "r1", Some(1), Some(2)).unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].seq, 2);
        assert_eq!(after[1].seq, 3);

        // 分页首屏（最近 2 条）与向上加载（seq<3 的最近 2 条）
        let page = list_messages_page(&conn, "r1", None, 2).unwrap();
        assert_eq!(page.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![2, 3]);
        let up = list_messages_page(&conn, "r1", Some(3), 2).unwrap();
        assert_eq!(up.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![1, 2]);
    }

    fn task(id: &str, no: i64, status: &str) -> TaskRow {
        TaskRow {
            id: id.to_string(),
            room_id: "r1".to_string(),
            task_no: no,
            description: format!("desc-{id}"),
            assignee: None,
            depends_on: "[]".to_string(),
            status: status.to_string(),
            result_summary: None,
            error: None,
            started_at: None,
            completed_at: None,
        }
    }

    #[test]
    fn tasks_overlay_events_to_terminal_snapshot() {
        let conn = mem_conn();
        insert_task(&conn, &task("t1", 1, "pending")).unwrap();
        let mut running = task("t1", 1, "running");
        running.assignee = Some("u1".to_string());
        running.started_at = Some(100);
        update_task(&conn, &running).unwrap();
        let mut done = task("t1", 1, "success");
        done.assignee = Some("u1".to_string());
        done.result_summary = Some("ok".to_string());
        done.started_at = Some(100);
        done.completed_at = Some(200);
        update_task(&conn, &done).unwrap();

        let tasks = list_tasks(&conn, "r1").unwrap();
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!(t.status, "success");
        assert_eq!(t.result_summary.as_deref(), Some("ok"));
        assert_eq!(t.started_at, Some(100));
        assert_eq!(t.completed_at, Some(200));
        assert_eq!(get_task(&conn, "r1", "t1").unwrap().unwrap().status, "success");
        assert!(get_task(&conn, "r1", "nope").unwrap().is_none());
    }

    #[test]
    fn tasks_are_ordered_by_task_no() {
        let conn = mem_conn();
        insert_task(&conn, &task("b", 2, "pending")).unwrap();
        insert_task(&conn, &task("a", 1, "pending")).unwrap();
        let tasks = list_tasks(&conn, "r1").unwrap();
        assert_eq!(tasks.iter().map(|t| t.task_no).collect::<Vec<_>>(), vec![1, 2]);
    }
}
