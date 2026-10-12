//! Agent 连接传输抽象层（契约）
//!
//! 设计依据：`docs/PilotDesk-Agent连接方式统一抽象设计-v2.0.md`（v2.6）§4.1 / §4.2
//! 实施依据：`docs/PilotDesk-Agent连接方式统一抽象-实施计划-v1.0.md`（v1.2）§5.2
//!
//! 本模块**只定义契约**，不含任何具体传输实现：
//! - 会话面：`AgentSession`（inprocess / subprocess / http / acp 各自实现）
//! - 统一事件：`UnifiedAgentEvent`（与前端 `agentEventBus` 的 12 类事件一一对应）
//! - 能力标志：`Capabilities`（由运行时协商/探测得到，不落用户配置）
//!
//! P0 阶段本模块尚未被上层引用（`agent/mod.rs` 的接线在 P0-4 落地），
//! 故暂时整体 `allow(dead_code)`；接线完成后删除该 allow。
#![allow(dead_code)]

pub mod jsonrpc;
pub mod subprocess;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────
//  事件模型
// ──────────────────────────────────────────────

/// 统一 Agent 事件：与前端 `agentEventBus` 的 12 类事件一一对应。
///
/// 各传输实现负责把协议事件"投影"到本枚举；上层只认本枚举。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UnifiedAgentEvent {
    /// 正文增量 → `agent-chunk`
    Chunk { session_id: String, content: String },
    /// 思考链增量 → `agent-reasoning`
    Reasoning { session_id: String, content: String },
    /// 工具调用开始 → `agent-tool-start`
    ToolStart {
        session_id: String,
        tool_id: String,
        name: String,
        args: String,
    },
    /// 工具调用结束 → `agent-tool-result`
    ToolResult {
        session_id: String,
        tool_id: String,
        result: String,
        success: bool,
    },
    /// 文件变更 → `agent-file-diff`
    FileDiff {
        session_id: String,
        path: String,
        diff: String,
    },
    /// 权限请求 → `agent-approval-required`
    ApprovalRequired {
        session_id: String,
        call_id: String,
        name: String,
        args: String,
        risk: String,
    },
    /// 执行计划更新（ACP `plan`）
    Plan {
        session_id: String,
        steps: Vec<String>,
    },
    /// 用量 → `agent-usage`
    Usage {
        session_id: String,
        prompt: u32,
        completion: u32,
        total: u32,
    },
    /// 对端会话 id → `agent-session`
    AgentSessionId {
        session_id: String,
        agent_session_id: String,
    },
    /// 正常结束 → `agent-done`
    Done { session_id: String },
    /// 已取消 → `agent-cancelled`
    Cancelled { session_id: String },
    /// 失败 → `agent-error`
    Error { session_id: String, error: String },
}

/// 事件出口：由调用方注入，实现侧通过它回调。
/// 形态与现有 `agent::AsyncCallbacks` 一致，改造面最小。
pub type EventSink = Arc<dyn Fn(UnifiedAgentEvent) + Send + Sync>;

/// 空事件出口（测试 / 占位用）。
pub fn noop_sink() -> EventSink {
    Arc::new(|_| {})
}

// ──────────────────────────────────────────────
//  能力标志
// ──────────────────────────────────────────────

/// 某种传输/某个对端实际具备的能力。
///
/// 由运行时协商（ACP `initialize` / API 能力 / 端点探测）填充并缓存，
/// **不落用户配置**——这正是"改后配置更少"的来源（设计文档 §4.2）。
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// 提供结构化工具调用事件
    pub tool_events: bool,
    /// 支持反向权限请求（可交由本应用审批）
    pub permission: bool,
    /// 提供思考链
    pub reasoning: bool,
    /// 支持会话恢复
    pub resume: bool,
    /// 常驻会话（进程/连接存活即会话存活）
    pub persistent: bool,
    /// 可程序化调用（false = 仅内嵌承载，对应设计文档 §1.1 的 L2）
    pub automatable: bool,
}

// ──────────────────────────────────────────────
//  连接与执行参数
// ──────────────────────────────────────────────

/// 接入方式（`AgentConfig.transport` 的取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    /// 内置 API Agent（进程内）
    InProcess,
    /// 本地 CLI（兼容模式，文本解析）
    Subprocess,
    /// HTTP / SSE（在线版开放 API、远端 Agent；含未来 A2A 方言位）
    Http,
    /// Agent Client Protocol（stdio JSON-RPC）
    Acp,
    /// MCP（能力面；不承载会话）
    Mcp,
    /// 内嵌 Web（承载，不可程序化调用）
    Webview,
}

impl TransportKind {
    /// 落库 / 市场用的稳定字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InProcess => "inprocess",
            Self::Subprocess => "subprocess",
            Self::Http => "http",
            Self::Acp => "acp",
            Self::Mcp => "mcp",
            Self::Webview => "webview",
        }
    }
}

impl Default for TransportKind {
    /// 缺省 `subprocess`：保证现有 Agent 配置**零迁移**（设计文档 §4.2）。
    fn default() -> Self {
        Self::Subprocess
    }
}

/// 建立一次会话所需的连接参数。
#[derive(Debug, Clone)]
pub struct AgentConn {
    /// 本应用侧会话 id（事件回传时携带）
    pub session_id: String,
    /// Agent 标识（`agents.agent_type`）
    pub agent_id: String,
    /// 接入方式
    pub transport: TransportKind,
    /// 工作目录（空串表示使用默认工作区）
    pub cwd: String,
    /// transport 专属参数（JSON）。**密钥不在此处**——走凭据引用（见设计文档 §4.3）。
    pub conn: serde_json::Value,
    /// 恢复用的对端会话 id（ACP `session/load` / CLI `--resume`）
    pub resume_id: Option<String>,
}

/// 会话句柄。
#[derive(Debug, Clone)]
pub struct SessionHandle {
    /// 本应用侧会话 id
    pub session_id: String,
    /// 对端会话 id（ACP sessionId / A2A taskId / CLI resume id）；未建立时为 None
    pub remote_id: Option<String>,
}

/// 一轮用户输入。
#[derive(Debug, Clone, Default)]
pub struct PromptInput {
    pub text: String,
    /// 多模态图片（base64 data URL 或 http(s) URL），与 `ChatMessage.images` 同构
    pub images: Vec<String>,
}

/// 一轮结束原因（归一化 ACP `stopReason` 等）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// 正常结束
    EndTurn,
    /// 达到输出上限
    MaxTokens,
    /// 被取消
    Cancelled,
    /// 对端拒绝
    Refusal,
    /// 其他 / 未知
    Other(String),
}

/// 权限请求的回应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// 仅本次允许
    Allow,
    /// 本会话内持续允许
    AllowAlways,
    /// 拒绝
    Deny,
}

// ──────────────────────────────────────────────
//  错误
// ──────────────────────────────────────────────

/// 传输层统一错误。
#[derive(Debug, Clone)]
pub enum AgentError {
    /// 建立连接失败
    Connect(String),
    /// 协议错误（JSON-RPC error / 响应不可解析）
    Protocol(String),
    /// 传输层错误（进程 / I/O / 网络）
    Transport(String),
    /// 子进程非 0 退出（携带退出码与合并后的输出，供上层做友好错误映射）
    ProcessExited { exit_code: i32, stderr: String },
    /// 当前传输不支持该操作
    Unsupported(String),
    /// 已取消
    Cancelled,
    /// 超时
    Timeout(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(m) => write!(f, "连接失败：{}", m),
            Self::Protocol(m) => write!(f, "协议错误：{}", m),
            Self::Transport(m) => write!(f, "传输错误：{}", m),
            Self::ProcessExited { exit_code, stderr } => {
                if stderr.is_empty() {
                    write!(f, "进程异常退出 (exit code {})", exit_code)
                } else {
                    write!(f, "进程异常退出 (exit code {})：{}", exit_code, stderr)
                }
            }
            Self::Unsupported(m) => write!(f, "不支持的操作：{}", m),
            Self::Cancelled => write!(f, "已取消"),
            Self::Timeout(m) => write!(f, "超时：{}", m),
        }
    }
}

impl std::error::Error for AgentError {}

// ──────────────────────────────────────────────
//  会话面契约
// ──────────────────────────────────────────────

/// 一个可对话的 Agent 连接。
///
/// 实现者：`inprocess`（内置 API Agent）、`subprocess`（现有 CLI 路径，P0-3）、
/// `http`（P1）、`acp`（P2）；`webview` 只实现承载语义（不实现本 trait 的 prompt）。
#[async_trait::async_trait]
pub trait AgentSession: Send + Sync {
    /// 本实现对应的接入方式
    fn transport(&self) -> TransportKind;

    /// 能力标志（协商 / 探测结果）
    fn capabilities(&self) -> Capabilities;

    /// 建立连接并返回会话句柄；事件经 `sink` 回传。
    async fn connect(
        &mut self,
        conn: &AgentConn,
        sink: EventSink,
    ) -> Result<SessionHandle, AgentError>;

    /// 提交一轮用户输入。
    ///
    /// 取 `&self`（而非 `&mut self`）：会话实例以 `Arc<dyn AgentSession>` 共享，
    /// 这样"运行中取消"（`cancel`）能与 `prompt` 并发进入。
    async fn prompt(
        &self,
        handle: &SessionHandle,
        input: PromptInput,
    ) -> Result<StopReason, AgentError>;

    /// 取消当前回合。
    async fn cancel(&self, handle: &SessionHandle) -> Result<(), AgentError>;

    /// 回应反向权限请求；不支持该能力的实现保持默认 no-op（能力缺失即降级，不报错）。
    async fn permission_reply(
        &self,
        _handle: &SessionHandle,
        _req_id: &str,
        _decision: PermissionDecision,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    /// 关闭连接（释放进程 / 网络资源）。
    async fn close(&self, handle: &SessionHandle) -> Result<(), AgentError>;
}
