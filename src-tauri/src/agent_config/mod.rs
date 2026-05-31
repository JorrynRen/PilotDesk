pub mod claude;
pub mod hermes;

use serde::{Deserialize, Serialize};

/// Unified agent config model for frontend display/editing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_type: String,
    pub config_dir: String,
    pub claude: Option<claude::ClaudeConfig>,
    pub hermes: Option<hermes::HermesConfig>,
}

/// Mask an API key for display: sk-abc...xyz
pub fn mask_api_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.len() <= 8 {
        return "*".repeat(trimmed.len());
    }
    let prefix = &trimmed[..6];
    let suffix = &trimmed[trimmed.len() - 4..];
    format!("{}...{}", prefix, suffix)
}
