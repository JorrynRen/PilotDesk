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
pub const ENCLAVE_SCHEMA_VERSION: i64 = 13;

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
    viewer_id       TEXT NOT NULL DEFAULT '',      -- 该快照对应的登录身份（user.id）：换账号后旧行自动失效
    host_device_id  TEXT NOT NULL DEFAULT '',      -- 平台登记的主机设备 id（空 = 尚未登记）
    host_device_name TEXT NOT NULL DEFAULT '',     -- 主机设备名（展示用）
    host_addr       TEXT NOT NULL DEFAULT '',      -- 平台下发的主机局域网地址（广播被禁时的连接兜底）
    host_port       INTEGER NOT NULL DEFAULT 0,    -- 平台下发的主机端口（0 = 未上报）
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
    target_kb_id   TEXT NOT NULL DEFAULT '',       -- 任务产出的目标库；空 = 默认库
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
    progress_id TEXT,                              -- 关联的过程记录（task_comments.id）
    uploaded_by TEXT NOT NULL DEFAULT '',
    uploaded_at INTEGER NOT NULL,
    PRIMARY KEY (task_id, file_id)
);

-- 任务文件挂载（v6 起）：同一份原件可在同一任务内**多处挂载**
-- scope = 'task'（任务资料） / 'progress:<进度记录id>' / 'report:<汇报id>'。
-- task_files 仅作为 v5 及以前的历史表保留，不再读写。
CREATE TABLE IF NOT EXISTS task_file_links (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL,
    file_id     TEXT NOT NULL,
    scope       TEXT NOT NULL,
    report_id   TEXT,
    progress_id TEXT,
    uploaded_by TEXT NOT NULL DEFAULT '',
    uploaded_at INTEGER NOT NULL,
    UNIQUE (task_id, file_id, scope)
);
CREATE INDEX IF NOT EXISTS idx_task_file_links_task ON task_file_links(task_id, uploaded_at);
CREATE INDEX IF NOT EXISTS idx_task_file_links_scope ON task_file_links(report_id, progress_id);

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
    memory_id         TEXT NOT NULL,             -- 确定性主键：一个任务一条（"task:{task_id}"）
    title             TEXT NOT NULL DEFAULT '',
    content           TEXT NOT NULL DEFAULT '',
    tags              TEXT NOT NULL DEFAULT '',
    origin            TEXT NOT NULL DEFAULT '',  -- task_output / manual / ...
    source_ref        TEXT NOT NULL DEFAULT '',  -- task_id
    metadata          TEXT NOT NULL DEFAULT '{}',
    added_by          TEXT NOT NULL DEFAULT '',
    added_at          INTEGER NOT NULL,          -- 首次沉淀时间
    updated_at        INTEGER NOT NULL DEFAULT 0,-- 最近一次覆盖时间（0 = 存量行，读时回退 added_at）
    stale             INTEGER NOT NULL DEFAULT 0,-- 1 = 来源任务已恢复汇报，等待重新确认
    PRIMARY KEY (knowledge_base_id, memory_id)
);
CREATE INDEX IF NOT EXISTS idx_kb_memories_source ON knowledge_base_memories(source_ref);

-- 条目 ↔ 交付物：必须是「某条知识」的附件，而不是整个库的附件集合，
-- 否则条目更新/撤下时无法定位该删哪些行（v7 起主键含 memory_id）。
CREATE TABLE IF NOT EXISTS knowledge_base_files (
    knowledge_base_id TEXT NOT NULL,
    memory_id         TEXT NOT NULL,
    file_id           TEXT NOT NULL,             -- 引用 file_assets 的同一原文，不重复拷贝
    added_at          INTEGER NOT NULL,
    PRIMARY KEY (knowledge_base_id, memory_id, file_id)
);

-- 文件之间的关系（团队库）：正文—附件 / 新版—旧版。
-- 与个人库 `kb_file_relations` 语义同构，但**按库隔离**：关系只在本库内成立
-- （同一原件被多个库引用时，A 库的标注不会带进 B 库）。纯标注：不删文件、不影响检索。
CREATE TABLE IF NOT EXISTS knowledge_base_file_relations (
    knowledge_base_id TEXT NOT NULL,
    kind              TEXT NOT NULL,             -- attachment（正文→附件）/ supersedes（新版→旧版）
    from_file_id      TEXT NOT NULL,
    to_file_id        TEXT NOT NULL,
    created_by        TEXT NOT NULL DEFAULT '',
    created_at        INTEGER NOT NULL,
    PRIMARY KEY (knowledge_base_id, kind, from_file_id, to_file_id)
);
CREATE INDEX IF NOT EXISTS idx_kb_file_rel_to ON knowledge_base_file_relations(to_file_id);

-- 投喂「待整理」队列：内容已收进主机，但当前没有可用模型做结构化整理。
-- 与个人库同构（kb_ingest_queue）；区别是这里多了 org_id，且只有主机消费。
CREATE TABLE IF NOT EXISTS kb_ingest_queue (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL,
    kb_id       TEXT NOT NULL,
    kind        TEXT NOT NULL,                   -- snippet / url / file
    source_ref  TEXT NOT NULL,
    payload     TEXT NOT NULL DEFAULT '{}',
    status      TEXT NOT NULL DEFAULT 'pending',  -- pending / running / failed
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    UNIQUE(org_id, kb_id, kind, source_ref)
);
CREATE INDEX IF NOT EXISTS idx_kb_ingest_queue_status ON kb_ingest_queue(org_id, status, created_at);

-- 知识库「待确认」候选：模型产出（AI 生成）先落这里，人工核对后采纳才进库。
-- 与个人库 kb_candidates 同构；团队库没有候选层时，模型产出只能直写共享库，风险不可接受。
CREATE TABLE IF NOT EXISTS kb_candidates (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL,
    kb_id       TEXT NOT NULL,
    title       TEXT NOT NULL DEFAULT '',
    content     TEXT NOT NULL DEFAULT '',
    tags        TEXT NOT NULL DEFAULT '',
    metadata    TEXT NOT NULL DEFAULT '{}',
    origin      TEXT NOT NULL DEFAULT 'ai',
    created_by  TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_kb_candidates_org ON kb_candidates(org_id, kb_id, created_at);

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
        // v3 → v4：过程记录附件
        if current < 4 {
            Self::ensure_column(conn, "task_files", "progress_id", "TEXT")?;
        }
        // v4 → v5：组织快照绑定登录身份（换账号后旧快照不得继续用于权限判定）。
        // 存量行补列后 viewer_id = ''，在下一次快照同步前一律视为无效（默认拒绝）。
        if current < 5 {
            Self::ensure_column(conn, "org_snapshot", "viewer_id", "TEXT NOT NULL DEFAULT ''")?;
        }
        // v5 → v6：附件改为「挂载关系」表（同一原件可多处挂载）；迁移历史 task_files 行。
        // task_files 保留不再读写，后续版本再视情况清理。
        if current < 6 {
            conn.execute_batch(
                "INSERT OR IGNORE INTO task_file_links
                    (id, task_id, file_id, scope, report_id, progress_id, uploaded_by, uploaded_at)
                 SELECT lower(hex(randomblob(16))), task_id, file_id,
                        CASE
                          WHEN progress_id IS NOT NULL AND progress_id <> '' THEN 'progress:' || progress_id
                          WHEN report_id IS NOT NULL AND report_id <> '' THEN 'report:' || report_id
                          ELSE 'task'
                        END,
                        report_id, progress_id, uploaded_by, uploaded_at
                 FROM task_files",
            )
            .map_err(|e| format!("迁移附件挂载关系失败: {}", e))?;
        }
        // v6 → v7：知识条目补齐「更新时间 / 待更新」；知识附件从「库级集合」改为「条目级」。
        // 旧表主键只有 (库, 文件)，无法定位文件属于哪条知识 → 必须重建表并回填 memory_id。
        if current < 7 {
            Self::ensure_column(
                conn,
                "knowledge_base_memories",
                "updated_at",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::ensure_column(
                conn,
                "knowledge_base_memories",
                "stale",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            if !Self::has_column(conn, "knowledge_base_files", "memory_id")? {
                conn.execute_batch(
                    "CREATE TABLE knowledge_base_files_v7 (
                         knowledge_base_id TEXT NOT NULL,
                         memory_id         TEXT NOT NULL,
                         file_id           TEXT NOT NULL,
                         added_at          INTEGER NOT NULL,
                         PRIMARY KEY (knowledge_base_id, memory_id, file_id)
                     );
                     -- 回填：按「文件挂载到该任务某次汇报」反查所属知识条目；
                     -- 查不到归属的旧行直接丢弃（它们是当前不可见、也无法维护的悬空引用）。
                     INSERT INTO knowledge_base_files_v7
                         (knowledge_base_id, memory_id, file_id, added_at)
                     SELECT kbf.knowledge_base_id,
                            COALESCE((
                              SELECT m.memory_id
                              FROM knowledge_base_memories m
                              JOIN task_file_links l
                                ON l.task_id = m.source_ref
                               AND l.file_id = kbf.file_id
                               AND l.report_id IS NOT NULL
                              WHERE m.knowledge_base_id = kbf.knowledge_base_id
                              LIMIT 1
                            ), ''),
                            kbf.file_id,
                            kbf.added_at
                     FROM knowledge_base_files kbf;
                     DELETE FROM knowledge_base_files_v7 WHERE memory_id = '';
                     DROP TABLE knowledge_base_files;
                     ALTER TABLE knowledge_base_files_v7 RENAME TO knowledge_base_files;",
                )
                .map_err(|e| format!("迁移知识库附件归属失败: {}", e))?;
            }
        }
        // v7 → v8：组织快照记录「当前登记的主机设备」。
        // 同一 owner 账号可能在多台设备登录；只有设备 id 与登记一致的本机才对外服务，
        // 避免第二台设备用空库启动主机服务（见 host::device_id / org::host_org_ids）。
        if current < 8 {
            Self::ensure_column(
                conn,
                "org_snapshot",
                "host_device_id",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            Self::ensure_column(
                conn,
                "org_snapshot",
                "host_device_name",
                "TEXT NOT NULL DEFAULT ''",
            )?;
        }
        if current < 9 {
            Self::ensure_column(conn, "org_snapshot", "host_addr", "TEXT NOT NULL DEFAULT ''")?;
            Self::ensure_column(conn, "org_snapshot", "host_port", "INTEGER NOT NULL DEFAULT 0")?;
        }
        // v11 → v12：任务产出可选归档到自定义库（空 = 默认库，存量任务行为不变）
        if current < 12 {
            Self::ensure_column(conn, "org_tasks", "target_kb_id", "TEXT NOT NULL DEFAULT ''")?;
        }
        // v12 → v13：知识库文件之间的关系（正文—附件 / 新版—旧版）。
        // 新表由全量建表语句幂等创建，无存量数据需要回填，这里只留版本语义。

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
        if Self::has_column(conn, table, column)? {
            return Ok(());
        }
        conn.execute(
            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, ddl),
            [],
        )
        .map_err(|e| format!("为表 {} 补列 {} 失败: {}", table, column, e))?;
        Ok(())
    }

    /// 判断列是否存在（主键变更等需要「重建表」的迁移据此决定是否已执行）。
    fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .map_err(|e| format!("读取表 {} 结构失败: {}", table, e))?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        Ok(cols.iter().any(|c| c == column))
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
