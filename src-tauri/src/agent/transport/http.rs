//! HTTP / SSE 传输：接入"在线版 Agent 的开放 API"（设计文档 D8）。
//!
//! 复用 `api_agent::client::ApiClient`——它已具备 OpenAI 兼容请求体、
//! 流式 SSE 解析、429 退避重试、chunk 空闲超时与**协作式取消**（`with_abort`），
//! 本实现只负责把一轮交互投影为 `UnifiedAgentEvent`（`Chunk` / `Reasoning` / `Usage`）。
//!
//! 与 `subprocess` 保持同一约定：**只发增量事件**（Chunk / Reasoning / Usage），
//! `Done` / `Error` 由上层按 `Result` 决定，避免两处各发一次收尾事件。
//!
//! 边界：本传输承载的是"模型端点"（在线产品开放 API）。
//! 需要工具调用 / 审批的完整编排仍属内置 API Agent（`inprocess`，P1 后续）。
//!
//! `AgentConn.conn` 期望字段（JSON）：`baseUrl`、`apiKey`、`model`。
//! 注：`apiKey` 暂随连接参数携带；待 §七 步骤 B 的 `agent_secrets` 落地后改为凭据引用。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::agent::transport::{
    AgentConn, AgentError, AgentSession, Capabilities, EventSink, PromptInput, PromptOutput,
    SessionHandle, StopReason, TransportKind, UnifiedAgentEvent,
};
use crate::api_agent::client::ApiClient;
use crate::api_agent::types::{ChatMessage, ChatRequest};

/// 在线版 Agent 的 HTTP 会话（无状态：每轮一个请求）。
pub struct HttpSession {
    endpoint: String,
    api_key: String,
    model: String,
    session_id: String,
    sink: Option<EventSink>,
    abort: Arc<AtomicBool>,
}

impl HttpSession {
    pub fn new() -> Self {
        Self {
            endpoint: String::new(),
            api_key: String::new(),
            model: String::new(),
            session_id: String::new(),
            sink: None,
            abort: Arc::new(AtomicBool::new(false)),
        }
    }

    fn emit(&self, ev: UnifiedAgentEvent) {
        if let Some(sink) = &self.sink {
            sink(ev);
        }
    }

    fn conn_str(conn: &serde_json::Value, key: &str) -> String {
        conn.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    }
}

impl Default for HttpSession {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentSession for HttpSession {
    fn transport(&self) -> TransportKind {
        TransportKind::Http
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // 本传输不带 tools（纯模型端点）
            tool_events: false,
            permission: false,
            // OpenAI 兼容流可带 reasoning_content（DeepSeek 等）
            reasoning: true,
            resume: false,
            persistent: false,
            automatable: true,
        }
    }

    async fn connect(
        &mut self,
        conn: &AgentConn,
        sink: EventSink,
    ) -> Result<SessionHandle, AgentError> {
        self.endpoint = Self::conn_str(&conn.conn, "baseUrl");
        // 凭据优先取运行时注入的 `secret`（§4.3：密钥不进 conn）；回落 `conn.apiKey` 以兼容早期配置
        self.api_key = conn
            .secret
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| Self::conn_str(&conn.conn, "apiKey"));
        self.model = Self::conn_str(&conn.conn, "model");
        if self.endpoint.is_empty() {
            return Err(AgentError::Connect(
                "缺少 conn.baseUrl（在线版 API 端点）".to_string(),
            ));
        }
        if self.model.is_empty() {
            return Err(AgentError::Connect(
                "缺少 conn.model（该端点的模型名）".to_string(),
            ));
        }
        self.session_id = conn.session_id.clone();
        self.sink = Some(sink);
        self.abort = Arc::clone(&conn.abort);
        Ok(SessionHandle {
            session_id: conn.session_id.clone(),
            remote_id: None,
        })
    }

    async fn prompt(
        &self,
        _handle: &SessionHandle,
        input: PromptInput,
    ) -> Result<PromptOutput, AgentError> {
        // 单轮请求：仅携带本轮用户输入（多轮上下文由上层维护）
        let mut user_msg = ChatMessage::user(&input.text);
        if !input.images.is_empty() {
            user_msg.images = Some(input.images.clone());
        }
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![user_msg],
            tools: None,
            tool_choice: None,
            stream: true,
            temperature: None,
            max_tokens: None,
            response_format: None,
        };

        let client = ApiClient::new_openai(self.endpoint.clone(), self.api_key.clone())
            .with_abort(Arc::clone(&self.abort));

        // 各回调各自持有 sink / session_id 的克隆，避免同时借用同一变量
        let s_chunk = self.sink.clone();
        let s_reason = self.sink.clone();
        let s_usage = self.sink.clone();
        let sid_chunk = self.session_id.clone();
        let sid_reason = self.session_id.clone();
        let sid_usage = self.session_id.clone();

        let mut text = String::new();

        let result = client
            .chat_stream(
                &request,
                |c: &str| {
                    text.push_str(c);
                    if let Some(sink) = &s_chunk {
                        sink(UnifiedAgentEvent::Chunk {
                            session_id: sid_chunk.clone(),
                            content: c.to_string(),
                        });
                    }
                },
                |r: &str| {
                    if let Some(sink) = &s_reason {
                        sink(UnifiedAgentEvent::Reasoning {
                            session_id: sid_reason.clone(),
                            content: r.to_string(),
                        });
                    }
                },
                |_id: &str, _name: &str, _args: &str| {
                    // 本传输不带 tools，工具事件不会产生
                },
                |prompt: u32, completion: u32, total: u32, _cache_read: u32, _cache_write: u32| {
                    if let Some(sink) = &s_usage {
                        sink(UnifiedAgentEvent::Usage {
                            session_id: sid_usage.clone(),
                            prompt,
                            completion,
                            total,
                        });
                    }
                },
            )
            .await;

        match result {
            Ok(resp) => Ok(PromptOutput {
                stop_reason: match resp.finish_reason.as_str() {
                    "length" => StopReason::MaxTokens,
                    _ => StopReason::EndTurn,
                },
                // 以流式累积为准；若回调未触发（异常路径）则回落响应体
                text: if text.is_empty() { resp.content } else { text },
            }),
            Err(e) => {
                // 区分"用户取消"与真实失败（与 subprocess 的约定一致）
                if self.abort.load(Ordering::SeqCst) {
                    Err(AgentError::Cancelled)
                } else {
                    Err(AgentError::Transport(e))
                }
            }
        }
    }

    async fn cancel(&self, _handle: &SessionHandle) -> Result<(), AgentError> {
        // ApiClient 的流式读取循环每个 chunk 检查一次该标志
        self.abort.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn close(&self, _handle: &SessionHandle) -> Result<(), AgentError> {
        // 无状态：无长驻连接需要释放
        Ok(())
    }
}
