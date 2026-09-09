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

/// LLM 调用用量明细（四桶互斥口径）：`prompt`=未命中输入、`cache_read`=缓存命中读取、
/// `cache_write`=缓存写入（如 Anthropic cache_creation）；`cached_tokens` 落库时取其两者之和。
/// 供主请求流式/非流式、滚动摘要、意图路由等所有 LLM 调用统一解析与落库。
#[derive(Debug, Clone, Copy, Default)]
pub struct UsageRecord {
    pub prompt: u32,
    pub completion: u32,
    pub total: u32,
    pub cache_read: u32,
    pub cache_write: u32,
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
    pub temperature: Option<f64>,
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
    /// 思考模式（如 DeepSeek）要求：assistant 消息必须原样回传上轮的 reasoning_content，
    /// 否则服务端校验上下文不完整返回 HTTP 400。仅 assistant 消息携带。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
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
            reasoning_content: None,
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
            reasoning_content: None,
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
            reasoning_content: None,
        }
    }

    #[allow(dead_code)]
    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".into(),
            content: Some(content.into()),
            images: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_reasoning(content: &str, reasoning: &str) -> Self {
        Self {
            role: "assistant".into(),
            content: Some(content.into()),
            images: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: if reasoning.is_empty() { None } else { Some(reasoning.to_string()) },
        }
    }

    #[allow(dead_code)]
    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content: None,
            images: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_tools_and_reasoning(tool_calls: Vec<ToolCall>, reasoning: &str) -> Self {
        Self {
            role: "assistant".into(),
            content: None,
            images: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            reasoning_content: if reasoning.is_empty() { None } else { Some(reasoning.to_string()) },
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
            reasoning_content: None,
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
    /// OpenAI 兼容：`prompt_tokens_details.cached_tokens`
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    /// DeepSeek 等兼容实现：`prompt_cache_hit_tokens`（顶层）
    #[serde(default)]
    pub prompt_cache_hit_tokens: Option<u32>,
}

/// OpenAI 兼容的 `prompt_tokens_details`（缓存命中 token 数，前缀缓存命中率观测用）。
#[derive(Debug, Clone, Deserialize)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
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
    /// Token 用量统计（`cached_tokens`：总缓存 = 缓存读取 + 缓存写入，缺失为 0）
    #[serde(rename = "usage")]
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
        #[serde(default)]
        cached_tokens: u32,
    },
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
