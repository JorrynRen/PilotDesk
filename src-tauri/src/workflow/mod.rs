pub mod registry;

pub mod template;

pub mod executor;

pub mod engine;

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



pub(crate) fn instance_from_row(row: &rusqlite::Row) -> rusqlite::Result<WorkflowInstance> {

    let status_str: String = row.get(3)?;

    Ok(WorkflowInstance {

        id: row.get(0)?,

        definition_id: row.get(1)?,

        definition_name: row.get(2)?,

        status: serde_json::from_str(&format!("\"{}\"", status_str))

            .unwrap_or(WorkflowInstanceStatus::Pending),

        context: serde_json::from_str(&row.get::<_, String>(4)?)

            .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),

        trigger: row.get(5)?,

        trigger_detail: row.get(6)?,

        started_at: row.get(7)?,

        completed_at: row.get(8)?,

        completion_rate: row.get(9)?,

        error: row.get(10)?,

        created_at: row.get(11)?,

    })

}



pub fn list_instances(conn: &Connection, definition_id: Option<&str>) -> Result<Vec<WorkflowInstance>, AppError> {

    let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match definition_id {

        Some(did) => (

            "SELECT id, definition_id, definition_name, status, context,

                    trigger, trigger_detail,

                    started_at, completed_at, completion_rate, error, created_at

             FROM workflow_instances WHERE definition_id = ?1 ORDER BY created_at DESC".to_string(),

            vec![Box::new(did.to_string())],

        ),

        None => (

            "SELECT id, definition_id, definition_name, status, context,

                    trigger, trigger_detail,

                    started_at, completed_at, completion_rate, error, created_at

             FROM workflow_instances ORDER BY created_at DESC".to_string(),

            vec![],

        ),

    };



    let mut stmt = conn.prepare(&sql)?;

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let instances = stmt.query_map(params_refs.as_slice(), |row| instance_from_row(row))?

        .collect::<Result<Vec<_>, _>>()?;

    Ok(instances)

}



pub fn create_instance(conn: &Connection, instance: &WorkflowInstance) -> Result<(), AppError> {

    let now = crate::utils::now();

    conn.execute(

        "INSERT INTO workflow_instances (id, definition_id, definition_name, status, context,

         trigger, trigger_detail, started_at, completed_at,

         completion_rate, error, created_at)

         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",

        params![

            instance.id, instance.definition_id, instance.definition_name,

            match &instance.status {

                WorkflowInstanceStatus::Pending => "pending",

                WorkflowInstanceStatus::Running => "running",

                WorkflowInstanceStatus::Paused => "paused",

                WorkflowInstanceStatus::Success => "success",

                WorkflowInstanceStatus::Failed => "failed",

                WorkflowInstanceStatus::Cancelled => "cancelled",

                WorkflowInstanceStatus::Timeout => "timeout",

            }.to_string(),

            instance.context.to_string(),

            instance.trigger, instance.trigger_detail,

            instance.started_at, instance.completed_at,

            instance.completion_rate, instance.error, now,

        ],

    )?;

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



fn get_node_execution_count(conn: &Connection, workflow_id: Option<&str>) -> Result<i64, AppError> {

    eprintln!("[get_node_type_stats] called workflow_id={:?}", workflow_id);

    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {

        Some(wid) => ("WHERE execution_id IN (SELECT id FROM workflow_instances WHERE definition_id = ?1)".to_string(), vec![Box::new(wid.to_string())]),

        None => ("".to_string(), vec![]),

    };

    let sql = format!("SELECT COALESCE(COUNT(*), 0) FROM node_executions {}", filter_clause);

    let mut stmt = conn.prepare(&sql)?;

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let count: i64 = stmt.query_row(params_refs.as_slice(), |row| row.get(0))?;

    Ok(count)

}



fn get_node_failed_count(conn: &Connection, workflow_id: Option<&str>) -> Result<i64, AppError> {

    eprintln!("[get_node_type_stats] called workflow_id={:?}", workflow_id);

    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {

        Some(wid) => ("WHERE execution_id IN (SELECT id FROM workflow_instances WHERE definition_id = ?1) AND status = 'failed'".to_string(), vec![Box::new(wid.to_string())]),

        None => ("WHERE status = 'failed'".to_string(), vec![]),

    };

    let sql = format!("SELECT COALESCE(COUNT(*), 0) FROM node_executions {}", filter_clause);

    let mut stmt = conn.prepare(&sql)?;

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let count: i64 = stmt.query_row(params_refs.as_slice(), |row| row.get(0))?;

    Ok(count)

}



pub fn get_workflow_stats(conn: &Connection, workflow_id: Option<&str>, days: Option<i64>) -> Result<WorkflowStats, AppError> {

    let now_ts = crate::utils::now();

    let seven_days_ago = now_ts - 7 * 24 * 3600;

    let thirty_days_ago = now_ts - 30 * 24 * 3600;

    // days=0 或 None 表示全量统计，此时 lastNDaysCount = total
    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;
    let n_days_ago = if is_all { 0 } else { now_ts - days_value * 24 * 3600 };



    eprintln!("[get_workflow_stats] called workflow_id={:?} days={:?} is_all={}", workflow_id, days_value, is_all);

    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {

        Some(wid) => ("WHERE definition_id = ?1".to_string(), vec![Box::new(wid.to_string())]),

        None => ("".to_string(), vec![]),

    };



    let total_sql = format!("SELECT COUNT(*) as total, \

        COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as success, \

        COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed, \

        COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END), 0) as cancelled, \

        COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0) as running, \

        COALESCE(SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END), 0) as pending, \

        COALESCE(SUM(CASE WHEN status = 'paused' THEN 1 ELSE 0 END), 0) as paused, \

        COALESCE(SUM(CASE WHEN status = 'timeout' THEN 1 ELSE 0 END), 0) as timeout \

        FROM workflow_instances {}", filter_clause);



    let mut stmt = conn.prepare(&total_sql)?;

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let (total, success, failed, cancelled, running, pending, paused, timeout): (i64, i64, i64, i64, i64, i64, i64, i64) = stmt.query_row(params_refs.as_slice(), |row| {

        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))

    })?;



    let duration_sql = format!(

        "SELECT COALESCE(AVG(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 END), 0) as avg_dur, \

                COALESCE(MAX(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 END), 0) as max_dur, \

                COALESCE(MIN(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 END), 0) as min_dur \

         FROM workflow_instances {}", filter_clause);



    let mut stmt2 = conn.prepare(&duration_sql)?;

    let params_refs2: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let (avg_duration_ms, max_duration_ms, min_duration_ms): (f64, i64, i64) = stmt2.query_row(params_refs2.as_slice(), |row| {

        Ok((row.get(0)?, row.get(1)?, row.get(2)?))

    })?;



    // is_all 模式下 last_n 直接等于 COUNT(*)，否则按 created_at >= n_days_ago
    let recent_sql = format!(
        "SELECT COALESCE(SUM(CASE WHEN created_at >= ?1 THEN 1 ELSE 0 END), 0) as last_7, \
                COALESCE(SUM(CASE WHEN created_at >= ?2 THEN 1 ELSE 0 END), 0) as last_30, \
                CASE WHEN ?3 = 1 THEN COUNT(*) ELSE COALESCE(SUM(CASE WHEN created_at >= ?4 THEN 1 ELSE 0 END), 0) END as last_n \
         FROM workflow_instances {}", filter_clause);

    let mut stmt3 = conn.prepare(&recent_sql)?;
    let all_flag: i64 = if is_all { 1 } else { 0 };
    let mut recent_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(seven_days_ago),
        Box::new(thirty_days_ago),
        Box::new(all_flag),
        Box::new(n_days_ago),
    ];
    if let Some(wid) = workflow_id {
        recent_params.push(Box::new(wid.to_string()));
    }
    let recent_refs: Vec<&dyn rusqlite::types::ToSql> = recent_params.iter().map(|p| p.as_ref()).collect();
    let (last_7_days_count, last_30_days_count, last_n_days_count): (i64, i64, i64) = stmt3.query_row(recent_refs.as_slice(), |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?;

    let success_rate = if total > 0 { (success as f64 / total as f64) * 100.0 } else { 0.0 };

    // ========== 区间内统计（按 days 窗口） ==========
    // 构造区间 WHERE：is_all 时 created_at 无条件，否则 created_at >= n_days_ago
    // range_filter_clause / range_params 是对 workflow_instances 的区间过滤（同时保留 workflow_id 过滤）
    let (range_filter, range_params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match (workflow_id, is_all) {
        (Some(wid), false) => ("WHERE definition_id = ?1 AND created_at >= ?2".to_string(),
            vec![Box::new(wid.to_string()), Box::new(n_days_ago)]),
        (Some(wid), true) => ("WHERE definition_id = ?1".to_string(),
            vec![Box::new(wid.to_string())]),
        (None, false) => ("WHERE created_at >= ?1".to_string(),
            vec![Box::new(n_days_ago)]),
        (None, true) => ("".to_string(), vec![]),
    };

    let range_sql = format!(
        "SELECT COUNT(*) as r_total, \
                COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as r_success, \
                COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as r_failed, \
                COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END), 0) as r_cancelled, \
                COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0) as r_running, \
                COALESCE(SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END), 0) as r_pending, \
                COALESCE(SUM(CASE WHEN status = 'paused' THEN 1 ELSE 0 END), 0) as r_paused, \
                COALESCE(SUM(CASE WHEN status = 'timeout' THEN 1 ELSE 0 END), 0) as r_timeout, \
                COALESCE(AVG(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 END), 0) as r_avg_dur, \
                COALESCE(MAX(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 END), 0) as r_max_dur, \
                COALESCE(MIN(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 END), 0) as r_min_dur \
         FROM workflow_instances {}", range_filter);
    let mut stmt_r = conn.prepare(&range_sql)?;
    let range_refs: Vec<&dyn rusqlite::types::ToSql> = range_params_vec.iter().map(|p| p.as_ref()).collect();
    let (range_total, range_success, range_failed, range_cancelled, range_running, range_pending, range_paused, range_timeout, range_avg_duration_ms, range_max_duration_ms, range_min_duration_ms)
        : (i64, i64, i64, i64, i64, i64, i64, i64, f64, i64, i64)
        = stmt_r.query_row(range_refs.as_slice(), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?))
        })?;
    let range_success_rate = if range_total > 0 { (range_success as f64 / range_total as f64) * 100.0 } else { 0.0 };



    Ok(WorkflowStats {

        total_executions: total,

        success_count: success,

        failed_count: failed,

        cancelled_count: cancelled,

        running_count: running,

        pending_count: pending,

        paused_count: paused,

        timeout_count: timeout,

        success_rate,

        avg_duration_ms,

        max_duration_ms,

        min_duration_ms,

        total_node_executions: get_node_execution_count(conn, workflow_id)?,

        node_failed_count: get_node_failed_count(conn, workflow_id)?,

        last_7_days_count,

        last_30_days_count,

        last_n_days_count,

        days: days_value,

        range_total,
        range_success,
        range_failed,
        range_cancelled,
        range_running,
        range_pending,
        range_paused,
        range_timeout,
        range_success_rate,
        range_avg_duration_ms,
        range_max_duration_ms,
        range_min_duration_ms,

    })

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



pub fn get_execution_timeline(conn: &Connection, workflow_id: Option<&str>, days: i64) -> Result<Vec<ExecutionTimelinePoint>, AppError> {

    // days<=0 表示"全部"，但柱状图需要有上限，所以最多取 365 天
    let effective_days = if days <= 0 { 365 } else { days };

    // 根据跨度自动选择聚合粒度：≤90天按天，91~365按周，>365按月
    let granularity: &str = if effective_days <= 90 { "day" }
        else if effective_days <= 365 { "week" }
        else { "month" };
    // 不同粒度用不同的分组 SQL 表达式
    let (date_expr, order_expr) = match granularity {
        "month" => (
            "strftime('%Y-%m', created_at, 'unixepoch') AS bucket",
            "MIN(created_at)",
        ),
        "week" => (
            // SQLite W周：2024-W01 形式，可排序
            "strftime('%Y-W%W', created_at, 'unixepoch') AS bucket",
            "MIN(created_at)",
        ),
        _ => (
            "DATE(created_at, 'unixepoch') AS bucket",
            "bucket",
        ),
    };

    let now_ts = crate::utils::now();

    let start_ts = now_ts - effective_days * 24 * 3600;

    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {

        Some(wid) => ("WHERE definition_id = ?1 AND created_at >= ?2".to_string(),

            vec![Box::new(wid.to_string()), Box::new(start_ts)]),

        None => ("WHERE created_at >= ?1".to_string(), vec![Box::new(start_ts)]),

    };

    let sql = format!(

        "SELECT {}, COUNT(*) as total, \

                COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as success, \

                COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed, \

                COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END), 0) as cancelled, \

                COALESCE(AVG(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 END), 0) as avg_dur \

         FROM workflow_instances {} GROUP BY bucket ORDER BY {} ASC",

        date_expr, filter_clause, order_expr);

    let mut stmt = conn.prepare(&sql)?;

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let points = stmt.query_map(params_refs.as_slice(), |row| {

        Ok(ExecutionTimelinePoint {

            date: row.get(0)?, total: row.get(1)?, success: row.get(2)?,

            failed: row.get(3)?, cancelled: row.get(4)?, avg_duration_ms: row.get(5)?,

            granularity: granularity.to_string(),

        })

    })?.collect::<Result<Vec<_>, _>>()?;

    Ok(points)

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



pub fn get_node_type_stats(conn: &Connection, workflow_id: Option<&str>, days: Option<i64>) -> Result<Vec<NodeTypeStat>, AppError> {

    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;

    let now_ts = crate::utils::now();
    // 全量模式 start_ts=0（等于 unix 纪元前不会过滤任何记录）
    let start_ts = if is_all { 0 } else { now_ts - days_value * 24 * 3600 };

    eprintln!("[get_node_type_stats] called workflow_id={:?} days={} is_all={}", workflow_id, days_value, is_all);

    // 1. Build (definition_id, node_id) → node_type map from workflow_definitions.stages
    let def_sql = match workflow_id {
        Some(_) => "SELECT id, stages FROM workflow_definitions WHERE id = ?1",
        None => "SELECT id, stages FROM workflow_definitions",
    };

    let mut stmt = conn.prepare(def_sql)?;
    let def_params: Vec<Box<dyn rusqlite::types::ToSql>> = match workflow_id {
        Some(wid) => vec![Box::new(wid.to_string())],
        None => vec![],
    };
    let def_refs: Vec<&dyn rusqlite::types::ToSql> = def_params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(def_refs.as_slice(), |row| {
        let id: String = row.get(0)?;
        let stages_str: String = row.get(1)?;
        Ok((id, stages_str))
    })?.collect::<Result<Vec<_>, _>>()?;

    let mut node_type_map: std::collections::HashMap<(String, String), String> = std::collections::HashMap::new();
    for (def_id, stages_str) in &rows {
        match serde_json::from_str::<Vec<Stage>>(stages_str) {
            Ok(stages) => {
                for stage in &stages {
                    for node in &stage.nodes {
                        let nt = format!("{:?}", node.node_type).to_lowercase();
                        node_type_map.insert((def_id.clone(), node.id.clone()), nt);
                    }
                }
            }
            Err(e) => {
                eprintln!("[get_node_type_stats] def {} parse error: {:?}", def_id, e);
            }
        }
    }

    // 2. Query node_executions joined with workflow_instances for the time window
    //    全量模式（is_all）去掉 created_at 过滤条件；否则加 created_at >= start_ts
    let exec_sql = match (workflow_id, is_all) {
        (Some(_), false) => "SELECT ne.node_id, ne.status, COALESCE(ne.duration_ms, 0), wi.definition_id \
                    FROM node_executions ne JOIN workflow_instances wi ON ne.execution_id = wi.id \
                    WHERE wi.definition_id = ?1 AND wi.created_at >= ?2",
        (Some(_), true) => "SELECT ne.node_id, ne.status, COALESCE(ne.duration_ms, 0), wi.definition_id \
                    FROM node_executions ne JOIN workflow_instances wi ON ne.execution_id = wi.id \
                    WHERE wi.definition_id = ?1",
        (None, false) => "SELECT ne.node_id, ne.status, COALESCE(ne.duration_ms, 0), wi.definition_id \
                 FROM node_executions ne JOIN workflow_instances wi ON ne.execution_id = wi.id \
                 WHERE wi.created_at >= ?1",
        (None, true) => "SELECT ne.node_id, ne.status, COALESCE(ne.duration_ms, 0), wi.definition_id \
                 FROM node_executions ne JOIN workflow_instances wi ON ne.execution_id = wi.id",
    };
    let mut stmt2 = conn.prepare(exec_sql)?;
    let exec_params: Vec<Box<dyn rusqlite::types::ToSql>> = match (workflow_id, is_all) {
        (Some(wid), false) => vec![Box::new(wid.to_string()), Box::new(start_ts)],
        (Some(wid), true) => vec![Box::new(wid.to_string())],
        (None, false) => vec![Box::new(start_ts)],
        (None, true) => vec![],
    };
    let exec_refs: Vec<&dyn rusqlite::types::ToSql> = exec_params.iter().map(|p| p.as_ref()).collect();
    let exec_rows = stmt2.query_map(exec_refs.as_slice(), |row| {
        let node_id: String = row.get(0)?;
        let status: String = row.get(1)?;
        let duration_ms: i64 = row.get(2)?;
        let definition_id: String = row.get(3)?;
        Ok((node_id, status, duration_ms, definition_id))
    })?.collect::<Result<Vec<_>, _>>()?;

    // 3. Aggregate by node_type: (count, failed_count, sum_duration_ms)
    let mut agg: std::collections::HashMap<String, (i64, i64, f64)> = std::collections::HashMap::new();
    for (node_id, status, duration_ms, definition_id) in &exec_rows {
        if let Some(nt) = node_type_map.get(&(definition_id.clone(), node_id.clone())) {
            let entry = agg.entry(nt.clone()).or_insert((0, 0, 0.0));
            entry.0 += 1;
            if status == "failed" {
                entry.1 += 1;
            }
            entry.2 += *duration_ms as f64;
        }
    }

    // 4. Build result
    let stats: Vec<NodeTypeStat> = agg.into_iter().map(|(node_type, (count, failed_count, sum_duration))| {
        let avg_duration_ms = if count > 0 { sum_duration / count as f64 } else { 0.0 };
        NodeTypeStat { node_type, count, failed_count, avg_duration_ms }
    }).collect();

    eprintln!("[get_node_type_stats] DONE: {} types, days={}", stats.len(), days_value);
    Ok(stats)
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



pub fn get_top_workflows(conn: &Connection, days: Option<i64>, limit: Option<i64>, sort_by: Option<&str>) -> Result<Vec<TopWorkflowStat>, AppError> {

    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;
    let limit_value = limit.unwrap_or(5).max(1) as i64;

    let now_ts = crate::utils::now();
    let start_ts = if is_all { 0 } else { now_ts - days_value * 24 * 3600 };

    let order_clause = match sort_by {
        Some("failed") => "failed DESC, total DESC",
        _ => "total DESC",
    };

    // is_all: 不加 created_at 过滤
    let sql = if is_all {
        format!(
            "SELECT definition_id, \
                    COALESCE(MAX(definition_name), '') as name, \
                    COUNT(*) as total, \
                    COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as success, \
                    COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed, \
                    COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END), 0) as cancelled \
             FROM workflow_instances WHERE definition_id != '' \
             GROUP BY definition_id ORDER BY {} LIMIT ?1", order_clause)
    } else {
        format!(
            "SELECT definition_id, \
                    COALESCE(MAX(definition_name), '') as name, \
                    COUNT(*) as total, \
                    COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as success, \
                    COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed, \
                    COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END), 0) as cancelled \
             FROM workflow_instances WHERE created_at >= ?1 AND definition_id != '' \
             GROUP BY definition_id ORDER BY {} LIMIT ?2", order_clause)
    };

    let mut stmt = conn.prepare(&sql)?;
    let rows = if is_all {
        stmt.query_map(params![limit_value], |row| {
            let definition_id: String = row.get(0)?;
            let definition_name: String = row.get(1)?;
            let total: i64 = row.get(2)?;
            let success: i64 = row.get(3)?;
            let failed: i64 = row.get(4)?;
            let cancelled: i64 = row.get(5)?;
            let failed_rate = if total > 0 { (failed as f64 / total as f64) * 100.0 } else { 0.0 };
            Ok(TopWorkflowStat { definition_id, definition_name, total, success, failed, cancelled, failed_rate })
        })?.collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![start_ts, limit_value], |row| {
            let definition_id: String = row.get(0)?;
            let definition_name: String = row.get(1)?;
            let total: i64 = row.get(2)?;
            let success: i64 = row.get(3)?;
            let failed: i64 = row.get(4)?;
            let cancelled: i64 = row.get(5)?;
            let failed_rate = if total > 0 { (failed as f64 / total as f64) * 100.0 } else { 0.0 };
            Ok(TopWorkflowStat { definition_id, definition_name, total, success, failed, cancelled, failed_rate })
        })?.collect::<Result<Vec<_>, _>>()?
    };

    Ok(rows)
}



#[derive(Debug, Clone, Serialize, Deserialize)]

#[serde(rename_all = "camelCase")]

pub struct TopErrorStat {

    pub error: String,

    pub count: i64,

    pub last_occurred_at: i64,

}



pub fn get_top_errors(conn: &Connection, days: Option<i64>, limit: Option<i64>) -> Result<Vec<TopErrorStat>, AppError> {

    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;
    let limit_value = limit.unwrap_or(5).max(1) as i64;

    let now_ts = crate::utils::now();
    let start_ts = if is_all { 0 } else { now_ts - days_value * 24 * 3600 };

    // 聚合 error 文本：trim 防止空白差异；空 error 归类为 "(无错误信息)"
    // is_all: 不加 created_at 过滤
    let sql = if is_all {
        "SELECT \
            CASE WHEN TRIM(COALESCE(error, '')) = '' THEN '(无错误信息)' ELSE TRIM(error) END as err_text, \
            COUNT(*) as cnt, \
            MAX(created_at) as last_at \
         FROM workflow_instances WHERE status = 'failed' \
         GROUP BY err_text ORDER BY cnt DESC LIMIT ?1"
    } else {
        "SELECT \
            CASE WHEN TRIM(COALESCE(error, '')) = '' THEN '(无错误信息)' ELSE TRIM(error) END as err_text, \
            COUNT(*) as cnt, \
            MAX(created_at) as last_at \
         FROM workflow_instances WHERE created_at >= ?1 AND status = 'failed' \
         GROUP BY err_text ORDER BY cnt DESC LIMIT ?2"
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = if is_all {
        stmt.query_map(params![limit_value], |row| {
            let error: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let last_occurred_at: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            Ok(TopErrorStat { error, count, last_occurred_at })
        })?.collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![start_ts, limit_value], |row| {
            let error: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let last_occurred_at: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            Ok(TopErrorStat { error, count, last_occurred_at })
        })?.collect::<Result<Vec<_>, _>>()?
    };

    Ok(rows)
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



pub fn get_node_execution_logs(conn: &Connection, node_execution_id: &str) -> Result<Vec<NodeExecutionLogEntry>, AppError> {

    let mut stmt = conn.prepare(

        "SELECT id, execution_id, node_execution_id, timestamp, level, message, metadata FROM node_execution_logs WHERE node_execution_id = ?1 ORDER BY timestamp ASC"

    )?;

    let logs = stmt.query_map(params![node_execution_id], |row| {

        Ok(NodeExecutionLogEntry { id: row.get(0)?, execution_id: row.get(1)?, node_execution_id: row.get(2)?, timestamp: row.get(3)?, level: row.get(4)?, message: row.get(5)?, metadata: row.get(6)? })

    })?.collect::<Result<Vec<_>, _>>()?;

    Ok(logs)

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



pub fn list_recoverable_executions(conn: &Connection) -> Result<Vec<RecoverableExecution>, AppError> {

    let instances = list_instances(conn, None)?;

    let mut recoverable = Vec::new();

    for inst in instances {

        let status_str = format!("{:?}", inst.status);

        if status_str != "Paused" && status_str != "Running" { continue; }

        // 从 node_executions 表查询节点状态

        let mut stmt = conn.prepare(

            "SELECT node_id, status FROM node_executions WHERE execution_id = ?1"

        )?;

        let node_results: Vec<(String, String)> = stmt.query_map(

            rusqlite::params![inst.id],

            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),

        )?.collect::<Result<Vec<_>, _>>()?;

        let mut completed = Vec::new();

        let mut failed = Vec::new();

        let mut pending = Vec::new();

        for (node_id, status) in node_results {

            match status.as_str() {

                "completed" | "success" => completed.push(node_id),

                "failed" => failed.push(node_id),

                _ => pending.push(node_id),

            }

        }

        if !failed.is_empty() || !pending.is_empty() {

            recoverable.push(RecoverableExecution { execution: inst, completed_nodes: completed, failed_nodes: failed, pending_nodes: pending });

        }

    }

    Ok(recoverable)

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



pub fn get_pending_human_inputs(conn: &Connection) -> Result<Vec<PendingHumanInput>, AppError> {

    // 从 node_executions 表查询 running 状态的 interact 节点

    let mut stmt = conn.prepare(

        "SELECT ne.execution_id, ne.node_id, ne.input_data

         FROM node_executions ne

         JOIN workflow_instances wi ON ne.execution_id = wi.id

         WHERE ne.status = 'running'

           AND wi.status IN ('running', 'paused')

         ORDER BY ne.created_at DESC"

    )?;

    let pending = stmt.query_map([], |row| {

        let execution_id: String = row.get(0)?;

        let node_id: String = row.get(1)?;

        let input_data: Option<String> = row.get(2)?;

        let prompt = input_data.as_ref()

            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())

            .and_then(|v| v.get("prompt").and_then(|p| p.as_str()).map(|s| s.to_string()))

            .unwrap_or_else(|| "请输入".to_string());

        let input_type = input_data.as_ref()

            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())

            .and_then(|v| v.get("inputType").and_then(|t| t.as_str()).map(|s| s.to_string()))

            .unwrap_or_else(|| "text".to_string());

        let nid = node_id.clone();

        Ok(PendingHumanInput {

            execution_id,

            node_id,

            node_label: nid,

            prompt,

            input_type,

            created_at: 0,

        })

    })?.collect::<Result<Vec<_>, _>>()?;

    Ok(pending)

}



// Re-export engine types for commands

pub use engine::{ExecutionMode, ValidationResult};

