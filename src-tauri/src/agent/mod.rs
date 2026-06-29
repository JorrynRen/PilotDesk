use std::collections::HashMap;
use crate::utils::paths::resolve_in_path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tauri::Emitter;
use crate::commands::agents::AgentConfig;
use crate::agent::handler::ProcessHandler;
use crate::utils::errors::AppError;

pub mod handler;

// ------------------------------------------------------------------
//  输出解析器
// ------------------------------------------------------------------

#[allow(dead_code)]
fn parse_claude_output(line: &str) -> Option<String> {
    if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
        if event["type"] == "assistant" {
            if let Some(content) = event["message"]["content"].as_array() {
                for block in content {
                    if block["type"] == "text" {
                        if let Some(text) = block["text"].as_str() {
                            return Some(text.to_string() + "\n");
                        }
                    }
                }
            }
        }
    }
    None
}

fn strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_escape = false;
    for c in text.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' || c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn is_content_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() { return false; }
    if trimmed.starts_with("Initializing agent") { return false; }
    if trimmed.starts_with("Resume this session") { return false; }
    if trimmed.starts_with("hermes --resume") { return false; }
    if trimmed.starts_with("Session:") { return false; }
    if trimmed.starts_with("Duration:") { return false; }
    if trimmed.starts_with("Messages:") { return false; }
    if trimmed.starts_with("Query:") { return false; }
    true
}

#[allow(dead_code)]
fn parse_hermes_output(line: &str) -> Option<String> {
    let clean = strip_ansi(line);
    if is_content_line(&clean) {
        Some(clean + "\n")
    } else {
        None
    }
}

#[allow(dead_code)]
fn parse_codex_output(line: &str) -> Option<String> {
    if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
        if event["type"] == "item.completed" {
            if let Some(text) = event["item"]["text"].as_str() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string() + "\n");
                }
            }
        }
        None
    } else {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            Some(trimmed.to_string() + "\n")
        } else {
            None
        }
    }
}

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

/// 共享的进程启动结果
struct SpawnedProcess {
    child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    stderr_buf: Arc<Mutex<String>>,
    agent_type: String,
}

pub struct AgentManager {
    processes: HashMap<String, AgentProcess>,
}

impl AgentManager {
    pub fn new() -> Self {
        Self { processes: HashMap::new() }
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
        let mut child = {
            let mut cmd = Command::new("cmd");
            cmd.arg("/C");
            cmd.arg(&resolved_cmd);
            cmd.args(&args);
            cmd.current_dir(&work_dir);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            cmd.kill_on_drop(true);
            cmd.env_remove("PYTHONHOME");
            cmd.spawn()
                .map_err(|e| format!("启动 {} 失败: {}", agent_type, e))?
        };
        #[cfg(not(target_os = "windows"))]
        let mut child = {
            let mut cmd = Command::new(&resolved_cmd);
            cmd.args(&args);
            cmd.current_dir(&work_dir);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            cmd.kill_on_drop(true);
            cmd.env_remove("PYTHONHOME");
            cmd.spawn()
                .map_err(|e| format!("启动 {} 失败: {}", agent_type, e))?
        };

        let stdout = child.stdout.take()
            .ok_or_else(|| format!("无法获取 {} stdout", agent_type))?;
        let stderr = child.stderr.take()
            .ok_or_else(|| format!("无法获取 {} stderr", agent_type))?;

        let stderr_buf = Arc::new(Mutex::new(String::new()));

        Ok(SpawnedProcess {
            child,
            stdout,
            stderr,
            stderr_buf,
            agent_type,
        })
    }

    // -- 前端会话模式：Event 推送 --

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
        let process_handler = handler::StdioHandler::from_config(config.clone());
        let aborted = Arc::new(AtomicBool::new(false));
        let aborted_clone = aborted.clone();

        let SpawnedProcess { mut child, stdout, stderr, stderr_buf, agent_type } =
            Self::spawn_agent_process(&config, &message, agent_session_id.as_deref(), cwd.as_deref().unwrap_or(""))?;

        let pid = child.id().unwrap_or(0);

        self.processes.insert(session_id.clone(), AgentProcess {
            pid: Some(pid),
            aborted: aborted_clone,
        });

        // 后台收集 stderr（含 session_id 提取）
        let handler_for_stderr = process_handler.clone();
        let app_clone_for_stderr = app_handle.clone();
        let sid_for_stderr = session_id.clone();
        let stderr_buf_for_reader = stderr_buf.clone();
        let agent_type_name_for_stderr = agent_type.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            use tokio::io::AsyncBufReadExt;
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(sid_agent) = handler_for_stderr.extract_session_id(&line, true) {
                    let _ = app_clone_for_stderr.emit("agent-session", serde_json::json!({
                        "sessionId": sid_for_stderr,
                        "agentSessionId": sid_agent,
                    }));
                }
                if let Ok(mut buf) = stderr_buf_for_reader.lock() {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
            if let Ok(buf) = stderr_buf_for_reader.lock() {
                if !buf.is_empty() {
                    log::warn!("[Agent/{}] send_message stderr: {}", agent_type_name_for_stderr, buf.trim());
                }
            }
        });

        // 主任务：读取 stdout（带 300s 超时）+ 等待子进程 + 检查退出码
        let app_clone = app_handle.clone();
        let sid = session_id.clone();
        let agent_type_name2 = agent_type.clone();

        tokio::spawn(async move {
            let timeout_duration = Duration::from_secs(300);

            let read_result = timeout(timeout_duration, async {
                let reader = BufReader::new(stdout);
                use tokio::io::AsyncBufReadExt;
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    if aborted.load(Ordering::Relaxed) { break; }

                    if let Some(sid_agent) = process_handler.extract_session_id(&line, false) {
                        let _ = app_clone.emit("agent-session", serde_json::json!({
                            "sessionId": sid,
                            "agentSessionId": sid_agent,
                        }));
                    }

                    if let Some(content) = process_handler.parse_output_line(&line) {
                        let _ = app_clone.emit("agent-chunk", serde_json::json!({
                            "sessionId": sid,
                            "content": content,
                        }));
                    }
                }
            }).await;

            if read_result.is_err() {
                let _ = app_clone.emit("agent-error", serde_json::json!({
                    "sessionId": sid,
                    "error": "请求超时：智能体未在 300 秒内响应，请检查智能体状态后重试",
                }));
                let _ = app_clone.emit("agent-done", serde_json::json!({
                    "sessionId": sid,
                }));
                return;
            }

            match child.wait().await {
                Ok(status) => {
                    if let Some(code) = status.code() {
                        if code != 0 {
                            let stderr_text = stderr_buf.lock()
                                .map(|b| b.clone())
                                .unwrap_or_default();
                            let err_msg = friendly_agent_error(&agent_type_name2, code, &stderr_text);
                            let _ = app_clone.emit("agent-error", serde_json::json!({
                                "sessionId": sid,
                                "error": err_msg,
                            }));
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[Agent/{}] wait() failed: {}", agent_type_name2, e);
                }
            }

            let _ = app_clone.emit("agent-done", serde_json::json!({
                "sessionId": sid,
            }));
        });

        Ok(())
    }

    // -- 单次执行模式：直接返回完整输出 --

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
        let process_handler = handler::StdioHandler::from_config(config.clone());

        let SpawnedProcess { mut child, stdout, stderr, stderr_buf, agent_type } =
            Self::spawn_agent_process(config, prompt, agent_session_id, cwd)
                .map_err(|e| AppError::External(e))?;

        // 后台收集 stderr（含 session_id 提取）
        let stderr_buf_clone = stderr_buf.clone();
        let agent_type_clone = agent_type.clone();
        let process_handler_for_stderr = process_handler.clone();
        let agent_session_id_shared: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
        let sid_shared = agent_session_id_shared.clone();
        tokio::spawn(async move {
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
                    log::warn!("[Agent/{}] execute_once stderr: {}", agent_type_clone, buf.trim());
                }
            }
        });

        // 读取 stdout
        let reader = BufReader::new(stdout);
        use tokio::io::AsyncBufReadExt;
        let mut lines = reader.lines();
        let mut full_output = String::new();
        let mut agent_session_id: Option<String> = None;

        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(sid) = process_handler.extract_session_id(&line, false) {
                agent_session_id = Some(sid);
            }
            if let Some(content) = process_handler.parse_output_line(&line) {
                on_chunk(content.clone());
                full_output.push_str(&content);
            }
        }

        // 合并 stderr 中提取的 session_id（hermes 的 session_id 来自 stderr）
        if agent_session_id.is_none() {
            if let Ok(sid) = agent_session_id_shared.lock() {
                agent_session_id = sid.clone();
            }
        }

        let status = child.wait().await
            .map_err(|e| AppError::External(format!("等待进程失败: {}", e)))?;

        if let Some(code) = status.code() {
            if code != 0 {
                let stderr_text = stderr_buf.lock()
                    .map(|b| b.clone())
                    .unwrap_or_default();
                let err_msg = friendly_agent_error(&agent_type, code, &stderr_text);
                return Err(AppError::External(err_msg));
            }
        }

        Ok((full_output.trim().to_string(), agent_session_id))
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
