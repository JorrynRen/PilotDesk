#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tauri::{Emitter, Manager};
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
                stage_edges: vec![],
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
    #[allow(unused_variables)]
    version: Option<i64>, // TODO: 版本管理支持（当前未实现版本化执行）
    input_data: Option<Value>,
    // 前端预生成的实例 ID（解决快速工作流的竞态条件）
    instance_id: Option<String>,
) -> Result<workflow::WorkflowInstance, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;

    // 1. 获取工作流定义
    let def = workflow::get_definition(&conn, &workflow_id)
        .map_err(|e| format!("查询失败: {}", e))?
        .ok_or_else(|| "工作流不存在".to_string())?;

    // 2. 创建工作流实例（优先使用前端预生成的 ID，消除 IPC 竞态）
    let instance_id = instance_id.unwrap_or_else(new_id);
    let now_ts = now();
    let instance = workflow::WorkflowInstance {
        id: instance_id.clone(),
        definition_id: def.id.clone(),
        definition_name: def.name.clone(),
        status: workflow::WorkflowInstanceStatus::Running,
        context: serde_json::json!({}),
        steps: None,
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
        log::info!("[WorkflowEngine] 开始执行工作流: id={}, name={}", instance_id_clone, def_clone.name);
        // 克隆值供内层 spawn 使用（外层已 move）
        let inner_executor = executor.clone();
        let inner_def = def_clone.clone();
        let inner_id = instance_id_clone.clone();
        let inner_input = input_data.unwrap_or(Value::Null);
        let inner_emitter = app_handle_clone.clone();
        let inner_concurrency = max_concurrency;
        // 内层 spawn：引擎在此运行，panic 会被 JoinHandle 捕获
        let inner_handle = tokio::spawn(async move {
            WorkflowEngine::execute_with_concurrency(
                &inner_executor,
                &inner_def,
                &inner_id,
                inner_input,
                &inner_emitter,
                inner_concurrency,
            ).await
        });
        // 外层 await JoinHandle：捕获内层 panic
        match inner_handle.await {
            Ok(Ok(_output)) => {
                log::info!("[WorkflowEngine] 工作流执行成功: id={}", instance_id_clone);
                let _ = app_handle_clone.emit("workflow:execution-status", serde_json::json!({
                    "execution_id": instance_id_clone,
                    "definition_id": def_clone.id,
                    "definition_name": def_clone.name,
                    "status": "completed",
                }));
            }
            Ok(Err(e)) => {
                log::error!("[WorkflowEngine] 工作流执行失败: id={}, error={}", instance_id_clone, e);
                // 回滚：更新实例状态为 failed
                if let Some(conn) = app_handle_clone.try_state::<crate::DbState>().and_then(|s| s.get_conn().ok()) {
                    let _ = conn.execute(
                        "UPDATE workflow_instances SET status = 'failed', error = ?1, completed_at = ?2, updated_at = ?2 WHERE id = ?3",
                        rusqlite::params![e.to_string(), crate::utils::now(), instance_id_clone],
                    );
                }
                let _ = app_handle_clone.emit("workflow:execution-status", serde_json::json!({
                    "execution_id": instance_id_clone,
                    "definition_id": def_clone.id,
                    "definition_name": def_clone.name,
                    "status": "failed",
                    "error": e.to_string(),
                }));
            }
            Err(join_err) => {
                let msg = if join_err.is_panic() {
                    join_err.into_panic().downcast::<String>().map(|s| *s).unwrap_or_else(|_| "未知 panic".to_string())
                } else {
                    "任务被取消".to_string()
                };
                log::error!("[WorkflowEngine] 工作流执行 panic: id={}, error={}", instance_id_clone, msg);
                // 回滚：更新实例状态为 failed
                if let Some(conn) = app_handle_clone.try_state::<crate::DbState>().and_then(|s| s.get_conn().ok()) {
                    let _ = conn.execute(
                        "UPDATE workflow_instances SET status = 'failed', error = ?1, completed_at = ?2, updated_at = ?2 WHERE id = ?3",
                        rusqlite::params![msg, crate::utils::now(), instance_id_clone],
                    );
                }
                let _ = app_handle_clone.emit("workflow:execution-status", serde_json::json!({
                    "execution_id": instance_id_clone,
                    "definition_id": def_clone.id,
                    "definition_name": def_clone.name,
                    "status": "failed",
                    "error": format!("工作流引擎内部错误: {}", msg),
                }));
            }
        }
    });

    Ok(instance)
}

/// 中止工作流执行
#[tauri::command]
pub async fn cancel_workflow(
    state: tauri::State<'_, crate::DbState>,
    executor: tauri::State<'_, Arc<crate::workflow::executor::NodeExecutor>>,
    execution_id: String,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    // 1. 标记数据库状态为已取消
    let status_str = serde_json::to_string(&workflow::WorkflowInstanceStatus::Cancelled).unwrap();
    let now = crate::utils::now();
    conn.execute(
        "UPDATE workflow_instances SET status = ?1, error = ?2, updated_at = ?3 WHERE id = ?4",
        rusqlite::params![status_str, "用户中止", now, execution_id],
    ).map_err(|e| format!("更新失败: {}", e))?;

    // 2. 停止该执行关联的所有 Agent 子进程
    // 工作流引擎的临时 session_id 格式为 wf_{execution_id}_{node_id}
    let node_ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT node_id FROM node_executions WHERE execution_id = ?1"
        ).map_err(|e| format!("查询失败: {}", e))?;
        let rows = stmt.query_map(rusqlite::params![execution_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询失败: {}", e))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let agent_manager = executor.inner().agent_manager();
    let mut mgr = agent_manager.lock().await;
    for node_id in &node_ids {
        let session_id = format!("wf_{}_{}", execution_id, node_id);
        mgr.stop_generation(&session_id);
    }
    log::info!("[cancel_workflow] 已取消执行: {}, 关联节点: {:?}", execution_id, node_ids);
    Ok(())
}

/// 删除工作流执行记录
#[tauri::command]
pub fn delete_execution(
    state: tauri::State<'_, crate::DbState>,
    execution_id: String,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    conn.execute("DELETE FROM workflow_instances WHERE id = ?1", rusqlite::params![execution_id])
        .map_err(|e| format!("删除执行记录失败: {}", e))?;
    Ok(())
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

/// 响应插件命令执行结果（前端执行完插件命令后回传）
#[tauri::command]
pub async fn respond_plugin_execute(
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    execution_id: String,
    node_id: String,
    result: crate::workflow::executors::plugin_executor::PluginExecuteResult,
) -> Result<(), String> {
    executor.plugin_execute_manager.resolve(&execution_id, &node_id, result)
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

/// 导出工作流专用结构体（过滤导入时不使用的字段）
/// 所有 ID 替换为短标识符（s1/s2 表示阶段，n1/n2 表示节点，e1/e2 表示边）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportWorkflowDefinition {
    pub name: String,
    pub version: String,
    pub description: String,
    pub trigger: workflow::TriggerConfig,
    pub stages: Vec<ExportStage>,
    #[serde(default)]
    pub stage_edges: Vec<ExportEdge>,
    pub enabled: bool,
}

/// 导出阶段（过滤 id）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportStage {
    pub name: String,
    pub order: usize,
    pub nodes: Vec<ExportNode>,
    pub edges: Vec<ExportEdge>,
    pub gate: workflow::GateConfig,
}

/// 导出节点（过滤 id/plugin_id/command_id/input_schema/output_schema）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportNode {
    #[serde(rename = "type")]
    pub node_type: workflow::WorkflowNodeType,
    pub label: String,
    pub params: Option<serde_json::Value>,
    pub delay_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub retry_count: Option<u32>,
    pub retry_delay_ms: Option<u64>,
    pub input_mapping: Option<serde_json::Value>,
    pub output_mapping: Option<serde_json::Value>,
    pub position: Option<serde_json::Value>,
}

/// 导出边（过滤 id，source/target 引用导出节点短标识符）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEdge {
    pub source: String,
    pub target: String,
    pub label: Option<String>,
    pub condition: Option<String>,
}

impl From<workflow::WorkflowDefinition> for ExportWorkflowDefinition {
    fn from(def: workflow::WorkflowDefinition) -> Self {
        // 按阶段构建短标识符映射（s1, s2, ...）
        let stage_short_ids: std::collections::HashMap<String, String> =
            def.stages.iter().enumerate()
                .map(|(i, s)| (s.id.clone(), format!("s{}", i + 1)))
                .collect();

        // 阶段连线导出：source/target 使用阶段短标识符
        let stage_edges: Vec<ExportEdge> = def.stage_edges.iter().filter_map(|se| {
            let src = stage_short_ids.get(&se.source).cloned();
            let tgt = stage_short_ids.get(&se.target).cloned();
            match (src, tgt) {
                (Some(source), Some(target)) => Some(ExportEdge {
                    source, target, label: None, condition: None,
                }),
                _ => None, // 跳过引用无效阶段ID的连线
            }
        }).collect();

        // 按阶段构建局部节点编号映射（每个阶段从 n1 开始）
        // 注意：必须按阶段局部编号，与 into_definition 中的编号方式一致
        let stages: Vec<ExportStage> = def.stages.into_iter().map(|stage| {
            // 为该阶段内的节点构建 old_uuid -> index 映射
            let mut stage_node_ids: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for node in &stage.nodes {
                let ni = stage_node_ids.len();
                stage_node_ids.entry(node.id.clone()).or_insert(ni);
            }

            let nodes: Vec<ExportNode> = stage.nodes.into_iter().map(|node| {
                // Replace UUIDs in input_mapping/output_mapping with short IDs (n1, n2, ...)
                let input_mapping = Self::remap_mapping_short_ids(&node.input_mapping, &stage_node_ids);
                let output_mapping = Self::remap_mapping_short_ids(&node.output_mapping, &stage_node_ids);
                ExportNode {
                    node_type: node.node_type,
                    label: node.label,
                    params: node.params,
                    delay_ms: node.delay_ms,
                    timeout_ms: node.timeout_ms,
                    retry_count: node.retry_count,
                    retry_delay_ms: node.retry_delay_ms,
                    input_mapping,
                    output_mapping,
                    position: node.position,
                }
            }).collect();

            let edges: Vec<ExportEdge> = stage.edges.into_iter().map(|edge| {
                let src_idx = stage_node_ids.get(&edge.source).copied().unwrap_or(0);
                let tgt_idx = stage_node_ids.get(&edge.target).copied().unwrap_or(0);
                ExportEdge {
                    source: format!("n{}", src_idx + 1),
                    target: format!("n{}", tgt_idx + 1),
                    label: edge.label,
                    condition: edge.condition,
                }
            }).collect();

            ExportStage {
                name: stage.name,
                order: stage.order,
                nodes,
                edges,
                gate: stage.gate,
            }
        }).collect();

        Self {
            name: def.name,
            version: def.version,
            description: def.description,
            trigger: def.trigger,
            stages,
            stage_edges,
            enabled: def.enabled,
        }
    }
}

impl ExportWorkflowDefinition {
    /// 导入时重建完整 WorkflowDefinition（生成新 UUID）
    pub fn into_definition(self) -> workflow::WorkflowDefinition {
        let now_ts = crate::utils::now();
        let wf_id = crate::utils::new_id();

        let stages: Vec<workflow::Stage> = self.stages.into_iter().map(|stage| {
            let nodes: Vec<workflow::WorkflowNode> = stage.nodes.into_iter().map(|en| {
                workflow::WorkflowNode {
                    id: String::new(), // will be filled below
                    node_type: en.node_type,
                    label: en.label,
                    plugin_id: None,
                    command_id: None,
                    params: en.params,
                    delay_ms: en.delay_ms,
                    timeout_ms: en.timeout_ms,
                    retry_count: en.retry_count,
                    retry_delay_ms: en.retry_delay_ms,
                    input_schema: None,
                    output_schema: None,
                    input_mapping: en.input_mapping,
                    output_mapping: en.output_mapping,
                    position: en.position,
                }
            }).collect();

            // Assign new UUIDs to nodes
            let nodes: Vec<workflow::WorkflowNode> = nodes.into_iter().map(|mut n| {
                n.id = crate::utils::new_id();
                n
            }).collect();

            // Build node short_id -> new_uuid map for this stage
            let stage_node_map: std::collections::HashMap<String, String> = nodes.iter().enumerate()
                .map(|(i, n)| (format!("n{}", i + 1), n.id.clone()))
                .collect();

            let edges: Vec<workflow::WorkflowEdge> = stage.edges.into_iter().map(|ee| {
                let new_source = stage_node_map.get(&ee.source).cloned().unwrap_or_else(crate::utils::new_id);
                let new_target = stage_node_map.get(&ee.target).cloned().unwrap_or_else(crate::utils::new_id);
                workflow::WorkflowEdge {
                    id: crate::utils::new_id(),
                    source: new_source,
                    target: new_target,
                    label: ee.label,
                    condition: ee.condition,
                }
            }).collect();

            // Update input_mapping/output_mapping: replace short IDs (n1, n2) with new UUIDs
            let nodes: Vec<workflow::WorkflowNode> = nodes.into_iter().map(|mut n| {
                if let Some(ref mapping) = n.input_mapping {
                    if let Some(val) = mapping.as_object() {
                        let new_mapping = val.iter()
                            .map(|(k, v)| {
                                let new_v = if let Some(s) = v.as_str() {
                                    Self::remap_mapping_value_uuids(s, &stage_node_map)
                                        .map(serde_json::Value::String)
                                        .unwrap_or_else(|| v.clone())
                                } else {
                                    v.clone()
                                };
                                (k.clone(), new_v)
                            })
                            .collect();
                        n.input_mapping = Some(serde_json::Value::Object(new_mapping));
                    }
                }
                if let Some(ref mapping) = n.output_mapping {
                    if let Some(val) = mapping.as_object() {
                        let new_mapping = val.iter()
                            .map(|(k, v)| {
                                let new_v = if let Some(s) = v.as_str() {
                                    Self::remap_mapping_value_uuids(s, &stage_node_map)
                                        .map(serde_json::Value::String)
                                        .unwrap_or_else(|| v.clone())
                                } else {
                                    v.clone()
                                };
                                (k.clone(), new_v)
                            })
                            .collect();
                        n.output_mapping = Some(serde_json::Value::Object(new_mapping));
                    }
                }
                n
            }).collect();

            workflow::Stage {
                id: crate::utils::new_id(),
                name: stage.name,
                order: stage.order,
                nodes,
                edges,
                gate: stage.gate,
            }
        }).collect();

        // 构建阶段短标识符 -> 新UUID 映射
        let stage_short_to_uuid: std::collections::HashMap<String, String> =
            stages.iter().enumerate()
                .map(|(i, s)| (format!("s{}", i + 1), s.id.clone()))
                .collect();

        // 恢复阶段连线
        let stage_edges: Vec<workflow::WorkflowEdge> = self.stage_edges.into_iter().map(|se| {
            workflow::WorkflowEdge {
                id: crate::utils::new_id(),
                source: stage_short_to_uuid.get(&se.source).cloned().unwrap_or_default(),
                target: stage_short_to_uuid.get(&se.target).cloned().unwrap_or_default(),
                label: None,
                condition: None,
            }
        }).collect();

        workflow::WorkflowDefinition {
            id: wf_id,
            name: self.name,
            version: self.version,
            description: self.description,
            trigger: self.trigger,
            stages,
            stage_edges,
input_schema: None,
            output_schema: None,
            max_depth: Some(10),
            created_at: now_ts,
            updated_at: now_ts,
            enabled: self.enabled,
        }
    }

    /// Replace node UUIDs in mapping values with short IDs (n1, n2, ...)
    /// Used during export to ensure mapping references are in short ID format
    fn remap_mapping_short_ids(
        mapping: &Option<serde_json::Value>,
        uuid_to_short: &std::collections::HashMap<String, usize>,
    ) -> Option<serde_json::Value> {
        mapping.as_ref().and_then(|m| m.as_object()).map(|obj| {
            let new_obj: serde_json::Map<String, serde_json::Value> = obj.iter()
                .map(|(k, v)| {
                    let new_v = if let Some(s) = v.as_str() {
                        let mut result = s.to_string();
                        // Replace all node UUIDs with their short ID counterparts
                        for (uuid, &idx) in uuid_to_short {
                            let short_id = format!("n{}", idx + 1);
                            result = result.replace(uuid, &short_id);
                        }
                        serde_json::Value::String(result)
                    } else {
                        v.clone()
                    };
                    (k.clone(), new_v)
                })
                .collect();
            serde_json::Value::Object(new_obj)
        })
    }

    /// Replace short IDs (n1, n2) in mapping values with new UUIDs
    /// Used during import to restore mapping references to actual node UUIDs
    fn remap_mapping_value_uuids(
        value: &str,
        short_to_uuid: &std::collections::HashMap<String, String>,
    ) -> Option<String> {
        let mut result = value.to_string();
        for (short_id, new_uuid) in short_to_uuid {
            result = result.replace(short_id, new_uuid);
        }
        Some(result)
    }
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
    let export_def: ExportWorkflowDefinition = def.into();
    serde_json::to_string_pretty(&export_def).map_err(|e| format!("序列化失败: {}", e))
}

/// 从 JSON 导入工作流
#[tauri::command]
pub fn import_workflow(
    state: tauri::State<'_, crate::DbState>,
    json_data: String,
) -> Result<workflow::WorkflowDefinition, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let export_def: ExportWorkflowDefinition = serde_json::from_str(&json_data)
        .map_err(|e| format!("JSON 解析失败: {}", e))?;

    let def = export_def.into_definition();

    workflow::create_definition(&conn, &def).map_err(|e| format!("导入失败: {}", e))?;
    Ok(def)
}



/// 导出工作流到文件
#[tauri::command]
pub fn export_workflow_to_file(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    file_path: String,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    let def = workflow::get_definition(&conn, &id)
        .map_err(|e| format!("查询失败: {}", e))?
        .ok_or_else(|| "工作流不存在".to_string())?;
    let export_def: ExportWorkflowDefinition = def.into();
    let json = serde_json::to_string_pretty(&export_def)
        .map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(&file_path, json)
        .map_err(|e| format!("写入文件失败: {}", e))?;
    Ok(())
}

/// 从文件导入工作流
#[tauri::command]
pub fn import_workflow_from_file(
    state: tauri::State<'_, crate::DbState>,
    file_path: String,
) -> Result<workflow::WorkflowDefinition, String> {
    let json = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("读取文件失败: {}", e))?;
    import_workflow(state, json)
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
    // 按 ID 直接查询实例，避免全量扫描（block scope 隔离 stmt，确保 Send 安全）
    let instance = {
        let mut stmt = conn.prepare(
            "SELECT id, definition_id, definition_name, status, context, steps,
                    current_node_id, trigger, trigger_detail,
                    started_at, completed_at, estimated_remaining, error, created_at
             FROM workflow_instances WHERE id = ?1"
        ).map_err(|e| format!("查询失败: {}", e))?;
        stmt.query_row(rusqlite::params![execution_id], |row| crate::workflow::instance_from_row(row))
            .map_err(|e| format!("查询失败: {}", e))?
    };
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
// 执行计划查询
// ════════════════════════════════════════════════════════════════════════

/// 获取工作流的执行计划（拓扑排序的阶段顺序 + 可达节点集合）
/// 前端用于就绪状态标记和执行前验证，消除前后端拓扑逻辑重复
///
/// 支持两种调用方式：
/// 1. 传入 definition JSON（前端编辑时，实时计算，无需保存）
/// 2. 传入 workflow_id（从数据库读取已保存的定义）
/// 优先使用 definition 参数，workflow_id 仅在 definition 为 None 时使用
#[tauri::command]
pub async fn get_execution_plan(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: Option<String>,
    definition: Option<serde_json::Value>,
) -> Result<crate::workflow::engine::ExecutionPlan, String> {
    let def = if let Some(def_val) = definition {
        serde_json::from_value::<crate::workflow::WorkflowDefinition>(def_val)
            .map_err(|e| format!("工作流定义解析失败: {}", e))?
    } else {
        let wf_id = workflow_id.ok_or_else(|| "必须提供 workflow_id 或 definition 参数".to_string())?;
        let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
        workflow::get_definition(&conn, &wf_id)
            .map_err(|e| format!("查询失败: {}", e))?
            .ok_or_else(|| "工作流不存在".to_string())?
    };
    Ok(crate::workflow::engine::WorkflowEngine::compute_execution_plan(&def))
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
