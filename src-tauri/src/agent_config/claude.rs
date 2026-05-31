use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::utils::errors::AppError;
use crate::utils::paths::claude_config_dir;

/// Claude Code configuration file structure
/// Reflects ~/.claude/settings.json (user-level)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    /// Selected model (e.g., "claude-sonnet-4-20250514")
    pub model: Option<String>,
    /// API key (masked before sending to frontend)
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    /// MCP servers configuration
    pub mcp_servers: Option<serde_json::Value>,
    /// Custom instructions / system prompt
    pub custom_instructions: Option<String>,
    /// Theme preference
    pub theme: Option<String>,
    /// Max tokens for responses
    pub max_tokens: Option<u32>,
    /// Additional settings stored as key-value
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            model: None,
            api_key: None,
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
pub struct ClaudeConfigPublic {
    pub model: Option<String>,
    pub api_key_masked: Option<String>,
    pub api_key_set: bool,
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
        ClaudeConfigPublic {
            model: self.model.clone(),
            api_key_masked: self.api_key.as_ref().map(|k| {
                if k.is_empty() {
                    String::new()
                } else {
                    super::mask_api_key(k)
                }
            }),
            api_key_set: self.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
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
        if let Some(model) = update.model {
            self.model = Some(model);
        }
        if let Some(api_key) = update.api_key {
            if api_key != "UNCHANGED" {
                self.api_key = Some(api_key);
            }
        }
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
