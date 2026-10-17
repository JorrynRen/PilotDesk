//! `enclave.db` 连接层：团队业务数据（任务/汇报/产出/文件/组织知识库）的唯一权威源。
//!
//! 范本取自 [`crate::api_agent::db::MemoryStore`]：独立库文件、WAL、幂等建表、`Arc<Mutex<Connection>>`。
//!
//! 两条硬约束：
//! 1. `Mutex<Connection>` 是**同步**锁 —— **绝不在持锁期间 `.await`**（先把网络调用做完，再进同步块写库）。
//! 2. 版本过高**拒绝打开、绝不 wipe** —— 本库会被整库复制到另一台机器（主机转移），
//!    静默重建等于删掉别人的团队资产。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

/// `enclave.db` 的独立表结构版本。
///
/// 与主库的 `db::init::SCHEMA_VERSION` **互相独立**：两库生命周期不同（主库随个人数据演进，
/// 本库随团队能力演进），共用版本号会让任一侧的升级牵动另一侧。
pub const ENCLAVE_SCHEMA_VERSION: i64 = 3;

/// 全量建表语句（全部 `IF NOT EXISTS`，可重复执行）。
///
/// 时间列统一用 **INTEGER unix 秒**（与 `MEMORY.db` 口径一致，避免 TEXT DATETIME 的比较歧义）。
/// 本批一次性写全，后续增量用 [`EnclaveStore::ensure_column`] 补列。
pub const ENCLAVE_SCHEMA_SQL: &str = r#"
-- ───────────────── 治理：只读快照 + 主机本地策略 ─────────────────
-- 组织快照（只读，来自平台 /organizations/mine；本地不写）
CREATE TABLE IF NOT EXISTS org_snapshot (
    org_id          TEXT PRIMARY KEY,
    name            TEXT NOT NULL DEFAULT '',
    owner_user_id   TEXT NOT NULL DEFAULT '',   -- 平台 owner（/organizations/mine 的 ownerId），即主机身份来源
    plan_key        TEXT NOT NULL DEFAULT '',
    plan_expires_at TEXT,
    seats           INTEGER NOT NULL DEFAULT 0,
    member_count    INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL DEFAULT '',
    my_role         TEXT NOT NULL DEFAULT 'member', -- 当前登录用户在该组织中的角色（平台 role 字段，只读）
    synced_at       INTEGER NOT NULL DEFAULT 0
);

-- 本机登录身份快照（平台 /me）：授权器据此确定 actor，审计据此留痕。单行。
CREATE TABLE IF NOT EXISTS local_identity (
    user_id      TEXT PRIMARY KEY,
    email        TEXT NOT NULL DEFAULT '',
    nickname     TEXT NOT NULL DEFAULT '',
    synced_at    INTEGER NOT NULL DEFAULT 0
);

-- 飞地本机设置（随库走：主机转移后沿用）。当前键：team_files_root
CREATE TABLE IF NOT EXISTS enclave_settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL DEFAULT 0
);

-- 成员快照（只读，来自平台；成员消失置 seat_status='removed'，不物理删）
CREATE TABLE IF NOT EXISTS member_snapshot (
    org_id       TEXT NOT NULL,
    user_id      TEXT NOT NULL,
    role         TEXT NOT NULL DEFAULT 'member',   -- owner / admin / member
    display_name TEXT NOT NULL DEFAULT '',
    email        TEXT NOT NULL DEFAULT '',
    seat_status  TEXT NOT NULL DEFAULT 'active',
    synced_at    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (org_id, user_id)
);

-- 数据出境策略（组织级总闸，默认不允许团队数据发给云端模型）
CREATE TABLE IF NOT EXISTS org_policy (
    org_id            TEXT PRIMARY KEY,
    allow_cloud_model INTEGER NOT NULL DEFAULT 0,
    updated_by        TEXT NOT NULL DEFAULT '',
    updated_at        INTEGER NOT NULL DEFAULT 0
);

-- 主机应急开关（语义：临时关闭飞地对外服务；本机使用不受影响）
CREATE TABLE IF NOT EXISTS host_emergency (
    org_id           TEXT PRIMARY KEY,
    enclave_disabled INTEGER NOT NULL DEFAULT 0,
    updated_by       TEXT NOT NULL DEFAULT '',
    updated_at       INTEGER NOT NULL DEFAULT 0
);

-- 策略 / 应急变更审计（仅主机本地，不涉及业务内容）
CREATE TABLE IF NOT EXISTS policy_audit (
    id         TEXT PRIMARY KEY,
    org_id     TEXT NOT NULL DEFAULT '',
    actor_id   TEXT NOT NULL DEFAULT '',
    action     TEXT NOT NULL,                      -- policy.allow_cloud_model / host.emergency / transfer.*
    detail     TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_policy_audit_org ON policy_audit(org_id, created_at);

-- 主机转移（本机侧迁移与接管记录）
CREATE TABLE IF NOT EXISTS host_transfer (
    id             TEXT PRIMARY KEY,
    org_id         TEXT NOT NULL,
    direction      TEXT NOT NULL,                  -- out（转出）/ in（接管）
    phase          TEXT NOT NULL,                  -- accepted / migrating / verified / completed / failed
    peer_device_id TEXT NOT NULL DEFAULT '',
    package_hash   TEXT NOT NULL DEFAULT '',
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_host_transfer_org ON host_transfer(org_id, created_at);

-- ───────────────── 业务：任务 / 汇报 / 文件 ─────────────────
-- 两层任务结构：parent_task_id 为空 = 父任务（容器）；非空 = 子任务（原子：1 负责人 + 1 交付物）
CREATE TABLE IF NOT EXISTS org_tasks (
    id             TEXT PRIMARY KEY,
    org_id         TEXT NOT NULL,
    parent_task_id TEXT,
    title          TEXT NOT NULL,
    description    TEXT NOT NULL DEFAULT '',
    creator_id     TEXT NOT NULL,
    assignee_id    TEXT,
    collaborators  TEXT NOT NULL DEFAULT '[]',     -- JSON 数组（用户 id）
    status         TEXT NOT NULL DEFAULT 'pending',-- pending/in_progress/submitted/done/archived
    priority       TEXT NOT NULL DEFAULT 'normal',
    due_date       INTEGER,
    started_at     INTEGER,
    completed_at   INTEGER,
    blocked_by     TEXT NOT NULL DEFAULT '[]',     -- JSON 数组（前置任务 id，纯 AND 语义）
    visibility     TEXT NOT NULL DEFAULT 'org',
    metadata       TEXT NOT NULL DEFAULT '{}',
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_org_tasks_parent ON org_tasks(org_id, parent_task_id);
CREATE INDEX IF NOT EXISTS idx_org_tasks_status ON org_tasks(org_id, status, updated_at);
CREATE INDEX IF NOT EXISTS idx_org_tasks_assignee ON org_tasks(assignee_id);

-- 完成汇报（四态：draft/submitted/confirmed/returned）
-- sedimented_at / sediment_error：产出沉淀到组织知识库的幂等守卫与失败原因
CREATE TABLE IF NOT EXISTS task_reports (
    id             TEXT PRIMARY KEY,
    task_id        TEXT NOT NULL,
    reporter_id    TEXT NOT NULL,
    content        TEXT NOT NULL DEFAULT '',
    materials      TEXT NOT NULL DEFAULT '[]',
    deliverables   TEXT NOT NULL DEFAULT '[]',
    status         TEXT NOT NULL DEFAULT 'draft',
    confirmed_by   TEXT,
    confirmed_at   INTEGER,
    sedimented_at  INTEGER,
    sediment_error TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_reports_task ON task_reports(task_id);

CREATE TABLE IF NOT EXISTS task_comments (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL,
    author_id    TEXT NOT NULL,
    content      TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'text',
    reply_to_id  TEXT,
    mentions     TEXT NOT NULL DEFAULT '[]',
    attachments  TEXT NOT NULL DEFAULT '[]',
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    deleted_at   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_task_comments_task ON task_comments(task_id, created_at);

CREATE TABLE IF NOT EXISTS report_feedback (
    id         TEXT PRIMARY KEY,
    report_id  TEXT NOT NULL,
    author_id  TEXT NOT NULL,
    action     TEXT NOT NULL,                     -- confirm / return / comment
    content    TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_report_feedback_report ON report_feedback(report_id);

CREATE TABLE IF NOT EXISTS file_assets (
    id           TEXT PRIMARY KEY,
    file_name    TEXT NOT NULL,
    file_path    TEXT NOT NULL,                   -- 相对 <app_root>/enclave/files 的文件名
    file_type    TEXT NOT NULL DEFAULT '',
    file_size    INTEGER NOT NULL DEFAULT 0,
    file_hash    TEXT NOT NULL DEFAULT '',        -- sha256（去重与完整性校验）
    storage_type TEXT NOT NULL DEFAULT 'local',
    owner_type   TEXT NOT NULL DEFAULT '',
    owner_id     TEXT NOT NULL DEFAULT '',
    metadata     TEXT NOT NULL DEFAULT '{}',
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_file_assets_hash ON file_assets(file_hash);

CREATE TABLE IF NOT EXISTS task_files (
    task_id     TEXT NOT NULL,
    file_id     TEXT NOT NULL,
    report_id   TEXT,
    uploaded_by TEXT NOT NULL DEFAULT '',
    uploaded_at INTEGER NOT NULL,
    PRIMARY KEY (task_id, file_id)
);

-- ───────────────── 业务：组织知识库 ─────────────────
-- 每组织一个隐式库（id = "kb-org-{orgId}"，is_implicit=1）：任务产出的归档地
CREATE TABLE IF NOT EXISTS knowledge_bases (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    scope         TEXT NOT NULL DEFAULT 'org',   -- personal / org
    owner_type    TEXT NOT NULL DEFAULT 'org',
    owner_id      TEXT NOT NULL DEFAULT '',
    is_implicit   INTEGER NOT NULL DEFAULT 0,
    field_schema  TEXT NOT NULL DEFAULT '[]',
    filter_rules  TEXT NOT NULL DEFAULT '[]',
    visibility    TEXT NOT NULL DEFAULT 'org',
    created_by    TEXT NOT NULL DEFAULT '',
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

-- 组织知识条目（正文直接落库：团队知识在 enclave.db，不依赖个人 MEMORY.db）
CREATE TABLE IF NOT EXISTS knowledge_base_memories (
    knowledge_base_id TEXT NOT NULL,
    memory_id         TEXT NOT NULL,             -- 确定性主键（如 "task:{task_id}:report:{report_id}"）
    title             TEXT NOT NULL DEFAULT '',
    content           TEXT NOT NULL DEFAULT '',
    tags              TEXT NOT NULL DEFAULT '',
    origin            TEXT NOT NULL DEFAULT '',  -- task_output / manual / ...
    source_ref        TEXT NOT NULL DEFAULT '',  -- task_id
    metadata          TEXT NOT NULL DEFAULT '{}',
    added_by          TEXT NOT NULL DEFAULT '',
    added_at          INTEGER NOT NULL,
    PRIMARY KEY (knowledge_base_id, memory_id)
);
CREATE INDEX IF NOT EXISTS idx_kb_memories_source ON knowledge_base_memories(source_ref);

CREATE TABLE IF NOT EXISTS knowledge_base_files (
    knowledge_base_id TEXT NOT NULL,
    file_id           TEXT NOT NULL,             -- 引用 file_assets 的同一原文，不重复拷贝
    added_at          INTEGER NOT NULL,
    PRIMARY KEY (knowledge_base_id, file_id)
);

-- ───────────────── 通知 / 资源级角色例外 ─────────────────
CREATE TABLE IF NOT EXISTS notifications (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL DEFAULT '',
    user_id     TEXT NOT NULL,
    type        TEXT NOT NULL,
    title       TEXT NOT NULL DEFAULT '',
    body        TEXT NOT NULL DEFAULT '',
    target_type TEXT NOT NULL DEFAULT '',
    target_id   TEXT NOT NULL DEFAULT '',
    read_at     INTEGER,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notifications_user ON notifications(user_id, read_at);

-- 资源级角色**显式例外**（自动绑定之外才写这张表；见设计 §8.3）
CREATE TABLE IF NOT EXISTS resource_roles (
    resource_type TEXT NOT NULL,                 -- task / kb
    resource_id   TEXT NOT NULL,
    org_id        TEXT NOT NULL DEFAULT '',
    user_id       TEXT NOT NULL,
    role          TEXT NOT NULL,                 -- assignee/collaborator/watcher / owner/editor/viewer
    granted_by    TEXT NOT NULL DEFAULT '',
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (resource_type, resource_id, user_id)
);

-- ───────────────── 组织知识库全文索引 ─────────────────
-- trigram：中文按任意子串命中（unicode61 会把整串汉字当一个 token，中文等于搜不到）
CREATE VIRTUAL TABLE IF NOT EXISTS kb_mem_fts USING fts5(
    knowledge_base_id UNINDEXED,
    memory_id         UNINDEXED,
    title,
    content,
    tags,
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS enclave_kb_mem_ai AFTER INSERT ON knowledge_base_memories BEGIN
    INSERT INTO kb_mem_fts(knowledge_base_id, memory_id, title, content, tags)
    VALUES (new.knowledge_base_id, new.memory_id, new.title, new.content, new.tags);
END;

CREATE TRIGGER IF NOT EXISTS enclave_kb_mem_au AFTER UPDATE ON knowledge_base_memories BEGIN
    DELETE FROM kb_mem_fts
     WHERE knowledge_base_id = old.knowledge_base_id AND memory_id = old.memory_id;
    INSERT INTO kb_mem_fts(knowledge_base_id, memory_id, title, content, tags)
    VALUES (new.knowledge_base_id, new.memory_id, new.title, new.content, new.tags);
END;

CREATE TRIGGER IF NOT EXISTS enclave_kb_mem_ad AFTER DELETE ON knowledge_base_memories BEGIN
    DELETE FROM kb_mem_fts
     WHERE knowledge_base_id = old.knowledge_base_id AND memory_id = old.memory_id;
END;
"#;

/// `enclave.db` 连接层。
pub struct EnclaveStore {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
    schema_version: i64,
}

impl EnclaveStore {
    /// 打开（或创建）`enclave.db`。
    ///
    /// 独立库文件 + WAL + 幂等建表；版本高于当前应用则**报错拒绝打开**（不 wipe）。
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("创建飞地数据目录失败 {}: {}", dir.display(), e))?;
        }

        let conn = Connection::open(path)
            .map_err(|e| format!("无法打开飞地数据库 {}: {}", path.display(), e))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             PRAGMA foreign_keys=ON;",
        )
        .map_err(|e| format!("初始化飞地数据库连接失败: {}", e))?;

        Self::migrate(&conn)?;

        let schema_version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(ENCLAVE_SCHEMA_VERSION);

        log::info!(
            "[Enclave] 飞地数据库已就绪: {} (schema v{})",
            path.display(),
            schema_version
        );

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
            schema_version,
        })
    }

    /// 版本检查 + 幂等建表 + 落版本号。
    fn migrate(conn: &Connection) -> Result<(), String> {
        let current: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);

        if current > ENCLAVE_SCHEMA_VERSION {
            return Err(format!(
                "飞地数据库版本（{}）高于当前应用（{}），已拒绝打开：请升级应用后再试（不会清空数据）",
                current, ENCLAVE_SCHEMA_VERSION
            ));
        }

        conn.execute_batch(ENCLAVE_SCHEMA_SQL)
            .map_err(|e| format!("初始化飞地表结构失败: {}", e))?;

        // v1 → v2：存量库的 `org_snapshot` 加不了新列（CREATE TABLE IF NOT EXISTS 不会改已有表），
        // 迁移期显式补列；新库建表语句里已含该列，这里会直接跳过。
        if current < 2 {
            Self::ensure_column(conn, "org_snapshot", "my_role", "TEXT NOT NULL DEFAULT 'member'")?;
        }

        if current != ENCLAVE_SCHEMA_VERSION {
            conn.execute_batch(&format!("PRAGMA user_version = {}", ENCLAVE_SCHEMA_VERSION))
                .map_err(|e| format!("写入飞地数据库版本号失败: {}", e))?;
        }
        Ok(())
    }

    /// 借出连接执行同步操作。
    ///
    /// **调用方绝不可在这个闭包里 `.await`**：`Mutex<Connection>` 是同步锁，
    /// 持锁跨 await 会阻塞后台任务、并可能把 tokio 单线程调度器拖死。
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "飞地数据库连接已损坏（锁中毒）".to_string())?;
        f(&conn)
    }

    /// 缺列则补列（SQLite 支持带默认值的 `ADD COLUMN`）。
    ///
    /// 每次 `ENCLAVE_SCHEMA_VERSION` 递增时，在 [`Self::migrate`] 里对存量库调用。
    pub fn ensure_column(
        conn: &Connection,
        table: &str,
        column: &str,
        ddl: &str,
    ) -> Result<(), String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .map_err(|e| format!("读取表 {} 结构失败: {}", table, e))?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        if cols.iter().any(|c| c == column) {
            return Ok(());
        }
        conn.execute(
            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, ddl),
            [],
        )
        .map_err(|e| format!("为表 {} 补列 {} 失败: {}", table, column, e))?;
        Ok(())
    }

    /// 数据库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 打开时确定的结构版本。
    pub fn schema_version(&self) -> i64 {
        self.schema_version
    }
}
