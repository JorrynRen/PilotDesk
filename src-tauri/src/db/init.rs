use rusqlite::Connection;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use crate::utils::paths::db_path;
use crate::utils::errors::AppError;
use std::fs;

/// 对外 schema 版本：v5 起从"版本不一致即整库重置"改为增量迁移，历史库一律保留；
/// 打开到比当前更新的库时明确报错，绝不 wipe。v1-v4 为预发布期整库重置，无独立旧库留存。
/// 约定：每次修改 FINAL_SCHEMA_SQL 都必须把 SCHEMA_VERSION +1，
/// 否则 user_version 相等的旧库走快速路径、不会补齐缺表/缺列。
pub const SCHEMA_VERSION: i64 = 5;

/// 当前终态建表脚本（唯一 schema 定义）。依据既有生产库 schema 固化：
/// - 全部使用 IF NOT EXISTS，可每次启动安全执行；
/// - v5 起老库按列增量补齐（见 migrate_schema/ensure_column），新列一律带 DEFAULT 回填历史行。
/// 额外的基础兜底表（file_history、inspirations_fts）仍在 init_db 中执行。
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
        , version TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS api_providers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            api_endpoint TEXT NOT NULL DEFAULT '',
            api_key TEXT DEFAULT '',
            api_key_masked TEXT DEFAULT '',
            api_key_set INTEGER DEFAULT 0,
            models TEXT NOT NULL DEFAULT '[]',
            sort_order INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        , api_format TEXT NOT NULL DEFAULT 'openai');
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
            updated_at INTEGER NOT NULL);
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
            enabled INTEGER NOT NULL DEFAULT 1);
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

    let manager = SqliteConnectionManager::file(&db_path);
    let pool = Pool::builder()
        .max_size(8)
        .build(manager)?;

    // 数据库操作在单连接上进行
    let conn = pool.get()?;

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;"
    )?;

    // ── schema 增量迁移（v5 起）──
    // 历史库一律保留：旧版库逐项补列升级；比当前更新的库（由更新版本应用创建/打开过）
    // 明确报错拒绝打开，绝不整库重置。先执行幂等终态建表脚本兜底缺表，再补老库缺列。
    migrate_schema(&conn)?;

    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS inspirations_fts USING fts5(title, content, content=inspirations, content_rowid=rowid);
        CREATE INDEX IF NOT EXISTS idx_inspirations_favorite ON inspirations(is_favorite, updated_at);
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

    for (agent_type, display_name, description, cli_command, npm_package, pip_package,
         install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
         output_parser, output_filter_regex, supports_session_continuity,
         session_id_source, session_id_event_type, session_id_field, resume_arg_template,
         skills_dir, skill_display_mode,
         color, icon, sort_order) in agent_seeds {
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
    let workspace_default = crate::utils::paths::app_data_dir().to_string_lossy().into_owned();
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
    let rules_json = serde_json::to_string(
        &crate::api_agent::agent_loop::default_permission_rules(),
    )
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
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

/// 幂等补列：目标表已有该列则跳过，否则 `ALTER TABLE ADD COLUMN`。
/// 新列一律带 DEFAULT，历史行自动回填默认值，不触发全表重写。
fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<(), AppError> {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 比当前更新的库（未来版本应用创建）拒绝打开，绝不 wipe。
    #[test]
    fn migrate_rejects_newer_version_without_wiping() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE keep_me (id INTEGER);").unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();

        let err = migrate_schema(&conn).unwrap_err();
        assert!(err.to_string().contains("升级"), "错误应提示升级应用: {}", err);
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

