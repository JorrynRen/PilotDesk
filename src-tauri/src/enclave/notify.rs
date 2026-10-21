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

/// 写一条通知。
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
) -> Result<(), String> {
    let Some(uid) = user_id.filter(|u| !u.is_empty() && *u != actor) else {
        return Ok(());
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
        return Ok(());
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
    Ok(())
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
