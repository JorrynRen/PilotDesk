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
use serde::{Deserialize, Serialize};

use crate::enclave::tasks;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_implicit: bool,
    pub entry_count: i64,
    /// 库内交付物文件数（按 file_id 去重）
    pub file_count: i64,
    /// 库内最近一次条目更新（无条目时为 0）
    pub last_entry_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 知识条目引用的交付物（同一份 `file_assets` 原件，不重复拷贝）。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbFileView {
    pub file_id: String,
    pub file_name: String,
    pub file_type: String,
    pub file_size: i64,
    pub file_hash: String,
}

#[derive(Serialize, Deserialize)]
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
    // 名称只在首次创建时取组织名；之后允许 owner / admin 人工修改，不再被组织名覆盖
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
                     FROM knowledge_base_memories m WHERE m.knowledge_base_id = kb.id),
                    (SELECT COUNT(DISTINCT f.file_id)
                     FROM knowledge_base_files f
                     JOIN file_assets fa ON fa.id = f.file_id
                     WHERE f.knowledge_base_id = kb.id)
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
                file_count: r.get(8)?,
            })
        })
        .map_err(|e| format!("查询组织知识库失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取组织知识库失败：{}", e))
}

/// 人工维护知识库信息（仅 owner / admin，授权在命令层）：名称 + 描述。
pub fn update_kb(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    name: &str,
    description: &str,
) -> Result<KbView, String> {
    ensure_kb_owned(conn, org_id, kb_id)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("知识库名称不能为空".to_string());
    }
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE knowledge_bases SET name = ?3, description = ?4, updated_at = ?5
         WHERE id = ?1 AND owner_type = 'org' AND owner_id = ?2",
        params![kb_id, org_id, name, description.trim(), now],
    )
    .map_err(|e| format!("保存知识库信息失败：{}", e))?;
    list_kbs(conn, org_id)?
        .into_iter()
        .find(|k| k.id == kb_id)
        .ok_or_else(|| "知识库不存在或不属于该组织".to_string())
}

/// 文件视图（团队知识库「文件」页）：一行 = 某条目引用的一个交付物。
///
/// 同一份原件可能被多个条目引用，因此按 (memory_id, file_id) 展开；
/// 前端据此按「来源任务 / 知识条目」建目录树。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbFileListView {
    pub file_id: String,
    pub file_name: String,
    pub file_type: String,
    pub file_size: i64,
    pub file_hash: String,
    pub memory_id: String,
    pub entry_title: String,
    pub source_ref: String,
    /// 来源任务的负责人 / 优先级：供「按负责人 / 优先级」分组
    pub assignee_id: String,
    pub priority: String,
    pub added_at: i64,
}

/// 团队知识库的全部交付物（按条目展开，供前端建目录树）。
pub fn list_files(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
) -> Result<Vec<KbFileListView>, String> {
    ensure_kb_owned(conn, org_id, kb_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT fa.id, fa.file_name, fa.file_type, fa.file_size, fa.file_hash,
                    kbf.memory_id, COALESCE(m.title, ''), COALESCE(m.source_ref, ''), kbf.added_at,
                    COALESCE(t.assignee_id, ''), COALESCE(t.priority, 'normal')
             FROM knowledge_base_files kbf
             JOIN file_assets fa ON fa.id = kbf.file_id
             LEFT JOIN knowledge_base_memories m
               ON m.knowledge_base_id = kbf.knowledge_base_id
              AND m.memory_id = kbf.memory_id
             LEFT JOIN org_tasks t ON t.id = m.source_ref
             WHERE kbf.knowledge_base_id = ?1
             ORDER BY kbf.added_at DESC, fa.file_name",
        )
        .map_err(|e| format!("查询知识库文件失败：{}", e))?;
    let rows = stmt
        .query_map(params![kb_id], |r| {
            Ok(KbFileListView {
                file_id: r.get(0)?,
                file_name: r.get(1)?,
                file_type: r.get(2)?,
                file_size: r.get(3)?,
                file_hash: r.get(4)?,
                memory_id: r.get(5)?,
                entry_title: r.get(6)?,
                source_ref: r.get(7)?,
                added_at: r.get(8)?,
                assignee_id: r.get(9)?,
                priority: r.get(10)?,
            })
        })
        .map_err(|e| format!("查询知识库文件失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取知识库文件失败：{}", e))
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

/* ── 人工投喂与「待整理」队列 ──
 *
 * 口径与个人库一致：**入库永远可用，加工按能力降级**。
 * 主机是唯一权威：成员端的投喂命令经 WS 转发到这里执行，队列也只有主机消费。
 */

/// 待整理队列的一行（主机权威）。
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KbIngestQueueView {
    pub id: String,
    pub org_id: String,
    pub kb_id: String,
    /// snippet / url / file
    pub kind: String,
    pub source_ref: String,
    pub payload: String,
    /// pending / running / failed
    pub status: String,
    pub attempts: i64,
    pub last_error: String,
    pub created_at: i64,
    pub updated_at: i64,
}

fn queue_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<KbIngestQueueView> {
    Ok(KbIngestQueueView {
        id: r.get(0)?,
        org_id: r.get(1)?,
        kb_id: r.get(2)?,
        kind: r.get(3)?,
        source_ref: r.get(4)?,
        payload: r.get(5)?,
        status: r.get(6)?,
        attempts: r.get(7)?,
        last_error: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

/// 组织成员即可贡献组织知识库（设计 §8.2：成员具备 KB Editor 能力）。
pub fn ensure_kb_write(conn: &Connection, org_id: &str) -> Result<(), String> {
    crate::enclave::authz::role_for_org(conn, org_id)?
        .map(|_| ())
        .ok_or_else(|| "尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string())
}

pub fn enqueue_ingest(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    kind: &str,
    source_ref: &str,
    payload: &str,
) -> Result<String, String> {
    ensure_kb_write(conn, org_id)?;
    ensure_kb_owned(conn, org_id, kb_id)?;
    let now = chrono::Utc::now().timestamp();
    let id = format!("kbq:{}", uuid::Uuid::new_v4());
    conn.execute(
        "INSERT INTO kb_ingest_queue
            (id, org_id, kb_id, kind, source_ref, payload, status, attempts, last_error, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, '', ?7, ?7)
         ON CONFLICT(org_id, kb_id, kind, source_ref) DO UPDATE SET
            payload = excluded.payload,
            updated_at = excluded.updated_at,
            status = CASE WHEN kb_ingest_queue.status = 'failed' THEN 'pending'
                          ELSE kb_ingest_queue.status END",
        params![id, org_id, kb_id, kind, source_ref, payload, now],
    )
    .map_err(|e| format!("写入待整理队列失败：{}", e))?;
    conn.query_row(
        "SELECT id FROM kb_ingest_queue
         WHERE org_id = ?1 AND kb_id = ?2 AND kind = ?3 AND source_ref = ?4",
        params![org_id, kb_id, kind, source_ref],
        |r| r.get(0),
    )
    .map_err(|e| format!("读取待整理队列失败：{}", e))
}

pub fn list_ingest_queue(
    conn: &Connection,
    org_id: &str,
    kb_id: Option<&str>,
) -> Result<Vec<KbIngestQueueView>, String> {
    ensure_kb_write(conn, org_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, org_id, kb_id, kind, source_ref, payload, status, attempts,
                    last_error, created_at, updated_at
             FROM kb_ingest_queue
             WHERE org_id = ?1 AND (?2 IS NULL OR kb_id = ?2)
             ORDER BY CASE status WHEN 'failed' THEN 1 WHEN 'pending' THEN 0 ELSE 2 END,
                      created_at ASC
             LIMIT 100",
        )
        .map_err(|e| format!("查询待整理队列失败：{}", e))?;
    stmt.query_map(params![org_id, kb_id], queue_row)
        .map(|rows| rows.flatten().collect())
        .map_err(|e| format!("读取待整理队列失败：{}", e))
}

pub fn ingest_queue_pending(conn: &Connection, org_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM kb_ingest_queue
         WHERE org_id = ?1 AND status IN ('pending', 'running')",
        params![org_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// 取一条待整理并标记 running（单消费者）。
pub fn claim_next_ingest(
    conn: &Connection,
    org_id: &str,
) -> Result<Option<KbIngestQueueView>, String> {
    let row = conn
        .query_row(
            "SELECT id, org_id, kb_id, kind, source_ref, payload, status, attempts,
                    last_error, created_at, updated_at
             FROM kb_ingest_queue
             WHERE org_id = ?1 AND status = 'pending'
             ORDER BY created_at ASC LIMIT 1",
            params![org_id],
            queue_row,
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(row) = row else { return Ok(None) };
    let changed = conn
        .execute(
            "UPDATE kb_ingest_queue SET status = 'running', updated_at = ?2
             WHERE id = ?1 AND status = 'pending'",
            params![row.id, chrono::Utc::now().timestamp()],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Ok(None);
    }
    Ok(Some(row))
}

pub fn finish_ingest(conn: &Connection, id: &str) {
    let _ = conn.execute("DELETE FROM kb_ingest_queue WHERE id = ?1", params![id]);
}

pub fn fail_ingest(conn: &Connection, id: &str, error: &str) {
    let _ = conn.execute(
        "UPDATE kb_ingest_queue
            SET attempts = attempts + 1,
                last_error = ?2,
                status = CASE WHEN attempts + 1 >= 5 THEN 'failed' ELSE 'pending' END,
                updated_at = ?3
          WHERE id = ?1",
        params![id, error, chrono::Utc::now().timestamp()],
    );
}

pub fn reset_running_ingest(conn: &Connection, org_id: &str) {
    let _ = conn.execute(
        "UPDATE kb_ingest_queue SET status = 'pending'
         WHERE org_id = ?1 AND status = 'running'",
        params![org_id],
    );
}

pub fn retry_ingest(conn: &Connection, org_id: &str, id: &str) -> Result<(), String> {
    ensure_kb_write(conn, org_id)?;
    conn.execute(
        "UPDATE kb_ingest_queue
            SET status = 'pending', attempts = 0, last_error = '', updated_at = ?2
          WHERE id = ?1 AND org_id = ?3",
        params![id, chrono::Utc::now().timestamp(), org_id],
    )
    .map_err(|e| format!("重试失败：{}", e))?;
    Ok(())
}

pub fn drop_ingest(conn: &Connection, org_id: &str, id: &str) -> Result<bool, String> {
    ensure_kb_write(conn, org_id)?;
    conn.execute(
        "DELETE FROM kb_ingest_queue WHERE id = ?1 AND org_id = ?2",
        params![id, org_id],
    )
    .map(|n| n > 0)
    .map_err(|e| format!("移除失败：{}", e))
}

/// 一条待写入的投喂产物。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestEntryInput {
    pub memory_id: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: String,
    /// 溯源元信息（模型给出的专属属性等），JSON 字符串
    #[serde(default)]
    pub metadata: String,
}

/// 把整理好的条目写入组织库（幂等：同来源重跑时先清旧条目）。
pub fn write_ingest_entries(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    origin: &str,
    source_ref: &str,
    actor: &str,
    entries: &[IngestEntryInput],
) -> Result<usize, String> {
    ensure_kb_write(conn, org_id)?;
    ensure_kb_owned(conn, org_id, kb_id)?;
    if entries.is_empty() {
        return Ok(0);
    }
    let now = chrono::Utc::now().timestamp();
    // 人工改过的条目锁定：重跑只清理"模型生成且未被人工编辑"的旧条目。
    // `update_entry` 会写 editedBy / editedAt，这里以它作为锁定标记。
    let mut stmt = conn
        .prepare(
            "SELECT memory_id FROM knowledge_base_memories
             WHERE knowledge_base_id = ?1 AND origin = ?2 AND source_ref = ?3
               AND metadata LIKE '%\"editedBy\"%'",
        )
        .map_err(|e| format!("读取人工编辑条目失败：{}", e))?;
    let preserved: Vec<String> = stmt
        .query_map(params![kb_id, origin, source_ref], |r| r.get(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    drop(stmt);

    // 重跑幂等：先摘掉同来源的旧条目（只动该 origin，避免误伤任务产出）
    conn.execute(
        "DELETE FROM knowledge_base_files
         WHERE knowledge_base_id = ?1 AND memory_id IN (
             SELECT memory_id FROM knowledge_base_memories
             WHERE knowledge_base_id = ?1 AND origin = ?2 AND source_ref = ?3
               AND memory_id NOT IN (SELECT value FROM json_each(?4))
         )",
        params![
            kb_id,
            origin,
            source_ref,
            serde_json::to_string(&preserved).unwrap_or_else(|_| "[]".into())
        ],
    )
    .map_err(|e| format!("清理旧投喂附件失败：{}", e))?;
    conn.execute(
        "DELETE FROM knowledge_base_memories
         WHERE knowledge_base_id = ?1 AND origin = ?2 AND source_ref = ?3
           AND memory_id NOT IN (SELECT value FROM json_each(?4))",
        params![
            kb_id,
            origin,
            source_ref,
            serde_json::to_string(&preserved).unwrap_or_else(|_| "[]".into())
        ],
    )
    .map_err(|e| format!("清理旧投喂条目失败：{}", e))?;

    for e in entries {
        if preserved.iter().any(|m| m == &e.memory_id) {
            continue; // 人工编辑过的条目：重跑不覆盖
        }
        conn.execute(
            "INSERT INTO knowledge_base_memories
                (knowledge_base_id, memory_id, title, content, tags, origin, source_ref,
                 metadata, added_by, added_at, updated_at, stale)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, 0)
             ON CONFLICT(knowledge_base_id, memory_id) DO UPDATE SET
                title = excluded.title,
                content = excluded.content,
                tags = excluded.tags,
                metadata = excluded.metadata,
                updated_at = excluded.updated_at,
                stale = 0",
            params![
                kb_id,
                e.memory_id,
                e.title,
                e.content,
                e.tags,
                origin,
                source_ref,
                if e.metadata.trim().is_empty() { "{}" } else { &e.metadata },
                actor,
                now
            ],
        )
        .map_err(|e| format!("写入知识条目失败：{}", e))?;
    }
    Ok(entries.len())
}

/// 库名 + 专属属性 schema（投喂整理时喂给模型）。
pub fn kb_meta(conn: &Connection, org_id: &str, kb_id: &str) -> Result<(String, String), String> {
    ensure_kb_owned(conn, org_id, kb_id)?;
    conn.query_row(
        "SELECT name, field_schema FROM knowledge_bases WHERE id = ?1",
        params![kb_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|e| format!("读取知识库定义失败：{}", e))
}

/// 加工能力缓存：主机侧算好写进 `enclave_settings`，成员端读同一份（成员没有主库，无法自行解析 Provider）。
pub fn save_ingest_capability(
    conn: &Connection,
    org_id: &str,
    json: &str,
) -> Result<(), String> {
    let key = format!("kb_ingest_capability:{}", org_id);
    conn.execute(
        "INSERT INTO enclave_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, json, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| format!("保存加工能力失败：{}", e))?;
    Ok(())
}

pub fn load_ingest_capability(conn: &Connection, org_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM enclave_settings WHERE key = ?1",
        params![format!("kb_ingest_capability:{}", org_id)],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/* ── 待确认候选（模型产出先落这里，人工采纳后才进库） ── */

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KbCandidateView {
    pub id: String,
    pub org_id: String,
    pub kb_id: String,
    pub title: String,
    pub content: String,
    pub tags: String,
    pub metadata: String,
    /// ai / ...
    pub origin: String,
    pub created_by: String,
    pub created_at: i64,
}

fn candidate_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<KbCandidateView> {
    Ok(KbCandidateView {
        id: r.get(0)?,
        org_id: r.get(1)?,
        kb_id: r.get(2)?,
        title: r.get(3)?,
        content: r.get(4)?,
        tags: r.get(5)?,
        metadata: r.get(6)?,
        origin: r.get(7)?,
        created_by: r.get(8)?,
        created_at: r.get(9)?,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn add_candidate(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    actor: &str,
    title: &str,
    content: &str,
    tags: &str,
    metadata: &str,
    origin: &str,
) -> Result<KbCandidateView, String> {
    ensure_kb_write(conn, org_id)?;
    ensure_kb_owned(conn, org_id, kb_id)?;
    let id = format!("kbc:{}", uuid::Uuid::new_v4());
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO kb_candidates
            (id, org_id, kb_id, title, content, tags, metadata, origin, created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            org_id,
            kb_id,
            title,
            content,
            tags,
            if metadata.trim().is_empty() { "{}" } else { metadata },
            origin,
            actor,
            now
        ],
    )
    .map_err(|e| format!("写入待确认候选失败：{}", e))?;
    get_candidate(conn, org_id, kb_id, &id)?.ok_or_else(|| "候选写入后不可读".to_string())
}

pub fn list_candidates(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
) -> Result<Vec<KbCandidateView>, String> {
    ensure_kb_write(conn, org_id)?;
    ensure_kb_owned(conn, org_id, kb_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, org_id, kb_id, title, content, tags, metadata, origin, created_by, created_at
             FROM kb_candidates WHERE org_id = ?1 AND kb_id = ?2
             ORDER BY created_at DESC LIMIT 100",
        )
        .map_err(|e| format!("查询待确认候选失败：{}", e))?;
    stmt.query_map(params![org_id, kb_id], candidate_row)
        .map(|rows| rows.flatten().collect())
        .map_err(|e| format!("读取待确认候选失败：{}", e))
}

fn get_candidate(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    id: &str,
) -> Result<Option<KbCandidateView>, String> {
    conn.query_row(
        "SELECT id, org_id, kb_id, title, content, tags, metadata, origin, created_by, created_at
         FROM kb_candidates WHERE id = ?1 AND org_id = ?2 AND kb_id = ?3",
        params![id, org_id, kb_id],
        candidate_row,
    )
    .optional()
    .map_err(|e| format!("读取待确认候选失败：{}", e))
}

/// 采纳候选：写入组织库并删除候选。
pub fn adopt_candidate(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    id: &str,
    actor: &str,
) -> Result<KbEntryView, String> {
    ensure_kb_write(conn, org_id)?;
    let cand = get_candidate(conn, org_id, kb_id, id)?
        .ok_or_else(|| "候选不存在或已被处理".to_string())?;
    let memory_id = format!("manual:ai:{}", id);
    let entries = vec![IngestEntryInput {
        memory_id: memory_id.clone(),
        title: cand.title,
        content: cand.content,
        tags: cand.tags,
        metadata: cand.metadata,
    }];
    write_ingest_entries(conn, org_id, kb_id, "ai", id, actor, &entries)?;
    conn.execute(
        "DELETE FROM kb_candidates WHERE id = ?1 AND org_id = ?2",
        params![id, org_id],
    )
    .map_err(|e| format!("删除候选失败：{}", e))?;
    get_entry(conn, org_id, kb_id, &memory_id)?
        .ok_or_else(|| "采纳后未找到知识条目".to_string())
}

pub fn reject_candidate(
    conn: &Connection,
    org_id: &str,
    kb_id: &str,
    id: &str,
) -> Result<bool, String> {
    ensure_kb_write(conn, org_id)?;
    conn.execute(
        "DELETE FROM kb_candidates WHERE id = ?1 AND org_id = ?2 AND kb_id = ?3",
        params![id, org_id, kb_id],
    )
    .map(|n| n > 0)
    .map_err(|e| format!("删除候选失败：{}", e))
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
