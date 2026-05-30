use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Session {
    pub id: String,
    pub agent_type: String,
    pub title: String,
    pub cwd: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_message_preview: String,
    pub message_count: i64,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub mode: String,
    pub timestamp: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Inspiration {
    pub id: String,
    pub icon: String,
    pub title: String,
    pub content: String,
    pub source_agent: String,
    pub is_favorite: bool,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BotChannel {
    pub id: String,
    pub agent_type: String,
    pub platform: String,
    pub method: String,
    pub status: String,
    pub trigger_prefix: String,
    pub response_format: String,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EnvInfo {
    pub node_version: Option<String>,
    pub git_version: Option<String>,
    pub python_version: Option<String>,
    pub claude_code_version: Option<String>,
    pub hermes_version: Option<String>,
}
