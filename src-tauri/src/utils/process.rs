//! 进程超时策略与状态判断
//!
//! 提供基于虚拟控制台状态能力的超时检测基础设施。
//! 核心原则：超时检测通过虚拟控制台提供的 `try_wait` / `is_running` 判断进程状态，
//! 不旁路 OS API（如 OpenProcess / kill(pid, 0)）。
//!
//! 本模块提供：
//! - `TimeoutPolicy`：可配置的超时策略（检查间隔、最大等待时间）
//! - `TimeoutError`：超时错误类型（进程已退出 / 仍在运行 / 超过上限）
//! - `poll_console_alive`：基于 AsyncConsole 的轮询检测纯函数
//! - `check_process_state`：基于 try_wait 结果判断进程状态并生成错误消息

use std::time::Duration;

/// 超时策略配置
///
/// 调用方通过此结构指定超时检测参数。
/// 默认值针对 Agent LLM 推理场景（长时间等待）优化。
#[derive(Debug, Clone)]
pub struct TimeoutPolicy {
    /// 每次轮询的间隔时间
    pub check_interval: Duration,
    /// 最大等待总时长（超过后无论进程状态均返回错误）
    pub max_wait: Duration,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        // Agent LLM 推理场景：每 500ms 轮询一次，最多等待 10 分钟
        Self {
            check_interval: Duration::from_millis(500),
            max_wait: Duration::from_secs(600),
        }
    }
}

impl TimeoutPolicy {
    /// 创建快速超时策略（适用于简单命令执行）
    #[allow(dead_code)]
    pub fn quick() -> Self {
        Self {
            check_interval: Duration::from_millis(500),
            max_wait: Duration::from_secs(30),
        }
    }

    /// 创建标准超时策略（适用于版本检测等中等耗时命令）
    #[allow(dead_code)]
    pub fn standard() -> Self {
        Self {
            check_interval: Duration::from_millis(500),
            max_wait: Duration::from_secs(120),
        }
    }

    /// 创建 LLM 推理超时策略（适用于 Agent 会话交互）
    pub fn llm_inference() -> Self {
        Self {
            check_interval: Duration::from_millis(500),
            max_wait: Duration::from_secs(600),
        }
    }

    /// 自定义超时策略
    pub fn custom(check_interval_secs: u64, max_wait_secs: u64) -> Self {
        Self {
            check_interval: Duration::from_secs(check_interval_secs),
            max_wait: Duration::from_secs(max_wait_secs),
        }
    }
}

/// 超时错误类型
#[derive(Debug)]
pub enum TimeoutError {
    /// 进程已退出（附带退出码和 stderr 内容）
    ProcessExited {
        exit_code: i32,
        stderr_summary: String,
    },
    /// 进程仍在运行但超过最大等待时间
    StillAlive {
        elapsed_secs: f64,
        max_wait_secs: f64,
    },
    /// 超时轮询的底层数据源已关闭（channel/接收端断开）
    #[allow(dead_code)]
    ChannelDisconnected(String),
    /// 主动取消（外部触发取消，非超时）
    #[allow(dead_code)]
    Cancelled(String),
}

impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeoutError::ProcessExited { exit_code, stderr_summary } => {
                if stderr_summary.is_empty() {
                    write!(f, "进程已退出 (code={})", exit_code)
                } else {
                    write!(f, "进程已退出 (code={}), stderr: {}", exit_code, stderr_summary)
                }
            }
            TimeoutError::StillAlive { elapsed_secs, max_wait_secs } => {
                write!(f, "超过最大等待时间 ({:.0}s)，进程仍在运行 (已等待 {:.0}s)", max_wait_secs, elapsed_secs)
            }
            TimeoutError::ChannelDisconnected(msg) => {
                write!(f, "执行通道异常断开: {}", msg)
            }
            TimeoutError::Cancelled(msg) => {
                write!(f, "已取消: {}", msg)
            }
        }
    }
}

impl std::error::Error for TimeoutError {}

/// 基于虚拟控制台的 try_wait 结果判断进程状态。
///
/// 纯函数，不持有任何状态，便于测试。
///
/// - `try_wait_result`：`Some(exit_code)` = 已退出，`None` = 仍在运行
/// - `elapsed`：已等待时长
/// - `max_wait`：最大等待时长
/// - `operation_name`：操作名称（用于错误消息）
/// - `stderr_summary`：stderr 摘要（进程退出时附带到错误消息中）
///
/// 返回 `None` 表示进程仍在运行且未超时（应继续等待），
/// 返回 `Some(TimeoutError)` 表示应终止等待并向上层报告。
pub fn check_process_state(
    try_wait_result: Option<i32>,
    elapsed: Duration,
    max_wait: Duration,
    #[allow(unused_variables)]
    operation_name: &str,
    #[allow(unused_variables)]
    stderr_summary: &str,
) -> Option<TimeoutError> {
    match try_wait_result {
        Some(exit_code) => {
            // 进程已退出 → 无论是否超时，报告进程退出
            Some(TimeoutError::ProcessExited {
                exit_code,
                stderr_summary: stderr_summary.to_string(),
            })
        }
        None => {
            // 进程仍在运行 → 检查是否超过最大等待时间
            if elapsed >= max_wait {
                Some(TimeoutError::StillAlive {
                    elapsed_secs: elapsed.as_secs_f64(),
                    max_wait_secs: max_wait.as_secs_f64(),
                })
            } else {
                // 仍在运行且未超时 → 继续等待
                None
            }
        }
    }
}

/// 构建进程退出的超时错误消息（便捷函数）。
///
/// 当检测到进程已退出时，调用此函数构建包含 stderr 摘要的错误。
#[allow(dead_code)]
pub fn make_exited_error(
    operation_name: &str,
    exit_code: i32,
    elapsed: Duration,
    stderr_summary: &str,
) -> String {
    if stderr_summary.is_empty() {
        format!("{} 超时 ({:.1}s)，进程已退出 (code={})", operation_name, elapsed.as_secs_f64(), exit_code)
    } else {
        format!("{} 超时 ({:.1}s)，进程已退出 (code={}), stderr: {}",
            operation_name, elapsed.as_secs_f64(), exit_code, stderr_summary)
    }
}

/// 构建进程仍在运行的超时错误消息（便捷函数）。
pub fn make_still_alive_error(operation_name: &str, elapsed: Duration, max_wait: Duration) -> String {
    format!("{} 超过最大等待时间 ({:.0}s)，进程仍在运行 (已等待 {:.0}s)",
        operation_name, max_wait.as_secs_f64(), elapsed.as_secs_f64())
}

/// 获取 stderr 的摘要文本（截取最后 N 个字符，避免错误消息过长）。
///
/// - `stderr_lines`：stderr 的全部行
/// - `max_chars`：摘要最大字符数
/// - `tail_lines`：取最后几行
pub fn summarize_stderr(stderr_lines: &[String], max_chars: usize, tail_lines: usize) -> String {
    if stderr_lines.is_empty() {
        return String::new();
    }

    let tail: Vec<&String> = if stderr_lines.len() <= tail_lines {
        stderr_lines.iter().collect()
    } else {
        stderr_lines[stderr_lines.len() - tail_lines..].iter().collect()
    };

    let joined = tail.iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");

    if joined.len() <= max_chars {
        joined
    } else {
        format!("...{}", &joined[joined.len() - max_chars..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeout_policy_defaults() {
        let policy = TimeoutPolicy::default();
        assert_eq!(policy.check_interval, Duration::from_millis(500));
        assert_eq!(policy.max_wait, Duration::from_secs(600));
    }

    #[test]
    fn test_timeout_policy_quick() {
        let policy = TimeoutPolicy::quick();
        assert_eq!(policy.check_interval, Duration::from_millis(500));
        assert_eq!(policy.max_wait, Duration::from_secs(30));
    }

    #[test]
    fn test_check_process_state_still_running() {
        let result = check_process_state(
            None, // 仍在运行
            Duration::from_secs(10),
            Duration::from_secs(600),
            "test_op",
            "",
        );
        assert!(result.is_none(), "进程运行中且未超时应返回 None");
    }

    #[test]
    fn test_check_process_state_exited() {
        let result = check_process_state(
            Some(1), // 已退出
            Duration::from_secs(10),
            Duration::from_secs(600),
            "test_op",
            "some error",
        );
        assert!(result.is_some());
        match result.unwrap() {
            TimeoutError::ProcessExited { exit_code, stderr_summary } => {
                assert_eq!(exit_code, 1);
                assert_eq!(stderr_summary, "some error");
            }
            _ => panic!("应返回 ProcessExited"),
        }
    }

    #[test]
    fn test_check_process_state_still_alive_timeout() {
        let result = check_process_state(
            None, // 仍在运行
            Duration::from_secs(600),
            Duration::from_secs(600),
            "test_op",
            "",
        );
        assert!(result.is_some());
        match result.unwrap() {
            TimeoutError::StillAlive { elapsed_secs, max_wait_secs } => {
                assert_eq!(elapsed_secs, 600.0);
                assert_eq!(max_wait_secs, 600.0);
            }
            _ => panic!("应返回 StillAlive"),
        }
    }

    #[test]
    fn test_summarize_stderr_empty() {
        assert_eq!(summarize_stderr(&[], 200, 3), "");
    }

    #[test]
    fn test_summarize_stderr_truncation() {
        let lines: Vec<String> = (0..10).map(|i| format!("line {}", i)).collect();
        let summary = summarize_stderr(&lines, 20, 3);
        assert!(summary.starts_with("line 7 | line 8 | line 9"));
    }
}
