//! 无状态 Agent 单轮执行（AgentLoop 复用层）。
//!
//! 抽取自 `lib.rs::run_api_agent` 中「构造 AgentLoop 并执行一次」的通用逻辑，
//! 作为群聊 `PilotDeskLlmClient` 的底层，满足三点改造要求：
//! 1. **无状态**：入参 =（system_prompt + messages + tool_registry），不读写 session；
//! 2. **流式化**：`on_delta` 透出 LLM 增量；
//! 3. **审批化**：`ApprovalHandler` 参数（群聊场景指向 Director 裁决）。

use crate::api_agent::agent_loop::{
    AgentLoop, AgentLoopConfig, AgentLoopOutput, ApprovalHandler, AskUserBehavior, ContinueHandler,
    PermissionRules, SecurityMode, ToolRegistry,
};
use crate::api_agent::client::ApiClient;
use crate::api_agent::types::{ApiFormat, ChatMessage};
use std::sync::Arc;

/// 一次无状态 Agent 发言的输入。
pub struct AgentTurnInput {
    pub system_prompt: String,
    pub messages: Vec<ChatMessage>,
    pub max_iterations: usize,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

/// 一次无状态 Agent 发言的选项（流式 + 审批 + 停滞/上限续判 + 协作式取消）。
pub struct AgentTurnOptions {
    pub on_delta: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    /// "实质产出进展"心跳（每完成一轮有产出的迭代触发一次），供上层任务级停滞秒表重置；
    /// 群聊执行期接入，使长任务正常推进不会被固定任务级超时误杀。
    pub on_progress: Option<Arc<dyn Fn() + Send + Sync>>,
    pub approval_handler: Option<ApprovalHandler>,
    /// 停滞/迭代上限时是否继续的裁决回调（群聊场景指向 Director）。
    pub continue_handler: Option<ContinueHandler>,
    /// 审批方身份标签（拒绝文案用，如 "用户" / "主持人"；None 默认 "用户"）。
    pub approval_label: Option<String>,
    /// 是否把审批请求作为"用户亲自决策"弹窗发往前端（默认 false）。
    /// 仅用户亲自审批的场景置 true；群聊等由主持人程序化裁决的场景必须保持 false。
    pub notify_user_approval: bool,
    /// 协作式取消令牌（房间停止/暂停时在迭代边界提前结束；会话模式为 None，零影响）。
    pub cancel_token: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// 持久化权限规则（命令 deny/risky/allow + 模式策略；None 时用默认空规则 = 全部走兜底策略）。
    pub permission_rules: Option<PermissionRules>,
    /// 工作区目录（工作区内路径免审批；会话传 session.cwd，群聊传房间 cwd）。
    pub workspace: Option<String>,
    /// 会话安全模式（严格/标准/宽松/无限制；None 默认标准）。
    pub security_mode: Option<SecurityMode>,
    /// 用量归因的 provider id（群聊工具分支注入；None → AgentLoop 回退查 `sessions.api_provider`）。
    pub usage_provider: Option<String>,
    /// 用量落库的 scope_key（群聊=房间级 `groupchat:{roomId}`；None → AgentLoop 回退事件会话 id）。
    pub usage_scope: Option<String>,
    /// ask_user 工具行为：会话=工具内阻塞+60s 超时；群聊=拦截转轮末确认（无超时、落库恢复）。
    pub ask_user_behavior: AskUserBehavior,
}

impl Default for AgentTurnOptions {
    fn default() -> Self {
        Self {
            on_delta: None,
            on_progress: None,
            approval_handler: None,
            continue_handler: None,
            approval_label: None,
            notify_user_approval: false,
            cancel_token: None,
            permission_rules: None,
            workspace: None,
            security_mode: None,
            usage_provider: None,
            usage_scope: None,
            ask_user_behavior: AskUserBehavior::Session,
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
    .with_api_format(format);

    if let Some(rules) = options.permission_rules {
        agent_loop = agent_loop.with_permission_rules(rules);
    }
    if options.workspace.is_some() {
        agent_loop = agent_loop.with_workspace(options.workspace);
    }
    if let Some(mode) = options.security_mode {
        agent_loop = agent_loop.with_security_mode(mode);
    }

    if let Some(cb) = options.on_delta {
        agent_loop = agent_loop.with_on_delta(cb);
    }
    if let Some(cb) = options.on_progress {
        agent_loop = agent_loop.with_on_progress(cb);
    }
    if let Some(handler) = options.approval_handler {
        agent_loop = agent_loop.with_approval_handler(handler);
    }
    if let Some(label) = options.approval_label.as_deref() {
        agent_loop = agent_loop.with_approval_label(label);
    }
    if options.notify_user_approval {
        agent_loop = agent_loop.with_user_approval_notification();
    }
    if let Some(handler) = options.continue_handler {
        agent_loop = agent_loop.with_continue_handler(handler);
    }
    if let Some(token) = options.cancel_token {
        agent_loop = agent_loop.with_cancel_token(token);
    }
    if let Some(provider) = options.usage_provider {
        agent_loop = agent_loop.with_provider_label(&provider);
    }
    if let Some(scope) = options.usage_scope.as_deref() {
        agent_loop = agent_loop.with_usage_scope(scope);
    }
    agent_loop = agent_loop.with_ask_user_behavior(options.ask_user_behavior);

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
