use crate::agent_config::{claude, hermes};
use crate::utils::errors::AppError;
use tauri::command;

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConfigResult {
    pub claude: Option<claude::ClaudeConfigPublic>,
    pub hermes: Option<hermes::HermesConfigPublic>,
    pub claude_installed: bool,
    pub hermes_installed: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUpdatePayload {
    pub model: Option<String>,
    pub api_endpoint: Option<String>,
    pub api_key: Option<String>,
    pub mcp_servers: Option<serde_json::Value>,
    pub custom_instructions: Option<String>,
    pub theme: Option<String>,
    pub max_tokens: Option<u32>,
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesUpdatePayload {
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub api_endpoint: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub mcp_servers: Option<serde_json::Value>,
    pub skills_dir: Option<String>,
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Get all agent configurations (masked for security)
/// Always returns config data — even when agent is not "installed" (no config dir),
/// returns default config with null fields so the frontend can display what's available.
#[command]
pub fn get_config() -> Result<ConfigResult, AppError> {
    let claude_installed = claude::is_installed();
    let hermes_installed = hermes::is_installed();

    // Always load config — ClaudeConfig::load() returns default when file/dir doesn't exist
    let claude = claude::ClaudeConfig::load()?;
    let hermes = hermes::HermesConfig::load()?;

    Ok(ConfigResult {
        claude: Some(claude.to_public()),
        hermes: Some(hermes.to_public()),
        claude_installed,
        hermes_installed,
    })
}

/// Save Claude Code configuration
#[command]
pub fn save_claude_config(update: ClaudeUpdatePayload) -> Result<claude::ClaudeConfigPublic, AppError> {
    let mut config = claude::ClaudeConfig::load()?;
    config.apply_update(claude::ClaudeConfigUpdate {
        model: update.model,
        api_endpoint: update.api_endpoint,
        api_key: update.api_key,
        mcp_servers: update.mcp_servers,
        custom_instructions: update.custom_instructions,
        theme: update.theme,
        max_tokens: update.max_tokens,
        extra: update.extra,
    });
    config.save()?;
    Ok(config.to_public())
}

/// Save Hermes Agent configuration
#[command]
pub fn save_hermes_config(update: HermesUpdatePayload) -> Result<hermes::HermesConfigPublic, AppError> {
    let mut config = hermes::HermesConfig::load()?;
    config.apply_update(hermes::HermesConfigUpdate {
        model: update.model,
        api_key: update.api_key,
        api_endpoint: update.api_endpoint,
        temperature: update.temperature,
        max_tokens: update.max_tokens,
        system_prompt: update.system_prompt,
        mcp_servers: update.mcp_servers,
        skills_dir: update.skills_dir,
        extra: update.extra,
    });
    config.save()?;
    Ok(config.to_public())
}

/// Get the raw API key for an agent (for frontend to use with sendApiRequest)
#[command]
pub fn get_agent_api_key(agent_type: String) -> Result<Option<String>, AppError> {
    match agent_type.as_str() {
        "claude" => {
            let config = claude::ClaudeConfig::load()?;
            Ok(config.env.as_ref()
                .and_then(|e| e.anthropic_api_key.clone())
                .filter(|k| !k.is_empty()))
        }
        "hermes" => {
            let config = hermes::HermesConfig::load()?;
            Ok(config.api_key.filter(|k| !k.is_empty()))
        }
        _ => Err(AppError {
            code: "ERR_INVALID_AGENT".to_string(),
            message: format!("不支持的 Agent 类型: {}", agent_type),
            details: None,
        }),
    }
}


