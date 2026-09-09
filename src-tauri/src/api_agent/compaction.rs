//! 上下文压缩策略化（P0-2）——把"何时压缩、保留多少"从硬编码抬升为可插拔策略。
//!
//! 借鉴 deepseek-harness `dsh-compaction` 的"定义/提供分离"：本文件只定义策略缝
//! （`CompactionPolicy`），当前滚动摘要行为作为默认实现 `DefaultCompactionPolicy`
//! 提供，保留窗口参数直接引用 [`summarize`] 的既有常量（不复制第二份，防漂移）。
//!
//! 接线说明：策略经 [`conversation_stats`] + [`DefaultCompactionPolicy`] 在 lib.rs 的
//! 滚动摘要触发点生效；后续可替换为可配置策略而不改触发逻辑。
use crate::api_agent::context::TokenEstimator;
use crate::api_agent::summarize::{RECENT_TOKEN_LIMIT, RECENT_TURN_LIMIT};
use crate::api_agent::types::ChatMessage;

/// 会话上下文统计快照（供策略判定是否该压缩）。
#[derive(Debug, Clone, Copy)]
pub struct ConversationStats {
    /// 完整轮数（以 user 消息为界）
    pub turn_count: usize,
    /// 消息总 token 估算
    pub token_estimate: usize,
}

/// 从消息列表计算上下文统计（user 消息计轮数，全部消息计 token 估算）。
pub fn conversation_stats(messages: &[ChatMessage]) -> ConversationStats {
    let mut turn_count = 0usize;
    let mut token_estimate = 0usize;
    for m in messages {
        if m.role == "user" {
            turn_count += 1;
        }
        token_estimate += TokenEstimator::estimate_message(m);
    }
    ConversationStats { turn_count, token_estimate }
}

/// 压缩策略：决定"何时把早期历史折叠进摘要"。
pub trait CompactionPolicy: Send + Sync {
    /// 是否需要对当前上下文发起压缩。
    fn should_compact(&self, stats: &ConversationStats) -> bool;
    /// 保留窗口 `(轮数上限, token 上限)`——完整保留的最新对话部分。
    fn retention(&self) -> (usize, usize);
}

/// 默认策略 = 当前滚动摘要的双阈值（10 轮 / 8000 token，常量来源见 [`summarize`]）。
#[derive(Debug, Clone, Copy)]
pub struct DefaultCompactionPolicy {
    pub recent_turn_limit: usize,
    pub recent_token_limit: usize,
}

impl DefaultCompactionPolicy {
    /// 绑定当前滚动摘要常量，保持行为不变。
    pub fn current() -> Self {
        Self {
            recent_turn_limit: RECENT_TURN_LIMIT,
            recent_token_limit: RECENT_TOKEN_LIMIT,
        }
    }
}

impl Default for DefaultCompactionPolicy {
    fn default() -> Self {
        Self::current()
    }
}

impl CompactionPolicy for DefaultCompactionPolicy {
    fn retention(&self) -> (usize, usize) {
        (self.recent_turn_limit, self.recent_token_limit)
    }

    fn should_compact(&self, stats: &ConversationStats) -> bool {
        let (turns, tokens) = self.retention();
        stats.turn_count > turns || stats.token_estimate > tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_matches_double_threshold() {
        let policy = DefaultCompactionPolicy::current();
        assert_eq!(policy.retention(), (RECENT_TURN_LIMIT, RECENT_TOKEN_LIMIT));

        // 低于双阈值：不压缩。
        assert!(!policy.should_compact(&ConversationStats { turn_count: RECENT_TURN_LIMIT, token_estimate: RECENT_TOKEN_LIMIT }));
        // 任一侧越界：压缩。
        assert!(policy.should_compact(&ConversationStats { turn_count: RECENT_TURN_LIMIT + 1, token_estimate: 0 }));
        assert!(policy.should_compact(&ConversationStats { turn_count: 0, token_estimate: RECENT_TOKEN_LIMIT + 1 }));
    }
}
