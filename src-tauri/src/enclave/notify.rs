//! 通知（沟通层）：任务指派、汇报提交 / 确认 / 退回的站内提醒。
//!
//! 设计约束（§5.8 / §10.1）：
//! - 通知写**主机** `enclave.db`，成员端经局域网 WS 拉取，不在成员端缓存；
//! - 不做独立 IM（§10.2），只承载"任务/汇报状态变更"这类与业务对象强绑定的提醒。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NotificationView {
    pub id: String,
    pub org_id: String,
    pub user_id: String,
    /// 事件类型：`task.assigned` / `report.submitted` / `report.confirmed` / `report.returned`
    pub kind: String,
    pub title: String,
    pub body: String,
    /// 关联对象类型（task / report）
    pub target_type: String,
    pub target_id: String,
    pub read_at: Option<i64>,
    pub created_at: i64,
}

/// 写一条通知；返回是否真正写入（被跳过时为 false）。
///
/// `user_id` 为空或等于操作者时直接跳过（不给自己发提醒）；同一未读的
/// `(user, kind, target)` 只保留一条，避免重复操作刷屏。
pub fn notify(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    user_id: Option<&str>,
    kind: &str,
    title: &str,
    body: &str,
    target_type: &str,
    target_id: &str,
) -> Result<bool, String> {
    let Some(uid) = user_id.filter(|u| !u.is_empty() && *u != actor) else {
        return Ok(false);
    };
    let dup: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications
             WHERE org_id = ?1 AND user_id = ?2 AND type = ?3
               AND target_type = ?4 AND target_id = ?5 AND read_at IS NULL",
            params![org_id, uid, kind, target_type, target_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("查询通知失败：{}", e))?;
    if dup > 0 {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO notifications
            (id, org_id, user_id, type, title, body, target_type, target_id, read_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
        params![
            crate::utils::new_id(),
            org_id,
            uid,
            kind,
            title,
            body,
            target_type,
            target_id,
            chrono::Utc::now().timestamp()
        ],
    )
    .map_err(|e| format!("写入通知失败：{}", e))?;
    Ok(true)
}

/// 列通知（按时间倒序）。`unread_only=true` 只看未读。
pub fn list(
    conn: &Connection,
    org_id: &str,
    user_id: &str,
    limit: i64,
    unread_only: bool,
) -> Result<Vec<NotificationView>, String> {
    let limit = limit.clamp(1, 100);
    let sql = if unread_only {
        "SELECT id, org_id, user_id, type, title, body, target_type, target_id, read_at, created_at
         FROM notifications
         WHERE org_id = ?1 AND user_id = ?2 AND read_at IS NULL
         ORDER BY created_at DESC LIMIT ?3"
    } else {
        "SELECT id, org_id, user_id, type, title, body, target_type, target_id, read_at, created_at
         FROM notifications
         WHERE org_id = ?1 AND user_id = ?2
         ORDER BY created_at DESC LIMIT ?3"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| format!("查询通知失败：{}", e))?;
    let rows = stmt
        .query_map(params![org_id, user_id, limit], |r| {
            Ok(NotificationView {
                id: r.get(0)?,
                org_id: r.get(1)?,
                user_id: r.get(2)?,
                kind: r.get(3)?,
                title: r.get(4)?,
                body: r.get(5)?,
                target_type: r.get(6)?,
                target_id: r.get(7)?,
                read_at: r.get(8)?,
                created_at: r.get(9)?,
            })
        })
        .map_err(|e| format!("读取通知失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取通知失败：{}", e))
}

/// 标记已读：`ids` 为空 = 全部未读；返回本次标记条数。
pub fn mark_read(
    conn: &Connection,
    org_id: &str,
    user_id: &str,
    ids: &[String],
) -> Result<usize, String> {
    let now = chrono::Utc::now().timestamp();
    if ids.is_empty() {
        return conn
            .execute(
                "UPDATE notifications SET read_at = ?3
                 WHERE org_id = ?1 AND user_id = ?2 AND read_at IS NULL",
                params![org_id, user_id, now],
            )
            .map_err(|e| format!("标记通知已读失败：{}", e));
    }
    let mut marked = 0usize;
    for id in ids {
        marked += conn
            .execute(
                "UPDATE notifications SET read_at = ?4
                 WHERE org_id = ?1 AND user_id = ?2 AND id = ?3 AND read_at IS NULL",
                params![org_id, user_id, id, now],
            )
            .map_err(|e| format!("标记通知已读失败：{}", e))?;
    }
    Ok(marked)
}

/// 删除通知：`ids` 为空 = 清空该用户全部**已读**通知；返回删除条数。
///
/// 只删自己的通知（`user_id` 是当前操作者），不提供「清空全部」以免误删未读提醒。
pub fn delete(
    conn: &Connection,
    org_id: &str,
    user_id: &str,
    ids: &[String],
) -> Result<usize, String> {
    if ids.is_empty() {
        return conn
            .execute(
                "DELETE FROM notifications
                  WHERE org_id = ?1 AND user_id = ?2 AND read_at IS NOT NULL",
                params![org_id, user_id],
            )
            .map_err(|e| format!("清理已读通知失败：{}", e));
    }
    let mut removed = 0usize;
    for id in ids {
        removed += conn
            .execute(
                "DELETE FROM notifications WHERE org_id = ?1 AND user_id = ?2 AND id = ?3",
                params![org_id, user_id, id],
            )
            .map_err(|e| format!("删除通知失败：{}", e))?;
    }
    Ok(removed)
}

/// 截止提醒扫描（主机侧定时调用）：24 小时内到期 → `task.due_soon`，已过期 → `task.overdue`。
///
/// 只提醒未完成任务（pending / in_progress / submitted）的负责人；同一任务同一天
/// 每种提醒只发一条，且同一条未读提醒不会被重复写入（读过后次日仍逾期会再提醒）。
/// 返回本次新写入的条数。
pub fn sweep_due_reminders(conn: &Connection, now: i64) -> Result<usize, String> {
    let day_start = start_of_local_day(now);
    let horizon = now + 86_400;
    let mut stmt = conn
        .prepare(
            "SELECT org_id, id, title, assignee_id, due_date
             FROM org_tasks
             WHERE assignee_id IS NOT NULL AND assignee_id <> ''
               AND due_date IS NOT NULL AND due_date < ?1
               AND status IN ('pending', 'in_progress', 'submitted')",
        )
        .map_err(|e| format!("扫描截止任务失败：{}", e))?;
    let rows = stmt
        .query_map(params![horizon], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| format!("扫描截止任务失败：{}", e))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取截止任务失败：{}", e))?;

    let mut sent = 0usize;
    for (org_id, task_id, title, assignee_id, due_date) in rows {
        let overdue = due_date < now;
        let kind = if overdue { "task.overdue" } else { "task.due_soon" };
        let dup: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications
                 WHERE org_id = ?1 AND user_id = ?2 AND type = ?3
                   AND target_type = 'task' AND target_id = ?4 AND created_at >= ?5",
                params![org_id, assignee_id, kind, task_id, day_start],
                |r| r.get(0),
            )
            .map_err(|e| format!("查询截止提醒失败：{}", e))?;
        if dup > 0 {
            continue;
        }
        let body = format!(
            "「{}」{}（截止 {}）",
            title,
            if overdue { "已逾期" } else { "将在 24 小时内截止" },
            due_text(due_date)
        );
        let inserted = notify(
            conn,
            &org_id,
            "",
            Some(&assignee_id),
            kind,
            if overdue { "任务已逾期" } else { "任务即将到期" },
            &body,
            "task",
            &task_id,
        )?;
        if inserted {
            sent += 1;
        }
    }
    Ok(sent)
}

/// 本地自然日 0 点（unix 秒）：截止提醒按天去重。
fn start_of_local_day(now: i64) -> i64 {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(now, 0)
        .single()
        .and_then(|d| d.date_naive().and_hms_opt(0, 0, 0))
        .and_then(|n| n.and_local_timezone(Local).single())
        .map(|n| n.timestamp())
        .unwrap_or(now - now % 86_400)
}

/// 截止时间展示：`YYYY-MM-DD`（本地时区）。
fn due_text(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| ts.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enclave::db::EnclaveStore;

    fn store() -> EnclaveStore {
        let dir = std::env::temp_dir().join(format!("pd-notify-{}", crate::utils::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        EnclaveStore::open(&dir.join("enclave.db")).unwrap()
    }

    #[test]
    fn notify_skips_self_and_dedupes_unread() {
        let store = store();
        store
            .with_conn(|c| {
                notify(c, "1", "u1", Some("u1"), "task.assigned", "t", "b", "task", "t1")?;
                notify(c, "1", "u1", Some("u2"), "task.assigned", "t", "b", "task", "t1")?;
                notify(c, "1", "u1", Some("u2"), "task.assigned", "t", "b", "task", "t1")?;
                let rows = list(c, "1", "u2", 20, false)?;
                assert_eq!(rows.len(), 1);
                mark_read(c, "1", "u2", &[])?;
                assert_eq!(list(c, "1", "u2", 20, true)?.len(), 0);
                // 已读后同一对象可以再提醒（例如再次退回）
                notify(c, "1", "u1", Some("u2"), "task.assigned", "t", "b", "task", "t1")?;
                assert_eq!(list(c, "1", "u2", 20, true)?.len(), 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn due_reminders_cover_soon_and_overdue_once_per_day() {
        let store = store();
        store
            .with_conn(|c| {
                let now = 1_700_000_000;
                for (id, status, due) in [
                    ("t-soon", "in_progress", now + 3_600),
                    ("t-over", "pending", now - 3_600),
                    ("t-far", "pending", now + 3 * 86_400),
                    ("t-done", "done", now - 3_600),
                ] {
                    c.execute(
                        "INSERT INTO org_tasks
                            (id, org_id, title, creator_id, assignee_id, status, due_date,
                             created_at, updated_at)
                         VALUES (?1, '8', ?1, 'u1', 'u2', ?2, ?3, ?4, ?4)",
                        params![id, status, due, now],
                    )
                    .unwrap();
                }
                // 24h 内到期 + 已逾期各一条；远端与已完成不提醒
                assert_eq!(sweep_due_reminders(c, now)?, 2);
                let rows = list(c, "8", "u2", 20, false)?;
                assert_eq!(rows.len(), 2);
                assert!(rows.iter().any(|n| n.kind == "task.due_soon"));
                assert!(rows.iter().any(|n| n.kind == "task.overdue"));
                // 同一天再扫一次不重复
                assert_eq!(sweep_due_reminders(c, now + 600)?, 0);
                // 第二天：t-soon 转为逾期提醒（新 kind）；t-over 的未读逾期提醒仍在，不重复写
                assert_eq!(sweep_due_reminders(c, now + 86_400)?, 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn delete_only_own_and_clear_only_read() {
        let store = store();
        store
            .with_conn(|c| {
                notify(c, "1", "u1", Some("u2"), "comment.new", "t", "b", "task", "t1")?;
                notify(c, "1", "u1", Some("u2"), "report.submitted", "t", "b", "task", "t2")?;
                let rows = list(c, "1", "u2", 20, false)?;
                assert_eq!(rows.len(), 2);
                // 别人的通知删不掉
                assert_eq!(delete(c, "1", "u3", &[rows[0].id.clone()])?, 0);
                // 单条删除
                assert_eq!(delete(c, "1", "u2", &[rows[0].id.clone()])?, 1);
                // ids 为空只清已读：剩下那条未读不会被清
                assert_eq!(delete(c, "1", "u2", &[])?, 0);
                mark_read(c, "1", "u2", &[])?;
                assert_eq!(delete(c, "1", "u2", &[])?, 1);
                assert_eq!(list(c, "1", "u2", 20, false)?.len(), 0);
                Ok(())
            })
            .unwrap();
    }
}
