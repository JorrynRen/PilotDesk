//! PilotDesk 适配层：`PilotDeskLlmClient`（实现框架层 `LlmClient`）。
//!
//! - Director：无工具模式（单次流式补全，输出结构化决策）。
//! - API 参与者：复用抽取后的 `run_agent_turn`（`AgentLoop` 工具循环 + 流式 + 审批）。

use std::sync::Arc;

use crate::api_agent::agent_loop::{ApprovalHandler, AuthLevel, ContinueHandler, RiskLevel, ToolRegistry};
use crate::api_agent::agent_turn::{run_agent_turn, AgentTurnInput, AgentTurnOptions};
use crate::api_agent::client::ApiClient;
use crate::api_agent::types::{ApiFormat, ChatMessage as ApiChatMessage, ChatRequest};

use super::super::participant::{ChatMessage, DeltaFn, LlmClient};

pub struct PilotDeskLlmClient {
    endpoint: String,
    api_key: String,
    model: String,
    format: ApiFormat,
    /// 工具集（Some 时走 AgentLoop 工具循环，None 时走单次补全）。
    tool_registry: Option<Arc<ToolRegistry>>,
    /// 工具模式所需的 AppHandle（AgentLoop 内部用于发射 `agent-*` 事件）。
    app_handle: Option<tauri::AppHandle>,
    /// 工具审批回调（群聊场景指向 Director 裁决）。
    approval_handler: Option<Arc<dyn Fn(&str, &str, &str, RiskLevel) -> bool + Send + Sync>>,
    /// 停滞/迭代上限继续裁决回调（群聊场景指向 Director）。
    continue_handler: Option<Arc<dyn Fn(usize, usize) -> bool + Send + Sync>>,
    /// 工具模式下向前端发射 `agent-*` 事件时使用的会话标识（群聊用 `groupchat:{room_id}:{participant_id}`）。
    event_session_id: Option<String>,
}

impl PilotDeskLlmClient {
    /// 无工具模式（Director）。
    pub fn new(endpoint: String, api_key: String, model: String, format: ApiFormat) -> Self {
        Self {
            endpoint,
            api_key,
            model,
            format,
            tool_registry: None,
            app_handle: None,
            approval_handler: None,
            continue_handler: None,
            event_session_id: None,
        }
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

    fn build_client(&self) -> ApiClient {
        if matches!(self.format, ApiFormat::Anthropic) {
            ApiClient::new(self.endpoint.clone(), self.api_key.clone(), self.format.clone())
        } else {
            ApiClient::new_openai(self.endpoint.clone(), self.api_key.clone())
        }
    }

    async fn complete_raw(
        &self,
        system: &str,
        api_messages: Vec<ApiChatMessage>,
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<String, String> {
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
                        |_, _, _| {},
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
                        |_, _, _| {},
                    )
                    .await?
            }
        };

        Ok(response.content)
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
        let (content, _) = self.complete_with_tool_calls(system, messages, on_delta).await?;
        Ok(content)
    }

    async fn complete_with_tool_calls(
        &self,
        system: &str,
        messages: &[ChatMessage],
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<(String, String), String> {
        let mut api_messages: Vec<ApiChatMessage> = Vec::with_capacity(messages.len());
        for m in messages {
            api_messages.push(to_api_message(m));
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
                    approval_handler,
                    auth_level: AuthLevel::Confirm,
                    continue_handler,
                    // 群聊场景审批方为 Director（主持人），拒绝文案据此区分来源
                    approval_label: Some("主持人".into()),
                },
            )
            .await?;
            let tool_calls = serde_json::to_string(&output.tool_calls).unwrap_or_else(|_| "[]".into());
            Ok((output.content, tool_calls))
        } else {
            let content = self.complete_raw(system, api_messages, on_delta).await?;
            Ok((content, String::new()))
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
        "assistant" => ApiChatMessage::assistant(&m.content),
        _ => ApiChatMessage::system(&m.content),
    };
    msg.name = m.name.clone();
    msg
}
