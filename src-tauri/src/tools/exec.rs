//! 进程执行原语（会话模式与群聊参与者共用）。
//!
//! - `run_command`：执行 Windows Shell 命令（cmd.exe /C），异步管道实时读 + 活性检测 + 32KB 截断
//! - `run_python`：执行 Python 内联代码或 .py 脚本（file/code 二选一）
//!
//! V3.5c 根治「本地工具卡死」三要素：
//! 1. **异步管道实时读**：spawn 后立即排空 stdout/stderr，杜绝「输出超管道缓冲(4KB) → 写端背压
//!    阻塞 → 进程永不退出」的伪卡死；同时也消除同步 `wait_with_output` 因孙进程持有写端句柄而
//!    无限阻塞的永久卡死（该同步阻塞连 tokio timeout 都无法打断）。
//! 2. **活性检测判真实卡死**（本地进程完全可观测，不依赖固定秒级盲杀）：进程存活 + 无新输出
//!    超过静默窗 + CPU 时间不再增长 → 判真实卡死（等输入/死循环/后台残留）→ 终止整棵进程树。
//!    有输出或有 CPU 增长即视为正常工作，只保留一个宽松总上限防长驻命令。
//! 3. **stdin 显式置空**：杜绝「交互命令等待输入」造成的静默挂起。
//!
//! 自 `api_agent/exec.rs`（run_python）与 `lib.rs`（execute_command 内联逻辑）合并迁移
//! （工具架构统一 v1.0，轮 4）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tokio::io::AsyncReadExt;

/// 静默观察窗：进程存活且无任何新输出持续超过该时长，才进入「疑似卡死」复核。
const STALL_WINDOW_SECS: u64 = 60;
/// CPU 复核窗：疑似卡死后再观察该时长内的 CPU 增量，确认进程确实停滞。
const CPU_PROBE_SECS: u64 = 6;
/// CPU 增量噪声阈值（100ns 单位）：复核窗内 CPU 增长 ≥200ms 视为仍在计算（非卡死）。
const CPU_NOISE_100NS: u64 = 2_000_000;
/// 宽松总执行上限：仅防长驻命令（dev server/watch 等）无限运行；不替代活性判定。
const TOTAL_LIMIT_SECS: u64 = 600;
/// 活性轮询间隔。
const POLL_MS: u64 = 500;

/// Python 环境可用性缓存（全局，首次检测后复用）。
static PY_AVAILABLE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

fn python_available() -> Result<bool, String> {
    let cache = PY_AVAILABLE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap();
    if let Some(v) = *guard {
        return Ok(v);
    }
    let mut cmd = std::process::Command::new("python");
    cmd.args(["--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let ok = cmd
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    *guard = Some(ok);
    Ok(ok)
}

/// 共享输出缓冲：两个管道读任务持续写入，主循环据此判定输出活性并取最终文本。
#[derive(Default)]
struct LiveBuf {
    /// 最近一次收到输出的时刻（None = 尚未收到任何输出）。
    last_activity: Option<Instant>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
type SharedBuf = Arc<Mutex<LiveBuf>>;

/// 管道排空任务：持续读 stdout/stderr 直到 EOF（防背压死锁 + 输出活性数据源）。
async fn drain<R: tokio::io::AsyncRead + Unpin>(mut reader: R, shared: SharedBuf, is_err: bool) {
    let mut chunk = [0u8; 8192];
    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut g = match shared.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        g.last_activity = Some(Instant::now());
        if is_err {
            g.stderr.extend_from_slice(&chunk[..n]);
        } else {
            g.stdout.extend_from_slice(&chunk[..n]);
        }
    }
}

/// 进程 CPU 累计时间（kernel+user，100ns 单位）。失败（非 Windows/权限/进程已退）返回 None。
fn process_cpu_100ns(pid: u32) -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut creation: FILETIME = std::mem::zeroed();
            let mut exit: FILETIME = std::mem::zeroed();
            let mut kernel: FILETIME = std::mem::zeroed();
            let mut user: FILETIME = std::mem::zeroed();
            let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            let k = ((kernel.dwHighDateTime as u64) << 32) | kernel.dwLowDateTime as u64;
            let u = ((user.dwHighDateTime as u64) << 32) | user.dwLowDateTime as u64;
            Some(k + u)
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 终止整棵进程树：Windows 用 taskkill /T /F（按父子关系递归杀孙进程，防后台残留
/// 继续持有管道句柄）；随后 kill + wait 收尸。
async fn kill_tree(pid: u32, child: &mut tokio::process::Child) {
    #[cfg(windows)]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn cleanup_tmp(target: Option<&PathBuf>) {
    if let Some(p) = target {
        let _ = std::fs::remove_file(p);
    }
}

/// 敏感环境变量关键字：子进程环境剔除键名（不区分大小写）包含任一关键字的变量，
/// 防止模型生成的命令/脚本通过 env 读到 API 密钥等凭据。
const SENSITIVE_ENV_KEYWORDS: [&str; 5] = ["KEY", "SECRET", "TOKEN", "PASSWORD", "CREDENTIAL"];

/// 收集并过滤当前环境变量：剔除键名含敏感关键字（KEY/SECRET/TOKEN/PASSWORD/CREDENTIAL，
/// 大小写不敏感）的项。SystemRoot/Path/PATHEXT/ComSpec 等 Windows 必需变量名不含上述
/// 关键字而正常保留，保证 cmd.exe / python 能正确加载与按 PATH 查找可执行文件。
fn scrubbed_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    std::env::vars_os()
        .filter(|(k, _)| {
            let upper = k.to_string_lossy().to_uppercase();
            !SENSITIVE_ENV_KEYWORDS.iter().any(|kw| upper.contains(kw))
        })
        .collect()
}

/// 卡死终止前的输出预览（≤1KB 尾部），供错误信息携带「已产出证据」给调用方。
fn tail_preview(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = crate::decode_windows_output(stdout);
    let err = crate::decode_windows_output(stderr);
    if !err.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&err);
    }
    let combined = combined.trim();
    if combined.is_empty() {
        return "（无任何输出）".to_string();
    }
    const TAIL: usize = 1000;
    if combined.chars().count() <= TAIL {
        return combined.to_string();
    }
    let cut: String = combined.chars().skip(combined.chars().count() - TAIL).collect();
    format!("…（以下为终止前输出尾部 {} 字符）\n{}", cut.chars().count(), cut)
}

/// 统一执行受管进程：异步管道实时读 + 活性检测 + 总执行上限 + 进程树终止。
/// `label` 仅用于日志/错误文案；`tmp_cleanup` 非空表示退出（含卡死终止）后删除该临时文件。
async fn run_managed(
    label: &str,
    program: &str,
    args: &[String],
    cwd: &str,
    tmp_cleanup: Option<PathBuf>,
) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null()) // 杜绝交互等待输入挂起
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true); // 未来被取消（如房间停止）时不残留主进程
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW，不弹出控制台窗口

    // 环境变量治理：子进程不继承敏感变量（scrubbed_env 已剔除含 KEY/SECRET/TOKEN/
    // PASSWORD/CREDENTIAL 键名项）。先 env_clear 再逐个重放过滤结果，确保无残留继承；
    // SystemRoot/Path 等系统必需变量不含上述关键字，正常随重放保留（cmd.exe 依赖）。
    let scrubbed = scrubbed_env();
    cmd.env_clear();
    for (k, v) in &scrubbed {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(|e| {
        cleanup_tmp(tmp_cleanup.as_ref());
        format!("启动{}失败: {}", label, e)
    })?;

    let shared: SharedBuf = Arc::new(Mutex::new(LiveBuf::default()));
    let mut drain_handles = Vec::new();
    if let Some(out) = child.stdout.take() {
        drain_handles.push(tokio::spawn(drain(out, shared.clone(), false)));
    }
    if let Some(err) = child.stderr.take() {
        drain_handles.push(tokio::spawn(drain(err, shared.clone(), true)));
    }
    let pid = child.id().unwrap_or(0);

    let stall = Duration::from_secs(STALL_WINDOW_SECS);
    let probe = Duration::from_secs(CPU_PROBE_SECS);
    let total = Duration::from_secs(TOTAL_LIMIT_SECS);
    let start = Instant::now();

    // 真实卡死复核状态：进入静默超窗的时刻 + 该时刻 CPU 基线。
    let mut stall_started: Option<Instant> = None;
    let mut stall_base_cpu: Option<u64> = None;
    let mut exit_code: Option<i32> = None;
    let mut kill_reason: Option<String> = None;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                break;
            }
            Ok(None) => {}
            Err(e) => {
                kill_tree(pid, &mut child).await;
                cleanup_tmp(tmp_cleanup.as_ref());
                return Err(format!("{}运行异常: {}", label, e));
            }
        }

        let elapsed = start.elapsed();
        let silence = shared
            .lock()
            .map(|g| g.last_activity.map(|t| t.elapsed()).unwrap_or(elapsed))
            .unwrap_or(elapsed);

        // 宽松总上限：仅防长驻命令无限运行（本地活性正常的长任务不在此列）。
        if elapsed > total {
            kill_reason = Some(format!(
                "{label}超过总执行上限（{total} 秒），已终止整棵进程树。\n原因分类：{desc}\n建议：{advice}",
                label = label,
                total = TOTAL_LIMIT_SECS,
                desc = "命令疑似需长期驻留或无限运行（如 dev server / watch / 阻塞式服务）。",
                advice = "请勿重跑同一条会驻留/阻塞的命令。若确实需要启动常驻服务，请改为后台启动方式（如 start /B、nohup 或拆分为独立任务），并用轮询/探测命令确认其就绪后继续后续步骤。",
            ));
            break;
        }

        // 活性判定：静默进入复核窗 → 复核窗内 CPU 无实质增长 → 判真实卡死。
        if silence > stall {
            if stall_started.is_none() {
                stall_started = Some(Instant::now());
                stall_base_cpu = process_cpu_100ns(pid);
            }
            match process_cpu_100ns(pid) {
                Some(cur) => {
                    if stall_base_cpu
                        .map(|base| cur.saturating_sub(base) >= CPU_NOISE_100NS)
                        .unwrap_or(false)
                    {
                        // CPU 仍在增长 → 在计算，非卡死：清除复核状态继续等。
                        stall_started = None;
                        stall_base_cpu = None;
                    }
                    // 无增长：保持复核，等 probe 窗满后判卡死。
                }
                // 无法探测 CPU（进程已退出/平台不支持）：不据此放行——进程是否已退出由
                // try_wait 判定；无法证明在计算时按复核窗到期处理（本工具仅 Windows 运行，
                // OpenProcess 正常总能探测到，此分支不影响常规判定）。
                None => {}
            }
            if let Some(st) = stall_started {
                if st.elapsed() >= probe {
                    kill_reason = Some(format!(
                        "{label}疑似真实卡死：无任何输出且进程停滞超过 {secs} 秒，CPU 无增长。已终止整棵进程树。\n原因分类：{desc}\n建议：{advice}",
                        label = label,
                        secs = STALL_WINDOW_SECS + CPU_PROBE_SECS,
                        desc = "命令无输出且无计算，最可能是等待交互输入（stdin 已被置空会立即 EOF，交互式提示会直接退出或报错）、死循环、或后台子进程挂起未退出。",
                        advice = "不要盲目重跑同一条命令。请先检查终止前输出与命令副作用（是否已生成部分文件/已执行一半的改动），再：(1) 若是需要确认/选择的命令，改用其非交互参数（如 -y/--yes/--batch/--force，或直接把参数内联）重试；(2) 若是长任务/死循环风险，先拆分命令或改用脚本分步执行；(3) 命令已产生部分成果时，基于已有产出续接完成目标，不要整条从头重做。",
                    ));
                    break;
                }
            }
        } else {
            stall_started = None;
            stall_base_cpu = None;
        }

        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }

    if let Some(reason) = kill_reason {
        let (stdout, stderr) = {
            let g = shared.lock().map(|g| (g.stdout.clone(), g.stderr.clone())).unwrap_or_default();
            g
        };
        let preview = tail_preview(&stdout, &stderr);
        kill_tree(pid, &mut child).await;
        cleanup_tmp(tmp_cleanup.as_ref());
        return Err(format!("{}\n已保留终止前输出：{}", reason, preview));
    }

    // 正常退出收尾：reap 进程后短暂等待读任务写尽管道缓冲。
    // 进程已退出 → 写端关闭 → 读任务自然 EOF；限时 200ms，防后台孙进程仍持有写端
    // 句柄把读任务无限拖住（此时仅截取已读到的输出，不再阻塞主流程）。
    let _ = child.wait().await;
    if !drain_handles.is_empty() {
        let _ = tokio::time::timeout(Duration::from_millis(200), async {
            for h in drain_handles {
                let _ = h.await;
            }
        })
        .await;
    }
    let (stdout, stderr) = {
        let g = shared.lock().map(|g| (g.stdout.clone(), g.stderr.clone())).unwrap_or_default();
        g
    };
    cleanup_tmp(tmp_cleanup.as_ref());

    let stdout_text = crate::decode_windows_output(&stdout).trim().to_string();
    let stderr_text = crate::decode_windows_output(&stderr).trim().to_string();

    // ── 组装结果 ──
    let mut result = String::new();
    if !stdout_text.is_empty() {
        result.push_str(&stdout_text);
    }
    if !stderr_text.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(&stderr_text);
    }

    // ── 智能截断：head + tail 策略（保留开头和结尾，丢弃中间冗余）──
    // 32KB 上限足够覆盖绝大部分命令输出，超出时保留前 16KB + 后 16KB
    const MAX_OUTPUT: usize = 32_768; // 32KB
    if result.len() > MAX_OUTPUT {
        let head_size = MAX_OUTPUT / 2;
        let tail_size = MAX_OUTPUT - head_size;
        // 安全找到字符边界，避免在多字节字符中间截断导致 panic
        let head_end = result
            .char_indices()
            .take_while(|&(i, _)| i < head_size)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        let tail_start = result
            .char_indices()
            .filter(|&(i, _)| i >= result.len() - tail_size)
            .next()
            .map_or(result.len(), |(i, _)| i);
        let head = &result[..head_end];
        let tail = &result[tail_start..];
        result = format!(
            "{}\n\n... (中间 {} 字节已省略，总输出 {} 字节) ...\n\n{}",
            head,
            result.len() - head_end - (result.len() - tail_start),
            result.len(),
            tail
        );
    }

    let code = exit_code.unwrap_or(-1);
    if result.is_empty() {
        Ok(format!("命令执行完成（退出码: {}），无输出", code))
    } else {
        Ok(format!("退出码: {}\n{}", code, result))
    }
}

/// 执行 Windows Shell 命令（cmd.exe /C），返回 stdout/stderr 与退出码。
/// 异步管道实时读 + 活性检测 + 32KB head/tail 截断（详见模块注释）。
pub async fn run_command(cwd: &str, command: &str) -> Result<String, String> {
    run_managed(
        "命令",
        "cmd.exe",
        &["/C".to_string(), command.to_string()],
        cwd,
        None,
    )
    .await
}

/// 执行 Python：`code` 与 `file` 二选一（file 优先），返回 stdout/stderr 与退出码。
/// 与 `run_command` 同一套异步管道 + 活性检测机制。
pub async fn run_python(cwd: &str, code: Option<&str>, file: Option<&str>) -> Result<String, String> {
    if !python_available()? {
        return Err(
            "当前系统未检测到 Python 环境。请安装 Python 后重试。\n\
             安装时请勾选 \"Add Python to PATH\"。\n\
             版本号请根据实际需求选择（建议使用当前最新稳定版）。"
                .to_string(),
        );
    }

    // ── 确定脚本源：优先 file（脚本文件），否则 code（内联代码写临时文件）──
    let (target, tmp_cleanup): (PathBuf, Option<PathBuf>) = match (file, code) {
        (Some(f), _) if !f.trim().is_empty() => {
            let p = PathBuf::from(f.trim());
            let abs = if p.is_absolute() {
                p
            } else {
                std::path::Path::new(cwd).join(&p)
            };
            if !abs.exists() {
                return Err(format!("Python 脚本文件不存在: {}", abs.display()));
            }
            (abs, None)
        }
        (_, Some(c)) if !c.trim().is_empty() => {
            let tmp_dir = std::env::temp_dir().join("pilotdesk_py");
            let _ = std::fs::create_dir_all(&tmp_dir);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let tmp_file = tmp_dir.join(format!("script_{}.py", ts));
            std::fs::write(&tmp_file, c).map_err(|e| format!("写入临时脚本失败: {}", e))?;
            let cleanup = Some(tmp_file.clone());
            (tmp_file, cleanup)
        }
        _ => {
            return Err(
                "execute_python 缺少参数：请提供 file（脚本路径）或 code（内联代码）".to_string(),
            )
        }
    };

    let target_str = target.to_string_lossy().into_owned();
    run_managed("Python", "python", &[target_str], cwd, tmp_cleanup).await
}
