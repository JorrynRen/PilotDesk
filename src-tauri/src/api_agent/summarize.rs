//! 增量式滚动摘要（Rolling Summary）
//!
//! 当会话历史超出保留窗口（最近 N 轮 / token 双阈值）时，将超出部分
//! 交由当前会话模型进行语义压缩，而非暴力截断。
//!
//! 摘要聚焦三类"记忆锚点"，避免早期关键信息因注意力分散（lost in the middle）丢失：
//! 1. 用户原始目标与需求
//! 2. 关键决策、方向切换或重要结论
//! 3. 尚未完成的待办事项

use crate::api_agent::client::ApiClient;
use crate::api_agent::context::TokenEstimator;
use crate::api_agent::types::*;

/// 保留窗口：最近 N 轮完整对话
pub const RECENT_TURN_LIMIT: usize = 10;
/// 保留窗口：最近对话的 token 上限
pub const RECENT_TOKEN_LIMIT: usize = 8000;
/// 滚动摘要的字符上限
pub const SUMMARY_MAX_CHARS: usize = 1500;

/// 双阈值切分：将完整对话拆分为 (older_messages, recent_messages)。
///
/// - `recent_messages`：从最新一轮向前累加，直到达到「10 轮」或「8000 token」
///   两者中更保守的边界，完整保留。
/// - `older_messages`：保留窗口之外的更早消息，交由摘要层压缩。
///
/// 保证至少保留最近一轮，避免单轮过长时整段历史被清空。
pub fn split_recent_window(messages: &[ChatMessage]) -> (Vec<ChatMessage>, Vec<ChatMessage>) {
    if messages.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // 每个 turn 的起始下标（role == "user"）
    let turn_starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "user")
        .map(|(i, _)| i)
        .collect();

    if turn_starts.is_empty() {
        // 异常：没有任何 user 消息，全部归入 recent
        return (Vec::new(), messages.to_vec());
    }

    let mut recent_start = messages.len();
    let mut turns = 0usize;
    let mut tokens = 0usize;

    // 从最后一个 turn 向前遍历
    for &start in turn_starts.iter().rev() {
        let turn_tokens: usize = messages[start..recent_start]
            .iter()
            .map(|m| TokenEstimator::estimate_message(m))
            .sum();

        if turns >= RECENT_TURN_LIMIT {
            break;
        }
        // 至少保留最近一轮；token 限制仅在已有保留内容后生效
        if turns > 0 && tokens + turn_tokens > RECENT_TOKEN_LIMIT {
            break;
        }

        tokens += turn_tokens;
        turns += 1;
        recent_start = start;
    }

    let recent = messages[recent_start..].to_vec();
    let older = messages[..recent_start].to_vec();
    (older, recent)
}

/// 生成滚动摘要：将旧摘要与超出保留窗口的早期对话合并，压缩为一段新摘要。
///
/// `on_usage`：摘要请求产生的用量回调（prompt/completion/total/cache_read/cache_write），
/// 由调用方决定落库口径（计入会话用量统计）。
pub async fn generate_rolling_summary(
    client: &ApiClient,
    model: &str,
    format: &ApiFormat,
    old_summary: &str,
    older_messages: &[ChatMessage],
    mut on_usage: impl FnMut(u32, u32, u32, u32, u32),
) -> Result<String, String> {
    let prompt = build_summary_prompt(old_summary, older_messages);

    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![ChatMessage::user(&prompt)],
        tools: None,
        tool_choice: None,
        stream: false,
        temperature: Some(0.3),
        max_tokens: Some(1024),
    };

    let response = match format {
        ApiFormat::Anthropic => client.chat_anthropic(&request).await?,
        _ => client.chat(&request).await?,
    };

    if let Some(u) = response.usage {
        on_usage(u.prompt, u.completion, u.total, u.cache_read, u.cache_write);
    }

    // ── 摘要 fail-closed 守卫：宁可不更新、保留旧摘要，也不接受半截/失真产物 ──
    let summary = response.content.trim().to_string();
    if summary.is_empty() {
        return Err("摘要生成为空文本，已拒绝（保留旧摘要）。".to_string());
    }
    if matches!(response.finish_reason.as_str(), "max_tokens" | "length") {
        return Err(format!(
            "摘要生成被 max_tokens 截断（finish_reason={}），已拒绝半截摘要（保留旧摘要）。",
            response.finish_reason
        ));
    }
    if summary.chars().count() > SUMMARY_MAX_CHARS {
        return Err(format!(
            "摘要超过长度上限（{} 字，实际 {} 字），已拒绝而非静默截断（保留旧摘要）。",
            SUMMARY_MAX_CHARS,
            summary.chars().count()
        ));
    }
    // 摘要必须比被压缩的早期对话更小，否则说明压缩失败、写入只会挤占上下文。
    let older_tokens: usize = older_messages.iter().map(|m| TokenEstimator::estimate_message(m)).sum();
    let summary_tokens = TokenEstimator::estimate(&summary);
    if older_tokens > 0 && summary_tokens >= older_tokens {
        return Err(format!(
            "摘要未比原文更小（摘要 ~{} tokens >= 早期对话 ~{} tokens），已拒绝（保留旧摘要）。",
            summary_tokens, older_tokens
        ));
    }

    Ok(summary)
}

/// 组装摘要生成 prompt
fn build_summary_prompt(old_summary: &str, older_messages: &[ChatMessage]) -> String {
    let mut transcript = String::new();
    for msg in older_messages {
        match msg.role.as_str() {
            "user" => {
                if let Some(c) = &msg.content {
                    transcript.push_str(&format!("用户: {}\n", truncate_to_chars(c, 4000)));
                }
            }
            "assistant" => {
                if let Some(c) = &msg.content {
                    transcript.push_str(&format!("助手: {}\n", truncate_to_chars(c, 4000)));
                }
                if let Some(tcs) = &msg.tool_calls {
                    for tc in tcs {
                        transcript.push_str(&format!(
                            "助手调用工具 {}: {}\n",
                            tc.function.name,
                            truncate_to_chars(&tc.function.arguments, 1000)
                        ));
                    }
                }
            }
            "tool" => {
                if let Some(c) = &msg.content {
                    transcript.push_str(&format!("工具结果: {}\n", truncate_to_chars(c, 2000)));
                }
            }
            _ => {}
        }
    }

    let existing = if old_summary.is_empty() {
        "（无）".to_string()
    } else {
        old_summary.to_string()
    };

    format!(
        "你是对话摘要器。请将下方早期对话压缩为一段简洁的中文摘要，用于替代被裁剪的历史消息。\n\
        摘要需重点保留三类信息：\n\
        1. 用户的原始目标与需求；\n\
        2. 关键决策、方向切换或重要结论；\n\
        3. 尚未完成的待办事项。\n\
        只输出纯文本（不要 Markdown 代码块、不要复述工具调用细节）；新摘要应合并吸收下方[已有摘要]再补入本轮要点，不要照搬旧文。\n\
        不要逐句复述，只保留对未来对话仍有长期价值的信息。摘要不得超过 {} 字。\n\n\
        [已有摘要]\n{}\n\n[本轮需要合并的早期对话]\n{}",
        SUMMARY_MAX_CHARS,
        existing,
        transcript,
    )
}

/// 按字符数截断（UTF-8 安全）
fn truncate_to_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("...");
    out
}
