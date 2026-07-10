use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Emitter;
use crate::agent::config::AgentConfig;
use crate::agent::handler::ProcessHandler;
use crate::utils::errors::AppError;
use crate::utils::process::{TimeoutPolicy, check_process_state, make_still_alive_error, summarize_stderr};

use crate::virtual_console::factory::ConsoleFactory;
// AsyncConsole trait 已通过 ConsoleFactory::create_async() 内部使用，无需直接导入
pub mod handler;
pub mod line_processor;
pub mod list_skills;
pub mod config;

// ------------------------------------------------------------------
//  统一错误映射
// ------------------------------------------------------------------

fn friendly_agent_error(agent_type: &str, exit_code: i32, stderr: &str) -> String {
    let d = stderr.to_lowercase();
    let prefix = format!("{} 进程异常退出 (exit code {})", agent_type, exit_code);

    if d.contains("insufficient") && (d.contains("balance") || d.contains("quota")) {
        return "请求失败：API 账户余额不足。请前往 API 提供商后台充值后重试。".into();
    }
    if d.contains("403") {
        let detail = &stderr[..stderr.len().min(200)];
        return format!("请求被拒 (HTTP 403)：{}。请检查 API Key 权限、账户余额或模型可用性。", detail);
    }
    if d.contains("401") {
        return "认证失败 (HTTP 401)：API Key 无效或已过期。请检查 API Key 是否正确。".into();
    }
    if d.contains("model") && (d.contains("not found") || d.contains("not support")) {
        let detail = &stderr[..stderr.len().min(200)];
        return format!("模型不可用：{}。请检查模型名称是否正确，或更换模型后重试。", detail);
    }

    let detail = &stderr[..stderr.len().min(300)];
    if detail.is_empty() { prefix } else { format!("{}：{}", prefix, detail) }
}

// ------------------------------------------------------------------
//  统一执行参数与结果类型
// ------------------------------------------------------------------

/// 同步命令执行参数（简单模式）
///
/// 用于版本查询、安装/卸载等短生命周期命令。
/// 通过同步虚拟控制台 (VirtualConsole) 执行，智能超时轮询。
#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    /// 命令字符串（将被 cmd /C 或 sh -c 包装）
    pub command: String,
    /// 工作目录（空字符串使用当前目录）
    pub cwd: String,
    /// 前端会话 ID（用于控制台桥接跟踪）
    pub session_id: String,
    /// 来源类型（agent 类型、插件名称等）
    pub source_type: String,
    /// 超时策略
    pub timeout: TimeoutPolicy,
}

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
/// 通过异步虚拟控制台 (AsyncConsole) 执行，双通道 IO 循环。
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
    // execute_command 是唯一的命令执行入口。
    //
    //   - 同步路径（opts.async_opts == None）：
    //     通过 ConsoleFactory::auto_create() 创建同步虚拟控制台，
    //     执行简单命令（版本查询、安装等），智能超时轮询。
    //     返回 ExecuteResult。
    //
    //   - 异步路径（opts.async_opts == Some(...)）：
    //     通过 ConsoleFactory::create_async() 创建异步虚拟控制台，
    //     使用 ProcessHandler 驱动双通道 IO 循环，
    //     提取 session_id，推送输出片段到前端。
    //     返回 AsyncResult。
    //
    // 恢复会话与首次会话除多一个 agent_session_id 参数外完全相同。

    /// 统一命令执行入口
    ///
    /// # 同步路径
    /// - 创建同步虚拟控制台 → spawn → 读取 stdout → wait → 返回 ExecuteResult
    /// - 超时轮询：通过 channel 传递结果，外层按 TimeoutPolicy 轮询
    ///
    /// # 异步路径
    /// - 创建异步虚拟控制台 → ProcessHandler.build_command → spawn → 双通道 IO 循环
    /// - session_id 由 ProcessHandler.extract_session_id 根据 session_id_source 自动提取
    /// - cwd 会话前先 CD 到指定目录
    pub async fn execute_command(
        &self,
        opts: ExecuteOptions,
        async_opts: Option<AsyncOptions>,
        callbacks: Option<AsyncCallbacks>,
        config: Option<&AgentConfig>,
        message: Option<&str>,
        agent_session_id: Option<&str>,
    ) -> Result<ExecuteResult, String> {
        if async_opts.is_some() {
            // ── 异步路径 ──
            self.execute_async(async_opts.unwrap(), callbacks.unwrap(), config.unwrap(), message.unwrap(), agent_session_id).await
        } else {
            // ── 同步路径 ──
            self.execute_sync(opts).await
        }
    }

    // ------------------------------------------------------------------
    //  同步路径实现
    // ------------------------------------------------------------------

    /// 同步命令执行（简单模式）
    ///
    /// 通过 ConsoleFactory::auto_create() 创建同步虚拟控制台，
    /// 在 spawn_blocking 中执行命令并读取输出，通过 channel 返回结果。
    /// 外层按 TimeoutPolicy 轮询 channel，并检查进程状态。
    async fn execute_sync(&self, opts: ExecuteOptions) -> Result<ExecuteResult, String> {
        let command = opts.command.clone();
        let command_for_log = command.clone();
        let work_dir = if opts.cwd.is_empty() {
            std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            opts.cwd.clone()
        };
        let timeout = opts.timeout;
        let session_id = opts.session_id.clone();
        let _source_type = opts.source_type.clone();

        log::info!("[Agent/execute_sync] session='{}' cmd='{}' cwd='{}'",
            session_id, command, work_dir);

        let (tx, rx) = std::sync::mpsc::channel::<Result<ExecuteResult, String>>();


        // spawn_blocking 中执行同步虚拟控制台
        tokio::task::spawn_blocking(move || {
            let result = (|| -> Result<ExecuteResult, String> {
                #[cfg(target_os = "windows")]
                let (exe, args): (&str, &[&str]) = ("cmd", &["/C", &command]);
                #[cfg(not(target_os = "windows"))]
                let (exe, args): (&str, &[&str]) = ("sh", &["-c", &command]);

                let mut console = ConsoleFactory::auto_create()
                    .map_err(|e| format!("创建虚拟控制台失败: {}", e))?;

                let mut handle = console.spawn(exe, args, &work_dir)
                    .map_err(|e| format!("启动进程失败: {}", e))?;

                let pid = handle.pid;
                log::info!("[Agent/execute_sync] console spawned: pid={} cmd='{}'", pid, command);

                // 注册 PID 到桥接（同步路径无法回调，直接设置）
                // PID 注册通过结果传递回外层处理

                // 读取 stdout
                let mut output_lines = Vec::new();
                loop {
                    match handle.stdout.read_line() {
                        Ok(line) if line.is_empty() => break,
                        Ok(line) => output_lines.push(line),
                        Err(_) => break,
                    }
                }

                let exit_code = match console.wait(None) {
                    Ok(status) => status.code().unwrap_or(-1),
                    Err(e) => return Err(format!("等待进程失败: {}", e)),
                };

                let combined = output_lines.join("");
                Ok(ExecuteResult {
                    stdout: combined.trim().to_string(),
                    exit_code,
                    stderr: String::new(),
                })
            })();

            let _ = tx.send(result);
        });

        // 智能超时轮询
        let start = Instant::now();
        loop {
            match rx.try_recv() {
                Ok(Ok(result)) => {
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    return Err(e);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // 尚未完成，检查超时
                    let elapsed = start.elapsed();
                    if elapsed >= timeout.max_wait {
                        // 超过最大等待时间，报告超时
                        // 注意：同步控制台不支持 try_wait，无法精确判断进程状态
                        return Err(make_still_alive_error(
                            &format!("execute_sync:{}", command_for_log),
                            elapsed,
                            timeout.max_wait,
                        ));
                    }

                    tokio::time::sleep(timeout.check_interval).await;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // channel 断开（spawn_blocking 任务 panic）
                    return Err(format!("执行通道异常断开"));
                }
            }
        }
    }

    // ------------------------------------------------------------------
    //  异步路径实现（核心 IO 循环）
    // ------------------------------------------------------------------

    /// 异步会话执行（会话模式）
    ///
    /// 通过 AsyncConsole 创建异步虚拟控制台，
    /// ProcessHandler 驱动命令构建和输出解析，
    /// 双通道（stdout + stderr）IO 循环提取 session_id 和输出片段。
    ///
    /// 会话启动前先执行 "CD {cwd}" 确保 Agent 在指定目录下工作。
    #[allow(unused_assignments)]
    async fn execute_async(
        &self,
        async_opts: AsyncOptions,
        callbacks: AsyncCallbacks,
        config: &AgentConfig,
        message: &str,
        agent_session_id: Option<&str>,
    ) -> Result<ExecuteResult, String> {
        let cwd = if async_opts.cwd.is_empty() {
            std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            async_opts.cwd.clone()
        };
        let session_id = async_opts.session_id.clone();
        let _source_type = async_opts.source_type.clone();
        let timeout = async_opts.timeout;
        let agent_type = config.agent_type.clone();

        log::info!("[Agent/execute_async] session='{}' agent_type='{}' cwd='{}' msg_len={}",
            session_id, agent_type, cwd, message.len());

        // 创建异步控制台
        let mut console = ConsoleFactory::create_async().await
            .map_err(|e| format!("创建异步控制台失败: {}", e))?;

        let process_handler = handler::StdioHandler::from_config(config.clone());


        // 构建命令
        let (effective_cmd, args) = process_handler.build_command(message, agent_session_id);

        // 统一 cmd /C 包装（Windows）或直接执行（非 Windows）
        #[cfg(target_os = "windows")]
        let (exe, spawn_args): (String, Vec<String>) = {
            let mut a = vec!["/C".to_string(), effective_cmd];
            a.extend(args.iter().cloned());
            ("C:\\Windows\\System32\\cmd.exe".to_string(), a)
        };
        #[cfg(not(target_os = "windows"))]
        let (exe, spawn_args): (String, Vec<String>) = (effective_cmd, args);

        let spawn_args_refs: Vec<&str> = spawn_args.iter().map(|s| s.as_str()).collect();


        // Spawn 进程
        let (pid, mut stdout_rx, mut stderr_rx) = console
            .spawn(&exe, &spawn_args_refs, &cwd).await
            .map_err(|e| {
                format!("启动进程失败: {}", e)
            })?;


        (callbacks.on_pid)(pid);
        log::info!("[Agent/execute_async] async console spawned, pid={}", pid);

        // 注册 PID 到进程管理表
        {
            let aborted = Arc::new(AtomicBool::new(false));
            let aborted_clone = aborted.clone();
            self.processes.lock().unwrap().insert(session_id.clone(), AgentProcess {
                pid: Some(pid),
                aborted: aborted_clone,
            });
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
                        crate::utils::process::TimeoutError::ProcessExited { exit_code, .. } => {
                            log::warn!("[Agent/execute_async] process exited early: code={}", exit_code);
                        }
                        crate::utils::process::TimeoutError::StillAlive { .. } => {
                            log::error!("[Agent/execute_async] max wait exceeded, killing pid={}", pid);
                            let _ = console.kill().await;
                            // 构建 stderr 摘要
                            let _stderr_summary = {
                                let lines = stderr_lines.lock().map(|l| l.clone()).unwrap_or_default();
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
            let recv_timeout = if elapsed >= timeout.max_wait.saturating_sub(timeout.check_interval) {
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

        log::info!("[Agent/execute_async] read loop ended, output_len={}", full_output.len());

        // 等待进程退出
        let exit_code = console.wait().await
            .map_err(|e| format!("异步控制台 wait 失败: {}", e))?;

        let stderr_text = stderr_buf.lock()
            .map(|b| b.clone())
            .unwrap_or_default();

        log::info!("[Agent/execute_async] done: exit_code={}, output_len={}, stderr_len={}",
            exit_code, full_output.len(), stderr_text.len());

        // 异步路径也返回 ExecuteResult（统一出口类型）
        // agent_session_id 已通过 on_session_id 回调推送到前端
        Ok(ExecuteResult {
            stdout: full_output.trim().to_string(),
            exit_code,
            stderr: stderr_text,
        })
    }

    // ------------------------------------------------------------------
    //  高层便捷方法（基于 execute_command 统一入口）
    // ------------------------------------------------------------------

    /// 便捷方法：执行同步命令并返回 stdout
    pub async fn execute_command_output(
        &self, command: &str, cwd: &str, timeout_secs: u64,
        label: &str,
    ) -> Result<String, String> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis()).unwrap_or(0);
        let opts = ExecuteOptions {
            command: command.to_string(),
            cwd: cwd.to_string(),
            session_id: format!("cmd_{}_{}", label, ts),
            source_type: label.to_string(),
            timeout: TimeoutPolicy::custom(timeout_secs, timeout_secs * 10),
        };

        let result = self.execute_command(opts, None, None, None, None, None).await?;
        if result.exit_code != 0 {
            Err(format!("命令执行失败 (exit code {}): {}", result.exit_code, result.stderr))
        } else {
            Ok(result.stdout)
        }
    }

    /// 异步便捷方法：执行命令并返回 stdout（纯 tokio::process，不经过虚拟控制台）
    ///
    /// 用于环境检测、版本查询等短命令场景。
    /// 比 execute_sync 更轻量：无 ConPTY 开销、无 spawn_blocking 线程开销。
    /// 与 execute_async 同路径：tokio 原生异步 I/O。
    pub async fn execute_command_output_async(
        &self,
        command: &str,
        cwd: &str,
        timeout_secs: u64,
        label: &str,
    ) -> Result<String, String> {
        let work_dir = if cwd.is_empty() {
            std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            cwd.to_string()
        };

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis()).unwrap_or(0);
        let session_id = format!("async_cmd_{}_{}", label, ts);

        log::info!("[Agent/execute_command_output_async] session='{}' cmd='{}' cwd='{}'",
            session_id, command, work_dir);

        // Windows: cmd /C 包装
        #[cfg(target_os = "windows")]
        let (exe, args): (&str, Vec<&str>) = ("cmd", vec!["/C", command]);
        #[cfg(not(target_os = "windows"))]
        let (exe, args): (&str, Vec<&str>) = ("sh", vec!["-c", command]);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            async {
                let mut cmd = tokio::process::Command::new(exe);
                cmd.args(&args)
                    .current_dir(&work_dir)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .env_remove("PYTHONHOME")
                    .kill_on_drop(true);

                cmd.output().await
            }
        ).await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    Ok(stdout.trim().to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!("命令执行失败 (exit code {:?}): {}", output.status.code(), stderr.trim()))
                }
            }
            Ok(Err(e)) => {
                Err(format!("启动进程失败: {}", e))
            }
            Err(_) => {
                Err(format!("命令执行超时 ({}s): {}", timeout_secs, command))
            }
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
                let _ = app_for_chunk.emit("agent-chunk", serde_json::json!({
                    "sessionId": sid_for_chunk,
                    "content": chunk,
                }));
            }),
            on_session_id: std::sync::Arc::new(move |sid_agent| {
                let _ = app_for_sid.emit("agent-session", serde_json::json!({
                    "sessionId": sid_for_sid,
                    "agentSessionId": sid_agent,
                }));
            }),
            abort_check: Box::new(move || {
                // 检查 aborted 标志
                let val = aborted_clone.load(Ordering::Relaxed);
                if val {
                    // 额外检查进程表中的 aborted（stop_generation 可能通过 pid 直接 kill）
                    if let Ok(procs) = processes_arc.lock() {
                        if let Some(proc) = procs.get(&*session_for_abort_check) {
                            return proc.aborted.load(Ordering::Relaxed);
                        }
                    }
                }
                val
            }),
            on_pid: Box::new(move |spawned_pid: u32| {
                log::info!("[Agent/send_message] on_pid callback: session={}, pid={}", &*session_for_pid, spawned_pid);
            }),
        };

        let app_for_result = app_handle.clone();
        let sid_for_result = session_id.clone();
        let source_type_for_error = source_type.clone();

        let result = self.execute_command(
            ExecuteOptions {
                command: String::new(), // 异步路径不使用
                cwd: String::new(),
                session_id: session_id.clone(),
                source_type: source_type.clone(),
                timeout: TimeoutPolicy::llm_inference(),
            },
            Some(async_opts),
            Some(callbacks),
            Some(&config),
            Some(&message),
            agent_session_id.as_deref(),
        ).await;

        match result {
            Ok(exec_result) => {
                if exec_result.exit_code != 0 {
                    let err_msg = friendly_agent_error(&source_type_for_error, exec_result.exit_code, &exec_result.stderr);
                    let _ = app_for_result.emit("agent-error", serde_json::json!({
                        "sessionId": sid_for_result,
                        "error": err_msg,
                    }));
                }
            }
            Err(e) => {
                let _ = app_for_result.emit("agent-error", serde_json::json!({
                    "sessionId": sid_for_result,
                    "error": format!("{}", e),
                }));
            }
        }

        let _ = app_for_result.emit("agent-done", serde_json::json!({
            "sessionId": sid_for_result,
        }));

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
                    log::info!("[Agent/execute_once] on_pid callback: session={}, pid={}", session_pid, spawned_pid);
                    let aborted = Arc::new(AtomicBool::new(false));
                    let aborted_clone = aborted.clone();
                    processes_pid.lock().unwrap().insert(session_pid.clone(), AgentProcess {
                        pid: Some(spawned_pid),
                        aborted: aborted_clone,
                    });
                })
            },
        };

        let result = self.execute_command(
            ExecuteOptions {
                command: String::new(),
                cwd: String::new(),
                session_id: _temp_session_id.to_string(),
                source_type: source_type.clone(),
                timeout: TimeoutPolicy::llm_inference(),
            },
            Some(async_opts),
            Some(callbacks),
            Some(config),
            Some(prompt),
            agent_session_id,
        ).await.map_err(|e| AppError::External(e))?;

        if result.exit_code != 0 {
            return Err(AppError::External(friendly_agent_error(
                &source_type, result.exit_code, &result.stderr,
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
                    let _ = std::process::Command::new("taskkill")
                        .args(&["/PID", &pid.to_string(), "/F"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = std::process::Command::new("kill")
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

    pub fn close_session(&mut self, session_id: &str) {
        self.stop_generation(session_id);
        log::info!("[Agent] Session closed: {}", session_id);
    }

    pub async fn list_skills(_agent_type: &str, config: Option<&crate::agent::config::AgentConfig>) -> Vec<crate::db::models::SkillInfo> {
        if let Some(cfg) = config {
            if !cfg.skills_dir.is_empty() {
                let resolved = cfg.skills_dir.replace("{agent_type}", &cfg.agent_type);
                let skills_dir = if resolved.starts_with("~/") {
                    if let Some(home) = home_dir() {
                        home.join(&resolved[2..])
                    } else {
                        std::path::PathBuf::from(&resolved)
                    }
                } else {
                    std::path::PathBuf::from(&resolved)
                };
                return scan_skills_dir(&skills_dir, &cfg.skill_entry_file, &cfg.skill_display_mode);
            }
        }
        vec![]
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
                        let _ = std::process::Command::new("taskkill")
                            .args(&["/PID", &pid.to_string(), "/F"])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn();
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        let _ = std::process::Command::new("kill")
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

/// 获取用户 home 目录（跨平台）
fn home_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

// ------------------------------------------------------------------
//  Skill 解析与扫描
// ------------------------------------------------------------------

fn parse_skill_md(path: &std::path::Path) -> Option<crate::db::models::SkillInfo> {
    let raw = std::fs::read_to_string(path).ok()?;
    let raw = raw.trim();
    if !raw.starts_with("---") {
        return None;
    }
    // 找到第二个 "---"
    let end = raw[3..].find("---")?;
    let frontmatter = &raw[3..3 + end];

    // 用 serde_yaml 解析 frontmatter
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter).ok()?;
    let mapping = value.as_mapping()?;

    let name = mapping.get(&serde_yaml::Value::String("name".into()))?
        .as_str()?.to_string();
    let description = mapping.get(&serde_yaml::Value::String("description".into()))?
        .as_str()?.trim_matches('"').to_string();

        Some(crate::db::models::SkillInfo::new(&name, &description, ""))
}

/// 扫描技能目录
/// display_mode: recursive（递归显示全部）或 collection（只显示集合名）
fn scan_skills_dir(skills_dir: &std::path::Path, entry_file: &str, display_mode: &str) -> Vec<crate::db::models::SkillInfo> {
    if !skills_dir.exists() || !skills_dir.is_dir() {
        return vec![];
    }

    let mut skills = Vec::new();

    // 如果当前目录有入口文件，直接解析并返回
    let own_skill = skills_dir.join(entry_file);
    if own_skill.exists() {
        if let Some(info) = parse_skill_md(&own_skill) {
            skills.push(info);
        }
        // collection 模式：只显示集合名，不递归子目录
        if display_mode == "collection" {
            return skills;
        }
        // recursive 模式：解析入口文件后继续递归子目录
        if display_mode != "recursive" {
            return skills;
        }
    }

    // 递归遍历子目录
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            skills.extend(scan_skills_dir(&path, entry_file, display_mode));
        }
    }
    skills
}
