mod agent;
mod api_agent;
mod commands;
mod db;
mod groupchat;
mod plugin;
mod tools;
mod workflow;
mod terminal;
mod utils;

use db::init::{init_db, DbPool};
use tokio::sync::Mutex as AsyncMutex;
use std::sync::Arc;
use agent::AgentManager;
use tauri::Manager;
use tauri::Emitter;
use rusqlite::params;
use rusqlite::OptionalExtension;
use workflow::executor::NodeExecutor;
use workflow::scheduler::WorkflowScheduler;
use api_agent::client::ApiClient;
use api_agent::agent_loop::{AgentLoop, AgentLoopConfig, ToolRegistry, RiskLevel};
use api_agent::agent_loop::ToolHandler;
use api_agent::types::*;
use api_agent::system_prompt::{SystemPromptBuilder, GitContext};
use api_agent::skills::SkillLoader;
use api_agent::context::{SlidingWindow, DEFAULT_CONTEXT_TOKENS, infer_context_window};
use api_agent::summarize::{split_recent_window, generate_rolling_summary};
use api_agent::db::MemoryStore;
use crate::db::models::Attachment;
use std::os::windows::process::CommandExt;

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
                MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0)
            };
            if len > 0 {
                let mut buf: Vec<u16> = vec![0; len as usize];
                unsafe {
                    let written = MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, buf.as_mut_ptr(), len);
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

/// 检测命令中是否包含危险操作（供 write_file 脚本内容检查使用）
fn check_dangerous_command(cmd: &str) -> Option<&'static str> {
    let lower = cmd.to_lowercase();
    let blocked: &[&str] = &[
        "format c:", "format d:", "format e:", "format /",
        "del /f /s c:\\", "del /f /s d:\\",
        "rmdir /s c:\\", "rmdir /s d:\\",
        "rd /s c:\\", "rd /s d:\\",
        "taskkill /f /im wininit", "taskkill /f /im csrss", "taskkill /f /im lsass",
        "taskkill /f /im smss", "taskkill /f /im svchost",
        "shutdown /s", "shutdown /r",
        "bcdedit /delete", "diskpart",
    ];
    for pat in blocked {
        if lower.contains(pat) {
            return Some(pat);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRisk {
    /// 安全：读操作，直接执行，无需审批
    Safe,
    /// 中等：写/改操作，需用户确认
    Medium,
    /// 高危：系统破坏性操作，直接拦截
    Blocked,
}

/// 分析一条 Shell 命令，返回其风险等级。
/// 同时返回越界路径（若有），供调用方决定是否额外提示用户确认。
pub fn classify_command(cmd: &str) -> (CommandRisk, Option<String>) {
    let cmd_stripped = cmd.trim();
    if cmd_stripped.is_empty() {
        return (CommandRisk::Safe, None);
    }

    // ── 提取实际执行的命令名 ──
    let cmd_lower = cmd_stripped.to_lowercase();

    // 取"&&"/"||"/";之前或第一个单词作为主命令
    let primary_cmd = cmd_stripped
        .split(|c| c == '&' || c == '|' || c == ';')
        .next()
        .unwrap_or(cmd_stripped)
        .trim()
        .to_lowercase();

    // 去掉 cmd /c 等前缀
    let exe = if primary_cmd.starts_with("cmd /c") {
        primary_cmd["cmd /c".len()..].trim().to_string()
    } else {
        primary_cmd.clone()
    };

    // ── 危险黑名单：绝对拦截 ──
    let blocked_patterns: &[&str] = &[
        // 磁盘/分区
        "format c:", "format d:", "format e:", "format /",
        "diskpart", "clean all",
        // 批量删除系统
        "del /f /s c:\\", "del /f /s d:\\", "del /f /s e:\\",
        "del /f /s %systemdrive%", "del /f /s %windir%", "del /f /s %systemroot%",
        "rmdir /s c:\\", "rmdir /s d:\\",
        "rd /s c:\\", "rd /s d:\\",
        // 终止关键进程
        "taskkill /f /im wininit", "taskkill /f /im csrss", "taskkill /f /im lsass",
        "taskkill /f /im smss", "taskkill /f /im winlogon", "taskkill /f /im services",
        "taskkill /f /im svchost", "taskkill /f /im system", "taskkill /f /im idle",
        "taskkill /f /im explorer", "taskkill /f /im dwm", "taskkill /f /im spoolsv",
        "taskkill /f /im taskmgr",
        // 终止全部运行进程
        "taskkill /f /fi \"status eq running\"",
        "taskkill /f /fi \"session eq", "taskkill /f /fi \"username eq",
        // 注册表/启动项破坏
        "reg delete hklm", "reg delete hkey_local_machine",
        "reg delete hkcr", "reg delete hkey_classes_root",
        "reg add hklm\\system\\currentcontrolset\\control",
        "bcdedit /delete", "bcdedit /set {default}",
        "bootsect /nt60", "bootrec /fixmbr",
        // 关机/重启
        "shutdown /s", "shutdown /r", "shutdown /g",
        "shutdown /p", "shutdown /t 0",
        // 系统任务计划/用户账户破坏
        "schtasks /delete /tn \\microsoft", "schtasks /delete /f /tn \\microsoft",
        "net user administrator /delete",
        // 权限篡改系统目录
        "takeown /f c:\\windows", "takeown /f %windir%",
        "icacls c:\\windows /grant", "icacls %windir% /grant",
        "cacls c:\\windows", "cacls %windir%",
        // 覆写系统文件（高危重定向）
        "echo > c:\\", "echo > %windir%", "echo > %systemroot%",
        "> c:\\windows\\", "> %windir%\\", ">> c:\\windows\\", ">> %windir%\\",
    ];
    for pat in blocked_patterns {
        if cmd_lower.contains(pat) {
            return (CommandRisk::Blocked, None);
        }
    }

    // ── 工作目录越界检查 ──
    if let Some(workspace) = std::env::current_dir().ok().and_then(|p| p.canonicalize().ok()) {
        let workspace_str = workspace.to_string_lossy().to_lowercase();
        // 在命令字符串中检测绝对路径引用
        let path_candidates: Vec<&str> = cmd_stripped
            .split_whitespace()
            .filter(|w| w.starts_with("c:\\") || w.starts_with("d:\\") || w.starts_with("e:\\"))
            .collect();
        for candidate in path_candidates {
            if let Ok(abs) = std::path::Path::new(candidate).canonicalize() {
                let abs_str = abs.to_string_lossy().to_lowercase();
                if !abs_str.starts_with(&workspace_str) {
                    return (CommandRisk::Medium, Some(format!(
                        "命令引用的路径 {} 超出工作区目录，是否仍要执行？", candidate
                    )));
                }
            }
        }
    }

    // ── 中等风险：写入/修改/删除操作 ──
    let medium_patterns: &[&str] = &[
        " del ", " rd ", " rmdir ",
        " copy ", " xcopy ", " move ", " robocopy ",
        " ren ", " rename ", " attrib ", " mklink ",
        " > ", " >> ",
        "reg add ", "reg delete ", "reg import ",
        "schtasks /create", "schtasks /change",
        "net user ", "net localgroup ",
        "powershell", "powershell.exe",
        "start ",
    ];
    for pat in medium_patterns {
        if cmd_lower.contains(pat) {
            return (CommandRisk::Medium, None);
        }
    }

    // ── 安全白名单：只读/信息类操作 ──
    let safe_tokens: &[&str] = &[
        "dir", "type", "echo", "findstr", "find ",
        "where", "which", "assoc", "ftype",
        "tasklist", "taskmgr", "systeminfo", "ver",
        "vol", "fsutil", "fsstor",
        "ipconfig", "ping", "nslookup", "tracert",
        "netstat", "route", "net ",
        "sc query", "sc config",
        "title", "set", "cls", "color",
        "tree", "fc", "certutil", "signtool",
        "git ", "npm ", "pip ", "python ", "py ",
        "curl ", "wget ",
    ];
    let first_word = exe.split_whitespace().next().unwrap_or("");
    for safe in safe_tokens {
        if first_word == safe.trim_end_matches(' ') || first_word.starts_with(safe.trim_end_matches(' ')) {
            return (CommandRisk::Safe, None);
        }
    }

    // 未匹配任何规则 → 保守判定为中等风险
    (CommandRisk::Medium, None)
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

impl DbState {
    pub fn get_conn(&self) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, crate::utils::errors::AppError> {
        self.pool.get().map_err(|e| crate::utils::errors::AppError::Lock(format!("数据库连接获取失败: {}", e)))
    }
}

// ── 数据库命令 ──

#[tauri::command]
fn list_tags(state: tauri::State<'_, DbState>) -> Result<Vec<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::list_tags(&conn)
}

#[tauri::command]
fn list_api_providers(state: tauri::State<'_, DbState>) -> Result<Vec<commands::api_provider::ApiProvider>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::list_api_providers(&conn)
}

#[tauri::command]
fn get_inspiration(state: tauri::State<'_, DbState>, id: String) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::get_inspiration(&conn, id)
}

#[tauri::command]
fn create_inspiration(state: tauri::State<'_, DbState>, payload: commands::inspiration::CreateInspirationPayload) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::create_inspiration(&conn, payload)
}

#[tauri::command]
fn update_inspiration(state: tauri::State<'_, DbState>, payload: commands::inspiration::UpdateInspirationPayload) -> Result<commands::inspiration::Inspiration, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::inspiration::update_inspiration(&conn, payload)
}

#[tauri::command]
fn delete_inspiration(state: tauri::State<'_, DbState>, id: String) -> Result<(), crate::utils::errors::AppError> {
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
fn get_api_provider(state: tauri::State<'_, DbState>, id: String) -> Result<Option<commands::api_provider::ApiProvider>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::get_api_provider(&conn, &id)
}

#[tauri::command]
fn upsert_api_provider(state: tauri::State<'_, DbState>, payload: commands::api_provider::CreateOrUpdateProvider) -> Result<commands::api_provider::ApiProvider, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::upsert_api_provider(&conn, &payload)
}

#[tauri::command]
fn delete_api_provider(state: tauri::State<'_, DbState>, id: String) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::delete_api_provider(&conn, &id)
}

#[tauri::command]
fn get_api_key(state: tauri::State<'_, DbState>, id: String) -> Result<Option<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::get_api_key(&conn, &id)
}

#[tauri::command]
fn reorder_api_providers(state: tauri::State<'_, DbState>, ids: Vec<String>) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::api_provider::reorder_api_providers(&conn, &ids)
}

#[tauri::command]
fn get_app_setting(state: tauri::State<'_, DbState>, key: String) -> Result<Option<String>, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::app_settings::get_setting(&conn, &key)
}

#[tauri::command]
fn set_app_setting(state: tauri::State<'_, DbState>, key: String, value: String) -> Result<(), crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::app_settings::set_setting(&conn, &key, &value)
}

#[tauri::command]
fn get_theme(state: tauri::State<'_, DbState>) -> Result<String, crate::utils::errors::AppError> {
    let conn = state.get_conn()?;
    commands::theme::get_theme(&conn)
}

#[tauri::command]
fn set_theme_cmd(state: tauri::State<'_, DbState>, theme: String) -> Result<String, crate::utils::errors::AppError> {
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
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    attachments: Option<Vec<Attachment>>,
) -> Result<(), String> {
    // ── API Agent 路径：使用 AgentLoop ──
    if agent_type == "api" {
        log::info!("[API Agent] 收到消息: session={}, msg_len={}, attachments={}, model_prompt={}",
            session_id, message.len(), attachments.as_ref().map_or(0, |v| v.len()), system_prompt.as_deref().unwrap_or("(none)").len());
        let app_clone = app.clone();
        let pending = app.try_state::<PendingApprovals>()
            .ok_or("PendingApprovals 状态未初始化")?.inner().clone();
        let attachments = attachments.unwrap_or_default();
        return run_api_agent(
            app_clone, &state, &session_id, &message, &attachments,
            &system_prompt.unwrap_or_default(),
            pending,
            temperature,
            max_tokens,
        ).await;
    }

    // ── CLI Agent 路径（原有逻辑）──
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let config = commands::agents::get_agent_inner(&conn, &agent_type)
        .map_err(|e| format!("查询 Agent 配置失败: {}", e))?
        .ok_or_else(|| format!("未知 Agent 类型: {}", agent_type))?;
    // cwd 为空时统一使用全局工作区路径
    let resolved_cwd = if cwd.as_deref().map_or(true, |s| s.is_empty()) {
        Some(crate::utils::paths::resolve_workspace_path(None, "", &conn)
            .to_string_lossy().to_string())
    } else {
        cwd
    };
    let mut mgr = agent_mgr.lock().await;
    mgr.send_message_with_config(app, session_id, config, message, mode, resolved_cwd, system_prompt, agent_session_id).await
}

#[tauri::command]
async fn agent_stop_generation(
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    session_id: String,
) -> Result<(), String> {
    let mut mgr = agent_mgr.lock().await;
    mgr.stop_generation(&session_id);
    Ok(())
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
        log::info!("[Approval] session={}, call={}, approved={}", session_id, call_id, approved);
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
        log::info!("[ContinueLoop] session={}, should_continue={}", session_id, should_continue);
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
        log::info!("[AskUser] session={}, call={} 已收到用户回复", session_id, call_id);
    } else {
        log::warn!("[AskUser] 未找到确认请求（可能已超时）: session={}, call={}", session_id, call_id);
    }
    Ok(())
}

/// 待审批工具调用管理（使用 tokio::sync::oneshot 避免阻塞工作线程）
#[derive(Clone)]
pub struct PendingApprovals {
    approvals: Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
    /// 迭代上限确认请求（session_id → sender）
    continue_reqs: Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
    /// ask_user 工具确认请求（call_id → 用户回复文本 sender）
    confirmation_reqs: Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<String>>>>,
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
        self.continue_reqs.lock().unwrap().insert(session_id.to_string(), tx);
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
async fn agent_list_skills(state: tauri::State<'_, DbState>, agent_type: String) -> Result<Vec<crate::db::models::SkillInfo>, String> {
    // API Agent 的技能目录独立于 DB 配置（~/.pilotdesk/skills/），使用 SkillLoader 扫描
    if agent_type == "api" {
        let loader = SkillLoader::new(get_api_agent_skills_dir());
        let skills = loader.list_skills()
            .into_iter()
            .map(|e| crate::db::models::SkillInfo::new(&e.name, &e.description, ""))
            .collect();
        return Ok(skills);
    }

    let config = state.get_conn()
        .ok()
        .and_then(|conn| commands::agents::get_agent_inner(&conn, &agent_type).ok()?)
        ;
    Ok(agent::AgentManager::list_skills(&agent_type, config.as_ref()).await)
}

// ════════════════════════════════════════════════════════════
// API Agent 执行（AgentLoop 编排）
// ════════════════════════════════════════════════════════════

/// 获取 API Agent 技能目录路径
/// 返回 ~/.pilotdesk/skills/ 如果存在
fn get_api_agent_skills_dir() -> Option<String> {
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
///   同时记录落盘路径，供图生图工具（edit_image / image_variation）引用；
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
                    let mime = if att.mime.is_empty() { "image/png" } else { att.mime.as_str() };
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
            "用户附加了以下图片，如需图生图可调用 edit_image / image_variation，其 image 参数可填这些路径：\n{}",
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

/// 使用 AgentLoop 执行 API Agent 对话
async fn run_api_agent(
    app: tauri::AppHandle,
    state: &DbState,
    session_id: &str,
    message: &str,
    attachments: &[Attachment],
    system_prompt: &str,
    pending_approvals: PendingApprovals,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;

    // 0. 加载持久化权限规则（allow/deny 列表）
    let permission_rules = commands::permission::load_rules(&conn).unwrap_or_default();

    // 1. 加载会话信息（获取 api_provider 和 api_model）
    let session = commands::session::get_session_inner(&conn, session_id)
        .map_err(|e| format!("查询会话失败: {}", e))?
        .ok_or_else(|| "会话不存在".to_string())?;

    // 1.1 API Agent 生成并持久化 agent_session_id（供外部系统恢复会话）
    let _agent_session_id = if session.agent_session_id.is_none() {
        let generated = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "UPDATE sessions SET agent_session_id = ?1 WHERE id = ?2",
            params![generated, session_id],
        ).map_err(|e| format!("保存 agent_session_id 失败: {}", e))?;
        log::info!("[API Agent] 生成 agent_session_id: {} -> {}", session_id, generated);
        // 通知前端更新会话状态
        let _ = app.emit("agent-session", serde_json::json!({
            "sessionId": session_id,
            "agentSessionId": generated,
        }));
        generated
    } else {
        session.agent_session_id.clone().unwrap()
    };

    let provider_id = session.api_provider.clone()
        .ok_or_else(|| "API 会话缺少提供商配置".to_string())?;
    let model = session.api_model.clone()
        .ok_or_else(|| "API 会话缺少模型配置".to_string())?;

    // 2. 获取 API 提供商配置
    let provider = commands::api_provider::get_api_provider(&conn, &provider_id)
        .map_err(|e| format!("查询提供商失败: {}", e))?
        .ok_or_else(|| format!("API 提供商不存在: {}", provider_id))?;

    let api_key = commands::api_provider::get_api_key(&conn, &provider_id)
        .map_err(|e| format!("获取 API Key 失败: {}", e))?
        .ok_or_else(|| format!("API Key 未配置: {}", provider_id))?;

    // 3. 加载技能（Progressive Disclosure: 先注入 name+description）
    let skills_dir = get_api_agent_skills_dir();
    let skill_loader = Arc::new(SkillLoader::new(skills_dir));

    // 3.5 初始化记忆库（SQLite: pilotdesk_agent.db，首次自动从 memories.json 迁移）
    let memory_store = {
        let config_dir = crate::api_agent::system_prompt::get_pilotdesk_config_dir()
            .ok_or_else(|| "无法获取配置目录".to_string())?;
        MemoryStore::new(&config_dir)?
    };
    let memory_store = Arc::new(memory_store);

    // 4. 组装 System Prompt（Base + MEMORY.md + USER.md + Skill 列表 + KV 记忆）
    let api_agent_base_prompt = concat!(
        "<agent_role>\n",
        "你是一个智能编程助手（PilotDesk Agent）。你可以：\n",
        "1. 使用常识和内置知识直接回答一般性问题（如日期、常识、编程概念等）——无需调用任何工具\n",
        "2. 调用 read_file 读取文件内容——安全低风险，无需审批\n",
        "3. 调用 list_files 列出目录内容——安全低风险，无需审批\n",
        "4. 调用 write_file 创建文件（脚本、代码、配置等）——写入完成后告知用户文件路径\n",
        "5. 调用 execute_command 执行命令——高风险操作，需用户确认\n",
        "6. 调用 search_memory 查找用户保存的偏好和项目事实——仅在需要了解用户背景时使用\n",
        "7. 调用 load_skill 加载特定技能——仅在确定需要该技能执行任务时使用\n",
        "8. 调用 save_memory 保存重要信息供后续对话使用\n",
        "9. 调用 web_search 联网搜索（默认 Bing 中国版，返回标题/链接/摘要）——低风险\n",
        "10. 调用 web_fetch 抓取指定网页的正文文本——低风险\n",
        "11. 调用 browser 工具访问网页（action=fetch 抓取渲染后页面 / action=screenshot 截图）——中风险需确认\n",
        "12. 调用 generate_image 根据文字描述生成图片（仅 OpenAI 兼容提供商可用）\n",
        "13. 调用 edit_image 编辑已有图片（图生图，需提供图片路径/URL，仅 OpenAI 兼容提供商可用）\n",
        "14. 调用 image_variation 生成已有图片的风格变体（图生图，仅 OpenAI 兼容提供商可用）\n",
        "15. 调用 task 把独立子任务交给子代理处理（调研、分析、规划、写作等）\n",
        "\n",
        "重要规则：\n",
        "- 优先使用你的内置知识回答问题，不要为了使用工具而使用工具\n",
        "- 优先使用 read_file 查看文件内容，list_files 浏览目录——不要用 execute_command 做这些\n",
        "- 如果用户的问题是常识性的（如询问人物、民科问题、日期、编程语法等），请直接回答，不要调用任何工具\n",
        "- 如果用户要求你创建文件（脚本、代码、文档），务必调用 write_file 工具来完成\n",
        "- 写入文件时请使用 <environment> 中提供的真实路径，不要猜测用户名\n",
        "- execute_command 仅用于运行脚本、git 操作、系统信息查询等真正需要执行的场景\n",
        "- search_memory 仅用于查找用户之前保存的个性化信息，不是通用搜索引擎\n",
        "- 当需要最新信息、实时数据或事实核查时，使用 web_search 联网搜索；无法联网获取时如实告知用户\n",
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
        "</environment>"
    );

    // 注入真实路径信息，避免 LLM 猜测错误的用户名
    let home = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "未知".to_string());
    let desktop = dirs::desktop_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{}\\Desktop", home));
    let cwd_display = if session.cwd.is_empty() { "未知".to_string() } else { session.cwd.clone() };

    let api_agent_base_prompt = api_agent_base_prompt
        .replace("{HOME}", &home)
        .replace("{DESKTOP}", &desktop)
        .replace("{CWD}", &cwd_display);

    let full_base_prompt = if system_prompt.is_empty() {
        api_agent_base_prompt.to_string()
    } else {
        format!("{}\n\n{}", api_agent_base_prompt, system_prompt)
    };

    let full_system_prompt = {
        let mut builder = SystemPromptBuilder::new(full_base_prompt)
            .with_memory_md(Some(&session.cwd).filter(|c| !c.is_empty()).map(|c| c.as_str()))
            .with_user_md()
            .with_kv_memories(memory_store.format_for_prompt(5))
            .with_skills(skill_loader.list_skills());

        // Git 仓库上下文
        if !session.cwd.is_empty() {
            if let Some(git) = GitContext::from_cwd(&session.cwd) {
                builder = builder.with_git_context(&git);
            }
        }

        // 项目上下文文件（CLAUDE.md、README.md）
        if !session.cwd.is_empty() {
            builder = builder.with_project_context(&session.cwd);
        }

        builder.build()
    };

    // 5. 加载会话消息历史（用于首次请求时的回退）
    let history = commands::session::get_session_messages_inner(&conn, session_id)
        .map_err(|e| format!("加载消息历史失败: {}", e))?;

    // 5.5 会话连续性：加载持久化上下文快照（滚动摘要 + 最近 N 轮完整对话）
    // 注意：session_contexts 不存 system prompt，system prompt 每次动态构建。
    let (saved_summary, saved_recent): (String, Vec<ChatMessage>) = {
        let ctx_sql = "SELECT conversation_messages, summary FROM session_contexts WHERE session_id = ?1";
        match conn.query_row(ctx_sql, params![session_id], |row| {
            let recent_json: String = row.get(0)?;
            let summary: String = row.get(1)?;
            Ok((recent_json, summary))
        }).optional() {
            Ok(Some((recent_json, summary))) => {
                let recent: Vec<ChatMessage> = serde_json::from_str(&recent_json).unwrap_or_default();
                (summary, recent)
            }
            _ => (String::new(), Vec::new()),
        }
    };

    let mut messages: Vec<ChatMessage> = Vec::new();

    // 注入滚动摘要（独立 system 消息，紧随主 system prompt 之后）
    if !saved_summary.is_empty() {
        messages.push(ChatMessage::system(&format!(
            "<conversation_summary>\n{}\n</conversation_summary>",
            saved_summary
        )));
    }

    if saved_recent.is_empty() {
        // 首次请求：从 messages 表构建历史（仅 user/assistant 角色）
        let mut msgs: Vec<ChatMessage> = history
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| match m.role.as_str() {
                "assistant" => ChatMessage::assistant(&m.content),
                _ => ChatMessage::user(&m.content),
            })
            .collect();

        // 若当前用户消息已被前端 fire-and-forget 持久化，去掉末尾重复项
        if let Some(last) = msgs.last() {
            if last.role == "user" && last.content.as_deref() == Some(message) {
                msgs.pop();
            }
        }
        messages.extend(msgs);
    } else {
        messages.extend(saved_recent);
    }

    // 追加当前用户消息（仅一次，避免重复）
    // 图片附件转 base64 送入多模态；文件附件在正文中追加路径说明，供模型用 read_file 读取。
    let (images, file_note) = split_attachments(attachments);
    let mut user_content = message.to_string();
    if !file_note.is_empty() {
        user_content.push_str(&file_note);
    }
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
    let client = if matches!(api_format, ApiFormat::Anthropic) {
        ApiClient::new(provider.api_endpoint, api_key, api_format.clone())
    } else {
        ApiClient::new_openai(provider.api_endpoint, api_key)
    };
    // 摘要生成复用一个独立客户端实例（AgentLoop 会独占消费 client）
    let summary_client = client.clone();
    let summary_model = model.clone();
    let summary_format = api_format.clone();

    // 7. 构建工具注册表（含 load_skill 等内置工具）
    let mut tool_registry = ToolRegistry::new();
    let skill_loader_clone = skill_loader.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::skills::LoadSkillTool::new(skill_loader_clone),
    ));

    // 注册 KV 记忆工具（实现见 tools/memory.rs）
    let memory_clone = memory_store.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::memory::SaveMemoryTool::new(memory_clone),
    ));

    let memory_clone2 = memory_store.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::memory::SearchMemoryTool::new(memory_clone2),
    ));

    // 注册读文件工具（Low 风险 — 只读，不修改任何文件；工具实现见 tools/read_file.rs）
    let cwd_for_read = session.cwd.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::read_file::ReadFileTool::new(cwd_for_read),
    ));

    // 注册列出文件工具（Low 风险 — 只读；工具实现见 tools/list_files.rs）
    let cwd_for_list = session.cwd.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::list_files::ListFilesTool::new(cwd_for_list),
    ));

    // 注册 Glob 工具（Low 风险 — 文件名/路径模式匹配；工具实现见 tools/glob.rs）
    let cwd_for_glob = session.cwd.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::glob::GlobTool::new(cwd_for_glob),
    ));

    // 注册 Grep 工具（Low 风险 — 内容搜索；工具实现见 tools/grep.rs）
    let cwd_for_grep = session.cwd.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::grep::GrepTool::new(cwd_for_grep),
    ));

    // 注册写文件工具（Medium 风险 — 修改文件需确认；工具实现见 tools/write_file.rs）
    let cwd_for_write = session.cwd.clone(); // 工作目录（用于路径解析）
    let app_for_write = app.clone();
    let sid_for_write = session_id.to_string();
    let pool_for_write = state.pool.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::write_file::WriteFileTool::new(cwd_for_write).with_hooks(
            crate::tools::write_file::WriteFileHooks {
                app: app_for_write,
                session_id: sid_for_write,
                pool: pool_for_write,
            },
        ),
    ));

    // 注册 Edit 工具（精确字符串替换，与 write_file 同风险；工具实现见 tools/edit_file.rs）
    let cwd_for_edit = session.cwd.clone();
    let app_for_edit = app.clone();
    let sid_for_edit = session_id.to_string();
    let pool_for_edit = state.pool.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::edit_file::EditFileTool::new(cwd_for_edit).with_hooks(
            crate::tools::write_file::WriteFileHooks {
                app: app_for_edit,
                session_id: sid_for_edit,
                pool: pool_for_edit,
            },
        ),
    ));

    // 注册命令执行工具（High 风险，每次强制用户确认；工具实现见 tools/execute_command.rs）
    let cwd_for_exec = session.cwd.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::execute_command::ExecuteCommandTool::new(cwd_for_exec),
    ));

    // 注册 Python 代码执行工具（Low 风险 — 仅执行代码，副作用由 Python 脚本自行决定；实现见 tools/execute_python.rs）
    let cwd_for_py = session.cwd.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::execute_python::ExecutePythonTool::new(cwd_for_py),
    ));

    // 注册 TodoWrite 工具（Low 风险 — 会话内任务追踪，无文件副作用；实现见 tools/todo_write.rs）
    tool_registry.register(std::sync::Arc::new(
        crate::tools::todo_write::TodoWriteTool::new(),
    ));

    // 图片生成 / 图生图工具（仅 OpenAI 兼容格式，复用当前 provider 的 endpoint/key）
    if !matches!(api_format, ApiFormat::Anthropic) {
        tool_registry.register(std::sync::Arc::new(
            crate::tools::image_gen::ImageGenTool::new(image_endpoint.clone(), image_api_key.clone()),
        ));
        tool_registry.register(std::sync::Arc::new(
            crate::tools::image_to_image::ImageEditTool::new(image_endpoint.clone(), image_api_key.clone()),
        ));
        tool_registry.register(std::sync::Arc::new(
            crate::tools::image_to_image::ImageVariationTool::new(image_endpoint, image_api_key),
        ));
    }

    // 注册 MCP 服务器工具（stdio 子进程，握手后暴露 tools）
    let mcp_servers = commands::mcp::load_servers(&conn).unwrap_or_default();
    for server in &mcp_servers {
        let mut client = match crate::tools::mcp::McpClient::connect(server).await {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[MCP] 连接服务器 {} 失败: {}", server.name, e);
                continue;
            }
        };
        let tools = match client.list_tools().await {
            Ok(t) => t,
            Err(e) => {
                log::warn!("[MCP] 列出工具失败 {}: {}", server.name, e);
                continue;
            }
        };
        let tool_count = tools.len();
        let shared = std::sync::Arc::new(tokio::sync::Mutex::new(client));
        for info in tools {
            tool_registry.register(std::sync::Arc::new(
                crate::tools::mcp::McpToolHandler::new(shared.clone(), &server.name, info),
            ));
        }
        log::info!("[MCP] 已加载服务器 {}（{} 个工具）", server.name, tool_count);
    }

    // 注册子代理 task 工具（单次 LLM 调用，聚焦子任务）
    tool_registry.register(std::sync::Arc::new(
        crate::tools::subagent::TaskTool::new(client.clone(), model.clone(), api_format.clone()),
    ));

    // 注册浏览器自动化工具（本机 Edge/Chrome 无头模式，抓取/截图）
    tool_registry.register(std::sync::Arc::new(
        crate::tools::browser::BrowserTool::new(session.cwd.clone()),
    ));

    // 注册联网搜索工具（默认 Bing 中国版，可在设置中切换 Tavily/Bing API）
    let search_config = commands::search::load_search_config(&conn);
    tool_registry.register(std::sync::Arc::new(
        crate::tools::web_search::WebSearchTool::new(search_config),
    ));

    // 注册网页抓取工具
    tool_registry.register(std::sync::Arc::new(
        crate::tools::web_fetch::WebFetchTool::new(),
    ));

    // 注册 ask_user 工具（模型向用户发起确认请求；确认通道与审批共用 PendingApprovals）
    let ask_user_pending = pending_approvals.clone();
    tool_registry.register(std::sync::Arc::new(
        crate::tools::ask_user::AskUserTool::new(app.clone(), session_id.to_string(), ask_user_pending),
    ));

    let tool_registry = Arc::new(tool_registry);
    let tools = tool_registry.get_definitions();

    // 8. 创建 AgentLoop（直接发送 Tauri 事件到前端，确保审批前实时送达）
    let pending_shared = pending_approvals.clone();
    let app_for_agent = app.clone();
    let sid_for_agent = session_id.to_string();
    log::info!("[API Agent] 启动 AgentLoop: session={}, model={}", session_id, model);
    
    let agent_loop = AgentLoop::new(client, tool_registry, model, app_for_agent, sid_for_agent)
        .with_api_format(api_format)
        .with_permission_rules(permission_rules)
        .with_approval_handler(Box::new(move |call_id: &str, tool_name: &str, _args: &str, risk: RiskLevel| {
            let rx = pending_shared.register(call_id.to_string());
            
            log::info!("[Approval] 等待用户审批: tool={}, risk={:?}", tool_name, risk);
            
            // 使用 block_in_place 避免阻塞 tokio 工作线程（防止多次审批耗尽线程池导致崩溃）
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(120), rx).await
                })
            });
            match result {
                Ok(Ok(approved)) => {
                    log::info!("[Approval] 用户{}: {}", if approved { "批准" } else { "拒绝" }, tool_name);
                    approved
                }
                _ => {
                    log::warn!("[Approval] 审批超时，默认允许: {}", tool_name);
                    true
                }
            }
        }));

    // 迭代上限确认回调（复用 PendingApprovals 的 oneshot + block_in_place 模式）
    let pending_continue = pending_approvals.clone();
    let sid_continue = session_id.to_string();
    let agent_loop = agent_loop.with_continue_handler(Box::new(move |current, max| {
        log::info!("[ContinueLoop] 等待用户确认: {}/{}", current, max);
        let rx = pending_continue.register_continue(&sid_continue);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(120), rx).await
            })
        });
        match result {
            Ok(Ok(should_continue)) => {
                log::info!("[ContinueLoop] 用户{}", if should_continue { "继续执行" } else { "终止" });
                should_continue
            }
            _ => {
                log::warn!("[ContinueLoop] 超时，默认继续执行");
                true // 超时默认继续
            }
        }
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

    // 10. 后台执行 Agent Loop（AgentLoop 内部直接发射 Tauri 事件到前端）
    // 整体超时保护：180s，防止工具调用场景多轮往返导致会话永久挂起
    const AGENT_LOOP_TIMEOUT_SECS: u64 = 180;
    let sid_done = session_id.to_string();
    // 会话上下文持久化（AgentLoop 完成后保存滚动摘要 + 最近 N 轮，不存 system prompt）
    let pool_for_ctx = state.pool.clone();
    let session_id_for_ctx = session_id.to_string();
    let api_provider_id_for_ctx = provider_id.clone();
    let api_model_for_ctx = summary_model.clone();
    tokio::spawn(async move {
        match tokio::time::timeout(
            std::time::Duration::from_secs(AGENT_LOOP_TIMEOUT_SECS),
            agent_loop.run(config),
        )
        .await
        {
            Ok(Ok(output)) => {
                log::info!("[API Agent] 对话完成: session={}", sid_done);

                // 1. 双阈值切分：older（需语义压缩）+ recent（完整保留）
                let (older, recent) = split_recent_window(&output.messages);

                // 2. 读取旧摘要（增量式合并的基础）
                let (mut new_summary, mut summary_updated_at): (String, i64) = (String::new(), 0);
                if let Ok(ctx_conn) = pool_for_ctx.get() {
                    if let Ok(Some((s, ts))) = ctx_conn
                        .query_row(
                            "SELECT summary, summary_updated_at FROM session_contexts WHERE session_id = ?1",
                            params![&session_id_for_ctx],
                            |row| {
                                let s: String = row.get(0)?;
                                let ts: i64 = row.get(1)?;
                                Ok((s, ts))
                            },
                        )
                        .optional()
                    {
                        new_summary = s;
                        summary_updated_at = ts;
                    }
                }

                // 3. 存在超出保留窗口的早期消息时，触发增量摘要（复用当前会话模型）
                if !older.is_empty() {
                    match generate_rolling_summary(
                        &summary_client,
                        &summary_model,
                        &summary_format,
                        &new_summary,
                        &older,
                    )
                    .await
                    {
                        Ok(s) if !s.trim().is_empty() => {
                            new_summary = s;
                            summary_updated_at = crate::utils::now();
                        }
                        Ok(_) => {
                            log::warn!("[API Agent] 摘要生成为空，保留旧摘要");
                        }
                        Err(e) => {
                            log::warn!("[API Agent] 摘要生成失败，保留旧摘要: {}", e);
                        }
                    }
                }

                // 4. 持久化 summary + recent（不含 system prompt）
                let recent_json = serde_json::to_string(&recent).unwrap_or_default();
                let now = crate::utils::now();
                let _ = pool_for_ctx.get()
                    .map_err(|e| log::error!("获取数据库连接失败: {}", e))
                    .and_then(|ctx_conn| {
                        ctx_conn.execute(
                            "INSERT INTO session_contexts (session_id, api_provider_id, api_model, conversation_messages, summary, summary_updated_at, last_updated_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                             ON CONFLICT(session_id) DO UPDATE SET
                                 conversation_messages = ?4, summary = ?5, summary_updated_at = ?6, last_updated_at = ?7",
                            params![
                                &session_id_for_ctx,
                                &api_provider_id_for_ctx,
                                &api_model_for_ctx,
                                recent_json,
                                new_summary,
                                summary_updated_at,
                                now,
                            ],
                        ).map_err(|e| log::error!("保存会话上下文失败: {}", e))
                    });
            }
            Ok(Err(e)) => {
                log::error!("[API Agent] 对话失败: session={}, error={}", sid_done, e);
                let _ = app.emit("agent-error", serde_json::json!({
                    "sessionId": sid_done,
                    "error": e,
                }));
            }
            Err(_) => {
                let timeout_msg = format!(
                    "模型响应超时（超过 {} 秒），已自动终止。请重试或检查 API 提供商状态。",
                    AGENT_LOOP_TIMEOUT_SECS
                );
                log::warn!(
                    "[API Agent] AgentLoop 整体超时 ({:?}): session={}",
                    std::time::Duration::from_secs(AGENT_LOOP_TIMEOUT_SECS),
                    sid_done,
                );
                let _ = app.emit("agent-error", serde_json::json!({
                    "sessionId": sid_done,
                    "error": timeout_msg,
                }));
            }
        }
    });

    Ok(())
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
                        "node_modules" | "target" | ".git" | "dist" | "build" | "__pycache__" | "venv" | ".venv"
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
    for key in ["PYTHONHOME", "PYTHONPATH", "CONDA_PREFIX", "PYTHONNOUSERSITE"] {
        std::env::remove_var(key);
    }

    let pool = init_db().expect("Failed to initialize database");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        // dirindex 自定义协议：目录索引页（?path=<urlencoded 绝对路径>）
        .register_uri_scheme_protocol("dirindex", |_ctx, request| {
            commands::dirindex::handle_dirindex(request)
        })
                .manage(DbState { pool: pool.clone() })
        .manage(AsyncMutex::new(AgentManager::new()))
        .manage(PendingApprovals::new())
        .manage(groupchat::room::RoomRegistry::new())
        .manage(std::sync::Mutex::new(terminal::console_bridge::ConsoleBridge::new()))
        .manage(AsyncMutex::new(terminal::TerminalManager::new()))

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
            commands::session::archive_session,
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
            commands::permission::get_permission_rules,
            commands::permission::set_permission_rules,
            commands::search::get_search_config,
            commands::search::set_search_config,
            commands::mcp::get_mcp_servers,
            commands::mcp::set_mcp_servers,
            commands::file_history::list_file_history,
            commands::file_history::undo_file_history,
            commands::fs_util::write_text_file,
            get_theme,
            set_theme_cmd,
            agent_send_message_with_config,
            agent_stop_generation,
            agent_approve_tool,
            agent_continue_loop,
            agent_respond_confirmation,
            agent_create_session,
            agent_close_session,
            agent_list_skills,
            plugin::plugin_discover,
            plugin::plugin_list,
            plugin::plugin_enable,
            plugin::plugin_disable,
            plugin::store::read_plugin_readme,
            plugin::plugin_get_sandbox_info,
            plugin::plugin_install_zip,
            plugin::plugin_uninstall,
    plugin::plugin_set_sandbox_enabled,
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
            commands::workflow::save_workflow_dag,
            commands::workflow::start_workflow,
            commands::workflow::cancel_workflow,
            commands::workflow::delete_execution,
            commands::workflow::get_execution,
            commands::workflow::list_executions,
            commands::workflow::get_node_executions,
            commands::workflow::respond_human_input,
            commands::workflow::respond_plugin_execute,
            commands::workflow::read_file_content,
            commands::workflow::list_node_types,
            commands::workflow::create_schedule,
            commands::workflow::list_schedules,
            commands::workflow::delete_schedule,
            commands::workflow::export_workflow_to_file,
            commands::workflow::import_workflow_from_file,

            commands::workflow::get_workflow_stats,
            commands::workflow::get_execution_timeline,
            commands::workflow::get_node_type_stats,
            commands::workflow::get_top_workflows,
            commands::workflow::get_top_errors,
            commands::workflow::get_workflow_max_concurrency,
            commands::workflow::set_workflow_max_concurrency,
            commands::workflow::duplicate_workflow,
            commands::workflow::list_workflow_versions,
            commands::workflow::save_workflow_version,
            commands::workflow::restore_workflow_version,
            commands::workflow::delete_workflow_version,
            commands::workflow::get_node_execution_logs,
            commands::workflow::list_recoverable_executions,
            commands::workflow::execute_workflow_mode,
            commands::workflow::get_execution_plan,
            commands::workflow::validate_workflow,
            commands::workflow::get_pending_human_inputs,
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
            commands::groupchat::groupchat_pause,
            commands::groupchat::groupchat_resume,
            commands::groupchat::groupchat_abort,
            commands::groupchat::groupchat_get_room,
            commands::groupchat::groupchat_get_messages,
            commands::groupchat::groupchat_get_tasks,
            commands::groupchat::groupchat_list_rooms,
            commands::groupchat::groupchat_get_participants,
            commands::groupchat::groupchat_get_stances,
            commands::groupchat::groupchat_export_workflow,
            terminal::commands::terminal_create,
            terminal::commands::terminal_write,
            terminal::commands::terminal_close,
            terminal::commands::terminal_resize,
            terminal::commands::terminal_list,
            terminal::commands::terminal_attach,
            terminal::commands::terminal_get_config,
            utils::market::fetch_agents_config,
        ])
        .setup(move |app| {
            // 初始化资源路径（Windows: app_data_dir = %APPDATA%/com.pilotdesk.app/）
            let builtin = app.path().resource_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("resources"));
            let user = crate::utils::paths::user_resources_dir();

            // 确保用户资源子目录存在（首次运行时创建）
            for sub in &["agents", "icons", "assets"] {
                let dir = user.join(sub);
                if !dir.exists() {
                    let _ = std::fs::create_dir_all(&dir);
                }
            }

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

            log::info!("PilotDesk initialized successfully.");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
