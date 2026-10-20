//! 任务评论（沟通层 §10.1）：任务内的讨论流。
//!
//! 与「过程记录」共用 `task_comments` 表，但语义不同：
//! - 过程记录 `content_type='progress'`：只在进行中写入，承载进度与过程附件；
//! - 任务评论 `content_type='comment'`：待办 / 进行中 / 待确认 / 已完成都可讨论，
//!   支持回复与 @ 提及；归档 / 作废后只读。
//!
//! 权限：任务参与者（创建者 / 负责人 / 协作者）与组织管理者可写；其余成员只读。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::tasks::{self, TaskRole};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommentView {
    pub id: String,
    pub task_id: String,
    pub author_id: String,
    pub content: String,
    pub reply_to_id: Option<String>,
    pub mentions: Vec<String>,
    pub created_at: i64,
}

fn parse_mentions(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn can_write(role: TaskRole) -> bool {
    tasks::is_participant(role) || role == TaskRole::Manager
}

/// 只保留组织内在用成员，避免把通知写给已移除/不存在的账号。
fn sanitize_mentions(
    conn: &Connection,
    org_id: &str,
    mentions: &[String],
) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for uid in mentions {
        let uid = uid.trim();
        if uid.is_empty() || out.iter().any(|x| x == uid) {
            continue;
        }
        let active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM member_snapshot
                 WHERE org_id = ?1 AND user_id = ?2 AND seat_status = 'active'",
                params![org_id, uid],
                |r| r.get(0),
            )
            .map_err(|e| format!("校验提及成员失败：{}", e))?;
        if active > 0 {
            out.push(uid.to_string());
        }
    }
    Ok(out)
}

fn ensure_task_commentable(conn: &Connection, org_id: &str, task_id: &str) -> Result<(), String> {
    let status = tasks::task_status(conn, org_id, task_id)?;
    if matches!(
        status.as_str(),
        tasks::STATUS_ARCHIVED | tasks::STATUS_CANCELLED
    ) {
        return Err(format!(
            "任务已{}，讨论只读",
            tasks::status_label(&status)
        ));
    }
    Ok(())
}

/// 发一条评论；`reply_to_id` 必须指向同一任务下未删除的评论。
pub fn add_comment(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    content: &str,
    reply_to_id: Option<&str>,
    mentions: &[String],
) -> Result<CommentView, String> {
    let role = tasks::actor_task_role(conn, org_id, task_id, actor)?;
    if !can_write(role) {
        return Err("只有任务参与者或组织管理者可以参与讨论".to_string());
    }
    ensure_task_commentable(conn, org_id, task_id)?;
    let content = content.trim();
    if content.is_empty() {
        return Err("评论内容不能为空".to_string());
    }
    if content.chars().count() > 4000 {
        return Err("评论内容过长（最多 4000 字）".to_string());
    }
    if let Some(rid) = reply_to_id {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM task_comments
                 WHERE id = ?1 AND task_id = ?2 AND content_type = 'comment' AND deleted_at IS NULL",
                params![rid, task_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("校验回复目标失败：{}", e))?;
        if exists == 0 {
            return Err("回复的评论不存在或已删除".to_string());
        }
    }
    let mentions = sanitize_mentions(conn, org_id, mentions)?;
    let id = crate::utils::new_id();
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO task_comments
            (id, task_id, author_id, content, content_type, reply_to_id, mentions, attachments,
             created_at, updated_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, 'comment', ?5, ?6, '[]', ?7, ?7, NULL)",
        params![
            id,
            task_id,
            actor,
            content,
            reply_to_id,
            serde_json::to_string(&mentions).unwrap_or_else(|_| "[]".to_string()),
            now
        ],
    )
    .map_err(|e| format!("保存评论失败：{}", e))?;

    notify_participants(conn, org_id, actor, task_id, content, &mentions)?;

    Ok(CommentView {
        id,
        task_id: task_id.to_string(),
        author_id: actor.to_string(),
        content: content.to_string(),
        reply_to_id: reply_to_id.map(str::to_string),
        mentions,
        created_at: now,
    })
}

/// 评论提醒：发给任务参与者 + 被 @ 的成员（不给自己发；同一未读任务只留一条）。
fn notify_participants(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    content: &str,
    mentions: &[String],
) -> Result<(), String> {
    let row: Option<(String, Option<String>, String)> = conn
        .query_row(
            "SELECT creator_id, assignee_id, collaborators FROM org_tasks
             WHERE id = ?1 AND org_id = ?2",
            params![task_id, org_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((creator, assignee, collaborators_json)) = row else {
        return Ok(());
    };
    let mut recipients: Vec<String> = vec![creator];
    if let Some(a) = assignee {
        recipients.push(a);
    }
    recipients.extend(parse_mentions(&collaborators_json));
    recipients.extend(mentions.iter().cloned());

    let preview: String = content.chars().take(40).collect();
    for uid in recipients {
        if uid.is_empty() || uid == actor {
            continue;
        }
        let _ = super::notify::notify(
            conn,
            org_id,
            actor,
            Some(&uid),
            "comment.new",
            "任务有新讨论",
            &preview,
            "task",
            task_id,
        );
    }
    Ok(())
}

/// 列评论（按时间正序；最多返回最近 200 条）。
pub fn list_comments(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
) -> Result<Vec<CommentView>, String> {
    tasks::task_status(conn, org_id, task_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, author_id, content, reply_to_id, mentions, created_at
             FROM task_comments
             WHERE task_id = ?1 AND content_type = 'comment' AND deleted_at IS NULL
             ORDER BY created_at DESC, id DESC
             LIMIT 200",
        )
        .map_err(|e| format!("查询评论失败：{}", e))?;
    let rows = stmt
        .query_map(params![task_id], |r| {
            Ok(CommentView {
                id: r.get(0)?,
                task_id: r.get(1)?,
                author_id: r.get(2)?,
                content: r.get(3)?,
                reply_to_id: r.get(4)?,
                mentions: parse_mentions(&r.get::<_, String>(5)?),
                created_at: r.get(6)?,
            })
        })
        .map_err(|e| format!("查询评论失败：{}", e))?;
    let mut list = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取评论失败：{}", e))?;
    list.reverse();
    Ok(list)
}

/// 删除评论（软删）：作者本人或组织管理者。
pub fn delete_comment(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    comment_id: &str,
) -> Result<(), String> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT c.author_id, c.task_id FROM task_comments c
             JOIN org_tasks t ON t.id = c.task_id
             WHERE c.id = ?1 AND t.org_id = ?2 AND c.content_type = 'comment'
               AND c.deleted_at IS NULL",
            params![comment_id, org_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((author, task_id)) = row else {
        return Err("评论不存在或已删除".to_string());
    };
    let role = tasks::actor_task_role(conn, org_id, &task_id, actor)?;
    if author != actor && role != TaskRole::Manager {
        return Err("只能删除自己的评论（组织管理者可删除任意评论）".to_string());
    }
    conn.execute(
        "UPDATE task_comments SET deleted_at = ?2, updated_at = ?2 WHERE id = ?1",
        params![comment_id, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| format!("删除评论失败：{}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_parse_is_lenient() {
        assert_eq!(parse_mentions("[\"a\",\"b\"]"), vec!["a", "b"]);
        assert!(parse_mentions("not-json").is_empty());
    }
}
