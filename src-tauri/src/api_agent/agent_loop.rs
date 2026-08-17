//! Agent Loop 任务编排
//!
//! 为 API Agent 提供 tool-calling 循环能力。
//! CLI Agent（Claude/Hermes/CodeX）不使用此模块。

use crate::api_agent::client::ApiClient;
use crate::api_agent::types::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// 工具协议自 `tools/` 模块迁入（工具架构统一 v1.0），re-export 保持既有引用兼容。
pub use crate::tools::{
    classify_command, ApprovalHandler, CommandRisk, ContinueHandler, RiskLevel, ToolHandler,
    ToolRegistry, ToolTag,
};

/// 持久化权限规则（allow/deny 列表，存于 app_settings 的 `agent_permission_rules` key）
/// 用于在默认风险审批之前做确定性拦截/放行。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRules {
    #[serde(default)]
    pub allow_paths: Vec<String>,
    #[serde(default)]
    pub deny_paths: Vec<String>,
    #[serde(default)]
    pub allow_commands: Vec<String>,
    #[serde(default)]
    pub deny_commands: Vec<String>,
}

impl PermissionRules {
    /// 判断路径是否命中规则。`Some(true)`=放行，`Some(false)`=拦截，`None`=无规则命中。
    pub fn check_path(&self, path: &str) -> Option<bool> {
        let norm = path.replace('\\', "/").to_lowercase();
        if norm.is_empty() {
            return None;
        }
        for d in &self.deny_paths {
            let p = d.trim().replace('\\', "/").to_lowercase();
            if !p.is_empty() && glob_match(&p, &norm) {
                return Some(false);
            }
        }
        for a in &self.allow_paths {
            let p = a.trim().replace('\\', "/").to_lowercase();
            if !p.is_empty() && glob_match(&p, &norm) {
                return Some(true);
            }
        }
        None
    }

    /// 判断命令是否命中规则。`Some(true)`=放行，`Some(false)`=拦截，`None`=无规则命中。
    pub fn check_command(&self, cmd: &str) -> Option<bool> {
        let c = cmd.trim().to_lowercase();
        if c.is_empty() {
            return None;
        }
        for d in &self.deny_commands {
            let p = d.trim().to_lowercase();
            if !p.is_empty() && c.contains(&p) {
                return Some(false);
            }
        }
        for a in &self.allow_commands {
            let p = a.trim().to_lowercase();
            if !p.is_empty() && c.contains(&p) {
                return Some(true);
            }
        }
        None
    }
}

/// 简单 glob 匹配：`*` 不跨 `/`，`**` 跨任意字符，`?` 匹配单个非 `/` 字符。
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // (pattern_idx, text_idx)
    let mut memo = std::collections::HashSet::new();
    fn matches(p: &[char], t: &[char], pi: usize, ti: usize, memo: &mut std::collections::HashSet<(usize, usize)>) -> bool {
        if !memo.insert((pi, ti)) {
            return false;
        }
        if pi == p.len() {
            return ti == t.len();
        }
        match p[pi] {
            '*' => {
                // `**` 跨任意，`*` 不跨 `/`
                let cross = pi + 1 < p.len() && p[pi + 1] == '*';
                let step = if cross { 2 } else { 1 };
                if matches(p, t, pi + step, ti, memo) {
                    return true;
                }
                if ti < t.len() && (cross || t[ti] != '/') {
                    if matches(p, t, pi, ti + 1, memo) {
                        return true;
                    }
                }
                false
            }
            '?' => {
                if ti < t.len() && t[ti] != '/' {
                    matches(p, t, pi + 1, ti + 1, memo)
                } else {
                    false
                }
            }
            c => {
                if ti < t.len() && t[ti] == c {
                    matches(p, t, pi + 1, ti + 1, memo)
                } else {
                    false
                }
            }
        }
    }
    matches(&p, &t, 0, 0, &mut memo)
}

/// 当前会话的授权级别
/// 控制哪些风险等级的工具需要用户确认
#[derive(Debug, Clone)]
pub enum AuthLevel {
    /// 静默模式：所有操作自动执行
    #[allow(dead_code)]
    Silent,
    /// 确认模式：中风险及以上需要确认（默认）
    Confirm,
    /// 阻止模式：高风险操作被阻止
    #[allow(dead_code)]
    Block,
}

/// Agent Loop 配置
pub struct AgentLoopConfig {
    /// 最大迭代次数（软上限，达到后请求用户确认）
    pub max_iterations: usize,
    /// 系统提示词（含 MEMORY.md + USER.md + 技能列表）
    pub system_prompt: String,
    /// 可用工具列表
    pub tools: Vec<ToolDefinition>,
    /// 会话历史消息（不含 system prompt）
    pub messages: Vec<ChatMessage>,
    /// 模型温度（0.0-2.0），None 表示不传递
    pub temperature: Option<f32>,
    /// 最大生成 token 数，None 表示使用模型默认
    pub max_tokens: Option<u32>,
    /// 上下文窗口大小（token），None 使用默认
    #[allow(dead_code)]
    pub context_tokens: Option<usize>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 20,
            system_prompt: String::new(),
            tools: Vec::new(),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            context_tokens: None,
        }
    }
}

/// Agent Loop 执行结果
pub struct AgentLoopOutput {
    /// 最终回复内容
    pub content: String,
    /// 完整对话消息（不含 system prompt，供会话上下文持久化）
    pub messages: Vec<ChatMessage>,
    /// 本轮工具调用链（tool_start/tool_result 步骤，供群聊消息持久化溯源）。
    pub tool_calls: Vec<ThinkingChainStep>,
}

/// Agent Loop 编排器
///
/// 执行 tool-calling 循环：
///   用户输入 → LLM 调用 → 有 tool_calls?
///   ├─ 是 → 风险评估 → 执行工具 → 结果注入 → 继续 LLM
///   └─ 否 → 返回最终答案
pub struct AgentLoop {
    client: ApiClient,
    tool_registry: Arc<ToolRegistry>,
    model: String,
    temperature: f32,
    auth_level: AuthLevel,
    approval_handler: Option<ApprovalHandler>,
    /// 审批方身份标签（拒绝文案用，默认 "用户"；群聊场景由 Director 裁决时为 "主持人"）
    approval_label: String,
    /// 迭代上限回调（达到限制时请求用户确认是否继续）
    continue_handler: Option<ContinueHandler>,
    /// Tauri AppHandle，用于直接发送事件到前端（绕过广播 channel，确保实时性）
    app_handle: tauri::AppHandle,
    /// 当前会话 ID
    session_id: String,
    /// 当前会话已授权的文件路径（避免同一文件反复弹窗）
    approved_files: std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// 当前会话已授权的目录（该目录下所有 write_file 自动放行）
    approved_dirs: std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// 已批准执行的 execute_command（按规范化命令字符串缓存，避免重复弹窗）
    approved_commands: std::sync::Mutex<std::collections::HashSet<String>>,
    /// 持久化权限规则（allow/deny 列表）
    permission_rules: PermissionRules,
    /// API 格式（OpenAI / Anthropic）
    api_format: ApiFormat,
    /// 流式增量回调（逐 chunk 透出，供群聊框架 token_stream 使用）
    on_delta: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

impl AgentLoop {
    pub fn new(
        client: ApiClient,
        tool_registry: Arc<ToolRegistry>,
        model: String,
        app_handle: tauri::AppHandle,
        session_id: String,
    ) -> Self {
        Self {
            client,
            tool_registry,
            model,
            temperature: 0.7,
            auth_level: AuthLevel::Confirm,
            approval_handler: None,
            approval_label: "用户".to_string(),
            continue_handler: None,
            app_handle,
            session_id,
            approved_files: std::sync::Mutex::new(std::collections::HashSet::new()),
            approved_dirs: std::sync::Mutex::new(std::collections::HashSet::new()),
            approved_commands: std::sync::Mutex::new(std::collections::HashSet::new()),
            permission_rules: PermissionRules::default(),
            api_format: ApiFormat::default(),
            on_delta: None,
        }
    }

    /// 设置流式增量回调（每个 LLM 文本分片调用一次）
    pub fn with_on_delta(mut self, on_delta: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.on_delta = Some(on_delta);
        self
    }

    pub fn with_api_format(mut self, format: ApiFormat) -> Self {
        self.api_format = format;
        self
    }

    #[allow(dead_code)]
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = temp;
        self
    }

    /// 设置授权级别
    pub fn with_auth_level(mut self, level: AuthLevel) -> Self {
        self.auth_level = level;
        self
    }

    /// 设置持久化权限规则（allow/deny 列表）
    pub fn with_permission_rules(mut self, rules: PermissionRules) -> Self {
        self.permission_rules = rules;
        self
    }

    /// 设置审批回调
    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    /// 设置审批方身份标签（拒绝文案用，如 "用户" / "主持人"）
    pub fn with_approval_label(mut self, label: &str) -> Self {
        self.approval_label = label.to_string();
        self
    }

    /// 设置迭代上限回调（达到限制时阻塞等待用户确认）
    pub fn with_continue_handler(mut self, handler: ContinueHandler) -> Self {
        self.continue_handler = Some(handler);
        self
    }

    /// 检查是否需要用户审批
    fn needs_approval(&self, risk: &RiskLevel) -> bool {
        match self.auth_level {
            AuthLevel::Silent => false,
            AuthLevel::Confirm => *risk >= RiskLevel::Medium,
            AuthLevel::Block => *risk >= RiskLevel::High,
        }
    }

    /// 直接发送事件到前端（不经过广播 channel，确保审批前实时送达）
    fn emit_to_frontend(&self, event: AgentLoopEvent) {
        use tauri::Emitter;
        let sid = &self.session_id;
        match event {
            AgentLoopEvent::Reasoning { content } => {
                let _ = self.app_handle.emit("agent-reasoning", serde_json::json!({
                    "sessionId": sid,
                    "content": content,
                }));
            }
            AgentLoopEvent::Chunk { content } => {
                let _ = self.app_handle.emit("agent-chunk", serde_json::json!({
                    "sessionId": sid,
                    "content": content,
                }));
            }
            AgentLoopEvent::ToolStart { id, name, arguments } => {
                let _ = self.app_handle.emit("agent-tool-start", serde_json::json!({
                    "sessionId": sid,
                    "toolId": id,
                    "toolName": name,
                    "arguments": arguments,
                }));
            }
            AgentLoopEvent::ToolResult { id, name, result, success } => {
                let _ = self.app_handle.emit("agent-tool-result", serde_json::json!({
                    "sessionId": sid,
                    "toolId": id,
                    "toolName": name,
                    "result": result,
                    "success": success,
                }));
            }
            AgentLoopEvent::ApprovalRequired { call_id, tool_name, arguments, risk_description } => {
                let _ = self.app_handle.emit("agent-approval-required", serde_json::json!({
                    "sessionId": sid,
                    "toolId": call_id,
                    "toolName": tool_name,
                    "arguments": arguments,
                    "riskDescription": risk_description,
                }));
            }
            AgentLoopEvent::Done { content } => {
                let _ = self.app_handle.emit("agent-done", serde_json::json!({
                    "sessionId": sid,
                    "content": content,
                }));
            }
            AgentLoopEvent::Error { message } => {
                let _ = self.app_handle.emit("agent-error", serde_json::json!({
                    "sessionId": sid,
                    "error": message,
                }));
            }
            AgentLoopEvent::IterationLimit { current, max } => {
                let _ = self.app_handle.emit("agent-iteration-limit", serde_json::json!({
                    "sessionId": sid,
                    "current": current,
                    "max": max,
                }));
            }
            AgentLoopEvent::Usage { prompt_tokens, completion_tokens, total_tokens } => {
                let _ = self.app_handle.emit("agent-usage", serde_json::json!({
                    "sessionId": sid,
                    "promptTokens": prompt_tokens,
                    "completionTokens": completion_tokens,
                    "totalTokens": total_tokens,
                }));
            }
        }
    }

    /// 执行 Agent Loop
    pub async fn run(
        &self,
        config: AgentLoopConfig,
    ) -> Result<AgentLoopOutput, String> {
        let mut messages: Vec<ChatMessage> = Vec::new();
        let mut max_iters = config.max_iterations;

        // 1. 注入 system prompt
        if !config.system_prompt.is_empty() {
            messages.push(ChatMessage::system(&config.system_prompt));
        }

        // 2. 添加历史消息 + 用户输入
        messages.extend(config.messages);

        let tools = if config.tools.is_empty() {
            None
        } else {
            Some(config.tools)
        };

        // 3. Agent Loop 主循环（软上限 + 用户确认可继续）
        let mut iteration: usize = 0;
        let mut empty_response_count: usize = 0; // 连续空响应计数
        let mut reasoning_only_count: usize = 0; // 连续只推理不调用工具的次数
        // 进展感知动态收敛：连续无进展轮数。有进展（工具成功且结果非空 / 输出非空文本）即清零。
        // 达到阈值先注入收敛提示，再超阈值则请求 continue_handler（群聊为 Director 裁决）决定收尾。
        const STALL_THRESHOLD: usize = 3;
        let mut stall_count: usize = 0;
        let mut stall_prompted: bool = false;
        loop {
            // ── 迭代上限检查：达到限制时请求用户确认 ──
            if iteration >= max_iters {
                log::warn!("[AgentLoop] 达到迭代上限: {}/{}", iteration, max_iters);
                self.emit_to_frontend(AgentLoopEvent::IterationLimit {
                    current: iteration,
                    max: max_iters,
                });

                let should_continue = if let Some(ref handler) = self.continue_handler {
                    handler(iteration, max_iters)
                } else {
                    false
                };

                if should_continue {
                    max_iters += 15; // 追加 15 轮
                    log::info!("[AgentLoop] 用户确认继续，新上限: {}", max_iters);
                } else {
                    log::info!("[AgentLoop] 用户选择终止，进入总结收尾（不返回空内容）");
                    self.emit_to_frontend(AgentLoopEvent::Done {
                        content: String::new(),
                    });
                    return self.finish_with_summary(&messages, iteration).await;
                }
            }

            let request = ChatRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
                tool_choice: None,
                stream: true,
                temperature: config.temperature,
                max_tokens: config.max_tokens,
            };

            // 调用 LLM（流式）
            let mut content_buffer = String::new();

            let slf = self;
            let response = match self.api_format {
                ApiFormat::Anthropic => {
                    self.client.chat_stream_anthropic(
                        &request,
                        |chunk| {
                            content_buffer.push_str(chunk);
                            slf.emit_to_frontend(AgentLoopEvent::Chunk {
                                content: chunk.to_string(),
                            });
                            if let Some(cb) = &slf.on_delta {
                                cb(chunk);
                            }
                        },
                        |_| {}, // Anthropic 不支持 reasoning_content
                        |call_id, name, arguments| {
                            slf.emit_to_frontend(AgentLoopEvent::ToolStart {
                                id: call_id.to_string(),
                                name: name.to_string(),
                                arguments: arguments.to_string(),
                            });
                        },
                        |prompt, completion, total| {
                            slf.emit_to_frontend(AgentLoopEvent::Usage {
                                prompt_tokens: prompt,
                                completion_tokens: completion,
                                total_tokens: total,
                            });
                        },
                    ).await.map_err(|e| {
                        log::error!("[AgentLoop] chat_stream_anthropic 失败: {}", e);
                        slf.emit_to_frontend(AgentLoopEvent::Error { message: e.clone() });
                        e
                    })?
                }
                _ => {
                    self.client.chat_stream(
                        &request,
                        |chunk| {
                            content_buffer.push_str(chunk);
                            slf.emit_to_frontend(AgentLoopEvent::Chunk {
                                content: chunk.to_string(),
                            });
                            if let Some(cb) = &slf.on_delta {
                                cb(chunk);
                            }
                        },
                        |reasoning| {
                            slf.emit_to_frontend(AgentLoopEvent::Reasoning {
                                content: reasoning.to_string(),
                            });
                        },
                        |call_id, name, arguments| {
                            slf.emit_to_frontend(AgentLoopEvent::ToolStart {
                                id: call_id.to_string(),
                                name: name.to_string(),
                                arguments: arguments.to_string(),
                            });
                        },
                        |prompt, completion, total| {
                            slf.emit_to_frontend(AgentLoopEvent::Usage {
                                prompt_tokens: prompt,
                                completion_tokens: completion,
                                total_tokens: total,
                            });
                        },
                    ).await.map_err(|e| {
                        log::error!("[AgentLoop] chat_stream 失败: {}", e);
                        slf.emit_to_frontend(AgentLoopEvent::Error { message: e.clone() });
                        e
                    })?
                }
            };

            log::info!(
                "[AgentLoop] chat_stream 返回: iteration={}/{}, tool_calls={}, content_len={}, finish_reason={}",
                iteration + 1, max_iters, response.tool_calls.len(), content_buffer.len(), response.finish_reason
            );

            // 无工具调用 → 检查是否有实际内容
            if response.tool_calls.is_empty() {
                // 关键区分：finish_reason == "tool_calls" 表示模型意图调用工具
                // 但某些模型（如 Agnes）在 reasoning 阶段只输出 reasoning_content 而
                // 不带 tool_calls，此时应继续迭代而不是当作完成
                let is_tool_call_finish = response.finish_reason == "tool_calls";

                if content_buffer.trim().is_empty() {
                    empty_response_count += 1;
                    if is_tool_call_finish {
                        // 模型发出了 tool_calls 信号但没有具体工具调用
                        // 说明模型还在"思考"阶段，继续迭代
                        reasoning_only_count += 1;
                        log::info!(
                            "[AgentLoop] LLM 发出 tool_calls 信号但无具体调用，继续迭代: {}/{}，连续空响应: {}，连续推理: {}",
                            iteration + 1, max_iters, empty_response_count, reasoning_only_count
                        );
                        // 连续推理 2 轮后注入更强提示
                        if reasoning_only_count >= 2 {
                            log::warn!("[AgentLoop] 连续 {} 轮只推理不调用工具，注入强制提示", reasoning_only_count);
                            messages.push(ChatMessage::system(
                                "你已连续多轮只输出思考内容而未推进任务。请基于当前任务直接给出结论；若确实需要查看文件、搜索或执行操作，再调用相应工具，不要继续描述计划。"
                            ));
                            reasoning_only_count = 0;
                        }
                        iteration += 1;
                        continue;
                    }
                    // 模型明确返回了 stop 但内容为空
                    reasoning_only_count = 0; // 非 tool_calls finish，重置推理计数
                    log::warn!(
                        "[AgentLoop] LLM 返回空内容+无工具调用, iteration={}/{}, finish_reason={}, 连续空响应={}",
                        iteration + 1, max_iters, response.finish_reason, empty_response_count
                    );
                    // 允许最多 2 次重试（总共 3 次尝试）
                    if empty_response_count <= 2 {
                        let retry_prompt = match empty_response_count {
                            1 => "请基于当前任务继续推进：若需要查看文件或目录、搜索或执行操作，请调用相应工具；否则请直接给出回答。".to_string(),
                            2 => "你尚未给出有效回复。请直接完成当前任务：需要时调用工具，不需要时直接输出结论。".to_string(),
                            _ => unreachable!(),
                        };
                        log::info!("[AgentLoop] 空响应第 {} 次，注入提示重试", empty_response_count);
                        messages.push(ChatMessage::user(&retry_prompt));
                        iteration += 1;
                        continue;
                    }
                    // 超过重试次数 → 返回友好提示
                    let fallback = format!(
                        "模型未能生成有效回复（已连续 {} 次空响应）。请尝试重新提问，或更换支持工具调用的模型。",
                        empty_response_count
                    );
                    self.emit_to_frontend(AgentLoopEvent::Done {
                        content: fallback.clone(),
                    });
                    return Ok(AgentLoopOutput {
                        content: fallback,
                        messages: strip_system(&messages),
                        tool_calls: extract_tool_calls(&messages),
                    });
                }

                // 有文本内容，正常结束
                log::info!("[AgentLoop] 发送 Done 事件, content_len={}", content_buffer.len());
                self.emit_to_frontend(AgentLoopEvent::Done {
                    content: content_buffer.clone(),
                });
                log::info!("[AgentLoop] Done 事件已发送");
                // 将最终回复追加到消息历史，供会话上下文持久化。
                // 否则无工具调用的收尾回复会被遗漏，导致 session_contexts 快照丢失助手回复、上下文不完整。
                messages.push(ChatMessage::assistant(&content_buffer));
                return Ok(AgentLoopOutput {
                    content: content_buffer,
                    messages: strip_system(&messages),
                    tool_calls: extract_tool_calls(&messages),
                });
            }

            // 保存 assistant 消息（含 tool_calls，保留前缀文本）
            let mut assistant_msg = ChatMessage::assistant_with_tools(response.tool_calls.clone());
            if !content_buffer.is_empty() {
                assistant_msg.content = Some(std::mem::take(&mut content_buffer));
            }
            messages.push(assistant_msg);

            // 本轮是否产生实质进展（用于停滞检测）：有非空文本输出，或任一工具执行成功且结果非空。
            let mut made_progress = !content_buffer.trim().is_empty();

            // 执行每个工具调用
            for tc in &response.tool_calls {
                // 查找工具的风险等级：execute_command 支持动态风险评估，其余工具使用注册时等级
                let risk = if tc.function.name == "execute_command" {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                    match classify_command(cmd) {
                        (CommandRisk::Blocked, _) => {
                            // 危险命令直接拦截，无需走审批流程
                            let msg = format!(
                                "安全拦截：命令包含禁止操作。被拦截模式: {}", cmd
                            );
                            self.emit_to_frontend(AgentLoopEvent::ToolResult {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                result: msg.clone(),
                                success: false,
                            });
                            messages.push(ChatMessage::tool_result(&tc.id, &msg));
                            continue;
                        }
                        (CommandRisk::Safe, _) => RiskLevel::Low,
                        (CommandRisk::Medium, Some(prompt)) => {
                            // 越界路径：先触发审批弹窗，批准后才继续执行
                            self.emit_to_frontend(AgentLoopEvent::ApprovalRequired {
                                call_id: tc.id.clone(),
                                tool_name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                                risk_description: format!("中等风险操作（命令引用工作区外路径）: {}", prompt),
                            });
                            if let Some(ref handler) = self.approval_handler {
                                let approved = handler(
                                    &tc.id,
                                    &tc.function.name,
                                    &tc.function.arguments,
                                    RiskLevel::Medium,
                                );
                                if !approved {
                                    let msg = format!("用户拒绝了工具调用: {}", tc.function.name);
                                    self.emit_to_frontend(AgentLoopEvent::ToolResult {
                                        id: tc.id.clone(),
                                        name: tc.function.name.clone(),
                                        result: msg.clone(),
                                        success: false,
                                    });
                                    messages.push(ChatMessage::tool_result(&tc.id, &msg));
                                    continue;
                                }
                                cache_approved_command(&self.approved_commands, cmd);
                            } else {
                                let msg = "用户拒绝了工具调用".to_string();
                                self.emit_to_frontend(AgentLoopEvent::ToolResult {
                                    id: tc.id.clone(),
                                    name: tc.function.name.clone(),
                                    result: msg.clone(),
                                    success: false,
                                });
                                messages.push(ChatMessage::tool_result(&tc.id, &msg));
                                continue;
                            }
                            RiskLevel::Medium
                        }
                        (CommandRisk::Medium, None) => RiskLevel::Medium,
                    }
                } else {
                    self.tool_registry
                        .get_risk_level(&tc.function.name)
                        .unwrap_or(RiskLevel::Medium)
                };

                // ── 持久化权限规则（allow/deny）优先于默认风险审批 ──
                let perm_decision = match tc.function.name.as_str() {
                    "write_file" | "edit_file" | "read_file" => {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        args.get("path")
                            .and_then(|v| v.as_str())
                            .and_then(|p| self.permission_rules.check_path(p))
                    }
                    "execute_command" => {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        args.get("command")
                            .and_then(|v| v.as_str())
                            .and_then(|c| self.permission_rules.check_command(c))
                    }
                    _ => None,
                };
                if perm_decision == Some(false) {
                    let msg = "安全规则拦截：该操作命中持久化 deny 规则，已被阻止。".to_string();
                    self.emit_to_frontend(AgentLoopEvent::ToolResult {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        result: msg.clone(),
                        success: false,
                    });
                    messages.push(ChatMessage::tool_result(&tc.id, &msg));
                    continue;
                }

                // 风险审批
                if self.needs_approval(&risk) && perm_decision != Some(true) {
                    // ── 路径授权缓存：同一文件/目录不重复弹窗 ──
                    let path_cached = if tc.function.name == "write_file" {
                        is_write_path_approved(
                            &self.approved_files,
                            &self.approved_dirs,
                            &tc.function.arguments,
                        )
                    } else {
                        false
                    };

                    // ── 命令授权缓存：同一 execute_command 不重复弹窗 ──
                    let cmd_cached = if tc.function.name == "execute_command"
                        && risk == RiskLevel::Medium
                    {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                        is_execute_command_approved(&self.approved_commands, cmd)
                    } else {
                        false
                    };

                    let approved = if path_cached || cmd_cached {
                        log::info!(
                            "[AgentLoop] 缓存命中，免审批: {} (path_cache={}, cmd_cache={})",
                            tc.function.name,
                            path_cached,
                            cmd_cached
                        );
                        true
                    } else if let Some(ref handler) = self.approval_handler {
                        self.emit_to_frontend(AgentLoopEvent::ApprovalRequired {
                            call_id: tc.id.clone(),
                            tool_name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                            risk_description: risk.description().to_string(),
                        });
                        handler(&tc.id, &tc.function.name, &tc.function.arguments, risk.clone())
                    } else {
                        log::warn!("[AgentLoop] 无审批回调，拒绝高风险工具: {}", tc.function.name);
                        false
                    };

                    if !approved {
                        let msg = format!("{}拒绝了工具调用: {}", self.approval_label, tc.function.name);
                        self.emit_to_frontend(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: msg.clone(),
                            success: false,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &msg));
                        continue;
                    }

                    // 授权通过后缓存路径（下次同一文件免审批）
                    if tc.function.name == "write_file" {
                        cache_approved_path(
                            &self.approved_files,
                            &self.approved_dirs,
                            &tc.function.arguments,
                        );
                    }
                }

                // 执行工具（带 60s 兜底超时；execute_command / execute_python 内部各有 30s 超时）
                log::info!("[AgentLoop] 开始执行工具: {}, args={}", tc.function.name, tc.function.arguments);
                let tool_name_clone = tc.function.name.clone();
                // ask_user 内部自带等待用户回复的超时（60s），跳过外层兜底超时以免冲突。
                let exec_result = if tc.function.name == "ask_user" {
                    self.tool_registry.execute(&tc.function.name, &tc.function.arguments).await
                } else {
                    match tokio::time::timeout(std::time::Duration::from_secs(60),
                        self.tool_registry.execute(&tc.function.name, &tc.function.arguments)
                    ).await {
                        Ok(r) => r,
                        Err(_elapsed) => Err("工具执行超时（超过 60 秒）".to_string()),
                    }
                };
                match exec_result {
                    Ok(result) => {
                        log::info!("[AgentLoop] 工具执行成功: {}, result_len={}", tool_name_clone, result.len());
                        self.emit_to_frontend(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: result.clone(),
                            success: true,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &result));
                        if !result.trim().is_empty() {
                            made_progress = true;
                        }
                    }
                    Err(err) => {
                        log::error!("[AgentLoop] 工具执行失败: {}, error={}", tool_name_clone, err);
                        self.emit_to_frontend(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: err.clone(),
                            success: false,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &format!("错误: {}", err)));
                    }
                }
            }

            iteration += 1;

            // ── 停滞检测：进展感知动态收敛，替代对固定轮数的硬依赖 ──
            if made_progress {
                stall_count = 0;
                stall_prompted = false;
            } else {
                stall_count += 1;
                if stall_count >= STALL_THRESHOLD {
                    if !stall_prompted {
                        // 第一次达到阈值：注入收敛提示，给模型最后一次推进机会。
                        stall_prompted = true;
                        stall_count = 0;
                        log::warn!("[AgentLoop] 连续 {} 轮无进展，注入收敛提示", STALL_THRESHOLD);
                        messages.push(ChatMessage::system(
                            "你已连续多轮未推进任务（未成功执行工具且未输出内容）。请基于已完成的工具结果直接给出最终结论；若确有必要，再调用具体工具继续推进，不要空转。",
                        ));
                    } else {
                        // 已提示一次仍无进展：请求 continue_handler（群聊为 Director 裁决）决定是否收尾。
                        let should_continue = self
                            .continue_handler
                            .as_ref()
                            .map(|h| h(iteration, STALL_THRESHOLD))
                            .unwrap_or(false);
                        if should_continue {
                            stall_prompted = false;
                            stall_count = 0;
                            log::info!("[AgentLoop] 停滞经裁决继续推进");
                        } else {
                            log::warn!("[AgentLoop] 连续无进展（{} 轮）且裁决收尾，进入总结收尾", STALL_THRESHOLD + 1);
                            return self.finish_with_summary(&messages, iteration).await;
                        }
                    }
                }
            }
        }
    }

    /// 优雅收尾：终止前总结已完成的工作，避免返回空内容，或二次执行时从头再来。
    ///
    /// 优先追加一轮非流式"总结轮"（基于最近工具结果归纳最终结论）；
    /// 总结轮失败或超时则退化为从工具调用链提取确定性摘要。
    async fn finish_with_summary(
        &self,
        messages: &[ChatMessage],
        iteration: usize,
    ) -> Result<AgentLoopOutput, String> {
        let tool_calls = extract_tool_calls(messages);
        let summary_text = self.summarize_work(messages).await;
        log::info!("[AgentLoop] 收尾完成: iteration={}, summary_len={}", iteration, summary_text.len());
        self.emit_to_frontend(AgentLoopEvent::Done {
            content: summary_text.clone(),
        });
        Ok(AgentLoopOutput {
            content: summary_text,
            messages: strip_system(messages),
            tool_calls,
        })
    }

    /// 尝试用 LLM 总结已完成的工作；失败时退化为工具调用链摘要。
    async fn summarize_work(&self, messages: &[ChatMessage]) -> String {
        let fallback = build_work_summary(messages);
        if fallback.is_empty() {
            return String::new();
        }
        // 追加总结指令，非流式单次调用（不触发 on_delta，避免前端流式与收尾文本混淆）。
        let mut summary_messages = messages.to_vec();
        summary_messages.push(ChatMessage::user(
            "请根据以上对话（尤其是已执行的工具调用及其结果）总结你到目前为止已完成的工作、获得的关键信息与结论，作为最终回答输出。不要执行任何新工具，不要继续原任务，不要输出计划。",
        ));
        let request = ChatRequest {
            model: self.model.clone(),
            messages: summary_messages,
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: Some(0.5),
            max_tokens: Some(2000),
        };
        match self.client.chat(&request).await {
            Ok(resp) if !resp.content.trim().is_empty() => resp.content,
            _ => fallback,
        }
    }
}

/// 从工具调用链生成确定性工作摘要（工具名 + 结果摘要），供收尾兜底与失败续接使用。
fn build_work_summary(messages: &[ChatMessage]) -> String {
    let steps = extract_tool_calls(messages);
    let mut lines: Vec<String> = Vec::new();
    for step in steps {
        if step.step_type == "tool_result" {
            let name = step.tool_name.as_deref().unwrap_or("tool");
            let result = step.tool_result.as_deref().unwrap_or("");
            let truncated = if result.chars().count() > 200 {
                let s: String = result.chars().take(200).collect();
                format!("{}…", s)
            } else {
                result.to_string()
            };
            lines.push(format!("- {}：{}", name, truncated));
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("已完成以下工作（工具调用记录）：\n{}", lines.join("\n"))
    }
}

/// 剔除 system 消息（主 system prompt 与循环中注入的强制提示），
/// 仅保留 user/assistant/tool 对话消息，供会话上下文持久化。
fn strip_system(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .filter(|m| m.role != "system")
        .cloned()
        .collect()
}

/// 从 AgentLoop 累积的对话消息中还原工具调用链（tool_start/tool_result），供消息持久化溯源。
///
/// 工具执行成功与否通过 tool_result 内容的错误前缀判定，仅用于前端展示（✅/❌）。
fn extract_tool_calls(messages: &[ChatMessage]) -> Vec<ThinkingChainStep> {
    let mut steps: Vec<ThinkingChainStep> = Vec::new();
    let mut name_by_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let now = crate::utils::now();

    for m in messages {
        match m.role.as_str() {
            "assistant" => {
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        name_by_id.insert(tc.id.clone(), tc.function.name.clone());
                        steps.push(ThinkingChainStep {
                            id: tc.id.clone(),
                            step_type: "tool_start".to_string(),
                            content: None,
                            tool_name: Some(tc.function.name.clone()),
                            tool_args: Some(tc.function.arguments.clone()),
                            tool_result: None,
                            tool_success: None,
                            file_path: None,
                            file_diff: None,
                            timestamp: now,
                        });
                    }
                }
            }
            "tool" => {
                if let Some(call_id) = &m.tool_call_id {
                    let tool_name = name_by_id.get(call_id).cloned();
                    let raw = m.content.clone().unwrap_or_default();
                    let success = !raw.starts_with("错误:")
                        && !raw.contains("拒绝了工具调用")
                        && !raw.starts_with("安全拦截")
                        && !raw.starts_with("安全规则拦截")
                        && !raw.starts_with("工具执行超时");
                    steps.push(ThinkingChainStep {
                        id: call_id.clone(),
                        step_type: "tool_result".to_string(),
                        content: None,
                        tool_name,
                        tool_args: None,
                        tool_result: Some(raw),
                        tool_success: Some(success),
                        file_path: None,
                        file_diff: None,
                        timestamp: now,
                    });
                }
            }
            _ => {}
        }
    }

    steps
}

/// 检查 write_file 的路径是否已在授权缓存中
fn is_write_path_approved(
    approved_files: &std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    approved_dirs: &std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    arguments: &str,
) -> bool {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let raw_path = match args["path"].as_str() {
        Some(p) => p,
        None => return false,
    };
    let path = std::path::PathBuf::from(raw_path);
    // 尝试规范化路径
    let check = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => path,
    };
    let files = approved_files.lock().unwrap();
    let dirs = approved_dirs.lock().unwrap();
    files.contains(&check) || dirs.iter().any(|d| check.starts_with(d))
}

/// 将 write_file 的路径加入授权缓存
fn cache_approved_path(
    approved_files: &std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    approved_dirs: &std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    arguments: &str,
) {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(_) => return,
    };
    let raw_path = match args["path"].as_str() {
        Some(p) => p,
        None => return,
    };
    let path = std::path::PathBuf::from(raw_path);
    // 规范化路径
    let canonical = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => path.clone(),
    };

    // 缓存文件路径
    approved_files.lock().unwrap().insert(canonical.clone());

    // 同时缓存父目录（该目录下所有 write_file 后续全免审批）
    if let Some(parent) = canonical.parent() {
        approved_dirs.lock().unwrap().insert(parent.to_path_buf());
        log::info!("[AgentLoop] 路径授权缓存: file={}, dir={}", canonical.display(), parent.display());
    }
}

/// 检查 execute_command 的命令是否已在授权缓存中
fn is_execute_command_approved(
    approved_commands: &std::sync::Mutex<std::collections::HashSet<String>>,
    cmd: &str,
) -> bool {
    let normalized = cmd.trim().to_lowercase();
    approved_commands.lock().unwrap().contains(&normalized)
}

/// 将 execute_command 的命令加入授权缓存
fn cache_approved_command(
    approved_commands: &std::sync::Mutex<std::collections::HashSet<String>>,
    cmd: &str,
) {
    let normalized = cmd.trim().to_lowercase();
    approved_commands.lock().unwrap().insert(normalized.clone());
    log::info!("[AgentLoop] 命令授权缓存: {}", normalized);
}

