// ──────────────────────────────────────────────
//  ProcessHandler — 进程协议抽象
// ──────────────────────────────────────────────
//  支持 stdio / http / sse 三种交互模式，
//  降低新增 Agent 的进程管理门槛。
//
//  噪声过滤已迁移至 line_processor 模块（L0 平台层 + L1 Agent 配置层）。

use crate::agent::config::AgentConfig;
use crate::agent::line_processor::LineProcessor;

/// 从 CLI 输出中提取的**结构化事件**（尚未绑定会话 id）。
///
/// 仅在 `output_parser = "json-stream"` 且对端确有结构化输出时产生；
/// 由传输层（`transport::subprocess`）绑定会话 id 后投影为 `UnifiedAgentEvent`。
///
/// 字段口径来自**实测样本**（见实施计划 §十三）：claude `stream-json` 的
/// `assistant.message.usage`（已实测）、`assistant.message.content[].tool_use`、
/// `user.message.content[].tool_result`、`result.is_error`。
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// 本轮某次 API 调用的用量（Anthropic 口径：`input_tokens` 不含缓存部分）
    Usage {
        prompt: u32,
        completion: u32,
        total: u32,
        /// 缓存命中读取（claude `cache_read_input_tokens` / codex `cached_input_tokens`）
        cache_read: u32,
        /// 缓存写入（claude `cache_creation_input_tokens` / codex `cache_write_input_tokens`）
        cache_write: u32,
        /// 对端自报模型标签（claude `message.model` 有值；codex 的 JSON 流不提供）
        model: Option<String>,
    },
    /// 工具调用开始
    ToolStart {
        tool_id: String,
        name: String,
        args: String,
    },
    /// 工具调用结束
    ToolResult {
        tool_id: String,
        result: String,
        success: bool,
    },
    /// 对端自报失败（判据**优先于** exit code）
    Failed { message: String },
}

/// 进程交互协议抽象
pub trait ProcessHandler: Send + Sync {
    /// 构建命令和参数
    /// 返回 (命令名, 参数列表)
    fn build_command(&self, message: &str, agent_session_id: Option<&str>)
        -> (String, Vec<String>);

    /// 解析输出行，返回可展示的内容
    fn parse_output_line(&self, line: &str) -> Option<String>;

    /// 从输出行提取 agent session_id
    /// is_stderr: 当前行是否来自 stderr
    fn extract_session_id(&self, line: &str) -> Option<String>;

    /// 从一行输出中提取**结构化事件**（工具调用 / 用量 / 失败标记）。
    ///
    /// 默认返回空：多数 CLI 无结构化输出，只有 `json-stream` 类解析器才有。
    /// 传输层据此补上此前被整体丢弃的工具链路与用量信息。
    fn parse_stream_events(&self, _line: &str) -> Vec<StreamEvent> {
        Vec::new()
    }
}

// ──────────────────────────────────────────────
//  Stdio 模式处理器
// ──────────────────────────────────────────────

#[derive(Clone)]
pub struct StdioHandler {
    pub config: AgentConfig,
    /// 行处理流水线：ANSI 剥离 + 噪声过滤
    line_processor: LineProcessor,
}

impl StdioHandler {
    pub fn from_config(config: AgentConfig) -> Self {
        let line_processor = LineProcessor::default_with_filter(&config.output_filter_regex);
        Self {
            config,
            line_processor,
        }
    }
}

impl ProcessHandler for StdioHandler {
    fn build_command(
        &self,
        message: &str,
        agent_session_id: Option<&str>,
    ) -> (String, Vec<String>) {
        // ── 选择模板 ──
        // 有 agent_session_id → 使用 resume_arg_template（完整的命令模板，含 --resume {session_id}）
        // 无 agent_session_id → 使用 run_cmd_template
        let template_str = if let Some(sid) = agent_session_id {
            log::info!(
                "[Handler] build_command: session_id={:?} resume_tpl={}",
                sid,
                self.config.resume_arg_template
            );
            self.config.resume_arg_template.replace("{session_id}", sid)
        } else {
            log::info!("[Handler] build_command: no session_id, using run_cmd_template");
            self.config.run_cmd_template.clone()
        };

        if template_str.is_empty() {
            return (self.config.cli_command(), vec![message.to_string()]);
        }

        // ── 解析 {message} 占位符 ──
        let parts: Vec<&str> = template_str.splitn(2, "{message}").collect();
        let prefix_str = parts.first().unwrap_or(&"").trim();
        let suffix_str = parts.get(1).map(|s| s.trim()).unwrap_or("");

        // 解析前缀为固定参数（命令 + CLI 标志）
        let prefix_parts = simple_split(prefix_str);
        if prefix_parts.is_empty() {
            return (self.config.cli_command(), vec![message.to_string()]);
        }

        let cmd = prefix_parts[0].clone();
        let mut args: Vec<String> = prefix_parts[1..].to_vec();

        // 检测前缀最后一个参数是否使用 = 语法（如 --query= 或 -q=）
        let uses_equals_syntax = args.last().map_or(false, |a| a.ends_with('='));

        // 消息内容作为单个原子参数：换行符→字面 \\n；引号/空格交由进程启动时的 argv 转义统一处理，
        // 不再手动加双引号（手动加引号会在 Windows 上经二次转义，导致中文消息被错误拆分）。
        let sanitized = message
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "\\n");
        if uses_equals_syntax {
            if let Some(last) = args.last_mut() {
                last.push_str(&sanitized);
            }
        } else {
            args.push(sanitized);
        }

        // 追加后缀固定参数
        if !suffix_str.is_empty() {
            let suffix_parts = simple_split(suffix_str);
            args.extend(suffix_parts);
        }

        (cmd, args)
    }

    fn parse_output_line(&self, line: &str) -> Option<String> {
        // 统一通过 LineProcessor 处理：
        //   L0: ConPTY 路径噪声过滤
        //   L1: ANSI 剥离 + output_filter_regex 过滤
        // 解析器层（json-stream / ansi-text / raw-text）在此之后处理
        let processor = &self.line_processor;

        match self.config.output_parser.as_str() {
            "json-stream" => {
                // json-stream 不需要行级噪声过滤（所有内容在 JSON 结构体内）
                parse_json_stream(line)
            }
            "ansi-text" => {
                // ansi-text 走 LineProcessor 流水线
                processor.process_line(line)
            }
            _ => {
                // raw-text: 非空行直接返回（仍需 LineProcessor 过滤噪声）
                processor.process_line(line).map(|s| s + "\n")
            }
        }
    }

    fn extract_session_id(&self, line: &str) -> Option<String> {
        let source = self.config.session_id_source.as_str();
        // 根据 session_id_source 决定提取方式，调用方无需关心来源
        match source {
            "stdout-text" | "stderr-text" => {
                extract_session_id_from_text(line, &self.config.session_id_field)
            }
            "stdout-json" | "stderr-json" => extract_session_id_from_json(
                line,
                &self.config.session_id_event_type,
                &self.config.session_id_field,
            ),
            _ => None,
        }
    }

    fn parse_stream_events(&self, line: &str) -> Vec<StreamEvent> {
        // 只有 json-stream 解析器才有结构化事件；ansi-text / raw-text 无
        if self.config.output_parser != "json-stream" {
            return Vec::new();
        }
        parse_json_stream_events(line)
    }
}

/// 解析 claude `stream-json` 的**结构化事件**。
///
/// 样本依据（实施计划 §十三）：`tool_use` / `text` 同处 `assistant.message.content[]`；
/// 诚实标注：**当前样本仅实测到 `text` 与 `message.usage`**，
/// `tool_use` / `tool_result` 为文档化块类型，实现后需一次真实工具调用复核。
///
/// 非 JSON 行（如 `[claude-code:unrecognized_model] {...}` 噪声）直接返回空。
fn parse_json_stream_events(line: &str) -> Vec<StreamEvent> {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();

    match event["type"].as_str().unwrap_or("") {
        "assistant" => {
            // ── 用量（已实测字段；此前被整体丢弃）──
            // Anthropic 口径：input_tokens 不含缓存，故四桶相加为 total（与 api_agent 落库口径一致）
            let usage = &event["message"]["usage"];
            if usage.is_object() {
                let prompt = usage["input_tokens"].as_u64().unwrap_or(0) as u32;
                let completion = usage["output_tokens"].as_u64().unwrap_or(0) as u32;
                let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0) as u32;
                let cache_write = usage["cache_creation_input_tokens"]
                    .as_u64()
                    .unwrap_or(0) as u32;
                if prompt + completion + cache_read + cache_write > 0 {
                    out.push(StreamEvent::Usage {
                        prompt,
                        completion,
                        total: prompt + completion + cache_read + cache_write,
                        cache_read,
                        cache_write,
                        model: event["message"]["model"]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                    });
                }
            }

            // ── 工具调用 ──
            if let Some(blocks) = event["message"]["content"].as_array() {
                for b in blocks {
                    if b["type"] == "tool_use" {
                        out.push(StreamEvent::ToolStart {
                            tool_id: b["id"].as_str().unwrap_or_default().to_string(),
                            name: b["name"].as_str().unwrap_or_default().to_string(),
                            args: b["input"].to_string(),
                        });
                    }
                }
            }
        }
        "user" => {
            // ── 工具结果（claude 以 user 消息回传 tool_result）──
            if let Some(blocks) = event["message"]["content"].as_array() {
                for b in blocks {
                    if b["type"] == "tool_result" {
                        let content = match &b["content"] {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        out.push(StreamEvent::ToolResult {
                            tool_id: b["tool_use_id"].as_str().unwrap_or_default().to_string(),
                            result: content,
                            success: !b["is_error"].as_bool().unwrap_or(false),
                        });
                    }
                }
            }
        }
        "result" => {
            // ── 对端自报失败：比 exit code 更可靠的判据（实测：exit=1 而 subtype=success）──
            if event["is_error"].as_bool().unwrap_or(false) {
                out.push(StreamEvent::Failed {
                    message: event["result"]
                        .as_str()
                        .unwrap_or("Agent 自报失败")
                        .to_string(),
                });
            }
        }
        // codex `exec --json`：回合结束给一次**累计**用量（实测样本：
        // `{"type":"turn.completed","usage":{"input_tokens":8915,"cached_input_tokens":0,
        //  "cache_write_input_tokens":0,"output_tokens":3,"reasoning_output_tokens":0}}`）。
        // OpenAI 口径 `input_tokens` 含缓存命中部分，故未命中输入 = input - cached。
        "turn.completed" => {
            let usage = &event["usage"];
            if usage.is_object() {
                let input = usage["input_tokens"].as_u64().unwrap_or(0) as u32;
                let cache_read = usage["cached_input_tokens"].as_u64().unwrap_or(0) as u32;
                let cache_write = usage["cache_write_input_tokens"].as_u64().unwrap_or(0) as u32;
                let completion = usage["output_tokens"].as_u64().unwrap_or(0) as u32;
                let prompt = input.saturating_sub(cache_read);
                let total = prompt + cache_read + cache_write + completion;
                if total > 0 {
                    out.push(StreamEvent::Usage {
                        prompt,
                        completion,
                        total,
                        cache_read,
                        cache_write,
                        // codex 的 JSON 流不带模型名
                        model: None,
                    });
                }
            }
        }
        _ => {}
    }

    out
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
//  Session ID 提取器
// ──────────────────────────────────────────────

/// 从 stdout JSON 事件中提取 session_id
fn extract_session_id_from_json(line: &str, event_type: &str, field: &str) -> Option<String> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;

    // event_type 是 event["type"] 的值（如 "system"、"thread.started"）
    // 用于过滤特定类型的事件，空字符串表示不过滤
    if !event_type.is_empty() {
        if event.get("type")?.as_str()? != event_type {
            return None;
        }
    }

    // 从根对象提取指定字段
    event.get(field)?.as_str().map(|s| s.to_string())
}

/// 从 stderr 文本行中提取 session_id（前缀匹配，自动剥离 ANSI 颜色码）
fn extract_session_id_from_text(line: &str, field: &str) -> Option<String> {
    // 先剥离 ANSI 颜色码（Hermes 等 CLI 工具可能输出带颜色的文本）
    let clean = crate::agent::line_processor::strip_ansi_text(line);

    // 确定 key 前缀：提取 field 中冒号前的部分（含冒号）作为 key
    // 兼容 field 中冒号后任意数量的空白（如 "Session: " 匹配 "Session:        xxx"）
    let key = if field.is_empty() {
        "session_id:"
    } else if let Some(colon_pos) = field.find(':') {
        &field[..=colon_pos]
    } else {
        field
    };

    // 检查行是否以 key 开头，跳过 key 后的任意空白，剩余部分即为 session_id
    if let Some(after_key) = clean.strip_prefix(key) {
        let sid = after_key.trim();
        if !sid.is_empty() {
            Some(sid.to_string())
        } else {
            None
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi() {
        assert_eq!(
            crate::agent::line_processor::strip_ansi_text("\x1b[36mHello\x1b[0m"),
            "Hello"
        );
        assert_eq!(
            crate::agent::line_processor::strip_ansi_text("\x1b[1m\x1b[31mRed Bold\x1b[0m"),
            "Red Bold"
        );
        assert_eq!(
            crate::agent::line_processor::strip_ansi_text("No ANSI"),
            "No ANSI"
        );
        assert_eq!(crate::agent::line_processor::strip_ansi_text(""), "");
    }

    #[test]
    fn test_extract_session_id_from_text_plain() {
        // 无 ANSI 码，正常提取
        let result = extract_session_id_from_text("Session: abc123", "Session: ");
        assert_eq!(result, Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_session_id_from_text_with_ansi() {
        // 有 ANSI 码，修复版应正确提取
        let line = "\x1b[36mSession: abc123\x1b[0m";
        let result = extract_session_id_from_text(line, "Session: ");
        assert_eq!(
            result,
            Some("abc123".to_string()),
            "ANSI codes should be stripped before matching"
        );
    }

    #[test]
    fn test_extract_session_id_from_text_various() {
        let cases = vec![
            ("Session: abc-def_123", "Session: ", Some("abc-def_123")),
            ("Session:  abc123", "Session: ", Some("abc123")),
            ("Session: abc123  ", "Session: ", Some("abc123")),
            ("session_id: abc123", "", Some("abc123")),
            ("\x1b[32mSession: xyz\x1b[0m", "Session: ", Some("xyz")),
            // Hermes 实际 stderr 输出格式：session_id: + 空格 + 值
            (
                "session_id: 20260703_114256_e2449d",
                "session_id: ",
                Some("20260703_114256_e2449d"),
            ),
            // 默认 field 为 "session_id: " 时匹配 "session_id:" + 任意空白
            ("session_id:        xyz789", "", Some("xyz789")),
        ];
        for (line, field, expected) in cases {
            let result = extract_session_id_from_text(line, field);
            assert_eq!(
                result,
                expected.map(|s| s.to_string()),
                "Failed on line={:?}, field={:?}",
                line,
                field
            );
        }
    }

    #[test]
    fn test_extract_session_id_from_text_no_match() {
        let cases = vec![
            ("  Session: abc123", "Session: "),    // 前导空格
            ("[Session: abc123]", "Session: "),    // 括号
            ("Session ID: abc123", "Session: "),   // 不同前缀
            ("Chat Session: abc123", "Session: "), // 不同前缀
        ];
        for (line, field) in cases {
            let result = extract_session_id_from_text(line, field);
            assert_eq!(
                result, None,
                "Should not match on line={:?}, field={:?}",
                line, field
            );
        }
    }

    #[test]
    fn test_extract_session_id_from_text_default_field() {
        // 空 field 时使用默认前缀 "session_id: "
        let result = extract_session_id_from_text("session_id: abc123", "");
        assert_eq!(result, Some("abc123".to_string()));

        let result = extract_session_id_from_text("Session: abc123", "");
        assert_eq!(
            result, None,
            "Default prefix is 'session_id: ', not 'Session: '"
        );
    }
}

/// 结构化事件解析测试：用例取自**实测样本**（实施计划 §十三）。
#[cfg(test)]
mod stream_event_tests {
    use super::*;

    /// assistant 消息：文本块 + 用量（Anthropic 口径，total = 四桶之和）
    #[test]
    fn parses_assistant_usage() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"pong"}],"usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":4}},"session_id":"s1"}"#;
        assert_eq!(
            parse_json_stream_events(line),
            vec![StreamEvent::Usage {
                prompt: 10,
                completion: 2,
                total: 19,
                cache_read: 3,
                cache_write: 4,
                model: Some("claude-sonnet-4-5".into()),
            }]
        );
    }

    /// codex `turn.completed`：OpenAI 口径 `input_tokens` 含缓存，未命中 = input - cached
    #[test]
    fn parses_codex_turn_completed_usage() {
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":8915,"cached_input_tokens":900,"cache_write_input_tokens":0,"output_tokens":3,"reasoning_output_tokens":0}}"#;
        assert_eq!(
            parse_json_stream_events(line),
            vec![StreamEvent::Usage {
                prompt: 8015,
                completion: 3,
                total: 8918,
                cache_read: 900,
                cache_write: 0,
                model: None,
            }]
        );
    }

    /// 工具调用块（文档化块类型；本次样本未观测到，待真实工具调用复核）
    #[test]
    fn parses_tool_use_block() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1","name":"Read","input":{"file_path":"a.rs"}}]}}"#;
        match parse_json_stream_events(line).as_slice() {
            [StreamEvent::ToolStart { tool_id, name, args }] => {
                assert_eq!(tool_id, "tu_1");
                assert_eq!(name, "Read");
                assert!(args.contains("a.rs"));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    /// 工具结果块（claude 以 user 消息回传）
    #[test]
    fn parses_tool_result_block() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_1","content":"ok"}]}}"#;
        assert_eq!(
            parse_json_stream_events(line),
            vec![StreamEvent::ToolResult {
                tool_id: "tu_1".into(),
                result: "ok".into(),
                success: true,
            }]
        );
    }

    /// result 信封的 is_error：优先于 exit code 的失败判据
    #[test]
    fn parses_result_error_envelope() {
        let line = r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: 400"}"#;
        match parse_json_stream_events(line).as_slice() {
            [StreamEvent::Failed { message }] => assert!(message.contains("API Error")),
            other => panic!("unexpected: {:?}", other),
        }
    }

    /// 非 JSON 噪声行（`[claude-code:unrecognized_model] {…}`）被忽略
    #[test]
    fn ignores_noise_line() {
        assert!(
            parse_json_stream_events("[claude-code:unrecognized_model] {\"model\":\"x\"}").is_empty()
        );
    }

    /// 全零用量不产生事件（避免噪声 Usage）
    #[test]
    fn skips_zero_usage() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":0,"output_tokens":0}}}"#;
        assert!(parse_json_stream_events(line).is_empty());
    }
}
