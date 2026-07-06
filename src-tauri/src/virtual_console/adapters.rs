//! 虚拟控制台适配器模块
//!
//! 提供高层封装，将 VirtualConsole trait 适配到具体的业务场景。

use std::collections::HashMap;
use std::io;

use crate::virtual_console::traits::*;
use crate::virtual_console::config::ConsoleConfig;
use crate::virtual_console::events::{EventSystem, LogEventListener, StatsEventListener};
use crate::virtual_console::factory::ConsoleFactory;

/// Agent 控制台适配器
pub struct AgentConsoleAdapter {
    console: Box<dyn VirtualConsole>,
    session_id: Option<String>,
    output_buffer: String,
    error_buffer: String,
    config: ConsoleConfig,
    metrics: ConsoleMetrics,
    event_system: EventSystem,
}

/// 控制台指标
#[derive(Debug, Clone)]
pub struct ConsoleMetrics {
    pub start_time: std::time::Instant,
    pub output_bytes: u64,
    pub error_bytes: u64,
    pub lines_processed: usize,
    pub session_id_extracted: bool,
}

impl AgentConsoleAdapter {
    pub fn new(console: Box<dyn VirtualConsole>, config: ConsoleConfig) -> io::Result<Self> {
        let event_system = EventSystem::new();
        Ok(Self {
            console,
            session_id: None,
            output_buffer: String::new(),
            error_buffer: String::new(),
            config,
            metrics: ConsoleMetrics {
                start_time: std::time::Instant::now(),
                output_bytes: 0,
                error_bytes: 0,
                lines_processed: 0,
                session_id_extracted: false,
            },
            event_system,
        })
    }

    pub fn spawn_agent(&mut self, command: &str, args: &[&str], cwd: &str) -> io::Result<u32> {
        let handle = self.console.spawn(command, args, cwd)?;
        let _ = self.event_system.add_listener(Box::new(LogEventListener::new()));
        let _ = self.event_system.add_listener(Box::new(StatsEventListener::new()));
        Ok(handle.pid)
    }

    pub fn terminate(&mut self) -> io::Result<()> { self.console.terminate() }
    pub fn kill(&mut self) -> io::Result<()> { self.console.kill() }

    pub fn wait(&mut self) -> io::Result<i32> {
        let status = self.console.wait(Some(std::time::Duration::from_millis(self.config.timeout_ms)))?;
        Ok(status.code().unwrap_or(-1))
    }

    pub fn get_full_output(&self) -> (String, String) {
        (self.output_buffer.clone(), self.error_buffer.clone())
    }

    pub fn get_session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn get_metrics(&self) -> &ConsoleMetrics {
        &self.metrics
    }
}

/// 简单的虚拟控制台适配器
pub struct SimpleConsoleAdapter {
    console: Box<dyn VirtualConsole>,
    output_buffer: String,
    error_buffer: String,
}

impl SimpleConsoleAdapter {
    pub fn new(console: Box<dyn VirtualConsole>) -> Self {
        Self {
            console,
            output_buffer: String::new(),
            error_buffer: String::new(),
        }
    }

    pub fn execute_command(&mut self, command: &str, args: &[&str], cwd: &str) -> io::Result<(String, String)> {
        let _handle = self.console.spawn(command, args, cwd)?;
        let _exit_code = self.console.wait(None)?;
        Ok((self.output_buffer.clone(), self.error_buffer.clone()))
    }

    pub fn send_input(&mut self, input: &str) -> io::Result<()> {
        self.console.write(input.as_bytes())
    }
}

/// 会话管理适配器
pub struct SessionManagerAdapter {
    consoles: HashMap<String, Box<dyn VirtualConsole>>,
    active_sessions: HashMap<String, SessionInfo>,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub console_type: String,
    pub start_time: std::time::Instant,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
    Terminated,
}

impl SessionManagerAdapter {
    pub fn new() -> Self {
        Self {
            consoles: HashMap::new(),
            active_sessions: HashMap::new(),
        }
    }

    pub fn create_session(&mut self, session_id: &str, command: &str, args: &[&str], cwd: &str) -> io::Result<()> {
        let mut console = ConsoleFactory::auto_create()?;
        let _handle = console.spawn(command, args, cwd)?;

        let session_info = SessionInfo {
            session_id: session_id.to_string(),
            console_type: "auto".to_string(),
            start_time: std::time::Instant::now(),
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: cwd.to_string(),
            status: SessionStatus::Running,
        };

        self.consoles.insert(session_id.to_string(), console);
        self.active_sessions.insert(session_id.to_string(), session_info);
        Ok(())
    }

    pub fn get_session(&self, session_id: &str) -> Option<&SessionInfo> {
        self.active_sessions.get(session_id)
    }

    pub fn list_sessions(&self) -> Vec<&SessionInfo> {
        self.active_sessions.values().collect()
    }

    pub fn terminate_session(&mut self, session_id: &str) -> io::Result<()> {
        if let Some(console) = self.consoles.get_mut(session_id) {
            console.terminate()?;
            if let Some(session_info) = self.active_sessions.get_mut(session_id) {
                session_info.status = SessionStatus::Terminated;
            }
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "Session not found"))
        }
    }

    pub fn remove_session(&mut self, session_id: &str) -> io::Result<()> {
        if self.consoles.remove(session_id).is_some() {
            self.active_sessions.remove(session_id);
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "Console not found"))
        }
    }

    pub fn get_session_count(&self) -> usize {
        self.active_sessions.len()
    }

    pub fn get_running_sessions(&self) -> Vec<&SessionInfo> {
        self.active_sessions
            .values()
            .filter(|session| session.status == SessionStatus::Running)
            .collect()
    }
}
