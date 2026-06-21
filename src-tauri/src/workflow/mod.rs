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
