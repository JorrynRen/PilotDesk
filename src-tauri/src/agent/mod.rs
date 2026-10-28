use crate::agent::async_process::AsyncConsole;
use crate::agent::config::AgentConfig;
use crate::agent::handler::ProcessHandler;
use crate::utils::errors::AppError;
use crate::utils::process::{
    check_process_state, hidden_command, hidden_tokio_command, make_still_alive_error,
    summarize_stderr, TimeoutPolicy,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Emitter;

use crate::decode_windows_output;
pub mod async_process;
pub mod config;
// P1 描述符：把 agents / api_providers 两张表统一成同一种内存模型（步骤 A，零 DDL）
pub mod descriptor;
pub mod handler;
// 宿主会话导入（`session/list` + `session/load` 回放），与运行链路互不复用状态
pub mod host_sessions;
pub mod line_processor;
pub mod list_skills;
pub mod lifecycle;
pub mod registry;
// P0 连接传输抽象层（契约）。当前仅声明、未被上层引用，接线在 P0-4 落地。
pub mod transport;

// ------------------------------------------------------------------
//  统一错误映射
// ------------------------------------------------------------------

pub(crate) fn friendly_agent_error(agent_type: &str, exit_code: i32, stderr: &str) -> String {
    let d = stderr.to_lowercase();
    let prefix = format!("{} 进程异常退出 (exit code {})", agent_type, exit_code);

    if d.contains("insufficient") && (d.contains("balance") || d.contains("quota")) {
        return "请求失败：API 账户余额不足。请前往 API 提供商后台充值后重试。".into();
    }
    if d.contains("403") {
        let detail = &stderr[..stderr.len().min(200)];
        return format!(
            "请求被拒 (HTTP 403)：{}。请检查 API Key 权限、账户余额或模型可用性。",
            detail
        );
    }
    if d.contains("401") {
        return "认证失败 (HTTP 401)：API Key 无效或已过期。请检查 API Key 是否正确。".into();
    }
    if d.contains("model") && (d.contains("not found") || d.contains("not support")) {
        let detail = &stderr[..stderr.len().min(200)];
        return format!(
            "模型不可用：{}。请检查模型名称是否正确，或更换模型后重试。",
            detail
        );
    }

    let detail = &stderr[..stderr.len().min(300)];
    if detail.is_empty() {
        prefix
    } else {
        format!("{}：{}", prefix, detail)
    }
}

// ------------------------------------------------------------------
//  P0-v2 开关
// ------------------------------------------------------------------

/// v2 开关：缺省**开启**（CLI 会话经传输抽象层执行）。
///
/// 设 `PILOTDESK_TRANSPORT_V2=0`（或 `false`）可**立即回退**到 legacy 路径，
/// **无需重新编译** —— 作为线上兜底开关。
fn transport_v2_enabled() -> bool {
    !matches!(
        std::env::var("PILOTDESK_TRANSPORT_V2").as_deref(),
        Ok("0") | Ok("false")
    )
}

// ------------------------------------------------------------------
//  统一执行参数与结果类型
// ------------------------------------------------------------------

/// 同步命令执行结果
#[derive(Debug, Clone)]
pub struct ExecuteResult {
    /// 标准输出（ trimmed）
    pub stdout: String,
    /// 退出码
    pub exit_code: i32,
    /// 标准错误
    pub stderr: String,
}

/// 异步会话执行参数（会话模式）
///
/// 用于 Agent LLM 交互等长生命周期会话。
/// 通过 async_process::TokioConsole 执行，双通道 IO 循环。
#[derive(Debug, Clone)]
pub struct AsyncOptions {
    /// 工作目录（会话启动前先 CD 到此目录）
    pub cwd: String,
    /// 前端会话 ID
    pub session_id: String,
    /// 来源类型
    pub source_type: String,
    /// 超时策略
    pub timeout: TimeoutPolicy,
}

/// 异步 IO 回调集
///
/// 由调用方提供，execute_command 在 IO 循环中按需调用。
pub struct AsyncCallbacks {
    /// 收到 Agent 输出片段时调用（用于 Event 推送到前端）
    pub on_chunk: Box<dyn Fn(String) + Send>,
    /// 提取到 Agent 会话 ID 时调用（用于 Event 推送到前端）
    pub on_session_id: std::sync::Arc<dyn Fn(String) + Send + Sync>,
    /// 检查是否应取消执行（abort_check）
    pub abort_check: Box<dyn Fn() -> bool + Send>,
    /// 进程 spawn 成功后调用（携带 PID，用于注册到进程管理表）
    pub on_pid: Box<dyn Fn(u32) + Send>,
}

// ------------------------------------------------------------------
//  进程管理
// ------------------------------------------------------------------

struct AgentProcess {
    pid: Option<u32>,
    aborted: Arc<AtomicBool>,
}

/// Agent 管理器
pub struct AgentManager {
    processes: Arc<Mutex<HashMap<String, AgentProcess>>>,
}

impl AgentManager {
    pub fn new() -> Self {
        Self {
            processes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 创建共享实例（用于 fire-and-forget 后台任务）
    /// 克隆进程追踪表的 Arc，使后台任务持有独立实例但共享进程表
    pub fn shared(&self) -> Self {
        Self {
            processes: Arc::clone(&self.processes),
        }
    }

    // ------------------------------------------------------------------
    //  统一命令执行入口
    // ------------------------------------------------------------------
    //
    // execute_command 是唯一的命令执行入口（当前仅异步路径有效）。
    //
    //   - 异步路径（opts.async_opts == Some(...)）：
    //     通过 async_process::TokioConsole 创建异步进程控制台，
    //     使用 ProcessHandler 驱动双通道 IO 循环，
    //     提取 session_id，推送输出片段到前端。
    //     返回 AsyncResult。
    //
    //   - 同步路径已移除：短命令请用 execute_command_output_async。
    //
    // 恢复会话与首次会话除多一个 agent_session_id 参数外完全相同。

    /// 统一命令执行入口
    ///
    /// # 异步路径
    /// - 通过 async_process::TokioConsole 创建异步进程控制台，
    ///   再由 ProcessHandler.build_command 构建命令并执行，双通道 IO 循环
    /// - session_id 由 ProcessHandler.extract_session_id 根据 session_id_source 自动提取
    /// - cwd 会话前先 CD 到指定目录
    ///
    /// 同步路径已随 virtual_console 模块一并移除（原仅死代码触达），
    /// 短命令请使用 execute_command_output_async。
    pub async fn execute_command(
        &self,
        async_opts: Option<AsyncOptions>,
        callbacks: Option<AsyncCallbacks>,
        config: Option<&AgentConfig>,
        message: Option<&str>,
        agent_session_id: Option<&str>,
    ) -> Result<ExecuteResult, String> {
        if async_opts.is_some() {
            self.execute_async(
                async_opts.unwrap(),
                callbacks.unwrap(),
                config.unwrap(),
                message.unwrap(),
                agent_session_id,
            )
            .await
        } else {
            Err("同步执行路径已移除，请使用 execute_command_output_async".to_string())
        }
    }

    // ------------------------------------------------------------------
    //  异步路径实现（核心 IO 循环）
    // ------------------------------------------------------------------

    /// 异步会话执行（会话模式）——分发入口。
    ///
    /// - 设置 `PILOTDESK_TRANSPORT_V2=1` → 走传输抽象层（`execute_async_v2`，P0-v2）
    /// - 否则 → 既有实现（`execute_async_legacy`，**默认路径**）
    ///
    /// 两条路径返回同一 `ExecuteResult` 契约，故上层调用方无需感知差异。
    #[allow(unused_assignments)]
    async fn execute_async(
        &self,
        async_opts: AsyncOptions,
        callbacks: AsyncCallbacks,
        config: &AgentConfig,
        message: &str,
        agent_session_id: Option<&str>,
    ) -> Result<ExecuteResult, String> {
        if transport_v2_enabled() {
            self.execute_async_v2(async_opts, callbacks, config, message, agent_session_id)
                .await
        } else {
            self.execute_async_legacy(async_opts, callbacks, config, message, agent_session_id)
                .await
        }
    }

    /// 既有实现（保留原样，作为默认路径与回退）。
    ///
    /// 通过 async_process::TokioConsole 创建异步进程控制台，
    /// ProcessHandler 驱动命令构建和输出解析，
    /// 双通道（stdout + stderr）IO 循环提取 session_id 和输出片段。
    ///
    /// 会话启动前先执行 "CD {cwd}" 确保 Agent 在指定目录下工作。
    #[allow(unused_assignments)]
    async fn execute_async_legacy(
        &self,
        async_opts: AsyncOptions,
        callbacks: AsyncCallbacks,
        config: &AgentConfig,
        message: &str,
        agent_session_id: Option<&str>,
    ) -> Result<ExecuteResult, String> {
        let cwd = if async_opts.cwd.is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            async_opts.cwd.clone()
        };
        let session_id = async_opts.session_id.clone();
        let _source_type = async_opts.source_type.clone();
        let timeout = async_opts.timeout;
        let agent_type = config.agent_type.clone();

        log::info!(
            "[Agent/execute_async] session='{}' agent_type='{}' cwd='{}' msg_len={}",
            session_id,
            agent_type,
            cwd,
            message.len()
        );

        // 创建异步进程控制台
        let mut console = async_process::TokioConsole::new();

        let process_handler = handler::StdioHandler::from_config(config.clone());

        // 构建命令：直接启动 CLI 进程（不经 cmd.exe /C），交由 Rust 的 argv 转义统一处理，
        // 避免手动引号 + cmd.exe 二次转义导致中文消息被错误拆分。
        let (exe, spawn_args): (String, Vec<String>) =
            process_handler.build_command(message, agent_session_id);

        log::info!(
            "[Agent/execute_async] CMD: {} {} | session_id={:?}",
            exe,
            spawn_args.join(" "),
            agent_session_id
        );

        let spawn_args_refs: Vec<&str> = spawn_args.iter().map(|s| s.as_str()).collect();

        // Spawn 进程
        let (pid, mut stdout_rx, mut stderr_rx) = console
            .spawn(&exe, &spawn_args_refs, &cwd)
            .await
            .map_err(|e| format!("启动进程失败: {}", e))?;

        (callbacks.on_pid)(pid);
        log::info!("[Agent/execute_async] async console spawned, pid={}", pid);

        // 注册 PID 到进程管理表：取消令牌复用运行登记表的令牌（无锁通路），
        // 使「停止生成」在 AgentManager 锁被占住时也能即时生效（与 v2 路径同口径）。
        {
            let aborted = crate::api_agent::session_runs::cancel_token(&session_id)
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
            let aborted_clone = aborted.clone();
            self.processes.lock().unwrap().insert(
                session_id.clone(),
                AgentProcess {
                    pid: Some(pid),
                    aborted: aborted_clone,
                },
            );
        }

        // stderr 缓冲区（共享）
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let stderr_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        // 主线程读取 stdout + 智能超时轮询
        let mut full_output = String::new();
        let poll_start = Instant::now();

        loop {
            // 检查取消
            if (callbacks.abort_check)() {
                log::info!("[Agent/execute_async] abort requested, killing pid={}", pid);
                let _ = console.kill().await;
                break;
            }

            // 智能超时轮询：检查 AsyncConsole 状态
            let elapsed = poll_start.elapsed();
            if elapsed >= timeout.check_interval {
                // 定期检查进程状态
                let try_wait_result = console.try_wait();

                if let Some(error) = check_process_state(
                    try_wait_result,
                    elapsed,
                    timeout.max_wait,
                    &format!("Agent({})", agent_type),
                    "",
                ) {
                    match error {
                        crate::utils::process::TimeoutError::ProcessExited {
                            exit_code, ..
                        } => {
                            log::warn!(
                                "[Agent/execute_async] process exited early: code={}",
                                exit_code
                            );
                        }
                        crate::utils::process::TimeoutError::StillAlive { .. } => {
                            log::error!(
                                "[Agent/execute_async] max wait exceeded, killing pid={}",
                                pid
                            );
                            let _ = console.kill().await;
                            // 构建 stderr 摘要
                            let _stderr_summary = {
                                let lines =
                                    stderr_lines.lock().map(|l| l.clone()).unwrap_or_default();
                                summarize_stderr(&lines, 300, 5)
                            };
                            return Err(make_still_alive_error(
                                &format!("Agent({})", agent_type),
                                elapsed,
                                timeout.max_wait,
                            ));
                        }
                        crate::utils::process::TimeoutError::ChannelDisconnected(msg) => {
                            return Err(format!("执行通道异常断开: {}", msg));
                        }
                        crate::utils::process::TimeoutError::Cancelled(msg) => {
                            return Err(format!("已取消: {}", msg));
                        }
                    }
                }
            }

            // 检查 stdout/stderr channel 是否都已关闭（进程退出后管道 EOF 会关闭 channel）
            // 使用 is_closed() 检测，不消耗数据，避免 Empty 误判
            let stdout_closed = stdout_rx.is_closed();
            let stderr_closed = stderr_rx.is_closed();
            if stdout_closed && stderr_closed {
                log::info!("[Agent/execute_async] both channels closed, breaking loop");
                break;
            }

            // 根据 session_id_source 决定从哪个流提取 session_id
            let sid_from_stdout = config.session_id_source.starts_with("stdout-");
            let sid_from_stderr = config.session_id_source.starts_with("stderr-");

            // 超时计算
            let recv_timeout = if elapsed >= timeout.max_wait.saturating_sub(timeout.check_interval)
            {
                timeout.max_wait.saturating_sub(elapsed)
            } else {
                timeout.check_interval
            };

            // 使用 biased select! 确保 stdout/stderr 优先于超时
            // sleep 作为超时兜底，确保循环不会无限挂起
            tokio::select! {
                biased;

                // stdout 分支
                Some(line) = stdout_rx.recv() => {
                    // 仅当 session_id 来源为 stdout 时提取
                    if sid_from_stdout {
                        if let Some(ref sid) = process_handler.extract_session_id(&line) {
                            (callbacks.on_session_id)(sid.clone());
                        }
                    }
                    // 解析输出行
                    if let Some(content) = process_handler.parse_output_line(&line) {
                        (callbacks.on_chunk)(content.clone());
                        full_output.push_str(&content);
                    }
                }
                // stderr 分支
                Some(line) = stderr_rx.recv() => {
                    // 仅当 session_id 来源为 stderr 时提取
                    if sid_from_stderr {
                        if let Some(ref sid) = process_handler.extract_session_id(&line) {
                            (callbacks.on_session_id)(sid.clone());
                        }
                    }
                    // stderr 缓冲
                    if let Ok(mut buf) = stderr_buf.lock() {
                        buf.push_str(&line);
                    }
                    if let Ok(mut lines) = stderr_lines.lock() {
                        lines.push(line);
                    }
                }
                // 超时分支
                _ = tokio::time::sleep(Duration::from_secs_f64(recv_timeout.as_secs_f64())) => {
                    // 超时 → 回到循环顶部继续轮询
                }
            }
        }

        // stderr 日志汇总
        if let Ok(buf) = stderr_buf.lock() {
            if !buf.is_empty() {
                log::warn!("[Agent/{}] stderr: {}", agent_type, buf.trim());
            }
        }

        log::info!(
            "[Agent/execute_async] read loop ended, output_len={}",
            full_output.len()
        );

        // 等待进程退出
        let exit_code = console
            .wait()
            .await
            .map_err(|e| format!("异步控制台 wait 失败: {}", e))?;

        let stderr_text = stderr_buf.lock().map(|b| b.clone()).unwrap_or_default();

        log::info!(
            "[Agent/execute_async] done: exit_code={}, output_len={}, stderr_len={}",
            exit_code,
            full_output.len(),
            stderr_text.len()
        );

        // 异步路径也返回 ExecuteResult（统一出口类型）
        // agent_session_id 已通过 on_session_id 回调推送到前端
        Ok(ExecuteResult {
            stdout: full_output.trim().to_string(),
            exit_code,
            stderr: stderr_text,
        })
    }

    // ------------------------------------------------------------------
    //  P0-v2：经传输抽象层执行（PILOTDESK_TRANSPORT_V2=1 启用）
    // ------------------------------------------------------------------

    /// 经 `transport::subprocess` 执行一轮 CLI 会话（P0-v2 路径）。
    ///
    /// 与 legacy 路径**语义等价**：命令构建 / 输出解析 / session_id 提取 /
    /// 超时轮询 / 取消 / 错误映射全部沿用同一套元信息与工具函数；
    /// pid 与取消令牌仍登记进既有 `processes` 进程表，故
    /// 「停止生成」「取消工作流」等按 pid 终止的实现**零改动**。
    async fn execute_async_v2(
        &self,
        async_opts: AsyncOptions,
        callbacks: AsyncCallbacks,
        config: &AgentConfig,
        message: &str,
        agent_session_id: Option<&str>,
    ) -> Result<ExecuteResult, String> {
        use crate::agent::transport::{
            AgentConn, AgentError, AgentSession, PidCallback, PromptInput, TransportKind,
            UnifiedAgentEvent,
        };

        let session_id = async_opts.session_id.clone();
        let agent_type = config.agent_type.clone();
        let cwd = if async_opts.cwd.is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            async_opts.cwd.clone()
        };

        log::info!(
            "[Agent/execute_async_v2] session='{}' agent_type='{}' cwd='{}' msg_len={}",
            session_id,
            agent_type,
            cwd,
            message.len()
        );

        // 取消令牌**复用运行登记表的令牌**（`api_agent::session_runs`）：
        // `agent_stop_generation` 在拿 `AgentManager` 锁**之前**就会 `session_runs::cancel()`，
        // 而 CLI 生成期间那把锁被 send 一直持有 —— 只有这条**无锁**通路能即时取消。
        // 同一令牌随后登记进进程表，故 `stop_generation` 的置位也作用在它上面。
        let aborted = crate::api_agent::session_runs::cancel_token(&session_id)
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let aborted_for_map = Arc::clone(&aborted);
        let processes_for_pid = Arc::clone(&self.processes);
        let session_for_pid = session_id.clone();
        let on_pid: PidCallback = Arc::new(move |pid: u32| {
            processes_for_pid.lock().unwrap().insert(
                session_for_pid.clone(),
                AgentProcess {
                    pid: Some(pid),
                    aborted: Arc::clone(&aborted_for_map),
                },
            );
        });

        // 事件出口：把 UnifiedAgentEvent 投影回既有 Tauri 事件
        // （`on_chunk` 是 `Box<dyn Fn + Send>`，用 Mutex 包成 `Sync` 以满足 EventSink）
        let on_chunk = Mutex::new(callbacks.on_chunk);
        let on_session_id = std::sync::Arc::clone(&callbacks.on_session_id);
        let sink: crate::agent::transport::EventSink = Arc::new(move |ev: UnifiedAgentEvent| {
            match ev {
                UnifiedAgentEvent::Chunk { content, .. } => {
                    if let Ok(f) = on_chunk.lock() {
                        f(content);
                    }
                }
                UnifiedAgentEvent::AgentSessionId {
                    agent_session_id, ..
                } => {
                    (on_session_id)(agent_session_id);
                }
                _ => {}
            }
        });

        let conn = AgentConn {
            session_id: session_id.clone(),
            agent_id: agent_type.clone(),
            transport: TransportKind::Subprocess,
            cwd,
            conn: serde_json::Value::Null,
            secret: None,
            resume_id: agent_session_id.map(|s| s.to_string()),
            abort: Arc::clone(&aborted),
            on_pid: Some(on_pid),
            approvals: None,
            // 本地 CLI 兼容路径不接 ACP 模式/审批策略（其权限由启动参数模板决定）
            acp_mode: None,
            security_mode: None,
            enforce_security_matrix: false,
        };

        let mut session =
            crate::agent::transport::subprocess::SubprocessSession::new(config.clone());
        let handle = session
            .connect(&conn, sink)
            .await
            .map_err(|e| format!("建立会话失败: {}", e))?;

        let result = session
            .prompt(
                &handle,
                PromptInput {
                    text: message.to_string(),
                    images: Vec::new(),
                },
            )
            .await;

        // 运行结束即摘除进程表项（与 legacy 的 stop_generation 收尾口径一致）
        if let Ok(mut procs) = self.processes.lock() {
            procs.remove(&session_id);
        }

        match result {
            Ok(out) => Ok(ExecuteResult {
                stdout: out.text.trim().to_string(),
                exit_code: 0,
                stderr: String::new(),
            }),
            // 非 0 退出：与 legacy 一致地返回 Ok + 非 0 退出码，交由上层做友好错误映射
            Err(AgentError::ProcessExited { exit_code, stderr }) => Ok(ExecuteResult {
                stdout: String::new(),
                exit_code,
                stderr,
            }),
            Err(AgentError::Cancelled) => Err("已取消".to_string()),
            Err(e) => Err(format!("{}", e)),
        }
    }

    // ------------------------------------------------------------------
    //  高层便捷方法（基于 execute_command 统一入口）
    // ------------------------------------------------------------------

    /// 异步便捷方法：执行命令并返回 stdout（纯 tokio::process，不经过虚拟控制台）
    ///
    /// 用于环境检测、版本查询等短命令场景。
    /// 轻量实现：无 ConPTY 开销、无 spawn_blocking 线程开销。
    /// 与 execute_async 同路径：tokio 原生异步 I/O。
    pub async fn execute_command_output_async(
        &self,
        command: &str,
        cwd: &str,
        timeout_secs: u64,
        label: &str,
    ) -> Result<String, String> {
        let work_dir = if cwd.is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            cwd.to_string()
        };

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let session_id = format!("async_cmd_{}_{}", label, ts);

        log::info!(
            "[Agent/execute_command_output_async] session='{}' cmd='{}' cwd='{}'",
            session_id,
            command,
            work_dir
        );

        // Windows: cmd /C 包装；其余平台：sh -c。
        // Windows 下把命令行"原样"交给 cmd.exe：argv 传递会为含引号的参数插入 `\"` 转义，
        // 而 cmd.exe 不识别该转义（含引号的命令会被误解析为以 `\\` 开头的 UNC 路径）。
        #[cfg(target_os = "windows")]
        let exe = "cmd";
        #[cfg(not(target_os = "windows"))]
        let exe = "sh";

        let result = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), async {
            let mut cmd = hidden_tokio_command(exe);
            #[cfg(target_os = "windows")]
            {
                cmd.raw_arg(format!("/C {}", command));
            }
            #[cfg(not(target_os = "windows"))]
            {
                cmd.args(["-c", command]);
            }
            cmd.current_dir(&work_dir)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .env_remove("PYTHONHOME")
                .kill_on_drop(true);

            cmd.output().await
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    let stdout = decode_windows_output(&output.stdout);
                    Ok(stdout.trim().to_string())
                } else {
                    let stderr = decode_windows_output(&output.stderr);
                    Err(format!(
                        "命令执行失败 (exit code {:?}): {}",
                        output.status.code(),
                        stderr.trim()
                    ))
                }
            }
            Ok(Err(e)) => Err(format!("启动进程失败: {}", e)),
            Err(_) => Err(format!("命令执行超时 ({}s): {}", timeout_secs, command)),
        }
    }

    /// 前端会话模式：Event 推送
    pub async fn send_message_with_config(
        &mut self,
        app_handle: tauri::AppHandle,
        session_id: String,
        config: AgentConfig,
        message: String,
        _mode: String,
        cwd: Option<String>,
        _system_prompt: Option<String>,
        agent_session_id: Option<String>,
    ) -> Result<(), String> {
        let source_type = config.agent_type.clone();
        let async_opts = AsyncOptions {
            cwd: cwd.unwrap_or_default(),
            session_id: session_id.clone(),
            source_type: source_type.clone(),
            timeout: TimeoutPolicy::llm_inference(),
        };

        let app_for_chunk = app_handle.clone();
        let sid_for_chunk = session_id.clone();
        let app_for_sid = app_handle.clone();
        let sid_for_sid = session_id.clone();
        let aborted = Arc::new(AtomicBool::new(false));
        let aborted_clone = aborted.clone();
        let processes_arc = Arc::clone(&self.processes);
        let session_for_abort = Arc::new(session_id.clone());
        let session_for_abort_check = Arc::clone(&session_for_abort);
        let session_for_pid = Arc::clone(&session_for_abort);

        let callbacks = AsyncCallbacks {
            on_chunk: Box::new(move |chunk| {
                let _ = app_for_chunk.emit(
                    "agent-chunk",
                    serde_json::json!({
                        "sessionId": sid_for_chunk,
                        "content": chunk,
                    }),
                );
            }),
            on_session_id: std::sync::Arc::new(move |sid_agent| {
                let _ = app_for_sid.emit(
                    "agent-session",
                    serde_json::json!({
                        "sessionId": sid_for_sid,
                        "agentSessionId": sid_agent,
                    }),
                );
            }),
            abort_check: Box::new(move || {
                // 无锁通路优先：stop 命令在拿 AgentManager 锁【之前】就已 `session_runs::cancel()`，
                // 而 CLI 生成期间那把锁被 send 一直持有 —— 只有这条通路能即时生效。
                if crate::api_agent::session_runs::cancel_token(&*session_for_abort_check)
                    .is_some_and(|t| t.load(Ordering::Relaxed))
                {
                    return true;
                }
                // 进程表标志（stop_generation 按 pid 终止时的正交信号）
                if let Ok(procs) = processes_arc.lock() {
                    if let Some(proc) = procs.get(&*session_for_abort_check) {
                        if proc.aborted.load(Ordering::Relaxed) {
                            return true;
                        }
                    }
                }
                aborted_clone.load(Ordering::Relaxed)
            }),
            on_pid: Box::new(move |spawned_pid: u32| {
                log::info!(
                    "[Agent/send_message] on_pid callback: session={}, pid={}",
                    &*session_for_pid,
                    spawned_pid
                );
            }),
        };

        let app_for_result = app_handle.clone();
        let sid_for_result = session_id.clone();
        let source_type_for_error = source_type.clone();

        let result = self
            .execute_command(
                Some(async_opts),
                Some(callbacks),
                Some(&config),
                Some(&message),
                agent_session_id.as_deref(),
            )
            .await;

        // 失败轮不再补发 agent-done：错误已由 agent-error 收尾（前端总线在该事件里
        // finishRun(notifyEmpty=false) 落库已产出的正文，组件再写一条失败原因）。
        // 若这里照旧补 done，前端会把它当成"正常结束"再收一次尾 —— 而那时流式态已被丢弃、
        // 正文为空，于是又追一条"未收到模型有效响应"，用户看到的就是两条系统消息
        // （一条真实失败原因 + 一条误导性的兜底提示）。
        let mut failed = false;

        // 用户主动停止时（`session_runs` 的无锁取消通路已置位）取消**不是错误**：
        // 发 `agent-cancelled` 让前端收尾落库，避免弹出"错误：已取消"提示。
        let was_cancelled = crate::api_agent::session_runs::cancel_token(&sid_for_result)
            .is_some_and(|t| t.load(Ordering::Relaxed));
        if was_cancelled {
            log::info!(
                "[Agent/send_message] 已取消，发 agent-cancelled（不报错）: {}",
                sid_for_result
            );
            let _ = app_for_result.emit(
                "agent-cancelled",
                serde_json::json!({ "sessionId": sid_for_result }),
            );
            return Ok(());
        }

        match result {
            Ok(exec_result) => {
                if exec_result.exit_code != 0 {
                    failed = true;
                    // 某些 CLI 工具（如 Codex、Claude）将错误写入 stdout（JSON 流）
                    // 而非 stderr，因此需合并两路输出查找错误详情
                    let combined_error = if exec_result.stderr.is_empty()
                        && !exec_result.stdout.is_empty()
                    {
                        exec_result.stdout.clone()
                    } else if !exec_result.stderr.is_empty() {
                        exec_result.stderr.clone()
                    } else {
                        "（stdout 和 stderr 均为空，请检查 CLI 工具是否已安装并正确配置 API Key 和模型）".to_string()
                    };
                    let err_msg = friendly_agent_error(
                        &source_type_for_error,
                        exec_result.exit_code,
                        &combined_error,
                    );
                    let _ = app_for_result.emit(
                        "agent-error",
                        serde_json::json!({
                            "sessionId": sid_for_result,
                            "error": err_msg,
                        }),
                    );
                } else {
                    // 事件触发钩子：用户会话一轮生成结束（仅成功轮；工作流 Agent 节点走 execute_once，不经此处）
                    crate::workflow::triggers::dispatch_event(
                        &app_for_result,
                        "session.completed",
                        serde_json::json!({
                            "sessionId": sid_for_result,
                        }),
                    );
                }
            }
            Err(e) => {
                failed = true;
                let _ = app_for_result.emit(
                    "agent-error",
                    serde_json::json!({
                        "sessionId": sid_for_result,
                        "error": format!("{}", e),
                    }),
                );
            }
        }

        if failed {
            return Ok(());
        }

        let _ = app_for_result.emit(
            "agent-done",
            serde_json::json!({
                "sessionId": sid_for_result,
            }),
        );

        Ok(())
    }

    /// 单次执行模式：直接返回完整输出
    pub async fn execute_once(
        &mut self,
        config: &AgentConfig,
        prompt: &str,
        _params: &serde_json::Value,
        cwd: &str,
        _temp_session_id: &str,
        on_chunk: impl Fn(String) + Send + 'static,
        agent_session_id: Option<&str>,
    ) -> Result<(String, Option<String>), AppError> {
        let source_type = config.agent_type.clone();
        let async_opts = AsyncOptions {
            cwd: cwd.to_string(),
            session_id: _temp_session_id.to_string(),
            source_type: source_type.clone(),
            timeout: TimeoutPolicy::llm_inference(),
        };

        let on_chunk_owned = Arc::new(Mutex::new(Box::new(on_chunk) as Box<dyn Fn(String) + Send>));
        let on_chunk_clone = on_chunk_owned.clone();
        let sid_result = Arc::new(Mutex::new(None::<String>));

        let callbacks = AsyncCallbacks {
            on_chunk: Box::new(move |chunk| {
                if let Ok(func) = on_chunk_clone.lock() {
                    func(chunk);
                }
            }),
            on_session_id: std::sync::Arc::new({
                let sid_result_clone = sid_result.clone();
                move |sid| {
                    if let Ok(mut s) = sid_result_clone.lock() {
                        *s = Some(sid);
                    }
                }
            }),
            abort_check: {
                let processes_abort = Arc::clone(&self.processes);
                let session_abort = _temp_session_id.to_string();
                Box::new(move || {
                    if let Ok(procs) = processes_abort.lock() {
                        if let Some(proc) = procs.get(&session_abort) {
                            return proc.aborted.load(Ordering::Relaxed);
                        }
                    }
                    false
                })
            },
            on_pid: {
                let processes_pid = Arc::clone(&self.processes);
                let session_pid = _temp_session_id.to_string();
                Box::new(move |spawned_pid: u32| {
                    log::info!(
                        "[Agent/execute_once] on_pid callback: session={}, pid={}",
                        session_pid,
                        spawned_pid
                    );
                    let aborted = Arc::new(AtomicBool::new(false));
                    let aborted_clone = aborted.clone();
                    processes_pid.lock().unwrap().insert(
                        session_pid.clone(),
                        AgentProcess {
                            pid: Some(spawned_pid),
                            aborted: aborted_clone,
                        },
                    );
                })
            },
        };

        let result = self
            .execute_command(
                Some(async_opts),
                Some(callbacks),
                Some(config),
                Some(prompt),
                agent_session_id,
            )
            .await
            .map_err(|e| AppError::External(e))?;

        if result.exit_code != 0 {
            let combined_error = if result.stderr.is_empty() && !result.stdout.is_empty() {
                result.stdout.clone()
            } else {
                result.stderr.clone()
            };
            return Err(AppError::External(friendly_agent_error(
                &source_type,
                result.exit_code,
                &combined_error,
            )));
        }

        let agent_sid = sid_result.lock().unwrap().clone();
        Ok((result.stdout, agent_sid))
    }

    // ------------------------------------------------------------------
    //  进程控制
    // ------------------------------------------------------------------

    pub fn stop_generation(&mut self, session_id: &str) {
        if let Ok(processes) = self.processes.lock() {
            if let Some(process) = processes.get(session_id) {
                process.aborted.store(true, Ordering::Relaxed);
            }
            if let Some(pid) = processes.get(session_id).and_then(|p| p.pid) {
                #[cfg(target_os = "windows")]
                {
                    let _ = hidden_command("taskkill")
                        .args(&["/PID", &pid.to_string(), "/F"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = hidden_command("kill")
                        .arg("-9")
                        .arg(pid.to_string())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            }
        }
        if let Ok(mut processes) = self.processes.lock() {
            processes.remove(session_id);
        }
        log::info!("[Agent] Generation stopped: {}", session_id);
    }

    pub fn create_session(&mut self, session_id: &str, _agent_type: &str, _cwd: Option<&str>) {
        log::info!("[Agent] Session created: {}", session_id);
    }

    /// 停止 Agent 子进程（&self 版本，不需要 &mut self）
    ///
    /// 与 stop_generation 功能相同，但接受 &self 而非 &mut self，
    /// 因此调用方不需要持有 AsyncMutex<AgentManager> 的可变锁。
    /// 直接通过 processes Arc<HashMap> 操作进程表，用于 cancel_workflow
    /// 等需要快速响应取消的场景。
    pub fn stop_generation_no_mut(&self, session_id: &str) {
        if let Ok(processes) = self.processes.lock() {
            if let Some(process) = processes.get(session_id) {
                process.aborted.store(true, Ordering::Relaxed);
            }
            if let Some(pid) = processes.get(session_id).and_then(|p| p.pid) {
                #[cfg(target_os = "windows")]
                {
                    let _ = hidden_command("taskkill")
                        .args(&["/PID", &pid.to_string(), "/F"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = hidden_command("kill")
                        .arg("-9")
                        .arg(pid.to_string())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            }
        }
        if let Ok(mut processes) = self.processes.lock() {
            processes.remove(session_id);
        }
        log::info!("[Agent] Generation stopped (no_mut): {}", session_id);
    }

    pub fn close_session(&mut self, session_id: &str) {
        self.stop_generation(session_id);
        log::info!("[Agent] Session closed: {}", session_id);
    }

    pub async fn list_skills(
        _agent_type: &str,
        config: Option<&crate::agent::config::AgentConfig>,
    ) -> Vec<crate::db::models::SkillInfo> {
        // 技能根解析 + 扫描统一在 agent::list_skills 里（安装/卸载/读写也复用同一套路径口径）
        let Some(cfg) = config else { return vec![] };
        if cfg.skills_dir.is_empty() {
            return vec![];
        }
        match crate::agent::list_skills::resolve_skills_dir(&cfg.skills_dir, &cfg.agent_type) {
            Some(dir) => crate::agent::list_skills::scan_skills_dir(
                &dir,
                &cfg.skill_entry_file,
                &cfg.skill_display_mode,
            ),
            None => vec![],
        }
    }
}

/// 经传输抽象层跑一轮会话（供 `lib.rs` 按 `agents.transport` 路由非 subprocess 接入方式）。
///
/// 与 `execute_async_v2` 同口径：
/// - 取消走 `session_runs` 的**无锁**令牌（前端"停止生成"在拿 `AgentManager` 锁之前就置位它）；
/// - 收尾只在**一处**发事件（`agent-done` / `agent-error` / `agent-cancelled`），避免两条系统消息；
/// - **取消不是错误**：先判取消再判错误，否则会弹"错误：已取消"。
///
/// 事件载荷字段名逐字对齐前端 `agentEventBus`（见实施计划 §十五）：
/// ⚠️ 审批事件**没有 `callId` 字段**，前端以 `toolId` 作 callId —— 故这里把 `call_id` 写进 `toolId`。
pub async fn run_transport_session(
    app: tauri::AppHandle,
    descriptor: &crate::agent::descriptor::AgentDescriptor,
    session_id: &str,
    message: &str,
    cwd: &str,
    agent_session_id: Option<&str>,
    approvals: Option<std::sync::Arc<dyn crate::agent::transport::ApprovalGate>>,
    // 会话安全模式（仅 ACP 兜底路径使用：宿主不提供 modes 时决定要不要打断用户）
    security_mode: Option<crate::api_agent::agent_loop::SecurityMode>,
    // 用户固定的宿主会话模式 id（None = 跟随宿主；仅 ACP 使用）
    acp_mode: Option<&str>,
) -> Result<(), String> {
    use crate::agent::transport::{
        build_session, AgentConn, AgentError, PromptInput, UnifiedAgentEvent,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    let Some(mut session) = build_session(descriptor.transport, None) else {
        return Err(format!(
            "接入方式 {:?} 暂不支持会话（mcp 为能力面、webview 为承载）",
            descriptor.transport
        ));
    };
    log::info!(
        "[Agent/run_transport_session] session='{}' agent='{}' transport={:?}",
        session_id,
        descriptor.agent_id,
        descriptor.transport
    );

    // 事件出口：载荷字段名逐字对齐前端 `agentEventBus`（实施计划 §十五）。
    // `Usage` 暂不投影——其载荷键名尚未确认，不猜。
    //
    // 这里本地维护 `toolId → toolName`：`agent-tool-result` 要求 `toolName`，
    // 而统一事件里的 `ToolResult` 不带名字（ACP 的 `tool_call_update` 也不保证带 title）。
    let tool_names: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let app_evt = app.clone();
    let sid_evt = session_id.to_string();
    let sink: crate::agent::transport::EventSink = std::sync::Arc::new(move |ev| match ev {
        UnifiedAgentEvent::Chunk { content, .. } => {
            let _ = app_evt.emit(
                "agent-chunk",
                serde_json::json!({ "sessionId": sid_evt, "content": content }),
            );
        }
        UnifiedAgentEvent::Reasoning { content, .. } => {
            let _ = app_evt.emit(
                "agent-reasoning",
                serde_json::json!({ "sessionId": sid_evt, "content": content }),
            );
        }
        UnifiedAgentEvent::ToolStart {
            tool_id,
            name,
            args,
            ..
        } => {
            if let Ok(mut m) = tool_names.lock() {
                m.insert(tool_id.clone(), name.clone());
            }
            let _ = app_evt.emit(
                "agent-tool-start",
                serde_json::json!({
                    "sessionId": sid_evt,
                    "toolId": tool_id,
                    "toolName": name,
                    "arguments": args,
                }),
            );
        }
        UnifiedAgentEvent::ToolResult {
            tool_id,
            result,
            success,
            ..
        } => {
            // 名字取自本轮 `ToolStart` 的记录（缺则空串）
            let name = tool_names
                .lock()
                .ok()
                .and_then(|mut m| m.remove(&tool_id))
                .unwrap_or_default();
            let _ = app_evt.emit(
                "agent-tool-result",
                serde_json::json!({
                    "sessionId": sid_evt,
                    "toolId": tool_id,
                    "toolName": name,
                    "result": result,
                    "success": success,
                }),
            );
        }
        UnifiedAgentEvent::FileDiff { path, diff, .. } => {
            let _ = app_evt.emit(
                "agent-file-diff",
                serde_json::json!({ "sessionId": sid_evt, "path": path, "diff": diff }),
            );
        }
        UnifiedAgentEvent::AgentSessionId {
            agent_session_id, ..
        } => {
            let _ = app_evt.emit(
                "agent-session",
                serde_json::json!({ "sessionId": sid_evt, "agentSessionId": agent_session_id }),
            );
        }
        UnifiedAgentEvent::SessionModes {
            available_modes,
            current_mode_id,
            ..
        } => {
            // 前端据此渲染宿主模式选择器；空列表 = 该宿主不提供模式（回退安全模式）
            let _ = app_evt.emit(
                "agent-session-modes",
                serde_json::json!({
                    "sessionId": sid_evt,
                    "availableModes": available_modes,
                    "currentModeId": current_mode_id,
                }),
            );
        }
        UnifiedAgentEvent::ApprovalRequired {
            call_id,
            name,
            args,
            risk,
            ..
        } => {
            let _ = app_evt.emit(
                "agent-approval-required",
                serde_json::json!({
                    "sessionId": sid_evt,
                    "toolId": call_id,          // ← 前端以 toolId 作 callId（§十五 的坑）
                    "toolName": name,
                    "arguments": args,
                    "riskDescription": risk,
                    // 倒计时基准由后端下发：前端不再写死 120s（ACP 实际等 300s）
                    "timeoutSecs": crate::agent::transport::ACP_APPROVAL_TIMEOUT_SECS,
                }),
            );
        }
        UnifiedAgentEvent::ApprovalResolved {
            call_id,
            approved,
            timed_out,
            ..
        } => {
            // 与 required 成对：缺了它审批卡永远停在未决，通知中心也永远不消解
            let _ = app_evt.emit(
                "agent-approval-resolved",
                serde_json::json!({
                    "sessionId": sid_evt,
                    "toolId": call_id,
                    "toolName": "",
                    "approved": approved,
                    "timedOut": timed_out,
                    "risk": "",
                }),
            );
        }
        _ => {}
    });

    let abort = crate::api_agent::session_runs::cancel_token(session_id)
        .unwrap_or_else(|| std::sync::Arc::new(AtomicBool::new(false)));

    let conn = AgentConn {
        session_id: session_id.to_string(),
        agent_id: descriptor.agent_id.clone(),
        transport: descriptor.transport,
        cwd: cwd.to_string(),
        conn: descriptor.conn.clone(),
        secret: None,
        resume_id: agent_session_id.map(|s| s.to_string()),
        abort: std::sync::Arc::clone(&abort),
        on_pid: None,
        approvals,
        acp_mode: acp_mode
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        security_mode,
        // 会话模式路径：宿主提供模式时权限归宿主，安全模式只作兜底，不替宿主裁决
        enforce_security_matrix: false,
    };

    let handle = session
        .connect(&conn, sink)
        .await
        .map_err(|e| format!("建立会话失败: {}", e))?;
    let result = session
        .prompt(
            &handle,
            PromptInput {
                text: message.to_string(),
                images: Vec::new(),
            },
        )
        .await;

    // 取消优先：取消不是错误
    let cancelled = abort.load(Ordering::Relaxed)
        || crate::api_agent::session_runs::cancel_token(session_id)
            .is_some_and(|t| t.load(Ordering::Relaxed));

    if cancelled {
        // 先上报终态、再关闭对端：`agent-cancelled` 是前端"思考中 → 已停止"的唯一信号，
        // 必须排在可能阻塞的 `close()` 之前，否则对端回收慢会让 UI 卡在"生成中"。
        let _ = app.emit(
            "agent-cancelled",
            serde_json::json!({ "sessionId": session_id }),
        );
        // 取消后必须关闭对端：否则进程变成孤儿，继续往**已结束的回合**发流式事件，
        // 前端会看到"停止后仍在输出"，静默看门狗还会再补一次超时收尾。
        let _ = session.close(&handle).await;
        return Ok(());
    }

    match result {
        Ok(_) => {
            log::info!("[Agent/run_transport_session] 回合结束(完成)：session={}", session_id);
            // 先上报结束、再关闭对端：`agent-done` 是前端"思考中 → 完成"的唯一信号，
            // 必须排在可能阻塞的 `close()`（`child.kill()` 内部含 `wait()`）之前，
            // 否则对端回收慢就会让 UI 一直停在"生成中"。
            let _ = app.emit("agent-done", serde_json::json!({ "sessionId": session_id }));
            // 成功路径同样必须关闭对端：会话是**每轮新建**的（`build_session` 每次都会起
            // 一个新进程），不关闭会留下常驻的孤儿 `hermes acp` 进程——它们仍持有会话与
            // stdout，下一轮复用同一会话时会把事件串到已结束的回合里。
            let _ = session.close(&handle).await;
            Ok(())
        }
        Err(e) => {
            // 对端失活（后端看门狗判定）与普通错误分开上报：前者是**明确终态**，
            // 不该再补一条"请求失败"（两条互相矛盾的收尾文案）。
            let idle_secs = match &e {
                AgentError::IdleAborted { idle_secs } => Some(*idle_secs),
                _ => None,
            };
            match idle_secs {
                Some(secs) => {
                    log::warn!(
                        "[Agent/run_transport_session] 回合结束(对端失活)：session={} 静默={}s",
                        session_id,
                        secs
                    );
                    // 同上：终态事件先于 `close()`
                    let _ = app.emit(
                        "agent-idle-aborted",
                        serde_json::json!({ "sessionId": session_id, "idleSeconds": secs }),
                    );
                }
                None => {
                    log::warn!(
                        "[Agent/run_transport_session] 回合结束(错误)：session={} err={}",
                        session_id,
                        e
                    );
                    // 同上：终态事件先于 `close()`
                    let _ = app.emit(
                        "agent-error",
                        serde_json::json!({ "sessionId": session_id, "error": format!("{}", e) }),
                    );
                }
            }
            // 错误路径同样必须关闭对端：本轮已判死，但孤儿进程仍持有 stdout，
            // 会持续向已结束的回合推送事件（现场表现："报错后仍继续"）。
            let _ = session.close(&handle).await;
            // 与既有 CLI 口径一致：错误已上报，不再向调用方冒泡
            Ok(())
        }
    }
}

impl Drop for AgentManager {
    fn drop(&mut self) {
        if let Ok(mut processes) = self.processes.lock() {
            for (_, process) in processes.drain() {
                process.aborted.store(true, Ordering::Relaxed);
                if let Some(pid) = process.pid {
                    #[cfg(target_os = "windows")]
                    {
                        let _ = hidden_command("taskkill")
                            .args(&["/PID", &pid.to_string(), "/F"])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn();
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        let _ = hidden_command("kill")
                            .arg("-9")
                            .arg(pid.to_string())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn();
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------
//  Skill 解析与扫描
// ------------------------------------------------------------------
//  实现已统一到 `agent::list_skills`（扫描结果带落盘路径，安装/卸载/主文件读写也在此模块），
//  此处不再保留副本，避免两套口径漂移。
