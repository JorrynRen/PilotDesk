use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub claude_code_path: Option<String>,
    pub hermes_agent_path: Option<String>,
    pub theme: String,
    pub language: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            claude_code_path: None,
            hermes_agent_path: None,
            theme: "dark".to_string(),
            language: "zh-CN".to_string(),
        }
    }
}
