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

fn ensure_assignee(role: TaskRole) -> Result<(), String> {
    if role == TaskRole::Assignee {
        Ok(())
    } else {
        Err("只有任务负责人可以编写 / 提交汇报".to_string())
    }
}

fn ensure_manager(role: TaskRole) -> Result<(), String> {
    if matches!(role, TaskRole::Manager | TaskRole::Creator) {
        Ok(())
    } else {
        Err("当前角色无权确认或退回汇报".to_string())
    }
}

fn reject_closed_task(conn: &Connection, org_id: &str, task_id: &str) -> Result<(), String> {
    let status = tasks::task_status(conn, org_id, task_id)?;
    if status == tasks::STATUS_DONE || status == tasks::STATUS_ARCHIVED {
        return Err("任务已完成或归档，不能再修改汇报".to_string());
    }
    Ok(())
}

/// 找该负责人最近一份「草稿 / 已退回」的汇报（用于继续编辑）。
fn find_editable_report(
    conn: &Connection,
    task_id: &str,
    reporter_id: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT id FROM task_reports
         WHERE task_id = ?1 AND reporter_id = ?2 AND status IN ('draft','returned')
         ORDER BY created_at DESC LIMIT 1",
        params![task_id, reporter_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("查询汇报失败：{}", e))
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
    ensure_assignee(role)?;
    reject_closed_task(conn, org_id, task_id)?;

    let now = chrono::Utc::now().timestamp();
    let materials_json = serde_json::to_string(materials).unwrap_or_else(|_| "[]".into());
    let deliverables_json = serde_json::to_string(deliverables).unwrap_or_else(|_| "[]".into());

    if let Some(id) = find_editable_report(conn, task_id, actor)? {
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
    ensure_manager(role)?;
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
    ensure_manager(role)?;
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
    fn only_assignee_can_submit() {
        assert!(ensure_assignee(TaskRole::Assignee).is_ok());
        for r in [TaskRole::Manager, TaskRole::Creator, TaskRole::Collaborator, TaskRole::Watcher] {
            assert!(ensure_assignee(r).is_err(), "{:?} 不应能提交汇报", r);
        }
    }

    #[test]
    fn only_manager_can_confirm() {
        assert!(ensure_manager(TaskRole::Manager).is_ok());
        assert!(ensure_manager(TaskRole::Creator).is_ok());
        assert!(ensure_manager(TaskRole::Assignee).is_err());
        assert!(ensure_manager(TaskRole::Watcher).is_err());
    }
}
