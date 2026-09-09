//! 工具层：统一的工具协议、注册表与内建工具集。
//!
//! 本模块是 PilotDesk 工具架构统一的核心：
//! - 协议定义（`ToolHandler` / `RiskLevel` / `ToolTag`）
//! - 注册表（`ToolRegistry`，含按标签过滤的定义导出）
//! - 内建工具装配（`default_tools`，会话模式与群聊模式共用一套实现）
//!
//! 迁移说明（v1.0）：原协议定义与 `builtin_tool!` 宏位于 `api_agent/agent_loop.rs`，
//! 迁移到本模块后 `agent_loop.rs` 通过 `pub use crate::tools::*` 兼容既有引用。
//! `builtin_tool!` / `builtin_tool_risky!` 宏已随全部工具迁移为具名 struct 后删除。
//!
//! 迁移说明（v2.0 权限规则体系）：`CommandRisk` / `classify_command` 启发式清单已删除，
//! 命令风险改由 `PermissionRules`（deny > risky > allow > 默认策略）单一规则体系驱动。

use crate::api_agent::client::ApiClient;
use crate::api_agent::db::MemoryStore;
use crate::api_agent::skills::SkillLoader;
use crate::api_agent::types::{ApiFormat, ToolDefinition};
use crate::api_agent::web::SearchConfig;
use crate::PendingApprovals;
use serde::Serialize;
use std::sync::Arc;

pub mod ask_user;
pub mod browser;
pub mod create_document;
pub mod deps;
pub mod edit_file;
pub mod edit_image;
pub mod embed;
pub mod exec;
pub mod execute_command;
pub mod execute_python;
pub mod fetch_web;
pub mod generate_image;
pub mod generate_video;
pub mod glob;
pub mod grep;
pub mod history;
pub mod image_common;
pub mod list_files;
pub mod list_models;
pub mod mcp;
pub mod memory;
pub mod parse_document;
pub mod read_file;
pub mod search_web;
pub mod skills;
pub mod stt;
pub mod subagent;
pub mod todo_write;
pub mod tts;
pub mod write_file;

/// 工具风险等级
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// 低风险：读取文件、搜索等（静默执行）
    Low,
    /// 中风险：修改文件、网络请求等（确认后执行）
    Medium,
    /// 高风险：执行命令、删除文件等（严格确认）
    High,
}

impl RiskLevel {
    pub fn description(&self) -> &str {
        match self {
            RiskLevel::Low => "低风险操作",
            RiskLevel::Medium => "中风险操作（可能修改文件或访问网络）",
            RiskLevel::High => "高风险操作（执行系统命令或删除文件）",
        }
    }
}

/// 工具分类标签（域 + 能力 双维度，替代单值 category）。
///
/// 用途：
/// - 场景 allowlist 过滤（`ToolRegistry::get_definitions_filtered`）
/// - 权限分级（tags 与 `RiskLevel` 联动）
/// - 未来插件按标签暴露能力
// 设计内未消费（方案 v2：tags 留作元数据，供未来前端管理页分类筛选/批量操作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ToolTag {
    // ── 域标签（能力归属）──
    /// 文件系统：read_file / list_files / glob / grep / write_file / edit_file
    Filesystem,
    /// 网络：search_web / fetch_web / browser
    Web,
    /// 命令执行：execute_command / execute_python
    Exec,
    /// 图像生成/编辑：generate_image / edit_image
    Image,
    /// 视频生成：generate_video
    Video,
    /// 语音合成/识别：tts / stt
    Audio,
    /// MCP 外部服务器工具（运行时动态注册）
    Mcp,
    /// 代理能力：subagent / todo_write
    Agent,
    /// 人机交互：ask_user
    Interaction,

    // ── 能力标签（操作性质/成本）──
    /// 只读
    Read,
    /// 写入/修改
    Write,
    /// 执行
    Execute,
    /// 联网
    Network,
    /// 高成本（图像生成、子代理等）
    HighCost,
    /// 需要用户交互（阻塞等待）
    Interactive,
}

/// 工具执行器 trait
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    /// 工具名称
    fn name(&self) -> &str;
    /// 工具描述
    fn description(&self) -> &str;
    /// 参数定义（JSON Schema）
    fn parameters(&self) -> serde_json::Value;
    /// 工具风险等级（默认 Low）
    fn risk_level(&self) -> RiskLevel { RiskLevel::Low }
    /// 分类标签（默认空，用于场景 allowlist 过滤；本轮装配层不消费，供未来前端管理页使用）
    #[allow(dead_code)]
    fn tags(&self) -> &[ToolTag] { &[] }
    /// 执行工具
    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String>;
}

/// 审批回调：返回 true 表示批准，false 表示拒绝
pub type ApprovalHandler = Box<dyn Fn(&str, &str, &str, RiskLevel) -> bool + Send + Sync>;

/// 迭代上限回调：返回 true 表示继续，false 表示终止
pub type ContinueHandler = Box<dyn Fn(usize, usize) -> bool + Send + Sync>;

/// 工具注册表
pub struct ToolRegistry {
    handlers: Vec<Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    pub fn register(&mut self, handler: Arc<dyn ToolHandler>) {
        self.handlers.push(handler);
    }

    /// 卸载指定工具（热插拔 / 插件卸载入口；当前由未来工具插件流程消费）。
    #[allow(dead_code)]
    pub fn unregister(&mut self, name: &str) {
        self.handlers.retain(|h| h.name() != name);
    }

    /// 按前缀批量卸载（如 `mcp_` 前缀整体卸载，用于 MCP 服务器移除；当前由未来工具插件流程消费）。
    #[allow(dead_code)]
    pub fn remove_by_prefix(&mut self, prefix: &str) {
        self.handlers.retain(|h| !h.name().starts_with(prefix));
    }

    /// 获取所有工具的 OpenAI 格式定义
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.handlers
            .iter()
            .map(|h| ToolDefinition::new(h.name(), h.description(), h.parameters()))
            .collect()
    }

    /// 按标签过滤导出工具定义。
    ///
    /// - `allowed` 为空数组 = 全量
    /// - 工具 `tags` 与 `allowed` 任一交集即保留
    // tags 本轮不消费（元数据，供未来前端管理页分类筛选/批量操作）。
    #[allow(dead_code)]
    pub fn get_definitions_filtered(&self, allowed: &[ToolTag]) -> Vec<ToolDefinition> {
        if allowed.is_empty() {
            return self.get_definitions();
        }
        self.handlers
            .iter()
            .filter(|h| h.tags().iter().any(|t| allowed.contains(t)))
            .map(|h| ToolDefinition::new(h.name(), h.description(), h.parameters()))
            .collect()
    }

    /// 执行指定工具
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String, String> {
        let args: serde_json::Value = serde_json::from_str(arguments)
            .unwrap_or(serde_json::Value::Null);

        for handler in &self.handlers {
            if handler.name() == name {
                return handler.execute(args).await;
            }
        }

        Err(format!("未知工具: {}", name))
    }

    #[allow(dead_code)]
    pub fn has_handler(&self, name: &str) -> bool {
        self.handlers.iter().any(|h| h.name() == name)
    }

    /// 获取工具的风险等级
    pub fn get_risk_level(&self, name: &str) -> Option<RiskLevel> {
        self.handlers.iter().find(|h| h.name() == name).map(|h| h.risk_level())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 装配层（方案 v2：全量注册 + 场景清单）
//
// - `ToolEnv`：构建期环境依赖（运行时数据，不硬编码进清单）
// - `ToolProfile`：场景差异清单（声明式数据）——禁用（精确名 / "mcp:*" 通配）+ 构造参数
// - `default_tools`：全量内建工具（单一事实源；todo_write 除外——其构造需连接池，
//   见 build_registry），依赖缺失或 api_format 不满足时跳过
// - `assemble_tools`：`default_tools` 按场景禁用清单过滤（群聊装配用）
// - `build_registry`：统一装配入口（`assemble_tools` 过滤 + todo_write 会话注册 + MCP 整体装配）
// - tags 本轮不消费（元数据，供未来前端管理页分类筛选/批量操作）
// ─────────────────────────────────────────────────────────────────────────────

/// 模型条目（list_models 工具返回给 LLM 的最小信息，**不含 key**）。
/// `description` 为用户对模型用途的自由备注（可选，由 LLM 语义理解）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 提供商能力清单（list_models 数据源，装配层注入；endpoint 为服务地址，key 由工具内部持有）。
#[derive(Debug, Clone)]
pub struct ProviderModelInfo {
    pub provider_id: String,
    pub provider_name: String,
    pub endpoint: String,
    /// 协议格式（openai/anthropic），供 LLM 判断调用方式差异。
    pub api_format: String,
    pub models: Vec<ModelSpec>,
}

/// 构建期环境依赖（运行时数据，不硬编码进清单）。
pub struct ToolEnv {
    pub cwd: String,
    pub api_format: ApiFormat,
    /// 图像工具依赖：(endpoint, key)。`None` 或 Anthropic 格式时不注册图像工具。
    pub image: Option<(String, String)>,
    /// 音频工具依赖：(endpoint, key)。当前会话 provider 的 OpenAI 兼容地址（与 image 同源，
    /// Anthropic 时缺省不注册 tts/stt）；跨 provider 走 resolve_provider。
    pub audio: Option<(String, String)>,
    /// list_models 数据源（会话模式注入：遍历 api_providers + 模型备注，不含 key）。
    pub list_providers: Option<Arc<dyn Fn() -> Vec<ProviderModelInfo> + Send + Sync>>,
    /// 跨 provider 解析：providerId → (endpoint, key, api_format)。供生成工具按需换模型；
    /// **key 只进入工具内部请求构造，绝不进 LLM 上下文/返回值**。
    pub resolve_provider: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    pub search_config: SearchConfig,
    /// 子代理工具依赖（群聊缺省）。
    pub client: Option<ApiClient>,
    pub model: String,
    /// load_skill 依赖（群聊缺省）。
    pub skill_loader: Option<Arc<SkillLoader>>,
    /// save_memory / search_memory 依赖（群聊缺省）。
    pub memory_store: Option<Arc<MemoryStore>>,
    /// ask_user 依赖（emit 事件 + 确认通道；群聊缺省）。
    pub app: Option<tauri::AppHandle>,
    pub session_id: String,
    pub pending: Option<PendingApprovals>,
    /// 文件写入副作用服务（历史快照 + diff 事件）。会话/群聊装配处组装注入。
    pub file_history: Option<Arc<history::FileHistoryService>>,
    /// 权限规则（write_file 脚本内容/系统目录安全检查用；会话/群聊装配处注入）。
    pub permission_rules: Option<Arc<crate::api_agent::agent_loop::PermissionRules>>,
}

/// 会话模式禁用清单（全量可用）。
pub static SESSION_DISABLE: &[&str] = &[];

/// 群聊模式禁用清单（4 项）。
///
/// 已移除（v1.1 评估后启用）：generate_image / edit_image（注入 provider 图像端点）、
/// browser（零依赖）、load_skill（注入 SkillLoader）。variation_image 已并入 generate_image（image 数组）。
/// MCP（v1.2 解禁）：房间级连接池（RoomMcpAssets）异步装配，默认启用、用户 overrides 可禁用。
/// list_models（v1.2 解禁）：模型能力查询开放给群聊参与者，供其查模型清单自主选模型（配合图片等生成工具）。
/// ask_user（v3.4m 解禁）：AgentLoop 群聊模式拦截其调用 → 转为轮末确认请求（不走工具内阻塞，
/// 由房间确认流无限等待 + 落库恢复现场），与 ConfirmationRequest 确认卡片机制统一为同一套交互。
/// 保留禁用原因：
/// - task：群聊已是「Director 分派 → 执行者」多 agent 编排，执行者内再开子代理会层级爆炸
/// - todo_write：与群聊 TaskRow / Director 任务分派体系重复冲突
/// - save_memory / search_memory：多 agent 写入用户全局记忆库需审批治理，语义风险高
pub static GROUPCHAT_DISABLE: &[&str] = &[
    "task",
    "todo_write",
    "save_memory",
    "search_memory",
];

/// 内部能力项：文件历史（非 LLM 工具）。
///
/// write_file / edit_file 修改前的快照记录属自动副作用，Agent 不可直接调用。
/// 以"能力项"身份纳入工具管理清单（catalog + overrides），供按场景差异化禁用：
/// 装配处判断 `is_disabled(profile, FILE_HISTORY_CAP)` 决定是否注入 FileHistoryService。
pub const FILE_HISTORY_CAP: &str = "file_history";

/// 内部能力项：MCP 整体开关（`mcp:*` 前缀通配）。
///
/// 群聊默认启用（v1.2 解禁，房间级连接池 RoomMcpAssets 装配）；会话默认启用，
/// 用户可在工具管理页按场景整体禁用（overrides 追加 `mcp:*`）。装配处
/// `is_disabled(profile, MCP_CAP)` 决定是否从 DB 加载并注册 MCP 服务器工具。
pub const MCP_CAP: &str = "mcp:*";

/// 场景差异清单（声明式数据，非代码分支）。
pub struct ToolProfile {
    /// 清单名："session" / "groupchat"（元数据，供未来前端管理页展示/扩展）。
    #[allow(dead_code)]
    pub name: &'static str,
    /// 禁用：精确工具名 或 `xxx:*` 前缀通配（MCP 整体）。静态默认，架构性。
    pub disable: &'static [&'static str],
    /// 用户额外禁用（工具管理页持久化，追加禁用；精确工具名，无通配）。
    pub extra_disable: Vec<String>,
}

impl ToolProfile {
    pub fn session() -> Self {
        Self { name: "session", disable: SESSION_DISABLE, extra_disable: Vec::new() }
    }

    pub fn groupchat() -> Self {
        Self { name: "groupchat", disable: GROUPCHAT_DISABLE, extra_disable: Vec::new() }
    }

    /// 追加用户覆盖的禁用项（来自工具管理页持久化的 overrides）。
    pub fn add_extra_disable(&mut self, names: Vec<String>) {
        for n in names {
            let n = n.trim().to_string();
            if !n.is_empty() && !self.extra_disable.contains(&n) {
                self.extra_disable.push(n);
            }
        }
    }

    /// 按清单名解析（扩展点：未来前端管理页新增清单组合）。
    #[allow(dead_code)]
    pub fn resolve(name: &str) -> Option<Self> {
        match name {
            "session" => Some(Self::session()),
            "groupchat" => Some(Self::groupchat()),
            _ => None,
        }
    }
}

/// 环境聚合（Tauri 命令层持有，注入 build_registry）。
///
/// `db` 不能直接持有 `&rusqlite::Connection`：它非 `Sync`，会让 async
/// `build_registry` 的 future 跨 `await` 时非 `Send`。改持连接池
/// （`r2d2::Pool` 为 `Send + Sync`），在同步段内取连接使用。
pub struct BuildContext<'a> {
    pub env: &'a ToolEnv,
    pub pool: &'a crate::db::init::DbPool,
}

/// 禁用匹配：静态默认清单（精确名 / `xxx:*` 前缀通配）+ 用户额外禁用
/// （精确名，`xxx:*` 后缀同样视为前缀通配，供"整体禁用"类能力项使用）。
pub fn is_disabled(profile: &ToolProfile, tool_name: &str) -> bool {
    is_default_disabled(profile, tool_name)
        || profile.extra_disable.iter().any(|n| {
            if let Some(prefix) = n.strip_suffix('*') {
                tool_name.starts_with(prefix)
            } else {
                n == tool_name
            }
        })
}

/// 是否命中静态默认禁用清单（用于「架构性锁定」判定：默认禁用项不可被用户启用）。
pub fn is_default_disabled(profile: &ToolProfile, tool_name: &str) -> bool {
    profile.disable.iter().any(|entry| {
        if let Some(prefix) = entry.strip_suffix('*') {
            tool_name.starts_with(prefix)
        } else {
            *entry == tool_name
        }
    })
}

/// 全量内建工具（单一事实源）。依赖缺失或 api_format 不满足时对应工具不构造。
/// 将 list_providers 闭包适配为工具内部使用的「按 provider id 取模型名列表」闭包。
/// 返回 Vec<String> 时可直接用于模型白名单校验。
fn make_model_getter(
    list: &Option<Arc<dyn Fn() -> Vec<ProviderModelInfo> + Send + Sync>>,
) -> Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>> {
    list.as_ref().map(|list_fn| {
        let list_fn = list_fn.clone();
        let closure: Arc<dyn Fn(String) -> Vec<String> + Send + Sync> =
            Arc::new(move |provider_id: String| -> Vec<String> {
                let mut result = Vec::new();
                for p in list_fn() {
                    if !provider_id.is_empty() && p.provider_id != provider_id {
                        continue;
                    }
                    for m in &p.models {
                        result.push(m.name.clone());
                    }
                }
                result
            });
        closure
    })
}

pub fn default_tools(env: &ToolEnv) -> Vec<Arc<dyn ToolHandler>> {
    let mut tools: Vec<Arc<dyn ToolHandler>> = Vec::new();
    let cwd = env.cwd.clone();

    // ── 文件系统 ──
    tools.push(Arc::new(read_file::ReadFileTool::new(cwd.clone())));
    tools.push(Arc::new(list_files::ListFilesTool::new(cwd.clone())));
    tools.push(Arc::new(glob::GlobTool::new(cwd.clone())));
    tools.push(Arc::new(grep::GrepTool::new(cwd.clone())));

    // 文件写入副作用服务（历史快照 + diff 事件）经 env 统一注入，会话/群聊共用
    let history = env.file_history.clone();
    let rules = env.permission_rules.clone();
    tools.push(Arc::new(write_file::WriteFileTool::new(cwd.clone(), history.clone(), rules)));
    tools.push(Arc::new(edit_file::EditFileTool::new(cwd.clone(), history)));

    // ── 命令执行 ──
    tools.push(Arc::new(execute_command::ExecuteCommandTool::new(cwd.clone())));
    tools.push(Arc::new(execute_python::ExecutePythonTool::new(cwd.clone())));

    // ── 技能 / 记忆（会话专属能力）──
    if let Some(loader) = &env.skill_loader {
        tools.push(Arc::new(skills::LoadSkillTool::new(loader.as_ref().clone())));
    }
    if let Some(store) = &env.memory_store {
        tools.push(Arc::new(memory::SaveMemoryTool::new(store.as_ref().clone())));
        tools.push(Arc::new(memory::SearchMemoryTool::new(store.as_ref().clone())));
    }
    // 注：todo_write 不在此构造——其状态已事件化到 session_events，构造需 session_id + 连接池，
    // 由 build_registry 在统一装配入口按 profile 注册（会话模式），见 build_registry。

    // ── 图像（仅 OpenAI 兼容格式且有 endpoint 配置）──
    if !matches!(env.api_format, ApiFormat::Anthropic) {
        if let Some((endpoint, key)) = &env.image {
            let model_getter = make_model_getter(&env.list_providers);
            tools.push(Arc::new(generate_image::GenerateImageTool::new(
                endpoint.clone(),
                key.clone(),
                env.resolve_provider.clone(),
                None,
                model_getter.clone(),
            )));
            tools.push(Arc::new(edit_image::EditImageTool::new(
                endpoint.clone(),
                key.clone(),
                env.resolve_provider.clone(),
                model_getter,
            )));
        }
    }

    // ── 语音 / 向量化（仅 OpenAI 兼容格式且有 endpoint 配置；模型经 list_models + resolve_provider）──
    if !matches!(env.api_format, ApiFormat::Anthropic) {
        if let Some((endpoint, key)) = &env.audio {
            let model_getter = make_model_getter(&env.list_providers);
            tools.push(Arc::new(tts::TtsTool::new(
                endpoint.clone(),
                key.clone(),
                env.resolve_provider.clone(),
                model_getter.clone(),
                env.cwd.clone(),
                env.session_id.clone(),
            )));
            tools.push(Arc::new(stt::SttTool::new(
                endpoint.clone(),
                key.clone(),
                env.resolve_provider.clone(),
                model_getter.clone(),
            )));
            tools.push(Arc::new(embed::EmbedTool::new(
                endpoint.clone(),
                key.clone(),
                env.resolve_provider.clone(),
                model_getter,
            )));
        }
    }

    // ── 视频生成（复用语音/图像的 OpenAI 兼容 endpoint 与跨提供商解析；模型经 list_models）──
    if !matches!(env.api_format, ApiFormat::Anthropic) {
        if let Some((endpoint, key)) = &env.audio {
            let model_getter = make_model_getter(&env.list_providers);
            tools.push(Arc::new(generate_video::GenerateVideoTool::new(
                endpoint.clone(),
                key.clone(),
                env.resolve_provider.clone(),
                model_getter,
                env.app.clone(),
                env.session_id.clone(),
            )));
        }
    }

    // ── 文档解析 / 文档创建 ──
    tools.push(Arc::new(parse_document::ParseDocumentTool::new(
        env.cwd.clone(),
        env.session_id.clone(),
    )));
    tools.push(Arc::new(create_document::CreateDocumentTool::new(env.cwd.clone())));

    // ── 模型能力查询（注入数据源时注册；会话/群聊均注入，群聊 v1.2 解禁）──
    if let Some(list) = &env.list_providers {
        tools.push(Arc::new(list_models::ListModelsTool::new(list.clone())));
    }

    // ── 子代理 / 浏览器 ──
    if let Some(client) = &env.client {
        tools.push(Arc::new(subagent::TaskTool::new(
            client.clone(),
            env.model.clone(),
            env.api_format.clone(),
        )));
    }
    tools.push(Arc::new(browser::BrowserTool::new(cwd)));

    // ── 网络 ──
    tools.push(Arc::new(search_web::SearchWebTool::new(env.search_config.clone())));
    tools.push(Arc::new(fetch_web::FetchWebTool::new()));

    // ── 人机交互（会话模式）──
    if let (Some(app), Some(pending)) = (&env.app, &env.pending) {
        tools.push(Arc::new(ask_user::AskUserTool::new(
            app.clone(),
            env.session_id.clone(),
            pending.clone(),
        )));
    }

    tools
}

/// 同步装配核心：全量内建 → 按清单禁用。
/// 不含 MCP（MCP 为 async，见 `build_registry`）。
pub fn assemble_tools(profile: &ToolProfile, env: &ToolEnv) -> Vec<Arc<dyn ToolHandler>> {
    default_tools(env)
        .into_iter()
        .filter(|t| !is_disabled(profile, t.name()))
        .collect()
}

/// 统一装配入口：`assemble_tools` + todo_write 会话注册 + MCP 整体装配。
/// todo_write 因构造需 session_id + 连接池而未进 `default_tools`，本处在过滤后按
/// profile 追加注册（会话模式默认启用；群聊在 GROUPCHAT_DISABLE 中，命中 `is_disabled`
/// 不注册，与 `assemble_tools` 的过滤语义一致）。
/// `profile.disable` 不含 `mcp:*` 时从 DB 加载 MCP 服务器并连接注册。
pub async fn build_registry(profile: &ToolProfile, ctx: &BuildContext<'_>) -> Result<Arc<ToolRegistry>, String> {
    let mut registry = ToolRegistry::new();

    for tool in assemble_tools(profile, ctx.env) {
        registry.register(tool);
    }

    // todo_write（会话专属任务追踪，状态事件化到 session_events）：默认与会话 profile 装配
    if !is_disabled(profile, "todo_write") {
        registry.register(Arc::new(todo_write::TodoWriteTool::new(
            ctx.env.session_id.clone(),
            ctx.pool.clone(),
        )));
    }

    // MCP 整体装配（作为整体进清单，由本处统一处理，不做逐场景编码）
    if !is_disabled(profile, MCP_CAP) {
        // 同步段内取连接并读取配置，守卫在 await 前释放（只跨 await 传递 owned 数据）
        let servers = {
            let conn = ctx
                .pool
                .get()
                .map_err(|e| format!("获取数据库连接失败: {}", e))?;
            crate::commands::mcp::load_servers(&conn).unwrap_or_default()
        };
        // 连接池：装配期连接一次以枚举工具，运行期由 McpToolHandler 懒连接复用 + 失败重连。
        let pool = crate::tools::mcp::McpConnectionPool::new();
        for server in &servers {
            let client = match pool.get_or_connect(server).await {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("[MCP] 连接服务器 {} 失败: {}", server.name, e);
                    continue;
                }
            };
            let server_tools = match client.lock().await.list_tools().await {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("[MCP] 列出工具失败 {}: {}", server.name, e);
                    continue;
                }
            };
            let tool_count = server_tools.len();
            for info in server_tools {
                registry.register(Arc::new(
                    crate::tools::mcp::McpToolHandler::new(pool.clone(), server.clone(), info),
                ));
            }
            log::info!("[MCP] 已加载服务器 {}（{} 个工具）", server.name, tool_count);
        }
    }

    Ok(Arc::new(registry))
}

