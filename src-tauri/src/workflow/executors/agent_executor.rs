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

        // 读取工作区目录（统一使用 resolve_workspace_path）
        let workspace_dir: String = {
            let conn = self.pool.get().map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
            crate::utils::paths::resolve_workspace_path(None, "", &conn)
                .to_string_lossy().to_string()
        };

        // 读取会话延续参数
        let session_mode = node.config.get("session_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("new");

        // 解析延续会话 session_id：
        // 优先使用已由 engine.rs 通过 TemplateEngine 解析的 resume_session_id
        // 回退到 resume_session_ref 模板解析（兼容直接调用 executor 的路径）
        let mut resolved_session_id = node.config.get("resume_session_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        if resolved_session_id.is_none() {
            if let Some(ref_tmpl) = node.config.get("resume_session_ref").and_then(|v| v.as_str()) {
                if !ref_tmpl.is_empty() {
                    if let Value::Object(map) = &resolved_input {
                        let ctx: std::collections::HashMap<String, serde_json::Value> = map.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        if let Ok(sid) = TemplateEngine::resolve(ref_tmpl, &ctx) {
                            if !sid.is_empty() {
                                resolved_session_id = Some(sid);
                                log::info!("[AgentExecutor] Resolved resume_session_ref '{}' -> '{}'", ref_tmpl, resolved_session_id.as_deref().unwrap());
                            }
                        }
                    }
                }
            }
        }

        let resume_session_id = resolved_session_id.as_deref();

        let agent_session_id_param = if session_mode == "resume" {
            resume_session_id
        } else {
            None
        };

        let (output, agent_session_id) = self.agent_manager.lock().await.execute_once(
            &agent_config,
            &prompt,
            &Default::default(),
            &workspace_dir,
            &temp_session_id,
            move |chunk| {
                let _ = emitter_owned.emit("workflow:chunk", serde_json::json!({
                    "execution_id": exec_id,
                    "node_execution_id": node_id,
                    "content": chunk,
                }));
            },
            agent_session_id_param,
        ).await?;

        // output 只存 agent 原始返回文本，session_id 放 NodeOutput.session_id 字段
        // input_data 存最终提示词（覆盖 running 时写入的输入映射）
        // artifacts_path 存工作区路径（agent 运行时生成工件的位置）
        Ok(NodeOutput {
            output: Value::String(output),
            session_id: agent_session_id,
            input_data: Some(prompt.clone()),
            artifacts_path: Some(workspace_dir),
        })
    }
}
