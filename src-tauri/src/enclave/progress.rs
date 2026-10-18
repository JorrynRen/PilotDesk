//! 任务过程记录（进度说明 + 附件）。
//!
//! 一个任务可**多次**记录进度（每条独立留痕）；提交「完成汇报」后锁定，
//! 不再允许追加过程记录（设计口径：完成汇报是终态交付）。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::enclave::files::FileAssetView;
use crate::enclave::tasks;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressView {
    pub id: String,
    pub task_id: String,
    pub author_id: String,
    pub content: String,
    pub attachments: Vec<FileAssetView>,
    pub created_at: i64,
}

/// 过程记录列表 + 锁定状态（前端据此禁用输入框）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressList {
    pub items: Vec<ProgressView>,
    pub locked: bool,
    pub lock_reason: Option<String>,
}

/// 是否已提交完成汇报（submitted / confirmed 均视为已提交，过程记录从此锁定）。
fn final_report_submitted(conn: &Connection, task_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM task_reports WHERE task_id = ?1 AND status IN ('submitted','confirmed')
         )",
        params![task_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    .map_err(|e| format!("查询汇报状态失败：{}", e))
}

fn lock_state(conn: &Connection, task_id: &str) -> Result<(bool, Option<String>), String> {
    let status = conn
        .query_row(
            "SELECT status FROM org_tasks WHERE id = ?1",
            params![task_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("查询任务失败：{}", e))?
        .ok_or_else(|| "任务不存在".to_string())?;
    if status == tasks::STATUS_DONE
        || status == tasks::STATUS_ARCHIVED
        || status == tasks::STATUS_CANCELLED
    {
        return Ok((true, Some("任务已完成、归档或作废，不能再添加过程记录".to_string())));
    }
    if final_report_submitted(conn, task_id)? {
        return Ok((
            true,
            Some("已提交完成汇报，不能再添加过程记录".to_string()),
        ));
    }
    Ok((false, None))
}

fn attachments_of(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    progress_id: &str,
) -> Result<Vec<FileAssetView>, String> {
    let dir = crate::enclave::files::org_dir(conn, org_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT f.id, f.file_name, f.file_type, f.file_size, f.file_hash, f.file_path,
                    l.uploaded_by, l.uploaded_at
             FROM task_file_links l JOIN file_assets f ON f.id = l.file_id
             WHERE l.task_id = ?1 AND l.progress_id = ?2
             ORDER BY l.uploaded_at ASC",
        )
        .map_err(|e| format!("查询进度附件失败：{}", e))?;
    let rows = stmt
        .query_map(params![task_id, progress_id], |r| {
            let rel: String = r.get(5)?;
            Ok(FileAssetView {
                id: r.get(0)?,
                file_name: r.get(1)?,
                file_type: r.get(2)?,
                file_size: r.get(3)?,
                file_hash: r.get(4)?,
                uploaded_by: r.get(6)?,
                uploaded_at: r.get(7)?,
                abs_path: dir.join(rel).to_string_lossy().to_string(),
                report_id: None,
                progress_id: Some(progress_id.to_string()),
            })
        })
        .map_err(|e| format!("查询进度附件失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取进度附件失败：{}", e))
}

/// 追加一条过程记录（创建者 / 负责人 / 参与者）。
///
/// `attachments` 为待挂载的本机文件绝对路径：入库时按 sha256 去重落盘，
/// 并与这条记录绑定（过程成果 / 支撑文件）。
pub fn add_progress(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    content: &str,
    attachments: &[String],
) -> Result<ProgressView, String> {
    let role = tasks::actor_task_role(conn, org_id, task_id, actor)?;
    if !tasks::is_participant(role) {
        return Err("只有任务创建者、负责人或参与者可以记录进度".to_string());
    }
    if tasks::is_container(conn, org_id, task_id)? {
        return Err("该任务包含子任务，不记录过程记录；请在子任务中记录".to_string());
    }
    // 未开始（待办）不允许记录过程：否则「开始」这一步就没有意义
    let status = tasks::task_status(conn, org_id, task_id)?;
    if status != tasks::STATUS_IN_PROGRESS {
        return Err(if status == tasks::STATUS_PENDING {
            "任务尚未开始，请先点「开始」再记录过程".to_string()
        } else {
            format!(
                "当前状态（{}）下不能记录过程",
                tasks::status_label(&status)
            )
        });
    }
    let content = content.trim();
    if content.is_empty() {
        return Err("进度说明不能为空".to_string());
    }
    let (locked, reason) = lock_state(conn, task_id)?;
    if locked {
        return Err(reason.unwrap_or_else(|| "过程记录已锁定".to_string()));
    }
    let now = chrono::Utc::now().timestamp();
    let id = crate::utils::new_id();
    conn.execute(
        "INSERT INTO task_comments
            (id, task_id, author_id, content, content_type, reply_to_id, mentions, attachments,
             created_at, updated_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, 'progress', NULL, '[]', '[]', ?5, ?5, NULL)",
        params![id, task_id, actor, content, now],
    )
    .map_err(|e| format!("保存过程记录失败：{}", e))?;

    for src in attachments {
        crate::enclave::files::attach_progress_file(
            conn,
            org_id,
            actor,
            task_id,
            src,
            &id,
        )?;
    }

    Ok(ProgressView {
        id: id.clone(),
        task_id: task_id.to_string(),
        author_id: actor.to_string(),
        content: content.to_string(),
        attachments: attachments_of(conn, org_id, task_id, &id)?,
        created_at: now,
    })
}

/// 列出过程记录（时间正序）+ 锁定状态。
pub fn list_progress(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
) -> Result<ProgressList, String> {
    tasks::task_status(conn, org_id, task_id)?; // 校验任务归属
    let mut stmt = conn
        .prepare(
            "SELECT id, author_id, content, created_at FROM task_comments
             WHERE task_id = ?1 AND content_type = 'progress' AND deleted_at IS NULL
             ORDER BY created_at ASC",
        )
        .map_err(|e| format!("查询过程记录失败：{}", e))?;
    let rows: Vec<(String, String, String, i64)> = stmt
        .query_map(params![task_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| format!("查询过程记录失败：{}", e))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取过程记录失败：{}", e))?;

    let mut items = Vec::with_capacity(rows.len());
    for (id, author_id, content, created_at) in rows {
        let attachments = attachments_of(conn, org_id, task_id, &id)?;
        items.push(ProgressView {
            id,
            task_id: task_id.to_string(),
            author_id,
            content,
            attachments,
            created_at,
        });
    }
    let (locked, lock_reason) = lock_state(conn, task_id)?;
    Ok(ProgressList {
        items,
        locked,
        lock_reason,
    })
}
