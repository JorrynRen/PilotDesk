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
            agent_type TEXT NOT NULL CHECK(agent_type IN ('claude', 'hermes')),
            title TEXT NOT NULL DEFAULT '',
            cwd TEXT DEFAULT '',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_message_preview TEXT DEFAULT '',
            message_count INTEGER DEFAULT 0,
            status TEXT DEFAULT 'active' CHECK(status IN ('active', 'archived'))
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

    Ok(conn)
}
