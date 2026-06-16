use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use crate::utils::errors::AppError;
use crate::utils::crypto;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiProvider {
    pub id: String,
    pub name: String,
    pub api_endpoint: String,
    pub api_key_masked: String,
    pub api_key_set: bool,
    pub models: Vec<String>,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrUpdateProvider {
    pub id: String,
    pub name: String,
    pub api_endpoint: String,
    pub api_key: Option<String>,
    pub models: Vec<String>,
    pub sort_order: Option<i64>,
}

fn row_to_provider(row: &rusqlite::Row) -> rusqlite::Result<ApiProvider> {
    let models_json: String = row.get("models")?;
    let models: Vec<String> = serde_json::from_str(&models_json).unwrap_or_default();
    Ok(ApiProvider {
        id: row.get("id")?,
        name: row.get("name")?,
        api_endpoint: row.get("api_endpoint")?,
        api_key_masked: row.get("api_key_masked")?,
        api_key_set: row.get::<_, i64>("api_key_set")? != 0,
        models,
        sort_order: row.get("sort_order")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
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
    match key.filter(|k| !k.is_empty()) {
        Some(encrypted) => {
            crypto::decrypt(&encrypted)
                .map(Some)
                .map_err(|e| AppError::Config(format!("解密 API Key 失败: {}", e)))
        }
        None => Ok(None),
    }
}

/// Create or update an API provider
pub fn upsert_api_provider(conn: &rusqlite::Connection, data: &CreateOrUpdateProvider) -> Result<ApiProvider, AppError> {
    let now = crate::utils::now();
    let models_json = serde_json::to_string(&data.models).unwrap_or_else(|_| "[]".to_string());
    let sort_order = data.sort_order.unwrap_or(now);

    let (encrypted_key, masked, key_set) = match &data.api_key {
        Some(key) if !key.is_empty() => {
            let encrypted = crypto::encrypt(key)
                .map_err(|e| AppError::Config(format!("加密 API Key 失败: {}", e)))?;
            let masked = if key.len() > 8 {
                format!("{}****{}", &key[..4], &key[key.len()-4..])
            } else {
                "****".to_string()
            };
            (encrypted, masked, 1i64)
        }
        _ => {
            // Keep existing key info if not updating
            let existing = get_api_provider(conn, &data.id)?;
            match existing {
                Some(e) => (String::new(), e.api_key_masked, if e.api_key_set { 1 } else { 0 }),
                None => (String::new(), "".to_string(), 0),
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
            data.id, data.name, data.api_endpoint,
            encrypted_key, masked, key_set, models_json, sort_order, now, now
        ],
    )?;

    let provider = get_api_provider(conn, &data.id)?
        .ok_or_else(|| AppError::NotFound(format!("Provider {} not found after upsert", data.id)))?;
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
        let now = crate::utils::now();
        conn.execute(
            "UPDATE api_providers SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            params![i as i64, now, id],
        )?;
    }
    Ok(())
}
