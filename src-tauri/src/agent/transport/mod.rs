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

pub mod acp;
pub mod http;
pub mod jsonrpc;
pub mod subprocess;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────
//  事件模型
// ──────────────────────────────────────────────

/// 统一 Agent 事件：与前端 `agentEventBus` 的 12 类事件一一对应。
///
/// 各传输实现负责把协议事件"投影"到本枚举；上层只认本枚举。
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    /// 对端失活被终止 → `agent-idle-aborted`（**明确终态**：不是超时、不是普通失败）
    IdleAborted {
        session_id: String,
        idle_secs: u64,
    },
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

    /// 从落库字符串解析；未知或空 → `None`（由调用方决定回退，通常是 `subprocess`）。
    ///
    /// 与 `as_str` 配对，避免"落库一个值、读回另一个值"的漂移。
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "inprocess" => Some(Self::InProcess),
            "subprocess" => Some(Self::Subprocess),
            "http" => Some(Self::Http),
            "acp" => Some(Self::Acp),
            "mcp" => Some(Self::Mcp),
            "webview" => Some(Self::Webview),
            _ => None,
        }
    }
}

impl Default for TransportKind {
    /// 缺省 `subprocess`：保证现有 Agent 配置**零迁移**（设计文档 §4.2）。
    fn default() -> Self {
        Self::Subprocess
    }
}

/// 审批闸门：把"请用户批准"抽象出来，**避免传输层直接依赖 Tauri 状态**。
///
/// 调用方（`lib.rs`）用现有 `PendingApprovals::register(call_id)` 实现它——
/// 于是 CLI / API / ACP 三条路共用同一套审批 UI 与回收逻辑，无需各自造注册表。
///
/// 返回 `true` = 允许；`false` = 拒绝（含超时、无人在场）。
/// 实现方应自带超时：**未决**一律按拒绝处理，绝不默认放行。
#[async_trait::async_trait]
pub trait ApprovalGate: Send + Sync {
    /// 请求一次审批；`call_id` 用于把界面上的决定对应回本次请求。
    async fn request_approval(&self, call_id: &str) -> bool;
}

/// 建立一次会话所需的连接参数。
///
/// 注：不派生 `Debug` —— `on_pid` 是 `dyn Fn`，无法实现 `Debug`。
#[derive(Clone)]
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
    /// 运行时注入的凭据（密钥）：由调用方从独立存储解析后注入，**不落库、不进 `conn`**。
    pub secret: Option<String>,
    /// 恢复用的对端会话 id（ACP `session/load` / CLI `--resume`）
    pub resume_id: Option<String>,
    /// 取消令牌：调用方与实现**共享同一份**，任一侧置位即终止本回合
    pub abort: Arc<AtomicBool>,
    /// 进程启动回调（携带 pid）：供调用方注册进程表，以支持按 pid 终止整棵进程树
    pub on_pid: Option<PidCallback>,
    /// 审批闸门（`None` = 未接入审批 → 需要授权的请求一律按拒绝处理）
    pub approvals: Option<Arc<dyn ApprovalGate>>,
}

/// 进程启动回调（`Fn` 而非 `FnMut`：可能被多线程共享）。
pub type PidCallback = Arc<dyn Fn(u32) + Send + Sync>;

/// 一轮执行的产出。
#[derive(Debug, Clone)]
pub struct PromptOutput {
    /// 结束原因
    pub stop_reason: StopReason,
    /// 本回合累计正文（口径与现有 `ExecuteResult.stdout` 一致）
    pub text: String,
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
    /// 对端失活：连续静默超过阈值（无在途工具、未等待用户决策），已主动取消并终止本轮。
    ///
    /// 与 `Timeout` 区分开：这不是"我们等得不够久"，而是"对端已停止回传"，
    /// 上层据此给出**明确**的终态（`agent-idle-aborted`），而不是笼统的"请求失败"。
    IdleAborted { idle_secs: u64 },
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
            Self::IdleAborted { idle_secs } => {
                write!(f, "对端已停止回传（静默 {}s）", idle_secs)
            }
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
    ) -> Result<PromptOutput, AgentError>;

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

// ──────────────────────────────────────────────
//  会话工厂（路由原语）
// ──────────────────────────────────────────────

/// 按接入方式构造会话实现 —— 这是"路由"的**唯一原语**：
/// 上层只需拿到 `AgentDescriptor.transport`，交给它即可，无需认识各传输类型。
///
/// `config` 仅 `subprocess` 需要（命令模板与输出解析器都来自 `AgentConfig`）；
/// `http` / `acp` 自描述，不需要它。
///
/// 返回 `None` 表示该接入方式**尚无可承载会话的实现**：
/// - `Mcp`：是**能力面**（提供工具），不承载会话；
/// - `Webview`：是**承载**（需人工操作），不是可调用 Agent；
/// - `InProcess`：内置 API Agent 仍走 `lib.rs` 的 `agent_loop`（`inprocess.rs` 推迟到 P1）。
pub fn build_session(
    kind: TransportKind,
    config: Option<crate::agent::config::AgentConfig>,
) -> Option<Box<dyn AgentSession>> {
    match kind {
        TransportKind::Subprocess => config
            .map(|c| Box::new(subprocess::SubprocessSession::new(c)) as Box<dyn AgentSession>),
        TransportKind::Acp => Some(Box::new(acp::AcpSession::new())),
        TransportKind::Http => Some(Box::new(http::HttpSession::new())),
        TransportKind::InProcess | TransportKind::Mcp | TransportKind::Webview => None,
    }
}

#[cfg(test)]
mod factory_tests {
    use super::*;
    use crate::agent::config::AgentConfig;

    /// 自描述的传输无需 config 即可构造，且回报的 transport 与请求一致
    #[test]
    fn builds_self_describing_transports_without_config() {
        let acp = build_session(TransportKind::Acp, None).expect("acp 应可构造");
        assert_eq!(acp.transport(), TransportKind::Acp);

        let http = build_session(TransportKind::Http, None).expect("http 应可构造");
        assert_eq!(http.transport(), TransportKind::Http);
    }

    /// subprocess 依赖 AgentConfig：缺 config 必须拒绝构造，而不是造出个跑不了的会话
    #[test]
    fn subprocess_requires_config() {
        assert!(build_session(TransportKind::Subprocess, None).is_none());
        let cfg = AgentConfig {
            agent_type: "claude".into(),
            ..Default::default()
        };
        let s = build_session(TransportKind::Subprocess, Some(cfg)).expect("有 config 应可构造");
        assert_eq!(s.transport(), TransportKind::Subprocess);
    }

    /// 能力面 / 承载类 / 未实现者一律返回 None（而非造一个空会话）
    #[test]
    fn non_session_transports_return_none() {
        for k in [
            TransportKind::Mcp,
            TransportKind::Webview,
            TransportKind::InProcess,
        ] {
            assert!(
                build_session(k, None).is_none(),
                "{:?} 不应构造出会话",
                k
            );
        }
    }

    /// `as_str` / `from_db_str` 必须互为逆运算（落库值不允许漂移）
    #[test]
    fn transport_kind_db_roundtrip() {
        for k in [
            TransportKind::InProcess,
            TransportKind::Subprocess,
            TransportKind::Http,
            TransportKind::Acp,
            TransportKind::Mcp,
            TransportKind::Webview,
        ] {
            assert_eq!(TransportKind::from_db_str(k.as_str()), Some(k));
        }
        assert_eq!(TransportKind::from_db_str(""), None);
        assert_eq!(TransportKind::from_db_str("AGP"), None);
    }
}
