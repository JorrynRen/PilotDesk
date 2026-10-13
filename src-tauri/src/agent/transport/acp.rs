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

use serde_json::Value;

use crate::agent::transport::{StopReason, UnifiedAgentEvent};

/// 本实现使用的 ACP 协议版本（客户端在 `initialize` 中声明）。
pub const ACP_PROTOCOL_VERSION: u32 = 1;

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
