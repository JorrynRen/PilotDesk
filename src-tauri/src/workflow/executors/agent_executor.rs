use crate::agent::AgentManager;
use crate::api_agent::agent_loop::SecurityMode;
use crate::commands::agents::get_agent_inner;
use crate::db::init::DbPool;
use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeExecutorTrait, NodeOutput};
use crate::workflow::template::TemplateEngine;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;
use tokio::sync::Mutex as AsyncMutex;

/// Agent 节点（`agent_type == "api"`）新建会话的标题前缀。
const WORKFLOW_SESSION_TITLE_PREFIX: &str = "工作流 · ";

/// 工作流 Agent 节点的系统提示：命中「工具授权」策略的调用会暂停并请求用户审批。
const WORKFLOW_APPROVAL_PROMPT: &str = concat!(
    "<workflow_run>\n",
    "本次运行在工作流节点中，命中「工具授权」策略的工具调用需要用户确认：\n",
    "- 需要审批的调用会暂停等待，用户在超时前未处理时按拒绝执行\n",
    "- 优先使用只读工具（read_file / list_files 等）\n",
    "- 写文件请限定在工作区目录内\n",
    "- 需要更宽权限时，请让用户在节点的「工具授权」里调整后重新运行\n",
    "</workflow_run>"
);

/// 一次等待用户裁决的工具审批请求。
///
/// 与 `InteractManager` 同一范式：这份内存登记表是"此刻是否真有人在等"的唯一权威，
/// 前端查询命令据此还原可操作卡片；进程重启后登记表为空，事件日志里那条请求就成了
/// 只读的失效记录（不可再裁决），避免出现点也点不动的幽灵审批。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingToolApproval {
    pub call_id: String,
    pub execution_id: String,
    pub node_id: String,
    /// 节点名（供界面显示「阶段名·节点名」；阶段名前端按节点 id 反查）
    pub node_label: String,
    pub tool_name: String,
    /// 工具调用参数原文（JSON 字符串）
    pub arguments: String,
    pub risk: String,
    pub created_at: i64,
    /// 进程内已无等待者（进程重启/等待已结束）：只读失效记录
    #[serde(default)]
    pub stale: bool,
}

struct ToolApprovalEntry {
    item: PendingToolApproval,
    tx: oneshot::Sender<bool>,
}

/// 工作流 Agent 节点工具审批的等待登记表（call_id → 等待者）。
pub struct WorkflowApprovalManager {
    pending: std::sync::Mutex<HashMap<String, ToolApprovalEntry>>,
}

impl WorkflowApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 登记一次等待，返回 receiver 供审批线程阻塞等待用户裁决。
    pub fn register(&self, item: PendingToolApproval) -> oneshot::Receiver<bool> {
        let call_id = item.call_id.clone();
        let (tx, rx) = oneshot::channel();
        if let Ok(mut map) = self.pending.lock() {
            map.insert(call_id, ToolApprovalEntry { item, tx });
        }
        rx
    }

    /// 用户裁决：命中则唤醒等待者；未命中（已超时/进程已重启）报错，
    /// 前端据此提示"审批已失效"，而不是假装提交成功。
    pub fn resolve(&self, call_id: &str, approved: bool) -> Result<(), AppError> {
        let entry = self
            .pending
            .lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .remove(call_id)
            .ok_or_else(|| AppError::NotFound(format!("没有等待中的审批: {}", call_id)))?;
        entry
            .tx
            .send(approved)
            .map_err(|_| AppError::External("审批通道已关闭".into()))?;
        Ok(())
    }

    /// 结束等待（超时/取消）：立即移出登记表，使登记表严格等于"此刻有人在等"。
    pub fn forget(&self, call_id: &str) {
        if let Ok(mut map) = self.pending.lock() {
            map.remove(call_id);
        }
    }

    /// 撤销某次执行下所有等待（取消工作流时调用）。
    ///
    /// 只移除 sender、不发裁决：receiver 随即收到 Err，等待中的审批线程按拒绝继续，
    /// 于是被取消的执行不会把线程悬挂到超时（默认 30 分钟）。
    pub fn forget_execution(&self, execution_id: &str) {
        if let Ok(mut map) = self.pending.lock() {
            map.retain(|_, e| e.item.execution_id != execution_id);
        }
    }

    /// 撤销某节点下的等待（节点超时时调用）：登记表严格等于"此刻有人在等"，
    /// 避免节点已判失败、审批卡却还可以点（点了也找不到等待者）。
    pub fn forget_node(&self, execution_id: &str, node_id: &str) {
        if let Ok(mut map) = self.pending.lock() {
            map.retain(|_, e| !(e.item.execution_id == execution_id && e.item.node_id == node_id));
        }
    }

    /// 当前**真在等待**的审批项（前端刷新/切页后据此恢复可操作卡片）。
    pub fn pending_items(&self) -> Vec<PendingToolApproval> {
        match self.pending.lock() {
            Ok(map) => {
                let mut items: Vec<_> = map.values().map(|e| e.item.clone()).collect();
                items.sort_by_key(|i| i.created_at);
                items
            }
            Err(_) => Vec::new(),
        }
    }
}

impl Default for WorkflowApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 工作流 Agent 节点的审批上下文：谁在等、等在哪次执行的哪个节点。
#[derive(Clone)]
pub struct WorkflowApprovalTarget {
    pub execution_id: String,
    pub node_id: String,
    /// 节点名（审批请求里带上，界面显示「阶段名·节点名」用）
    pub node_label: String,
    pub manager: Arc<WorkflowApprovalManager>,
}

pub struct AgentExecutor {
    agent_manager: Arc<AsyncMutex<AgentManager>>,
    pool: DbPool,
    /// 工具审批等待登记表（审批入口在工作流页/通知中心，不弹会话内联卡）
    approvals: Arc<WorkflowApprovalManager>,
}

impl AgentExecutor {
    pub fn new(
        agent_manager: Arc<AsyncMutex<AgentManager>>,
        pool: DbPool,
        approvals: Arc<WorkflowApprovalManager>,
    ) -> Self {
        Self {
            agent_manager,
            pool,
            approvals,
        }
    }
}

#[async_trait]
impl NodeExecutorTrait for AgentExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let agent_type = node
            .config
            .get("agent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("claude");
        let prompt_template = node
            .config
            .get("prompt_template")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 将 resolved_input 转为 HashMap 作为模板上下文
        // resolved_input 已经是 engine.rs 中 resolve_node_input 解析后的确定值
        let prompt = if let Value::Object(map) = &resolved_input {
            let ctx: std::collections::HashMap<String, serde_json::Value> =
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            // 宽松解析：未解析的占位符按空处理（避免把 {{...}} 原文当提示词发给 Agent）
            let (prompt, unresolved) = TemplateEngine::resolve_lossy(prompt_template, &ctx);
            if !unresolved.is_empty() {
                log::warn!(
                    "[AgentExecutor] 节点 {} 提示词模板未解析（按空处理）：{:?}",
                    node.id,
                    unresolved
                );
            }
            prompt
        } else {
            prompt_template.to_string()
        };
        let temp_session_id = format!("wf_{}_{}", execution_id, node.id);

        // 产物目录：`<全局工作区>/outputs/<工作流名>`（与群聊 `<工作区>/outputs/<房间名>` 同构，
        // 按名字归类、不按执行实例分层、不做清理）。
        //
        // 它同时承担三个角色，与群聊一致：节点的 cwd（工具相对路径解析）、AgentLoop 的
        // 授权边界（目录内路径免审批）、以及写进 artifacts_path 的产物落点。
        //
        // 顺带取出本次执行对应的定义 id：新建会话行时写进 `origin`（`workflow:{定义id}`），
        // 供用量统计按工作流归因（执行记录删除后仍可追溯，见 commands/usage.rs）。
        let (definition_id, workspace_dir): (Option<String>, String) = {
            let conn = self
                .pool
                .get()
                .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
            let def = crate::workflow::events::derive_instance(&conn, execution_id)
                .ok()
                .flatten()
                .and_then(|inst| {
                    crate::workflow::get_definition(&conn, &inst.definition_id)
                        .ok()
                        .flatten()
                });
            let (def_id, def_name) = match def {
                Some(d) => (Some(d.id), d.name),
                None => (None, String::new()),
            };
            let dir = crate::utils::paths::workflow_artifacts_dir(&conn, &def_name)
                .to_string_lossy()
                .to_string();
            (def_id, dir)
        };

        // 读取会话延续参数
        let session_mode = node
            .config
            .get("session_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("new");

        // 解析延续会话 session_id：
        // 优先使用已由 engine.rs 通过 TemplateEngine 解析的 resume_session_id
        // 回退到 resume_session_ref 模板解析（兼容直接调用 executor 的路径）
        let mut resolved_session_id = node
            .config
            .get("resume_session_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        if resolved_session_id.is_none() {
            if let Some(ref_tmpl) = node
                .config
                .get("resume_session_ref")
                .and_then(|v| v.as_str())
            {
                if !ref_tmpl.is_empty() {
                    if let Value::Object(map) = &resolved_input {
                        let ctx: std::collections::HashMap<String, serde_json::Value> =
                            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        if let Ok(sid) = TemplateEngine::resolve(ref_tmpl, &ctx) {
                            if !sid.is_empty() {
                                resolved_session_id = Some(sid);
                                log::info!(
                                    "[AgentExecutor] Resolved resume_session_ref '{}' -> '{}'",
                                    ref_tmpl,
                                    resolved_session_id.as_deref().unwrap()
                                );
                            }
                        }
                    }
                }
            }
        }

        let resume_session_id = resolved_session_id.as_deref();

        // ── API Agent 路径：与会话模式的 API 会话同一条执行路径
        //    （AgentLoop + 工具 + 审批 + 流式事件 + 会话落库），不再走 CLI 子进程 ──
        if agent_type == "api" {
            return self
                .execute_api(
                    node,
                    &prompt,
                    &workspace_dir,
                    &temp_session_id,
                    session_mode,
                    resume_session_id,
                    execution_id,
                    emitter,
                    definition_id.as_deref(),
                )
                .await;
        }

        let agent_session_id_param = if session_mode == "resume" {
            resume_session_id
        } else {
            None
        };
        let exec_id = execution_id.to_string();
        let node_id = node.id.clone();
        let emitter_owned = emitter.clone();

        // 从 DB 查询 Agent 配置（复用 agent 会话已实现的方法）
        let agent_config = {
            let conn = self
                .pool
                .get()
                .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
            get_agent_inner(&conn, agent_type)
                .map_err(|e| AppError::Db(e.to_string()))?
                .unwrap_or_else(|| {
                    crate::commands::agents::get_agent_config_by_type(agent_type)
                        .unwrap_or_default()
                })
        };
        let (output, agent_session_id) = self
            .agent_manager
            .lock()
            .await
            .execute_once(
                &agent_config,
                &prompt,
                &Default::default(),
                &workspace_dir,
                &temp_session_id,
                move |chunk| {
                    let _ = emitter_owned.emit(
                        "workflow:chunk",
                        serde_json::json!({
                            "execution_id": exec_id,
                            "node_execution_id": node_id,
                            "content": chunk,
                        }),
                    );
                },
                agent_session_id_param,
            )
            .await?;

        // output 只存 agent 原始返回文本，session_id 放 NodeOutput.session_id 字段
        // input_data 存最终提示词（覆盖 running 时写入的输入映射）
        // artifacts_path 存工作区路径（agent 运行时生成工件的位置）
        Ok(NodeOutput {
            output: Value::String(normalize_agent_output(&output, &node.id)),
            session_id: agent_session_id,
            input_data: Some(prompt.clone()),
            artifacts_path: Some(workspace_dir),
        })
    }
}

impl AgentExecutor {
    /// 以 API Agent 模式执行节点：厂商/模型取自节点配置，目标是新建或延续的会话行。
    /// 执行体与「会话模式的 API 会话」共用 `run_api_agent_inner`（带工具、审批、流式、
    /// 会话落库），节点输出为该轮最终助手文本。
    #[allow(clippy::too_many_arguments)]
    async fn execute_api(
        &self,
        node: &NodeDef,
        prompt: &str,
        workspace_dir: &str,
        temp_session_id: &str,
        session_mode: &str,
        resume_session_id: Option<&str>,
        execution_id: &str,
        emitter: &tauri::AppHandle,
        // 本次执行对应的工作流定义 id（新建会话行写进 origin 供用量归因；derive 失败为 None）
        definition_id: Option<&str>,
    ) -> Result<NodeOutput, AppError> {
        // 延续会话的「模型来源」（仅 resume 生效，默认跟随会话 = 沿用会话行里的模型）：
        //   "node"     → 本轮用节点配置的 厂商/模型 发请求（只延续会话上下文，不改写会话行）
        //   其它/缺省   → 跟随会话自身模型，此时节点上的厂商/模型配置运行期不生效
        let resume_follows_node_model = session_mode == "resume"
            && node
                .config
                .get("resume_model_source")
                .and_then(|v| v.as_str())
                == Some("node");
        let api_provider = node
            .config
            .get("api_provider")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let api_model = node
            .config
            .get("api_model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        // 新建会话要用它建会话行；"延续会话 + 跟随节点模型"要用它发请求。
        // 只有"延续会话 + 跟随会话模型"这一种组合不需要节点上的厂商/模型
        //（前端此时把这两项置灰，所以不能在这里强制要求填写）。
        if session_mode != "resume" || resume_follows_node_model {
            if api_provider.is_none() || api_model.is_none() {
                return Err(AppError::InvalidInput(format!(
                    "Agent 节点「{}」使用 API 模式，请在节点面板中选择厂商与模型（api_provider / api_model）",
                    node.label
                )));
            }
        }
        // 仅本轮运行生效的模型覆盖：跟随节点时用节点配置，跟随会话时交给会话行
        let (provider_override, model_override) = if resume_follows_node_model {
            (api_provider.clone(), api_model.clone())
        } else {
            (None, None)
        };
        // 节点级工具授权：strict/standard/relaxed/unrestricted（兼容中文别名）。
        // 缺省、空串或无法识别的取值一律回退标准模式（解析在 run_api_agent_inner 统一做）。
        let security_mode = node
            .config
            .get("security_mode")
            .and_then(|v| v.as_str())
            .filter(|s| SecurityMode::from_str(s).is_some())
            .map(|s| s.to_string());
        let session_id = if session_mode == "resume" {
            let Some(sid) = resume_session_id else {
                return Err(AppError::InvalidInput(format!(
                    "Agent 节点「{}」选择延续会话，但没有解析出要延续的会话 id（resume_session_ref 未指向具体会话）",
                    node.label
                )));
            };
            let conn = self
                .pool
                .get()
                .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
            let existing = crate::commands::session::get_session_inner(&conn, sid)
                .map_err(|e| AppError::Db(e.to_string()))?;
            if existing.is_none() {
                return Err(AppError::InvalidInput(format!(
                    "Agent 节点「{}」要延续的会话不存在: {}",
                    node.label, sid
                )));
            }
            sid.to_string()
        } else {
            let conn = self
                .pool
                .get()
                .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
            crate::commands::session::create_session_inner(
                &conn,
                "api".to_string(),
                Some(workspace_dir.to_string()),
                Some(format!("{}{}", WORKFLOW_SESSION_TITLE_PREFIX, node.label)),
                api_provider.clone(),
                api_model.clone(),
                None,
                None,
                None,
                // 标记来源：这是工作流节点自动创建的内部会话，不是用户会话。
                // 「延续会话」候选等用户可见列表据此排除（不再依赖标题前缀判定）。
                // 带上定义 id（`workflow:{定义id}`）供用量按工作流归因；derive 失败时回落旧值，
                // 判据一律按前缀 `workflow` 匹配（见 utils/sessionType.ts）。
                Some(match definition_id {
                    Some(id) => format!("workflow:{}", id),
                    None => "workflow".to_string(),
                }),
            )
            .map_err(|e| AppError::Db(e.to_string()))?
            .id
        };
        let pending = emitter
            .try_state::<crate::PendingApprovals>()
            .map(|state| state.inner().clone())
            .ok_or_else(|| AppError::External("PendingApprovals 状态未初始化".into()))?;

        // 运行别名：工作流以 `wf_{execution}_{node}` 取消，而 API 运行登记在真实会话 id 上，
        // 登记别名后 `cancel_workflow` 对两种 Agent 节点用同一个键取消。
        crate::api_agent::session_runs::set_alias(temp_session_id, &session_id);
        // 审批上下文：命中「工具授权」策略的调用会注册到这张登记表并暂停等待用户裁决，
        // 前端在工作流页/通知中心/编辑器里呈现（不弹会话内联卡）。
        let approval_target = WorkflowApprovalTarget {
            execution_id: execution_id.to_string(),
            node_id: node.id.clone(),
            node_label: node.label.clone(),
            manager: self.approvals.clone(),
        };
        // 产物目录写进系统提示：agent 才知道该往哪落盘（工具的相对路径也基于该目录解析）
        let system_prompt = format!(
            "{}\n<workflow_output_dir>\n本次工作流的产物目录：{}\n请把产物写入该目录；相对路径也基于它解析。\n\
             该目录已由系统创建，写文件会自动补齐缺失的上级目录，无需（也不要用 mkdir）创建。\n</workflow_output_dir>",
            WORKFLOW_APPROVAL_PROMPT, workspace_dir
        );
        let result = crate::run_api_agent_inner(
            &self.pool,
            emitter,
            &session_id,
            prompt,
            &[],
            &system_prompt,
            pending,
            None,
            None,
            security_mode,
            true, // persist_turn：节点没有前端调用方，本轮用户提示由后端补写
            Some(approval_target),
            // 产物目录只对本轮运行生效：延续会话时也用它覆盖被续会话的 cwd，
            // 从而与本节点的 cwd/授权边界一致，且不改写会话行。
            Some(workspace_dir.to_string()),
            provider_override,
            model_override,
        )
        .await;
        crate::api_agent::session_runs::clear_alias(temp_session_id);
        let text = result.map_err(AppError::External)?;
        Ok(NodeOutput {
            output: Value::String(normalize_agent_output(&text, &node.id)),
            session_id: Some(session_id),
            input_data: Some(prompt.to_string()),
            artifacts_path: Some(workspace_dir.to_string()),
        })
    }
}

/// Agent 节点执行结果归一化：去掉首尾空行、连续空行折叠为单个空行。
///
/// 模型流式正文常带前导空行（实测同一 provider 的 Agent 节点结果几乎都以 `\n\n` 开头），
/// 会话消息落库时已由 [`crate::commands::session::normalize_message_content`] 归一化，
/// CLI Agent 路径也做了 `trim`——只有工作流节点结果这条路径没有处理，于是那几行空行
/// 只在这里暴露（还会被写进 context / 门控合并值，干扰下游判断与取值）。
fn normalize_agent_output(text: &str, node_id: &str) -> String {
    let normalized = crate::commands::session::normalize_message_content(text);
    if normalized != text {
        log::info!(
            "[AgentExecutor] 节点 {} 执行结果已归一化（去除多余空行）：{} -> {} 字节",
            node_id,
            text.len(),
            normalized.len()
        );
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_output_drops_leading_blank_lines() {
        // 模型正文前导空行（实测常见 `\n\n`）不得进入节点执行结果 / context
        assert_eq!(
            normalize_agent_output("\n\n-0.9899924966004454", "n1"),
            "-0.9899924966004454"
        );
    }

    #[test]
    fn agent_output_keeps_single_blank_line_and_trims_tail() {
        // 段落间的单个空行保留（Markdown 结构），连续空行折叠、尾部空行去除
        assert_eq!(
            normalize_agent_output("第一段\n\n\n\n第二段\n\n", "n1"),
            "第一段\n\n第二段"
        );
        assert_eq!(normalize_agent_output("   \n  \n", "n1"), "");
        assert_eq!(normalize_agent_output("已经干净", "n1"), "已经干净");
    }

    fn approval_item(call_id: &str) -> PendingToolApproval {
        PendingToolApproval {
            call_id: call_id.to_string(),
            execution_id: "ex1".to_string(),
            node_id: "n1".to_string(),
            node_label: "阶段一·任务".to_string(),
            tool_name: "execute_command".to_string(),
            arguments: "{}".to_string(),
            risk: "high".to_string(),
            created_at: 1,
            stale: false,
        }
    }

    /// 登记表必须严格等于"此刻真有人在等"：注册即在、裁决即消、超时撤销即消。
    /// 这是前端把它当可操作卡片展示的依据（否则又会出现点不动的幽灵审批）。
    #[test]
    fn approval_registry_tracks_only_live_waiters() {
        let m = WorkflowApprovalManager::new();
        assert!(m.pending_items().is_empty());

        let mut rx = m.register(approval_item("c1"));
        assert_eq!(m.pending_items().len(), 1);

        m.resolve("c1", true).unwrap();
        assert!(m.pending_items().is_empty());
        assert!(rx.try_recv().unwrap(), "用户裁决应原样送达等待者");

        let _rx2 = m.register(approval_item("c2"));
        m.forget("c2");
        assert!(m.pending_items().is_empty());
    }

    /// 未命中（审批已超时按拒绝续跑 / 进程重启）必须报错：
    /// 前端据此提示"审批已失效"，而不是假装提交成功。
    #[test]
    fn resolve_unknown_call_id_errors() {
        let m = WorkflowApprovalManager::new();
        assert!(m.resolve("missing", true).is_err());
    }

    /// 节点超时时只撤销该节点的审批等待：登记表仍需严格等于"此刻真有人在等"，
    /// 否则节点已判失败、审批卡却还能点（点了找不到等待者）。
    #[test]
    fn forget_node_drops_only_that_node() {
        let m = WorkflowApprovalManager::new();
        let _r1 = m.register(approval_item("c1")); // ex1 / n1
        let mut other = approval_item("c2");
        other.node_id = "n2".to_string();
        let _r2 = m.register(other);
        assert_eq!(m.pending_items().len(), 2);

        m.forget_node("ex1", "n1");
        let left = m.pending_items();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].call_id, "c2");
    }
}
