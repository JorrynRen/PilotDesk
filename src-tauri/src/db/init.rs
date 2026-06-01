use rusqlite::{Connection, Result};
use crate::utils::paths::db_path;
use crate::utils::errors::AppError;
use std::fs;

pub fn init_db() -> Result<Connection, AppError> {
    let db_path = db_path();
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let conn = Connection::open(&db_path)?;
    
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;"
    )?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            agent_type TEXT NOT NULL CHECK(agent_type IN ('claude', 'hermes', 'api')),
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
            source_agent TEXT DEFAULT 'manual' CHECK(source_agent IN ('claude', 'hermes', 'manual')),
            is_favorite INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS inspiration_tags (
            inspiration_id TEXT NOT NULL REFERENCES inspirations(id) ON DELETE CASCADE,
            tag TEXT NOT NULL,
            PRIMARY KEY (inspiration_id, tag)
        );

        CREATE TABLE IF NOT EXISTS bot_channels (
            id TEXT PRIMARY KEY,
            agent_type TEXT NOT NULL CHECK(agent_type IN ('claude', 'hermes')),
            platform TEXT NOT NULL DEFAULT 'wechat',
            method TEXT DEFAULT 'clawbot',
            status TEXT DEFAULT 'disconnected',
            trigger_prefix TEXT DEFAULT '',
            response_format TEXT DEFAULT 'markdown',
            config TEXT DEFAULT '{}',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS inspirations_fts USING fts5(title, content, content=inspirations, content_rowid=rowid);
        CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_type, updated_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status, updated_at);
        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_inspirations_favorite ON inspirations(is_favorite, updated_at);
        CREATE INDEX IF NOT EXISTS idx_inspirations_tags ON inspiration_tags(tag);"
    )?;

    // Migration: add api_provider / api_model columns to existing sessions table
    migrate_add_api_columns(&conn)?;

    // Migration: rebuild sessions table to support 'api' agent_type
    migrate_add_api_agent_type(&conn)?;

    // Create api_providers table (migrated from localStorage)
    migrate_add_api_providers(&conn)?;

    // Create app_settings table (key-value settings storage)
    migrate_add_app_settings(&conn)?;

    Ok(conn)
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
    Ok(())
}

/// Migration: ensure api_provider and api_model columns exist on sessions table
fn migrate_add_api_columns(conn: &Connection) -> Result<(), AppError> {
    // Check if api_provider column exists
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

/// Migration: rebuild sessions table to support 'api' agent_type
/// (old CHECK constraint only allowed 'claude'/'hermes')
fn migrate_add_api_agent_type(conn: &Connection) -> Result<(), AppError> {
    // Check if 'api' is accepted by trying a dry run
    let accepts_api = conn
        .execute("INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at) VALUES ('__migration_test_api', 'api', '', '', 0, 0)", [])
        .is_ok();

    if accepts_api {
        // Clean up test row
        conn.execute("DELETE FROM sessions WHERE id = '__migration_test_api'", [])?;
        return Ok(());
    }

    // Need to rebuild table with updated CHECK constraint
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions_new (
            id TEXT PRIMARY KEY,
            agent_type TEXT NOT NULL CHECK(agent_type IN ('claude', 'hermes', 'api')),
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
