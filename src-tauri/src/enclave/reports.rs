//! 飞地完成汇报：四态流转（草稿 → 已提交 → 已确认 / 已退回）+ 留言。
//!
//! 与任务状态机联动（M4 只做状态，M5 在「确认」后触发产出沉淀）：
//! - 提交汇报 → 任务 `in_progress → submitted`
//! - 确认汇报 → 任务 `submitted → done`（写 `completed_at`）
//! - 退回汇报 → 任务 `submitted → in_progress`
//!
//! 权限：草稿/提交仅**负责人**；确认/退回为**管理视角**（manager/creator）。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::enclave::tasks::{self, TaskRole};

pub const REPORT_DRAFT: &str = "draft";
pub const REPORT_SUBMITTED: &str = "submitted";
pub const REPORT_CONFIRMED: &str = "confirmed";
pub const REPORT_RETURNED: &str = "returned";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportView {
    pub id: String,
    pub task_id: String,
    pub reporter_id: String,
    pub content: String,
    pub materials: Vec<String>,
    pub deliverables: Vec<String>,
    pub status: String,
    pub confirmed_by: Option<String>,
    pub confirmed_at: Option<i64>,
    /// 产出沉淀时间（M5 用；NULL = 尚未沉淀）
    pub sedimented_at: Option<i64>,
    pub sediment_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn parse_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

const SELECT_COLS: &str = "id, task_id, reporter_id, content, materials, deliverables, status,
     confirmed_by, confirmed_at, sedimented_at, sediment_error, created_at, updated_at";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReportView> {
    let materials: String = r.get(4)?;
    let deliverables: String = r.get(5)?;
    Ok(ReportView {
        id: r.get(0)?,
        task_id: r.get(1)?,
        reporter_id: r.get(2)?,
        content: r.get(3)?,
        materials: parse_list(&materials),
        deliverables: parse_list(&deliverables),
        status: r.get(6)?,
        confirmed_by: r.get(7)?,
        confirmed_at: r.get(8)?,
        sedimented_at: r.get(9)?,
        sediment_error: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

/// 某任务的全部汇报（新→旧）。
pub fn list_reports(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
) -> Result<Vec<ReportView>, String> {
    tasks::task_status(conn, org_id, task_id)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM task_reports WHERE task_id = ?1 ORDER BY created_at DESC",
            SELECT_COLS
        ))
        .map_err(|e| format!("查询汇报失败：{}", e))?;
    let rows = stmt
        .query_map(params![task_id], map_row)
        .map_err(|e| format!("查询汇报失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取汇报失败：{}", e))
}

fn load_report(conn: &Connection, task_id: &str, report_id: &str) -> Result<ReportView, String> {
    conn.query_row(
        &format!(
            "SELECT {} FROM task_reports WHERE id = ?1 AND task_id = ?2",
            SELECT_COLS
        ),
        params![report_id, task_id],
        map_row,
    )
    .optional()
    .map_err(|e| format!("查询汇报失败：{}", e))?
    .ok_or_else(|| "汇报不存在或不属于该任务".to_string())
}

fn ensure_participant(role: TaskRole) -> Result<(), String> {
    if tasks::is_participant(role) {
        Ok(())
    } else {
        Err("只有任务创建者、负责人或参与者可以填写 / 提交汇报".to_string())
    }
}

/// 确认 / 退回跟「状态归属」一致：仅创建者与负责人。
fn ensure_status_owner(role: TaskRole) -> Result<(), String> {
    if matches!(role, TaskRole::Creator | TaskRole::Assignee) {
        Ok(())
    } else {
        Err("只有任务创建者或负责人可以确认 / 退回汇报".to_string())
    }
}

fn reject_closed_task(conn: &Connection, org_id: &str, task_id: &str) -> Result<(), String> {
    let status = tasks::task_status(conn, org_id, task_id)?;
    if status == tasks::STATUS_DONE
        || status == tasks::STATUS_ARCHIVED
        || status == tasks::STATUS_CANCELLED
    {
        return Err("任务已完成、归档或作废，不能再修改汇报".to_string());
    }
    Ok(())
}

/// 找该任务「草稿 / 已退回」的完成汇报（一个任务只有一份完成汇报，可继续编辑）。
fn find_editable_report(conn: &Connection, task_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT id FROM task_reports
         WHERE task_id = ?1 AND status IN ('draft','returned')
         ORDER BY created_at DESC LIMIT 1",
        params![task_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("查询汇报失败：{}", e))
}

/// 是否已有「已提交 / 已确认」的完成汇报（存在则不再允许提交）。
fn has_submitted_report(conn: &Connection, task_id: &str) -> Result<bool, String> {
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

/// 保存草稿（有可编辑草稿则覆盖，否则新建）。
pub fn save_draft(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    content: &str,
    materials: &[String],
    deliverables: &[String],
) -> Result<ReportView, String> {
    let role = tasks::actor_task_role(conn, org_id, task_id, actor)?;
    ensure_participant(role)?;
    reject_closed_task(conn, org_id, task_id)?;
    if has_submitted_report(conn, task_id)? {
        return Err("完成汇报已提交，不能再次提交或修改".to_string());
    }
    // 前置任务未完成时不允许提交（即便任务此前已被置为进行中）
    if tasks::blocked_now(conn, org_id, task_id)? {
        return Err("存在未完成的前置任务，暂不能提交完成汇报".to_string());
    }

    let now = chrono::Utc::now().timestamp();
    let materials_json = serde_json::to_string(materials).unwrap_or_else(|_| "[]".into());
    let deliverables_json = serde_json::to_string(deliverables).unwrap_or_else(|_| "[]".into());

    if let Some(id) = find_editable_report(conn, task_id)? {
        conn.execute(
            "UPDATE task_reports SET content=?3, materials=?4, deliverables=?5,
                    status='draft', updated_at=?6
             WHERE id=?1 AND task_id=?2",
            params![id, task_id, content, materials_json, deliverables_json, now],
        )
        .map_err(|e| format!("保存汇报失败：{}", e))?;
        return load_report(conn, task_id, &id);
    }

    let id = crate::utils::new_id();
    conn.execute(
        "INSERT INTO task_reports
            (id, task_id, reporter_id, content, materials, deliverables, status,
             confirmed_by, confirmed_at, sedimented_at, sediment_error, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'draft', NULL, NULL, NULL, NULL, ?7, ?7)",
        params![
            id,
            task_id,
            actor,
            content,
            materials_json,
            deliverables_json,
            now
        ],
    )
    .map_err(|e| format!("创建汇报失败：{}", e))?;
    load_report(conn, task_id, &id)
}

/// 提交汇报：先落内容，再把任务推进到「待确认」。
pub fn submit_report(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    content: &str,
    materials: &[String],
    deliverables: &[String],
) -> Result<ReportView, String> {
    let draft = save_draft(conn, org_id, actor, task_id, content, materials, deliverables)?;
    // 任务必须已开始（in_progress → submitted 才能通过状态机）
    tasks::set_status(conn, org_id, actor, task_id, tasks::STATUS_SUBMITTED)?;
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE task_reports SET status='submitted', updated_at=?2 WHERE id=?1",
        params![draft.id, now],
    )
    .map_err(|e| format!("提交汇报失败：{}", e))?;
    load_report(conn, task_id, &draft.id)
}

/// 确认汇报：任务完成 + 记确认人（产出沉淀在 M5 由同一入口触发）。
pub fn confirm_report(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    report_id: &str,
) -> Result<ReportView, String> {
    let role = tasks::actor_task_role(conn, org_id, task_id, actor)?;
    ensure_status_owner(role)?;
    let report = load_report(conn, task_id, report_id)?;
    if report.status != REPORT_SUBMITTED {
        return Err("只能确认「已提交」的汇报".to_string());
    }
    // 状态机兜底：submitted → done 需要管理视角
    tasks::set_status(conn, org_id, actor, task_id, tasks::STATUS_DONE)?;

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE task_reports SET status='confirmed', confirmed_by=?2, confirmed_at=?3, updated_at=?3
         WHERE id=?1",
        params![report_id, actor, now],
    )
    .map_err(|e| format!("确认汇报失败：{}", e))?;
    conn.execute(
        "INSERT INTO report_feedback (id, report_id, author_id, action, content, created_at)
         VALUES (?1, ?2, ?3, 'confirm', '', ?4)",
        params![crate::utils::new_id(), report_id, actor, now],
    )
    .map_err(|e| format!("记录确认留痕失败：{}", e))?;

    // M5：确认即沉淀进组织知识库。沉淀是派生动作——失败**不回滚确认**，只记 sediment_error。
    if let Err(e) =
        crate::enclave::knowledge::sediment_task_output(conn, org_id, task_id, report_id, actor)
    {
        log::warn!("[Enclave] 产出沉淀失败（已记 sediment_error）：{}", e);
    }

    load_report(conn, task_id, report_id)
}

/// 退回汇报：附原因，任务回到进行中。
pub fn return_report(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    report_id: &str,
    reason: &str,
) -> Result<ReportView, String> {
    let role = tasks::actor_task_role(conn, org_id, task_id, actor)?;
    ensure_status_owner(role)?;
    let report = load_report(conn, task_id, report_id)?;
    if report.status != REPORT_SUBMITTED {
        return Err("只能退回「已提交」的汇报".to_string());
    }
    tasks::set_status(conn, org_id, actor, task_id, tasks::STATUS_IN_PROGRESS)?;

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE task_reports SET status='returned', updated_at=?2 WHERE id=?1",
        params![report_id, now],
    )
    .map_err(|e| format!("退回汇报失败：{}", e))?;
    conn.execute(
        "INSERT INTO report_feedback (id, report_id, author_id, action, content, created_at)
         VALUES (?1, ?2, ?3, 'return', ?4, ?5)",
        params![crate::utils::new_id(), report_id, actor, reason, now],
    )
    .map_err(|e| format!("记录退回原因失败：{}", e))?;
    load_report(conn, task_id, report_id)
}

/// 恢复汇报：把最近一份「已确认」的汇报退回可编辑状态。
///
/// 用于「恢复汇报」——已完成 / 已归档任务重新进入进行中，作者补充正文或交付物后重新提交。
/// 同时清空沉淀标记，使再次确认时能把补充后的内容更新进组织知识库。
pub fn reopen_confirmed_report(
    conn: &Connection,
    task_id: &str,
    actor: &str,
) -> Result<(), String> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM task_reports
             WHERE task_id = ?1 AND status = 'confirmed'
             ORDER BY confirmed_at DESC, created_at DESC LIMIT 1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("查询已确认汇报失败：{}", e))?;
    let Some(id) = id else {
        return Ok(()); // 没有已确认汇报（如从待办直接归档的任务）→ 无需恢复
    };
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE task_reports
         SET status = 'returned', confirmed_by = NULL, confirmed_at = NULL,
             sedimented_at = NULL, sediment_error = NULL, updated_at = ?2
         WHERE id = ?1",
        params![id, now],
    )
    .map_err(|e| format!("恢复汇报失败：{}", e))?;
    conn.execute(
        "INSERT INTO report_feedback (id, report_id, author_id, action, content, created_at)
         VALUES (?1, ?2, ?3, 'reopen', '', ?4)",
        params![crate::utils::new_id(), id, actor, now],
    )
    .map_err(|e| format!("记录恢复留痕失败：{}", e))?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackView {
    pub id: String,
    pub report_id: String,
    pub author_id: String,
    pub action: String,
    pub content: String,
    pub created_at: i64,
}

/// 汇报留言（确认 / 退回记录 + 人工补充）。
pub fn list_feedback(
    conn: &Connection,
    report_id: &str,
) -> Result<Vec<FeedbackView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, report_id, author_id, action, content, created_at
             FROM report_feedback WHERE report_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| format!("查询汇报留言失败：{}", e))?;
    let rows = stmt
        .query_map(params![report_id], |r| {
            Ok(FeedbackView {
                id: r.get(0)?,
                report_id: r.get(1)?,
                author_id: r.get(2)?,
                action: r.get(3)?,
                content: r.get(4)?,
                created_at: r.get(5)?,
            })
        })
        .map_err(|e| format!("查询汇报留言失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取汇报留言失败：{}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_participants_can_submit() {
        assert!(ensure_participant(TaskRole::Creator).is_ok());
        assert!(ensure_participant(TaskRole::Assignee).is_ok());
        assert!(ensure_participant(TaskRole::Collaborator).is_ok());
        assert!(ensure_participant(TaskRole::Watcher).is_err());
        assert!(ensure_participant(TaskRole::Manager).is_err());
    }

    #[test]
    fn only_status_owners_can_confirm() {
        assert!(ensure_status_owner(TaskRole::Creator).is_ok());
        assert!(ensure_status_owner(TaskRole::Assignee).is_ok());
        assert!(ensure_status_owner(TaskRole::Collaborator).is_err());
        assert!(ensure_status_owner(TaskRole::Watcher).is_err());
        assert!(ensure_status_owner(TaskRole::Manager).is_err());
    }
}
