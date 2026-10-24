//! 团队报告与绩效派生（只读聚合）。
//!
//! 全部指标都由 `enclave.db` 的既有业务数据推导，不新增业务状态：
//! - 任务：周期内新增 / 完成、当前进行中 / 待确认 / 逾期；
//! - 汇报：周期内确认数；
//! - 知识：周期内新增条目与交付物（按组织知识库统计）；
//! - 成员：按负责人聚合完成数、在办数、逾期数与平均完成耗时。
//!
//! 设计口径：**不做自动绩效评级**，只提供可核对的数据（设计文档 §13 第三阶段）。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// 成员维度的绩效数据（都是可核对的计数，不含评分）。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberStat {
    pub user_id: String,
    /// 本周期完成的任务数
    pub completed: i64,
    /// 当前进行中
    pub in_progress: i64,
    /// 当前待确认（已提交汇报）
    pub submitted: i64,
    /// 当前逾期（有截止时间且已过，且未完成 / 未归档 / 未作废）
    pub overdue: i64,
    /// 本周期完成任务的「开始 → 完成」平均小时数（0 = 没有可算的样本）
    pub avg_hours: f64,
}

/// 一个周期的团队报告。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamStats {
    /// 周期起点（unix 秒，含）
    pub from: i64,
    /// 周期终点（unix 秒，不含）
    pub to: i64,
    pub generated_at: i64,
    pub created: i64,
    pub completed: i64,
    pub reports_confirmed: i64,
    pub entries_added: i64,
    pub files_added: i64,
    pub members: Vec<MemberStat>,
}

/// 统计一个组织在 `[from, to)` 内的数据（授权由调用方完成）。
pub fn team_stats(
    conn: &Connection,
    org_id: &str,
    from: i64,
    to: i64,
) -> Result<TeamStats, String> {
    if from <= 0 || to <= from {
        return Err("时间范围不正确".to_string());
    }
    let now = chrono::Utc::now().timestamp();
    let count = |sql: &str| -> Result<i64, String> {
        conn.query_row(sql, params![org_id, from, to], |r| r.get(0))
            .map_err(|e| format!("统计失败：{}", e))
    };

    let created = count(
        "SELECT COUNT(*) FROM org_tasks
         WHERE org_id = ?1 AND created_at >= ?2 AND created_at < ?3",
    )?;
    let completed = count(
        "SELECT COUNT(*) FROM org_tasks
         WHERE org_id = ?1 AND completed_at IS NOT NULL
           AND completed_at >= ?2 AND completed_at < ?3",
    )?;
    let reports_confirmed = count(
        "SELECT COUNT(*) FROM task_reports r
         JOIN org_tasks t ON t.id = r.task_id
         WHERE t.org_id = ?1 AND r.confirmed_at IS NOT NULL
           AND r.confirmed_at >= ?2 AND r.confirmed_at < ?3",
    )?;
    let entries_added = count(
        "SELECT COUNT(*) FROM knowledge_base_memories m
         JOIN knowledge_bases kb ON kb.id = m.knowledge_base_id
         WHERE kb.owner_type = 'org' AND kb.owner_id = ?1
           AND m.added_at >= ?2 AND m.added_at < ?3",
    )?;
    let files_added = count(
        "SELECT COUNT(DISTINCT f.file_id) FROM knowledge_base_files f
         JOIN knowledge_bases kb ON kb.id = f.knowledge_base_id
         WHERE kb.owner_type = 'org' AND kb.owner_id = ?1
           AND f.added_at >= ?2 AND f.added_at < ?3",
    )?;

    let mut stmt = conn
        .prepare(
            "SELECT assignee_id,
                    COALESCE(SUM(CASE WHEN completed_at >= ?2 AND completed_at < ?3 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'in_progress' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'submitted' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status IN ('pending', 'in_progress', 'submitted')
                                       AND due_date IS NOT NULL AND due_date < ?4 THEN 1 ELSE 0 END), 0),
                    AVG(CASE WHEN completed_at >= ?2 AND completed_at < ?3
                             THEN (completed_at - COALESCE(started_at, created_at)) / 3600.0
                        END)
             FROM org_tasks
             WHERE org_id = ?1 AND assignee_id IS NOT NULL AND assignee_id <> ''
             GROUP BY assignee_id",
        )
        .map_err(|e| format!("统计成员数据失败：{}", e))?;
    let members = stmt
        .query_map(params![org_id, from, to, now], |r| {
            Ok(MemberStat {
                user_id: r.get(0)?,
                completed: r.get(1)?,
                in_progress: r.get(2)?,
                submitted: r.get(3)?,
                overdue: r.get(4)?,
                avg_hours: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
            })
        })
        .map_err(|e| format!("统计成员数据失败：{}", e))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取成员统计失败：{}", e))?;

    let mut members = members;
    // 完成多的排前面；完成数相同按在办、待确认、逾期
    members.sort_by(|a, b| {
        b.completed
            .cmp(&a.completed)
            .then(b.in_progress.cmp(&a.in_progress))
            .then(b.submitted.cmp(&a.submitted))
            .then(b.overdue.cmp(&a.overdue))
            .then(a.user_id.cmp(&b.user_id))
    });

    Ok(TeamStats {
        from,
        to,
        generated_at: now,
        created,
        completed,
        reports_confirmed,
        entries_added,
        files_added,
        members,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::enclave::db::ENCLAVE_SCHEMA_SQL)
            .unwrap();
        conn.execute(
            "INSERT INTO local_identity (user_id, email, nickname, synced_at)
             VALUES ('1', 'a@b.c', '测试', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id, synced_at)
             VALUES ('8', '测试组织', '1', 'owner', '1', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO knowledge_bases
                (id, name, description, scope, owner_type, owner_id, is_implicit,
                 field_schema, filter_rules, visibility, created_by, created_at, updated_at)
             VALUES ('kb-org-8', '团队库', '', 'org', 'org', '8', 1, '[]', '[]', 'org', '1', 0, 0)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn stats_aggregate_period_and_members() {
        let conn = seeded();
        // 周期 [1000, 2000)：周期内完成 1 个（耗时 2h）、新增 1 个；周期外完成 1 个
        conn.execute(
            "INSERT INTO org_tasks
                (id, org_id, title, creator_id, assignee_id, status, due_date,
                 started_at, completed_at, created_at, updated_at)
             VALUES ('t1', '8', '周期内', '1', '2', 'done', 1500, 1000, 1720, 1010, 1720)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO org_tasks
                (id, org_id, title, creator_id, assignee_id, status, due_date,
                 started_at, completed_at, created_at, updated_at)
             VALUES ('t2', '8', '周期外', '1', '2', 'done', NULL, 300, 400, 300, 400)",
            [],
        )
        .unwrap();
        // 进行中且逾期
        conn.execute(
            "INSERT INTO org_tasks
                (id, org_id, title, creator_id, assignee_id, status, due_date,
                 created_at, updated_at)
             VALUES ('t3', '8', '逾期', '1', '3', 'in_progress', 1200, 900, 900)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_reports
                (id, task_id, reporter_id, content, status, confirmed_at, created_at, updated_at)
             VALUES ('r1', 't1', '2', '完成', 'confirmed', 1750, 1700, 1750)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO knowledge_base_memories
                (knowledge_base_id, memory_id, title, content, added_by, added_at, updated_at)
             VALUES ('kb-org-8', 'task:t1', '任务产出', '内容', '1', 1710, 1710)",
            [],
        )
        .unwrap();

        let s = team_stats(&conn, "8", 1000, 2000).unwrap();
        assert_eq!(s.created, 1);
        assert_eq!(s.completed, 1);
        assert_eq!(s.reports_confirmed, 1);
        assert_eq!(s.entries_added, 1);
        assert_eq!(s.members.len(), 2);
        let m2 = s.members.iter().find(|m| m.user_id == "2").unwrap();
        assert_eq!(m2.completed, 1);
        // started_at=1000，completed_at=1720 → 720s = 0.2h
        assert!(
            (m2.avg_hours - 0.2).abs() < 1e-6,
            "平均耗时按开始→完成计算，实际 {}",
            m2.avg_hours
        );
        let m3 = s.members.iter().find(|m| m.user_id == "3").unwrap();
        assert_eq!(m3.in_progress, 1);
        assert_eq!(m3.overdue, 1);
    }
}
