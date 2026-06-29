use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use crate::agent::AgentManager;
use crate::utils::errors::AppError;
use tauri::Emitter;
use tokio::sync::Mutex as AsyncMutex;
use crate::workflow::registry::{NodeDef, NodeOutput, NodeExecutorTrait};
use crate::db::init::DbPool;
use crate::commands::agents::get_agent_inner;
use crate::workflow::template::TemplateEngine;

pub struct AgentExecutor {
    agent_manager: Arc<AsyncMutex<AgentManager>>,
    pool: DbPool,
}

impl AgentExecutor {
    pub fn new(agent_manager: Arc<AsyncMutex<AgentManager>>, pool: DbPool) -> Self {
        Self { agent_manager, pool }
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
        let prompt_template = node.config.get("prompt_template")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 将 resolved_input 转为 HashMap 作为模板上下文
        // resolved_input 已经是 engine.rs 中 resolve_node_input 解析后的确定值
        let prompt = if let Value::Object(map) = &resolved_input {
            let ctx: std::collections::HashMap<String, serde_json::Value> = map.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            TemplateEngine::resolve(prompt_template, &ctx).unwrap_or_else(|_| prompt_template.to_string())
        } else {
            prompt_template.to_string()
        };

        // 换行转义：CLI 参数中的实际换行会导致 cmd.exe 截断，转义为 \n 字面量
        let context_prompt = prompt.replace('\n', "\\n");

        let temp_session_id = format!("wf_{}_{}", execution_id, node.id);
        let exec_id = execution_id.to_string();
        let node_id = node.id.clone();
        let emitter_owned = emitter.clone();

        // 从 DB 查询 Agent 配置（复用 agent 会话已实现的方法）
        let agent_config = {
            let conn = self.pool.get().map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
            get_agent_inner(&conn, agent_type)
                .map_err(|e| AppError::Db(e.to_string()))?
                .unwrap_or_else(|| {
                    crate::commands::agents::get_agent_config_by_type(agent_type).unwrap_or_default()
                })
        };

        let (output, _agent_session_id) = self.agent_manager.lock().await.execute_once(
            &agent_config,
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
            }),
        })
    }
}
