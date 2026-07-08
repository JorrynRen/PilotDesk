#![allow(dead_code)]
//! 虚拟控制台会话跟踪器
//!
//! 为前端虚拟控制台 UI 面板提供会话状态查询能力。
//! 记录每个终端会话的 PID、命令、工作目录、运行状态。

use std::collections::HashMap;
use std::time::Instant;

/// 虚拟控制台会话跟踪器
///
/// 为每个活跃终端会话维护状态信息，供前端虚拟控制台面板查询。
pub struct ConsoleBridge {
    /// 会话ID -> 终端会话状态
    sessions: HashMap<String, ConsoleSession>,
}

/// 终端会话状态
#[derive(Debug, Clone)]
pub struct ConsoleSession {
    /// 前端会话 ID
    pub session_id: String,
    /// 进程 PID
    pub pid: u32,
    /// 执行的命令
    pub command: String,
    /// 工作目录
    pub cwd: String,
    /// 会话状态
    pub status: SessionStatus,
    /// 启动时间
    pub started_at: Instant,
    /// 累计输出字节数
    pub output_bytes: u64,
}

/// 会话状态
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    /// 正在启动
    Starting,
    /// 已完成（正常退出）
    Completed,
    /// 已终止（用户主动停止）
    Terminated,
    /// 异常退出
    Failed,
}

impl ConsoleSession {
    pub fn new(
        session_id: String,
        pid: u32,
        command: String,
        cwd: String,
    ) -> Self {
        Self {
            session_id,
            pid,
            command,
            cwd,
            status: SessionStatus::Starting,
            started_at: Instant::now(),
            output_bytes: 0,
        }
    }
}

/// 可序列化的会话信息（供 Tauri 命令返回前端）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsoleSessionView {
    pub session_id: String,
    pub pid: u32,
    pub command: String,
    pub cwd: String,
    pub status: String,
    pub elapsed_ms: u64,
    pub output_bytes: u64,
}

impl From<&ConsoleSession> for ConsoleSessionView {
    fn from(s: &ConsoleSession) -> Self {
        Self {
            session_id: s.session_id.clone(),
            pid: s.pid,
            command: s.command.clone(),
            cwd: s.cwd.clone(),
            status: format!("{:?}", s.status),
            elapsed_ms: s.started_at.elapsed().as_millis() as u64,
            output_bytes: s.output_bytes,
        }
    }
}

impl ConsoleBridge {
    /// 创建新的跟踪器实例
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    // ── 会话生命周期 ──

    /// 注册新会话（在 spawn 成功后调用）
    pub fn register_session(
        &mut self,
        session_id: &str,
        pid: u32,
        command: &str,
        cwd: &str,
    ) {
        let session = ConsoleSession::new(
            session_id.to_string(),
            pid,
            command.to_string(),
            cwd.to_string(),
        );
        self.sessions.insert(session_id.to_string(), session);
        log::info!(
            "[ConsoleBridge] Session registered: '{}' pid={}",
            session_id, pid
        );
    }

    /// 标记会话完成（正常退出，exit_code == 0）
    pub fn mark_completed(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.status = SessionStatus::Completed;
            log::info!(
                "[ConsoleBridge] Session '{}' completed (elapsed={}ms)",
                session_id,
                session.started_at.elapsed().as_millis()
            );
        }
    }

    /// 标记会话失败（异常退出，exit_code != 0）
    pub fn mark_failed(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.status = SessionStatus::Failed;
            log::info!(
                "[ConsoleBridge] Session '{}' failed (elapsed={}ms)",
                session_id,
                session.started_at.elapsed().as_millis()
            );
        }
    }

    /// 标记会话被终止（用户主动停止）
    pub fn mark_terminated(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.status = SessionStatus::Terminated;
            log::info!(
                "[ConsoleBridge] Session '{}' terminated (elapsed={}ms)",
                session_id,
                session.started_at.elapsed().as_millis()
            );
        }
    }

    /// 注销会话
    pub fn unregister_session(&mut self, session_id: &str) {
        if self.sessions.remove(session_id).is_some() {
            log::info!("[ConsoleBridge] Session '{}' unregistered", session_id);
        }
    }

    // ── 查询接口 ──

    /// 获取所有会话的可序列化视图
    pub fn list_session_views(&self) -> Vec<ConsoleSessionView> {
        self.sessions.values().map(ConsoleSessionView::from).collect()
    }

    /// 获取指定会话的可序列化视图
    pub fn get_session_view(&self, session_id: &str) -> Option<ConsoleSessionView> {
        self.sessions.get(session_id).map(ConsoleSessionView::from)
    }
}

impl Drop for ConsoleBridge {
    fn drop(&mut self) {
        let count = self.sessions.len();
        if count > 0 {
            log::info!("[ConsoleBridge] Drop: {} sessions still tracked", count);
        }
    }
}
