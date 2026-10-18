//! 飞地组织知识库：任务产出自动沉淀 + 检索（FTS5 trigram）。
//!
//! 每组织一个**隐式库**（`knowledge_bases.id = "kb-org-{orgId}"`，`is_implicit=1`），
//! 单人组织即归档地，成员增加后天然成为共享库。正文直接落 `enclave.db`
//! （`knowledge_base_memories`），同步维护 `kb_mem_fts` 全文索引。
//!
//! 模型口径（v7 起）：
//! - **一个任务一条知识**（`memory_id = "task:{taskId}"`）：汇报是版本流水，知识库只留当前有效版本；
//! - 交付物是**条目的附件**（`knowledge_base_files.memory_id`），重新确认时整体重建；
//! - 条目只由任务确认流程派生；恢复汇报 → `stale=1`（待更新），任务作废 → 撤下条目；
//! - owner / admin 可人工修改标题/正文/标签或移除非任务产出的条目。
//!
//! 幂等：确定性主键 + `ON CONFLICT DO UPDATE` + `task_reports.sedimented_at` 守卫。

use std::collections::HashMap;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
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
    /// 库内最近一次条目更新（无条目时为 0）
    pub last_entry_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 知识条目引用的交付物（同一份 `file_assets` 原件，不重复拷贝）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbFileView {
    pub file_id: String,
    pub file_name: String,
    pub file_type: String,
    pub file_size: i64,
    pub file_hash: String,
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
    /// 溯源信息：reportId / taskTitle / confirmedBy / confirmedAt / revision / editedBy / editedAt
    pub metadata: serde_json::Value,
    pub added_by: String,
    /// 首次沉淀时间
    pub added_at: i64,
    /// 最近一次覆盖时间
    pub updated_at: i64,
    /// 来源任务已恢复汇报：内容等待重新确认
    pub stale: bool,
    /// 条目自己的交付物列表
    pub files: Vec<KbFileView>,
}

/// 组织的隐式知识库 id。
pub fn implicit_kb_id(org_id: &str) -> String {
    format!("kb-org-{}", org_id)
}

fn snapshot_org_name(conn: &Connection, org_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT name FROM org_snapshot WHERE org_id = ?1",
        params![org_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .filter(|s| !s.trim().is_empty())
}

fn org_name(conn: &Connection, org_id: &str) -> String {
    snapshot_org_name(conn, org_id).unwrap_or_else(|| format!("组织 {}", org_id))
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
    // 组织改名后让知识库名称跟随（快照缺失时保留原名，避免退化成「组织 {id}」）
    if snapshot_org_name(conn, org_id).is_some() {
        conn.execute(
            "UPDATE knowledge_bases SET name = ?2, updated_at = ?3 WHERE id = ?1 AND name <> ?2",
            params![id, name, now],
        )
        .map_err(|e| format!("同步组织知识库名称失败：{}", e))?;
    }
    Ok(id)
}

/// 本组织的知识库列表（含隐式库；隐式库不存在时懒创建）。
pub fn list_kbs(conn: &Connection, org_id: &str) -> Result<Vec<KbView>, String> {
    ensure_org_kb(conn, org_id, "")?;
    let mut stmt = conn
        .prepare(
            "SELECT kb.id, kb.name, kb.description, kb.is_implicit, kb.created_at, kb.updated_at,
                    (SELECT COUNT(*) FROM knowledge_base_memories m WHERE m.knowledge_base_id = kb.id),
                    (SELECT COALESCE(MAX(COALESCE(NULLIF(m.updated_at, 0), m.added_at)), 0)
                     FROM knowledge_base_memories m WHERE m.knowledge_base_id = kb.id)
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
                last_entry_at: r.get(7)?,
            })
        })
        .map_err(|e| format!("查询组织知识库失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取组织知识库失败：{}", e))
}

fn map_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<KbEntryView> {
    let metadata_raw: String = r.get(6)?;
    let added_at: i64 = r.get(8)?;
    let updated_at: i64 = r.get(10)?;
    Ok(KbEntryView {
        memory_id: r.get(0)?,
        title: r.get(1)?,
        content: r.get(2)?,
        tags: r.get(3)?,
        origin: r.get(4)?,
        source_ref: r.get(5)?,
        metadata: serde_json::from_str(&metadata_raw)
            .unwrap_or_else(|_| serde_json::json!({})),
        added_by: r.get(7)?,
        added_at,
        updated_at: if updated_at == 0 { added_at } else { updated_at },
        stale: r.get::<_, i64>(9)? != 0,
        files: Vec::new(),
    })
}

const ENTRY_COLS: &str =
    "memory_id, title, content, tags, origin, source_ref, metadata, added_by, added_at, stale, updated_at";

fn qualified_cols(alias: &str) -> String {
    ENTRY_COLS
        .split(", ")
        .map(|c| format!("{}.{}", alias, c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 批量补齐条目的交付物（内连 `file_assets`：原件已被删除的悬空引用自然不出现）。
fn attach_files(
    conn: &Connection,
    kb_id: &str,
    entries: &mut [KbEntryView],
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let ids: Vec<String> = entries.iter().map(|e| e.memory_id.clone()).collect();
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT kbf.memory_id, fa.id, fa.file_name, fa.file_type, fa.file_size, fa.file_hash
         FROM knowledge_base_files kbf
         JOIN file_assets fa ON fa.id = kbf.file_id
         WHERE kbf.knowledge_base_id = ? AND kbf.memory_id IN ({})
         ORDER BY fa.file_name",
        placeholders
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("查询知识库文件失败：{}", e))?;
    let mut args: Vec<String> = Vec::with_capacity(ids.len() + 1);
    args.push(kb_id.to_string());
    args.extend(ids);
    let rows = stmt
        .query_map(params_from_iter(args.iter()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                KbFileView {
                    file_id: r.get(1)?,
                    file_name: r.get(2)?,
                    file_type: r.get(3)?,
                    file_size: r.get(4)?,
                    file_hash: r.get(5)?,
                },
            ))
        })
        .map_err(|e| format!("查询知识库文件失败：{}", e))?;
    let mut grouped: HashMap<String, Vec<KbFileView>> = HashMap::new();
    for row in rows {
        let (memory_id, file) = row.map_err(|e| format!("读取知识库文件失败：{}", e))?;
        grouped.entry(memory_id).or_default().push(file);
    }
    for e in entries.iter_mut() {
        if let Some(files) = grouped.remove(&e.memory_id) {
            e.files = files;
        }
    }
    Ok(())
}

/// 校验知识库属于该组织（默认拒绝）。
fn ensure_kb_owned(conn: &Connection, org_id: &str, kb_id: &str) -> Result<(), String> {
    let owned: Option<String> = conn
        .query_row(
            "SELECT id FROM knowledge_bases WHERE id = ?1 AND owner_type='org' AND owner_id = ?2",
            params![kb_id, org_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("查询组织知识库失败：{}", e))?;
    if owned.is_none() {
        return Err("知识库不存在或不属于该组织".to_string());
    }
    Ok(())
}

/// 检索组织知识库。
///
/// - 空查询 → 按更新时间倒序列出；
/// - 查询长度 ≥ 3 → 走 FTS5 trigram（短语匹配，避免特殊字符破坏 MATCH 语法）；
/// - 查询长度 < 3（中文字常见两字词）→ trigram 无法命中，退化为 `LIKE` 子串匹配；
/// - 两条路径都额外匹配交付物文件名。
pub fn search_entries(
    conn: &Connection,
    org_id: &str,
    kb_id: Option<&str>,
    query: &str,
    limit: i64,
) -> Result<Vec<KbEntryView>, String> {
    let kb = kb_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| implicit_kb_id(org_id));
    ensure_kb_owned(conn, org_id, &kb)?;

    let q = query.trim();
    let limit = limit.clamp(1, 200);
    let mut entries = if q.is_empty() {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM knowledge_base_memories
                 WHERE knowledge_base_id = ?1
                 ORDER BY COALESCE(NULLIF(updated_at, 0), added_at) DESC LIMIT ?2",
                ENTRY_COLS
            ))
            .map_err(|e| format!("查询知识条目失败：{}", e))?;
        let rows = stmt
            .query_map(params![kb, limit], map_entry)
            .map_err(|e| format!("查询知识条目失败：{}", e))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| format!("读取知识条目失败：{}", e))?
    } else {
        let like = format!("%{}%", q);
        if q.chars().count() >= 3 {
            // 短语查询：把用户输入整体作为字符串字面量，内部双引号翻倍转义
            let phrase = format!("\"{}\"", q.replace('"', "\"\""));
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {} FROM knowledge_base_memories m
                     WHERE m.knowledge_base_id = ?1
                       AND (m.memory_id IN (
                             SELECT f.memory_id FROM kb_mem_fts f
                             WHERE f.knowledge_base_id = ?1 AND kb_mem_fts MATCH ?2
                           )
                           OR EXISTS (
                             SELECT 1 FROM knowledge_base_files kbf
                             JOIN file_assets fa ON fa.id = kbf.file_id
                             WHERE kbf.knowledge_base_id = ?1
                               AND kbf.memory_id = m.memory_id
                               AND fa.file_name LIKE ?3))
                     ORDER BY COALESCE(NULLIF(m.updated_at, 0), m.added_at) DESC LIMIT ?4",
                    qualified_cols("m")
                ))
                .map_err(|e| format!("查询知识条目失败：{}", e))?;
            let rows = stmt
                .query_map(params![kb, phrase, like, limit], map_entry)
                .map_err(|e| format!("查询知识条目失败：{}", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| format!("读取知识条目失败：{}", e))?
        } else {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {} FROM knowledge_base_memories m
                     WHERE m.knowledge_base_id = ?1
                       AND (m.title LIKE ?2 OR m.content LIKE ?2 OR m.tags LIKE ?2
                            OR EXISTS (
                              SELECT 1 FROM knowledge_base_files kbf
                              JOIN file_assets fa ON fa.id = kbf.file_id
                              WHERE kbf.knowledge_base_id = ?1
                                AND kbf.memory_id = m.memory_id
                                AND fa.file_name LIKE ?2))
                     ORDER BY COALESCE(NULLIF(m.updated_at, 0), m.added_at) DESC LIMIT ?3",
                    qualified_cols("m")
                ))
                .map_err(|e| format!("查询知识条目失败：{}", e))?;
            let rows = stmt
                .query_map(params![kb, like, limit], map_entry)
                .map_err(|e| format!("查询知识条目失败：{}", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| format!("读取知识条目失败：{}", e))?
        }
    };
    attach_files(conn, &kb, &mut entries)?;
    Ok(entries)
}

/// 取单条（校验库归属）。
pub fn get_entry(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    memory_id: &str,
) -> Result<Option<KbEntryView>, String> {
    ensure_kb_owned(conn, org_id, kb_id)?;
    let mut entry = conn
        .query_row(
            &format!(
                "SELECT {} FROM knowledge_base_memories m
                 WHERE m.knowledge_base_id = ?1 AND m.memory_id = ?2",
                qualified_cols("m")
            ),
            params![kb_id, memory_id],
            map_entry,
        )
        .optional()
        .map_err(|e| format!("查询知识条目失败：{}", e))?;
    if let Some(e) = entry.as_mut() {
        attach_files(conn, kb_id, std::slice::from_mut(e))?;
    }
    Ok(entry)
}

/// 把已确认的汇报沉淀为组织知识库条目（**唯一自动触发点**：`reports::confirm_report`）。
///
/// 一个任务只保留一条条目：重新确认时覆盖正文与交付物集合，并清除「待更新」标记。
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
    // 任务 + 汇报（同时校验组织归属）
    let row = conn
        .query_row(
            "SELECT t.title, t.description, t.assignee_id, t.completed_at,
                    r.content, r.status, r.sedimented_at
             FROM task_reports r JOIN org_tasks t ON t.id = r.task_id
             WHERE r.id = ?1 AND r.task_id = ?2 AND t.org_id = ?3",
            params![report_id, task_id, org_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("读取汇报失败：{}", e))?
        .ok_or_else(|| "汇报不存在或不属于该组织".to_string())?;
    let (task_title, task_desc, assignee_id, completed_at, report_content, status, sedimented_at) =
        row;

    if status != crate::enclave::reports::REPORT_CONFIRMED {
        return Err("只有已确认的汇报才能沉淀".to_string());
    }
    if sedimented_at.is_some() {
        return Ok(()); // 幂等：已沉淀
    }

    let kb_id = ensure_org_kb(conn, org_id, actor)?;
    let memory_id = format!("task:{}", task_id);
    let now = chrono::Utc::now().timestamp();

    // 正文口径：已确认汇报正文；为空时回退任务描述（保证条目有内容可检索）
    let content = if report_content.trim().is_empty() {
        task_desc
    } else {
        report_content
    };

    // 修订号：同一条目每被确认一次 +1，便于前端显示「第 N 次确认」
    let revision = conn
        .query_row(
            "SELECT metadata FROM knowledge_base_memories
             WHERE knowledge_base_id = ?1 AND memory_id = ?2",
            params![kb_id, memory_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("读取知识条目失败：{}", e))?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("revision").and_then(|x| x.as_i64()))
        .unwrap_or(0)
        + 1;

    let metadata = serde_json::json!({
        "taskId": task_id,
        "reportId": report_id,
        "taskTitle": task_title,
        "assigneeId": assignee_id,
        "completedAt": completed_at,
        "confirmedBy": actor,
        "confirmedAt": now,
        "revision": revision,
    });

    // 清理旧口径（按汇报 id 生成的多条历史条目），避免同一任务重复沉淀
    conn.execute(
        "DELETE FROM knowledge_base_files
         WHERE knowledge_base_id = ?1 AND memory_id LIKE ?2",
        params![kb_id, format!("task:{}:report:%", task_id)],
    )
    .map_err(|e| format!("清理历史产出附件失败：{}", e))?;
    conn.execute(
        "DELETE FROM knowledge_base_memories
         WHERE knowledge_base_id = ?1 AND memory_id LIKE ?2",
        params![kb_id, format!("task:{}:report:%", task_id)],
    )
    .map_err(|e| format!("清理历史产出条目失败：{}", e))?;

    conn.execute(
        "INSERT INTO knowledge_base_memories
            (knowledge_base_id, memory_id, title, content, tags, origin, source_ref,
             metadata, added_by, added_at, updated_at, stale)
         VALUES (?1, ?2, ?3, ?4, '任务产出', 'task_output', ?5, ?6, ?7, ?8, ?8, 0)
         ON CONFLICT(knowledge_base_id, memory_id) DO UPDATE SET
            title = excluded.title,
            content = excluded.content,
            tags = excluded.tags,
            metadata = excluded.metadata,
            updated_at = excluded.updated_at,
            stale = 0",
        params![
            kb_id,
            memory_id,
            task_title,
            content,
            task_id,
            metadata.to_string(),
            actor,
            now
        ],
    )
    .map_err(|e| format!("写入组织知识库失败：{}", e))?;

    // 交付物：**按本次汇报整体重建**该条目的文件集合（先删后插），
    // 避免「恢复汇报 → 换交付物 → 重新确认」后旧文件继续挂在知识库。
    conn.execute(
        "DELETE FROM knowledge_base_files WHERE knowledge_base_id = ?1 AND memory_id = ?2",
        params![kb_id, memory_id],
    )
    .map_err(|e| format!("清理知识库旧交付物失败：{}", e))?;
    conn.execute(
        "INSERT OR IGNORE INTO knowledge_base_files
            (knowledge_base_id, memory_id, file_id, added_at)
         SELECT ?1, ?2, l.file_id, ?3
         FROM task_file_links l
         JOIN file_assets fa ON fa.id = l.file_id
         WHERE l.task_id = ?4 AND l.report_id = ?5",
        params![kb_id, memory_id, now, task_id, report_id],
    )
    .map_err(|e| format!("关联知识库文件失败：{}", e))?;

    conn.execute(
        "UPDATE task_reports SET sedimented_at = ?2, sediment_error = NULL WHERE id = ?1",
        params![report_id, now],
    )
    .map_err(|e| format!("更新沉淀状态失败：{}", e))?;
    Ok(())
}

/// 来源任务恢复汇报：条目标记为「待更新」（内容仍是上次确认的版本）。
pub fn mark_task_output_stale(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE knowledge_base_memories SET stale = 1
         WHERE knowledge_base_id = ?1 AND memory_id = ?2 AND stale = 0",
        params![implicit_kb_id(org_id), format!("task:{}", task_id)],
    )
    .map_err(|e| format!("标记知识条目待更新失败：{}", e))?;
    Ok(())
}

/// 任务作废：撤下该任务的产出条目及其附件关联（知识库只保留有效产出）。
pub fn withdraw_task_output(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
) -> Result<(), String> {
    let kb_id = implicit_kb_id(org_id);
    let memory_id = format!("task:{}", task_id);
    conn.execute(
        "DELETE FROM knowledge_base_files WHERE knowledge_base_id = ?1 AND memory_id = ?2",
        params![kb_id, memory_id],
    )
    .map_err(|e| format!("撤下知识条目附件失败：{}", e))?;
    conn.execute(
        "DELETE FROM knowledge_base_memories WHERE knowledge_base_id = ?1 AND memory_id = ?2",
        params![kb_id, memory_id],
    )
    .map_err(|e| format!("撤下知识条目失败：{}", e))?;
    Ok(())
}

/// 人工修改条目（仅 owner / admin，授权在命令层）。
///
/// 修改不会改变 `source_ref` 与文件集合；再次确认汇报时正文仍以汇报为准覆盖。
pub fn update_entry(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    memory_id: &str,
    title: &str,
    content: &str,
    tags: &str,
    actor: &str,
) -> Result<KbEntryView, String> {
    ensure_kb_owned(conn, org_id, kb_id)?;
    let title = title.trim();
    if title.is_empty() {
        return Err("标题不能为空".to_string());
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT metadata FROM knowledge_base_memories
             WHERE knowledge_base_id = ?1 AND memory_id = ?2",
            params![kb_id, memory_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("读取知识条目失败：{}", e))?;
    let metadata_raw = existing.ok_or_else(|| "知识条目不存在".to_string())?;

    let mut metadata = serde_json::from_str::<serde_json::Value>(&metadata_raw)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    metadata.insert("editedBy".into(), serde_json::json!(actor));
    metadata.insert("editedAt".into(), serde_json::json!(now));

    conn.execute(
        "UPDATE knowledge_base_memories
         SET title = ?3, content = ?4, tags = ?5, metadata = ?6, updated_at = ?7
         WHERE knowledge_base_id = ?1 AND memory_id = ?2",
        params![
            kb_id,
            memory_id,
            title,
            content.trim(),
            tags.trim(),
            serde_json::Value::Object(metadata).to_string(),
            now
        ],
    )
    .map_err(|e| format!("保存知识条目失败：{}", e))?;

    get_entry(conn, org_id, kb_id, memory_id)?
        .ok_or_else(|| "知识条目不存在".to_string())
}

/// 移除条目（仅 owner / admin，授权在命令层）；附件关联一并清除，原件仍归任务侧管理。
pub fn remove_entry(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    memory_id: &str,
) -> Result<(), String> {
    ensure_kb_owned(conn, org_id, kb_id)?;
    conn.execute(
        "DELETE FROM knowledge_base_files WHERE knowledge_base_id = ?1 AND memory_id = ?2",
        params![kb_id, memory_id],
    )
    .map_err(|e| format!("移除知识条目附件失败：{}", e))?;
    let removed = conn
        .execute(
            "DELETE FROM knowledge_base_memories WHERE knowledge_base_id = ?1 AND memory_id = ?2",
            params![kb_id, memory_id],
        )
        .map_err(|e| format!("移除知识条目失败：{}", e))?;
    if removed == 0 {
        return Err("知识条目不存在".to_string());
    }
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
        assert!(qualified_cols("m").starts_with("m.memory_id"));
    }
}
