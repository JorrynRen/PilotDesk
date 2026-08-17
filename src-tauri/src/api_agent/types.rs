//! API Agent 类型定义

use std::str::FromStr;
use serde::{Deserialize, Serialize};

/// API 格式（决定请求结构和解析方式）
#[derive(Debug, Clone, Default)]
pub enum ApiFormat {
    #[default]
    OpenAI,
    Anthropic,
}

impl FromStr for ApiFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(ApiFormat::Anthropic),
            _ => Ok(ApiFormat::OpenAI),
        }
    }
}

/// OpenAI 兼容的 Chat Completion 请求
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

/// Chat 消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,        // "system" | "user" | "assistant" | "tool"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 多模态输入：图片（base64 data URL 或 http(s) URL）。仅用于 user 消息，发送时转成 content parts。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".into(),
            content: Some(content.into()),
            images: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            images: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user_with_images(content: &str, images: Vec<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            images: if images.is_empty() { None } else { Some(images) },
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".into(),
            content: Some(content.into()),
            images: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content: None,
            images: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            images: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }
}

/// Function Calling 工具定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,  // "function"
    pub function: FunctionDef,
}

impl ToolDefinition {
    pub fn new(name: &str, description: &str, parameters: serde_json::Value) -> Self {
        Self {
            tool_type: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: Some(description.into()),
                parameters: Some(parameters),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// LLM 返回的工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,   // "function"
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,  // JSON 字符串
}

/// SSE 流式事件
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamChunk {
    pub id: Option<String>,
    pub object: Option<String>,
    pub created: Option<i64>,
    pub model: Option<String>,
    pub choices: Option<Vec<StreamChoice>>,
    /// Token 用量（最后一块返回）
    #[serde(default)]
    pub usage: Option<StreamUsage>,
}

/// 流式响应 token 用量
#[derive(Debug, Clone, Deserialize)]
pub struct StreamUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamChoice {
    pub index: Option<u32>,
    pub delta: Option<StreamDelta>,
    #[serde(rename = "finish_reason")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    /// DeepSeek 思维链内容（DeepSeek v3/R1 特有字段）
    #[serde(default)]
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<StreamToolCallDelta>>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamToolCallDelta {
    pub index: Option<u32>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: Option<StreamFunctionDelta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamFunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// Agent Loop 事件（发给前端）
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum AgentLoopEvent {
    #[serde(rename = "reasoning")]
    Reasoning { content: String },
    #[serde(rename = "chunk")]
    Chunk { content: String },
    #[serde(rename = "tool_start")]
    ToolStart { id: String, name: String, arguments: String },
    #[serde(rename = "tool_result")]
    ToolResult { id: String, name: String, result: String, success: bool },
    #[serde(rename = "approval_required")]
    ApprovalRequired {
        call_id: String,
        tool_name: String,
        arguments: String,
        risk_description: String,
    },
    #[serde(rename = "done")]
    Done { content: String },
    #[serde(rename = "error")]
    Error { message: String },
    /// 迭代达到上限，请求用户确认是否继续
    #[serde(rename = "iteration_limit")]
    IterationLimit { current: usize, max: usize },
    /// Token 用量统计
    #[serde(rename = "usage")]
    Usage { prompt_tokens: u32, completion_tokens: u32, total_tokens: u32 },
}

/// 思维链/工具调用步骤（与前端 `ThinkingChainStep` 及会话模式 `messages.tool_calls` 持久化格式对齐）。
///
/// 序列化键与前端 `MessageBubble` 解析的格式一致：`id/type/content/toolName/args/result/success/filePath/fileDiff/ts`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingChainStep {
    pub id: String,
    #[serde(rename = "type")]
    pub step_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(rename = "toolName", skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(rename = "args", skip_serializing_if = "Option::is_none")]
    pub tool_args: Option<String>,
    #[serde(rename = "result", skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<String>,
    #[serde(rename = "success", skip_serializing_if = "Option::is_none")]
    pub tool_success: Option<bool>,
    #[serde(rename = "filePath", skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(rename = "fileDiff", skip_serializing_if = "Option::is_none")]
    pub file_diff: Option<String>,
    #[serde(rename = "ts")]
    pub timestamp: i64,
}
