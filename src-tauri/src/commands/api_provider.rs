use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use crate::utils::errors::AppError;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiProvider {
    pub id: String,
    pub name: String,
    pub apiEndpoint: String,
    pub apiKeyMasked: String,
    pub apiKeySet: bool,
    pub models: Vec<String>,
    pub sortOrder: i64,
    pub createdAt: i64,
    pub updatedAt: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrUpdateProvider {
    pub id: String,
    pub name: String,
    pub apiEndpoint: String,
    pub apiKey: Option<String>,
    pub models: Vec<String>,
    pub sortOrder: Option<i64>,
}

fn row_to_provider(row: &rusqlite::Row) -> rusqlite::Result<ApiProvider> {
    let models_json: String = row.get("models")?;
    let models: Vec<String> = serde_json::from_str(&models_json).unwrap_or_default();
    Ok(ApiProvider {
        id: row.get("id")?,
        name: row.get("name")?,
        apiEndpoint: row.get("api_endpoint")?,
        apiKeyMasked: row.get("api_key_masked")?,
        apiKeySet: row.get::<_, i64>("api_key_set")? != 0,
        models,
        sortOrder: row.get("sort_order")?,
        createdAt: row.get("created_at")?,
        updatedAt: row.get("updated_at")?,
    })
}

/// List all API providers, ordered by sort_order
pub fn list_api_providers(conn: &rusqlite::Connection) -> Result<Vec<ApiProvider>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, api_endpoint, api_key_masked, api_key_set, models, sort_order, created_at, updated_at
         FROM api_providers ORDER BY sort_order"
    )?;
    let providers = stmt.query_map([], row_to_provider)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(providers)
}

/// Get a single API provider by ID
pub fn get_api_provider(conn: &rusqlite::Connection, id: &str) -> Result<Option<ApiProvider>, AppError> {
    conn.query_row(
        "SELECT id, name, api_endpoint, api_key_masked, api_key_set, models, sort_order, created_at, updated_at
         FROM api_providers WHERE id = ?",
        params![id],
        row_to_provider,
    ).optional().map_err(Into::into)
}

/// Get the raw API key for a provider (not exposed to frontend in list)
pub fn get_api_key(conn: &rusqlite::Connection, id: &str) -> Result<Option<String>, AppError> {
    let key: Option<String> = conn.query_row(
        "SELECT api_key FROM api_providers WHERE id = ?",
        params![id],
        |row| row.get("api_key"),
    ).optional()?;
    // Filter out empty strings
    Ok(key.filter(|k| !k.is_empty()))
}

/// Create or update an API provider
pub fn upsert_api_provider(conn: &rusqlite::Connection, data: &CreateOrUpdateProvider) -> Result<ApiProvider, AppError> {
    let now = chrono::Utc::now().timestamp();
    let models_json = serde_json::to_string(&data.models).unwrap_or_else(|_| "[]".to_string());
    let sort_order = data.sortOrder.unwrap_or(now);

    let (masked, key_set) = match &data.apiKey {
        Some(key) if !key.is_empty() => {
            let masked = if key.len() > 8 {
                format!("{}****{}", &key[..4], &key[key.len()-4..])
            } else {
                "****".to_string()
            };
            (masked, 1i64)
        }
        _ => {
            // Keep existing key info if not updating
            let existing = get_api_provider(conn, &data.id)?;
            match existing {
                Some(e) => (e.apiKeyMasked, if e.apiKeySet { 1 } else { 0 }),
                None => ("".to_string(), 0),
            }
        }
    };

    conn.execute(
        "INSERT INTO api_providers (id, name, api_endpoint, api_key, api_key_masked, api_key_set, models, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, NULLIF(?4, ''), ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            name = ?2, api_endpoint = ?3, api_key = COALESCE(NULLIF(?4, ''), api_key),
            api_key_masked = ?5, api_key_set = ?6, models = ?7,
            sort_order = ?8, updated_at = ?10",
        params![
            data.id, data.name, data.apiEndpoint,
            data.apiKey.as_deref().unwrap_or(""),
            masked, key_set, models_json, sort_order, now, now
        ],
    )?;

    let provider = get_api_provider(conn, &data.id)?
        .ok_or_else(|| AppError {
            code: "ERR_NOT_FOUND".into(),
            message: format!("Provider {} not found after upsert", data.id),
            details: None,
        })?;
    Ok(provider)
}

/// Delete an API provider by ID
pub fn delete_api_provider(conn: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM api_providers WHERE id = ?", params![id])?;
    Ok(())
}

/// Reorder providers: save the full ordered list
pub fn reorder_api_providers(conn: &rusqlite::Connection, ids: &[String]) -> Result<(), AppError> {
    for (i, id) in ids.iter().enumerate() {
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE api_providers SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            params![i as i64, now, id],
        )?;
    }
    Ok(())
}
