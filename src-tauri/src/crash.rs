//! 崩溃上报（阶段一）：把未处理的 Rust panic 落成本地崩溃文件，
//! 下次启动时由前端取走、预填进反馈弹窗，由用户确认后提交。
//!
//! 只做**本地留存 + 启动提示**，不接任何远端崩溃服务：
//! - `install_panic_hook` 接管 panic：写 `crash-<epoch>-<pid>.log`（摘要 + backtrace），
//!   同时在应用日志里打一条 error（反馈弹窗的「最近错误日志」会一并带上）；
//! - `crash_take_pending` 在启动时取未提示过的崩溃文件（最多 3 条），读完改名 `.reported`，
//!   保证同一次崩溃只提示一次，原文件仍保留供人工排查；
//! - `cleanup` 清掉超过 14 天的 `.reported`，并限制未处理文件的堆积数量。
//!
//! 为什么复用反馈链路而不自建上报：反馈已有实名鉴权、日志片段、环境信息与后台工单，
//! 崩溃只是「预填好的反馈」，不另造一套存储与后台。

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// 未提示崩溃文件的保留上限：崩溃循环时不让目录无限增长
const MAX_PENDING_FILES: usize = 10;
/// 已提示崩溃文件的保留时长（秒）
const REPORTED_RETENTION_SECS: u64 = 14 * 24 * 3600;
/// 一次启动最多取几条（多条也只弹一次，摘要会在正文里逐条列出）
const MAX_TAKE: usize = 3;
/// 摘要截断长度：反馈正文上限 500 字，要给时间/位置/其它信息留足空间
const MAX_SUMMARY_CHARS: usize = 240;

fn crash_dir() -> PathBuf {
    crate::utils::paths::app_root_dir()
        .join("logs")
        .join("crashes")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// panic payload → 人类可读文本（`panic!("…")` 与 `panic!("{}", x)` 分别是 `&str` / `String`）
fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "非字符串 panic".to_string()
}

fn panic_summary(
    payload: &(dyn std::any::Any + Send),
    location: Option<&std::panic::Location<'_>>,
) -> String {
    let msg = payload_text(payload);
    match location {
        Some(loc) => format!(
            "{}（{}:{}:{}）",
            truncate_chars(msg.trim(), MAX_SUMMARY_CHARS),
            loc.file(),
            loc.line(),
            loc.column()
        ),
        None => truncate_chars(msg.trim(), MAX_SUMMARY_CHARS),
    }
}

/// 写崩溃文件：头部是机器可解析的 `key: value`，空行之后是完整 backtrace。
fn write_crash_file(ts: u64, thread: &str, summary: &str) -> std::io::Result<PathBuf> {
    let dir = crash_dir();
    std::fs::create_dir_all(&dir)?;
    // 带上 pid：同秒内多个线程同时 panic 也不会互相覆盖
    let path = dir.join(format!("crash-{}-{}.log", ts, std::process::id()));
    let mut file = std::fs::File::create(&path)?;
    writeln!(
        file,
        "time: {}",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z")
    )?;
    writeln!(file, "version: {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(
        file,
        "platform: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )?;
    writeln!(file, "thread: {}", thread)?;
    writeln!(file, "summary: {}", summary)?;
    writeln!(
        file,
        "\nbacktrace:\n{}",
        std::backtrace::Backtrace::force_capture()
    )?;
    file.flush()?;
    Ok(path)
}

/// 接管 panic：落文件 + 打一行应用日志，然后继续走默认钩子（开发时终端仍能看到）。
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let summary = panic_summary(info.payload(), info.location());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        match write_crash_file(now_secs(), &thread, &summary) {
            Ok(path) => log::error!(
                "[crash] 未处理的 panic（线程 {}）：{}；已写入 {}",
                thread,
                summary,
                path.display()
            ),
            Err(e) => log::error!(
                "[crash] 未处理的 panic（线程 {}）：{}；崩溃文件写入失败：{}",
                thread,
                summary,
                e
            ),
        }
        default_hook(info);
    }));
}

/// 启动清理：删过期 `.reported`，并把未提示文件收敛到上限以内（保留最新的）。
pub fn cleanup() {
    let dir = crash_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut pending: Vec<(SystemTime, PathBuf)> = Vec::new();

    for entry in entries.flatten() {
        let name = match entry.file_name().to_str() {
            Some(n) if n.starts_with("crash-") => n.to_string(),
            _ => continue,
        };
        let path = entry.path();
        let mtime = entry.metadata().and_then(|m| m.modified()).unwrap_or(now);
        if name.ends_with(".reported") {
            let age = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
            if age > REPORTED_RETENTION_SECS {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        if name.ends_with(".log") {
            pending.push((mtime, path));
        }
    }

    if pending.len() > MAX_PENDING_FILES {
        pending.sort_by(|a, b| a.0.cmp(&b.0));
        let excess = pending.len() - MAX_PENDING_FILES;
        for (_, path) in pending.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 解析崩溃文件头部，返回（摘要, 补充信息）。
///
/// 头部解析失败（文件被截断等）时退回「第一行非空内容」，保证仍能提示出来。
fn parse_crash(raw: &str) -> (String, String) {
    let mut summary = String::new();
    let mut time = String::new();
    let mut version = String::new();
    let mut platform = String::new();
    let mut thread = String::new();

    for line in raw.lines() {
        if line.trim().is_empty() {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "time" => time = value,
            "version" => version = value,
            "platform" => platform = value,
            "thread" => thread = value,
            "summary" => summary = value,
            _ => {}
        }
    }

    if summary.is_empty() {
        summary = raw
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("未知异常")
            .to_string();
    }

    let mut parts: Vec<String> = Vec::new();
    if !time.is_empty() {
        parts.push(format!("时间 {}", time));
    }
    if !version.is_empty() {
        parts.push(format!("版本 {}", version));
    }
    if !platform.is_empty() {
        parts.push(format!("平台 {}", platform));
    }
    if !thread.is_empty() {
        parts.push(format!("线程 {}", thread));
    }
    (summary, parts.join(" · "))
}

/// 一条待提示的崩溃
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCrash {
    /// 文件标识（崩溃时间 + pid），前端可用于去重
    pub id: String,
    /// 崩溃时间（毫秒；取自文件名，供反馈弹窗聚焦日志）
    pub ts: i64,
    /// 一行摘要：panic 消息 + 位置
    pub summary: String,
    /// 补充信息：时间 / 版本 / 平台 / 线程
    pub detail: String,
}

/// 取走未提示的崩溃（最多 3 条，按时间倒序），读完改名 `.reported`。
#[tauri::command]
pub fn crash_take_pending() -> Vec<PendingCrash> {
    let dir = crash_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut files: Vec<(u64, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("crash-") || !name.ends_with(".log") {
            continue;
        }
        let Some(ts) = name
            .strip_prefix("crash-")
            .and_then(|rest| rest.split('-').next())
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        files.push((ts, name.to_string(), path));
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(MAX_TAKE);

    let mut out = Vec::with_capacity(files.len());
    for (ts, name, path) in files {
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let (summary, detail) = parse_crash(&raw);
        // 改名即「已提示」：同一份崩溃不会每次启动都弹；原文件保留，栈还能手工看
        let reported = path.with_extension("log.reported");
        if std::fs::rename(&path, &reported).is_err() {
            // 改名失败（文件被占用等）就不提示，避免下次启动重复弹同一条
            continue;
        }
        out.push(PendingCrash {
            id: name,
            ts: (ts as i64) * 1000,
            summary,
            detail,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crash_header() {
        let raw = "time: 2026-10-31T12:00:00.000+08:00\nversion: 0.1.0\nplatform: windows x86_64\nthread: main\nsummary: 测试失败（src/main.rs:1:1）\n\nbacktrace:\n   0: foo\n";
        let (summary, detail) = parse_crash(raw);
        assert_eq!(summary, "测试失败（src/main.rs:1:1）");
        assert!(detail.contains("版本 0.1.0"));
        assert!(detail.contains("线程 main"));
    }

    #[test]
    fn falls_back_to_first_line_when_header_broken() {
        let (summary, detail) = parse_crash("乱码文件\n第二行");
        // 第一行没有冒号：头部解析不出 key，退回第一行内容
        assert_eq!(summary, "乱码文件");
        assert!(detail.is_empty());
    }
}
