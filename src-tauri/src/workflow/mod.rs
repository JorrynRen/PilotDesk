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
// 节点类型 — 精简为 6 种实体节点
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
    #[serde(rename = "any")]
    Any,
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
    #[serde(rename = "custom")]
    Custom(String),
}

/// 阶段门控配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateConfig {
    pub strategy: GateStrategy,
    pub merge_strategy: MergeStrategy,
    pub threshold: Option<usize>,
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
    pub gate: GateConfig,
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
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
    pub max_depth: Option<u32>,
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
    /// @deprecated 引擎已不再写入此字段，数据源改为 node_executions 表（v68 迁移后为 NULL）
    #[allow(dead_code)]
    pub steps: Option<serde_json::Value>,
    pub current_node_id: Option<String>,
    pub trigger: String,
    pub trigger_detail: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub estimated_remaining: Option<i64>,
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
        max_depth: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        enabled: row.get(11)?,
    })
}

const DEF_COLUMNS: &str = "id, name, version, description, trigger, stages, input_schema, output_schema, max_depth, created_at, updated_at, enabled";

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
         input_schema, output_schema, max_depth, created_at, updated_at, enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            def.id, def.name, def.version, def.description,
            serde_json::to_string(&def.trigger).map_err(|e| AppError::External(e.to_string()))?,
            serde_json::to_string(&def.stages).map_err(|e| AppError::External(e.to_string()))?,
            def.input_schema.as_ref().map(|v| v.to_string()),
            def.output_schema.as_ref().map(|v| v.to_string()),
            def.max_depth, now, now, def.enabled,
        ],
    )?;
    Ok(())
}

pub fn update_definition(conn: &Connection, def: &WorkflowDefinition) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "UPDATE workflow_definitions SET name = ?1, version = ?2, description = ?3,
         trigger = ?4, stages = ?5, input_schema = ?6, output_schema = ?7,
         max_depth = ?8, updated_at = ?9, enabled = ?10
         WHERE id = ?11",
        params![
            def.name, def.version, def.description,
            serde_json::to_string(&def.trigger).map_err(|e| AppError::External(e.to_string()))?,
            serde_json::to_string(&def.stages).map_err(|e| AppError::External(e.to_string()))?,
            def.input_schema.as_ref().map(|v| v.to_string()),
            def.output_schema.as_ref().map(|v| v.to_string()),
            def.max_depth, now, def.enabled, def.id,
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
        steps: row.get::<_, Option<String>>(5)?.and_then(|s| serde_json::from_str(&s).ok()),
        current_node_id: row.get(6)?,
        trigger: row.get(7)?,
        trigger_detail: row.get(8)?,
        started_at: row.get(9)?,
        completed_at: row.get(10)?,
        estimated_remaining: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
    })
}

pub fn list_instances(conn: &Connection, definition_id: Option<&str>) -> Result<Vec<WorkflowInstance>, AppError> {
    let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match definition_id {
        Some(did) => (
            "SELECT id, definition_id, definition_name, status, context, steps,
                    current_node_id, trigger, trigger_detail,
                    started_at, completed_at, estimated_remaining, error, created_at
             FROM workflow_instances WHERE definition_id = ?1 ORDER BY created_at DESC".to_string(),
            vec![Box::new(did.to_string())],
        ),
        None => (
            "SELECT id, definition_id, definition_name, status, context, steps,
                    current_node_id, trigger, trigger_detail,
                    started_at, completed_at, estimated_remaining, error, created_at
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
        "INSERT INTO workflow_instances (id, definition_id, definition_name, status, context, steps,
         current_node_id, trigger, trigger_detail, started_at, completed_at,
         estimated_remaining, error, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            instance.id, instance.definition_id, instance.definition_name,
            serde_json::to_string(&instance.status).map_err(|e| AppError::External(e.to_string()))?,
            instance.context.to_string(),
            instance.steps.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "null".to_string()),
            instance.current_node_id, instance.trigger, instance.trigger_detail,
            instance.started_at, instance.completed_at,
            instance.estimated_remaining, instance.error, now,
        ],
    )?;
    Ok(())
}

/// 更新实例状态
///
/// 注意：`steps` 参数已废弃（引擎已改为写入 node_executions 表），保留仅为向后兼容。
/// 新代码应使用 `update_instance_status_minimal` 或直接更新 node_executions 表。
#[allow(dead_code)]
#[allow(deprecated)]
pub fn update_instance_status(
    conn: &Connection,
    id: &str,
    status: &WorkflowInstanceStatus,
    context: Option<&serde_json::Value>,
    #[allow(unused_variables)] _steps: Option<&serde_json::Value>,
    current_node_id: Option<&str>,
    error: Option<&str>,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    let status_str = serde_json::to_string(status).map_err(|e| AppError::External(e.to_string()))?;
    conn.execute(
        "UPDATE workflow_instances SET status = ?1, context = COALESCE(?2, context),
         current_node_id = ?3, error = ?4, updated_at = ?5
         WHERE id = ?6",
        params![
            status_str,
            context.map(|c| c.to_string()),
            current_node_id, error, now, id,
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
    pub success_rate: f64,
    pub avg_duration_ms: f64,
    pub max_duration_ms: i64,
    pub min_duration_ms: i64,
    pub total_node_executions: i64,
    pub node_failed_count: i64,
    pub last_7_days_count: i64,
    pub last_30_days_count: i64,
}

fn get_node_execution_count(conn: &Connection, workflow_id: Option<&str>) -> Result<i64, AppError> {
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

pub fn get_workflow_stats(conn: &Connection, workflow_id: Option<&str>) -> Result<WorkflowStats, AppError> {
    let now_ts = crate::utils::now();
    let seven_days_ago = now_ts - 7 * 24 * 3600;
    let thirty_days_ago = now_ts - 30 * 24 * 3600;

    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {
        Some(wid) => ("WHERE definition_id = ?1".to_string(), vec![Box::new(wid.to_string())]),
        None => ("".to_string(), vec![]),
    };

    let total_sql = format!("SELECT COUNT(*) as total, \
        COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as success, \
        COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed, \
        COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END), 0) as cancelled \
        FROM workflow_instances {}", filter_clause);

    let mut stmt = conn.prepare(&total_sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let (total, success, failed, cancelled): (i64, i64, i64, i64) = stmt.query_row(params_refs.as_slice(), |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?;

    let duration_sql = format!(
        "SELECT COALESCE(AVG(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 ELSE NULL END), 0) as avg_dur, \
                COALESCE(MAX(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 ELSE 0 END), 0) as max_dur, \
                COALESCE(MIN(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 ELSE 0 END), 0) as min_dur \
         FROM workflow_instances {}", filter_clause);

    let mut stmt2 = conn.prepare(&duration_sql)?;
    let params_refs2: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let (avg_duration_ms, max_duration_ms, min_duration_ms): (f64, i64, i64) = stmt2.query_row(params_refs2.as_slice(), |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?;

    let recent_sql = format!(
        "SELECT COALESCE(SUM(CASE WHEN created_at >= ?1 THEN 1 ELSE 0 END), 0) as last_7, \
                COALESCE(SUM(CASE WHEN created_at >= ?2 THEN 1 ELSE 0 END), 0) as last_30 \
         FROM workflow_instances {}", filter_clause);

    let mut stmt3 = conn.prepare(&recent_sql)?;
    let mut recent_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(seven_days_ago), Box::new(thirty_days_ago)];
    if let Some(wid) = workflow_id {
        recent_params.push(Box::new(wid.to_string()));
    }
    let recent_refs: Vec<&dyn rusqlite::types::ToSql> = recent_params.iter().map(|p| p.as_ref()).collect();
    let (last_7_days_count, last_30_days_count): (i64, i64) = stmt3.query_row(recent_refs.as_slice(), |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;

    let success_rate = if total > 0 { (success as f64 / total as f64) * 100.0 } else { 0.0 };

    Ok(WorkflowStats {
        total_executions: total,
        success_count: success,
        failed_count: failed,
        cancelled_count: cancelled,
        success_rate,
        avg_duration_ms,
        max_duration_ms,
        min_duration_ms,
        total_node_executions: get_node_execution_count(conn, workflow_id)?,
        node_failed_count: get_node_failed_count(conn, workflow_id)?,
        last_7_days_count,
        last_30_days_count,
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
    pub avg_duration_ms: f64,
}

pub fn get_execution_timeline(conn: &Connection, workflow_id: Option<&str>, days: i64) -> Result<Vec<ExecutionTimelinePoint>, AppError> {
    let now_ts = crate::utils::now();
    let start_ts = now_ts - days * 24 * 3600;
    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {
        Some(wid) => ("WHERE definition_id = ?1 AND created_at >= ?2".to_string(),
            vec![Box::new(wid.to_string()), Box::new(start_ts)]),
        None => ("WHERE created_at >= ?1".to_string(), vec![Box::new(start_ts)]),
    };
    let sql = format!(
        "SELECT DATE(created_at, 'unixepoch') as day, COUNT(*) as total, \
                COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as success, \
                COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed, \
                COALESCE(AVG(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 ELSE NULL END), 0) as avg_dur \
         FROM workflow_instances {} GROUP BY day ORDER BY day ASC", filter_clause);
    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let points = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(ExecutionTimelinePoint {
            date: row.get(0)?, total: row.get(1)?, success: row.get(2)?,
            failed: row.get(3)?, avg_duration_ms: row.get(4)?,
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

pub fn get_node_type_stats(conn: &Connection, workflow_id: Option<&str>) -> Result<Vec<NodeTypeStat>, AppError> {
    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {
        Some(wid) => ("WHERE id = ?1".to_string(), vec![Box::new(wid.to_string())]),
        None => ("".to_string(), vec![]),
    };
    let sql = format!("SELECT stages FROM workflow_definitions {}", filter_clause);
    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let mut type_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let stages_str: String = row.get(0)?;
        Ok(stages_str)
    })?.collect::<Result<Vec<_>, _>>()?;
    for stages_str in &rows {
        if let Ok(stages) = serde_json::from_str::<Vec<Stage>>(stages_str) {
            for stage in &stages {
                for node in &stage.nodes {
                    let nt = format!("{:?}", node.node_type).to_lowercase();
                    *type_counts.entry(nt).or_insert(0) += 1;
                }
            }
        }
    }
    let stats: Vec<NodeTypeStat> = type_counts.into_iter().map(|(node_type, count)| {
        NodeTypeStat { node_type, count, failed_count: 0, avg_duration_ms: 0.0 }
    }).collect();
    Ok(stats)
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
    let def = get_definition(conn, workflow_id)?.ok_or_else(|| AppError::NotFound("工作流不存在".into()))?;
    let new_version = def.version.parse::<i64>().unwrap_or(0) + 1;
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO workflow_versions (id, workflow_id, version, snapshot, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, workflow_id, new_version, snapshot, now],
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
