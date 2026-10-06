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
pub(crate) const MEMORY_DB_FILE: &str = "MEMORY.db";

/// 记忆库完整路径（`{db_dir}/MEMORY.db`）。
/// KnowledgeStore 也走这里，避免两处各拼一份路径（文件名/分隔符改动只改一处）。
pub(crate) fn memory_db_path(db_dir: &str) -> String {
    format!(
        "{}/{}",
        db_dir.trim_end_matches(['/', '\\']),
        MEMORY_DB_FILE
    )
}

/// **会话记忆**（计入条数上限、参与冷热维护）的判定条件，用于 COUNT 与维护候选筛选，
/// 表名固定为 `key_memories`。取反就是知识库条目。
///
/// 注意：这里的写法是"满足条件 = 计入配额"，与常量名里的 EXEMPT 是反的（历史命名遗留）。
/// 知识条目（`category='knowledge'` 或存在 `kb_entry_links` 关联）**不占条数上限、也不按冷热清理**，
/// 生命周期由所属知识库决定（删库时按"是否仍被其它库关联"处理，见 knowledge::delete_base）。
pub(crate) const QUOTA_EXEMPT_SQL: &str = "category <> 'knowledge' AND NOT EXISTS \
     (SELECT 1 FROM kb_entry_links l WHERE l.entry_key = key_memories.key)";

/// 建 KV 记忆主表（幂等）。
///
/// 由 `MemoryStore::new` 与 `knowledge::ensure_schema` 共用：知识库的 FTS 触发器挂在
/// `key_memories` 上，单独建知识库表时也必须先把这张表建出来，否则触发器创建会失败。
pub(crate) fn ensure_key_memories_table(conn: &Connection) -> Result<(), String> {
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
    .map_err(|e| format!("创建记忆表失败: {}", e))
}

/// KV 记忆条数上限：总条数超过该值即驱逐“最冷且未 pin”的多余条目。
///
/// 上限是**客户端自行调节的默认值 + 技术硬上限**（不再是档位配额）：
/// 用户可在设置页调整，持久化于**主库 `pilotdesk.db` 的 `app_settings`**（与其它设置一致）。
/// MEMORY.db 只存记忆数据；上限由持有主库连接的调用方读出后**传入** [`MemoryStore`]。
pub const MEMORY_MAX_ENTRIES_KEY: &str = "memory_max_entries";
/// KV 记忆条数上限的默认值（用户未调整时生效）。
pub const MEMORY_MAX_ENTRIES_DEFAULT: usize = 500;
/// KV 记忆条数上限的可调下界。
pub const MEMORY_MAX_ENTRIES_MIN: usize = 100;
/// KV 记忆条数上限的技术硬上限（避免本地数据库无限膨胀）。
pub const MEMORY_MAX_ENTRIES_HARD_MAX: usize = 10000;

/// 把记忆条数上限夹取到 `[MEMORY_MAX_ENTRIES_MIN, MEMORY_MAX_ENTRIES_HARD_MAX]`。
pub fn clamp_memory_max_entries(value: usize) -> usize {
    value.clamp(MEMORY_MAX_ENTRIES_MIN, MEMORY_MAX_ENTRIES_HARD_MAX)
}

/// 从**主库** `app_settings` 读取当前生效的 KV 记忆条数上限（缺省 500），并 clamp 到 100..=10000。
///
/// 注意：传入的必须是**主库连接**（`pilotdesk.db`）——上限已从 MEMORY.db 迁移到主库与其它设置同处。
/// 表不存在 / 值非法时回退默认值。
pub fn load_memory_max_entries(conn: &Connection) -> usize {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        params![MEMORY_MAX_ENTRIES_KEY],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| s.trim().parse::<usize>().ok())
    .map(clamp_memory_max_entries)
    .unwrap_or(MEMORY_MAX_ENTRIES_DEFAULT)
}

/// 冷记忆（僵尸）判定冷却期（秒）：超过该时长既未被检索也未被编辑的未 pin 条目可清理
/// （活跃度取 max(last_accessed_at, updated_at)，见 maintenance_candidates）。
pub const MEMORY_IDLE_SECS: u64 = 180 * 24 * 3600;
/// KV 记忆访问次数上限：访问次数 ≤ 该值且冷却期超限的未 pin 条目才允许自动清理。
pub const MEMORY_MIN_ACCESS: u64 = 1;

/// 检索/注入排序的时间衰减半衰期（天）：超过该时长未访问，热度折半。
/// 取值更温和（30 天）：对话常用记忆的访问间隔通常以周计，7 天衰减过激。
const RANK_HALF_LIFE_DAYS: f64 = 30.0;

/// 合并判定的 token 数下限：字数太少的文本用相似度判定没有意义（噪声大），
/// 短内容只认包含关系。
const MERGE_MIN_TOKENS: usize = 5;
/// 合并判定的 Jaccard 相似度阈值：达到才视为"同一件事的另一种说法"。
const MERGE_MIN_SIMILARITY: f64 = 0.75;

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

/// `save_memory` 工具路径的写入结果（供工具向模型/用户如实反馈是否真的新增）。
#[derive(Debug, Clone)]
pub enum SaveOutcome {
    /// 新增了一行
    Inserted(MemoryEntry),
    /// 未新增行：内容与既有记忆等价/近似，已合并进 existing_key（同名 key 也走这条：
    /// 补充、包含、改写式重复都合并，不整块覆盖）
    Merged {
        /// 合并后的既有条目（含合并后的内容/标签/pin，供调用方如实反馈）
        entry: MemoryEntry,
        existing_key: String,
        kind: MergeKind,
    },
    /// 未写入：同名 key 已存在且内容与本次无关，覆盖会丢信息（本次未改动任何内容）。
    /// 调用方应改走 `update_memory`（那条路径才是"明确替换"）。
    Conflict { entry: MemoryEntry },
}

/// 合并类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeKind {
    /// 归一化后内容完全等价
    Exact,
    /// 一方内容被另一方整体包含（保留更完整者）
    Superset,
    /// 高相似但互不包含（整理合并双方独有行）
    Similar,
}

/// `update_memory` 的字段更新请求：`None` 表示该字段保持不变。
#[derive(Debug, Clone, Default)]
pub struct MemoryUpdate {
    /// 改名目标 key；缺省或与现 key 相同即不改名
    pub new_key: Option<String>,
    pub value: Option<String>,
    pub category: Option<String>,
    /// 整体替换（不是并集）：空串即清空标签
    pub tags: Option<String>,
    /// pin/重要：可双向设置（区别于 `save_memory` 的或运算，取消 pin 只能经此路径）
    pub pin: Option<bool>,
}

/// `update_memory` 的结果：更新前/后条目，供工具渲染"改动前后对照"。
#[derive(Debug, Clone)]
pub struct UpdateOutcome {
    pub before: MemoryEntry,
    pub after: MemoryEntry,
}

/// SQLite 版 KV 记忆库
///
/// 数据库路径：{config_dir}/MEMORY.db
/// 表结构：key_memories (key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
pub struct MemoryStore {
    conn: Arc<Mutex<Connection>>,
    /// KV 记忆条数上限：由**持有主库连接的调用方**读主库设置后传入，本库不再自行读设置。
    /// 打开时做一次懒惰维护（[`MemoryStore::new`]）与写入后懒惰维护（[`MemoryStore::prune`]）都用它。
    limit: usize,
}

impl MemoryStore {
    /// 打开或创建记忆库
    ///
    /// `db_dir` 为 PilotDesk 配置目录的路径（通常为 %APPDATA%/PilotDesk）。
    /// `limit` 为 KV 记忆条数上限（由调用方从**主库**设置读出，通常经
    /// [`load_memory_max_entries`]；本组件不再自行读设置，以免两库设置分裂）。
    /// 首次打开时自动建表。
    pub fn new(db_dir: &str, limit: usize) -> Result<Self, String> {
        let db_path = memory_db_path(db_dir);

        let conn =
            Connection::open(&db_path).map_err(|e| format!("无法打开记忆库 {}: {}", db_path, e))?;

        // 启用 WAL 模式，提升并发读取性能
        conn.execute_batch("PRAGMA journal_mode=WAL;").ok();

        // 建表（新库含维护列；旧库随后按列缺失逐个 ALTER 迁移）
        ensure_key_memories_table(&conn)?;

        // 记忆子系统的设置表：保留建表以兼容旧库（历史上 KV 记忆上限曾存于此）。
        // 上限现已迁移到主库（见 [`MEMORY_MAX_ENTRIES_KEY`]），本表仅作存量兼容。
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL
            );",
        )
        .map_err(|e| format!("创建记忆设置表失败: {}", e))?;

        // 知识库表与 FTS 索引：必须在这里一并建好 ——
        // QUOTA_EXEMPT_SQL 会查 kb_entry_links，条目增删的触发器也会写 kb_entry_fts，
        // 只有打开过知识库页面的用户才有这张表的话，上面的查询会直接报错。
        super::knowledge::ensure_schema(&conn)?;

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
            limit: clamp_memory_max_entries(limit),
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

    /// 按 key 保存或更新一条记忆（pin=true 标记为重要：永不参与自动淘汰；tags 为逗号分隔检索标签）。
    /// 新插入计 1 次访问；覆盖更新只刷新内容与 `updated_at`，不碰访问热度
    /// （`access_count` 与 `last_accessed_at` 只由 search_memory 的真实检索命中累加，
    /// 避免"反复覆盖=高频"失真；编辑过的条目靠 `updated_at` 参与存活判定，见
    /// `maintenance_candidates`）。
    ///
    /// 仅供设置页手动编辑入口使用：不做内容级去重（不同 key 的等价内容会各占一行），
    /// 工具路径请用 `save_memory`。
    pub fn upsert_memory(
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
            // 尝试更新已存在的 key（覆盖内容、刷新编辑时间，但不碰访问热度）
            let affected = conn
                .execute(
                    "UPDATE key_memories
                     SET value = ?1, category = ?2, updated_at = ?3, pin = ?4, tags = ?5
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
        // 用 quota_count（排除知识库条目）：知识条目不占这上限
        if self.quota_count() > self.limit {
            self.prune();
        }
        entry
    }

    /// 保存一条记忆（工具路径）：同名 key 覆盖；否则在同分类内做内容级去重，
    /// 归一化等价则合并进已有条目；近似重复（token Jaccard 相似度）在保留关键信息前提下整理合并；
    /// 都无法合并才新增。返回结果供工具如实反馈是否真的新增。
    ///
    /// 合并命中时：tags 取并集、pin 取或运算、updated_at 刷新（不新增行，热度不累加）。
    pub fn save_memory(
        &self,
        key: &str,
        value: &str,
        category: &str,
        pin: bool,
        tags: &str,
    ) -> SaveOutcome {
        let new_norm = normalize_value(value);
        let outcome = {
            let conn = self.conn.lock().unwrap();

            // 同名 key：不做整块覆盖。覆盖会丢信息（补充类内容、改写后的等价内容会被静默替换，
            // 只刷新时间），因此这里先判合并关系：能合就合并进该 key，判不出关系就拒绝写入，
            // 让调用方改走 update_memory（那条路径才是"明确替换/改名/改分类"的入口）。
            if let Some(existing) = get_entry_by_key(&conn, key) {
                let existing_norm = normalize_value(&existing.value);
                match merge_values(&existing.value, &existing_norm, &new_norm, value) {
                    Some((kind, merged_value)) => {
                        let merged_tags = merge_tags(&existing.tags, tags);
                        let merged_pin = existing.pin || pin;
                        let _ = conn.execute(
                            // 编辑不刷新访问热度：last_accessed_at / access_count 只由检索命中累加。
                            "UPDATE key_memories
                             SET value = ?1, category = ?2, updated_at = ?3, pin = ?4, tags = ?5
                             WHERE key = ?6",
                            params![
                                merged_value,
                                category,
                                now_secs(),
                                merged_pin as i64,
                                merged_tags,
                                key
                            ],
                        );
                        let entry = get_entry_by_key(&conn, key).expect("刚更新的记忆必存在");
                        SaveOutcome::Merged {
                            entry,
                            existing_key: key.to_string(),
                            kind,
                        }
                    }
                    None => SaveOutcome::Conflict { entry: existing },
                }
            } else {
                let candidates = list_entries_by_category(&conn, category);

                // 归一化内容完全等价：合并进已有条目，不新增
                if let Some(existing) = candidates
                    .iter()
                    .filter(|e| normalize_value(&e.value) == new_norm)
                    .max_by_key(|e| e.access_count)
                {
                    let entry = merge_into_existing(&conn, existing, &existing.value, pin, tags);
                    SaveOutcome::Merged {
                        entry,
                        existing_key: existing.key.clone(),
                        kind: MergeKind::Exact,
                    }
                } else if let Some((existing, kind, merged_value)) =
                    find_mergeable(&candidates, &new_norm, value)
                {
                    let entry = merge_into_existing(&conn, existing, &merged_value, pin, tags);
                    SaveOutcome::Merged {
                        entry,
                        existing_key: existing.key.clone(),
                        kind,
                    }
                } else {
                    // 新增（与 upsert 相同字段）
                    let tags = normalize_tags(tags);
                    let now = now_secs();
                    let _ = conn.execute(
                        "INSERT INTO key_memories
                             (key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
                         VALUES (?1, ?2, ?3, ?4, ?4, 1, ?4, ?5, ?6)",
                        params![key, value, category, now, pin as i64, tags],
                    );
                    let entry = get_entry_by_key(&conn, key).expect("刚插入的记忆必存在");
                    SaveOutcome::Inserted(entry)
                }
            }
        };

        // 写入后懒惰维护：超配额即驱逐（僵尸清理在 open 与维护命令处执行）
        // 用 quota_count（排除知识库条目）：知识条目不占这上限
        if self.quota_count() > self.limit {
            self.prune();
        }
        outcome
    }

    /// 直接更新一条既有记忆的语义字段（不新增、不做内容级合并）。
    ///
    /// 语义：
    /// - `key` 定位条目，不存在即报错（新增走 `save_memory`，本方法绝不隐式建行）；
    /// - `patch` 中为 `None` 的字段保持原值；
    /// - `new_key` 改名：目标 key 已被占用时报错，不覆盖另一条记忆；
    /// - `tags` 整体替换（空串即清空），`pin` 可双向设置；
    /// - 只刷新 `updated_at`：`access_count` 与 `last_accessed_at` 是检索热度信号，编辑不刷新它们；
    /// - `created_at` 不变。
    pub fn update_memory(&self, key: &str, patch: &MemoryUpdate) -> Result<UpdateOutcome, String> {
        let conn = self.conn.lock().unwrap();
        let before = get_entry_by_key(&conn, key).ok_or_else(|| {
            format!(
                "记忆不存在（key={}）：本工具只改已有条目，新增请用 save_memory",
                key
            )
        })?;

        let next_key = match patch.new_key.as_deref().filter(|k| *k != key) {
            Some(target) => {
                if get_entry_by_key(&conn, target).is_some() {
                    return Err(format!(
                        "目标 key 已存在（key={}）：改名会覆盖既有记忆，已拒绝；请换一个 key，或先用 delete_memory 处理它",
                        target
                    ));
                }
                target.to_string()
            }
            None => key.to_string(),
        };
        let next_value = patch.value.as_deref().unwrap_or(&before.value).to_string();
        let next_category = patch
            .category
            .as_deref()
            .unwrap_or(&before.category)
            .to_string();
        let next_tags = match patch.tags.as_deref() {
            Some(t) => normalize_tags(t),
            None => before.tags.clone(),
        };
        let next_pin = patch.pin.unwrap_or(before.pin);

        conn.execute(
            "UPDATE key_memories
             SET key = ?1, value = ?2, category = ?3, tags = ?4, pin = ?5, updated_at = ?6
             WHERE key = ?7",
            params![
                next_key,
                next_value,
                next_category,
                next_tags,
                next_pin as i64,
                now_secs(),
                key
            ],
        )
        .map_err(|e| format!("更新记忆失败: {}", e))?;

        let after = get_entry_by_key(&conn, &next_key).expect("刚更新的记忆必存在");
        Ok(UpdateOutcome { before, after })
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
    /// - 匹配面 = key / tags（LIKE 子串），value 不参与匹配；
    /// - query 按空白/中英文逗号/顿号/分号/竖线拆词，词间 OR（任一命中即可）；
    /// - query 与 categories 至少提供一个。
    ///
    /// 排序：pin 恒置顶（两级：先 pin 组、组内按分），组内分数 =
    /// 热度（ln(visits+1)·半衰期衰减，半衰期 30 天）+ 命中加权
    /// （key 域 +0.8 / tags 域 +1.2，首词外每个额外命中词 +0.6）；
    /// 同分按最近访问新者优先。limit 为显式截断上限（可选）。
    pub fn select_memories(
        &self,
        query: Option<&str>,
        categories: Option<&[String]>,
        limit: Option<usize>,
    ) -> Vec<MemoryEntry> {
        self.query_memories(query, categories, limit, false)
    }

    /// 只取**会话记忆**（排除知识库条目）：意图注入走这条。
    ///
    /// 为什么要分开：知识条目只有 key / tags 参与匹配（正文在 value 里，这条链路看不到），
    /// 于是"知识库能不能被检索到"取决于路由有没有给出分类 —— 给出分类时整库被
    /// `category IN (...)` 静默排除，不给出时又只可能命中标题。知识条目改走
    /// `search_knowledge`（正文 FTS）之后，这里必须显式排除，否则同一条会被两条路重复召回。
    pub fn select_session_memories(
        &self,
        query: Option<&str>,
        categories: Option<&[String]>,
        limit: Option<usize>,
    ) -> Vec<MemoryEntry> {
        self.query_memories(query, categories, limit, true)
    }

    /// `select_memories` / `select_session_memories` 的共同实现；`session_only` 时追加
    /// 「会话记忆」判据（`QUOTA_EXEMPT_SQL` 取真 —— 知识条目不占配额，与之互斥）。
    fn query_memories(
        &self,
        query: Option<&str>,
        categories: Option<&[String]>,
        limit: Option<usize>,
        session_only: bool,
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

        // 组装查询：category 分域 + (key|tags) LIKE，词间 OR
        // （任一命中即入候选，命中词数与命中域共同决定加权，见 rank_score）
        let mut sql = String::from(
            "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
             FROM key_memories WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if session_only {
            sql.push_str(&format!(" AND ({})", QUOTA_EXEMPT_SQL));
        }
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
        if !words.is_empty() {
            // 每个词是一组两域 OR；词与词之间再 OR：多词查询只要任一词命中即入候选，
            // 全词命中者靠 rank_score 的额外命中加权排到前面。
            let word_clause = "key LIKE ? ESCAPE '\\' OR tags LIKE ? ESCAPE '\\'";
            let joined = std::iter::repeat(word_clause)
                .take(words.len())
                .collect::<Vec<_>>()
                .join(" OR ");
            sql.push_str(&format!(" AND ({})", joined));
            for w in &words {
                let like = rusqlite::types::Value::Text(format!("%{}%", like_escape(w)));
                args.push(like.clone());
                args.push(like);
            }
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

    /// 批量取命中条目的**来源信息**：所属知识库名 + 专属属性。
    ///
    /// 一次查完而不是逐条查：一次注入最多十几条，没必要 N+1。
    /// 一条 SQL 同时拿两样东西，避免为"库名"和"属性"各查一遍（它们同源：都在 `kb_entry_links` 上）。
    pub fn knowledge_origins(&self, keys: &[String]) -> EntryOrigins {
        let mut out = EntryOrigins::default();
        if keys.is_empty() {
            return out;
        }
        let ph = std::iter::repeat("?")
            .take(keys.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT l.entry_key, b.name, l.kb_meta FROM kb_entry_links l
             JOIN knowledge_bases b ON b.id = l.kb_id
             WHERE l.entry_key IN ({}) ORDER BY b.created_at ASC",
            ph
        );
        let args: Vec<rusqlite::types::Value> = keys
            .iter()
            .map(|k| rusqlite::types::Value::Text(k.clone()))
            .collect();
        let conn = self.conn.lock().unwrap();
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return out;
        };
        if let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        }) {
            for (key, name, meta) in rows.filter_map(|r| r.ok()) {
                out.bases.entry(key.clone()).or_default().push(name);
                // 多库时属性取**第一个库**的（与来源标注取第一个库名同一口径）：
                // 同一条知识在不同库里的专属属性可以不同，注入块只能显示一份
                out.metas.entry(key).or_insert(meta);
            }
        }
        out
    }

    /// 按**正文**检索知识库条目（走 `kb_entry_fts`，trigram，中文按任意子串命中）。
    ///
    /// 为什么不能复用 `select_memories`：它的匹配面只有 key / tags，而知识条目的正文存在
    /// `value` 里 —— 于是"知识库能被检索到"一直是半残的：只有内容恰好出现在标题或标签里
    /// 才可能被召回，而且还要看路由有没有给出分类。索引一直都在（知识库页搜索正在用它），
    /// 只是没接到会话链路上。
    ///
    /// 口径与知识库页搜索一致：词间 OR（任一命中即入候选）；不足 `FTS_MIN_CHARS` 的词进不了
    /// trigram，退化走 LIKE。排序按**命中词数**（trigram 的命中语义就是子串包含，直接在 Rust 侧
    /// 数既准又便宜），其次 pin、热度、新近 —— 方向与 `select_memories` 一致。
    pub fn search_knowledge(&self, query: &str, limit: usize) -> Vec<MemoryEntry> {
        if limit == 0 {
            return Vec::new();
        }
        let words = split_keywords(query);
        if words.is_empty() {
            return Vec::new();
        }

        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(expr) = super::knowledge::fts_query(query) {
            clauses.push("key IN (SELECT key FROM kb_entry_fts WHERE kb_entry_fts MATCH ?)".into());
            args.push(rusqlite::types::Value::Text(expr));
        }
        for w in words
            .iter()
            .filter(|w| w.chars().count() < super::knowledge::FTS_MIN_CHARS)
        {
            let pat = rusqlite::types::Value::Text(format!("%{}%", like_escape(w)));
            clauses.push(
                "(value LIKE ? ESCAPE '\\' OR key LIKE ? ESCAPE '\\' OR tags LIKE ? ESCAPE '\\')"
                    .into(),
            );
            for _ in 0..3 {
                args.push(pat.clone());
            }
        }
        if clauses.is_empty() {
            return Vec::new();
        }

        let sql = format!(
            "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
             FROM key_memories WHERE category = 'knowledge' AND ({})",
            clauses.join(" OR ")
        );

        // 锁的作用域收到最小：查询拿到候选就放锁，排序在锁外做
        let candidates: Vec<MemoryEntry> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = match conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row_to_entry);
            let entries: Vec<MemoryEntry> = match rows {
                Ok(rs) => rs.filter_map(|r| r.ok()).collect(),
                Err(_) => return Vec::new(),
            };
            // 到这里 rows / stmt / conn 依次析构，锁随之释放
            entries
        };

        let mut scored: Vec<(usize, MemoryEntry)> = candidates
            .into_iter()
            .map(|e| {
                let hits = words
                    .iter()
                    .filter(|w| {
                        ci_contains(&e.key, w)
                            || ci_contains(&e.tags, w)
                            || ci_contains(&e.value, w)
                    })
                    .count();
                (hits, e)
            })
            .filter(|(hits, _)| *hits > 0)
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.pin.cmp(&a.1.pin))
                .then_with(|| b.1.access_count.cmp(&a.1.access_count))
                .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
        });
        scored.truncate(limit);
        scored.into_iter().map(|(_, e)| e).collect()
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
        let key_args: Vec<rusqlite::types::Value> = keys
            .iter()
            .map(|k| rusqlite::types::Value::Text(k.clone()))
            .collect();
        let conn = self.conn.lock().unwrap();
        if let Err(e) = conn.execute(
            &format!(
                "UPDATE key_memories SET access_count = access_count + 1, last_accessed_at = ?1
                 WHERE key IN ({})",
                ph
            ),
            rusqlite::params_from_iter(
                std::iter::once(rusqlite::types::Value::Integer(now_secs() as i64))
                    .chain(key_args.into_iter()),
            ),
        ) {
            log::warn!("[Agent DB] search 刷新访问时间失败: {}", e);
        }
    }

    /// 获取访问频率最高的 N 条记忆（按累计热度）。生产注入已改为意图检索
    /// （`memory_intent::select_memories`），此方法仅供单元测试保持对旧语义的回归覆盖，
    /// 故仅编译于测试目标。
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

    /// 统计/单测用：按统一评分排序取前 N（pin 恒置顶 → 热度(半衰期衰减) → 最近访问新者优先；
    /// 无查询词故命中加权为 0）。供设置页「高频 top-5」统计参考与测试使用；
    /// 生产注入只走意图检索（`memory_intent`），不再按此无条件注入。
    /// 与设置页列表同一口径：只统计会话记忆，不把知识库条目算进「高频」。
    pub fn ranked_top(&self, limit: usize) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(&format!(
            "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
             FROM key_memories WHERE {}",
            QUOTA_EXEMPT_SQL
        )) {
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

    /// 获取记忆总数（含知识库条目，用于展示）
    pub fn count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM key_memories", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize
    }

    /// 计入条数上限的条数（排除知识库条目，见 QUOTA_EXEMPT_SQL）。
    /// 配额判定与驱逐都必须用它 —— 否则批量导入知识会把会话记忆挤掉。
    pub fn quota_count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM key_memories WHERE {}",
                QUOTA_EXEMPT_SQL
            ),
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize
    }

    /// 当前生效的记忆条数上限：即调用方构造本库时传入的值（缺省 500，clamp 100..=10000）。
    /// 本库不自行读设置——上限的真身在**主库 `app_settings`**（见 [`load_memory_max_entries`]）。
    pub fn memory_limit(&self) -> usize {
        self.limit
    }

    /// 全量列表（支持按分类与关键词过滤；关键词不增计访问次数），按更新时间倒序。
    ///
    /// 关键词过滤的匹配面与检索路径（`select_memories`）一致：key / tags 子串，value 不参与匹配。
    ///
    /// **只列会话记忆**：知识库条目（见 `QUOTA_EXEMPT_SQL`）不在这里出现 —— 它们的生命周期
    /// 归所属知识库，删除语义也不同（从库中移除、不再被任何库关联才删本体），在 KV 列表里
    /// 露出只会诱导用错误的语义去删。检索路径不受影响，知识条目照旧参与
    /// `select_memories` / `search_memory`（共享检索链路是知识库复用这张表的目的）。
    pub fn list_all(&self, category: Option<&str>, query: Option<&str>) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags FROM key_memories WHERE 1=1",
        );
        sql.push_str(&format!(" AND {}", QUOTA_EXEMPT_SQL));
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(cat) = category.filter(|c| !c.trim().is_empty()) {
            sql.push_str(" AND category = ?");
            args.push(rusqlite::types::Value::Text(cat.to_string()));
        }
        if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
            sql.push_str(" AND (key LIKE ? ESCAPE '\\' OR tags LIKE ? ESCAPE '\\')");
            let like = rusqlite::types::Value::Text(format!("%{}%", like_escape(q)));
            args.push(like.clone());
            args.push(like);
        }
        sql.push_str(" ORDER BY updated_at DESC");

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let params = rusqlite::params_from_iter(args.iter());
        let rows = stmt.query_map(params, row_to_entry);
        rows.map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 删除单条记忆并返回被删条目（同一把锁内读取+删除，避免回显的快照与实际删除的不是同一条）。
    pub fn take_memory(&self, key: &str) -> Option<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let entry = get_entry_by_key(&conn, key)?;
        let removed = conn
            .execute("DELETE FROM key_memories WHERE key = ?1", params![key])
            .unwrap_or(0)
            > 0;
        removed.then_some(entry)
    }

    /// 删除单条记忆；返回是否命中并删除。
    pub fn delete_memory(&self, key: &str) -> bool {
        self.take_memory(key).is_some()
    }

    /// 删除一条**会话记忆**：知识库条目被拒（`Err`），key 不存在返回 `Ok(None)`。
    ///
    /// 知识条目的生命周期由所属知识库决定（删库 / 从库中移除，仅当不再被任何库关联才删本体，
    /// 见 `knowledge::unlink_entry` / `knowledge::delete_base`）。设置页与 `delete_memory` 工具
    /// 这条通用删除路径不认得知识库语义，会绕过它，所以在入口处收口；
    /// 知识库内部的事务删除与 `prune()`（候选已排除知识条目）不走这里。
    pub fn take_session_memory(&self, key: &str) -> Result<Option<MemoryEntry>, String> {
        let conn = self.conn.lock().unwrap();
        let Some(entry) = get_entry_by_key(&conn, key) else {
            return Ok(None);
        };
        if entry.category == super::knowledge::CATEGORY_KNOWLEDGE {
            return Err(format!(
                "「{}」属于知识库，不能从记忆管理里删除；请到「知识库」页把它从对应库中移除（不再被任何库关联时才会连同条目一起删除）。",
                key
            ));
        }
        let removed = conn
            .execute("DELETE FROM key_memories WHERE key = ?1", params![key])
            .unwrap_or(0)
            > 0;
        Ok(removed.then_some(entry))
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
        conn.query_row("SELECT COUNT(*) FROM key_memories WHERE pin = 1", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize
    }

    /// 自动维护候选条目（供预览/清理共用，确定性排序）：
    /// 未 pin 的「僵尸」（超冷却期既未被检索也未被编辑，且访问次数 ≤ MEMORY_MIN_ACCESS），
    /// 以及超配额时按"最久未活跃 → 访问最少 → 最早创建"挑出的多余条目，二者去重。
    ///
    /// 「活跃」取 `max(last_accessed_at, updated_at)`：写入（工具保存/设置页编辑）只刷
    /// `updated_at`，若只看 `last_accessed_at`，刚编辑过却从未被检索的条目会被误判成冷记忆。
    pub fn maintenance_candidates(&self) -> Vec<MemoryEntry> {
        let idle_before = now_secs().saturating_sub(MEMORY_IDLE_SECS);
        let conn = self.conn.lock().unwrap();

        let select = "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
                      FROM key_memories";

        let mut zombie: Vec<MemoryEntry> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(&format!(
            "{} WHERE pin = 0 AND {} AND max(last_accessed_at, updated_at) < ?1 AND access_count <= ?2
             ORDER BY max(last_accessed_at, updated_at) ASC, created_at ASC",
            select, QUOTA_EXEMPT_SQL
        )) {
            if let Ok(rows) = stmt.query_map(
                params![idle_before as i64, MEMORY_MIN_ACCESS as i64],
                row_to_entry,
            ) {
                zombie = rows.filter_map(|r| r.ok()).collect();
            }
        }

        // 配额只看"计入配额的行"（知识库条目不算），否则导入知识会把会话记忆挤出去
        let total = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM key_memories WHERE {}",
                    QUOTA_EXEMPT_SQL
                ),
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0) as usize;
        // 上限由调用方构造本库时传入（来自主库设置 `memory_max_entries`，缺省 500）；
        // 本库只持有该值，不再自行读设置，避免与主库设置分裂。
        let limit = self.limit;
        let overflow_n = total.saturating_sub(limit);

        let mut overflow: Vec<MemoryEntry> = Vec::new();
        if overflow_n > 0 {
            if let Ok(mut stmt) = conn.prepare(&format!(
                "{} WHERE pin = 0 AND {}
                 ORDER BY max(last_accessed_at, updated_at) ASC, access_count ASC, created_at ASC
                 LIMIT ?1",
                select, QUOTA_EXEMPT_SQL
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
}

impl Clone for MemoryStore {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
            limit: self.limit,
        }
    }
}

/// 命中条目的来源信息（`knowledge_origins` 的返回值）
#[derive(Debug, Clone, Default)]
pub struct EntryOrigins {
    /// key → 所属库名（按库创建顺序）
    pub bases: std::collections::HashMap<String, Vec<String>>,
    /// key → 该条目在**第一个**库里的专属属性 JSON（多库时以第一个为准）
    pub metas: std::collections::HashMap<String, String>,
}

/// 专属属性值 → 可读文本（空串 / null 视为"没填"）。
///
/// **渲染口径只此一处**：注入块（`meta_compact`）与 `search_knowledge` 工具都要把属性显示给人/模型看，
/// 两处各写一份必然漂移（这个项目已经栽过好几次同类问题）。
pub fn meta_value_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(if *b { "是" } else { "否" }.to_string()),
        _ => None,
    }
}

/// 专属属性 JSON 对象串 → 紧凑文本 `状态=现行、生效日期=2024-03-01`。
/// 解析不出来返回空串：属性是增益，不该让注入块整体失败。
pub fn meta_compact(meta_json: &str) -> String {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(meta_json)
    else {
        return String::new();
    };
    map.iter()
        .filter_map(|(k, v)| meta_value_text(v).map(|d| format!("{}={}", k, d)))
        .collect::<Vec<_>>()
        .join("、")
}

/// 一条带来源标注的命中：既是注入块的渲染输入，也是"这条从哪来"的唯一口径。
///
/// 来源标签由调用方给（会话记忆 `记忆·fact`、知识条目 `知识库·小说创作`）——
/// 让模型知道"这是用户偏好"还是"这是某份资料里的内容"，两者可信度与用法都不同。
/// `attrs` 是专属属性的紧凑文本（`meta_compact` 的产物，可能为空）：公文的"哪一版、是否现行"
/// 就在这里，注入时**必须带出来**，否则模型无从判断该引用哪个版本。
pub struct SourcedEntry {
    pub source: String,
    pub attrs: String,
    pub entry: MemoryEntry,
}

/// 把一批条目格式化为注入块 `<tag>…</tag>`，每行 `- [来源] key（属性）: value`。
/// 供意图检索注入（`memory_intent`）使用，保证注入格式统一。
///
/// `preview_chars` 是单条 value 的预览上限，**由调用方给**：会话记忆是短事实卡片（600 足够），
/// 知识条目是成块的正文（常常 400~1200 字，600 会把它砍掉一半）。
/// 整块预算（约 9000 字符）与来源无关，防的都是"长内容挤爆上下文"。
pub fn format_memories_block(
    entries: &[SourcedEntry],
    tag: &str,
    preview_chars: usize,
) -> Option<String> {
    const MAX_BLOCK_CHARS: usize = 9000;
    if entries.is_empty() {
        return None;
    }

    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    for e in entries {
        let value: String = if e.entry.value.chars().count() > preview_chars {
            let head: String = e.entry.value.chars().take(preview_chars).collect();
            format!("{}…", head)
        } else {
            e.entry.value.clone()
        };
        // 属性紧跟在标题后（`标题（状态=现行、生效日期=…）`）：公文场景下"哪一版"与标题同等重要，
        // 但它是限定语，不该另起一行把正文挤下去
        let attrs = if e.attrs.trim().is_empty() {
            String::new()
        } else {
            format!("（{}）", e.attrs.trim())
        };
        let line = format!("  - [{}] {}{}: {}\n", e.source, e.entry.key, attrs, value);
        used += line.chars().count();
        if used > MAX_BLOCK_CHARS && !lines.is_empty() {
            break;
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }

    let mut block = format!("<{}>\n", tag);
    for line in lines {
        block.push_str(&line);
    }
    block.push_str(&format!("</{}>", tag));
    Some(block)
}

/// 命中的来源标签（给模型看的注入块与给用户看的检索轨迹共用，避免两处对不上）。
///
/// - 会话记忆：`记忆·<分类>`
/// - 知识条目：`知识库·<第一个库名>`，多归属时补「等 N 个库」（与前端「N 库」同一口径）；
///   不属于任何库（理论上不该有）时回落到裸「知识库」。
pub fn source_label(kind: SourceKind<'_>) -> String {
    match kind {
        SourceKind::Memory(category) => format!("记忆·{}", category),
        SourceKind::Knowledge(names) => match names.split_first() {
            Some((first, rest)) if !rest.is_empty() => {
                format!("知识库·{} 等 {} 个库", first, names.len())
            }
            Some((first, _)) => format!("知识库·{}", first),
            None => "知识库".to_string(),
        },
    }
}

/// `source_label` 的输入：会话记忆只认分类，知识条目要库名列表
pub enum SourceKind<'a> {
    Memory(&'a str),
    Knowledge(&'a [String]),
}

// ── 辅助函数 ──

/// 归一化检索标签：按中英文分隔符（, ， 、 ; ； |）切分、去空后以英文逗号连接，
/// 保证入库/展示/检索只面对一种分隔符。
///
/// `pub(crate)`：知识库入口也用同一套归一化，避免两处各写一遍分隔符规则。
pub(crate) fn normalize_tags(raw: &str) -> String {
    raw.split(|c| matches!(c, ',' | '，' | '、' | ';' | '；' | '|'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// 归一化内容用于等价比较：所有空白折叠为单个空格并去首尾空白。
fn normalize_value(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 内容 token 集合：小写字母数字单词 + CJK 二元组（用于近似重复的 Jaccard 比较）。
fn value_tokens(s: &str) -> std::collections::HashSet<String> {
    let mut tokens: std::collections::HashSet<String> = std::collections::HashSet::new();
    let lower = s.to_lowercase();
    let mut word = String::new();
    let mut cjk_run: Vec<char> = Vec::new();

    let flush_word = |word: &mut String, tokens: &mut std::collections::HashSet<String>| {
        if !word.is_empty() {
            tokens.insert(std::mem::take(word));
        }
    };
    let flush_cjk = |run: &mut Vec<char>, tokens: &mut std::collections::HashSet<String>| {
        if run.len() >= 2 {
            for pair in run.windows(2) {
                tokens.insert(pair.iter().collect());
            }
        }
        run.clear();
    };

    for c in lower.chars() {
        let is_cjk =
            ('\u{3400}'..='\u{4DBF}').contains(&c) || ('\u{4E00}'..='\u{9FFF}').contains(&c);
        if is_cjk {
            flush_word(&mut word, &mut tokens);
            cjk_run.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut tokens);
            word.push(c);
        } else {
            flush_word(&mut word, &mut tokens);
            flush_cjk(&mut cjk_run, &mut tokens);
        }
    }
    flush_word(&mut word, &mut tokens);
    flush_cjk(&mut cjk_run, &mut tokens);
    tokens
}

/// 两个 token 集合的 Jaccard 相似度（交集/并集）；任一为空返回 0。
fn jaccard(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// 合并两组标签：归一化后按英文逗号去重（已有在前、新增补充），保持稳定顺序。
fn merge_tags(existing: &str, new: &str) -> String {
    let existing = normalize_tags(existing);
    let new = normalize_tags(new);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for t in existing.split(',').chain(new.split(',')) {
        let t = t.trim();
        if !t.is_empty() && seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
    }
    out.join(",")
}

/// 行级整理合并：已有行在前，其后追加新增中未出现的行（trim、去空行、顺序去重）。
fn merge_lines(existing: &str, new: &str) -> String {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<&str> = Vec::new();
    for line in existing.split('\n').chain(new.split('\n')) {
        let line = line.trim();
        if !line.is_empty() && seen.insert(line.to_string()) {
            out.push(line);
        }
    }
    out.join("\n")
}

/// 按 key 读取单条记忆（不存在返回 None）。
fn get_entry_by_key(conn: &Connection, key: &str) -> Option<MemoryEntry> {
    conn.query_row(
        "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
         FROM key_memories WHERE key = ?1",
        params![key],
        row_to_entry,
    )
    .ok()
}

/// 读取同分类下的全部记忆（供内容级去重候选筛选）。
fn list_entries_by_category(conn: &Connection, category: &str) -> Vec<MemoryEntry> {
    let mut stmt = match conn.prepare(
        "SELECT key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags
         FROM key_memories WHERE category = ?1",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map(params![category], row_to_entry)
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

/// 单对内容的合并判定（同名 key 写入与跨 key 近似去重共用）。
///
/// 判定顺序：归一化等价 → 一方包含另一方（短内容也能整体补进）→ token 相似度（同一件事的
/// 另一种说法）。返回（合并类型, 最终 value）；判不出关系、或整理后超 4000 字符时返回 None
/// —— 调用方此时不得覆盖写入，否则会丢信息。
fn merge_values(
    existing_value: &str,
    existing_norm: &str,
    new_norm: &str,
    new_value: &str,
) -> Option<(MergeKind, String)> {
    if existing_norm == new_norm {
        return Some((MergeKind::Exact, existing_value.to_string()));
    }
    if existing_norm.contains(new_norm) {
        return Some((MergeKind::Superset, existing_value.to_string()));
    }
    if new_norm.contains(existing_norm) {
        return Some((MergeKind::Superset, new_value.to_string()));
    }

    let new_tokens = value_tokens(new_norm);
    let existing_tokens = value_tokens(existing_norm);
    if new_tokens.len() < MERGE_MIN_TOKENS || existing_tokens.len() < MERGE_MIN_TOKENS {
        return None;
    }
    if jaccard(&new_tokens, &existing_tokens) < MERGE_MIN_SIMILARITY {
        return None;
    }
    let merged = merge_lines(existing_value, new_value);
    (merged.chars().count() <= 4000).then_some((MergeKind::Similar, merged))
}

/// 在既有条目中挑出可合并的近似重复：候选与新值 token 数均达阈值且 Jaccard 达相似阈值，
/// 取相似度最高者（平手取 access_count 更高者）。返回（既有条目、合并类型、最终 value）。
fn find_mergeable<'a>(
    candidates: &'a [MemoryEntry],
    new_norm: &str,
    new_value: &str,
) -> Option<(&'a MemoryEntry, MergeKind, String)> {
    let new_tokens = value_tokens(new_norm);
    if new_tokens.len() < MERGE_MIN_TOKENS {
        return None;
    }

    let mut best: Option<(&MemoryEntry, f64)> = None;
    for e in candidates {
        let tokens = value_tokens(&e.value);
        if tokens.len() < MERGE_MIN_TOKENS {
            continue;
        }
        let sim = jaccard(&new_tokens, &tokens);
        if sim < MERGE_MIN_SIMILARITY {
            continue;
        }
        let better = match best {
            None => true,
            Some((b, best_sim)) => {
                sim > best_sim || (sim == best_sim && e.access_count > b.access_count)
            }
        };
        if better {
            best = Some((e, sim));
        }
    }

    let (existing, _) = best?;
    merge_values(
        &existing.value,
        &normalize_value(&existing.value),
        new_norm,
        new_value,
    )
    .map(|(kind, value)| (existing, kind, value))
}

/// 把本次内容合并进既有条目：value 落库、tags 并集、pin 或运算、刷新 updated_at。
fn merge_into_existing(
    conn: &Connection,
    existing: &MemoryEntry,
    merged_value: &str,
    pin: bool,
    tags: &str,
) -> MemoryEntry {
    let merged_tags = merge_tags(&existing.tags, tags);
    let merged_pin = existing.pin || pin;
    let now = now_secs();
    let _ = conn.execute(
        "UPDATE key_memories SET value = ?1, updated_at = ?2, pin = ?3, tags = ?4 WHERE key = ?5",
        params![
            merged_value,
            now,
            merged_pin as i64,
            merged_tags,
            existing.key
        ],
    );
    get_entry_by_key(conn, &existing.key).expect("刚更新的记忆必存在")
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
/// `pub(crate)`：知识库检索的短词退化路径也用它，保证两处转义口径一致。
pub(crate) fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
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

/// 命中计数：key 域 / tags 域各自命中的关键词个数（一词同时命中两域则各域分别计一次）。
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
/// 热度 = ln(visits+1)·0.5·0.5^(天数/半衰期)；命中加权 = key 域 0.8 + tags 域 1.2
/// （key 是标识、tags 是人工标注，命中信号强度依次递增），
/// 首词之外的每个额外命中词再 +0.6（上限可控，避免多词查询反超“多域强相关”）。
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

/// 当前 Unix 秒级时间戳（记忆写入与盘点共用同一时间源）。
pub(crate) fn now_secs() -> u64 {
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
        let store = MemoryStore::new(&dir, MEMORY_MAX_ENTRIES_DEFAULT).unwrap();
        (store, dir)
    }

    #[test]
    fn test_save_and_search() {
        let (store, dir) = temp_store();

        store.save_memory(
            "project_language",
            "TypeScript",
            "fact",
            false,
            "ts,前端语言",
        );
        store.save_memory("db_path", "./data.db", "fact", false, "");
        store.save_memory("user_style", "concise", "preference", false, "");

        // key 命中
        let r = store.search_memory(Some("language"), None, None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "project_language");

        // tags 命中
        let r = store.search_memory(Some("ts"), None, None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "project_language");

        // category 分域：preference 只命中 user_style
        let r = store.search_memory(None, Some(&["preference".to_string()]), None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "user_style");

        // value 不参与匹配："data" 只存在于 db_path 的 value 中，不应命中
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
    fn multi_word_query_matches_any_word_and_ranks_more_hits_first() {
        let (store, dir) = temp_store();
        store.save_memory("cart_ui", "购物车页面", "fact", false, "购物车,配色");
        store.save_memory("cart_icon", "购物车图标资源", "fact", false, "购物车,图标");

        // 多词查询：任一词命中即入候选。此前逐词 AND —— 含标签词的整句/多词查询会 0 命中。
        let r = store.search_memory(Some("购物车 配色"), None, None);
        assert_eq!(r.len(), 2, "任一词命中即入候选，两条应各自返回");
        assert_eq!(r[0].key, "cart_ui", "命中词更多者应排前");

        // 标签词单独查询仍成立
        let r = store.search_memory(Some("配色"), None, None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "cart_ui");

        // 分隔符体系与拆词一致：顿号 / 逗号 / 分号 / 竖线均视为词边界
        let r = store.search_memory(Some("配色、图标"), None, None);
        assert_eq!(r.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn query_ignores_value_text() {
        let (store, dir) = temp_store();
        // 匹配面只有 key / tags：value 独有词不构成命中。
        // 三条 value 必须互不相同：save_memory 会按内容去重合并，value 相同会被并入同一条。
        store.save_memory("only_in_key", "alpha", "fact", false, "");
        store.save_memory("plain_key", "beta", "fact", false, "onlyintags");
        store.save_memory("another_key", "onlyinvalue", "fact", false, "");

        for (q, expected) in [("only_in_key", "only_in_key"), ("onlyintags", "plain_key")] {
            let r = store.search_memory(Some(q), None, None);
            assert_eq!(r.len(), 1, "query={} 应命中 1 条", q);
            assert_eq!(r[0].key, expected, "query={} 命中的记录不符", q);
        }

        // value 独有词：0 命中
        let r = store.search_memory(Some("onlyinvalue"), None, None);
        assert!(r.is_empty(), "value 不应作为匹配面");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_all_filter_matches_tags_and_ignores_value() {
        let (store, dir) = temp_store();
        store.save_memory("cart_ui", "onlyinvalue", "fact", false, "购物车,配色");
        store.save_memory("cart_icon", "购物车图标资源", "fact", false, "");

        // 标签命中
        let r = store.list_all(None, Some("配色"));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "cart_ui");

        // value 独有词不参与过滤
        assert!(
            store.list_all(None, Some("onlyinvalue")).is_empty(),
            "value 不应作为过滤面"
        );

        // 分类与关键词同时生效
        assert!(store
            .list_all(Some("preference"), Some("购物车"))
            .is_empty());

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

        // 改内容走 update_memory：不累加热度（热度只来自 search 命中）
        store
            .update_memory(
                "a",
                &MemoryUpdate {
                    value: Some("v2".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
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
    fn test_format_memories_block() {
        let (store, dir) = temp_store();

        store.save_memory("lang", "Rust", "fact", false, "");

        let entries: Vec<SourcedEntry> = store
            .ranked_top(5)
            .into_iter()
            .map(|e| SourcedEntry {
                source: source_label(SourceKind::Memory(&e.category)),
                attrs: String::new(),
                entry: e,
            })
            .collect();
        let formatted = format_memories_block(&entries, "key_memories", 600).unwrap();
        assert!(formatted.contains("<key_memories>"));
        assert!(formatted.contains("lang"));
        assert!(formatted.contains("Rust"));
        assert!(formatted.contains("</key_memories>"));
        // 每行都要带来源 —— 模型据此分辨"用户既有认知"与"某份资料里的内容"
        assert!(formatted.contains("[记忆·fact] lang"), "{}", formatted);

        // 专属属性紧跟在标题后注入：公文场景下"哪一版、是否现行"与标题同等重要
        let with_attrs: Vec<SourcedEntry> = store
            .ranked_top(1)
            .into_iter()
            .map(|e| SourcedEntry {
                source: "知识库·制度".to_string(),
                attrs: "状态=现行、生效日期=2024-03-01".to_string(),
                entry: e,
            })
            .collect();
        let formatted = format_memories_block(&with_attrs, "knowledge_base", 600).unwrap();
        assert!(
            formatted.contains("[知识库·制度] lang（状态=现行、生效日期=2024-03-01）: Rust"),
            "{}",
            formatted
        );

        // 标签可换（知识库用 <knowledge_base>），块名必须跟着走 —— 否则模型会把资料当记忆
        let kb = format_memories_block(&entries, "knowledge_base", 1200).unwrap();
        assert!(kb.starts_with("<knowledge_base>"));
        assert!(kb.ends_with("</knowledge_base>"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 来源标签：知识条目要带库名，多归属时标注「等 N 个库」（与前端「N 库」同一口径）
    #[test]
    fn source_labels_name_the_base_and_note_multiple_memberships() {
        assert_eq!(
            source_label(SourceKind::Memory("preference")),
            "记忆·preference"
        );
        let one = vec!["小说创作".to_string()];
        assert_eq!(source_label(SourceKind::Knowledge(&one)), "知识库·小说创作");
        let two = vec!["小说创作".to_string(), "写作规范".to_string()];
        assert_eq!(
            source_label(SourceKind::Knowledge(&two)),
            "知识库·小说创作 等 2 个库"
        );
        // 不属于任何库（理论上不该有）：回落成裸「知识库」，不显示空着的库名
        assert_eq!(source_label(SourceKind::Knowledge(&[])), "知识库");
    }

    /// 命中的来源信息（库名 + 专属属性）要能一次批量查到，且多库时都以**第一个库**为准。
    #[test]
    fn knowledge_origins_return_base_names_and_metas_in_one_batch() {
        let (store, dir) = temp_store();
        {
            let conn = store.conn.lock().unwrap();
            for (id, name, ts) in [("kb1", "小说创作", 1), ("kb2", "写作规范", 2)] {
                conn.execute(
                    "INSERT INTO knowledge_bases(id, name, description, field_schema, created_at, updated_at)
                     VALUES(?1, ?2, '', '[]', ?3, ?3)",
                    params![id, name, ts],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO key_memories(key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
                 VALUES('黄金三章', '正文', 'knowledge', 1, 1, 0, 0, 0, '')",
                [],
            )
            .unwrap();
            for (kb, meta) in [("kb1", r#"{"状态":"现行"}"#), ("kb2", "{}")] {
                conn.execute(
                    "INSERT INTO kb_entry_links(kb_id, entry_key, kb_meta, added_at)
                     VALUES(?1, '黄金三章', ?2, 1)",
                    params![kb, meta],
                )
                .unwrap();
            }
        }

        let origins =
            store.knowledge_origins(&["黄金三章".to_string(), "没有这个条目".to_string()]);
        let bases = origins.bases.get("黄金三章").expect("两个库都该查到");
        assert_eq!(bases.len(), 2);
        assert_eq!(bases[0], "小说创作", "按库创建顺序，第一个库在前");
        assert_eq!(
            origins.metas.get("黄金三章").map(String::as_str),
            Some(r#"{"状态":"现行"}"#),
            "属性取第一个库的（与来源标注同一口径）"
        );
        assert!(
            !origins.bases.contains_key("没有这个条目"),
            "没有归属的 key 不进结果"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 知识库条目必须**按正文**能被检索到 —— 这是"知识库建了却用不上"的核心修复。
    /// 顺带守住 `session_only` 的分工：会话记忆那条路不能再把知识条目捞回来（否则重复注入）。
    #[test]
    fn knowledge_entries_are_found_by_body_text_but_not_by_session_memory_search() {
        let (store, dir) = temp_store();

        // 一条普通会话记忆 + 一条知识条目：关键词只出现在**知识条目的正文**里
        // （标题和标签都不含它，正是"按 key/tags 匹配"必然漏掉的情形）
        store.save_memory("出行习惯", "偏好靠窗", "preference", false, "");
        // 建表与 FTS 触发器由 `MemoryStore::new` 一并准备好（它内部会调 knowledge::ensure_schema）
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO key_memories(key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
                 VALUES('退火工艺', '量子退火通过隧穿效应跳出局部最优，适合组合优化问题。', 'knowledge', 1, 2, 0, 0, 0, '优化')",
                [],
            )
            .unwrap();
        }

        // 正文命中：走 kb_entry_fts（trigram），标题与标签都不含"隧穿效应"
        let hit = store.search_knowledge("隧穿效应", 5);
        assert_eq!(hit.len(), 1, "正文里的词必须能召回");
        assert_eq!(hit[0].key, "退火工艺");

        // 两字词：trigram 索引不到，必须靠 LIKE 退化路径兜住（中文里两字词极常见）
        assert_eq!(store.search_knowledge("量子", 5).len(), 1, "两字词不能漏");
        // 不相关的词不许召回
        assert!(store.search_knowledge("完全不相关的词", 5).is_empty());

        // 会话记忆那条路：按 key 命中 preference，且不含知识条目
        // （注意按"靠窗"搜不到 —— 那条匹配面只有 key / tags，value 不参与，
        //   这正是知识条目必须另走一条正文检索的原因）
        assert!(store
            .select_session_memories(Some("靠窗"), None, Some(5))
            .is_empty());
        let session = store.select_session_memories(Some("出行"), None, Some(5));
        assert_eq!(session.len(), 1);
        assert_eq!(session[0].key, "出行习惯");
        // 就算按知识条目的标题去搜，会话记忆那条路也不该返回它（分工要守住）
        assert!(store
            .select_session_memories(Some("退火"), None, Some(5))
            .is_empty());
        // 而不加限制的 select_memories 仍然看得到它（旧语义不变，供 search_memory 工具使用）
        assert_eq!(store.select_memories(Some("退火"), None, Some(5)).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_maintenance_skips_pinned_and_removes_stale() {
        let (store, dir) = temp_store();

        store.save_memory("fresh", "1", "fact", false, "");
        store.save_memory("stale", "2", "fact", false, "");
        store.save_memory("pinned", "3", "fact", true, "");
        store.save_memory("edited", "4", "fact", false, "");

        // stale / pinned 推成"超冷却期且从未编辑"的僵尸候选；
        // edited 只把访问时间压旧、编辑时间保持刚写 → 仍属活跃，不应进候选。
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE key_memories SET last_accessed_at = 1, updated_at = 1 WHERE key = 'stale'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE key_memories SET last_accessed_at = 1, updated_at = 1 WHERE key = 'pinned'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE key_memories SET last_accessed_at = 1 WHERE key = 'edited'",
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
        // pin 条目绝不进候选；fresh 属近期写入；edited 属刚编辑过
        assert!(!keys.contains(&"pinned".to_string()));
        assert!(!keys.contains(&"fresh".to_string()));
        assert!(
            !keys.contains(&"edited".to_string()),
            "编辑过的条目不应被当冷记忆: {:?}",
            keys
        );

        assert_eq!(store.prune(), 1);
        assert!(store.search_memory(Some("stale"), None, None).is_empty());
        assert_eq!(store.search_memory(Some("pinned"), None, None).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_upsert_memory_edit_keeps_access_time() {
        let (store, dir) = temp_store();
        store.upsert_memory("k", "v1", "fact", false, "a");
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE key_memories SET last_accessed_at = 1 WHERE key = 'k'",
                [],
            )
            .unwrap();
        }

        // 设置页人工编辑：改内容/分类/pin，但不碰访问时间（编辑 ≠ 访问）
        let entry = store.upsert_memory("k", "v2", "preference", true, "a,b");
        assert_eq!(entry.value, "v2");
        assert_eq!(entry.category, "preference");
        assert!(entry.pin);
        assert_eq!(entry.last_accessed_at, 1, "人工编辑不应刷新访问时间");

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

    /// 读取单条记忆（只读路径，不刷新访问热度）：测试里取更新前快照用。
    fn entry_of(store: &MemoryStore, key: &str) -> MemoryEntry {
        store
            .list_all(None, Some(key))
            .into_iter()
            .find(|e| e.key == key)
            .expect("记忆应存在")
    }

    #[test]
    fn test_update_memory_changes_all_semantic_fields_and_keeps_heat() {
        let (store, dir) = temp_store();
        store.save_memory("k1", "v1", "fact", false, "a,b");
        // 先检索一次，让访问热度有值可对照
        store.search_memory(Some("k1"), None, None);
        let before = entry_of(&store, "k1");

        let patch = MemoryUpdate {
            new_key: Some("k2".to_string()),
            value: Some("v2".to_string()),
            category: Some("preference".to_string()),
            // 整体替换：不是并集，写错/过期的标签能被去掉
            tags: Some("c".to_string()),
            pin: Some(true),
        };
        let UpdateOutcome {
            before: b,
            after: a,
        } = store.update_memory("k1", &patch).unwrap();

        assert_eq!(b.key, "k1");
        assert_eq!(a.key, "k2");
        assert_eq!(a.value, "v2");
        assert_eq!(a.category, "preference");
        assert_eq!(a.tags, "c");
        assert!(a.pin);
        // 热度与创建时间属于检索/生命周期信号，编辑不刷新它们（updated_at 见下方说明）
        assert_eq!(a.created_at, before.created_at);
        assert_eq!(a.access_count, before.access_count);
        assert_eq!(a.last_accessed_at, before.last_accessed_at);
        // updated_at 为秒级，同秒内更新无法断言严格递增，只断言不回退
        assert!(a.updated_at >= before.updated_at);
        // 旧 key 已不存在
        assert!(store
            .list_all(None, Some("k1"))
            .iter()
            .all(|e| e.key != "k1"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_memory_can_clear_tags_and_unset_pin() {
        let (store, dir) = temp_store();
        store.save_memory("k", "v", "fact", true, "a,b");

        let patch = MemoryUpdate {
            tags: Some(String::new()),
            pin: Some(false),
            ..Default::default()
        };
        let UpdateOutcome { after, .. } = store.update_memory("k", &patch).unwrap();
        assert_eq!(after.tags, "");
        assert!(
            !after.pin,
            "取消 pin 只能经 update_memory（save 的 pin 是或运算）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_memory_rejects_unknown_key_and_taken_target() {
        let (store, dir) = temp_store();
        store.save_memory("k1", "v1", "fact", false, "");
        store.save_memory("k2", "v2", "fact", false, "");

        // 不存在的 key：不隐式新增
        let err = store
            .update_memory(
                "nope",
                &MemoryUpdate {
                    value: Some("x".to_string()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.contains("不存在"), "错误信息应说明 key 不存在: {}", err);

        // 改名撞已有 key：拒绝，且两条都不变
        let err = store
            .update_memory(
                "k1",
                &MemoryUpdate {
                    new_key: Some("k2".to_string()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(
            err.contains("已存在"),
            "错误信息应说明目标 key 被占用: {}",
            err
        );
        assert_eq!(entry_of(&store, "k1").value, "v1");
        assert_eq!(entry_of(&store, "k2").value, "v2");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_take_memory_returns_removed_entry() {
        let (store, dir) = temp_store();
        store.save_memory("k", "v", "preference", true, "a");

        let taken = store.take_memory("k").expect("命中应返回被删条目");
        assert_eq!(taken.key, "k");
        assert_eq!(taken.value, "v");
        assert_eq!(taken.category, "preference");
        assert!(taken.pin, "回显需带上 pin，误删恢复时才能原样写回");
        // 已删除：再次取为空，且 delete_memory 复用同一路径
        assert!(store.take_memory("k").is_none());
        assert!(!store.delete_memory("k"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// KV 记忆这一面（列表 + 删除）只认会话记忆：知识库条目不露头，也不能从通用删除路径删掉。
    #[test]
    fn kv_side_excludes_knowledge_entries_from_list_and_delete() {
        let (store, dir) = temp_store();
        store.save_memory("会话记忆", "普通事实", "fact", false, "");
        // 造一条知识库条目（本体 + 关联行），模拟「知识库」页投喂进来的内容
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO key_memories(key, value, category, created_at, updated_at, access_count, last_accessed_at, pin, tags)
                 VALUES('知识条目', '库里的内容', 'knowledge', 1, 1, 0, 0, 0, '')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO kb_entry_links(kb_id, entry_key, kb_meta, origin, source_ref, added_at)
                 VALUES('kb1', '知识条目', '{}', 'snippet', '', 1)",
                [],
            )
            .unwrap();
        }

        // 列表只列会话记忆（知识条目另有「知识库」页，删除语义也不同）
        let all = store.list_all(None, None);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].key, "会话记忆");

        // 通用删除路径拒删知识条目，且拒绝后条目仍在
        let err = store.take_session_memory("知识条目").unwrap_err();
        assert!(err.contains("知识库"), "报错应说明归知识库管：{}", err);
        assert_eq!(store.count(), 2, "被拒的删除不能真的删掉条目");
        // 检索链路不受影响：知识条目照旧参与（共享检索是复用这张表的目的）
        assert_eq!(store.select_memories(Some("知识条目"), None, None).len(), 1);

        // 会话记忆照常删；不存在的 key 返回 None 而不是报错
        assert!(store.take_session_memory("会话记忆").unwrap().is_some());
        assert_eq!(store.count(), 1);
        assert!(store.take_session_memory("会话记忆").unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_normalize_tags() {
        assert_eq!(normalize_tags(""), "");
        assert_eq!(
            normalize_tags("db, 连接配置，数据库；中文"),
            "db,连接配置,数据库,中文"
        );
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
        // 命中权重：tags > key
        let base = rank_score(1, now, now, 0, 0);
        let key = rank_score(1, now, now, 1, 0);
        let tag = rank_score(1, now, now, 0, 1);
        assert!(base < key && key < tag, "key < tags 的命中权重应递增");
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

    #[test]
    fn test_save_memory_same_key_merges_and_never_overwrites() {
        let (store, dir) = temp_store();
        let base = "alpha beta gamma delta epsilon";

        let first = store.save_memory("k", base, "fact", false, "a,b");
        assert!(matches!(first, SaveOutcome::Inserted(_)));

        // 把访问时间压成一个可辨识的旧值：写入不应刷新它（编辑≠访问）
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE key_memories SET last_accessed_at = 1 WHERE key = 'k'",
                [],
            )
            .unwrap();
        }

        // 补充内容（现值被新值整体包含）→ 保留更完整的一份，tags 并集
        match store.save_memory("k", &format!("{} zeta", base), "fact", false, "b,c") {
            SaveOutcome::Merged {
                kind,
                entry,
                existing_key,
            } => {
                assert_eq!(kind, MergeKind::Superset);
                assert_eq!(existing_key, "k");
                assert!(
                    entry.value.ends_with("zeta"),
                    "更完整的一份应被保留: {}",
                    entry.value
                );
                assert_eq!(entry.tags, "a,b,c");
                assert_eq!(
                    entry.last_accessed_at, 1,
                    "写入不应刷新访问时间（只有检索命中累加）"
                );
            }
            other => panic!("expected Merged Superset, got {:?}", other),
        }

        // 仅空白差异：归一化等价 → Exact，不重复写入
        match store.save_memory(
            "k",
            "  alpha beta gamma delta epsilon zeta  ",
            "fact",
            false,
            "",
        ) {
            SaveOutcome::Merged { kind, entry, .. } => {
                assert_eq!(kind, MergeKind::Exact);
                assert_eq!(entry.tags, "a,b,c");
            }
            other => panic!("expected Merged Exact, got {:?}", other),
        }

        // 与现值无关：拒绝覆盖（本次不写入，信息不丢）
        let before = entry_of(&store, "k");
        match store.save_memory("k", "完全不同的内容", "fact", false, "") {
            SaveOutcome::Conflict { entry } => assert_eq!(entry.value, before.value),
            other => panic!("expected Conflict, got {:?}", other),
        }
        assert_eq!(
            entry_of(&store, "k").value,
            before.value,
            "冲突时不得改动内容"
        );
        assert_eq!(store.count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_memory_same_key_merges_similar_content() {
        let (store, dir) = temp_store();
        store.save_memory(
            "k",
            "t1 t2 t3 t4 t5\nt6 t7 t8 t9 t10 oldmarker",
            "fact",
            false,
            "",
        );
        // 同名 key、高相似但互不包含 → 整理合并，双方独有信息都保留
        match store.save_memory(
            "k",
            "t1 t2 t3 t4 t5\nt6 t7 t8 t9 t10 newmarker",
            "fact",
            false,
            "",
        ) {
            SaveOutcome::Merged { kind, entry, .. } => {
                assert_eq!(kind, MergeKind::Similar);
                assert!(
                    entry.value.contains("oldmarker"),
                    "旧信息不应丢失: {}",
                    entry.value
                );
                assert!(
                    entry.value.contains("newmarker"),
                    "新信息应写入: {}",
                    entry.value
                );
            }
            other => panic!("expected Merged Similar, got {:?}", other),
        }
        assert_eq!(store.count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_memory_exact_duplicate_merges_into_existing() {
        let (store, dir) = temp_store();

        store.save_memory("k1", "alpha beta gamma delta epsilon", "fact", false, "a");
        // 不同 key、同分类、归一化内容等价 → 合并，不新增
        match store.save_memory(
            "k2",
            "alpha  beta   gamma delta epsilon",
            "fact",
            false,
            "b",
        ) {
            SaveOutcome::Merged {
                entry,
                existing_key,
                kind,
            } => {
                assert_eq!(kind, MergeKind::Exact);
                assert_eq!(existing_key, "k1");
                assert_eq!(entry.key, "k1");
                assert_eq!(entry.tags, "a,b");
            }
            other => panic!("expected Merged Exact, got {:?}", other),
        }
        assert_eq!(store.count(), 1);
        assert!(store.search_memory(Some("k2"), None, None).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_memory_superset_keeps_longer_value() {
        let (store, dir) = temp_store();

        let base = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        store.save_memory("k1", base, "fact", false, "");
        // 新值是旧值的超集（多一行）→ 保留更长的那个
        match store.save_memory("k2", &format!("{}\nlambda", base), "fact", false, "") {
            SaveOutcome::Merged {
                entry,
                existing_key,
                kind,
            } => {
                assert_eq!(kind, MergeKind::Superset);
                assert_eq!(existing_key, "k1");
                assert!(entry.value.contains("lambda"));
                assert!(entry.value.contains("kappa"));
            }
            other => panic!("expected Merged Superset, got {:?}", other),
        }
        assert_eq!(store.count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_memory_similar_merges_lines() {
        let (store, dir) = temp_store();

        store.save_memory(
            "k1",
            "t1 t2 t3 t4 t5\nt6 t7 t8 t9 t10 oldmarker",
            "fact",
            false,
            "",
        );
        // 高相似但互不包含 → 整理合并，双方独有行都保留
        match store.save_memory(
            "k2",
            "t1 t2 t3 t4 t5\nt6 t7 t8 t9 t10 newmarker",
            "fact",
            false,
            "",
        ) {
            SaveOutcome::Merged {
                entry,
                existing_key,
                kind,
            } => {
                assert_eq!(kind, MergeKind::Similar);
                assert_eq!(existing_key, "k1");
                assert!(entry.value.contains("oldmarker"));
                assert!(entry.value.contains("newmarker"));
            }
            other => panic!("expected Merged Similar, got {:?}", other),
        }
        assert_eq!(store.count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_memory_dissimilar_inserts_new() {
        let (store, dir) = temp_store();

        store.save_memory("k1", "alpha beta gamma delta epsilon", "fact", false, "");
        match store.save_memory("k2", "one two three four five", "fact", false, "") {
            SaveOutcome::Inserted(entry) => assert_eq!(entry.key, "k2"),
            other => panic!("expected Inserted, got {:?}", other),
        }
        assert_eq!(store.count(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_memory_same_content_other_category_inserts() {
        let (store, dir) = temp_store();

        store.save_memory("k1", "alpha beta gamma delta epsilon", "fact", false, "");
        // 分类不同：内容等价也不合并，避免丢失分类语义
        match store.save_memory(
            "k2",
            "alpha beta gamma delta epsilon",
            "preference",
            false,
            "",
        ) {
            SaveOutcome::Inserted(entry) => assert_eq!(entry.key, "k2"),
            other => panic!("expected Inserted, got {:?}", other),
        }
        assert_eq!(store.count(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
