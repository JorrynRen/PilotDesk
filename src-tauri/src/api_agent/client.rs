//! HTTP + SSE 流式 API 客户端
//!
//! 支持 OpenAI 兼容协议的 Chat Completion API 和 Anthropic Messages API。

use crate::api_agent::types::*;
use std::time::Duration;
use futures::StreamExt;

/// 安全截断 UTF-8 字符串至指定字节数（边界对齐到字符）
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes { return s; }
    s.char_indices()
        .take_while(|&(i, _)| i < max_bytes)
        .last()
        .map_or(s, |(_, c)| &s[..c.len_utf8()])
        .get(..max_bytes)
        .unwrap_or(s)
}

/// 是否需要在发送时把该消息的 content 转成多模态 parts（图片输入）
fn has_images(msg: &ChatMessage) -> bool {
    msg.role == "user" && msg.images.as_ref().map_or(false, |v| !v.is_empty())
}

/// 将单条消息转成 OpenAI Chat Completion 的 wire 格式（含多模态图片支持）
fn openai_message_json(msg: &ChatMessage) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("role".into(), serde_json::Value::String(msg.role.clone()));

    if has_images(msg) {
        let mut parts: Vec<serde_json::Value> = Vec::new();
        if let Some(text) = &msg.content {
            if !text.is_empty() {
                parts.push(serde_json::json!({"type": "text", "text": text}));
            }
        }
        if let Some(imgs) = &msg.images {
            for img in imgs {
                parts.push(serde_json::json!({"type": "image_url", "image_url": {"url": img}}));
            }
        }
        obj.insert("content".into(), serde_json::Value::Array(parts));
    } else if let Some(text) = &msg.content {
        obj.insert("content".into(), serde_json::Value::String(text.clone()));
    }

    if let Some(tcs) = &msg.tool_calls {
        obj.insert("tool_calls".into(), serde_json::to_value(tcs).unwrap_or_default());
    }
    if let Some(id) = &msg.tool_call_id {
        obj.insert("tool_call_id".into(), serde_json::Value::String(id.clone()));
    }
    if let Some(name) = &msg.name {
        obj.insert("name".into(), serde_json::Value::String(name.clone()));
    }
    serde_json::Value::Object(obj)
}

/// 构建 OpenAI Chat Completion 请求体（复制 ChatRequest 的 serde 省略语义 + 多模态 messages）
fn openai_request_body(request: &ChatRequest) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), serde_json::json!(request.model));
    let messages: Vec<serde_json::Value> = request.messages.iter().map(openai_message_json).collect();
    body.insert("messages".into(), serde_json::Value::Array(messages));
    if let Some(tools) = &request.tools {
        body.insert("tools".into(), serde_json::to_value(tools).unwrap_or_default());
    }
    if let Some(tc) = &request.tool_choice {
        body.insert("tool_choice".into(), serde_json::json!(tc));
    }
    body.insert("stream".into(), serde_json::json!(request.stream));
    if let Some(t) = request.temperature {
        body.insert("temperature".into(), serde_json::json!(t));
    }
    if let Some(mt) = request.max_tokens {
        body.insert("max_tokens".into(), serde_json::json!(mt));
    }
    serde_json::Value::Object(body)
}

/// 将图片 URL（base64 data URL 或 http(s) URL）转成 Anthropic image source
fn anthropic_image_source(img: &str) -> serde_json::Value {
    if let Some(stripped) = img.strip_prefix("data:") {
        if let Some(comma) = stripped.find(',') {
            let (meta, data) = stripped.split_at(comma);
            let media_type = meta.trim_end_matches(";base64").to_string();
            let data = &data[1..]; // 跳过逗号
            return serde_json::json!({"type": "base64", "media_type": media_type, "data": data});
        }
    }
    serde_json::json!({"type": "url", "url": img})
}

/// 将单条消息转成 Anthropic Messages 的 wire 格式（含多模态图片支持）
fn anthropic_message_json(msg: &ChatMessage) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("role".into(), serde_json::Value::String(msg.role.clone()));

    if has_images(msg) {
        let mut blocks: Vec<serde_json::Value> = Vec::new();
        if let Some(text) = &msg.content {
            if !text.is_empty() {
                blocks.push(serde_json::json!({"type": "text", "text": text}));
            }
        }
        if let Some(imgs) = &msg.images {
            for img in imgs {
                blocks.push(serde_json::json!({"type": "image", "source": anthropic_image_source(img)}));
            }
        }
        obj.insert("content".into(), serde_json::Value::Array(blocks));
    } else if let Some(text) = &msg.content {
        obj.insert("content".into(), serde_json::Value::String(text.clone()));
    }

    if let Some(tcs) = &msg.tool_calls {
        obj.insert("tool_calls".into(), serde_json::to_value(tcs).unwrap_or_default());
    }
    if let Some(id) = &msg.tool_call_id {
        obj.insert("tool_call_id".into(), serde_json::Value::String(id.clone()));
    }
    if let Some(name) = &msg.name {
        obj.insert("name".into(), serde_json::Value::String(name.clone()));
    }
    serde_json::Value::Object(obj)
}

/// OpenAI 兼容 API 客户端
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    api_endpoint: String,
    api_key: String,
    #[allow(dead_code)]
    format: ApiFormat,
}

/// Chat Completion 响应
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
}

impl ApiClient {
    pub fn new(api_endpoint: String, api_key: String, format: ApiFormat) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(300)) // 5 分钟超时（长对话）
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            api_endpoint,
            api_key,
            format,
        }
    }

    /// 创建 OpenAI 兼容客户端（默认）
    pub fn new_openai(api_endpoint: String, api_key: String) -> Self {
        Self::new(api_endpoint, api_key, ApiFormat::OpenAI)
    }

    /// 发送 Chat Completion 请求（流式）
    ///
    /// 返回 (完整内容, 工具调用列表, finish_reason)
    /// on_usage 回调返回最终 token 用量
    pub async fn chat_stream(
        &self,
        request: &ChatRequest,
        mut on_chunk: impl FnMut(&str),
        mut on_reasoning: impl FnMut(&str),
        mut on_tool_start: impl FnMut(&str, &str, &str), // (call_id, name, arguments)
        mut on_usage: impl FnMut(u32, u32, u32),        // (prompt, completion, total)
    ) -> Result<ChatResponse, String> {
        let url = format!("{}/chat/completions", self.api_endpoint.trim_end_matches('/'));
        log::info!("[ApiClient] 发送请求到: {}, model={}, msgs_count={}", url, request.model, request.messages.len());

        let response = self.http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&openai_request_body(request))
            .send()
            .await
            .map_err(|e| format!("API 请求失败: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            log::error!("[ApiClient] HTTP 错误 {}: {}", status, &body[..body.len().min(200)]);
            return Err(format!("API 返回错误 (HTTP {}): {}", status, body));
        }

        log::info!("[ApiClient] HTTP 200, 开始流式读取响应体");
        let mut stream = response.bytes_stream();

        let mut content = String::new();
        let mut finish_reason = String::from("stop");
        let mut pending_tool_calls: std::collections::HashMap<u32, ToolCall> = std::collections::HashMap::new();
        // 行缓冲区：跨 chunk 拼接不完整的行（字节缓冲，避免多字节 UTF-8 字符被 chunk 边界截断成乱码）
        let mut line_buf: Vec<u8> = Vec::new();

        // 逐 chunk 读取 HTTP 响应（真正的流式，不等待完整响应）
        while let Some(chunk_result) = stream.next().await {
            let chunk_bytes = chunk_result.map_err(|e| format!("流读取错误: {}", e))?;
            line_buf.extend_from_slice(&chunk_bytes);

            // 逐行处理（只处理以 \n 结尾的完整行）
            while let Some(nl_pos) = line_buf.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&line_buf[..nl_pos]).trim().to_string();
                line_buf.drain(..=nl_pos);

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
                        if let Some(ref fr) = choice.finish_reason {
                            finish_reason = fr.clone();
                        }

                        // 处理 usage（通常在最后一块返回）
                        if let Some(ref usage) = chunk.usage {
                            if let Some(total) = usage.total_tokens {
                                let prompt = usage.prompt_tokens.unwrap_or(0);
                                let completion = usage.completion_tokens.unwrap_or(0);
                                on_usage(prompt, completion, total);
                            }
                        }

                        if let Some(delta) = &choice.delta {
                            // 思维链内容（实时推送）
                            if let Some(ref rc) = delta.reasoning_content {
                                on_reasoning(rc);
                            }

                            // 文本内容（实时推送）
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
        }

        log::info!("[ApiClient] 流式读取完成, content_len={}", content.len());

        // 收尾：将待处理的工具调用加入列表
        let mut tool_calls: Vec<ToolCall> = Vec::new();
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

    /// 发送 Anthropic Messages API 请求（流式）
    pub async fn chat_stream_anthropic(
        &self,
        request: &ChatRequest,
        mut on_chunk: impl FnMut(&str),
        _on_reasoning: impl FnMut(&str),
        mut on_tool_start: impl FnMut(&str, &str, &str),
        mut on_usage: impl FnMut(u32, u32, u32),
    ) -> Result<ChatResponse, String> {
        let url = format!("{}/v1/messages", self.api_endpoint.trim_end_matches('/'));
        log::info!("[Anthropic] 发送请求到: {}, model={}", url, request.model);

        // 构建 Anthropic 请求体
        let anthropic_messages: Vec<serde_json::Value> = request.messages.iter().map(anthropic_message_json).collect();
        let anthropic_req = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "messages": anthropic_messages,
            "stream": true,
            "temperature": request.temperature,
            "tools": request.tools.clone(),
        });

        let response = self.http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .json(&anthropic_req)
            .send()
            .await
            .map_err(|e| format!("API 请求失败: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            log::error!("[Anthropic] HTTP 错误 {}: {}", status, truncate_utf8(&body, 200));
            return Err(format!("API 返回错误 (HTTP {}): {}", status, body));
        }

        let mut stream = response.bytes_stream();
        let mut content = String::new();
        let mut finish_reason = String::from("end_turn");
        let mut pending_tool_calls: std::collections::HashMap<u32, ToolCall> = std::collections::HashMap::new();
        let mut line_buf: Vec<u8> = Vec::new();
        let mut usage_total: Option<u32> = None;

        while let Some(chunk_result) = stream.next().await {
            let chunk_bytes = chunk_result.map_err(|e| format!("流读取错误: {}", e))?;
            line_buf.extend_from_slice(&chunk_bytes);

            // 只关心 `data:` 行，`event:`/空行一律忽略。Anthropic 的 data 行 JSON 自带
            // `type` 字段，无需与 `event:` 行配对；否则当 `event:` 与 `data:` 被拆到
            // 不同 HTTP chunk 时，`event:` 行会被误判为已结束而丢弃后续 `data:`，造成内容截断。
            while let Some(nl_pos) = line_buf.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&line_buf[..nl_pos]).trim().to_string();
                line_buf.drain(..=nl_pos);

                if line.is_empty() || !line.starts_with("data: ") {
                    continue;
                }

                let data = &line[6..]; // 去掉 "data: " 前缀
                let ev: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let Some(ev_type) = ev["type"].as_str() else {
                    continue;
                };

                match ev_type {
                    "content_block_delta" => {
                        let Some(delta_type) = ev["delta"]["type"].as_str() else {
                            continue;
                        };
                        match delta_type {
                            "text_delta" => {
                                if let Some(text) = ev["delta"]["text"].as_str() {
                                    content.push_str(text);
                                    on_chunk(text);
                                }
                            }
                            // 工具调用参数增量：标准协议为 input_json_delta/partial_json，
                            // 部分兼容实现沿用 input_delta/input，二者都做兼容。
                            "input_json_delta" => {
                                if let Some(args) = ev["delta"]["partial_json"].as_str() {
                                    if let Some(last) = pending_tool_calls.values_mut().last() {
                                        last.function.arguments.push_str(args);
                                    }
                                }
                            }
                            "input_delta" => {
                                if let Some(args) = ev["delta"]["input"].as_str() {
                                    if let Some(last) = pending_tool_calls.values_mut().last() {
                                        last.function.arguments.push_str(args);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_start" => {
                        if let Some(block_type) = ev["content_block"]["type"].as_str() {
                            if block_type == "tool_use" {
                                let idx = pending_tool_calls.len() as u32;
                                let tc_id = ev["content_block"]["id"].as_str().unwrap_or("").to_string();
                                let tc_name = ev["content_block"]["name"].as_str().unwrap_or("").to_string();
                                pending_tool_calls.insert(idx, ToolCall {
                                    id: tc_id,
                                    call_type: "function".into(),
                                    function: ToolCallFunction {
                                        name: tc_name,
                                        arguments: String::new(),
                                    },
                                });
                            }
                        }
                    }
                    "message_delta" => {
                        // 标准 Anthropic：stop_reason 与 usage 都随 message_delta 返回。
                        if let Some(reason) = ev["delta"]["stop_reason"].as_str() {
                            finish_reason = reason.to_string();
                        }
                        if let Some(u) = ev.get("usage") {
                            if let Some(total) = u["output_tokens"].as_u64() {
                                usage_total = Some(total as u32);
                            }
                        }
                    }
                    "message_stop" => {
                        // 部分兼容实现可能在 message_stop 携带 stop_reason，兜底读取。
                        if let Some(reason) = ev["stop_reason"].as_str() {
                            finish_reason = reason.to_string();
                        }
                    }
                    _ => {}
                }
            }
        }

        // 收集工具调用
        let mut tool_calls: Vec<ToolCall> = Vec::new();
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

        // 估算 prompt tokens（Anthropic 流式不直接返回 prompt tokens）
        let prompt_tokens = crate::api_agent::context::TokenEstimator::estimate(&content) as u32;
        let completion_tokens = usage_total.unwrap_or_else(|| content.len() as u32 / 3);
        let total_tokens = prompt_tokens + completion_tokens;
        on_usage(prompt_tokens, completion_tokens, total_tokens);

        log::info!("[Anthropic] 流式读取完成, content_len={}, tool_calls={}", content.len(), tool_calls.len());

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
        })
    }
}

impl ApiClient {
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
            .json(&openai_request_body(&req))
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

    /// 发送 Anthropic Messages API 请求（非流式）
    pub async fn chat_anthropic(
        &self,
        request: &ChatRequest,
    ) -> Result<ChatResponse, String> {
        let url = format!("{}/v1/messages", self.api_endpoint.trim_end_matches('/'));

        let anthropic_messages: Vec<serde_json::Value> = request.messages.iter().map(anthropic_message_json).collect();
        let body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens.unwrap_or(1024),
            "messages": anthropic_messages,
            "stream": false,
            "temperature": request.temperature,
        });

        let response = self.http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .json(&body)
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

        // Anthropic 返回 content 数组，取 text 块拼接
        let mut content = String::new();
        if let Some(blocks) = body["content"].as_array() {
            for block in blocks {
                if block["type"] == "text" {
                    if let Some(t) = block["text"].as_str() {
                        content.push_str(t);
                    }
                }
            }
        }

        let finish_reason = body["stop_reason"].as_str().unwrap_or("end_turn").to_string();

        Ok(ChatResponse {
            content,
            tool_calls: Vec::new(),
            finish_reason,
        })
    }
}
