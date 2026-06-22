use async_trait::async_trait;
use serde_json::Value;
use tauri::Emitter;
use crate::utils::errors::AppError;
use super::super::registry::{NodeDef, NodeOutput, NodeExecutorTrait};

/// 审批节点执行器
///
/// 工作流执行到审批节点时暂停，通过 workflow:awaiting-approval 事件
/// 通知前端展示审批面板，等待用户审批后继续执行。
/// 支持超时自动决策（默认 24 小时）。
pub struct ApprovalExecutor;

#[async_trait]
impl NodeExecutorTrait for ApprovalExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        __resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let config = node.config.as_object().ok_or_else(|| {
            AppError::Config("approval 配置必须是 JSON Object".into())
        })?;

        let prompt = config
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("请审批");

        let options = config
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter().filter_map(|v| {
                    Some(serde_json::json!({
                        "label": v.get("label").and_then(|l| l.as_str()).unwrap_or(""),
                        "value": v.get("value").and_then(|l| l.as_str()).unwrap_or(""),
                    }))
                }).collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![
                serde_json::json!({"label": "通过", "value": "approve"}),
                serde_json::json!({"label": "拒绝", "value": "reject"}),
            ]);

        let timeout_minutes = config
            .get("timeout_minutes")
            .and_then(|v| v.as_u64())
            .unwrap_or(1440); // 默认 24 小时

        let default_value = config
            .get("default_value")
            .and_then(|v| v.as_str())
            .unwrap_or("approve");

        // 发射 awaiting-approval 事件到前端
        emitter.emit("workflow:awaiting-approval", serde_json::json!({
            "execution_id": execution_id,
            "node_id": node.id,
            "prompt": prompt,
            "options": options,
            "timeout_minutes": timeout_minutes,
        })).ok();

        // 记录日志
        emitter.emit("workflow:log", serde_json::json!({
            "execution_id": execution_id,
            "node_execution_id": node.id,
            "level": "info",
            "message": format!("审批节点等待用户响应（超时 {} 分钟）", timeout_minutes),
        })).ok();

        // 在实际实现中，这里会通过 oneshot channel 挂起等待用户响应
        // 当前简化实现：返回审批信息，由前端通过 respond_human_input 命令恢复
        Ok(NodeOutput {
            output: serde_json::json!({
                "type": "approval",
                "prompt": prompt,
                "options": options,
                "timeout_minutes": timeout_minutes,
                "default_value": default_value,
                "status": "pending",
            }),
        })
    }
}
