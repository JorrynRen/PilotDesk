use crate::tools::{ModelSpec, ProviderModelInfo};
use crate::utils::crypto;
use crate::utils::errors::AppError;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiProvider {
    pub id: String,
    pub name: String,
    pub api_endpoint: String,
    pub api_key_masked: String,
    pub api_key_set: bool,
    pub models: Vec<String>,
    pub api_format: String,
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
    pub api_format: Option<String>,
    pub sort_order: Option<i64>,
}

/// 解析 models 列：对象数组 [{name, note}]（A 方案，唯一事实源）；兼容旧格式字符串数组 ["a","b"]。
fn parse_model_entries(models_json: &str) -> Vec<(String, String)> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(models_json) {
        if let Some(arr) = v.as_array() {
            let mut entries = Vec::with_capacity(arr.len());
            for x in arr {
                if let Some(name) = x.get("name").and_then(|n| n.as_str()) {
                    let note = x
                        .get("note")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    entries.push((name.to_string(), note));
                }
            }
            return entries;
        }
    }
    // 兼容旧格式：纯模型名字符串数组
    serde_json::from_str::<Vec<String>>(models_json)
        .unwrap_or_default()
        .into_iter()
        .map(|n| (n, String::new()))
        .collect()
}

fn row_to_provider(row: &rusqlite::Row) -> rusqlite::Result<ApiProvider> {
    let models_json: String = row.get("models")?;
    let models: Vec<String> = parse_model_entries(&models_json)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let api_format: String = row
        .get("api_format")
        .unwrap_or_else(|_| "openai".to_string());
    Ok(ApiProvider {
        id: row.get("id")?,
        name: row.get("name")?,
        api_endpoint: row.get("api_endpoint")?,
        api_key_masked: row.get("api_key_masked")?,
        api_key_set: row.get::<_, i64>("api_key_set")? != 0,
        models,
        api_format,
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
    let providers = stmt
        .query_map([], row_to_provider)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(providers)
}

/// Get a single API provider by ID
pub fn get_api_provider(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<ApiProvider>, AppError> {
    conn.query_row(
        "SELECT id, name, api_endpoint, api_key_masked, api_key_set, models, sort_order, created_at, updated_at
         FROM api_providers WHERE id = ?",
        params![id],
        row_to_provider,
    ).optional().map_err(Into::into)
}

/// Get the raw API key for a provider (not exposed to frontend in list)
pub fn get_api_key(conn: &rusqlite::Connection, id: &str) -> Result<Option<String>, AppError> {
    let key: Option<String> = conn
        .query_row(
            "SELECT api_key FROM api_providers WHERE id = ?",
            params![id],
            |row| row.get("api_key"),
        )
        .optional()?;
    match key.filter(|k| !k.is_empty()) {
        Some(encrypted) => crypto::decrypt(&encrypted)
            .map(Some)
            .map_err(|e| AppError::Config(format!("解密 API Key 失败: {}", e))),
        None => Ok(None),
    }
}

/// Create or update an API provider
pub fn upsert_api_provider(
    conn: &rusqlite::Connection,
    data: &CreateOrUpdateProvider,
) -> Result<ApiProvider, AppError> {
    let now = crate::utils::now();
    let sort_order = data.sort_order.unwrap_or(now);

    // models 列 = 模型清单+备注（对象数组，唯一事实源）：按 data.models 重建，
    // 保留已有模型备注；被移除的模型备注随之丢弃，消除孤儿数据。
    let existing_notes: HashMap<String, String> = {
        let cur: Option<String> = conn
            .query_row(
                "SELECT models FROM api_providers WHERE id = ?",
                params![data.id],
                |r| r.get(0),
            )
            .optional()?;
        cur.as_deref()
            .map(parse_model_entries)
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    let models_json = serde_json::to_string(
        &data
            .models
            .iter()
            .map(|name| serde_json::json!({ "name": name, "note": existing_notes.get(name).cloned().unwrap_or_default() }))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());

    let (encrypted_key, masked, key_set) = match &data.api_key {
        Some(key) if !key.is_empty() => {
            let encrypted = crypto::encrypt(key)
                .map_err(|e| AppError::Config(format!("加密 API Key 失败: {}", e)))?;
            // 掩码按字符取：Key 一般是 ASCII，但这里是自由输入文本，中文/特殊字符按字节切会 panic
            let masked = if key.chars().count() > 8 {
                format!(
                    "{}****{}",
                    crate::utils::text::head_chars(key, 4),
                    crate::utils::text::tail_chars(key, 4)
                )
            } else {
                "****".to_string()
            };
            (encrypted, masked, 1i64)
        }
        _ => {
            // Keep existing key info if not updating
            let existing = get_api_provider(conn, &data.id)?;
            match existing {
                Some(e) => (
                    String::new(),
                    e.api_key_masked,
                    if e.api_key_set { 1 } else { 0 },
                ),
                None => (String::new(), "".to_string(), 0),
            }
        }
    };

    let api_format = data
        .api_format
        .clone()
        .unwrap_or_else(|| "openai".to_string());

    conn.execute(
        "INSERT INTO api_providers (id, name, api_endpoint, api_key, api_key_masked, api_key_set, models, api_format, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, NULLIF(?4, ''), ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(id) DO UPDATE SET
            name = ?2, api_endpoint = ?3, api_key = COALESCE(NULLIF(?4, ''), api_key),
            api_key_masked = ?5, api_key_set = ?6, models = ?7, api_format = ?8,
            sort_order = ?9, updated_at = ?11",
        params![
            data.id, data.name, data.api_endpoint,
            encrypted_key, masked, key_set, models_json, api_format, sort_order, now, now
        ],
    )?;

    let provider = get_api_provider(conn, &data.id)?.ok_or_else(|| {
        AppError::NotFound(format!("Provider {} not found after upsert", data.id))
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
        let now = crate::utils::now();
        conn.execute(
            "UPDATE api_providers SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            params![i as i64, now, id],
        )?;
    }
    Ok(())
}

// ── 模型备注（model_notes）──
// 模型清单与备注统一存于 api_providers.models 列（对象数组 [{name,note}]，A 方案唯一事实源）。
// get/set 命令签名不变，前端零改动。

/// 读取全部模型备注（providerId → modelName → 备注，含空备注）。
pub fn get_model_notes(
    conn: &rusqlite::Connection,
) -> Result<HashMap<String, HashMap<String, String>>, AppError> {
    let mut stmt = conn.prepare("SELECT id, models FROM api_providers")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut map: HashMap<String, HashMap<String, String>> = HashMap::new();
    for r in rows {
        let (pid, models_json) = r?;
        map.insert(pid, parse_model_entries(&models_json).into_iter().collect());
    }
    Ok(map)
}

/// 保存模型备注（逐模型更新 models 列中对应 name 的 note；不存在的模型行自动补齐）。
pub fn set_model_notes(
    conn: &rusqlite::Connection,
    notes: &HashMap<String, HashMap<String, String>>,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    for (pid, mnotes) in notes {
        let cur: Option<String> = conn
            .query_row(
                "SELECT models FROM api_providers WHERE id = ?",
                params![pid],
                |r| r.get(0),
            )
            .optional()?;
        let mut entries: Vec<(String, String)> =
            cur.as_deref().map(parse_model_entries).unwrap_or_default();
        for (name, note) in mnotes {
            match entries.iter_mut().find(|(n, _)| n == name) {
                Some(e) => e.1 = note.clone(),
                None => entries.push((name.clone(), note.clone())),
            }
        }
        let json = serde_json::to_string(
            &entries
                .iter()
                .map(|(n, note)| serde_json::json!({ "name": n, "note": note }))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "UPDATE api_providers SET models = ?1, updated_at = ?2 WHERE id = ?3",
            params![json, now, pid],
        )?;
    }
    Ok(())
}

/// 收集所有提供商的模型清单（合并备注），供 list_models 工具使用；**不含 key**。
pub fn collect_provider_models(conn: &rusqlite::Connection) -> Vec<ProviderModelInfo> {
    let all_notes = get_model_notes(conn).unwrap_or_default();
    list_api_providers(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let p_notes = all_notes.get(&p.id).cloned().unwrap_or_default();
            let models = p
                .models
                .iter()
                .map(|name| ModelSpec {
                    name: name.clone(),
                    description: p_notes.get(name).cloned(),
                })
                .collect();
            ProviderModelInfo {
                provider_id: p.id,
                provider_name: p.name,
                endpoint: p.api_endpoint,
                api_format: p.api_format,
                models,
                is_session: false,
            }
        })
        .collect()
}

#[tauri::command]
pub fn get_model_notes_cmd(
    state: tauri::State<'_, crate::DbState>,
) -> Result<HashMap<String, HashMap<String, String>>, String> {
    let conn = state.get_conn()?;
    get_model_notes(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_model_notes_cmd(
    state: tauri::State<'_, crate::DbState>,
    notes: HashMap<String, HashMap<String, String>>,
) -> Result<(), String> {
    let conn = state.get_conn()?;
    set_model_notes(&conn, &notes).map_err(|e| e.to_string())
}
