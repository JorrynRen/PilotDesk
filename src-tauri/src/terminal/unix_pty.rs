//! Unix 终端实现：基于 portable-pty（openpty / forkpty）。
//!
//! 仅编译于非 Windows 平台（文件顶部 cfg）。提供与 Windows ConPTY
//! 等价的进程生命周期接口，供 `pty::TerminalProcess` 统一分发。

#![cfg(not(target_os = "windows"))]

use std::io::{Read, Write};
use std::sync::Mutex;

use portable_pty::{CommandBuilder, MasterPty, PtySize};

/// Unix PTY 进程封装（portable-pty）
pub struct UnixPtyProcess {
    /// master 端（用于 resize；writer/reader 已取出）
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// 子进程（kill 用）
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// 写入端（用户输入 → PTY）
    writer: Mutex<Box<dyn Write + Send>>,
    pid: u32,
}

impl UnixPtyProcess {
    /// 启动 shell 进程并返回 (进程封装, stdout 读流)。
    ///
    /// `cmdline` 为 "程序 参数..." 形式（如 "bash" / "sh -lc"）；
    /// 程序与参数按空白拆分。
    pub fn spawn(
        cmdline: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(Self, Box<dyn Read + Send>), String> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("openpty 失败: {}", e))?;

        let mut parts = cmdline.split_whitespace();
        let program = parts.next().unwrap_or("sh").to_string();
        let mut cmd = CommandBuilder::new(program);
        for arg in parts {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("spawn shell 失败: {}", e))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("take_writer 失败: {}", e))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("try_clone_reader 失败: {}", e))?;
        let pid = child.process_id().unwrap_or(0);

        Ok((
            Self {
                master: Mutex::new(pair.master),
                child,
                writer: Mutex::new(writer),
                pid,
            },
            reader,
        ))
    }

    pub fn id(&self) -> u32 {
        self.pid
    }

    /// 写入用户输入
    pub fn write(&self, data: &str) -> Result<(), String> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| "writer 锁获取失败".to_string())?;
        w.write_all(data.as_bytes())
            .map_err(|e| format!("写入终端失败: {}", e))?;
        w.flush().map_err(|e| format!("flush 失败: {}", e))
    }

    /// 调整 PTY 尺寸
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let m = self
            .master
            .lock()
            .map_err(|_| "master 锁获取失败".to_string())?;
        m.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("resize 失败: {}", e))
    }

    /// 终止子进程
    ///
    /// portable-pty 的 `Child::kill` 需要 `&mut self`（内部要可变地操作子进程句柄）。
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}
