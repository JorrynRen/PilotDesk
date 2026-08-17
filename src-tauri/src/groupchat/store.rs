//! 群聊 5 表 SQLite 读写（直接 rusqlite，无额外存储抽象）。

use rusqlite::{params, Connection, OptionalExtension};

use crate::utils::errors::AppError;

use super::models::{MessageRow, ParticipantRow, Room, StanceRow, TaskRow};

// ── Room ──

pub fn insert_room(conn: &Connection, room: &Room) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO groupchat_rooms
            (id, title, topic, status, strategy, max_rounds, max_parallel,
             director_id, current_task_id, created_at, updated_at, goal_notes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
        ],
    )?;
    Ok(())
}

pub fn get_room(conn: &Connection, room_id: &str) -> Result<Option<Room>, AppError> {
    conn.query_row(
        "SELECT id, title, topic, status, strategy, max_rounds, max_parallel,
                director_id, current_task_id, created_at, updated_at, goal_notes
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
                director_id, current_task_id, created_at, updated_at, goal_notes
         FROM groupchat_rooms ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], row_to_room)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 删除房间及其全部关联数据（任务/立场/消息/参与者/房间）。
pub fn delete_room(conn: &Connection, room_id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM groupchat_tasks WHERE room_id = ?1", params![room_id])?;
    conn.execute("DELETE FROM groupchat_stances WHERE room_id = ?1", params![room_id])?;
    conn.execute("DELETE FROM groupchat_messages WHERE room_id = ?1", params![room_id])?;
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
         FROM groupchat_participants WHERE room_id = ?1",
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

// ── Message ──

pub fn insert_message(conn: &Connection, m: &MessageRow) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO groupchat_messages
            (id, room_id, round, seq, sender, recipients, kind, reply_to, content, attachments, tool_calls, extra, timestamp)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            m.id,
            m.room_id,
            m.round,
            m.seq,
            m.sender,
            m.recipients,
            m.kind,
            m.reply_to,
            m.content,
            m.attachments,
            m.tool_calls,
            m.extra,
            m.timestamp,
        ],
    )?;
    Ok(())
}

pub fn list_messages(
    conn: &Connection,
    room_id: &str,
    after_seq: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<MessageRow>, AppError> {
    let after = after_seq.unwrap_or(0);
    // -1 表示不限制条数（SQLite LIMIT -1 等价于无上限），取消测试期的加载条数限制。
    let limit = limit.unwrap_or(-1);
    let mut stmt = conn.prepare(
        "SELECT id, room_id, round, seq, sender, recipients, kind, reply_to, content, attachments, tool_calls, extra, timestamp
         FROM groupchat_messages WHERE room_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![room_id, after, limit], row_to_message)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 房间消息总数（供前端分页判断 hasMore 与虚拟列表 firstItemIndex）。
pub fn count_messages(conn: &Connection, room_id: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COUNT(*) FROM groupchat_messages WHERE room_id = ?1",
        params![room_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// 分页拉取消息，返回按 `seq` 升序的最多 `limit` 条。
/// - `before_seq` 为 `None`：取最近 `limit` 条（首屏）；
/// - `before_seq` 为 `Some(b)`：取 `seq < b` 的最近 `limit` 条（向上加载更早消息）。
/// 内部统一 `ORDER BY seq DESC LIMIT` 再反转为升序，保证返回顺序稳定为正序。
pub fn list_messages_page(
    conn: &Connection,
    room_id: &str,
    before_seq: Option<i64>,
    limit: i64,
) -> Result<Vec<MessageRow>, AppError> {
    const COLS: &str = "id, room_id, round, seq, sender, recipients, kind, reply_to, content, attachments, tool_calls, extra, timestamp";

    let mut rows = match before_seq {
        Some(b) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLS} FROM groupchat_messages WHERE room_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3"
            ))?;
            let mapped = stmt.query_map(params![room_id, b, limit], row_to_message)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLS} FROM groupchat_messages WHERE room_id = ?1 ORDER BY seq DESC LIMIT ?2"
            ))?;
            let mapped = stmt.query_map(params![room_id, limit], row_to_message)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        }
    };
    rows.reverse();
    Ok(rows)
}

/// 房间当前最大消息序号，用于 Actor 重启后延续单调递增的 seq（避免新消息与历史消息序号冲突）。
pub fn max_message_seq(conn: &Connection, room_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM groupchat_messages WHERE room_id = ?1",
        params![room_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// 房间当前最大消息轮次，用于 Actor 重启后延续 round（避免新消息轮次回退）。
pub fn max_message_round(conn: &Connection, room_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(round), 0) FROM groupchat_messages WHERE room_id = ?1",
        params![room_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: row.get(0)?,
        room_id: row.get(1)?,
        round: row.get(2)?,
        seq: row.get(3)?,
        sender: row.get(4)?,
        recipients: row.get(5)?,
        kind: row.get(6)?,
        reply_to: row.get(7)?,
        content: row.get(8)?,
        attachments: row.get(9)?,
        tool_calls: row.get(10)?,
        extra: row.get(11)?,
        timestamp: row.get(12)?,
    })
}

// ── Stance ──

pub fn upsert_stance(
    conn: &Connection,
    room_id: &str,
    participant_id: &str,
    stance: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO groupchat_stances (room_id, participant_id, stance, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(room_id, participant_id) DO UPDATE SET
            stance = excluded.stance, updated_at = excluded.updated_at",
        params![room_id, participant_id, stance, crate::utils::now()],
    )?;
    Ok(())
}

pub fn list_stances(conn: &Connection, room_id: &str) -> Result<Vec<StanceRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT room_id, participant_id, stance, updated_at
         FROM groupchat_stances WHERE room_id = ?1",
    )?;
    let rows = stmt.query_map(params![room_id], |row| {
        Ok(StanceRow {
            room_id: row.get(0)?,
            participant_id: row.get(1)?,
            stance: row.get(2)?,
            updated_at: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ── Task ──

pub fn insert_task(conn: &Connection, t: &TaskRow) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO groupchat_tasks
            (id, room_id, task_no, description, assignee, depends_on, status,
             result_summary, error, started_at, completed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            t.id,
            t.room_id,
            t.task_no,
            t.description,
            t.assignee,
            t.depends_on,
            t.status,
            t.result_summary,
            t.error,
            t.started_at,
            t.completed_at,
        ],
    )?;
    Ok(())
}

pub fn list_tasks(conn: &Connection, room_id: &str) -> Result<Vec<TaskRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, room_id, task_no, description, assignee, depends_on, status,
                result_summary, error, started_at, completed_at
         FROM groupchat_tasks WHERE room_id = ?1 ORDER BY task_no ASC",
    )?;
    let rows = stmt.query_map(params![room_id], row_to_task)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_task(conn: &Connection, room_id: &str, task_id: &str) -> Result<Option<TaskRow>, AppError> {
    conn.query_row(
        "SELECT id, room_id, task_no, description, assignee, depends_on, status,
                result_summary, error, started_at, completed_at
         FROM groupchat_tasks WHERE room_id = ?1 AND id = ?2",
        params![room_id, task_id],
        row_to_task,
    )
    .optional()
    .map_err(Into::into)
}

/// 更新任务可变字段（description/assignee/status/result_summary/error/started_at/completed_at）。
pub fn update_task(conn: &Connection, t: &TaskRow) -> Result<(), AppError> {
    conn.execute(
        "UPDATE groupchat_tasks SET description = ?1, assignee = ?2, status = ?3, result_summary = ?4,
                error = ?5, started_at = ?6, completed_at = ?7
         WHERE id = ?8 AND room_id = ?9",
        params![
            t.description,
            t.assignee,
            t.status,
            t.result_summary,
            t.error,
            t.started_at,
            t.completed_at,
            t.id,
            t.room_id,
        ],
    )?;
    Ok(())
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: row.get(0)?,
        room_id: row.get(1)?,
        task_no: row.get(2)?,
        description: row.get(3)?,
        assignee: row.get(4)?,
        depends_on: row.get(5)?,
        status: row.get(6)?,
        result_summary: row.get(7)?,
        error: row.get(8)?,
        started_at: row.get(9)?,
        completed_at: row.get(10)?,
    })
}
