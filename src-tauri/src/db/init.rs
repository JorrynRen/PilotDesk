use rusqlite::{params, Connection};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use crate::utils::paths::db_path;
use crate::utils::errors::AppError;
use std::fs;

const MIGRATION_VERSION: i64 = 6;

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
            agent_type TEXT NOT NULL CHECK(agent_type IN ('claude', 'hermes', 'api', 'codex')),
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
            source_agent TEXT DEFAULT 'manual' CHECK(source_agent IN ('claude', 'hermes', 'codex', 'api', 'manual')),
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

    // Versioned migrations
    let current_version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap_or(0);

    if current_version < 1 {
        migrate_add_api_columns(&conn)?;
    }
    if current_version < 2 {
        migrate_add_type(&conn)?;
    }
    if current_version < 3 {
        migrate_add_api_providers(&conn)?;
    }
    if current_version < 4 {
        migrate_add_app_settings(&conn)?;
    }
    if current_version < 5 {
        migrate_add_install_logs(&conn)?;
    }
    if current_version < 6 {
        migrate_add_message_extensions(&conn)?;
    }

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
            agent_type TEXT NOT NULL CHECK(agent_type IN ('claude', 'hermes', 'api', 'codex')),
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
