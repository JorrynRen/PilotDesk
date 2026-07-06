//! Linux 平台虚拟控制台实现
//!
//! 基于 std::process::Command + piped stdio 的真实终端实现。
//! 正确持有 Child 引用，提供真实的 stdin/stdout/stderr 读写能力。

use super::traits::*;
use std::io::{self, Read, Write as StdWrite};
use std::process::{Child, Command, ExitStatus, Stdio};
#[cfg(unix)] use std::os::unix::process::ExitStatusExt;
use std::time::Duration;

/// Linux 虚拟控制台实现
pub struct LinuxVirtualConsole {
    child: Option<Child>,
    pid: u32,
    running: bool,
}

impl LinuxVirtualConsole {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            child: None,
            pid: 0,
            running: false,
        })
    }
}

impl VirtualConsole for LinuxVirtualConsole {
    fn spawn(&mut self, command: &str, args: &[&str], cwd: &str) -> io::Result<ConsoleHandle> {
        let mut cmd = Command::new(command);
        if !cwd.is_empty() {
            cmd.current_dir(cwd);
        }
        cmd.args(args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.env_remove("PYTHONHOME");

        let mut child = cmd.spawn()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let pid = child.id();
        let stdin_pipe = child.stdin.take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "无法获取 stdin"))?;
        let stdout_pipe = child.stdout.take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "无法获取 stdout"))?;
        let stderr_pipe = child.stderr.take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "无法获取 stderr"))?;

        self.child = Some(child);
        self.pid = pid;
        self.running = true;

        let handle = ConsoleHandle {
            pid,
            stdin: Box::new(StdinWriter::new(stdin_pipe)),
            stdout: Box::new(StdoutReader::new(stdout_pipe)),
            stderr: Box::new(StderrReader::new(stderr_pipe)),
            is_conpty: false,
        };

        Ok(handle)
    }

    fn write(&mut self, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "请通过 ConsoleHandle.stdin 写入"))
    }

    fn read(&mut self) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "请通过 ConsoleHandle.stdout/stderr 读取"))
    }

    fn read_line(&mut self) -> io::Result<String> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "请通过 ConsoleHandle.stdout.read_line 读取"))
    }

    fn terminate(&mut self) -> io::Result<()> {
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
        }
        self.running = false;
        Ok(())
    }

    fn kill(&mut self) -> io::Result<()> {
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
        }
        self.running = false;
        Ok(())
    }

    fn wait(&mut self, _timeout: Option<Duration>) -> io::Result<ExitStatus> {
        if let Some(ref mut child) = self.child {
            let status = child.wait()?;
            self.running = false;
            Ok(status)
        } else {
            Ok(ExitStatus::from_raw(0))
        }
    }

    fn pid(&self) -> u32 { self.pid }
    fn is_running(&self) -> bool { self.running }
    fn status(&self) -> ProcessStatus {
        if self.running { ProcessStatus::Running } else { ProcessStatus::Unknown }
    }

    fn close(&mut self) -> io::Result<()> {
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
        self.running = false;
        Ok(())
    }
}

/// 基于 ChildStdin 的独立写入器
pub struct StdinWriter {
    inner: Option<std::process::ChildStdin>,
}

impl StdinWriter {
    pub fn new(inner: std::process::ChildStdin) -> Self {
        Self { inner: Some(inner) }
    }
}

impl ConsoleWriter for StdinWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(ref mut inner) = self.inner {
            inner.write_all(data)?;
            inner.flush()?;
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "stdin 已关闭"))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(ref mut inner) = self.inner {
            inner.flush()
        } else {
            Ok(())
        }
    }

    fn close(&mut self) -> io::Result<()> {
        self.inner.take();
        Ok(())
    }
}

/// 基于 ChildStdout 的独立读取器
pub struct StdoutReader {
    inner: Option<std::process::ChildStdout>,
}

impl StdoutReader {
    pub fn new(inner: std::process::ChildStdout) -> Self {
        Self { inner: Some(inner) }
    }
}

impl ConsoleReader for StdoutReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(ref mut inner) = self.inner {
            inner.read(buffer)
        } else {
            Ok(0)
        }
    }

    fn read_line(&mut self) -> io::Result<String> {
        if let Some(ref mut inner) = self.inner {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(inner);
            let mut buf = String::new();
            reader.read_line(&mut buf)?;
            Ok(buf)
        } else {
            Ok(String::new())
        }
    }

    fn has_data(&self) -> bool {
        self.inner.is_some()
    }

    fn close(&mut self) -> io::Result<()> {
        self.inner.take();
        Ok(())
    }
}

/// 基于 ChildStderr 的独立读取器
pub struct StderrReader {
    inner: Option<std::process::ChildStderr>,
}

impl StderrReader {
    pub fn new(inner: std::process::ChildStderr) -> Self {
        Self { inner: Some(inner) }
    }
}

impl ConsoleReader for StderrReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(ref mut inner) = self.inner {
            inner.read(buffer)
        } else {
            Ok(0)
        }
    }

    fn read_line(&mut self) -> io::Result<String> {
        if let Some(ref mut inner) = self.inner {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(inner);
            let mut buf = String::new();
            reader.read_line(&mut buf)?;
            Ok(buf)
        } else {
            Ok(String::new())
        }
    }

    fn has_data(&self) -> bool {
        self.inner.is_some()
    }

    fn close(&mut self) -> io::Result<()> {
        self.inner.take();
        Ok(())
    }
}
