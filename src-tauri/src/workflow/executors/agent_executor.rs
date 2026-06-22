use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use crate::agent::AgentManager;
use crate::utils::errors::AppError;
use tauri::Emitter;
use tokio::sync::Mutex as AsyncMutex;
use crate::workflow::registry::{NodeDef, NodeOutput, NodeExecutorTrait};

pub struct AgentExecutor {
    agent_manager: Arc<AsyncMutex<AgentManager>>,
}

impl AgentExecutor {
    pub fn new(agent_manager: Arc<AsyncMutex<AgentManager>>) -> Self {
        Self { agent_manager }
    }
}

#[async_trait]
impl NodeExecutorTrait for AgentExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let agent_type = node.config.get("agent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("claude");
        let prompt = node.config.get("prompt_template")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 将上游输入注入到 prompt 上下文中
        let context_prompt = if !resolved_input.is_null() {
            format!("{}

上游输入:
{}", prompt, serde_json::to_string_pretty(&resolved_input).unwrap_or_default())
        } else {
            prompt.to_string()
        };

        let temp_session_id = format!("wf_{}_{}", execution_id, node.id);
        let exec_id = execution_id.to_string();
        let node_id = node.id.clone();
        let emitter_owned = emitter.clone();

        let (output, agent_session_id) = self.agent_manager.lock().await.execute_once(
            agent_type,
            &context_prompt,
            &Default::default(),
            "",
            &temp_session_id,
            move |chunk| {
                let _ = emitter_owned.emit("workflow:chunk", serde_json::json!({
                    "execution_id": exec_id,
                    "node_execution_id": node_id,
                    "content": chunk,
                }));
            },
        ).await?;

        Ok(NodeOutput {
            output: serde_json::json!({
                "text": output,
                "agent_session_id": agent_session_id,
            }),
        })
    }
}
