use serde::{Deserialize, Serialize};
use std::fs;

use crate::utils::errors::AppError;
use crate::utils::paths::hermes_config_dir;

/// Hermes Agent YAML configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HermesConfig {
    /// Default model identifier
    pub model: Option<String>,
    /// API endpoint URL
    pub api_endpoint: Option<String>,
    /// API key (masked before sending to frontend)
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    /// Temperature setting (0.0 - 1.0)
    pub temperature: Option<f64>,
    /// Max tokens for responses
    pub max_tokens: Option<u32>,
    /// Custom system prompt
    pub system_prompt: Option<String>,
    /// MCP servers configuration
    pub mcp_servers: Option<serde_json::Value>,
    /// Skills directory path
    pub skills_dir: Option<String>,
    /// Additional settings
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Default for HermesConfig {
    fn default() -> Self {
        Self {
            model: None,
            api_endpoint: None,
            api_key: None,
            temperature: None,
            max_tokens: None,
            system_prompt: None,
            mcp_servers: None,
            skills_dir: None,
            extra: None,
        }
    }
}

/// Public representation (api_key masked)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HermesConfigPublic {
    pub model: Option<String>,
    pub api_endpoint: Option<String>,
    pub api_key_masked: Option<String>,
    pub api_key_set: bool,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub mcp_servers: Option<serde_json::Value>,
    pub skills_dir: Option<String>,
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

impl HermesConfig {
    /// Read Hermes config from ~/.hermes/config.json
    /// (We use JSON format internally for simplicity; YAML can be added later)
    pub fn load() -> Result<Self, AppError> {
        let dir = hermes_config_dir();
        if !dir.exists() {
            return Ok(Self::default());
        }
        
        // Try JSON config first, then YAML
        let json_path = dir.join("config.json");
        if json_path.exists() {
            let content = fs::read_to_string(&json_path)?;
            return serde_json::from_str(&content).map_err(|e| AppError {
                code: "ERR_PARSE_HERMES_CONFIG".to_string(),
                message: "解析 Hermes 配置文件失败".to_string(),
                details: Some(e.to_string()),
            });
        }

        // Try YAML config
        let yaml_path = dir.join("config.yaml");
        if yaml_path.exists() {
            let content = fs::read_to_string(&yaml_path)?;
            return Self::parse_yaml(&content);
        }

        // Try env file for API key at least
        let env_path = dir.join(".env");
        let mut config = Self::default();
        if env_path.exists() {
            let env_content = fs::read_to_string(&env_path)?;
            for line in env_content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();
                    match key {
                        "HERMES_API_KEY" | "API_KEY" => {
                            config.api_key = Some(value.to_string());
                        }
                        "HERMES_MODEL" | "MODEL" => {
                            config.model = Some(value.to_string());
                        }
                        "HERMES_API_ENDPOINT" | "API_ENDPOINT" => {
                            config.api_endpoint = Some(value.to_string());
                        }
                        "HERMES_TEMPERATURE" | "TEMPERATURE" => {
                            if let Ok(t) = value.parse::<f64>() {
                                config.temperature = Some(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(config)
    }

    /// Save Hermes config to ~/.hermes/config.json
    pub fn save(&self) -> Result<(), AppError> {
        let dir = hermes_config_dir();
        fs::create_dir_all(&dir)?;
        let config_path = dir.join("config.json");
        let content = serde_json::to_string_pretty(self).map_err(|e| AppError {
            code: "ERR_SERIALIZE_HERMES_CONFIG".to_string(),
            message: "序列化 Hermes 配置失败".to_string(),
            details: Some(e.to_string()),
        })?;
        fs::write(&config_path, content)?;
        Ok(())
    }

    /// Parse YAML content manually (simple key-value + nested)
    fn parse_yaml(content: &str) -> Result<Self, AppError> {
        let mut config = Self::default();
        // Simple YAML parser for flat and nested structures
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Handle simple key: value
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().trim_start_matches('"').trim_end_matches('"').to_string();
                match key.as_str() {
                    "model" => config.model = if value.is_empty() { None } else { Some(value) },
                    "api_endpoint" => config.api_endpoint = if value.is_empty() { None } else { Some(value) },
                    "api_key" => config.api_key = if value.is_empty() { None } else { Some(value) },
                    "temperature" => config.temperature = value.parse::<f64>().ok(),
                    "max_tokens" => config.max_tokens = value.parse::<u32>().ok(),
                    "system_prompt" => config.system_prompt = if value.is_empty() { None } else { Some(value) },
                    "skills_dir" => config.skills_dir = if value.is_empty() { None } else { Some(value) },
                    _ => {}
                }
            }
        }
        Ok(config)
    }

    /// Convert to public representation with masked API key
    pub fn to_public(&self) -> HermesConfigPublic {
        HermesConfigPublic {
            model: self.model.clone(),
            api_endpoint: self.api_endpoint.clone(),
            api_key_masked: self.api_key.as_ref().map(|k| {
                if k.is_empty() {
                    String::new()
                } else {
                    super::mask_api_key(k)
                }
            }),
            api_key_set: self.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            system_prompt: self.system_prompt.clone(),
            mcp_servers: self.mcp_servers.clone(),
            skills_dir: self.skills_dir.clone(),
            extra: self.extra.clone(),
        }
    }

    /// Apply changes from frontend
    pub fn apply_update(&mut self, update: HermesConfigUpdate) {
        if let Some(model) = update.model {
            self.model = Some(model);
        }
        if let Some(api_key) = update.api_key {
            if api_key != "UNCHANGED" {
                self.api_key = Some(api_key);
            }
        }
        if let Some(api_endpoint) = update.api_endpoint {
            self.api_endpoint = Some(api_endpoint);
        }
        if let Some(temperature) = update.temperature {
            self.temperature = Some(temperature);
        }
        if let Some(max_tokens) = update.max_tokens {
            self.max_tokens = Some(max_tokens);
        }
        if let Some(system_prompt) = update.system_prompt {
            self.system_prompt = Some(system_prompt);
        }
        if let Some(mcp_servers) = update.mcp_servers {
            self.mcp_servers = Some(mcp_servers);
        }
        if let Some(skills_dir) = update.skills_dir {
            self.skills_dir = Some(skills_dir);
        }
        if let Some(extra) = update.extra {
            self.extra = Some(extra);
        }
    }
}

/// Update payload from frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HermesConfigUpdate {
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

/// Check if Hermes Agent config directory exists
pub fn is_installed() -> bool {
    hermes_config_dir().exists()
}
