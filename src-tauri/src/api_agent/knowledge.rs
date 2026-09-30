//! 知识库领域模块（建在 MEMORY.db 上）
//!
//! 定位：知识库是 KV 记忆的一个**命名子集**（多对多），不是另一套存储。
//!   - `knowledge_bases`  库定义：名称 / 描述 / 专属字段 schema —— 用户只维护这三样，
//!                        不含服务范围与注入策略（命中与注入由统一的 KV 记忆策略负责）
//!   - `kb_entry_links`   多对多关联，并承载**该库自己的**专属属性值 `kb_meta`(JSON)、
//!                        来源(origin/source_ref)与分块序号 —— 值必须挂在关联上：
//!                        同一 key 属于两个库时，两个库的专属字段不同，值放在条目上表达不了
//!   - `key_memories`     条目本体（category='knowledge'），通用属性（tags/pin/热度）直接复用
//!   - `kb_files`/`kb_file_links`  文件知识宿主（原文放「全局工作区/Knowledge/files」，
//!                        一份原文被多个库引用，不重复拷贝）
//!   - `kb_candidates`    待确认队列（暂存区，采纳后才进 key_memories）
//!   - `kb_entry_fts`     FTS5(trigram) 索引：中文按任意子串命中
//!
//! 知识条目不占 600 条会话记忆配额、也不参与冷记忆清理（见 db.rs 的 QUOTA_EXEMPT_SQL）。

use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::db::{like_escape, memory_db_path, normalize_tags, now_secs};
use crate::utils::paths;

/// 知识条目的分类枚举值（区别于 fact/preference/skill/event）
pub const CATEGORY_KNOWLEDGE: &str = "knowledge";

/// 文件关系：正文 → 附件（`from` 是正文，`to` 是它的附件）
pub const REL_ATTACHMENT: &str = "attachment";
/// 文件关系：新版 → 旧版（`from` 替代了 `to`；仅标注，旧版仍然保留并可检索）
pub const REL_SUPERSEDES: &str = "supersedes";

/// 迁移/查询用的默认上限（列表页一次最多取这么多，避免一次把整库拉进前端）
const ENTRY_LIST_LIMIT: usize = 500;

/// 单个原文一次最多取回多少分块（图谱「展开文件」用）。
///
/// 与 `ENTRY_LIST_LIMIT` 是两回事：那个是"整库列表"的闸门，这个是"一个文件"的闸门。
/// 展开一个文件天然有界（只查一个 `source_ref`），所以这里给得比列表页宽松，
/// 但仍要有硬上限 —— 否则一个 5 万分块的文件能一次把前端压死。
const FILE_CHUNK_LIMIT: usize = 200;

/// 「列出本库有哪些原文」的上限（`find_sources` 无关键词时的清单）。
/// 它只是给模型一个目录概览，不该把上千份文件的名单整个塞进上下文。
const SRC_LIST_LIMIT: usize = 60;

/* ────────────────────────── 投喂参数 ────────────────────────── */

/// 单个文件上限：再大就不适合"抽正文 + 分块"这条路，提示用户先拆分
const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
/// 网页正文上限（字符）：防止一篇文章把整库撑满
const MAX_URL_TEXT_CHARS: usize = 200_000;
/// 分块目标长度（字符）：接近一段完整论述
const CHUNK_TARGET: usize = 1000;
/// 单块硬上限：单段本身就超长时按此剪断
const CHUNK_MAX: usize = 2000;
/// 单文件最多切多少块：兜住"一整本书"这种极端输入
const MAX_CHUNKS_PER_FILE: usize = 400;
/// 小于该长度的块会被并进上一块（避免产生"---"这类噪声块，且不丢内容）
const MIN_CHUNK: usize = 20;
/// 喂模型前，相邻块合并到的**最小**长度（见 `merge_small_blocks`）。
/// 取值远小于 `CHUNK_TARGET`：只要保证"块不是碎片"，不用凑成最终分块的大小 ——
/// 合并过头会把模型的分组自由度压掉。
pub const MIN_MODEL_BLOCK: usize = 120;

/* ────────────────────────── 建表 ────────────────────────── */

/// 幂等建表。`MemoryStore::new` 与 `KnowledgeStore::open` 都要调用：
/// QUOTA_EXEMPT_SQL 依赖 `kb_entry_links`，条目触发器依赖 `kb_entry_fts`，
/// 只在知识库页面建表的话，普通记忆写入会直接报错。
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    // 先保证主表存在：FTS 触发器挂在 key_memories 上，主表缺失会导致触发器创建失败
    super::db::ensure_key_memories_table(conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS knowledge_bases (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            field_schema TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS kb_entry_links (
            kb_id TEXT NOT NULL,
            entry_key TEXT NOT NULL,
            kb_meta TEXT NOT NULL DEFAULT '{}',
            origin TEXT NOT NULL DEFAULT '',
            source_ref TEXT NOT NULL DEFAULT '',
            chunk_index INTEGER,
            chunk_total INTEGER,
            added_at INTEGER NOT NULL,
            PRIMARY KEY (kb_id, entry_key)
        );
        CREATE INDEX IF NOT EXISTS idx_kb_links_entry ON kb_entry_links(entry_key);
        -- 分块数重算与 relink_chunks 都按 source_ref 反查，值得一个索引
        CREATE INDEX IF NOT EXISTS idx_kb_links_source ON kb_entry_links(source_ref);

        CREATE TABLE IF NOT EXISTS kb_files (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            rel_path TEXT NOT NULL DEFAULT '',
            url TEXT NOT NULL DEFAULT '',
            mime TEXT NOT NULL DEFAULT '',
            size INTEGER NOT NULL DEFAULT 0,
            sha256 TEXT NOT NULL DEFAULT '',
            extract_status TEXT NOT NULL DEFAULT 'pending',
            chunk_count INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS kb_file_links (
            kb_id TEXT NOT NULL,
            file_id TEXT NOT NULL,
            added_at INTEGER NOT NULL,
            PRIMARY KEY (kb_id, file_id)
        );
        CREATE INDEX IF NOT EXISTS idx_kb_file_links_file ON kb_file_links(file_id);

        -- 文件之间的关系（公文类场景：正文—附件；制度类场景：新版—旧版）。
        -- 方向约定：attachment: from = 正文、to = 附件；supersedes: from = 新版、to = 旧版。
        -- 只标注、不替换：新旧文件与条目都保留、都进检索 —— 查上一版制度写了什么是真实诉求。
        CREATE TABLE IF NOT EXISTS kb_file_relations (
            from_file_id TEXT NOT NULL,
            to_file_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (from_file_id, to_file_id, kind)
        );
        -- 孤儿判定要按 to 端反查（还有没有文件引用我），给个索引
        CREATE INDEX IF NOT EXISTS idx_kb_file_rel_to ON kb_file_relations(to_file_id);

        CREATE TABLE IF NOT EXISTS kb_candidates (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kb_id TEXT NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            kb_meta TEXT NOT NULL DEFAULT '{}',
            tags TEXT NOT NULL DEFAULT '',
            origin TEXT NOT NULL DEFAULT '',
            source_ref TEXT NOT NULL DEFAULT '',
            reason TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_kb_candidates_kb ON kb_candidates(kb_id);

        -- trigram：中文按任意子串命中。unicode61 会把整串汉字当成一个 token，中文等于搜不到。
        CREATE VIRTUAL TABLE IF NOT EXISTS kb_entry_fts USING fts5(key, value, tags, tokenize='trigram');

        -- 知识库内部元信息（目前只存触发器版本）。触发器不在这个批次里建，
        -- 而是由 ensure_triggers 按版本 DROP + CREATE —— 见那里的说明。
        CREATE TABLE IF NOT EXISTS kb_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("创建知识库表失败: {}", e))?;

    // 存量库补列（老的 kb_candidates 没有 tags）
    ensure_columns(
        conn,
        "kb_candidates",
        &[("tags", "TEXT NOT NULL DEFAULT ''")],
    )?;
    // 文件整理（AI 预处理）状态：0=未整理 1=已整理；失败原因留给用户看
    ensure_columns(
        conn,
        "kb_files",
        &[
            ("enriched", "INTEGER NOT NULL DEFAULT 0"),
            ("enrich_error", "TEXT NOT NULL DEFAULT ''"),
            // 「覆盖要可见」的第一件：AI 主导维护之下，用户至少要能知道属性**什么时候**被刷过。
            // 只记成功时间：失败时保留上一次成功的时间，用户看到的是"上次整理于 X（本次失败）"
            ("enriched_at", "INTEGER NOT NULL DEFAULT 0"),
        ],
    )?;

    // 存量知识条目补索引（只在索引为空时做一次，避免重复插入把索引撑大）
    let indexed: i64 = conn
        .query_row("SELECT COUNT(*) FROM kb_entry_fts", [], |r| r.get(0))
        .unwrap_or(0);
    if indexed == 0 {
        let _ = conn.execute(
            "INSERT INTO kb_entry_fts(key, value, tags)
             SELECT key, value, tags FROM key_memories WHERE category = 'knowledge'",
            [],
        );
    }

    // 存量兜底：在 kb_entry_cleanup_ad 触发器上线之前，从「KV 记忆」删条目留下的悬空关联
    // 会一直躺在表里（连带让 delete_base 的孤儿判定失真）。删条目本体却没有关联的悬空情形
    // 只有这一种，删掉即可；随后按关联行重算一遍分块数（幂等，正常情况下是 no-op）。
    let orphans = conn
        .execute(
            "DELETE FROM kb_entry_links WHERE entry_key NOT IN (SELECT key FROM key_memories)",
            [],
        )
        .unwrap_or(0);
    if orphans > 0 {
        log::info!(
            "[Knowledge] 已清理 {} 行悬空关联（条目本体已不存在）",
            orphans
        );
    }
    let _ = conn.execute(
        "UPDATE kb_files
            SET chunk_count = (
                SELECT COUNT(DISTINCT l.chunk_index) FROM kb_entry_links l
                 WHERE l.source_ref = kb_files.rel_path AND l.chunk_index IS NOT NULL
            )
          WHERE rel_path <> ''",
        [],
    );

    // 触发器按版本重建（必须在补索引之后：FTS 索引要先填好，触发器只负责后续同步）
    ensure_triggers(conn)?;
    // 自愈：把失去归属的知识条目收进一个专门的库，让它们重新可见（详见函数说明）
    heal_orphan_entries(conn)?;
    Ok(())
}

/// 触发器定义版本。**改动任何一个触发器体都必须把它 +1** ——
/// 否则存量库不会被重建（见 ensure_triggers）。
const TRIGGER_VERSION: &str = "2";

/// 知识库的全部触发器。定义集中在这里，由 ensure_triggers 按版本重建。
///
/// 为什么不用 `CREATE TRIGGER IF NOT EXISTS`：那样对**存量库永远不生效**。这个坑已经真实发生过 ——
/// 早期版本用 FTS5 的 `'delete'` 特殊命令清索引（只对 contentless / external-content 表有效），
/// 对普通 FTS5 表执行会报 `SQL logic error`；后来代码里改成了普通 DELETE，但用户库里留的还是旧定义，
/// 于是**删知识条目 100% 失败**，配合当时 delete_base 里 `.unwrap_or(0)` 吞错，
/// 表现为"删库只删掉了库定义与关联，条目本体全留成了僵尸"。
const TRIGGER_DEFS: &[(&str, &str)] = &[
    // FTS 同步：只索引 category='knowledge' 的行。
    // 清索引用普通 DELETE（不是 FTS5 的 'delete' 特殊命令）：后者只对 contentless /
    // external-content 表有效，普通 FTS5 表用它直接报 SQL logic error。
    (
        "kb_fts_ai",
        "CREATE TRIGGER kb_fts_ai AFTER INSERT ON key_memories
         WHEN new.category = 'knowledge'
         BEGIN
             INSERT INTO kb_entry_fts(key, value, tags) VALUES (new.key, new.value, new.tags);
         END;",
    ),
    (
        "kb_fts_au",
        "CREATE TRIGGER kb_fts_au AFTER UPDATE ON key_memories
         WHEN new.category = 'knowledge' AND old.category = 'knowledge'
         BEGIN
             DELETE FROM kb_entry_fts WHERE key = old.key;
             INSERT INTO kb_entry_fts(key, value, tags) VALUES (new.key, new.value, new.tags);
         END;",
    ),
    (
        "kb_fts_au2",
        "CREATE TRIGGER kb_fts_au2 AFTER UPDATE ON key_memories
         WHEN new.category = 'knowledge' AND old.category <> 'knowledge'
         BEGIN
             INSERT INTO kb_entry_fts(key, value, tags) VALUES (new.key, new.value, new.tags);
         END;",
    ),
    (
        "kb_fts_ad",
        "CREATE TRIGGER kb_fts_ad AFTER DELETE ON key_memories
         WHEN old.category = 'knowledge'
         BEGIN
             DELETE FROM kb_entry_fts WHERE key = old.key;
         END;",
    ),
    // 条目本体被删时（不管从哪个入口：设置页 / delete_memory 工具 / 自动维护），
    // 连带清掉它在各库的关联行，并重算受影响原文的分块数。
    // 少了这一步，「KV 记忆」那条通用删除路径会留下指向不存在条目的悬空关联，
    // 文件行显示的分块数也会一直停在投喂时的旧值。
    // 挂在触发器上是因为删除入口有四处，逐个补逻辑必然漏；知识库自己的事务删除
    // （unlink_entry 先删关联再删本体）走到这里时关联已不存在，天然幂等。
    (
        "kb_entry_cleanup_ad",
        "CREATE TRIGGER kb_entry_cleanup_ad AFTER DELETE ON key_memories
         WHEN old.category = 'knowledge'
         BEGIN
             -- 计数里排除 old.key 自己：此刻它还没被删掉（DELETE 在下面的语句里），
             -- 不排除就会把即将消失的那一块也算进去，重算等于白算。
             UPDATE kb_files
                SET chunk_count = (
                    SELECT COUNT(DISTINCT l.chunk_index) FROM kb_entry_links l
                     WHERE l.source_ref = kb_files.rel_path
                       AND l.chunk_index IS NOT NULL
                       AND l.entry_key <> old.key
                )
              WHERE rel_path <> ''
                AND rel_path IN (SELECT l.source_ref FROM kb_entry_links l
                                  WHERE l.entry_key = old.key AND l.source_ref <> '');
             DELETE FROM kb_entry_links WHERE entry_key = old.key;
         END;",
    ),
];

/// 按版本重建触发器：版本一致就跳过（每次打开库都会走到这里，所以必须是廉价的）。
fn ensure_triggers(conn: &Connection) -> Result<(), String> {
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM kb_meta WHERE key = 'trigger_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if current.as_deref() == Some(TRIGGER_VERSION) {
        return Ok(());
    }
    for (name, ddl) in TRIGGER_DEFS {
        conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {};", name))
            .map_err(|e| format!("删除旧触发器 {} 失败: {}", name, e))?;
        conn.execute_batch(ddl)
            .map_err(|e| format!("创建触发器 {} 失败: {}", name, e))?;
    }
    conn.execute(
        "INSERT INTO kb_meta(key, value) VALUES('trigger_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![TRIGGER_VERSION],
    )
    .map_err(|e| e.to_string())?;
    log::info!("[Knowledge] 触发器已重建到版本 {}", TRIGGER_VERSION);
    Ok(())
}

/// 自愈库的 id / 名称 / 说明（孤立条目的收容处）
pub const RECOVERED_BASE_ID: &str = "kb-recovered";
const RECOVERED_BASE_NAME: &str = "孤立知识（自愈恢复）";
const RECOVERED_BASE_DESC: &str =
    "这些知识条目曾经属于某个知识库，但因一次缺陷（删库时删除条目失败且错误被吞掉）失去了全部归属，\
     在任何列表里都看不到、却仍会被记忆检索命中。为确保不误删（正文可能已是唯一副本），\
     程序把它们收进这个库：你可以在这里查看、导出，确认无用后自行删除本库。";

/// 自愈：`category='knowledge'` 却没有任何库关联的条目 = 谁都看不见的孤儿。
///
/// **刻意不删**：这些条目的原文登记（`kb_files`）与磁盘文件往往也已一并没了，
/// 条目正文可能是仅存的一份；直接删等于把用户数据悄悄抹掉。
/// 收进一个专门的库之后，它们重新可见、可导出、可由用户自己决定去留。
/// 幂等：没有孤儿时是 no-op。
fn heal_orphan_entries(conn: &Connection) -> Result<(), String> {
    let orphans: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT m.key FROM key_memories m
                  WHERE m.category = ?1
                    AND NOT EXISTS (SELECT 1 FROM kb_entry_links l WHERE l.entry_key = m.key)",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![CATEGORY_KNOWLEDGE], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    if orphans.is_empty() {
        return Ok(());
    }
    let now = now_secs();
    conn.execute(
        "INSERT OR IGNORE INTO knowledge_bases(id, name, description, field_schema, created_at, updated_at)
         VALUES(?1, ?2, ?3, '[]', ?4, ?4)",
        params![RECOVERED_BASE_ID, RECOVERED_BASE_NAME, RECOVERED_BASE_DESC, now],
    )
    .map_err(|e| e.to_string())?;

    let mut healed = 0usize;
    for key in &orphans {
        // 分块 key 形如 `<文件名>-<sha8>#<序号>` → 当初来自文件投喂；其余来源已无从判断，留空
        let from_file = key
            .rsplit_once('#')
            .map(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false);
        healed += conn
            .execute(
                "INSERT OR IGNORE INTO kb_entry_links(kb_id, entry_key, kb_meta, origin, source_ref, added_at)
                 VALUES(?1, ?2, '{}', ?3, '', ?4)",
                params![RECOVERED_BASE_ID, key, if from_file { "file" } else { "" }, now],
            )
            .unwrap_or(0);
    }
    log::warn!(
        "[Knowledge] 自愈：把 {} 条失去归属的知识条目收进「{}」库（未删除任何内容）",
        healed,
        RECOVERED_BASE_NAME
    );
    Ok(())
}

/* ────────────────────────── 视图类型（直接给前端） ────────────────────────── */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBaseView {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 专属字段定义（JSON 数组字符串，前端 parse 后渲染表单）
    pub fields_json: String,
    pub entry_count: usize,
    pub pinned_count: usize,
    pub file_count: usize,
    pub pending_count: usize,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEntryView {
    pub key: String,
    pub value: String,
    pub category: String,
    pub tags: String,
    pub pin: bool,
    pub access_count: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// 本库的专属属性值（JSON 对象字符串）
    pub meta_json: String,
    pub origin: String,
    pub source_ref: String,
    pub chunk_index: Option<u32>,
    pub chunk_total: Option<u32>,
    /// 该条目关联的全部知识库（前端据此显示「N 库」）
    pub base_ids: Vec<String>,
}

/// 某个标题在库里所处的位置 —— 决定 `save_knowledge` 能不能直接覆盖。
///
/// 为什么要区分：文件分块的正文是**原文区间的切片**，覆盖它等于让条目脱离原文
/// （破坏"分块 = 原文切片"这条不变式）；而 AI / 片段 / 工作沉淀产出的条目自成一类，
/// 覆盖是正常的迭代。
#[derive(Debug, Clone, PartialEq)]
pub enum EntrySlot {
    /// 库里没有这个标题 → 新增
    New,
    /// 已有这个标题、且被 `bases` 个库引用 → 可覆盖，但要如实告知影响面
    Existing { bases: usize },
    /// 已有这个标题，但它是某份原文的分块 → **不许覆盖**
    ChunkOfFile {
        source_ref: String,
        chunk_index: Option<u32>,
    },
}

/// 按原文查找的命中（`read_kb_file` 的定位阶段）
#[derive(Debug, Clone, PartialEq)]
pub struct SourceHit {
    /// 相对路径：`read_file_chunks` 的唯一入参
    pub source_ref: String,
    /// 给人/给模型看的文件名（已剥掉 `<sha8>-` 前缀）
    pub name: String,
    /// 分块数
    pub chunk_count: usize,
}

/// 文件的**专属属性**——由该文件所有分块的 `kb_meta` 聚合而来。
///
/// 为什么是聚合而不是字段：`kb_files` 上**没有属性列**，专属属性只挂在条目关联行
/// （`kb_entry_links.kb_meta`）上；所以"这个文件属于哪个部门"这种问题的答案，
/// 只能来自它下面那些分块。
///
/// 给的是**全部取值**而不是"一个值 + 冲突标记"，因为文件树要表达三种情况：
///   0 个值 = 分块都没填 → **没有子目录**（这个文件留在根目录）；
///   1 个值 = 进那一个目录；
///   N 个值 = **同时出现在 N 个目录**（虚拟重复：是同一份文件，不是 N 份 —— 勾选/导出/移除都按文件 id 走）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeFileAttrView {
    /// 原文相对路径（`kb_files.rel_path` 的口径，与条目 `source_ref` 一致）
    pub source_ref: String,
    /// 字段 → 该文件全部**去重后的取值**（排序，便于稳定渲染目录顺序）
    pub values: HashMap<String, Vec<String>>,
    /// 参与聚合的分块数（0 = 没有分块，必然没有属性）
    pub chunk_count: u32,
}

/// 单个字段最多记几个取值。
///
/// 能用来分目录的只有 select / boolean（取值天然有界），这个上限是防脏数据
/// （手改过的 `kb_meta`、被误配成 text 的字段）把文件树炸成成百上千个目录。
const ATTR_VALUES_LIMIT: usize = 8;

/// 8 位十六进制 = 内容 sha 的前缀（`files/<sha8>-名字` / `<标题>-<sha8>#1`）。
/// 这类前缀是去重指纹，**任何给人看的地方都不该出现**。
pub(crate) fn is_sha8(s: &str) -> bool {
    s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// 原文相对路径 → 文件名：`files/<sha8>-名字.md` → `名字.md`。
///
/// 只剥**这一种**前缀，不能见到 `-` 就切（正常文件名里也可能有连字符）。
pub fn display_file_name(rel_path: &str) -> String {
    let base = rel_path.rsplit(['/', '\\']).next().unwrap_or(rel_path);
    match base.split_once('-') {
        Some((sha, rest)) if is_sha8(sha) && !rest.is_empty() => rest.to_string(),
        _ => base.to_string(),
    }
}

/// 条目 key → 给人看的标题：`<标题>-<sha8>#<序号>` → `<标题>`。
///
/// 只剥**这一种**后缀，不能见到 `#数字` 就削（正常标题也可能以 `#1` 结尾）。
/// 与前端 `entryTitle()` 同一口径；导出、工具返回、注入块都必须过它。
pub fn display_entry_title(key: &str) -> String {
    let Some((head, tail)) = key.rsplit_once('#') else {
        return key.to_string();
    };
    if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
        return key.to_string();
    }
    match head.rsplit_once('-') {
        Some((title, sha)) if is_sha8(sha) && !title.is_empty() => title.to_string(),
        _ => key.to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeFileView {
    pub id: String,
    pub name: String,
    /// 相对知识库文件根的路径（原文共享一份，不按库拷贝）
    pub rel_path: String,
    /// 绝对路径（命令层用知识库根拼好，供「打开原文」用）
    pub abs_path: String,
    pub url: String,
    pub mime: String,
    pub size: u64,
    pub extract_status: String,
    pub chunk_count: u32,
    /// 该原文的分块是否已过 AI 整理（打标 + 填专属属性）
    pub enriched: bool,
    pub enrich_error: String,
    /// 上次**成功**整理的时间（0 = 从没整理过）。让用户知道属性是什么时候被刷的
    pub enriched_at: u64,
    pub base_ids: Vec<String>,
    pub created_at: u64,
}

/// 文件之间的关系（前端据此把附件挂到正文下、标注新旧版本）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeFileRelationView {
    pub from_file_id: String,
    pub to_file_id: String,
    /// attachment | supersedes
    pub kind: String,
}

/// 投喂结果（文件 / 网页共用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestOutcome {
    pub file_id: String,
    pub file_name: String,
    pub rel_path: String,
    /// 该文件在库里的分块数（复用已有原文时即已有分块数）
    pub chunks: usize,
    /// 原文已在库里（同 sha256）：只补关联，没有重复落盘
    pub reused: bool,
    /// done | unsupported | failed
    pub status: String,
    /// 分块数超过上限被截断
    pub truncated: bool,
    /// AI 相关的提醒（一切正常时为空串）：AI 分组未生效的原因、AI 没能取名等。
    /// 投喂是"AI 参与分块"的主路径，降级**必须是可见的**：否则用户以为内容是 AI 分的、
    /// 名字是 AI 取的，实际是规则切开 + 原名，看着就像功能没生效。
    pub ai_note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeCandidateView {
    pub id: i64,
    pub base_id: String,
    pub key: String,
    pub value: String,
    pub meta_json: String,
    /// 逗号分隔（与记忆表 tags 同口径）
    pub tags: String,
    pub origin: String,
    pub source_ref: String,
    pub reason: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteBaseOutcome {
    /// 不再被任何库关联、被一并删除的条目数
    pub entries: usize,
    /// 不再被任何库引用的文件数
    pub files: usize,
}

/// 删库前导出快照：库定义 + 本库条目（正文/标签/专属属性/来源/分块序号）+ 本库文件登记。
///
/// 理由：`knowledge_bases` 一行删掉之后，这个库的存在本身就无法从库里推回来。
/// 而"我明明只删了文件，库怎么没了"这类误伤必须事后能查、内容能捞回 ——
/// 所以删之前把它导出成 JSON（含条目正文，可能是唯一副本）。
///
/// 返回 `(库名, 快照路径)`；任何一步失败都返回 None：**快照是保险，不该阻塞删除本身**。
fn write_delete_snapshot(
    tx: &rusqlite::Transaction,
    db_dir: &Path,
    id: &str,
) -> Option<(String, String)> {
    let (name, description, field_schema, created_at, updated_at) = tx
        .query_row(
            "SELECT name, description, field_schema, created_at, updated_at
               FROM knowledge_bases WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .ok()?;

    let entries: Vec<serde_json::Value> = {
        let mut stmt = tx
            .prepare(
                "SELECT m.key, m.value, m.tags, m.pin, l.origin, l.source_ref,
                        l.chunk_index, l.chunk_total, l.kb_meta
                   FROM kb_entry_links l JOIN key_memories m ON m.key = l.entry_key
                  WHERE l.kb_id = ?1",
            )
            .ok()?;
        let rows = stmt
            .query_map(params![id], |r| {
                Ok(serde_json::json!({
                    "key": r.get::<_, String>(0)?,
                    "value": r.get::<_, String>(1)?,
                    "tags": r.get::<_, String>(2)?,
                    "pin": r.get::<_, i64>(3)?,
                    "origin": r.get::<_, String>(4)?,
                    "sourceRef": r.get::<_, String>(5)?,
                    "chunkIndex": r.get::<_, Option<i64>>(6)?,
                    "chunkTotal": r.get::<_, Option<i64>>(7)?,
                    "meta": r.get::<_, String>(8)?,
                }))
            })
            .ok()?;
        rows.filter_map(|r| r.ok()).collect()
    };
    let files: Vec<serde_json::Value> = {
        let mut stmt = tx
            .prepare(
                "SELECT f.id, f.name, f.rel_path, f.url, f.sha256, f.mime, f.size, f.extract_status
                   FROM kb_file_links fl JOIN kb_files f ON f.id = fl.file_id
                  WHERE fl.kb_id = ?1",
            )
            .ok()?;
        let rows = stmt
            .query_map(params![id], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?,
                    "name": r.get::<_, String>(1)?,
                    "relPath": r.get::<_, String>(2)?,
                    "url": r.get::<_, String>(3)?,
                    "sha256": r.get::<_, String>(4)?,
                    "mime": r.get::<_, String>(5)?,
                    "size": r.get::<_, i64>(6)?,
                    "extractStatus": r.get::<_, String>(7)?,
                }))
            })
            .ok()?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let dir = db_dir.join("deleted-knowledge");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}-{}.json", now_secs(), id));
    let doc = serde_json::json!({
        "base": {
            "id": id,
            "name": name,
            "description": description,
            "fieldSchema": field_schema,
            "createdAt": created_at,
            "updatedAt": updated_at,
        },
        "deletedAt": now_secs(),
        "entries": entries,
        "files": files,
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&doc).ok()?).ok()?;
    Some((name, path.to_string_lossy().into_owned()))
}

/// 从库中移除文件的结果
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveFilesOutcome {
    /// 从本库移除、并因不再被任何库关联而删除本体的条目数
    pub entries: usize,
    /// 不再被任何库引用、连登记行与磁盘原文一起删除的文件数
    pub files: usize,
}

/// 批量解除关联的结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlinkEntriesOutcome {
    /// 真正解除了关联的条数（不在本库的 key 不计入）
    pub unlinked: usize,
    /// 其中不再被任何库关联、被一并删除本体的条数
    pub deleted: usize,
}

/* ────────────────────────── store ────────────────────────── */

/// 把条目查询共用的那 14 列读成视图。
///
/// `list_entries` 与 `list_file_chunks` 的 SELECT 列顺序必须完全一致，抽到一处：
/// 以后加列时只改这里，不会出现"改了一个查询、另一个悄悄读错列"。
fn entry_view_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeEntryView> {
    let base_ids: String = row.get::<_, Option<String>>(13)?.unwrap_or_default();
    Ok(KnowledgeEntryView {
        key: row.get(0)?,
        value: row.get(1)?,
        category: row.get(2)?,
        tags: row.get(3)?,
        pin: row.get::<_, i64>(4)? != 0,
        access_count: row.get::<_, i64>(5)? as u64,
        created_at: row.get::<_, i64>(6)? as u64,
        updated_at: row.get::<_, i64>(7)? as u64,
        meta_json: row.get(8)?,
        origin: row.get(9)?,
        source_ref: row.get(10)?,
        chunk_index: row.get::<_, Option<i64>>(11)?.map(|v| v as u32),
        chunk_total: row.get::<_, Option<i64>>(12)?.map(|v| v as u32),
        base_ids: base_ids
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
    })
}

/// 条目查询的 SELECT 列表（顺序与 `entry_view_from_row` 一一对应）
const ENTRY_VIEW_COLUMNS: &str =
    "SELECT m.key, m.value, m.category, m.tags, m.pin, m.access_count, m.created_at, m.updated_at,
            l.kb_meta, l.origin, l.source_ref, l.chunk_index, l.chunk_total,
            (SELECT group_concat(x.kb_id, ',') FROM kb_entry_links x WHERE x.entry_key = m.key)
     FROM kb_entry_links l JOIN key_memories m ON m.key = l.entry_key";

pub struct KnowledgeStore {
    conn: Arc<Mutex<Connection>>,
    /// 配置目录：删库前把快照写到 `<db_dir>/deleted-knowledge/`（见 write_delete_snapshot）
    db_dir: String,
}

/// 重命名一条已登记的文件，返回新的 `rel_path`。
///
/// "重命名"实际要动三处，**漏掉任何一处都会留下不一致**：
///   1. 磁盘上的文件名（`rel_path` 里带着名字）；
///   2. `kb_files.name` / `rel_path`（界面显示与"打开原文"都用它们）；
///   3. **分块条目的 `source_ref`** —— 分块靠"`source_ref` = 文件的 `rel_path`"这个字符串约定
///      关联（没有外键），改了路径却不同步，那些条目就成了指向不存在文件的孤儿：
///      图谱上消失、清理逻辑也认不出来。
///
/// 磁盘改名失败（文件被占用等）时**只更新显示名、保留旧 `rel_path`**：
/// 用户要的是"名字对"，而路径是内部细节 —— 总好过把 `rel_path` 改成一个不存在的路径，
/// 那会让"打开原文"直接坏掉。
fn rename_stored_file(
    tx: &rusqlite::Transaction<'_>,
    root: &std::path::Path,
    file_id: &str,
    old_rel: &str,
    sha8: &str,
    new_name: &str,
) -> Result<String, String> {
    let new_rel = format!(
        "files/{}-{}",
        sha8,
        paths::sanitize_dir_name(new_name, "file")
    );
    let moved = std::fs::rename(root.join(old_rel), root.join(&new_rel)).is_ok();
    if !moved {
        log::warn!(
            "[KB] 重命名磁盘文件失败，仅更新显示名：{} → {}",
            old_rel,
            new_rel
        );
    }
    if moved {
        tx.execute(
            "UPDATE kb_entry_links SET source_ref = ?1 WHERE source_ref = ?2",
            params![new_rel, old_rel],
        )
        .map_err(|e| format!("同步分块来源失败: {}", e))?;
        tx.execute(
            "UPDATE kb_files SET name = ?1, rel_path = ?2 WHERE id = ?3",
            params![new_name, new_rel, file_id],
        )
        .map_err(|e| format!("更新文件名失败: {}", e))?;
        Ok(new_rel)
    } else {
        tx.execute(
            "UPDATE kb_files SET name = ?1 WHERE id = ?2",
            params![new_name, file_id],
        )
        .map_err(|e| format!("更新文件名失败: {}", e))?;
        Ok(old_rel.to_string())
    }
}

impl KnowledgeStore {
    /// 打开记忆库并确保知识库表齐备（与 MemoryStore 同一个文件、各自的连接，
    /// 库已启用 WAL，两连接并存不会互相阻塞读取）。
    pub fn open(db_dir: &str) -> Result<Self, String> {
        let path = memory_db_path(db_dir);
        let conn =
            Connection::open(&path).map_err(|e| format!("无法打开记忆库 {}: {}", path, e))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;").ok();
        conn.execute_batch("PRAGMA busy_timeout=5000;").ok();
        ensure_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_dir: db_dir.to_string(),
        })
    }

    /* ── 库定义 ── */

    pub fn list_bases(&self) -> Vec<KnowledgeBaseView> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT b.id, b.name, b.description, b.field_schema, b.created_at, b.updated_at,
                    (SELECT COUNT(*) FROM kb_entry_links l WHERE l.kb_id = b.id),
                    (SELECT COUNT(*) FROM kb_entry_links l JOIN key_memories m ON m.key = l.entry_key
                      WHERE l.kb_id = b.id AND m.pin = 1),
                    (SELECT COUNT(*) FROM kb_file_links f WHERE f.kb_id = b.id),
                    (SELECT COUNT(*) FROM kb_candidates c WHERE c.kb_id = b.id)
             FROM knowledge_bases b ORDER BY b.created_at ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| {
            Ok(KnowledgeBaseView {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                fields_json: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                entry_count: row.get::<_, i64>(6)? as usize,
                pinned_count: row.get::<_, i64>(7)? as usize,
                file_count: row.get::<_, i64>(8)? as usize,
                pending_count: row.get::<_, i64>(9)? as usize,
            })
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    pub fn create_base(
        &self,
        id: &str,
        name: &str,
        description: &str,
        field_schema: &str,
    ) -> Result<KnowledgeBaseView, String> {
        let id = id.trim();
        let name = name.trim();
        if id.is_empty() || name.is_empty() {
            return Err("知识库 id 与名称不能为空".into());
        }
        validate_field_schema(field_schema)?;
        let now = now_secs();
        let conn = self.conn.lock().unwrap();
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM knowledge_bases WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_some() {
            return Err(format!("知识库 {} 已存在", id));
        }
        conn.execute(
            "INSERT INTO knowledge_bases(id, name, description, field_schema, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, name, description, field_schema, now],
        )
        .map_err(|e| format!("创建知识库失败: {}", e))?;
        drop(conn);
        Ok(KnowledgeBaseView {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            fields_json: field_schema.to_string(),
            entry_count: 0,
            pinned_count: 0,
            file_count: 0,
            pending_count: 0,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn update_base(
        &self,
        id: &str,
        name: &str,
        description: &str,
        field_schema: &str,
    ) -> Result<(), String> {
        validate_field_schema(field_schema)?;
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE knowledge_bases SET name = ?2, description = ?3, field_schema = ?4, updated_at = ?5
                 WHERE id = ?1",
                params![id, name.trim(), description, field_schema, now_secs()],
            )
            .map_err(|e| format!("更新知识库失败: {}", e))?;
        if n == 0 {
            return Err(format!("知识库 {} 不存在", id));
        }
        Ok(())
    }

    /// 删库：库定义、关联、候选一并清掉；随后删除孤儿条目与孤儿文件
    /// （仍被其它库引用的会保留 —— 这是多对多的必然结果）。
    ///
    /// `root` 用于连带删掉孤儿文件的磁盘原文（原文一定是投喂时复制进 `root/files/` 的，
    /// 删它不会碰到用户的原位文件）。
    pub fn delete_base(&self, id: &str, root: &Path) -> Result<DeleteBaseOutcome, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        let linked_keys: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT entry_key FROM kb_entry_links WHERE kb_id = ?1")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        let linked_files: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT file_id FROM kb_file_links WHERE kb_id = ?1")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };

        // 删之前先留快照 + 写审计。
        //
        // 为什么：库定义一旦删掉就无法从库里推回来，而"我只删了文件、库怎么没了"这类误伤
        // 必须事后**查得清、内容能捞回**。快照只写文件、不进数据库，失败也不阻塞删除。
        let snap = write_delete_snapshot(&tx, Path::new(&self.db_dir), id);
        let base_name = snap.as_ref().map(|(n, _)| n.clone()).unwrap_or_default();

        tx.execute("DELETE FROM kb_entry_links WHERE kb_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM kb_file_links WHERE kb_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM kb_candidates WHERE kb_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM knowledge_bases WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;

        // 孤儿条目：删掉行本身（触发器会同步清 FTS 索引）
        let mut entries_deleted = 0usize;
        for key in &linked_keys {
            // 这两步都**不能吞错**：删条目失败却继续提交，就会留下"库定义已删、关联已清、
            // 条目本体还在"的僵尸数据（历史上真发生过，见 TRIGGER_DEFS 的说明）。
            // 失败就整体回滚，让用户看到错误而不是一个半完成状态。
            let still: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM kb_entry_links WHERE entry_key = ?1",
                    params![key],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if still == 0 {
                entries_deleted += tx
                    .execute("DELETE FROM key_memories WHERE key = ?1", params![key])
                    .map_err(|e| format!("删除孤儿条目「{}」失败: {}", key, e))?;
            }
        }

        // 孤儿文件：两个维度都为空才删（见 purge_orphan_files），并沿「正文 → 附件」级联
        let files_deleted = purge_orphan_files(&tx, root, linked_files)?;

        // 审计：**每一次库定义被删除都留痕**（时间、库名、连带数量、快照路径）。
        // 这类"我没点删除库啊"的争议只能靠运行时证据收敛，所以写进库里而不是只打日志。
        let audit = serde_json::json!({
            "id": id,
            "name": base_name,
            "at": now_secs(),
            "entries": entries_deleted,
            "files": files_deleted,
            "snapshot": snap.as_ref().map(|(_, p)| p.clone()).unwrap_or_default(),
        });
        tx.execute(
            "INSERT INTO kb_meta(key, value) VALUES('last_base_delete', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![audit.to_string()],
        )
        .map_err(|e| e.to_string())?;
        log::warn!("[Knowledge] 删除知识库定义: {}", audit);

        tx.commit().map_err(|e| e.to_string())?;
        Ok(DeleteBaseOutcome {
            entries: entries_deleted,
            files: files_deleted,
        })
    }

    /* ── 条目 ── */

    /// 列表/检索：通用属性（关键词、来源、重要）× 专属属性（meta_filter JSON 对象，逐字段等值）
    pub fn list_entries(
        &self,
        kb_id: &str,
        query: Option<&str>,
        origin: Option<&str>,
        pin_only: bool,
        meta_filter: Option<&str>,
    ) -> Vec<KnowledgeEntryView> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(ENTRY_VIEW_COLUMNS);
        sql.push_str(" WHERE l.kb_id = ?");
        let mut args: Vec<SqlValue> = vec![SqlValue::Text(kb_id.to_string())];

        if pin_only {
            sql.push_str(" AND m.pin = 1");
        }
        if let Some(o) = origin.filter(|s| !s.is_empty()) {
            sql.push_str(" AND l.origin = ?");
            args.push(SqlValue::Text(o.to_string()));
        }

        // 正文检索：≥3 字走 FTS（trigram，支持中文子串），否则退化为 LIKE
        // （trigram 索引至少要 3 个字符，短词的 MATCH 一定为空 —— 不能直接交给 FTS）
        if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
            match fts_query(q) {
                Some(expr) => {
                    sql.push_str(
                        " AND m.key IN (SELECT key FROM kb_entry_fts WHERE kb_entry_fts MATCH ?)",
                    );
                    args.push(SqlValue::Text(expr));
                }
                None => {
                    let pat = format!("%{}%", like_escape(q));
                    sql.push_str(" AND (m.key LIKE ? ESCAPE '\\' OR m.value LIKE ? ESCAPE '\\' OR m.tags LIKE ? ESCAPE '\\')");
                    for _ in 0..3 {
                        args.push(SqlValue::Text(pat.clone()));
                    }
                }
            }
        }

        if let Some(filter) = meta_filter.filter(|s| !s.trim().is_empty()) {
            if let Ok(serde_json::Value::Object(map)) =
                serde_json::from_str::<serde_json::Value>(filter)
            {
                for (field, value) in map {
                    let Some(path) = json_path_for(&field) else {
                        continue;
                    };
                    let v = match value {
                        serde_json::Value::String(s) => SqlValue::Text(s),
                        serde_json::Value::Number(n) => n
                            .as_i64()
                            .map(SqlValue::Integer)
                            .or_else(|| n.as_f64().map(SqlValue::Real))
                            .unwrap_or(SqlValue::Null),
                        serde_json::Value::Bool(b) => SqlValue::Integer(b as i64),
                        _ => continue,
                    };
                    sql.push_str(" AND json_extract(l.kb_meta, ?) = ?");
                    args.push(SqlValue::Text(path));
                    args.push(v);
                }
            }
        }

        sql.push_str(&format!(
            " ORDER BY m.pin DESC, m.updated_at DESC LIMIT {}",
            ENTRY_LIST_LIMIT
        ));

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params_from_iter(args), entry_view_from_row)
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 按名字片段找原文（可能多个）。`needle` 为空 = 列出本库全部有内容的原文。
    ///
    /// 从 `kb_entry_links` 聚合而不是查 `kb_files`：
    ///   ① 读文件读到的是**分块内容**，抽不出正文的格式（docx / 图片）本来就没得读，
    ///      列出来只会让模型白试一次；
    ///   ② 这条聚合不需要知识库根目录（`list_files` 要 root 才能拼 abs_path）。
    pub fn find_sources(&self, kb_id: &str, needle: &str, limit: usize) -> Vec<SourceHit> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT l.source_ref, COUNT(*) FROM kb_entry_links l
             WHERE l.kb_id = ?1 AND l.source_ref <> ''",
        );
        let mut args: Vec<SqlValue> = vec![SqlValue::Text(kb_id.to_string())];
        let needle = needle.trim();
        if !needle.is_empty() {
            sql.push_str(" AND l.source_ref LIKE ?2 ESCAPE '\\'");
            args.push(SqlValue::Text(format!("%{}%", like_escape(needle))));
        }
        sql.push_str(&format!(
            " GROUP BY l.source_ref ORDER BY COUNT(*) DESC, l.source_ref LIMIT {}",
            limit.clamp(1, SRC_LIST_LIMIT)
        ));
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        });
        match rows {
            Ok(it) => it
                .filter_map(|r| r.ok())
                .map(|(source_ref, n)| SourceHit {
                    name: display_file_name(&source_ref),
                    source_ref,
                    chunk_count: n.max(0) as usize,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// 整库「文件 → 专属属性取值」聚合：给文件视图按属性分目录用。
    ///
    /// **文件没有属性列**，属性只在分块的 `kb_entry_links.kb_meta` 上，所以这里按 `source_ref`
    /// 把该库所有分块的 meta 收上来，逐字段收集**去重后的全部取值**（空串/缺失视为"没填"，不收集）：
    /// 0 个 = 该文件在这个字段上没有子目录（留在根）；N 个 = 同时挂到 N 个目录（虚拟重复）。
    ///
    /// 一次查全库而不是逐文件查：调用方是"渲染一棵树"，逐文件查会变成 N 次往返；
    /// 而这个查询只取 `source_ref + kb_meta` 两列，比 `list_entries` 轻得多（也不受 500 条闸门限制）。
    pub fn file_attr_map(&self, kb_id: &str) -> Vec<KnowledgeFileAttrView> {
        // 每个 source_ref：分块数 + 字段 → 取值集合（用 HashSet 去重，最后排序输出）
        type FieldValues = std::collections::HashSet<String>;
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT l.source_ref, l.kb_meta FROM kb_entry_links l
              WHERE l.kb_id = ?1 AND l.source_ref <> ''",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut acc: HashMap<String, (u32, HashMap<String, FieldValues>)> = HashMap::new();
        if let Ok(rows) = stmt.query_map(params![kb_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            for (source_ref, meta_json) in rows.filter_map(|r| r.ok()) {
                let entry = acc.entry(source_ref).or_insert_with(|| (0, HashMap::new()));
                entry.0 += 1;
                let Ok(serde_json::Value::Object(map)) =
                    serde_json::from_str::<serde_json::Value>(&meta_json)
                else {
                    continue; // 坏 JSON 不参与聚合（属性是增益，不该让整棵树失败）
                };
                for (field, value) in map {
                    // 渲染口径与注入块 / 检索回显共用 `meta_value_text`（布尔 → 是/否）
                    let Some(text) = super::db::meta_value_text(&value) else {
                        continue;
                    };
                    let text = text.trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    let slot = entry.1.entry(field).or_default();
                    if slot.len() < ATTR_VALUES_LIMIT {
                        slot.insert(text);
                    }
                }
            }
        }
        let mut out: Vec<KnowledgeFileAttrView> = acc
            .into_iter()
            .map(|(source_ref, (chunk_count, fields))| {
                let values = fields
                    .into_iter()
                    .map(|(field, set)| {
                        let mut v: Vec<String> = set.into_iter().collect();
                        v.sort();
                        (field, v)
                    })
                    .collect();
                KnowledgeFileAttrView {
                    source_ref,
                    values,
                    chunk_count,
                }
            })
            .collect();
        out.sort_by(|a, b| a.source_ref.cmp(&b.source_ref));
        out
    }

    /// 某个原文的分块（图谱「展开文件」用）：按分块序号返回。
    ///
    /// 为什么不复用 `list_entries`：它有 `ENTRY_LIST_LIMIT`（整库 500 条）的闸门，
    /// 而且是按 `pin/updated_at` 排序取前 500 —— 一个 1000 块的文件，前端根本拿不全它的分块。
    /// 这里只查一个 `source_ref`，天然有界，所以「展开某个文件」能做到完整。
    ///
    /// `limit` 传 `None` = **不设上限**：图谱要不要展开某个文件，由**前端按整库规模**决定，
    /// 而不是服务端悄悄截断 —— 截断会让人以为"这个文件就这么多块"。
    /// 传 `Some(n)` 仍会被夹到 `[1, FILE_CHUNK_LIMIT]`（只给测试与特殊调用方用）。
    pub fn list_file_chunks(
        &self,
        kb_id: &str,
        source_ref: &str,
        limit: Option<usize>,
    ) -> Vec<KnowledgeEntryView> {
        if source_ref.is_empty() {
            return Vec::new();
        }
        let tail = match limit {
            Some(n) => format!(" LIMIT {}", n.clamp(1, FILE_CHUNK_LIMIT)),
            None => String::new(),
        };
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "{} WHERE l.kb_id = ?1 AND l.source_ref = ?2 ORDER BY COALESCE(l.chunk_index, 0), m.key{tail}",
            ENTRY_VIEW_COLUMNS
        );
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![kb_id, source_ref], entry_view_from_row)
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 按文件读分块（`read_kb_file` 工具用）：从第 `from` 块起取 `limit` 块，并回传总块数。
    ///
    /// 与 `list_file_chunks` 的区别是**要能翻页**（模型先读前 12 块、再用 from=13 续读），
    /// 以及需要总块数才能告诉模型"还有多少"。
    pub fn read_file_chunks(
        &self,
        kb_id: &str,
        source_ref: &str,
        from: usize,
        limit: usize,
    ) -> (Vec<KnowledgeEntryView>, usize) {
        if source_ref.is_empty() {
            return (Vec::new(), 0);
        }
        let limit = limit.clamp(1, FILE_CHUNK_LIMIT);
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM kb_entry_links WHERE kb_id = ?1 AND source_ref = ?2",
                params![kb_id, source_ref],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if total <= 0 {
            return (Vec::new(), 0);
        }
        let sql = format!(
            "{} WHERE l.kb_id = ?1 AND l.source_ref = ?2 ORDER BY COALESCE(l.chunk_index, 0), m.key LIMIT {limit} OFFSET {}",
            ENTRY_VIEW_COLUMNS,
            from.saturating_sub(1)
        );
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return (Vec::new(), total.max(0) as usize),
        };
        let rows: Vec<KnowledgeEntryView> = stmt
            .query_map(params![kb_id, source_ref], entry_view_from_row)
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        (rows, total.max(0) as usize)
    }

    /// 这个标题在库里处于什么状态 —— 决定"能不能覆盖写"。
    ///
    /// 判据是**全局**的（不按库）：条目本体只有一行（`key_memories`），
    /// 多库之间是共享引用，所以覆盖会同时影响所有引用它的库。
    pub fn entry_slot(&self, entry_key: &str) -> EntrySlot {
        let conn = self.conn.lock().unwrap();
        let row: Option<(i64, Option<String>, Option<i64>)> = conn
            .query_row(
                "SELECT COUNT(DISTINCT kb_id),
                        MAX(CASE WHEN source_ref <> '' THEN source_ref END),
                        MAX(CASE WHEN source_ref <> '' THEN chunk_index END)
                 FROM kb_entry_links WHERE entry_key = ?1",
                params![entry_key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let Some((bases, source_ref, chunk_index)) = row else {
            return EntrySlot::New;
        };
        if let Some(source_ref) = source_ref {
            return EntrySlot::ChunkOfFile {
                source_ref,
                chunk_index: chunk_index.map(|n| n as u32),
            };
        }
        if bases > 0 {
            EntrySlot::Existing {
                bases: bases as usize,
            }
        } else {
            EntrySlot::New
        }
    }

    /// 写入/更新一条知识条目，并建立与 `kb_id` 的关联。
    ///
    /// `meta_json` 为 `None` 时保留该关联上已有的专属属性值（增量更新场景不改配置）。
    pub fn save_entry(
        &self,
        kb_id: &str,
        key: &str,
        value: &str,
        tags: &str,
        origin: &str,
        source_ref: &str,
        meta_json: Option<&str>,
    ) -> Result<(), String> {
        validate_entry_input(key, value, meta_json)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if !base_exists(&tx, kb_id)? {
            return Err(format!("知识库 {} 不存在", kb_id));
        }
        save_entry_tx(
            &tx,
            kb_id,
            key.trim(),
            value,
            &normalize_tags(tags),
            origin,
            source_ref,
            meta_json,
            None,
        )?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /* ── 投喂：原文入库 + 正文分块 ── */

    /// 投喂的落库阶段（持锁）。
    ///
    /// `plans` 来自 AI 分组（正文已按原文区间切好）：给了就按它写条目；为 None 时退回**规则分块** ——
    /// 模型不可用、超时、输出不合规都不该让投喂失败，用户的原文永远要能落到库里。
    ///
    /// `origin` 是**本次投喂的来源**（`file` / `url` / `cloud`），由调用方显式给出：
    /// 以前靠"url 是否为空"推断，新增云文档来源后这个推断会把它误记成 `url`（来源是列表与图谱的
    /// 筛选维度，记错了用户按来源筛就会丢数据）。传空串时保留旧的推断口径，兼容老调用方。
    pub fn ingest_prepared(
        &self,
        root: &std::path::Path,
        kb_id: &str,
        prep: IngestPrep,
        url: &str,
        origin: &str,
        plans: Option<Vec<PlannedChunk>>,
        plans_truncated: bool,
        ai_note: &str,
    ) -> Result<IngestOutcome, String> {
        let (chunks, truncated, note) = match plans {
            Some(p) => (p, plans_truncated, String::new()),
            None => {
                let mut c = chunk_text(&prep.text, CHUNK_TARGET, CHUNK_MAX);
                let trunc = c.len() > MAX_CHUNKS_PER_FILE;
                c.truncate(MAX_CHUNKS_PER_FILE);
                (
                    c.into_iter()
                        .map(|text| PlannedChunk {
                            text,
                            title: String::new(),
                            tags: String::new(),
                            meta_json: "{}".into(),
                        })
                        .collect(),
                    trunc,
                    ai_note.to_string(),
                )
            }
        };
        self.ingest_shared(root, kb_id, &prep, url, origin, chunks, truncated, &note)
    }

    /// 文件/网页投喂的共同后半程：登记文件（按 sha 去重）→ 关联 → 写分块或补关联。
    ///
    /// `chunks` 已是"要落库的每条知识"（正文由调用方按原文区间切好）；没有 AI 时就是规则分块。
    fn ingest_shared(
        &self,
        root: &std::path::Path,
        kb_id: &str,
        prep: &IngestPrep,
        url: &str,
        origin: &str,
        chunks: Vec<PlannedChunk>,
        truncated: bool,
        ai_note: &str,
    ) -> Result<IngestOutcome, String> {
        let IngestPrep {
            name: file_name,
            sha,
            mime,
            status,
            write_bytes,
            ..
        } = prep;
        let sha: &str = sha;
        let mime: &str = mime;
        let status: &str = status;
        let write_bytes: &[u8] = write_bytes;
        let size = write_bytes.len() as u64;
        // 目录自足：不管调用方有没有准备过，落盘前都确保根与 files/ 就绪
        paths::ensure_knowledge_dirs(root)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if !base_exists(&tx, kb_id)? {
            return Err(format!("知识库 {} 不存在", kb_id));
        }
        let now = now_secs();

        let existing: Option<(String, String, String)> = tx
            .query_row(
                "SELECT id, rel_path, name FROM kb_files WHERE sha256 = ?1 LIMIT 1",
                params![sha],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        let (file_id, rel_path, reused) = match existing.filter(|(_, rel, _)| !rel.is_empty()) {
            Some((id, rel, stored_name)) => {
                // sha 命中 = 内容已在库里。**若本次带了不同的名字，就把这次调用当作"重命名已有文件"**
                // —— 否则用户会看到"投喂成功、名字没变"，又是一次静默失败（这个模块的老毛病）。
                let rel = if !file_name.is_empty() && *file_name != stored_name {
                    rename_stored_file(&tx, root, &id, &rel, &sha[..8], file_name)?
                } else {
                    rel
                };
                (id, rel, true)
            }
            None => {
                let rel = format!(
                    "files/{}-{}",
                    &sha[..8],
                    paths::sanitize_dir_name(file_name, "file")
                );
                std::fs::write(root.join(&rel), write_bytes)
                    .map_err(|e| format!("写入知识库原文失败: {}", e))?;
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO kb_files(id, name, rel_path, url, mime, size, sha256, extract_status, chunk_count, created_at)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
                    params![id, file_name, rel, url, mime, size as i64, sha, status, now],
                )
                .map_err(|e| format!("登记文件失败: {}", e))?;
                (id, rel, false)
            }
        };

        tx.execute(
            "INSERT OR IGNORE INTO kb_file_links(kb_id, file_id, added_at) VALUES(?1, ?2, ?3)",
            params![kb_id, file_id, now],
        )
        .map_err(|e| format!("关联文件失败: {}", e))?;

        let chunk_count = if reused {
            // 原文已在库里：不重复落盘、不重复抽取，只把已有分块关联到本库
            relink_chunks(&tx, kb_id, &rel_path, now)?
        } else {
            // 调用方给了来源就用它；只给空串时才回落到"有 url 算网页、否则算文件"的老口径
            let origin = if origin.is_empty() {
                if url.is_empty() {
                    "file"
                } else {
                    "url"
                }
            } else {
                origin
            };
            let seed = key_stem(file_name);
            let n = write_planned_chunks(
                &tx,
                kb_id,
                &seed,
                file_name,
                &sha[..8],
                &rel_path,
                origin,
                &chunks,
            )?;
            tx.execute(
                "UPDATE kb_files SET chunk_count = ?2, extract_status = ?3 WHERE id = ?1",
                params![file_id, n as i64, status],
            )
            .map_err(|e| e.to_string())?;
            n
        };

        tx.commit().map_err(|e| e.to_string())?;
        Ok(IngestOutcome {
            file_id,
            file_name: file_name.to_string(),
            rel_path,
            chunks: chunk_count,
            reused,
            status: status.to_string(),
            truncated,
            ai_note: ai_note.to_string(),
        })
    }

    /// 从库中移除文件：**只解关联，绝不碰库定义**。
    ///
    /// 用户想"把这些文件从库里删掉"时不该被迫删掉整个库 —— 这个入口就是为它准备的。
    /// 语义（与删库同一套孤儿口径）：
    ///   1. 解除这些文件与本库的关联（`kb_file_links`）；
    ///   2. 这些原文在本库的分块条目：解除本库关联，不再被任何库关联的条目连本体一起删；
    ///   3. 文件本身：不再被任何库引用、也没有文件引用它 → 连登记行与磁盘原文一起删（含附件级联）。
    ///
    /// 仍被其它库引用的文件与条目一律保留。
    pub fn remove_files(
        &self,
        kb_id: &str,
        file_ids: &[String],
        root: &Path,
    ) -> Result<RemoveFilesOutcome, String> {
        if file_ids.is_empty() {
            return Ok(RemoveFilesOutcome::default());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut entries = 0usize;
        let mut touched: Vec<String> = Vec::new();

        for fid in file_ids {
            let rel: Option<String> = tx
                .query_row(
                    "SELECT rel_path FROM kb_files WHERE id = ?1",
                    params![fid],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let Some(rel) = rel else { continue };
            let unlinked = tx
                .execute(
                    "DELETE FROM kb_file_links WHERE kb_id = ?1 AND file_id = ?2",
                    params![kb_id, fid],
                )
                .map_err(|e| e.to_string())?;
            if unlinked == 0 {
                continue; // 本来就不属于本库
            }
            touched.push(fid.clone());

            // 本库这份原文的分块条目：解除本库关联；不再被任何库关联的条目连本体删除
            let keys: Vec<String> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT entry_key FROM kb_entry_links WHERE kb_id = ?1 AND source_ref = ?2",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(params![kb_id, rel], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?;
                rows.filter_map(|r| r.ok()).collect()
            };
            for key in keys {
                tx.execute(
                    "DELETE FROM kb_entry_links WHERE kb_id = ?1 AND entry_key = ?2",
                    params![kb_id, key],
                )
                .map_err(|e| e.to_string())?;
                let still: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM kb_entry_links WHERE entry_key = ?1",
                        params![key],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if still == 0 {
                    entries += tx
                        .execute(
                            "DELETE FROM key_memories WHERE key = ?1 AND category = ?2",
                            params![key, CATEGORY_KNOWLEDGE],
                        )
                        .map_err(|e| format!("删除条目「{}」失败: {}", key, e))?;
                }
            }
        }

        // 文件级收尾：两个维度都为空才删（登记行 + 磁盘原文 + 关系边，含附件级联）
        let files = purge_orphan_files(&tx, root, touched)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(RemoveFilesOutcome { entries, files })
    }

    /// 解除与某库的关联；若该条目不再属于任何库，则连同条目本体一起删除。
    /// 返回是否连带删除了条目（供前端如实提示）。与删库时的孤儿口径一致。
    pub fn unlink_entry(&self, kb_id: &str, key: &str) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM kb_entry_links WHERE kb_id = ?1 AND entry_key = ?2",
            params![kb_id, key],
        )
        .map_err(|e| e.to_string())?;
        let still: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM kb_entry_links WHERE entry_key = ?1",
                params![key],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut deleted = false;
        if still == 0 {
            deleted = tx
                .execute("DELETE FROM key_memories WHERE key = ?1", params![key])
                .map_err(|e| format!("删除条目「{}」失败: {}", key, e))?
                > 0;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(deleted)
    }

    /// 批量解除与某库的关联（列表里多选后一次处理）。与单条 `unlink_entry` 完全同一口径，
    /// 只是放进**一个事务**：逐条调用会开 N 次库、提交 N 次，而且中途失败会留下半完成状态
    /// （用户看到"删了一半"，也不知道该不该重试）。返回 (解除条数, 连带删除本体的条数)。
    pub fn unlink_entries(
        &self,
        kb_id: &str,
        keys: &[String],
    ) -> Result<UnlinkEntriesOutcome, String> {
        if keys.is_empty() {
            return Ok(UnlinkEntriesOutcome {
                unlinked: 0,
                deleted: 0,
            });
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut unlinked = 0usize;
        let mut deleted = 0usize;
        for key in keys {
            let n = tx
                .execute(
                    "DELETE FROM kb_entry_links WHERE kb_id = ?1 AND entry_key = ?2",
                    params![kb_id, key],
                )
                .map_err(|e| e.to_string())?;
            if n == 0 {
                continue; // 本来就不属于这个库（重复勾选/已在别处删掉）：不计入
            }
            unlinked += 1;
            let still: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM kb_entry_links WHERE entry_key = ?1",
                    params![key],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if still == 0 {
                deleted += tx
                    .execute("DELETE FROM key_memories WHERE key = ?1", params![key])
                    .map_err(|e| format!("删除条目「{}」失败: {}", key, e))?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(UnlinkEntriesOutcome { unlinked, deleted })
    }

    /* ── 文件知识（原文共享，多库引用） ── */

    /// 文件列表。`root` 用来把相对路径拼成绝对路径（前端「打开原文」要用），
    /// 由命令层从主库的 app_settings 解析后传入（本模块不碰主库）。
    pub fn list_files(&self, root: &Path, kb_id: &str) -> Vec<KnowledgeFileView> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT f.id, f.name, f.rel_path, f.url, f.mime, f.size, f.extract_status, f.chunk_count, f.created_at,
                    (SELECT group_concat(x.kb_id, ',') FROM kb_file_links x WHERE x.file_id = f.id),
                    f.enriched, f.enrich_error, f.enriched_at
             FROM kb_file_links l JOIN kb_files f ON f.id = l.file_id
             WHERE l.kb_id = ?1 ORDER BY f.created_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![kb_id], |row| {
            let base_ids: String = row.get::<_, Option<String>>(9)?.unwrap_or_default();
            let rel_path: String = row.get(2)?;
            Ok(KnowledgeFileView {
                abs_path: if rel_path.is_empty() {
                    String::new()
                } else {
                    root.join(&rel_path).to_string_lossy().into_owned()
                },
                id: row.get(0)?,
                name: row.get(1)?,
                rel_path,
                url: row.get(3)?,
                mime: row.get(4)?,
                size: row.get::<_, i64>(5)? as u64,
                extract_status: row.get(6)?,
                chunk_count: row.get::<_, i64>(7)? as u32,
                created_at: row.get::<_, i64>(8)? as u64,
                base_ids: base_ids
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
                enriched: row.get::<_, i64>(10).unwrap_or(0) != 0,
                enrich_error: row
                    .get::<_, Option<String>>(11)
                    .unwrap_or_default()
                    .unwrap_or_default(),
                enriched_at: row.get::<_, i64>(12).unwrap_or(0) as u64,
            })
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /* ── 文件之间的关系（正文—附件 / 新旧版本） ── */

    /// 建立文件关系（幂等）。方向：`attachment` = from(正文) → to(附件)，
    /// `supersedes` = from(新版) → to(旧版)。
    ///
    /// 两类关系在 **to 端都要求唯一**：一个文件只能有一个正文、只能被一个新版取代。
    /// 否则"它的正文是谁""哪一版才是现行"就没有确定答案，附件树与"历史版本"标注都会含糊。
    /// 同时拒绝自指与成环（A 是 B 的附件、B 又是 A 的附件）。
    pub fn link_files(&self, kind: &str, from: &str, to: &str) -> Result<(), String> {
        if !matches!(kind, REL_ATTACHMENT | REL_SUPERSEDES) {
            return Err(format!("未知的文件关系类型: {}", kind));
        }
        if from == to {
            return Err("不能让文件关联到自己".into());
        }
        let conn = self.conn.lock().unwrap();
        for id in [from, to] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM kb_files WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if exists == 0 {
                return Err("文件不存在（可能已被删除），请刷新后重试".into());
            }
        }

        // to 端已被别的文件占用：说清占位者是谁，用户才知道先解除哪一条
        let occupied: Option<String> = conn
            .query_row(
                "SELECT from_file_id FROM kb_file_relations WHERE kind = ?1 AND to_file_id = ?2",
                params![kind, to],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(owner) = occupied.filter(|o| o != from) {
            let owner_name: String = conn
                .query_row(
                    "SELECT name FROM kb_files WHERE id = ?1",
                    params![owner],
                    |r| r.get(0),
                )
                .unwrap_or(owner);
            return Err(format!(
                "「{}」已经是{}了，请先解除已有关系",
                owner_name,
                if kind == REL_ATTACHMENT {
                    "另一个文件的附件"
                } else {
                    "另一个版本的替代对象"
                }
            ));
        }

        // 成环检查：从 to 沿同类关系往下走，能回到 from 就是环
        let mut cursor = to.to_string();
        let mut seen = std::collections::HashSet::new();
        while seen.insert(cursor.clone()) {
            let next: Option<String> = conn
                .query_row(
                    "SELECT to_file_id FROM kb_file_relations WHERE kind = ?1 AND from_file_id = ?2",
                    params![kind, cursor],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            match next {
                Some(n) if n == from => return Err("这会形成循环关系，请先调整已有的关系".into()),
                Some(n) => cursor = n,
                None => break,
            }
        }

        conn.execute(
            "INSERT OR IGNORE INTO kb_file_relations(from_file_id, to_file_id, kind, created_at)
             VALUES(?1, ?2, ?3, ?4)",
            params![from, to, kind, now_secs()],
        )
        .map_err(|e| format!("建立文件关系失败: {}", e))?;
        Ok(())
    }

    /// 解除文件关系；返回是否命中（未命中说明本来就没有这条关系）。
    pub fn unlink_files(&self, kind: &str, from: &str, to: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "DELETE FROM kb_file_relations WHERE kind = ?1 AND from_file_id = ?2 AND to_file_id = ?3",
                params![kind, from, to],
            )
            .map_err(|e| e.to_string())?;
        Ok(n > 0)
    }

    /// 列出本库文件之间的关系：两端都在本库的才返回
    /// （跨库的关系由另一侧的关系列表表达，避免前端拿到不属于本库的文件 id）。
    pub fn list_file_relations(&self, kb_id: &str) -> Vec<KnowledgeFileRelationView> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT r.from_file_id, r.to_file_id, r.kind
               FROM kb_file_relations r
              WHERE EXISTS (SELECT 1 FROM kb_file_links l WHERE l.file_id = r.from_file_id AND l.kb_id = ?1)
                AND EXISTS (SELECT 1 FROM kb_file_links l WHERE l.file_id = r.to_file_id AND l.kb_id = ?1)",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![kb_id], |r| {
            Ok(KnowledgeFileRelationView {
                from_file_id: r.get(0)?,
                to_file_id: r.get(1)?,
                kind: r.get(2)?,
            })
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /* ── 文件整理（AI 预处理） ── */

    /// 某个原文在某个库里的分块（按分块序号）：返回 (条目 key, 正文)
    pub fn file_chunks(&self, kb_id: &str, source_ref: &str) -> Vec<(String, String)> {
        if source_ref.is_empty() {
            return Vec::new();
        }
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT l.entry_key, m.value FROM kb_entry_links l
             JOIN key_memories m ON m.key = l.entry_key
             WHERE l.kb_id = ?1 AND l.source_ref = ?2
             ORDER BY COALESCE(l.chunk_index, 0)",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![kb_id, source_ref], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 写回整理结果：专属属性落**关联表**（每库各自一份值），标签落记忆表（全库通用）
    pub fn update_entry_enrich(
        &self,
        kb_id: &str,
        entry_key: &str,
        meta_json: &str,
        tags: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE kb_entry_links SET kb_meta = ?3 WHERE kb_id = ?1 AND entry_key = ?2",
            params![kb_id, entry_key, meta_json],
        )
        .map_err(|e| format!("写回专属属性失败: {}", e))?;
        conn.execute(
            "UPDATE key_memories SET tags = ?2 WHERE key = ?1 AND tags = ''",
            params![entry_key, normalize_tags(tags)],
        )
        .map_err(|e| format!("写回标签失败: {}", e))?;
        Ok(())
    }

    /// 标记某个原文的整理结果（未整理 = 0）。
    /// 成功时记下时间（`enriched_at`）；失败时**保留**上一次成功的时间 ——
    /// 界面上"上次整理于 X（本次失败）"比把时间清成 0 有用得多。
    pub fn mark_file_enriched(&self, file_id: &str, ok: bool, error: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        if ok {
            conn.execute(
                "UPDATE kb_files SET enriched = 1, enrich_error = '', enriched_at = ?2 WHERE id = ?1",
                params![file_id, now_secs()],
            )
            .map_err(|e| format!("更新整理状态失败: {}", e))?;
        } else {
            conn.execute(
                "UPDATE kb_files SET enriched = 0, enrich_error = ?2 WHERE id = ?1",
                params![file_id, error],
            )
            .map_err(|e| format!("更新整理状态失败: {}", e))?;
        }
        Ok(())
    }

    /* ── 待确认队列 ── */

    pub fn list_candidates(&self, kb_id: &str) -> Vec<KnowledgeCandidateView> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, kb_id, key, value, kb_meta, tags, origin, source_ref, reason, created_at
             FROM kb_candidates WHERE kb_id = ?1 ORDER BY created_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![kb_id], |row| {
            Ok(KnowledgeCandidateView {
                id: row.get(0)?,
                base_id: row.get(1)?,
                key: row.get(2)?,
                value: row.get(3)?,
                meta_json: row.get(4)?,
                tags: row.get(5)?,
                origin: row.get(6)?,
                source_ref: row.get(7)?,
                reason: row.get(8)?,
                created_at: row.get::<_, i64>(9)? as u64,
            })
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// 入队一条候选（app 整理完的结果先落这里，采纳才进库）
    #[allow(clippy::too_many_arguments)]
    pub fn add_candidate(
        &self,
        kb_id: &str,
        key: &str,
        value: &str,
        meta_json: &str,
        tags: &str,
        origin: &str,
        source_ref: &str,
        reason: &str,
    ) -> Result<i64, String> {
        validate_json_object(meta_json, "专属属性值")?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO kb_candidates(kb_id, key, value, kb_meta, tags, origin, source_ref, reason, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                kb_id,
                key,
                value,
                meta_json,
                normalize_tags(tags),
                origin,
                source_ref,
                reason,
                now_secs()
            ],
        )
        .map_err(|e| format!("写入待确认候选失败: {}", e))?;
        Ok(conn.last_insert_rowid())
    }

    /// 采纳候选：写入条目 + 关联，然后删除候选（允许采纳前改标题/正文/属性）
    pub fn adopt_candidate(
        &self,
        id: i64,
        key: Option<&str>,
        value: Option<&str>,
        meta_json: Option<&str>,
    ) -> Result<(), String> {
        let (kb_id, c_key, c_value, c_meta, c_tags, origin, source_ref) = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT kb_id, key, value, kb_meta, tags, origin, source_ref FROM kb_candidates WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "候选不存在（可能已被处理）".to_string())?
        };
        self.save_entry(
            &kb_id,
            key.unwrap_or(&c_key),
            value.unwrap_or(&c_value),
            &c_tags,
            &origin,
            &source_ref,
            Some(meta_json.unwrap_or(&c_meta)),
        )?;
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM kb_candidates WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn reject_candidate(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM kb_candidates WHERE id = ?1", params![id])
            .map_err(|e| format!("丢弃候选失败: {}", e))?;
        Ok(())
    }
}

/* ────────────────────────── 条目写入与投喂辅助 ────────────────────────── */

/// 清理孤儿文件（级联）：删掉登记行与磁盘原文，并沿「正文 → 附件」方向继续检查。
///
/// **孤儿的判据是两个维度都为空**（"与知识条目同构"的落点）：
///   - 没有任何库引用（不在任何库的文件列表里）；
///   - 没有任何文件引用它（不是别人的附件 / 旧版）。
///
/// 只看文件维度会误删「被单独放进另一个库的附件」这类文件 —— 它们必须保留。
/// `seeds` 是调用方已经解除库关联、注定要删的文件；被引用着的文件会连同它的关系一起留在原地。
/// 返回删除的文件数。
fn purge_orphan_files(
    tx: &rusqlite::Transaction,
    root: &Path,
    seeds: Vec<String>,
) -> Result<usize, String> {
    let mut pending = seeds;
    // 只跳过"已经删掉"的：不能用"访问过就跳过"—— 附件可能在第一轮还被别人引用着（保留），
    // 等它的正文被删、关系解除后才该删；用访问集去重会让它永远漏删。
    // 不会无限循环：入队只发生在删除发生时，而删除数受文件总数限制（关系图无环，见 link_files）。
    let mut deleted_ids = std::collections::HashSet::new();
    let mut deleted = 0usize;

    while let Some(id) = pending.pop() {
        if deleted_ids.contains(&id) {
            continue;
        }
        // 计数失败当 0 会误判成"没人引用"从而误删，所以这两处也必须传播错误
        let still_in_lib: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM kb_file_links WHERE file_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let referenced: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM kb_file_relations WHERE to_file_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if still_in_lib > 0 || referenced > 0 {
            // 还被库或别的文件引用着：保留（它作为"正文"指向的附件关系也就跟着保留）
            continue;
        }

        // 真要删了：记下它的附件（随后删掉出边），级联检查这些附件是否也成了孤儿
        let children: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT to_file_id FROM kb_file_relations WHERE from_file_id = ?1")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        let rel_path: Option<String> = tx
            .query_row(
                "SELECT rel_path FROM kb_files WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        // 原文被删 ⇒ 由它切出来的分块条目也一并清掉。
        //
        // 分块条目（key_memories 行）与文件是两套独立记录，只靠 `source_ref ↔ rel_path` 的约定
        // 挂在一起，**没有外键**。所以这里必须显式清：只删 kb_files 行的话，条目会留在各库的
        // 条目列表里（kb_entry_links 还在 → JOIN 得到），成为"有条目、没原文"的悬空数据。
        // 走到这一步说明该文件已无任何库引用、也无人引用它，它的分块自然也不该留。
        if let Some(rel) = rel_path.as_deref().filter(|r| !r.is_empty()) {
            let keys: Vec<String> = {
                let mut stmt = tx
                    .prepare("SELECT entry_key FROM kb_entry_links WHERE source_ref = ?1")
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(params![rel], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?;
                rows.filter_map(|r| r.ok()).collect()
            };
            for key in keys {
                tx.execute(
                    "DELETE FROM kb_entry_links WHERE entry_key = ?1",
                    params![key],
                )
                .map_err(|e| e.to_string())?;
                // 只删知识条目本体：同名 key 若已是会话记忆则不碰（save_entry_tx 里有同样的保护）
                tx.execute(
                    "DELETE FROM key_memories WHERE key = ?1 AND category = ?2",
                    params![key, CATEGORY_KNOWLEDGE],
                )
                .map_err(|e| e.to_string())?;
            }
        }

        tx.execute(
            "DELETE FROM kb_file_relations WHERE from_file_id = ?1 OR to_file_id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        if tx
            .execute("DELETE FROM kb_files WHERE id = ?1", params![id])
            .map_err(|e| format!("删除文件登记 {} 失败: {}", id, e))?
            > 0
        {
            deleted += 1;
            deleted_ids.insert(id.clone());
            if let Some(rel) = rel_path.filter(|r| !r.is_empty()) {
                // 原文删不掉不影响事务（比如用户手动改了根目录）：登记行已清，孤儿原文留着不阻塞
                let _ = std::fs::remove_file(root.join(&rel));
            }
        }
        pending.extend(children);
    }
    Ok(deleted)
}

/// 缺列则补列（与 db.rs 的 ensure_column 同一套做法，用于知识库表的增量迁移）
fn ensure_columns(conn: &Connection, table: &str, cols: &[(&str, &str)]) -> Result<(), String> {
    let existing: Vec<String> = {
        let mut stmt = match conn.prepare(&format!("PRAGMA table_info({})", table)) {
            Ok(s) => s,
            Err(_) => return Ok(()), // 表还不存在时由建表语句负责
        };
        stmt.query_map([], |r| r.get::<_, String>(1))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    };
    for (name, ddl) in cols {
        if existing.iter().any(|c| c == name) {
            continue;
        }
        conn.execute(
            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, name, ddl),
            [],
        )
        .map_err(|e| format!("迁移表 {}(加列 {})失败: {}", table, name, e))?;
    }
    Ok(())
}

fn validate_entry_input(key: &str, value: &str, meta_json: Option<&str>) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.chars().count() > 200 {
        return Err("知识条目标题不能为空且不超过 200 字符".into());
    }
    if value.trim().is_empty() {
        return Err("知识条目内容不能为空".into());
    }
    if let Some(m) = meta_json {
        validate_json_object(m, "专属属性值")?;
    }
    Ok(())
}

fn base_exists(tx: &rusqlite::Transaction, kb_id: &str) -> Result<bool, String> {
    let n: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM knowledge_bases WHERE id = ?1",
            params![kb_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 条目写入的共用实现（供单条保存与投喂分块共用；调用方负责事务与提交）
#[allow(clippy::too_many_arguments)]
fn save_entry_tx(
    tx: &rusqlite::Transaction,
    kb_id: &str,
    key: &str,
    value: &str,
    tags: &str,
    origin: &str,
    source_ref: &str,
    meta_json: Option<&str>,
    chunk: Option<(u32, u32)>,
) -> Result<(), String> {
    // 同名键若已是「会话记忆」，不覆盖 —— 两条链路语义不同，混用会互相踩
    let existing_cat: Option<String> = tx
        .query_row(
            "SELECT category FROM key_memories WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(cat) = existing_cat {
        if cat != CATEGORY_KNOWLEDGE {
            return Err(format!(
                "「{}」已是一条会话记忆（{}），请换一个标题",
                key, cat
            ));
        }
    }

    let now = now_secs();
    tx.execute(
        "INSERT INTO key_memories(key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
         VALUES(?1, ?2, ?3, ?4, ?4, 0, 0, 0, ?5)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, tags = excluded.tags",
        params![key, value, CATEGORY_KNOWLEDGE, now, tags],
    )
    .map_err(|e| format!("写入知识条目失败: {}", e))?;

    let (chunk_index, chunk_total) = match chunk {
        Some((i, t)) => (Some(i as i64), Some(t as i64)),
        None => (None, None),
    };
    tx.execute(
        "INSERT INTO kb_entry_links(kb_id, entry_key, kb_meta, origin, source_ref, chunk_index, chunk_total, added_at)
         VALUES(?1, ?2, COALESCE(?3, '{}'), ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(kb_id, entry_key) DO UPDATE SET
            kb_meta = CASE WHEN ?3 IS NULL THEN kb_meta ELSE ?3 END,
            origin = CASE WHEN ?4 = '' THEN origin ELSE ?4 END,
            source_ref = CASE WHEN ?5 = '' THEN source_ref ELSE ?5 END,
            chunk_index = COALESCE(?6, chunk_index),
            chunk_total = COALESCE(?7, chunk_total)",
        params![kb_id, key, meta_json, origin, source_ref, chunk_index, chunk_total, now],
    )
    .map_err(|e| format!("建立知识库关联失败: {}", e))?;
    Ok(())
}

/// 把某个原文（source_ref）已有的分块关联复制到另一个库：
/// 文件被第二个库引用时不重新落盘/抽取，但它的分块必须在新库里可见。返回该原文的分块总数。
fn relink_chunks(
    tx: &rusqlite::Transaction,
    kb_id: &str,
    source_ref: &str,
    now: u64,
) -> Result<usize, String> {
    let rows: Vec<(String, String, String, String, Option<i64>, Option<i64>)> = {
        let mut stmt = tx
            .prepare(
                "SELECT entry_key, kb_meta, origin, source_ref, chunk_index, chunk_total
                 FROM kb_entry_links WHERE source_ref = ?1 AND source_ref <> ''",
            )
            .map_err(|e| e.to_string())?;
        let it = stmt
            .query_map(params![source_ref], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        it.filter_map(|r| r.ok()).collect()
    };
    let total = rows.len();
    for (key, meta, origin, sref, ci, ct) in rows {
        tx.execute(
            "INSERT OR IGNORE INTO kb_entry_links(kb_id, entry_key, kb_meta, origin, source_ref, chunk_index, chunk_total, added_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![kb_id, key, meta, origin, sref, ci, ct, now],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(total)
}

/* ────────────────────────── 投喂的本地准备（不碰数据库） ────────────────────────── */

/// 投喂前的本地准备：抽取正文 → 原子块粗切。
///
/// 单独成一步是为了让 AI 分组能在**不持任何锁**的前提下 await ——
/// 连接池 guard 与互斥锁都不能跨 await，"AI 加工"和"落库"必须是两段。
pub struct IngestPrep {
    pub name: String,
    /// **来源身份**，不是文件内容的指纹：按"本地提取结果"算。
    ///
    /// 网页那份产物是 AI 整理后的版本，整理结果每次略有不同 —— 拿它算 sha 的话，
    /// 同一个页面每投喂一次就会多入库一份。所以 sha 恒定取自**来源**（提取出的全文），
    /// 而 `write_bytes` 是要落盘的东西，两者在网页 + AI 生效时**刻意不同**。
    pub sha: String,
    pub mime: String,
    /// done | unsupported | failed
    pub status: &'static str,
    /// 提取出的正文：文件投喂时它同时是"给 AI 看的块预览来源"与"切片来源"；
    /// 网页投喂时它只是 AI 整理的**输入**（产物见 `write_bytes`）
    pub text: String,
    pub blocks: Vec<Block>,
    /// 要落盘的东西：文件投喂 = 原始字节（一字不改）；网页投喂 = AI 整理后的 .md
    pub write_bytes: Vec<u8>,
}

/// 计划落库的一条知识：正文已由调用方按原文区间切好
#[derive(Debug, Clone, Default)]
pub struct PlannedChunk {
    pub text: String,
    /// AI 给的标题（降级路径为空 → key 退回文件名）
    pub title: String,
    pub tags: String,
    pub meta_json: String,
}

/// 本地准备一个待投喂的文件（读盘 + 抽取 + 粗切，不碰库）
///
/// `as_markdown` = 界面上那个「存为 markdown 格式」（含内容整理）：内容会由 AI 加工并落盘为 `.md`，
/// **不再与用户的源文件逐字一致**。它同时决定本文件的"来源身份"（见下面的 sha）。
pub fn prepare_file(source: &std::path::Path, as_markdown: bool) -> Result<IngestPrep, String> {
    let bytes =
        std::fs::read(source).map_err(|e| format!("读取文件失败 {}: {}", source.display(), e))?;
    if bytes.is_empty() {
        return Err("文件内容为空".into());
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!(
            "文件超过 {} MB，请先拆分或改用片段投喂",
            MAX_FILE_BYTES / 1024 / 1024
        ));
    }
    let name = source
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    // 加工形态并进"来源身份"：原样一份、整理后一份是**两种产物**，应当能共存。
    // 不并的话，同一份文件第二次用另一种形态投喂会命中 sha 去重 → 用户的加工意图被静默忽略，
    // 他看到"投喂成功"，拿到的却是上次那份（重命名/去重键这两个坑，都是同一个道理）
    let sha = if as_markdown {
        sha256_hex_tagged(&bytes, b":md")
    } else {
        sha256_hex(&bytes)
    };
    let (text, status) = extract_text(&name, &bytes);
    let text = text.unwrap_or_default();
    let blocks = split_blocks_ranged(&text);
    Ok(IngestPrep {
        mime: guess_mime(&name).to_string(),
        name,
        sha,
        status,
        blocks,
        text,
        write_bytes: bytes,
    })
}

/// 网页原文的落盘名（`<清洗后的标题>.md`）。
///
/// 抽出来是因为这个名字有**两个来源**：用户填的，或 AI 给整篇取的。
/// 两条路径必须用同一个格式化口径，否则会出现"AI 取的名带 .md、用户填的不带"这类不一致。
pub fn web_file_name(title: &str) -> String {
    format!("{}.md", paths::sanitize_dir_name(title, "webpage"))
}

/// 本地准备一段网页正文（HTML 的抓取与粗提取在命令层完成）
pub fn prepare_web(title: &str, text: &str) -> Result<IngestPrep, String> {
    let body = text.trim();
    if body.is_empty() {
        return Err("抓到的页面没有正文".into());
    }
    if body.chars().count() > MAX_URL_TEXT_CHARS {
        return Err(format!(
            "页面正文超过 {} 字符，请改用片段投喂",
            MAX_URL_TEXT_CHARS
        ));
    }
    let name = web_file_name(title);
    let bytes = body.as_bytes().to_vec();
    Ok(IngestPrep {
        sha: sha256_hex(&bytes),
        mime: "text/markdown".into(),
        name,
        status: "done",
        blocks: split_blocks_ranged(body),
        text: body.to_string(),
        write_bytes: bytes,
    })
}

/// 本地准备一份云文档正文（取文在命令层完成；这里只做归一化与粗切）。
///
/// 与 `prepare_web` 的差别只有一处：**不做正文长度拦截**。
/// 那条 20 万字符的上限是为"抓来的网页"设的（网页噪声多、来源不可控，超长基本是抓错了页面）；
/// 云文档是**用户自己的原文**，长度只该受分块上限（`MAX_CHUNKS_PER_FILE`）约束 ——
/// 在这里拦掉等于"用户明明能打开这篇文档，程序却拒绝投喂"。
/// 字节上限仍然保留：一条命令跨网络搬运超大数据没有意义。
pub fn prepare_cloud(title: &str, text: &str) -> Result<IngestPrep, String> {
    let body = text.trim();
    if body.is_empty() {
        return Err("这篇文档没有正文".into());
    }
    let bytes = body.as_bytes().to_vec();
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!(
            "文档正文超过 {} MB，请改用片段投喂",
            MAX_FILE_BYTES / 1024 / 1024
        ));
    }
    Ok(IngestPrep {
        sha: sha256_hex(&bytes),
        mime: "text/markdown".into(),
        name: web_file_name(title),
        status: "done",
        blocks: split_blocks_ranged(body),
        text: body.to_string(),
        write_bytes: bytes,
    })
}

/// 单文件分块数的兜底：超过就截断并标记。
///
/// 分块数无上限时，一份超大文档能把图谱与列表一起压死（每个分块都是一个节点）。
/// 规则分块那条路径自己也截，这里把它抽出来给 AI 整理的路径复用 —— 两条路径的上限必须是同一个数。
pub fn cap_chunks(chunks: Vec<PlannedChunk>) -> (Vec<PlannedChunk>, bool) {
    if chunks.len() <= MAX_CHUNKS_PER_FILE {
        return (chunks, false);
    }
    (chunks.into_iter().take(MAX_CHUNKS_PER_FILE).collect(), true)
}

/// 喂模型之前，把过碎的相邻块先合并。
///
/// 为什么需要：像「1. 求告 / + 主要人物：… / + 核心冲突：…」这种大纲式文档（网页另存、清单型资料），
/// 原子块会碎成上百个三四十字的小块。碎块一多，一次调用装不下（`PLAN_BLOCKS_PER_CALL`）、
/// 模型要么给不出可用分组、要么输出被 max_tokens 截断，整篇就降级成规则分块 ——
/// 用户看到的现象是「AI 没生效」+ 分块标题是从半句话上截下来的碎片。
/// 合并后块数下降一个量级，一次调用就够，批边界切开语义的问题也一并消失。
///
/// 只并**相邻**块；本身够长的块原样保留，所以段落型文档的行为完全不变。
pub fn merge_small_blocks(text: &str, blocks: &[Block], min_chars: usize) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for b in blocks {
        if let Some(last) = out.last_mut() {
            if last.text.chars().count() < min_chars {
                last.end = b.end;
                // 正文重新从原文切片，而不是把两段文本拼起来 —— 拼起来会丢掉中间的空行，
                // 而 `assemble_plans` 正是按 [start, end) 从原文取字的，两边必须是同一份内容
                last.text = text[last.start..last.end].to_string();
                continue;
            }
        }
        out.push(b.clone());
    }
    out
}

/// 按 AI 给的块序号，从**原文区间**拼出每条知识的正文（一字不改）。
///
/// 区间是连续的原始切片（含中间空行）；`parse_plans` 已保证区间内不含被丢弃的噪声块。
/// `blocks` 必须是**喂给模型的那一份**（即 `merge_small_blocks` 之后的结果）——
/// 否则序号对不上，切出来的正文会错位。
/// 返回 (计划, 是否因单文件块数上限被截断)。
pub fn assemble_plans(
    prep: &IngestPrep,
    blocks: &[Block],
    plans: &[crate::api_agent::kb_llm::ChunkPlan],
) -> (Vec<PlannedChunk>, bool) {
    let mut out: Vec<PlannedChunk> = Vec::new();
    for p in plans {
        if out.len() >= MAX_CHUNKS_PER_FILE {
            break;
        }
        if p.from == 0 || p.from > blocks.len() {
            continue;
        }
        let from = p.from - 1;
        let to = p.to.min(blocks.len()).saturating_sub(1);
        if to < from {
            continue;
        }
        let text = prep.text[blocks[from].start..blocks[to].end].to_string();
        out.push(PlannedChunk {
            text,
            title: p.title.clone(),
            tags: p.tags.join(","),
            meta_json: serde_json::to_string(&p.meta).unwrap_or_else(|_| "{}".into()),
        });
    }
    let truncated = plans.len() > out.len();
    (out, truncated)
}

/// 写入分块条目。
///
/// key = `<标题>-<sha8>#<序号>`：标题来自 AI（可读，且能被 key/tags 的关键词检索命中），
/// 没有标题（降级路径）就用文件名 stem；`-<sha8>` + `#序号` 保证跨文件、跨内容版本都不串块。
///
/// **同批内必须去重**：两条同标题会让 `save_entry_tx` 的 UPSERT 撞 key，
/// 后一条会**覆盖**前一条的正文 —— 那等于丢用户内容。去重方式是给标题加 `（2）` 后缀。
#[allow(clippy::too_many_arguments)]
fn write_planned_chunks(
    tx: &rusqlite::Transaction,
    kb_id: &str,
    stem_seed: &str,
    file_name: &str,
    sha8: &str,
    rel_path: &str,
    origin: &str,
    chunks: &[PlannedChunk],
) -> Result<usize, String> {
    let total = chunks.len() as u32;
    let mut used_stems: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut written = 0usize;
    for (i, c) in chunks.iter().enumerate() {
        if c.text.trim().is_empty() {
            continue;
        }
        let base = if c.title.trim().is_empty() {
            // AI 没给标题：从正文推导一个能看的小标题，不要退化成"整个文件名"
            let derived = derive_title(&c.text);
            if derived.is_empty() {
                stem_seed.to_string()
            } else {
                paths::sanitize_dir_name(&derived, stem_seed)
            }
        } else {
            // 标题里带 # 会打乱 `#序号` 的约定，一并清洗掉
            paths::sanitize_dir_name(&c.title.replace('#', ""), stem_seed)
        };
        let base: String = base.chars().take(60).collect();
        let mut stem = base.clone();
        let mut n = 2;
        while !used_stems.insert(stem.clone()) {
            stem = format!("{}（{}）", base, n);
            n += 1;
        }
        let key = format!("{}-{}#{}", stem, sha8, i + 1);
        let meta = if c.meta_json.trim().is_empty() || c.meta_json.trim() == "{}" {
            None
        } else {
            Some(c.meta_json.as_str())
        };
        save_entry_tx(
            tx,
            kb_id,
            &key,
            &c.text,
            &c.tags,
            origin,
            rel_path,
            meta,
            Some((i as u32 + 1, total)),
        )
        .map_err(|e| format!("写入「{}」失败: {}", file_name, e))?;
        written += 1;
    }
    Ok(written)
}

/// 从正文推导一个**能看**的标题（AI 没给标题时的兜底）。
///
/// 为什么不直接"截前 N 个字符"：那样会把半句话、半个词当标题，一眼就能看出是机器截的。
/// 这里按优先级取：markdown 标题 → 首个短句 → 30 字内就近的**句末**标点 →
/// 退到停顿标点（逗号处断出来仍是半句，但总比硬截断好）→ 实在没有再按字数截断
/// （且末尾加省略号，诚实标明被截断，不假装是完整标题）。
/// 末尾的悬空标点一律去掉 —— `核心冲突：……展开争夺，` 这种收尾正是"无意义截断标题"的标志。
pub fn derive_title(text: &str) -> String {
    const MAX: usize = 30;
    const ENDS: &str = "。！？.!?";
    const PAUSES: &str = "；;，,、：:";
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if first.is_empty() {
        return String::new();
    }
    // 去掉行首的 markdown 记号（标题、引用、列表）
    let body = first
        .trim_start_matches(['#', '>', '-', '*', '+', ' '])
        .trim();
    let body = if body.is_empty() { first } else { body };
    let chars: Vec<char> = body.chars().collect();
    if chars.len() <= MAX {
        return trim_pauses(body);
    }
    if let Some(pos) = chars.iter().take(MAX).rposition(|c| ENDS.contains(*c)) {
        return chars[..=pos].iter().collect();
    }
    if let Some(pos) = chars.iter().take(MAX).rposition(|c| PAUSES.contains(*c)) {
        if pos >= 8 {
            return chars[..pos].iter().collect();
        }
    }
    let mut s: String = chars[..MAX].iter().collect();
    s.push('…');
    s
}

/// 去掉标题末尾的悬空标点（`…展开争夺，` → `…展开争夺`）
fn trim_pauses(s: &str) -> String {
    s.trim_end_matches(['；', ';', '，', ',', '、', '：', ':', ' '])
        .to_string()
}

/// 文件名去扩展名 → 事件标题用的短名（分块 key 的前缀）
fn key_stem(file_name: &str) -> String {
    let stem = file_name
        .rsplit_once('.')
        .map(|(a, _)| a)
        .unwrap_or(file_name);
    let cleaned = paths::sanitize_dir_name(stem, "doc");
    cleaned.chars().take(80).collect()
}

/// 内容指纹：同内容 = 同 sha = 只存一份（多库共享的判据）
pub fn sha256_hex(bytes: &[u8]) -> String {
    sha256_hex_tagged(bytes, b"")
}

/// 带标签的内容指纹：用于"**同一份源文件、不同加工形态**要各自入库"的场合
/// （原样 vs 存为 markdown）。
///
/// 为什么不直接 `format!("{}:md", sha)`：这个字段在库里是"来源身份"，
/// 保持它是标准 64 位十六进制 sha256，长度与字符集都稳定，日后要拿它做校验/比对才不会被这个后缀绊住。
/// 标签为空时与原实现**逐位相同**，所以存量库的 sha 不需要迁移。
pub fn sha256_hex_tagged(bytes: &[u8], tag: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    if !tag.is_empty() {
        h.update(tag);
    }
    format!("{:x}", h.finalize())
}

fn guess_mime(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        "csv" | "tsv" => "text/csv",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "yaml" | "yml" => "application/yaml",
        "xml" => "application/xml",
        _ => "",
    }
}

/// 文本类扩展名（直接按 UTF-8 读；读不了再降级 lossy）
const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "json", "yaml", "yml", "toml", "csv", "tsv", "log", "ini", "conf",
    "xml", "html", "htm", "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "c", "cpp", "h",
    "hpp", "cs", "rb", "php", "sh", "ps1", "sql", "css", "scss", "vue", "svelte", "kt", "swift",
];

/// 抽取正文：返回 (正文, 状态)。状态 `unsupported` 表示"已登记原文但暂不支持抽取"，
/// 不是错误 —— 用户仍能在「文件」里看到它，只是内容不进检索。
fn extract_text(name: &str, bytes: &[u8]) -> (Option<String>, &'static str) {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    // HTML 走 markdown 转换，而不是当纯文本原样收下：直接留原始标签的话，
    // 分块里全是 `<div class="...">`，真正的正文反而被埋掉（与网页投喂共用同一个提取层）
    if ext == "html" || ext == "htm" {
        let text = crate::tools::browser::html_to_markdown(&String::from_utf8_lossy(bytes));
        if text.trim().is_empty() {
            // 纯 JS 渲染的空壳页会走到这里：文件仍然登记、可打开，但内容不进检索
            return (None, "failed");
        }
        return (Some(text), "done");
    }
    if TEXT_EXTS.contains(&ext.as_str()) {
        return (Some(String::from_utf8_lossy(bytes).into_owned()), "done");
    }
    if ext == "pdf" || bytes.starts_with(b"%PDF") {
        return match pdf_extract::extract_text_from_mem(bytes) {
            Ok(t) => (Some(t), "done"),
            Err(_) => (None, "failed"),
        };
    }
    (None, "unsupported")
}

/// 正文切块（**块级**，不跨块切断）。
///
/// 为什么不是简单的"按空行切段 + 贪心装箱"：那样会出现三种破块：
///   1. 围栏代码块 / markdown 表格被空行或长度切开 → 代码与表格支离破碎；
///   2. 标题被单独留在上一块末尾（有标题没内容）；
///   3. 单段超长时按固定字数硬切 → 切在句子中间。
///
/// 这里改成：先解析出**原子块**（围栏 / 表格 / 列表项 / 段落，标题粘到其后的块上），
/// 原子块整体进块；单个原子块超过 `max` 时只在**句子/行边界**上切；过短的尾块并入上一块（不丢内容）。
pub fn chunk_text(text: &str, target: usize, max: usize) -> Vec<String> {
    let blocks = split_blocks(text);
    if blocks.is_empty() {
        return Vec::new();
    }

    // 超长块先按句子/行边界拆开（围栏与表格不拆：宁可超长，也不切碎）
    let mut units: Vec<String> = Vec::new();
    for b in blocks {
        if b.chars().count() <= max || is_atomic(&b) {
            units.push(b);
        } else {
            units.extend(split_by_sentence(&b, max));
        }
    }

    // 贪心装箱：target 只是"期望长度"，装不下就收块；比 target 长的单块自成一块（不跨块切）
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for u in units {
        let ulen = u.chars().count();
        if !cur.is_empty() && cur.chars().count() + ulen + 2 > target {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(&u);
    }
    if !cur.trim().is_empty() {
        chunks.push(cur);
    }

    // 过短的尾块并进上一块
    let mut merged: Vec<String> = Vec::new();
    for c in chunks {
        if c.chars().count() < MIN_CHUNK {
            if let Some(last) = merged.last_mut() {
                last.push_str("\n\n");
                last.push_str(&c);
                continue;
            }
        }
        merged.push(c);
    }
    merged
}

/// 围栏代码块与表格：整体不可再切（切了就不可读、也不可复用）
fn is_atomic(block: &str) -> bool {
    let first = block.trim_start();
    first.starts_with("```") || first.starts_with("~~~") || first.starts_with('|')
}

/// 粗切出来的**原子块**：文本 + 它在规范化正文里的字节区间。
///
/// 带上区间是"AI 只定边界、正文原样"的基础：模型只返回块序号，正文由这里按原始区间取，
/// 中间的空行/标题层级都保持原样，**绝不经过模型改写**。
#[derive(Debug, Clone)]
pub struct Block {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// 解析原子块（带原文区间）。规则：
/// - 围栏 ``` / ~~~ 一直到收尾围栏（内部空行不拆）
/// - 表格：连续的 `|` 行
/// - 列表：从一个列表项开始，直到空行；**每个列表项是一个独立单元**（拆分只会发生在项之间）
/// - 标题：粘到其后的那个块上（连续标题合在一起）
/// - 其余：连续非空行合成一个段落
pub fn split_blocks_ranged(text: &str) -> Vec<Block> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    // 每行在 normalized 里的起始偏移（行间恰好一个 '\n'）
    let mut offsets: Vec<usize> = Vec::with_capacity(lines.len());
    {
        let mut off = 0usize;
        for (idx, l) in lines.iter().enumerate() {
            offsets.push(off);
            off += l.len();
            if idx + 1 < lines.len() {
                off += 1;
            }
        }
    }
    let line_end = |i: usize| offsets[i] + lines[i].len();
    let slice = |a: usize, b: usize| -> &str { &normalized[offsets[a]..line_end(b)] };

    // 行号区间（含首含尾）：标题是"待粘到后继块"的一段，可能连续多行
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut heading: Option<(usize, usize)> = None;
    macro_rules! push_span {
        ($body:expr) => {{
            let (bs, be) = $body;
            if !slice(bs, be).trim().is_empty() {
                match heading.take() {
                    Some((hs, _)) => spans.push((hs, be)), // 标题与正文合成一块（区间连起来）
                    None => spans.push((bs, be)),
                }
            }
        }};
    }

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // 标题：先攒着，粘到后面的块
        if trimmed.starts_with('#') {
            heading = Some(match heading.take() {
                Some((hs, _)) => (hs, i),
                None => (i, i),
            });
            i += 1;
            continue;
        }

        // 围栏代码块
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let fence = &trimmed[..3];
            let start = i;
            i += 1;
            while i < lines.len() {
                let end = lines[i].trim_start().starts_with(fence);
                i += 1;
                if end {
                    break;
                }
            }
            push_span!((start, i - 1));
            continue;
        }

        // 表格
        if trimmed.starts_with('|') {
            let start = i;
            i += 1;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                i += 1;
            }
            push_span!((start, i - 1));
            continue;
        }

        // 列表：逐项成块（项内可含缩进续行）
        if is_list_marker(trimmed) {
            while i < lines.len() {
                let t = lines[i].trim();
                if t.is_empty() {
                    break;
                }
                if is_list_marker(t) {
                    let start = i;
                    i += 1;
                    // 吸收该项的续行（缩进且不是新项/新标题）
                    while i < lines.len() {
                        let nt = lines[i].trim();
                        if nt.is_empty() || is_list_marker(nt) || nt.starts_with('#') {
                            break;
                        }
                        i += 1;
                    }
                    push_span!((start, i - 1));
                } else {
                    break;
                }
            }
            continue;
        }

        // 段落：连续非空行（遇标题/围栏/表格/列表即停）
        let start = i;
        i += 1;
        while i < lines.len() {
            let t = lines[i].trim();
            if t.is_empty()
                || t.starts_with('#')
                || t.starts_with("```")
                || t.starts_with("~~~")
                || t.starts_with('|')
                || is_list_marker(t)
            {
                break;
            }
            i += 1;
        }
        push_span!((start, i - 1));
    }

    // 只剩标题（文档以标题结尾）时也保留下来
    if let Some((hs, he)) = heading {
        spans.push((hs, he));
    }

    spans
        .into_iter()
        .map(|(a, b)| {
            // 文本与区间取同一份内容（都去掉首尾空白），保证按区间切片等于按块取文本
            let raw = slice(a, b);
            let lead = raw.len() - raw.trim_start().len();
            let tail = raw.trim_end().len();
            Block {
                text: raw[lead..tail].to_string(),
                start: offsets[a] + lead,
                end: offsets[a] + tail,
            }
        })
        .collect()
}

fn split_blocks(text: &str) -> Vec<String> {
    split_blocks_ranged(text)
        .into_iter()
        .map(|b| b.text)
        .collect()
}

fn is_list_marker(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return true;
    }
    // 有序列表：1. / 1) / 1、
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return false;
    }
    let rest = &t[digits.len()..];
    rest.starts_with(". ") || rest.starts_with(") ") || rest.starts_with('、')
}

/// 超长块按句子切：先切句（保留句末标点），再按 max 打包；单句仍超长才按字符硬切。
/// 行末也是合法切点（长列表项、无标点的技术文本）。
fn split_by_sentence(block: &str, max: usize) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut buf = String::new();
    for ch in block.chars() {
        buf.push(ch);
        let boundary = matches!(ch, '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';' | '\n');
        if boundary && buf.chars().count() >= 40 {
            sentences.push(buf.trim_end().to_string());
            buf.clear();
        }
    }
    if !buf.trim().is_empty() {
        sentences.push(buf.trim_end().to_string());
    }

    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for s in sentences {
        if s.chars().count() > max {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            // 单句超长：最后手段，按 max 硬切（无法避免，但不能让它拖垮整块）
            let chars: Vec<char> = s.chars().collect();
            for w in chars.chunks(max) {
                out.push(w.iter().collect());
            }
            continue;
        }
        if !cur.is_empty() && cur.chars().count() + s.chars().count() > max {
            out.push(std::mem::take(&mut cur));
        }
        cur.push_str(&s);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/* ────────────────────────── 校验/构造辅助 ────────────────────────── */

/// 专属字段定义必须是 JSON 数组（只做结构校验，字段语义由前端编辑器保证）
fn validate_field_schema(raw: &str) -> Result<(), String> {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Array(_)) => Ok(()),
        Ok(_) => Err("专属字段定义必须是 JSON 数组".into()),
        Err(e) => Err(format!("专属字段定义不是合法 JSON: {}", e)),
    }
}

fn validate_json_object(raw: &str, label: &str) -> Result<(), String> {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(_)) => Ok(()),
        Ok(_) => Err(format!("{}必须是 JSON 对象", label)),
        Err(e) => Err(format!("{}不是合法 JSON: {}", label, e)),
    }
}

/// `kb_meta` 的 JSON 路径：字段名随用户定义，直接拼进 SQL 会注入 ——
/// 这里只接受字母/数字/下划线/汉字，返回的路径仍作为**绑定参数**传给 json_extract。
fn json_path_for(field: &str) -> Option<String> {
    if field.is_empty() || field.len() > 64 {
        return None;
    }
    if !field
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || ('\u{4e00}'..='\u{9fff}').contains(&c))
    {
        return None;
    }
    Some(format!("$.\"{}\"", field))
}

/// trigram 分词器的最小可检索长度。
///
/// **短于此长度的词一定命中不了 FTS**（trigram 至少要有 3 个字符才能切出窗口），
/// 所以凡是走 `kb_entry_fts` 的地方都必须对短词退化到 LIKE —— 而中文里两字词极常见
/// （「量子」「审批」「报销」），漏掉这个退化就等于"中文短词搜不到"。
pub(crate) const FTS_MIN_CHARS: usize = 3;

/// 把用户输入转成 FTS 表达式：按空白/常见分隔符切词，只留够长的词，词间 OR。
///
/// `pub(crate)`：知识库页搜索、条目的正文检索（`MemoryStore::search_knowledge`）共用，
/// 三处必须同一口径 —— 否则"页面能搜到、会话里搜不到"这种差异会再出现一次。
pub(crate) fn fts_query(raw: &str) -> Option<String> {
    let terms: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '、' | ';' | '；' | '|'))
        .map(str::trim)
        .filter(|t| t.chars().count() >= FTS_MIN_CHARS)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_agent::kb_llm::ChunkPlan;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn tmp_dir() -> String {
        // 带上纳秒：只用 pid + 序号会撞车（操作系统会复用 pid，上一轮留下的库文件还在，
        // 于是 create_base 报"已存在"这种莫名其妙的失败）
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk_kb_test_{}_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    const FIELDS: &str = r#"[{"key":"module","label":"模块","type":"select"}]"#;

    #[test]
    fn schema_and_trigram_search_works() {
        let dir = tmp_dir();
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "描述", FIELDS).unwrap();
        store
            .save_entry(
                "kb1",
                "知识库数据模型",
                "库定义与条目分表，关联行承载专属属性",
                "知识库,SQLite",
                "snippet",
                "",
                Some(r#"{"module":"后端"}"#),
            )
            .unwrap();

        // 中文子串命中（trigram 的意义所在）
        let hit = store.list_entries("kb1", Some("数据模型"), None, false, None);
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].key, "知识库数据模型");
        assert_eq!(hit[0].meta_json, r#"{"module":"后端"}"#);
        assert_eq!(hit[0].base_ids, vec!["kb1".to_string()]);

        // 正文命中（key 里没有的词）
        let by_value = store.list_entries("kb1", Some("专属属性"), None, false, None);
        assert_eq!(by_value.len(), 1);

        // 短词（<3 字）走 LIKE 退化路径，仍要能命中（SQLite 标签里含 "SQ"）
        let short = store.list_entries("kb1", Some("SQ"), None, false, None);
        assert_eq!(short.len(), 1);
    }

    #[test]
    fn entry_filters_by_origin_pin_and_meta() {
        let dir = tmp_dir();
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", FIELDS).unwrap();
        store
            .save_entry(
                "kb1",
                "A",
                "内容A",
                "",
                "snippet",
                "",
                Some(r#"{"module":"前端"}"#),
            )
            .unwrap();
        store
            .save_entry(
                "kb1",
                "B",
                "内容B",
                "",
                "file",
                "docs/x.md",
                Some(r#"{"module":"后端"}"#),
            )
            .unwrap();

        assert_eq!(
            store
                .list_entries("kb1", None, Some("file"), false, None)
                .len(),
            1
        );
        assert_eq!(
            store
                .list_entries("kb1", None, None, false, Some(r#"{"module":"前端"}"#))
                .len(),
            1
        );
        // 非法字段名（含引号）不参与过滤，也不能让 SQL 出错
        assert_eq!(
            store
                .list_entries("kb1", None, None, false, Some(r#"{"a\"b":"x"}"#))
                .len(),
            2
        );
    }

    #[test]
    fn delete_base_only_removes_orphan_entries() {
        let dir = tmp_dir();
        let root = std::path::PathBuf::from(&dir);
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", FIELDS).unwrap();
        store.create_base("kb2", "库二", "", "[]").unwrap();
        store
            .save_entry("kb1", "共享", "两个库都有", "", "snippet", "", None)
            .unwrap();
        store
            .save_entry("kb2", "共享", "两个库都有", "", "snippet", "", None)
            .unwrap();
        store
            .save_entry("kb1", "独有", "只属于库一", "", "snippet", "", None)
            .unwrap();

        let out = store.delete_base("kb1", &root).unwrap();
        assert_eq!(out.entries, 1, "只应删掉不再被任何库关联的那条");
        assert_eq!(store.list_entries("kb2", None, None, false, None).len(), 1);

        // 删掉最后一个引用它的库 → 条目本体才被删除
        let out2 = store.delete_base("kb2", &root).unwrap();
        assert_eq!(out2.entries, 1);
        let conn = store.conn.lock().unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM key_memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    /// 从「KV 记忆」侧删条目（只删 key_memories 行的通用删除路径）时，
    /// 触发器必须把关联行一并清掉、并重算原文的分块数 —— 否则留下悬空关联 + 虚高计数。
    #[test]
    fn deleting_entry_from_kv_side_cleans_links_and_recounts_chunks() {
        let dir = tmp_dir();
        let root = std::path::PathBuf::from(&dir);
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();

        let src = root.join("notes.md");
        std::fs::write(&src, "知识库分块测试。".repeat(400)).unwrap();
        let out = ingest_no_ai(&store, &root, "kb1", &src).unwrap();
        assert!(out.chunks >= 2, "样本应被切成多块，实际 {}", out.chunks);

        let keys: Vec<String> = store
            .list_entries("kb1", None, Some("file"), false, None)
            .iter()
            .map(|e| e.key.clone())
            .collect();
        assert_eq!(keys.len(), out.chunks);

        {
            let conn = store.conn.lock().unwrap();
            conn.execute("DELETE FROM key_memories WHERE key = ?1", params![keys[0]])
                .unwrap();
            let left: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM kb_entry_links WHERE entry_key = ?1",
                    params![keys[0]],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(left, 0, "触发器应把该条目的关联行一并清掉，不留悬空");
        }

        let files = store.list_files(&root, "kb1");
        assert_eq!(
            files[0].chunk_count as usize,
            out.chunks - 1,
            "分块数应重算，而非停在投喂时的旧值"
        );
        assert_eq!(
            store.list_entries("kb1", None, None, false, None).len(),
            out.chunks - 1
        );
    }

    #[test]
    fn chunk_text_prefers_paragraphs_and_keeps_content() {
        // 三段各 ~20 字，目标 30 → 会被切成多块
        let text = "第一段内容比较长一些用于测试装箱行为。\n\n第二段内容也比较长一些用于测试。\n\n第三段内容同样不短。\n\n尾。";
        let chunks = chunk_text(text, 30, 200);
        assert!(chunks.len() >= 2, "应当切开多块，实际 {:?}", chunks);
        // 不丢内容：拼回去所有原文片段都在（顺序拼接后应包含每段）
        let joined = chunks.join("\n");
        assert!(joined.contains("第一段"));
        assert!(joined.contains("第三段"));
        assert!(joined.contains("尾"), "过短的尾块应被并入而不是丢弃");

        // 单段超长 → 硬切，且每块不超过 max
        let long = "字".repeat(250);
        let hard = chunk_text(&long, 60, 100);
        assert!(hard.len() >= 3);
        assert!(hard.iter().all(|c| c.chars().count() <= 100));
    }

    #[test]
    fn chunk_keeps_code_tables_and_headings_whole() {
        let text = "# 标题\n\n正文第一段说明。\n\n```rust\nfn main() {\n\n    println!(\"hi\");\n}\n```\n\n| 列A | 列B |\n| --- | --- |\n| 1 | 2 |\n\n结尾说明。";
        let chunks = chunk_text(text, 30, 400);
        let joined = chunks.join("\n");
        assert!(
            joined.contains("fn main() {\n\n    println!(\"hi\");\n}"),
            "围栏代码块应整块保留（内部空行不该拆开）: {:?}",
            chunks
        );
        assert!(
            joined.contains("| 列A | 列B |\n| --- | --- |\n| 1 | 2 |"),
            "表格应整块保留: {:?}",
            chunks
        );
        // 标题不与正文分离：不应出现"只有标题"的块
        assert!(
            !chunks.iter().any(|c| c.trim() == "# 标题"),
            "标题不应自成一块: {:?}",
            chunks
        );
    }

    #[test]
    fn chunk_splits_long_paragraph_at_sentence_boundary() {
        // 一句话 27 字，重复 10 次；max=120 → 必须在句末切开，不能切在句子中间
        let sentence = "这是一个用于测试分块的句子，它足够长以便触发切分逻辑。";
        let text = sentence.repeat(10);
        let chunks = chunk_text(&text, 120, 120);
        assert!(chunks.len() >= 2, "应当切成多块: {:?}", chunks);
        for c in &chunks {
            assert!(
                c.chars().count() <= 120,
                "块超长({}): {}",
                c.chars().count(),
                c
            );
            assert!(c.trim_end().ends_with('。'), "应当在句末切开: {:?}", c);
        }
        // 不丢内容：10 个句号一个不少
        let stops = chunks.join("").chars().filter(|c| *c == '。').count();
        assert_eq!(stops, 10);
    }

    #[test]
    fn ingest_file_dedups_by_content_and_relinks_other_bases() {
        let db_dir = tmp_dir();
        let root = std::path::PathBuf::from(tmp_dir());
        let src_dir = std::path::PathBuf::from(tmp_dir());
        let src = src_dir.join("notes.md");
        // 两段各 ~700 字：按生产参数（目标 1000 字）会切成 2 块，用来验证多块入库
        let para1 = format!("第一段正文内容，讲一个结论。{}", "甲".repeat(700));
        let para2 = format!("第二段正文内容，讲另一个结论。{}", "乙".repeat(700));
        std::fs::write(&src, format!("# 标题\n\n{}\n\n{}\n", para1, para2)).unwrap();

        let store = KnowledgeStore::open(&db_dir).unwrap();
        store.create_base("kb1", "库一", "", FIELDS).unwrap();
        store.create_base("kb2", "库二", "", "[]").unwrap();

        let first = ingest_no_ai(&store, &root, "kb1", &src).unwrap();
        assert_eq!(first.status, "done");
        assert!(first.chunks >= 2, "两段正文应切成至少两块");
        assert!(!first.reused);
        assert!(root.join(&first.rel_path).exists(), "原文应落到知识库目录");

        // 同一份内容投给另一个库：不重复落盘、不重复抽取，只补关联
        let second = ingest_no_ai(&store, &root, "kb2", &src).unwrap();
        assert!(second.reused, "同 sha256 应复用已有原文");
        assert_eq!(second.rel_path, first.rel_path);
        assert_eq!(second.chunks, first.chunks, "分块应被关联到新库而不是重切");
        assert_eq!(store.list_files(&root, "kb1").len(), 1);
        assert_eq!(store.list_files(&root, "kb2").len(), 1);
        assert_eq!(
            store.list_entries("kb2", None, None, false, None).len(),
            first.chunks
        );

        // 两个库看到的是同一批分块条目
        let e1 = store.list_entries("kb1", None, None, false, None);
        let e2 = store.list_entries("kb2", None, None, false, None);
        assert_eq!(e1[0].key, e2[0].key);
        assert_eq!(e1[0].base_ids.len(), 2, "该条目现在属于两个库");
    }

    #[test]
    fn ingest_file_registers_unsupported_format_without_chunks() {
        let db_dir = tmp_dir();
        let root = std::path::PathBuf::from(tmp_dir());
        let src_dir = std::path::PathBuf::from(tmp_dir());
        let src = src_dir.join("shot.png");
        std::fs::write(
            &src,
            [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3],
        )
        .unwrap();

        let store = KnowledgeStore::open(&db_dir).unwrap();
        store.create_base("kb1", "库一", "", FIELDS).unwrap();
        let out = ingest_no_ai(&store, &root, "kb1", &src).unwrap();
        assert_eq!(out.status, "unsupported");
        assert_eq!(out.chunks, 0);
        // 原文仍然登记了：用户能在「文件」里看到它，只是内容不进检索
        assert_eq!(store.list_files(&root, "kb1").len(), 1);
        assert!(store
            .list_entries("kb1", None, None, false, None)
            .is_empty());
    }

    #[test]
    fn knowledge_entries_are_exempt_from_memory_prune_and_quota() {
        let dir = tmp_dir();
        let kb = KnowledgeStore::open(&dir).unwrap();
        kb.create_base("kb1", "库一", "", FIELDS).unwrap();
        kb.save_entry(
            "kb1",
            "知识条目",
            "不该被冷记忆清理删掉",
            "",
            "snippet",
            "",
            None,
        )
        .unwrap();

        // 伪装成一条极冷的条目：不 pin、访问 0、时间戳很旧
        {
            let conn = kb.conn.lock().unwrap();
            conn.execute(
                "UPDATE key_memories SET access_count = 0, last_accessed_at = 1, updated_at = 1",
                [],
            )
            .unwrap();
        }

        let mem = super::super::db::MemoryStore::new(&dir).unwrap();
        assert_eq!(mem.quota_count(), 0, "知识条目不占 600 条配额");
        assert_eq!(mem.count(), 1, "但总数里仍算它一条");
        assert!(
            mem.maintenance_candidates().is_empty(),
            "知识条目不应成为清理候选"
        );
        assert_eq!(mem.prune(), 0);
        assert_eq!(kb.list_entries("kb1", None, None, false, None).len(), 1);
    }

    /// 触发器必须能**按版本重建**：老库里留下的旧定义（用 FTS5 的 'delete' 特殊命令清索引）
    /// 会让"删知识条目"100% 失败，而 `CREATE TRIGGER IF NOT EXISTS` 永远修不好它。
    /// 这就是"删库只删掉库定义、条目全留成僵尸"这个 bug 的根因，必须有回归测试守着。
    #[test]
    fn stale_triggers_are_rebuilt_so_entry_delete_works() {
        let dir = tmp_dir();
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();
        store
            .save_entry("kb1", "条目", "内容", "", "snippet", "", None)
            .unwrap();

        // 伪造"老库"：把 kb_fts_ad 换成旧定义，并清掉版本号（模拟升级前的存量库）
        {
            let conn = store.conn.lock().unwrap();
            conn.execute_batch(
                "DROP TRIGGER kb_fts_ad;
                 CREATE TRIGGER kb_fts_ad AFTER DELETE ON key_memories
                 WHEN old.category = 'knowledge'
                 BEGIN
                     INSERT INTO kb_entry_fts(kb_entry_fts, key, value, tags)
                     VALUES ('delete', old.key, old.value, old.tags);
                 END;
                 DELETE FROM kb_meta WHERE key = 'trigger_version';",
            )
            .unwrap();
            assert!(
                conn.execute("DELETE FROM key_memories WHERE key = '条目'", [])
                    .is_err(),
                "旧触发器应当报错 —— 这正是用户遇到的现象"
            );
        }
        drop(store);

        // 再次打开（等价于下次启动）：触发器按版本重建，删条目恢复正常
        let store2 = KnowledgeStore::open(&dir).unwrap();
        {
            let conn = store2.conn.lock().unwrap();
            conn.execute("DELETE FROM key_memories WHERE key = '条目'", [])
                .expect("触发器重建后删条目应当成功");
        }
        assert!(store2
            .list_entries("kb1", None, None, false, None)
            .is_empty());
    }

    /// 自愈：失去全部归属的知识条目既在任何列表里看不见、又仍会被记忆检索命中；
    /// 程序把它们收进一个专门的库让它们重新可见，但**绝不删除**（正文可能已是唯一副本）。
    #[test]
    fn orphan_entries_are_healed_into_recovery_base_without_data_loss() {
        let dir = tmp_dir();
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();
        store
            .save_entry(
                "kb1",
                "孤儿条目",
                "这段正文只此一份",
                "",
                "snippet",
                "",
                None,
            )
            .unwrap();
        // 制造"失去归属"的残留状态（历史上删库失败就是这样留下的）
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "DELETE FROM kb_entry_links WHERE entry_key = '孤儿条目'",
                [],
            )
            .unwrap();
        }
        drop(store);

        let store2 = KnowledgeStore::open(&dir).unwrap();
        assert!(
            store2
                .list_bases()
                .iter()
                .any(|b| b.id == RECOVERED_BASE_ID),
            "应当建立收容库"
        );
        let healed = store2.list_entries(RECOVERED_BASE_ID, None, None, false, None);
        assert_eq!(healed.len(), 1);
        assert_eq!(healed[0].key, "孤儿条目");
        assert_eq!(
            healed[0].value, "这段正文只此一份",
            "内容必须原样保留，不能被删"
        );
    }

    /// 区间切片必须等于块文本：这是"AI 只给序号、正文原样"的地基
    #[test]
    fn block_ranges_slice_equals_block_text() {
        let text = "# 标题\n\n正文一。\n\n- 项一\n- 项二\n\n| a | b |\n| - | - |\n\n```rust\nfn f() {}\n```\n\n尾段。";
        let blocks = split_blocks_ranged(text);
        assert!(
            blocks.len() >= 5,
            "应切出标题+正文、两个列表项、表格、代码块、尾段"
        );
        for b in &blocks {
            assert_eq!(&text[b.start..b.end], b.text, "区间切片必须与块文本一致");
        }
        assert!(
            blocks[0].text.contains("# 标题") && blocks[0].text.contains("正文一"),
            "标题应粘到其后的块上"
        );
    }

    /// AI 分组：按块序号从原文区间取正文 —— 跨块时含中间空行，一字不改
    #[test]
    fn assemble_plans_keeps_original_text() {
        let prep = prepare_web("t", "第一段。\n\n第二段。\n\n第三段。").unwrap();
        assert_eq!(prep.blocks.len(), 3);
        let plans = vec![
            ChunkPlan {
                from: 1,
                to: 2,
                title: "前半".into(),
                tags: vec!["a".into()],
                meta: Default::default(),
            },
            ChunkPlan {
                from: 3,
                to: 3,
                title: String::new(),
                tags: vec![],
                meta: Default::default(),
            },
        ];
        let (chunks, truncated) = assemble_plans(&prep, &prep.blocks, &plans);
        assert!(!truncated);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks[0].text, "第一段。\n\n第二段。",
            "跨块应保留原始空行，不重新拼接"
        );
        assert_eq!(chunks[1].text, "第三段。");
        assert_eq!(chunks[0].title, "前半");
        assert_eq!(chunks[0].tags, "a");
        // 越界的计划被丢弃，不会 panic
        let bad = vec![ChunkPlan {
            from: 0,
            to: 9,
            title: String::new(),
            tags: vec![],
            meta: Default::default(),
        }];
        assert!(assemble_plans(&prep, &prep.blocks, &bad).0.is_empty());
    }

    /// 碎块先合并再喂模型：块数降下来，模型才给得出分组（否则整篇降级成规则分块）
    #[test]
    fn merge_small_blocks_glues_fragments_but_keeps_paragraphs() {
        // 大纲式的碎块：一次调用装不下，模型容易给不出可用分组
        let text =
            "1. 求告\n\n+ 主要人物：求告者\n\n+ 核心冲突：求告者与对立方对抗。\n\n".repeat(10);
        let blocks = split_blocks_ranged(&text);
        assert!(
            blocks.len() >= 20,
            "前提：这份文本本来就碎，实际 {}",
            blocks.len()
        );
        let merged = merge_small_blocks(&text, &blocks, MIN_MODEL_BLOCK);
        assert!(
            merged.len() < blocks.len(),
            "碎块应被合并：{} → {}",
            blocks.len(),
            merged.len()
        );
        // 合并只改边界、不改内容：首尾必须覆盖原文，每块都是原文切片
        // （`assemble_plans` 按合并后的序号从原文取字，错一处就会串块）
        assert_eq!(merged.first().unwrap().start, blocks.first().unwrap().start);
        assert_eq!(merged.last().unwrap().end, blocks.last().unwrap().end);
        for m in &merged {
            assert_eq!(m.text, text[m.start..m.end], "合并块的 text 必须是原文切片");
        }
        // 除尾块外都凑到了下限 —— 这就是"别再拿碎片去喂模型"的实现方式
        for m in &merged[..merged.len() - 1] {
            assert!(
                m.text.chars().count() >= MIN_MODEL_BLOCK,
                "合并结果没凑够下限: {} 字",
                m.text.chars().count()
            );
        }

        // 够长的段落不动：合并过头会把模型的分组自由度压掉
        let para = "这是一段已经足够长的正文，超过了下限就不该与相邻块合并。".repeat(5);
        let doc = format!("{}\n\n短尾巴。", para);
        let b = split_blocks_ranged(&doc);
        let m = merge_small_blocks(&doc, &b, MIN_MODEL_BLOCK);
        assert_eq!(m.len(), 2, "长段落自成一块，短尾块不该把前一块拉进来");
        assert_eq!(m[0].text, para);
    }

    /// 切正文用的是**喂模型那一份**块清单，序号必须与它一致（错位 = 正文串块）
    #[test]
    fn assemble_plans_uses_the_same_block_list_it_was_planned_against() {
        let prep = prepare_web("t", &"碎。\n\n".repeat(40)).unwrap();
        let blocks = merge_small_blocks(&prep.text, &prep.blocks, MIN_MODEL_BLOCK);
        assert!(blocks.len() < prep.blocks.len());
        let plans = vec![ChunkPlan {
            from: 1,
            to: 1,
            title: "第一组".into(),
            tags: vec![],
            meta: Default::default(),
        }];
        let (chunks, _) = assemble_plans(&prep, &blocks, &plans);
        assert_eq!(chunks.len(), 1);
        // 第一组 = 合并后的第一块，而不是"原来的第一块"
        assert_eq!(chunks[0].text, prep.text[blocks[0].start..blocks[0].end]);
        assert!(chunks[0].text.chars().count() > 20);
    }

    /// 同标题的两条不能撞 key：撞了会被 UPSERT 覆盖，等于丢内容
    #[test]
    fn planned_chunks_with_same_title_get_distinct_keys() {
        let dir = tmp_dir();
        let root = std::path::PathBuf::from(&dir);
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();
        let prep = prepare_web("doc", "甲。\n\n乙。").unwrap();
        let plans = vec![
            ChunkPlan {
                from: 1,
                to: 1,
                title: "同一标题".into(),
                tags: vec![],
                meta: Default::default(),
            },
            ChunkPlan {
                from: 2,
                to: 2,
                title: "同一标题".into(),
                tags: vec![],
                meta: Default::default(),
            },
        ];
        let (chunks, truncated) = assemble_plans(&prep, &prep.blocks, &plans);
        assert!(!truncated);
        let out = store
            .ingest_prepared(&root, "kb1", prep, "", "file", Some(chunks), false, "")
            .unwrap();
        assert_eq!(out.chunks, 2);
        assert!(out.ai_note.is_empty(), "走了 AI 计划就不该有降级说明");
        let entries = store.list_entries("kb1", None, None, false, None);
        assert_eq!(entries.len(), 2, "两条都必须在（同标题不能互相覆盖）");
        assert!(
            entries.iter().all(|e| e.key.starts_with("同一标题")),
            "key 应用 AI 标题"
        );
        let values: Vec<String> = entries.iter().map(|e| e.value.clone()).collect();
        assert!(values.contains(&"甲。".to_string()) && values.contains(&"乙。".to_string()));
    }

    /// 从库中移除文件：库定义必须原样保留；只属于本库的文件与条目被清掉，共享的保留。
    /// 这正是用户踩过的坑：想删文件却只好去点「删除知识库」→ 库定义也没了。
    #[test]
    fn remove_files_never_touches_base_definition() {
        let dir = tmp_dir();
        let (store, root, _main, att1, att2) = setup_doc_with_attachments(&dir);
        assert_eq!(store.list_files(&root, "kb1").len(), 3);

        let out = store
            .remove_files("kb1", &[att1.clone(), att2.clone()], &root)
            .unwrap();
        assert_eq!(
            out.files, 1,
            "只属于 kb1 的附件应连原文一起删；附件二还被 kb2 引用"
        );
        assert!(out.entries > 0, "该附件在库里的条目应一并清掉");

        // 库定义必须还在 —— 这是本用例的核心断言
        let bases = store.list_bases();
        assert!(bases.iter().any(|b| b.id == "kb1"), "库定义不能被删");
        assert!(bases.iter().any(|b| b.id == "kb2"));

        let left = store.list_files(&root, "kb1");
        assert_eq!(left.len(), 1, "只剩正文");
        assert!(!left.iter().any(|f| f.id == att1 || f.id == att2));
        assert_eq!(
            store.list_files(&root, "kb2").len(),
            1,
            "共享的附件二留在 kb2"
        );
        assert!(store
            .list_bases()
            .iter()
            .any(|b| b.id == "kb1" && b.file_count == 1));
    }

    /// 标题兜底：不能把半句话/半个词当标题（这正是用户反馈的"硬截断标题"）
    #[test]
    fn derive_title_prefers_headings_and_stops_at_punctuation() {
        assert_eq!(
            derive_title("# 第三章 审批流程\n\n正文…"),
            "第三章 审批流程"
        );
        assert_eq!(derive_title("   \n\n短标题"), "短标题");
        assert!(derive_title("").is_empty());

        // 超长首行：在 30 字内就近的标点处断开，且**不留悬空标点**
        let long = "这是一句很长的说明，后面还有很多内容要继续写下去才能超过三十个字符的限制。";
        let t = derive_title(long);
        assert_eq!(t, "这是一句很长的说明", "逗号断出来之后要把逗号收掉");
        assert!(t.chars().count() <= 30, "不该超过 30 字: {}", t);

        // 优先句末标点：逗号在前也先认句号（断在逗号上仍是半句）
        let t =
            derive_title("先说一个前提，然后是这一条的核心结论在这里画上句号了。后面还有别的。");
        assert!(t.ends_with('。'), "应优先在句末标点处断开: {}", t);

        // 首行短但收在逗号上：这是"半句话标题"的典型形态，收尾标点要去掉
        assert_eq!(
            derive_title("核心冲突：亲属间为权力、爱情等展开争夺，\n如《雷雨》所示。"),
            "核心冲突：亲属间为权力、爱情等展开争夺"
        );

        // 完全没有标点时才硬截断，且末尾标明被截断
        let t = derive_title(&"甲".repeat(50));
        assert!(t.ends_with('…'));
        assert_eq!(t.chars().count(), 31);
    }

    /// 「按文件读」的两段式：先按名字找到原文，再从第 n 块起翻页读。
    /// 这是 `read_kb_file` 工具的底层（工具本身不做 SQL）。
    #[test]
    fn sources_are_found_by_name_and_chunks_read_in_pages() {
        let dir = tmp_dir();
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "制度库", "", "[]").unwrap();
        {
            let conn = store.conn.lock().unwrap();
            for i in 1..=5u32 {
                let key = format!("第{}条-1a2b3c4d#{}", i, i);
                conn.execute(
                    "INSERT INTO key_memories(key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
                     VALUES(?1, ?2, 'knowledge', 1, 1, 0, 0, 0, '')",
                    params![key, format!("第 {} 条的内容", i)],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO kb_entry_links(kb_id, entry_key, source_ref, chunk_index, chunk_total, added_at)
                     VALUES('kb1', ?1, 'files/1a2b3c4d-管理办法.md', ?2, 5, 1)",
                    params![key, i as i64],
                )
                .unwrap();
            }
        }

        // 按名字片段定位：给人看的名字必须**不含** sha 前缀
        let hits = store.find_sources("kb1", "管理办法", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "管理办法.md");
        assert_eq!(hits[0].chunk_count, 5);
        // 无关键词 = 目录模式；找不到时是空列表而不是报错
        assert_eq!(store.find_sources("kb1", "", 10).len(), 1);
        assert!(store.find_sources("kb1", "不存在的名字", 10).is_empty());

        // 翻页：第 3 块起读 2 块，并回传总块数（模型据此决定还要不要续读）
        let (page, total) = store.read_file_chunks("kb1", &hits[0].source_ref, 3, 2);
        assert_eq!(total, 5);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].chunk_index, Some(3));
        assert_eq!(page[1].chunk_index, Some(4));
        // 越界的 from：空页 + 仍然给出总块数（不报错，也不偷偷从头开始）
        let (empty, total) = store.read_file_chunks("kb1", &hits[0].source_ref, 99, 2);
        assert!(empty.is_empty());
        assert_eq!(total, 5);
        // 不存在的原文：空页 + 0 块
        assert_eq!(store.read_file_chunks("kb1", "files/nope.md", 1, 2).1, 0);
    }

    /// 文件属性是**分块聚合**出来的（`kb_files` 上没有属性列）：
    /// 收集该文件分块在该字段上的**全部去重取值** —— 0 个（没填，文件树里留在根）、
    /// 1 个（一个目录）、N 个（同时挂 N 个目录 = 虚拟重复）。
    #[test]
    fn file_attrs_collect_all_distinct_chunk_values() {
        let dir = tmp_dir();
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "制度库", "", "[]").unwrap();
        {
            let conn = store.conn.lock().unwrap();
            let add = |key: &str, src: &str, idx: u32, meta: &str| {
                conn.execute(
                    "INSERT INTO key_memories(key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
                     VALUES(?1, '正文', 'knowledge', 1, 1, 0, 0, 0, '')",
                    params![key],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO kb_entry_links(kb_id, entry_key, source_ref, chunk_index, chunk_total, added_at, kb_meta)
                     VALUES('kb1', ?1, ?2, ?3, 3, 1, ?4)",
                    params![key, src, idx as i64, meta],
                )
                .unwrap();
            };
            // 两块同部门 → 一个取值（"没填"的年份不参与）
            add(
                "A-1a2b3c4d#1",
                "files/1a2b3c4d-制度A.md",
                1,
                r#"{"部门":"财务部","年份":2024}"#,
            );
            add(
                "A-1a2b3c4d#2",
                "files/1a2b3c4d-制度A.md",
                2,
                r#"{"部门":"财务部"}"#,
            );
            // 两块不同部门 → 两个取值（会在两个目录各出现一次）
            add(
                "B-5f6e7d8c#1",
                "files/5f6e7d8c-制度B.md",
                1,
                r#"{"部门":"财务部"}"#,
            );
            add(
                "B-5f6e7d8c#2",
                "files/5f6e7d8c-制度B.md",
                2,
                r#"{"部门":"人事部"}"#,
            );
            // 全空 / 只有一个空串 → 一个取值都没有（留在根目录）
            add("C-9a8b7c6d#1", "files/9a8b7c6d-无属性.md", 1, "{}");
            add(
                "C-9a8b7c6d#2",
                "files/9a8b7c6d-无属性.md",
                2,
                r#"{"部门":"   "}"#,
            );
        }

        let map: HashMap<String, KnowledgeFileAttrView> = store
            .file_attr_map("kb1")
            .into_iter()
            .map(|v| (v.source_ref.clone(), v))
            .collect();
        assert_eq!(map.len(), 3, "一个 source_ref 一行");

        let a = &map["files/1a2b3c4d-制度A.md"];
        assert_eq!(a.chunk_count, 2);
        assert_eq!(
            a.values.get("部门").map(Vec::as_slice),
            Some(&["财务部".to_string()][..])
        );
        // 只有一块填了年份也算取值（空/缺失不参与收集，否则"只有一块填过"会变成多值）
        assert_eq!(
            a.values.get("年份").map(Vec::as_slice),
            Some(&["2024".to_string()][..])
        );

        // 多值：两个取值都给出来（前端据此把同一份文件挂到两个目录）
        let b = &map["files/5f6e7d8c-制度B.md"];
        assert_eq!(
            b.values.get("部门").map(Vec::as_slice),
            Some(&["人事部".to_string(), "财务部".to_string()][..]),
            "取值排序输出，渲染目录顺序才稳定"
        );

        // 完全没填：这个字段根本没有 key（= 没有子目录 → 留在根）
        let c = &map["files/9a8b7c6d-无属性.md"];
        assert!(c.values.get("部门").is_none());
        assert!(c.values.is_empty());

        // 只统计本库
        store.create_base("kb2", "库二", "", "[]").unwrap();
        assert!(store.file_attr_map("kb2").is_empty());
    }

    /// 「按原文取分块」是图谱展开文件的数据入口。它绕开了整库 500 条的闸门（那正是它的存在理由），
    /// 但和 `list_entries` 共用 `entry_view_from_row` 的 14 列读取 —— 这里逐列断言，
    /// 守住"两个 SELECT 的列顺序漂移"这类只在运行时才会显形、且表现为数据错位的 bug。
    #[test]
    fn file_chunks_query_returns_only_that_file_in_chunk_order() {
        let dir = tmp_dir();
        let root = std::path::PathBuf::from(&dir);
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();

        let body: String = (1..=200)
            .map(|i| {
                format!(
                    "第 {} 条 这是用于测试的条文内容，要足够长才能被切成多个块。\n\n",
                    i
                )
            })
            .collect();
        let src = root.join("管理办法.md");
        std::fs::write(&src, &body).unwrap();
        let main = ingest_no_ai(&store, &root, "kb1", &src).unwrap();

        let other_src = root.join("另一份.md");
        std::fs::write(&other_src, "另一份文件的内容。".repeat(200)).unwrap();
        let other = ingest_no_ai(&store, &root, "kb1", &other_src).unwrap();

        assert!(
            main.chunks >= 3,
            "这份原文应被切成多块，实际 {}",
            main.chunks
        );

        let rows = store.list_file_chunks("kb1", &main.rel_path, None);
        assert_eq!(rows.len(), main.chunks, "应取到该原文的全部分块");
        for (i, r) in rows.iter().enumerate() {
            // source_ref / origin 没读串列
            assert_eq!(r.source_ref, main.rel_path);
            assert_eq!(r.origin, "file");
            // chunk_index / chunk_total 没读串列，且按序号升序
            assert_eq!(r.chunk_index, Some(i as u32 + 1));
            assert_eq!(r.chunk_total, Some(main.chunks as u32));
            assert_eq!(r.category, CATEGORY_KNOWLEDGE);
            assert!(r.base_ids.iter().any(|b| b == "kb1"));
            // key 与 value 没互换 / 没取空
            assert!(!r.key.is_empty());
            assert!(r.value.starts_with('第'));
        }
        // 只取本原文：别的文件的块不能混进来
        assert!(rows.iter().all(|r| !r.value.contains("另一份文件")));
        assert_eq!(
            store.list_file_chunks("kb1", &other.rel_path, None).len(),
            other.chunks,
        );

        // `Some(n)` 仍生效，并被夹到 [1, FILE_CHUNK_LIMIT]；`None` = 全部
        assert_eq!(
            store.list_file_chunks("kb1", &main.rel_path, Some(2)).len(),
            2
        );
        assert_eq!(
            store.list_file_chunks("kb1", &main.rel_path, Some(0)).len(),
            1
        );
        assert_eq!(
            store.list_file_chunks("kb1", &main.rel_path, None).len(),
            main.chunks
        );
        // 未知 / 空 source_ref：返回空而不是报错或返回全库
        assert!(store
            .list_file_chunks("kb1", "files/不存在的.md", None)
            .is_empty());
        assert!(store.list_file_chunks("kb1", "", None).is_empty());
    }

    /// 网页/HTML 的"提取正文 → 分块"是一条链：**转换出来的 markdown 必须能被分块器切成多块**。
    ///
    /// 修复前 `html_to_text` 会把所有空行丢掉，于是整页正文被当成**一个段落**（`blocks.len() == 1`）——
    /// 后果不是"分块难看"，而是 AI 整理的按字符分批永远只有一批：页面一大就超上下文/超输出上限，
    /// 必然降级回原始噪声文本。这条断言就是那个 bug 的回归测试。
    #[test]
    fn html_markdown_output_splits_into_multiple_blocks() {
        let html = "<h2>第一章 总则</h2><p>第一条 为规范管理，制定本办法。</p>\
                    <p>第二条 本办法适用于全体员工。</p><ul><li>甲项</li><li>乙项</li></ul>";
        let md = crate::tools::browser::html_to_markdown(html);
        assert!(md.contains("# 第一章 总则"), "标题要保下来：{md}");
        let blocks = split_blocks_ranged(&md);
        let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
        assert!(
            blocks.len() >= 3,
            "应切出多个块（修复前是 1 块）：{} 块 {texts:?}",
            blocks.len()
        );
        // 区间切片必须等于块文本（分块契约：每组区间 = 一段连续原文）
        for b in &blocks {
            assert_eq!(&md[b.start..b.end], b.text);
        }
    }

    /// 方案 C 的产物契约：落盘的那份 .md 是**能再次分块的文档**，且每条知识都是它的一段连续文本。
    /// 断言这个是因为"文件"和"条目"一旦变成两套内容，用户点「打开原文」就会看到与库里不一致的东西。
    #[test]
    fn composed_page_doc_keeps_every_section_as_contiguous_text() {
        use crate::api_agent::kb_llm::PageSection;
        let sections = vec![
            PageSection {
                title: "适用范围".into(),
                content: "本制度适用于全体员工。".into(),
                tags: vec![],
                meta: Default::default(),
            },
            PageSection {
                title: "生效日期".into(),
                content: "自 2026 年 1 月 1 日起施行。".into(),
                tags: vec![],
                meta: Default::default(),
            },
            PageSection {
                title: "解释部门".into(),
                content: "由综合管理部负责解释。".into(),
                tags: vec![],
                meta: Default::default(),
            },
        ];
        let doc = crate::api_agent::kb_llm::compose_page_doc("政府采购管理办法", &sections);
        for s in &sections {
            assert!(
                doc.contains(s.content.as_str()),
                "文件里找不到这条正文：{}",
                s.content
            );
        }
        let blocks = split_blocks_ranged(&doc);
        assert!(
            blocks.len() >= sections.len(),
            "落盘文档应能再次切成不少于条目数的块：{} 块 vs {} 条",
            blocks.len(),
            sections.len()
        );
        for b in &blocks {
            assert_eq!(&doc[b.start..b.end], b.text);
        }
    }

    /// 测试用的"不走 AI"投喂：本地准备 + 规则分块落库（与 AI 路径共用落库阶段）
    fn ingest_no_ai(
        store: &KnowledgeStore,
        root: &std::path::Path,
        kb_id: &str,
        src: &std::path::Path,
    ) -> Result<IngestOutcome, String> {
        let prep = prepare_file(src, false)?;
        store.ingest_prepared(root, kb_id, prep, "", "file", None, false, "")
    }

    /// 「存为 markdown」是**另一种产物**：来源身份必须区分开，否则第二次用另一种形态投喂
    /// 会命中 sha 去重、用户的加工意图被静默忽略。
    /// 而原样形态的 sha 必须与改动前**逐位相同** —— 存量库按它去重，一旦变了，
    /// 所有老文件都会被当成"没见过的新文件"再存一遍。
    #[test]
    fn as_markdown_gets_a_distinct_source_identity() {
        let dir = tmp_dir();
        let src = std::path::PathBuf::from(&dir).join("制度.html");
        std::fs::write(&src, "<p>正文</p>").unwrap();
        let plain = prepare_file(&src, false).unwrap();
        let organized = prepare_file(&src, true).unwrap();
        assert_ne!(plain.sha, organized.sha, "两种形态必须是两个来源身份");
        assert_eq!(
            plain.sha,
            sha256_hex(b"<p>\xE6\xAD\xA3\xE6\x96\x87</p>"),
            "原样形态的 sha 不许变"
        );
        // 同一份源文件的两种形态能各自入库（不同 sha → 两行），而不是互相顶掉
        let root = std::path::PathBuf::from(&dir);
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();
        store
            .ingest_prepared(&root, "kb1", plain, "", "file", None, false, "")
            .unwrap();
        store
            .ingest_prepared(&root, "kb1", organized, "", "file", None, false, "")
            .unwrap();
        let files = store.list_files(&root, "kb1");
        assert_eq!(
            files.len(),
            2,
            "两种形态应各自成行：{:?}",
            files.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    /// sha 命中且这次带了新名字 = **重命名已有文件**，而不是"投喂成功、名字没变"（静默失败）。
    /// 磁盘名、`kb_files` 记录、分块条目的 `source_ref` 三处都要跟着走。
    #[test]
    fn reingest_with_a_new_name_renames_the_stored_file() {
        let dir = tmp_dir();
        let src = std::path::PathBuf::from(&dir).join("s(1).htm");
        std::fs::write(&src, "<p>第一条正文。</p><p>第二条正文。</p>").unwrap();
        let root = std::path::PathBuf::from(&dir);
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();
        let out = ingest_no_ai(&store, &root, "kb1", &src).unwrap();
        assert_eq!(store.list_files(&root, "kb1")[0].name, "s(1).htm");

        // 重新投喂同一份内容，这次带上更正后的名字
        let mut prep = prepare_file(&src, false).unwrap();
        prep.name = "2024年度采购管理办法.htm".into();
        store
            .ingest_prepared(&root, "kb1", prep, "", "file", None, false, "")
            .unwrap();

        let files = store.list_files(&root, "kb1");
        assert_eq!(files.len(), 1, "同内容不该多出一个文件行");
        assert_eq!(files[0].name, "2024年度采购管理办法.htm", "名字应被更正");
        // 分块条目的 source_ref 要跟着新路径，否则它们会变成指向不存在文件的孤儿
        let chunks = store.list_file_chunks("kb1", &files[0].rel_path, None);
        assert!(
            !chunks.is_empty(),
            "改名后原文件的正文与分块都要在：{:?}",
            out.file_id
        );
    }

    /// 投喂三个文件（正文 / 两个附件），返回 (store, root, 三个 file_id)
    fn setup_doc_with_attachments(
        dir: &str,
    ) -> (KnowledgeStore, std::path::PathBuf, String, String, String) {
        let root = std::path::PathBuf::from(dir);
        let store = KnowledgeStore::open(dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();
        store.create_base("kb2", "库二", "", "[]").unwrap();
        let ingest = |name: &str, body: &str| {
            let src = root.join(name);
            std::fs::write(&src, body).unwrap();
            ingest_no_ai(&store, &root, "kb1", &src).unwrap().file_id
        };
        let main = ingest("管理办法.md", "管理办法正文。".repeat(80).as_str());
        let att1 = ingest("附件一.md", "附件一的内容。".repeat(80).as_str());
        let att2 = ingest("附件二.md", "附件二的内容。".repeat(80).as_str());
        // 附件二同时被库二引用（同 sha → 复用同一份原文）
        let src = root.join("附件二.md");
        ingest_no_ai(&store, &root, "kb2", &src).unwrap();
        (store, root, main, att1, att2)
    }

    #[test]
    fn file_relations_reject_self_occupied_and_cycle() {
        let dir = tmp_dir();
        let (store, _root, main, att1, att2) = setup_doc_with_attachments(&dir);

        store.link_files(REL_ATTACHMENT, &main, &att1).unwrap();
        // 幂等：重复建立同一条关系不报错
        store.link_files(REL_ATTACHMENT, &main, &att1).unwrap();
        // 自指 / 未知类型 / 不存在的文件
        assert!(store.link_files(REL_ATTACHMENT, &main, &main).is_err());
        assert!(store.link_files("unknown", &main, &att1).is_err());
        assert!(store
            .link_files(REL_ATTACHMENT, &main, "no-such-id")
            .is_err());
        // to 端唯一：附件一已经有正文了，别的文件不能再来认领
        let err = store.link_files(REL_ATTACHMENT, &att2, &att1).unwrap_err();
        assert!(err.contains("附件"), "报错要说清被谁占用：{}", err);
        // 成环：附件一不能再把正文收成自己的附件
        assert!(store.link_files(REL_ATTACHMENT, &att1, &main).is_err());
        // 解除后可以改挂
        assert!(store.unlink_files(REL_ATTACHMENT, &main, &att1).unwrap());
        assert!(!store.unlink_files(REL_ATTACHMENT, &main, &att1).unwrap());
        store.link_files(REL_ATTACHMENT, &att2, &att1).unwrap();

        // 版本关系同理，且与附件关系互不干扰
        store.link_files(REL_SUPERSEDES, &main, &att2).unwrap();
        assert_eq!(store.list_file_relations("kb1").len(), 2);
    }

    /// 批量解除：只属于本库的连本体一起删，共享的只解除关联；不存在的 key 不计入。
    #[test]
    fn batch_unlink_entries_removes_orphans_only() {
        let dir = tmp_dir();
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", FIELDS).unwrap();
        store.create_base("kb2", "库二", "", "[]").unwrap();
        store
            .save_entry("kb1", "独有一", "只属于库一", "", "snippet", "", None)
            .unwrap();
        store
            .save_entry("kb1", "独有二", "只属于库一", "", "snippet", "", None)
            .unwrap();
        store
            .save_entry("kb1", "共享", "两个库都有", "", "snippet", "", None)
            .unwrap();
        store
            .save_entry("kb2", "共享", "两个库都有", "", "snippet", "", None)
            .unwrap();

        let out = store
            .unlink_entries(
                "kb1",
                &[
                    "独有一".into(),
                    "独有二".into(),
                    "共享".into(),
                    "不存在".into(),
                ],
            )
            .unwrap();
        assert_eq!(out.unlinked, 3, "不存在的 key 不该计入");
        assert_eq!(out.deleted, 2, "只有不再被任何库关联的两条删了本体");

        assert!(store
            .list_entries("kb1", None, None, false, None)
            .is_empty());
        assert_eq!(store.list_entries("kb2", None, None, false, None).len(), 1);

        // 空列表是 no-op（前端可能在全选后清空选择再点）
        let empty = store.unlink_entries("kb1", &[]).unwrap();
        assert_eq!((empty.unlinked, empty.deleted), (0, 0));
    }

    /// 契约：原文被删 ⇒ 由它切出来的分块条目一并消失。
    /// 分块条目与文件之间只有 `source_ref ↔ rel_path` 的约定、没有外键，
    /// 所以这条必须显式保证 —— 否则会留下「有条目、没原文」的悬空数据
    /// （以后新增"单独删文件"入口时也依赖这个出口，不必再补一次）。
    #[test]
    fn deleting_file_also_removes_its_chunk_entries() {
        let dir = tmp_dir();
        let root = std::path::PathBuf::from(&dir);
        let store = KnowledgeStore::open(&dir).unwrap();
        store.create_base("kb1", "库一", "", "[]").unwrap();

        let src = root.join("制度.md");
        std::fs::write(&src, "制度条文内容。".repeat(120)).unwrap();
        let out = ingest_no_ai(&store, &root, "kb1", &src).unwrap();
        assert!(out.chunks >= 1);
        assert_eq!(
            store.list_entries("kb1", None, None, false, None).len(),
            out.chunks
        );

        store.delete_base("kb1", &root).unwrap();
        assert!(store.list_files(&root, "kb1").is_empty());

        let conn = store.conn.lock().unwrap();
        let entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM key_memories", [], |r| r.get(0))
            .unwrap();
        let links: i64 = conn
            .query_row("SELECT COUNT(*) FROM kb_entry_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entries, 0, "原文没了，分块条目本体不该留");
        assert_eq!(links, 0, "也不该留悬空的关联行");
    }

    /// 删正文时的孤儿口径：**库引用与文件引用都为空**才删；仍被别的库引用的附件要留。
    #[test]
    fn deleting_base_cascades_attachments_only_when_truly_orphan() {
        let dir = tmp_dir();
        let (store, root, main, att1, att2) = setup_doc_with_attachments(&dir);
        store.link_files(REL_ATTACHMENT, &main, &att1).unwrap();
        store.link_files(REL_ATTACHMENT, &main, &att2).unwrap();

        let paths = |id: &str| {
            let f = store
                .list_files(&root, "kb1")
                .into_iter()
                .find(|f| f.id == id);
            f.map(|f| root.join(&f.rel_path)).unwrap_or_default()
        };
        let (p_main, p_att1, p_att2) = (paths(&main), paths(&att1), paths(&att2));
        assert!(p_main.exists() && p_att1.exists() && p_att2.exists());

        // 顺序上先处理附件（栈序）：附件一这时还被正文引用着 → 必须保留；
        // 等正文被删、关系解除后，它才该被删 —— 这一条专门挡住"访问过就跳过"的漏删
        let out = store.delete_base("kb1", &root).unwrap();
        assert_eq!(
            out.files, 2,
            "正文 + 只属于它的附件一被删；附件二还在库二里"
        );

        assert!(!p_main.exists(), "正文登记行与磁盘原文都应删除");
        assert!(!p_att1.exists(), "只属于正文的附件应连带删除");
        assert!(p_att2.exists(), "附件二还被库二引用，必须保留原文");
        assert_eq!(store.list_files(&root, "kb2").len(), 1, "附件二仍属于库二");
        assert!(
            store.list_file_relations("kb2").is_empty(),
            "正文已删，关系随之一并解除，不留悬空引用"
        );
    }
}
