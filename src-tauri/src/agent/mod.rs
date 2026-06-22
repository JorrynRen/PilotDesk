use std::collections::HashMap;
use crate::db::models::SkillInfo;
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



// ──────────────────────────────────────────────
//  输出解析器
// ──────────────────────────────────────────────

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
    // Codex --json mode outputs JSONL events
    if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
        // Extract text from item.completed events
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
        // Fallback: plain text output
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            Some(trimmed.to_string() + "\n")
        } else {
            None
        }
    }
}

// ──────────────────────────────────────────────
//  统一错误映射
// ──────────────────────────────────────────────

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

// ──────────────────────────────────────────────
//  进程管理
// ──────────────────────────────────────────────

struct AgentProcess {
    pid: Option<u32>,
    aborted: Arc<AtomicBool>,
}

pub struct AgentManager {
    processes: HashMap<String, AgentProcess>,
}

impl AgentManager {
    pub fn new() -> Self {
        Self { processes: HashMap::new() }
    }

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
        let process_handler = handler::StdioHandler::from_config(config);
        let agent_type = process_handler.config.agent_type.clone();
        let cmd_name = process_handler.config.cli_command.clone();

        let aborted = Arc::new(AtomicBool::new(false));
        let aborted_clone = aborted.clone();

        let (cmd_name_from_template, args) = process_handler.build_command(&message, agent_session_id.as_deref());
        // Use the command name from template if available, otherwise fallback to cli_command
        let effective_cmd = if cmd_name_from_template.is_empty() {
            cmd_name.clone()
        } else {
            cmd_name_from_template
        };

        let work_dir = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        // 使用 resolve_in_path 解析完整路径（PATH 解析）
        let resolved_cmd = resolve_in_path(&effective_cmd)
            .unwrap_or_else(|| effective_cmd.to_string());

        // Windows: .cmd/.bat 文件需要 cmd /C 包装
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

        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take()
            .ok_or_else(|| format!("无法获取 {} stdout", agent_type))?;
        let stderr = child.stderr.take()
            .ok_or_else(|| format!("无法获取 {} stderr", agent_type))?;

        // 共享 stderr 缓冲区
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let stderr_buf_for_reader = stderr_buf.clone();

        self.processes.insert(session_id.clone(), AgentProcess {
            pid: Some(pid),
            aborted: aborted_clone,
        });

        let handler_for_stderr = process_handler.clone();
        let app_clone = app_handle.clone();
        let sid = session_id.clone();
        let agent_type_name = agent_type.clone();

        let agent_type_name_for_stderr = agent_type_name.clone();
        let app_clone_for_stderr = app_handle.clone();
        let sid_for_stderr = session_id.clone();

        // 主任务：读取 stdout（带 300s 超时）+ 等待子进程 + 检查退出码
        tokio::spawn(async move {
            let timeout_duration = Duration::from_secs(300);

            let read_result = timeout(timeout_duration, async {
                let reader = BufReader::new(stdout);
                use tokio::io::AsyncBufReadExt;
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    if aborted.load(Ordering::Relaxed) { break; }

                    // 使用 handler 提取 session_id（从 stdout）
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
                            let err_msg = friendly_agent_error(&agent_type_name, code, &stderr_text);
                            let _ = app_clone.emit("agent-error", serde_json::json!({
                                "sessionId": sid,
                                "error": err_msg,
                            }));
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[Agent/{}] wait() failed: {}", agent_type_name, e);
                }
            }

            let _ = app_clone.emit("agent-done", serde_json::json!({
                "sessionId": sid,
            }));
        });

        // 后台收集 stderr
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            use tokio::io::AsyncBufReadExt;
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // 使用 handler 提取 session_id（从 stderr）
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
                    log::warn!("[Agent/{}] stderr: {}", agent_type_name_for_stderr, buf.trim());
                }
            }
        });

        Ok(())
    }

    /// 单次执行模式：启动 Agent → 发送消息 → 收集输出 → 返回
    /// 与 send_message_with_config 的区别：
    /// - 不依赖前端 Tauri Event 流式推送，直接返回完整输出
    /// - 每次启动新进程（不使用 --resume）
    /// - 提取 agent_session_id 用于调试追溯
    pub async fn execute_once(
        &mut self,
        agent_type: &str,
        prompt: &str,
        _params: &serde_json::Value,
        cwd: &str,
        _temp_session_id: &str,
        on_chunk: impl Fn(String) + Send + 'static,
    ) -> Result<(String, Option<String>), AppError> {
        // 从 DB 读取 Agent 配置
        let config = crate::commands::agents::get_agent_config_by_type(agent_type)
            .unwrap_or_default();
        let process_handler = handler::StdioHandler::from_config(config);
        let cmd_name = process_handler.config.cli_command.clone();

        let (cmd_name_from_template, args) = process_handler.build_command(prompt, None);
        let effective_cmd = if cmd_name_from_template.is_empty() {
            cmd_name.clone()
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
            let mut cmd = tokio::process::Command::new("cmd");
            cmd.arg("/C");
            cmd.arg(&resolved_cmd);
            cmd.args(&args);
            cmd.current_dir(&work_dir);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            cmd.kill_on_drop(true);
            cmd.env_remove("PYTHONHOME");
            cmd.spawn()
                .map_err(|e| AppError::External(format!("启动 {} 失败: {}", agent_type, e)))?
        };
        #[cfg(not(target_os = "windows"))]
        let mut child = {
            let mut cmd = tokio::process::Command::new(&resolved_cmd);
            cmd.args(&args);
            cmd.current_dir(&work_dir);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            cmd.kill_on_drop(true);
            cmd.env_remove("PYTHONHOME");
            cmd.spawn()
                .map_err(|e| AppError::External(format!("启动 {} 失败: {}", agent_type, e)))?
        };

        let stdout = child.stdout.take()
            .ok_or_else(|| AppError::External("无法获取 stdout".into()))?;
        let stderr = child.stderr.take()
            .ok_or_else(|| AppError::External("无法获取 stderr".into()))?;

        // 收集 stderr
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let stderr_buf_clone = stderr_buf.clone();
        let agent_type_clone = agent_type.to_string();

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            use tokio::io::AsyncBufReadExt;
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
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
            // 提取 agent_session_id
            if let Some(sid) = process_handler.extract_session_id(&line, false) {
                agent_session_id = Some(sid);
            }
            if let Some(content) = process_handler.parse_output_line(&line) {
                on_chunk(content.clone());
                full_output.push_str(&content);
            }
        }

        // 等待进程退出
        let status = child.wait().await
            .map_err(|e| AppError::External(format!("等待进程失败: {}", e)))?;

        if let Some(code) = status.code() {
            if code != 0 {
                let stderr_text = stderr_buf.lock()
                    .map(|b| b.clone())
                    .unwrap_or_default();
                let err_msg = friendly_agent_error(agent_type, code, &stderr_text);
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
                // 通过 PID 强制终止进程
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
        // 仅使用 DB 中配置的技能目录，无硬编码 fallback
        if let Some(cfg) = config {
            if !cfg.skills_dir.is_empty() {
                // 有明确定义的技能目录路径（支持 {agent_type} 占位符和 ~ 扩展）
                let resolved = cfg.skills_dir.replace("{agent_type}", &cfg.agent_type);
                let skills_dir = if resolved.starts_with("~/") {
                    // 展开 ~ 为用户 home 目录
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
        for (sid, process) in self.processes.drain() {
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
            log::info!("[Agent] Cleaned up session: {}", sid);
        }
    }
}

// ──────────────────────────────────────────────
//  已安装 Agent 检测
// ──────────────────────────────────────────────

/// 检测系统上已安装的 Agent（通过 PATH 查找 CLI 命令）
/// 从 DB 读取 agents 列表，动态检测
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
    // Check PATH
    if let Ok(paths) = std::env::var("PATH") {
        for dir in std::env::split_paths(&paths) {
            let exe = dir.join(name);
            if exe.with_extension("exe").exists() || exe.with_extension("cmd").exists() || exe.exists() {
                return true;
            }
            // Also check with .CMD on Windows
            if exe.with_extension("CMD").exists() {
                return true;
            }
        }
    }
    false
}

// ──────────────────────────────────────────────
//  技能列表 — 从 SKILL.md 文件解析
// ──────────────────────────────────────────────

/// 从 SKILL.md 文件的 YAML frontmatter 中解析 name 和 description
fn parse_skill_md(path: &std::path::Path) -> Option<SkillInfo> {
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

    Some(SkillInfo::new(&name, &description, ""))
}

/// 扫描技能目录
/// display_mode: recursive（递归显示全部）或 collection（只显示集合名）
fn scan_skills_dir(skills_dir: &std::path::Path, entry_file: &str, display_mode: &str) -> Vec<SkillInfo> {
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

/// 获取用户主目录
fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(std::path::PathBuf::from)
}


