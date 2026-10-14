use crate::tools::{ModelSpec, ProviderModelInfo};
use crate::utils::crypto;
use crate::utils::errors::AppError;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// v9 物理合并后的统一存储映射（`api_providers` → `agents` + `agent_secrets`）：
/// - provider id `p` ↔ `agents.agent_type = 'api:<p>'`，`transport = 'http'`；
/// - 连接参数存 `agents.conn`（JSON：`{baseUrl, apiFormat, models:[{name,note}]}`）——**不含密钥**；
/// - 密钥存独立表 `agent_secrets`，行 id = `agents.secret_ref`（= `secret:<p>`），值为**密文**。
///
/// 本文件所有对外函数的签名 / 返回结构与合并前**完全一致**，因此命令层、知识库、语音、
/// 圆桌、组织上报、工作流、前端 store 全部零改动。
///
/// 旧表 `api_providers` 自本版本起**冻结为只读遗留表**：运行期不再读写它，
/// 仅由 `db/init.rs` 的一次性迁移读取一次（见 `backfill_api_providers_into_agents`）。
const AGENT_PREFIX: &str = "api:";
const SECRET_PREFIX: &str = "secret:";

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

// ── 映射辅助 ──────────────────────────────────────────────

/// provider id → `agents.agent_type`（`'p'` → `'api:p'`）。
/// **幂等**：已带 `api:` 前缀的输入原样返回，故 `sessions.api_provider` 无论是裸 id
/// 还是统一写法 `api:<id>` 都能解析（旧值可回退，无需数据迁移）。
fn agent_type_of(id: &str) -> String {
    if id.starts_with(AGENT_PREFIX) {
        id.to_string()
    } else {
        format!("{}{}", AGENT_PREFIX, id)
    }
}

/// provider id → `agent_secrets.id`（`'p'` / `'api:p'` → `'secret:p'`）。
/// 同样先剥离可能存在的 `api:` 前缀，保证两种写法指向同一份凭据。
fn secret_id_of(id: &str) -> String {
    let bare = id.strip_prefix(AGENT_PREFIX).unwrap_or(id);
    format!("{}{}", SECRET_PREFIX, bare)
}

/// `agents.agent_type` → provider id（`'api:p'` → `Some('p')`；非 `api:` 前缀 → `None`）
fn provider_id_of(agent_type: &str) -> Option<String> {
    agent_type.strip_prefix(AGENT_PREFIX).map(|s| s.to_string())
}

/// 解析 models 列/字段：对象数组 [{name, note}]（A 方案，唯一事实源）；兼容旧格式字符串数组 ["a","b"]。
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

/// `agents.conn` 的解析结果（连接参数，不含密钥）。
#[derive(Default)]
struct ConnParams {
    base_url: String,
    api_format: String,
    models_json: String,
}

/// 解析 `agents.conn`（JSON）：缺字段一律回退空串 / `[]`，坏 JSON 不 panic。
fn parse_conn(conn_json: &str) -> ConnParams {
    let v: serde_json::Value =
        serde_json::from_str(conn_json).unwrap_or(serde_json::Value::Null);
    ConnParams {
        base_url: v
            .get("baseUrl")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        api_format: v
            .get("apiFormat")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        models_json: v
            .get("models")
            .map(|m| m.to_string())
            .unwrap_or_else(|| "[]".to_string()),
    }
}

/// 由连接参数组装 `agents.conn` JSON（models 为对象数组 `[{name,note}]`）。
fn build_conn_json(base_url: &str, api_format: &str, entries: &[(String, String)]) -> String {
    let models: Vec<serde_json::Value> = entries
        .iter()
        .map(|(n, note)| serde_json::json!({ "name": n, "note": note }))
        .collect();
    serde_json::json!({
        "baseUrl": base_url,
        "apiFormat": api_format,
        "models": models,
    })
    .to_string()
}

/// 由密文计算脱敏掩码与是否已配置：Key 字符数 > 8 → 前 4 + `****` + 后 4，否则 `****`。
/// 解密失败（密文损坏 / 换机）时只回 `****`，不泄露任何内容。
fn mask_from_cipher(cipher: &str) -> (bool, String) {
    if cipher.is_empty() {
        return (false, String::new());
    }
    match crypto::decrypt(cipher) {
        Ok(plain) => {
            let masked = if plain.chars().count() > 8 {
                format!(
                    "{}****{}",
                    crate::utils::text::head_chars(&plain, 4),
                    crate::utils::text::tail_chars(&plain, 4)
                )
            } else {
                "****".to_string()
            };
            (true, masked)
        }
        Err(_) => (true, "****".to_string()),
    }
}

/// 统一查询：`agents`（transport='http' 且 agent_type LIKE 'api:%'）LEFT JOIN 凭据表。
/// 末列 `COALESCE(s.secret,'')` 即该 provider 的密文（无凭据行时空串）。
const PROVIDER_SELECT: &str =
    "SELECT a.agent_type, a.display_name, COALESCE(a.conn, ''), COALESCE(a.sort_order, 0),
            a.created_at, a.updated_at, COALESCE(s.secret, '')
     FROM agents a
     LEFT JOIN agent_secrets s ON s.id = a.secret_ref
     WHERE a.transport = 'http' AND a.agent_type LIKE 'api:%'";

/// 一行 → `ApiProvider`（id 去掉 `api:` 前缀回原文；掩码由密文现算）。
fn row_to_provider(row: &rusqlite::Row) -> rusqlite::Result<ApiProvider> {
    let agent_type: String = row.get(0)?;
    let conn_json: String = row.get(2)?;
    let cipher: String = row.get(6)?;
    let cp = parse_conn(&conn_json);
    let api_format = if cp.api_format.trim().is_empty() {
        "openai".to_string()
    } else {
        cp.api_format
    };
    let (api_key_set, api_key_masked) = mask_from_cipher(&cipher);
    Ok(ApiProvider {
        id: provider_id_of(&agent_type).unwrap_or(agent_type),
        name: row.get(1)?,
        api_endpoint: cp.base_url,
        api_key_masked,
        api_key_set,
        models: parse_model_entries(&cp.models_json)
            .into_iter()
            .map(|(n, _)| n)
            .collect(),
        api_format,
        sort_order: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

// ── 对外函数（签名与合并前一致）────────────────────────────

/// List all API providers, ordered by sort_order
pub fn list_api_providers(conn: &rusqlite::Connection) -> Result<Vec<ApiProvider>, AppError> {
    let mut stmt = conn.prepare(&format!(
        "{} ORDER BY a.sort_order, a.agent_type",
        PROVIDER_SELECT
    ))?;
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
        &format!("{} AND a.agent_type = ?1", PROVIDER_SELECT),
        params![agent_type_of(id)],
        row_to_provider,
    )
    .optional()
    .map_err(Into::into)
}

/// Get the raw API key for a provider (not exposed to frontend in list)
pub fn get_api_key(conn: &rusqlite::Connection, id: &str) -> Result<Option<String>, AppError> {
    let cipher: Option<String> = conn
        .query_row(
            "SELECT COALESCE(s.secret, '') FROM agents a
             JOIN agent_secrets s ON s.id = a.secret_ref
             WHERE a.agent_type = ?1",
            params![agent_type_of(id)],
            |row| row.get(0),
        )
        .optional()?;
    match cipher.filter(|k| !k.is_empty()) {
        Some(encrypted) => crypto::decrypt(&encrypted)
            .map(Some)
            .map_err(|e| AppError::Config(format!("解密 API Key 失败: {}", e))),
        None => Ok(None),
    }
}

/// 读取单个 provider 的协议格式与模型条目（名称+备注），供**组织上报**等需要完整信息的调用方使用。
///
/// 列表接口为省流量省略了模型备注，这里直接查库，保证上报内容（协议族 + 模型清单 + 备注）
/// 与本地配置完全一致。返回 `(api_format, [(模型名, 备注), ...])`；`api_format` 为空时回落到 `openai`。
pub fn get_provider_format_and_models(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<(String, Vec<(String, String)>), AppError> {
    let conn_json: Option<String> = conn
        .query_row(
            "SELECT COALESCE(conn, '') FROM agents WHERE agent_type = ?1",
            params![agent_type_of(id)],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    let cp = parse_conn(conn_json.as_deref().unwrap_or(""));
    let api_format = if cp.api_format.trim().is_empty() {
        "openai".to_string()
    } else {
        cp.api_format
    };
    Ok((api_format, parse_model_entries(&cp.models_json)))
}

/// Create or update an API provider
pub fn upsert_api_provider(
    conn: &rusqlite::Connection,
    data: &CreateOrUpdateProvider,
) -> Result<ApiProvider, AppError> {
    let now = crate::utils::now();
    let sort_order = data.sort_order.unwrap_or(now);
    let agent_type = agent_type_of(&data.id);
    let secret_id = secret_id_of(&data.id);

    // 模型清单+备注（对象数组，唯一事实源）：按 data.models 重建，保留已有模型备注；
    // 被移除的模型备注随之丢弃，消除孤儿数据。
    let existing_notes: HashMap<String, String> = {
        let cur: Option<String> = conn
            .query_row(
                "SELECT COALESCE(conn, '') FROM agents WHERE agent_type = ?1",
                params![agent_type],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        cur.as_deref()
            .map(|j| parse_model_entries(&parse_conn(j).models_json))
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    let entries: Vec<(String, String)> = data
        .models
        .iter()
        .map(|name| {
            (
                name.clone(),
                existing_notes.get(name).cloned().unwrap_or_default(),
            )
        })
        .collect();

    let api_format = data
        .api_format
        .clone()
        .unwrap_or_else(|| "openai".to_string());
    let conn_json = build_conn_json(&data.api_endpoint, &api_format, &entries);

    // 密钥：仅在显式传入非空 Key 时写入凭据表（加密存密文）；
    // Key 为空表示"不修改"，保留原凭据行——掩码/keySet 均由密文在读取时现算。
    if let Some(key) = data.api_key.as_ref().filter(|k| !k.is_empty()) {
        let encrypted = crypto::encrypt(key)
            .map_err(|e| AppError::Config(format!("加密 API Key 失败: {}", e)))?;
        conn.execute(
            "INSERT INTO agent_secrets (id, secret, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(id) DO UPDATE SET secret = ?2, updated_at = ?3",
            params![secret_id, encrypted, now],
        )?;
    }

    conn.execute(
        "INSERT INTO agents
            (agent_type, display_name, description, cli_command,
             transport, conn, secret_ref, sort_order, created_at, updated_at)
         VALUES (?1, ?2, '', '', 'http', ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(agent_type) DO UPDATE SET
            display_name = ?2, conn = ?3, secret_ref = ?4, sort_order = ?5, updated_at = ?6",
        params![agent_type, data.name, conn_json, secret_id, sort_order, now],
    )?;

    let provider = get_api_provider(conn, &data.id)?.ok_or_else(|| {
        AppError::NotFound(format!("Provider {} not found after upsert", data.id))
    })?;
    Ok(provider)
}

/// Delete an API provider by ID（连带删除其凭据行；`api_providers` 遗留表不再触碰）
pub fn delete_api_provider(conn: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    let agent_type = agent_type_of(id);
    conn.execute(
        "DELETE FROM agent_secrets WHERE id = (SELECT secret_ref FROM agents WHERE agent_type = ?1)",
        params![agent_type],
    )?;
    conn.execute("DELETE FROM agents WHERE agent_type = ?1", params![agent_type])?;
    Ok(())
}

/// Reorder providers: save the full ordered list
pub fn reorder_api_providers(conn: &rusqlite::Connection, ids: &[String]) -> Result<(), AppError> {
    for (i, id) in ids.iter().enumerate() {
        let now = crate::utils::now();
        conn.execute(
            "UPDATE agents SET sort_order = ?1, updated_at = ?2 WHERE agent_type = ?3",
            params![i as i64, now, agent_type_of(id)],
        )?;
    }
    Ok(())
}

// ── 模型备注（model_notes）──
// 模型清单与备注统一存于 `agents.conn` 的 models 数组（对象数组 [{name,note}]，A 方案唯一事实源）。
// get/set 命令签名不变，前端零改动。

/// 读取全部模型备注（providerId → modelName → 备注，含空备注）。
pub fn get_model_notes(
    conn: &rusqlite::Connection,
) -> Result<HashMap<String, HashMap<String, String>>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT agent_type, COALESCE(conn, '') FROM agents \
         WHERE transport = 'http' AND agent_type LIKE 'api:%'",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<String, HashMap<String, String>> = HashMap::new();
    for r in rows {
        let (agent_type, conn_json) = r?;
        let Some(pid) = provider_id_of(&agent_type) else {
            continue;
        };
        let entries = parse_model_entries(&parse_conn(&conn_json).models_json);
        map.insert(pid, entries.into_iter().collect());
    }
    Ok(map)
}

/// 保存模型备注（逐模型更新 conn 中 models 数组对应 name 的 note；不存在的模型行自动补齐）。
pub fn set_model_notes(
    conn: &rusqlite::Connection,
    notes: &HashMap<String, HashMap<String, String>>,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    for (pid, mnotes) in notes {
        let cur: Option<String> = conn
            .query_row(
                "SELECT COALESCE(conn, '') FROM agents WHERE agent_type = ?1",
                params![agent_type_of(pid)],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        // 无此 provider：与合并前一致，静默跳过（原 SQL UPDATE 亦为无副作用）。
        let Some(cur_json) = cur else { continue };
        let cp = parse_conn(&cur_json);
        let mut entries: Vec<(String, String)> = parse_model_entries(&cp.models_json);
        for (name, note) in mnotes {
            match entries.iter_mut().find(|(n, _)| n == name) {
                Some(e) => e.1 = note.clone(),
                None => entries.push((name.clone(), note.clone())),
            }
        }
        let new_conn = build_conn_json(&cp.base_url, &cp.api_format, &entries);
        conn.execute(
            "UPDATE agents SET conn = ?1, updated_at = ?2 WHERE agent_type = ?3",
            params![new_conn, now, agent_type_of(pid)],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init::FINAL_SCHEMA_SQL;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(FINAL_SCHEMA_SQL).unwrap();
        conn
    }

    fn payload(id: &str, key: Option<&str>) -> CreateOrUpdateProvider {
        CreateOrUpdateProvider {
            id: id.to_string(),
            name: "甲商".to_string(),
            api_endpoint: "https://api.example.com/v1".to_string(),
            api_key: key.map(|s| s.to_string()),
            models: vec!["m-a".to_string(), "m-b".to_string()],
            api_format: Some("openai".to_string()),
            sort_order: Some(3),
        }
    }

    /// upsert 落到 `agents(transport='http')`：id/名称/端点/模型/sort_order 正确，且**不写**旧表。
    #[test]
    fn upsert_writes_agents_table_but_not_legacy_table() {
        let conn = mem_conn();
        upsert_api_provider(&conn, &payload("p1", None)).unwrap();

        let (agent_type, name, transport, conn_json, secret_ref, sort_order): (
            String,
            String,
            String,
            String,
            Option<String>,
            i64,
        ) = conn
            .query_row(
                "SELECT agent_type, display_name, transport, conn, secret_ref, sort_order
                 FROM agents",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(agent_type, "api:p1");
        assert_eq!(name, "甲商");
        assert_eq!(transport, "http");
        assert_eq!(secret_ref.as_deref(), Some("secret:p1"));
        assert_eq!(sort_order, 3);
        let v: serde_json::Value = serde_json::from_str(&conn_json).unwrap();
        assert_eq!(v["baseUrl"], "https://api.example.com/v1");
        assert_eq!(v["apiFormat"], "openai");
        assert_eq!(v["models"][0]["name"], "m-a");

        // 未传 Key：不得凭空造出凭据行
        let nsec: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nsec, 0);

        // 旧表 `api_providers` 不再被写入（冻结为只读遗留表）
        let nold: i64 = conn
            .query_row("SELECT COUNT(*) FROM api_providers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nold, 0);

        // 读出：裸 id 与 `api:<id>` 两种写法等价
        let a = get_api_provider(&conn, "p1").unwrap().unwrap();
        let b = get_api_provider(&conn, "api:p1").unwrap().unwrap();
        assert_eq!(a.id, "p1");
        assert_eq!(b.id, "p1");
        assert_eq!(a.name, "甲商");
        assert_eq!(a.models, vec!["m-a", "m-b"]);
        assert!(!a.api_key_set);
    }

    /// Key 为空表示"不修改"：保留既有凭据行；掩码由密文现算（解不开也只回 `****`）。
    #[test]
    fn empty_key_keeps_existing_secret_and_masks_from_cipher() {
        let conn = mem_conn();
        upsert_api_provider(&conn, &payload("p1", None)).unwrap();
        // 模拟已配 Key（用不可解密的密文，脱敏路径须安全回退）
        conn.execute(
            "INSERT INTO agent_secrets (id, secret, created_at, updated_at)
             VALUES ('secret:p1', 'CIPHERTEXT', 1, 1)",
            [],
        )
        .unwrap();

        let mut p = payload("p1", None);
        p.name = "改后名".to_string();
        upsert_api_provider(&conn, &p).unwrap();

        // 凭据行原样保留、只一份
        let (n, sec): (i64, String) = (
            conn.query_row("SELECT COUNT(*) FROM agent_secrets", [], |r| r.get(0))
                .unwrap(),
            conn.query_row("SELECT secret FROM agent_secrets", [], |r| r.get(0))
                .unwrap(),
        );
        assert_eq!(n, 1);
        assert_eq!(sec, "CIPHERTEXT");

        let out = get_api_provider(&conn, "p1").unwrap().unwrap();
        assert_eq!(out.name, "改后名");
        assert!(out.api_key_set, "有密文即视为已配 Key");
        assert_eq!(out.api_key_masked, "****");
    }

    /// 排序落到 `agents.sort_order`；删除同时清掉 agents 行与凭据行。
    #[test]
    fn reorder_and_delete_target_agents_and_secrets() {
        let conn = mem_conn();
        upsert_api_provider(&conn, &payload("p1", None)).unwrap();
        upsert_api_provider(&conn, &payload("p2", None)).unwrap();
        conn.execute(
            "INSERT INTO agent_secrets (id, secret, created_at, updated_at)
             VALUES ('secret:p1', 'CIPHER', 1, 1)",
            [],
        )
        .unwrap();

        reorder_api_providers(&conn, &["p2".to_string(), "p1".to_string()]).unwrap();
        let order: Vec<String> = conn
            .prepare("SELECT agent_type FROM agents ORDER BY sort_order")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(order, vec!["api:p2", "api:p1"]);

        delete_api_provider(&conn, "p1").unwrap();
        let nagent: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE agent_type = 'api:p1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let nsec: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_secrets WHERE id = 'secret:p1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nagent, 0);
        assert_eq!(nsec, 0);
    }
}
