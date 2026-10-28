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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::jsonrpc::JsonRpcPeer;
use crate::agent::transport::{
    AcpMode, AgentConn, AgentError, AgentSession, Capabilities, EventSink, PermissionDecision,
    PromptInput, PromptOutput, SessionHandle, StopReason, TransportKind, UnifiedAgentEvent,
};
use crate::api_agent::agent_loop::{approval_floor, SecurityMode};
use crate::tools::RiskLevel;

/// 本实现使用的 ACP 协议版本（客户端在 `initialize` 中声明）。
pub const ACP_PROTOCOL_VERSION: u32 = 1;

/// 单轮 `session/prompt` 的等待上限。
///
/// **不能沿用控制面的 30s 默认超时**：一次"含工具调用"的复杂任务要跑数分钟，
/// 30s 判死会让对端迟到的回合结束响应被丢弃（`jsonrpc` 超时后即把 pending 移除，
/// 迟到的响应按 id 找不到接收端 → 静默丢弃），回合结束信号永久丢失，
/// 表现为"任务已完成却仍在等输出"，最终由前端静默看门狗以 180s 误报
/// "长时间无响应：已停止本轮"。回合时长 ≈ 任务时长，故与后端
/// `AGENT_LOOP_TIMEOUT_SECS`(1800) 对齐；控制面见 `HANDSHAKE_TIMEOUT`。
pub const TURN_TIMEOUT: Duration = Duration::from_secs(1800);

/// 控制面（`initialize` / `session/new` / `session/load`）的等待上限。
///
/// **不能沿用 `jsonrpc` 的 30s 默认值**：npx 形态的 ACP 适配器
/// （如 `npx -y @agentclientprotocol/claude-agent-acp`）首次运行要下载整包及其依赖，
/// 本机实测冷启动 139.8s、预热后 14.6s —— 30s 判死会让这类适配器**首次接入必然失败**，
/// 且报错停在 `initialize 超时（30s）`，把对端真正的失败原因（写在 stderr 里）一并掩盖。
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(120);

/// 静默巡检粒度（看门狗每 5s 检查一次对端是否还在回传）。
const IDLE_POLL: Duration = Duration::from_secs(5);

/// "对端已静默 N 秒"的**提示**阈值（只打日志，不终止、不误报）。
///
/// 取 180s：无在途工具时，单次 LLM 请求的**实测**上限远低于此
/// （本轮实测 47.6s；重试退避 2.37s），而任何真实工具执行都有工具在途
/// （见 `IdleState`，此时**不计静默**）。
const IDLE_NOTICE: Duration = Duration::from_secs(180);

/// "对端失活"的**终止**阈值：无在途工具、未等待用户决策，却连续静默 900s
/// → 判对端已失活，主动发 `session/cancel` 并按终止收尾。
///
/// 为什么不沿用 `TURN_TIMEOUT`(1800s)：那是"整轮任务"的上限，用它收尾会让用户
/// 在对端早已失活的情况下多等 15 分钟，且最终只拿到一条"超时"文案。
/// 900s 高于所有可能的合法静默（无工具在途时唯一合法来源就是 LLM 请求，
/// 而 `HERMES_API_TIMEOUT` 兜底 1800s 只在**挂死**时才可能触及）。
const IDLE_ABORT: Duration = Duration::from_secs(900);

/// 看门狗读取的活性状态：判定"这段静默是否**合法**"。
///
/// 两个计数器都来自协议事件本身（而非猜测）：ACP 下工具**开始**与**结果**都是
/// 显式上报的（`tool_call` / `tool_call_update`），只要看到开始而未见结果，
/// 就说明工具仍在执行——此时对端静默是正常的，**绝不能**按静默终止。
#[derive(Default)]
struct IdleState {
    /// 已上报开始、尚未上报结果的工具数（`tool_call` +1 / `tool_call_update` 终态 -1）
    in_flight_tools: AtomicUsize,
    /// 正在等待用户决策的请求数（审批闸门）——等用户天然的"静默"不是对端故障
    awaiting_user: AtomicUsize,
}

impl IdleState {
    /// 当前静默是否**可豁免**（有工具在途或正在等用户决策）。
    fn exempt(&self) -> bool {
        self.in_flight_tools.load(Ordering::Relaxed) > 0
            || self.awaiting_user.load(Ordering::Relaxed) > 0
    }

    fn dec(counter: &AtomicUsize) {
        // 事件可能重复/乱序（对端行为不受我们控制），计数只做**饱和**递减，
        // 绝不回绕成天文数字（否则一次误减会让看门狗永久失效）。
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_sub(1))
        });
    }
}

/// ACP 方法名（客户端 → 对端）。
pub mod method {
    /// 握手：交换协议版本与能力
    pub const INITIALIZE: &str = "initialize";
    /// 新建会话
    pub const SESSION_NEW: &str = "session/new";
    /// 列出既有会话（导入宿主会话用；对端不支持时回 `-32601`）
    pub const SESSION_LIST: &str = "session/list";
    /// 载入既有会话（会话恢复）
    pub const SESSION_LOAD: &str = "session/load";
    /// 恢复既有会话但**不重放**历史（`sessionCapabilities.resume`）
    pub const SESSION_RESUME: &str = "session/resume";
    /// 删除宿主侧会话（`sessionCapabilities.delete`；不支持时回 `-32601`）
    pub const SESSION_DELETE: &str = "session/delete";
    /// 切换会话级模式（`modes.availableModes` 里的 id；模式集由宿主自定义）
    pub const SESSION_SET_MODE: &str = "session/set_mode";
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

/// 已有 `resume_id` 时如何接上对端会话（按 `initialize` 公布的能力位降级）。
///
/// 优先级：`resume`（不重放）→ `load`（重放，需抑制事件）→ `new`（对端不支持恢复）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumePlan {
    Resume,
    Load,
    New,
}

fn resume_plan(has_resume_id: bool, resume_supported: bool, load_supported: bool) -> ResumePlan {
    if !has_resume_id {
        return ResumePlan::New;
    }
    if resume_supported {
        ResumePlan::Resume
    } else if load_supported {
        ResumePlan::Load
    } else {
        ResumePlan::New
    }
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

// ──────────────────────────────────────────────
//  会话模式 / 兜底审批策略
// ──────────────────────────────────────────────

/// 解析会话响应里的 `modes`（`session/new` / `resume` / `load` 都会带回）。
///
/// 模式**取值由宿主自定义**，这里只做形状归一，不做任何语义映射。
pub fn parse_session_modes(resp: &Value) -> (Vec<AcpMode>, Option<String>) {
    let modes = &resp["modes"];
    let available = modes["availableModes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?.to_string();
                    Some(AcpMode {
                        id,
                        name: m["name"].as_str().unwrap_or_default().to_string(),
                        description: m["description"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (available, modes["currentModeId"].as_str().map(str::to_string))
}

/// 权限请求的粗粒度风险：只看 ACP 标准的 `toolCall.kind`。
///
/// 分级与既有工具口径对齐（`execute_command`=High、写文件=Medium、读=Low）；
/// `kind` 缺失或为未知值 → 按中风险（对应「标准模式」文案里的"未知…需确认"）。
pub fn classify_tool_risk(params: &Value) -> RiskLevel {
    match params["toolCall"]["kind"].as_str().unwrap_or("") {
        "read" | "search" | "think" | "fetch" => RiskLevel::Low,
        "edit" | "move" | "switch_mode" => RiskLevel::Medium,
        "delete" | "execute" => RiskLevel::High,
        // `other`、空串、协议未来新增的 kind：一律按中风险
        _ => RiskLevel::Medium,
    }
}

/// 宿主未提供 `modes` 时：按安全模式决定"这条权限请求要不要打断用户"。
///
/// 返回 `true` = 直接放行（不发审批卡）；`false` = 照常问人。
/// 阈值与 API Agent 共用 [`approval_floor`]，不另写一套尺度。
pub fn auto_allow_by_security(security: Option<SecurityMode>, params: &Value) -> bool {
    match approval_floor(security.unwrap_or_default()) {
        // NULL 档（无限制）：一律放行
        None => true,
        // 低于阈值 → 不打扰；达到/超过阈值 → 问人
        Some(floor) => classify_tool_risk(params) < floor,
    }
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

#[cfg(test)]
mod mode_tests {
    use super::*;
    use serde_json::json;

    /// 会话响应里的 modes 形状归一（三种响应同构）；缺失时给空列表 + None。
    #[test]
    fn parses_session_modes() {
        let resp = json!({
            "sessionId": "s1",
            "modes": {
                "currentModeId": "agent",
                "availableModes": [
                    { "id": "read-only", "name": "Read-only", "description": "Ask before edits" },
                    { "id": "agent", "name": "Auto review" }
                ]
            }
        });
        let (modes, current) = parse_session_modes(&resp);
        assert_eq!(current.as_deref(), Some("agent"));
        assert_eq!(modes.len(), 2);
        assert_eq!(modes[0].id, "read-only");
        assert_eq!(modes[1].description, None);

        let (empty, none) = parse_session_modes(&json!({ "sessionId": "s2" }));
        assert!(empty.is_empty());
        assert_eq!(none, None);
    }

    /// `toolCall.kind` → 风险：与既有工具口径对齐；未知/缺失按中风险。
    #[test]
    fn classifies_tool_risk_by_kind() {
        let risk = |kind: &str| classify_tool_risk(&json!({ "toolCall": { "kind": kind } }));
        assert_eq!(risk("read"), RiskLevel::Low);
        assert_eq!(risk("search"), RiskLevel::Low);
        assert_eq!(risk("think"), RiskLevel::Low);
        assert_eq!(risk("fetch"), RiskLevel::Low);
        assert_eq!(risk("edit"), RiskLevel::Medium);
        assert_eq!(risk("move"), RiskLevel::Medium);
        assert_eq!(risk("switch_mode"), RiskLevel::Medium);
        assert_eq!(risk("execute"), RiskLevel::High);
        assert_eq!(risk("delete"), RiskLevel::High);
        assert_eq!(risk("other"), RiskLevel::Medium);
        assert_eq!(risk("brand-new-kind"), RiskLevel::Medium);
        // kind 缺失
        assert_eq!(
            classify_tool_risk(&json!({ "toolCall": {} })),
            RiskLevel::Medium
        );
        assert_eq!(classify_tool_risk(&json!({})), RiskLevel::Medium);
    }

    /// 兜底审批策略：四档 × 三档风险。宿主有模式时这条路径不会被执行。
    #[test]
    fn auto_allow_matrix() {
        let allow = |mode: SecurityMode, kind: &str| {
            auto_allow_by_security(Some(mode), &json!({ "toolCall": { "kind": kind } }))
        };
        // 严格：一律问（Low 也要问）
        assert!(!allow(SecurityMode::Strict, "read"));
        assert!(!allow(SecurityMode::Strict, "edit"));
        assert!(!allow(SecurityMode::Strict, "execute"));
        // 标准：只放行低风险
        assert!(allow(SecurityMode::Standard, "read"));
        assert!(!allow(SecurityMode::Standard, "edit"));
        assert!(!allow(SecurityMode::Standard, "execute"));
        // 宽松：放行低/中，高危仍问
        assert!(allow(SecurityMode::Relaxed, "read"));
        assert!(allow(SecurityMode::Relaxed, "edit"));
        assert!(!allow(SecurityMode::Relaxed, "execute"));
        // 无限制：全放
        assert!(allow(SecurityMode::Unrestricted, "execute"));
        // 未传安全模式 → 标准
        assert!(auto_allow_by_security(
            None,
            &json!({ "toolCall": { "kind": "read" } })
        ));
        assert!(!auto_allow_by_security(
            None,
            &json!({ "toolCall": { "kind": "edit" } })
        ));
    }
}

// ──────────────────────────────────────────────
//  子进程回收
// ──────────────────────────────────────────────

/// 仍在运行的 ACP 子进程 pid。
///
/// 正常收尾（`close()` / 会话对象 Drop）会把自己摘掉；应用退出时没人有机会收尾，
/// 由 [`kill_all_live_sessions`] 按 pid 连整棵进程树一起强杀，避免留下孤儿。
///
/// 为什么按 pid 而不是持有 Child：退出回调是**同步**上下文，拿不到 tokio 的锁，
/// 也无法 await `child.kill()`；`taskkill /T /F` 还能一并带走 npx 派生的 node 子进程。
static LIVE_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn register_pid(pid: u32) {
    if let Ok(mut v) = LIVE_PIDS.lock() {
        v.push(pid);
    }
}

fn unregister_pid(pid: u32) {
    if let Ok(mut v) = LIVE_PIDS.lock() {
        v.retain(|p| *p != pid);
    }
}

/// 强杀一棵进程树（Windows 用 `taskkill /T /F`，其余平台直接 `kill -9`）。
///
/// 供「一次性探测」复用：`npx` 形态下真正干活的是它派生的 node，只 kill 父进程会留残。
pub(crate) fn kill_process_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let _ = crate::utils::process::hidden_command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = crate::utils::process::hidden_command("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

/// 应用退出时强杀所有仍在运行的 ACP 进程树。
///
/// 只处理"退出时还没收尾"的那部分：正常回合结束的进程早已被 `close()` 摘除。
pub fn kill_all_live_sessions() {
    let pids: Vec<u32> = LIVE_PIDS
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default();
    if pids.is_empty() {
        return;
    }
    log::info!(
        "[acp] 应用退出：强杀 {} 个仍在运行的 ACP 进程（含其子进程树）",
        pids.len()
    );
    for pid in pids {
        kill_process_tree(pid);
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
    /// 取消令牌：与调用方**共享同一份**（`connect` 时取 `conn.abort`）。
    /// `prompt` 等待期间轮询它——置位即主动结束等待并发 `session/cancel`，
    /// 让"停止生成"能即时打断 ACP 回合（否则只能干等到回合自然结束或回合超时）。
    abort: Arc<AtomicBool>,
    /// 活跃度看门狗的状态（与 `session/update` / 权限处理器共享）
    idle: Arc<IdleState>,
    /// 子进程 pid（0 = 已收尾）。退出时按它强杀进程树，`close()` / Drop 时摘除。
    child_pid: AtomicU32,
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
            abort: Arc::new(AtomicBool::new(false)),
            idle: Arc::new(IdleState::default()),
            child_pid: AtomicU32::new(0),
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
        // 共享取消令牌：调用方（session_runs）置位后，`prompt` 的轮询即可感知
        self.abort = Arc::clone(&conn.abort);

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
            .stderr(Stdio::piped())
            // Child 句柄被丢弃时兜底终止（与 `Inner::drop` 的 start_kill 双保险）
            .kill_on_drop(true);

        // 适配器要求的环境变量（官方目录 npx 条目可能带，如禁用自更新）：
        // 在 conn 里以 `env` 对象保存，启动时注入。
        if let Some(env) = conn.conn["env"].as_object() {
            for (k, v) in env {
                if let Some(s) = v.as_str() {
                    cmd.env(k, s);
                }
            }
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| AgentError::Connect(format!("启动 ACP 进程失败: {}", e)))?;
        // 登记 pid：应用退出时（没人有机会走 close）由全局回收兜底
        if let Some(pid) = child.id() {
            self.child_pid.store(pid, Ordering::Relaxed);
            register_pid(pid);
        }

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

        // ── 1b) 事件泵：把 sink 从读循环里剥离 ──
        // 读循环**必须永远保持排水**：ACP 的回合结束响应与被丢弃的 session/update 走同一条
        // stdout，一旦读取被下游（Tauri IPC / 前端渲染）的背压卡住，对端写满管道后会连
        // 回合结束响应都发不出来——现场表现正是"内容早已显示、随后长时间静默、最终以超时
        // 收尾"（对端侧会留下一个卡在 `session_update()` 里的 pending 任务）。
        // 改经无界通道交给独立任务转发后，读循环只剩"解析 + 分发 + 唤醒等待者"，
        // 不再被消费者拖住；积压超阈值时打点，供下次复现时确认背压来源。
        let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<UnifiedAgentEvent>();
        let sink_pump = Arc::clone(&sink);
        tokio::spawn(async move {
            while let Some(ev) = evt_rx.recv().await {
                let backlog = evt_rx.len();
                if backlog > 64 {
                    log::warn!(
                        "[acp] 事件泵积压 {} 条：下游消费过慢（读循环不受影响）",
                        backlog
                    );
                }
                sink_pump(ev);
            }
        });

        // 回放抑制：`session/load` 会把整段历史以 `session/update` 重放一遍；
        // 我们自己已保存消息记录，原样转发会让当前回合"重播全部旧消息"。
        let suppress_replay = Arc::new(AtomicBool::new(false));
        let suppress_update = Arc::clone(&suppress_replay);

        // ── 2) 注册"对端 → 客户端"的处理器 ──
        // 2a) session/update：流式正文/思考/工具/计划
        let tx_update = evt_tx.clone();
        let sid_update = self.session_id.clone();
        let text_buf = Arc::clone(&self.text_buf);
        let idle_update = Arc::clone(&self.idle);
        peer.on(
            peer_method::SESSION_UPDATE,
            Arc::new(move |_peer, _id, params| {
                // 回放期间丢弃：上下文由对端自己恢复，历史由本应用落库
                if suppress_update.load(Ordering::Relaxed) {
                    return;
                }
                for ev in project_session_update(&sid_update, &params) {
                    // 在途工具计数：工具执行期间对端静默是**正常**的（见 IdleState）
                    match &ev {
                        UnifiedAgentEvent::ToolStart { .. } => {
                            idle_update.in_flight_tools.fetch_add(1, Ordering::Relaxed);
                        }
                        UnifiedAgentEvent::ToolResult { .. } => {
                            IdleState::dec(&idle_update.in_flight_tools);
                        }
                        _ => {}
                    }
                    if let UnifiedAgentEvent::Chunk { content, .. } = &ev {
                        if let Ok(mut buf) = text_buf.lock() {
                            buf.push_str(content);
                        }
                    }
                    let _ = tx_update.send(ev);
                }
            }),
        );

        // 2b) session/request_permission：权限请求 → 审批闸门
        // 安全默认：闸门缺失、或超时未决 → **拒绝**（绝不默认放行）；
        // 且只在**真有闸门**时才发审批卡——没闸门却发卡，只会让用户点了没反应。
        //
        // 权限语义归属：
        // - 默认：宿主提供 `modes`（会话建立后才知道，故用共享标志）→ 完全交给宿主，
        //   这里保持"有请求就问人"，不做客户端二次裁决；
        // - 默认：宿主不提供 `modes` → 回退到本应用的安全模式（兜底审批策略）；
        // - 例外：`enforce_security_matrix`（圆桌）→ 不看宿主有没有模式，一律按安全模式的
        //   风险矩阵过滤，高于阈值的才交审批闸门（主持人），使一个房间内各类接入方式同一尺度。
        let host_modes_available = Arc::new(AtomicBool::new(false));
        let host_modes_perm = Arc::clone(&host_modes_available);
        let security_perm = conn.security_mode;
        let enforce_matrix = conn.enforce_security_matrix;
        let tx_perm = evt_tx.clone();
        let sid_perm = self.session_id.clone();
        let gate_perm = conn.approvals.clone();
        let idle_perm = Arc::clone(&self.idle);
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

                // 兜底策略：低风险请求直接放行，不打断用户（`enforce_matrix` 时无视宿主模式，
                // 宿主提供了模式也按同一矩阵过滤，口径见 `enforce_security_matrix` 文档）。
                // 固定选 `allow_once`：不做 `allow_always`，避免把"本次放行"持久化进宿主会话。
                if (enforce_matrix || !host_modes_perm.load(Ordering::Relaxed))
                    && auto_allow_by_security(security_perm, &params)
                {
                    match pick_option_id(&options, PermissionDecision::Allow) {
                        Some(option_id) => {
                            log::info!(
                                "[acp] 按会话安全模式自动放行权限请求: kind={} session={}",
                                params["toolCall"]["kind"].as_str().unwrap_or(""),
                                sid_perm
                            );
                            let peer_auto = peer.clone();
                            if let Some(id) = id {
                                tokio::spawn(async move {
                                    peer_auto
                                        .respond(
                                            id,
                                            Ok(json!({
                                                "outcome": { "outcome": "selected", "optionId": option_id }
                                            })),
                                        )
                                        .await;
                                });
                            }
                            return;
                        }
                        // 对端没给 allow 类选项：不能凭空放行，退回人工审批
                        None => log::warn!(
                            "[acp] 自动放行失败（对端未提供 allow 类选项），转人工审批: session={}",
                            sid_perm
                        ),
                    }
                }

                let gate = gate_perm.clone();

                if gate.is_some() {
                    let _ = tx_perm.send(UnifiedAgentEvent::ApprovalRequired {
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
                // 等用户决策期间**豁免**静默判定：用户可能几分钟后才点，这不算对端故障
                let awaiting = gate.is_some();
                let idle_gate = Arc::clone(&idle_perm);
                if awaiting {
                    idle_gate.awaiting_user.fetch_add(1, Ordering::Relaxed);
                }
                // 终态回传通道与审批请求同源（同一会话的事件流）
                let tx_resolved = tx_perm.clone();
                let sid_resolved = sid_perm.clone();
                let resolved_call_id = call_id.clone();
                // 一并把工具信息交出去：工作流审批界面要显示"批准的是哪个工具、什么参数"
                let tool_name = params["toolCall"]["title"]
                    .as_str()
                    .unwrap_or("工具调用")
                    .to_string();
                let tool_args = params["toolCall"]["rawInput"].to_string();
                tokio::spawn(async move {
                    let (allowed, timed_out) = match gate {
                        Some(g) => match tokio::time::timeout(
                            std::time::Duration::from_secs(
                                crate::agent::transport::ACP_APPROVAL_TIMEOUT_SECS,
                            ),
                            g.request_approval_with_details(
                                &call_id,
                                &tool_name,
                                &tool_args,
                            ),
                        )
                        .await
                        {
                            Ok(v) => (v, false),
                            Err(_) => {
                                // 超时：清掉注册表里残留的 sender，并按安全默认拒绝
                                log::warn!(
                                    "[acp] 权限请求超时未决 → 拒绝: call={}",
                                    resolved_call_id
                                );
                                g.discard(&call_id);
                                (false, true)
                            }
                        },
                        None => {
                            log::warn!("[acp] 权限请求：审批闸门未接入 → 拒绝: {}", params);
                            (false, false)
                        }
                    };
                    if awaiting {
                        IdleState::dec(&idle_gate.awaiting_user);
                    }
                    // 必须回传终态：前端审批卡与通知中心的"未决项"靠它翻转。
                    // 只发 required 不发 resolved，卡片会永远停在未决（超时后显示"等待超时裁决…"）。
                    if awaiting {
                        let _ = tx_resolved.send(UnifiedAgentEvent::ApprovalResolved {
                            session_id: sid_resolved,
                            call_id: resolved_call_id,
                            approved: allowed,
                            timed_out,
                        });
                    }
                    if let Some(id) = id {
                        peer.respond(id, Ok(permission_response(&options, allowed)))
                            .await;
                    }
                });
            }),
        );

        // ── 3) initialize 握手 ──
        let init = peer
            .request_with_timeout(
                method::INITIALIZE,
                json!({
                    "protocolVersion": ACP_PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": { "readTextFile": false, "writeTextFile": false }
                    }
                }),
                HANDSHAKE_TIMEOUT,
            )
            .await
            .map_err(|e| with_peer_stderr(e, &stderr_text))?;

        // 对端能力位：决定既有会话走 resume（不重放）还是 load（重放，需抑制事件）。
        // `sessionCapabilities.resume` 在 schema 里是对象（如 `{}`），也兼容布尔写法。
        let agent_caps = &init["agentCapabilities"];
        let resume_supported = agent_caps["sessionCapabilities"]["resume"]
            .as_object()
            .is_some()
            || agent_caps["sessionCapabilities"]["resume"]
                .as_bool()
                .unwrap_or(false);
        let load_supported = agent_caps["loadSession"].as_bool().unwrap_or(false);

        // ── 4) 建/恢复会话 ──
        // 有 resume_id 时优先 `session/resume`（不重放）；对端不支持 resume 才退回
        // `session/load`（会重放历史，期间抑制事件）；两者都不支持只能新建。
        let new_params = json!({ "cwd": conn.cwd, "mcpServers": [] });
        let mut resume_params = new_params.clone();
        if let Some(rid) = &conn.resume_id {
            resume_params["sessionId"] = json!(rid);
        }
        let plan = resume_plan(conn.resume_id.is_some(), resume_supported, load_supported);
        let resp_result = match plan {
            ResumePlan::Resume => {
                let first = peer
                    .request_with_timeout(
                        method::SESSION_RESUME,
                        resume_params.clone(),
                        HANDSHAKE_TIMEOUT,
                    )
                    .await;
                match first {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        // 声明支持 resume 但实际失败（版本/实现差异）：按能力退回，
                        // 不要因为恢复失败让整轮消息直接报错。
                        log::warn!("[acp] session/resume 失败，按能力退回：{}", e);
                        if load_supported {
                            suppress_replay.store(true, Ordering::Relaxed);
                            let r = peer
                                .request_with_timeout(
                                    method::SESSION_LOAD,
                                    resume_params.clone(),
                                    HANDSHAKE_TIMEOUT,
                                )
                                .await;
                            suppress_replay.store(false, Ordering::Relaxed);
                            r
                        } else {
                            // 恢复失败且无 load 兜底：只能新建，本轮不再有历史上下文
                            log::warn!(
                                "[acp] 会话恢复失败且对端不支持 load，已新建会话（无历史上下文）: resume_id={}",
                                conn.resume_id.as_deref().unwrap_or_default()
                            );
                            peer.request_with_timeout(
                                method::SESSION_NEW,
                                new_params.clone(),
                                HANDSHAKE_TIMEOUT,
                            )
                            .await
                        }
                    }
                }
            }
            ResumePlan::Load => {
                suppress_replay.store(true, Ordering::Relaxed);
                let r = peer
                    .request_with_timeout(
                        method::SESSION_LOAD,
                        resume_params.clone(),
                        HANDSHAKE_TIMEOUT,
                    )
                    .await;
                // 回放在 load 响应前已全部送达，这里解除抑制
                suppress_replay.store(false, Ordering::Relaxed);
                r
            }
            ResumePlan::New => {
                // 有 resume_id 却落到新建：对端既没声明 resume 也没声明 load。
                // 不阻断本轮（否则用户没法继续用），但必须留痕，便于排查"上下文丢了"。
                if conn.resume_id.is_some() {
                    log::warn!(
                        "[acp] 对端未声明会话恢复能力，已新建会话（无历史上下文）: agent_session_id={}",
                        conn.resume_id.as_deref().unwrap_or_default()
                    );
                }
                peer.request_with_timeout(
                    method::SESSION_NEW,
                    new_params.clone(),
                    HANDSHAKE_TIMEOUT,
                )
                .await
            }
        };
        let resp = resp_result.map_err(|e| with_peer_stderr(e, &stderr_text))?;

        self.remote_id = resp["sessionId"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| conn.resume_id.clone());

        // 把对端会话 id 回传前端：前端写回 `sessions.agent_session_id`，
        // 下一轮才能 resume/load 同一会话。缺了这一步，每条消息都会 `session/new`
        // 出一个孤立的宿主会话（同一会话列表里"每条消息一个 session"）。
        if let Some(remote) = self.remote_id.clone() {
            self.emit(UnifiedAgentEvent::AgentSessionId {
                session_id: self.session_id.clone(),
                agent_session_id: remote,
            });
        }

        // ── 5) 会话模式（`modes` 由 `new`/`resume`/`load` 三种响应其一带回）──
        // 模式取值由宿主自定义，本应用只原样展示，不做映射；宿主提供模式时，
        // 权限语义完全交给它（见 2b），这里的标志位就是那条分界的开关。
        let (available_modes, current_mode_id) = parse_session_modes(&resp);
        let host_has_modes = !available_modes.is_empty();
        host_modes_available.store(host_has_modes, Ordering::Relaxed);
        // 空列表也要发：前端据此区分"宿主不提供模式（回退安全模式）"与"尚未得知"
        self.emit(UnifiedAgentEvent::SessionModes {
            session_id: self.session_id.clone(),
            available_modes: available_modes.clone(),
            current_mode_id: current_mode_id.clone(),
        });
        log::info!(
            "[acp] 会话模式: session={} current={:?} available={:?}",
            self.session_id,
            current_mode_id,
            available_modes.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()
        );

        // 每轮显式写回用户选择的模式：进程是每轮重建的，且实测 codex-acp 的
        // `set_mode` **不跨进程持久**，不写回就等于没选。
        //
        // 只写用户显式选定的宿主模式：模式命名/语义由宿主自定义，本应用不做映射
        // （口径同会话模式的 `HostModeSelector`——不按关键词猜哪一档是什么权限）。
        if let Some(want) = conn
            .acp_mode
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !host_has_modes {
                log::warn!(
                    "[acp] 宿主未提供会话模式，忽略期望值 {}: session={}",
                    want,
                    self.session_id
                );
            } else if !available_modes.iter().any(|m| m.id == want) {
                // 适配器升级后 id 可能变化：不阻断本轮，前端会按最新列表纠正选择
                log::warn!(
                    "[acp] 期望模式 {} 不在宿主可用列表中（当前 {:?}），本轮按宿主当前模式执行",
                    want,
                    current_mode_id
                );
            } else if let Some(remote) = self.remote_id.clone() {
                match peer
                    .request_with_timeout(
                        method::SESSION_SET_MODE,
                        json!({ "sessionId": remote, "modeId": want }),
                        HANDSHAKE_TIMEOUT,
                    )
                    .await
                {
                    Ok(_) => log::info!("[acp] 已切换到宿主模式 {}: session={}", want, self.session_id),
                    // 切换失败不阻断本轮：用户至少还能继续对话，日志留痕便于排查
                    Err(e) => log::warn!("[acp] session/set_mode 失败（继续本轮）: {}", e),
                }
            }
        }

        // ACP 天生具备结构化能力（工具事件/思考/常驻会话/会话恢复）；
        // resume 按对端实际公布的能力位标记（resume 或 load 任一可用即算可恢复）。
        // permission 暂标 false：审批尚未接入（见 2b 的 TODO），不能声称支持。
        self.caps = Capabilities {
            tool_events: true,
            permission: false,
            reasoning: true,
            resume: resume_supported || load_supported,
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
        // "停止生成"接线：把共享取消令牌与回合请求一起竞速。
        // 否则该等待最长会挂到 TURN_TIMEOUT(1800s)，用户点了停止也没有任何反应。
        let abort = Arc::clone(&self.abort);
        let sid_cancel = sid.clone();

        // 后端**权威**活性看门狗：判定"对端是否还在回传"的依据只有一条——
        // 对端最近一次真正写入 stdout 的时刻（`JsonRpcPeer::last_activity`）。
        // 前端此前自造的 900s 兜底只是猜测（且它只关前端视图、从未把取消送达对端），
        // 这里改为在**协议层**判定并真取消，前端只消费后端给的终态。
        let turn_start = Instant::now();
        let idle_watch = {
            let peer_watch = JsonRpcPeer::clone(peer);
            let idle_state = Arc::clone(&self.idle);
            let sid_watch = sid.clone();
            async move {
                let mut noticed = false;
                loop {
                    tokio::time::sleep(IDLE_POLL).await;
                    // 有工具在途 / 正在等用户决策 → 这段静默是合法的，不计时
                    if idle_state.exempt() {
                        noticed = false;
                        continue;
                    }
                    // 基线取「对端最近回传」与「本轮请求发出」两者中较晚者：
                    // 本轮请求发出后尚未收到任何回包时，静默应从发出时刻算起。
                    let idle_secs = peer_watch
                        .last_activity()
                        .max(turn_start)
                        .elapsed()
                        .as_secs();
                    if !noticed && idle_secs >= IDLE_NOTICE.as_secs() {
                        noticed = true;
                        log::warn!(
                            "[acp] 对端已静默 {}s（无在途工具、未等用户决策）：session={}",
                            idle_secs,
                            sid_watch
                        );
                    }
                    if idle_secs >= IDLE_ABORT.as_secs() {
                        return idle_secs;
                    }
                }
            }
        };

        let outcome = tokio::select! {
            r = peer.request_with_timeout(
                method::SESSION_PROMPT,
                json!({ "sessionId": sid, "prompt": blocks }),
                TURN_TIMEOUT,
            ) => r,
            _ = async {
                // 200ms 轮询：与既有 CLI/API 路径的协作式取消粒度一致
                while !abort.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            } => {
                // 先按协议取消（让对端终止其内部工具），再按"已取消"返回；
                // run_transport_session 的取消分支随后会 close 对端并上报 agent-cancelled。
                log::info!("[acp] 收到取消信号，发送 session/cancel：session={}", sid_cancel);
                if let Err(e) = peer
                    .notify(method::SESSION_CANCEL, json!({ "sessionId": sid_cancel }))
                    .await
                {
                    log::warn!("[acp] session/cancel 未送达对端：err={}", e);
                }
                return Err(AgentError::Cancelled);
            }
            idle_secs = idle_watch => {
                // 对端已失活：先按协议取消（尽力让对端收尾），再以**明确的终态**返回，
                // 由调用方上报 `agent-idle-aborted`（而不是伪装成超时/普通错误）。
                log::warn!(
                    "[acp] 对端失活：静默 {}s 超过阈值 {}s，发送 session/cancel 并终止本轮：session={}",
                    idle_secs,
                    IDLE_ABORT.as_secs(),
                    sid_cancel
                );
                if let Err(e) = peer
                    .notify(method::SESSION_CANCEL, json!({ "sessionId": sid_cancel }))
                    .await
                {
                    log::warn!("[acp] session/cancel 未送达对端：err={}", e);
                }
                return Err(AgentError::IdleAborted { idle_secs });
            }
        };
        let resp = outcome.map_err(|e| {
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
        // 先按 pid 强杀整棵进程树（npx 形态下真正干活的是 node 子进程，
        // 只 kill 父进程会把它留下），再走句柄 kill 做兜底与回收。
        let pid = self.child_pid.swap(0, Ordering::Relaxed);
        if pid != 0 {
            kill_process_tree(pid);
        }
        if let Some(peer) = &self.peer {
            peer.kill().await;
        }
        if pid != 0 {
            unregister_pid(pid);
        }
        Ok(())
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        // 兜底：本轮如果没走 close（panic / future 被 cancel），这里收尾。
        // 顺序很关键——先丢弃 peer 触发 `Inner::drop` 终止子进程，再摘除 pid 登记，
        // 避免"进程还在、登记已摘"让退出回收漏掉它。
        drop(self.peer.take());
        let pid = self.child_pid.swap(0, Ordering::Relaxed);
        if pid != 0 {
            unregister_pid(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resume_plan_falls_back_by_capability() {
        // 首次会话（无 resume_id）一律新建
        assert_eq!(resume_plan(false, true, true), ResumePlan::New);
        // 已有 id：resume 优先（不重放）
        assert_eq!(resume_plan(true, true, true), ResumePlan::Resume);
        // 不支持 resume → 退回 load（重放，调用方会抑制事件）
        assert_eq!(resume_plan(true, false, true), ResumePlan::Load);
        // 两者都不支持 → 只能新建（新 id 会回传前端，后续仍可继续）
        assert_eq!(resume_plan(true, false, false), ResumePlan::New);
    }

    #[test]
    fn pid_registry_tracks_and_releases() {
        register_pid(4_242_424);
        assert!(LIVE_PIDS
            .lock()
            .map(|v| v.contains(&4_242_424))
            .unwrap_or(false));
        unregister_pid(4_242_424);
        assert!(!LIVE_PIDS
            .lock()
            .map(|v| v.contains(&4_242_424))
            .unwrap_or(true));
    }

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
