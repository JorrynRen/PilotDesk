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

/// Test API connection by actually sending a minimal request to the configured endpoint
#[command]
pub async fn test_api_connection(agent_type: String) -> Result<TestConnectionResult, AppError> {
    let start = std::time::Instant::now();

    match agent_type.as_str() {
        "claude" => {
            let config = claude::ClaudeConfig::load()?;
            let api_key = config.env.as_ref().and_then(|e| e.anthropic_api_key.as_ref())
                .filter(|k| !k.is_empty())
                .ok_or_else(|| AppError {
                    code: "ERR_NO_API_KEY".to_string(),
                    message: "未配置 API Key，请先在表单中填写 API Key 并保存".to_string(),
                    details: None,
                })?;
            let base_url = config.env.as_ref().and_then(|e| e.anthropic_base_url.as_ref())
                .filter(|u| !u.is_empty())
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://api.anthropic.com".to_string());
            let model = config.env.as_ref().and_then(|e| e.anthropic_model.as_ref())
                .filter(|m| !m.is_empty())
                .map(|m| m.clone())
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

            let endpoint = format!("{}/v1/messages", base_url);
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| AppError {
                    code: "ERR_HTTP_CLIENT".to_string(),
                    message: format!("HTTP 客户端创建失败: {}", e),
                    details: None,
                })?;

            let body = serde_json::json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}]
            });

            let resp = client.post(&endpoint)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| AppError {
                    code: "ERR_CONNECTION_FAILED".to_string(),
                    message: format!("连接失败: {}", e),
                    details: None,
                })?;

            let latency = start.elapsed().as_millis() as u64;
            let status = resp.status();

            if status.is_success() {
                Ok(TestConnectionResult {
                    agent_type: agent_type.clone(),
                    success: true,
                    message: format!("连接成功 ({}ms)", latency),
                    latency_ms: Some(latency),
                })
            } else {
                let body_text = resp.text().await.unwrap_or_default();
                let preview = body_text.chars().take(300).collect::<String>();
                Ok(TestConnectionResult {
                    agent_type: agent_type.clone(),
                    success: false,
                    message: format!("API 错误 (HTTP {}): {}", status.as_u16(), preview),
                    latency_ms: Some(latency),
                })
            }
        }
        "hermes" => {
            let config = hermes::HermesConfig::load()?;
            let api_key = config.api_key.as_ref()
                .filter(|k| !k.is_empty())
                .ok_or_else(|| AppError {
                    code: "ERR_NO_API_KEY".to_string(),
                    message: "未配置 API Key，请先在表单中填写 API Key 并保存".to_string(),
                    details: None,
                })?;
            let base_url = config.api_endpoint.as_ref()
                .filter(|u| !u.is_empty())
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://api.siliconflow.cn/v1".to_string());
            let model = config.model.as_ref()
                .filter(|m| !m.is_empty())
                .cloned()
                .unwrap_or_else(|| "deepseek-ai/DeepSeek-V3".to_string());

            // Determine if endpoint already includes /chat/completions
            let endpoint = if base_url.ends_with("chat/completions") {
                base_url
            } else {
                format!("{}/chat/completions", base_url.trim_end_matches('/'))
            };

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| AppError {
                    code: "ERR_HTTP_CLIENT".to_string(),
                    message: format!("HTTP 客户端创建失败: {}", e),
                    details: None,
                })?;

            let body = serde_json::json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}]
            });

            let resp = client.post(&endpoint)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| AppError {
                    code: "ERR_CONNECTION_FAILED".to_string(),
                    message: format!("连接失败: {}", e),
                    details: None,
                })?;

            let latency = start.elapsed().as_millis() as u64;
            let status = resp.status();

            if status.is_success() {
                Ok(TestConnectionResult {
                    agent_type: agent_type.clone(),
                    success: true,
                    message: format!("连接成功 ({}ms)", latency),
                    latency_ms: Some(latency),
                })
            } else {
                let body_text = resp.text().await.unwrap_or_default();
                let preview = body_text.chars().take(300).collect::<String>();
                Ok(TestConnectionResult {
                    agent_type: agent_type.clone(),
                    success: false,
                    message: format!("API 错误 (HTTP {}): {}", status.as_u16(), preview),
                    latency_ms: Some(latency),
                })
            }
        }
        _ => Err(AppError {
            code: "ERR_INVALID_AGENT".to_string(),
            message: format!("不支持的 Agent 类型: {}", agent_type),
            details: None,
        }),
    }
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TestConnectionResult {
    pub agent_type: String,
    pub success: bool,
    pub message: String,
    pub latency_ms: Option<u64>,
}
