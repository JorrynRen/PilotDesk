//! PilotDesk 适配层：`PilotDeskCliRunner`（实现框架层 `CliRunner`）。
//!
//! 复用 `AgentManager.execute_command`（长时异步执行入口），`command` 字段即 agent_type（claude/hermes/codex）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

use crate::agent::{AgentManager, AsyncCallbacks, AsyncOptions};
use crate::db::init::DbPool;
use crate::utils::process::TimeoutPolicy;

use super::super::participant::{CliConfig, CliOutput, CliRunner, DeltaFn};

pub struct PilotDeskCliRunner {
    agent_manager: Arc<AsyncMutex<AgentManager>>,
    pool: DbPool,
    cwd: String,
    /// agent_type -> agent_session_id（跨轮 `--resume` 持续会话）。
    sessions: Mutex<HashMap<String, String>>,
}

impl PilotDeskCliRunner {
    pub fn new(agent_manager: Arc<AsyncMutex<AgentManager>>, pool: DbPool, cwd: String) -> Self {
        Self {
            agent_manager,
            pool,
            cwd,
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl CliRunner for PilotDeskCliRunner {
    async fn run(
        &self,
        config: &CliConfig,
        prompt: &str,
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<CliOutput, String> {
        let agent_type = config.command.as_str();

        let conn = self
            .pool
            .get()
            .map_err(|e| format!("数据库连接失败: {}", e))?;
        let agent_config = crate::commands::agents::get_agent_inner(&conn, agent_type)
            .map_err(|e| format!("查询 Agent 配置失败: {}", e))?
            .ok_or_else(|| format!("未知 Agent 类型: {}", agent_type))?;

        // 跨轮会话续接
        let resume_session_id = self.sessions.lock().unwrap().get(agent_type).cloned();

        // 新会话 id 提取（用于跨轮 --resume）
        let new_sid: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let new_sid_clone = new_sid.clone();

        // 真流式：on_chunk 直接转发给 on_delta，不再收集后回放。
        let on_delta_clone = on_delta.clone();

        let session_key = agent_type.to_string();
        let async_opts = AsyncOptions {
            cwd: self.cwd.clone(),
            session_id: session_key.clone(),
            source_type: agent_type.to_string(),
            timeout: TimeoutPolicy::llm_inference(),
        };
        let callbacks = AsyncCallbacks {
            on_chunk: Box::new(move |chunk: String| {
                if let Some(cb) = &on_delta_clone {
                    cb(chunk.as_str());
                }
            }),
            on_session_id: Arc::new(move |sid: String| {
                if let Ok(mut s) = new_sid_clone.lock() {
                    *s = Some(sid);
                }
            }),
            // 群聊 abort 走 RoomRuntime 控制标志，未接入 stop_generation；
            // execute_async 内部已自行注册进程，abort/pid 由框架层管理。
            abort_check: Box::new(|| false),
            on_pid: Box::new(|_pid| {}),
        };

        let mgr = self.agent_manager.lock().await;
        let result = mgr
            .execute_command(
                Some(async_opts),
                Some(callbacks),
                Some(&agent_config),
                Some(prompt),
                resume_session_id.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())?;

        // 更新跨轮会话 id
        if let Some(sid) = new_sid.lock().unwrap().clone() {
            self.sessions
                .lock()
                .unwrap()
                .insert(agent_type.to_string(), sid);
        }

        if result.exit_code != 0 {
            let combined = if result.stderr.is_empty() && !result.stdout.is_empty() {
                result.stdout.clone()
            } else {
                result.stderr.clone()
            };
            return Err(format!(
                "{} 进程异常退出 (exit code {}): {}",
                agent_type,
                result.exit_code,
                combined.trim()
            ));
        }

        Ok(CliOutput {
            stdout: result.stdout,
            session_id: resume_session_id,
        })
    }
}
