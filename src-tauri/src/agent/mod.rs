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

// ──────────────────────────────────────────────
//  Agent 类型枚举 — 新增 Agent 只需在此添加变体
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    Claude,
    Hermes,
    Codex,
}

impl AgentType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(Self::Claude),
            "hermes" => Some(Self::Hermes),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Hermes => "hermes",
            Self::Codex => "codex",
        }
    }

    pub fn cli_command(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Hermes => "hermes",
            Self::Codex => "codex",
        }
    }

    pub fn build_args(&self, message: &str, _mode: &str, _system_prompt: Option<&str>) -> Vec<String> {
        match self {
            Self::Claude => vec![
                "-p".into(),
                "--verbose".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--dangerously-skip-permissions".into(),
                message.into(),
            ],
            Self::Hermes => vec!["chat".into(), "-q".into(), message.into(), "-Q".into()],
            Self::Codex => vec!["exec".into(), message.into()],
        }
    }

    pub fn parse_output_line(&self, line: &str) -> Option<String> {
        match self {
            Self::Claude => parse_claude_output(line),
            Self::Hermes => parse_hermes_output(line),
            Self::Codex => parse_codex_output(line),
        }
    }

    pub fn friendly_error(&self, exit_code: i32, stderr: &str) -> String {
        friendly_agent_error(self.as_str(), exit_code, stderr)
    }
}

// ──────────────────────────────────────────────
//  输出解析器
// ──────────────────────────────────────────────

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

fn parse_hermes_output(line: &str) -> Option<String> {
    let clean = strip_ansi(line);
    if is_content_line(&clean) {
        Some(clean + "\n")
    } else {
        None
    }
}

fn parse_codex_output(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.is_empty() {
        Some(trimmed.to_string() + "\n")
    } else {
        None
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

    pub async fn send_message(
        &mut self,
        app_handle: tauri::AppHandle,
        session_id: String,
        agent_type: String,
        message: String,
        mode: String,
        cwd: Option<String>,
        system_prompt: Option<String>,
    ) -> Result<(), String> {
        let agent = AgentType::from_str(&agent_type)
            .ok_or_else(|| format!("未知 Agent 类型: {}", agent_type))?;

        let aborted = Arc::new(AtomicBool::new(false));
        let aborted_clone = aborted.clone();

        let args = agent.build_args(&message, &mode, system_prompt.as_deref());
        let cmd_name = agent.cli_command();

        let work_dir = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        // 使用 resolve_in_path 解析完整路径（PATH 解析）
        let resolved_cmd = resolve_in_path(cmd_name)
            .unwrap_or_else(|| cmd_name.to_string());

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
            // 清除 PYTHONHOME 环境变量，防止污染子进程
            cmd.env_remove("PYTHONHOME");
            cmd.spawn()
                .map_err(|e| format!("启动 {} 失败: {}", cmd_name, e))?
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
                .map_err(|e| format!("启动 {} 失败: {}", cmd_name, e))?
        };

        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take()
            .ok_or_else(|| format!("无法获取 {} stdout", cmd_name))?;
        let stderr = child.stderr.take()
            .ok_or_else(|| format!("无法获取 {} stderr", cmd_name))?;

        // 共享 stderr 缓冲区，供 stdout 任务和 stderr 收集任务使用
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let stderr_buf_for_reader = stderr_buf.clone();

        self.processes.insert(session_id.clone(), AgentProcess {
            pid: Some(pid),
            aborted: aborted_clone,
        });

        let app_clone = app_handle.clone();
        let sid = session_id.clone();
        let agent_type_name = agent_type.clone();

        let agent_type_name_for_stderr = agent_type_name.clone();

        // 主任务：读取 stdout（带 300s 超时）+ 等待子进程 + 检查退出码
        tokio::spawn(async move {
            let timeout_duration = Duration::from_secs(300);

            // 带超时的 stdout 读取
            let read_result = timeout(timeout_duration, async {
                let reader = BufReader::new(stdout);
                use tokio::io::AsyncBufReadExt;
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    if aborted.load(Ordering::Relaxed) { break; }
                    if let Some(content) = agent.parse_output_line(&line) {
                        let _ = app_clone.emit("agent-chunk", serde_json::json!({
                            "sessionId": sid,
                            "content": content,
                        }));
                    }
                }
            }).await;

            // 超时处理
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

            // 等待子进程退出并检查退出码
            match child.wait().await {
                Ok(status) => {
                    if let Some(code) = status.code() {
                        if code != 0 {
                            let stderr_text = stderr_buf.lock()
                                .map(|b| b.clone())
                                .unwrap_or_default();
                            let err_msg = agent.friendly_error(code, &stderr_text);
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

    pub async fn list_skills(agent_type: &str) -> Vec<crate::db::models::SkillInfo> {
        // 所有已安装的 agent 都支持技能搜索
        // 有明确定义路径的走定义路径，未定义的走智能目录 ".agent名称/skills/"
        match agent_type {
            "claude" => claude_skills().await,
            "hermes" => hermes_skills().await,
            "codex" => codex_skills().await,
            other => {
                // 智能目录: ~/.{agent_name}/skills/
                if let Some(home) = home_dir() {
                    let skills_dir = home.join(format!(".{}", other)).join("skills");
                    scan_skills_dir(&skills_dir)
                } else {
                    vec![]
                }
            }
        }
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
pub fn detect_installed_agents() -> Vec<String> {
    let agents = ["claude", "hermes", "codex"];
    let mut installed = Vec::new();
    for name in &agents {
        if which(name) {
            installed.push(name.to_string());
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

/// 递归扫描目录，遇到包含 SKILL.md 的目录即停止向下递归
fn scan_skills_dir(skills_dir: &std::path::Path) -> Vec<SkillInfo> {
    if !skills_dir.exists() || !skills_dir.is_dir() {
        return vec![];
    }

    let mut skills = Vec::new();

    // 如果当前目录有 SKILL.md，直接解析并返回（不递归子目录）
    let own_skill = skills_dir.join("SKILL.md");
    if own_skill.exists() {
        if let Some(info) = parse_skill_md(&own_skill) {
            skills.push(info);
        }
        return skills;
    }

    // 否则递归遍历子目录
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            skills.extend(scan_skills_dir(&path));
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

async fn claude_skills() -> Vec<SkillInfo> {
    let home = match home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let skills_dir = home.join(".claude").join("skills");
    scan_skills_dir(&skills_dir)
}

async fn hermes_skills() -> Vec<SkillInfo> {
    let home = match home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    // 优先 ~/AppData/Local/hermes/skills/，回退 ~/.hermes/skills/
    let primary = home.join("AppData").join("Local").join("hermes").join("skills");
    let skills = scan_skills_dir(&primary);
    if !skills.is_empty() {
        return skills;
    }
    let fallback = home.join(".hermes").join("skills");
    scan_skills_dir(&fallback)
}

async fn codex_skills() -> Vec<SkillInfo> {
    let home = match home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let skills_dir = home.join(".codex").join("skills");
    scan_skills_dir(&skills_dir)
}
