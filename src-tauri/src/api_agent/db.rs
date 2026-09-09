//! 独立数据库 — MEMORY.db
//!
//! API Agent 专属的 KV 记忆库（SQLite），与主项目 pilotdesk.db 分离。
//! 包含 key_memories 表，替代原有的 JSON 文件持久化方案。
//!
//! 设计原则：
//!   - 线程安全（Arc<Mutex<Connection>>）
//!   - 自动建表（首次打开时创建）
//!   - 不替代 CLI Agent 内部记忆

/// 记忆库文件名（位于 PilotDesk 配置目录下）。
const MEMORY_DB_FILE: &str = "MEMORY.db";

/// KV 记忆配额上限：总条数超过该值即驱逐“最冷且未 pin”的多余条目。
pub const MEMORY_MAX_ENTRIES: usize = 600;
/// 冷记忆（僵尸）判定冷却期（秒）：超过该时长未被访问的未 pin 条目可清理。
pub const MEMORY_IDLE_SECS: u64 = 180 * 24 * 3600;
/// KV 记忆访问次数上限：访问次数 ≤ 该值且冷却期超限的未 pin 条目才允许自动清理。
pub const MEMORY_MIN_ACCESS: u64 = 1;

/// 检索/注入排序的时间衰减半衰期（天）：超过该时长未访问，热度折半。
/// 取值更温和（30 天）：对话常用记忆的访问间隔通常以周计，7 天衰减过激。
const RANK_HALF_LIFE_DAYS: f64 = 30.0;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// 单条记忆记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
    pub category: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub access_count: u64,
    /// 最近一次被检索/读取的真实时间（内容编辑不刷新它）
    pub last_accessed_at: u64,
    /// 重要标记：置位后永不参与自动淘汰
    pub pin: bool,
    /// 检索标签（逗号分隔，如 "js,构建,脚本"）；补充 key 词面与用户话术之间的检索面
    pub tags: String,
}

/// SQLite 版 KV 记忆库
///
/// 数据库路径：{config_dir}/MEMORY.db
/// 表结构：key_memories (key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
pub struct MemoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl MemoryStore {
    /// 打开或创建记忆库
    ///
    /// `db_dir` 为 PilotDesk 配置目录的路径（通常为 %APPDATA%/PilotDesk）。
    /// 首次打开时自动建表。
    pub fn new(db_dir: &str) -> Result<Self, String> {
        let db_path = format!("{}/{}", db_dir.trim_end_matches(['/', '\\']), MEMORY_DB_FILE);

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("无法打开记忆库 {}: {}", db_path, e))?;

        // 启用 WAL 模式，提升并发读取性能
        conn.execute_batch("PRAGMA journal_mode=WAL;").ok();

        // 建表（新库含维护列；旧库随后按列缺失逐个 ALTER 迁移）
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS key_memories (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT 'fact',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 1,
                last_accessed_at INTEGER NOT NULL DEFAULT 0,
                pin INTEGER NOT NULL DEFAULT 0,
                tags TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_memories_category ON key_memories(category);
            CREATE INDEX IF NOT EXISTS idx_memories_access ON key_memories(access_count DESC);",
        )
        .map_err(|e| format!("创建记忆表失败: {}", e))?;

        // 存量库迁移：缺列时补列；last_accessed_at 回填为当时的 updated_at
        Self::ensure_column(&conn, "last_accessed_at", "INTEGER NOT NULL DEFAULT 0")?;
        Self::ensure_column(&conn, "pin", "INTEGER NOT NULL DEFAULT 0")?;
        Self::ensure_column(&conn, "tags", "TEXT NOT NULL DEFAULT ''")?;
        conn.execute(
            "UPDATE key_memories SET last_accessed_at = updated_at WHERE last_accessed_at = 0",
            [],
        )
        .ok();

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        // 打开即做一次懒惰维护（僵尸清理 + 配额驱逐），保持库自愈
        let cleaned = store.prune();
        if cleaned > 0 {
            log::info!("[Agent DB] 记忆库自动维护: 清理 {} 条冷记忆", cleaned);
        }

        log::info!(
            "[Agent DB] 记忆库已就绪: {}, 当前记忆数: {}",
            db_path,
            store.count()
        );
        Ok(store)
    }

    /// 表列名集合（用于判断缺失列）。
    fn table_columns(conn: &Connection) -> Vec<String> {
        let mut stmt = match conn.prepare("PRAGMA table_info(key_memories)") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let cols = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        cols
    }

    /// 缺列则补列（SQLite 支持带默认值的 ADD COLUMN）。
    fn ensure_column(conn: &Connection, column: &str, ddl: &str) -> Result<(), String> {
        if Self::table_columns(conn).iter().any(|c| c == column) {
            return Ok(());
        }
        conn.execute(
            &format!("ALTER TABLE key_memories ADD COLUMN {} {}", column, ddl),
            [],
        )
        .map_err(|e| format!("迁移记忆表(加列 {})失败: {}", column, e))?;
        Ok(())
    }

    /// 保存或更新一条记忆（pin=true 标记为重要：永不参与自动淘汰；tags 为逗号分隔检索标签）。
    /// 新插入计 1 次访问；覆盖更新仅刷新内容与访问时间，不累加热度
    /// （热度只应来自 search_memory 的真实检索命中，避免“反复覆盖=高频”失真）。
    pub fn save_memory(
        &self,
        key: &str,
        value: &str,
        category: &str,
        pin: bool,
        tags: &str,
    ) -> MemoryEntry {
        // 归一化标签：兼容中英文逗号/顿号/分号/竖线分隔，统一为英文逗号
        let tags = normalize_tags(tags);
        let now = now_secs();
        let entry = {
            let conn = self.conn.lock().unwrap();
            // 尝试更新已存在的 key（覆盖内容、刷新访问时间，但计数不变）
            let affected = conn
                .execute(
                    "UPDATE key_memories
                     SET value = ?1, category = ?2, updated_at = ?3, last_accessed_at = ?3,
                         pin = ?4, tags = ?5
                     WHERE key = ?6",
                    params![value, category, now, pin as i64, tags, key],
                )
                .unwrap_or(0);

            if affected == 0 {
                // 插入新记录
                conn.execute(
                    "INSERT INTO key_memories
                         (key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
                     VALUES (?1, ?2, ?3, ?4, ?4, 1, ?4, ?5, ?6)",
                    params![key, value, category, now, pin as i64, tags],
                )
                .unwrap();
            }

            conn.query_row(
                "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
                 FROM key_memories WHERE key = ?1",
                params![key],
                row_to_entry,
            )
            .unwrap()
        };

        // 写入后懒惰维护：超配额即驱逐（僵尸清理在 open 与维护命令处执行）
        if self.count() > MEMORY_MAX_ENTRIES {
            self.prune();
        }
        entry
    }

    /// 检索记忆（工具/查询路径）：先只读检索排序，再对命中返回子集刷新访问计数。
    ///
    /// 语义与排序同 `select_memories`；计数只在此路径累加，意图注入走 `select_memories`。
    pub fn search_memory(
        &self,
        query: Option<&str>,
        categories: Option<&[String]>,
        limit: Option<usize>,
    ) -> Vec<MemoryEntry> {
        let entries = self.select_memories(query, categories, limit);
        self.touch_memories(&entries);
        entries
    }

    /// 只读检索记忆（不刷新访问计数）：供意图注入等系统侧路径使用，避免“注入→热度上升→继续注入”。
    ///
    /// 语义：
    /// - category 分域：categories 数组任一匹配即进入候选；
    /// - 匹配面 = key 与 tags（LIKE 子串），不使用 value 做匹配（value 仅随命中返回）；
    /// - query 按空白/中英文逗号/顿号/分号/竖线拆词，词间 OR（任一命中即可）；
    /// - query 与 categories 至少提供一个。
    ///
    /// 排序：pin 恒置顶（两级：先 pin 组、组内按分），组内分数 =
    /// 热度（ln(visits+1)·半衰期衰减，半衰期 30 天）+ 命中加权
    /// （key 域 +0.8 / tags 域 +1.2，首词外每个额外命中词 +0.6）；同分按最近访问新者优先。
    /// limit 为显式截断上限（可选）。
    pub fn select_memories(
        &self,
        query: Option<&str>,
        categories: Option<&[String]>,
        limit: Option<usize>,
    ) -> Vec<MemoryEntry> {
        let words = query.map(split_keywords).unwrap_or_default();
        let cats: Vec<String> = categories
            .filter(|c| !c.is_empty())
            .map(|c| c.to_vec())
            .unwrap_or_default();
        if words.is_empty() && cats.is_empty() {
            return Vec::new();
        }

        let now = now_secs();
        let conn = self.conn.lock().unwrap();

        // 组装查询：category 分域 + key/tags LIKE（value 不参与匹配）
        let mut sql = String::from(
            "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
             FROM key_memories WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if !cats.is_empty() {
            let ph = std::iter::repeat("?")
                .take(cats.len())
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(&format!(" AND category IN ({})", ph));
            for c in &cats {
                args.push(rusqlite::types::Value::Text(c.clone()));
            }
        }
        for w in &words {
            let like = format!("%{}%", like_escape(w));
            sql.push_str(" AND (key LIKE ? ESCAPE '\\' OR tags LIKE ? ESCAPE '\\')");
            args.push(rusqlite::types::Value::Text(like.clone()));
            args.push(rusqlite::types::Value::Text(like));
        }

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row_to_entry);
        let entries: Vec<MemoryEntry> = match rows {
            Ok(rs) => rs.filter_map(|r| r.ok()).collect(),
            Err(_) => return Vec::new(),
        };

        // 排序：pin 恒置顶；组内 = 热度(半衰期衰减) + 命中加权；同分按最近访问新者优先
        let mut ranked = entries;
        ranked.sort_by(|a, b| rank_cmp(a, b, &words, now));
        if let Some(n) = limit {
            ranked.truncate(n);
        }
        ranked
    }

    /// 命中记录：计数 +1、刷新访问时间（仅对传入子集；不动 updated_at）。
    fn touch_memories(&self, entries: &[MemoryEntry]) {
        if entries.is_empty() {
            return;
        }
        let keys: Vec<String> = entries.iter().map(|e| e.key.clone()).collect();
        // 统一使用编号占位（从 1 起）：?1=时间戳，?2..=key 列表
        let ph = (2..=keys.len() + 1)
            .map(|i| format!("?{}", i))
            .collect::<Vec<_>>()
            .join(",");
        let key_args: Vec<rusqlite::types::Value> =
            keys.iter().map(|k| rusqlite::types::Value::Text(k.clone())).collect();
        let conn = self.conn.lock().unwrap();
        if let Err(e) = conn.execute(
            &format!(
                "UPDATE key_memories SET access_count = access_count + 1, last_accessed_at = ?1
                 WHERE key IN ({})",
                ph
            ),
            rusqlite::params_from_iter(
                std::iter::once(rusqlite::types::Value::Integer(now_secs() as i64)).chain(key_args.into_iter())
            ),
        ) {
            log::warn!("[Agent DB] search 刷新访问时间失败: {}", e);
        }
    }

    /// 获取访问频率最高的 N 条记忆（按累计热度）。生产注入已改用 `ranked_top`，
    /// 此方法仅供单元测试保持对旧语义的回归覆盖，故仅编译于测试目标。
    #[cfg(test)]
    pub fn get_top(&self, limit: usize) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
                 FROM key_memories
                 ORDER BY access_count DESC
                 LIMIT ?1",
            )
            .unwrap();

        stmt.query_map(params![limit as i64], row_to_entry)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// 注入/统计用：按统一评分排序取前 N（pin 恒置顶 → 热度(半衰期衰减) → 最近访问新者优先；
    /// 无查询词故命中加权为 0）。为 system prompt 注入与设置页「当前注入」预览提供一致口径。
    pub fn ranked_top(&self, limit: usize) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
             FROM key_memories",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([], row_to_entry);
        let mut entries: Vec<MemoryEntry> = match rows {
            Ok(rs) => rs.filter_map(|r| r.ok()).collect(),
            Err(_) => return Vec::new(),
        };
        let now = now_secs();
        let no_words: Vec<String> = Vec::new();
        entries.sort_by(|a, b| rank_cmp(a, b, &no_words, now));
        entries.truncate(limit);
        entries
    }

    /// 获取记忆总数
    pub fn count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM key_memories", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// 全量列表（支持按分类与关键词过滤；关键词不增计访问次数），按更新时间倒序。
    pub fn list_all(&self, category: Option<&str>, query: Option<&str>) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags FROM key_memories WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(cat) = category.filter(|c| !c.trim().is_empty()) {
            sql.push_str(" AND category = ?");
            args.push(rusqlite::types::Value::Text(cat.to_string()));
        }
        if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
            sql.push_str(" AND (key LIKE ? OR value LIKE ?)");
            let like = format!("%{}%", q);
            args.push(rusqlite::types::Value::Text(like.clone()));
            args.push(rusqlite::types::Value::Text(like));
        }
        sql.push_str(" ORDER BY updated_at DESC");

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let params = rusqlite::params_from_iter(args.iter());
        let rows = stmt.query_map(params, row_to_entry);
        rows.map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    /// 删除单条记忆；返回是否命中并删除。
    pub fn delete_memory(&self, key: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM key_memories WHERE key = ?1", params![key]).unwrap_or(0) > 0
    }

    /// 设置某条记忆的 pin（重要）标记；返回是否命中。
    pub fn set_pin(&self, key: &str, pin: bool) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE key_memories SET pin = ?1 WHERE key = ?2",
            params![pin as i64, key],
        )
        .unwrap_or(0)
            > 0
    }

    /// 重要(pin)记忆数量。
    pub fn pinned_count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM key_memories WHERE pin = 1", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// 自动维护候选条目（供预览/清理共用，确定性排序）：
    /// 未 pin 的「僵尸」（超冷却期未访问且访问次数 ≤ MEMORY_MIN_ACCESS），
    /// 以及超配额时按“最久未访问 → 访问最少 → 最早创建”挑出的多余条目，二者去重。
    pub fn maintenance_candidates(&self) -> Vec<MemoryEntry> {
        let idle_before = now_secs().saturating_sub(MEMORY_IDLE_SECS);
        let conn = self.conn.lock().unwrap();

        let select = "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
                      FROM key_memories";

        let mut zombie: Vec<MemoryEntry> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(&format!(
            "{} WHERE pin = 0 AND last_accessed_at < ?1 AND access_count <= ?2
             ORDER BY last_accessed_at ASC, created_at ASC",
            select
        )) {
            if let Ok(rows) = stmt.query_map(
                params![idle_before as i64, MEMORY_MIN_ACCESS as i64],
                row_to_entry,
            ) {
                zombie = rows.filter_map(|r| r.ok()).collect();
            }
        }

        let total = conn
            .query_row("SELECT COUNT(*) FROM key_memories", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize;
        let overflow_n = total.saturating_sub(MEMORY_MAX_ENTRIES);

        let mut overflow: Vec<MemoryEntry> = Vec::new();
        if overflow_n > 0 {
            if let Ok(mut stmt) = conn.prepare(&format!(
                "{} WHERE pin = 0
                 ORDER BY last_accessed_at ASC, access_count ASC, created_at ASC
                 LIMIT ?1",
                select
            )) {
                if let Ok(rows) = stmt.query_map(params![overflow_n as i64], row_to_entry) {
                    overflow = rows.filter_map(|r| r.ok()).collect();
                }
            }
        }

        // 去重合并（僵尸在前、配额补充在后），保持确定性顺序
        let mut seen = std::collections::HashSet::new();
        zombie
            .into_iter()
            .chain(overflow)
            .filter(|e| seen.insert(e.key.clone()))
            .collect()
    }

    /// 执行懒惰维护：删除全部候选条目（僵尸清理 + 配额驱逐）。返回删除条数。
    pub fn prune(&self) -> usize {
        let keys: Vec<String> = self
            .maintenance_candidates()
            .iter()
            .map(|e| e.key.clone())
            .collect();
        if keys.is_empty() {
            return 0;
        }
        let mut deleted = 0;
        {
            let conn = self.conn.lock().unwrap();
            for key in &keys {
                if conn
                    .execute("DELETE FROM key_memories WHERE key = ?1", params![key])
                    .unwrap_or(0)
                    > 0
                {
                    deleted += 1;
                    log::info!("[Agent DB] 记忆自动维护: 删除冷记忆 key={}", key);
                }
            }
        }
        deleted
    }

    /// 格式化记忆为 system prompt 中的注入块（fallback 路径）。
    /// 使用统一评分排序（ranked_top）取前 N，交由共享的 `format_memories_block`
    /// 完成 value 预览与字符预算。
    pub fn format_for_prompt(&self, limit: usize) -> Option<String> {
        let top = self.ranked_top(limit);
        format_memories_block(&top)
    }

    }

impl Clone for MemoryStore {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
        }
    }
}

/// 把一批记忆条目格式化为 system prompt 注入块 `<key_memories>…</key_memories>`。
/// 对单条 value 预览（600 字符）与整块字符做预算（约 9000），防止长记忆挤爆上下文。
/// 供 fallback 注入（format_for_prompt）与意图路由注入共用，保证格式一致。
pub fn format_memories_block(entries: &[MemoryEntry]) -> Option<String> {
    const VALUE_PREVIEW_CHARS: usize = 600;
    const MAX_BLOCK_CHARS: usize = 9000;
    if entries.is_empty() {
        return None;
    }

    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    for entry in entries {
        let value: String = if entry.value.chars().count() > VALUE_PREVIEW_CHARS {
            let head: String = entry.value.chars().take(VALUE_PREVIEW_CHARS).collect();
            format!("{}…", head)
        } else {
            entry.value.clone()
        };
        let line = format!("  - [{}] {}: {}\n", entry.category, entry.key, value);
        used += line.chars().count();
        if used > MAX_BLOCK_CHARS && !lines.is_empty() {
            break;
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }

    let mut block = String::from("<key_memories>\n");
    for line in lines {
        block.push_str(&line);
    }
    block.push_str("</key_memories>");
    Some(block)
}

// ── 辅助函数 ──

/// 归一化检索标签：按中英文分隔符（, ， 、 ; ； |）切分、去空后以英文逗号连接，
/// 保证入库/展示/检索只面对一种分隔符。
fn normalize_tags(raw: &str) -> String {
    raw.split(|c| matches!(c, ',' | '，' | '、' | ';' | '；' | '|'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// 检索关键词拆词：按空白与常见分隔符切分（不按空格拆中文短语；去空）。
fn split_keywords(raw: &str) -> Vec<String> {
    raw.split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '、' | ';' | '；' | '|'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// 转义 LIKE 元字符（% _ \），配合 ESCAPE '\' 使用，避免输入里的通配符造成超范围匹配。
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// 大小写不敏感子串匹配（用于打分阶段对齐 SQLite LIKE 的近似语义）。
fn ci_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let h = haystack.to_lowercase();
    let n = needle.to_lowercase();
    h.contains(&n)
}

/// 命中计数：key 域 / tags 域各自命中的关键词个数（一词同时命中两域则两域各计一次）。
fn hit_counts(words: &[String], key: &str, tags: &str) -> (usize, usize) {
    let mut key_hits = 0usize;
    let mut tag_hits = 0usize;
    for w in words {
        if ci_contains(key, w) {
            key_hits += 1;
        }
        if ci_contains(tags, w) {
            tag_hits += 1;
        }
    }
    (key_hits, tag_hits)
}

/// 检索/注入候选排序分（组内，不含 pin 置顶层级）：
/// 热度 = ln(visits+1)·0.5·0.5^(天数/半衰期)；命中加权 = key 域 0.8 + tags 域 1.2，
/// 首词之外的每个额外命中词再 +0.6（上限可控，避免多词查询反超“双域强相关”）。
fn rank_score(visits: u64, last_access: u64, now: u64, key_hits: usize, tag_hits: usize) -> f64 {
    let days = now.saturating_sub(last_access) as f64 / 86_400.0;
    let decay = 0.5f64.powf(days / RANK_HALF_LIFE_DAYS);
    let hot = ((visits as f64) + 1.0).ln() * 0.5 * decay;
    let key_bonus = if key_hits > 0 { 0.8 } else { 0.0 };
    let tag_bonus = if tag_hits > 0 { 1.2 } else { 0.0 };
    let extra = 0.6 * (key_hits.saturating_sub(1) + tag_hits.saturating_sub(1)) as f64;
    hot + key_bonus + tag_bonus + extra
}

/// 统一排序比较器（search_memory 与 ranked_top 共用）：pin 置顶 → 分数降序 → 最近访问新者优先。
fn rank_cmp(a: &MemoryEntry, b: &MemoryEntry, words: &[String], now: u64) -> std::cmp::Ordering {
    if a.pin != b.pin {
        return b.pin.cmp(&a.pin); // pin=true 在前
    }
    let (ak, at) = hit_counts(words, &a.key, &a.tags);
    let (bk, bt) = hit_counts(words, &b.key, &b.tags);
    let sa = rank_score(a.access_count, a.last_accessed_at, now, ak, at);
    let sb = rank_score(b.access_count, b.last_accessed_at, now, bk, bt);
    match sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal) {
        std::cmp::Ordering::Equal => b.last_accessed_at.cmp(&a.last_accessed_at),
        o => o,
    }
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<MemoryEntry> {
    Ok(MemoryEntry {
        key: row.get(0)?,
        value: row.get(1)?,
        category: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        access_count: row.get(5)?,
        last_accessed_at: row.get(6)?,
        pin: row.get::<_, i64>(7)? != 0,
        tags: row.get(8)?,
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    static INIT: Once = Once::new();

    /// 每测试独立目录（原子计数 + 时间戳）：避免并行测试共享同一 MEMORY.db。
    static TEST_DIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn temp_db_dir() -> String {
        let n = TEST_DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::env::temp_dir()
            .join(format!(
                "memory_db_test_{}_{}_{}",
                std::process::id(),
                now_secs(),
                n
            ))
            .to_string_lossy()
            .to_string()
    }

    fn temp_store() -> (MemoryStore, String) {
        INIT.call_once(|| {
            let _ = env_logger::builder().is_test(true).try_init();
        });

        let dir = temp_db_dir();
        std::fs::create_dir_all(&dir).ok();
        let store = MemoryStore::new(&dir).unwrap();
        (store, dir)
    }

    #[test]
    fn test_save_and_search() {
        let (store, dir) = temp_store();

        store.save_memory("project_language", "TypeScript", "fact", false, "ts,前端语言");
        store.save_memory("db_path", "./data.db", "fact", false, "");
        store.save_memory("user_style", "concise", "preference", false, "");

        // key 命中
        let r = store.search_memory(Some("language"), None, None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "project_language");

        // tags 命中（value 不参与匹配）
        let r = store.search_memory(Some("ts"), None, None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "project_language");

        // category 分域：preference 只命中 user_style
        let r = store.search_memory(None, Some(&["preference".to_string()]), None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "user_style");

        // value 不再作为匹配面："data" 只存在于 db_path 的 value 中，应无命中
        let r = store.search_memory(Some("data"), None, None);
        assert!(r.is_empty());

        // limit 生效
        let r = store.search_memory(Some("style"), None, Some(1));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "user_style");

        // 清理
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_existing() {
        let (store, dir) = temp_store();

        store.save_memory("a", "v1", "fact", false, "");
        let results = store.search_memory(Some("a"), None, None);
        assert_eq!(results[0].value, "v1");
        // 返回快照为读取时计数（1），检索后库内应 +1 为 2
        {
            let conn = store.conn.lock().unwrap();
            let c: i64 = conn
                .query_row(
                    "SELECT access_count FROM key_memories WHERE key = 'a'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(c, 2);
        }

        // 覆盖更新不累加热度（热度只来自 search 命中）
        store.save_memory("a", "v2", "fact", false, "");
        {
            let conn = store.conn.lock().unwrap();
            let c: i64 = conn
                .query_row(
                    "SELECT access_count FROM key_memories WHERE key = 'a'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(c, 2);
        }
        let results = store.search_memory(Some("a"), None, None);
        assert_eq!(results[0].value, "v2");
        {
            let conn = store.conn.lock().unwrap();
            let c: i64 = conn
                .query_row(
                    "SELECT access_count FROM key_memories WHERE key = 'a'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(c, 3); // 再次检索命中 +1
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_get_top() {
        let (store, dir) = temp_store();

        store.save_memory("a", "1", "fact", false, "");
        store.save_memory("b", "2", "fact", false, "");
        store.save_memory("c", "3", "preference", false, "");

        // 多次检索 key "a"
        store.search_memory(Some("a"), None, None);
        store.search_memory(Some("a"), None, None);

        let top = store.get_top(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].key, "a"); // 访问次数最高

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_format_for_prompt() {
        let (store, dir) = temp_store();

        store.save_memory("lang", "Rust", "fact", false, "");

        let formatted = store.format_for_prompt(5).unwrap();
        assert!(formatted.contains("<key_memories>"));
        assert!(formatted.contains("lang"));
        assert!(formatted.contains("Rust"));
        assert!(formatted.contains("</key_memories>"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_maintenance_skips_pinned_and_removes_stale() {
        let (store, dir) = temp_store();

        store.save_memory("fresh", "1", "fact", false, "");
        store.save_memory("stale", "2", "fact", false, "");
        store.save_memory("pinned", "3", "fact", true, "");

        // 人为把 stale 与 pinned 推成“超冷却期未访问”的僵尸候选
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE key_memories SET last_accessed_at = 1 WHERE key = 'stale'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE key_memories SET last_accessed_at = 1 WHERE key = 'pinned'",
                [],
            )
            .unwrap();
        }

        let keys: Vec<String> = store
            .maintenance_candidates()
            .iter()
            .map(|e| e.key.clone())
            .collect();
        assert!(keys.contains(&"stale".to_string()));
        // pin 条目绝不进候选；fresh 仍属近期访问
        assert!(!keys.contains(&"pinned".to_string()));
        assert!(!keys.contains(&"fresh".to_string()));

        assert_eq!(store.prune(), 1);
        assert!(store.search_memory(Some("stale"), None, None).is_empty());
        assert_eq!(store.search_memory(Some("pinned"), None, None).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_set_pin() {
        let (store, dir) = temp_store();

        store.save_memory("x", "1", "fact", false, "");
        assert!(!store.set_pin("missing", true));
        assert!(store.set_pin("x", true));
        assert!(store.search_memory(Some("x"), None, None)[0].pin);
        // 访问不改变 pin
        assert!(store.search_memory(Some("x"), None, None)[0].pin);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_normalize_tags() {
        assert_eq!(normalize_tags(""), "");
        assert_eq!(normalize_tags("db, 连接配置，数据库；中文"), "db,连接配置,数据库,中文");
        assert_eq!(normalize_tags(" a 、b、c ,d "), "a,b,c,d");
        assert_eq!(normalize_tags(" ,, ，、 ; | "), "");
    }

    #[test]
    fn test_rank_score_features() {
        let now = now_secs();
        let day = 86_400u64;
        // 单调性：访问越多越高；命中加分越高；越久未访问越低
        assert!(rank_score(10, now, now, 0, 0) > rank_score(1, now, now, 0, 0));
        assert!(rank_score(1, now, now, 1, 1) > rank_score(1, now, now, 0, 0));
        assert!(rank_score(1, now - 100 * day, now, 0, 0) < rank_score(1, now, now, 0, 0));
        // 老古董高频（200 次但 100 天未访问）应低于新鲜低频
        assert!(rank_score(1, now, now, 0, 0) > rank_score(200, now - 100 * day, now, 0, 0));
    }

    #[test]
    fn test_search_ranking_order() {
        let (store, dir) = temp_store();

        store.save_memory("fresh_key", "fresh value", "fact", false, "");
        store.save_memory("old_hot", "old hot value", "fact", false, "");
        store.save_memory("important", "pin value", "fact", true, "");
        {
            let conn = store.conn.lock().unwrap();
            // old_hot：高频但 100 天未访问
            conn.execute(
                "UPDATE key_memories SET access_count = 200, last_accessed_at = 1 WHERE key = 'old_hot'",
                [],
            )
            .unwrap();
            // important：pin 但同样很久未访问（验证 pin 恒置顶）
            conn.execute(
                "UPDATE key_memories SET last_accessed_at = 1 WHERE key = 'important'",
                [],
            )
            .unwrap();
        }

        let r = store.search_memory(None, Some(&["fact".to_string()]), None);
        let keys: Vec<String> = r.iter().map(|e| e.key.clone()).collect();
        assert_eq!(keys[0], "important"); // pin 恒置顶
        assert_eq!(keys[1], "fresh_key"); // 新鲜低频 > 老古董高频
        assert_eq!(keys[2], "old_hot");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
