//! 上下文管理与 Token 控制
//!
//! 提供滑动窗口消息截断和 Token 估算。
//! Token 计算使用字符级启发式估算（约 3.5 字符/token），
//! 不依赖外部 tokenizer，保持轻量。

use crate::api_agent::types::ChatMessage;

/// Token 估算器
///
/// 使用简单字符级启发式：英文约 4 字符/token，中文约 1.5 字符/token。
/// 混合文本取加权平均 ≈ 3.5 字符/token，偏保守估计。
pub struct TokenEstimator;

impl TokenEstimator {
    /// 估算字符串的 token 数
    pub fn estimate(text: &str) -> usize {
        let chars = text.chars().count();
        if chars == 0 {
            return 0;
        }

        // 统计中文字符数（Unicode CJK 范围）
        let cjk_count = text.chars().filter(|c| is_cjk(*c)).count();
        let latin_count = chars - cjk_count;

        // 英文约 4 字符/token，中文约 1.5 字符/token
        let tokens = (latin_count as f64 / 4.0 + cjk_count as f64 / 1.5).ceil();
        tokens as usize
    }

    /// 估算 ChatMessage 的 token 数
    pub fn estimate_message(msg: &ChatMessage) -> usize {
        let mut tokens = 4; // 消息元数据开销

        if let Some(ref content) = msg.content {
            tokens += Self::estimate(content);
        }

        // 工具调用开销
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                tokens += Self::estimate(&tc.function.name);
                tokens += Self::estimate(&tc.function.arguments);
                tokens += 10; // JSON 结构开销
            }
        }

        tokens
    }
}

/// 滑动窗口管理器
///
/// 当消息历史超过指定 token 限制时，从最早的消息开始移除，
/// 保留最近的 N 条消息。同时尽量保持 user/assistant 成对。
pub struct SlidingWindow {
    /// 最大 token 限制（仅计算消息，不含 system prompt）
    max_tokens: usize,
}

impl SlidingWindow {
    pub fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }

    /// 应用滑动窗口截断（仅作为兜底）
    ///
    /// 职责已收敛为：兜底处理单条超长的工具结果（如 read_file 32KB、大目录、长日志）。
    /// 历史语义压缩交由滚动摘要层负责，本方法不再承担常规历史裁剪。
    ///
    /// 规则：
    /// 1. 跳过 role == "system" 的消息（主 system prompt 不在 messages 中，
    ///    这里主要指注入的 `<conversation_summary>` 摘要消息），不对其计数或裁剪；
    /// 2. 从尾部（最新消息）向前累加，保留最近的 user/assistant/tool 消息；
    /// 3. 若某条消息自身超过限制，截断其 content 而不是丢弃整条。
    pub fn trim(&self, messages: &[ChatMessage]) -> Vec<ChatMessage> {
        if messages.is_empty() {
            return Vec::new();
        }

        // 受保护消息（system，含摘要）与可裁剪消息分离
        let protected: Vec<ChatMessage> = messages
            .iter()
            .filter(|m| m.role == "system")
            .cloned()
            .collect();
        let trimmable: Vec<ChatMessage> = messages
            .iter()
            .filter(|m| m.role != "system")
            .cloned()
            .collect();

        if trimmable.is_empty() {
            return messages.to_vec();
        }

        let total_tokens: usize = trimmable
            .iter()
            .map(|m| TokenEstimator::estimate_message(m))
            .sum();

        if total_tokens <= self.max_tokens {
            return messages.to_vec();
        }

        // 从尾部（最新消息）向前累加，找到需要保留的消息
        let mut kept: Vec<ChatMessage> = Vec::new();
        let mut kept_tokens = 0usize;

        for msg in trimmable.into_iter().rev() {
            let msg_tokens = TokenEstimator::estimate_message(&msg);

            // 单条消息超长：截断内容后作为唯一保留项，避免整条丢弃
            if msg_tokens > self.max_tokens {
                kept.push(truncate_message_content(&msg, self.max_tokens));
                break;
            }

            if kept_tokens + msg_tokens > self.max_tokens {
                break;
            }
            kept_tokens += msg_tokens;
            kept.push(msg);
        }
        kept.reverse();

        // 受保护消息（摘要）保持在最前，后接保留的最近消息
        let mut result = protected;
        result.extend(kept);

        log::debug!(
            "[Context] 滑动窗口兜底截断: {} → {} 条消息, token: {} → {} (limit: {})",
            messages.len(),
            result.len(),
            total_tokens,
            kept_tokens,
            self.max_tokens,
        );

        result
    }
}

/// 将单条消息的 content 截断到指定 token 预算（UTF-8 安全，保留字符边界）
fn truncate_message_content(msg: &ChatMessage, max_tokens: usize) -> ChatMessage {
    let mut result = msg.clone();
    if let Some(ref content) = msg.content {
        result.content = Some(truncate_str_to_tokens(content, max_tokens));
    }
    result
}

/// 将字符串按 token 预算截断，逐字符累计估算，避免多字节字符边界问题
fn truncate_str_to_tokens(text: &str, max_tokens: usize) -> String {
    if TokenEstimator::estimate(text) <= max_tokens {
        return text.to_string();
    }

    let mut kept = String::new();
    let mut tokens = 0.0f64;
    for c in text.chars() {
        let c_tokens = if is_cjk(c) { 1.0 / 1.5 } else { 1.0 / 4.0 };
        if tokens + c_tokens > max_tokens as f64 {
            break;
        }
        tokens += c_tokens;
        kept.push(c);
    }

    if kept.chars().count() < text.chars().count() {
        kept.push_str("\n...[内容过长已截断]...");
    }
    kept
}

/// 判断字符是否为 CJK（中文/日文/韩文）
fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
    )
}

/// 默认上下文窗口大小（token）
/// 常见模型: 8K 上下文，预留 2K 给 system prompt 和回复
pub const DEFAULT_CONTEXT_TOKENS: usize = 32000;

/// 根据模型名称推断上下文窗口大小（token）
/// 返回 None 表示使用 DEFAULT_CONTEXT_TOKENS
pub fn infer_context_window(model: &str) -> Option<usize> {
    let model_lower = model.to_lowercase();
    // Anthropic Claude
    if model_lower.contains("claude-3-7") || model_lower.contains("claude-3.7") {
        return Some(200_000);
    }
    if model_lower.contains("claude-3-opus") || model_lower.contains("claude-3-sonnet")
        || model_lower.contains("claude-3-haiku") {
        return Some(200_000);
    }
    if model_lower.contains("claude-2") {
        return Some(100_000);
    }
    // OpenAI GPT
    if model_lower.contains("gpt-4o") || model_lower.contains("gpt-4-turbo") {
        return Some(128_000);
    }
    if model_lower.contains("gpt-4") {
        return Some(8192);
    }
    if model_lower.contains("gpt-3.5") {
        return Some(16_385);
    }
    // DeepSeek
    if model_lower.contains("deepseek-r1") {
        return Some(64_000);
    }
    if model_lower.contains("deepseek-v3") || model_lower.contains("deepseek-chat") {
        return Some(64_000);
    }
    // Gemini
    if model_lower.contains("gemini-2.5") || model_lower.contains("gemini-2.0") {
        return Some(1_000_000);
    }
    if model_lower.contains("gemini") {
        return Some(128_000);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_english() {
        let text = "Hello, how are you today?";
        let tokens = TokenEstimator::estimate(text);
        // ~27 chars / 4 = ~7 tokens
        assert!(tokens > 3 && tokens < 15);
    }

    #[test]
    fn test_estimate_chinese() {
        let text = "你好，今天怎么样？";
        let tokens = TokenEstimator::estimate(text);
        // 9 CJK chars / 1.5 = 6 tokens
        assert!(tokens > 3 && tokens < 12);
    }

    #[test]
    fn test_estimate_mixed() {
        let text = "Hello 世界! How are 你?";
        let tokens = TokenEstimator::estimate(text);
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_empty() {
        assert_eq!(TokenEstimator::estimate(""), 0);
    }

    #[test]
    fn test_sliding_window_no_truncation() {
        let messages = vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
        ];
        let window = SlidingWindow::new(10000);
        let trimmed = window.trim(&messages);
        assert_eq!(trimmed.len(), 2);
    }

    #[test]
    fn test_sliding_window_truncation() {
        // 创建很多消息以触发截断
        let mut messages = Vec::new();
        for i in 0..100 {
            messages.push(ChatMessage::user(&format!("message {}", i)));
            messages.push(ChatMessage::assistant(&format!("reply {}", i)));
        }

        let window = SlidingWindow::new(500);
        let trimmed = window.trim(&messages);
        assert!(trimmed.len() < messages.len());
        assert!(trimmed.len() > 0);
    }
}
