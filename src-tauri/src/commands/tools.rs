//! 工具管理：目录查询（tool_catalog）+ 用户覆盖持久化（overrides）。
//!
//! - `tool_catalog`：构造全依赖 ToolEnv 实例化全部内建工具，输出元数据
//!   （名称/描述/风险/tags）与各场景（会话/群聊）默认状态与锁定标记；
//! - overrides：用户在工具管理页的"追加禁用"清单，存于 app_settings
//!   （key = `tool_profile_overrides`），装配入口读取后注入 ToolProfile.extra_disable。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api_agent::client::ApiClient;
use crate::api_agent::db::MemoryStore;
use crate::api_agent::skills::SkillLoader;
use crate::api_agent::types::ApiFormat;
use crate::db::init::DbPool;
use crate::tools::{self, RiskLevel, ToolHandler, ToolProfile};
use crate::utils::errors::AppError;
use crate::PendingApprovals;

/// app_settings 存储键
const OVERRIDES_KEY: &str = "tool_profile_overrides";

/// 用户覆盖清单：各场景追加禁用的工具名列表。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolOverrides {
    #[serde(default)]
    pub session: Vec<String>,
    #[serde(default)]
    pub groupchat: Vec<String>,
}

/// 读取持久化的工具覆盖清单（不存在返回默认空）。
pub fn load_tool_overrides(conn: &Connection) -> ToolOverrides {
    crate::commands::app_settings::get_setting(conn, OVERRIDES_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<ToolOverrides>(&s).ok())
        .unwrap_or_default()
}

/// 工具目录单项（前端工具管理页展示 + 开关状态）。
#[derive(Serialize)]
pub struct ToolCatalogItem {
    pub name: String,
    pub description: String,
    /// low / medium / high
    pub risk: String,
    /// ToolTag 变体名（如 Filesystem / Web / HighCost）
    pub tags: Vec<String>,
    /// 会话模式：是否启用 / 是否架构性锁定
    pub session_enabled: bool,
    pub session_locked: bool,
    /// 群聊模式：是否启用 / 是否架构性锁定
    pub groupchat_enabled: bool,
    pub groupchat_locked: bool,
}

/// 构造全依赖 ToolEnv 实例化全部内建工具，生成目录。
/// 元数据展示用途：endpoint/key 等用占位值；依赖缺失时对应工具缺席（极端情况）。
/// todo_write 构造需连接池（状态事件化到 session_events），未走 `default_tools`，
/// 本处以真实工具实例补一份元数据项（见下方说明）。
pub fn build_tool_catalog(
    conn: &Connection,
    app: tauri::AppHandle,
    pool: &DbPool,
) -> Vec<ToolCatalogItem> {
    // list_models 数据源占位（catalog 仅展示元数据，不真正执行）：保证 list_models
    // 与图片等工具一样始终出现在目录中（否则依赖缺失导致缺席，前端无法统一管理）。
    let list_providers: Option<
        Arc<dyn Fn() -> Vec<crate::tools::ProviderModelInfo> + Send + Sync>,
    > = Some(Arc::new(|| Vec::new()));
    let resolve_provider: Option<
        Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>,
    > = Some(Arc::new(|_| None));
    // 记忆上限统一存主库（`conn` 即主库连接）：读主库设置后传入，MEMORY.db 不再自持设置
    let memory_limit = crate::api_agent::db::load_memory_max_entries(conn);
    let env = tools::ToolEnv {
        cwd: String::new(),
        api_format: ApiFormat::OpenAI,
        image: Some((String::new(), String::new())),
        vision: Some((String::new(), String::new())),
        audio: Some((String::new(), String::new())),
        list_providers,
        resolve_provider,
        search_config: crate::commands::search::load_search_config(conn),
        client: Some(ApiClient::new_openai(String::new(), String::new())),
        model: String::new(),
        skill_loader: Some(Arc::new(SkillLoader::new(None))),
        memory_store: crate::api_agent::system_prompt::get_pilotdesk_config_dir()
            .and_then(|d| MemoryStore::new(&d, memory_limit).ok())
            .map(Arc::new),
        // 工具管理页要把 search_knowledge 也列出来，所以这里与 memory_store 同样按配置目录打开
        knowledge_store: crate::api_agent::system_prompt::get_pilotdesk_config_dir()
            .and_then(|d| crate::api_agent::knowledge::KnowledgeStore::open(&d).ok())
            .map(Arc::new),
        app: Some(app),
        session_id: String::new(),
        pending: Some(PendingApprovals::new()),
        file_history: None,
        permission_rules: None,
    };
    let session_profile = ToolProfile::session();
    let groupchat_profile = ToolProfile::groupchat();

    // 元数据映射：工具实例 → 目录项（两场景默认启用/锁定状态一并计算）。
    fn to_catalog_item(
        handler: &dyn ToolHandler,
        session_profile: &ToolProfile,
        groupchat_profile: &ToolProfile,
    ) -> ToolCatalogItem {
        let name = handler.name().to_string();
        let description = handler.description().to_string();
        let risk = match handler.risk_level() {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
        }
        .to_string();
        let tags = handler
            .tags()
            .iter()
            .map(|tag| format!("{:?}", tag))
            .collect::<Vec<_>>();
        let session_enabled = !tools::is_disabled(session_profile, &name);
        let session_locked = tools::is_default_disabled(session_profile, &name);
        let groupchat_enabled = !tools::is_disabled(groupchat_profile, &name);
        let groupchat_locked = tools::is_default_disabled(groupchat_profile, &name);
        ToolCatalogItem {
            name,
            description,
            risk,
            tags,
            session_enabled,
            session_locked,
            groupchat_enabled,
            groupchat_locked,
        }
    }

    let mut items: Vec<ToolCatalogItem> = tools::default_tools(&env)
        .into_iter()
        .map(|t| to_catalog_item(t.as_ref(), &session_profile, &groupchat_profile))
        .collect();

    // todo_write（会话专属）：状态事件化后构造需 session_id + 连接池，不再进 `default_tools`；
    // 运行期由 build_registry 按会话注册。目录仍以真实工具元数据补一项，保持工具管理页
    // 可展示/可禁用（实例仅用于取元数据，不连接数据库）。
    let todo_tool = Arc::new(crate::tools::todo_write::TodoWriteTool::new(
        String::new(),
        pool.clone(),
    ));
    items.push(to_catalog_item(
        todo_tool.as_ref(),
        &session_profile,
        &groupchat_profile,
    ));

    // 内部能力项：文件历史（非 LLM 工具，write_file/edit_file 的自动副作用）。
    // 与内建工具同构纳入清单，支持按场景差异化禁用（overrides 追加禁用）。
    items.push(ToolCatalogItem {
        name: tools::FILE_HISTORY_CAP.to_string(),
        description: "write_file / edit_file 修改文件前的快照记录，供撤销回退。非 Agent 可调用工具，属自动副作用能力。".to_string(),
        risk: "low".to_string(),
        tags: vec!["Filesystem".to_string(), "Write".to_string()],
        session_enabled: !tools::is_disabled(&session_profile, tools::FILE_HISTORY_CAP),
        session_locked: tools::is_default_disabled(&session_profile, tools::FILE_HISTORY_CAP),
        groupchat_enabled: !tools::is_disabled(&groupchat_profile, tools::FILE_HISTORY_CAP),
        groupchat_locked: tools::is_default_disabled(&groupchat_profile, tools::FILE_HISTORY_CAP),
    });

    // 内部能力项：MCP 整体开关（mcp:* 前缀通配）。MCP 工具按设置动态加载，
    // 群聊默认架构性禁用（锁定）；会话默认启用，可整体禁用（overrides 追加 mcp:*）。
    items.push(ToolCatalogItem {
        name: tools::MCP_CAP.to_string(),
        description: "MCP 服务器工具按「MCP 服务器」设置中启用的服务器动态加载（不在本清单逐个展示）。此开关控制是否整体启用 MCP。".to_string(),
        risk: "medium".to_string(),
        tags: vec!["Mcp".to_string(), "Execute".to_string()],
        session_enabled: !tools::is_disabled(&session_profile, tools::MCP_CAP),
        session_locked: tools::is_default_disabled(&session_profile, tools::MCP_CAP),
        groupchat_enabled: !tools::is_disabled(&groupchat_profile, tools::MCP_CAP),
        groupchat_locked: tools::is_default_disabled(&groupchat_profile, tools::MCP_CAP),
    });

    items
}

/// 工具目录（含当前生效状态，含用户 overrides）。
#[tauri::command]
pub fn tool_catalog(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<ToolCatalogItem>, String> {
    let conn = state.get_conn()?;
    Ok(build_tool_catalog(&conn, app, &state.pool))
}

/// 读取用户工具覆盖清单。
#[tauri::command]
pub fn get_tool_overrides(
    state: tauri::State<'_, crate::DbState>,
) -> Result<ToolOverrides, String> {
    let conn = state.get_conn()?;
    Ok(load_tool_overrides(&conn))
}

/// 保存用户工具覆盖清单（覆盖式写入）。
#[tauri::command]
pub fn set_tool_overrides(
    state: tauri::State<'_, crate::DbState>,
    overrides: ToolOverrides,
) -> Result<(), String> {
    let conn = state.get_conn()?;
    let json = serde_json::to_string(&overrides).map_err(AppError::from)?;
    crate::commands::app_settings::set_setting(&conn, OVERRIDES_KEY, &json)
        .map_err(|e| e.to_string())
}

/// 群聊文件历史当前开关状态（读取持久化 overrides：未在群聊禁用清单即视为启用）。
#[tauri::command]
pub fn groupchat_get_file_history_enabled(
    state: tauri::State<'_, crate::DbState>,
) -> Result<bool, String> {
    let conn = state.get_conn()?;
    let overrides = load_tool_overrides(&conn);
    Ok(!overrides
        .groupchat
        .iter()
        .any(|t| t == tools::FILE_HISTORY_CAP))
}

/// 群聊文件历史开关：持久化 overrides + 热切换共享运行期标记（当前房间立即生效）。
#[tauri::command]
pub fn groupchat_set_file_history(
    state: tauri::State<'_, crate::DbState>,
    enabled: bool,
) -> Result<(), String> {
    let conn = state.get_conn()?;
    let mut overrides = load_tool_overrides(&conn);
    if enabled {
        overrides.groupchat.retain(|t| t != tools::FILE_HISTORY_CAP);
    } else if !overrides
        .groupchat
        .iter()
        .any(|t| t == tools::FILE_HISTORY_CAP)
    {
        overrides
            .groupchat
            .push(tools::FILE_HISTORY_CAP.to_string());
    }
    let json = serde_json::to_string(&overrides).map_err(AppError::from)?;
    crate::commands::app_settings::set_setting(&conn, OVERRIDES_KEY, &json)
        .map_err(|e| e.to_string())?;
    // 热切换运行期标记：无需重建参与者工具集，所有运行中的群聊房间立即停止/恢复记录。
    crate::tools::history::set_groupchat_file_history_enabled(enabled);
    Ok(())
}
