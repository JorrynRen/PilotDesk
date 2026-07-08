#![allow(dead_code)]
// ── traits.rs — 虚拟控制台统一接口 ──

pub use std::io::Result;
pub use std::process::ExitStatus;
pub use std::time::Duration;

/// 进程状态
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessStatus {
    /// 正在运行
    Running,
    /// 已正常退出
    Exited(i32),
    /// 被信号终止
    Terminated(i32),
    /// 未知状态
    Unknown,
}

/// 虚拟控制台统一接口（同步）
///
/// 提供跨平台的进程生命周期管理和 IO 操作能力。
/// 实现基于 std::process::Command + piped stdio。
pub trait VirtualConsole: Send + Sync {
    // ── 生命周期 ──

    /// 启动进程，返回控制台句柄（含 stdin/stdout/stderr 独立 IO）
    fn spawn(&mut self, command: &str, args: &[&str], cwd: &str) -> Result<ConsoleHandle>;

    /// 等待进程结束，返回退出状态
    fn wait(&mut self, timeout: Option<Duration>) -> Result<ExitStatus>;

    /// 非阻塞尝试等待进程结束。
    ///
    /// 返回 `Some(exit_code)` 表示进程已退出，
    /// `None` 表示仍在运行。
    /// 基于底层 `child.try_wait()` 实现，比布尔标志更准确。
    fn try_wait(&mut self) -> Option<i32>;

    /// 发送终止信号（优雅终止，进程可捕获处理）
    fn terminate(&mut self) -> Result<()>;

    /// 强制终止进程（不可恢复）
    fn kill(&mut self) -> Result<()>;

    /// 关闭控制台资源（wait child + 释放 handle）
    fn close(&mut self) -> Result<()>;

    // ── 状态查询 ──

    /// 获取进程 PID
    fn pid(&self) -> u32;

    /// 检查进程是否仍在运行（快速预检）。
    ///
    /// 基于内部状态标志，非精确检测。
    /// 需要精确判断时请使用 `try_wait()`。
    fn is_running(&self) -> bool;

    /// 获取进程状态枚举
    fn status(&self) -> ProcessStatus;

    // ── IO（已通过 ConsoleHandle 独立操作） ──

    /// 向控制台输入数据（已弃用，请通过 ConsoleHandle.stdin 操作）
    #[deprecated(note = "请通过 ConsoleHandle.stdin 写入")]
    fn write(&mut self, data: &[u8]) -> Result<()>;

    /// 读取控制台输出（已弃用，请通过 ConsoleHandle.stdout 读取）
    #[deprecated(note = "请通过 ConsoleHandle.stdout/stderr 读取")]
    fn read(&mut self) -> Result<Vec<u8>>;

    /// 读取一行输出（已弃用，请通过 ConsoleHandle.stdout.read_line 读取）
    #[deprecated(note = "请通过 ConsoleHandle.stdout.read_line 读取")]
    fn read_line(&mut self) -> Result<String>;
}

/// 控制台句柄
///
/// 包含 stdin/stdout/stderr 的独立 IO 接口，
/// 调用方通过这些接口进行数据读写，与 VirtualConsole 的进程管理解耦。
pub struct ConsoleHandle {
    pub pid: u32,
    pub stdin: Box<dyn ConsoleWriter>,
    pub stdout: Box<dyn ConsoleReader>,
    pub stderr: Box<dyn ConsoleReader>,
    pub is_conpty: bool,
}

/// 输入接口
pub trait ConsoleWriter: Send + Sync {
    fn write(&mut self, data: &[u8]) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}

/// 输出接口
pub trait ConsoleReader: Send + Sync {
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize>;
    fn read_line(&mut self) -> Result<String>;
    fn has_data(&self) -> bool;
    fn close(&mut self) -> Result<()>;
}

/// 异步控制台接口 — 基于 tokio::process::Command
///
/// 提供 async 版本的进程生命周期管理和 IO 操作能力。
/// 实现基于 tokio::process::Command + tokio channel。
#[async_trait::async_trait]
pub trait AsyncConsole: Send {
    // ── 生命周期 ──

    /// 异步启动进程，返回 (pid, stdout_rx, stderr_rx)
    async fn spawn(
        &mut self,
        command: &str,
        args: &[&str],
        cwd: &str,
    ) -> Result<(u32, tokio::sync::mpsc::Receiver<String>, tokio::sync::mpsc::Receiver<String>)>;

    /// 等待进程结束，返回退出码
    async fn wait(&mut self) -> Result<i32>;

    /// 非阻塞尝试等待进程结束。
    ///
    /// 返回 `Some(exit_code)` 表示进程已退出，
    /// `None` 表示仍在运行。
    /// 基于 `tokio::process::Child::try_wait()` 实现。
    fn try_wait(&mut self) -> Option<i32>;

    /// 强制终止进程
    async fn kill(&mut self) -> Result<()>;

    // ── 状态查询 ──

    /// 检查进程是否仍在运行。
    ///
    /// 基于 `child.try_wait()` 实现：
    /// - `try_wait()` 返回 `None` → 仍在运行 → true
    /// - `try_wait()` 返回 `Some(...)` → 已退出 → false
    /// - child 已被 take（未 spawn 或已 wait）→ false
    fn is_running(&mut self) -> bool;

    /// 获取进程 PID
    fn pid(&self) -> u32;
}
