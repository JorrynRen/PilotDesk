//! HTTP + SSE 流式 API 客户端
//!
//! 支持 OpenAI 兼容协议的 Chat Completion API 和 Anthropic Messages API。

use crate::api_agent::types::*;
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// SSE 流式读取的 chunk 间隔空闲默认阈值（秒）：未显式配置时使用；超过该时长仍未收到任何字节，
/// 判定为上游连接假死（真超时）并终止本次调用。区分于“慢但仍在推进”——只要持续有数据
/// （reasoning/文本/工具增量都算活性），就不触发。会话与群聊共用该请求层，可通过
/// `with_stream_idle` 覆盖（0 = 禁用空闲检测，仅调试用）。
const DEFAULT_STREAM_CHUNK_IDLE_SECS: u64 = 90;

/// HTTP 429 限流时的最大自动重试次数（不含首次请求）：总尝试 = 1 + RATE_LIMIT_RETRIES = 3 次。
const RATE_LIMIT_RETRIES: usize = 2;
/// HTTP 429 重试的退避间隔基数（毫秒）：第 `attempt` 次尝试失败后 sleep 基数 × attempt
/// （attempt 从 1 起，即 0.8s → 1.6s），给服务商额度窗口留出恢复时间。
const RATE_LIMIT_RETRY_BASE_DELAY_MS: u64 = 800;

/// 取消轮询间隔（毫秒）：决定"停止生成"在流式空闲等待期间的响应粒度。
const ABORT_POLL_MS: u64 = 200;

/// 取消轮询：每 `ABORT_POLL_MS` 检查一次取消标志，已置位即完成。
/// 仅在注入了取消标志时由 `select!` 的 `if` 守卫启用（未注入则不会进入轮询）。
async fn abort_poll(token: &Option<Arc<AtomicBool>>) {
    loop {
        if token.as_ref().is_some_and(|t| t.load(Ordering::SeqCst)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(ABORT_POLL_MS)).await;
    }
}

/// 安全截断 UTF-8 字符串至指定字节数（边界对齐到字符）
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    s.char_indices()
        .take_while(|&(i, _)| i < max_bytes)
        .last()
        .map_or(s, |(_, c)| &s[..c.len_utf8()])
        .get(..max_bytes)
        .unwrap_or(s)
}

/// 判断上游错误是否属于"上下文窗口溢出"（供 AgentLoop 触发溢出恢复）。
/// 覆盖 OpenAI 兼容与 Anthropic 两种协议的常见英文文案；大小写不敏感，
/// 未命中即视为普通错误，不做恢复。
pub(crate) fn is_context_overflow(msg: &str) -> bool {
    let m = msg.to_lowercase();
    [
        "maximum context length",
        "context_window_exceeded",
        "context length exceeded",
        "prompt is too long",
        "prompt too long",
        "input is too long",
        "too many tokens",
        "reduce the length",
        "over context_window",
        "token limit",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// 生成非 2xx 响应（含 429 重试耗尽）的错误文案。
/// HTTP 429：模型本身可用，只是被服务商限流/免费额度打回，返回带解决建议的中文提示并保留原始 body；
/// 其余状态码保持历史格式 `API 返回错误 (HTTP <status>): <body>` 逐字不变（避免影响上层错误匹配逻辑）。
/// 参数用 `StatusCode` 而非 u16：非 429 分支沿用其 Display 输出（含原因短语），与历史文案一致。
fn http_error_message(status: reqwest::StatusCode, body: &str) -> String {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        format!(
            "API 返回错误 (HTTP 429)：模型可用但触发服务商限流/免费额度上限（已自动重试仍失败）。可稍后重试、更换模型，或检查/升级服务商套餐（如 Token Plan）。原始信息: {}",
            body
        )
    } else {
        format!("API 返回错误 (HTTP {}): {}", status, body)
    }
}

/// 规整温度到 2 位小数（在 f64 域计算再输出）。
/// 若按 f32/f64 原始二进制值直接序列化，0.7 会被输出为 0.699999988079071，
/// 被严格校验小数位数的服务商（如错误码 1210：限制小数点 2 位）拒绝（HTTP 400）。
fn round_temperature(t: f64) -> f64 {
    (t * 100.0).round() / 100.0
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
        obj.insert(
            "tool_calls".into(),
            serde_json::to_value(tcs).unwrap_or_default(),
        );
    }
    if let Some(id) = &msg.tool_call_id {
        obj.insert("tool_call_id".into(), serde_json::Value::String(id.clone()));
    }
    if let Some(name) = &msg.name {
        obj.insert("name".into(), serde_json::Value::String(name.clone()));
    }
    if let Some(rc) = &msg.reasoning_content {
        // 思考模式（DeepSeek 等）要求原样回传上轮 reasoning_content，否则 HTTP 400。
        obj.insert(
            "reasoning_content".into(),
            serde_json::Value::String(rc.clone()),
        );
    }
    serde_json::Value::Object(obj)
}

/// 构建 OpenAI Chat Completion 请求体（复制 ChatRequest 的 serde 省略语义 + 多模态 messages）
fn openai_request_body(request: &ChatRequest) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), serde_json::json!(request.model));
    let messages: Vec<serde_json::Value> =
        request.messages.iter().map(openai_message_json).collect();
    body.insert("messages".into(), serde_json::Value::Array(messages));
    if let Some(tools) = &request.tools {
        body.insert(
            "tools".into(),
            serde_json::to_value(tools).unwrap_or_default(),
        );
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
    if let Some(rf) = &request.response_format {
        body.insert("response_format".into(), rf.clone());
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
                blocks.push(
                    serde_json::json!({"type": "image", "source": anthropic_image_source(img)}),
                );
            }
        }
        obj.insert("content".into(), serde_json::Value::Array(blocks));
    } else if let Some(text) = &msg.content {
        obj.insert("content".into(), serde_json::Value::String(text.clone()));
    }

    if let Some(tcs) = &msg.tool_calls {
        obj.insert(
            "tool_calls".into(),
            serde_json::to_value(tcs).unwrap_or_default(),
        );
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
    /// 流式 chunk 空闲超时；None = 禁用空闲检测（仅调试用）。
    stream_idle: Option<Duration>,
    /// 协作式取消标志（会话/群聊注入）：流式读取循环每收到一段便检查一次，
    /// 已取消则立刻停止读取并保留已累积内容——否则"停止生成"要等到整段生成结束才生效。
    abort: Option<Arc<std::sync::atomic::AtomicBool>>,
}

/// Chat Completion 响应
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    /// 思考模式（DeepSeek 等）的思维链内容；需随下一轮 assistant 消息原样回传。
    pub reasoning_content: String,
    /// 非流式响应中解析到的用量（流式路径经 on_usage 回调逐请求上报，此处为 None）。
    pub usage: Option<UsageRecord>,
}

/// 从 OpenAI 兼容非流式响应提取用量。`total_tokens` 缺失时以 prompt+completion 兜底；
/// 缓存读取兼容顶层 `prompt_cache_hit_tokens`（DeepSeek）与 `prompt_tokens_details.cached_tokens`
/// （OpenAI/Qwen）两种上报位置；该兼容路径无缓存写入字段，write 记为 0。
///
/// 口径：OpenAI 协议的 `prompt_tokens` **含**缓存命中部分，而本应用落库统一为「四桶互斥」
/// （prompt=未命中输入，与 Anthropic 的 `input_tokens` 同口径），故此处把命中部分从输入里拆出，
/// 使命中率公式 `read / (prompt + read + write)` 对两种协议都成立。
fn usage_from_openai_value(v: &serde_json::Value) -> Option<UsageRecord> {
    let prompt_total = v["prompt_tokens"].as_u64()? as u32;
    let completion = v["completion_tokens"].as_u64().unwrap_or(0) as u32;
    let cache_read = v["prompt_cache_hit_tokens"]
        .as_u64()
        .or_else(|| v["prompt_tokens_details"]["cached_tokens"].as_u64())
        .unwrap_or(0) as u32;
    Some(UsageRecord {
        prompt: prompt_total.saturating_sub(cache_read),
        completion,
        total: v["total_tokens"]
            .as_u64()
            .unwrap_or((prompt_total + completion) as u64) as u32,
        cache_read,
        cache_write: 0,
    })
}

/// 从 Anthropic 非流式响应提取用量：`input_tokens`/`output_tokens` 为往返口径，
/// 缓存命中读取与缓存写入分别取自 cache_read_input_tokens / cache_creation_input_tokens。
/// `input_tokens` 已不含缓存部分，无需拆分；合计取四桶之和（与 OpenAI 路径同口径）。
fn usage_from_anthropic_value(v: &serde_json::Value) -> Option<UsageRecord> {
    let prompt = v["input_tokens"].as_u64()? as u32;
    let completion = v["output_tokens"].as_u64().unwrap_or(0) as u32;
    let cache_read = v["cache_read_input_tokens"].as_u64().unwrap_or(0) as u32;
    let cache_write = v["cache_creation_input_tokens"].as_u64().unwrap_or(0) as u32;
    Some(UsageRecord {
        prompt,
        completion,
        total: prompt + completion + cache_read + cache_write,
        cache_read,
        cache_write,
    })
}

impl ApiClient {
    pub fn new(api_endpoint: String, api_key: String, format: ApiFormat) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(600)) // 单请求总时长上限（长对话/长生成；真正的“卡死”由流式 chunk 空闲检测判定）
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            api_endpoint,
            api_key,
            format,
            stream_idle: Some(Duration::from_secs(DEFAULT_STREAM_CHUNK_IDLE_SECS)),
            abort: None,
        }
    }

    /// 覆盖流式 chunk 空闲超时（秒）；0 = 禁用空闲检测（仅调试用）。
    pub fn with_stream_idle(mut self, secs: u64) -> Self {
        self.stream_idle = if secs == 0 {
            None
        } else {
            Some(Duration::from_secs(secs))
        };
        self
    }

    /// 注入协作式取消标志：流式读取循环会在每个 chunk 后检查，已取消即停止读取。
    pub fn with_abort(mut self, token: Arc<AtomicBool>) -> Self {
        self.abort = Some(token);
        self
    }

    /// 本次调用是否已被取消（调用方据此把中断与真实失败区分开）。
    pub fn aborted(&self) -> bool {
        self.abort
            .as_ref()
            .is_some_and(|t| t.load(Ordering::SeqCst))
    }

    /// 发送一次请求并在 HTTP 429（限流/额度不足）时自动退避重试。
    ///
    /// 每次尝试都重建完整请求：Authorization Bearer、Content-Type: application/json，
    /// `anthropic=true` 时另加 `anthropic-version: 2023-06-01`，JSON 请求体复用传入的 `body`（仅借用）。
    /// 429 且未达重试上限时 drop 掉响应后按 `RATE_LIMIT_RETRY_BASE_DELAY_MS × attempt` 退避再试；
    /// 重试耗尽后的最后一次响应原样返回（可能是 2xx，也可能仍是非 2xx，由调用方读取 body 生成
    /// 错误文案）；仅网络层错误返回 `Err`（格式 "API 请求失败: {}"）。
    async fn send_with_retry_on_429(
        &self,
        url: &str,
        body: &serde_json::Value,
        anthropic: bool,
    ) -> Result<reqwest::Response, String> {
        // attempt 从 1 起，总尝试 = RATE_LIMIT_RETRIES + 1 = 3（首次 + 2 次重试）。
        for attempt in 1..=RATE_LIMIT_RETRIES + 1 {
            let mut request = self
                .http
                .post(url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json");
            if anthropic {
                request = request.header("anthropic-version", "2023-06-01");
            }

            let response = request
                .json(body)
                .send()
                .await
                .map_err(|e| format!("API 请求失败: {}", e))?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                && attempt <= RATE_LIMIT_RETRIES
            {
                // 429 且尚未耗尽重试次数：丢弃响应体后按 0.8s、1.6s 退避再试。
                drop(response);
                let delay_ms = RATE_LIMIT_RETRY_BASE_DELAY_MS * attempt as u64;
                log::warn!(
                    "[ApiClient] HTTP 429 限流，{} ms 后进行第 {} 次自动重试（剩余 {} 次）",
                    delay_ms,
                    attempt,
                    RATE_LIMIT_RETRIES - attempt + 1
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                continue;
            }
            // 2xx 成功，或非 429 / 429 重试已耗尽：把最终响应交给调用方做状态检查。
            return Ok(response);
        }
        // 逻辑上循环必然在上面 return；这里给一个真实错误而不是 unreachable! ——
        // 将来若改动重试条件导致循环走空，应该是"这次请求失败"，而不是整个应用崩掉。
        Err("API 请求失败: 限流重试循环未返回响应".to_string())
    }

    /// 创建 OpenAI 兼容客户端（默认）
    pub fn new_openai(api_endpoint: String, api_key: String) -> Self {
        Self::new(api_endpoint, api_key, ApiFormat::OpenAI)
    }

    /// 发送 Chat Completion 请求（流式）
    ///
    /// 返回 (完整内容, 工具调用列表, finish_reason)
    /// on_usage 回调返回最终 token 用量（prompt, completion, total, cache_read, cache_write）
    pub async fn chat_stream(
        &self,
        request: &ChatRequest,
        mut on_chunk: impl FnMut(&str),
        mut on_reasoning: impl FnMut(&str),
        mut on_tool_start: impl FnMut(&str, &str, &str),
        mut on_usage: impl FnMut(u32, u32, u32, u32, u32), // (prompt, completion, total, cache_read, cache_write)
    ) -> Result<ChatResponse, String> {
        let url = format!(
            "{}/chat/completions",
            self.api_endpoint.trim_end_matches('/')
        );
        log::info!(
            "[ApiClient] 发送请求到: {}, model={}, msgs_count={}",
            url,
            request.model,
            request.messages.len()
        );

        // 请求体提前构造为 Value：429 自动重试共用同一请求体（helper 仅借用不移动）。
        let request_body = openai_request_body(request);
        let response = self
            .send_with_retry_on_429(&url, &request_body, false)
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            // 错误响应体常含中文（网关/服务商文案），按字节截断会切进字符内部 panic
            log::error!(
                "[ApiClient] HTTP 错误 {}: {}",
                status,
                crate::utils::text::elide_head(&body, 200)
            );
            return Err(http_error_message(status, &body));
        }

        log::info!("[ApiClient] HTTP 200, 开始流式读取响应体");
        let mut stream = response.bytes_stream();
        // 首个数据块 / 首个思考块 / 首个正文块到达时刻：用于区分"上游一直没给数据"（网关挂住）
        // 与"上游在持续输出思考但不出正文"（模型在思考），两者修法完全不同。
        let stream_started = std::time::Instant::now();
        let mut first_byte_logged = false;
        let mut first_reasoning_logged = false;
        let mut first_content_logged = false;

        let mut content = String::new();
        let mut reasoning = String::new();
        let mut finish_reason = String::from("stop");
        let mut pending_tool_calls: std::collections::HashMap<u32, ToolCall> =
            std::collections::HashMap::new();
        // 行缓冲区：跨 chunk 拼接不完整的行（字节缓冲，避免多字节 UTF-8 字符被 chunk 边界截断成乱码）
        let mut line_buf: Vec<u8> = Vec::new();

        // 处理一条 SSE `data:` 行（无行尾换行的残留行也在收尾处复用此逻辑）。
        // done 置 true 表示收到 [DONE]（结束当前行循环）。
        let mut process_data = |data: &str, done: &mut bool| {
            if data == "[DONE]" {
                *done = true;
                return;
            }

            let chunk: StreamChunk = match serde_json::from_str(data) {
                Ok(c) => c,
                Err(_) => return,
            };

            if let Some(choices) = &chunk.choices {
                for choice in choices {
                    if let Some(ref fr) = choice.finish_reason {
                        finish_reason = fr.clone();
                    }

                    // 处理 usage（通常在最后一块返回）
                    if let Some(ref usage) = chunk.usage {
                        if let Some(total) = usage.total_tokens {
                            let prompt_total = usage.prompt_tokens.unwrap_or(0);
                            let completion = usage.completion_tokens.unwrap_or(0);
                            // 缓存命中读取：兼容 prompt_tokens_details.cached_tokens（OpenAI/Qwen）与
                            // 顶层 prompt_cache_hit_tokens（DeepSeek）两种上报位置。
                            let cache_read = usage
                                .prompt_cache_hit_tokens
                                .or_else(|| {
                                    usage
                                        .prompt_tokens_details
                                        .as_ref()
                                        .and_then(|d| d.cached_tokens)
                                })
                                .unwrap_or(0);
                            // OpenAI/DeepSeek 兼容路径不区分缓存写入（无对应上报字段），恒为 0。
                            let cache_write = 0;
                            // 落库口径为「四桶互斥」：prompt_tokens 含命中部分，此处只报未命中输入。
                            let prompt = prompt_total.saturating_sub(cache_read);
                            on_usage(prompt, completion, total, cache_read, cache_write);
                        }
                    }

                    if let Some(delta) = &choice.delta {
                        // 思维链内容（实时推送 + 累积供回传）
                        if let Some(ref rc) = delta.reasoning_content {
                            if !first_reasoning_logged {
                                first_reasoning_logged = true;
                                log::info!(
                                    "[ApiClient] 首个思考块 @ {}ms",
                                    stream_started.elapsed().as_millis()
                                );
                            }
                            reasoning.push_str(rc);
                            on_reasoning(rc);
                        }

                        // 文本内容（实时推送）
                        if let Some(ref c) = delta.content {
                            if !first_content_logged {
                                first_content_logged = true;
                                log::info!(
                                    "[ApiClient] 首个正文块 @ {}ms",
                                    stream_started.elapsed().as_millis()
                                );
                            }
                            content.push_str(c);
                            on_chunk(c);
                        }

                        // 工具调用（增量式）
                        if let Some(tc_deltas) = &delta.tool_calls {
                            for tc in tc_deltas {
                                let idx = tc.index.unwrap_or(0);
                                let entry =
                                    pending_tool_calls.entry(idx).or_insert_with(|| ToolCall {
                                        id: String::new(),
                                        call_type: "function".into(),
                                        function: ToolCallFunction {
                                            name: String::new(),
                                            arguments: String::new(),
                                        },
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
        };

        // 逐 chunk 读取 HTTP 响应（真正的流式，不等待完整响应）。
        // 真超时检测（v3.5d）：等待下一个 chunk 超过配置的空闲阈值仍无数据即判上游假死；
        // 慢但持续输出（reasoning/文本/工具增量都算活性）不会被误杀。
        loop {
            // 协作式取消：已请求停止则立即结束读取（保留已累积内容），不再等上游把整段发完。
            if self.aborted() {
                log::info!("[ApiClient] 已请求取消，提前结束流式读取");
                break;
            }
            let next_chunk = match self.stream_idle {
                Some(idle) => tokio::select! {
                    r = tokio::time::timeout(idle, stream.next()) => match r {
                        Ok(x) => x,
                        Err(_) => {
                            return Err(format!(
                                "模型响应长时间无数据（超过 {} 秒未收到内容），判定为上游连接假死，已终止本次调用。请检查 API 提供商状态或稍后重试。",
                                idle.as_secs()
                            ));
                        }
                    },
                    // 空闲等待期间同样要能立刻响应"停止生成"，否则最长要等满空闲阈值才中断。
                    _ = abort_poll(&self.abort), if self.abort.is_some() => break,
                },
                None => stream.next().await,
            };
            let chunk_bytes = match next_chunk {
                Some(Ok(r)) => r,
                Some(Err(e)) => return Err(format!("流读取错误: {}", e)),
                None => break, // 服务端正常结束（流已读完）
            };
            if !first_byte_logged {
                first_byte_logged = true;
                log::info!(
                    "[ApiClient] 首个数据块 @ {}ms（{} 字节）",
                    stream_started.elapsed().as_millis(),
                    chunk_bytes.len()
                );
            }
            line_buf.extend_from_slice(&chunk_bytes);

            // 逐行处理（只处理以 \n 结尾的完整行）
            while let Some(nl_pos) = line_buf.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&line_buf[..nl_pos])
                    .trim()
                    .to_string();
                line_buf.drain(..=nl_pos);

                if line.is_empty() || !line.starts_with("data: ") {
                    continue;
                }

                let data = &line[6..]; // 去掉 "data: " 前缀
                let mut done = false;
                process_data(data, &mut done);
                if done {
                    break;
                }
            }
        }

        // 收尾：处理未以 \n 结尾的残留数据——部分服务端最后一块不带换行符直接断流，
        // 若不处理，tool_call 参数的末尾增量会丢失（表现为工具参数值被截断，如 `png` → `p`）。
        if !line_buf.is_empty() {
            let line = String::from_utf8_lossy(&line_buf[..]).trim().to_string();
            if line.starts_with("data: ") && line != "data: [DONE]" {
                let data = &line[6..];
                let mut done = false;
                process_data(data, &mut done);
            }
        }

        log::info!(
            "[ApiClient] 流式读取完成 @ {}ms, content_len={}, reasoning_len={}, finish_reason={}",
            stream_started.elapsed().as_millis(),
            content.len(),
            reasoning.len(),
            finish_reason
        );

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
            reasoning_content: reasoning,
            usage: None,
        })
    }

    /// 发送 Anthropic Messages API 请求（流式）
    pub async fn chat_stream_anthropic(
        &self,
        request: &ChatRequest,
        mut on_chunk: impl FnMut(&str),
        _on_reasoning: impl FnMut(&str),
        mut on_tool_start: impl FnMut(&str, &str, &str),
        mut on_usage: impl FnMut(u32, u32, u32, u32, u32), // (prompt, completion, total, cache_read, cache_write)
    ) -> Result<ChatResponse, String> {
        let url = format!("{}/v1/messages", self.api_endpoint.trim_end_matches('/'));
        log::info!("[Anthropic] 发送请求到: {}, model={}", url, request.model);

        // 构建 Anthropic 请求体
        let anthropic_messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(anthropic_message_json)
            .collect();
        let anthropic_req = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "messages": anthropic_messages,
            "stream": true,
            "temperature": request.temperature,
            "tools": request.tools.clone(),
        });

        let response = self
            .send_with_retry_on_429(&url, &anthropic_req, true)
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            log::error!(
                "[Anthropic] HTTP 错误 {}: {}",
                status,
                truncate_utf8(&body, 200)
            );
            return Err(http_error_message(status, &body));
        }

        let mut stream = response.bytes_stream();
        let mut content = String::new();
        let mut finish_reason = String::from("end_turn");
        let mut pending_tool_calls: std::collections::HashMap<u32, ToolCall> =
            std::collections::HashMap::new();
        let mut line_buf: Vec<u8> = Vec::new();
        let mut usage_total: Option<u32> = None;
        // Anthropic 前缀缓存（message_start.usage 上报）：cache_read_input_tokens=缓存命中读取、
        // cache_creation_input_tokens=缓存写入，分属两个字段，缺失为 0。
        let mut usage_cache_read: u32 = 0;
        let mut usage_cache_write: u32 = 0;

        // 处理一条 Anthropic SSE `data:` 行（无行尾换行的残留行也在收尾处复用此逻辑）。
        let mut process_data = |data: &str| {
            let ev: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => return,
            };
            let Some(ev_type) = ev["type"].as_str() else {
                return;
            };

            match ev_type {
                "content_block_delta" => {
                    let Some(delta_type) = ev["delta"]["type"].as_str() else {
                        return;
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
                            let tc_id =
                                ev["content_block"]["id"].as_str().unwrap_or("").to_string();
                            let tc_name = ev["content_block"]["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            pending_tool_calls.insert(
                                idx,
                                ToolCall {
                                    id: tc_id,
                                    call_type: "function".into(),
                                    function: ToolCallFunction {
                                        name: tc_name,
                                        arguments: String::new(),
                                    },
                                },
                            );
                        }
                    }
                }
                "message_start" => {
                    // Anthropic 前缀缓存在 message_start.usage 上报：cache_read_input_tokens
                    // （缓存命中读取）与 cache_creation_input_tokens（缓存写入）分别记录。
                    if let Some(u) = ev.get("usage") {
                        usage_cache_read =
                            u["cache_read_input_tokens"].as_u64().unwrap_or(0) as u32;
                        usage_cache_write =
                            u["cache_creation_input_tokens"].as_u64().unwrap_or(0) as u32;
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
        };

        // 逐 chunk 读取（真超时检测同上：chunk 间隔超配置的空闲阈值判上游假死）。
        loop {
            // 协作式取消：已请求停止则立即结束读取（保留已累积内容），不再等上游把整段发完。
            if self.aborted() {
                log::info!("[ApiClient] 已请求取消，提前结束流式读取");
                break;
            }
            let next_chunk = match self.stream_idle {
                Some(idle) => tokio::select! {
                    r = tokio::time::timeout(idle, stream.next()) => match r {
                        Ok(x) => x,
                        Err(_) => {
                            return Err(format!(
                                "模型响应长时间无数据（超过 {} 秒未收到内容），判定为上游连接假死，已终止本次调用。请检查 API 提供商状态或稍后重试。",
                                idle.as_secs()
                            ));
                        }
                    },
                    // 空闲等待期间同样要能立刻响应"停止生成"，否则最长要等满空闲阈值才中断。
                    _ = abort_poll(&self.abort), if self.abort.is_some() => break,
                },
                None => stream.next().await,
            };
            let chunk_bytes = match next_chunk {
                Some(Ok(r)) => r,
                Some(Err(e)) => return Err(format!("流读取错误: {}", e)),
                None => break, // 服务端正常结束（流已读完）
            };
            line_buf.extend_from_slice(&chunk_bytes);

            // 只关心 `data:` 行，`event:`/空行一律忽略。Anthropic 的 data 行 JSON 自带
            // `type` 字段，无需与 `event:` 行配对；否则当 `event:` 与 `data:` 被拆到
            // 不同 HTTP chunk 时，`event:` 行会被误判为已结束而丢弃后续 `data:`，造成内容截断。
            while let Some(nl_pos) = line_buf.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&line_buf[..nl_pos])
                    .trim()
                    .to_string();
                line_buf.drain(..=nl_pos);

                if line.is_empty() || !line.starts_with("data: ") {
                    continue;
                }

                let data = &line[6..]; // 去掉 "data: " 前缀
                process_data(data);
            }
        }

        // 收尾：处理未以 \n 结尾的残留数据——部分服务端最后一块不带换行符直接断流，
        // 若不处理，tool_call 参数的末尾增量会丢失（表现为工具参数值被截断，如 `png` → `p`）。
        if !line_buf.is_empty() {
            let line = String::from_utf8_lossy(&line_buf[..]).trim().to_string();
            if line.starts_with("data: ") {
                let data = &line[6..];
                process_data(data);
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
        // 合计取四桶之和（prompt 为未命中输入，另有缓存读取/写入两桶），与 OpenAI 路径同口径。
        let total_tokens = prompt_tokens + completion_tokens + usage_cache_read + usage_cache_write;
        on_usage(
            prompt_tokens,
            completion_tokens,
            total_tokens,
            usage_cache_read,
            usage_cache_write,
        );

        log::info!(
            "[Anthropic] 流式读取完成, content_len={}, tool_calls={}",
            content.len(),
            tool_calls.len()
        );

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning_content: String::new(), // Anthropic 无 reasoning_content 概念
            usage: None,
        })
    }
}

impl ApiClient {
    /// 发送 Chat Completion 请求（非流式）
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
        let url = format!(
            "{}/chat/completions",
            self.api_endpoint.trim_end_matches('/')
        );

        let mut req = request.clone();
        req.stream = false;

        // 请求体提前构造为 Value：429 自动重试共用同一请求体（helper 仅借用不移动）。
        let request_body = openai_request_body(&req);
        let response = self
            .send_with_retry_on_429(&url, &request_body, false)
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(http_error_message(status, &body));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("解析响应失败: {}", e))?;

        let choice = body["choices"][0].clone();
        let message = &choice["message"];

        let content = message["content"].as_str().unwrap_or("").to_string();
        let finish_reason = choice["finish_reason"]
            .as_str()
            .unwrap_or("stop")
            .to_string();

        let tool_calls: Vec<ToolCall> = message["tool_calls"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| serde_json::from_value(tc.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        let usage = body.get("usage").and_then(usage_from_openai_value);

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning_content: String::new(), // 非流式路径未捕获 reasoning，保持为空
            usage,
        })
    }

    /// 发送 Anthropic Messages API 请求（非流式）
    pub async fn chat_anthropic(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
        let url = format!("{}/v1/messages", self.api_endpoint.trim_end_matches('/'));

        let anthropic_messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(anthropic_message_json)
            .collect();
        let body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens.unwrap_or(1024),
            "messages": anthropic_messages,
            "stream": false,
            "temperature": request.temperature.map(round_temperature),
        });

        let response = self.send_with_retry_on_429(&url, &body, true).await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(http_error_message(status, &body));
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

        let finish_reason = body["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();
        let usage = body.get("usage").and_then(usage_from_anthropic_value);

        Ok(ChatResponse {
            content,
            tool_calls: Vec::new(),
            finish_reason,
            reasoning_content: String::new(), // Anthropic 无 reasoning_content 概念
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_overflow_detection() {
        // 两种协议的常见溢出文案均应命中。
        for msg in [
            "This model's maximum context length is 128000 tokens...",
            "Prompt is too long: 160001 tokens > 128000 maximum",
            "Invalid value: 'messages': context_window_exceeded",
            "request too large: reduce the length of your messages",
            "Input is too long for this model",
            "max model tokens exceeded: too many tokens",
        ] {
            assert!(is_context_overflow(msg), "应命中溢出: {}", msg);
        }
        // 普通错误不应命中。
        for msg in [
            "API key invalid",
            "rate limit exceeded, retry later",
            "model not found: gpt-999",
            "stream idle timeout",
            "网络连接失败",
        ] {
            assert!(!is_context_overflow(msg), "不应命中溢出: {}", msg);
        }
    }

    #[test]
    fn openai_usage_splits_cached_out_of_prompt() {
        // OpenAI 协议 prompt_tokens 含缓存命中部分 → 落库口径只记未命中输入。
        let v = serde_json::json!({
            "prompt_tokens": 18190,
            "completion_tokens": 934,
            "total_tokens": 19124,
            "prompt_tokens_details": { "cached_tokens": 18176 }
        });
        let u = usage_from_openai_value(&v).unwrap();
        assert_eq!(
            (u.prompt, u.completion, u.cache_read, u.cache_write),
            (14, 934, 18176, 0)
        );
        assert_eq!(u.total, 19124, "合计沿用服务端 total（= 未命中+命中+输出）");

        // DeepSeek 的顶层上报位置同样生效；命中数大于 prompt 时不得出现负数。
        let v2 = serde_json::json!({
            "prompt_tokens": 10,
            "completion_tokens": 1,
            "total_tokens": 11,
            "prompt_cache_hit_tokens": 256
        });
        let u2 = usage_from_openai_value(&v2).unwrap();
        assert_eq!((u2.prompt, u2.cache_read), (0, 256));

        // 无缓存字段：输入即未命中，行为与改动前一致。
        let v3 =
            serde_json::json!({"prompt_tokens": 100, "completion_tokens": 5, "total_tokens": 105});
        let u3 = usage_from_openai_value(&v3).unwrap();
        assert_eq!((u3.prompt, u3.cache_read, u3.cache_write), (100, 0, 0));
    }

    #[test]
    fn anthropic_usage_total_is_four_bucket_sum() {
        let v = serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 20,
            "cache_read_input_tokens": 300,
            "cache_creation_input_tokens": 40
        });
        let u = usage_from_anthropic_value(&v).unwrap();
        assert_eq!(
            (u.prompt, u.completion, u.cache_read, u.cache_write),
            (100, 20, 300, 40)
        );
        assert_eq!(u.total, 460, "合计 = 未命中输入 + 输出 + 缓存读 + 缓存写");
    }

    #[test]
    fn http_error_message_rate_limit_hint_vs_plain() {
        use reqwest::StatusCode;
        // 429（重试耗尽后）：文案含解决建议的中文提示，同时保留原始 body 供排查。
        let hint = http_error_message(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit for free users... Upgrade to a Token Plan",
        );
        assert!(
            hint.starts_with("API 返回错误 (HTTP 429)"),
            "应带 429 状态码: {}",
            hint
        );
        assert!(hint.contains("限流"), "应说明限流: {}", hint);
        assert!(hint.contains("Token Plan"), "应含升级套餐建议: {}", hint);
        assert!(
            hint.contains("rate limit for free users"),
            "应保留原始 body: {}",
            hint
        );
        // 其余状态码：保持历史文案格式（StatusCode Display 含原因短语），不含 429 提示。
        let plain = http_error_message(StatusCode::BAD_REQUEST, "bad body");
        assert_eq!(
            plain,
            format!(
                "API 返回错误 (HTTP {}): {}",
                StatusCode::BAD_REQUEST,
                "bad body"
            ),
            "非 429 文案应逐字保持原格式"
        );
        assert!(
            !plain.contains("限流"),
            "非 429 不应混入限流提示: {}",
            plain
        );
    }
}
