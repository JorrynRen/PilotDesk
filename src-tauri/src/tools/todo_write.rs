//! 任务列表追踪工具（会话模式专属）。
//!
//! todo_write 事件化（会话持久化事实源）：不再用工具实例内存 `Arc<Mutex<Vec>>` 存整表
//! （每次发消息都重建 registry/工具会导致列表每轮清零），改把每次执行的整表快照写为一条
//! `todo/state` 会话事件（model_visible=false，追加事件类型不属结构变更）。跨轮历史重建
//! 不含工具调用 args，模型可见的任务列表由 lib.rs 消息组装处读取 `latest_session_todos`
//! 投影并注入（见 run_api_agent）。仅会话模式注册（群聊 GROUPCHAT_DISABLE 含 "todo_write"）。

use crate::db::init::DbPool;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 任务列表追踪工具（持会话归属 + 连接池，执行期每次向事件日志写整表快照）。
pub struct TodoWriteTool {
    session_id: String,
    pool: DbPool,
}

impl TodoWriteTool {
    /// `session_id`：事件归属会话；`pool`：写 `todo/state` 事件时取连接用。
    pub fn new(session_id: String, pool: DbPool) -> Self {
        Self { session_id, pool }
    }
}

#[async_trait]
impl ToolHandler for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn description(&self) -> &str {
        "创建并维护当前会话的任务列表，用于多步骤复杂任务的进度追踪。每次调用传入完整 todos 数组会覆盖旧列表。每项含 id、content（任务描述）、status（pending/in_progress/completed）、priority（high/medium/low）。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "完整任务列表（覆盖式更新）",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "description": "任务唯一标识"},
                            "content": {"type": "string", "description": "任务描述"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                            "priority": {"type": "string", "enum": ["high", "medium", "low"]}
                        },
                        "required": ["id", "content", "status", "priority"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Write]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let todos = arguments["todos"].as_array().cloned().unwrap_or_default();

        // 先排版返回文案（与注入格式一致：completed=[x] / in_progress=[>] / 其它=[ ]）
        let mut lines: Vec<String> = Vec::new();
        for t in &todos {
            let content = t["content"].as_str().unwrap_or("");
            let status = t["status"].as_str().unwrap_or("pending");
            let priority = t["priority"].as_str().unwrap_or("medium");
            let mark = match status {
                "completed" => "[x]",
                "in_progress" => "[>]",
                _ => "[ ]",
            };
            lines.push(format!("{} {} ({})", mark, content, priority));
        }
        let body = if lines.is_empty() {
            "(空)".to_string()
        } else {
            lines.join("\n")
        };
        let count = todos.len();

        // 整表快照（last-write-wins）落为一条 todo/state 事件，非模型可见：
        // toolCallId 溯源本次 assistant 工具调用（uuid），ts 为写入时的 unix 秒。
        let payload = serde_json::json!({
            "todos": todos,
            "toolCallId": uuid::Uuid::new_v4().to_string(),
            "ts": crate::utils::now(),
        });
        let conn = self
            .pool
            .get()
            .map_err(|e| format!("获取数据库连接失败: {}", e))?;
        crate::eventlog::append_session_event(&conn, &self.session_id, "todo/state", &payload, false)
            .map_err(|e| format!("保存任务列表失败: {}", e))?;

        Ok(format!("任务列表已更新（共 {} 项）：\n{}", count, body))
    }
}
