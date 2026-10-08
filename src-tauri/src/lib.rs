mod agent;
mod api_agent;
mod commands;
mod db;
mod eventlog;
mod groupchat;
mod plugin;
mod terminal;
mod tools;
mod utils;
mod workflow;

use crate::db::models::Attachment;
use agent::AgentManager;
use api_agent::agent_loop::{AgentLoop, AgentLoopConfig, RiskLevel, SecurityMode};
use api_agent::client::ApiClient;
use api_agent::compaction::{conversation_stats, CompactionPolicy, DefaultCompactionPolicy};
use api_agent::context::{infer_context_window, SlidingWindow, DEFAULT_CONTEXT_TOKENS};
use api_agent::db::MemoryStore;
use api_agent::skills::SkillLoader;
use api_agent::summarize::{generate_rolling_summary, split_recent_window};
use api_agent::system_prompt::{GitContext, SystemPromptBuilder};
use api_agent::types::*;
use db::init::{init_db, DbPool};
use rusqlite::params;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;
use tokio::sync::Mutex as AsyncMutex;
use workflow::executor::NodeExecutor;
use workflow::executors::agent_executor::{PendingToolApproval, WorkflowApprovalTarget};
use workflow::scheduler::WorkflowScheduler;

/// Windows 控制台输出编码解码：先尝试 UTF-8，失败则用系统 ANSI/OEM 代码页（如 CP 936/GBK）解码。
/// cmd.exe 在中文 Windows 上默认以 GBK 输出，直接 from_utf8_lossy 会导致乱码。
pub fn decode_windows_output(bytes: &[u8]) -> String {
    // 快速路径：已经是合法 UTF-8，直接返回
    if let Ok(s) = String::from_utf8(bytes.to_vec()) {
        return s;
    }
    // 回退：用系统 ANSI / OEM 代码页解码（中文 Windows 均为 936/GBK）。
    // 注意：不能用 GetConsoleOutputCP()——Tauri 是 GUI 应用，没有关联控制台，该函数返回 0。
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::{GetACP, GetOEMCP, MultiByteToWideChar};
        for cp in [unsafe { GetACP() }, unsafe { GetOEMCP() }] {
            if cp == 0 || cp == 65001 {
                continue; // 0 表示未知；65001 是 UTF-8，前面已尝试
            }
            let len = unsafe {
                MultiByteToWideChar(
                    cp,
                    0,
                    bytes.as_ptr(),
                    bytes.len() as i32,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if len > 0 {
                let mut buf: Vec<u16> = vec![0; len as usize];
                unsafe {
                    let written = MultiByteToWideChar(
                        cp,
                        0,
                        bytes.as_ptr(),
                        bytes.len() as i32,
                        buf.as_mut_ptr(),
                        len,
                    );
                    if written > 0 {
                        if let Ok(s) = String::from_utf16(&buf[..written as usize]) {
                            return s;
                        }
                    }
                }
            }
        }
    }
    // 所有解码均失败，用 lossy 兜底
    String::from_utf8_lossy(bytes).into_owned()
}

/// 判断字节内容是否为二进制（含 NUL 字节，或存在非法 UTF-8 序列）。
/// 供 read_file / 附件等文本工具拒绝二进制文件，避免把图片等当文本返回乱码。
pub(crate) fn is_binary_bytes(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return true;
    }
    let sample = &bytes[..bytes.len().min(8192)];
    let mut invalid = 0usize;
    let mut rest = sample;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(_) => break,
            Err(e) => {
                let valid = e.valid_up_to();
                let err_len = e.error_len().unwrap_or(1);
                invalid += err_len;
                rest = &rest[valid + err_len..];
            }
        }
    }
    invalid > 0
}

pub struct DbState {
    pub pool: DbPool,
}

/// 资源路径管理
pub struct ResourcePaths {
    /// 内置资源目录（打包携带的 Agent 配置、默认图标等，只读）
    pub builtin: std::path::PathBuf,
    /// 用户资源目录（自定义图标、用户上传文件等，可读写）
    pub user: std::path::PathBuf,
}

/// 递归复制目录（目标目录会按需创建）
fn copy_dir_all(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// 内置插件 / 技能的首启播种：`<安装包>/resources/<sub>/<名字>` → `<用户目录>/<名字>`。
///
/// **只在目标不存在时复制**。理由是"播种"与"安装"是两件事：
///   - 用户可能已经装过、改过、甚至**故意删掉**某个内置项；
///   - 每次启动都覆盖不仅会冲掉用户的修改，还会让"卸载内置项"永远无法生效。
/// 复制失败只记日志、不让启动失败 —— 少一个内置项不该导致整个应用起不来。
fn seed_builtin_dir(builtin: &std::path::Path, sub: &str, dest_root: &std::path::Path) {
    // 开发模式 / 未打包时没有内置资源目录，属正常情况
    let src_root = builtin.join("resources").join(sub);
    if !src_root.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(&src_root) {
        Ok(it) => it,
        Err(e) => {
            log::warn!("读取内置目录失败 {}: {}", src_root.display(), e);
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(dest_root) {
        log::warn!("创建用户目录失败 {}: {}", dest_root.display(), e);
        return;
    }

    let mut copied = 0usize;
    for entry in entries.flatten() {
        let src = entry.path();
        if !src.is_dir() {
            continue;
        }
        let dest = dest_root.join(entry.file_name());
        if dest.exists() {
            continue;
        }
        match copy_dir_all(&src, &dest) {
            Ok(()) => copied += 1,
            Err(e) => log::warn!(
                "播种内置项失败 {} → {}: {}",
                src.display(),
                dest.display(),
                e
            ),
        }
    }
    if copied > 0 {
        log::info!(
            "已播种 {} 个内置{}到 {}",
            copied,
            if sub == "plugins" { "插件" } else { "技能" },
            dest_root.display()
        );
    }
}

impl DbState {
    pub fn get_conn(
        &self,
    ) -> Result<
        r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
        crate::utils::errors::AppError,
    > {
        self.pool
            .get()
            .map_err(|e| crate::utils::errors::AppError::Lock(format!("数据库连接获取失败: {}", e)))
    }
}

// ── 数据库命令 ──

#[tauri::command]
fn list_tags(
    state: tauri::State<'_, DbState>,
) -> Result<Vec<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::list_tags(&conn)
}

#[tauri::command]
fn list_api_providers(
    state: tauri::State<'_, DbState>,
) -> Result<Vec<commands::api_provider::ApiProvider>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::list_api_providers(&conn)
}

#[tauri::command]
fn get_inspiration(
    state: tauri::State<'_, DbState>,
    id: String,
) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::get_inspiration(&conn, id)
}

#[tauri::command]
fn create_inspiration(
    state: tauri::State<'_, DbState>,
    payload: commands::inspiration::CreateInspirationPayload,
) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::create_inspiration(&conn, payload)
}

#[tauri::command]
fn update_inspiration(
    state: tauri::State<'_, DbState>,
    payload: commands::inspiration::UpdateInspirationPayload,
) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::update_inspiration(&conn, payload)
}

#[tauri::command]
fn delete_inspiration(
    state: tauri::State<'_, DbState>,
    id: String,
) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::delete_inspiration(&conn, id)
}

#[tauri::command]
fn list_inspirations(
    state: tauri::State<'_, DbState>,
    tag: Option<String>,
    favorite_only: Option<bool>,
) -> Result<Vec<commands::inspiration::Inspiration>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::list_inspirations(&conn, tag, favorite_only.unwrap_or(false))
}

#[tauri::command]
fn search_inspirations(
    state: tauri::State<'_, DbState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<commands::inspiration::Inspiration>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::search_inspirations(&conn, query, limit.unwrap_or(50))
}

#[tauri::command]
fn get_api_provider(
    state: tauri::State<'_, DbState>,
    id: String,
) -> Result<Option<commands::api_provider::ApiProvider>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::get_api_provider(&conn, &id)
}

#[tauri::command]
fn upsert_api_provider(
    state: tauri::State<'_, DbState>,
    payload: commands::api_provider::CreateOrUpdateProvider,
) -> Result<commands::api_provider::ApiProvider, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::upsert_api_provider(&conn, &payload)
}

#[tauri::command]
fn delete_api_provider(
    state: tauri::State<'_, DbState>,
    id: String,
) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::delete_api_provider(&conn, &id)
}

#[tauri::command]
fn get_api_key(
    state: tauri::State<'_, DbState>,
    id: String,
) -> Result<Option<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::get_api_key(&conn, &id)
}

#[tauri::command]
fn reorder_api_providers(
    state: tauri::State<'_, DbState>,
    ids: Vec<String>,
) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::reorder_api_providers(&conn, &ids)
}

#[tauri::command]
fn get_app_setting(
    state: tauri::State<'_, DbState>,
    key: String,
) -> Result<Option<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::app_settings::get_setting(&conn, &key)
}

#[tauri::command]
fn set_app_setting(
    state: tauri::State<'_, DbState>,
    key: String,
    value: String,
) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::app_settings::set_setting(&conn, &key, &value)
}

#[tauri::command]
fn get_usage_summary(
    state: tauri::State<'_, DbState>,
    days: Option<i64>,
) -> Result<commands::usage::UsageSummary, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::usage::usage_summary(&conn, days)
}

#[tauri::command]
fn get_usage_attribution(
    state: tauri::State<'_, DbState>,
    days: Option<i64>,
) -> Result<commands::usage::UsageAttribution, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::usage::usage_attribution(&conn, days)
}

#[tauri::command]
fn get_session_usage(
    state: tauri::State<'_, DbState>,
    session_id: String,
) -> Result<commands::usage::UsageTotals, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::usage::session_usage(&conn, &session_id)
}

#[tauri::command]
fn get_room_usage(
    state: tauri::State<'_, DbState>,
    room_id: String,
) -> Result<commands::usage::RoomUsage, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::usage::room_usage(&conn, &room_id)
}

#[tauri::command]
fn get_theme(state: tauri::State<'_, DbState>) -> Result<String, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::theme::get_theme(&conn)
}

#[tauri::command]
fn set_theme_cmd(
    state: tauri::State<'_, DbState>,
    theme: String,
) -> Result<String, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::theme::set_theme(&conn, theme)
}

// ── Agent 命令 ──

/// 使用 AgentConfig 元信息驱动的 send_message
/// API Agent（agent_type == "api"）使用 AgentLoop 编排，
/// CLI Agent（claude/hermes/codex）使用子进程交互。
#[tauri::command]
async fn agent_send_message_with_config(
    app: tauri::AppHandle,
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    state: tauri::State<'_, DbState>,
    session_id: String,
    agent_type: String,
    message: String,
    mode: String,
    cwd: Option<String>,
    system_prompt: Option<String>,
    agent_session_id: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    attachments: Option<Vec<Attachment>>,
    // 会话安全模式（strict/standard/relaxed/unrestricted，缺省标准；本消息有效）
    security_mode: Option<String>,
) -> Result<(), String> {
    // ── API Agent 路径：使用 AgentLoop ──
    if agent_type == "api" {
        log::info!(
            "[API Agent] 收到消息: session={}, msg_len={}, attachments={}, model_prompt={}",
            session_id,
            message.len(),
            attachments.as_ref().map_or(0, |v| v.len()),
            system_prompt.as_deref().unwrap_or("(none)").len()
        );
        let app_clone = app.clone();
        let pending = app
            .try_state::<PendingApprovals>()
            .ok_or("PendingApprovals 状态未初始化")?
            .inner()
            .clone();
        let attachments = attachments.unwrap_or_default();
        return run_api_agent(
            app_clone,
            &state,
            &session_id,
            &message,
            &attachments,
            &system_prompt.unwrap_or_default(),
            pending,
            temperature,
            max_tokens,
            security_mode,
        )
        .await;
    }

    // ── CLI Agent 路径（原有逻辑）──
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    let config = commands::agents::get_agent_inner(&conn, &agent_type)
        .map_err(|e| format!("查询 Agent 配置失败: {}", e))?
        .ok_or_else(|| format!("未知 Agent 类型: {}", agent_type))?;
    // cwd 为空时统一使用全局工作区路径
    let resolved_cwd = if cwd.as_deref().map_or(true, |s| s.is_empty()) {
        Some(
            crate::utils::paths::resolve_workspace_path(None, "", &conn)
                .to_string_lossy()
                .to_string(),
        )
    } else {
        cwd
    };
    // CLI 会话同样登记运行态：两种模式的列表脉冲与重入判断口径一致。
    let sid_for_registry = session_id.clone();
    let run_token = crate::api_agent::session_runs::begin(&sid_for_registry);
    let mut mgr = agent_mgr.lock().await;
    let result = mgr
        .send_message_with_config(
            app,
            session_id,
            config,
            message,
            mode,
            resolved_cwd,
            system_prompt,
            agent_session_id,
        )
        .await;
    crate::api_agent::session_runs::finish(&sid_for_registry, &run_token);
    result
}

#[tauri::command]
async fn agent_stop_generation(
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    session_id: String,
) -> Result<(), String> {
    // API 会话：置协作式取消标志（Agent Loop 在迭代边界与流式读取期间检查）。
    // CLI 会话：走 AgentManager 的进程表终止整棵进程树。
    // 两条路径互不干扰，可同时执行——会话属于哪种模式由调用方决定，这里无需区分。
    let api_run_cancelled = crate::api_agent::session_runs::cancel(&session_id);
    log::info!(
        "[Agent] 停止生成: session={}, API 运行取消={}",
        session_id,
        api_run_cancelled
    );
    let mut mgr = agent_mgr.lock().await;
    mgr.stop_generation(&session_id);
    Ok(())
}

/// 当前有后台运行任务的会话 id 列表（前端列表脉冲与重入时对齐运行态）。
#[tauri::command]
fn agent_running_sessions() -> Vec<String> {
    crate::api_agent::session_runs::running_sessions()
}

/// 工具调用审批（前端弹窗后回调）
#[tauri::command]
async fn agent_approve_tool(
    pending: tauri::State<'_, PendingApprovals>,
    session_id: String,
    call_id: String,
    approved: bool,
) -> Result<(), String> {
    if pending.approve(&call_id, approved) {
        log::info!(
            "[Approval] session={}, call={}, approved={}",
            session_id,
            call_id,
            approved
        );
    } else {
        log::warn!("[Approval] 未找到审批请求: call={}", call_id);
    }
    Ok(())
}

/// 迭代上限确认（前端弹窗后回调）
#[tauri::command]
async fn agent_continue_loop(
    pending: tauri::State<'_, PendingApprovals>,
    session_id: String,
    should_continue: bool,
) -> Result<(), String> {
    if pending.respond_continue(&session_id, should_continue) {
        log::info!(
            "[ContinueLoop] session={}, should_continue={}",
            session_id,
            should_continue
        );
    } else {
        log::warn!("[ContinueLoop] 未找到继续请求: session={}", session_id);
    }
    Ok(())
}

/// ask_user 确认回复（前端确认块提交后回调；content 为格式化后的用户回复文本）
#[tauri::command]
async fn agent_respond_confirmation(
    pending: tauri::State<'_, PendingApprovals>,
    session_id: String,
    call_id: String,
    content: String,
) -> Result<(), String> {
    if pending.respond_confirmation(&call_id, content) {
        log::info!(
            "[AskUser] session={}, call={} 已收到用户回复",
            session_id,
            call_id
        );
    } else {
        log::warn!(
            "[AskUser] 未找到确认请求（可能已超时）: session={}, call={}",
            session_id,
            call_id
        );
    }
    Ok(())
}

/// 待审批工具调用管理（使用 tokio::sync::oneshot 避免阻塞工作线程）
#[derive(Clone)]
pub struct PendingApprovals {
    approvals: Arc<
        std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    /// 迭代上限确认请求（session_id → sender）
    continue_reqs: Arc<
        std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    /// ask_user 工具确认请求（call_id → 用户回复文本 sender）
    confirmation_reqs: Arc<
        std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<String>>>,
    >,
}

impl PendingApprovals {
    pub fn new() -> Self {
        Self {
            approvals: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            continue_reqs: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            confirmation_reqs: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// 注册 ask_user 工具确认请求（返回 receiver 供工具阻塞等待用户回复）。
    pub fn register_confirmation(&self, call_id: String) -> tokio::sync::oneshot::Receiver<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.confirmation_reqs.lock().unwrap().insert(call_id, tx);
        rx
    }

    /// 前端回传 ask_user 确认回复（唤醒等待中的工具调用）。
    pub fn respond_confirmation(&self, call_id: &str, reply: String) -> bool {
        if let Some(tx) = self.confirmation_reqs.lock().unwrap().remove(call_id) {
            let _ = tx.send(reply);
            true
        } else {
            false
        }
    }

    pub fn register(&self, call_id: String) -> tokio::sync::oneshot::Receiver<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.approvals.lock().unwrap().insert(call_id, tx);
        rx
    }

    pub fn approve(&self, call_id: &str, approved: bool) -> bool {
        if let Some(tx) = self.approvals.lock().unwrap().remove(call_id) {
            let _ = tx.send(approved);
            true
        } else {
            false
        }
    }

    /// 注册迭代上限确认请求（返回 receiver 用于阻塞等待）
    pub fn register_continue(&self, session_id: &str) -> tokio::sync::oneshot::Receiver<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.continue_reqs
            .lock()
            .unwrap()
            .insert(session_id.to_string(), tx);
        rx
    }

    /// 前端响应迭代上限确认请求
    pub fn respond_continue(&self, session_id: &str, should_continue: bool) -> bool {
        if let Some(tx) = self.continue_reqs.lock().unwrap().remove(session_id) {
            let _ = tx.send(should_continue);
            true
        } else {
            false
        }
    }

    /// 丢弃审批超时后残留的 sender（approvals map 静默移除，避免超时条目泄漏；
    /// 之后前端再响应会按"未找到"处理，与其它已超时请求语义一致）。
    pub fn discard(&self, call_id: &str) {
        self.approvals.lock().unwrap().remove(call_id);
    }

    /// 丢弃迭代上限确认超时后残留的 sender（continue_reqs map 静默移除）。
    pub fn discard_continue(&self, session_id: &str) {
        self.continue_reqs.lock().unwrap().remove(session_id);
    }

    /// 丢弃 ask_user 确认等待超时后残留的 sender（confirmation_reqs map 静默移除；
    /// 与 respond_confirmation 的 remove 路径互补，覆盖超时不再等待回复的场景）。
    pub fn discard_confirmation(&self, call_id: &str) {
        self.confirmation_reqs.lock().unwrap().remove(call_id);
    }
}

#[tauri::command]
async fn agent_create_session(
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    session_id: String,
    agent_type: String,
    cwd: Option<String>,
) -> Result<(), String> {
    let mut mgr = agent_mgr.lock().await;
    mgr.create_session(&session_id, &agent_type, cwd.as_deref());
    Ok(())
}

#[tauri::command]
async fn agent_close_session(
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    session_id: String,
) -> Result<(), String> {
    let mut mgr = agent_mgr.lock().await;
    mgr.close_session(&session_id);
    Ok(())
}

/// 获取终端会话桥接的会话列表
#[tauri::command]
async fn list_console_sessions(
    bridge: tauri::State<'_, std::sync::Mutex<crate::terminal::console_bridge::ConsoleBridge>>,
) -> Result<Vec<crate::terminal::console_bridge::ConsoleSessionView>, String> {
    let sessions = bridge.lock().unwrap().list_session_views();
    Ok(sessions)
}

#[tauri::command]
async fn agent_list_skills(
    state: tauri::State<'_, DbState>,
    agent_type: String,
) -> Result<Vec<crate::db::models::SkillInfo>, String> {
    // API Agent 的技能目录独立于 DB 配置（~/.pilotdesk/skills/），使用 SkillLoader 扫描。
    // 列表要带落盘路径（编辑主文件 / 卸载都靠它定位），路径从加载器已记录目录反查。
    if agent_type == "api" {
        let loader = SkillLoader::new(get_api_agent_skills_dir());
        let skills = loader
            .list_skills()
            .into_iter()
            .map(|e| {
                let info = crate::db::models::SkillInfo::new(&e.name, &e.description, "");
                match loader.skill_dir(&e.name) {
                    Some(dir) => {
                        let entry = dir.join(crate::agent::list_skills::DEFAULT_ENTRY_FILE);
                        info.with_paths(&dir, &entry)
                    }
                    None => info,
                }
            })
            .collect();
        return Ok(skills);
    }

    let config = state
        .get_conn()
        .ok()
        .and_then(|conn| commands::agents::get_agent_inner(&conn, &agent_type).ok()?);
    Ok(agent::AgentManager::list_skills(&agent_type, config.as_ref()).await)
}

// ════════════════════════════════════════════════════════════
// API Agent 执行（AgentLoop 编排）
// ════════════════════════════════════════════════════════════

/// 获取 API Agent 技能目录路径
/// 返回 ~/.pilotdesk/skills/ 如果存在
pub(crate) fn get_api_agent_skills_dir() -> Option<String> {
    let dir = crate::api_agent::system_prompt::get_pilotdesk_config_dir()
        .map(|d| format!("{}/skills", d))?;

    if std::path::Path::new(&dir).is_dir() {
        log::info!("[API Agent] 技能目录: {}", dir);
        Some(dir)
    } else {
        log::debug!("[API Agent] 技能目录不存在: {}", dir);
        None
    }
}

/// 将附件拆分为图片 base64 列表与文件引用说明。
/// - `kind == "image"`：读取落盘文件转 `data:<mime>;base64,...`，送入多模态输入；
///   同时记录落盘路径，供图生图工具（generate_image / edit_image）引用；
/// - 其余：仅生成路径说明，供模型通过 read_file 按需读取。
fn split_attachments(attachments: &[Attachment]) -> (Vec<String>, String) {
    use base64::Engine;

    let mut images = Vec::new();
    let mut file_lines = Vec::new();
    let mut image_lines = Vec::new();

    for att in attachments {
        if att.kind == "image" {
            match std::fs::read(&att.path) {
                Ok(bytes) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    let mime = if att.mime.is_empty() {
                        "image/png"
                    } else {
                        att.mime.as_str()
                    };
                    images.push(format!("data:{};base64,{}", mime, b64));
                }
                Err(e) => {
                    log::warn!("[API Agent] 读取图片附件失败 {}: {}", att.path, e);
                }
            }
            image_lines.push(format!("- {} (路径: {})", att.name, att.path));
        } else {
            file_lines.push(format!("- {} (路径: {})", att.name, att.path));
        }
    }

    let mut notes: Vec<String> = Vec::new();
    if !file_lines.is_empty() {
        notes.push(format!(
            "用户附加了以下文件，如需了解其内容请调用 read_file 读取对应路径：\n{}",
            file_lines.join("\n")
        ));
    }
    if !image_lines.is_empty() {
        notes.push(format!(
            "用户附加了以下图片，如需图生图可调用 generate_image（image 数组）或 edit_image，其图片参数可填这些路径：\n{}",
            image_lines.join("\n")
        ));
    }

    let note = if notes.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", notes.join("\n"))
    };

    (images, note)
}

/// 当前时间毫秒（用于给自动检索记录生成唯一 toolId）。
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 自动记忆检索（意图路由）的人类可读结果摘要，作为 memory_intent 调用记录的返回值。
/// 故障类状态附上具体原因，使前端可见归因、无需翻日志。
///
/// 命中列表带**来源**（`【知识库·小说创作】黄金三章` / `【记忆·preference】用户偏好`）：
/// 只说"命中 3 条"的话，用户不知道命中的是自己存的事实还是某份资料里的内容。
fn kv_retrieval_summary(trace: &crate::api_agent::memory_intent::RetrievalTrace) -> String {
    let n = trace.matched.len();
    let keys = if n == 0 {
        "（无）".to_string()
    } else {
        trace
            .matched
            .iter()
            .map(|h| format!("【{}】{}", h.source, h.key))
            .collect::<Vec<_>>()
            .join("、")
    };
    let base = match trace.status {
        "matched" => {
            if trace.injected {
                // 命中数含知识库条目（两条链路合并计数），所以措辞不能只说"记忆" ——
                // 详情里会逐个列出命中的 key
                format!("命中 {} 条记忆 / 知识并注入上下文：{}", n, keys)
            } else {
                "意图路由完成检索：0 命中，本轮不注入记忆".to_string()
            }
        }
        "no_keywords" => {
            "意图路由未返回任何关键词，本轮未检索、不注入记忆（消息可能没有可检索的主题）"
                .to_string()
        }
        // 路由故障（含熔断冷却）时一律不注入：避免把与当前话题无关的高频记忆塞进上下文。
        "route_breaker_open" => "意图路由处于熔断冷却期，本轮不注入记忆".to_string(),
        "route_unavailable" => "意图路由不可用，本轮不注入记忆".to_string(),
        "route_unparsed" => "意图路由响应解析失败，本轮不注入记忆".to_string(),
        "empty_input" => "用户消息为空，未发起意图路由，本轮不注入记忆".to_string(),
        other => {
            if trace.injected {
                format!("记忆注入状态 {}：注入 {} 条：{}", other, n, keys)
            } else {
                format!("记忆注入状态 {}：未注入", other)
            }
        }
    };

    let mut out = base;
    if !trace.reason.is_empty() {
        out.push_str(&format!("。原因：{}", trace.reason));
    }
    if !memory_intent_succeeded(trace.status) {
        out.push_str("；需要用户偏好或事实时模型可调用 search_memory。");
    }
    out
}

/// 意图路由流程是否正常完成（决定自动检索记录显示成功还是失败）：
/// 完成解析（命中 / 未返回关键词）与空输入都算完成；路由不可用、解析失败、熔断冷却算失败。
/// "未返回关键词"不算错误 —— 没有可检索的主题本就是正常结果，只是没有记忆可注入。
fn memory_intent_succeeded(status: &str) -> bool {
    matches!(status, "matched" | "no_keywords" | "empty_input")
}

/// 自动记忆检索记录的一行标题（思维链折叠行文案）。
/// 与 status 一一对应，作为系统步骤展示，不使用"调用 xxx"式工具语义。
fn memory_retrieval_title(trace: &crate::api_agent::memory_intent::RetrievalTrace) -> String {
    let label = match trace.status {
        "matched" => {
            if trace.injected {
                format!("命中 {} 条记忆 / 知识并注入", trace.matched.len())
            } else {
                "已检索，无命中".to_string()
            }
        }
        "no_keywords" => "未返回关键词，本轮不检索".to_string(),
        "route_breaker_open" => "熔断冷却中，本轮跳过".to_string(),
        "route_unavailable" => "检索不可用，本轮跳过".to_string(),
        "route_unparsed" => "响应解析失败，本轮跳过".to_string(),
        "empty_input" => "无输入，本轮跳过".to_string(),
        other => format!("状态 {}", other),
    };
    format!("自动记忆检索 · {}", label)
}

/// 自动记忆检索系统步骤的参数行（展开明细时显示）。
/// 只呈现"路由器实际用了什么模型、解析出什么"——命中与注入属于结论，放在 detail 里；
/// 两者分开才能一眼分辨"模型没解析出关键词"与"关键词没命中记忆"。
fn memory_retrieval_params(trace: &crate::api_agent::memory_intent::RetrievalTrace) -> String {
    let join = |v: &[String]| {
        if v.is_empty() {
            "（空）".to_string()
        } else {
            v.join("、")
        }
    };
    format!(
        "模型={}, 分类={}, 关键词={}",
        if trace.model.is_empty() {
            "（未调用）"
        } else {
            trace.model.as_str()
        },
        join(&trace.categories),
        join(&trace.keywords)
    )
}

/// 从消息持久化的思维链（ThinkingChainStep[] JSON）中提取 reasoning 步骤内容，
/// 供思考模式模型（DeepSeek 等）随下一轮 assistant 消息原样回传。
fn extract_reasoning_from_tool_calls(tool_calls: &Option<String>) -> String {
    let Some(json) = tool_calls else {
        return String::new();
    };
    let Ok(steps) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return String::new();
    };
    steps
        .iter()
        .filter(|s| s["type"].as_str() == Some("reasoning"))
        .filter_map(|s| s["content"].as_str().map(String::from))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 使用 AgentLoop 执行 API Agent 对话（会话模式入口）。
///
/// 会话模式是 fire-and-forget：前端不等待本命令返回（消息由前端 `save_message` 落库，
/// 输出经 `agent-*` 事件回传），因此执行体交给后台任务、命令立即返回 `Ok(())`。
/// 执行期失败由 `run_api_agent_inner` 发 `agent-error` 事件，与前端 invoke 失败的
/// `onError` 通路一致；命令返回值不再承载执行期错误。
async fn run_api_agent(
    app: tauri::AppHandle,
    state: &DbState,
    session_id: &str,
    message: &str,
    attachments: &[Attachment],
    system_prompt: &str,
    pending_approvals: PendingApprovals,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    security_mode: Option<String>,
) -> Result<(), String> {
    let pool = state.pool.clone();
    let session_id = session_id.to_string();
    let message = message.to_string();
    let attachments = attachments.to_vec();
    let system_prompt = system_prompt.to_string();

    tokio::spawn(async move {
        // persist_turn=false：会话模式的消息由前端 `save_message` 落库，后端不重复写。
        if let Err(e) = run_api_agent_inner(
            &pool,
            &app,
            &session_id,
            &message,
            &attachments,
            &system_prompt,
            pending_approvals,
            temperature,
            max_tokens,
            security_mode,
            false,
            // 会话模式：审批由用户在内联审批卡上决定（不是工作流节点，无需工作流审批上下文）
            None,
            // 会话模式：工作目录/授权边界沿用会话行自己的 cwd
            None,
            // 交互式聊天不做模型覆盖：始终按会话行里的 厂商/模型
            None,
            None,
        )
        .await
        {
            log::error!("[API Agent] 执行失败: session={}, error={}", session_id, e);
        }
    });

    Ok(())
}

/// 运行登记守卫：`run_api_agent_inner` 的 future 被丢弃（工作流取消时 engine 的
/// `tokio::select!` 会丢弃执行器 future）时也要注销运行记录，否则"运行中"永久为真、
/// 列表脉冲不灭，且该会话的"停止生成"会被误判为可取消。
struct ApiRunGuard {
    session_id: String,
    token: Arc<AtomicBool>,
}

impl Drop for ApiRunGuard {
    fn drop(&mut self) {
        crate::api_agent::session_runs::finish(&self.session_id, &self.token);
    }
}

/// 审批方身份标签（工作流工具审批）：命中审批的调用由用户在工作流页/通知中心裁决，
/// 拒绝文案据此说明是用户决定的。
const WORKFLOW_APPROVAL_LABEL: &str = "用户";

/// 工作流工具审批的等待上限：超时按拒绝处理（无人值守时放行等于绕过授权策略）。
const WORKFLOW_APPROVAL_TIMEOUT_SECS: u64 = 30 * 60;

/// 把一次工作流工具审批写进执行事件流（`approval/requested` / `approval/resolved`）。
///
/// 事件流是可回放的证据：进程重启后等待登记表必然为空，未闭环的请求据此标为失效记录，
/// 让用户看到"这次执行卡在哪次审批上"而不是只看到实例停在「执行中」。
fn record_tool_approval_event(
    pool: &DbPool,
    execution_id: &str,
    kind: &str,
    approval: &PendingToolApproval,
    decision: Option<bool>,
) {
    let Ok(conn) = pool.get() else { return };
    let mut payload = serde_json::json!({
        "executionId": approval.execution_id,
        "nodeId": approval.node_id,
        "nodeLabel": approval.node_label,
        "callId": approval.call_id,
        "toolName": approval.tool_name,
        "arguments": approval.arguments,
        "risk": approval.risk,
    });
    if let Some(approved) = decision {
        payload["approved"] = serde_json::json!(approved);
    }
    if let Err(e) =
        crate::eventlog::append_workflow_event(&conn, execution_id, kind, &payload, false)
    {
        log::warn!(
            "[Approval] 写入审批事件失败: kind={}, call={}, err={}",
            kind,
            approval.call_id,
            e
        );
    }
}

/// 执行一次 API Agent 对话（会话模式与工作流 Agent 节点共用），返回最终助手文本。
///
/// `persist_turn` 决定本轮用户提示由谁落库：工作流 Agent 节点没有前端调用方，置 true
/// 由后端补写用户消息；会话模式置 false（前端 `save_message` 已写，后端再写会重复）。
/// 助手消息两种模式都由前端 `agent-done` 事件总线落库，后端不写。
///
/// `workflow_approval` 决定审批走哪条路：`None` 为会话模式，向前端发内联审批卡等用户点；
/// `Some(target)` 为工作流 Agent 节点（命中节点「工具授权」策略的调用），审批注册进工作流的
/// 等待登记表，由工作流页/通知中心/编辑器裁决，超时按拒绝处理。
///
/// 任何失败都在此发一次 `agent-error`（含配置等启动期失败），使会话 UI 的失败提示
/// 与流式事件走同一条通路，无需调用方各自发事件。
pub(crate) async fn run_api_agent_inner(
    pool: &DbPool,
    app: &tauri::AppHandle,
    session_id: &str,
    message: &str,
    attachments: &[Attachment],
    system_prompt: &str,
    pending_approvals: PendingApprovals,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    security_mode: Option<String>,
    persist_turn: bool,
    workflow_approval: Option<WorkflowApprovalTarget>,
    workspace_override: Option<String>,
    provider_override: Option<String>,
    model_override: Option<String>,
) -> Result<String, String> {
    let result = run_api_agent_body(
        pool,
        app,
        session_id,
        message,
        attachments,
        system_prompt,
        pending_approvals,
        temperature,
        max_tokens,
        security_mode,
        persist_turn,
        workflow_approval,
        workspace_override,
        provider_override,
        model_override,
    )
    .await;
    if let Err(e) = &result {
        let _ = app.emit(
            "agent-error",
            serde_json::json!({
                "sessionId": session_id,
                "error": e,
            }),
        );
    }
    result
}

/// `run_api_agent_inner` 的执行体：所有失败都以 `Result::Err` 返回，由外层统一发 `agent-error`。
async fn run_api_agent_body(
    pool: &DbPool,
    app: &tauri::AppHandle,
    session_id: &str,
    message: &str,
    attachments: &[Attachment],
    system_prompt: &str,
    pending_approvals: PendingApprovals,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    security_mode: Option<String>,
    persist_turn: bool,
    workflow_approval: Option<WorkflowApprovalTarget>,
    workspace_override: Option<String>,
    provider_override: Option<String>,
    model_override: Option<String>,
) -> Result<String, String> {
    let conn = pool.get().map_err(|e| format!("数据库连接失败: {}", e))?;

    // 0. 加载持久化权限规则（清单分类：高风险/风险/安全）+ 会话安全模式（本消息有效）
    let permission_rules = commands::permission::load_rules(&conn).unwrap_or_default();

    // 1. 加载会话信息（获取 api_provider 和 api_model）
    let session = commands::session::get_session_inner(&conn, session_id)
        .map_err(|e| format!("查询会话失败: {}", e))?
        .ok_or_else(|| format!("会话不存在: {}", session_id))?;

    // 1.1 API Agent 生成并持久化 agent_session_id（供外部系统恢复会话）
    // 用 match 而不是 `if is_none() { … } else { unwrap() }`：后者把"刚判过非空"和"取值"
    // 分在两处，中间任何改动都会让那个 unwrap 变成一次崩溃。模式匹配一次拿全。
    let _agent_session_id = match session.agent_session_id.clone() {
        Some(id) => id,
        None => {
            let generated = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "UPDATE sessions SET agent_session_id = ?1 WHERE id = ?2",
                params![generated, session_id],
            )
            .map_err(|e| format!("保存 agent_session_id 失败: {}", e))?;
            log::info!(
                "[API Agent] 生成 agent_session_id: {} -> {}",
                session_id,
                generated
            );
            // 通知前端更新会话状态
            let _ = app.emit(
                "agent-session",
                serde_json::json!({
                    "sessionId": session_id,
                    "agentSessionId": generated,
                }),
            );
            generated
        }
    };

    // 1.2 模型来源覆盖（工作流节点「延续会话 + 模型来源=跟随节点」）：
    // 只影响本轮运行——**不改写会话行的 provider/model**，用户会话保持原样；
    // 传 None 时仍按会话行取值（交互式聊天与「跟随会话」都走这条）。
    let provider_id = provider_override
        .or_else(|| session.api_provider.clone())
        .ok_or_else(|| "API 会话缺少提供商配置".to_string())?;
    let model = model_override
        .or_else(|| session.api_model.clone())
        .ok_or_else(|| "API 会话缺少模型配置".to_string())?;

    // 2. 获取 API 提供商配置
    let provider = commands::api_provider::get_api_provider(&conn, &provider_id)
        .map_err(|e| format!("查询提供商失败: {}", e))?
        .ok_or_else(|| format!("提供商不存在: {}", provider_id))?;

    let api_key = commands::api_provider::get_api_key(&conn, &provider_id)
        .map_err(|e| format!("获取 API Key 失败: {}", e))?
        .ok_or_else(|| format!("API Key 未配置: {}", provider_id))?;

    // 3. 加载技能（Progressive Disclosure: 先注入 name+description）
    let skills_dir = get_api_agent_skills_dir();
    let skill_loader = Arc::new(SkillLoader::new(skills_dir));

    // 3.5 初始化记忆库（SQLite: MEMORY.db）
    // 记忆上限统一存主库（本轮的 `conn` 即主库连接）；读主库设置后传入，MEMORY.db 不再自持设置。
    let memory_store = {
        let config_dir = crate::api_agent::system_prompt::get_pilotdesk_config_dir()
            .ok_or_else(|| "无法获取配置目录".to_string())?;
        let limit = crate::api_agent::db::load_memory_max_entries(&conn);
        MemoryStore::new(&config_dir, limit)?
    };
    let memory_store = Arc::new(memory_store);

    // 3.5.1 知识库（同一个 MEMORY.db 的另一个连接，WAL 下并存读没问题）：
    // 一次 run 只开一次，同时供 system prompt 的目录摘要与 search_knowledge 工具用。
    // 开不起来不阻断本轮（知识库是可选能力）：摘要不注入、工具不注册，其余照常。
    let knowledge_store = crate::api_agent::system_prompt::get_pilotdesk_config_dir()
        .and_then(|d| crate::api_agent::knowledge::KnowledgeStore::open(&d).ok())
        .map(Arc::new);

    // 3.6 KV 记忆注入块：意图路由（启用时一次轻量 LLM 解析意图 → 分域检索注入）。
    // 注入只有意图命中一条路径：0 命中不注入；路由不可用 / 解析失败 / 熔断冷却期同样不注入
    // （不做 ranked-top 降级——评分高不等于与本轮话题相关）。
    // 设置同步读取（避免 &Connection 跨 await 破坏 Send）
    let memory_intent_enabled = crate::api_agent::memory_intent::read_enabled(&conn);
    let memory_intent_override = crate::api_agent::memory_intent::read_model_override(&conn);
    let memory_intent_timeout = crate::api_agent::memory_intent::read_timeout_secs(&conn);
    // 覆盖项指定了提供商就连 endpoint/格式/Key 一起换（路由可以单独走便宜的小模型）；
    // 指定了却已不可用（被删 / Key 被清）则退回当前会话的提供商，只覆盖模型名 ——
    // 一条过期配置不该让整条注入链路彻底失效。解析必须在 await 之前做完。
    let (intent_format, intent_endpoint, intent_key) = match memory_intent_override
        .as_ref()
        .filter(|o| !o.provider_id.is_empty())
    {
        Some(o) => match (
            commands::api_provider::get_api_provider(&conn, &o.provider_id),
            commands::api_provider::get_api_key(&conn, &o.provider_id),
        ) {
            (Ok(Some(p)), Ok(Some(k))) => (p.api_format, p.api_endpoint, k),
            _ => {
                log::warn!(
                    "[MemoryIntent] 指定的路由提供商 {} 不可用，退回当前会话提供商（仅覆盖模型名）",
                    o.provider_id
                );
                (
                    provider.api_format.clone(),
                    provider.api_endpoint.clone(),
                    api_key.clone(),
                )
            }
        },
        None => (
            provider.api_format.clone(),
            provider.api_endpoint.clone(),
            api_key.clone(),
        ),
    };
    let memory_intent_model = memory_intent_override
        .map(|o| o.model)
        .filter(|m| !m.is_empty());
    let (kv_memories_block, kv_intent_usage, kv_retrieval_trace) =
        crate::api_agent::memory_intent::memory_injection_block(
            memory_intent_enabled,
            memory_intent_model,
            memory_intent_timeout,
            &intent_format,
            &intent_endpoint,
            &intent_key,
            &model,
            message,
            &memory_store,
        )
        .await;

    // 3.6.1 自动记忆检索的可观测记录：意图路由是轮前系统步骤（必须在组装 messages 之前跑完，
    // 结果用于拼 <memory_context>），因此它不是模型发起的工具调用——以独立的系统步骤事件发出，
    // 前端在思维链里按系统步骤渲染（不计入工具调用数），避免出现"有调用记录却无此工具"的假象。
    // 关闭意图路由（disabled）时不发事件，尊重用户显式关闭的预期。
    if kv_retrieval_trace.status != "disabled" {
        let trace = &kv_retrieval_trace;
        let _ = app.emit(
            "agent-system-step",
            serde_json::json!({
                "sessionId": session_id,
                "stepId": format!("mem-intent-{}", now_millis()),
                "title": memory_retrieval_title(trace),
                "params": memory_retrieval_params(trace),
                "detail": kv_retrieval_summary(trace),
                "success": memory_intent_succeeded(trace.status),
            }),
        );
    }
    // 意图路由是一次真实 LLM 调用：把其用量计入本会话统计（决策：摘要/意图调用一并计费）。
    if let Some(u) = kv_intent_usage {
        let intent_api_format = provider.api_format.parse::<ApiFormat>().unwrap_or_default();
        if let Ok(usage_conn) = pool.get() {
            let _ = crate::api_agent::agent_loop::record_usage_row(
                &usage_conn,
                &session_id,
                &provider.id,
                &model,
                &intent_api_format,
                u.prompt,
                u.completion,
                u.total,
                u.cache_read,
                u.cache_write,
            );
        }
    }

    // 4. 组装 System Prompt（Base + MEMORY.md + USER.md + Skill 列表；KV 记忆块随后以 user 上下文注入）
    let api_agent_base_prompt = concat!(
        "<agent_role>\n",
        "你是一个智能编程助手（PilotDesk Agent）。你可以：\n",
        "1. 使用常识和内置知识直接回答一般性问题（如日期、常识、编程概念等）——无需调用任何工具\n",
        "2. 调用 read_file 读取文件内容——安全低风险，无需审批\n",
        "3. 调用 list_files 列出目录内容——安全低风险，无需审批\n",
        "4. 调用 write_file 创建文件（脚本、代码、配置等）——写入完成后告知用户文件路径\n",
        "5. 调用 execute_command 执行命令——高风险操作，需用户确认\n",
        "6. 调用 search_memory 查找用户保存的偏好和项目事实——仅在需要了解用户背景时使用\n",
        "7. 调用 search_knowledge 检索用户知识库里的资料（文档/制度/教程/规范等整理成的知识，返回正文与来源原文）\n",
        "8. 调用 read_kb_file 通读知识库里某份原文（按分块顺序；不传文件名会先列出有哪些文件）\n",
        "9. 调用 save_knowledge 把一条提炼过的知识写进指定知识库（目标库必填，直接入库）\n",
        "10. 调用 load_skill 加载特定技能——仅在确定需要该技能执行任务时使用\n",
        "11. 调用 save_memory 保存重要信息供后续对话使用\n",
        "12. 调用 search_web 联网搜索（默认 Bing 中国版，返回标题/链接/摘要）——低风险\n",
        "13. 调用 fetch_web 抓取指定网页的正文文本——低风险\n",
        "14. 调用 browser 工具访问网页（action=fetch 抓取渲染后页面 / action=screenshot 截图）——中风险需确认\n",
        "15. 调用 generate_image 根据文字描述生成图片（也支持传入 image 数组做图生图/编辑/变体，仅 OpenAI 兼容提供商可用）\n",
        "16. 调用 edit_image 编辑已有图片（图生图/遮罩编辑，需提供图片路径/URL，仅 OpenAI 兼容提供商可用）\n",
        "17. 调用 task 把独立子任务交给子代理处理（调研、分析、规划、写作等）\n",
        "\n",
        "重要规则：\n",
        "- 优先使用你的内置知识回答问题，不要为了使用工具而使用工具\n",
        "- 优先使用 read_file 查看文件内容，list_files 浏览目录——不要用 execute_command 做这些\n",
        "- 如果用户的问题是常识性的（如询问人物、民科问题、日期、编程语法等），请直接回答，不要调用任何工具\n",
        "- 如果用户要求你创建文件（脚本、代码、文档），务必调用 write_file 工具来完成\n",
        "- 写入文件时请使用 <environment> 中提供的真实路径，不要猜测用户名\n",
        "- execute_command 仅用于运行脚本、git 操作、系统信息查询等真正需要执行的场景\n",
        "- search_memory 仅用于查找用户之前保存的个性化信息，不是通用搜索引擎\n",
        "- 需要查用户整理过的资料（项目规范、公司制度、教程文档等）时用 search_knowledge，\n",
        "  不要拿 search_memory 去搜它们（它只匹配标题与标签，搜不到正文）；不确定用户有没有相关资料时，\n",
        "  先不带参数调用一次 search_knowledge 看清单，再带关键词检索\n",
        "- 要「记住」东西时先分清去处：**关于用户/项目的偏好与事实**（如「我喜欢用 TypeScript」）→ save_memory；\n",
        "  **从资料或讨论里提炼出的可复用知识**（结论、规则、方案）→ save_knowledge，且**必须指定目标知识库**，\n",
        "  不确定存哪个库就先问用户，不要猜\n",
        "- USER.md / MEMORY.md 是应用级记忆文件，已随本提示自动注入（见上方 <project_memory> /\n",
        "  <user_preferences>；对应块不存在即表示该文件不存在或为空），无需再用 read_file 读取；\n",
        "  需要查询用户已保存的偏好/事实时调用 search_memory，需要补充长期记忆时调用 save_memory\n",
        "- 这两个文件各有且只有一份，位置不同：MEMORY.md 在工作区目录下（项目级），USER.md 在 PilotDesk\n",
        "  配置目录下（用户级）——项目目录下没有 USER.md。二者真实路径已在 <environment> 中给出，\n",
        "  提及位置时只能照抄该路径，不要按“项目级/用户级”推断或拼接（例如把工作区目录与 USER.md 拼一起），\n",
        "  也不要假设它们在用户主目录下\n",
        "- 当需要最新信息、实时数据或事实核查时，使用 search_web 联网搜索；无法联网获取时如实告知用户\n",
        "- save_memory 仅在以下场景使用：\n",
        "  a) 用户明确要求你记住某事（如\"记住我喜欢用 TypeScript\"）\n",
        "  b) 发现用户的个人偏好、项目决策或重要上下文（如\"我的项目数据库使用 PostgreSQL\"）\n",
        "- 不要为了一般性知识问答调用任何工具——直接回答即可\n",
        "- 如果找不到相关记忆或技能，如实告知并继续用你的知识回答\n",
        "</agent_role>\n",
        "<environment>\n",
        "当前用户主目录: {HOME}\n",
        "当前用户桌面:  {DESKTOP}\n",
        "工作区目录:    {CWD}\n",
        "项目记忆文件:  {MEMORY_MD}（项目级，位于工作区目录）\n",
        "用户偏好文件:  {USER_MD}（用户级，位于配置目录；工作区下没有 USER.md）\n",
        "</environment>"
    );

    // 注入真实路径信息，避免 LLM 猜测错误的用户名
    let home = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "未知".to_string());
    let desktop = dirs::desktop_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{}\\Desktop", home));
    let cwd_display = if session.cwd.is_empty() {
        "未知".to_string()
    } else {
        session.cwd.clone()
    };

    // 记忆文件真实路径：与运行时注入用的是同一套解析（记忆根复用 resolve_memory_root，
    // USER.md 取统一配置根）。把路径写进 <environment>，避免模型按主目录猜路径后去读错文件
    // ——那会命中工作区外的路径子策略，弹出与用户意图无关的审批。
    let memory_root = crate::commands::memory::resolve_memory_root(
        &conn,
        if session.cwd.trim().is_empty() {
            None
        } else {
            Some(&session.cwd)
        },
    );
    let memory_md_display = std::path::Path::new(&memory_root)
        .join("MEMORY.md")
        .to_string_lossy()
        .to_string();
    let user_md_display = crate::api_agent::system_prompt::get_pilotdesk_config_dir()
        .map(|d| {
            std::path::Path::new(&d)
                .join("USER.md")
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| "未知".to_string());

    let api_agent_base_prompt = api_agent_base_prompt
        .replace("{HOME}", &home)
        .replace("{DESKTOP}", &desktop)
        .replace("{CWD}", &cwd_display)
        .replace("{MEMORY_MD}", &memory_md_display)
        .replace("{USER_MD}", &user_md_display);

    let full_base_prompt = if system_prompt.is_empty() {
        api_agent_base_prompt.to_string()
    } else {
        format!("{}\n\n{}", api_agent_base_prompt, system_prompt)
    };

    let full_system_prompt = {
        // session 作用域技能禁用集：被禁技能只从 `<available_skills>` 目录隐藏，
        // 不注入模型（load_skill 工具仍可点名加载，见 run_api_agent 下方 skill_loader 透传）。
        let disabled_skills =
            crate::commands::app_settings::load_skill_scope_disabled(&conn).session;
        let visible_skills: Vec<_> = skill_loader
            .list_skills()
            .into_iter()
            .filter(|e| !disabled_skills.contains(&e.name))
            .collect();
        // 知识库目录摘要：模型"知道自己有什么资料"才会去查（不进正文，只给概况）
        let knowledge_briefs: Vec<crate::api_agent::system_prompt::KnowledgeBaseBrief> =
            knowledge_store
                .as_ref()
                .map(|ks| {
                    ks.list_bases()
                        .into_iter()
                        .map(|b| crate::api_agent::system_prompt::KnowledgeBaseBrief {
                            name: b.name,
                            description: b.description,
                            entry_count: b.entry_count,
                            file_count: b.file_count,
                            // 专属字段定义要一起给模型：`save_knowledge` 的 meta 按它填
                            fields_json: b.fields_json,
                        })
                        .collect()
                })
                .unwrap_or_default();
        let mut builder = SystemPromptBuilder::new(full_base_prompt)
            .with_memory_md(Some(memory_root.as_str()))
            .with_user_md()
            .with_skills(visible_skills)
            .with_knowledge_bases(knowledge_briefs);

        // Git 仓库上下文
        if !session.cwd.is_empty() {
            if let Some(git) = GitContext::from_cwd(&session.cwd) {
                builder = builder.with_git_context(&git);
            }
        }

        builder.build()
    };

    // 5. 加载会话消息历史（事件为唯一事实源，session_contexts 不再存消息快照）
    let history = commands::session::get_session_messages_inner(&conn, session_id)
        .map_err(|e| format!("加载消息历史失败: {}", e))?;
    let has_history = !history.is_empty();

    // 5.5 会话连续性：读取滚动摘要（summary/result 事件为唯一事实源；recent 由事件派生重建）
    // 注意：不存 system prompt，system prompt 每次动态构建。
    let saved_summary: String = crate::eventlog::latest_session_summary(&conn, session_id)
        .map_err(|e| format!("加载会话摘要失败: {}", e))?
        .unwrap_or_default();

    let mut messages: Vec<ChatMessage> = Vec::new();

    // 注入滚动摘要（独立 system 消息，紧随主 system prompt 之后）
    if !saved_summary.is_empty() {
        messages.push(ChatMessage::system(&format!(
            "<conversation_summary>\n{}\n</conversation_summary>",
            saved_summary
        )));
    }

    // 模型可见历史 = 事件派生的 user/assistant 消息（映射与旧"首次请求回退"路径一致）。
    // 思维链只在**当前模型**需要时才回传：会话可以中途换模型，历史里可能留着另一个模型
    // 产生的 reasoning_content，送给普通模型会被部分 provider 直接拒掉。
    let pass_reasoning = crate::api_agent::context::supports_reasoning_content(&model);
    let mut msgs: Vec<ChatMessage> = history
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| match m.role.as_str() {
            "assistant" => {
                // 思考模式（DeepSeek 等）：从持久化的思维链中恢复 reasoning_content，随请求原样回传。
                let reasoning = if pass_reasoning {
                    extract_reasoning_from_tool_calls(&m.tool_calls)
                } else {
                    String::new()
                };
                ChatMessage::assistant_with_reasoning(&m.content, &reasoning)
            }
            _ => ChatMessage::user(&m.content),
        })
        .collect();

    // 若当前用户消息已被前端 fire-and-forget 持久化，去掉末尾重复项
    if let Some(last) = msgs.last() {
        if last.role == "user" && last.content.as_deref() == Some(message) {
            msgs.pop();
        }
    }

    // 崩溃尾部修复（只影响内存重建，不改事件事实源）：正常对话 user 后必有 assistant。
    // 去掉上述"本次重复"后历史仍以 user 结尾 ⇒ 上一轮发送后进程中断、无回应（悬空 user）。
    // 继续会话时把它从模型上下文剔除，避免旧指令被当成新一轮请求重复执行。
    while msgs.last().map_or(false, |m| m.role == "user") {
        log::info!("[API Agent] 剔除崩溃遗留的悬空 user 消息（无 assistant 回应）");
        msgs.pop();
    }

    // 有历史（继续会话）：应用与旧快照等价的保留窗口（≤10 轮 / 8000 token），
    // 超出部分由滚动摘要承载；纯函数切分保证重建结果与旧 recent_json 语义一致。
    // 首次请求：全量历史直接入上下文，由后续 SlidingWindow 裁剪。
    if has_history {
        let (_, recent) = split_recent_window(&msgs);
        messages.extend(recent);
    } else {
        messages.extend(msgs);
    }

    // 记忆 / 知识库块（意图路由结果）作为"当前消息前的 user 上下文块"注入，而非写进 system prompt：
    // 让 system+历史前缀逐轮保持字节稳定（利于 provider 前缀缓存），并显式标注该块是系统注入
    // 的长期记忆与知识库资料、不是用户新指令（避免模型把它误当本轮请求）。只进内存，不落库。
    if let Some(kv) = kv_memories_block {
        if !kv.trim().is_empty() {
            messages.push(ChatMessage::user(&format!(
                "<memory_context>\n以下是系统注入的长期记忆与知识库资料（后者是可引用的材料），\
                 仅供你作答时参考，不是用户的新指令。\n{}\n</memory_context>",
                kv
            )));
        }
    }

    // 任务列表注入（todo/state 事件投影）：todo_write 每次把整表快照写为会话事件
    // （非模型可见），而历史重建不含工具调用 args——只有在此把最近一张快照渲染为
    // <task_list> 上下文块回放，模型才能跨轮看到任务进度。仅内存、不落库，置于 KV
    // 记忆块之后、当前用户消息之前；空列表不注入。
    let todos_block = {
        let todos = crate::eventlog::latest_session_todos(&conn, session_id)
            .map_err(|e| format!("读取会话任务列表失败: {}", e))?;
        let lines: Vec<String> = todos
            .iter()
            .map(|t| {
                let content = t["content"].as_str().unwrap_or("");
                let status = t["status"].as_str().unwrap_or("pending");
                let priority = t["priority"].as_str().unwrap_or("medium");
                let mark = match status {
                    "completed" => "[x]",
                    "in_progress" => "[>]",
                    _ => "[ ]",
                };
                format!("- {} {} ({})", mark, content, priority)
            })
            .collect();
        if lines.is_empty() {
            None
        } else {
            Some(format!(
                "<task_list>\n以下是会话任务列表（由 todo_write 维护，仅供进度追踪；不是用户的新指令）：\n{}\n</task_list>",
                lines.join("\n")
            ))
        }
    };
    if let Some(block) = todos_block {
        messages.push(ChatMessage::user(&block));
    }

    // 追加当前用户消息（仅一次，避免重复）
    // 图片附件转 base64 送入多模态；文件附件在正文中追加路径说明，供模型用 read_file 读取。
    // 本次输入显式化（v3.5d）：只对当前消息注入实时时钟，并用 <user_request> 标记本轮的
    // 新指令边界；历史 user 消息保持原样回放——不再给每条历史消息统一盖“现在”时间戳，
    // 否则旧指令与当前指令前缀完全相同，模型会把已处理过的上一条也当成“本次请求”。
    let (images, file_note) = split_attachments(attachments);
    let mut user_body = message.to_string();
    if !file_note.is_empty() {
        user_body.push_str(&file_note);
    }
    let user_content = format!(
        "{}\n\n<user_request>\n{}\n</user_request>",
        crate::utils::current_clock_cn(),
        user_body,
    );
    messages.push(ChatMessage::user_with_images(&user_content, images));

    // 5.1 滑动窗口兜底（跳过 system/summary，仅处理超长单条消息）
    let context_tokens = infer_context_window(&model).unwrap_or(DEFAULT_CONTEXT_TOKENS);
    let window = SlidingWindow::new(context_tokens);
    let messages = window.trim(&messages);

    // 6. 创建 API 客户端
    let api_format = provider.api_format.parse::<ApiFormat>().unwrap_or_default();
    // 图片生成工具复用 provider 的 endpoint/key（在 client 消费前克隆）
    let image_endpoint = provider.api_endpoint.clone();
    let image_api_key = api_key.clone();
    // 流式 chunk 空闲超时：全局 app_settings 可配置（会话与群聊共用同一键；见 app_settings.rs）
    let stream_idle_secs = crate::commands::app_settings::load_stream_idle_secs(&conn);
    let client = if matches!(api_format, ApiFormat::Anthropic) {
        ApiClient::new(provider.api_endpoint, api_key, api_format.clone())
            .with_stream_idle(stream_idle_secs)
    } else {
        ApiClient::new_openai(provider.api_endpoint, api_key).with_stream_idle(stream_idle_secs)
    };
    // 摘要生成复用一个独立客户端实例（AgentLoop 会独占消费 client）
    let summary_client = client.clone();
    let summary_model = model.clone();
    let summary_format = api_format.clone();

    // 7. 构建工具注册表（全量装配 + 会话清单，见 tools/mod.rs 装配层）
    // 先合并工具管理页的会话模式 overrides（追加禁用，新会话生效），
    // 供能力项（文件历史）等按场景差异化禁用判断使用。
    let mut profile = crate::tools::ToolProfile::session();
    let overrides = crate::commands::tools::load_tool_overrides(&conn);
    profile.add_extra_disable(overrides.session);

    // 文件历史能力项：被用户按场景禁用时不注入（新会话生效）
    let file_history = if crate::tools::is_disabled(&profile, crate::tools::FILE_HISTORY_CAP) {
        None
    } else {
        Some(Arc::new(crate::tools::history::FileHistoryService {
            app: app.clone(),
            scope: session_id.to_string(),
            pool: pool.clone(),
            enabled: Arc::new(AtomicBool::new(true)),
        }))
    };

    // 模型能力查询与跨 provider 解析（闭包持有连接池；key 只在后端解析，绝不进 LLM 上下文）。
    let pool_for_providers = pool.clone();
    let session_pid = provider_id.clone();
    let list_providers: Option<
        Arc<dyn Fn() -> Vec<crate::tools::ProviderModelInfo> + Send + Sync>,
    > = Some(Arc::new(move || {
        let Ok(conn) = pool_for_providers.get() else {
            return Vec::new();
        };
        let mut list = crate::commands::api_provider::collect_provider_models(&conn);
        // 标注当前会话提供商：生成类工具要求 provider 与 model 分别传入，LLM 需要据此填正确的 provider_id
        for p in list.iter_mut() {
            p.is_session = p.provider_id == session_pid;
        }
        list
    }));
    let pool_for_resolve = pool.clone();
    let resolve_provider: Option<
        Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>,
    > = Some(Arc::new(move |pid| {
        let Ok(conn) = pool_for_resolve.get() else {
            return None;
        };
        let Ok(Some(p)) = crate::commands::api_provider::get_api_provider(&conn, pid) else {
            return None;
        };
        let Ok(Some(key)) = crate::commands::api_provider::get_api_key(&conn, pid) else {
            return None;
        };
        Some((p.api_endpoint, key, p.api_format))
    }));

    // 本轮运行的工作目录 / 授权边界：工作流节点传产物目录（只对本轮生效，不改写会话行，
    // 所以「延续会话」也不会把被续会话的旧 cwd 带过来）；会话模式沿用会话行自己的 cwd。
    let effective_cwd = workspace_override.unwrap_or_else(|| session.cwd.clone());
    let env = crate::tools::ToolEnv {
        cwd: effective_cwd.clone(),
        api_format: api_format.clone(),
        image: if matches!(api_format, ApiFormat::Anthropic) {
            None
        } else {
            Some((image_endpoint.clone(), image_api_key.clone()))
        },
        // read_image 两种协议通用：Anthropic 走 /v1/messages 的多模态 messages，不需 /images 端点。
        vision: Some((image_endpoint.clone(), image_api_key.clone())),
        audio: if matches!(api_format, ApiFormat::Anthropic) {
            None
        } else {
            Some((image_endpoint, image_api_key))
        },
        list_providers,
        resolve_provider,
        search_config: commands::search::load_search_config(&conn),
        client: Some(client.clone()),
        model: model.clone(),
        skill_loader: Some(skill_loader),
        memory_store: Some(memory_store),
        knowledge_store: knowledge_store.clone(),
        app: Some(app.clone()),
        session_id: session_id.to_string(),
        pending: Some(pending_approvals.clone()),
        file_history,
        permission_rules: Some(std::sync::Arc::new(permission_rules.clone())),
    };
    let ctx = crate::tools::BuildContext { env: &env, pool };
    let tool_registry = crate::tools::build_registry(&profile, &ctx).await?;
    let tools = tool_registry.get_definitions();

    // 8. 创建 AgentLoop（直接发送 Tauri 事件到前端，确保审批前实时送达）
    let app_for_agent = app.clone();
    let sid_for_agent = session_id.to_string();
    log::info!(
        "[API Agent] 启动 AgentLoop: session={}, model={}",
        session_id,
        model
    );

    let agent_loop = AgentLoop::new(client, tool_registry, model, app_for_agent, sid_for_agent)
        .with_api_format(api_format)
        .with_permission_rules(permission_rules)
        .with_workspace(Some(effective_cwd.clone()))
        .with_security_mode(
            security_mode
                .as_deref()
                .and_then(SecurityMode::from_str)
                .unwrap_or_default(),
        );

    // 审批回调按"谁在审批"分两条：
    // - 工作流 Agent 节点：命中节点「工具授权」策略的调用注册进工作流的等待登记表，
    //   向工作流页/通知中心/编辑器发 `workflow:approval-*` 等用户裁决；等不到（超时）
    //   按拒绝处理——工作流放行等于绕过授权策略。审批现场同时写进执行事件流以便恢复。
    // - 会话模式：向前端发 `agent-approval-required`（内联审批卡）阻塞等用户回复。
    let agent_loop =
        if let Some(target) = workflow_approval {
            let app_for_approval = app.clone();
            let pool_for_approval = pool.clone();
            agent_loop
                .with_approval_label(WORKFLOW_APPROVAL_LABEL)
                .with_approval_handler(Box::new(
                    move |call_id: &str, tool_name: &str, args: &str, risk: RiskLevel| {
                        let execution_id = target.execution_id.clone();
                        let node_id = target.node_id.clone();
                        let node_label = target.node_label.clone();
                        let risk_desc = risk.description().to_string();
                        let item = PendingToolApproval {
                            call_id: call_id.to_string(),
                            execution_id: execution_id.clone(),
                            node_id: node_id.clone(),
                            node_label: node_label.clone(),
                            tool_name: tool_name.to_string(),
                            arguments: args.to_string(),
                            risk: risk_desc.clone(),
                            created_at: crate::utils::now(),
                            stale: false,
                        };
                        // 先留证据再等人：进程中途退出也能从事件流看出卡在哪次审批上
                        record_tool_approval_event(
                            &pool_for_approval,
                            &execution_id,
                            "approval/requested",
                            &item,
                            None,
                        );
                        let _ = app_for_approval.emit(
                            "workflow:approval-required",
                            serde_json::json!({
                                "execution_id": execution_id,
                                "node_id": node_id,
                                "node_label": node_label,
                                "call_id": call_id,
                                "tool_name": tool_name,
                                "arguments": args,
                                "risk": risk_desc,
                            }),
                        );
                        log::info!(
                    "[Approval] 工作流节点等待用户审批: exec={}, node={}, tool={}, risk={:?}",
                    execution_id, node_id, tool_name, risk
                );

                        let item_snapshot = item.clone();
                        let rx = target.manager.register(item);
                        // 与会话路径同一手法：block_in_place 避免占死 tokio 工作线程
                        let result = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                tokio::time::timeout(
                                    std::time::Duration::from_secs(WORKFLOW_APPROVAL_TIMEOUT_SECS),
                                    rx,
                                )
                                .await
                            })
                        });
                        let (approved, timed_out) = match result {
                            Ok(Ok(decided)) => {
                                log::info!(
                                    "[Approval] 用户{}工作流工具调用: tool={}",
                                    if decided { "批准" } else { "拒绝" },
                                    item_snapshot.tool_name
                                );
                                (decided, false)
                            }
                            _ => {
                                // 超时或通道关闭：撤销等待登记（登记表严格等于"此刻有人在等"），按拒绝处理
                                target.manager.forget(&item_snapshot.call_id);
                                log::warn!(
                                    "[Approval] 工作流审批超时，按拒绝处理: call={}",
                                    item_snapshot.call_id
                                );
                                (false, true)
                            }
                        };
                        record_tool_approval_event(
                            &pool_for_approval,
                            &item_snapshot.execution_id,
                            "approval/resolved",
                            &item_snapshot,
                            Some(approved),
                        );
                        let _ = app_for_approval.emit(
                            "workflow:approval-resolved",
                            serde_json::json!({
                                "execution_id": item_snapshot.execution_id,
                                "node_id": item_snapshot.node_id,
                                "call_id": item_snapshot.call_id,
                                "approved": approved,
                                "timed_out": timed_out,
                            }),
                        );
                        approved
                    },
                ))
        } else {
            let pending_shared = pending_approvals.clone();
            let app_for_approval = app.clone();
            let sid_for_approval = session_id.to_string();
            agent_loop
                .with_approval_handler(Box::new(
                    move |call_id: &str, tool_name: &str, _args: &str, risk: RiskLevel| {
                        let rx = pending_shared.register(call_id.to_string());

                        log::info!(
                            "[Approval] 等待用户审批: tool={}, risk={:?}",
                            tool_name,
                            risk
                        );

                        // 使用 block_in_place 避免阻塞 tokio 工作线程（防止多次审批耗尽线程池导致崩溃）
                        let result = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                tokio::time::timeout(std::time::Duration::from_secs(120), rx).await
                            })
                        });
                        let (allow, timed_out) = match result {
                            Ok(Ok(approved)) => {
                                log::info!(
                                    "[Approval] 用户{}: {}",
                                    if approved { "批准" } else { "拒绝" },
                                    tool_name
                                );
                                (approved, false)
                            }
                            _ => {
                                // 超时策略：高风险默认拒绝，中/低风险默认允许
                                let allow = risk < RiskLevel::High;
                                log::warn!(
                                    "[Approval] 审批超时（risk={:?}）→ {}",
                                    risk,
                                    if allow {
                                        "默认允许"
                                    } else {
                                        "默认拒绝"
                                    }
                                );
                                // 丢弃超时后残留的 sender，避免 PendingApprovals.approvals 泄漏
                                pending_shared.discard(call_id);
                                (allow, true)
                            }
                        };
                        let _ = app_for_approval.emit(
                            "agent-approval-resolved",
                            serde_json::json!({
                                "sessionId": &sid_for_approval,
                                "toolId": call_id,
                                "toolName": tool_name,
                                "approved": allow,
                                "timedOut": timed_out,
                                "risk": risk.description(),
                            }),
                        );
                        allow
                    },
                ))
                // 会话路径的审批由用户亲自决策：向前端发审批请求（内联审批卡）等待回复
                .with_user_approval_notification()
        };

    // 迭代上限确认回调（复用 PendingApprovals 的 oneshot + block_in_place 模式）
    let pending_continue = pending_approvals.clone();
    let sid_continue = session_id.to_string();
    let app_for_continue = app.clone();
    let agent_loop = agent_loop.with_continue_handler(Box::new(move |current, max| {
        log::info!("[ContinueLoop] 等待用户确认: {}/{}", current, max);
        let rx = pending_continue.register_continue(&sid_continue);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(120), rx).await
            })
        });
        let (should_continue, timed_out) = match result {
            Ok(Ok(should_continue)) => {
                log::info!(
                    "[ContinueLoop] 用户{}",
                    if should_continue {
                        "继续执行"
                    } else {
                        "终止"
                    }
                );
                (should_continue, false)
            }
            _ => {
                log::warn!("[ContinueLoop] 超时，默认继续执行");
                // 丢弃超时后残留的 sender，避免 PendingApprovals.continue_reqs 泄漏
                pending_continue.discard_continue(&sid_continue);
                (true, true) // 超时默认继续
            }
        };
        // 与 agent-approval-resolved 对称：把决策结果发往前端，驱动内联卡的终态展示。
        let _ = app_for_continue.emit(
            "agent-iteration-limit-resolved",
            serde_json::json!({
                "sessionId": &sid_continue,
                "shouldContinue": should_continue,
                "timedOut": timed_out,
            }),
        );
        should_continue
    }));

    // 9. 配置 Agent Loop
    let config = AgentLoopConfig {
        max_iterations: 20,
        system_prompt: full_system_prompt.clone(),
        tools,
        messages: messages.clone(),
        temperature: temperature.or(session.temperature),
        max_tokens: max_tokens.or(session.max_tokens),
        context_tokens: Some(context_tokens),
    };

    if persist_turn {
        // 工作流 Agent 节点没有前端 sendChat 调用方，本轮用户提示只能由后端落库。
        // 助手消息不在此写：前端 agent-* 事件总线常驻（App.tsx 订阅，见 agentEventBus），
        // 收到本轮 agent-done 时已按流式正文 + 思维链写入 assistant/message——那是前端
        // "收尾落库不依赖组件是否挂载"的既有设计，后端再写一次会在同一会话里留下重复的助手消息。
        // 对称地，persist_turn=false（会话模式）时前后端都不缺落库方，后端一律不写。
        if let Err(e) = commands::session::save_message_inner(
            &conn, session_id, "user", message, "native", None, None, None, None,
        ) {
            log::error!(
                "[API Agent] 保存用户消息失败: session={}, error={}",
                session_id,
                e
            );
        }
    }

    // 10. 执行 Agent Loop（AgentLoop 内部直接发射 Tauri 事件到前端）
    // 绝对兜底保护（30min）：真正的“卡死”已由请求层检测（流式 chunk 空闲 90s / reqwest 总超时 /
    // 工具自管超时 / 用户取消 / 迭代上限）。此兜底仅防“全链路黑洞”等病态场景，不再承担
    // 合法长任务（长思考、多轮工具、等待用户确认）的误杀。
    const AGENT_LOOP_TIMEOUT_SECS: u64 = 1800;
    // 会话运行登记：既是"运行中"的真相来源（列表脉冲/重入判断），也是"停止生成"的取消通道。
    // 登记点紧贴执行，避免中途的提前返回留下无人注销的运行记录；守卫负责被 drop 时注销。
    let run_token = crate::api_agent::session_runs::begin(session_id);
    let _run_guard = ApiRunGuard {
        session_id: session_id.to_string(),
        token: std::sync::Arc::clone(&run_token),
    };
    let agent_loop = agent_loop.with_cancel_token(std::sync::Arc::clone(&run_token));

    // 连接在组装期用完即还：Agent Loop 可能一直跑到 30 分钟兜底上限，不应长期占用连接池。
    drop(conn);

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(AGENT_LOOP_TIMEOUT_SECS),
        agent_loop.run(config),
    )
    .await
    {
        Ok(Ok(output)) => {
            log::info!("[API Agent] 对话完成: session={}", session_id);
            output
        }
        Ok(Err(e)) => {
            log::error!("[API Agent] 对话失败: session={}, error={}", session_id, e);
            return Err(e);
        }
        Err(_) => {
            log::warn!(
                "[API Agent] AgentLoop 整体超时 ({:?}): session={}",
                std::time::Duration::from_secs(AGENT_LOOP_TIMEOUT_SECS),
                session_id,
            );
            return Err(format!(
                "任务执行超过最大保护时长（{} 秒），已终止（正常长任务由无进展/上游无数据/工具超时等真实信号终止，此分支仅作兜底）。请重试或检查 API 提供商状态。",
                AGENT_LOOP_TIMEOUT_SECS
            ));
        }
    };

    // 最终助手文本：取消息列表中最后一条 assistant（与落库/流式渲染同源）；
    // 无 assistant 消息或不含正文的收尾路径（如迭代上限摘要）回退到 AgentLoopOutput.content。
    let text = output
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .and_then(|m| m.content.clone())
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| output.content.clone());

    // 1. 上下文统计 + 策略判定（P0-2）：滚动摘要触发经 CompactionPolicy 决策。
    //    DefaultCompactionPolicy 与 split 阈值同源（行为不变），后续可换可配置策略。
    let older = split_recent_window(&output.messages).0;
    let compaction_stats = conversation_stats(&output.messages);
    let compaction_policy = DefaultCompactionPolicy::current();

    // 2. 读取旧摘要（滚动摘要事实源 = session_events 的 summary/result 事件；此处为增量合并基础）
    let old_summary: String = match pool.get() {
        Ok(ctx_conn) => crate::eventlog::latest_session_summary(&ctx_conn, session_id)
            .ok()
            .flatten()
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    let mut new_summary = old_summary.clone();
    let mut summary_changed = false;
    let summary_provider_id = provider.id.clone();

    // 3. 策略命中且存在超出保留窗口的早期消息时，触发增量摘要（复用当前会话模型）
    if !older.is_empty() && compaction_policy.should_compact(&compaction_stats) {
        match generate_rolling_summary(
            &summary_client,
            &summary_model,
            &summary_format,
            &new_summary,
            &older,
            // 摘要调用计入本会话用量统计（与主请求同表同 scope）。
            |p, c, t, cr, cw| {
                if let Ok(ctx_conn) = pool.get() {
                    let _ = crate::api_agent::agent_loop::record_usage_row(
                        &ctx_conn,
                        session_id,
                        &summary_provider_id,
                        &summary_model,
                        &summary_format,
                        p,
                        c,
                        t,
                        cr,
                        cw,
                    );
                }
            },
        )
        .await
        {
            Ok(s) if !s.trim().is_empty() => {
                new_summary = s;
                summary_changed = true;
            }
            Ok(_) => {
                log::warn!("[API Agent] 摘要生成为空，保留旧摘要");
            }
            Err(e) => {
                log::warn!("[API Agent] 摘要生成失败，保留旧摘要: {}", e);
            }
        }
    }

    // 4. 摘要变更时追加 summary/result 事件（未变更不落，避免事件噪音）
    if summary_changed && !new_summary.trim().is_empty() {
        let now = crate::utils::now();
        if let Ok(ctx_conn) = pool.get() {
            let event = serde_json::json!({
                "sessionId": session_id,
                "text": new_summary,
                "timestamp": now,
            });
            if let Err(e) = crate::eventlog::append_session_event(
                &ctx_conn,
                session_id,
                "summary/result",
                &event,
                true,
            ) {
                log::error!("[API Agent] 保存会话滚动摘要失败: {}", e);
            }
        }
    }

    // 5. 自动沉淀**不在这里做**（原轮末触发已移除）。
    //
    //    原因：① 它 `await` 在收尾返回路径上，会把整理耗时算进这一轮；
    //    ② 它吃内存 `output.messages`，而压缩会把早期消息替换成摘要 → 列表变短/错位 →
    //       下标水位可能重复或漏。
    //    现在改由 `commands::knowledge::kb_sediment_session`（显式命令）与
    //    `kb_auto_sediment_session`（App 空闲触发）从 **DB 重建对话**来做。

    Ok(text)
}

/// 将 glob 模式编译为正则表达式（供 glob / grep 工具使用）
/// 支持 `*`（不跨分隔符）、`**`（跨任意）、`?`（单个非分隔符字符）
fn glob_to_regex(pattern: &str) -> regex::Regex {
    let mut out = String::from("^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    out.push_str(".*");
                    i += 1;
                } else {
                    out.push_str("[^/]*");
                }
            }
            '?' => out.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            '\\' => out.push('/'),
            _ => out.push(c),
        }
        i += 1;
    }
    out.push('$');
    regex::Regex::new(&out).unwrap_or_else(|_| regex::Regex::new("^$").unwrap())
}

/// 递归收集目录下的文件相对路径（归一化为 `/` 分隔），最多 max 个。
/// 跳过隐藏目录与常见重目录（node_modules/target/.git/dist 等），避免扫描超时。
fn collect_files(root: &std::path::Path, max: usize) -> Vec<String> {
    let mut results: Vec<String> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    let mut visited_dirs = 0usize;
    const MAX_DIRS: usize = 2000;

    while let Some(dir) = stack.pop() {
        visited_dirs += 1;
        if visited_dirs > MAX_DIRS || results.len() >= max {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if results.len() >= max {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if name.starts_with('.')
                    || matches!(
                        name.as_str(),
                        "node_modules"
                            | "target"
                            | ".git"
                            | "dist"
                            | "build"
                            | "__pycache__"
                            | "venv"
                            | ".venv"
                    )
                {
                    continue;
                }
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                results.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    results
}

/// 解析用户提供的路径（展开 ~ / %USERPROFILE%，相对路径基于 cwd）
pub(crate) fn resolve_workspace_path(raw_path: &str, cwd: &str) -> std::path::PathBuf {
    let expanded = if raw_path.starts_with("~/") || raw_path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            home.join(&raw_path[2..]).to_string_lossy().to_string()
        } else {
            raw_path.to_string()
        }
    } else if raw_path.to_lowercase().starts_with("%userprofile%") {
        if let Some(home) = dirs::home_dir() {
            let rest = &raw_path[14..];
            home.join(rest.trim_start_matches('\\').trim_start_matches('/'))
                .to_string_lossy()
                .to_string()
        } else {
            raw_path.to_string()
        }
    } else {
        raw_path.to_string()
    };

    let p = std::path::Path::new(&expanded);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::path::Path::new(cwd).join(&expanded)
    }
}

/// 计算行级 diff（旧 vs 新），返回 `- old / + new` 文本。
/// 采用公共前缀/后缀法（轻量实现），超限文件返回空字符串（不计算）。
fn compute_diff(old: &str, new: &str) -> String {
    const MAX_DIFF_BYTES: usize = 256 * 1024;
    if old.len() > MAX_DIFF_BYTES || new.len() > MAX_DIFF_BYTES {
        return String::new();
    }

    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines.len() > 2000 || new_lines.len() > 2000 {
        return String::new();
    }

    // 公共前缀
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }

    // 公共后缀
    let mut suffix = 0;
    while suffix < old_lines.len() - prefix
        && suffix < new_lines.len() - prefix
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let old_mid = &old_lines[prefix..old_lines.len() - suffix];
    let new_mid = &new_lines[prefix..new_lines.len() - suffix];

    let mut out = String::new();
    for line in old_mid {
        out.push_str(&format!("- {}\n", line));
    }
    for line in new_mid {
        out.push_str(&format!("+ {}\n", line));
    }
    out
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 进程启动时清除 Python 环境变量干扰
    // PilotDesk可能会受到其他app的 PYTHONHOME / PYTHONPATH / CONDA_PREFIX 等环境变量干扰，
    // 导致子进程（hermes 等 Python CLI）加载错误的 .pth / site 模块而异常退出。
    // 此处一次性清除，所有后续 Command 子进程自动继承干净环境，无需逐处调用。
    for key in [
        "PYTHONHOME",
        "PYTHONPATH",
        "CONDA_PREFIX",
        "PYTHONNOUSERSITE",
    ] {
        std::env::remove_var(key);
    }

    let pool = init_db().expect("Failed to initialize database");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        // 自动更新：读取 tauri.conf.json 中 plugins.updater 配置的 endpoints/pubkey
        .plugin(tauri_plugin_updater::Builder::new().build())
        // 进程插件：供更新安装完成后重启应用（process:allow-restart）
        .plugin(tauri_plugin_process::init())
        // dirindex 自定义协议：目录索引页（?path=<urlencoded 绝对路径>）
        .register_uri_scheme_protocol("dirindex", |_ctx, request| {
            commands::dirindex::handle_dirindex(request)
        })
        .manage(DbState { pool: pool.clone() })
        .manage(AsyncMutex::new(AgentManager::new()))
        .manage(PendingApprovals::new())
        .manage(groupchat::room::RoomRegistry::new())
        .manage(std::sync::Mutex::new(
            terminal::console_bridge::ConsoleBridge::new(),
        ))
        .manage(AsyncMutex::new(terminal::TerminalManager::new()))
        // 会员登录：一次只允许一个进行中的授权会话（begin 与 complete 之间）
        .manage(commands::account::AccountLoginState::default())
        .invoke_handler(tauri::generate_handler![
            commands::env::detect_env,
            commands::env::clear_env_detect_cache,
            commands::env::install_agent,
            commands::env::uninstall_agent,
            commands::install_log::insert_log,
            commands::install_log::list_logs,
            commands::install_log::clear_logs,
            commands::update::check_pilotdesk_update,
            commands::update::check_agent_update,
            commands::session::list_sessions,
            list_console_sessions,
            commands::session::list_archived_sessions,
            commands::session::create_session,
            commands::session::get_session,
            commands::session::get_session_messages,
            commands::session::rename_session,
            commands::session::session_suggest_title,
            commands::session::update_session_cwd,
            commands::session::update_session_model,
            commands::session::archive_session,
            commands::session::unarchive_session,
            commands::session::delete_session,
            commands::session::save_message,
            commands::session::update_message,
            commands::session::update_session_agent_id,
            commands::session::search_sessions,
            commands::session::search_messages,
            commands::attachment::save_attachments,
            commands::attachment::groupchat_save_attachments,
            commands::attachment::delete_attachment,
            commands::attachment::open_path,
            commands::dirindex::path_is_directory,
            commands::env::ensure_dir,
            list_inspirations,
            get_inspiration,
            create_inspiration,
            update_inspiration,
            delete_inspiration,
            search_inspirations,
            list_tags,
            list_api_providers,
            get_api_provider,
            upsert_api_provider,
            delete_api_provider,
            get_api_key,
            reorder_api_providers,
            get_app_setting,
            set_app_setting,
            commands::memory::list_project_roots,
            commands::memory::get_project_memory,
            commands::memory::update_project_memory,
            commands::memory::project_memory_template,
            commands::memory::get_user_preferences,
            commands::memory::update_user_preferences,
            commands::memory::user_preferences_template,
            commands::memory::list_memory_entries,
            commands::memory::save_memory_entry,
            commands::memory::delete_memory_entry,
            commands::memory::set_memory_pin,
            commands::memory::preview_memory_maintenance,
            commands::memory::run_memory_maintenance,
            commands::memory::get_memory_stats,
            commands::memory::get_memory_max_entries,
            commands::memory::set_memory_max_entries,
            commands::knowledge::kb_list_bases,
            commands::knowledge::kb_create_base,
            commands::knowledge::kb_update_base,
            commands::knowledge::kb_delete_base,
            commands::knowledge::kb_list_entries,
            commands::knowledge::kb_list_file_chunks,
            commands::knowledge::kb_list_file_attrs,
            commands::knowledge::kb_save_entry,
            commands::knowledge::kb_unlink_entry,
            commands::knowledge::kb_unlink_entries,
            commands::knowledge::kb_list_files,
            commands::knowledge::kb_remove_files,
            commands::knowledge::kb_list_file_relations,
            commands::knowledge::kb_link_files,
            commands::knowledge::kb_unlink_files,
            commands::knowledge::kb_ingest_file,
            commands::knowledge::kb_ingest_url,
            commands::knowledge::kb_cloud_accounts_list,
            commands::knowledge::kb_cloud_account_add,
            commands::knowledge::kb_cloud_account_remove,
            commands::knowledge::kb_cloud_list_repos,
            commands::knowledge::kb_cloud_list_docs,
            commands::knowledge::kb_cloud_ingest_doc,
            commands::knowledge::kb_list_candidates,
            commands::knowledge::kb_digest_conversation,
            commands::knowledge::kb_sediment_session,
            commands::knowledge::kb_auto_sediment_session,
            commands::knowledge::kb_add_candidate,
            commands::knowledge::kb_generate,
            commands::knowledge::kb_effective_model,
            commands::knowledge::kb_root_info,
            commands::knowledge::kb_set_root,
            commands::knowledge::kb_export_files,
            commands::knowledge::kb_export_entries,
            commands::knowledge::kb_enrich_file,
            commands::knowledge::kb_adopt_candidate,
            commands::knowledge::kb_reject_candidate,
            commands::permission::get_permission_rules,
            commands::permission::set_permission_rules,
            commands::search::get_search_config,
            commands::search::set_search_config,
            commands::mcp::get_mcp_servers,
            commands::mcp::set_mcp_servers,
            commands::api_provider::get_model_notes_cmd,
            commands::api_provider::set_model_notes_cmd,
            commands::tools::tool_catalog,
            commands::tools::get_tool_overrides,
            commands::tools::set_tool_overrides,
            commands::tools::groupchat_get_file_history_enabled,
            commands::tools::groupchat_set_file_history,
            commands::file_history::list_file_history,
            commands::file_history::list_file_history_sessions,
            commands::file_history::undo_file_history,
            commands::file_history::delete_file_history,
            commands::fs_util::write_text_file,
            get_theme,
            set_theme_cmd,
            get_usage_summary,
            get_usage_attribution,
            get_session_usage,
            get_room_usage,
            commands::usage::export_usage_report_csv,
            commands::usage::usage_report_preview,
            commands::usage::usage_report_now,
            commands::cloud_sync::cloud_sync_now,
            commands::cloud_sync::cloud_sync_status,
            commands::cloud_sync::cloud_sync_set_enabled,
            agent_send_message_with_config,
            agent_stop_generation,
            agent_running_sessions,
            agent_approve_tool,
            agent_continue_loop,
            agent_respond_confirmation,
            agent_create_session,
            agent_close_session,
            agent_list_skills,
            commands::skills::skill_install,
            commands::skills::skill_uninstall,
            commands::skills::skill_read_entry,
            commands::skills::skill_write_entry,
            commands::voice::transcribe_audio,
            plugin::plugin_discover,
            plugin::plugin_list,
            plugin::plugin_enable,
            plugin::plugin_disable,
            plugin::store::read_plugin_readme,
            plugin::plugin_get_sandbox_info,
            plugin::plugin_install_zip,
            plugin::plugin_uninstall,
            plugin::plugin_set_sandbox_enabled,
            plugin::plugin_data_invoke,
            plugin::plugin_get_panel_content,
            plugin::plugin_read_entry,
            plugin::plugin_read_icon_file,
            commands::agents::list_agents,
            commands::agents::get_agent,
            commands::agents::add_agent,
            commands::agents::update_agent,
            commands::agents::delete_agent,
            commands::agents::export_agents_json,
            commands::agents::import_agents_json,
            commands::agents::reorder_agents,
            plugin::agent::plugin_agent_create_session,
            plugin::agent::plugin_agent_send_message,
            plugin::agent::plugin_agent_get_history,
            plugin::agent::plugin_agent_list_sessions,
            plugin::agent::plugin_agent_delete_session,
            plugin::agent::plugin_agent_list_agents,
            plugin::plugin_fs::plugin_fs_read_text,
            plugin::plugin_fs::plugin_fs_write_text,
            plugin::plugin_fs::plugin_fs_delete,
            plugin::plugin_fs::plugin_fs_exists,
            plugin::plugin_fs::plugin_fs_read_dir,
            plugin::shell::plugin_shell_exec,
            plugin::store::plugin_store_fetch_index,
            plugin::store::plugin_store_install,
            plugin::store::plugin_store_get_local_versions,
            commands::workflow::save_workflow_definition,
            commands::workflow::create_workflow,
            commands::workflow::list_workflows,
            commands::workflow::get_workflow,
            commands::workflow::update_workflow,
            commands::workflow::delete_workflow,
            commands::workflow::list_deleted_workflows,
            commands::workflow::restore_workflow,
            commands::workflow::purge_workflow,
            commands::workflow::empty_recycle_bin,
            commands::workflow::save_workflow_dag,
            commands::workflow::start_workflow,
            commands::workflow::cancel_workflow,
            commands::workflow::delete_executions,
            commands::workflow::get_execution,
            commands::workflow::list_executions,
            commands::workflow::get_node_executions,
            commands::workflow::probe_node_output_fields,
            commands::workflow::respond_human_input,
            commands::workflow::respond_plugin_execute,
            commands::workflow::read_file_content,
            commands::workflow::list_node_types,
            commands::workflow::create_schedule,
            commands::workflow::list_schedules,
            commands::workflow::set_schedule_enabled,
            commands::workflow::delete_schedule,
            commands::workflow::list_workflow_events,
            commands::workflow::export_workflow_to_file,
            commands::workflow::export_workflows_to_file,
            commands::workflow::import_workflow_from_file,
            commands::workflow::get_workflow_stats,
            commands::workflow::get_execution_timeline,
            commands::workflow::get_node_type_stats,
            commands::workflow::get_top_workflows,
            commands::workflow::get_top_errors,
            commands::workflow::get_workflow_max_concurrency,
            commands::workflow::set_workflow_max_concurrency,
            commands::workflow::set_workflow_max_subflow_depth,
            commands::workflow::duplicate_workflow,
            commands::workflow::list_workflow_versions,
            commands::workflow::save_workflow_version,
            commands::workflow::restore_workflow_version,
            commands::workflow::delete_workflow_version,
            commands::workflow::get_node_execution_logs,
            commands::workflow::list_recoverable_executions,
            commands::workflow::list_live_executions,
            commands::workflow::execute_workflow_mode,
            commands::workflow::get_execution_plan,
            commands::workflow::validate_workflow,
            commands::workflow::get_pending_human_inputs,
            commands::workflow::get_pending_tool_approvals,
            commands::workflow::respond_tool_approval,
            commands::workflow::check_subflow_cycle,
            commands::agents::upload_agent_icon,
            commands::agents::read_agent_icon,
            commands::groupchat::groupchat_create_room,
            commands::groupchat::groupchat_join_room,
            commands::groupchat::groupchat_delete_room,
            commands::groupchat::groupchat_remove_participant,
            commands::groupchat::groupchat_send_message,
            commands::groupchat::groupchat_respond_confirmation,
            commands::groupchat::groupchat_set_director,
            commands::groupchat::groupchat_set_output_dir,
            commands::groupchat::groupchat_pause,
            commands::groupchat::groupchat_resume,
            commands::groupchat::groupchat_abort,
            commands::groupchat::groupchat_room_actor_alive,
            commands::groupchat::groupchat_get_room,
            commands::groupchat::groupchat_get_messages,
            commands::groupchat::groupchat_message_watermark,
            commands::groupchat::groupchat_get_tasks,
            commands::groupchat::groupchat_task_skip,
            commands::groupchat::groupchat_task_add,
            commands::groupchat::groupchat_task_update_deps,
            commands::groupchat::groupchat_task_deps_preview,
            commands::groupchat::groupchat_list_rooms,
            commands::groupchat::groupchat_get_participants,
            commands::groupchat::groupchat_get_stances,
            commands::groupchat::groupchat_export_workflow,
            commands::groupchat::groupchat_promote_workflow,
            commands::session::session_export_workflow,
            commands::session::session_promote_workflow,
            commands::session::session_export_room_plan,
            commands::session::session_promote_to_room,
            commands::session::session_extract_tasks,
            terminal::commands::terminal_create,
            terminal::commands::terminal_write,
            terminal::commands::terminal_close,
            terminal::commands::terminal_resize,
            terminal::commands::terminal_list,
            terminal::commands::terminal_attach,
            terminal::commands::terminal_get_config,
            commands::account::account_login_begin,
            commands::account::account_login_complete,
            commands::account::account_status,
            commands::account::account_logout,
            commands::account::account_platform_base,
            commands::org_share::org_list_mine,
            commands::org_share::org_list_shared_workflows,
            commands::org_share::org_share_workflow,
            commands::org_share::org_import_workflow,
            commands::org_credentials::org_list_credential_providers,
            commands::org_credentials::org_apply_credential,
            commands::org_credentials::org_report_credential,
            utils::market::fetch_agents_config,
            utils::market::inspiration_market_index,
            utils::market::inspiration_market_fetch,
            utils::market::workflow_market_index,
            utils::market::workflow_market_installs,
            utils::market::workflow_market_install,
        ])
        .setup(move |app| {
            // 初始化资源路径（应用根目录：Win %APPDATA%\PilotDesk / unix ~/.config/pilotdesk）
            let builtin = app
                .path()
                .resource_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("resources"));
            let user = crate::utils::paths::user_resources_dir();

            // 确保用户资源子目录存在（首次运行时创建）
            for sub in &["agents", "icons", "assets"] {
                let dir = user.join(sub);
                if !dir.exists() {
                    let _ = std::fs::create_dir_all(&dir);
                }
            }

            // 首启播种内置插件 / 技能（安装包携带 → 用户目录；已存在的不覆盖）
            seed_builtin_dir(&builtin, "plugins", &crate::utils::paths::plugins_dir());
            seed_builtin_dir(&builtin, "skills", &crate::utils::paths::skills_dir());

            app.manage(ResourcePaths { builtin, user });

            // 初始化共享 PluginHost（供插件 store 和工作流 NodeExecutor 共用）
            let shared_plugin_host = std::sync::Mutex::new(plugin::PluginHost::new());
            app.manage(shared_plugin_host);

            // 初始化 NodeExecutor（工作流节点执行器）
            let agent_manager = Arc::new(AsyncMutex::new(AgentManager::new()));
            // 创建 AgentManager shared 副本（持有相同 processes Arc，用于取消时绕过 AsyncMutex 锁）
            let agent_manager_shared = agent_manager.blocking_lock().shared();
            // NodeExecutor 通过 AppHandle 运行时访问 managed state 中的 PluginHost
            let node_executor = Arc::new(NodeExecutor::new(
                agent_manager,
                agent_manager_shared,
                app.handle().clone(),
                pool.clone(),
            ));
            // 初始同步：此时 PluginHost 为空，后续 plugin_discover 会触发再次同步
            node_executor.sync_plugin_node_types();
            app.manage(node_executor.clone());

            // 启动工作流定时调度器
            let scheduler = WorkflowScheduler::new(pool.clone());
            let sched_executor = node_executor.clone();
            let sched_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                scheduler.start(sched_executor, sched_handle).await;
            });

            // 一次性迁移：KV 记忆上限从 MEMORY.db 旧键搬到主库（幂等，仅首次真正搬迁）
            match pool.get() {
                Ok(conn) => {
                    if let Err(e) = commands::memory::migrate_memory_limit_to_main(&conn) {
                        log::warn!("[Memory] 记忆上限迁移检查失败（不影响使用）：{}", e);
                    }
                }
                Err(e) => log::warn!("[Memory] 取主库连接失败，跳过记忆上限迁移：{}", e),
            }

            // 启动本地用量自动上报后台任务（每 30 分钟一次，受设置开关控制，失败静默）
            let report_pool = pool.clone();
            tauri::async_runtime::spawn(async move {
                commands::usage::run_auto_report_loop(DbState { pool: report_pool }).await;
            });

            // 启动云同步后台任务（每 10 分钟一次；仅「开关开启 + 已登录 + 有能力」时真正执行，
            // 其余静默跳过，失败只记日志——不影响离线可用）
            let sync_pool = pool.clone();
            tauri::async_runtime::spawn(async move {
                commands::cloud_sync::run_auto_sync_loop(DbState { pool: sync_pool }).await;
            });

            log::info!("PilotDesk initialized successfully.");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use crate::api_agent::memory_intent::RetrievalTrace;

    fn trace(status: &'static str, reason: &str) -> RetrievalTrace {
        RetrievalTrace {
            status,
            model: "m".to_string(),
            categories: Vec::new(),
            keywords: Vec::new(),
            matched: Vec::new(),
            injected: false,
            reason: reason.to_string(),
        }
    }

    #[test]
    fn auto_retrieval_success_only_for_completed_flows() {
        assert!(crate::memory_intent_succeeded("matched"));
        assert!(crate::memory_intent_succeeded("no_keywords"));
        assert!(crate::memory_intent_succeeded("empty_input"));
        // 故障类状态必须显示为失败，不能沿用曾经的硬编码 success:true。
        assert!(!crate::memory_intent_succeeded("route_unavailable"));
        assert!(!crate::memory_intent_succeeded("route_unparsed"));
        assert!(!crate::memory_intent_succeeded("route_breaker_open"));
    }

    #[test]
    fn summary_surfaces_failure_reason_and_guidance() {
        let s = crate::kv_retrieval_summary(&trace(
            "route_unavailable",
            "请求失败（第 1 次，耗时 4002ms）: operation timed out",
        ));
        assert!(s.contains("本轮不注入记忆"));
        assert!(s.contains("原因：请求失败"));
        assert!(s.contains("search_memory"));
    }

    #[test]
    fn summary_omits_reason_and_guidance_when_flow_completed() {
        let s = crate::kv_retrieval_summary(&trace("no_keywords", ""));
        assert!(s.contains("未返回任何关键词"));
        assert!(s.contains("本轮未检索、不注入记忆"));
        assert!(!s.contains("原因："));
        assert!(!s.contains("search_memory"));
    }

    #[test]
    fn title_uses_system_step_wording_per_status() {
        assert_eq!(
            crate::memory_retrieval_title(&trace("route_unavailable", "x")),
            "自动记忆检索 · 检索不可用，本轮跳过"
        );
        assert_eq!(
            crate::memory_retrieval_title(&trace("no_keywords", "")),
            "自动记忆检索 · 未返回关键词，本轮不检索"
        );
        let mut hit = trace("matched", "");
        hit.matched = vec![
            crate::api_agent::memory_intent::RetrievalHit {
                source: "记忆·fact".to_string(),
                key: "a".to_string(),
            },
            crate::api_agent::memory_intent::RetrievalHit {
                source: "知识库·小说创作".to_string(),
                key: "b".to_string(),
            },
        ];
        hit.injected = true;
        assert_eq!(
            crate::memory_retrieval_title(&hit),
            "自动记忆检索 · 命中 2 条记忆 / 知识并注入"
        );
        // 摘要要带上来源：只报 key 的话，用户分不清哪条是资料、哪条是自己的记忆
        let detail = crate::kv_retrieval_summary(&hit);
        assert!(detail.contains("【记忆·fact】a"), "{}", detail);
        assert!(detail.contains("【知识库·小说创作】b"), "{}", detail);
    }

    #[test]
    fn params_surface_router_model_and_parsed_intent() {
        let mut hit = trace("matched", "");
        hit.model = "agnes-2.5-flash".to_string();
        hit.categories = vec!["preference".to_string()];
        hit.keywords = vec!["db".to_string(), "npm".to_string()];
        assert_eq!(
            crate::memory_retrieval_params(&hit),
            "模型=agnes-2.5-flash, 分类=preference, 关键词=db、npm"
        );

        // 未发起路由（空输入）时明确标注，避免被误读成"模型返回了空意图"。
        let mut not_called = trace("empty_input", "");
        not_called.model = String::new();
        assert_eq!(
            crate::memory_retrieval_params(&not_called),
            "模型=（未调用）, 分类=（空）, 关键词=（空）"
        );
    }
}
