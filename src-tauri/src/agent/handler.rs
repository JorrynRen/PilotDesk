// ──────────────────────────────────────────────
//  ProcessHandler — 进程协议抽象 (Phase 5)
// ──────────────────────────────────────────────
//  支持 stdio / http / sse 三种交互模式，
//  降低新增 Agent 的进程管理门槛。

use crate::commands::agents::AgentConfig;

/// 进程交互协议抽象
pub trait ProcessHandler: Send + Sync {
    /// 构建命令和参数
    /// 返回 (命令名, 参数列表)
    fn build_command(&self, message: &str, agent_session_id: Option<&str>) -> (String, Vec<String>);

    /// 解析输出行，返回可展示的内容
    fn parse_output_line(&self, line: &str) -> Option<String>;

    /// 从输出行提取 agent session_id
    /// is_stderr: 当前行是否来自 stderr
    fn extract_session_id(&self, line: &str, is_stderr: bool) -> Option<String>;
}

// ──────────────────────────────────────────────
//  Stdio 模式处理器
// ──────────────────────────────────────────────

#[derive(Clone)]
pub struct StdioHandler {
    pub config: AgentConfig,
}

impl StdioHandler {
    pub fn from_config(config: AgentConfig) -> Self {
        Self { config }
    }
}

impl ProcessHandler for StdioHandler {
    fn build_command(&self, message: &str, agent_session_id: Option<&str>) -> (String, Vec<String>) {
        let template = &self.config.run_cmd_template;
        if template.is_empty() {
            // 回退：直接使用 cli_command + message
            return (self.config.cli_command.clone(), vec![message.to_string()]);
        }

        let cmd_str = if let Some(sid) = agent_session_id {
            let resume = self.config.resume_arg_template.replace("{session_id}", sid);
            // 在 {message} 位置插入 resume 参数 + 消息
            template.replace("{message}", &format!("{} {}", resume, message))
        } else {
            template.replace("{message}", message)
        };

        // 简单拆分命令和参数（按空格拆分，支持引号包裹的参数）
        let parts: Vec<String> = simple_split(&cmd_str);
        if parts.is_empty() {
            return (self.config.cli_command.clone(), vec![message.to_string()]);
        }
        let cmd = parts[0].clone();
        let args: Vec<String> = parts[1..].to_vec();
        (cmd, args)
    }

    fn parse_output_line(&self, line: &str) -> Option<String> {
        match self.config.output_parser.as_str() {
            "json-stream" => parse_json_stream(line),
            "ansi-text" => parse_ansi_text(line, &self.config.output_filter_regex),
            _ => {
                // raw-text: 非空行直接返回
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string() + "\n")
                }
            }
        }
    }

    fn extract_session_id(&self, line: &str, is_stderr: bool) -> Option<String> {
        match self.config.session_id_source.as_str() {
            "stdout-json" => {
                if is_stderr {
                    return None;
                }
                extract_session_id_from_json(line, &self.config.session_id_event_type, &self.config.session_id_field)
            }
            "stderr-text" => {
                if !is_stderr {
                    return None;
                }
                extract_session_id_from_text(line, &self.config.session_id_field)
            }
            _ => None,
        }
    }
}

// ──────────────────────────────────────────────
//  JSON stream 解析器
// ──────────────────────────────────────────────

fn parse_json_stream(line: &str) -> Option<String> {
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
        // Codex: item.completed events
        if event["type"] == "item.completed" {
            if let Some(text) = event["item"]["text"].as_str() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string() + "\n");
                }
            }
        }
    }
    None
}

// ──────────────────────────────────────────────
//  ANSI 文本解析器
// ──────────────────────────────────────────────

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
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("Initializing agent") {
        return false;
    }
    if trimmed.starts_with("Resume this session") {
        return false;
    }
    if trimmed.starts_with("hermes --resume") {
        return false;
    }
    if trimmed.starts_with("Session:") {
        return false;
    }
    if trimmed.starts_with("Duration:") {
        return false;
    }
    if trimmed.starts_with("Messages:") {
        return false;
    }
    if trimmed.starts_with("Query:") {
        return false;
    }
    true
}

fn parse_ansi_text(line: &str, filter_regex: &str) -> Option<String> {
    let clean = strip_ansi(line);

    // 如果配置了过滤正则，匹配的行跳过
    if !filter_regex.is_empty() {
        if let Ok(re) = regex::Regex::new(filter_regex) {
            if re.is_match(&clean) {
                return None;
            }
        }
    }

    if is_content_line(&clean) {
        Some(clean + "\n")
    } else {
        None
    }
}

// ──────────────────────────────────────────────
//  Session ID 提取器
// ──────────────────────────────────────────────

/// 从 stdout JSON 事件中提取 session_id
fn extract_session_id_from_json(line: &str, event_type: &str, field: &str) -> Option<String> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;

    // 如果指定了事件类型路径（如 "system/init"），验证事件类型匹配
    if !event_type.is_empty() {
        let parts: Vec<&str> = event_type.split('/').collect();
        let mut current = &event;
        for part in parts {
            current = current.get(part)?;
        }
        // 事件类型匹配成功，继续提取 field
    }

    // 提取指定字段
    event.get(field)?.as_str().map(|s| s.to_string())
}

/// 从 stderr 文本行中提取 session_id（前缀匹配）
fn extract_session_id_from_text(line: &str, field: &str) -> Option<String> {
    let prefix = if field.is_empty() {
        "session_id: "
    } else {
        // field 作为前缀使用
        field
    };
    if let Some(sid) = line.strip_prefix(prefix) {
        Some(sid.trim().to_string())
    } else {
        None
    }
}

/// 简单 shell 风格拆分（按空格拆分，支持双引号包裹的参数）
fn simple_split(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quote = !in_quote;
            }
            ' ' | '\t' if !in_quote => {
                if !current.is_empty() {
                    result.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}
