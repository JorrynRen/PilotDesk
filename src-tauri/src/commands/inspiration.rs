use rusqlite::{params, Connection};
use crate::utils::errors::AppError;
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct Inspiration {
    pub id: String,
    pub icon: String,
    pub title: String,
    pub content: String,
    pub source_agent: String,
    pub is_favorite: bool,
    pub tags: Vec<String>,
    pub created_at: f64,
    pub updated_at: f64,
}

#[derive(serde::Deserialize)]
pub struct CreateInspirationPayload {
    pub icon: Option<String>,
    pub title: String,
    pub content: String,
    pub source_agent: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
pub struct UpdateInspirationPayload {
    pub id: String,
    pub icon: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub source_agent: Option<String>,
    pub is_favorite: Option<bool>,
    pub tags: Option<Vec<String>>,
}

fn row_to_inspiration(row: &rusqlite::Row<'_>) -> Result<Inspiration, rusqlite::Error> {
    Ok(Inspiration {
        id: row.get("id")?,
        icon: row.get("icon")?,
        title: row.get("title")?,
        content: row.get("content")?,
        source_agent: row.get("source_agent")?,
        is_favorite: row.get::<_, i32>("is_favorite")? != 0,
        tags: vec![], // Populated separately
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn load_tags(conn: &Connection, inspiration_id: &str) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT tag FROM inspiration_tags WHERE inspiration_id = ?1 ORDER BY tag"
    )?;
    let tags: Vec<String> = stmt
        .query_map(params![inspiration_id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tags)
}

/// List all inspirations with optional tag filter
pub fn list_inspirations(conn: &Connection, tag: Option<String>, favorite_only: bool) -> Result<Vec<Inspiration>, AppError> {
    let mut sql = String::from(
        "SELECT id, icon, title, content, source_agent, is_favorite, created_at, updated_at FROM inspirations"
    );
    let mut conditions: Vec<String> = Vec::new();

    if favorite_only {
        conditions.push("is_favorite = 1".to_string());
    }

    if let Some(ref t) = tag {
        if !t.is_empty() {
            conditions.push(format!("id IN (SELECT inspiration_id FROM inspiration_tags WHERE tag = '{}')", t.replace("'", "''")));
        }
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    sql.push_str(" ORDER BY updated_at DESC");

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Inspiration> = stmt
        .query_map([], row_to_inspiration)?
        .filter_map(|r| r.ok())
        .map(|mut insp| {
            insp.tags = load_tags(conn, &insp.id).unwrap_or_default();
            insp
        })
        .collect();
    Ok(rows)
}

/// Get a single inspiration by ID
pub fn get_inspiration(conn: &Connection, id: String) -> Result<Inspiration, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, icon, title, content, source_agent, is_favorite, created_at, updated_at FROM inspirations WHERE id = ?1"
    )?;
    let mut rows = stmt.query_map(params![id], row_to_inspiration)?;
    let mut insp = rows.next().ok_or(AppError {
        code: "ERR_NOT_FOUND".to_string(),
        message: "灵感不存在".to_string(),
        details: None,
    })??;
    insp.tags = load_tags(conn, &insp.id)?;
    Ok(insp)
}

/// Create a new inspiration
pub fn create_inspiration(conn: &Connection, payload: CreateInspirationPayload) -> Result<Inspiration, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp() as f64;
    let icon = payload.icon.unwrap_or_else(|| "💡".to_string());
    let source_agent = payload.source_agent.unwrap_or_else(|| "manual".to_string());

    conn.execute(
        "INSERT INTO inspirations (id, icon, title, content, source_agent, is_favorite, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6)",
        params![id, icon, payload.title, payload.content, source_agent, now],
    )?;

    // Insert tags
    if let Some(tags) = payload.tags {
        let mut tag_stmt = conn.prepare("INSERT OR IGNORE INTO inspiration_tags (inspiration_id, tag) VALUES (?1, ?2)")?;
        for tag in tags {
            tag_stmt.execute(params![id, tag])?;
        }
    }

    // Trigger FTS5 reindex
    conn.execute("INSERT INTO inspirations_fts(inspirations_fts) VALUES('rebuild')", [])?;

    get_inspiration(conn, id)
}

/// Update an existing inspiration
pub fn update_inspiration(conn: &Connection, payload: UpdateInspirationPayload) -> Result<Inspiration, AppError> {
    let now = chrono::Utc::now().timestamp() as f64;

    // Check existence
    let existing = get_inspiration(conn, payload.id.clone())?;

    // Update fields
    let icon = payload.icon.unwrap_or(existing.icon);
    let title = payload.title.unwrap_or(existing.title);
    let content = payload.content.unwrap_or(existing.content);
    let source_agent = payload.source_agent.unwrap_or(existing.source_agent);
    let is_favorite = payload.is_favorite.unwrap_or(existing.is_favorite) as i32;

    conn.execute(
        "UPDATE inspirations SET icon = ?1, title = ?2, content = ?3, source_agent = ?4, is_favorite = ?5, updated_at = ?6 WHERE id = ?7",
        params![icon, title, content, source_agent, is_favorite, now, payload.id],
    )?;

    // Update tags if provided
    if let Some(tags) = payload.tags {
        // Delete existing tags
        conn.execute("DELETE FROM inspiration_tags WHERE inspiration_id = ?1", params![payload.id])?;
        // Insert new tags
        let mut tag_stmt = conn.prepare("INSERT OR IGNORE INTO inspiration_tags (inspiration_id, tag) VALUES (?1, ?2)")?;
        for tag in tags {
            tag_stmt.execute(params![payload.id, tag])?;
        }
    }

    // Reindex FTS5
    conn.execute("INSERT INTO inspirations_fts(inspirations_fts) VALUES('rebuild')", [])?;

    get_inspiration(conn, payload.id)
}

/// Delete an inspiration
pub fn delete_inspiration(conn: &Connection, id: String) -> Result<(), AppError> {
    let rows = conn.execute("DELETE FROM inspirations WHERE id = ?1", params![id])?;
    if rows == 0 {
        return Err(AppError {
            code: "ERR_NOT_FOUND".to_string(),
            message: "灵感不存在".to_string(),
            details: None,
        });
    }
    Ok(())
}

/// Full-text search using FTS5
pub fn search_inspirations(conn: &Connection, query: String, limit: u32) -> Result<Vec<Inspiration>, AppError> {
    let limit = limit.max(1).min(100);
    let sql = format!(
        "SELECT i.id, i.icon, i.title, i.content, i.source_agent, i.is_favorite, i.created_at, i.updated_at \
         FROM inspirations i \
         JOIN inspirations_fts fts ON i.rowid = fts.rowid \
         WHERE inspirations_fts MATCH ?1 \
         ORDER BY rank \
         LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Inspiration> = stmt
        .query_map(params![query, limit], row_to_inspiration)?
        .filter_map(|r| r.ok())
        .map(|mut insp| {
            insp.tags = load_tags(conn, &insp.id).unwrap_or_default();
            insp
        })
        .collect();
    Ok(rows)
}

/// Get all unique tags
pub fn list_tags(conn: &Connection) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare("SELECT DISTINCT tag FROM inspiration_tags ORDER BY tag")?;
    let tags: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tags)
}
