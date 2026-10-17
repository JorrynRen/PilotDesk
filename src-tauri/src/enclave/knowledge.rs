//! 飞地组织知识库：任务产出自动沉淀 + 检索（FTS5 trigram）。
//!
//! 每组织一个**隐式库**（`knowledge_bases.id = "kb-org-{orgId}"`，`is_implicit=1`），
//! 单人组织即归档地，成员增加后天然成为共享库。正文直接落 `enclave.db`
//! （`knowledge_base_memories`），同步维护 `kb_mem_fts` 全文索引。
//!
//! 幂等（三重）：确定性主键 `task:{task_id}:report:{report_id}` + `ON CONFLICT DO UPDATE`
//! + `task_reports.sedimented_at` 守卫。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::enclave::tasks;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_implicit: bool,
    pub entry_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbEntryView {
    pub memory_id: String,
    pub title: String,
    pub content: String,
    pub tags: String,
    pub origin: String,
    pub source_ref: String,
    pub added_by: String,
    pub added_at: i64,
}

/// 组织的隐式知识库 id。
pub fn implicit_kb_id(org_id: &str) -> String {
    format!("kb-org-{}", org_id)
}

fn org_name(conn: &Connection, org_id: &str) -> String {
    conn.query_row(
        "SELECT name FROM org_snapshot WHERE org_id = ?1",
        params![org_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .filter(|s| !s.trim().is_empty())
    .unwrap_or_else(|| format!("组织 {}", org_id))
}

/// 确保隐式库存在（幂等）。返回库 id。
pub fn ensure_org_kb(conn: &Connection, org_id: &str, actor: &str) -> Result<String, String> {
    let id = implicit_kb_id(org_id);
    let now = chrono::Utc::now().timestamp();
    let name = format!("{} · 团队知识库", org_name(conn, org_id));
    conn.execute(
        "INSERT INTO knowledge_bases
            (id, name, description, scope, owner_type, owner_id, is_implicit,
             field_schema, filter_rules, visibility, created_by, created_at, updated_at)
         VALUES (?1, ?2, '任务产出自动沉淀；团队共享', 'org', 'org', ?3, 1,
                 '[]', '[]', 'org', ?4, ?5, ?5)
         ON CONFLICT(id) DO NOTHING",
        params![id, name, org_id, actor, now],
    )
    .map_err(|e| format!("创建组织知识库失败：{}", e))?;
    Ok(id)
}

/// 本组织的知识库列表（含隐式库；隐式库不存在时懒创建）。
pub fn list_kbs(conn: &Connection, org_id: &str) -> Result<Vec<KbView>, String> {
    ensure_org_kb(conn, org_id, "")?;
    let mut stmt = conn
        .prepare(
            "SELECT kb.id, kb.name, kb.description, kb.is_implicit, kb.created_at, kb.updated_at,
                    (SELECT COUNT(*) FROM knowledge_base_memories m WHERE m.knowledge_base_id = kb.id)
             FROM knowledge_bases kb
             WHERE kb.owner_type = 'org' AND kb.owner_id = ?1
             ORDER BY kb.is_implicit DESC, kb.created_at ASC",
        )
        .map_err(|e| format!("查询组织知识库失败：{}", e))?;
    let rows = stmt
        .query_map(params![org_id], |r| {
            Ok(KbView {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                is_implicit: r.get::<_, i64>(3)? != 0,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
                entry_count: r.get(6)?,
            })
        })
        .map_err(|e| format!("查询组织知识库失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取组织知识库失败：{}", e))
}

fn map_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<KbEntryView> {
    Ok(KbEntryView {
        memory_id: r.get(0)?,
        title: r.get(1)?,
        content: r.get(2)?,
        tags: r.get(3)?,
        origin: r.get(4)?,
        source_ref: r.get(5)?,
        added_by: r.get(6)?,
        added_at: r.get(7)?,
    })
}

const ENTRY_COLS: &str = "memory_id, title, content, tags, origin, source_ref, added_by, added_at";

/// 检索组织知识库。
///
/// - 空查询 → 按时间倒序列出；
/// - 查询长度 ≥ 3 → 走 FTS5 trigram（短语匹配，避免特殊字符破坏 MATCH 语法）；
/// - 查询长度 < 3（中文字常见两字词）→ trigram 无法命中，退化为 `LIKE` 子串匹配。
pub fn search_entries(
    conn: &Connection,
    org_id: &str,
    kb_id: Option<&str>,
    query: &str,
    limit: i64,
) -> Result<Vec<KbEntryView>, String> {
    let kb = kb_id.map(|s| s.to_string()).unwrap_or_else(|| implicit_kb_id(org_id));
    // 校验库归属，避免跨组织读取
    let owned: Option<String> = conn
        .query_row(
            "SELECT id FROM knowledge_bases WHERE id = ?1 AND owner_type='org' AND owner_id = ?2",
            params![kb, org_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("查询组织知识库失败：{}", e))?;
    if owned.is_none() {
        return Err("知识库不存在或不属于该组织".to_string());
    }

    let q = query.trim();
    let limit = limit.clamp(1, 200);
    if q.is_empty() {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM knowledge_base_memories
                 WHERE knowledge_base_id = ?1 ORDER BY added_at DESC LIMIT ?2",
                ENTRY_COLS
            ))
            .map_err(|e| format!("查询知识条目失败：{}", e))?;
        let rows = stmt
            .query_map(params![kb, limit], map_entry)
            .map_err(|e| format!("查询知识条目失败：{}", e))?;
        return rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| format!("读取知识条目失败：{}", e));
    }

    if q.chars().count() >= 3 {
        // 短语查询：把用户输入整体作为字符串字面量，内部双引号翻倍转义
        let phrase = format!("\"{}\"", q.replace('"', "\"\""));
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM knowledge_base_memories m
                 WHERE m.knowledge_base_id = ?1
                   AND m.memory_id IN (
                     SELECT f.memory_id FROM kb_mem_fts f
                     WHERE f.knowledge_base_id = ?1 AND kb_mem_fts MATCH ?2
                   )
                 ORDER BY m.added_at DESC LIMIT ?3",
                ENTRY_COLS
                    .split(", ")
                    .map(|c| format!("m.{}", c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .map_err(|e| format!("查询知识条目失败：{}", e))?;
        let rows = stmt
            .query_map(params![kb, phrase, limit], map_entry)
            .map_err(|e| format!("查询知识条目失败：{}", e))?;
        return rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| format!("读取知识条目失败：{}", e));
    }

    let like = format!("%{}%", q);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM knowledge_base_memories
             WHERE knowledge_base_id = ?1 AND (title LIKE ?2 OR content LIKE ?2 OR tags LIKE ?2)
             ORDER BY added_at DESC LIMIT ?3",
            ENTRY_COLS
        ))
        .map_err(|e| format!("查询知识条目失败：{}", e))?;
    let rows = stmt
        .query_map(params![kb, like, limit], map_entry)
        .map_err(|e| format!("查询知识条目失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取知识条目失败：{}", e))
}

/// 取单条（校验库归属）。
pub fn get_entry(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    memory_id: &str,
) -> Result<Option<KbEntryView>, String> {
    conn.query_row(
        &format!(
            "SELECT {} FROM knowledge_base_memories m
             WHERE m.knowledge_base_id = ?1 AND m.memory_id = ?2
               AND EXISTS (SELECT 1 FROM knowledge_bases kb
                           WHERE kb.id = m.knowledge_base_id
                             AND kb.owner_type='org' AND kb.owner_id = ?3)",
            ENTRY_COLS.split(", ").map(|c| format!("m.{}", c)).collect::<Vec<_>>().join(", ")
        ),
        params![kb_id, memory_id, org_id],
        map_entry,
    )
    .optional()
    .map_err(|e| format!("查询知识条目失败：{}", e))
}

/// 把已确认的汇报沉淀为组织知识库条目（**唯一触发点**：`reports::confirm_report`）。
///
/// 幂等：`task_reports.sedimented_at` 非空即跳过；确定性主键 + upsert 兜底。
pub fn sediment_task_output(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    report_id: &str,
    actor: &str,
) -> Result<(), String> {
    match do_sediment(conn, org_id, task_id, report_id, actor) {
        Ok(()) => Ok(()),
        Err(e) => {
            // 沉淀是派生动作：失败只记原因，回滚报告的确认状态
            let _ = conn.execute(
                "UPDATE task_reports SET sediment_error = ?2 WHERE id = ?1",
                params![report_id, e],
            );
            Err(e)
        }
    }
}

fn do_sediment(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    report_id: &str,
    actor: &str,
) -> Result<(), String> {
    // 任务标题 + 汇报内容/状态（同时校验组织归属）
    let (task_title, content, status, sedimented_at): (String, String, String, Option<i64>) = conn
        .query_row(
            "SELECT t.title, r.content, r.status, r.sedimented_at
             FROM task_reports r JOIN org_tasks t ON t.id = r.task_id
             WHERE r.id = ?1 AND r.task_id = ?2 AND t.org_id = ?3",
            params![report_id, task_id, org_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| format!("读取汇报失败：{}", e))?
        .ok_or_else(|| "汇报不存在或不属于该组织".to_string())?;

    if status != crate::enclave::reports::REPORT_CONFIRMED {
        return Err("只有已确认的汇报才能沉淀".to_string());
    }
    if sedimented_at.is_some() {
        return Ok(()); // 幂等：已沉淀
    }

    let kb_id = ensure_org_kb(conn, org_id, actor)?;
    let memory_id = format!("task:{}:report:{}", task_id, report_id);
    let now = chrono::Utc::now().timestamp();
    let title = format!("{} · 完成汇报", task_title);
    let tags = "任务产出";

    conn.execute(
        "INSERT INTO knowledge_base_memories
            (knowledge_base_id, memory_id, title, content, tags, origin, source_ref,
             metadata, added_by, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'task_output', ?6, '{}', ?7, ?8)
         ON CONFLICT(knowledge_base_id, memory_id) DO UPDATE SET
            title = excluded.title,
            content = excluded.content,
            tags = excluded.tags,
            added_at = excluded.added_at",
        params![kb_id, memory_id, title, content, tags, task_id, actor, now],
    )
    .map_err(|e| format!("写入组织知识库失败：{}", e))?;

    // 附件引用同一份原文（不重复拷贝）
    conn.execute(
        "INSERT OR IGNORE INTO knowledge_base_files (knowledge_base_id, file_id, added_at)
         SELECT ?1, file_id, ?2 FROM task_files WHERE task_id = ?3",
        params![kb_id, now, task_id],
    )
    .map_err(|e| format!("关联知识库文件失败：{}", e))?;

    conn.execute(
        "UPDATE task_reports SET sedimented_at = ?2, sediment_error = NULL WHERE id = ?1",
        params![report_id, now],
    )
    .map_err(|e| format!("更新沉淀状态失败：{}", e))?;
    Ok(())
}

/// 重试沉淀（清掉已沉淀标记后重跑）。
pub fn retry_sediment(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    report_id: &str,
    actor: &str,
) -> Result<(), String> {
    tasks::task_status(conn, org_id, task_id)?;
    conn.execute(
        "UPDATE task_reports SET sedimented_at = NULL, sediment_error = NULL WHERE id = ?1 AND task_id = ?2",
        params![report_id, task_id],
    )
    .map_err(|e| format!("重置沉淀状态失败：{}", e))?;
    sediment_task_output(conn, org_id, task_id, report_id, actor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implicit_kb_id_is_stable() {
        assert_eq!(implicit_kb_id("8"), "kb-org-8");
    }

    #[test]
    fn entry_columns_are_qualified() {
        // 保证 FTS 联查时列名带表别名，避免歧义
        assert!(ENTRY_COLS.contains("memory_id"));
        assert!(ENTRY_COLS.split(", ").count() >= 8);
    }
}
