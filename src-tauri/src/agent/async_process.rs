//! 异步进程控制台（Agent 会话执行专用）
//!
//! 自已删除的 virtual_console 模块迁移而来，仅保留异步路径（TokioConsole + AsyncConsole）。
//! `AsyncConsole` 接口签名保持不变，作为未来跨平台升级（如 portable-pty 替换）的替换契约。

use std::io;

/// 异步进程接口 — 基于 tokio::process::Command
///
/// 提供 async 版本的进程生命周期管理和 IO 操作能力。
/// 实现基于 tokio::process::Command + tokio channel。
#[async_trait::async_trait]
pub trait AsyncConsole: Send {
    /// 异步启动进程，返回 (pid, stdout_rx, stderr_rx)
    async fn spawn(
        &mut self,
        command: &str,
        args: &[&str],
        cwd: &str,
    ) -> io::Result<(
        u32,
        tokio::sync::mpsc::Receiver<String>,
        tokio::sync::mpsc::Receiver<String>,
    )>;

    /// 等待进程结束，返回退出码
    async fn wait(&mut self) -> io::Result<i32>;

    /// 非阻塞尝试等待进程结束。
    ///
    /// 返回 `Some(exit_code)` 表示进程已退出，
    /// `None` 表示仍在运行。
    /// 基于 `tokio::process::Child::try_wait()` 实现。
    fn try_wait(&mut self) -> Option<i32>;

    /// 强制终止进程
    async fn kill(&mut self) -> io::Result<()>;

    /// 检查进程是否仍在运行。
    ///
    /// 基于 `child.try_wait()` 实现：
    /// - `try_wait()` 返回 `None` → 仍在运行 → true
    /// - `try_wait()` 返回 `Some(...)` → 已退出 → false
    /// - child 已被 take（未 spawn 或已 wait）→ false
    ///
    /// 当前 execute_async 未使用（状态判断走 try_wait），保留作为跨平台替换契约的一部分。
    #[allow(dead_code)]
    fn is_running(&mut self) -> bool;

    /// 获取进程 PID
    ///
    /// 当前 execute_async 未使用（PID 由 spawn 返回值提供），保留作为跨平台替换契约的一部分。
    #[allow(dead_code)]
    fn pid(&self) -> u32;
}

/// 基于 tokio::process::Command 的异步控制台实现。
///
/// stdout/stderr 通过双通道逐行推送；进程退出后管道 EOF 会关闭对应 channel。
pub struct TokioConsole {
    child: Option<tokio::process::Child>,
}

impl TokioConsole {
    pub fn new() -> Self {
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
    ) -> io::Result<(
        u32,
        tokio::sync::mpsc::Receiver<String>,
        tokio::sync::mpsc::Receiver<String>,
    )> {
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
                use tokio::io::AsyncBufReadExt;
                let mut reader = tokio::io::BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    match reader.read_line(&mut line).await {
                        Ok(0) => break,
                        Ok(_) => {
                            if !line.is_empty() {
                                let _ = tx_out.send(line.clone()).await;
                                line.clear();
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut reader = tokio::io::BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    match reader.read_line(&mut line).await {
                        Ok(0) => break,
                        Ok(_) => {
                            if !line.is_empty() {
                                let _ = tx_err.send(line.clone()).await;
                                line.clear();
                            }
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
