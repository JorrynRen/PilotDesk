use serde_json::Value;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use crate::workflow::registry::NodeDef;
use crate::utils::{new_id, now};
use crate::workflow::executor::NodeExecutor;
use crate::workflow::engine::WorkflowEngine;
use super::super::workflow;

// ════════════════════════════════════════════════════════════
// 工作流 CRUD 命令
// ════════════════════════════════════════════════════════════

/// 创建工作流
#[tauri::command]
pub fn create_workflow(
    state: tauri::State<'_, crate::DbState>,
    name: String,
    description: Option<String>,
) -> Result<workflow::WorkflowDefinition, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let id = new_id();
    let ts = now();
    let def = workflow::WorkflowDefinition {
        id: id.clone(),
        name,
        version: "1.0.0".to_string(),
        description: description.unwrap_or_default(),
        trigger: workflow::TriggerConfig {
            trigger_type: workflow::TriggerType::Manual,
            cron: None,
            event_name: None,
        },
        stages: vec![workflow::Stage {
            id: crate::utils::new_id(),
            name: "默认阶段".into(),
            order: 0,
            nodes: vec![],
            edges: vec![],
            gate: workflow::GateConfig::default(),
        }],
        input_schema: None,
        output_schema: None,
        max_depth: None,
        created_at: ts,
        updated_at: ts,
        enabled: true,
    };
    workflow::create_definition(&conn, &def).map_err(|e| format!("创建失败: {}", e))?;
    Ok(def)
}

/// 获取工作流列表
#[tauri::command]
pub fn list_workflows(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<workflow::WorkflowDefinition>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    workflow::list_definitions(&conn).map_err(|e| format!("查询失败: {}", e))
}

/// 获取工作流详情
#[tauri::command]
pub fn get_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<Option<workflow::WorkflowDefinition>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    workflow::get_definition(&conn, &id).map_err(|e| format!("查询失败: {}", e))
}

/// 更新工作流
#[tauri::command]
pub fn update_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    name: Option<String>,
    description: Option<String>,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let mut def = workflow::get_definition(&conn, &id)
        .map_err(|e| format!("查询失败: {}", e))?
        .ok_or_else(|| "工作流不存在".to_string())?;
    if let Some(n) = name { def.name = n; }
    if let Some(d) = description { def.description = d; }
    workflow::update_definition(&conn, &def).map_err(|e| format!("更新失败: {}", e))
}

/// 删除工作流
#[tauri::command]
pub fn delete_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    workflow::delete_definition(&conn, &id).map_err(|e| format!("删除失败: {}", e))
}

/// 保存完整工作流定义（全量对象，匹配前端 store 调用）
#[tauri::command]
pub fn save_workflow_definition(
    state: tauri::State<'_, crate::DbState>,
    definition: crate::workflow::WorkflowDefinition,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    // 检查是否存在
    let existing = crate::workflow::get_definition(&conn, &definition.id)
        .map_err(|e| format!("查询失败: {}", e))?;
    if existing.is_some() {
        crate::workflow::update_definition(&conn, &definition)
            .map_err(|e| format!("更新失败: {}", e))
    } else {
        crate::workflow::create_definition(&conn, &definition)
            .map_err(|e| format!("创建失败: {}", e))
    }
}

/// 保存工作流（阶段结构）
#[tauri::command]
pub fn save_workflow_dag(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    stages: Vec<workflow::Stage>,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let mut def = workflow::get_definition(&conn, &id)
        .map_err(|e| format!("查询失败: {}", e))?
        .ok_or_else(|| "工作流不存在".to_string())?;
    def.stages = stages;
    workflow::update_definition(&conn, &def).map_err(|e| format!("保存失败: {}", e))
}

// ════════════════════════════════════════════════════════════
// 执行控制命令
// ════════════════════════════════════════════════════════════

/// 启动工作流执行
#[tauri::command]
pub async fn start_workflow(
    state: tauri::State<'_, crate::DbState>,
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    app_handle: tauri::AppHandle,
    workflow_id: String,
    _version: Option<i64>,
    input_data: Option<Value>,
) -> Result<workflow::WorkflowInstance, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;

    // 1. 获取工作流定义
    let def = workflow::get_definition(&conn, &workflow_id)
        .map_err(|e| format!("查询失败: {}", e))?
        .ok_or_else(|| "工作流不存在".to_string())?;

    // 2. 创建工作流实例
    let instance_id = new_id();
    let now_ts = now();
    let instance = workflow::WorkflowInstance {
        id: instance_id.clone(),
        definition_id: def.id.clone(),
        definition_name: def.name.clone(),
        status: workflow::WorkflowInstanceStatus::Running,
        context: serde_json::json!({}),
        steps: serde_json::json!({}),
        current_node_id: None,
        trigger: "manual".to_string(),
        trigger_detail: None,
        started_at: Some(now_ts),
        completed_at: None,
        estimated_remaining: None,
        error: None,
        created_at: now_ts,
    };

    workflow::create_instance(&conn, &instance)
        .map_err(|e| format!("创建实例失败: {}", e))?;

    // 3. 读取最大并发数设置
    let max_concurrency: usize = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'workflow_max_concurrency'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    // 4. 在后台执行工作流（使用 WorkflowEngine）
    let def_clone = def.clone();
    let instance_id_clone = instance_id.clone();
    let executor = executor.inner().clone();
    let app_handle_clone = app_handle.clone();

    tokio::spawn(async move {
        if let Err(e) = WorkflowEngine::execute_with_concurrency(
            &executor,
            &def_clone,
            &instance_id_clone,
            input_data.unwrap_or(Value::Null),
            &app_handle_clone,
            max_concurrency,
        ).await {
            let _ = app_handle_clone.emit("workflow:execution-status", serde_json::json!({
                "execution_id": instance_id_clone,
                "status": "failed",
                "error": e.to_string(),
            }));
        }
    });

    Ok(instance)
}

/// 中止工作流执行
#[tauri::command]
pub fn cancel_workflow(
    state: tauri::State<'_, crate::DbState>,
    execution_id: String,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    workflow::update_instance_status(&conn, &execution_id,
        &workflow::WorkflowInstanceStatus::Cancelled, None, None, None, Some("用户中止"))
        .map_err(|e| format!("更新失败: {}", e))
}

/// 获取执行状态
#[tauri::command]
pub fn get_execution(
    state: tauri::State<'_, crate::DbState>,
    execution_id: String,
) -> Result<Option<workflow::WorkflowInstance>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    // 按 ID 直接查询，避免全量扫描
    let mut stmt = conn.prepare(
        "SELECT id, definition_id, definition_name, status, context, steps,
                current_node_id, trigger, trigger_detail,
                started_at, completed_at, estimated_remaining, error, created_at
         FROM workflow_instances WHERE id = ?1"
    ).map_err(|e| format!("查询失败: {}", e))?;
    let mut rows = stmt.query_map(rusqlite::params![execution_id], |row| workflow::instance_from_row(row))
        .map_err(|e| format!("查询失败: {}", e))?;
    match rows.next() {
        Some(Ok(inst)) => Ok(Some(inst)),
        Some(Err(e)) => Err(format!("解析失败: {}", e)),
        None => Ok(None),
    }
}

/// 获取执行历史
#[tauri::command]
pub fn list_executions(
    state: tauri::State<'_, crate::DbState>,
    definition_id: Option<String>,
) -> Result<Vec<workflow::WorkflowInstance>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    workflow::list_instances(&conn, definition_id.as_deref()).map_err(|e| format!("查询失败: {}", e))
}

/// 获取节点执行详情
#[tauri::command]
pub fn get_node_executions(
    state: tauri::State<'_, crate::DbState>,
    execution_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    // 从 node_executions 表查询（而非废弃的 instance.steps JSON）
    let mut stmt = conn.prepare(
        "SELECT node_id, status, input_data, output_data, error_message, started_at, finished_at
         FROM node_executions WHERE execution_id = ?1 ORDER BY started_at ASC"
    ).map_err(|e| format!("查询失败: {}", e))?;
    let rows = stmt.query_map(rusqlite::params![execution_id], |row| {
        Ok(serde_json::json!({
            "nodeId": row.get::<_, String>(0)?,
            "status": row.get::<_, String>(1)?,
            "input": row.get::<_, Option<String>>(2)?.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
            "output": row.get::<_, Option<String>>(3)?.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
            "error": row.get::<_, Option<String>>(4)?,
            "startedAt": row.get::<_, Option<i64>>(5)?,
            "finishedAt": row.get::<_, Option<i64>>(6)?,
        }))
    }).map_err(|e| format!("查询失败: {}", e))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("解析失败: {}", e))?;
    Ok(rows)
}

/// 响应人工介入
#[tauri::command]
pub async fn respond_human_input(
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    execution_id: String,
    node_id: String,
    response: String,
) -> Result<(), String> {
    executor.human_input_manager.resolve(&execution_id, &node_id, response)
        .map_err(|e| format!("响应失败: {}", e))
}



/// 创建定时调度
#[tauri::command]
pub fn create_schedule(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
    cron_expression: String,
    input_data: Option<String>,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let sched = crate::workflow::scheduler::WorkflowSchedule {
        id: crate::utils::new_id(),
        workflow_id,
        cron_expression,
        enabled: true,
        input_data: input_data.unwrap_or_default(),
        last_run_at: None,
        next_run_at: Some(crate::utils::now() + 60),
        created_at: crate::utils::now(),
        updated_at: crate::utils::now(),
    };
    crate::workflow::scheduler::create_schedule(&conn, &sched)
        .map_err(|e| format!("创建调度失败: {}", e))
}

/// 获取调度列表
#[tauri::command]
pub fn list_schedules(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<crate::workflow::scheduler::WorkflowSchedule>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::scheduler::list_schedules(&conn).map_err(|e| format!("查询失败: {}", e))
}

/// 删除调度
#[tauri::command]
pub fn delete_schedule(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::scheduler::delete_schedule(&conn, &id).map_err(|e| format!("删除失败: {}", e))
}

/// 导出工作流为 JSON
#[tauri::command]
pub fn export_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<String, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let def = workflow::get_definition(&conn, &id)
        .map_err(|e| format!("查询失败: {}", e))?
        .ok_or_else(|| "工作流不存在".to_string())?;
    serde_json::to_string_pretty(&def).map_err(|e| format!("序列化失败: {}", e))
}

/// 从 JSON 导入工作流
#[tauri::command]
pub fn import_workflow(
    state: tauri::State<'_, crate::DbState>,
    json_data: String,
) -> Result<workflow::WorkflowDefinition, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let mut def: workflow::WorkflowDefinition = serde_json::from_str(&json_data)
        .map_err(|e| format!("JSON 解析失败: {}", e))?;
    // 重新生成 ID 和更新时间
    def.id = crate::utils::new_id();
    def.created_at = crate::utils::now();
    def.updated_at = def.created_at;
    workflow::create_definition(&conn, &def).map_err(|e| format!("导入失败: {}", e))?;
    Ok(def)
}

/// 测试单个节点执行
#[tauri::command]
pub async fn test_node(
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    app_handle: tauri::AppHandle,
    node_type: String,
    config: String,
    input_data: Option<String>,
) -> Result<serde_json::Value, String> {
    let node_def = NodeDef {
        id: "test_node".to_string(),
        node_type,
        label: "测试节点".to_string(),
        config: serde_json::from_str(&config).map_err(|e| format!("配置解析失败: {}", e))?,
        plugin_id: None,
        command_id: None,
        timeout_seconds: Some(60),
        retry_count: Some(0),
        retry_interval_ms: None,
    };
    let input = input_data
        .map(|s| serde_json::from_str(&s).unwrap_or(Value::String(s)))
        .unwrap_or(Value::Null);
    let result = executor.execute(&node_def, input, "test", &app_handle).await
        .map_err(|e| format!("执行失败: {}", e))?;
    Ok(result.output)
}

/// 获取所有注册的节点类型
#[tauri::command]
pub fn list_node_types(
    executor: tauri::State<'_, Arc<NodeExecutor>>,
) -> Result<Vec<crate::workflow::registry::NodeTypeRegistrationInfo>, String> {
    Ok(executor.list_node_types())
}


// ════════════════════════════════════════════════════════════
// 执行统计命令
// ════════════════════════════════════════════════════════════

/// 获取工作流执行统计
#[tauri::command]
pub fn get_workflow_stats(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: Option<String>,
) -> Result<crate::workflow::WorkflowStats, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::get_workflow_stats(&conn, workflow_id.as_deref())
        .map_err(|e| format!("查询统计失败: {}", e))
}

/// 获取执行时间线
#[tauri::command]
pub fn get_execution_timeline(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: Option<String>,
    days: Option<i64>,
) -> Result<Vec<crate::workflow::ExecutionTimelinePoint>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::get_execution_timeline(&conn, workflow_id.as_deref(), days.unwrap_or(30))
        .map_err(|e| format!("查询时间线失败: {}", e))
}

/// 获取节点类型使用统计
#[tauri::command]
pub fn get_node_type_stats(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: Option<String>,
) -> Result<Vec<crate::workflow::NodeTypeStat>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::get_node_type_stats(&conn, workflow_id.as_deref())
        .map_err(|e| format!("查询节点类型统计失败: {}", e))
}


/// 获取工作流最大并发数
#[tauri::command]
pub fn get_workflow_max_concurrency(
    state: tauri::State<'_, crate::DbState>,
) -> Result<usize, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let value: String = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'workflow_max_concurrency'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "5".to_string());
    Ok(value.parse::<usize>().unwrap_or(5))
}

/// 设置工作流最大并发数
#[tauri::command]
pub fn set_workflow_max_concurrency(
    state: tauri::State<'_, crate::DbState>,
    max_concurrency: usize,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let clamped = max_concurrency.clamp(1, 20);
    conn.execute(
        "INSERT OR REPLACE INTO app_settings (key, value, updated_at) VALUES ('workflow_max_concurrency', ?1, ?2)",
        rusqlite::params![clamped.to_string(), crate::utils::now()],
    ).map_err(|e| format!("保存失败: {}", e))?;
    Ok(())
}

// ════════════════════════════════════════════════════════════
// 工作流版本管理命令
// ════════════════════════════════════════════════════════════

/// 复制工作流
#[tauri::command]
pub fn duplicate_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    new_name: String,
) -> Result<crate::workflow::WorkflowDefinition, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::duplicate_definition(&conn, &id, &new_name)
        .map_err(|e| format!("复制失败: {}", e))
}

/// 列出工作流版本历史
#[tauri::command]
pub fn list_workflow_versions(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
) -> Result<Vec<crate::workflow::WorkflowVersion>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::list_workflow_versions(&conn, &workflow_id)
        .map_err(|e| format!("查询版本失败: {}", e))
}

/// 保存工作流版本快照
#[tauri::command]
pub fn save_workflow_version(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
    snapshot: String,
) -> Result<crate::workflow::WorkflowVersion, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::save_workflow_version(&conn, &workflow_id, &snapshot)
        .map_err(|e| format!("保存版本失败: {}", e))
}

/// 恢复到指定版本
#[tauri::command]
pub fn restore_workflow_version(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
    version: i64,
) -> Result<crate::workflow::WorkflowVersion, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::restore_workflow_version(&conn, &workflow_id, version)
        .map_err(|e| format!("恢复版本失败: {}", e))
}

// ════════════════════════════════════════════════════════════
// 节点执行日志命令
// ════════════════════════════════════════════════════════════

/// 获取节点执行日志
#[tauri::command]
pub fn get_node_execution_logs(
    state: tauri::State<'_, crate::DbState>,
    node_execution_id: String,
) -> Result<Vec<crate::workflow::NodeExecutionLogEntry>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::get_node_execution_logs(&conn, &node_execution_id)
        .map_err(|e| format!("查询日志失败: {}", e))
}

// ════════════════════════════════════════════════════════════
// 执行恢复命令
// ════════════════════════════════════════════════════════════

/// 列出可恢复的执行
#[tauri::command]
pub fn list_recoverable_executions(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<crate::workflow::RecoverableExecution>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::list_recoverable_executions(&conn)
        .map_err(|e| format!("查询可恢复执行失败: {}", e))
}

/// 恢复执行（从 checkpoint 继续执行）
#[tauri::command]
pub async fn recover_execution(
    app_handle: tauri::AppHandle,
    execution_id: String,
) -> Result<serde_json::Value, String> {
    let conn = app_handle.state::<crate::DbState>().get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    // 从执行实例中获取 definition_id，而非直接用 execution_id 查定义
    let instance = crate::workflow::list_instances(&conn, None)
        .map_err(|e| format!("查询失败: {}", e))?
        .into_iter()
        .find(|i| i.id == execution_id)
        .ok_or_else(|| "执行实例不存在".to_string())?;
    let def = crate::workflow::get_definition(&conn, &instance.definition_id)
        .map_err(|e| format!("查询失败: {}", e))?
        .ok_or_else(|| "工作流定义不存在".to_string())?;
    let executor = app_handle.state::<std::sync::Arc<crate::workflow::executor::NodeExecutor>>();
    crate::workflow::engine::WorkflowEngine::recover_execution(
        &executor.inner(),
        &def,
        &execution_id,
        serde_json::Value::Object(serde_json::Map::new()),
        &app_handle,
        5, // 默认并发数
    ).await.map_err(|e| e.to_string())
        .map_err(|e| format!("恢复执行失败: {}", e))
}

// ════════════════════════════════════════════════════════════
// 人工介入查询命令
// ════════════════════════════════════════════════════════════

/// 获取所有待响应的人工介入节点
#[tauri::command]
pub fn get_pending_human_inputs(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<crate::workflow::PendingHumanInput>, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::get_pending_human_inputs(&conn)
        .map_err(|e| format!("查询待响应请求失败: {}", e))
}
