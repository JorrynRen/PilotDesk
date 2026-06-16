use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
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
    child: Option<Child>,
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

        let mut child = Command::new(cmd_name)
            .args(&args)
            .cwd(&work_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("启动 {} 失败: {}", cmd_name, e))?;

        let stdout = child.stdout.take()
            .ok_or_else(|| format!("无法获取 {} stdout", cmd_name))?;
        let stderr = child.stderr.take()
            .ok_or_else(|| format!("无法获取 {} stderr", cmd_name))?;

        self.processes.insert(session_id.clone(), AgentProcess {
            child: Some(child),
            aborted: aborted_clone,
        });

        let app_clone = app_handle.clone();
        let sid = session_id.clone();
        let agent_type_name = agent_type.clone();

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
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

            let _ = app_clone.emit("agent-done", serde_json::json!({
                "sessionId": sid,
            }));
        });

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            let mut stderr_buf = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr_buf.push_str(&line);
                stderr_buf.push('\n');
            }
            if !stderr_buf.is_empty() {
                log::warn!("[Agent/{}] stderr: {}", agent_type_name, stderr_buf.trim());
            }
        });

        Ok(())
    }

    pub fn stop_generation(&mut self, session_id: &str) {
        if let Some(process) = self.processes.get(session_id) {
            process.aborted.store(true, Ordering::Relaxed);
        }
        if let Some(process) = self.processes.get_mut(session_id) {
            if let Some(ref mut child) = process.child {
                let _ = child.start_kill();
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
        match agent_type {
            "claude" => claude_skills().await,
            "hermes" => hermes_skills().await,
            "codex" => codex_skills().await,
            _ => vec![],
        }
    }
}

impl Drop for AgentManager {
    fn drop(&mut self) {
        for (sid, mut process) in self.processes.drain() {
            process.aborted.store(true, Ordering::Relaxed);
            if let Some(ref mut child) = process.child {
                let _ = child.start_kill();
            }
            log::info!("[Agent] Cleaned up session: {}", sid);
        }
    }
}

// ──────────────────────────────────────────────
//  技能列表
// ──────────────────────────────────────────────

async fn claude_skills() -> Vec<crate::db::models::SkillInfo> {
    let mut skills = vec![
        SkillInfo::new("code-review", "代码审查与优化建议", "内置"),
        SkillInfo::new("translate", "多语言翻译", "内置"),
        SkillInfo::new("summarize", "文本摘要与总结", "内置"),
        SkillInfo::new("debug", "代码调试与错误诊断", "内置"),
        SkillInfo::new("refactor", "代码重构", "内置"),
        SkillInfo::new("test-gen", "单元测试生成", "内置"),
        SkillInfo::new("doc-gen", "文档生成", "内置"),
        SkillInfo::new("explain", "代码解释与分析", "内置"),
        SkillInfo::new("architect", "系统架构设计与评审", "内置"),
    ];

    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        let config_path = std::path::Path::new(&home).join(".claude").join("claude_desktop_config.json");
        if let Ok(raw) = tokio::fs::read_to_string(&config_path).await {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(servers) = config.get("mcpServers").and_then(|v| v.as_object()) {
                    for name in servers.keys() {
                        skills.push(SkillInfo::new(&format!("mcp:{}", name), &format!("MCP Server: {}", name), "MCP"));
                    }
                }
            }
        }
    }
    skills
}

async fn hermes_skills() -> Vec<crate::db::models::SkillInfo> {
    let mut skills = Vec::new();
    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        let skills_dir = std::path::Path::new(&home).join(".hermes").join("skills");
        if let Ok(mut entries) = tokio::fs::read_dir(&skills_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let manifest_path = entry.path().join("manifest.json");
                    let description = if let Ok(raw) = tokio::fs::read_to_string(&manifest_path).await {
                        if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) {
                            manifest.get("description").and_then(|v| v.as_str()).unwrap_or(&name).to_string()
                        } else { name.clone() }
                    } else { name.clone() };
                    skills.push(SkillInfo::new(&name, &description, "自定义"));
                }
            }
        }
    }
    if skills.is_empty() {
        skills = vec![
            SkillInfo::new("code-review", "代码审查与优化建议", "内置"),
            SkillInfo::new("translate", "多语言翻译", "内置"),
            SkillInfo::new("summarize", "文本摘要与总结", "内置"),
            SkillInfo::new("debug", "代码调试与错误诊断", "内置"),
            SkillInfo::new("refactor", "代码重构", "内置"),
            SkillInfo::new("test-gen", "单元测试生成", "内置"),
            SkillInfo::new("doc-gen", "文档生成", "内置"),
        ];
    }
    skills
}

async fn codex_skills() -> Vec<crate::db::models::SkillInfo> {
    vec![
        SkillInfo::new("code-gen", "代码生成与补全", "内置"),
        SkillInfo::new("explain", "代码解释与分析", "内置"),
        SkillInfo::new("refactor", "代码重构", "内置"),
        SkillInfo::new("debug", "代码调试与错误诊断", "内置"),
        SkillInfo::new("test-gen", "单元测试生成", "内置"),
    ]
}
