// ──────────────────────────────────────────────
//  LineProcessor — 统一输出行处理流水线
// ──────────────────────────────────────────────
//  三层架构：
//    L0 平台层：剥离 ConPTY 路径噪声（:Users...）
//    L1 Agent 配置层：默认剥离 ANSI 颜色码 + 按 output_filter_regex 过滤
//    输出：过滤后的纯文本行，供上层解析器消费
//
//  设计原则：
//    - ANSI 剥离默认开启（无配置字段），因为它是 session_id 正确提取的前置条件
//    - 噪声过滤全部通过 output_filter_regex 元数据驱动，零硬编码
//    - ConPTY 路径噪声是平台层客观存在的，硬编码在 L0

use regex::Regex;
use std::sync::LazyLock;

/// 预编译的 ConPTY 路径噪声正则
/// 匹配两类噪声：
///   1. 带 BEL 字符的路径噪声：`C:Users\x07...`、`:Users\x07...`
///   2. distlib launcher 输出的短行噪声（含 .exe 且长度 < 100）
static CONPTY_PATH_NOISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^[:A-Za-z]:.*?[\x07]|.*?\.exe[\x07]|^.{0,99}\.exe$)").unwrap()
});

/// 预编译的 ANSI 转义序列正则
/// 匹配所有 CSI (Control Sequence Introducer) 序列:
/// - SGR (颜色): \x1b[...m
/// - 光标移动: \x1b[...A/B/C/D/H/J/K/f/g
/// - 擦除: \x1b[...J/K
/// - 滚动: \x1b[...r/s
/// - 标题设置: \x1b]0;...\x07 (OSC sequences)
/// - BEL (\x07): 终端响铃字符
static ANSI_ESCAPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x07)").unwrap());

/// LineProcessor 状态
#[derive(Clone, Debug)]
pub struct LineProcessor {
    /// 是否剥离 ANSI 颜色码（默认 true）
    strip_ansi: bool,
    /// 是否剥离 ConPTY 路径噪声（默认 true）
    strip_conpty_noise: bool,
    /// Agent 专属过滤正则
    output_filter_regex: String,
    /// 预编译的过滤正则（延迟初始化）
    filter_re: Option<Regex>,
}

impl LineProcessor {
    /// 创建新的 LineProcessor
    ///
    /// # Arguments
    /// * `strip_ansi` — 是否剥离 ANSI 颜色码（默认 true）
    /// * `strip_conpty_noise` — 是否剥离 ConPTY 路径噪声（默认 true）
    /// * `output_filter_regex` — Agent 专属过滤正则表达式
    pub fn new(output_filter_regex: &str, strip_ansi: bool, strip_conpty_noise: bool) -> Self {
        let filter_re = if !output_filter_regex.is_empty() {
            Regex::new(output_filter_regex).ok()
        } else {
            None
        };

        Self {
            strip_ansi,
            strip_conpty_noise,
            output_filter_regex: output_filter_regex.to_string(),
            filter_re,
        }
    }

    /// 创建默认实例：ANSI 剥离和 ConPTY 噪声剥离均开启
    pub fn default_with_filter(output_filter_regex: &str) -> Self {
        Self::new(output_filter_regex, true, true)
    }

    /// 处理单行输出，返回过滤后的纯文本行
    ///
    /// 处理流程：
    ///   1. [L0 平台层] 剥离 ConPTY 路径噪声 → 命中则直接返回 None
    ///   2. [L1 Agent 配置层] 剥离 ANSI 颜色码
    ///   3. [L1 Agent 配置层] 按 output_filter_regex 过滤
    ///   4. 返回纯文本行（trim 后非空则保留）
    ///
    /// # Returns
    /// Some(cleaned_line) — 通过所有过滤的有效内容行
    /// None — 被过滤掉的噪声行或空行
    pub fn process_line(&self, line: &str) -> Option<String> {
        let trimmed = line.trim();

        // 空行直接丢弃
        if trimmed.is_empty() {
            return None;
        }

        // L0: 平台层 — ConPTY 路径噪声过滤
        if self.strip_conpty_noise && CONPTY_PATH_NOISE_RE.is_match(trimmed) {
            return None;
        }

        // L1: Agent 配置层 — ANSI 颜色码剥离
        let cleaned = if self.strip_ansi {
            ANSI_ESCAPE_RE.replace_all(trimmed, "").to_string()
        } else {
            trimmed.to_string()
        };

        // L1: Agent 配置层 — output_filter_regex 过滤
        if let Some(ref re) = self.filter_re {
            if re.is_match(&cleaned) {
                return None;
            }
        }

        // 过滤后仍非空则返回
        if !cleaned.is_empty() {
            Some(cleaned + "\n")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_basic() {
        let lp = LineProcessor::default_with_filter("");
        let result = lp.process_line("\x1b[36mHello World\x1b[0m");
        assert_eq!(result, Some("Hello World\n".to_string()));
    }

    #[test]
    fn test_strip_ansi_multiple_sequences() {
        let lp = LineProcessor::default_with_filter("");
        let result = lp.process_line("\x1b[1m\x1b[31mRed Bold\x1b[0m");
        assert_eq!(result, Some("Red Bold\n".to_string()));
    }

    #[test]
    fn test_no_ansi_needed() {
        let lp = LineProcessor::default_with_filter("");
        let result = lp.process_line("Plain text line");
        assert_eq!(result, Some("Plain text line\n".to_string()));
    }

    #[test]
    fn test_filter_regex_hermes_patterns() {
        let lp = LineProcessor::default_with_filter(
            r"^(Initializing agent|Resume this session|Session:|Duration:|Messages:|Query:)",
        );
        assert_eq!(lp.process_line("Initializing agent v0.1.0"), None);
        assert_eq!(lp.process_line("Resume this session?"), None);
        assert_eq!(lp.process_line("Session: abc123"), None);
        assert_eq!(lp.process_line("Duration: 5s"), None);
        assert_eq!(lp.process_line("Messages: 10"), None);
        assert_eq!(lp.process_line("Query: hello"), None);
        assert_eq!(
            lp.process_line("Here is my analysis..."),
            Some("Here is my analysis...\n".to_string())
        );
    }

    #[test]
    fn test_filter_regex_with_ansi() {
        let lp = LineProcessor::default_with_filter(
            r"^(Initializing agent|Resume this session|Session:|Duration:|Messages:|Query:)",
        );
        assert_eq!(lp.process_line("\x1b[1mSession: abc123\x1b[0m"), None);
        assert_eq!(
            lp.process_line("\x1b[32mInitializing agent...\x1b[0m"),
            None
        );
    }

    #[test]
    fn test_conpty_path_noise_drive_letter() {
        let lp = LineProcessor::default_with_filter("");
        let result = lp.process_line(":Users\x07SomePath");
        assert_eq!(result, None, "ConPTY path noise should be filtered");
        let result = lp.process_line("C:Users\\hermes.exe\x07");
        assert_eq!(result, None, "hermes.exe noise should be filtered");
    }

    #[test]
    fn test_conpty_path_noise_bare_exe() {
        let lp = LineProcessor::default_with_filter("");
        // Distlib launcher outputs short hermes.exe lines
        let result = lp.process_line("hermes.exe");
        assert_eq!(result, None, "bare hermes.exe should be filtered");
    }

    #[test]
    fn test_empty_line_filtered() {
        let lp = LineProcessor::default_with_filter("");
        assert_eq!(lp.process_line(""), None);
        assert_eq!(lp.process_line("   "), None);
        assert_eq!(lp.process_line("\n"), None);
    }

    #[test]
    fn test_empty_filter_regex() {
        let lp = LineProcessor::default_with_filter("");
        assert!(lp.process_line("Any content").is_some());
    }

    #[test]
    fn test_strip_ansi_disabled() {
        let lp = LineProcessor::new("", false, true);
        let result = lp.process_line("\x1b[36mColored\x1b[0m");
        assert_eq!(result, Some("\x1b[36mColored\x1b[0m\n".to_string()));
    }

    #[test]
    fn test_conpty_noise_disabled() {
        let lp = LineProcessor::new("", true, false);
        let result = lp.process_line(":Users\x07SomePath");
        assert!(result.is_some());
    }

    #[test]
    fn test_combined_pipeline() {
        let lp = LineProcessor::default_with_filter(r"^(Initializing agent|Session:|Query:)");
        assert_eq!(lp.process_line("\x1b[1mInitializing agent\x1b[0m"), None);
        assert_eq!(lp.process_line("Session: abc123"), None);
        assert_eq!(
            lp.process_line("\x1b[32mHere is the answer\x1b[0m"),
            Some("Here is the answer\n".to_string())
        );
        assert_eq!(
            lp.process_line("Normal response text"),
            Some("Normal response text\n".to_string())
        );
    }
}
