//! 子代理（task）工具：将子任务卸载到独立的单次 LLM 调用，返回聚焦结果。
//!
//! 这是「子代理/并行」能力的最小实现——使用与主 Agent 相同的模型，
//! 以独立的 system prompt 运行一次无工具调用，避免污染主对话上下文。
// 自 api_agent/subagent.rs 迁入（工具架构统一 v1.0，轮 7），逻辑不变。

use crate::api_agent::client::ApiClient;
use crate::api_agent::types::{ApiFormat, ChatMessage, ChatRequest};
use crate::tools::{RiskLevel, ToolHandler};
use async_trait::async_trait;

const SUBAGENT_SYSTEM_PROMPT: &str = "你是一个专注的子代理。请根据用户给出的子任务，\
独立、完整地完成分析或写作，并直接返回最终结果。不要询问澄清问题，不要输出过程，\
只输出最终答案。结果应简洁、准确、可直接供主代理使用。";

/// 子代理工具：单次 LLM 调用执行一个子任务
pub struct TaskTool {
    client: ApiClient,
    model: String,
    api_format: ApiFormat,
}

impl TaskTool {
    pub fn new(client: ApiClient, model: String, api_format: ApiFormat) -> Self {
        Self {
            client,
            model,
            api_format,
        }
    }
}

#[async_trait]
impl ToolHandler for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "将子任务交给独立子代理处理并返回聚焦结果。适用于需要并行调研、分析、规划或写作，\
且不希望污染主对话上下文的场景。子代理不访问工具，仅基于模型知识回答。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "子任务描述，越具体越好"
                }
            },
            "required": ["prompt"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let prompt = arguments["prompt"].as_str().ok_or("缺少 prompt 参数")?;

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(SUBAGENT_SYSTEM_PROMPT),
                ChatMessage::user(prompt),
            ],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: Some(0.3),
            max_tokens: Some(2048),
            response_format: None,
        };

        let resp = if matches!(self.api_format, ApiFormat::Anthropic) {
            self.client.chat_anthropic(&request).await?
        } else {
            self.client.chat(&request).await?
        };

        if resp.content.trim().is_empty() {
            return Err("子代理返回了空结果".to_string());
        }
        Ok(resp.content)
    }
}
