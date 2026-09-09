//! 发言权管理（FloorManager）：令牌串行 + 多策略（MVP 实现 round_robin）。

/// 宽容归一化：LLM 输出可能回显带 `[@...]` 括号或 `@` 前缀的参与者引用，
/// 匹配可发言列表前统一剥离为裸 id，避免 Director 选人被静默丢弃。
fn normalize_ref(raw: &str) -> &str {
    let s = raw.trim();
    if let Some(rest) = s.strip_prefix("[@") {
        rest.strip_suffix(']').unwrap_or(rest).trim()
    } else {
        s.strip_prefix('@').unwrap_or(s).trim()
    }
}

/// 发言权令牌。讨论阶段同一时刻仅一个参与者持有。
pub struct FloorManager {
    /// 可发言参与者顺序（api/cli 参与者；user 与 director 不占普通发言权）。
    order: Vec<String>,
    cursor: usize,
    current: Option<String>,
}

impl FloorManager {
    pub fn new(order: Vec<String>) -> Self {
        Self { order, cursor: 0, current: None }
    }

    /// 选择下一发言者。
    /// `preferred` 为 Director/规则层给出的候选；若其在可发言列表中则优先，否则轮询。
    pub fn next(&mut self, preferred: Option<&str>) -> Option<String> {
        if self.order.is_empty() {
            return None;
        }
        let speaker = match preferred {
            Some(p) if self.order.iter().any(|o| o == normalize_ref(p)) => normalize_ref(p).to_string(),
            _ => {
                let idx = self.cursor % self.order.len();
                self.cursor += 1;
                self.order[idx].clone()
            }
        };
        self.current = Some(speaker.clone());
        Some(speaker)
    }

    #[allow(dead_code)]
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    #[allow(dead_code)]
    pub fn release(&mut self) {
        self.current = None;
    }
}
