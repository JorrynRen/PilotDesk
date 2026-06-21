use serde::Serialize;
use std::sync::Mutex;

use super::PluginHost;
use crate::agent::AgentManager;
use crate::commands::agents::get_agent_inner;
use crate::DbState;
use tokio::sync::Mutex as AsyncMutex;

/// 会话信息
#[derive(Debug, Clone, Serialize)]
pub struct AgentSessionInfo {
    pub session_id: String,
    pub agent_type: String,
    pub created_at: String,
}

/// Agent 信息
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub agent_type: String,
    pub name: String,
    pub version: String,
}

/// Agent 响应
#[derive(Debug, Clone, Serialize)]
pub struct AgentResponse {
    pub content: String,
    pub session_id: String,
}

/// 消息记录
#[derive(Debug, Clone, Serialize)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// 创建会话
#[tauri::command]
pub async fn plugin_agent_create_session(
    host: tauri::State<'_, Mutex<PluginHost>>,
    plugin_id: String,
    agent_type: String,
    _options: Option<serde_json::Value>,
) -> Result<AgentSessionInfo, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let sandbox_info = host.get_sandbox_info();

    // 权限检查
    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    if sandbox_info.sandbox_enabled && !PluginHost::has_permission(plugin, "session:write") {
        return Err("沙箱已启用，需要 session:write 权限".to_string());
    }

    drop(host);

    // 创建会话 ID
    let session_id = format!("plugin_{}_{}", plugin_id, uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("unknown"));

    Ok(AgentSessionInfo {
        session_id,
        agent_type,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// 发送消息到 Agent
#[tauri::command]
pub async fn plugin_agent_send_message(
    app: tauri::AppHandle,
    host: tauri::State<'_, Mutex<PluginHost>>,
    agent_mgr: tauri::State<'_, AsyncMutex<AgentManager>>,
    state: tauri::State<'_, DbState>,
    plugin_id: String,
    session_id: String,
    content: String,
) -> Result<AgentResponse, String> {
    {
        let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
        let sandbox_info = host.get_sandbox_info();

        let plugins = host.list_plugins();
        let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
            .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

        if sandbox_info.sandbox_enabled && !PluginHost::has_permission(plugin, "session:execute") {
            return Err("沙箱已启用，需要 session:execute 权限".to_string());
        }
    }

    // 从 session_id 推断 agent_type (格式: plugin_{pluginId}_{agentType}_{uuid})
    let parts: Vec<&str> = session_id.split('_').collect();
    let agent_type = if parts.len() >= 3 { parts[2] } else { "hermes" };

    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let config = get_agent_inner(&conn, agent_type)
        .map_err(|e| format!("查询 Agent 配置失败: {}", e))?
        .ok_or_else(|| format!("未知 Agent 类型: {}", agent_type))?;

    let mut mgr = agent_mgr.lock().await;
    mgr.send_message_with_config(
        app,
        session_id.clone(),
        config,
        content,
        "stream".to_string(),
        None,
        None,
        None,
    ).await.map_err(|e| format!("发送消息失败: {}", e))?;

    Ok(AgentResponse {
        content: "消息已发送".to_string(),
        session_id,
    })
}

/// 获取会话历史
#[tauri::command]
pub async fn plugin_agent_get_history(
    host: tauri::State<'_, Mutex<PluginHost>>,
    state: tauri::State<'_, DbState>,
    plugin_id: String,
    session_id: String,
) -> Result<Vec<AgentMessage>, String> {
    let host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;
    let sandbox_info = host.get_sandbox_info();

    let plugins = host.list_plugins();
    let plugin = plugins.iter().find(|p| p.manifest.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    if sandbox_info.sandbox_enabled && !PluginHost::has_permission(plugin, "session:read") {
        return Err("沙箱已启用，需要 session:read 权限".to_string());
    }

    drop(host);

    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let messages = crate::commands::session::get_session_messages_inner(&conn, &session_id)
        .map_err(|e| format!("获取消息失败: {}", e))?;

    Ok(messages.into_iter().map(|m| AgentMessage {
        role: m.role,
        content: m.content,
        timestamp: m.timestamp.to_string(),
    }).collect())
}

/// 列出插件会话
#[tauri::command]
pub async fn plugin_agent_list_sessions(
    host: tauri::State<'_, Mutex<PluginHost>>,
    state: tauri::State<'_, DbState>,
    plugin_id: String,
) -> Result<Vec<AgentSessionInfo>, String> {
    let _host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;

    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let sessions = crate::commands::session::list_sessions_inner(&conn)
        .map_err(|e| format!("获取会话列表失败: {}", e))?;

    let prefix = format!("plugin_{}_", plugin_id);
    Ok(sessions.into_iter()
        .filter(|s| s.id.starts_with(&prefix))
        .map(|s| AgentSessionInfo {
            session_id: s.id,
            agent_type: s.agent_type,
            created_at: s.created_at.to_string(),
        })
        .collect())
}

/// 删除会话
#[tauri::command]
pub async fn plugin_agent_delete_session(
    host: tauri::State<'_, Mutex<PluginHost>>,
    state: tauri::State<'_, DbState>,
    _plugin_id: String,
    session_id: String,
) -> Result<(), String> {
    let _host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;

    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::commands::session::delete_session_inner(&conn, &session_id)
        .map_err(|e| format!("删除会话失败: {}", e))
}

/// 列出可用 Agent
#[tauri::command]
pub async fn plugin_agent_list_agents(
    host: tauri::State<'_, Mutex<PluginHost>>,
    state: tauri::State<'_, DbState>,
    _plugin_id: String,
) -> Result<Vec<AgentInfo>, String> {
    let _host = host.lock().map_err(|e| format!("锁定失败: {}", e))?;

    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let agents = crate::commands::agents::list_agents_inner(&conn)
        .map_err(|e| format!("获取 Agent 列表失败: {}", e))?;

    Ok(agents.into_iter().map(|a| AgentInfo {
        agent_type: a.agent_type,
        name: a.display_name,
        version: a.version,
    }).collect())
}
