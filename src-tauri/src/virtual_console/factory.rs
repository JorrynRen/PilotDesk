//! 虚拟控制台工厂模块
//!
//! 提供工厂模式创建跨平台虚拟控制台实例，
//! 根据 ConsoleType 自动选择对应平台实现。

use std::io;
use crate::virtual_console::traits::{VirtualConsole, AsyncConsole};
use crate::virtual_console::config::{ConsoleType, ConsoleConfig, detect_os_type};

#[cfg(target_os = "windows")]
use crate::virtual_console::windows::WindowsConpty;

#[cfg(target_os = "macos")]
use crate::virtual_console::macos::MacVirtualConsole;

#[cfg(target_os = "linux")]
use crate::virtual_console::linux::LinuxVirtualConsole;

/// 虚拟控制台工厂
///
/// 根据配置或自动检测的操作系统类型，创建对应平台的虚拟控制台实例。
pub struct ConsoleFactory;

impl ConsoleFactory {
    /// 自动检测操作系统并创建对应的虚拟控制台
    pub fn auto_create() -> io::Result<Box<dyn VirtualConsole>> {
        let os_type = detect_os_type();
        Self::create(os_type)
    }

    /// 自动检测操作系统并创建对应的虚拟控制台（带配置，当前仅做日志）
    #[allow(dead_code)]
    pub fn auto_create_with_config(_config: &ConsoleConfig) -> io::Result<Box<dyn VirtualConsole>> {
        let os_type = _config.console_type.unwrap_or_else(detect_os_type);
        Self::create(os_type)
    }

    /// 根据指定的控制台类型创建虚拟控制台
    pub fn create(console_type: ConsoleType) -> io::Result<Box<dyn VirtualConsole>> {
        match console_type {
            #[cfg(target_os = "windows")]
            ConsoleType::Windows => {
                let console = WindowsConpty::new()?;
                Ok(Box::new(console))
            }

            #[cfg(target_os = "macos")]
            ConsoleType::MacOS => {
                let console = MacVirtualConsole::new()?;
                Ok(Box::new(console))
            }

            #[cfg(target_os = "linux")]
            ConsoleType::Linux => {
                let console = LinuxVirtualConsole::new()?;
                Ok(Box::new(console))
            }

            #[cfg(not(target_os = "windows"))]
            ConsoleType::Windows => {
                Err(io::Error::new(io::ErrorKind::Unsupported, "Windows ConPTY not available on this platform"))
            }

            #[cfg(not(target_os = "macos"))]
            ConsoleType::MacOS => {
                Err(io::Error::new(io::ErrorKind::Unsupported, "macOS PTY not available on this platform"))
            }

            #[cfg(not(target_os = "linux"))]
            ConsoleType::Linux => {
                Err(io::Error::new(io::ErrorKind::Unsupported, "Linux PTY not available on this platform"))
            }
        }
    }

    /// 获取当前平台支持的控制系统列表
    #[allow(dead_code)]
    pub fn supported_types() -> Vec<ConsoleType> {
        let mut types = Vec::new();
        #[cfg(target_os = "windows")]
        types.push(ConsoleType::Windows);
        #[cfg(target_os = "macos")]
        types.push(ConsoleType::MacOS);
        #[cfg(target_os = "linux")]
        types.push(ConsoleType::Linux);
        types
    }

    /// 创建异步控制台（基于 tokio::process::Command）
    /// 供 Agent 会话使用
    pub async fn create_async() -> io::Result<Box<dyn AsyncConsole>> {
        Ok(Box::new(TokioConsole::new()))
    }
}

// ── 异步控制台实现（tokio::process::Command 封装） ──

struct TokioConsole {
    child: Option<tokio::process::Child>,
}

impl TokioConsole {
    fn new() -> Self {
        Self { child: None }
    }
}

#[async_trait::async_trait]
impl AsyncConsole for TokioConsole {
    async fn spawn(
        &mut self,
        command: &str,
        args: &[&str],
        cwd: &str,
    ) -> io::Result<(u32, tokio::sync::mpsc::Receiver<String>, tokio::sync::mpsc::Receiver<String>)> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env_remove("PYTHONHOME")
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);

        // 创建双通道用于 stdout/stderr 分离输出
        let (tx_out, rx_out) = tokio::sync::mpsc::channel::<String>(256);
        let (tx_err, rx_err) = tokio::sync::mpsc::channel::<String>(256);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Some(stdout) = stdout {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut reader = tokio::io::BufReader::new(stdout);
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = tx_out.send(String::from_utf8_lossy(&buf[..n]).to_string()).await;
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut reader = tokio::io::BufReader::new(stderr);
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = tx_err.send(String::from_utf8_lossy(&buf[..n]).to_string()).await;
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        self.child = Some(child);
        Ok((pid, rx_out, rx_err))
    }

    async fn wait(&mut self) -> io::Result<i32> {
        if let Some(mut child) = self.child.take() {
            let status = child.wait().await?;
            Ok(status.code().unwrap_or(-1))
        } else {
            Ok(-1)
        }
    }

    fn try_wait(&mut self) -> Option<i32> {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
                Ok(None) => None, // 仍在运行
                Err(_) => Some(-1),
            }
        } else {
            // child 已被 take（未 spawn 或已 wait/kill）
            None
        }
    }

    fn is_running(&mut self) -> bool {
        // try_wait() 返回 None → 仍在运行
        // try_wait() 返回 Some(_) → 已退出
        // child 为 None（未 spawn 或已 take）→ 不运行
        self.child.is_some() && self.try_wait().is_none()
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().and_then(|c| c.id()).unwrap_or(0)
    }

    async fn kill(&mut self) -> io::Result<()> {
        if let Some(ref mut child) = self.child {
            child.kill().await?;
        }
        Ok(())
    }
}
