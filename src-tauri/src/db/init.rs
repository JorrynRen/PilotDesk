use crate::utils::errors::AppError;
use crate::utils::paths::db_path;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::fs;

/// 对外 schema 版本：v5 起从"版本不一致即整库重置"改为增量迁移，历史库一律保留；
/// 打开到比当前更新的库时明确报错，绝不 wipe。v1-v4 为预发布期整库重置，无独立旧库留存。
/// 约定：每次修改 FINAL_SCHEMA_SQL 都必须把 SCHEMA_VERSION +1，
/// 否则 user_version 相等的旧库走快速路径、不会补齐缺表/缺列。
///
/// v10：物理移除旧表 `api_providers`——其数据已并入 `agents(transport='http')` + `agent_secrets`，
/// 终态脚本不再建该表；存量库在迁移时先一次性回填、再 `DROP`（见 `backfill_api_providers_into_agents`）。
pub const SCHEMA_VERSION: i64 = 10;

/// 当前终态建表脚本（唯一 schema 定义）。依据既有生产库 schema 固化：
/// - 全部使用 IF NOT EXISTS，可每次启动安全执行；
/// - v5 起老库按列增量补齐（见 migrate_schema/ensure_column），新列一律带 DEFAULT 回填历史行。
/// 额外的基础兜底表（file_history 等）仍在 init_db 中执行。
pub const FINAL_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS agents (
            agent_type TEXT PRIMARY KEY,
            display_name TEXT NOT NULL DEFAULT '',
            description TEXT NOT NULL DEFAULT '',
            cli_command TEXT NOT NULL DEFAULT '',
            npm_package TEXT,
            pip_package TEXT,
            version_flag TEXT NOT NULL DEFAULT '--version',
            install_cmd TEXT NOT NULL DEFAULT '',
            uninstall_cmd TEXT NOT NULL DEFAULT '',
            update_cmd TEXT NOT NULL DEFAULT '',
            version_cmd TEXT NOT NULL DEFAULT '',
            latest_version_cmd TEXT NOT NULL DEFAULT '',
            run_cmd_template TEXT NOT NULL DEFAULT '',
            output_parser TEXT NOT NULL DEFAULT 'raw-text',
            output_filter_regex TEXT NOT NULL DEFAULT '',
            version_pattern TEXT NOT NULL DEFAULT 'v?(\d+\.\d+\.\d+[\w.-]*)',
            supports_session_continuity INTEGER NOT NULL DEFAULT 0,
            session_id_source TEXT NOT NULL DEFAULT 'none',
            session_id_event_type TEXT NOT NULL DEFAULT '',
            session_id_field TEXT NOT NULL DEFAULT '',
            resume_arg_template TEXT NOT NULL DEFAULT '',
            skills_dir TEXT NOT NULL DEFAULT '',
            skill_entry_file TEXT NOT NULL DEFAULT 'SKILL.md',
            skill_display_mode TEXT NOT NULL DEFAULT 'collection',
            color TEXT NOT NULL DEFAULT '#6366F1',
            icon TEXT NOT NULL DEFAULT '\U0001f916',
            sort_order INTEGER DEFAULT 0,
            is_enabled INTEGER DEFAULT 1,
            is_builtin INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        , version TEXT NOT NULL DEFAULT ''
        -- v9：接入方式与连接参数（设计文档 §4.2）。
        -- transport 默认 'subprocess' —— 现有 CLI Agent 行为**零变化**；
        -- conn 为 transport 专属参数（JSON），**不含密钥**；密钥只留 secret_ref 指向 agent_secrets。
        , transport TEXT NOT NULL DEFAULT 'subprocess'
        , conn TEXT
        , secret_ref TEXT);
-- 凭据独立存储（设计文档 §4.3）：密钥不进 agents.conn，agents.secret_ref 指向本表。
-- 新表用 IF NOT EXISTS 每次启动即补齐，老库无需 schema 版本迁移。
CREATE TABLE IF NOT EXISTS agent_secrets (
            id TEXT PRIMARY KEY,
            secret TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
CREATE TABLE IF NOT EXISTS api_usage_log (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id        TEXT NOT NULL,
            provider          TEXT NOT NULL DEFAULT '',
            model             TEXT NOT NULL DEFAULT '',
            api_format        TEXT NOT NULL DEFAULT '',
            prompt_tokens     INTEGER NOT NULL DEFAULT 0,
            completion_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens      INTEGER NOT NULL DEFAULT 0,
            cached_tokens     INTEGER NOT NULL DEFAULT 0, -- 总缓存 = cache_read_tokens + cache_write_tokens（兼容既有汇总/展示）
            cache_read_tokens  INTEGER NOT NULL DEFAULT 0, -- 缓存命中读取 token
            cache_write_tokens INTEGER NOT NULL DEFAULT 0, -- 缓存写入 token
            created_at        INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS groupchat_participants (
            id              TEXT NOT NULL,
            room_id         TEXT NOT NULL,
            participant_type TEXT NOT NULL,
            agent_config    TEXT NOT NULL DEFAULT '{}',
            display_name    TEXT NOT NULL DEFAULT '',
            system_role     TEXT NOT NULL DEFAULT '',
            status          TEXT NOT NULL DEFAULT 'active',
            PRIMARY KEY (room_id, id),
            FOREIGN KEY (room_id) REFERENCES groupchat_rooms(id));
CREATE TABLE IF NOT EXISTS groupchat_rooms (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL DEFAULT '',
            topic         TEXT NOT NULL DEFAULT '',
            status        TEXT NOT NULL DEFAULT 'idle',
            strategy      TEXT NOT NULL DEFAULT 'round_robin',
            max_rounds    INTEGER NOT NULL DEFAULT 5,
            max_parallel  INTEGER NOT NULL DEFAULT 2,
            director_id   TEXT,
            current_task_id TEXT,
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL,
            goal_notes    TEXT NOT NULL DEFAULT '[]',
            allow_auto_cli INTEGER NOT NULL DEFAULT 1,
            output_dir    TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS groupchat_stances (
            room_id      TEXT NOT NULL,
            participant_id TEXT NOT NULL,
            stance       TEXT NOT NULL DEFAULT '',
            attitude     TEXT NOT NULL DEFAULT '',
            updated_at   INTEGER NOT NULL,
            PRIMARY KEY (room_id, participant_id));
CREATE TABLE IF NOT EXISTS inspiration_tags (
            inspiration_id TEXT NOT NULL,
            tag TEXT NOT NULL,
            PRIMARY KEY (inspiration_id, tag),
            FOREIGN KEY (inspiration_id) REFERENCES inspirations(id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS inspirations (
            id TEXT PRIMARY KEY,
            icon TEXT NOT NULL DEFAULT '💡',
            title TEXT NOT NULL,
            content TEXT NOT NULL DEFAULT '',
            source_agent TEXT DEFAULT 'manual',
            is_favorite INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            market_id TEXT);
CREATE TABLE IF NOT EXISTS install_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            message TEXT NOT NULL DEFAULT '',
            level TEXT NOT NULL DEFAULT 'info' CHECK(level IN ('info', 'warn', 'error', 'success')));
CREATE TABLE IF NOT EXISTS room_events (
    seq            INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id        TEXT NOT NULL,
    kind           TEXT NOT NULL,
    payload        TEXT NOT NULL DEFAULT '{}',
    model_visible  INTEGER NOT NULL DEFAULT 1,
    created_at     INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS session_events (
    seq            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id     TEXT NOT NULL,
    kind           TEXT NOT NULL,
    payload        TEXT NOT NULL DEFAULT '{}',
    model_visible  INTEGER NOT NULL DEFAULT 1,
    created_at     INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS "sessions" (
            id TEXT PRIMARY KEY,
            agent_type TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL DEFAULT '',
            cwd TEXT DEFAULT '',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_message_preview TEXT DEFAULT '',
            message_count INTEGER DEFAULT 0,
            status TEXT DEFAULT 'active' CHECK(status IN ('active', 'archived')),
            api_provider TEXT,
            api_model TEXT,
            agent_session_id TEXT
        , origin TEXT
        , temperature REAL DEFAULT 0.7, max_tokens INTEGER DEFAULT NULL);
CREATE TABLE IF NOT EXISTS workflow_definitions (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            version TEXT NOT NULL DEFAULT '1.0.0',
            description TEXT NOT NULL DEFAULT '',
            trigger TEXT NOT NULL DEFAULT '{"triggerType":"manual"}',
            stages TEXT NOT NULL DEFAULT '[]',
            input_schema TEXT,
            output_schema TEXT,
            icon TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1
        , deleted_at INTEGER);
CREATE TABLE IF NOT EXISTS workflow_events (
    seq            INTEGER PRIMARY KEY AUTOINCREMENT,
    execution_id   TEXT NOT NULL,
    kind           TEXT NOT NULL,
    payload        TEXT NOT NULL DEFAULT '{}',
    model_visible  INTEGER NOT NULL DEFAULT 1,
    created_at     INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS workflow_schedules (
            id TEXT PRIMARY KEY,
            workflow_id TEXT NOT NULL REFERENCES workflow_definitions(id) ON DELETE CASCADE,
            cron_expression TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            input_data TEXT DEFAULT '{}',
            last_run_at INTEGER,
            next_run_at INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS workflow_versions (
            id TEXT PRIMARY KEY,
            workflow_id TEXT NOT NULL REFERENCES workflow_definitions(id) ON DELETE CASCADE,
            version INTEGER NOT NULL,
            snapshot TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            origin TEXT NOT NULL DEFAULT 'manual',
            UNIQUE(workflow_id, version));
CREATE INDEX IF NOT EXISTS idx_api_usage_session ON api_usage_log (session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_install_logs_time ON install_logs(timestamp);
CREATE INDEX IF NOT EXISTS idx_room_events_room_seq ON room_events (room_id, seq);
CREATE INDEX IF NOT EXISTS idx_session_events_session_seq ON session_events (session_id, seq);
CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_type, updated_at);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status, updated_at);
CREATE INDEX IF NOT EXISTS idx_wf_schedule_next ON workflow_schedules(next_run_at, enabled);
CREATE INDEX IF NOT EXISTS idx_wf_versions_workflow ON workflow_versions(workflow_id, version DESC);
CREATE INDEX IF NOT EXISTS idx_workflow_defs_enabled ON workflow_definitions(enabled, updated_at);
CREATE INDEX IF NOT EXISTS idx_workflow_events_exec ON workflow_events (execution_id, seq);
"#;

pub type DbPool = Pool<SqliteConnectionManager>;

pub fn init_db() -> Result<DbPool, AppError> {
    let db_path = db_path();
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let manager = SqliteConnectionManager::file(&db_path).with_init(|c| {
        // WAL 下同一时刻只有一个写者：未设 busy_timeout 时，并发写（多实例执行 + 群聊 Actor +
        // 编辑器保存）会立刻返回 "database is locked"，而节点状态/事件的写入失败只记 warn。
        // 结果是"节点记录凭空消失"——界面表现为节点一直停在运行中、执行记录缺行。
        c.busy_timeout(std::time::Duration::from_secs(5))
    });
    let pool = Pool::builder().max_size(8).build(manager)?;

    // 数据库操作在单连接上进行
    let conn = pool.get()?;

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;",
    )?;

    // ── schema 增量迁移（v5 起）──
    // 历史库一律保留：旧版库逐项补列升级；比当前更新的库（由更新版本应用创建/打开过）
    // 明确报错拒绝打开，绝不整库重置。先执行幂等终态建表脚本兜底缺表，再补老库缺列。
    migrate_schema(&conn)?;

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_inspirations_favorite ON inspirations(is_favorite, updated_at);
        CREATE INDEX IF NOT EXISTS idx_inspirations_tags ON inspiration_tags(tag);"
    )?;

    // Agent 文件修改历史（write_file / edit_file 的撤销备份）
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS file_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL DEFAULT '',
            file_path TEXT NOT NULL,
            backup_content TEXT,
            file_existed INTEGER DEFAULT 1,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_file_history_session ON file_history(session_id, created_at DESC);"
    )?;

    // 云同步本地元数据（个人向跨设备同步，见 commands/cloud_sync.rs）：
    // 每个被同步对象一行——server_version = 本地已知的「远端版本」（新建为 0），
    // dirty = 本地有未推送改动（快速提示，非唯一判据），
    // last_synced_hash = 上次成功同步时本地内容的规范化哈希（判「本地是否真改过」的唯一可靠判据），
    // tombstone = 是否墓碑（本地彻底删除后阻止该 key 被 pull 复活；1 = 是），
    // tombstone_pushed = 墓碑对应的「删除意图」是否已送达云端（1 = 已送达，不再重复推送；
    //   0 = 尚未送达，作为补送候选继续尝试）。三种状态组合见 commands/cloud_sync.rs 模块注释。
    // 作为「基础兜底表」在此创建（与 file_history 同处）：新表用 IF NOT EXISTS 每次启动即补齐，
    // 老库无需 schema 版本迁移；只有「给旧表补列」才需要 SCHEMA_VERSION +1（见 migrate_schema）。
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_state (
            kind TEXT NOT NULL,
            object_key TEXT NOT NULL,
            server_version INTEGER NOT NULL DEFAULT 0,
            local_updated_at INTEGER NOT NULL DEFAULT 0,
            dirty INTEGER NOT NULL DEFAULT 0,
            last_synced_hash TEXT,
            tombstone INTEGER NOT NULL DEFAULT 0,
            tombstone_pushed INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (kind, object_key)
        );",
    )?;
    // 老库补列：既有 sync_state 表（无 last_synced_hash / tombstone / tombstone_pushed）在此按既有
    // ensure_column 做法幂等补齐——只加列、不改数据、不随 SCHEMA_VERSION 门控（同 workflow_definitions.deleted_at）。
    ensure_column(&conn, "sync_state", "last_synced_hash", "TEXT")?;
    ensure_column(&conn, "sync_state", "tombstone", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(
        &conn,
        "sync_state",
        "tombstone_pushed",
        "INTEGER NOT NULL DEFAULT 0",
    )?;

    // 老库补列：agents 增加 transport / conn / secret_ref（设计文档 §4.2 / §4.3）。
    // 与上面 sync_state 同一做法：只加列、带 DEFAULT 回填历史行、不随 SCHEMA_VERSION 门控。
    ensure_column(
        &conn,
        "agents",
        "transport",
        "TEXT NOT NULL DEFAULT 'subprocess'",
    )?;
    ensure_column(&conn, "agents", "conn", "TEXT")?;
    ensure_column(&conn, "agents", "secret_ref", "TEXT")?;

    // 4a：把既有 API 供应商一次性迁移成 `api:<id>` 的 http 接入 + 凭据副本（标记守卫、只跑一次）。
    backfill_api_providers_into_agents(&conn)?;

    // 4b（v10）：迁移完成后物理移除旧表 `api_providers`。
    // 顺序很关键——必须先回填再删（老库首次升级时数据还在旧表里）；
    // 已迁移过的库（标记已置位）回填直接空跑，这里仅清理遗留表。
    // 全新库无该表，`IF EXISTS` 空跑。
    conn.execute("DROP TABLE IF EXISTS api_providers", [])?;

    // ===== 种子数据（INSERT OR IGNORE，已有数据不覆盖） =====
    let now = crate::utils::now();

    // Agent 种子数据
    let agent_seeds: Vec<(&str, &str, &str, &str, Option<&str>, Option<&str>, &str, &str, &str, &str, &str, &str, &str, &str, i64, &str, &str, &str, &str, &str, &str, &str, &str, i64)> = vec![
        ("claude", "Claude Code", "Anthropic 官方 AI 编程助手，支持代码生成、调试、重构",
         "claude", Some("@anthropic-ai/claude-code"), None,
         "npm install -g @anthropic-ai/claude-code",
         "npm uninstall -g @anthropic-ai/claude-code",
         "claude update",
         "claude --version",
         "npm view @anthropic-ai/claude-code version",
         "claude -p --output-format stream-json --verbose --dangerously-skip-permissions -- {message}",
         "json-stream", "", 1, "stdout-json", "system", "session_id", "claude --resume {session_id} -p --output-format stream-json --verbose --dangerously-skip-permissions -- {message}",
         "~/.claude/skills/", "collection", "#3B82F6", "file:claude_icon.ico", 2),
        ("codex", "Codex CLI", "OpenAI 出品的终端 AI 编程助手",
         "codex", Some("@openai/codex"), None,
         "npm install -g @openai/codex",
         "npm uninstall -g @openai/codex",
         "codex update",
         "codex --version",
         "npm view @openai/codex version",
         "codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -- {message}",
         "json-stream", "", 1, "stdout-json", "thread.started", "thread_id", "codex exec resume {session_id} --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -- {message}",
         "~/.codex/skills/", "collection", "#F59E0B", "file:codex_icon.ico", 3),
        ("hermes", "Hermes Agent", "轻量级通用 AI Agent",
         "hermes", None, Some("hermes-agent"),
         "pip install hermes-agent",
         "pip uninstall hermes-agent -y",
         "hermes update",
         "hermes --version",
         "powershell -NoProfile -Command (Invoke-RestMethod https://pypi.org/pypi/hermes-agent/json).info.version",
         "hermes chat --query={message} -Q",
         "ansi-text",
         "^(Initializing agent|Resume this session|Session:|Duration:|Messages:|Query:)", 1,
         "stderr-text", "", "session_id: ", "hermes --resume {session_id} chat --query={message} -Q",
         "~/AppData/Local/hermes/skills/", "collection", "#8B5CF6", "file:hermes_icon.ico", 1),
    ];

    for (
        agent_type,
        display_name,
        description,
        cli_command,
        npm_package,
        pip_package,
        install_cmd,
        uninstall_cmd,
        update_cmd,
        version_cmd,
        latest_version_cmd,
        run_cmd_template,
        output_parser,
        output_filter_regex,
        supports_session_continuity,
        session_id_source,
        session_id_event_type,
        session_id_field,
        resume_arg_template,
        skills_dir,
        skill_display_mode,
        color,
        icon,
        sort_order,
    ) in agent_seeds
    {
        conn.execute(
            "INSERT OR IGNORE INTO agents (agent_type, display_name, description, cli_command, npm_package, pip_package,
             install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
             output_parser, output_filter_regex, version_pattern, supports_session_continuity,
             session_id_source, session_id_event_type, session_id_field, resume_arg_template,
             skills_dir, skill_entry_file, skill_display_mode,
             color, icon, sort_order, is_enabled, is_builtin, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
             'v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)', ?15, ?16, ?17, ?18, ?19, ?20, 'SKILL.md', ?21, ?22, ?23, ?24, 1, 1, '1.0', ?25, ?25)",
            rusqlite::params![agent_type, display_name, description, cli_command, npm_package, pip_package,
                install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
                output_parser, output_filter_regex, supports_session_continuity,
                session_id_source, session_id_event_type, session_id_field, resume_arg_template,
                skills_dir, skill_display_mode,
                color, icon, sort_order, now],
        )?;
    }

    // App Settings 种子数据
    let workspace_default = crate::utils::paths::app_root_dir()
        .to_string_lossy()
        .into_owned();
    let settings_seeds: Vec<(String, String)> = vec![
        ("mode_prompt_native".into(), String::new()),
        ("mode_prompt_fast".into(), "快速简洁回答，直接给出结论，无需详细解释推理过程".into()),
        ("mode_prompt_think".into(), "逐步分析推理，详细解释你的思路和过程，给出完整的推理链".into()),
        ("mode_prompt_expert".into(), "以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案".into()),
        ("mode_prompt_plan".into(), "先分析需求并制定清晰的分步执行计划，先向用户呈现完整计划等待确认，未经确认不要执行任何操作。".into()),
        ("pilotdesk-workspace".into(), workspace_default),
        ("workflow_max_concurrency".into(), "10".into()),
        ("workflow_max_subflow_depth".into(), "3".into()),
    ];
    for (key, value) in &settings_seeds {
        conn.execute(
            "INSERT OR IGNORE INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, value, now],
        )?;
    }

    // 权限规则默认种子：首启写入默认清单（原增量迁移的职责移交至此），已有自定义规则不被覆盖。
    let rules_json =
        serde_json::to_string(&crate::api_agent::agent_loop::default_permission_rules())
            .unwrap_or_else(|_| "{}".to_string());
    conn.execute(
        "INSERT OR IGNORE INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            crate::commands::permission::PERMISSION_RULES_KEY,
            rules_json,
            now
        ],
    )?;

    Ok(pool)
}

/// 幂等 schema 迁移入口（v5 起）。三步：
/// 1. current > SCHEMA_VERSION：拒绝打开（库来自更新版本的应用），绝不 wipe；
/// 2. 执行 FINAL_SCHEMA_SQL（IF NOT EXISTS，可重复执行）；
/// 3. current < SCHEMA_VERSION：逐项迁移补列，成功后把 user_version 提升到 SCHEMA_VERSION。
fn migrate_schema(conn: &Connection) -> Result<(), AppError> {
    let current_version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap_or(0);
    if current_version > SCHEMA_VERSION {
        return Err(AppError::Config(format!(
            "数据库 schema 版本（v{}）高于当前应用支持的最高版本（v{}）：该数据库由更新版本的 PilotDesk 创建或打开过。\
             为避免数据损坏，本次启动拒绝打开该库；请升级应用到最新版本后再试（不会清除任何数据）。",
            current_version, SCHEMA_VERSION
        )));
    }
    conn.execute_batch(FINAL_SCHEMA_SQL)?;
    // 无条件补列（不随 SCHEMA_VERSION 门控）：终态脚本已含 workflow_definitions.deleted_at，
    // 但既有 v8 库的 user_version 已等于当前版本、不会进入下面的增量分支——若把补列塞进
    // `if current_version < SCHEMA_VERSION` 里就得为此升版本号。软删除列只加不改数据、
    // ensure_column 幂等，故放在版本门控之外、每次启动安全执行，无需 +1 SCHEMA_VERSION。
    ensure_column(conn, "workflow_definitions", "deleted_at", "INTEGER")?;
    // 批次 3：版本快照来源（manual / cloud_sync / import）。与 deleted_at 同理：终态脚本已含该列，
    // 但既有库 user_version 已等于当前版本、不会进入增量分支；该列只加不改数据、ensure_column 幂等，
    // 故同样放在版本门控之外，无需 +1 SCHEMA_VERSION。
    ensure_column(
        conn,
        "workflow_versions",
        "origin",
        "TEXT NOT NULL DEFAULT 'manual'",
    )?;
    if current_version < SCHEMA_VERSION {
        // v4 → v5：api_usage_log 缓存读/写拆分。cache_read_tokens=缓存命中读取，
        // cache_write_tokens=缓存写入；cached_tokens 保留并恒等于两者之和（兼容既有汇总）。
        ensure_column(
            conn,
            "api_usage_log",
            "cache_read_tokens",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            conn,
            "api_usage_log",
            "cache_write_tokens",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        // v5 → v6：会话来源落库。工作流 Agent 节点创建的会话标 origin='workflow'，
        // 用户会话为 NULL——此前靠标题前缀「工作流 · 」判定来源，判据脆弱
        // （前后端各持一份常量、用户改标题即误判）。历史行按前缀一次性回填，
        // 此后前缀只作为展示用标题，不再承担判据职责。
        ensure_column(conn, "sessions", "origin", "TEXT")?;
        conn.execute(
            "UPDATE sessions SET origin = 'workflow' WHERE origin IS NULL AND title LIKE '工作流 · %'",
            [],
        )?;
        // v6 → v7：用量「四桶互斥」口径修正（历史数据一次性回填，仅 OpenAI 协议有确知语义）：
        // 1) v5 之前的行只有单列 cached_tokens（= 缓存命中数），cache_read_tokens 只是被
        //    加了 DEFAULT 0 的新列 → 回填为 cache_read_tokens = cached_tokens；
        // 2) OpenAI 协议的 prompt_tokens 含缓存命中部分，落库口径要求「输入=未命中输入」
        //    （与 Anthropic input_tokens 一致），故按命中数回冲输入。
        // Anthropic 历史行的 cached_tokens 是 read+creation 之和、无法无损拆分，保持原样。
        conn.execute(
            "UPDATE api_usage_log SET cache_read_tokens = cached_tokens
             WHERE api_format = 'OpenAI' AND cache_read_tokens = 0 AND cache_write_tokens = 0
               AND cached_tokens > 0",
            [],
        )?;
        conn.execute(
            "UPDATE api_usage_log SET prompt_tokens = prompt_tokens - cache_read_tokens
             WHERE api_format = 'OpenAI' AND cache_read_tokens > 0
               AND prompt_tokens >= cache_read_tokens",
            [],
        )?;
        // v7 → v8：灵感记来源。从「资源市集 › 灵感」导入的条目落 market_id（市场那条的稳定 id），
        // 用来判断"这条是否已导入"、再导入时做覆盖更新。用户自己新建的灵感为 NULL。
        // 没有它就只能靠标题比对，改个标题就认不出来，会反复导进重复条目。
        ensure_column(conn, "inspirations", "market_id", "TEXT")?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

/// 表是否存在（供旧表移除后的条件迁移使用，如 v10 的 `api_providers`）。
fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        rusqlite::params![table],
        |_| Ok(()),
    )
    .is_ok()
}

/// 幂等补列：目标表已有该列则跳过，否则 `ALTER TABLE ADD COLUMN`。
/// 新列一律带 DEFAULT，历史行自动回填默认值，不触发全表重写。
fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<(), AppError> {
    let exists = conn
        .prepare(&format!("PRAGMA table_info({})", table))?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == column);
    if !exists {
        conn.execute_batch(&format!(
            "ALTER TABLE \"{}\" ADD COLUMN \"{}\" {};",
            table, column, decl
        ))?;
    }
    Ok(())
}

/// 一次性迁移标记：置位后不再回填/重新同步。
///
/// 迁移完成后 `api_providers` 只是**只读遗留表**；若每次启动都回填，用户在管理页
/// 删除某个 provider 后会被遗留表里的历史行"复活"（`INSERT OR IGNORE` 会补回缺行）。
/// 因此回填必须只跑一次。
const API_PROVIDERS_MIGRATED_KEY: &str = "api_providers_migrated_into_agents";

/// 把 `api_providers` 一次性迁移为 `agents` 里的 http 接入（`api:<provider_id>`）+ `agent_secrets`。
///
/// **只读源表、纯增量、只跑一次**（以 `app_settings` 标记守卫）：源表 `api_providers`
/// 不被修改或删除，但迁移后运行期不再读写它，故不会被重复回填。
///
/// 凭据搬的是**密文**（`api_providers.api_key` 本身就是加密存储），保持同一存储约定；
/// `agents.conn` 里**只有** baseUrl/apiFormat/models，**不含密钥**。
///
/// 顺带修正早期回填的缺陷：早期版本未复制 `sort_order`，导致 `api:*` 行排序全为 0；
/// 这里统一按源表补齐一次（同样只做一次，避免覆盖用户在管理页里的排序）。
///
/// 用 Rust 逐行写入而非 SQL 的 `json_object()`：不依赖 SQLite 的 JSON1 扩展，
/// 避免不同运行环境的 SQLite 编译选项差异。
pub fn backfill_api_providers_into_agents(conn: &Connection) -> Result<(), AppError> {
    // v10 起终态脚本不再建该表：全新库 / 已移除旧表的库无迁移源，直接空跑。
    if !table_exists(conn, "api_providers") {
        return Ok(());
    }
    if crate::commands::app_settings::get_setting(conn, API_PROVIDERS_MIGRATED_KEY)?.as_deref()
        == Some("1")
    {
        return Ok(());
    }

    let rows: Vec<(
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        i64,
        i64,
        i64,
    )> = conn
        .prepare(
            "SELECT id, name, api_endpoint, api_format, models, api_key, \
                    COALESCE(sort_order, 0), created_at, updated_at \
             FROM api_providers",
        )?
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (id, name, endpoint, format, models_json, api_key, sort_order, created_at, updated_at) in
        rows
    {
        // models 列是 JSON 文本（对象数组）；解析失败退回空数组（不因一行坏数据中断回填）
        let models: serde_json::Value =
            serde_json::from_str(&models_json).unwrap_or_else(|_| serde_json::json!([]));
        let conn_json = serde_json::json!({
            "baseUrl": endpoint,
            "apiFormat": format,
            "models": models,
        })
        .to_string();

        conn.execute(
            "INSERT OR IGNORE INTO agents
                (agent_type, display_name, description, cli_command, transport, conn, secret_ref,
                 sort_order, created_at, updated_at)
             VALUES (?1, ?2, '', '', 'http', ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                format!("api:{}", id),
                name,
                conn_json,
                format!("secret:{}", id),
                sort_order,
                created_at,
                updated_at
            ],
        )?;

        if let Some(k) = api_key.filter(|k| !k.is_empty()) {
            conn.execute(
                "INSERT OR IGNORE INTO agent_secrets (id, secret, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![format!("secret:{}", id), k, created_at, updated_at],
            )?;
        }
    }

    // 一次性补齐既有 `api:*` 行的 sort_order（早期回填遗留的缺陷）：有源表行才覆盖，
    // 无源表行保持原值（COALESCE 兜底）。
    conn.execute(
        "UPDATE agents SET sort_order = COALESCE(
                (SELECT ap.sort_order FROM api_providers ap
                 WHERE 'api:' || ap.id = agents.agent_type),
                sort_order)
         WHERE transport = 'http' AND agent_type LIKE 'api:%'",
        [],
    )?;

    crate::commands::app_settings::set_setting(conn, API_PROVIDERS_MIGRATED_KEY, "1")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4a：API 供应商回填为 `api:<id>` 行 + 凭据副本，且**幂等**（跑两遍不增行）、源表原样
    #[test]
    fn backfill_api_providers_is_idempotent_and_keeps_source() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(FINAL_SCHEMA_SQL).unwrap();
        // v10 起终态脚本不再建旧表：此处手工造一个「迁移前」的旧表作为夹具。
        conn.execute_batch(
            "CREATE TABLE api_providers (
                id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '',
                api_endpoint TEXT NOT NULL DEFAULT '', api_key TEXT DEFAULT '',
                models TEXT NOT NULL DEFAULT '[]', api_format TEXT NOT NULL DEFAULT 'openai',
                sort_order INTEGER DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_providers
                (id, name, api_endpoint, api_key, models, api_format, sort_order, created_at, updated_at)
             VALUES ('p1','DeepSeek','https://api.deepseek.com/v1','CIPHERTEXT',
                     '[\"deepseek-chat\"]','openai',0,11,22)",
            [],
        )
        .unwrap();

        backfill_api_providers_into_agents(&conn).unwrap();
        backfill_api_providers_into_agents(&conn).unwrap(); // 幂等：第二次不得增行

        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE agent_type LIKE 'api:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "重复回填不得增行");

        let (name, transport, conn_json, secret_ref): (String, String, String, String) = conn
            .query_row(
                "SELECT display_name, transport, conn, secret_ref FROM agents WHERE agent_type='api:p1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(name, "DeepSeek");
        assert_eq!(transport, "http");
        assert_eq!(secret_ref, "secret:p1");
        let v: serde_json::Value = serde_json::from_str(&conn_json).unwrap();
        assert_eq!(v["baseUrl"], "https://api.deepseek.com/v1");
        assert_eq!(v["apiFormat"], "openai");
        assert_eq!(v["models"][0], "deepseek-chat");
        // 连接参数里**不得**出现密钥
        assert!(v.get("apiKey").is_none());
        assert!(v.get("secret").is_none());

        // 凭据：密文原样搬入，且只搬一份
        let sec: String = conn
            .query_row(
                "SELECT secret FROM agent_secrets WHERE id='secret:p1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sec, "CIPHERTEXT");
        let nsec: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nsec, 1);

        // 源表必须原样保留（回填是"加副本"，不是"搬家"）
        let src: String = conn
            .query_row(
                "SELECT api_key FROM api_providers WHERE id='p1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(src, "CIPHERTEXT", "源表必须原样保留");
    }

    /// v10：终态脚本不再建旧表 `api_providers`；旧表缺失时回填必须安全空跑（全新库不报错）。
    #[test]
    fn v10_legacy_table_absent_and_backfill_noops() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(FINAL_SCHEMA_SQL).unwrap();
        assert!(
            !table_exists(&conn, "api_providers"),
            "v10 终态脚本不得再建 api_providers"
        );
        // 无旧表 → 回填空跑、不得报错、不得凭空造 api:* 行
        backfill_api_providers_into_agents(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE agent_type LIKE 'api:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    /// v9 新增：`agents` 的接入方式/连接参数列 + 凭据表；老库补列幂等、带 DEFAULT 回填。
    #[test]
    fn v9_agent_transport_columns_and_secrets_table() {
        // 全新库：FINAL_SCHEMA_SQL 必须带上三列与凭据表
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(FINAL_SCHEMA_SQL).unwrap();

        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(agents)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for c in ["transport", "conn", "secret_ref"] {
            assert!(
                cols.contains(&c.to_string()),
                "agents 缺列 {}（实际 {:?}）",
                c,
                cols
            );
        }
        conn.execute(
            "INSERT INTO agent_secrets (id, secret, created_at, updated_at) VALUES ('k1','s',0,0)",
            [],
        )
        .unwrap();

        // 老库补列：ensure_column 幂等，且新列以 DEFAULT 回填历史行
        let old = rusqlite::Connection::open_in_memory().unwrap();
        old.execute_batch(
            "CREATE TABLE agents (agent_type TEXT PRIMARY KEY);
             INSERT INTO agents (agent_type) VALUES ('claude');",
        )
        .unwrap();
        for _ in 0..2 {
            ensure_column(
                &old,
                "agents",
                "transport",
                "TEXT NOT NULL DEFAULT 'subprocess'",
            )
            .unwrap();
        }
        let t: String = old
            .query_row(
                "SELECT transport FROM agents WHERE agent_type='claude'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(t, "subprocess", "历史行必须以 DEFAULT 回填");
    }

    /// 终态校验：FINAL_SCHEMA_SQL 能完整建出事件表与核心表，且不含任何旧表。
    #[test]
    fn final_schema_creates_final_state() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(FINAL_SCHEMA_SQL).unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for t in [
            "sessions",
            "agents",
            "workflow_definitions",
            "workflow_events",
            "room_events",
            "session_events",
            "api_usage_log",
            "groupchat_rooms",
            "groupchat_participants",
        ] {
            assert!(tables.contains(&t.to_string()), "终态缺少表 {}", t);
        }
        for t in [
            "messages",
            "session_contexts",
            "groupchat_messages",
            "groupchat_tasks",
            "workflow_instances",
            "node_executions",
            "node_execution_logs",
        ] {
            assert!(!tables.contains(&t.to_string()), "终态不应包含旧表 {}", t);
        }
        // 外键修正回归：workflow_schedules/versions 引用真实 workflow_definitions。
        for name in ["workflow_schedules", "workflow_versions"] {
            let sql: String = conn
                .prepare(&format!(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name='{}'",
                    name
                ))
                .unwrap()
                .query_row([], |r| r.get(0))
                .unwrap();
            assert!(
                sql.contains("REFERENCES workflow_definitions(id)"),
                "{} 外键应指向 workflow_definitions",
                name
            );
        }
        // agents 种子插入依赖的列必须齐全（skill_entry_file 等）。
        let agents_sql: String = conn
            .prepare("SELECT sql FROM sqlite_master WHERE type='table' AND name='agents'")
            .unwrap()
            .query_row([], |r| r.get(0))
            .unwrap();
        for col in [
            "skill_entry_file",
            "skill_display_mode",
            "skills_dir",
            "session_id_source",
        ] {
            assert!(agents_sql.contains(col), "agents 表缺少种子依赖列 {}", col);
        }
        // api_usage_log 缓存读/写拆分列必须出现在终态建表脚本中。
        let usage_sql: String = conn
            .prepare("SELECT sql FROM sqlite_master WHERE type='table' AND name='api_usage_log'")
            .unwrap()
            .query_row([], |r| r.get(0))
            .unwrap();
        for col in ["cache_read_tokens", "cache_write_tokens"] {
            assert!(usage_sql.contains(col), "api_usage_log 缺少终态列 {}", col);
        }
        // sessions 的来源列（工作流会话 vs 用户会话）必须在终态建表脚本中。
        let sessions_sql: String = conn
            .prepare("SELECT sql FROM sqlite_master WHERE type='table' AND name='sessions'")
            .unwrap()
            .query_row([], |r| r.get(0))
            .unwrap();
        assert!(sessions_sql.contains("origin"), "sessions 缺少 origin 列");
    }

    /// wipe_all_tables 已随 v5 整库重置策略一并删除；无 wipe 相关用例。
    ///
    /// v4 → v5 增量迁移：老库保留历史数据，仅补 cache_read/cache_write 两列并提升 user_version。
    #[test]
    fn migrate_adds_cache_read_write_columns_keeping_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // 模拟 v4 老库：api_usage_log 无缓存拆分列，且已有历史行。
        conn.execute_batch(
            "CREATE TABLE api_usage_log (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id        TEXT NOT NULL,
                provider          TEXT NOT NULL DEFAULT '',
                model             TEXT NOT NULL DEFAULT '',
                api_format        TEXT NOT NULL DEFAULT '',
                prompt_tokens     INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens      INTEGER NOT NULL DEFAULT 0,
                cached_tokens     INTEGER NOT NULL DEFAULT 0,
                created_at        INTEGER NOT NULL);
            INSERT INTO api_usage_log (session_id, provider, model, prompt_tokens, cached_tokens, created_at)
            VALUES ('s1', 'deepseek', 'deepseek-chat', 100, 40, 1);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 4i64).unwrap();

        migrate_schema(&conn).unwrap();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(api_usage_log)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            columns.contains(&"cache_read_tokens".to_string()),
            "应补 cache_read_tokens 列"
        );
        assert!(
            columns.contains(&"cache_write_tokens".to_string()),
            "应补 cache_write_tokens 列"
        );
        // 历史行保留，新增列按 DEFAULT 0 回填；cached_tokens 原值不受影响。
        let (prompt, cached, cache_read, cache_write): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT prompt_tokens, cached_tokens, cache_read_tokens, cache_write_tokens
                 FROM api_usage_log WHERE session_id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((prompt, cached, cache_read, cache_write), (100, 40, 0, 0));
        // 迁移后 user_version 提升到当前终态；再次执行保持幂等。
        let ver: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
        migrate_schema(&conn).unwrap();
    }

    /// v5 → v6 增量迁移：补 sessions.origin 列，并按标题前缀回填历史工作流会话。
    /// 回填只发生这一次——此后会话来源以列为准，前缀不再参与判定。
    #[test]
    fn migrate_backfills_workflow_session_origin() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                agent_type TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                cwd TEXT DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_message_preview TEXT DEFAULT '',
                message_count INTEGER DEFAULT 0,
                status TEXT DEFAULT 'active',
                api_provider TEXT,
                api_model TEXT,
                agent_session_id TEXT);
            INSERT INTO sessions (id, agent_type, title, created_at, updated_at) VALUES
                ('wf1', 'api', '工作流 · Agent 任务', 1, 1),
                ('u1',  'api', '普通的会话', 1, 1);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 5i64).unwrap();

        migrate_schema(&conn).unwrap();

        let wf: Option<String> = conn
            .query_row("SELECT origin FROM sessions WHERE id = 'wf1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let user: Option<String> = conn
            .query_row("SELECT origin FROM sessions WHERE id = 'u1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(wf.as_deref(), Some("workflow"), "前缀命中的历史行应回填");
        assert_eq!(user, None, "普通会话不应被标记");
    }

    /// v6 → v7 增量迁移：用量四桶口径修正。
    /// - 老行（v5 之前只有单列 cached_tokens）：回填 cache_read_tokens = cached_tokens；
    /// - OpenAI 行：prompt_tokens 含命中部分，按命中数回冲输入（输入=未命中）；
    /// - Anthropic 行（cached 是 read+creation 之和，无法无损拆分）：保持原样。
    #[test]
    fn migrate_fixes_openai_usage_buckets() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE api_usage_log (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id        TEXT NOT NULL,
                provider          TEXT NOT NULL DEFAULT '',
                model             TEXT NOT NULL DEFAULT '',
                api_format        TEXT NOT NULL DEFAULT '',
                prompt_tokens     INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens      INTEGER NOT NULL DEFAULT 0,
                cached_tokens     INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                created_at        INTEGER NOT NULL);",
        )
        .unwrap();
        // legacy：老行只写了 cached；modern：read 已由 v5 起的新代码落库
        conn.execute_batch(
            "INSERT INTO api_usage_log (session_id, api_format, prompt_tokens, completion_tokens, total_tokens, cached_tokens, cache_read_tokens, created_at)
             VALUES ('legacy',  'OpenAI',    1000, 100, 1100,  600,    0, 1),
                    ('modern',  'OpenAI',    2000, 200, 2200, 1500, 1500, 2),
                    ('anthropic','Anthropic', 500,  50,  550,  300,    0, 3);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 6i64).unwrap();

        migrate_schema(&conn).unwrap();

        let rows: Vec<(String, i64, i64, i64, i64)> = conn
            .prepare(
                "SELECT session_id, prompt_tokens, cached_tokens, cache_read_tokens, cache_write_tokens
                 FROM api_usage_log ORDER BY created_at",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(
            rows,
            vec![
                ("legacy".into(), 400, 600, 600, 0),
                ("modern".into(), 500, 1500, 1500, 0),
                ("anthropic".into(), 500, 300, 0, 0),
            ]
        );
        // 迁移后版本提升；再次执行保持幂等（UPDATE 不重复回冲）。
        let ver: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
        migrate_schema(&conn).unwrap();
        let prompt: i64 = conn
            .query_row(
                "SELECT prompt_tokens FROM api_usage_log WHERE session_id = 'legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(prompt, 400, "重复迁移不得二次回冲");
    }

    /// 既有 v8 库（workflow_definitions 尚无 deleted_at）经迁移补列：
    /// 列被加上、历史行默认未删除、user_version 不变（此列不随版本号门控），重复迁移幂等。
    #[test]
    fn migrate_adds_workflow_deleted_at_without_version_bump() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE workflow_definitions (
                id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 1);
             INSERT INTO workflow_definitions (id, name) VALUES ('d1', '旧工作流');",
        )
        .unwrap();
        // 既有 v8 库：版本号已等于当前版本，不会进入增量分支
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .unwrap();

        migrate_schema(&conn).unwrap();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(workflow_definitions)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            columns.contains(&"deleted_at".to_string()),
            "应补 deleted_at 列"
        );
        let (name, deleted): (String, Option<i64>) = conn
            .query_row(
                "SELECT name, deleted_at FROM workflow_definitions WHERE id = 'd1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "旧工作流");
        assert_eq!(deleted, None, "历史行默认未删除");
        let ver: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(ver, SCHEMA_VERSION, "补列不应改动 schema 版本号");
        migrate_schema(&conn).unwrap(); // 再次执行保持幂等
    }

    /// 比当前更新的库（未来版本应用创建）拒绝打开，绝不 wipe。
    #[test]
    fn migrate_rejects_newer_version_without_wiping() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE keep_me (id INTEGER);")
            .unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();

        let err = migrate_schema(&conn).unwrap_err();
        assert!(
            err.to_string().contains("升级"),
            "错误应提示升级应用: {}",
            err
        );
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='keep_me'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1, "未来版本库不得被清除");
    }
}
