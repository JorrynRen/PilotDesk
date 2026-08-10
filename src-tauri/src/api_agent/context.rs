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

    /// 应用滑动窗口截断
    ///
    /// 返回截断后的消息列表，确保总 token 数不超过限制。
    /// 优先保留最近的消息，尽量保持 user/assistant 配对。
    pub fn trim(&self, messages: &[ChatMessage]) -> Vec<ChatMessage> {
        if messages.is_empty() {
            return Vec::new();
        }

        let total_tokens: usize = messages.iter().map(|m| TokenEstimator::estimate_message(m)).sum();

        if total_tokens <= self.max_tokens {
            return messages.to_vec();
        }

        // 从尾部（最新消息）向前累加，找到需要保留的起点
        let mut kept_tokens = 0;
        let mut start_idx = messages.len();

        for (i, msg) in messages.iter().enumerate().rev() {
            let msg_tokens = TokenEstimator::estimate_message(msg);
            if kept_tokens + msg_tokens > self.max_tokens {
                start_idx = i + 1;
                break;
            }
            kept_tokens += msg_tokens;
        }

        // 确保不从 assistant 消息开始（保持 user/assistant 配对）
        if start_idx < messages.len() && messages[start_idx].role == "assistant" {
            start_idx += 1;
        }

        let result: Vec<ChatMessage> = messages[start_idx..].to_vec();

        log::debug!(
            "[Context] 滑动窗口截断: {} → {} 条消息, token: {} → {} (limit: {})",
            messages.len(),
            result.len(),
            total_tokens,
            kept_tokens,
            self.max_tokens,
        );

        result
    }
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
pub const DEFAULT_CONTEXT_TOKENS: usize = 6000;

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
