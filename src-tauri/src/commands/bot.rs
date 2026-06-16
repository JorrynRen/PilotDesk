use rusqlite::{params, Connection};
use crate::utils::errors::AppError;
pub use crate::db::models::BotChannel;

#[derive(serde::Deserialize)]
pub struct SaveBotChannelPayload {
    pub id: Option<String>,
    pub agent_type: String,
    pub platform: Option<String>,
    pub method: Option<String>,
    pub status: Option<String>,
    pub trigger_prefix: Option<String>,
    pub response_format: Option<String>,
    pub config: Option<serde_json::Value>,
}

fn row_to_bot_channel(row: &rusqlite::Row<'_>) -> Result<BotChannel, rusqlite::Error> {
    let config_str: String = row.get("config")?;
    let config: serde_json::Value = serde_json::from_str(&config_str).unwrap_or(serde_json::json!({}));
    Ok(BotChannel {
        id: row.get("id")?,
        agent_type: row.get("agent_type")?,
        platform: row.get("platform")?,
        method: row.get("method")?,
        status: row.get("status")?,
        trigger_prefix: row.get("trigger_prefix")?,
        response_format: row.get("response_format")?,
        config,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// List all bot channels
pub fn list_bot_channels(conn: &Connection) -> Result<Vec<BotChannel>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, platform, method, status, trigger_prefix, response_format, config, created_at, updated_at \
         FROM bot_channels ORDER BY updated_at DESC"
    )?;
    let channels: Vec<BotChannel> = stmt
        .query_map([], row_to_bot_channel)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(channels)
}

/// Get a single bot channel
pub fn get_bot_channel(conn: &Connection, id: String) -> Result<BotChannel, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_type, platform, method, status, trigger_prefix, response_format, config, created_at, updated_at \
         FROM bot_channels WHERE id = ?1"
    )?;
    let mut rows = stmt.query_map(params![id], row_to_bot_channel)?;
    rows.next()
        .ok_or(AppError::NotFound("Bot 通道不存在".to_string()))?
        .map_err(|e| AppError::Db(e.to_string()))
}

/// Create or update a bot channel
pub fn save_bot_channel(conn: &Connection, payload: SaveBotChannelPayload) -> Result<BotChannel, AppError> {
    let now = crate::utils::now();

    // 默认值统一计算（避免两个分支重复）
    let platform = payload.platform.unwrap_or_else(|| "wechat".to_string());
    let method = payload.method.unwrap_or_else(|| "clawbot".to_string());
    let status = payload.status.unwrap_or_else(|| "disconnected".to_string());
    let trigger_prefix = payload.trigger_prefix.unwrap_or_default();
    let response_format = payload.response_format.unwrap_or_else(|| "markdown".to_string());
    let config_str = payload
        .config
        .map(|c| serde_json::to_string(&c).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());

    let id = match payload.id {
        Some(existing_id) => {
            conn.execute(
                "UPDATE bot_channels SET agent_type = ?1, platform = ?2, method = ?3, status = ?4, \
                 trigger_prefix = ?5, response_format = ?6, config = ?7, updated_at = ?8 WHERE id = ?9",
                params![payload.agent_type, platform, method, status, trigger_prefix, response_format, config_str, now, existing_id],
            )?;
            existing_id
        }
        None => {
            let new_id = crate::utils::new_id();
            conn.execute(
                "INSERT INTO bot_channels (id, agent_type, platform, method, status, trigger_prefix, response_format, config, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![new_id, payload.agent_type, platform, method, status, trigger_prefix, response_format, config_str, now],
            )?;
            new_id
        }
    };
    get_bot_channel(conn, id)
}

/// Delete a bot channel
pub fn delete_bot_channel(conn: &Connection, id: String) -> Result<(), AppError> {
    let rows = conn.execute("DELETE FROM bot_channels WHERE id = ?1", params![id])?;
    if rows == 0 {
        return Err(AppError::NotFound("Bot 通道不存在".to_string()));
    }
    Ok(())
}
