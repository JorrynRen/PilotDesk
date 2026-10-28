//! PilotDesk 适配层：`PilotDeskCliRunner`（实现框架层 `CliRunner`）。
//!
//! 复用 `AgentManager.execute_command`（长时异步执行入口），`command` 字段即 agent_type（claude/hermes/codex）。
//!
//! 接入方式与「会话 / 工作流」同口径：`agents.transport` 非 `subprocess` 时改走
//! 传输抽象层（ACP = 适配器进程 + JSON-RPC）。否则 ACP 配置没有 `run_cmd_template`，
//! 子进程路径会退化成「运行时命令 + 提示词」（npm 型适配器即 `npm <提示词>` →
//! `Unknown command`），参与者必然失败。

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
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

/// 圆桌参与者的审批闸门：本地 CLI 参与者本就由自身启动参数决定权限（预置默认跳过审批），
/// ACP 参与者沿用同一口径——不放行则宿主每轮工具调用都会被拒，参与者直接干不了活。
///
/// 逐条审批（主持人裁决 / 前端审批卡）尚未接进圆桌，这里先自动放行并留日志，
/// 让"ACP 参与者能否跑起来"与"审批策略如何统一"分开处理。
struct ParticipantAllowAllGate;

#[async_trait::async_trait]
impl crate::agent::transport::ApprovalGate for ParticipantAllowAllGate {
    async fn request_approval(&self, call_id: &str) -> bool {
        log::warn!(
            "[groupchat/cli] ACP 权限请求自动放行（圆桌未接入逐条审批）: call={}",
            call_id
        );
        true
    }
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

    /// 非 `subprocess` 接入（ACP）的一轮执行：适配器进程 + JSON-RPC，返回本轮正文。
    async fn run_transport(
        &self,
        descriptor: &crate::agent::descriptor::AgentDescriptor,
        prompt: &str,
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<CliOutput, String> {
        use crate::agent::transport::{
            build_session, AgentConn, AgentError, PromptInput, UnifiedAgentEvent,
        };

        let Some(mut session) = build_session(descriptor.transport, None) else {
            return Err(format!(
                "接入方式 {:?} 暂不支持会话执行（mcp 为能力面、webview 为承载）",
                descriptor.transport
            ));
        };

        let resume_session_id = self.sessions.lock().unwrap().get(&descriptor.agent_id).cloned();

        // 正文与对端会话 id 都从统一事件里收：`prompt()` 的回包不保证带 sessionId
        let collected = Arc::new(Mutex::new(String::new()));
        let remote_sid: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let collected_sink = collected.clone();
        let sid_sink = remote_sid.clone();
        let sink: crate::agent::transport::EventSink = Arc::new(move |ev| match ev {
            UnifiedAgentEvent::Chunk { content, .. } => {
                if let Some(cb) = &on_delta {
                    cb(content.as_str());
                }
                if let Ok(mut buf) = collected_sink.lock() {
                    buf.push_str(&content);
                }
            }
            UnifiedAgentEvent::AgentSessionId {
                agent_session_id, ..
            } => {
                if let Ok(mut s) = sid_sink.lock() {
                    *s = Some(agent_session_id);
                }
            }
            _ => {}
        });

        // 圆桌未把房间级取消标志接进 CLI 参与者，这里用独立令牌（口径与既有实现一致）
        let abort = Arc::new(AtomicBool::new(false));
        let conn = AgentConn {
            session_id: format!("groupchat:{}", descriptor.agent_id),
            agent_id: descriptor.agent_id.clone(),
            transport: descriptor.transport,
            cwd: self.cwd.clone(),
            conn: descriptor.conn.clone(),
            secret: None,
            resume_id: resume_session_id.clone(),
            abort,
            on_pid: None,
            approvals: Some(Arc::new(ParticipantAllowAllGate)
                as Arc<dyn crate::agent::transport::ApprovalGate>),
            acp_mode: None,
            security_mode: None,
        };
        let handle = session
            .connect(&conn, sink)
            .await
            .map_err(|e| format!("建立会话失败: {}", e))?;
        let result = session
            .prompt(
                &handle,
                PromptInput {
                    text: prompt.to_string(),
                    images: Vec::new(),
                },
            )
            .await;
        // 每轮新建进程，不关会留下孤儿适配器继续推事件
        let _ = session.close(&handle).await;

        match result {
            Ok(out) => {
                if let Some(sid) = remote_sid.lock().unwrap().clone() {
                    self.sessions
                        .lock()
                        .unwrap()
                        .insert(descriptor.agent_id.clone(), sid);
                }
                let text = out.text.trim().to_string();
                let text = if text.is_empty() {
                    collected
                        .lock()
                        .map(|buf| buf.clone())
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                } else {
                    text
                };
                Ok(CliOutput {
                    stdout: text,
                    session_id: resume_session_id,
                })
            }
            Err(AgentError::Cancelled) => Err("已取消".to_string()),
            Err(AgentError::ProcessExited { exit_code, stderr }) => Err(
                crate::agent::friendly_agent_error(&descriptor.agent_id, exit_code, &stderr),
            ),
            Err(e) => Err(format!("{}", e)),
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

        // ── 接入方式分流：ACP 不能走 CLI 子进程（与会话 / 工作流同口径）──
        let descriptor =
            crate::agent::descriptor::AgentDescriptor::from_agents_row(&conn, agent_type)
                .map_err(|e| format!("解析接入方式失败: {}", e))?;
        drop(conn);
        if let Some(desc) = descriptor
            .as_ref()
            .filter(|d| d.transport != crate::agent::transport::TransportKind::Subprocess)
        {
            return self.run_transport(desc, prompt, on_delta).await;
        }

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
