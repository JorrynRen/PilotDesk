pub mod registry;

pub mod template;

pub mod executor;

pub mod engine;

/// 工作流事件读侧派生（workflow_events → 节点状态/输出投影，供旧表删除后的读切点使用）。
pub mod events;

/// 执行记录/看板/恢复等读切点：统一重导出到 `workflow_events` 折叠实现
/// （读侧已全部切至事件投影；旧 `workflow_instances`/`node_executions`/`node_execution_logs`
/// 表 SQL 读侧实现不再保留）。
pub use events::{
    get_execution_timeline, get_node_execution_logs, get_node_type_stats, get_pending_human_inputs,
    get_top_errors, get_top_workflows, get_workflow_stats, list_instances,
    list_recoverable_executions,
};

pub mod executors;

pub mod scheduler;



use serde::{Deserialize, Serialize};

use rusqlite::{params, Connection};

use crate::utils::errors::AppError;



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

    pub retry_count: Option<u32>,

    pub retry_delay_ms: Option<u64>,



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

    pub label: Option<String>,       // 条件标签（如 "score > 0.8"）

    pub condition: Option<String>,   // 条件表达式

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

    Count(usize),

    #[serde(rename = "threshold")]

    Threshold(String),

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

    Custom(String),

}



/// 阶段门控配置

#[derive(Debug, Clone, Serialize, Deserialize)]

#[serde(rename_all = "camelCase")]

pub struct GateConfig {

    pub strategy: GateStrategy,

    pub merge_strategy: MergeStrategy,

    pub threshold: Option<String>,

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

        input_schema: row.get::<_, Option<String>>(6)?

            .and_then(|s| serde_json::from_str(&s).ok()),

        output_schema: row.get::<_, Option<String>>(7)?

            .and_then(|s| serde_json::from_str(&s).ok()),

        icon: row.get(8)?,

        created_at: row.get(9)?,

        updated_at: row.get(10)?,

        enabled: row.get(11)?,

    })

}



const DEF_COLUMNS: &str = "id, name, version, description, trigger, stages, input_schema, output_schema, icon, created_at, updated_at, enabled";



pub fn list_definitions(conn: &Connection) -> Result<Vec<WorkflowDefinition>, AppError> {

    let sql = format!("SELECT {} FROM workflow_definitions ORDER BY updated_at DESC", DEF_COLUMNS);

    let mut stmt = conn.prepare(&sql)?;

    let defs = stmt.query_map([], |row| def_from_row(row))?

        .collect::<Result<Vec<_>, _>>()?;

    Ok(defs)

}



pub fn get_definition(conn: &Connection, id: &str) -> Result<Option<WorkflowDefinition>, AppError> {

    let sql = format!("SELECT {} FROM workflow_definitions WHERE id = ?1", DEF_COLUMNS);

    let mut stmt = conn.prepare(&sql)?;

    let mut rows = stmt.query_map(params![id], |row| def_from_row(row))?;

    match rows.next() {

        Some(Ok(def)) => Ok(Some(def)),

        Some(Err(e)) => Err(AppError::Db(e.to_string())),

        None => Ok(None),

    }

}



pub fn create_definition(conn: &Connection, def: &WorkflowDefinition) -> Result<(), AppError> {

    let now = crate::utils::now();

    conn.execute(

        "INSERT INTO workflow_definitions (id, name, version, description, trigger, stages,

         input_schema, output_schema, icon, created_at, updated_at, enabled)

         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",

        params![

            def.id, def.name, def.version, def.description,

            serde_json::to_string(&def.trigger).map_err(|e| AppError::External(e.to_string()))?,

            serde_json::to_string(&def.stages).map_err(|e| AppError::External(e.to_string()))?,

            def.input_schema.as_ref().map(|v| v.to_string()),

            def.output_schema.as_ref().map(|v| v.to_string()),

            def.icon.as_ref().map(|v| v.to_string()),

            now, now, def.enabled,

        ],

    )?;

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

            def.name, def.version, def.description,

            serde_json::to_string(&def.trigger).map_err(|e| AppError::External(e.to_string()))?,

            serde_json::to_string(&def.stages).map_err(|e| AppError::External(e.to_string()))?,

            def.input_schema.as_ref().map(|v| v.to_string()),

            def.output_schema.as_ref().map(|v| v.to_string()),

            def.icon.as_ref().map(|v| v.to_string()),

            now, def.enabled, def.id,

        ],

    )?;

    Ok(())

}



pub fn delete_definition(conn: &Connection, id: &str) -> Result<(), AppError> {

    conn.execute("DELETE FROM workflow_definitions WHERE id = ?1", params![id])?;

    Ok(())

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

    pub avg_duration_ms: f64,

    pub max_duration_ms: i64,

    pub min_duration_ms: i64,

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

    /// 区间内平均耗时（ms）
    pub range_avg_duration_ms: f64,

    /// 区间内最长耗时（ms）
    pub range_max_duration_ms: i64,

    /// 区间内最短耗时（ms）
    pub range_min_duration_ms: i64,

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

    pub avg_duration_ms: f64,

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

}



pub fn duplicate_definition(conn: &Connection, id: &str, new_name: &str) -> Result<WorkflowDefinition, AppError> {

    let original = get_definition(conn, id)?.ok_or_else(|| AppError::NotFound("工作流不存在".into()))?;

    let mut new_def = original;

    new_def.id = crate::utils::new_id();

    new_def.name = new_name.to_string();

    new_def.version = "1.0.0".to_string();

    new_def.created_at = crate::utils::now();

    new_def.updated_at = new_def.created_at;

    create_definition(conn, &new_def)?;

    Ok(new_def)

}



pub fn list_workflow_versions(conn: &Connection, workflow_id: &str) -> Result<Vec<WorkflowVersion>, AppError> {

    let mut stmt = conn.prepare(

        "SELECT id, workflow_id, version, snapshot, created_at FROM workflow_versions WHERE workflow_id = ?1 ORDER BY version DESC"

    )?;

    let versions = stmt.query_map(params![workflow_id], |row| {

        Ok(WorkflowVersion { id: row.get(0)?, workflow_id: row.get(1)?, version: row.get(2)?, snapshot: row.get(3)?, created_at: row.get(4)? })

    })?.collect::<Result<Vec<_>, _>>()?;

    Ok(versions)

}



pub fn save_workflow_version(conn: &Connection, workflow_id: &str, snapshot: &str) -> Result<WorkflowVersion, AppError> {
    let _def = get_definition(conn, workflow_id)?.ok_or_else(|| AppError::NotFound("工作流不存在".into()))?;

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
        "INSERT INTO workflow_versions (id, workflow_id, version, snapshot, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, workflow_id, new_version, snapshot, now],
    )?;

    // 回写 definition.version 保持同步
    conn.execute(
        "UPDATE workflow_definitions SET version = ?1 WHERE id = ?2",
        params![new_version.to_string(), workflow_id],
    )?;

    Ok(WorkflowVersion { id, workflow_id: workflow_id.to_string(), version: new_version, snapshot: snapshot.to_string(), created_at: now })
}



pub fn restore_workflow_version(conn: &Connection, workflow_id: &str, version: i64) -> Result<WorkflowVersion, AppError> {

    let mut stmt = conn.prepare(

        "SELECT id, workflow_id, version, snapshot, created_at FROM workflow_versions WHERE workflow_id = ?1 AND version = ?2"

    )?;

    let ver = stmt.query_row(params![workflow_id, version], |row| {

        Ok(WorkflowVersion { id: row.get(0)?, workflow_id: row.get(1)?, version: row.get(2)?, snapshot: row.get(3)?, created_at: row.get(4)? })

    })?;

    let snapshot: WorkflowDefinition = serde_json::from_str(&ver.snapshot).map_err(|e| AppError::Json(e.to_string()))?;

    update_definition(conn, &snapshot)?;

    Ok(ver)

}

/// 删除指定版本快照
pub fn delete_workflow_version(conn: &Connection, workflow_id: &str, version: i64) -> Result<(), AppError> {
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

