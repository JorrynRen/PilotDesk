//! ACP 宿主会话导入命令（薄适配层）。
//!
//! 真正的协议交互在 `agent::host_sessions`：本层只做
//! 「就绪门禁 → 取接入方式与启动参数 → 调一次 → 落库/绑定宿主会话 id」。

use rusqlite::Connection;
use serde_json::Value;
use tauri::State;
use tokio::sync::Mutex as AsyncMutex;

use crate::agent::config::AgentConfig;
use crate::agent::host_sessions::{
    delete_session, list_sessions, load_messages, DeleteSessionOutcome, ImportedMessage,
    ListSessionsOutcome,
};
use crate::agent::AgentManager;
use crate::commands::agents::{get_agent_inner, get_agent_transport_inner};
use crate::utils::errors::AppError;
use crate::DbState;

/// 一次导入所需的 ACP 启动参数（从 `agents.conn` JSON 取出）。
struct HostAcpConfig {
    command: String,
    args: Vec<String>,
    env: Option<serde_json::Map<String, Value>>,
    cwd: String,
}

/// 门禁所需的全部输入。
///
/// 刻意**不持有** `&Connection`：`rusqlite::Connection` 不是 `Sync`，
/// 带着它跨 await 会让命令 future 不满足 Tauri 要求的 `Send`。
struct Prepared {
    cfg: HostAcpConfig,
    agent_config: AgentConfig,
    conn_json: Option<String>,
}

/// 接入方式校验 + 启动参数解析（同步，跑在取连接的作用域内）。
fn prepare(conn: &Connection, agent_type: &str, cwd: Option<&str>) -> Result<Prepared, AppError> {
    let info = get_agent_transport_inner(conn, agent_type)?;
    if info.transport != "acp" {
        return Err(AppError::InvalidInput(
            "仅 ACP 接入支持导入宿主会话".to_string(),
        ));
    }
    let agent_config = get_agent_inner(conn, agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型 '{}' 不存在", agent_type)))?;

    let conn_json: Value = match info.conn.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => serde_json::from_str(s)?,
        _ => Value::Null,
    };
    let command = conn_json["command"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if command.trim().is_empty() {
        return Err(AppError::Config(format!(
            "Agent「{}」未配置 ACP 启动命令",
            agent_type
        )));
    }
    let args = conn_json["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let env = conn_json["env"].as_object().cloned();

    let cwd = match cwd.map(str::trim).filter(|s| !s.is_empty()) {
        Some(dir) => dir.to_string(),
        // 与发消息同口径：未指定时回退全局工作区
        None => crate::utils::paths::resolve_workspace_path(None, "", conn)
            .to_string_lossy()
            .to_string(),
    };

    Ok(Prepared {
        cfg: HostAcpConfig {
            command,
            args,
            env,
            cwd,
        },
        agent_config,
        conn_json: info.conn,
    })
}

/// 与发消息同一道就绪门禁（`deep=false`：导入不需要重新做一次完整检测）。
async fn ensure_ready(
    agent_mgr: &AsyncMutex<AgentManager>,
    prepared: &Prepared,
) -> Result<(), AppError> {
    let mgr = agent_mgr.lock().await;
    match crate::commands::agent_market::readiness_gate(
        &mgr,
        &prepared.agent_config,
        "acp",
        prepared.conn_json.as_deref(),
        false,
    )
    .await
    {
        Some(err) => Err(AppError::External(err)),
        None => Ok(()),
    }
}

#[tauri::command]
pub async fn acp_list_sessions(
    agent_mgr: State<'_, AsyncMutex<AgentManager>>,
    state: State<'_, DbState>,
    agent_type: String,
    cwd: Option<String>,
    cursor: Option<String>,
) -> Result<ListSessionsOutcome, AppError> {
    let prepared = {
        let conn = state.get_conn()?;
        prepare(&conn, &agent_type, cwd.as_deref())?
    };
    ensure_ready(&*agent_mgr, &prepared).await?;
    let cfg = prepared.cfg;

    // 用户明确选了目录就按该目录过滤；没选则列出全部宿主会话
    let filter = cwd.as_deref().map(str::trim).filter(|s| !s.is_empty());
    list_sessions(
        &agent_type,
        &cfg.command,
        &cfg.args,
        cfg.env.as_ref(),
        &cfg.cwd,
        filter,
        // 追加参数：`nextCursor` 分页（前端「加载更多」透传）
        cursor.as_deref(),
    )
    .await
    .map_err(AppError::External)
}

#[tauri::command]
pub async fn acp_import_host_session(
    agent_mgr: State<'_, AsyncMutex<AgentManager>>,
    state: State<'_, DbState>,
    agent_type: String,
    host_session_id: String,
    cwd: Option<String>,
    local_session_id: String,
) -> Result<usize, AppError> {
    let prepared = {
        let conn = state.get_conn()?;
        prepare(&conn, &agent_type, cwd.as_deref())?
    };
    ensure_ready(&*agent_mgr, &prepared).await?;
    let cfg = prepared.cfg;

    let messages = load_messages(
        &agent_type,
        &cfg.command,
        &cfg.args,
        cfg.env.as_ref(),
        &cfg.cwd,
        &host_session_id,
    )
    .await
    .map_err(AppError::External)?;

    // 协议交互期间不长期占用连接池：落库前重新取连接
    let conn = state.get_conn()?;
    for msg in &messages {
        crate::commands::session::save_message_inner(
            &conn,
            &local_session_id,
            &msg.role,
            &msg.content,
            "native",
            None,
            None,
            None,
            None,
        )?;
    }
    crate::commands::session::update_session_agent_id_inner(
        &conn,
        &local_session_id,
        &host_session_id,
    )?;

    Ok(messages.len())
}

/// 只读预览宿主会话：回放消息但**不落库**。
///
/// 与导入分开，是为了「只看看」不污染本地会话列表：用户点开预览再关掉，
/// 本地库不应多出一条会话。
#[tauri::command]
pub async fn acp_load_host_session(
    agent_mgr: State<'_, AsyncMutex<AgentManager>>,
    state: State<'_, DbState>,
    agent_type: String,
    host_session_id: String,
    cwd: Option<String>,
) -> Result<Vec<ImportedMessage>, AppError> {
    let prepared = {
        let conn = state.get_conn()?;
        prepare(&conn, &agent_type, cwd.as_deref())?
    };
    ensure_ready(&*agent_mgr, &prepared).await?;
    let cfg = prepared.cfg;

    load_messages(
        &agent_type,
        &cfg.command,
        &cfg.args,
        cfg.env.as_ref(),
        &cfg.cwd,
        &host_session_id,
    )
    .await
    .map_err(AppError::External)
}

/// 删除宿主侧会话。
///
/// 刻意**不跑就绪门禁**：删除是清理动作，不该因为"上次安装/升级失败"被挡住；
/// 真起不来时由协议错误直接回显（含 stderr 尾部）。
#[tauri::command]
pub async fn acp_delete_host_session(
    state: State<'_, DbState>,
    agent_type: String,
    host_session_id: String,
    cwd: Option<String>,
) -> Result<DeleteSessionOutcome, AppError> {
    let prepared = {
        let conn = state.get_conn()?;
        prepare(&conn, &agent_type, cwd.as_deref())?
    };
    let cfg = prepared.cfg;

    delete_session(
        &agent_type,
        &cfg.command,
        &cfg.args,
        cfg.env.as_ref(),
        &cfg.cwd,
        &host_session_id,
    )
    .await
    .map_err(AppError::External)
}
