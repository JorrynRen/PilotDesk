use std::collections::HashMap;
use crate::utils::paths::resolve_in_path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::io::BufReader;
use tokio::process::Command;
use tauri::Emitter;
use crate::commands::agents::AgentConfig;
use crate::agent::handler::ProcessHandler;
use crate::utils::errors::AppError;
use crate::virtual_console::factory::ConsoleFactory;
use crate::virtual_console::traits::VirtualConsole;
use crate::agent::console_bridge::{AgentConsoleBridge, CommandKind};

pub mod handler;
pub mod line_processor;
pub mod console_bridge;

#[cfg(target_os = "windows")]
pub mod session_process;
#[cfg(target_os = "windows")]
pub use session_process::ConptyProcess;

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
//  进程管理
// ------------------------------------------------------------------

struct AgentProcess {
    pid: Option<u32>,
    aborted: Arc<AtomicBool>,
}

/// Unified process wrapper: tokio Child or ConPTY process
enum ProcessChild {
    Tokio(tokio::process::Child),
    #[cfg(target_os = "windows")]
    Conpty(ConptyProcess),
}

impl ProcessChild {
    async fn wait(&mut self) -> Result<i32, String> {
        match self {
            ProcessChild::Tokio(child) => {
                let status = child.wait().await
                    .map_err(|e| format!("Wait failed: {}", e))?;
                Ok(status.code().unwrap_or(-1))
            }
            ProcessChild::Conpty(conpty) => conpty.wait().await,
        }
    }
    fn id(&self) -> u32 {
        match self {
            ProcessChild::Tokio(child) => child.id().unwrap_or(0),
            ProcessChild::Conpty(conpty) => conpty.id(),
        }
    }
    fn send_eof(&self) {
        if let ProcessChild::Conpty(conpty) = self {
            conpty.send_eof();
        }
    }

    fn close_stdout_pipe(&self) {
        if let ProcessChild::Conpty(conpty) = self {
            conpty.close_stdout_pipe();
        }
    }
}

/// Shared process spawn result
struct SpawnedProcess {
    child: ProcessChild,
    stdout: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
    stderr: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
    stderr_buf: Arc<Mutex<String>>,
    agent_type: String,
    conpty_mode: bool,
}

pub struct AgentManager {
    processes: HashMap<String, AgentProcess>,
    /// 虚拟控制台桥接：Agent 会话终端状态跟踪器
    console_bridge: Arc<std::sync::Mutex<AgentConsoleBridge>>,
}

impl AgentManager {
    pub fn new() -> Self {
        Self {
            processes: HashMap::new(),
            console_bridge: Arc::new(std::sync::Mutex::new(AgentConsoleBridge::new())),
        }
    }

    /// 获取虚拟控制台桥接的 Arc clone
    pub fn console_bridge_clone(&self) -> Arc<std::sync::Mutex<AgentConsoleBridge>> {
        Arc::clone(&self.console_bridge)
    }

    /// 通过锁获取虚拟控制台桥接的可变引用
    pub fn with_console_bridge<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut AgentConsoleBridge) -> R,
    {
        let mut bridge = self.console_bridge.lock().unwrap();
        f(&mut bridge)
    }

    // ------------------------------------------------------------------
    //  统一命令执行 — 所有 Agent 管理操作的统一入口
    // ------------------------------------------------------------------

    /// 统一命令执行入口（智能超时等待）
    ///
    /// 所有 Agent 管理命令（版本查询、安装、卸载、更新检查等）
    /// 均通过此方法，经由虚拟控制台桥接跟踪。
    ///
    /// 超时机制：
    /// - 每隔 timeout_secs 检查一次执行结果
    /// - 超时后检查子进程 PID 存活性
    ///   - 存活 -> 假超时（进程仍在工作），继续等待
    ///   - 不存活 -> 真超时（进程已退出），报错
    /// - 最大等待上限 = timeout_secs * 10
    pub async fn execute_command(
        &self,
        command: &str,
        cwd: &str,
        timeout_secs: u64,
        command_kind: CommandKind,
        label: &str,
    ) -> Result<(String, i32, String), String> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis()).unwrap_or(0);
        let session_id = format!("cmd_{}_{}", label, ts);
        let work_dir = if cwd.is_empty() {
            std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
        } else { cwd.to_string() };

        let bridge_arc = Arc::clone(&self.console_bridge);
        let bridge_sid = session_id.clone();
        {
            let mut bridge = self.console_bridge.lock().unwrap();
            bridge.register_session(&session_id, label, 0, command, vec![], &work_dir, false, command_kind.clone());
        }

        log::info!("[Agent/execute_command] kind={:?} label='{}' cmd='{}' cwd='{}'", command_kind, label, command, work_dir);

        let cmd_owned = command.to_string();
        let work_dir_owned = work_dir.clone();
        let shared_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let shared_pid_clone = Arc::clone(&shared_pid);
        let (tx, rx) = std::sync::mpsc::channel::<(u32, Result<(String, i32, String), String>)>();

        // 在独立线程中通过虚拟控制台同步执行命令（匹配 plugin/shell.rs 模式）
        std::thread::spawn(move || {
            let result = (|| -> Result<(String, i32, String), String> {
                let mut console = ConsoleFactory::auto_create()
                    .map_err(|e| format!("创建虚拟控制台失败: {}", e))?;

                #[cfg(target_os = "windows")]
                let (exe, args): (&str, &[&str]) = ("cmd", &["/C", &cmd_owned]);
                #[cfg(not(target_os = "windows"))]
                let (exe, args): (&str, &[&str]) = ("sh", &["-c", &cmd_owned]);

                let mut handle = console.spawn(exe, args, &work_dir_owned)
                    .map_err(|e| format!("虚拟控制台 spawn 失败: {}", e))?;

                let pid = handle.pid;
                shared_pid_clone.store(pid, std::sync::atomic::Ordering::Relaxed);
                log::info!("[Agent/execute_command] console spawned: pid={} cmd='{}'", pid, cmd_owned);

                let mut stdout_bytes = Vec::new();
                { let mut buf = [0u8; 4096]; loop {
                    match handle.stdout.read(&mut buf) {
                        Ok(0) => break, Ok(n) => stdout_bytes.extend_from_slice(&buf[..n]),
                        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }}

                let mut stderr_bytes = Vec::new();
                { let mut buf = [0u8; 4096]; loop {
                    match handle.stderr.read(&mut buf) {
                        Ok(0) => break, Ok(n) => stderr_bytes.extend_from_slice(&buf[..n]),
                        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }}

                let _ = handle.stdin.close();
                let status = console.wait(None).map_err(|e| format!("虚拟控制台 wait 失败: {}", e))?;
                let exit_code = status.code().unwrap_or(-1);

                let stdout_text = String::from_utf8_lossy(&stdout_bytes).trim().to_string();
                let stderr_text = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

                Ok((stdout_text, exit_code, stderr_text))
            })();

            let pid = shared_pid_clone.load(std::sync::atomic::Ordering::Relaxed);
            let _ = tx.send((pid, result));
        });

        // 智能超时：使用 process.rs 统一策略
        let pid = shared_pid.load(std::sync::atomic::Ordering::Relaxed);
        match crate::utils::process::wait_channel_with_alive(
            pid,
            &rx,
            &format!("execute_command:{}", command),
        ) {
            Ok((_pid, Ok((stdout, exit_code, stderr)))) => {
                let mut bridge = bridge_arc.lock().unwrap();
                if exit_code == 0 { bridge.mark_completed(&bridge_sid); }
                else { bridge.mark_failed(&bridge_sid); }
                bridge.unregister_session(&bridge_sid);
                return Ok((stdout, exit_code, stderr));
            }
            Ok((_pid, Err(e))) => {
                let mut bridge = bridge_arc.lock().unwrap();
                bridge.mark_failed(&bridge_sid);
                bridge.unregister_session(&bridge_sid);
                return Err(e);
            }
            Err(e) => {
                let mut bridge = bridge_arc.lock().unwrap();
                bridge.mark_terminated(&bridge_sid);
                bridge.unregister_session(&bridge_sid);
                return Err(format!("{}", e));
            }
        }
    }

    /// 便捷方法：执行命令并返回 stdout
    pub async fn execute_command_output(
        &self, command: &str, cwd: &str, timeout_secs: u64,
        command_kind: CommandKind, label: &str,
    ) -> Result<String, String> {
        let (stdout, exit_code, stderr) = self.execute_command(command, cwd, timeout_secs, command_kind, label).await?;
        if exit_code != 0 {
            Err(format!("命令执行失败 (exit code {}): {}", exit_code, stderr))
        } else { Ok(stdout) }
    }

// -- 共享方法：构建命令 -> 启动进程 -> 返回管道 --

    fn spawn_agent_process(
        config: &AgentConfig,
        message: &str,
        agent_session_id: Option<&str>,
        cwd: &str,
    ) -> Result<SpawnedProcess, String> {
        let process_handler = handler::StdioHandler::from_config(config.clone());
        let agent_type = config.agent_type.clone();
        let cmd_name = config.cli_command.clone();

        let (cmd_name_from_template, args) = process_handler.build_command(message, agent_session_id);
        let effective_cmd = if cmd_name_from_template.is_empty() {
            cmd_name
        } else {
            cmd_name_from_template
        };

        let work_dir = if cwd.is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            cwd.to_string()
        };

        let resolved_cmd = resolve_in_path(&effective_cmd)
            .unwrap_or_else(|| effective_cmd.to_string());

        #[cfg(target_os = "windows")]
        let (child, stdout, stderr, conpty_mode) = {
            // Detect if resolved_cmd is a batch script (.cmd / .bat)
            let is_batch = resolved_cmd.to_lowercase().ends_with(".cmd")
                || resolved_cmd.to_lowercase().ends_with(".bat");

            let cmdline = if is_batch {
                // Batch scripts require cmd.exe /C because CreateProcessW + ConPTY
                // cannot delegate .cmd→cmd.exe (ERROR_BAD_EXE_FORMAT / os error 193).
                // Also needed for fallback Command::new() which doesn't auto-resolve .cmd.
                let mut c = String::from("cmd /C ");
                c.push_str(&escape_cmdline_arg(&resolved_cmd));
                for arg in &args {
                    c.push(' ');
                    c.push_str(&escape_cmdline_arg(arg));
                }
                c
            } else {
                // Native executables (.exe): use resolved_cmd directly.
                // CreateProcessW handles PATH resolution internally.
                let mut c = String::new();
                c.push_str(&escape_cmdline_arg(&resolved_cmd));
                for arg in &args {
                    c.push(' ');
                    c.push_str(&escape_cmdline_arg(arg));
                }
                c
            };

            match crate::agent::session_process::spawn_with_conpty(&cmdline, &work_dir, 80, 30) {
                Ok((c, o, e, cm)) => (
                    ProcessChild::Conpty(c),
                    Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                    Box::new(e) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                    cm,
                ),
                Err(e) => {
                    log::warn!("[Agent/{}] ConPTY failed, fallback: {}", agent_type, e);
                    let mut cmd = if is_batch {
                        // Batch scripts need cmd /C even in fallback
                        let mut c = Command::new("C:\\Windows\\System32\\cmd.exe");
                        c.arg("/C");
                        c.arg(&resolved_cmd);
                        c.args(&args);
                        c
                    } else {
                        let mut cmd = Command::new(&resolved_cmd);
                        cmd.args(&args);
                        cmd
                    };
                    cmd.current_dir(&work_dir);
                    cmd.stdout(std::process::Stdio::piped());
                    cmd.stderr(std::process::Stdio::piped());
                    cmd.kill_on_drop(true);
                    cmd.env_remove("PYTHONHOME");
                    let mut spawned = cmd.spawn()
                        .map_err(|e| format!("启动 {} 失败: {}", agent_type, e))?;
                    let o = spawned.stdout.take()
                        .ok_or_else(|| format!("无法获取 {} stdout", agent_type))?;
                    let e = spawned.stderr.take()
                        .ok_or_else(|| format!("无法获取 {} stderr", agent_type))?;
                    (
                        ProcessChild::Tokio(spawned),
                        Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                        Box::new(e) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                        false,
                    )
                }
            }
        };

        #[cfg(not(target_os = "windows"))]
        let (child, stdout, stderr, conpty_mode) = {
            let mut cmd = Command::new(&resolved_cmd);
            cmd.args(&args);
            cmd.current_dir(&work_dir);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            cmd.kill_on_drop(true);
            cmd.env_remove("PYTHONHOME");
            let mut spawned = cmd.spawn()
                .map_err(|e| format!("启动 {} 失败: {}", agent_type, e))?;
            let o = spawned.stdout.take()
                .ok_or_else(|| format!("无法获取 {} stdout", agent_type))?;
            let e = spawned.stderr.take()
                .ok_or_else(|| format!("无法获取 {} stderr", agent_type))?;
            (
                ProcessChild::Tokio(spawned),
                Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                Box::new(e) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                false,
            )
        };

        let stderr_buf = Arc::new(Mutex::new(String::new()));

        Ok(SpawnedProcess {
            child,
            stdout,
            stderr,
            stderr_buf,
            agent_type,
            conpty_mode,
        })
    }

    // -- 核心 IO 循环：启动进程 → 读取 stdout/stderr → 提取 session_id --
    // agent 模块的权威实现，send_message_with_config 和 execute_once 均基于此

    /// 内部 IO 执行 — 启动 Agent 进程，收集 stdout/stderr，提取 session_id
    ///
    /// 返回: (完整输出文本, agent_session_id, 退出码, stderr文本)
    /// 这是 agent 模块的核心 IO 逻辑，两个公开入口方法均调用此函数。
    async fn execute_agent_inner(
        config: &AgentConfig,
        prompt: &str,
        agent_session_id: Option<&str>,
        cwd: &str,
        on_chunk: impl Fn(String) + Send + 'static,
        on_session_id: impl Fn(String) + Send + 'static,
        abort_check: impl Fn() -> bool + Send + 'static,
    ) -> Result<(String, Option<String>, i32, String), String> {
        let process_handler = handler::StdioHandler::from_config(config.clone());

        let SpawnedProcess { mut child, stdout, stderr, stderr_buf, agent_type, conpty_mode } =
            Self::spawn_agent_process(config, prompt, agent_session_id, cwd)?;

        use tokio::io::AsyncBufReadExt;
        let mut full_output = String::new();
        let mut agent_session_id_result: Option<String> = None;

        if conpty_mode {
            // ConPTY merged mode: stdout+stderr in same pipe
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if abort_check() { break; }
                if let Some(ref sid) = process_handler.extract_session_id(&line, false) {
                    agent_session_id_result = Some(sid.clone());
                    on_session_id(sid.clone());
                } else if let Some(ref sid) = process_handler.extract_session_id(&line, true) {
                    agent_session_id_result = Some(sid.clone());
                    on_session_id(sid.clone());
                }
                if let Some(content) = process_handler.parse_output_line(&line) {
                    on_chunk(content.clone());
                    full_output.push_str(&content);
                }
            }
            // After reading all output, send EOF (Ctrl+D) to terminate interactive CLI tools
            // (e.g., hermes chat stays in interactive mode after processing --query)
            child.send_eof();
            drop(stderr);
        } else {
            // Stdio mode: stderr in background task
            let stderr_buf_clone = stderr_buf.clone();
            let agent_type_clone = agent_type.clone();
            let process_handler_for_stderr = process_handler.clone();
            let agent_session_id_shared: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
            let sid_shared = agent_session_id_shared.clone();
            let stderr_handle = tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                use tokio::io::AsyncBufReadExt;
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(sid) = process_handler_for_stderr.extract_session_id(&line, true) {
                        if let Ok(mut s) = sid_shared.lock() {
                            *s = Some(sid);
                        }
                    }
                    if let Ok(mut buf) = stderr_buf_clone.lock() {
                        buf.push_str(&line);
                        buf.push('\n');
                    }
                }
                if let Ok(buf) = stderr_buf_clone.lock() {
                    if !buf.is_empty() {
                        log::warn!("[Agent/{}] stderr: {}", agent_type_clone, buf.trim());
                    }
                }
            });

            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if abort_check() { break; }
                if let Some(ref sid) = process_handler.extract_session_id(&line, false) {
                    agent_session_id_result = Some(sid.clone());
                    on_session_id(sid.clone());
                }
                if let Some(content) = process_handler.parse_output_line(&line) {
                    on_chunk(content.clone());
                    full_output.push_str(&content);
                }
            }

            // Wait for stderr background task
            if let Err(e) = stderr_handle.await {
                log::warn!("[Agent/{}] stderr task join error: {:?}", agent_type, e);
            }

            // Extract session_id from stderr
            if agent_session_id_result.is_none() {
                if let Ok(sid) = agent_session_id_shared.lock() {
                    if let Some(s) = sid.clone() {
                        agent_session_id_result = Some(s.clone());
                        on_session_id(s);
                    }
                }
            }
        } // end else (stdio mode)

        log::info!("[ConPTY/IO] read loop ended, full_output_len={}, conpty_mode={}",
            full_output.len(), conpty_mode);

        let exit_code = child.wait().await?;

        log::info!("[Agent/{}] execute_agent_inner done: exit_code={}, conpty_mode={}, output_len={}, stderr_len={}",
            agent_type, exit_code, conpty_mode, full_output.len(), stderr_buf.lock().map(|b| b.len()).unwrap_or(0));

        let stderr_text = stderr_buf.lock()
            .map(|b| b.clone())
            .unwrap_or_default();

        Ok((full_output.trim().to_string(), agent_session_id_result, exit_code, stderr_text))
    }

    // -- 前端会话模式：Event 推送（基于 execute_agent_inner） --

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
        let aborted = Arc::new(AtomicBool::new(false));
        let aborted_clone = aborted.clone();

        let pid = {
            let SpawnedProcess { child, .. } =
                Self::spawn_agent_process(&config, &message, agent_session_id.as_deref(), cwd.as_deref().unwrap_or(""))?;
            child.id()
        };

        self.processes.insert(session_id.clone(), AgentProcess {
            pid: Some(pid),
            aborted: aborted_clone,
        });

        // 后台任务：调用共享 IO 循环，通过 Event 推送结果
        let app_clone = app_handle.clone();
        let sid = session_id.clone();
        let agent_type_name = config.agent_type.clone();
        let aborted_clone = aborted.clone();
        let config_owned = config.clone();
        let message_owned = message.clone();
        let agent_session_id_owned = agent_session_id.clone();
        let cwd_owned = cwd.clone();

        tokio::spawn(async move {
            let app = app_clone;
            let sid_inner = sid;
            let agent_type_name_inner = agent_type_name;
            let app_for_result = app.clone();
            let sid_inner_for_result = sid_inner.clone();
            let aborted_inner = aborted_clone;

            let result = crate::utils::process::wait_future_with_alive(
                pid,
                Box::pin(async move {
                    let app_for_chunk = app.clone();
                    let sid_for_chunk = sid_inner.clone();
                    let app_for_sid = app.clone();
                    let sid_for_sid = sid_inner.clone();

                    Self::execute_agent_inner(
                        &config_owned,
                        &message_owned,
                        agent_session_id_owned.as_deref(),
                        cwd_owned.as_deref().unwrap_or(""),
                        move |chunk| {
                            let _ = app_for_chunk.emit("agent-chunk", serde_json::json!({
                                "sessionId": sid_for_chunk,
                                "content": chunk,
                            }));
                        },
                        move |sid_agent| {
                            let _ = app_for_sid.emit("agent-session", serde_json::json!({
                                "sessionId": sid_for_sid,
                                "agentSessionId": sid_agent,
                            }));
                        },
                        move || aborted_inner.load(Ordering::Relaxed),
                    ).await
                }),
                &format!("Agent {}", agent_type_name_inner),
            ).await;

            match result {
                Ok((_output, _sid_from_inner, exit_code, stderr_text)) => {
                    if exit_code != 0 {
                        let err_msg = friendly_agent_error(&agent_type_name_inner, exit_code, &stderr_text);
                        let _ = app_for_result.emit("agent-error", serde_json::json!({
                            "sessionId": sid_inner_for_result,
                            "error": err_msg,
                        }));
                    }
                }
                Err(e) => {
                    let _ = app_for_result.emit("agent-error", serde_json::json!({
                        "sessionId": sid_inner_for_result,
                        "error": format!("{}", e),
                    }));
                }
            }

            let _ = app_for_result.emit("agent-done", serde_json::json!({
                "sessionId": sid_inner_for_result,
            }));
        });

        Ok(())
    }

    // -- 单次执行模式：直接返回完整输出（基于 execute_agent_inner） --

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
        // ── 智能超时策略（与 send_message_with_config 一致）──
        // 先 spawn 获取 PID，再通过 wait_future_with_alive 包裹 execute_agent_inner
        // 30s 轮询 + 10min 上限，通过 is_process_alive 区分假超时和真超时
        let pid = {
            let spawned = Self::spawn_agent_process(config, prompt, agent_session_id, cwd)
                .map_err(|e| AppError::External(e))?;
            spawned.child.id()
        };

        // 为 async 'static 闭包准备 owned 副本
        let config_owned = config.clone();
        let prompt_owned = prompt.to_string();
        let cwd_owned = cwd.to_string();
        let agent_session_id_owned = agent_session_id.map(|s| s.to_string());
        let agent_type_label = config.agent_type.clone();
        let agent_type_for_error = config.agent_type.clone();

        let timeout_result = crate::utils::process::wait_future_with_alive(
            pid,
            Box::pin(async move {
                Self::execute_agent_inner(
                    &config_owned,
                    &prompt_owned,
                    agent_session_id_owned.as_deref(),
                    &cwd_owned,
                    on_chunk,
                    |_| {},
                    || false,
                ).await
            }),
            &format!("Agent(workflow) {}", agent_type_label),
        ).await;

        match timeout_result {
            Ok((_output, sid, exit_code, stderr_text)) => {
                if exit_code != 0 {
                    return Err(AppError::External(friendly_agent_error(
                        &agent_type_for_error, exit_code, &stderr_text,
                    )));
                }
                Ok((_output, sid))
            }
            Err(e) => Err(AppError::External(format!("{}", e))),
        }
    }
    pub fn stop_generation(&mut self, session_id: &str) {
        if let Some(process) = self.processes.get(session_id) {
            process.aborted.store(true, Ordering::Relaxed);
        }
        if let Some(process) = self.processes.get_mut(session_id) {
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
        self.processes.remove(session_id);
        log::info!("[Agent] Generation stopped: {}", session_id);
    }

    pub fn create_session(&mut self, session_id: &str, _agent_type: &str, _cwd: Option<&str>) {
        log::info!("[Agent] Session created: {}", session_id);
    }

    pub fn close_session(&mut self, session_id: &str) {
        self.stop_generation(session_id);
        log::info!("[Agent] Session closed: {}", session_id);
    }

    pub async fn list_skills(_agent_type: &str, config: Option<&crate::commands::agents::AgentConfig>) -> Vec<crate::db::models::SkillInfo> {
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
        for (_, process) in self.processes.drain() {
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

/// 获取用户 home 目录（跨平台）
fn home_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

// ------------------------------------------------------------------
//  已安装 Agent 检测
// ------------------------------------------------------------------

pub fn detect_installed_agents(agents: &[crate::commands::agents::AgentConfig]) -> Vec<String> {
    let mut installed = Vec::new();
    for agent in agents {
        if !agent.is_enabled {
            continue;
        }
        if which(&agent.cli_command) {
            installed.push(agent.agent_type.clone());
        }
    }
    installed
}

fn which(name: &str) -> bool {
    if let Ok(paths) = std::env::var("PATH") {
        for dir in std::env::split_paths(&paths) {
            let exe = dir.join(name);
            if exe.with_extension("exe").exists() || exe.with_extension("cmd").exists() || exe.exists() {
                return true;
            }
            if exe.with_extension("CMD").exists() {
                return true;
            }
        }
    }
    false
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

/// 转义命令行参数（用于 ConPTY cmdline 构建）
fn escape_cmdline_arg(s: &str) -> String {
    let needs_quote = s.is_empty()
        || s.contains(' ')
        || s.contains('\t')
        || s.contains('\n')
        || s.contains('"');
    if needs_quote {
        let mut escaped = String::with_capacity(s.len() + 2);
        escaped.push('"');
        for c in s.chars() {
            match c {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                _ => escaped.push(c),
            }
        }
        escaped.push('"');
        escaped
    } else {
        s.to_string()
    }
}
