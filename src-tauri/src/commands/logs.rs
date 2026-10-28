//! 后端日志的**倒序读取**（反馈弹窗的「日志片段」来源）。
//!
//! 日志本身由 `lib.rs::init_logging` 落到 `<配置根>/logs/app.log`（5MB 滚动，保留 3 份），
//! 重启不会丢。这里**只读不写**，不新增任何存储：
//! - 从文件末尾向前读（最多 4MB），不整文件加载；
//! - 多文件按修改时间倒序合并（`app.log` → `.1` → …）；
//! - 解析成结构化条目，按级别 / 模块 / 时间 / 聚焦窗口过滤；
//! - 输出前脱敏（令牌、密钥、用户目录）并截断。
//!
//! 噪音治理不靠关键词猜测：`env_logger` 的 target 就是模块路径，
//! 环境探测类日志（`commands::env` / `agent::lifecycle` / `commands::agent_market`）
//! 由前端按 target 前缀整体排除。

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use chrono::DateTime;
use serde::{Deserialize, Serialize};

/// 单次最多从尾部读多少字节（4MB；避免大文件全量加载）
const MAX_TAIL_BYTES: u64 = 4 * 1024 * 1024;
/// 单条消息最大字符数
const MAX_MESSAGE_CHARS: usize = 500;
/// 结果条数默认值 / 上限
const DEFAULT_LIMIT: usize = 40;
const MAX_LIMIT: usize = 200;
/// 聚焦窗口默认值（秒）：通知时间前后 3 分钟
const DEFAULT_FOCUS_WINDOW_SECS: u64 = 180;
const MIN_FOCUS_WINDOW_SECS: u64 = 10;
const MAX_FOCUS_WINDOW_SECS: u64 = 3600;

/// 日志条目（脱敏、截断后）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// 毫秒时间戳（UTC）
    pub ts: i64,
    /// 小写级别：`error` / `warn` / `info` / `debug` / `trace`
    pub level: String,
    /// 模块路径（`env_logger` 的 target）
    pub target: String,
    pub message: String,
}

/// 读取查询
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    /// 需要的级别（小写）；为空时默认 `warn` + `error`
    #[serde(default)]
    pub levels: Vec<String>,
    /// 要排除的 target 前缀（探测类噪音）
    #[serde(default)]
    pub exclude_targets: Vec<String>,
    /// 时间下限（毫秒），0 = 不限
    #[serde(default)]
    pub since_ts: i64,
    /// 条数上限
    #[serde(default)]
    pub limit: usize,
    /// 聚焦时间点（毫秒）：非 0 时只保留该时刻前后的窗口
    #[serde(default)]
    pub focus_ts: i64,
    /// 聚焦窗口（秒）
    #[serde(default)]
    pub focus_window_secs: u64,
}

/// 读取结果
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub entries: Vec<LogEntry>,
    /// 本次实际覆盖到的**最早**日志时间（0 = 没读到）。用于界面提示"可见范围自何时起"
    pub coverage_start: i64,
    /// 本次实际扫描的文件数（含滚动产物）
    pub scanned_files: usize,
}

fn log_dir() -> PathBuf {
    crate::utils::paths::app_root_dir().join("logs")
}

/// 候选日志文件：当前文件 + 滚动产物，按修改时间倒序。
fn candidate_files() -> Vec<PathBuf> {
    let dir = log_dir();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for name in ["app.log", "app.log.1", "app.log.2", "app.log.3"] {
        let path = dir.join(name);
        if let Ok(md) = std::fs::metadata(&path) {
            if md.is_file() {
                files.push((md.modified().unwrap_or(UNIX_EPOCH), path));
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().map(|(_, p)| p).collect()
}

/// 从文件尾部读最多 `budget` 字节，返回**正序**行。
///
/// 两个可能被截断的位置都要丢掉：起始偏移落在行中间、文件正被追加导致末行不完整。
fn read_tail_lines(path: &Path, budget: u64) -> Vec<String> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let len = match file.metadata() {
        Ok(m) => m.len(),
        Err(_) => return Vec::new(),
    };
    if len == 0 {
        return Vec::new();
    }
    let start = len.saturating_sub(budget);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }

    let ends_with_newline = buf.last() == Some(&b'\n');
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|s| s.trim_end_matches('\r').to_string())
        .collect();

    // 起始偏移不在行首：第一行只是半截，丢掉
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    // 文件正在追加：最后一行可能还没写完
    if !ends_with_newline && !lines.is_empty() {
        lines.pop();
    }
    lines.retain(|l| !l.is_empty());
    lines
}

/// `env_logger` 行格式（时间戳为**本机时区带偏移**）：`[2026-10-28T11:29:06.727+08:00 INFO  module] message`
///
/// 同时兼容早期版本落的 UTC（`...Z`）——升级前后的日志文件会同时存在于滚动产物里。
fn log_line_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"^\[(?P<ts>\d{4}-\d{2}-\d{2}T[0-9:.]+(?:Z|[+-]\d{2}:\d{2}))\s+(?P<level>[A-Z]+)\s+(?P<target>[^\]]+)\]\s?(?P<msg>.*)$",
        )
        .expect("日志行正则编译失败")
    })
}

fn parse_ts(raw: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn sanitize_regexes() -> &'static Vec<(regex::Regex, &'static str)> {
    static RES: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    RES.get_or_init(|| {
        vec![
            // Authorization: Bearer xxx / bearer xxx
            (
                regex::Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._\-]+").unwrap(),
                "${1}***",
            ),
            // OpenAI 风格密钥
            (
                regex::Regex::new(r"sk-[A-Za-z0-9_\-]{8,}").unwrap(),
                "sk-***",
            ),
            // api_key= / token: / secret=
            (
                regex::Regex::new(r#"(?i)((?:api[_-]?key|token|secret)\s*[=:]\s*)[^\s,;&'"]+"#)
                    .unwrap(),
                "${1}***",
            ),
            // Windows 用户目录：C:\Users\<name>
            (
                regex::Regex::new(r#"([A-Za-z]:\\Users\\)[^\\\s'"]+"#).unwrap(),
                "${1}***",
            ),
            // Unix 用户目录：/home/<name>
            (
                regex::Regex::new(r#"(/home/)[^/\s'"]+"#).unwrap(),
                "${1}***",
            ),
        ]
    })
}

fn sanitize(text: &str) -> String {
    let mut out = text.to_string();
    for (re, replacement) in sanitize_regexes() {
        out = re.replace_all(&out, *replacement).to_string();
    }
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// 把一个文件的行解析成**正序**条目。
///
/// 解析失败的行视为上一条消息的续行（多行错误/堆栈）：`env_logger` 不会给续行加前缀，
/// 直接丢弃会让最有价值的堆栈信息消失。
fn parse_lines(lines: &[String]) -> Vec<LogEntry> {
    let re = log_line_regex();
    let mut out: Vec<LogEntry> = Vec::new();
    for line in lines {
        let Some(caps) = re.captures(line) else {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(line);
            }
            continue;
        };
        let Some(ts) = parse_ts(&caps["ts"]) else {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(line);
            }
            continue;
        };
        out.push(LogEntry {
            ts,
            level: caps["level"].to_lowercase(),
            target: caps["target"].trim().to_string(),
            message: caps["msg"].to_string(),
        });
    }
    out
}

impl LogQuery {
    fn normalized(&self) -> NormalizedQuery {
        let levels: Vec<String> = if self.levels.is_empty() {
            vec!["warn".to_string(), "error".to_string()]
        } else {
            self.levels
                .iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        };
        NormalizedQuery {
            levels,
            exclude_targets: self
                .exclude_targets
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            since_ts: self.since_ts,
            limit: if self.limit == 0 {
                DEFAULT_LIMIT
            } else {
                self.limit.min(MAX_LIMIT)
            },
            focus_ts: self.focus_ts,
            focus_window_secs: if self.focus_window_secs == 0 {
                DEFAULT_FOCUS_WINDOW_SECS
            } else {
                self.focus_window_secs
                    .clamp(MIN_FOCUS_WINDOW_SECS, MAX_FOCUS_WINDOW_SECS)
            },
        }
    }
}

struct NormalizedQuery {
    levels: Vec<String>,
    exclude_targets: Vec<String>,
    since_ts: i64,
    limit: usize,
    focus_ts: i64,
    focus_window_secs: u64,
}

impl NormalizedQuery {
    fn matches(&self, e: &LogEntry) -> bool {
        if !self.levels.contains(&e.level) {
            return false;
        }
        if self
            .exclude_targets
            .iter()
            .any(|prefix| e.target.starts_with(prefix))
        {
            return false;
        }
        if self.since_ts > 0 && e.ts < self.since_ts {
            return false;
        }
        if self.focus_ts > 0 {
            let window = (self.focus_window_secs as i64) * 1000;
            if (e.ts - self.focus_ts).abs() > window {
                return false;
            }
        }
        true
    }
}

/// 倒序读取最近的日志条目。
///
/// 只读 `app.log` 及滚动产物，不修改任何文件；文件读不到时返回空结果而不是报错
/// （反馈弹窗不该因为日志缺失而无法打开）。
#[tauri::command]
pub async fn read_recent_logs(query: LogQuery) -> Result<LogPage, String> {
    let q = query.normalized();
    let mut entries: Vec<LogEntry> = Vec::new();
    let mut coverage_start = 0i64;
    let files = candidate_files();
    let scanned_files = files.len();

    for path in files {
        if entries.len() >= q.limit {
            break;
        }
        let lines = read_tail_lines(&path, MAX_TAIL_BYTES);
        if lines.is_empty() {
            continue;
        }
        let parsed = parse_lines(&lines);
        if let Some(first) = parsed.first() {
            // 越靠后的文件越旧，覆盖起点取每次读到的更早时间
            if coverage_start == 0 || first.ts < coverage_start {
                coverage_start = first.ts;
            }
        }
        // 文件内正序 → 整体倒序（当前文件最新，先遍历）
        for entry in parsed.into_iter().rev() {
            if !q.matches(&entry) {
                continue;
            }
            entries.push(LogEntry {
                ts: entry.ts,
                level: entry.level,
                target: entry.target,
                message: truncate_chars(&sanitize(&entry.message), MAX_MESSAGE_CHARS),
            });
            if entries.len() >= q.limit {
                break;
            }
        }
    }

    Ok(LogPage {
        entries,
        coverage_start,
        scanned_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ts: &str, level: &str, target: &str, msg: &str) -> String {
        // env_logger 的级别与 target 之间是两个空格
        format!("[{} {}  {}] {}", ts, level, target, msg)
    }

    fn rfc3339_millis(raw: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn parses_local_offset_timestamp() {
        let lines = vec![line(
            "2026-10-28T11:29:06.727+08:00",
            "WARN",
            "pilotdesk_lib::commands::session",
            "创建会话失败",
        )];
        let entries = parse_lines(&lines);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, "warn");
        assert_eq!(entries[0].target, "pilotdesk_lib::commands::session");
        assert_eq!(entries[0].ts, rfc3339_millis("2026-10-28T03:29:06.727Z"));
    }

    #[test]
    fn parses_legacy_utc_timestamp() {
        let lines = vec![line(
            "2026-10-21T16:34:51.758Z",
            "ERROR",
            "pilotdesk_lib",
            "旧版本日志",
        )];
        let entries = parse_lines(&lines);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, "error");
        assert_eq!(entries[0].ts, rfc3339_millis("2026-10-21T16:34:51.758Z"));
    }

    #[test]
    fn attaches_continuation_lines_to_previous_entry() {
        let lines = vec![
            line("2026-10-28T11:29:06.727+08:00", "ERROR", "pilotdesk_lib::x", "失败"),
            "    at foo::bar (x.rs:1)".to_string(),
        ];
        let entries = parse_lines(&lines);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].message.contains("at foo::bar"));
    }

    #[test]
    fn sanitizes_secrets_and_user_paths() {
        let raw = "Authorization: Bearer abc12345 api_key=secret1 C:\\Users\\alice\\x sk-1234567890";
        let out = sanitize(raw);
        assert!(!out.contains("abc12345"));
        assert!(!out.contains("secret1"));
        assert!(!out.contains("alice"));
        assert!(!out.contains("1234567890"));
    }

    #[test]
    fn query_excludes_probe_targets_and_filters_level() {
        let q = LogQuery {
            levels: vec!["warn".into(), "error".into()],
            exclude_targets: vec!["pilotdesk_lib::commands::env".into()],
            since_ts: 0,
            limit: 10,
            focus_ts: 0,
            focus_window_secs: 0,
        }
        .normalized();

        let warn_probe = LogEntry {
            ts: 1,
            level: "warn".into(),
            target: "pilotdesk_lib::commands::env".into(),
            message: "版本查询失败".into(),
        };
        let warn_session = LogEntry {
            ts: 2,
            level: "warn".into(),
            target: "pilotdesk_lib::commands::session".into(),
            message: "创建会话失败".into(),
        };
        let info_session = LogEntry {
            ts: 3,
            level: "info".into(),
            target: "pilotdesk_lib::commands::session".into(),
            message: "正常".into(),
        };

        assert!(!q.matches(&warn_probe));
        assert!(q.matches(&warn_session));
        assert!(!q.matches(&info_session));
    }
}

