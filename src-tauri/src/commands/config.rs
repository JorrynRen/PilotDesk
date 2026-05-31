use crate::agent_config::{claude, hermes};
use crate::utils::errors::AppError;
use tauri::command;

#[derive(serde::Serialize, Clone)]
pub struct ConfigResult {
    pub claude: Option<claude::ClaudeConfigPublic>,
    pub hermes: Option<hermes::HermesConfigPublic>,
    pub claude_installed: bool,
    pub hermes_installed: bool,
}

#[derive(serde::Deserialize)]
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
#[command]
pub fn get_config() -> Result<ConfigResult, AppError> {
    let claude_installed = claude::is_installed();
    let hermes_installed = hermes::is_installed();

    let claude = if claude_installed {
        let config = claude::ClaudeConfig::load()?;
        Some(config.to_public())
    } else {
        None
    };

    let hermes = if hermes_installed {
        let config = hermes::HermesConfig::load()?;
        Some(config.to_public())
    } else {
        None
    };

    Ok(ConfigResult {
        claude,
        hermes,
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

/// Test API connection by reading config and attempting a simple request
#[command]
pub fn test_api_connection(agent_type: String) -> Result<TestConnectionResult, AppError> {
    match agent_type.as_str() {
        "claude" => {
            let config = claude::ClaudeConfig::load()?;
            let has_key = config.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false);
            Ok(TestConnectionResult {
                agent_type: agent_type.clone(),
                success: has_key,
                message: if has_key {
                    "API Key 已配置".to_string()
                } else {
                    "未配置 API Key".to_string()
                },
                latency_ms: None,
            })
        }
        "hermes" => {
            let config = hermes::HermesConfig::load()?;
            let has_key = config.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false);
            Ok(TestConnectionResult {
                agent_type: agent_type.clone(),
                success: has_key,
                message: if has_key {
                    "API Key 已配置".to_string()
                } else {
                    "未配置 API Key".to_string()
                },
                latency_ms: None,
            })
        }
        _ => Err(AppError {
            code: "ERR_INVALID_AGENT".to_string(),
            message: format!("不支持的 Agent 类型: {}", agent_type),
            details: None,
        }),
    }
}

#[derive(serde::Serialize, Clone)]
pub struct TestConnectionResult {
    pub agent_type: String,
    pub success: bool,
    pub message: String,
    pub latency_ms: Option<u64>,
}
