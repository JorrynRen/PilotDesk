//! 字符串截断/预览的**安全**工具（按字符计数，永不切进多字节字符内部）。
//!
//! 为什么要单独收一处：日志、错误消息、UI 预览里到处要"取前 N 个字符"，随手写成
//! `&s[..n]`（或取尾部 `&s[s.len() - n..]`）在纯英文下看不出问题，一旦内容是中文/emoji
//! 就会 panic：`end byte index 50 is not a char boundary; it is inside '图'`。
//! 这类崩溃发生在**离调用点很远**的地方（如某个节点的输入解析日志），排查成本极高。
//!
//! 约定：要截断或取预览，一律走本模块（或等价的 `s.chars().take(n)`）。
//! 只有两种情况可以直接按字节切片，且必须写清理由：
//! 1. 切片下标来自 `find` / `char_indices`（天然落在字符边界上）；
//! 2. 被切的内容**保证是 ASCII**（uuid、base64、`data: ` 之类协议前缀、`%USERPROFILE%` 等）。

/// 取开头最多 `max_chars` 个字符（按字符计数，不会切进字符内部）。
pub fn head_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// 取结尾最多 `max_chars` 个字符（按字符计数，不会切进字符内部）。
pub fn tail_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    s.chars().skip(total - max_chars).collect()
}

/// 开头预览：超长时截断并追加省略号（未超长原样返回）。
pub fn elide_head(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    format!("{}...", head_chars(s, max_chars))
}

/// 尾部预览（常用于"取最后几行/末尾片段"的日志与报错）。
pub fn elide_tail(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    format!("...{}", tail_chars(s, max_chars))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_tail_count_by_chars_not_bytes() {
        // "图片" == 6 字节；旧写法 `&s[..5]` 正是 panic 现场（第 5 字节落在 '图' 内部）
        let s = "生成图片的任务描述";
        assert_eq!(head_chars(s, 2), "生成");
        assert_eq!(head_chars(s, 100), s);
        assert_eq!(tail_chars(s, 2), "描述");
        assert_eq!(tail_chars(s, 100), s);
        assert_eq!(head_chars(s, 0), "");
        assert_eq!(tail_chars(s, 0), "");
    }

    #[test]
    fn elide_helpers_never_panic_on_boundaries() {
        // 逐字节长度全试一遍：任何取整都可能切到字符内部，按字符实现则都不会 panic
        let s = "a中文🙂b";
        for n in 0..s.len() * 2 {
            let _ = head_chars(s, n);
            let _ = tail_chars(s, n);
            let _ = elide_head(s, n);
            let _ = elide_tail(s, n);
        }
        assert_eq!(elide_head("短", 5), "短");
        assert_eq!(elide_head("一二三四五六", 3), "一二三...");
        assert_eq!(elide_tail("一二三四五六", 2), "...五六");
    }

    #[test]
    fn empty_string_is_safe() {
        assert_eq!(head_chars("", 5), "");
        assert_eq!(tail_chars("", 5), "");
        assert_eq!(elide_head("", 5), "");
        assert_eq!(elide_tail("", 5), "");
    }
}
