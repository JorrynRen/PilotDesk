//! 进程超时策略与状态判断
//!
//! 提供基于进程状态能力的超时检测基础设施。
//! 核心原则：超时检测通过 tokio::process::Child 的 `try_wait` 判断进程状态，
//! 不旁路 OS API（如 OpenProcess / kill(pid, 0)）。
//!
//! 本模块提供：
//! - `TimeoutPolicy`：可配置的超时策略（检查间隔、最大等待时间）
//! - `TimeoutError`：超时错误类型（进程已退出 / 仍在运行 / 超过上限）
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

    /// 创建 LLM 推理超时策略（适用于 Agent 会话交互）
    pub fn llm_inference() -> Self {
        Self {
            check_interval: Duration::from_millis(500),
            max_wait: Duration::from_secs(600),
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
            TimeoutError::ProcessExited {
                exit_code,
                stderr_summary,
            } => {
                if stderr_summary.is_empty() {
                    write!(f, "进程已退出 (code={})", exit_code)
                } else {
                    write!(
                        f,
                        "进程已退出 (code={}), stderr: {}",
                        exit_code, stderr_summary
                    )
                }
            }
            TimeoutError::StillAlive {
                elapsed_secs,
                max_wait_secs,
            } => {
                write!(
                    f,
                    "超过最大等待时间 ({:.0}s)，进程仍在运行 (已等待 {:.0}s)",
                    max_wait_secs, elapsed_secs
                )
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

/// 基于 tokio::process::Child::try_wait 结果判断进程状态。
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
    #[allow(unused_variables)] operation_name: &str,
    #[allow(unused_variables)] stderr_summary: &str,
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
        format!(
            "{} 超时 ({:.1}s)，进程已退出 (code={})",
            operation_name,
            elapsed.as_secs_f64(),
            exit_code
        )
    } else {
        format!(
            "{} 超时 ({:.1}s)，进程已退出 (code={}), stderr: {}",
            operation_name,
            elapsed.as_secs_f64(),
            exit_code,
            stderr_summary
        )
    }
}

/// 构建进程仍在运行的超时错误消息（便捷函数）。
pub fn make_still_alive_error(
    operation_name: &str,
    elapsed: Duration,
    max_wait: Duration,
) -> String {
    format!(
        "{} 超过最大等待时间 ({:.0}s)，进程仍在运行 (已等待 {:.0}s)",
        operation_name,
        max_wait.as_secs_f64(),
        elapsed.as_secs_f64()
    )
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
        stderr_lines[stderr_lines.len() - tail_lines..]
            .iter()
            .collect()
    };

    let joined = tail
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");

    if joined.chars().count() <= max_chars {
        joined
    } else {
        // 按字符取尾部：进程输出（构建报错等）常含中文，按字节取尾会切进字符内部 panic
        crate::utils::text::elide_tail(&joined, max_chars)
    }
}

/// Windows `CREATE_NO_WINDOW`：不为新子进程分配控制台窗口。
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 给 `std::process::Command` 关闭控制台窗口（非 Windows 平台空操作）。
pub fn hide_console(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// 给 `tokio::process::Command` 关闭控制台窗口（非 Windows 平台空操作）。
pub fn hide_console_tokio(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// 构造一个"不弹控制台窗口"的 `std::process::Command`（返回值可继续链式调用 args/current_dir）。
pub fn hidden_command(program: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    hide_console(&mut cmd);
    cmd
}

// ──────────────────────────────────────────────
//  Windows 可执行文件解析
// ──────────────────────────────────────────────

/// 把裸命令名解析为**可被 CreateProcess 真正启动**的目标。
///
/// 背景：Windows 的 `CreateProcess` **只自动补 `.exe`**，不会走 `PATHEXT`。
/// npm 全局安装的 CLI（如 `claude` / `codex`）只有 `*.cmd` / `*.ps1` 垫片、
/// **没有同名 `.exe`**，因此 `Command::new("claude")` 必然 "program not found"。
///
/// 返回 `(程序, 前置参数)`；调用方执行 `程序 前置参数... 原参数...`。
///
/// 解析顺序（仅 Windows 生效）：
/// 1. 已含路径分隔符 → 原样；
/// 2. PATH 中存在 `name.exe` → 直连；
/// 3. 存在 `name.cmd` / `name.bat` → 解析垫片取**真实目标**：
///    `.exe` 直连（零 shell、参数无损）；`.js` / `.cjs` 用 `node` 启动；
///    解析不出则退回 `cmd /C <垫片>`；
/// 4. 只有 `name.ps1` → `powershell -NoProfile -ExecutionPolicy Bypass -File <垫片>`。
#[cfg(target_os = "windows")]
pub fn resolve_windows_program(program: &str) -> (String, Vec<String>) {
    use std::path::{Path, PathBuf};

    // 1) 已带路径：原样交给 CreateProcess
    if program.contains('\\') || program.contains('/') {
        return (program.to_string(), Vec::new());
    }

    let dirs: Vec<PathBuf> = std::env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .collect();

    // 2) name.exe 直连
    for d in &dirs {
        let p = d.join(format!("{}.exe", program));
        if p.is_file() {
            return (p.to_string_lossy().to_string(), Vec::new());
        }
    }

    // 3) name.cmd / name.bat 垫片
    for ext in ["cmd", "bat"] {
        for d in &dirs {
            let shim = d.join(format!("{}.{}", program, ext));
            if !shim.is_file() {
                continue;
            }
            let shim_dir = shim.parent().unwrap_or(Path::new("."));
            if let Ok(content) = std::fs::read_to_string(&shim) {
                let targets = extract_shim_targets(&content, &shim_dir.to_string_lossy());
                if let Some((prog, prefix)) = pick_shim_target(&targets) {
                    return (prog, prefix);
                }
            }
            // 垫片格式无法解析：退回经 cmd 启动（此路径参数会多经一层 cmd 解析）
            return (
                "cmd".to_string(),
                vec![
                    "/C".to_string(),
                    shim.to_string_lossy().to_string(),
                ],
            );
        }
    }

    // 4) name.ps1 垫片
    for d in &dirs {
        let p = d.join(format!("{}.ps1", program));
        if p.is_file() {
            return (
                "powershell".to_string(),
                vec![
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-File".to_string(),
                    p.to_string_lossy().to_string(),
                ],
            );
        }
    }

    (program.to_string(), Vec::new())
}

/// 非 Windows：无需解析（POSIX 由 execvp 走 PATH，可直接执行脚本）。
#[cfg(not(target_os = "windows"))]
pub fn resolve_windows_program(program: &str) -> (String, Vec<String>) {
    (program.to_string(), Vec::new())
}

/// 从 npm 垫片内容里提取候选目标路径（展开 `%~dp0` / `%dp0%`），**不检查存在性**。
///
/// 只取引号内的字符串——npm 生成的垫片一律用引号包裹路径。
#[cfg(target_os = "windows")]
fn extract_shim_targets(content: &str, shim_dir: &str) -> Vec<String> {
    let dir = format!("{}\\", shim_dir.trim_end_matches('\\'));
    let expanded = content.replace("%~dp0", &dir).replace("%dp0%", &dir);

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for ch in expanded.chars() {
        match ch {
            '"' => {
                if in_quote {
                    // 展开后可能出现双反斜杠（`%dp0%` 自带尾分隔符 + 垫片里的 `\`），归一化
                    let t = cur.trim().replace("\\\\", "\\");
                    if t.len() > 3
                        && (t.ends_with(".exe") || t.ends_with(".js") || t.ends_with(".cjs"))
                    {
                        out.push(t);
                    }
                    cur.clear();
                }
                in_quote = !in_quote;
            }
            _ => {
                if in_quote {
                    cur.push(ch);
                }
            }
        }
    }
    out
}

/// 在候选中选真实目标：优先**存在**的 `.exe`（直连），其次存在的 `.js` / `.cjs`（用 `node` 启动）。
#[cfg(target_os = "windows")]
fn pick_shim_target(targets: &[String]) -> Option<(String, Vec<String>)> {
    for t in targets {
        if t.ends_with(".exe") && std::path::Path::new(t).is_file() {
            return Some((t.clone(), Vec::new()));
        }
    }
    for t in targets {
        if (t.ends_with(".js") || t.ends_with(".cjs")) && std::path::Path::new(t).is_file() {
            return Some(("node".to_string(), vec![t.clone()]));
        }
    }
    None
}

#[cfg(all(test, target_os = "windows"))]
mod windows_shim_tests {
    use super::*;

    /// 实测样本：`claude.cmd` → 直连 node_modules 下的 `claude.exe`
    const CLAUDE_CMD: &str = concat!(
        "@ECHO off\nGOTO start\n:find_dp0\nSET dp0=%~dp0\nEXIT /b\n:start\n",
        "SETLOCAL\nCALL :find_dp0\n",
        "\"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe\"   %*\n"
    );

    /// 实测样本：`codex.cmd` → `node` + `codex.js`
    const CODEX_CMD: &str = concat!(
        "@ECHO off\nGOTO start\n:find_dp0\nSET dp0=%~dp0\nEXIT /b\n:start\nSETLOCAL\nCALL :find_dp0\n\n",
        "IF EXIST \"%dp0%\\node.exe\" (\n  SET \"_prog=%dp0%\\node.exe\"\n) ELSE (\n",
        "  SET \"_prog=node\"\n  SET PATHEXT=%PATHEXT:;.JS;=;%\n)\n\n",
        "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\n"
    );

    #[test]
    fn claude_shim_yields_real_exe() {
        assert_eq!(
            extract_shim_targets(CLAUDE_CMD, r"C:\npm"),
            vec![r"C:\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe".to_string()]
        );
    }

    #[test]
    fn codex_shim_yields_js_target() {
        let targets = extract_shim_targets(CODEX_CMD, r"C:\npm");
        assert!(
            targets.iter().any(|t| t.ends_with(r"@openai\codex\bin\codex.js")),
            "targets={:?}",
            targets
        );
    }

    /// 已含路径分隔符 → 原样透传（不做任何猜测）
    #[test]
    fn path_like_program_passes_through() {
        let (p, a) = resolve_windows_program(r"C:\x\y.exe");
        assert_eq!(p, r"C:\x\y.exe");
        assert!(a.is_empty());
    }
}

/// 构造一个"不弹控制台窗口"的 `tokio::process::Command`。
pub fn hidden_tokio_command(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    hide_console_tokio(&mut cmd);
    cmd
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
            TimeoutError::ProcessExited {
                exit_code,
                stderr_summary,
            } => {
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
            TimeoutError::StillAlive {
                elapsed_secs,
                max_wait_secs,
            } => {
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
        // 实现语义：取最后 3 行 join（"line 7 | line 8 | line 9"），长度 24 > max_chars 20
        // → 只保留末尾 20 字符并在开头补 "..."，因此不再以整行开头。
        let summary = summarize_stderr(&lines, 20, 3);
        assert!(
            summary.starts_with("..."),
            "截断应带省略号前缀: {}",
            summary
        );
        assert!(
            summary.ends_with("line 9"),
            "应保留最后一行结尾: {}",
            summary
        );
        assert_eq!(summary.len(), 23, "3 个省略号 + 末尾 20 字符: {}", summary);
        assert!(
            summary.contains("line 8"),
            "应包含倒数第二行内容: {}",
            summary
        );
    }
}
