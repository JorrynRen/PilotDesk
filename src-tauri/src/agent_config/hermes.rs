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
#[serde(rename_all = "camelCase")]
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
    /// Read Hermes config from ~/.hermes/
    /// Priority: config.yaml (new version) > config.json (old version) > .env
    pub fn load() -> Result<Self, AppError> {
        let dir = hermes_config_dir();
        if !dir.exists() {
            return Ok(Self::default());
        }

        // Try YAML config first (new Hermes version)
        let yaml_path = dir.join("config.yaml");
        if yaml_path.exists() {
            let content = fs::read_to_string(&yaml_path)?;
            return Self::parse_yaml(&content);
        }

        // Try JSON config (old Hermes version)
        let json_path = dir.join("config.json");
        if json_path.exists() {
            let content = fs::read_to_string(&json_path)?;
            return serde_json::from_str(&content).map_err(|e| AppError {
                code: "ERR_PARSE_HERMES_CONFIG".to_string(),
                message: "解析 Hermes 配置文件失败".to_string(),
                details: Some(e.to_string()),
            });
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

    /// Save Hermes config — writes to config.yaml (new version) or config.json (old version)
    /// Priority: if config.yaml exists → update YAML; otherwise → write JSON
    pub fn save(&self) -> Result<(), AppError> {
        let dir = hermes_config_dir();
        fs::create_dir_all(&dir)?;

        let yaml_path = dir.join("config.yaml");
        let json_path = dir.join("config.json");

        if yaml_path.exists() {
            // New Hermes version: update config.yaml in-place
            let yaml_content = fs::read_to_string(&yaml_path)?;
            let mut yaml_value: serde_json::Value = serde_yaml::from_str(&yaml_content).map_err(|e| AppError {
                code: "ERR_PARSE_HERMES_YAML".to_string(),
                message: "解析 Hermes YAML 配置失败".to_string(),
                details: Some(e.to_string()),
            })?;

            // Update model section
            if let Some(model_obj) = yaml_value.get_mut("model") {
                if let Some(model) = &self.model {
                    model_obj["default"] = serde_json::Value::String(model.clone());
                }
                if let Some(api_endpoint) = &self.api_endpoint {
                    model_obj["base_url"] = serde_json::Value::String(api_endpoint.clone());
                }
                if let Some(api_key) = &self.api_key {
                    model_obj["api_key"] = serde_json::Value::String(api_key.clone());
                }
            }

            // Update agent section
            if let Some(agent_obj) = yaml_value.get_mut("agent") {
                if let Some(temperature) = self.temperature {
                    agent_obj["temperature"] = serde_json::Value::Number(
                        serde_json::Number::from_f64(temperature).unwrap_or(serde_json::Number::from(0))
                    );
                }
                if let Some(max_tokens) = self.max_tokens {
                    agent_obj["max_turns"] = serde_json::Value::Number(
                        serde_json::Number::from(max_tokens)
                    );
                }
            }

            // Update display section
            if let Some(system_prompt) = &self.system_prompt {
                if let Some(display_obj) = yaml_value.get_mut("display") {
                    display_obj["personality"] = serde_json::Value::String(system_prompt.clone());
                } else {
                    let mut display = serde_json::Map::new();
                    display.insert("personality".to_string(), serde_json::Value::String(system_prompt.clone()));
                    yaml_value["display"] = serde_json::Value::Object(display);
                }
            }

            // Write back YAML
            let updated_yaml = serde_yaml::to_string(&yaml_value).map_err(|e| AppError {
                code: "ERR_SERIALIZE_HERMES_YAML".to_string(),
                message: "序列化 Hermes YAML 配置失败".to_string(),
                details: Some(e.to_string()),
            })?;
            fs::write(&yaml_path, updated_yaml)?;
        } else {
            // Old Hermes version or no YAML yet: write JSON
            let json_content = serde_json::to_string_pretty(self).map_err(|e| AppError {
                code: "ERR_SERIALIZE_HERMES_CONFIG".to_string(),
                message: "序列化 Hermes 配置失败".to_string(),
                details: Some(e.to_string()),
            })?;
            fs::write(&json_path, json_content)?;
        }

        Ok(())
    }

    /// Parse YAML content using serde_yaml to handle nested structures
    fn parse_yaml(content: &str) -> Result<Self, AppError> {
        // First try to parse the full YAML structure
        let yaml_value: serde_json::Value = serde_yaml::from_str(content).map_err(|e| AppError {
            code: "ERR_PARSE_HERMES_YAML".to_string(),
            message: "解析 Hermes YAML 配置失败".to_string(),
            details: Some(e.to_string()),
        })?;

        let mut config = Self::default();

        // Extract model info from nested model.default
        if let Some(model_obj) = yaml_value.get("model") {
            config.model = model_obj.get("default")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Use model.base_url as api_endpoint
            config.api_endpoint = model_obj.get("base_url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Use model.api_key
            config.api_key = model_obj.get("api_key")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }

        // Extract temperature from agent config
        if let Some(agent_obj) = yaml_value.get("agent") {
            config.max_tokens = agent_obj.get("max_turns")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            config.temperature = agent_obj.get("temperature")
                .and_then(|v| v.as_f64());
        }

        // Extract display settings
        if let Some(display_obj) = yaml_value.get("display") {
            config.system_prompt = display_obj.get("personality")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }

        // Extract custom_providers for additional endpoint info
        if let Some(providers) = yaml_value.get("custom_providers").and_then(|v| v.as_array()) {
            if let Some(first) = providers.first() {
                // If no api_endpoint from model, try custom_providers
                if config.api_endpoint.is_none() {
                    config.api_endpoint = first.get("base_url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                if config.api_key.is_none() {
                    config.api_key = first.get("api_key")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
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
