//! Agent 虚拟控制台桥接模块
//!
//! 将虚拟控制台能力集成到 Agent 模块中。
//! 作为 AgentManager 内部的会话跟踪器，记录每个 Agent 会话的
//! 终端状态（PID、命令、工作目录、运行状态），提供统一的查询接口。
//!
//! 设计原则：不替代已有的进程管理路径（tokio::process::Command / ConPTY），
//! 而是在其之上提供统一的会话视图，供前端虚拟控制台面板查询和操作。

use std::collections::HashMap;
use std::time::Instant;

/// Agent 虚拟控制台桥接
///
/// 为 AgentManager 内每个活跃会话维护终端状态信息。
/// 与 AgentManager.processes 并行运行，互不干扰。
pub struct AgentConsoleBridge {
    /// 会话ID -> 终端会话状态
    sessions: HashMap<String, ConsoleSession>,
}

/// 终端会话状态
#[derive(Debug, Clone)]
pub struct ConsoleSession {
    /// 前端会话 ID
    pub session_id: String,
    /// Agent 类型（claud / hermes / codex 等）
    pub agent_type: String,
    /// 进程 PID
    pub pid: u32,
    /// 执行的命令
    pub command: String,
    /// 命令参数
    pub args: Vec<String>,
    /// 工作目录
    pub cwd: String,
    /// 是否为 ConPTY 模式
    pub conpty_mode: bool,
    /// 命令类型
    pub command_kind: CommandKind,
    /// 会话状态
    pub status: SessionStatus,
    /// 启动时间
    pub started_at: Instant,
    /// 累计输出字节数
    pub output_bytes: u64,
    /// 累计错误输出字节数
    pub error_bytes: u64,
}

/// 会话状态
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    /// 正在启动
    Starting,
    /// 运行中
    Running,
    /// 已完成（正常退出）
    Completed,
    /// 已终止（用户主动停止）
    Terminated,
    /// 异常退出
    Failed,
}

/// 命令类型（区分不同的 Agent 操作场景）
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum CommandKind {
    /// 会话交互（send_message）
    Session,
    /// 单次执行（execute_once）
    ExecuteOnce,
    /// 版本查询
    VersionCheck,
    /// 安装
    Install,
    /// 卸载
    Uninstall,
    /// 更新检查
    UpdateCheck,
    /// 其他命令
    Other,
}

impl ConsoleSession {
    pub fn new(
        session_id: String,
        agent_type: String,
        pid: u32,
        command: String,
        args: Vec<String>,
        cwd: String,
        conpty_mode: bool,
        command_kind: CommandKind,
    ) -> Self {
        Self {
            session_id,
            agent_type,
            pid,
            command,
            args,
            cwd,
            conpty_mode,
            command_kind,
            status: SessionStatus::Starting,
            started_at: Instant::now(),
            output_bytes: 0,
            error_bytes: 0,
        }
    }
}

/// 可序列化的会话信息（供 Tauri 命令返回前端）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsoleSessionView {
    pub session_id: String,
    pub agent_type: String,
    pub pid: u32,
    pub command: String,
    pub cwd: String,
    pub conpty_mode: bool,
    pub command_kind: String,
    pub status: String,
    pub elapsed_ms: u64,
    pub output_bytes: u64,
    pub error_bytes: u64,
}

impl From<&ConsoleSession> for ConsoleSessionView {
    fn from(s: &ConsoleSession) -> Self {
        Self {
            session_id: s.session_id.clone(),
            agent_type: s.agent_type.clone(),
            pid: s.pid,
            command: s.command.clone(),
            cwd: s.cwd.clone(),
            conpty_mode: s.conpty_mode,
            command_kind: format!("{:?}", s.command_kind),
            status: format!("{:?}", s.status),
            elapsed_ms: s.started_at.elapsed().as_millis() as u64,
            output_bytes: s.output_bytes,
            error_bytes: s.error_bytes,
        }
    }
}

impl AgentConsoleBridge {
    /// 创建新的桥接实例
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    // ── 会话生命周期 ──

    /// 注册新会话（在 AgentManager spawn 成功后调用）
    /// 注册新会话（在 AgentManager spawn 成功后调用）
    pub fn register_session(
        &mut self,
        session_id: &str,
        agent_type: &str,
        pid: u32,
        command: &str,
        args: Vec<String>,
        cwd: &str,
        conpty_mode: bool,
        command_kind: CommandKind,
    ) {
        let session = ConsoleSession::new(
            session_id.to_string(),
            agent_type.to_string(),
            pid,
            command.to_string(),
            args,
            cwd.to_string(),
            conpty_mode,
            command_kind,
        );
        self.sessions.insert(session_id.to_string(), session);
        log::info!(
            "[ConsoleBridge] Session registered: '{}' agent={} pid={} conpty={}",
            session_id, agent_type, pid, conpty_mode
        );
    }

    /// 标记会话为运行中（IO 循环开始读取输出）
    pub fn mark_running(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.status = SessionStatus::Running;
        }
    }

    /// 更新会话的输出统计
    pub fn update_output_stats(&mut self, session_id: &str, output_bytes: u64, error_bytes: u64) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.output_bytes = output_bytes;
            session.error_bytes = error_bytes;
        }
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

    /// 注销会话（在 close_session / stop_generation 后调用）
    pub fn unregister_session(&mut self, session_id: &str) {
        if self.sessions.remove(session_id).is_some() {
            log::info!("[ConsoleBridge] Session '{}' unregistered", session_id);
        }
    }

    // ── 查询接口 ──

    /// 获取指定会话的可序列化视图
    pub fn get_session_view(&self, session_id: &str) -> Option<ConsoleSessionView> {
        self.sessions.get(session_id).map(ConsoleSessionView::from)
    }

    /// 获取所有活跃会话的可序列化视图
    pub fn list_session_views(&self) -> Vec<ConsoleSessionView> {
        self.sessions.values().map(ConsoleSessionView::from).collect()
    }

    /// 获取活跃会话数量
    pub fn active_count(&self) -> usize {
        self.sessions.len()
    }

    /// 检查指定会话是否存在
    pub fn has_session(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }

    /// 获取指定会话的 PID
    pub fn get_pid(&self, session_id: &str) -> Option<u32> {
        self.sessions.get(session_id).map(|s| s.pid)
    }
}

impl Default for AgentConsoleBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AgentConsoleBridge {
    fn drop(&mut self) {
        let count = self.sessions.len();
        if count > 0 {
            log::info!("[ConsoleBridge] Drop: {} sessions still tracked", count);
        }
    }
}
