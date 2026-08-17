//! 跨平台终端进程抽象：Windows=手写 ConPTY，Unix=portable-pty。
//!
//! 统一 `TerminalProcess` 接口与 `StdoutStream` 输出流，
//! 使 `TerminalManager` 无需感知平台差异。

#[cfg(target_os = "windows")]
use super::conpty_process::ConptyProcess;
#[cfg(not(target_os = "windows"))]
use super::unix_pty::UnixPtyProcess;

/// 终端进程（平台分发）
pub enum TerminalProcess {
    #[cfg(target_os = "windows")]
    Windows(ConptyProcess),
    #[cfg(not(target_os = "windows"))]
    Unix(UnixPtyProcess),
}

/// 终端输出流（attach 后按平台异步读取）
pub enum StdoutStream {
    #[cfg(target_os = "windows")]
    Windows(tokio::fs::File),
    #[cfg(not(target_os = "windows"))]
    Unix(Box<dyn std::io::Read + Send>),
}

impl TerminalProcess {
    /// 启动终端进程（平台分发），返回 (进程封装, 输出流)
    pub fn spawn(
        cmdline: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(Self, StdoutStream), String> {
        #[cfg(target_os = "windows")]
        {
            let (proc, stdout, _stderr, _mode) =
                super::conpty_process::spawn_with_conpty(cmdline, cwd, cols, rows)?;
            Ok((Self::Windows(proc), StdoutStream::Windows(stdout)))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let (proc, reader) = UnixPtyProcess::spawn(cmdline, cwd, cols, rows)?;
            Ok((Self::Unix(proc), StdoutStream::Unix(reader)))
        }
    }

    pub fn id(&self) -> u32 {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(p) => p.id(),
            #[cfg(not(target_os = "windows"))]
            Self::Unix(p) => p.id(),
        }
    }

    /// 写入用户输入
    pub fn write(&self, data: &str) -> Result<(), String> {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(p) => p.write(data),
            #[cfg(not(target_os = "windows"))]
            Self::Unix(p) => p.write(data),
        }
    }

    /// 调整终端尺寸
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(p) => p.resize(cols, rows),
            #[cfg(not(target_os = "windows"))]
            Self::Unix(p) => p.resize(cols, rows),
        }
    }

    /// 终止进程
    pub fn kill(&self) {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(p) => p.kill(),
            #[cfg(not(target_os = "windows"))]
            Self::Unix(p) => p.kill(),
        }
    }

    /// 关闭 stdout 读端以触发 EOF（Unix 无此概念，no-op）
    pub fn close_stdout_pipe(&self) {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(p) => p.close_stdout_pipe(),
            #[cfg(not(target_os = "windows"))]
            Self::Unix(_) => {}
        }
    }
}
