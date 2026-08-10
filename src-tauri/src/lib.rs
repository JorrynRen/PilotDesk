mod agent;
mod api_agent;
mod commands;
mod db;
mod plugin;
mod workflow;
mod terminal;
mod virtual_console;
mod utils;

use db::init::{init_db, DbPool};
use tokio::sync::Mutex as AsyncMutex;
use std::sync::Arc;
use agent::AgentManager;
use tauri::Manager;
use tauri::Emitter;
use workflow::executor::NodeExecutor;
use workflow::scheduler::WorkflowScheduler;
use api_agent::client::ApiClient;
use api_agent::agent_loop::{AgentLoop, AgentLoopConfig, ToolRegistry, RiskLevel};
use api_agent::agent_loop::ToolHandler;
use api_agent::types::*;
use api_agent::system_prompt::SystemPromptBuilder;
use api_agent::skills::SkillLoader;
use api_agent::context::SlidingWindow;
use api_agent::context::DEFAULT_CONTEXT_TOKENS;
use api_agent::memory::MemoryStore;
use serde_json::json;

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
) -> Result<(), String> {
    // ── API Agent 路径：使用 AgentLoop ──
    if agent_type == "api" {
        let app_clone = app.clone();
        let pending = app.try_state::<PendingApprovals>()
            .ok_or("PendingApprovals 状态未初始化")?.inner().clone();
        return run_api_agent(
            app_clone, &state, &session_id, &message,
            &system_prompt.unwrap_or_default(),
            pending,
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

/// 待审批工具调用管理（线程安全，可跨任务共享）
#[derive(Clone)]
pub struct PendingApprovals {
    approvals: Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::mpsc::Sender<bool>>>>,
}

impl PendingApprovals {
    pub fn new() -> Self {
        Self { approvals: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())) }
    }

    pub fn register(&self, call_id: String) -> std::sync::mpsc::Receiver<bool> {
        let (tx, rx) = std::sync::mpsc::channel();
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

/// 使用 AgentLoop 执行 API Agent 对话
async fn run_api_agent(
    app: tauri::AppHandle,
    state: &DbState,
    session_id: &str,
    _message: &str,
    system_prompt: &str,
    pending_approvals: PendingApprovals,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;

    // 1. 加载会话信息（获取 api_provider 和 api_model）
    let session = commands::session::get_session_inner(&conn, session_id)
        .map_err(|e| format!("查询会话失败: {}", e))?
        .ok_or_else(|| "会话不存在".to_string())?;

    let provider_id = session.api_provider
        .ok_or_else(|| "API 会话缺少提供商配置".to_string())?;
    let model = session.api_model
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

    // 3.5 初始化记忆库（文件持久化: ~/.pilotdesk/memories.json）
    let memory_store = {
        let config_dir = crate::api_agent::system_prompt::get_pilotdesk_config_dir()
            .unwrap_or_else(|| {
                std::env::temp_dir().to_string_lossy().to_string()
            });
        MemoryStore::new(format!("{}/memories.json", config_dir))
    };
    let memory_store = Arc::new(memory_store);

    // 4. 组装 System Prompt（Base + MEMORY.md + USER.md + Skill 列表 + KV 记忆）
    let full_system_prompt = SystemPromptBuilder::new(system_prompt.to_string())
        .with_memory_md(Some(&session.cwd).filter(|c| !c.is_empty()).map(|c| c.as_str()))
        .with_user_md()
        .with_kv_memories(memory_store.format_for_prompt(5))
        .with_skills(skill_loader.list_skills())
        .build();

    // 5. 加载会话消息历史（仅 user/assistant 角色）
    let history = commands::session::get_session_messages_inner(&conn, session_id)
        .map_err(|e| format!("加载消息历史失败: {}", e))?;

    let messages: Vec<ChatMessage> = history
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| {
            match m.role.as_str() {
                "user" => ChatMessage::user(&m.content),
                "assistant" => ChatMessage::assistant(&m.content),
                _ => ChatMessage::user(&m.content),
            }
        })
        .collect();

    // 5.1 应用滑动窗口截断（超出 token 限制时保留最近消息）
    let window = SlidingWindow::new(DEFAULT_CONTEXT_TOKENS);
    let messages = window.trim(&messages);

    // 6. 创建 API 客户端
    let client = ApiClient::new(provider.api_endpoint, api_key);

    // 7. 构建工具注册表（含 load_skill 等内置工具）
    let mut tool_registry = ToolRegistry::new();
    let skill_loader_clone = skill_loader.clone();
    tool_registry.register(builtin_tool!(
        "load_skill",
        "加载指定技能的完整内容。当需要详细了解某个技能的使用方法时调用此工具。",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "技能名称（来自 available_skills 列表）"
                }
            },
            "required": ["name"]
        }),
        move |args| {
            let name = args["name"].as_str().ok_or("缺少 name 参数")?;
            skill_loader_clone
                .load_skill(name)
                .ok_or_else(|| format!("技能不存在: {}", name))
        }
    ));

    // 注册 KV 记忆工具
    let memory_clone = memory_store.clone();
    tool_registry.register(builtin_tool_risky!(
        "save_memory",
        "保存一条键值对记忆到持久化知识库。格式：key=名称，value=内容，category=分类（fact/preference/skill/event）",
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "记忆的键（唯一标识，如 project_language）"
                },
                "value": {
                    "type": "string",
                    "description": "记忆的值（要保存的内容）"
                },
                "category": {
                    "type": "string",
                    "description": "记忆分类：fact（事实）、preference（偏好）、skill（技能）、event（事件）",
                    "enum": ["fact", "preference", "skill", "event"]
                }
            },
            "required": ["key", "value", "category"]
        }),
        RiskLevel::Medium,
        move |args| {
            let key = args["key"].as_str().ok_or("缺少 key 参数")?;
            let value = args["value"].as_str().ok_or("缺少 value 参数")?;
            let category = args["category"].as_str().ok_or("缺少 category 参数")?;
            let entry = memory_clone.save_memory(key, value, category);
            Ok(format!("记忆已保存: [{}] {} = {}", entry.category, entry.key, entry.value))
        }
    ));

    let memory_clone2 = memory_store.clone();
    tool_registry.register(builtin_tool!(
        "search_memory",
        "在记忆库中搜索键值对。返回匹配的所有记忆，按访问频率排序。",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词（匹配 key 和 value）"
                }
            },
            "required": ["query"]
        }),
        move |args| {
            let query = args["query"].as_str().ok_or("缺少 query 参数")?;
            let results = memory_clone2.search_memory(query);
            if results.is_empty() {
                Ok("未找到匹配的记忆。".to_string())
            } else {
                let formatted: Vec<String> = results
                    .iter()
                    .map(|e| format!("- [{}] {}: {}", e.category, e.key, e.value))
                    .collect();
                Ok(format!("找到 {} 条记忆：\n{}", results.len(), formatted.join("\n")))
            }
        }
    ));

    let tool_registry = Arc::new(tool_registry);
    let tools = tool_registry.get_definitions();

    // 8. 创建 AgentLoop（含审批处理）
    let pending_shared = pending_approvals;
    
    let agent_loop = AgentLoop::new(client, tool_registry, model)
        .with_approval_handler(Box::new(move |call_id: &str, tool_name: &str, _args: &str, risk: RiskLevel| {
            let rx = pending_shared.register(call_id.to_string());
            
            log::info!("[Approval] 等待用户审批: tool={}, risk={:?}", tool_name, risk);
            
            // 等待前端审批（阻塞当前任务，超时120秒后默认拒绝）
            match rx.recv_timeout(std::time::Duration::from_secs(120)) {
                Ok(approved) => {
                    log::info!("[Approval] 用户{}: {}", if approved { "批准" } else { "拒绝" }, tool_name);
                    approved
                }
                Err(_) => {
                    log::warn!("[Approval] 审批超时，默认拒绝: {}", tool_name);
                    false
                }
            }
        }));

    // 9. 配置 Agent Loop
    let config = AgentLoopConfig {
        max_iterations: 10,
        system_prompt: full_system_prompt,
        tools,
        messages,
    };

    // 10. 创建广播通道（AgentLoop → Tauri 事件转换）
    let (tx, mut rx) = tokio::sync::broadcast::channel::<AgentLoopEvent>(100);

    // 11. 启动事件监听器（将 AgentLoop 事件转换为 Tauri 事件）
    let app_listener = app.clone();
    let sid_listener = session_id.to_string();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match event {
                AgentLoopEvent::Chunk { content } => {
                    let _ = app_listener.emit("agent-chunk", json!({
                        "sessionId": sid_listener,
                        "content": content,
                    }));
                }
                AgentLoopEvent::ToolStart { id, name, arguments } => {
                    let _ = app_listener.emit("agent-tool-start", json!({
                        "sessionId": sid_listener,
                        "toolId": id,
                        "toolName": name,
                        "arguments": arguments,
                    }));
                }
                AgentLoopEvent::ToolResult { id, name, result, success } => {
                    let _ = app_listener.emit("agent-tool-result", json!({
                        "sessionId": sid_listener,
                        "toolId": id,
                        "toolName": name,
                        "result": result,
                        "success": success,
                    }));
                }
                AgentLoopEvent::ApprovalRequired { call_id, tool_name, arguments, risk_description } => {
                    let _ = app_listener.emit("agent-approval-required", json!({
                        "sessionId": sid_listener,
                        "toolId": call_id,
                        "toolName": tool_name,
                        "arguments": arguments,
                        "riskDescription": risk_description,
                    }));
                }
                AgentLoopEvent::Done { content: _ } => {
                    let _ = app_listener.emit("agent-done", json!({
                        "sessionId": sid_listener,
                    }));
                }
                AgentLoopEvent::Error { message } => {
                    let _ = app_listener.emit("agent-error", json!({
                        "sessionId": sid_listener,
                        "error": message,
                    }));
                }
            }
        }
    });

    // 12. 后台执行 Agent Loop
    let app_done = app.clone();
    let sid_done = session_id.to_string();
    tokio::spawn(async move {
        match agent_loop.run(config, tx).await {
            Ok(_content) => {
                // Done event already sent via broadcast
                log::info!("[API Agent] 对话完成: session={}", sid_done);
            }
            Err(e) => {
                // Error event already sent via broadcast
                log::error!("[API Agent] 对话失败: session={}, error={}", sid_done, e);
            }
        }
        // 确保最终发送 done（如果 run 出错未发送 done）
        let _ = app_done.emit("agent-done", json!({
            "sessionId": sid_done,
        }));
    });

    Ok(())
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
                .manage(DbState { pool: pool.clone() })
        .manage(AsyncMutex::new(AgentManager::new()))
        .manage(PendingApprovals::new())
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
            commands::virtual_console::list_virtual_console_sessions,
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
            get_theme,
            set_theme_cmd,
            agent_send_message_with_config,
            agent_stop_generation,
            agent_approve_tool,
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
