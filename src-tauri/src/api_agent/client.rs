//! HTTP + SSE 流式 API 客户端
//!
//! 支持 OpenAI 兼容协议的 Chat Completion API，包括：
//! - 非流式（POST 一次性返回）
//! - 流式（SSE 逐块返回）

use crate::api_agent::types::*;
use std::time::Duration;

/// OpenAI 兼容 API 客户端
pub struct ApiClient {
    http: reqwest::Client,
    api_endpoint: String,
    api_key: String,
}

/// Chat Completion 响应
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
}

impl ApiClient {
    pub fn new(api_endpoint: String, api_key: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(300)) // 5 分钟超时（长对话）
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            api_endpoint,
            api_key,
        }
    }

    /// 发送 Chat Completion 请求（流式）
    ///
    /// 返回 (完整内容, 工具调用列表, finish_reason)
    pub async fn chat_stream(
        &self,
        request: &ChatRequest,
        mut on_chunk: impl FnMut(&str),
        mut on_tool_start: impl FnMut(&str, &str, &str), // (call_id, name, arguments)
    ) -> Result<ChatResponse, String> {
        let url = format!("{}/chat/completions", self.api_endpoint.trim_end_matches('/'));

        let response = self.http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(|e| format!("API 请求失败: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("API 返回错误 (HTTP {}): {}", status, body));
        }

        let body = response.text().await.map_err(|e| format!("读取响应失败: {}", e))?;

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason = String::from("stop");
        let mut pending_tool_calls: std::collections::HashMap<u32, ToolCall> = std::collections::HashMap::new();

        // 解析 SSE 事件流
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || !line.starts_with("data: ") {
                continue;
            }

            let data = &line[6..]; // 去掉 "data: " 前缀
            if data == "[DONE]" {
                break;
            }

            let chunk: StreamChunk = match serde_json::from_str(data) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if let Some(choices) = &chunk.choices {
                for choice in choices {
                    // 记录 finish_reason
                    if let Some(ref fr) = choice.finish_reason {
                        finish_reason = fr.clone();
                    }

                    if let Some(delta) = &choice.delta {
                        // 文本内容
                        if let Some(ref c) = delta.content {
                            content.push_str(c);
                            on_chunk(c);
                        }

                        // 工具调用（增量式）
                        if let Some(tc_deltas) = &delta.tool_calls {
                            for tc in tc_deltas {
                                let idx = tc.index.unwrap_or(0);
                                let entry = pending_tool_calls.entry(idx).or_insert_with(|| {
                                    ToolCall {
                                        id: String::new(),
                                        call_type: "function".into(),
                                        function: ToolCallFunction {
                                            name: String::new(),
                                            arguments: String::new(),
                                        },
                                    }
                                });

                                if let Some(ref id) = tc.id {
                                    entry.id = id.clone();
                                }
                                if let Some(ref func) = tc.function {
                                    if let Some(ref name) = func.name {
                                        entry.function.name.push_str(name);
                                    }
                                    if let Some(ref args) = func.arguments {
                                        entry.function.arguments.push_str(args);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 收尾：将待处理的工具调用加入列表
        let mut indices: Vec<u32> = pending_tool_calls.keys().copied().collect();
        indices.sort();
        for idx in indices {
            if let Some(tc) = pending_tool_calls.remove(&idx) {
                if !tc.function.name.is_empty() {
                    on_tool_start(&tc.id, &tc.function.name, &tc.function.arguments);
                    tool_calls.push(tc);
                }
            }
        }

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
        })
    }

    /// 发送 Chat Completion 请求（非流式）
    pub async fn chat(
        &self,
        request: &ChatRequest,
    ) -> Result<ChatResponse, String> {
        let url = format!("{}/chat/completions", self.api_endpoint.trim_end_matches('/'));

        let mut req = request.clone();
        req.stream = false;

        let response = self.http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("API 请求失败: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("API 返回错误 (HTTP {}): {}", status, body));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("解析响应失败: {}", e))?;

        let choice = body["choices"][0].clone();
        let message = &choice["message"];

        let content = message["content"].as_str().unwrap_or("").to_string();
        let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop").to_string();

        let tool_calls: Vec<ToolCall> = message["tool_calls"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| serde_json::from_value(tc.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
        })
    }
}
