//! Agent Client Protocol（ACP）传输 —— **第一步：协议类型与事件投影**。
//!
//! 设计依据：`docs/PilotDesk-Agent连接方式统一抽象设计-v2.0.md`（v2.6）§3 / §P2。
//! 复用 `transport::jsonrpc::JsonRpcPeer`（双向：既能发请求，也能接对端反向请求）。
//!
//! 本文件当前只包含**纯函数**部分（方法名、`stopReason` 归一、`session/update` 投影）——
//! 它们不依赖进程与网络，可直接单测。
//!
//! 尚未实现（下一步）：`AcpSession`（spawn ACP 进程 → `initialize` 握手 → `session/new`
//! → `session/prompt` 流式投影 → `session/request_permission` 接进 `PendingApprovals`
//! → `session/cancel`）。真机原型用 `hermes acp`（原生通道，链路最短）。
//!
//! ⚠️ 方法名与字段名取自 ACP schema（v1）；**真机联调前需按实测校准**——
//! 本仓库已有先例（Codex `mcp-server` 被 App Server 取代），协议会变。

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::jsonrpc::JsonRpcPeer;
use crate::agent::transport::{
    AgentConn, AgentError, AgentSession, Capabilities, EventSink, PermissionDecision, PromptInput,
    PromptOutput, SessionHandle, StopReason, TransportKind, UnifiedAgentEvent,
};

/// 本实现使用的 ACP 协议版本（客户端在 `initialize` 中声明）。
pub const ACP_PROTOCOL_VERSION: u32 = 1;

/// 单轮 `session/prompt` 的等待上限。
///
/// **不能沿用控制面的 30s 默认超时**：一次"含工具调用"的复杂任务要跑数分钟，
/// 30s 判死会让对端迟到的回合结束响应被丢弃（`jsonrpc` 超时后即把 pending 移除，
/// 迟到的响应按 id 找不到接收端 → 静默丢弃），回合结束信号永久丢失，
/// 表现为"任务已完成却仍在等输出"，最终由前端静默看门狗以 180s 误报
/// "长时间无响应：已停止本轮"。回合时长 ≈ 任务时长，故与后端
/// `AGENT_LOOP_TIMEOUT_SECS`(1800) 对齐；控制面（initialize / session-new）仍用 30s。
pub const TURN_TIMEOUT: Duration = Duration::from_secs(1800);

/// ACP 方法名（客户端 → 对端）。
pub mod method {
    /// 握手：交换协议版本与能力
    pub const INITIALIZE: &str = "initialize";
    /// 新建会话
    pub const SESSION_NEW: &str = "session/new";
    /// 载入既有会话（会话恢复）
    pub const SESSION_LOAD: &str = "session/load";
    /// 提交一轮用户输入
    pub const SESSION_PROMPT: &str = "session/prompt";
    /// 取消当前回合（通知）
    pub const SESSION_CANCEL: &str = "session/cancel";
}

/// ACP **对端 → 客户端** 的方法名（反向请求/通知）。
pub mod peer_method {
    /// 流式更新（通知）：正文/思考/工具/计划
    pub const SESSION_UPDATE: &str = "session/update";
    /// 权限请求（请求，需回写响应）
    pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
}

/// ACP `stopReason` → 本应用 `StopReason`。
///
/// 未知取值落到 `Other`，避免协议新增取值时破坏枚举（设计文档 D6 的教训）。
pub fn map_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "cancelled" => StopReason::Cancelled,
        "refusal" => StopReason::Refusal,
        other => StopReason::Other(other.to_string()),
    }
}

/// 把 `session/update` 通知投影为统一事件（绑定会话 id）。
///
/// 支持的通知种类（`update.sessionUpdate`）：
/// `agent_message_chunk` / `agent_thought_chunk` / `tool_call` / `tool_call_update` / `plan`。
/// 未知种类返回空 —— ACP 会持续新增种类，客户端必须能忽略。
pub fn project_session_update(session_id: &str, params: &Value) -> Vec<UnifiedAgentEvent> {
    let update = &params["update"];
    let kind = update["sessionUpdate"].as_str().unwrap_or("");
    let sid = || session_id.to_string();

    match kind {
        // 正文增量
        "agent_message_chunk" => match update["content"]["text"].as_str() {
            Some(t) => vec![UnifiedAgentEvent::Chunk {
                session_id: sid(),
                content: t.to_string(),
            }],
            None => Vec::new(),
        },
        // 思考链增量
        "agent_thought_chunk" => match update["content"]["text"].as_str() {
            Some(t) => vec![UnifiedAgentEvent::Reasoning {
                session_id: sid(),
                content: t.to_string(),
            }],
            None => Vec::new(),
        },
        // 工具调用开始
        "tool_call" => vec![UnifiedAgentEvent::ToolStart {
            session_id: sid(),
            tool_id: update["toolCallId"].as_str().unwrap_or_default().to_string(),
            name: update["title"]
                .as_str()
                .or_else(|| update["kind"].as_str())
                .unwrap_or_default()
                .to_string(),
            args: update["rawInput"].to_string(),
        }],
        // 工具调用结束（仅终态产出结果事件）
        "tool_call_update" => {
            let status = update["status"].as_str().unwrap_or("");
            if status == "completed" || status == "failed" {
                vec![UnifiedAgentEvent::ToolResult {
                    session_id: sid(),
                    tool_id: update["toolCallId"].as_str().unwrap_or_default().to_string(),
                    result: update["rawOutput"].to_string(),
                    success: status == "completed",
                }]
            } else {
                Vec::new()
            }
        }
        // 执行计划
        "plan" => {
            let steps = update["entries"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|e| {
                            e["content"]
                                .as_str()
                                .or_else(|| e["title"].as_str())
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default();
            vec![UnifiedAgentEvent::Plan {
                session_id: sid(),
                steps,
            }]
        }
        _ => Vec::new(),
    }
}

/// 把对端 stderr（若有）附加到协议错误上。
///
/// ACP 对端的**致命错误常常只写在 stderr**（例：`hermes acp` 缺 `.[acp]` 依赖），
/// 不附加的话用户只会看到"JSON-RPC 请求超时"，无从排查。
fn with_peer_stderr(err: String, stderr: &Arc<Mutex<String>>) -> AgentError {
    let detail = stderr
        .lock()
        .map(|b| b.trim().to_string())
        .unwrap_or_default();
    if detail.is_empty() {
        AgentError::Protocol(err)
    } else {
        AgentError::Protocol(format!("{}（对端 stderr: {}）", err, detail))
    }
}

// ──────────────────────────────────────────────
//  权限请求：选项匹配
// ──────────────────────────────────────────────

/// ACP 权限请求给出的一个选项（`params.options[]` 元素）。
///
/// ⚠️ 形状取自 schema（`{optionId, name, kind}`），**尚未观测到真实样例**——
/// 本步只做"决策 → optionId"的纯逻辑，接入审批时一并用真机复核。
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionOption {
    pub option_id: String,
    /// 语义类型，常见值：`allow_once` / `allow_always` / `reject_once` / `reject_always`
    pub kind: String,
}

/// 从权限请求参数里解析选项列表（缺失/形状不符时返回空）。
pub fn parse_permission_options(params: &Value) -> Vec<PermissionOption> {
    params["options"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|o| PermissionOption {
                    option_id: o["optionId"].as_str().unwrap_or_default().to_string(),
                    kind: o["kind"].as_str().unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 把我们的三态决策映射到 ACP 的 `optionId`。
///
/// 匹配顺序：**精确 kind** → **前缀**（`allow*` / `reject*`）→ 兜底
/// （`AllowAlways` 退化为 `Allow`；`Deny` 取最后一项——多数实现把拒绝放末位）。
/// 无可用选项时返回 `None`，调用方应据此按拒绝处理（**绝不默认放行**）。
pub fn pick_option_id(options: &[PermissionOption], decision: PermissionDecision) -> Option<String> {
    if options.is_empty() {
        return None;
    }

    let exact: &[&str] = match decision {
        PermissionDecision::Allow => &["allow_once", "allow"],
        PermissionDecision::AllowAlways => &["allow_always"],
        PermissionDecision::Deny => &["reject_once", "reject"],
    };
    for want in exact {
        if let Some(o) = options.iter().find(|o| o.kind == *want) {
            return Some(o.option_id.clone());
        }
    }

    let prefix = match decision {
        PermissionDecision::Allow | PermissionDecision::AllowAlways => "allow",
        PermissionDecision::Deny => "reject",
    };
    if let Some(o) = options.iter().find(|o| o.kind.starts_with(prefix)) {
        return Some(o.option_id.clone());
    }

    match decision {
        // 无 allow_always 可用 → 退化为一次允许（不会扩大授权范围）
        PermissionDecision::AllowAlways => pick_option_id(options, PermissionDecision::Allow),
        // 拒绝兜底：取末位选项
        PermissionDecision::Deny => options.last().map(|o| o.option_id.clone()),
        PermissionDecision::Allow => None,
    }
}

/// 构造 ACP 权限响应体（`outcome.selected.optionId`）。
///
/// 期望允许但**没有** `allow*` 选项时，退回挑一个**合法**的拒绝项——
/// 宁可按拒绝回答，也不发一个对方不认识的 optionId。
/// 纯函数以便单测；`optionId` 形状取自 schema，**待真机复核**。
pub fn permission_response(options: &[PermissionOption], allowed: bool) -> Value {
    let decision = if allowed {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Deny
    };
    let option_id = pick_option_id(options, decision)
        .or_else(|| pick_option_id(options, PermissionDecision::Deny))
        .unwrap_or_else(|| "reject_once".to_string());
    json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    use serde_json::json;

    fn opts(items: &[(&str, &str)]) -> Vec<PermissionOption> {
        items
            .iter()
            .map(|(id, kind)| PermissionOption {
                option_id: (*id).to_string(),
                kind: (*kind).to_string(),
            })
            .collect()
    }

    /// 标准四选项：精确 kind 优先命中
    #[test]
    fn picks_exact_kinds() {
        let o = opts(&[
            ("a1", "allow_once"),
            ("a2", "allow_always"),
            ("r1", "reject_once"),
        ]);
        assert_eq!(
            pick_option_id(&o, PermissionDecision::Allow),
            Some("a1".to_string())
        );
        assert_eq!(
            pick_option_id(&o, PermissionDecision::AllowAlways),
            Some("a2".to_string())
        );
        assert_eq!(
            pick_option_id(&o, PermissionDecision::Deny),
            Some("r1".to_string())
        );
    }

    /// 前缀兜底 + 安全默认
    #[test]
    fn falls_back_by_prefix_and_never_auto_allows() {
        // 无 allow_once（只有 allow_always）→ 前缀命中
        let o = opts(&[("a2", "allow_always"), ("r2", "reject_always")]);
        assert_eq!(
            pick_option_id(&o, PermissionDecision::Allow),
            Some("a2".to_string())
        );
        assert_eq!(
            pick_option_id(&o, PermissionDecision::Deny),
            Some("r2".to_string())
        );

        // 只有拒绝项时，Allow 必须返回 None（**绝不默认放行**）
        let only_reject = opts(&[("r1", "reject_once")]);
        assert_eq!(pick_option_id(&only_reject, PermissionDecision::Allow), None);

        // kind 形状不符（空串）→ 前缀不命中；Deny 取末位
        let unknown = opts(&[("u1", ""), ("u2", "")]);
        assert_eq!(
            pick_option_id(&unknown, PermissionDecision::Deny),
            Some("u2".to_string())
        );
        assert_eq!(pick_option_id(&[], PermissionDecision::Deny), None);
    }

    /// 参数解析：正常 / 缺失
    #[test]
    fn parses_options_from_params() {
        let p = json!({ "options": [ { "optionId": "x", "kind": "allow_once", "name": "允许" } ] });
        assert_eq!(parse_permission_options(&p), opts(&[("x", "allow_once")]));
        assert!(parse_permission_options(&json!({})).is_empty());
    }

    /// 响应体构造：允许 / 拒绝 / 无 allow 项时的安全退化
    #[test]
    fn builds_permission_response() {
        let o = opts(&[("a1", "allow_once"), ("r1", "reject_once")]);
        assert_eq!(permission_response(&o, true)["outcome"]["optionId"], "a1");
        assert_eq!(permission_response(&o, false)["outcome"]["optionId"], "r1");

        // 期望允许但对方没给 allow* 选项 → 退回一个**合法**的拒绝项（不发未知 id）
        let only_reject = opts(&[("r1", "reject_once")]);
        assert_eq!(
            permission_response(&only_reject, true)["outcome"]["optionId"],
            "r1"
        );
    }
}

// ──────────────────────────────────────────────
//  AcpSession：会话面实现
// ──────────────────────────────────────────────

/// 一条 ACP 连接（常驻子进程 + 双向 JSON-RPC）。
///
/// 与 `subprocess` 的关键差别：**常驻会话**——进程与连接在会话存续期内保持，
/// 不需要每轮冷启动、不需要靠正则抓 session id。
pub struct AcpSession {
    peer: Option<JsonRpcPeer>,
    session_id: String,
    remote_id: Option<String>,
    sink: Option<EventSink>,
    caps: Capabilities,
    /// 本轮累计正文（`Chunk` 事件由 `session/update` 处理器追加）
    text_buf: Arc<Mutex<String>>,
}

impl AcpSession {
    pub fn new() -> Self {
        Self {
            peer: None,
            session_id: String::new(),
            remote_id: None,
            sink: None,
            caps: Capabilities::default(),
            text_buf: Arc::new(Mutex::new(String::new())),
        }
    }

    fn emit(&self, ev: UnifiedAgentEvent) {
        if let Some(sink) = &self.sink {
            sink(ev);
        }
    }

    fn conn_str(conn: &Value, key: &str) -> String {
        conn.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    }
}

impl Default for AcpSession {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentSession for AcpSession {
    fn transport(&self) -> TransportKind {
        TransportKind::Acp
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    async fn connect(
        &mut self,
        conn: &AgentConn,
        sink: EventSink,
    ) -> Result<SessionHandle, AgentError> {
        self.session_id = conn.session_id.clone();
        self.sink = Some(Arc::clone(&sink));

        // ── 1) 启动 ACP 进程 ──
        // `conn.command` / `conn.args`：如 hermes → ["acp"]、gemini → ["--acp"]
        let command = Self::conn_str(&conn.conn, "command");
        if command.is_empty() {
            return Err(AgentError::Connect(
                "缺少 conn.command（ACP 可执行文件）".to_string(),
            ));
        }
        let args: Vec<String> = conn.conn["args"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // 复用 Windows 垫片解析（npm 安装的 CLI 常常没有同名 .exe）
        let (program, prefix) = crate::utils::process::resolve_windows_program(&command);
        let mut cmd = crate::utils::process::hidden_tokio_command(&program);
        cmd.args(&prefix)
            .args(&args)
            .current_dir(&conn.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| AgentError::Connect(format!("启动 ACP 进程失败: {}", e)))?;

        // stderr **必须捕获**：ACP 对端把"依赖未安装"这类致命信息写在 stderr，
        // 若丢成 Stdio::null()，失败只会表现为 initialize 超时（30s），用户看不到真正原因。
        // （实测：`hermes acp` 未装 `.[acp]` extra 时正是如此。）
        let stderr_text = Arc::new(Mutex::new(String::new()));
        if let Some(mut err) = child.stderr.take() {
            let buf = Arc::clone(&stderr_text);
            tokio::spawn(async move {
                let mut s = String::new();
                let _ = err.read_to_string(&mut s).await;
                if let Ok(mut b) = buf.lock() {
                    b.push_str(&s);
                }
            });
        }

        let peer = JsonRpcPeer::new(child).map_err(AgentError::Connect)?;

        // ── 2) 注册"对端 → 客户端"的处理器 ──
        // 2a) session/update：流式正文/思考/工具/计划
        let sink_update = Arc::clone(&sink);
        let sid_update = self.session_id.clone();
        let text_buf = Arc::clone(&self.text_buf);
        peer.on(
            peer_method::SESSION_UPDATE,
            Arc::new(move |_peer, _id, params| {
                for ev in project_session_update(&sid_update, &params) {
                    if let UnifiedAgentEvent::Chunk { content, .. } = &ev {
                        if let Ok(mut buf) = text_buf.lock() {
                            buf.push_str(content);
                        }
                    }
                    sink_update(ev);
                }
            }),
        );

        // 2b) session/request_permission：权限请求 → 审批闸门
        // 安全默认：闸门缺失、或超时未决 → **拒绝**（绝不默认放行）；
        // 且只在**真有闸门**时才发审批卡——没闸门却发卡，只会让用户点了没反应。
        let sink_perm = Arc::clone(&sink);
        let sid_perm = self.session_id.clone();
        let gate_perm = conn.approvals.clone();
        peer.on(
            peer_method::SESSION_REQUEST_PERMISSION,
            Arc::new(move |peer, id, params| {
                let options = parse_permission_options(&params);
                let call_id = format!(
                    "acp-{}-{}",
                    sid_perm,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0)
                );
                let gate = gate_perm.clone();

                if gate.is_some() {
                    sink_perm(UnifiedAgentEvent::ApprovalRequired {
                        session_id: sid_perm.clone(),
                        call_id: call_id.clone(),
                        name: params["toolCall"]["title"]
                            .as_str()
                            .unwrap_or("工具调用")
                            .to_string(),
                        args: params["toolCall"]["rawInput"].to_string(),
                        risk: String::new(),
                    });
                }

                let peer = peer.clone();
                tokio::spawn(async move {
                    let allowed = match gate {
                        Some(g) => tokio::time::timeout(
                            std::time::Duration::from_secs(300),
                            g.request_approval(&call_id),
                        )
                        .await
                        .unwrap_or(false),
                        None => {
                            log::warn!("[acp] 权限请求：审批闸门未接入 → 拒绝: {}", params);
                            false
                        }
                    };
                    if let Some(id) = id {
                        peer.respond(id, Ok(permission_response(&options, allowed)))
                            .await;
                    }
                });
            }),
        );

        // ── 3) initialize 握手 ──
        let _init = peer
            .request(
                method::INITIALIZE,
                json!({
                    "protocolVersion": ACP_PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": { "readTextFile": false, "writeTextFile": false }
                    }
                }),
            )
            .await
            .map_err(|e| with_peer_stderr(e, &stderr_text))?;

        // ── 4) 建会话（有 resume_id 则走 session/load）──
        let new_params = json!({ "cwd": conn.cwd, "mcpServers": [] });
        let resp = match &conn.resume_id {
            Some(rid) => {
                let mut p = new_params.clone();
                p["sessionId"] = json!(rid);
                peer.request(method::SESSION_LOAD, p).await
            }
            None => peer.request(method::SESSION_NEW, new_params).await,
        }
        .map_err(|e| with_peer_stderr(e, &stderr_text))?;

        self.remote_id = resp["sessionId"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| conn.resume_id.clone());

        // ACP 天生具备结构化能力（工具事件/思考/常驻会话/会话恢复）；
        // permission 暂标 false：审批尚未接入（见 2b 的 TODO），不能声称支持。
        self.caps = Capabilities {
            tool_events: true,
            permission: false,
            reasoning: true,
            resume: true,
            persistent: true,
            automatable: true,
        };
        self.peer = Some(peer);

        Ok(SessionHandle {
            session_id: self.session_id.clone(),
            remote_id: self.remote_id.clone(),
        })
    }

    async fn prompt(
        &self,
        handle: &SessionHandle,
        input: PromptInput,
    ) -> Result<PromptOutput, AgentError> {
        let peer = self
            .peer
            .as_ref()
            .ok_or_else(|| AgentError::Transport("ACP 尚未连接".to_string()))?;
        let sid = handle
            .remote_id
            .clone()
            .ok_or_else(|| AgentError::Transport("会话未建立（缺少对端 sessionId）".to_string()))?;

        if let Ok(mut buf) = self.text_buf.lock() {
            buf.clear();
        }

        let mut blocks = vec![json!({ "type": "text", "text": input.text })];
        for img in &input.images {
            // ACP 图片块形状待真机校准（此处按 uri 传）
            blocks.push(json!({ "type": "image", "uri": img }));
        }

        // 诊断打点：本轮"结束信号"是否到达、几时到达。
        log::info!("[acp] session/prompt 发出，等待对端回合结束响应：session={}", sid);
        let resp = peer
            .request_with_timeout(
                method::SESSION_PROMPT,
                json!({ "sessionId": sid, "prompt": blocks }),
                TURN_TIMEOUT,
            )
            .await
            .map_err(|e| {
                log::warn!("[acp] session/prompt 未拿到回合结束响应：session={} err={}", sid, e);
                AgentError::Protocol(e)
            })?;
        log::info!(
            "[acp] session/prompt 收到回合结束响应：session={} stopReason={}",
            sid,
            resp["stopReason"].as_str().unwrap_or("<缺失>")
        );

        let stop = resp["stopReason"].as_str().unwrap_or("end_turn");
        let text = self
            .text_buf
            .lock()
            .map(|b| b.clone())
            .unwrap_or_default();

        Ok(PromptOutput {
            stop_reason: map_stop_reason(stop),
            text,
        })
    }

    async fn cancel(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        let peer = self
            .peer
            .as_ref()
            .ok_or_else(|| AgentError::Transport("ACP 尚未连接".to_string()))?;
        let sid = handle.remote_id.clone().unwrap_or_default();
        // 走协议取消（由对端负责终止其内部工具），而不是粗暴 kill 进程
        peer.notify(method::SESSION_CANCEL, json!({ "sessionId": sid }))
            .await
            .map_err(AgentError::Protocol)
    }

    async fn close(&self, _handle: &SessionHandle) -> Result<(), AgentError> {
        if let Some(peer) = &self.peer {
            peer.kill().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stop_reason_maps_known_values() {
        assert_eq!(map_stop_reason("end_turn"), StopReason::EndTurn);
        assert_eq!(map_stop_reason("max_tokens"), StopReason::MaxTokens);
        assert_eq!(map_stop_reason("cancelled"), StopReason::Cancelled);
        assert_eq!(map_stop_reason("refusal"), StopReason::Refusal);
        // 未知取值不破坏枚举
        assert_eq!(
            map_stop_reason("some_future_reason"),
            StopReason::Other("some_future_reason".to_string())
        );
    }

    #[test]
    fn projects_message_and_thought_chunks() {
        let msg = json!({ "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "hello" } } });
        assert_eq!(
            project_session_update("s1", &msg),
            vec![UnifiedAgentEvent::Chunk {
                session_id: "s1".into(),
                content: "hello".into()
            }]
        );

        let thought = json!({ "update": { "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hmm" } } });
        assert_eq!(
            project_session_update("s1", &thought),
            vec![UnifiedAgentEvent::Reasoning {
                session_id: "s1".into(),
                content: "hmm".into()
            }]
        );
    }

    #[test]
    fn projects_tool_call_lifecycle() {
        let start = json!({ "update": { "sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "Read file", "rawInput": { "path": "a.rs" } } });
        match project_session_update("s1", &start).as_slice() {
            [UnifiedAgentEvent::ToolStart {
                tool_id,
                name,
                args,
                ..
            }] => {
                assert_eq!(tool_id, "call_1");
                assert_eq!(name, "Read file");
                assert!(args.contains("a.rs"));
            }
            other => panic!("unexpected: {:?}", other),
        }

        // 非终态不产出结果事件
        let running = json!({ "update": { "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "in_progress" } });
        assert!(project_session_update("s1", &running).is_empty());

        let done = json!({ "update": { "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "completed", "rawOutput": "ok" } });
        match project_session_update("s1", &done).as_slice() {
            [UnifiedAgentEvent::ToolResult {
                tool_id,
                result,
                success,
                ..
            }] => {
                assert_eq!(tool_id, "call_1");
                assert!(success);
                assert!(result.contains("ok"));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn projects_plan_and_ignores_unknown() {
        let plan = json!({ "update": { "sessionUpdate": "plan", "entries": [ { "content": "step 1" }, { "content": "step 2" } ] } });
        assert_eq!(
            project_session_update("s1", &plan),
            vec![UnifiedAgentEvent::Plan {
                session_id: "s1".into(),
                steps: vec!["step 1".to_string(), "step 2".to_string()]
            }]
        );

        // 未知种类必须被安静忽略（ACP 会持续新增）
        let unknown = json!({ "update": { "sessionUpdate": "brand_new_kind", "foo": 1 } });
        assert!(project_session_update("s1", &unknown).is_empty());
    }
}
