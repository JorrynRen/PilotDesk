//! PilotDesk 适配层：`PilotDeskLlmClient`（实现框架层 `LlmClient`）。
//!
//! - Director：无工具模式（单次流式补全，输出结构化决策）。
//! - API 参与者：复用抽取后的 `run_agent_turn`（`AgentLoop` 工具循环 + 流式 + 审批）。

use std::sync::Arc;

use crate::api_agent::agent_loop::{
    record_usage_row, ApprovalHandler, AskUserBehavior, AuthLevel, ContinueHandler, PermissionRules,
    RiskLevel, SecurityMode, ToolRegistry,
};
use crate::api_agent::agent_turn::{run_agent_turn, AgentTurnInput, AgentTurnOptions};
use crate::api_agent::client::ApiClient;
use crate::api_agent::types::{ApiFormat, ChatMessage as ApiChatMessage, ChatRequest};

use super::super::participant::{ChatMessage, DeltaFn, LlmClient};

/// 流式 chunk 空闲超时默认值（秒），与 app_settings 默认一致；构造时由 build_llm_client 注入。
const DEFAULT_STREAM_IDLE_SECS: u64 = 90;

pub struct PilotDeskLlmClient {
    endpoint: String,
    api_key: String,
    model: String,
    format: ApiFormat,
    /// 用量归因（无工具补全分支）：写 `api_usage_log` 的 scope_key（房间级占位；None 不记录）。
    usage_key: Option<String>,
    /// 用量归因：provider 名（写 `api_usage_log.provider`；默认空串）。
    provider: String,
    /// 工具集（Some 时走 AgentLoop 工具循环，None 时走单次补全）。
    tool_registry: Option<Arc<ToolRegistry>>,
    /// 工具模式所需的 AppHandle（AgentLoop 内部用于发射 `agent-*` 事件）；
    /// 无工具补全分支也用它拿 DbState 写用量并发射 `usage-recorded`。
    app_handle: Option<tauri::AppHandle>,
    /// 工具审批回调（群聊场景指向 Director 裁决）。
    approval_handler: Option<Arc<dyn Fn(&str, &str, &str, RiskLevel) -> bool + Send + Sync>>,
    /// 停滞/迭代上限继续裁决回调（群聊场景指向 Director）。
    continue_handler: Option<Arc<dyn Fn(usize, usize) -> bool + Send + Sync>>,
    /// 工具模式下向前端发射 `agent-*` 事件时使用的会话标识（群聊用 `groupchat:{room_id}:{participant_id}`）。
    event_session_id: Option<String>,
    /// 协作式取消令牌（指向房间 control.abort；停止/暂停时在迭代边界提前结束工具循环）。
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// 持久化权限规则（命令 deny/risky/allow + 模式策略；AgentLoop 工具循环注入）。
    permission_rules: PermissionRules,
    /// 工作区目录（工作区内路径免审批；房间 cwd）。
    workspace: Option<String>,
    /// 会话安全模式（本次/本波次有效，不持久化；None 默认标准）。
    security_mode: Option<SecurityMode>,
    /// 流式 chunk 空闲超时（秒；0 = 禁用空闲检测）。与 app_settings `api_stream_idle_secs` 同源。
    stream_idle_secs: u64,
}

impl PilotDeskLlmClient {
    /// 无工具模式（Director）。
    pub fn new(endpoint: String, api_key: String, model: String, format: ApiFormat) -> Self {
        Self {
            endpoint,
            api_key,
            model,
            format,
            usage_key: None,
            provider: String::new(),
            tool_registry: None,
            app_handle: None,
            approval_handler: None,
            continue_handler: None,
            event_session_id: None,
            cancel: None,
            permission_rules: PermissionRules::default(),
            workspace: None,
            security_mode: None,
            stream_idle_secs: DEFAULT_STREAM_IDLE_SECS,
        }
    }

    /// 设置用量归因元数据 + AppHandle（构造时注入）：provider 名写 `api_usage_log.provider`，
    /// usage_key 为 scope_key（群聊房间级占位），AppHandle 用于拿 DbState 并发射 `usage-recorded`。
    pub fn with_usage_meta(mut self, provider: String, usage_key: Option<String>, app: tauri::AppHandle) -> Self {
        self.provider = provider;
        self.usage_key = usage_key;
        self.app_handle = Some(app);
        self
    }

    /// 设置持久化权限规则（命令 deny/risky/allow 清单分类）。
    pub fn with_permission_rules(mut self, rules: PermissionRules) -> Self {
        self.permission_rules = rules;
        self
    }

    /// 设置工作区目录（工作区内路径免审批）。
    pub fn with_workspace(mut self, workspace: Option<String>) -> Self {
        self.workspace = workspace;
        self
    }

    /// 设置会话安全模式（本次/本波次有效，不持久化）。
    pub fn with_security_mode(mut self, mode: Option<SecurityMode>) -> Self {
        self.security_mode = mode;
        self
    }

    /// 覆盖流式 chunk 空闲超时（秒；0 = 禁用空闲检测）。会话全局可配（app_settings
    /// `api_stream_idle_secs`），群聊在 `build_llm_client` 构造时读取注入。
    pub fn with_stream_idle_secs(mut self, secs: u64) -> Self {
        self.stream_idle_secs = secs;
        self
    }

    /// 工具模式（API 参与者）：复用 `run_agent_turn`。
    pub fn with_tools(mut self, tool_registry: Arc<ToolRegistry>, app_handle: tauri::AppHandle) -> Self {
        self.tool_registry = Some(tool_registry);
        self.app_handle = Some(app_handle);
        self
    }

    /// 设置工具审批回调（Director 裁决）。
    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(Arc::from(handler));
        self
    }

    /// 设置停滞/迭代上限继续裁决回调（Director 裁决）。
    pub fn with_continue_handler(mut self, handler: ContinueHandler) -> Self {
        self.continue_handler = Some(Arc::from(handler));
        self
    }

    /// 设置 `agent-*` 事件的会话标识（用于前端区分参与者）。
    pub fn with_event_session_id(mut self, id: String) -> Self {
        self.event_session_id = Some(id);
        self
    }

    /// 设置协作式取消令牌（指向房间 control.abort，停止/暂停时迭代边界提前结束）。
    pub fn with_cancel(mut self, token: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cancel = Some(token);
        self
    }

    fn build_client(&self) -> ApiClient {
        let client = if matches!(self.format, ApiFormat::Anthropic) {
            ApiClient::new(self.endpoint.clone(), self.api_key.clone(), self.format.clone())
        } else {
            ApiClient::new_openai(self.endpoint.clone(), self.api_key.clone())
        };
        client.with_stream_idle(self.stream_idle_secs)
    }

    async fn complete_raw(
        &self,
        system: &str,
        api_messages: Vec<ApiChatMessage>,
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<(String, String), String> {
        let client = self.build_client();

        let mut api_messages = api_messages;
        api_messages.insert(0, ApiChatMessage::system(system));

        let request = ChatRequest {
            model: self.model.clone(),
            messages: api_messages,
            tools: None,
            tool_choice: None,
            stream: true,
            temperature: Some(0.7),
            max_tokens: None,
        };

        // 思考模式（DeepSeek 等）：reasoning_content 由 chat_stream 累积进 ChatResponse，随下一轮原样回传。
        // 用量（prompt/completion/total/cache_read/cache_write）可能随末尾 chunk 回调一次；多次回调以最后一次为准。
        let mut last_usage: Option<(u32, u32, u32, u32, u32)> = None;

        let response = match self.format {
            ApiFormat::Anthropic => {
                client
                    .chat_stream_anthropic(
                        &request,
                        |chunk| {
                            if let Some(cb) = &on_delta {
                                cb(chunk);
                            }
                        },
                        |_| {},
                        |_, _, _| {},
                        |prompt, completion, total, cache_read, cache_write| {
                            last_usage = Some((prompt, completion, total, cache_read, cache_write));
                        },
                    )
                    .await?
            }
            _ => {
                client
                    .chat_stream(
                        &request,
                        |chunk| {
                            if let Some(cb) = &on_delta {
                                cb(chunk);
                            }
                        },
                        |_| {},
                        |_, _, _| {},
                        |prompt, completion, total, cache_read, cache_write| {
                            last_usage = Some((prompt, completion, total, cache_read, cache_write));
                        },
                    )
                    .await?
            }
        };

        // 无工具补全分支（Director 等）用量落库 + `usage-recorded` 脏标记。
        if let Some((prompt, completion, total, cache_read, cache_write)) = last_usage {
            self.record_raw_usage(prompt, completion, total, cache_read, cache_write);
        }

        Ok((response.content, response.reasoning_content))
    }

    /// 无工具补全分支（Director 等）的用量落库 + `usage-recorded` 脏标记。
    /// 写 `api_usage_log`（缓存读/写拆分，`cached_tokens` 列=两者之和；含 provider 归因，
    /// scope_key = usage_key），成功后才发射脏标记。
    /// 未注入 usage_key/provider 或拿不到 DbState 时静默跳过，不影响主流程。
    fn record_raw_usage(&self, prompt: u32, completion: u32, total: u32, cache_read: u32, cache_write: u32) {
        use tauri::Manager;
        let (Some(scope_key), Some(app)) = (&self.usage_key, &self.app_handle) else { return };
        let Some(state) = app.try_state::<crate::DbState>() else { return };
        let Ok(conn) = state.pool.get() else { return };
        if record_usage_row(
            &conn,
            scope_key,
            &self.provider,
            &self.model,
            &self.format,
            prompt,
            completion,
            total,
            cache_read,
            cache_write,
        )
        .is_ok()
        {
            use tauri::Emitter;
            let _ = app.emit("usage-recorded", serde_json::json!({
                "provider": self.provider,
                "model": self.model,
            }));
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for PilotDeskLlmClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<String, String> {
        let (content, _, _) = self.complete_with_tool_calls(system, messages, on_delta, None).await?;
        Ok(content)
    }

    async fn complete_with_tool_calls(
        &self,
        system: &str,
        messages: &[ChatMessage],
        on_delta: Option<Arc<DeltaFn>>,
        on_progress: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(String, String, String), String> {
        let mut api_messages: Vec<ApiChatMessage> = Vec::with_capacity(messages.len());
        for m in messages {
            api_messages.push(to_api_message(m));
        }
        // 统一兜底（v3.5c）：部分上游要求请求体以 user query 结尾，否则 400
        //（"No user query found in messages"）。群聊发言上下文按参与者阅读进度/窗口过滤，
        // 可能为空或以 assistant/director（scheduling/statement）结尾——例如发言者紧邻被
        // 再次点名、期间无新可分派给该参与者的消息。此处保证工具/纯补全两分支的请求
        // 恒包含 user 且以 user 结尾，从根上消除该 400。
        let ends_with_user = api_messages.last().map(|m| m.role == "user").unwrap_or(false);
        if !ends_with_user {
            api_messages.push(ApiChatMessage::user(
                "请基于以上上下文，继续完成你的本轮发言。",
            ));
        }

        if let (Some(registry), Some(app)) = (&self.tool_registry, &self.app_handle) {
            let approval_handler = self.approval_handler.clone().map(|h| {
                Box::new(move |call_id: &str, tool_name: &str, args: &str, risk: RiskLevel| {
                    h(call_id, tool_name, args, risk)
                }) as ApprovalHandler
            });
            let continue_handler = self.continue_handler.clone().map(|h| {
                Box::new(move |current: usize, max: usize| h(current, max)) as ContinueHandler
            });

            let output = run_agent_turn(
                self.build_client(),
                registry.clone(),
                &self.model,
                self.format.clone(),
                app.clone(),
                self.event_session_id.as_deref().unwrap_or("groupchat"),
                AgentTurnInput {
                    system_prompt: system.to_string(),
                    messages: api_messages,
                    // 绝对兜底上限（非工作限制）：进展感知停滞检测会在无进展时提前收尾，
                    // 此处只需保证极端死循环可中断，复杂任务的长工具链不再受轮数约束。
                    max_iterations: 24,
                    temperature: Some(0.7),
                    max_tokens: None,
                },
                AgentTurnOptions {
                    on_delta,
                    on_progress,
                    approval_handler,
                    auth_level: AuthLevel::Confirm,
                    continue_handler,
                    // 群聊场景审批方为 Director（主持人），拒绝文案据此区分来源
                    approval_label: Some("主持人".into()),
                    // 协作式取消：停止/暂停时迭代边界提前结束（Director 无工具循环，不注入）
                    cancel_token: self.cancel.clone(),
                    // 持久化权限规则：群聊与会话共用同一规则体系（deny>risky>allow>默认）
                    permission_rules: Some(self.permission_rules.clone()),
                    // 工作区目录（工作区内路径免审批）
                    workspace: self.workspace.clone(),
                    // 会话安全模式（本次/本波次有效）
                    security_mode: self.security_mode,
                    // 用量归因：写 api_usage_log.provider（AgentLoop 优先取此注入值）
                    usage_provider: Some(self.provider.clone()),
                    // 群聊 ask_user：拦截工具调用 → 轮末确认（无超时、落库恢复现场）
                    ask_user_behavior: AskUserBehavior::TurnEnd,
                },
            )
            .await?;
            let tool_calls = serde_json::to_string(&output.tool_calls).unwrap_or_else(|_| "[]".into());
            // 思考模式（DeepSeek 等）：从最终 assistant 消息提取 reasoning_content 供落库/回传。
            let reasoning = output
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "assistant")
                .and_then(|m| m.reasoning_content.clone())
                .unwrap_or_default();
            Ok((output.content, tool_calls, reasoning))
        } else {
            let (content, reasoning) = self.complete_raw(system, api_messages, on_delta).await?;
            Ok((content, String::new(), reasoning))
        }
    }
}

fn to_api_message(m: &ChatMessage) -> ApiChatMessage {
    let mut msg = match m.role.as_str() {
        "user" => {
            let imgs = m.images.clone().unwrap_or_default();
            if imgs.is_empty() {
                ApiChatMessage::user(&m.content)
            } else {
                ApiChatMessage::user_with_images(&m.content, imgs)
            }
        }
        "assistant" => ApiChatMessage::assistant_with_reasoning(&m.content, m.reasoning_content.as_deref().unwrap_or("")),
        _ => ApiChatMessage::system(&m.content),
    };
    msg.name = m.name.clone();
    msg
}
