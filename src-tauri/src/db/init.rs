use rusqlite::{params, Connection};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use crate::utils::paths::db_path;
use crate::utils::errors::AppError;
use std::fs;

/// 所有迁移版本号（必须保持升序排列）
/// 新增迁移时：1) 在此数组末尾追加版本号  2) 在 run_migrations match 中添加对应分支
/// MIGRATION_VERSION 自动取数组最大值，无需手动维护
const MIGRATION_VERSIONS: &[i64] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 65, 66, 67, 68, 69];

/// MIGRATION_VERSION 自动从 MIGRATION_VERSIONS 数组计算最大值
/// 新增迁移时只需在数组中追加版本号，此值自动同步，无需手动维护
const MIGRATION_VERSION: i64 = {
    let mut max = 0i64;
    let mut i = 0;
    while i < MIGRATION_VERSIONS.len() {
        if MIGRATION_VERSIONS[i] > max {
            max = MIGRATION_VERSIONS[i];
        }
        i += 1;
    }
    max
};

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

    // Run migrations on a single connection
    let conn = pool.get()?;

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;"
    )?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
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
            api_model TEXT
        );

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
            content TEXT NOT NULL DEFAULT '',
            mode TEXT DEFAULT 'native' CHECK(mode IN ('native', 'fast', 'think', 'expert')),
            timestamp INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS inspirations (
            id TEXT PRIMARY KEY,
            icon TEXT NOT NULL DEFAULT '💡',
            title TEXT NOT NULL,
            content TEXT NOT NULL DEFAULT '',
            source_agent TEXT DEFAULT 'manual',
            is_favorite INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS inspiration_tags (
            inspiration_id TEXT NOT NULL REFERENCES inspirations(id) ON DELETE CASCADE,
            tag TEXT NOT NULL,
            PRIMARY KEY (inspiration_id, tag)
        );

CREATE VIRTUAL TABLE IF NOT EXISTS inspirations_fts USING fts5(title, content, content=inspirations, content_rowid=rowid);
        CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_type, updated_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status, updated_at);
        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_inspirations_favorite ON inspirations(is_favorite, updated_at);
        CREATE INDEX IF NOT EXISTS idx_inspirations_tags ON inspiration_tags(tag);"
    )?;

    // Versioned migrations — 由 MIGRATION_VERSIONS 数组驱动
    let current_version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap_or(0);

    run_migrations(&conn, current_version)?;

if current_version < MIGRATION_VERSION {
        conn.pragma_update(None, "user_version", MIGRATION_VERSION)?;
    }

    Ok(pool)
}


fn migrate_add_api_providers(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS api_providers (
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
        )"
    )?;
    Ok(())
}

fn migrate_add_app_settings(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL
        )"
    )?;

    let seeds: Vec<(&str, &str)> = vec![
        ("mode_prompt_native", ""),
        ("mode_prompt_fast", "快速简洁回答，直接给出结论，无需详细解释推理过程"),
        ("mode_prompt_think", "逐步分析推理，详细解释你的思路和过程，给出完整的推理链"),
        ("mode_prompt_expert", "以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案"),
        ("pilotdesk-workspace", "~\\AppData\\Roaming\\PilotDesk"),
    ];
    let now = crate::utils::now();
    for (key, value) in seeds {
        conn.execute(
            "INSERT OR IGNORE INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params![key, value, now],
        )?;
    }

    Ok(())
}

fn migrate_add_api_columns(conn: &Connection) -> Result<(), AppError> {
    let has_api_provider = conn
        .prepare("SELECT api_provider FROM sessions LIMIT 0")
        .is_ok();

    if !has_api_provider {
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN api_provider TEXT;
             ALTER TABLE sessions ADD COLUMN api_model TEXT;"
        )?;
    }

    Ok(())
}

fn migrate_add_type(conn: &Connection) -> Result<(), AppError> {
    let accepts_api = conn
        .execute("INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at) VALUES ('__migration_test_api', 'api', '', '', 0, 0)", [])
        .is_ok();
    let accepts_codex = conn
        .execute("INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at) VALUES ('__migration_test_codex', 'codex', '', '', 0, 0)", [])
        .is_ok();

    if accepts_api && accepts_codex {
        conn.execute("DELETE FROM sessions WHERE id IN ('__migration_test_api', '__migration_test_codex')", [])?;
        return Ok(());
    }

    conn.execute("DELETE FROM sessions WHERE id IN ('__migration_test_api', '__migration_test_codex')", [])?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions_new (
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
            api_model TEXT
        );
        

        INSERT OR IGNORE INTO sessions_new (id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model)
        SELECT id, agent_type, title, cwd, created_at, updated_at, last_message_preview, message_count, status, api_provider, api_model FROM sessions;

        DROP TABLE sessions;
        ALTER TABLE sessions_new RENAME TO sessions;

        CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_type, updated_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status, updated_at);"
    )?;

    Ok(())
}

fn migrate_add_message_extensions(conn: &Connection) -> Result<(), AppError> {
    let has_reasoning = conn
        .prepare("SELECT reasoning_content FROM messages LIMIT 0")
        .is_ok();

    if !has_reasoning {
        conn.execute_batch(
            "ALTER TABLE messages ADD COLUMN reasoning_content TEXT DEFAULT NULL;
             ALTER TABLE messages ADD COLUMN tool_calls TEXT DEFAULT NULL;
             ALTER TABLE messages ADD COLUMN tool_call_id TEXT DEFAULT NULL;
             ALTER TABLE messages ADD COLUMN tool_name TEXT DEFAULT NULL;"
        )?;
    }

    Ok(())
}

fn migrate_add_install_logs(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS install_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            message TEXT NOT NULL DEFAULT '',
            level TEXT NOT NULL DEFAULT 'info' CHECK(level IN ('info', 'warn', 'error', 'success'))
        );
        

        CREATE INDEX IF NOT EXISTS idx_install_logs_time ON install_logs(timestamp);"
    )?;
    Ok(())
}

fn migrate_add_agent_session_id(conn: &Connection) -> Result<(), AppError> {
    let has_column = conn
        .prepare("SELECT agent_session_id FROM sessions LIMIT 0")
        .is_ok();

    if !has_column {
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN agent_session_id TEXT DEFAULT NULL;"
        )?;
    }

    Ok(())
}

fn migrate_add_agents_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agents (
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
            version_pattern TEXT NOT NULL DEFAULT 'v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)',
            supports_session_continuity INTEGER NOT NULL DEFAULT 0,
            session_id_source TEXT NOT NULL DEFAULT 'none',
            session_id_event_type TEXT NOT NULL DEFAULT '',
            session_id_field TEXT NOT NULL DEFAULT '',
            resume_arg_template TEXT NOT NULL DEFAULT '',
            skills_dir TEXT NOT NULL DEFAULT '',
            skill_display_mode TEXT NOT NULL DEFAULT 'collection',
            color TEXT NOT NULL DEFAULT '#6366F1',
            icon TEXT NOT NULL DEFAULT '\\U0001f916',
            sort_order INTEGER DEFAULT 0,
            is_enabled INTEGER DEFAULT 1,
            is_builtin INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        "
    )?;

    let now = crate::utils::now();
    let seeds: Vec<(&str, &str, &str, &str, Option<&str>, Option<&str>, &str, &str, &str, &str, &str, &str, &str, &str, i64, &str, &str, &str, &str, &str, &str, &str, &str, i64)> = vec![
        ("claude", "Claude Code", "Anthropic 出品的 AI 编程助手",
         "claude", Some("@anthropic-ai/claude-code"), None,
         "npm install -g @anthropic-ai/claude-code",
         "npm uninstall -g @anthropic-ai/claude-code",
         "claude update",
         "claude --version",
         "npm view @anthropic-ai/claude-code version",
         "claude -p --output-format stream-json --verbose --dangerously-skip-permissions -- {message}",
         "json-stream", "", 1, "stdout-json", "system/init", "session_id", "--resume {session_id}",
         "#3B82F6", "file:claude_icon.ico", "~/.claude/skills/", "collection", 1),
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
         "stderr-text", "", "", "--resume {session_id}",
         "#8B5CF6", "file:hermes_icon.ico", "~/AppData/Local/hermes/skills/", "collection", 2),
        ("codex", "Codex CLI", "OpenAI 出品的终端 AI 编程助手",
         "codex", Some("@openai/codex"), None,
         "npm install -g @openai/codex",
         "npm uninstall -g @openai/codex",
         "codex update",
         "codex --version",
         "npm view @openai/codex version",
         "codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -- {message}",
         "json-stream", "", 1, "stdout-json", "thread.started", "thread_id", "exec resume {session_id}",
         "#F59E0B", "file:codex_icon.ico", "~/.codex/skills/", "collection", 3),
    ];

    for (agent_type, display_name, description, cli_command, npm_package, pip_package,
         install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
         output_parser, output_filter_regex, supports_session_continuity,
         session_id_source, session_id_event_type, session_id_field, resume_arg_template,
         skills_dir, skill_display_mode,
         color, icon, sort_order) in seeds {
        conn.execute(
            "INSERT OR IGNORE INTO agents (agent_type, display_name, description, cli_command, npm_package, pip_package,
             version_flag, install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
             output_parser, output_filter_regex, version_pattern, supports_session_continuity,
             session_id_source, session_id_event_type, session_id_field, resume_arg_template,
             skills_dir, skill_display_mode,
             color, icon, sort_order, is_enabled, is_builtin, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, '--version', ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
             'v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)', ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, 1, 1, ?25, ?25)",
            rusqlite::params![agent_type, display_name, description, cli_command, npm_package, pip_package,
                install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
                output_parser, output_filter_regex, supports_session_continuity,
                session_id_source, session_id_event_type, session_id_field, resume_arg_template,
                skills_dir, skill_display_mode,
                color, icon, sort_order, now],
        )?;
    }

    Ok(())
}

fn migrate_add_skill_fields(conn: &Connection) -> Result<(), AppError> {
    // Add skills_dir, skill_entry_file, skill_display_mode columns
    // Remove version_flag column (SQLite doesn't support DROP COLUMN before 3.35.0,
    // so we recreate the table)
    let has_skills_dir = conn.prepare("SELECT skills_dir FROM agents LIMIT 0").is_ok();
    if !has_skills_dir {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agents_new (
                agent_type TEXT PRIMARY KEY,
                display_name TEXT NOT NULL DEFAULT '',
                description TEXT NOT NULL DEFAULT '',
                cli_command TEXT NOT NULL DEFAULT '',
                npm_package TEXT,
                pip_package TEXT,
                install_cmd TEXT NOT NULL DEFAULT '',
                uninstall_cmd TEXT NOT NULL DEFAULT '',
                update_cmd TEXT NOT NULL DEFAULT '',
                version_cmd TEXT NOT NULL DEFAULT '',
                latest_version_cmd TEXT NOT NULL DEFAULT '',
                run_cmd_template TEXT NOT NULL DEFAULT '',
                output_parser TEXT NOT NULL DEFAULT 'raw-text',
                output_filter_regex TEXT NOT NULL DEFAULT '',
                version_pattern TEXT NOT NULL DEFAULT 'v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)',
                supports_session_continuity INTEGER NOT NULL DEFAULT 0,
                session_id_source TEXT NOT NULL DEFAULT 'none',
                session_id_event_type TEXT NOT NULL DEFAULT '',
                session_id_field TEXT NOT NULL DEFAULT '',
                resume_arg_template TEXT NOT NULL DEFAULT '',
                skills_dir TEXT NOT NULL DEFAULT '',
                skill_entry_file TEXT NOT NULL DEFAULT 'SKILL.md',
                skill_display_mode TEXT NOT NULL DEFAULT 'collection',
                color TEXT NOT NULL DEFAULT '#6366F1',
                icon TEXT NOT NULL DEFAULT '\\U0001f916',
                sort_order INTEGER DEFAULT 0,
                is_enabled INTEGER DEFAULT 1,
                is_builtin INTEGER DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
        
            INSERT OR IGNORE INTO agents_new (
                agent_type, display_name, description, cli_command,
                npm_package, pip_package,
                install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd,
                run_cmd_template, output_parser, output_filter_regex, version_pattern,
                supports_session_continuity, session_id_source, session_id_event_type,
                session_id_field, resume_arg_template,
                skills_dir, skill_entry_file, skill_display_mode,
                color, icon, sort_order, is_enabled, is_builtin, created_at, updated_at
            )
            SELECT agent_type, display_name, description, cli_command,
                npm_package, pip_package,
                install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd,
                run_cmd_template, output_parser, output_filter_regex, version_pattern,
                supports_session_continuity, session_id_source, session_id_event_type,
                session_id_field, resume_arg_template,
                '', 'SKILL.md', 'recursive',
                color, icon, sort_order, is_enabled, is_builtin, created_at, updated_at
            FROM agents;
            DROP TABLE agents;
            ALTER TABLE agents_new RENAME TO agents;"
        )?;
    }
    Ok(())
}

fn migrate_agents_full_schema(conn: &Connection) -> Result<(), AppError> {
    // Drop old table and recreate with full schema
    conn.execute_batch("DROP TABLE IF EXISTS agents;")?;
    // Reuse the full schema from migrate_add_agents_table
    migrate_add_agents_table(conn)
}


/// Migration v12 — 更新预置 Agent 图标字段（Emoji -> file:xxx.ico）
fn migrate_update_agent_icons(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "UPDATE agents SET icon = ?1 WHERE agent_type = ?2",
        rusqlite::params!["file:claude_icon.ico", "claude"],
    )?;
    conn.execute(
        "UPDATE agents SET icon = ?1 WHERE agent_type = ?2",
        rusqlite::params!["file:hermes_icon.ico", "hermes"],
    )?;
    conn.execute(
        "UPDATE agents SET icon = ?1 WHERE agent_type = ?2",
        rusqlite::params!["file:codex_icon.ico", "codex"],
    )?;
    Ok(())
}

/// Migration v13 — 移除 sessions 表对 agent_type 的 CHECK 约束
/// 确保自定义 Agent 类型可以正常创建会话
fn migrate_remove_agent_type_check(conn: &Connection) -> Result<(), AppError> {
    // SQLite cannot ALTER TABLE DROP CHECK, so recreate the table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions_v13 (
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
        );
        
        INSERT OR IGNORE INTO sessions_v13 SELECT * FROM sessions;
        DROP TABLE sessions;
        ALTER TABLE sessions_v13 RENAME TO sessions;
        CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_type, updated_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status, updated_at);"
    )?;
    Ok(())
}

/// Migration v14 — agents 表增加 version 字段，用于 Agent 市场版本管理
fn migrate_add_agent_version(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "ALTER TABLE agents ADD COLUMN version TEXT NOT NULL DEFAULT '';"
    )?;
    // 为内置 Agent 设置初始版本号
    conn.execute(
        "UPDATE agents SET version = '1.0' WHERE is_builtin = 1 AND (version IS NULL OR version = '')",
        [],
    )?;
    Ok(())
}

/// Migration v15 — 更新内置 Agent 技能配置（skills_dir / skill_display_mode）
fn migrate_update_builtin_skills(conn: &Connection) -> Result<(), AppError> {
    // 更新 skill_display_mode 为 collection（只显示集合名）
    conn.execute(
        "UPDATE agents SET skill_display_mode = 'collection' WHERE is_builtin = 1",
        [],
    )?;
    // 所有内置 Agent 显式设置 skills_dir（后端在 skills_dir 为空时也会回退 ~/.{agent_type}/skills/）
    conn.execute(
        "UPDATE agents SET skills_dir = '~/.claude/skills/' WHERE agent_type = 'claude'",
        [],
    )?;
    conn.execute(
        "UPDATE agents SET skills_dir = '~/AppData/Local/hermes/skills/' WHERE agent_type = 'hermes'",
        [],
    )?;
    // 内置 Agent 卸载命令改用包管理器直接卸载（-y 自动确认），避免交互式终端检测
    conn.execute(
        "UPDATE agents SET uninstall_cmd = 'pip uninstall hermes-agent -y' WHERE agent_type = 'hermes'",
        [],
    )?;
    conn.execute(
        "UPDATE agents SET uninstall_cmd = 'npm uninstall -g @anthropic-ai/claude-code' WHERE agent_type = 'claude'",
        [],
    )?;
    conn.execute(
        "UPDATE agents SET uninstall_cmd = 'npm uninstall -g @openai/codex' WHERE agent_type = 'codex'",
        [],
    )?;
    conn.execute(
        "UPDATE agents SET skills_dir = '~/.codex/skills/' WHERE agent_type = 'codex'",
        [],
    )?;
    Ok(())
}

/// Migration v16 — 修复 claude 技能配置种子数据（v15 执行时未覆盖）
fn migrate_fix_claude_skills(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "UPDATE agents SET skills_dir = '~/.claude/skills/', skill_display_mode = 'collection' WHERE agent_type = 'claude'",
        [],
    )?;
    Ok(())
}

/// Migration v17 — 修复 Hermes 消息参数注入问题
/// 将 run_cmd_template 从 -q {message} 改为 --query={message}
/// 配合 handler.rs 的 = 拼接逻辑，确保 --help 等不被 argparse 拦截
fn migrate_fix_hermes_template(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "UPDATE agents SET run_cmd_template = 'hermes chat --query={message} -Q' WHERE agent_type = 'hermes'",
        [],
    )?;
    Ok(())
}


/// Migration v18 — 修复 Claude 和 Codex 消息参数注入问题
/// Claude: -p {message} → --prompt={message}（= 语法，yargs 将 = 后内容作为值）
/// Codex: 末尾添加 -- 分隔符，告诉 argparse 停止解析标志
fn migrate_fix_claude_codex_templates(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "UPDATE agents SET run_cmd_template = 'claude -p --output-format stream-json --verbose --dangerously-skip-permissions -- {message}' WHERE agent_type = 'claude'",
        [],
    )?;
    conn.execute(
        "UPDATE agents SET run_cmd_template = 'codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -- {message}' WHERE agent_type = 'codex'",
        [],
    )?;
    Ok(())
}

/// Migration v19 — 修复 Claude 模板：末尾添加 -- 分隔符（-p 是布尔标志，{message} 是位置参数）
fn migrate_fix_claude_prompt_template(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "UPDATE agents SET run_cmd_template = 'claude -p --output-format stream-json --verbose --dangerously-skip-permissions -- {message}' WHERE agent_type = 'claude'",
        [],
    )?;
    Ok(())
}

/// Migration v20 — 修复 Claude 模板 --prompt= → -- {message}（v19 执行时 SQL 仍为 --prompt=）
fn migrate_fix_claude_dash_dash(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "UPDATE agents SET run_cmd_template = 'claude -p --output-format stream-json --verbose --dangerously-skip-permissions -- {message}' WHERE agent_type = 'claude'",
        [],
    )?;
    Ok(())
}


fn migrate_add_workflow_tables(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS workflow_definitions (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            version TEXT NOT NULL DEFAULT '1.0.0',
            description TEXT NOT NULL DEFAULT '',
            trigger TEXT NOT NULL DEFAULT '{"triggerType":"manual"}',
            stages TEXT NOT NULL DEFAULT '[]',
            input_schema TEXT,
            output_schema TEXT,
            max_depth INTEGER DEFAULT 10,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1
        );
        

        CREATE TABLE IF NOT EXISTS workflow_instances (
            id TEXT PRIMARY KEY,
            definition_id TEXT NOT NULL REFERENCES workflow_definitions(id) ON DELETE CASCADE,
            definition_name TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'running', 'paused', 'success', 'failed', 'cancelled', 'timeout')),
            context TEXT NOT NULL DEFAULT '{}',
            steps TEXT NOT NULL DEFAULT '{}',
            current_node_id TEXT,
            trigger TEXT NOT NULL DEFAULT 'manual' CHECK(trigger IN ('manual', 'cron', 'event')),
            trigger_detail TEXT,
            started_at INTEGER,
            completed_at INTEGER,
            estimated_remaining INTEGER,
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        );
        

        "#,
    )?;
    Ok(())
}
/// v22: 节点执行记录表
fn migrate_add_node_execution_tables(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS node_executions (
            id TEXT PRIMARY KEY,
            execution_id TEXT NOT NULL REFERENCES workflow_instances(id) ON DELETE CASCADE,
            node_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'running', 'completed', 'failed', 'skipped', 'cancelled')),
            input_data TEXT DEFAULT NULL,
            output_data TEXT DEFAULT NULL,
            error_message TEXT DEFAULT NULL,
            started_at INTEGER,
            finished_at INTEGER,
            retry_count INTEGER DEFAULT 0,
            duration_ms INTEGER DEFAULT 0,
            artifacts_path TEXT DEFAULT NULL,
            agent_session_id TEXT DEFAULT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        

        CREATE TABLE IF NOT EXISTS node_execution_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            execution_id TEXT NOT NULL,
            node_execution_id TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            level TEXT NOT NULL DEFAULT 'info' CHECK(level IN ('debug', 'info', 'warn', 'error')),
            message TEXT NOT NULL,
            metadata TEXT DEFAULT NULL
        );
        

        CREATE INDEX IF NOT EXISTS idx_node_exec_execution ON node_executions(execution_id);
        CREATE INDEX IF NOT EXISTS idx_node_exec_status ON node_executions(execution_id, status);
        CREATE INDEX IF NOT EXISTS idx_node_logs_exec ON node_execution_logs(node_execution_id, timestamp);"
    )?;
    Ok(())
}

/// v23: 定时调度表
fn migrate_add_workflow_schedule(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workflow_schedules (
            id TEXT PRIMARY KEY,
            workflow_id TEXT NOT NULL REFERENCES workflow_definitions(id) ON DELETE CASCADE,
            cron_expression TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            input_data TEXT DEFAULT '{}',
            last_run_at INTEGER,
            next_run_at INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        

        CREATE INDEX IF NOT EXISTS idx_wf_schedule_next ON workflow_schedules(next_run_at, enabled);"
    )?;
    Ok(())
}

/// v65: 性能优化索引
fn migrate_add_performance_indexes(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_workflow_defs_enabled ON workflow_definitions(enabled, updated_at);
         CREATE INDEX IF NOT EXISTS idx_workflow_instances_completed ON workflow_instances(completed_at, status);
         CREATE INDEX IF NOT EXISTS idx_node_executions_node ON node_executions(node_id, execution_id);
         CREATE INDEX IF NOT EXISTS idx_node_execution_logs_level ON node_execution_logs(node_execution_id, level, timestamp);
         ANALYZE;"
    )?;
    Ok(())
}

/// v66: 缺失的工作流表和索引
fn migrate_add_workflow_missing_tables(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workflow_versions (
            id TEXT PRIMARY KEY,
            workflow_id TEXT NOT NULL REFERENCES workflow_definitions(id) ON DELETE CASCADE,
            version INTEGER NOT NULL,
            snapshot TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            UNIQUE(workflow_id, version)
        );
        

        
        
        CREATE INDEX IF NOT EXISTS idx_wf_versions_workflow ON workflow_versions(workflow_id, version DESC);
        CREATE INDEX IF NOT EXISTS idx_wf_exec_workflow ON workflow_instances(definition_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_wf_exec_status ON workflow_instances(status, created_at DESC);"
    )?;
    Ok(())
}

/// v67: 迁移旧版 workflow_definitions 数据
/// 旧版表有 nodes/edges 列，新版改为 trigger/stages 列
/// 将旧 nodes/edges JSON 包装为默认阶段，设置默认 trigger
fn migrate_legacy_workflow_columns(conn: &Connection) -> Result<(), AppError> {
    // 检查旧列是否存在
    let has_nodes_col: bool = conn.prepare("SELECT nodes FROM workflow_definitions LIMIT 1").is_ok();
    if !has_nodes_col {
        // 已经是新 schema，无需迁移
        return Ok(());
    }

    // 读取所有旧数据
    let mut stmt = conn.prepare(
        "SELECT id, name, version, description, nodes, edges, input_schema, output_schema, max_depth, created_at, updated_at, enabled FROM workflow_definitions"
    )?;
    let rows: Vec<(String, String, String, String, String, String, Option<String>, Option<String>, Option<i32>, i64, i64, bool)> = stmt.query_map([], |row| {
        Ok((
            row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
            row.get::<_, String>(4)?, row.get::<_, String>(5)?,
            row.get(6)?, row.get(7)?, row.get(8)?,
            row.get(9)?, row.get(10)?, row.get(11)?,
        ))
    })?.filter_map(|r| r.ok()).collect();

    if rows.is_empty() {
        // 无数据，直接重建表
        conn.execute_batch(
            "DROP TABLE IF EXISTS workflow_definitions;"
        )?;
        // 重新创建新表（复用 v21 的建表逻辑）
        migrate_add_workflow_tables(conn)?;
        return Ok(());
    }

    // 重建表：旧表名 → 新表
    conn.execute_batch(
        "ALTER TABLE workflow_definitions RENAME TO workflow_definitions_old;"
    )?;

    // 创建新表
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS workflow_definitions (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            version TEXT NOT NULL DEFAULT '1.0.0',
            description TEXT NOT NULL DEFAULT '',
            trigger TEXT NOT NULL DEFAULT '{"triggerType":"manual"}',
            stages TEXT NOT NULL DEFAULT '[]',
            input_schema TEXT,
            output_schema TEXT,
            max_depth INTEGER DEFAULT 10,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1
        );"#
    )?;

    // 逐行迁移数据
    for (id, name, version, description, nodes_json, _edges_json, input_schema, output_schema, max_depth, created_at, updated_at, enabled) in &rows {
        let stages_json = format!(
            r#"[{{"id":"{}","name":"默认阶段","order":0,"nodes":{},"edges":[],"gate":{{"strategy":"all","mergeStrategy":"merge"}}}}]"#,
            crate::utils::new_id(),
            nodes_json,
        );
        let trigger_json = r#"{"triggerType":"manual"}"#.to_string();

        conn.execute(
            "INSERT INTO workflow_definitions (id, name, version, description, trigger, stages, input_schema, output_schema, max_depth, created_at, updated_at, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                id, name, version, description,
                trigger_json, stages_json,
                input_schema, output_schema, max_depth,
                created_at, updated_at, enabled,
            ],
        )?;
    }

    // 删除旧表
    conn.execute_batch("DROP TABLE IF EXISTS workflow_definitions_old;")?;

    // 清理旧版遗留表（已不再使用）
    conn.execute_batch(
        "DROP TABLE IF EXISTS workflow_nodes;
         DROP TABLE IF EXISTS workflow_edges;"
    )?;

    Ok(())
}

/// v68: 废弃 workflow_instances.steps 字段
/// 引擎已不再写入 steps 字段（改为 node_executions 表），将所有 rows 的 steps 设为 NULL，
/// 后续版本可直接 DROP COLUMN。
fn migrate_deprecate_steps_column(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "UPDATE workflow_instances SET steps = NULL WHERE steps IS NOT NULL",
        [],
    )?;
    Ok(())
}

/// 根据 MIGRATION_VERSIONS 数组循环执行迁移
/// 新增迁移时：在 MIGRATION_VERSIONS 末尾追加版本号，并在 match 中添加对应分支
fn migrate_add_stage_edges_column(conn: &Connection) -> Result<(), AppError> {
    let has_col: bool = conn
        .prepare("SELECT stage_edges FROM workflow_definitions LIMIT 0")
        .is_ok();
    if !has_col {
        conn.execute_batch(
            "ALTER TABLE workflow_definitions ADD COLUMN stage_edges TEXT NOT NULL DEFAULT '[]';",
        )?;
    }
    Ok(())
}

fn run_migrations(conn: &Connection, current_version: i64) -> Result<(), AppError> {
    for &ver in MIGRATION_VERSIONS {
        if current_version < ver {
            match ver {
                1 => migrate_add_api_columns(conn)?,
                2 => migrate_add_type(conn)?,
                3 => migrate_add_api_providers(conn)?,
                4 => migrate_add_app_settings(conn)?,
                5 => migrate_add_install_logs(conn)?,
                6 => migrate_add_message_extensions(conn)?,
                7 => migrate_add_agent_session_id(conn)?,
                8 => migrate_add_agents_table(conn)?,
                9 => migrate_agents_full_schema(conn)?,
                11 => migrate_add_skill_fields(conn)?,
                12 => migrate_update_agent_icons(conn)?,
                13 => migrate_remove_agent_type_check(conn)?,
                14 => migrate_add_agent_version(conn)?,
                15 => migrate_update_builtin_skills(conn)?,
                16 => migrate_fix_claude_skills(conn)?,
                17 => migrate_fix_hermes_template(conn)?,
                18 => migrate_fix_claude_codex_templates(conn)?,
                19 => migrate_fix_claude_prompt_template(conn)?,
                20 => migrate_fix_claude_dash_dash(conn)?,
                21 => migrate_add_workflow_tables(conn)?,
                22 => migrate_add_node_execution_tables(conn)?,
                23 => migrate_add_workflow_schedule(conn)?,
                65 => migrate_add_performance_indexes(conn)?,
                66 => migrate_add_workflow_missing_tables(conn)?,
                67 => migrate_legacy_workflow_columns(conn)?,
                68 => migrate_deprecate_steps_column(conn)?,
                69 => migrate_add_stage_edges_column(conn)?,
                _ => return Err(AppError::Config(format!("未知的迁移版本号: {}", ver))),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确保 MIGRATION_VERSIONS 数组是严格升序的
    /// 新增迁移时如果忘记按升序插入，此测试会失败
    #[test]
    fn migration_versions_ascending() {
        for i in 1..MIGRATION_VERSIONS.len() {
            assert!(
                MIGRATION_VERSIONS[i] > MIGRATION_VERSIONS[i - 1],
                "MIGRATION_VERSIONS[{}] ({}) 必须大于 MIGRATION_VERSIONS[{}] ({})",
                i, MIGRATION_VERSIONS[i], i - 1, MIGRATION_VERSIONS[i - 1]
            );
        }
    }

    /// 确保 MIGRATION_VERSIONS 中每个版本号在 run_migrations 中都有对应 match 分支
    /// 使用空数据库运行所有迁移，验证不会 panic 且所有表正确创建
    #[test]
    fn migration_versions_have_handlers() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        // 只创建最基础的 sessions 表（v1 迁移依赖它）
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
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
                api_model TEXT
            );
        

            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL DEFAULT '',
                mode TEXT DEFAULT 'native',
                timestamp INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS inspirations (
                id TEXT PRIMARY KEY,
                icon TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL,
                content TEXT NOT NULL DEFAULT '',
                source_agent TEXT DEFAULT 'manual',
                is_favorite INTEGER DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS inspiration_tags (
                inspiration_id TEXT NOT NULL,
                tag TEXT NOT NULL,
                PRIMARY KEY (inspiration_id, tag)
            );
"
        ).unwrap();

        // 从版本 0 开始运行所有迁移
        let result = run_migrations(&conn, 0);
        assert!(result.is_ok(), "迁移执行失败: {:?}", result.err());

        // 验证关键工作流表已创建
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"workflow_definitions".to_string()), "workflow_definitions 表未创建 ");
        assert!(tables.contains(&"workflow_instances".to_string()), "workflow_instances 表未创建 ");
        assert!(tables.contains(&"node_executions".to_string()), "node_executions 表未创建 ");
        assert!(tables.contains(&"node_execution_logs".to_string()), "node_execution_logs 表未创建 ");
        assert!(tables.contains(&"workflow_schedules".to_string()), "workflow_schedules 表未创建 ");
        assert!(tables.contains(&"workflow_versions".to_string()), "workflow_versions 表未创建 ");

    }
}
