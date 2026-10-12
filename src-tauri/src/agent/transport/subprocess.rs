//! 本地 CLI 传输（subprocess）：包装现有 `StdioHandler` + `async_process::TokioConsole`。
//!
//! 与改造前的 `AgentManager::execute_async` **语义对齐**（P0 阶段零行为变化）：
//! 命令构建（元信息模板 + `--resume`）、输出解析（`output_parser`）、
//! session_id 提取（`session_id_source`）、双通道 IO 循环、智能超时轮询、取消。
//!
//! 唯一差别是**出口**：本实现把结果投影为 `UnifiedAgentEvent`
//! （P0 阶段只产出原有 4 类：`Chunk` / `AgentSessionId` / `Done` / `Error`）。
//!
//! 取消与进程登记沿用调用方注入的 `AgentConn.abort` / `AgentConn.on_pid`，
//! 因此"按 pid 终止进程树"与"共享取消标志"两种既有机制完全保留。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::agent::async_process::{AsyncConsole, TokioConsole};
use crate::agent::config::AgentConfig;
use crate::agent::handler::{ProcessHandler, StdioHandler, StreamEvent};
use crate::agent::transport::{
    AgentConn, AgentError, AgentSession, Capabilities, EventSink, PidCallback, PromptInput,
    PromptOutput, SessionHandle, StopReason, TransportKind, UnifiedAgentEvent,
};
use crate::utils::process::{
    check_process_state, make_still_alive_error, TimeoutError, TimeoutPolicy,
};

/// 本地 CLI 会话（每轮一个子进程，续会话靠 CLI 自身 `--resume`）。
pub struct SubprocessSession {
    config: AgentConfig,
    timeout: TimeoutPolicy,
    session_id: String,
    cwd: String,
    sink: Option<EventSink>,
    /// 取消令牌（与调用方共享同一份 `Arc`）
    abort: Arc<AtomicBool>,
    /// 进程启动回调（把 pid 交回调用方登记）
    on_pid: Option<PidCallback>,
}

impl SubprocessSession {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            timeout: TimeoutPolicy::llm_inference(),
            session_id: String::new(),
            cwd: String::new(),
            sink: None,
            abort: Arc::new(AtomicBool::new(false)),
            on_pid: None,
        }
    }

    fn emit(&self, ev: UnifiedAgentEvent) {
        if let Some(sink) = &self.sink {
            sink(ev);
        }
    }

    /// 构造"对端会话 id"事件（stdout / stderr 两处提取共用）。
    fn emit_remote_id(&self, sid: String) {
        self.emit(UnifiedAgentEvent::AgentSessionId {
            session_id: self.session_id.clone(),
            agent_session_id: sid,
        });
    }
}

#[async_trait::async_trait]
impl AgentSession for SubprocessSession {
    fn transport(&self) -> TransportKind {
        TransportKind::Subprocess
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // 文本 / JSON 解析不产出结构化工具事件与思考链
            tool_events: false,
            permission: false,
            reasoning: false,
            resume: self.config.supports_session_continuity,
            // 每轮冷启动进程，非常驻会话
            persistent: false,
            automatable: true,
        }
    }

    async fn connect(
        &mut self,
        conn: &AgentConn,
        sink: EventSink,
    ) -> Result<SessionHandle, AgentError> {
        self.session_id = conn.session_id.clone();
        self.cwd = if conn.cwd.is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            conn.cwd.clone()
        };
        self.sink = Some(sink);
        self.abort = Arc::clone(&conn.abort);
        self.on_pid = conn.on_pid.clone();
        Ok(SessionHandle {
            session_id: conn.session_id.clone(),
            remote_id: conn.resume_id.clone(),
        })
    }

    async fn prompt(
        &self,
        handle: &SessionHandle,
        input: PromptInput,
    ) -> Result<PromptOutput, AgentError> {
        let handler = StdioHandler::from_config(self.config.clone());
        let (exe, args) = handler.build_command(&input.text, handle.remote_id.as_deref());
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let mut console = TokioConsole::new();
        let (pid, mut stdout_rx, mut stderr_rx) = console
            .spawn(&exe, &arg_refs, &self.cwd)
            .await
            .map_err(|e| AgentError::Connect(format!("启动进程失败: {}", e)))?;

        // 把 pid 交回调用方登记（保留"按 pid 终止进程树"的既有能力）
        if let Some(cb) = &self.on_pid {
            cb(pid);
        }

        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let mut full_output = String::new();
        // 对端自报的失败信息（优先于 exit code）
        let mut failed: Option<String> = None;
        let poll_start = Instant::now();

        let sid_from_stdout = self.config.session_id_source.starts_with("stdout-");
        let sid_from_stderr = self.config.session_id_source.starts_with("stderr-");

        loop {
            // 取消检查（调用方与实现共享同一令牌）
            if self.abort.load(Ordering::Relaxed) {
                log::info!("[transport/subprocess] abort requested, killing pid={}", pid);
                let _ = console.kill().await;
                return Err(AgentError::Cancelled);
            }

            // 智能超时轮询
            let elapsed = poll_start.elapsed();
            if elapsed >= self.timeout.check_interval {
                if let Some(error) = check_process_state(
                    console.try_wait(),
                    elapsed,
                    self.timeout.max_wait,
                    &format!("Agent({})", self.config.agent_type),
                    "",
                ) {
                    match error {
                        TimeoutError::ProcessExited { .. } => {
                            // 进程已退出：交给下方 channel 关闭检测收尾
                        }
                        TimeoutError::StillAlive { .. } => {
                            let _ = console.kill().await;
                            return Err(AgentError::Timeout(make_still_alive_error(
                                &format!("Agent({})", self.config.agent_type),
                                elapsed,
                                self.timeout.max_wait,
                            )));
                        }
                        TimeoutError::ChannelDisconnected(m) => {
                            return Err(AgentError::Transport(format!("执行通道异常断开: {}", m)));
                        }
                        TimeoutError::Cancelled(m) => {
                            return Err(AgentError::Transport(format!("已取消: {}", m)));
                        }
                    }
                }
            }

            // 双通道都关闭 = 进程退出（管道 EOF）
            if stdout_rx.is_closed() && stderr_rx.is_closed() {
                break;
            }

            let recv_timeout = if elapsed
                >= self.timeout.max_wait.saturating_sub(self.timeout.check_interval)
            {
                self.timeout.max_wait.saturating_sub(elapsed)
            } else {
                self.timeout.check_interval
            };

            tokio::select! {
                biased;

                Some(line) = stdout_rx.recv() => {
                    if sid_from_stdout {
                        if let Some(sid) = handler.extract_session_id(&line) {
                            self.emit_remote_id(sid);
                        }
                    }
                    if let Some(content) = handler.parse_output_line(&line) {
                        full_output.push_str(&content);
                        self.emit(UnifiedAgentEvent::Chunk {
                            session_id: self.session_id.clone(),
                            content,
                        });
                    }
                    // 结构化事件：补齐此前被整体丢弃的工具链路与用量
                    for ev in handler.parse_stream_events(&line) {
                        match ev {
                            StreamEvent::Usage {
                                prompt,
                                completion,
                                total,
                            } => self.emit(UnifiedAgentEvent::Usage {
                                session_id: self.session_id.clone(),
                                prompt,
                                completion,
                                total,
                            }),
                            StreamEvent::ToolStart { tool_id, name, args } => {
                                self.emit(UnifiedAgentEvent::ToolStart {
                                    session_id: self.session_id.clone(),
                                    tool_id,
                                    name,
                                    args,
                                })
                            }
                            StreamEvent::ToolResult {
                                tool_id,
                                result,
                                success,
                            } => self.emit(UnifiedAgentEvent::ToolResult {
                                session_id: self.session_id.clone(),
                                tool_id,
                                result,
                                success,
                            }),
                            StreamEvent::Failed { message } => failed = Some(message),
                        }
                    }
                }
                Some(line) = stderr_rx.recv() => {
                    if sid_from_stderr {
                        if let Some(sid) = handler.extract_session_id(&line) {
                            self.emit_remote_id(sid);
                        }
                    }
                    if let Ok(mut buf) = stderr_buf.lock() {
                        buf.push_str(&line);
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs_f64(recv_timeout.as_secs_f64())) => {}
            }
        }

        let exit_code = console
            .wait()
            .await
            .map_err(|e| AgentError::Transport(format!("异步控制台 wait 失败: {}", e)))?;

        if exit_code != 0 {
            // 某些 CLI 把错误写进 stdout（JSON 流）而非 stderr，故两路合并后再交给上层映射
            let stderr_text = stderr_buf.lock().map(|b| b.clone()).unwrap_or_default();
            let combined = if stderr_text.is_empty() && !full_output.is_empty() {
                full_output
            } else {
                stderr_text
            };
            return Err(AgentError::ProcessExited {
                exit_code,
                stderr: combined,
            });
        }

        // 对端自报失败优先于 exit code（实测：claude 可能 exit=1 而 subtype=success，
        // 反之也可能退出码为 0 却 is_error=true）→ 统一按失败上报，交由上层做友好映射
        if let Some(msg) = failed {
            return Err(AgentError::ProcessExited {
                exit_code: if exit_code == 0 { 1 } else { exit_code },
                stderr: msg,
            });
        }

        Ok(PromptOutput {
            stop_reason: StopReason::EndTurn,
            text: full_output,
        })
    }

    async fn cancel(&self, _handle: &SessionHandle) -> Result<(), AgentError> {
        self.abort.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn close(&self, _handle: &SessionHandle) -> Result<(), AgentError> {
        // 本实现每轮一个进程，无长驻资源需要释放
        Ok(())
    }
}
