//! 无状态 Agent 单轮执行（AgentLoop 复用层）。
//!
//! 抽取自 `lib.rs::run_api_agent` 中「构造 AgentLoop 并执行一次」的通用逻辑，
//! 作为群聊 `PilotDeskLlmClient` 的底层，满足三点改造要求：
//! 1. **无状态**：入参 =（system_prompt + messages + tool_registry），不读写 session；
//! 2. **流式化**：`on_delta` 透出 LLM 增量；
//! 3. **审批化**：`ApprovalHandler` 参数（群聊场景指向 Director 裁决）。

use crate::api_agent::agent_loop::{
    AgentLoop, AgentLoopConfig, AgentLoopOutput, ApprovalHandler, AuthLevel, ContinueHandler,
    ToolRegistry,
};
use crate::api_agent::client::ApiClient;
use crate::api_agent::types::{ApiFormat, ChatMessage};
use std::sync::Arc;

/// 一次无状态 Agent 发言的输入。
pub struct AgentTurnInput {
    pub system_prompt: String,
    pub messages: Vec<ChatMessage>,
    pub max_iterations: usize,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// 一次无状态 Agent 发言的选项（流式 + 审批 + 停滞/上限续判）。
pub struct AgentTurnOptions {
    pub on_delta: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    pub approval_handler: Option<ApprovalHandler>,
    pub auth_level: AuthLevel,
    /// 停滞/迭代上限时是否继续的裁决回调（群聊场景指向 Director）。
    pub continue_handler: Option<ContinueHandler>,
    /// 审批方身份标签（拒绝文案用，如 "用户" / "主持人"；None 默认 "用户"）。
    pub approval_label: Option<String>,
}

impl Default for AgentTurnOptions {
    fn default() -> Self {
        Self {
            on_delta: None,
            approval_handler: None,
            auth_level: AuthLevel::Confirm,
            continue_handler: None,
            approval_label: None,
        }
    }
}

/// 执行一次无状态 Agent 发言（AgentLoop 工具循环，单轮 + 工具次数上限）。
///
/// `event_session_id` 仅用于 `AgentLoop` 内部向前端发射 `agent-*` 事件；
/// 群聊场景传入房间内唯一的占位 id 即可（前端群聊仅监听 `groupchat-event`）。
pub async fn run_agent_turn(
    client: ApiClient,
    tool_registry: Arc<ToolRegistry>,
    model: &str,
    format: ApiFormat,
    app_handle: tauri::AppHandle,
    event_session_id: &str,
    input: AgentTurnInput,
    options: AgentTurnOptions,
) -> Result<AgentLoopOutput, String> {
    let tools = tool_registry.get_definitions();

    let mut agent_loop = AgentLoop::new(
        client,
        tool_registry,
        model.to_string(),
        app_handle,
        event_session_id.to_string(),
    )
    .with_api_format(format)
    .with_auth_level(options.auth_level);

    if let Some(cb) = options.on_delta {
        agent_loop = agent_loop.with_on_delta(cb);
    }
    if let Some(handler) = options.approval_handler {
        agent_loop = agent_loop.with_approval_handler(handler);
    }
    if let Some(label) = options.approval_label.as_deref() {
        agent_loop = agent_loop.with_approval_label(label);
    }
    if let Some(handler) = options.continue_handler {
        agent_loop = agent_loop.with_continue_handler(handler);
    }

    agent_loop
        .run(AgentLoopConfig {
            max_iterations: input.max_iterations,
            system_prompt: input.system_prompt,
            tools,
            messages: input.messages,
            temperature: input.temperature,
            max_tokens: input.max_tokens,
            context_tokens: None,
        })
        .await
}
