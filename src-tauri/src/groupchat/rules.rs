//! 确定性兜底规则（RuleEngine）：Director 超时/失效/输出非法时接管。

/// 收敛判定所需的确定性规则。
/// 语义收敛已交由 Director::director_review 判定，此处只保留 done 防抖计数。
pub struct RuleEngine {
    /// 连续判定「已收敛（done）」的轮数；连续 2 轮才真正收敛，避免单次误判。
    pub converged_rounds: usize,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self { converged_rounds: 0 }
    }

    /// 记录一轮 Director 收敛裁决，返回是否应收敛（连续 2 轮 done）。
    pub fn should_converge(&mut self, converged: bool) -> bool {
        if converged {
            self.converged_rounds += 1;
        } else {
            self.converged_rounds = 0;
        }
        self.converged_rounds >= 2
    }

    /// 重置收敛计数（用户插队/确认回复后调用，避免过早收敛）。
    pub fn reset(&mut self) {
        self.converged_rounds = 0;
    }

    /// 兜底下一发言者：纯轮询（Director 失效时无角色匹配度可用）。
    #[allow(dead_code)]
    pub fn fallback_next(order: &[String], cursor: &mut usize) -> Option<String> {
        if order.is_empty() {
            return None;
        }
        let idx = *cursor % order.len();
        *cursor += 1;
        Some(order[idx].clone())
    }
}
