//! 任务列表追踪工具（会话模式专属）。
//!
//! 迁移自 `lib.rs`（工具架构统一 v1.0，轮 5）：闭包持有的 `Mutex<Vec>` 状态
//! 收敛为 struct 字段。仅会话模式注册（群聊参与者不追踪会话级任务）。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// 任务列表追踪工具
pub struct TodoWriteTool {
    state: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Default for TodoWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoWriteTool {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(Vec::<serde_json::Value>::new())),
        }
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
        let mut guard = self.state.lock().unwrap();
        *guard = todos;

        let mut lines: Vec<String> = Vec::new();
        for t in guard.iter() {
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
        Ok(format!("任务列表已更新（共 {} 项）：\n{}", guard.len(), body))
    }
}
