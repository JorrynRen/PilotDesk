use serde::{Deserialize, Serialize};
use std::fs;

use crate::utils::errors::AppError;
use crate::utils::paths::claude_config_dir;

/// Claude Code configuration file structure
/// Reflects ~/.claude/settings.json (user-level)
///
/// Actual file format:
/// ```json
/// {
///   "env": {
///     "ANTHROPIC_API_KEY": "sk-...",
///     "ANTHROPIC_BASE_URL": "https://api.siliconflow.cn",
///     "ANTHROPIC_MODEL": "deepseek-ai/DeepSeek-V3"
///   },
///   "availableModels": ["deepseek-ai/DeepSeek-V3"],
///   "enabledPlugins": { ... }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeConfig {
    /// Environment variables (ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL, ANTHROPIC_MODEL)
    pub env: Option<ClaudeEnv>,
    /// Available models list
    #[serde(default)]
    pub available_models: Option<Vec<String>>,
    /// Enabled plugins
    #[serde(default)]
    pub enabled_plugins: Option<serde_json::Value>,
    /// MCP servers configuration
    #[serde(default)]
    pub mcp_servers: Option<serde_json::Value>,
    /// Custom instructions / system prompt
    #[serde(default)]
    pub custom_instructions: Option<String>,
    /// Theme preference
    #[serde(default)]
    pub theme: Option<String>,
    /// Max tokens for responses
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Additional settings stored as key-value
    #[serde(default)]
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Environment variables section in Claude config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeEnv {
    #[serde(rename = "ANTHROPIC_API_KEY")]
    pub anthropic_api_key: Option<String>,
    #[serde(rename = "ANTHROPIC_BASE_URL")]
    pub anthropic_base_url: Option<String>,
    #[serde(rename = "ANTHROPIC_MODEL")]
    pub anthropic_model: Option<String>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            env: None,
            available_models: None,
            enabled_plugins: None,
            mcp_servers: None,
            custom_instructions: None,
            theme: None,
            max_tokens: None,
            extra: None,
        }
    }
}

/// Public representation (api_key masked)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeConfigPublic {
    pub model: Option<String>,
    pub api_endpoint: Option<String>,
    pub api_key_masked: Option<String>,
    pub api_key_set: bool,
    pub available_models: Option<Vec<String>>,
    pub mcp_servers: Option<serde_json::Value>,
    pub custom_instructions: Option<String>,
    pub theme: Option<String>,
    pub max_tokens: Option<u32>,
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ClaudeConfig {
    /// Read Claude config from ~/.claude/settings.json
    pub fn load() -> Result<Self, AppError> {
        let settings_path = claude_config_dir().join("settings.json");
        if !settings_path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).map_err(|e| AppError {
            code: "ERR_PARSE_CLAUDE_CONFIG".to_string(),
            message: "解析 Claude 配置文件失败".to_string(),
            details: Some(e.to_string()),
        })
    }

    /// Save Claude config to ~/.claude/settings.json
    pub fn save(&self) -> Result<(), AppError> {
        let dir = claude_config_dir();
        fs::create_dir_all(&dir)?;
        let settings_path = dir.join("settings.json");
        let content = serde_json::to_string_pretty(self).map_err(|e| AppError {
            code: "ERR_SERIALIZE_CLAUDE_CONFIG".to_string(),
            message: "序列化 Claude 配置失败".to_string(),
            details: Some(e.to_string()),
        })?;
        fs::write(&settings_path, content)?;
        Ok(())
    }

    /// Convert to public representation with masked API key
    pub fn to_public(&self) -> ClaudeConfigPublic {
        let model = self.env.as_ref().and_then(|e| e.anthropic_model.clone());
        let api_endpoint = self.env.as_ref().and_then(|e| e.anthropic_base_url.clone());
        let api_key = self.env.as_ref().and_then(|e| e.anthropic_api_key.clone());

        ClaudeConfigPublic {
            model,
            api_endpoint,
            api_key_masked: api_key.as_ref().map(|k| {
                if k.is_empty() {
                    String::new()
                } else {
                    super::mask_api_key(k)
                }
            }),
            api_key_set: api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
            available_models: self.available_models.clone(),
            mcp_servers: self.mcp_servers.clone(),
            custom_instructions: self.custom_instructions.clone(),
            theme: self.theme.clone(),
            max_tokens: self.max_tokens,
            extra: self.extra.clone(),
        }
    }

    /// Apply changes from frontend (only update provided fields)
    /// `api_key` field: if "UNCHANGED" or empty, keep existing; otherwise update
    pub fn apply_update(&mut self, update: ClaudeConfigUpdate) {
        let mut env = self.env.clone().unwrap_or(ClaudeEnv {
            anthropic_api_key: None,
            anthropic_base_url: None,
            anthropic_model: None,
        });

        if let Some(model) = update.model {
            env.anthropic_model = Some(model);
        }
        if let Some(api_endpoint) = update.api_endpoint {
            env.anthropic_base_url = Some(api_endpoint);
        }
        if let Some(api_key) = update.api_key {
            if api_key != "UNCHANGED" {
                env.anthropic_api_key = Some(api_key);
            }
        }
        self.env = Some(env);

        if let Some(mcp_servers) = update.mcp_servers {
            self.mcp_servers = Some(mcp_servers);
        }
        if let Some(custom_instructions) = update.custom_instructions {
            self.custom_instructions = Some(custom_instructions);
        }
        if let Some(theme) = update.theme {
            self.theme = Some(theme);
        }
        if let Some(max_tokens) = update.max_tokens {
            self.max_tokens = Some(max_tokens);
        }
        if let Some(extra) = update.extra {
            self.extra = Some(extra);
        }
    }
}

/// Update payload from frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfigUpdate {
    pub model: Option<String>,
    pub api_endpoint: Option<String>,
    pub api_key: Option<String>,
    pub mcp_servers: Option<serde_json::Value>,
    pub custom_instructions: Option<String>,
    pub theme: Option<String>,
    pub max_tokens: Option<u32>,
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Check if Claude Code config directory exists
pub fn is_installed() -> bool {
    claude_config_dir().exists()
}
