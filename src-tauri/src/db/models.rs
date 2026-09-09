use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_model: Option<String>,
    /// Agent-side session ID (e.g. Claude Code session UUID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    /// 模型温度 (0.0-2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// 最大生成 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// "image" 或 "file"
    pub kind: String,
    /// 原始文件名
    pub name: String,
    /// 落盘后的绝对路径
    pub path: String,
    /// MIME 类型
    pub mime: String,
    /// 文件大小（字节）
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub mode: String,
    pub timestamp: i64,
    /// 完整思维链（reasoning + tool 调用 + file_diff 步骤，JSON 数组字符串）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<String>,
    /// Tool call ID for role='tool' messages
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool name for role='tool' messages
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// 附件（图片/文件，落盘 + 路径引用），仅用于 user 消息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct EnvInfo {
    pub node_version: Option<String>,
    pub git_version: Option<String>,
    pub python_version: Option<String>,
    pub agent_versions: std::collections::HashMap<String, Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_latest_versions: Option<std::collections::HashMap<String, Option<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: i64,
    pub timestamp: i64,
    pub message: String,
    pub level: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub category: String,
}

impl SkillInfo {
    pub fn new(name: &str, description: &str, category: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            category: category.to_string(),
        }
    }
}
