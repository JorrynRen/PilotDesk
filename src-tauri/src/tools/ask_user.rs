//! ask_user 工具：模型向用户发起确认请求（工具级基础能力，注册进 ToolRegistry 后谁用都行）。
//!
//! 执行模型：emit 确认请求事件给前端 → 阻塞等待用户回复（60s 超时）→ 回复作为 tool_result
//! 回流 messages，同一 AgentLoop run 内下一迭代即可被模型读取。与审批通道（PendingApprovals）
//! 同构：oneshot 唤醒 + 超时交还模型。不需要落库 / 现场恢复（会话历史由工具调用链自然保存）。
//!
//! 自 `api_agent/ask_user.rs` 迁入（工具架构统一 v1.0，轮 6）。

use tauri::Emitter;

use crate::groupchat::models::{ConfirmationItem, summarize_confirmation_title};
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use crate::PendingApprovals;

/// 等用户回复的最长时间（会话模式快节奏，超时后由模型自行决定继续或收尾）。
const CONFIRM_TIMEOUT_SECS: u64 = 60;

pub struct AskUserTool {
    app: tauri::AppHandle,
    session_id: String,
    pending: PendingApprovals,
}

impl AskUserTool {
    pub fn new(app: tauri::AppHandle, session_id: String, pending: PendingApprovals) -> Self {
        Self { app, session_id, pending }
    }
}

#[async_trait::async_trait]
impl ToolHandler for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "向用户发起确认请求并等待其回复。当任务推进需要用户拍板、补充关键信息或选择方案时调用：\
         返回用户的回复内容。replyMode=open 用于开放式提问；replyMode=structured 用于结构化确认项\
         （confirm/select/text，用户可逐项填写或选择）。注意：调用后任务会暂停直至用户回复或超时（60 秒）。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "确认事项摘要标题（不超过 22 字，展示给用户）"
                },
                "prompt": {
                    "type": "string",
                    "description": "需要用户确认的问题或说明"
                },
                "replyMode": {
                    "type": "string",
                    "enum": ["open", "structured"],
                    "description": "open=开放式回复；structured=结构化确认项列表"
                },
                "items": {
                    "type": "array",
                    "description": "结构化确认项（replyMode=structured 时必填）",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "确认项唯一标识" },
                            "label": { "type": "string", "description": "确认项问题" },
                            "inputType": {
                                "type": "string",
                                "enum": ["text", "select", "confirm"],
                                "description": "text=文本填写；select=单选（需 options）；confirm=是/否"
                            },
                            "options": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "select 类型的选项列表"
                            },
                            "required": { "type": "boolean", "description": "是否必填" },
                            "placeholder": { "type": "string", "description": "text 类型的输入提示" }
                        },
                        "required": ["label"]
                    }
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<String, String> {
        let prompt = args["prompt"].as_str().unwrap_or("").trim().to_string();
        if prompt.is_empty() {
            return Err("ask_user 缺少 prompt 参数".to_string());
        }
        let title = args["title"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| summarize_confirmation_title(&s))
            .unwrap_or_else(|| summarize_confirmation_title(&prompt));
        let reply_mode = match args["replyMode"].as_str() {
            Some("structured") => "structured".to_string(),
            _ => "open".to_string(),
        };
        // 复用群聊的确认项解析（camelCase 字段），保证前端控件数据一致。
        let items: Vec<ConfirmationItem> = if reply_mode == "structured" {
            args["items"]
                .as_array()
                .map(|arr| arr.iter().map(|it| ConfirmationItem::from_json(it, "inputType")).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let call_id = uuid::Uuid::new_v4().to_string();

        // 1. 通知前端渲染确认块
        let _ = self.app.emit("agent-confirmation-request", serde_json::json!({
            "sessionId": self.session_id,
            "callId": call_id,
            "title": title,
            "prompt": prompt,
            "replyMode": reply_mode,
            "items": items,
        }));

        // 2. 阻塞等待用户回复（60s 超时后交还模型，由模型决定继续或收尾）
        let rx = self.pending.register_confirmation(call_id.clone());
        match tokio::time::timeout(std::time::Duration::from_secs(CONFIRM_TIMEOUT_SECS), rx).await {
            Ok(Ok(reply)) => {
                log::info!("[AskUser] 用户已回复: call_id={}, len={}", call_id, reply.len());
                Ok(reply)
            }
            Ok(Err(_)) => Err("确认通道已关闭".to_string()),
            Err(_) => {
                log::warn!("[AskUser] 确认等待超时（{}s）: call_id={}", CONFIRM_TIMEOUT_SECS, call_id);
                Ok(format!("用户未在 {} 秒内回复，确认请求已超时，请根据已有信息自行决定如何继续或收尾。", CONFIRM_TIMEOUT_SECS))
            }
        }
    }

    fn risk_level(&self) -> RiskLevel {
        // 低风险：不触发审批弹窗（确认本身即是用户交互）。
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Interaction, ToolTag::Interactive]
    }
}
