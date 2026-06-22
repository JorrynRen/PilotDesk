pub mod registry;
pub mod template;
pub mod executor;
pub mod engine;
pub mod agents;
pub mod scheduler;

use serde::{Deserialize, Serialize};
use rusqlite::{params, Connection};
use crate::utils::errors::AppError;

// ── 数据模型 ──

/// 工作流节点类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowNodeType {
    #[serde(rename = "trigger:cron")]
    TriggerCron,
    #[serde(rename = "trigger:event")]
    TriggerEvent,
    #[serde(rename = "trigger:manual")]
    TriggerManual,
    #[serde(rename = "plugin:command")]
    PluginCommand,
    Condition,
    Parallel,
    Delay,
    Approval,
    Subflow,
}

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
    pub condition: Option<String>,
    pub cron: Option<String>,
    pub event_name: Option<String>,
    pub delay_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub retry_count: Option<u32>,
    pub retry_delay_ms: Option<u64>,
    pub input_mapping: Option<serde_json::Value>,
    pub output_mapping: Option<serde_json::Value>,
    pub position: Option<serde_json::Value>,
}

/// 工作流边
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub label: Option<String>,
    pub condition: Option<String>,
}

/// 工作流定义
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinition {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
    pub max_depth: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
    pub enabled: bool,
}

/// 工作流实例状态
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// 步骤执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct StepExecution {
    pub node_id: String,
    pub status: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration: Option<i64>,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub retry_count: u32,
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
    pub steps: serde_json::Value,
    pub current_node_id: Option<String>,
    pub trigger: String,
    pub trigger_detail: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub estimated_remaining: Option<i64>,
    pub error: Option<String>,
    pub created_at: i64,
}

// ── 数据库操作 ──

/// 列出所有工作流定义
pub fn list_definitions(conn: &Connection) -> Result<Vec<WorkflowDefinition>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, version, description, nodes, edges, input_schema, output_schema,
                max_depth, created_at, updated_at, enabled
         FROM workflow_definitions ORDER BY updated_at DESC"
    )?;

    let defs = stmt.query_map([], |row| {
        Ok(WorkflowDefinition {
            id: row.get(0)?,
            name: row.get(1)?,
            version: row.get(2)?,
            description: row.get(3)?,
            nodes: serde_json::from_str(&row.get::<_, String>(4)?)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
            edges: serde_json::from_str(&row.get::<_, String>(5)?)
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
    })?.collect::<Result<Vec<_>, _>>()?;

    Ok(defs)
}

/// 获取单个工作流定义
pub fn get_definition(conn: &Connection, id: &str) -> Result<Option<WorkflowDefinition>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, version, description, nodes, edges, input_schema, output_schema,
                max_depth, created_at, updated_at, enabled
         FROM workflow_definitions WHERE id = ?1"
    )?;

    let mut rows = stmt.query_map(params![id], |row| {
        Ok(WorkflowDefinition {
            id: row.get(0)?,
            name: row.get(1)?,
            version: row.get(2)?,
            description: row.get(3)?,
            nodes: serde_json::from_str(&row.get::<_, String>(4)?)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
            edges: serde_json::from_str(&row.get::<_, String>(5)?)
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
    })?;

    match rows.next() {
        Some(Ok(def)) => Ok(Some(def)),
        Some(Err(e)) => Err(AppError::Db(e.to_string())),
        None => Ok(None),
    }
}

/// 创建工作流定义
pub fn create_definition(conn: &Connection, def: &WorkflowDefinition) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO workflow_definitions (id, name, version, description, nodes, edges,
         input_schema, output_schema, max_depth, created_at, updated_at, enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            def.id, def.name, def.version, def.description,
            serde_json::to_string(&def.nodes).map_err(|e| AppError::External(e.to_string()))?,
            serde_json::to_string(&def.edges).map_err(|e| AppError::External(e.to_string()))?,
            def.input_schema.as_ref().map(|v| v.to_string()),
            def.output_schema.as_ref().map(|v| v.to_string()),
            def.max_depth, now, now, def.enabled,
        ],
    )?;
    Ok(())
}

/// 更新工作流定义
pub fn update_definition(conn: &Connection, def: &WorkflowDefinition) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "UPDATE workflow_definitions SET name = ?1, version = ?2, description = ?3,
         nodes = ?4, edges = ?5, input_schema = ?6, output_schema = ?7,
         max_depth = ?8, updated_at = ?9, enabled = ?10
         WHERE id = ?11",
        params![
            def.name, def.version, def.description,
            serde_json::to_string(&def.nodes).map_err(|e| AppError::External(e.to_string()))?,
            serde_json::to_string(&def.edges).map_err(|e| AppError::External(e.to_string()))?,
            def.input_schema.as_ref().map(|v| v.to_string()),
            def.output_schema.as_ref().map(|v| v.to_string()),
            def.max_depth, now, def.enabled, def.id,
        ],
    )?;
    Ok(())
}

/// 删除工作流定义
pub fn delete_definition(conn: &Connection, id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM workflow_definitions WHERE id = ?1", params![id])?;
    // CASCADE will delete related instances
    Ok(())
}

/// 列出工作流实例
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

    let instances = stmt.query_map(params_refs.as_slice(), |row| {
        let status_str: String = row.get(3)?;
        Ok(WorkflowInstance {
            id: row.get(0)?,
            definition_id: row.get(1)?,
            definition_name: row.get(2)?,
            status: serde_json::from_str(&format!("\"{}\"", status_str))
                .unwrap_or(WorkflowInstanceStatus::Pending),
            context: serde_json::from_str(&row.get::<_, String>(4)?)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
            steps: serde_json::from_str(&row.get::<_, String>(5)?)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
            current_node_id: row.get(6)?,
            trigger: row.get(7)?,
            trigger_detail: row.get(8)?,
            started_at: row.get(9)?,
            completed_at: row.get(10)?,
            estimated_remaining: row.get(11)?,
            error: row.get(12)?,
            created_at: row.get(13)?,
        })
    })?.collect::<Result<Vec<_>, _>>()?;

    Ok(instances)
}

/// 创建工作流实例
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
            instance.steps.to_string(),
            instance.current_node_id, instance.trigger, instance.trigger_detail,
            instance.started_at, instance.completed_at,
            instance.estimated_remaining, instance.error, now,
        ],
    )?;
    Ok(())
}

/// 更新工作流实例状态
pub fn update_instance_status(
    conn: &Connection,
    id: &str,
    status: &WorkflowInstanceStatus,
    context: Option<&serde_json::Value>,
    steps: Option<&serde_json::Value>,
    current_node_id: Option<&str>,
    error: Option<&str>,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    let status_str = serde_json::to_string(status).map_err(|e| AppError::External(e.to_string()))?;

    conn.execute(
        "UPDATE workflow_instances SET status = ?1, context = COALESCE(?2, context),
         steps = COALESCE(?3, steps), current_node_id = ?4, error = ?5, updated_at = ?6
         WHERE id = ?7",
        params![
            status_str,
            context.map(|c| c.to_string()),
            steps.map(|s| s.to_string()),
            current_node_id, error, now, id,
        ],
    )?;
    Ok(())
}

// ── Tauri Commands ──

#[tauri::command]
pub fn workflow_list_definitions(state: tauri::State<'_, crate::DbState>) -> Result<Vec<WorkflowDefinition>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    list_definitions(&conn).map_err(|e| format!("查询失败: {}", e))
}

#[tauri::command]
pub fn workflow_get_definition(state: tauri::State<'_, crate::DbState>, id: String) -> Result<Option<WorkflowDefinition>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    get_definition(&conn, &id).map_err(|e| format!("查询失败: {}", e))
}

#[tauri::command]
pub fn workflow_create_definition(state: tauri::State<'_, crate::DbState>, definition: WorkflowDefinition) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    create_definition(&conn, &definition).map_err(|e| format!("创建失败: {}", e))
}

#[tauri::command]
pub fn workflow_update_definition(state: tauri::State<'_, crate::DbState>, definition: WorkflowDefinition) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    update_definition(&conn, &definition).map_err(|e| format!("更新失败: {}", e))
}

#[tauri::command]
pub fn workflow_delete_definition(state: tauri::State<'_, crate::DbState>, id: String) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    delete_definition(&conn, &id).map_err(|e| format!("删除失败: {}", e))
}

#[tauri::command]
pub fn workflow_list_instances(state: tauri::State<'_, crate::DbState>, definition_id: Option<String>) -> Result<Vec<WorkflowInstance>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    list_instances(&conn, definition_id.as_deref()).map_err(|e| format!("查询失败: {}", e))
}

#[tauri::command]
pub fn workflow_create_instance(state: tauri::State<'_, crate::DbState>, instance: WorkflowInstance) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    create_instance(&conn, &instance).map_err(|e| format!("创建失败: {}", e))
}

#[tauri::command]
pub fn workflow_update_instance_status(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    status: String,
    context: Option<serde_json::Value>,
    steps: Option<serde_json::Value>,
    current_node_id: Option<String>,
    error: Option<String>,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;

    let status_enum: WorkflowInstanceStatus = serde_json::from_str(&format!("\"{}\"", status))
        .map_err(|e| format!("无效的状态值 '{}': {}", status, e))?;

    update_instance_status(&conn, &id, &status_enum, context.as_ref(), steps.as_ref(),
        current_node_id.as_deref(), error.as_deref())
        .map_err(|e| format!("更新失败: {}", e))
}


// ════════════════════════════════════════════════════════════
// 执行统计查询
// ════════════════════════════════════════════════════════════

/// 工作流执行统计
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStats {
    /// 总执行次数
    pub total_executions: i64,
    /// 成功次数
    pub success_count: i64,
    /// 失败次数
    pub failed_count: i64,
    /// 取消次数
    pub cancelled_count: i64,
    /// 成功率（0-100）
    pub success_rate: f64,
    /// 平均执行时长（毫秒）
    pub avg_duration_ms: f64,
    /// 最长执行时长（毫秒）
    pub max_duration_ms: i64,
    /// 最短执行时长（毫秒）
    pub min_duration_ms: i64,
    /// 总节点执行数
    pub total_node_executions: i64,
    /// 节点失败数
    pub node_failed_count: i64,
    /// 最近 7 天执行次数
    pub last_7_days_count: i64,
    /// 最近 30 天执行次数
    pub last_30_days_count: i64,
}

/// 获取工作流执行统计
pub fn get_workflow_stats(conn: &Connection, workflow_id: Option<&str>) -> Result<WorkflowStats, AppError> {
    let now_ts = crate::utils::now();
    let seven_days_ago = now_ts - 7 * 24 * 3600;
    let thirty_days_ago = now_ts - 30 * 24 * 3600;

    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {
        Some(wid) => (
            "WHERE definition_id = ?1".to_string(),
            vec![Box::new(wid.to_string())],
        ),
        None => ("".to_string(), vec![]),
    };

    // 总执行次数和各状态统计
    let total_sql = format!("SELECT COUNT(*) as total, 
        COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as success,
        COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed,
        COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END), 0) as cancelled
        FROM workflow_instances {}", filter_clause);

    let mut stmt = conn.prepare(&total_sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let (total, success, failed, cancelled): (i64, i64, i64, i64) = stmt.query_row(
        params_refs.as_slice(),
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;

    // 平均/最大/最小执行时长
    let duration_sql = format!(
        "SELECT COALESCE(AVG(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 ELSE NULL END), 0) as avg_dur,
                COALESCE(MAX(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 ELSE 0 END), 0) as max_dur,
                COALESCE(MIN(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 ELSE 0 END), 0) as min_dur
         FROM workflow_instances {}", filter_clause);

    let mut stmt2 = conn.prepare(&duration_sql)?;
    let params_refs2: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let (avg_duration_ms, max_duration_ms, min_duration_ms): (f64, i64, i64) = stmt2.query_row(
        params_refs2.as_slice(),
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;

    // 最近 7 天和 30 天执行次数
    let recent_sql = format!(
        "SELECT 
            COALESCE(SUM(CASE WHEN created_at >= ?1 THEN 1 ELSE 0 END), 0) as last_7,
            COALESCE(SUM(CASE WHEN created_at >= ?2 THEN 1 ELSE 0 END), 0) as last_30
         FROM workflow_instances {}", filter_clause);

    let mut stmt3 = conn.prepare(&recent_sql)?;
    let mut recent_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(seven_days_ago),
        Box::new(thirty_days_ago),
    ];
    if let Some(wid) = workflow_id {
        recent_params.push(Box::new(wid.to_string()));
    }
    let recent_refs: Vec<&dyn rusqlite::types::ToSql> = recent_params.iter().map(|p| p.as_ref()).collect();
    let (last_7_days_count, last_30_days_count): (i64, i64) = stmt3.query_row(
        recent_refs.as_slice(),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

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
        total_node_executions: 0,
        node_failed_count: 0,
        last_7_days_count,
        last_30_days_count,
    })
}

/// 执行时间线数据点
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionTimelinePoint {
    pub date: String,
    pub total: i64,
    pub success: i64,
    pub failed: i64,
    pub avg_duration_ms: f64,
}

/// 获取执行时间线（按天分组）
/// 返回最近 N 天的每日执行统计
pub fn get_execution_timeline(conn: &Connection, workflow_id: Option<&str>, days: i64) -> Result<Vec<ExecutionTimelinePoint>, AppError> {
    let now_ts = crate::utils::now();
    let start_ts = now_ts - days * 24 * 3600;

    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {
        Some(wid) => (
            "WHERE definition_id = ?1 AND created_at >= ?2".to_string(),
            vec![Box::new(wid.to_string()), Box::new(start_ts)],
        ),
        None => (
            "WHERE created_at >= ?1".to_string(),
            vec![Box::new(start_ts)],
        ),
    };

    let sql = format!(
        "SELECT 
            DATE(created_at, 'unixepoch') as day,
            COUNT(*) as total,
            COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) as success,
            COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed,
            COALESCE(AVG(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL THEN (completed_at - started_at) * 1000 ELSE NULL END), 0) as avg_dur
         FROM workflow_instances
         {}
         GROUP BY day
         ORDER BY day ASC", filter_clause);

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let points = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(ExecutionTimelinePoint {
            date: row.get(0)?,
            total: row.get(1)?,
            success: row.get(2)?,
            failed: row.get(3)?,
            avg_duration_ms: row.get(4)?,
        })
    })?.collect::<Result<Vec<_>, _>>()?;

    Ok(points)
}

/// 节点类型使用统计
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTypeStat {
    pub node_type: String,
    pub count: i64,
    pub failed_count: i64,
    pub avg_duration_ms: f64,
}

/// 获取节点类型使用统计
pub fn get_node_type_stats(conn: &Connection, workflow_id: Option<&str>) -> Result<Vec<NodeTypeStat>, AppError> {
    let (filter_clause, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match workflow_id {
        Some(wid) => (
            "WHERE id = ?1".to_string(),
            vec![Box::new(wid.to_string())],
        ),
        None => ("".to_string(), vec![]),
    };

    let sql = format!("SELECT nodes FROM workflow_definitions {}", filter_clause);
    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let mut type_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let nodes_str: String = row.get(0)?;
        Ok(nodes_str)
    })?.collect::<Result<Vec<_>, _>>()?;

    for nodes_str in &rows {
        if let Ok(nodes) = serde_json::from_str::<Vec<serde_json::Value>>(nodes_str) {
            for node in &nodes {
                if let Some(nt) = node.get("type").and_then(|v| v.as_str()) {
                    *type_counts.entry(nt.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    let stats: Vec<NodeTypeStat> = type_counts.into_iter().map(|(node_type, count)| {
        NodeTypeStat {
            node_type,
            count,
            failed_count: 0,
            avg_duration_ms: 0.0,
        }
    }).collect();

    Ok(stats)
}


// ════════════════════════════════════════════════════════════
// 工作流版本管理
// ════════════════════════════════════════════════════════════

/// 工作流版本
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowVersion {
    pub id: String,
    pub workflow_id: String,
    pub version: i64,
    pub snapshot: String,
    pub created_at: i64,
}

/// 复制工作流定义
pub fn duplicate_definition(conn: &Connection, id: &str, new_name: &str) -> Result<WorkflowDefinition, AppError> {
    let original = get_definition(conn, id)?
        .ok_or_else(|| AppError::NotFound("工作流不存在".into()))?;
    let mut new_def = original;
    new_def.id = crate::utils::new_id();
    new_def.name = new_name.to_string();
    new_def.version = "1.0.0".to_string();
    new_def.created_at = crate::utils::now();
    new_def.updated_at = new_def.created_at;
    create_definition(conn, &new_def)?;
    Ok(new_def)
}

/// 列出工作流版本
pub fn list_workflow_versions(conn: &Connection, workflow_id: &str) -> Result<Vec<WorkflowVersion>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, version, snapshot, created_at
         FROM workflow_versions WHERE workflow_id = ?1 ORDER BY version DESC"
    )?;
    let versions = stmt.query_map(params![workflow_id], |row| {
        Ok(WorkflowVersion {
            id: row.get(0)?,
            workflow_id: row.get(1)?,
            version: row.get(2)?,
            snapshot: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?.collect::<Result<Vec<_>, _>>()?;
    Ok(versions)
}

/// 保存工作流版本快照
pub fn save_workflow_version(conn: &Connection, workflow_id: &str, snapshot: &str) -> Result<WorkflowVersion, AppError> {
    let def = get_definition(conn, workflow_id)?
        .ok_or_else(|| AppError::NotFound("工作流不存在".into()))?;
    let new_version = def.version.parse::<i64>().unwrap_or(0) + 1;
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO workflow_versions (id, workflow_id, version, snapshot, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, workflow_id, new_version, snapshot, now],
    )?;
    Ok(WorkflowVersion {
        id,
        workflow_id: workflow_id.to_string(),
        version: new_version,
        snapshot: snapshot.to_string(),
        created_at: now,
    })
}

/// 恢复到指定版本
pub fn restore_workflow_version(conn: &Connection, workflow_id: &str, version: i64) -> Result<WorkflowVersion, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, version, snapshot, created_at FROM workflow_versions
         WHERE workflow_id = ?1 AND version = ?2"
    )?;
    let ver = stmt.query_row(params![workflow_id, version], |row| {
        Ok(WorkflowVersion {
            id: row.get(0)?,
            workflow_id: row.get(1)?,
            version: row.get(2)?,
            snapshot: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;

    // 从 snapshot 恢复定义
    let snapshot: WorkflowDefinition = serde_json::from_str(&ver.snapshot)
        .map_err(|e| AppError::Json(e.to_string()))?;
    update_definition(conn, &snapshot)?;

    Ok(ver)
}

// ════════════════════════════════════════════════════════════
// 节点执行日志
// ════════════════════════════════════════════════════════════

/// 节点执行日志
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

/// 获取节点执行日志
pub fn get_node_execution_logs(conn: &Connection, node_execution_id: &str) -> Result<Vec<NodeExecutionLogEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, execution_id, node_execution_id, timestamp, level, message, metadata
         FROM node_execution_logs WHERE node_execution_id = ?1 ORDER BY timestamp ASC"
    )?;
    let logs = stmt.query_map(params![node_execution_id], |row| {
        Ok(NodeExecutionLogEntry {
            id: row.get(0)?,
            execution_id: row.get(1)?,
            node_execution_id: row.get(2)?,
            timestamp: row.get(3)?,
            level: row.get(4)?,
            message: row.get(5)?,
            metadata: row.get(6)?,
        })
    })?.collect::<Result<Vec<_>, _>>()?;
    Ok(logs)
}

// ════════════════════════════════════════════════════════════
// 执行恢复
// ════════════════════════════════════════════════════════════

/// 可恢复的执行实例
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverableExecution {
    pub execution: WorkflowInstance,
    pub completed_nodes: Vec<String>,
    pub failed_nodes: Vec<String>,
    pub pending_nodes: Vec<String>,
}

/// 列出可恢复的执行（paused 或 running 状态且存在未完成节点）
pub fn list_recoverable_executions(conn: &Connection) -> Result<Vec<RecoverableExecution>, AppError> {
    let instances = list_instances(conn, None)?;
    let mut recoverable = Vec::new();

    for inst in instances {
        let status_str = format!("{:?}", inst.status);
        if status_str != "Paused" && status_str != "Running" {
            continue;
        }

        let steps = inst.steps.as_object().cloned().unwrap_or_default();
        let mut completed = Vec::new();
        let mut failed = Vec::new();
        let mut pending = Vec::new();

        for (node_id, step) in &steps {
            match step.get("status").and_then(|s| s.as_str()) {
                Some("success") => completed.push(node_id.clone()),
                Some("failed") => failed.push(node_id.clone()),
                _ => pending.push(node_id.clone()),
            }
        }

        // 只有存在失败或待处理的节点时才可恢复
        if !failed.is_empty() || !pending.is_empty() {
            recoverable.push(RecoverableExecution {
                execution: inst,
                completed_nodes: completed,
                failed_nodes: failed,
                pending_nodes: pending,
            });
        }
    }

    Ok(recoverable)
}

/// 恢复执行（将失败节点重置为 pending）
pub fn recover_execution(conn: &Connection, execution_id: &str) -> Result<WorkflowInstance, AppError> {
    let instances = list_instances(conn, None)?;
    let mut instance = instances.into_iter()
        .find(|i| i.id == execution_id)
        .ok_or_else(|| AppError::NotFound("执行实例不存在".into()))?;

    let status_str = format!("{:?}", instance.status);
    if status_str != "Paused" && status_str != "Failed" {
        return Err(AppError::InvalidInput("只能恢复已暂停或已失败的工作流".into()));
    }

    // 将失败节点重置为 pending
    if let Some(steps) = instance.steps.as_object_mut() {
        for (_, step) in steps.iter_mut() {
            if step.get("status").and_then(|s| s.as_str()) == Some("failed") {
                step.as_object_mut().map(|obj| {
                    obj.insert("status".to_string(), serde_json::Value::String("pending".to_string()));
                    obj.remove("error");
                });
            }
        }
    }

    instance.status = WorkflowInstanceStatus::Running;
    instance.error = None;

    update_instance_status(conn, &instance.id, &instance.status,
        Some(&instance.context), Some(&instance.steps),
        instance.current_node_id.as_deref(), None)?;

    Ok(instance)
}

// ════════════════════════════════════════════════════════════
// 人工介入查询
// ════════════════════════════════════════════════════════════

/// 待响应的人工介入
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

/// 获取所有待响应的人工介入节点
pub fn get_pending_human_inputs(conn: &Connection) -> Result<Vec<PendingHumanInput>, AppError> {
    let instances = list_instances(conn, None)?;
    let mut pending = Vec::new();

    for inst in instances {
        let status_str = format!("{:?}", inst.status);
        if status_str != "Paused" && status_str != "Running" {
            continue;
        }

        if let Some(steps) = inst.steps.as_object() {
            for (node_id, step) in steps {
                if step.get("status").and_then(|s| s.as_str()) == Some("running") {
                    // 检查该节点是否有人工介入配置
                    if let Some(output) = step.get("output") {
                        if output.get("type").and_then(|t| t.as_str()) == Some("human_input") {
                            pending.push(PendingHumanInput {
                                execution_id: inst.id.clone(),
                                node_id: node_id.clone(),
                                node_label: node_id.clone(),
                                prompt: output.get("prompt").and_then(|p| p.as_str()).unwrap_or("请输入").to_string(),
                                input_type: output.get("inputType").and_then(|t| t.as_str()).unwrap_or("text").to_string(),
                                created_at: inst.created_at,
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(pending)
}
