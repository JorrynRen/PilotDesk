//! 群聊参与者的工具集（复用 AgentLoop 的 `ToolHandler`）。
//!
//! 提供文件/命令/浏览器/图像/技能等核心工具，供 `PilotDeskLlmClient` 在群聊场景下复用
//! `run_agent_turn` 时注入。风险分级：
//! - Low（读/静默）：read_file / list_files / glob / grep / search_web / fetch_web /
//!   load_skill / generate_image / edit_image —— 自动放行
//! - Medium（写/副作用）：write_file / edit_file / browser —— 触发 ApprovalHandler（Director 裁决）
//! - High（执行）：execute_command —— 触发 ApprovalHandler（Director 裁决）
//!
//! 注：图像工具为 Low（不触发审批），但其调用付费 API（tags 含 HighCost）。
//! 若参与者 provider 不支持 images API，运行期会返回 HTTP 错误——由参与者
//! 主动询问用户当前服务商支持的模型来规避，而非装配期拦截。
//!
//! 装配方式（工具架构统一 v1.0，轮 9）：全量注册表 + 群聊场景清单
//! （`ToolProfile::groupchat`）→ `assemble_tools` 同步装配。
//! MCP：房间级连接池（RoomMcpAssets，run() 启动时异步枚举一次）按 profile 过滤注册，默认启用。
//!
//! 依赖注入（v1.1）：`api_format` 为参与者 provider 的 API 格式（Anthropic 时不注册
//! 图像工具）；`image` 为参与者 provider 的图像端点 (endpoint, key)；技能加载器复用
//! 全局 API Agent 技能目录（`~/.pilotdesk/skills/`）。子代理/记忆仍缺省；ask_user v3.4m
//! 解禁（AgentLoop 群聊模式拦截其调用转为轮末确认，见 build_groupchat_tool_registry）。

use crate::api_agent::agent_loop::ToolRegistry;
use crate::api_agent::skills::SkillLoader;
use crate::api_agent::types::ApiFormat;
use crate::db::init::DbPool;
use crate::tools::history::FileHistoryService;
use crate::tools::mcp::McpServerConfig;
use crate::tools::ToolHandler;
use std::sync::Arc;

/// 构建群聊参与者工具集。`cwd` 为工作区目录（用于相对路径解析与越界保护）。
/// `api_format` / `image` 取自参与者自己的 provider 配置；`model` 为该参与者的模型名
/// （read_image 缺省模型）；`vision` 为 read_image 的 (endpoint, key)，两种协议通用。
/// 技能加载器读取全局技能目录。
/// `app` / `pool` / `room_id` 用于组装文件写入副作用服务（历史快照 + diff 事件），
/// 历史记录以 room_id 作为归属标识落库（与会话模式隔离）。
pub fn build_groupchat_tool_registry(
    conn: &rusqlite::Connection,
    cwd: &str,
    model: &str,
    api_format: ApiFormat,
    image: Option<(String, String)>,
    vision: Option<(String, String)>,
    app: &tauri::AppHandle,
    pool: &DbPool,
    room_id: &str,
    mcp: Option<&RoomMcpAssets>,
) -> Arc<ToolRegistry> {
    // 群聊环境：仅文件/执行/网络/浏览器/图像/技能工具所需的依赖；
    // 子代理/记忆均缺省（对应 GROUPCHAT_DISABLE 保留项）；ask_user 单独注册（见下，
    // v3.4m 解禁，AgentLoop 群聊模式拦截其调用转为轮末确认）。MCP 由房间级
    // RoomMcpAssets 装配（下方 mcp 参数注入，默认启用、overrides 可禁用）。
    // 先合并群聊模式 overrides（追加禁用，新房间生效），供能力项开关判断。
    let mut profile = crate::tools::ToolProfile::groupchat();
    let overrides = crate::commands::tools::load_tool_overrides(conn);
    profile.add_extra_disable(overrides.groupchat);

    // 文件历史能力项：被用户按场景禁用时不注入（新房间生效）；
    // 启用时以 room_id 作为归属标识落库（与会话模式隔离）。
    // enabled 指向群聊场景共享开关，群聊页「文件历史」开关热切换后即时生效。
    let file_history = if crate::tools::is_disabled(&profile, crate::tools::FILE_HISTORY_CAP) {
        None
    } else {
        Some(Arc::new(FileHistoryService {
            app: app.clone(),
            scope: room_id.to_string(),
            pool: pool.clone(),
            enabled: crate::tools::history::groupchat_file_history_flag(),
        }))
    };

    // 模型能力查询与跨 provider 解析（闭包持有连接池；key 只在后端解析，绝不进 LLM 上下文）。
    // 群聊 v1.2 解禁 list_models：参与者可查模型清单自主选模型（配合图片等生成工具）。
    let pool_for_providers = pool.clone();
    let list_providers: Option<
        Arc<dyn Fn() -> Vec<crate::tools::ProviderModelInfo> + Send + Sync>,
    > = Some(Arc::new(move || {
        let Ok(conn) = pool_for_providers.get() else {
            return Vec::new();
        };
        crate::commands::api_provider::collect_provider_models(&conn)
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

    let env = crate::tools::ToolEnv {
        cwd: cwd.to_string(),
        api_format,
        image: image.clone(),
        vision,
        audio: image,
        list_providers,
        resolve_provider,
        search_config: crate::commands::search::load_search_config(conn),
        client: None,
        model: model.to_string(),
        skill_loader: crate::get_api_agent_skills_dir().map(|dir| {
            log::info!("[群聊] 技能目录: {}", dir);
            Arc::new(SkillLoader::new(Some(dir)))
        }),
        memory_store: None,
        // 知识库检索与记忆工具同源，群聊一并缺省（只读，日后要放开从这里给上即可）
        knowledge_store: None,
        app: None,
        session_id: String::new(),
        pending: None,
        file_history,
        permission_rules: Some(Arc::new(
            crate::commands::permission::load_rules(conn).unwrap_or_default(),
        )),
    };

    let mut registry = ToolRegistry::new();
    for tool in crate::tools::assemble_tools(&profile, &env) {
        registry.register(tool);
    }

    // 群聊注册 ask_user（v3.4m 解禁）：仅需其参数定义进入工具清单供 LLM 调用；
    // 群聊模式（AskUserBehavior::TurnEnd）下 AgentLoop 会在执行前拦截其调用转为轮末确认请求，
    // execute 不会被触发。pending 用独立实例占位，避免与全局审批通道混淆。
    if !crate::tools::is_disabled(&profile, "ask_user") {
        registry.register(Arc::new(crate::tools::ask_user::AskUserTool::new(
            app.clone(),
            room_id.to_string(),
            crate::PendingApprovals::new(),
        )));
    }

    // 群聊 MCP 解禁：房间级连接池（run() 启动时异步枚举一次），所有参与者共享同一连接。
    // 每个 MCP 工具仍按 profile 过滤，尊重用户 overrides 按需禁用（默认启用）。
    if let Some(assets) = mcp {
        for (server, infos) in &assets.entries {
            for info in infos {
                let handler = crate::tools::mcp::McpToolHandler::new(
                    assets.pool.clone(),
                    server.clone(),
                    info.clone(),
                );
                if !crate::tools::is_disabled(&profile, handler.name()) {
                    registry.register(Arc::new(handler));
                }
            }
        }
    }

    Arc::new(registry)
}

/// 房间级 MCP 资产：连接池 + 已枚举的工具清单（群聊解禁，异步构建一次）。
pub struct RoomMcpAssets {
    pub pool: crate::tools::mcp::McpConnectionPool,
    /// (服务器配置, 枚举出的工具信息)
    pub entries: Vec<(
        crate::tools::mcp::McpServerConfig,
        Vec<crate::tools::mcp::McpToolInfo>,
    )>,
}

/// 异步构建房间级 MCP 资产：连接一次并枚举全部已配置服务器的工具。
/// 入参为已加载的服务器配置（调用方同步读取，避免 `&Connection` 跨 await 导致非 Send）。
/// 连接/枚举失败仅告警跳过（与会话模式行为一致），不阻断房间启动。
pub async fn build_room_mcp_assets(servers: Vec<McpServerConfig>) -> RoomMcpAssets {
    let pool = crate::tools::mcp::McpConnectionPool::new();
    let mut entries = Vec::new();
    for server in &servers {
        match pool.get_or_connect(server).await {
            Ok(client) => match client.lock().await.list_tools().await {
                Ok(tools) => entries.push((server.clone(), tools)),
                Err(e) => log::warn!("[群聊 MCP] 列出工具失败 {}: {}", server.name, e),
            },
            Err(e) => log::warn!("[群聊 MCP] 连接服务器 {} 失败: {}", server.name, e),
        }
    }
    RoomMcpAssets { pool, entries }
}
