//! Agent Loop 任务编排
//!
//! 为 API Agent 提供 tool-calling 循环能力。
//! CLI Agent（Claude/Hermes/CodeX）不使用此模块。

use crate::api_agent::client::ApiClient;
use crate::api_agent::types::*;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 工具风险等级
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// 低风险：读取文件、搜索等（静默执行）
    Low,
    /// 中风险：修改文件、网络请求等（确认后执行）
    Medium,
    /// 高风险：执行命令、删除文件等（严格确认）
    High,
}

impl RiskLevel {
    pub fn description(&self) -> &str {
        match self {
            RiskLevel::Low => "低风险操作",
            RiskLevel::Medium => "中风险操作（可能修改文件或访问网络）",
            RiskLevel::High => "高风险操作（执行系统命令或删除文件）",
        }
    }
}

/// 当前会话的授权级别
/// 控制哪些风险等级的工具需要用户确认
#[derive(Debug, Clone)]
pub enum AuthLevel {
    /// 静默模式：所有操作自动执行
    Silent,
    /// 确认模式：中风险及以上需要确认（默认）
    Confirm,
    /// 阻止模式：高风险操作被阻止
    Block,
}

/// 工具执行器 trait
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    /// 工具名称
    fn name(&self) -> &str;
    /// 工具描述
    fn description(&self) -> &str;
    /// 参数定义（JSON Schema）
    fn parameters(&self) -> serde_json::Value;
    /// 工具风险等级（默认 Low）
    fn risk_level(&self) -> RiskLevel { RiskLevel::Low }
    /// 执行工具
    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String>;
}

/// 审批回调：返回 true 表示批准，false 表示拒绝
pub type ApprovalHandler = Box<dyn Fn(&str, &str, &str, RiskLevel) -> bool + Send + Sync>;

/// 工具注册表
pub struct ToolRegistry {
    handlers: Vec<Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    pub fn register(&mut self, handler: Arc<dyn ToolHandler>) {
        self.handlers.push(handler);
    }

    /// 获取所有工具的 OpenAI 格式定义
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.handlers
            .iter()
            .map(|h| {
                ToolDefinition::new(h.name(), h.description(), h.parameters())
            })
            .collect()
    }

    /// 执行指定工具
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String, String> {
        let args: serde_json::Value = serde_json::from_str(arguments)
            .unwrap_or(serde_json::Value::Null);

        for handler in &self.handlers {
            if handler.name() == name {
                return handler.execute(args).await;
            }
        }

        Err(format!("未知工具: {}", name))
    }

    pub fn has_handler(&self, name: &str) -> bool {
        self.handlers.iter().any(|h| h.name() == name)
    }

    /// 获取工具的风险等级
    pub fn get_risk_level(&self, name: &str) -> Option<RiskLevel> {
        self.handlers.iter().find(|h| h.name() == name).map(|h| h.risk_level())
    }
}

/// Agent Loop 配置
pub struct AgentLoopConfig {
    /// 最大迭代次数（防止无限循环）
    pub max_iterations: usize,
    /// 系统提示词（含 MEMORY.md + USER.md + 技能列表）
    pub system_prompt: String,
    /// 可用工具列表
    pub tools: Vec<ToolDefinition>,
    /// 会话历史消息（不含 system prompt）
    pub messages: Vec<ChatMessage>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            system_prompt: String::new(),
            tools: Vec::new(),
            messages: Vec::new(),
        }
    }
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
}

impl AgentLoop {
    pub fn new(client: ApiClient, tool_registry: Arc<ToolRegistry>, model: String) -> Self {
        Self {
            client,
            tool_registry,
            model,
            temperature: 0.7,
            auth_level: AuthLevel::Confirm,
            approval_handler: None,
        }
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = temp;
        self
    }

    /// 设置授权级别
    pub fn with_auth_level(mut self, level: AuthLevel) -> Self {
        self.auth_level = level;
        self
    }

    /// 设置审批回调
    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(handler);
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

    /// 执行 Agent Loop
    pub async fn run(
        &self,
        config: AgentLoopConfig,
        stream_tx: broadcast::Sender<AgentLoopEvent>,
    ) -> Result<String, String> {
        let mut messages: Vec<ChatMessage> = Vec::new();

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

        // 3. Agent Loop 主循环
        for _iteration in 0..config.max_iterations {
            let request = ChatRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
                tool_choice: None,
                stream: true,
                temperature: Some(self.temperature),
                max_tokens: None,
            };

            // 调用 LLM（流式）
            let mut content_buffer = String::new();

            let stream_tx_clone = stream_tx.clone();
            let response = self.client.chat_stream(
                &request,
                |chunk| {
                    content_buffer.push_str(chunk);
                    let _ = stream_tx_clone.send(AgentLoopEvent::Chunk {
                        content: chunk.to_string(),
                    });
                },
                |call_id, name, arguments| {
                    let _ = stream_tx_clone.send(AgentLoopEvent::ToolStart {
                        id: call_id.to_string(),
                        name: name.to_string(),
                        arguments: arguments.to_string(),
                    });
                },
            ).await.map_err(|e| {
                let _ = stream_tx.send(AgentLoopEvent::Error { message: e.clone() });
                e
            })?;

            // 无工具调用 → 返回最终答案
            if response.tool_calls.is_empty() {
                let _ = stream_tx.send(AgentLoopEvent::Done {
                    content: content_buffer.clone(),
                });
                return Ok(content_buffer);
            }

            // 保存 assistant 消息（含 tool_calls）
            messages.push(ChatMessage::assistant_with_tools(response.tool_calls.clone()));

            // 执行每个工具调用
            for tc in &response.tool_calls {
                // 查找工具的风险等级
                let risk = self.tool_registry
                    .get_risk_level(&tc.function.name)
                    .unwrap_or(RiskLevel::Medium);

                // 风险审批
                if self.needs_approval(&risk) {
                    let approved = if let Some(ref handler) = self.approval_handler {
                        let _ = stream_tx.send(AgentLoopEvent::ApprovalRequired {
                            call_id: tc.id.clone(),
                            tool_name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                            risk_description: risk.description().to_string(),
                        });
                        handler(&tc.id, &tc.function.name, &tc.function.arguments, risk.clone())
                    } else {
                        // 无审批回调时，记录日志并拒绝
                        log::warn!("[AgentLoop] 无审批回调，拒绝高风险工具: {}", tc.function.name);
                        false
                    };

                    if !approved {
                        let msg = format!("用户拒绝了工具调用: {}", tc.function.name);
                        let _ = stream_tx.send(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: msg.clone(),
                            success: false,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &msg));
                        continue;
                    }
                }

                // 执行工具
                match self.tool_registry.execute(&tc.function.name, &tc.function.arguments).await {
                    Ok(result) => {
                        let _ = stream_tx.send(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: result.clone(),
                            success: true,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &result));
                    }
                    Err(err) => {
                        let _ = stream_tx.send(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: err.clone(),
                            success: false,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &format!("错误: {}", err)));
                    }
                }
            }
        }

        let msg = format!("达到最大迭代次数 ({})", config.max_iterations);
        let _ = stream_tx.send(AgentLoopEvent::Error {
            message: msg.clone(),
        });
        Err(msg)
    }
}

/// 创建一个内置工具的便捷宏（默认低风险）
#[macro_export]
macro_rules! builtin_tool {
    ($name:expr, $desc:expr, $params:expr, $handler:expr) => {{
        struct BuiltinTool {
            name: &'static str,
            desc: &'static str,
            params: serde_json::Value,
            risk: RiskLevel,
            handler: Box<dyn Fn(serde_json::Value) -> Result<String, String> + Send + Sync>,
        }

        #[async_trait::async_trait]
        impl ToolHandler for BuiltinTool {
            fn name(&self) -> &str { self.name }
            fn description(&self) -> &str { self.desc }
            fn parameters(&self) -> serde_json::Value { self.params.clone() }
            fn risk_level(&self) -> RiskLevel { self.risk.clone() }
            async fn execute(&self, args: serde_json::Value) -> Result<String, String> {
                (self.handler)(args)
            }
        }

        Arc::new(BuiltinTool {
            name: $name,
            desc: $desc,
            params: $params,
            risk: RiskLevel::Low,
            handler: Box::new($handler),
        })
    }};
}

/// 创建一个内置工具（指定风险等级）
#[macro_export]
macro_rules! builtin_tool_risky {
    ($name:expr, $desc:expr, $params:expr, $risk:expr, $handler:expr) => {{
        struct BuiltinTool {
            name: &'static str,
            desc: &'static str,
            params: serde_json::Value,
            risk: RiskLevel,
            handler: Box<dyn Fn(serde_json::Value) -> Result<String, String> + Send + Sync>,
        }

        #[async_trait::async_trait]
        impl ToolHandler for BuiltinTool {
            fn name(&self) -> &str { self.name }
            fn description(&self) -> &str { self.desc }
            fn parameters(&self) -> serde_json::Value { self.params.clone() }
            fn risk_level(&self) -> RiskLevel { self.risk.clone() }
            async fn execute(&self, args: serde_json::Value) -> Result<String, String> {
                (self.handler)(args)
            }
        }

        Arc::new(BuiltinTool {
            name: $name,
            desc: $desc,
            params: $params,
            risk: $risk,
            handler: Box::new($handler),
        })
    }};
}
