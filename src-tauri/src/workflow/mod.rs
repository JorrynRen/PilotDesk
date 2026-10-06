pub mod engine;
pub mod executor;
pub mod registry;
pub mod template;

/// 工作流事件读侧派生（workflow_events → 节点状态/输出投影，供旧表删除后的读切点使用）。
pub mod events;

/// 执行记录/看板/恢复等读切点：统一重导出到 `workflow_events` 折叠实现
/// （读侧已全部切至事件投影；旧 `workflow_instances`/`node_executions`/`node_execution_logs`
/// 表 SQL 读侧实现不再保留）。
pub use events::{
    get_execution_timeline, get_node_execution_logs, get_node_type_stats, get_pending_human_inputs,
    get_top_errors, get_top_workflows, get_workflow_stats, list_instances,
    list_recoverable_executions, list_unresolved_tool_approvals,
};
pub mod executors;
pub mod plan;
pub mod scheduler;
pub mod triggers;
use crate::utils::errors::AppError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

// ════════════════════════════════════════════════════════════

// 节点类型 — 精简为 8 种实体节点

// ════════════════════════════════════════════════════════════

/// 实体节点类型（控制逻辑由边/Gate/属性承载）

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowNodeType {
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "api")]
    Api,
    #[serde(rename = "transform")]
    Transform,
    #[serde(rename = "interact")]
    Interact,
    #[serde(rename = "plugin")]
    Plugin,
    #[serde(rename = "subflow")]
    Subflow,
    #[serde(rename = "start")]
    Start,
    #[serde(rename = "end")]
    End,
}

// ════════════════════════════════════════════════════════════

// 触发器配置

// ════════════════════════════════════════════════════════════

/// 触发器类型

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum TriggerType {
    Cron,
    Event,
    Manual,
}

/// 触发器配置（工作流起始属性，非节点）

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerConfig {
    pub trigger_type: TriggerType,
    pub cron: Option<String>,
    pub event_name: Option<String>,
}

// ════════════════════════════════════════════════════════════

// 节点定义

// ════════════════════════════════════════════════════════════

/// 工作流节点

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: WorkflowNodeType,
    pub label: String,
    pub plugin_id: Option<String>,
    pub command_id: Option<String>,
    pub params: Option<serde_json::Value>,

    // 控制属性（附着在实体节点上）
    pub delay_ms: Option<u64>,
    pub timeout_ms: Option<u64>,

    // 输入输出规格
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
    pub input_mapping: Option<serde_json::Value>,
    pub output_mapping: Option<serde_json::Value>,

    // 画布位置
    pub position: Option<serde_json::Value>,
}

// ════════════════════════════════════════════════════════════

// 边定义（数据流 + 控制流）

// ════════════════════════════════════════════════════════════

/// 工作流边

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub label: Option<String>, // 条件标签（如 "score > 0.8"）

    pub condition: Option<String>, // 条件表达式
}

// ════════════════════════════════════════════════════════════

// 阶段门控配置

// ════════════════════════════════════════════════════════════

/// 门控策略

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum GateStrategy {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "count")]
    Count,
    #[serde(rename = "threshold")]
    Threshold,
}

/// 合并策略

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MergeStrategy {
    #[serde(rename = "merge")]
    Merge,
    #[serde(rename = "concat")]
    Concat,
    #[serde(rename = "pick_first")]
    PickFirst,
    #[serde(rename = "pick_last")]
    PickLast,
    #[serde(rename = "custom")]
    Custom,
}

/// 阶段门控配置

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateConfig {
    pub strategy: GateStrategy,
    pub merge_strategy: MergeStrategy,
    /// count 策略时为完成节点数（字符串数字，如 "3"）；threshold 策略时为条件表达式（如 ">= 60"）
    pub threshold: Option<String>,
    /// custom 合并策略的脚本（选择器模式为 `calc:<filter>:<merge_as>:<value_op>`，编辑器模式为 JS 代码）
    pub custom_script: Option<String>,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            strategy: GateStrategy::All,
            merge_strategy: MergeStrategy::Merge,
            threshold: None,
            custom_script: None,
        }
    }
}

// ════════════════════════════════════════════════════════════

// 阶段定义

// ════════════════════════════════════════════════════════════

/// 阶段 — 工作流的基本组织单元

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    pub id: String,
    pub name: String,
    pub order: usize,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    #[serde(default)]
    pub stage_edges: Vec<WorkflowEdge>,
    pub gate: GateConfig,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub offset_x: f64,
    #[serde(default)]
    pub offset_y: f64,
}

// ════════════════════════════════════════════════════════════

// 工作流定义

// ════════════════════════════════════════════════════════════

/// 工作流定义

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinition {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub trigger: TriggerConfig,
    pub stages: Vec<Stage>,
    pub icon: Option<String>,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
    pub enabled: bool,
}

// ════════════════════════════════════════════════════════════

// 工作流实例

// ════════════════════════════════════════════════════════════

/// 工作流实例状态

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowInstanceStatus {
    Pending,
    Running,
    Paused,
    Success,
    Failed,
    Cancelled,
    Timeout,
}

/// 工作流实例

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowInstance {
    pub id: String,
    pub definition_id: String,
    pub definition_name: String,
    pub status: WorkflowInstanceStatus,
    pub context: serde_json::Value,
    pub trigger: String,
    pub trigger_detail: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub completion_rate: f64,

    /// 条件分支未命中 / 上游无产出而跳过的节点数（「已结算」= 完成 + 跳过，见 engine 的
    /// `resolve_terminal_status`）。用于在 UI 上把"100% 但有些节点没跑"讲清楚。
    #[serde(default)]
    pub skipped_count: u32,
    /// 本次执行的**最终产出**（产出卡片取值来源，见 engine 的 `resolve_final_output`）：
    /// End 节点 input_mapping 解析结果 → End 上游节点产出 → 最后一个有产出的节点。
    /// 这里是**未归一化**的原始值，解包包装键 / 识别主正文 / 分块渲染都在前端。
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    /// 产出的取值来源：`end` / `end-upstream` / `last-node`（`none` 表示本次无产出，此时
    /// `output` 为 `None`，用于在卡片上解释"为什么没有结果"）。
    #[serde(default)]
    pub output_source: Option<String>,
    /// 产出所在节点的标签（展示"结果来自「<节点名>」"）
    #[serde(default)]
    pub output_node_label: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
}

// ════════════════════════════════════════════════════════════

// 数据库操作

// ════════════════════════════════════════════════════════════

fn def_from_row(row: &rusqlite::Row) -> rusqlite::Result<WorkflowDefinition> {
    Ok(WorkflowDefinition {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        description: row.get(3)?,
        trigger: serde_json::from_str(&row.get::<_, String>(4)?)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
        stages: serde_json::from_str(&row.get::<_, String>(5)?)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
        input_schema: row
            .get::<_, Option<String>>(6)?
            .and_then(|s| serde_json::from_str(&s).ok()),
        output_schema: row
            .get::<_, Option<String>>(7)?
            .and_then(|s| serde_json::from_str(&s).ok()),
        icon: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        enabled: row.get(11)?,
    })
}

const DEF_COLUMNS: &str = "id, name, version, description, trigger, stages, input_schema, output_schema, icon, created_at, updated_at, enabled";

/// 列出**未删除**的工作流定义（默认视图，回收站内的已软删除项不可见）。
pub fn list_definitions(conn: &Connection) -> Result<Vec<WorkflowDefinition>, AppError> {
    let sql = format!(
        "SELECT {} FROM workflow_definitions WHERE deleted_at IS NULL ORDER BY updated_at DESC",
        DEF_COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let defs = stmt
        .query_map([], |row| def_from_row(row))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(defs)
}

/// 按 id 读取定义——**包含已软删除（回收站）的项**。
///
/// 语义刻意保持"能查到已删除"：回收站的恢复 / 彻底删除需要拿到已删除定义；`deleted_at`
/// 不在返回结构里，调用方若**不应**看到已删除项，请改用 [`get_active_definition`] 或在
/// 调用点用 [`is_definition_deleted`] 过滤，而不是改本函数。
pub fn get_definition(conn: &Connection, id: &str) -> Result<Option<WorkflowDefinition>, AppError> {
    let sql = format!(
        "SELECT {} FROM workflow_definitions WHERE id = ?1",
        DEF_COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![id], |row| def_from_row(row))?;
    match rows.next() {
        Some(Ok(def)) => Ok(Some(def)),
        Some(Err(e)) => Err(AppError::Db(e.to_string())),
        None => Ok(None),
    }
}

/// 按 id 读取**未删除**的定义：已软删除或不存在都返回 `None`。
///
/// 给"运行 / 同步等不应看到回收站项"的调用点使用（见 [`get_definition`] 的说明）。
pub fn get_active_definition(
    conn: &Connection,
    id: &str,
) -> Result<Option<WorkflowDefinition>, AppError> {
    match get_definition(conn, id)? {
        Some(def) if !is_definition_deleted(conn, id)? => Ok(Some(def)),
        _ => Ok(None),
    }
}

/// 该定义是否处于软删除（回收站）状态：仅"存在且 `deleted_at` 非空"返回 true。
pub fn is_definition_deleted(conn: &Connection, id: &str) -> Result<bool, AppError> {
    let deleted: Option<Option<i64>> = conn
        .query_row(
            "SELECT deleted_at FROM workflow_definitions WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(matches!(deleted, Some(Some(_))))
}

/// 运行 / 执行时解析**子工作流**引用：已软删除时给出明确中文错误。
///
/// 与 [`get_definition`] 的区别：这里把"已被删除（在回收站里）"与"根本不存在"分开报，
/// 便于用户从回收站恢复或替换该 Subflow 节点，而不是看到一个含糊的"定义不存在"。
pub fn resolve_subflow_for_run(conn: &Connection, id: &str) -> Result<WorkflowDefinition, AppError> {
    match get_definition(conn, id)? {
        Some(def) if !is_definition_deleted(conn, id)? => Ok(def),
        Some(def) => Err(AppError::InvalidInput(format!(
            "子工作流「{}」已被删除，无法运行；请从回收站恢复或替换该节点",
            if def.name.trim().is_empty() {
                id
            } else {
                def.name.as_str()
            }
        ))),
        None => Err(AppError::InvalidInput(format!(
            "子工作流定义不存在: {}",
            id
        ))),
    }
}

pub fn create_definition(conn: &Connection, def: &WorkflowDefinition) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO workflow_definitions (id, name, version, description, trigger, stages,
         input_schema, output_schema, icon, created_at, updated_at, enabled)

         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            def.id,
            def.name,
            def.version,
            def.description,
            serde_json::to_string(&def.trigger).map_err(|e| AppError::External(e.to_string()))?,
            serde_json::to_string(&def.stages).map_err(|e| AppError::External(e.to_string()))?,
            def.input_schema.as_ref().map(|v| v.to_string()),
            def.output_schema.as_ref().map(|v| v.to_string()),
            def.icon.as_ref().map(|v| v.to_string()),
            now,
            now,
            def.enabled,
        ],
    )?;
    // 本地工作流有改动 → 云同步标脏（best-effort，表缺失/未启用时静默；见 cloud_sync）
    crate::commands::cloud_sync::mark_workflow_dirty(conn, &def.id);
    Ok(())
}

pub fn update_definition(conn: &Connection, def: &WorkflowDefinition) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "UPDATE workflow_definitions SET name = ?1, version = ?2, description = ?3,
         trigger = ?4, stages = ?5, input_schema = ?6, output_schema = ?7,
         icon = ?8, updated_at = ?9, enabled = ?10

         WHERE id = ?11",
        params![
            def.name,
            def.version,
            def.description,
            serde_json::to_string(&def.trigger).map_err(|e| AppError::External(e.to_string()))?,
            serde_json::to_string(&def.stages).map_err(|e| AppError::External(e.to_string()))?,
            def.input_schema.as_ref().map(|v| v.to_string()),
            def.output_schema.as_ref().map(|v| v.to_string()),
            def.icon.as_ref().map(|v| v.to_string()),
            now,
            def.enabled,
            def.id,
        ],
    )?;
    // 本地工作流有改动 → 云同步标脏（best-effort）
    crate::commands::cloud_sync::mark_workflow_dirty(conn, &def.id);
    Ok(())
}

/// **软删除**工作流定义：写 `deleted_at`（进回收站），不真删任何数据。
///
/// 用于「用户主动删除」与「云同步的远端删除传播」——两者都只是把定义移入回收站，
/// 之后可由 [`restore_workflow`] 恢复，或由 [`purge_workflow`] / [`empty_recycle_bin`] 彻底删除。
/// 幂等：已删除（`deleted_at` 非空）时不再改动时间戳。
pub fn soft_delete_definition(conn: &Connection, id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE workflow_definitions SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
        params![crate::utils::now(), id],
    )?;
    // 本地删除 → 云同步标脏（远端删除传播时会随后把 dirty 复位，不产生回推）
    crate::commands::cloud_sync::mark_workflow_dirty(conn, id);
    Ok(())
}

/// **彻底删除**工作流定义，并级联清理其 Agent 节点自动创建的内部会话与用量
/// （口径与「删会话即清用量」一致：删实体即清用量，避免统计里留下指向已删定义的悬空归因）。
///
/// 用于内部**替换 / 重建**路径（云同步 v1 就地覆盖的旧子流清理、市场重装覆盖旧定义）与
/// 回收站的「彻底删除」；**不会**进入回收站。用户主动删除请用 [`soft_delete_definition`]。
///
/// 用量绑定的是**定义**而不是实例：删执行记录只影响 `workflow_events`，定义仍存在，
/// 故统计保留（见 `commands::workflow::delete_executions_inner`）；只有删定义才走到这里清理。
///
/// 节点会话的来源标记为 `workflow:{定义id}`（见 `workflow::executors::agent_executor`）；
/// 更早的历史行只记 `workflow`、不含定义 id，无法归属到具体定义，故不在清理范围内。
pub fn hard_delete_definition(conn: &Connection, id: &str) -> Result<(), AppError> {
    let origin = format!("workflow:{}", id);
    let session_ids: Vec<String> = conn
        .prepare("SELECT id FROM sessions WHERE origin = ?1")?
        .query_map(params![origin], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    for sid in &session_ids {
        conn.execute(
            "DELETE FROM session_events WHERE session_id = ?1",
            params![sid],
        )?;
        conn.execute(
            "DELETE FROM api_usage_log WHERE session_id = ?1",
            params![sid],
        )?;
    }
    conn.execute("DELETE FROM sessions WHERE origin = ?1", params![origin])?;
    conn.execute(
        "DELETE FROM workflow_definitions WHERE id = ?1",
        params![id],
    )?;
    // 本地删除 → 云同步标脏（sync_state 行保留；pull 端删除后会把 dirty 复位，不产生回推）
    crate::commands::cloud_sync::mark_workflow_dirty(conn, id);
    Ok(())
}

// ── 回收站 ──

/// 回收站条目（软删除的工作流）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedWorkflow {
    pub id: String,
    pub name: String,
    /// 删除时间（秒级 Unix 时间戳，与 created_at / updated_at 同口径）
    pub deleted_at: i64,
    /// 节点总数（便于界面展示规模，低成本统计）
    pub node_count: usize,
    /// 其中 Subflow 节点数
    pub subflow_count: usize,
}

/// 列出回收站（已软删除）的工作流，按删除时间倒序。
pub fn list_deleted_workflows(conn: &Connection) -> Result<Vec<DeletedWorkflow>, AppError> {
    let sql = format!(
        "SELECT {}, deleted_at FROM workflow_definitions
         WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC",
        DEF_COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |row| {
            let def = def_from_row(row)?;
            let deleted_at: Option<i64> = row.get(12)?;
            let node_count = def.stages.iter().map(|s| s.nodes.len()).sum();
            let subflow_count = def
                .stages
                .iter()
                .flat_map(|s| s.nodes.iter())
                .filter(|n| n.node_type == WorkflowNodeType::Subflow)
                .count();
            Ok(DeletedWorkflow {
                id: def.id,
                name: def.name,
                deleted_at: deleted_at.unwrap_or(0),
                node_count,
                subflow_count,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 取一个不与现有**未删除**工作流重名的名称；重名时自动追加序号（「名称 (2)」「名称 (3)」…）。
fn unique_workflow_name(conn: &Connection, base: &str) -> Result<String, AppError> {
    let base = base.trim();
    let base = if base.is_empty() { "未命名工作流" } else { base };
    let alive: std::collections::HashSet<String> =
        list_definitions(conn)?.into_iter().map(|d| d.name).collect();
    if !alive.contains(base) {
        return Ok(base.to_string());
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{} ({})", base, n);
        if !alive.contains(&candidate) {
            return Ok(candidate);
        }
        n += 1;
    }
}

/// 从回收站恢复工作流：清 `deleted_at`。
///
/// 若与现有未删除工作流**重名**，自动追加序号（如「名称 (2)」）。返回 `(id, 最终名称)`。
/// 幂等：若该工作流本就未删除，直接返回当前名称，不做改动。
pub fn restore_workflow(conn: &Connection, id: &str) -> Result<(String, String), AppError> {
    let def = get_definition(conn, id)?
        .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?;
    if !is_definition_deleted(conn, id)? {
        return Ok((id.to_string(), def.name));
    }
    let name = unique_workflow_name(conn, &def.name)?;
    conn.execute(
        "UPDATE workflow_definitions SET name = ?1, deleted_at = NULL, updated_at = ?2 WHERE id = ?3",
        params![name, crate::utils::now(), id],
    )?;
    // 恢复 = 本地又存在该工作流 → 标脏，下一轮同步把它推回远端
    crate::commands::cloud_sync::mark_workflow_dirty(conn, id);
    // 恢复是**用户的显式意图** → 视为「本地有真实改动」：清空该 key 的 last_synced_hash，
    // 确保下轮 push 把它作为新版本推上去（**云端复活**），解决「恢复后同步又被删」。
    crate::commands::cloud_sync::mark_workflow_restored(conn, id);
    Ok((id.to_string(), name))
}

/// 彻底删除回收站中的某个工作流（真删，连同版本记录级联清理）。
///
/// 仅允许删除**已在回收站中**的项，避免绕过用户删除的引用守卫直接抹掉活跃工作流。
pub fn purge_workflow(conn: &Connection, id: &str) -> Result<(), AppError> {
    if !is_definition_deleted(conn, id)? {
        return Err(AppError::InvalidInput(
            "该工作流不在回收站中，无法彻底删除".to_string(),
        ));
    }
    hard_delete_definition(conn, id)?;
    // 彻底删除 → 保留 sync_state 行并置墓碑（阻止该 key 被 pull 复活）；墓碑永久保留，不做自动清理。
    crate::commands::cloud_sync::mark_workflow_purged(conn, id);
    Ok(())
}

/// 清空回收站：逐个彻底删除，返回实际清除数量。
pub fn empty_recycle_bin(conn: &Connection) -> Result<usize, AppError> {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM workflow_definitions WHERE deleted_at IS NOT NULL")?
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    let mut purged = 0usize;
    for id in &ids {
        hard_delete_definition(conn, id)?;
        // 同 purge_workflow：置墓碑，阻止已彻底删除的 key 被 pull 复活
        crate::commands::cloud_sync::mark_workflow_purged(conn, id);
        purged += 1;
    }
    Ok(purged)
}

// ── 实例操作 ──

pub fn create_instance(conn: &Connection, instance: &WorkflowInstance) -> Result<(), AppError> {
    // 事件化：实例创建（execution/created，事件唯一事实源）。
    let created_status = match &instance.status {
        WorkflowInstanceStatus::Pending => "pending",
        WorkflowInstanceStatus::Running => "running",
        WorkflowInstanceStatus::Paused => "paused",
        WorkflowInstanceStatus::Success => "success",
        WorkflowInstanceStatus::Failed => "failed",
        WorkflowInstanceStatus::Cancelled => "cancelled",
        WorkflowInstanceStatus::Timeout => "timeout",
    };
    let event = serde_json::json!({
        "executionId": instance.id,
        "definitionId": instance.definition_id,
        "definitionName": instance.definition_name,
        "status": created_status,
        "trigger": instance.trigger,
        "triggerDetail": instance.trigger_detail,
        "startedAt": instance.started_at,
        "createdAt": crate::utils::now(),
    });
    crate::eventlog::append_workflow_event(conn, &instance.id, "execution/created", &event, true)?;
    Ok(())
}

// ════════════════════════════════════════════════════════════

// 统计查询

// ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStats {
    pub total_executions: i64,
    pub success_count: i64,
    pub failed_count: i64,
    pub cancelled_count: i64,
    pub running_count: i64,
    pub pending_count: i64,
    pub paused_count: i64,
    pub timeout_count: i64,
    pub success_rate: f64,

    /// 以下耗时字段均为 `Option`：`null` = 无可用样本（无已终态执行），
    /// `0` = 该执行不足 1 秒（事件时间戳为秒级，短执行会取整为 0）
    pub avg_duration_ms: Option<f64>,
    pub max_duration_ms: Option<i64>,
    pub min_duration_ms: Option<i64>,
    pub total_node_executions: i64,
    pub node_failed_count: i64,
    pub last_7_days_count: i64,
    pub last_30_days_count: i64,

    /// 最近 N 天执行次数（N 由 days 参数决定，未传则等于 30）
    pub last_n_days_count: i64,

    /// 统计所基于的天数窗口（前端用于显示标签）
    pub days: i64,

    /// 区间内（days 窗口）的执行次数（= last_n_days_count，冗余便于前端字段命名一致）
    pub range_total: i64,

    /// 区间内成功次数
    pub range_success: i64,

    /// 区间内失败次数
    pub range_failed: i64,

    /// 区间内取消次数
    pub range_cancelled: i64,

    /// 区间内运行中次数
    pub range_running: i64,

    /// 区间内待触发次数
    pub range_pending: i64,

    /// 区间内已暂停次数
    pub range_paused: i64,

    /// 区间内超时次数
    pub range_timeout: i64,

    /// 区间内成功率（0-100）
    pub range_success_rate: f64,

    /// 区间内平均耗时（ms；null = 无样本）
    pub range_avg_duration_ms: Option<f64>,

    /// 区间内最长耗时（ms；null = 无样本）
    pub range_max_duration_ms: Option<i64>,

    /// 区间内最短耗时（ms；null = 无样本）
    pub range_min_duration_ms: Option<i64>,
}

// ════════════════════════════════════════════════════════════

// 执行时间线

// ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionTimelinePoint {
    pub date: String,
    pub total: i64,
    pub success: i64,
    pub failed: i64,
    pub cancelled: i64,

    /// 该分桶内的平均耗时（ms；null = 该分桶无已完成执行）
    pub avg_duration_ms: Option<f64>,

    /// Aggregation granularity: 'day' | 'week' | 'month'
    pub granularity: String,
}

// ════════════════════════════════════════════════════════════

// 节点类型统计

// ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTypeStat {
    pub node_type: String,
    pub count: i64,
    pub failed_count: i64,
    pub avg_duration_ms: f64,
}

// ════════════════════════════════════════════════════════════

// Top 工作流排行 / Top 错误聚合

// ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopWorkflowStat {
    pub definition_id: String,
    pub definition_name: String,
    pub total: i64,
    pub success: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub failed_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopErrorStat {
    pub error: String,
    pub count: i64,
    pub last_occurred_at: i64,
}

// ════════════════════════════════════════════════════════════

// 版本管理

// ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowVersion {
    pub id: String,
    pub workflow_id: String,
    pub version: i64,
    pub snapshot: String,
    pub created_at: i64,
    /// 快照来源：`manual`（用户手动保存）/ `cloud_sync`（云同步自动快照）/ `import`（导入自动快照）。
    #[serde(default = "default_version_origin")]
    pub origin: String,
}

fn default_version_origin() -> String {
    VERSION_ORIGIN_MANUAL.to_string()
}

/// 用户手动保存的版本（永久保留，不参与自动清理）。
pub const VERSION_ORIGIN_MANUAL: &str = "manual";
/// 云同步覆盖前的自动快照。
pub const VERSION_ORIGIN_CLOUD_SYNC: &str = "cloud_sync";
/// 导入覆盖前的自动快照。
pub const VERSION_ORIGIN_IMPORT: &str = "import";
/// 自动快照（cloud_sync / import）每来源保留的最近条数；手动版本不受此限制。
pub const AUTO_VERSION_KEEP: i64 = 3;

/// 比较内容时剔除的「易变 / 元数据」字段：时间戳与版本号不应触发快照。
const VOLATILE_CONTENT_KEYS: &[&str] = &[
    "updatedAt",
    "updated_at",
    "createdAt",
    "created_at",
    "version",
];

/// 把 JSON 递归规范化（对象键排序、剔除易变字段、数组保持顺序）后写入 `out`。
fn canonicalize_json(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            let mut first = true;
            for k in keys {
                if VOLATILE_CONTENT_KEYS.contains(&k.as_str()) {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&serde_json::Value::String(k.clone()).to_string());
                out.push(':');
                canonicalize_json(&map[k], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonicalize_json(v, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// 规范化内容哈希（FNV-1a 64 位）。
///
/// 用途：判断「内容是否真的变了」——字段顺序不同、易变字段（updatedAt/createdAt/version 等）不同
/// 都不算改动；不依赖原始 JSON 文本比对，避免序列化顺序差异造成误判。
pub(crate) fn normalized_content_hash(value: &serde_json::Value) -> u64 {
    let mut s = String::new();
    canonicalize_json(value, &mut s);
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn duplicate_definition(
    conn: &Connection,
    id: &str,
    new_name: &str,
) -> Result<WorkflowDefinition, AppError> {
    let original =
        get_definition(conn, id)?.ok_or_else(|| AppError::NotFound("工作流不存在".into()))?;
    let mut new_def = original;
    new_def.id = crate::utils::new_id();
    new_def.name = new_name.to_string();
    new_def.version = "1.0.0".to_string();
    new_def.created_at = crate::utils::now();
    new_def.updated_at = new_def.created_at;
    create_definition(conn, &new_def)?;
    Ok(new_def)
}

pub fn list_workflow_versions(
    conn: &Connection,
    workflow_id: &str,
) -> Result<Vec<WorkflowVersion>, AppError> {
    let mut stmt = conn.prepare(

        "SELECT id, workflow_id, version, snapshot, created_at, origin FROM workflow_versions WHERE workflow_id = ?1 ORDER BY version DESC"

    )?;
    let versions = stmt
        .query_map(params![workflow_id], |row| {
            Ok(WorkflowVersion {
                id: row.get(0)?,
                workflow_id: row.get(1)?,
                version: row.get(2)?,
                snapshot: row.get(3)?,
                created_at: row.get(4)?,
                origin: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(versions)
}

pub fn save_workflow_version(
    conn: &Connection,
    workflow_id: &str,
    snapshot: &str,
    origin: &str,
) -> Result<WorkflowVersion, AppError> {
    let _def = get_definition(conn, workflow_id)?
        .ok_or_else(|| AppError::NotFound("工作流不存在".into()))?;

    // 查询当前最大版本号，+1 作为新版本号
    let max_version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM workflow_versions WHERE workflow_id = ?1",
        params![workflow_id],
        |row| row.get(0),
    )?;
    let new_version = max_version + 1;
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO workflow_versions (id, workflow_id, version, snapshot, created_at, origin) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, workflow_id, new_version, snapshot, now, origin],
    )?;

    // 回写 definition.version 保持同步
    conn.execute(
        "UPDATE workflow_definitions SET version = ?1 WHERE id = ?2",
        params![new_version.to_string(), workflow_id],
    )?;
    Ok(WorkflowVersion {
        id,
        workflow_id: workflow_id.to_string(),
        version: new_version,
        snapshot: snapshot.to_string(),
        created_at: now,
        origin: origin.to_string(),
    })
}

/// 保存自动快照：**仅当内容与最新版本不同**时才保存（内容未变化时不打快照，避免版本表膨胀）。
///
/// 保存后按 `origin` 分组清理：`cloud_sync` / `import` 各自只保留最近 [`AUTO_VERSION_KEEP`] 个；
/// `manual` 不清理（用户手动保存的版本永久保留）。返回是否真的保存了新快照。
pub(crate) fn save_version_snapshot_if_changed(
    conn: &Connection,
    workflow_id: &str,
    def: &WorkflowDefinition,
    origin: &str,
) -> Result<bool, AppError> {
    let current = serde_json::to_value(def).map_err(|e| AppError::Json(e.to_string()))?;
    let hash = normalized_content_hash(&current);
    let latest: Option<String> = conn
        .query_row(
            "SELECT snapshot FROM workflow_versions WHERE workflow_id = ?1 ORDER BY version DESC LIMIT 1",
            params![workflow_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(snap) = latest {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&snap) {
            if normalized_content_hash(&v) == hash {
                return Ok(false);
            }
        }
    }
    let snapshot = serde_json::to_string(def).map_err(|e| AppError::Json(e.to_string()))?;
    save_workflow_version(conn, workflow_id, &snapshot, origin)?;
    prune_auto_versions(conn, workflow_id, origin)?;
    Ok(true)
}

/// 覆盖本地已有工作流内容前，先把**当前内容**存为版本快照（内容与最新版本相同则跳过）。
///
/// 所有「用新内容覆盖本地已有工作流内容」的路径（云同步 pull / 导入 / 就地更新）都应先调用它，
/// 保证被覆盖的旧内容可回溯。工作流不存在时返回 `false`（无可备份内容）。
pub(crate) fn snapshot_before_overwrite(
    conn: &Connection,
    workflow_id: &str,
    origin: &str,
) -> Result<bool, AppError> {
    let Some(def) = get_definition(conn, workflow_id)? else {
        return Ok(false);
    };
    save_version_snapshot_if_changed(conn, workflow_id, &def, origin)
}

/// 按来源清理自动快照：`manual` 不清理；其余来源各只保留最近 [`AUTO_VERSION_KEEP`] 个。
/// 幂等：`DELETE ... NOT IN (最近 N 个)`，同连接内执行。
fn prune_auto_versions(
    conn: &Connection,
    workflow_id: &str,
    origin: &str,
) -> Result<(), AppError> {
    if origin == VERSION_ORIGIN_MANUAL {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM workflow_versions
          WHERE workflow_id = ?1 AND origin = ?2
            AND version NOT IN (
                SELECT version FROM workflow_versions
                 WHERE workflow_id = ?1 AND origin = ?2
                 ORDER BY version DESC LIMIT ?3)",
        params![workflow_id, origin, AUTO_VERSION_KEEP],
    )?;
    Ok(())
}

pub fn restore_workflow_version(
    conn: &Connection,
    workflow_id: &str,
    version: i64,
) -> Result<WorkflowVersion, AppError> {
    let mut stmt = conn.prepare(

        "SELECT id, workflow_id, version, snapshot, created_at, origin FROM workflow_versions WHERE workflow_id = ?1 AND version = ?2"

    )?;
    let ver = stmt.query_row(params![workflow_id, version], |row| {
        Ok(WorkflowVersion {
            id: row.get(0)?,
            workflow_id: row.get(1)?,
            version: row.get(2)?,
            snapshot: row.get(3)?,
            created_at: row.get(4)?,
            origin: row.get(5)?,
        })
    })?;
    let snapshot: WorkflowDefinition =
        serde_json::from_str(&ver.snapshot).map_err(|e| AppError::Json(e.to_string()))?;
    update_definition(conn, &snapshot)?;
    Ok(ver)
}

/// 删除指定版本快照
pub fn delete_workflow_version(
    conn: &Connection,
    workflow_id: &str,
    version: i64,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM workflow_versions WHERE workflow_id = ?1 AND version = ?2",
        params![workflow_id, version],
    )?;
    Ok(())
}

// ════════════════════════════════════════════════════════════

// 节点执行日志

// ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeExecutionLogEntry {
    pub id: i64,
    pub execution_id: String,
    pub node_execution_id: String,
    pub timestamp: i64,
    pub level: String,
    pub message: String,
    pub metadata: Option<String>,
}

// ════════════════════════════════════════════════════════════

// 执行恢复

// ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverableExecution {
    pub execution: WorkflowInstance,
    pub completed_nodes: Vec<String>,
    pub failed_nodes: Vec<String>,
    pub pending_nodes: Vec<String>,
}

// ════════════════════════════════════════════════════════════

// 人工介入查询

// ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingHumanInput {
    pub execution_id: String,
    pub node_id: String,
    pub node_label: String,
    pub prompt: String,
    pub input_type: String,
    pub created_at: i64,
}

// Re-export engine types for commands

pub use engine::{ExecutionMode, ValidationResult};

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE workflow_definitions (id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '');
             CREATE TABLE sessions (id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', origin TEXT);
             CREATE TABLE session_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
                kind TEXT NOT NULL, payload TEXT NOT NULL DEFAULT '{}',
                model_visible INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL);
             CREATE TABLE api_usage_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
                prompt_tokens INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
             INSERT INTO workflow_definitions (id, name) VALUES ('d1', '数值计算'), ('d2', '其它工作流');
             INSERT INTO sessions (id, title, origin) VALUES
                ('s1', '工作流 · 节点A', 'workflow:d1'),
                ('s2', '工作流 · 节点B', 'workflow:d1'),
                ('s3', '用户会话', NULL),
                ('s4', '工作流 · 历史节点', 'workflow');
             INSERT INTO session_events (session_id, kind, created_at) VALUES ('s1', 'user/message', 1);
             INSERT INTO api_usage_log (session_id, prompt_tokens, created_at) VALUES
                ('s1', 100, 1), ('s2', 200, 1), ('s3', 7, 1), ('s4', 5, 1);",
        )
        .unwrap();
        conn
    }

    /// 删定义须级联清理其节点会话（含事件）与用量；用户会话、历史无定义 id 的会话不受影响。
    #[test]
    fn hard_delete_definition_cascades_node_sessions_and_usage() {
        let conn = mem_conn();
        hard_delete_definition(&conn, "d1").unwrap();

        let sessions: Vec<String> = conn
            .prepare("SELECT id FROM sessions ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(sessions, vec!["s3".to_string(), "s4".to_string()]);

        let usage: Vec<(String, i64)> = conn
            .prepare("SELECT session_id, prompt_tokens FROM api_usage_log ORDER BY session_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(usage, vec![("s3".to_string(), 7), ("s4".to_string(), 5)]);

        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0, "被删会话的事件一并清理");

        let defs: i64 = conn
            .query_row("SELECT COUNT(*) FROM workflow_definitions", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(defs, 1, "其它定义不受影响");
    }

    /// 待响应的入口是 `get_pending_human_inputs` 命令，前端按这份 JSON 的键名取值并回传
    /// （`respond_human_input(executionId, nodeId, response)`）。
    ///
    /// 这里锁死键名形状：曾经前端类型写成 snake_case，`execution_id` 取到 undefined，
    /// 序列化参数时整个键被丢掉，后端报 `missing required key executionId`，
    /// 「指挥中心 → 待处理」里的人工交互怎么点都提交不上去。
    #[test]
    fn pending_human_input_serializes_camel_case_for_frontend() {
        let item = PendingHumanInput {
            execution_id: "ex1".to_string(),
            node_id: "n1".to_string(),
            node_label: "确认偏好".to_string(),
            prompt: "请选择风格".to_string(),
            input_type: "select".to_string(),
            created_at: 1,
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["executionId"], "ex1");
        assert_eq!(v["nodeId"], "n1");
        assert_eq!(v["nodeLabel"], "确认偏好");
        assert_eq!(v["prompt"], "请选择风格");
        assert_eq!(v["inputType"], "select");
        assert_eq!(v["createdAt"], 1);
        assert!(
            v.get("execution_id").is_none(),
            "前端按 camelCase 取值，不能同时出现 snake_case 键"
        );
    }

    // ── 回收站 / 软删除 ──

    /// 完整 schema 的内存库（workflow_definitions 全列 + workflow_versions 外键），
    /// 并开启外键约束以验证「彻底删除 → 版本记录级联清理」。
    fn full_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::init::FINAL_SCHEMA_SQL)
            .unwrap();
        conn
    }

    /// 造一个含单个 Subflow 节点（引用 `subflow_of`）的工作流定义；`subflow_of` 为 None 时无节点。
    fn def_with_subflow(id: &str, name: &str, subflow_of: Option<&str>) -> WorkflowDefinition {
        let nodes = match subflow_of {
            Some(target) => vec![WorkflowNode {
                id: crate::utils::new_id(),
                node_type: WorkflowNodeType::Subflow,
                label: "子流".into(),
                plugin_id: None,
                command_id: None,
                params: Some(serde_json::json!({ "definitionId": target })),
                delay_ms: None,
                timeout_ms: None,
                input_schema: None,
                output_schema: None,
                input_mapping: None,
                output_mapping: None,
                position: None,
            }],
            None => vec![],
        };
        WorkflowDefinition {
            id: id.into(),
            name: name.into(),
            version: "1.0.0".into(),
            description: String::new(),
            trigger: TriggerConfig {
                trigger_type: TriggerType::Manual,
                cron: None,
                event_name: None,
            },
            stages: vec![Stage {
                id: crate::utils::new_id(),
                name: "默认阶段".into(),
                order: 0,
                nodes,
                edges: vec![],
                stage_edges: vec![],
                gate: GateConfig::default(),
                collapsed: false,
                offset_x: 0.0,
                offset_y: 0.0,
            }],
            icon: None,
            input_schema: None,
            output_schema: None,
            created_at: 1,
            updated_at: 1,
            enabled: true,
        }
    }

    /// 软删除后不出现在列表、但回收站可见；恢复后重新出现。
    #[test]
    fn soft_delete_hides_from_list_and_restore_brings_back() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("d1", "被删工作流", None)).unwrap();

        soft_delete_definition(&conn, "d1").unwrap();
        assert!(
            list_definitions(&conn).unwrap().is_empty(),
            "软删除后不应出现在工作流列表"
        );
        assert!(
            get_definition(&conn, "d1").unwrap().is_some(),
            "get_definition 仍应能查到（回收站需要）"
        );
        assert!(
            get_active_definition(&conn, "d1").unwrap().is_none(),
            "活跃定义查询应过滤已删除"
        );
        assert!(is_definition_deleted(&conn, "d1").unwrap());

        let deleted = list_deleted_workflows(&conn).unwrap();
        assert_eq!(deleted.len(), 1, "回收站应可见该工作流");
        assert_eq!(deleted[0].id, "d1");
        assert_eq!(deleted[0].name, "被删工作流");

        // 幂等：重复软删除不报错、仍为已删除
        soft_delete_definition(&conn, "d1").unwrap();
        assert_eq!(list_deleted_workflows(&conn).unwrap().len(), 1);

        let (id, name) = restore_workflow(&conn, "d1").unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("d1", "被删工作流"));
        assert_eq!(list_definitions(&conn).unwrap().len(), 1, "恢复后重新出现");
        assert!(list_deleted_workflows(&conn).unwrap().is_empty());
        assert!(!is_definition_deleted(&conn, "d1").unwrap());
    }

    /// 恢复时重名 → 自动追加序号。
    #[test]
    fn restore_appends_suffix_on_name_conflict() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("alive", "同名工作流", None)).unwrap();
        create_definition(&conn, &def_with_subflow("dead", "同名工作流", None)).unwrap();
        soft_delete_definition(&conn, "dead").unwrap();

        let (id, name) = restore_workflow(&conn, "dead").unwrap();
        assert_eq!(id, "dead");
        assert_eq!(name, "同名工作流 (2)");
        let names: Vec<String> = list_definitions(&conn)
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(names.contains(&"同名工作流".to_string()));
        assert!(names.contains(&"同名工作流 (2)".to_string()));
    }

    /// 回收站条目附带节点 / 子流统计，且按删除时间倒序。
    #[test]
    fn list_deleted_workflows_reports_stats_newest_first() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("sub", "子流", None)).unwrap();
        create_definition(&conn, &def_with_subflow("main", "主流程", Some("sub"))).unwrap();
        soft_delete_definition(&conn, "sub").unwrap();
        soft_delete_definition(&conn, "main").unwrap();
        // 直接指定删除时间以稳定断言排序（秒级时间戳下连续删除可能同秒）
        conn.execute(
            "UPDATE workflow_definitions SET deleted_at = 100 WHERE id = 'sub'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE workflow_definitions SET deleted_at = 200 WHERE id = 'main'",
            [],
        )
        .unwrap();

        let deleted = list_deleted_workflows(&conn).unwrap();
        assert_eq!(deleted.len(), 2);
        assert_eq!(deleted[0].id, "main", "按删除时间倒序（最新在前）");
        assert_eq!(deleted[0].node_count, 1);
        assert_eq!(deleted[0].subflow_count, 1);
        assert_eq!(deleted[1].id, "sub");
        assert_eq!(deleted[1].node_count, 0);
        assert_eq!(deleted[1].subflow_count, 0);
    }

    /// 彻底删除：定义与其版本记录一并消失；未进回收站的不允许排除。
    #[test]
    fn purge_removes_definition_and_cascades_versions() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("d1", "带版本工作流", None)).unwrap();
        save_workflow_version(&conn, "d1", "{}", VERSION_ORIGIN_MANUAL).unwrap();
        assert_eq!(list_workflow_versions(&conn, "d1").unwrap().len(), 1);

        // 未进回收站 → 拒绝彻底删除（防止绕过用户删除守卫）
        assert!(purge_workflow(&conn, "d1").is_err());

        soft_delete_definition(&conn, "d1").unwrap();
        purge_workflow(&conn, "d1").unwrap();

        assert!(get_definition(&conn, "d1").unwrap().is_none(), "定义应真删");
        assert_eq!(
            list_workflow_versions(&conn, "d1").unwrap().len(),
            0,
            "版本记录应随定义级联清理"
        );
        assert!(list_deleted_workflows(&conn).unwrap().is_empty());
    }

    /// 清空回收站：逐条彻底删除，未删除项不受影响。
    #[test]
    fn empty_recycle_bin_purges_all_deleted() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("a", "甲", None)).unwrap();
        create_definition(&conn, &def_with_subflow("b", "乙", None)).unwrap();
        create_definition(&conn, &def_with_subflow("c", "丙", None)).unwrap();
        soft_delete_definition(&conn, "a").unwrap();
        soft_delete_definition(&conn, "b").unwrap();

        let purged = empty_recycle_bin(&conn).unwrap();
        assert_eq!(purged, 2);
        assert!(list_deleted_workflows(&conn).unwrap().is_empty());
        assert!(get_definition(&conn, "a").unwrap().is_none());
        assert!(get_definition(&conn, "b").unwrap().is_none());
        assert!(get_definition(&conn, "c").unwrap().is_some(), "未删除项不受影响");
    }

    /// 内部替换路径走彻底删除：不进回收站。
    #[test]
    fn hard_delete_does_not_enter_recycle_bin() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("d1", "内部替换的旧定义", None)).unwrap();
        hard_delete_definition(&conn, "d1").unwrap();
        assert!(get_definition(&conn, "d1").unwrap().is_none());
        assert!(
            list_deleted_workflows(&conn).unwrap().is_empty(),
            "内部替换 / 重建不进回收站"
        );
    }

    /// 运行解析子流：已被软删除时报出明确中文错误；不存在时报不存在。
    #[test]
    fn resolve_subflow_for_run_reports_deleted_subflow() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("sub", "数值计算子流", None)).unwrap();

        // 未删除：正常解析
        assert_eq!(resolve_subflow_for_run(&conn, "sub").unwrap().id, "sub");

        soft_delete_definition(&conn, "sub").unwrap();
        let err = resolve_subflow_for_run(&conn, "sub").unwrap_err().to_string();
        assert!(err.contains("数值计算子流"), "错误应含子流名: {}", err);
        assert!(err.contains("已被删除"), "错误应说明已被删除: {}", err);
        assert!(err.contains("回收站"), "错误应引导回收站恢复: {}", err);

        let missing = resolve_subflow_for_run(&conn, "nope").unwrap_err().to_string();
        assert!(missing.contains("不存在"), "缺失应报不存在: {}", missing);
    }

    // ── 批次 3：版本快照 ──

    /// 规范化哈希：字段顺序不同 / 易变字段（时间戳、版本号）不同 → 视为内容相同；
    /// 实际内容不同 → 视为有改动。
    #[test]
    fn normalized_content_hash_ignores_order_and_volatile_fields() {
        let a = serde_json::json!({
            "b": 1,
            "a": { "y": 2, "x": 3 },
            "updatedAt": 100,
            "createdAt": 5,
            "version": "1.0.0"
        });
        let b = serde_json::json!({
            "a": { "x": 3, "y": 2 },
            "b": 1,
            "updatedAt": 999,
            "version": "9.9.9"
        });
        assert_eq!(
            normalized_content_hash(&a),
            normalized_content_hash(&b),
            "字段顺序不同、易变字段不同应判为无改动"
        );
        let c = serde_json::json!({ "a": { "x": 3, "y": 2 }, "b": 2 });
        assert_ne!(
            normalized_content_hash(&a),
            normalized_content_hash(&c),
            "内容不同应判为有改动"
        );
    }

    /// 覆盖前快照：内容与最新版本相同不建；内容变化才建，且 origin 正确。
    #[test]
    fn snapshot_before_overwrite_dedups_and_marks_origin() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("v1", "内容A", None)).unwrap();

        assert!(
            snapshot_before_overwrite(&conn, "v1", VERSION_ORIGIN_IMPORT).unwrap(),
            "首次应建快照"
        );
        let vs = list_workflow_versions(&conn, "v1").unwrap();
        assert_eq!(vs.len(), 1);
        assert_eq!(vs[0].origin, VERSION_ORIGIN_IMPORT);

        // 内容未变化 → 不再新建
        assert!(!snapshot_before_overwrite(&conn, "v1", VERSION_ORIGIN_IMPORT).unwrap());
        assert_eq!(list_workflow_versions(&conn, "v1").unwrap().len(), 1);

        // 内容变化 → 新建
        let mut d = get_definition(&conn, "v1").unwrap().unwrap();
        d.name = "内容B".into();
        update_definition(&conn, &d).unwrap();
        assert!(snapshot_before_overwrite(&conn, "v1", VERSION_ORIGIN_IMPORT).unwrap());
        assert_eq!(list_workflow_versions(&conn, "v1").unwrap().len(), 2);
    }

    /// 保留策略：自动快照各来源只留最近 [`AUTO_VERSION_KEEP`] 个；手动版本不清理。
    #[test]
    fn auto_versions_retain_last_three_manual_kept() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("v2", "初始", None)).unwrap();

        for i in 0..5 {
            let mut d = get_definition(&conn, "v2").unwrap().unwrap();
            d.name = format!("自动{}", i);
            update_definition(&conn, &d).unwrap();
            assert!(snapshot_before_overwrite(&conn, "v2", VERSION_ORIGIN_CLOUD_SYNC).unwrap());
        }
        let auto = list_workflow_versions(&conn, "v2")
            .unwrap()
            .into_iter()
            .filter(|v| v.origin == VERSION_ORIGIN_CLOUD_SYNC)
            .count();
        assert_eq!(auto, AUTO_VERSION_KEEP as usize, "云同步快照只保留最近 3 个");

        for i in 0..5 {
            let mut d = get_definition(&conn, "v2").unwrap().unwrap();
            d.name = format!("手动{}", i);
            update_definition(&conn, &d).unwrap();
            let snap = serde_json::to_string(&d).unwrap();
            save_workflow_version(&conn, "v2", &snap, VERSION_ORIGIN_MANUAL).unwrap();
        }
        let manual = list_workflow_versions(&conn, "v2")
            .unwrap()
            .into_iter()
            .filter(|v| v.origin == VERSION_ORIGIN_MANUAL)
            .count();
        assert_eq!(manual, 5, "手动保存的版本永久保留、不清理");
    }

    /// 批次 2 遗留修复：软删除保留定时调度（恢复后自动生效）；彻底删除才清理调度（外键级联）。
    #[test]
    fn soft_delete_keeps_schedule_purge_removes_it() {
        let conn = full_conn();
        create_definition(&conn, &def_with_subflow("s1", "带调度工作流", None)).unwrap();
        conn.execute(
            "INSERT INTO workflow_schedules (id, workflow_id, cron_expression, enabled, input_data, created_at, updated_at)
             VALUES ('sch1', 's1', '0 0 * * * *', 1, '{}', 0, 0)",
            [],
        )
        .unwrap();

        soft_delete_definition(&conn, "s1").unwrap();
        let after_soft: i64 = conn
            .query_row("SELECT COUNT(*) FROM workflow_schedules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_soft, 1, "软删除应保留定时调度");

        purge_workflow(&conn, "s1").unwrap();
        let after_purge: i64 = conn
            .query_row("SELECT COUNT(*) FROM workflow_schedules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_purge, 0, "彻底删除应清理定时调度");
    }
}
