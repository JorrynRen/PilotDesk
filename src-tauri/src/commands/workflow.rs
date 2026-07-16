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

            stage_edges: vec![],

            gate: workflow::GateConfig::default(),

            collapsed: false,

            offset_x: 0.0,

            offset_y: 0.0,

        }],



input_schema: None,

        output_schema: None,

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

        trigger: "manual".to_string(),

        trigger_detail: None,

        started_at: Some(now_ts),

        completed_at: None,

        completion_rate: 0.0,

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

        .unwrap_or(10);



    // 4. 在后台执行工作流（使用 WorkflowEngine）

    let def_clone = def.clone();

    let instance_id_clone = instance_id.clone();

    let executor = executor.inner().clone();

    let app_handle_clone = app_handle.clone();



    tokio::spawn(async move {

        log::info!("[WorkflowEngine] 开始执行工作流: id={}, name={}", instance_id_clone, def_clone.name);

        // ── 兜底校验（前端已预检，此处为安全冗余）──

        if let Some(conn) = app_handle_clone.try_state::<crate::DbState>().and_then(|s| s.get_conn().ok()) {

            match crate::workflow::engine::WorkflowEngine::validate_workflow_for_execution(

                &def_clone, &conn, &crate::workflow::ExecutionMode::default(),

            ) {

                Ok(result) if !result.ok => {

                    log::error!("[WorkflowEngine] 工作流校验未通过: {:?}", result.checks);

                    let errors: Vec<String> = result.checks.iter()

                        .filter(|c| c.severity == "error")

                        .map(|c| c.message.clone())

                        .collect();

                    if let Some(conn) = app_handle_clone.try_state::<crate::DbState>().and_then(|s| s.get_conn().ok()) {

                        let _ = conn.execute(

                            "UPDATE workflow_instances SET status = 'failed', error = ?1, completed_at = ?2, updated_at = ?2 WHERE id = ?3",

                            rusqlite::params![errors.join("; "), crate::utils::now(), instance_id_clone],

                        );

                    }

                    return;

                }

                Ok(_) => {}

                Err(e) => {

                    log::warn!("[WorkflowEngine] 兜底校验异常（不阻塞执行）: {}", e);

                }

            }

        }

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

                let _ = app_handle_clone.emit("workflow:execution-progress", serde_json::json!({

                    "execution_id": instance_id_clone,

                    "definition_id": def_clone.id,

                    "mode": "full",

                    "execution": {

                        "status": "completed",

                        "definition_name": def_clone.name,

                    },

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

                let _ = app_handle_clone.emit("workflow:execution-progress", serde_json::json!({

                    "execution_id": instance_id_clone,

                    "definition_id": def_clone.id,

                    "mode": "full",

                    "execution": {

                        "status": "failed",

                        "error": e.to_string(),

                        "definition_name": def_clone.name,

                    },

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

                let _ = app_handle_clone.emit("workflow:execution-progress", serde_json::json!({

                    "execution_id": instance_id_clone,

                    "definition_id": def_clone.id,

                    "mode": "full",

                    "execution": {

                        "status": "failed",

                        "error": format!("工作流引擎内部错误: {}", msg),

                        "definition_name": def_clone.name,

                    },

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

    // 注意：serde_json::to_string 输出 JSON 字符串（带引号），不能用于 SQL CHECK 约束

    // 必须使用纯字符串字面量以符合 CHECK(status IN (...))

    let now = crate::utils::now();

    conn.execute(

        "UPDATE workflow_instances SET status = 'cancelled', error = ?1, updated_at = ?2 WHERE id = ?3",

        rusqlite::params!["用户中止", now, execution_id],

    ).map_err(|e| format!("更新失败: {}", e))?;



    // 2. 取消执行并立即终止 Agent 子进程（不获取 AgentManager 大锁）
    //    先设置 cancelled AtomicBool 使 tokio::select! 立即响应，
    //    再通过 processes HashMap 直接 kill 子进程（绕过 AsyncMutex 锁竞争）
    let node_ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT node_id FROM node_executions WHERE execution_id = ?1"
        ).map_err(|e| format!("查询失败: {}", e))?;
        let rows = stmt.query_map(rusqlite::params![execution_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询失败: {}", e))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    executor.inner().cancel_execution_and_kill_agents(&execution_id, &node_ids);

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

        "SELECT id, definition_id, definition_name, status, context,

                trigger, trigger_detail,

                started_at, completed_at, completion_rate, error, created_at

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

    /// 关联码（子工作流有值，主工作流为 None）

    pub ref_code: Option<String>,

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

    pub collapsed: bool,

    pub offset_x: f64,

    pub offset_y: f64,

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

    /// 基础转换（不含子工作流处理，用于向后兼容的 export_workflow 命令）

    fn from(def: workflow::WorkflowDefinition) -> Self {

        let stages = Self::convert_stages(&def.stages, &Default::default());

        let stage_short_ids: std::collections::HashMap<String, String> =

            def.stages.iter().enumerate()

                .map(|(i, s)| (s.id.clone(), format!("s{}", i + 1)))

                .collect();

        let stage_edges = Self::convert_stage_edges(&def.stages, &stage_short_ids);

        Self {

            name: def.name,

            version: def.version,

            description: def.description,

            trigger: def.trigger,

            stages,

            stage_edges,

            enabled: def.enabled,

            ref_code: None,

        }

    }

}



impl ExportWorkflowDefinition {

    /// 导出为带子工作流捆绑的结构（返回主工作流 ExportDef + 所有子工作流 ExportDef）

    /// subflow_map: definitionId -> (ExportWorkflowDefinition, 子工作流原始名称)

    pub fn from_with_subflows(

        def: workflow::WorkflowDefinition,

        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,

    ) -> Result<(Self, Vec<(String, Self)>), String> {

        let mut subflow_defs: Vec<(String, Self)> = Vec::new(); // (ref_code, export_def)

        let mut def_id_to_ref: std::collections::HashMap<String, String> = std::collections::HashMap::new(); // original defId -> ref_code

        let mut ref_counter: usize = 0;



        // 递归收集子工作流

        Self::collect_subflows_recursive(&def, conn, &mut subflow_defs, &mut def_id_to_ref, &mut ref_counter)?;



        // 转换主工作流（替换 Subflow 节点的 params）

        let stage_short_ids: std::collections::HashMap<String, String> =

            def.stages.iter().enumerate()

                .map(|(i, s)| (s.id.clone(), format!("s{}", i + 1)))

                .collect();

        let stages = Self::convert_stages_with_subflow_remap(&def.stages, &stage_short_ids, &def_id_to_ref);

        let stage_edges = Self::convert_stage_edges(&def.stages, &stage_short_ids);



        let main_export = Self {

            name: def.name.clone(),

            version: def.version,

            description: def.description,

            trigger: def.trigger,

            stages,

            stage_edges,

            enabled: def.enabled,

            ref_code: None,

        };



        Ok((main_export, subflow_defs))

    }



    /// 递归收集子工作流定义并生成关联码

    fn collect_subflows_recursive(

        def: &workflow::WorkflowDefinition,

        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,

        subflow_defs: &mut Vec<(String, Self)>,

        def_id_to_ref: &mut std::collections::HashMap<String, String>,

        ref_counter: &mut usize,

    ) -> Result<(), String> {

        for stage in &def.stages {

            for node in &stage.nodes {

                if node.node_type != workflow::WorkflowNodeType::Subflow { continue; }

                let subflow_def_id = node.params.as_ref()

                    .and_then(|p| p.get("definitionId"))

                    .and_then(|v| v.as_str())

                    .unwrap_or("");



                if subflow_def_id.is_empty() { continue; }



                // 如果已处理过此子工作流，跳过（同一子工作流被多个节点引用）

                if def_id_to_ref.contains_key(subflow_def_id) { continue; }



                // 加载子工作流定义

                let sub_def = workflow::get_definition(conn, subflow_def_id)

                    .map_err(|e| format!("加载子工作流失败: {}", e))?

                    .ok_or_else(|| format!("子工作流定义不存在: {}", subflow_def_id))?;



                *ref_counter += 1;

                let ref_code = format!("ref_{}", ref_counter);

                def_id_to_ref.insert(subflow_def_id.to_string(), ref_code.clone());



                // 递归收集更深层子工作流

                Self::collect_subflows_recursive(&sub_def, conn, subflow_defs, def_id_to_ref, ref_counter)?;



                // 转换子工作流本身（替换其 Subflow 节点的 params）

                let sub_stage_short_ids: std::collections::HashMap<String, String> =

                    sub_def.stages.iter().enumerate()

                        .map(|(i, s)| (s.id.clone(), format!("s{}", i + 1)))

                        .collect();

                let sub_stages = Self::convert_stages_with_subflow_remap(&sub_def.stages, &sub_stage_short_ids, def_id_to_ref);

                let sub_stage_edges = Self::convert_stage_edges(&sub_def.stages, &sub_stage_short_ids);



                let sub_export = Self {

                    name: sub_def.name,

                    version: sub_def.version,

                    description: sub_def.description,

                    trigger: sub_def.trigger,

                    stages: sub_stages,

                    stage_edges: sub_stage_edges,

                    enabled: sub_def.enabled,

                    ref_code: Some(ref_code.clone()),

                };

                subflow_defs.push((ref_code, sub_export));

            }

        }

        Ok(())

    }



    /// 转换阶段列表（基础版，不处理子工作流替换）

    /// 预构建全局节点 UUID → 短标识映射（跨阶段连续编号 n1, n2, n3...）

    fn convert_stages(

        stages: &[workflow::Stage],

        stage_uuid_to_short: &std::collections::HashMap<String, String>,

    ) -> Vec<ExportStage> {

        // 预构建全局节点 UUID → 短标识映射（跨阶段连续编号）

        let mut global_uuid_to_short: std::collections::HashMap<String, String> = stage_uuid_to_short.clone();

        let mut node_counter: usize = 0;

        for stage in stages {

            for node in &stage.nodes {

                node_counter += 1;

                global_uuid_to_short.insert(node.id.clone(), format!("n{}", node_counter));

            }

        }



        stages.iter().map(|stage| {

            // 阶段内节点编号仍按数组顺序（n1, n2...），用于边的 source/target

            let stage_node_ids: std::collections::HashMap<String, usize> =

                stage.nodes.iter().enumerate()

                    .map(|(i, node)| (node.id.clone(), i))

                    .collect();

            let nodes: Vec<ExportNode> = stage.nodes.iter().map(|node| {

                ExportNode {

                    node_type: node.node_type.clone(),

                    label: node.label.clone(),

                    params: node.params.clone(),

                    delay_ms: node.delay_ms,

                    timeout_ms: node.timeout_ms,

                    retry_count: node.retry_count,

                    retry_delay_ms: node.retry_delay_ms,

                    input_mapping: Self::remap_mapping_short_ids(&node.input_mapping, &global_uuid_to_short),

                    output_mapping: Self::remap_mapping_short_ids(&node.output_mapping, &global_uuid_to_short),

                    position: node.position.clone(),

                }

            }).collect();

            let edges: Vec<ExportEdge> = stage.edges.iter().map(|edge| {

                let src_idx = stage_node_ids.get(&edge.source).copied().unwrap_or(0);

                let tgt_idx = stage_node_ids.get(&edge.target).copied().unwrap_or(0);

                ExportEdge {

                    source: format!("n{}", src_idx + 1),

                    target: format!("n{}", tgt_idx + 1),

                    label: edge.label.clone(),

                    condition: edge.condition.clone(),

                }

            }).collect();

            ExportStage {

                name: stage.name.clone(),

                order: stage.order,

                nodes,

                edges,

                gate: stage.gate.clone(),

                collapsed: stage.collapsed,

                offset_x: stage.offset_x,

                offset_y: stage.offset_y,

            }

        }).collect()

    }



    /// 转换阶段列表（含子工作流节点 params 替换）

    /// 预构建全局节点 UUID → 短标识映射（跨阶段连续编号 n1, n2, n3...）

    fn convert_stages_with_subflow_remap(

        stages: &[workflow::Stage],

        stage_uuid_to_short: &std::collections::HashMap<String, String>,

        def_id_to_ref: &std::collections::HashMap<String, String>,

    ) -> Vec<ExportStage> {

        // 预构建全局节点 UUID → 短标识映射（跨阶段连续编号）

        let mut global_uuid_to_short: std::collections::HashMap<String, String> = stage_uuid_to_short.clone();

        let mut node_counter: usize = 0;

        for stage in stages {

            for node in &stage.nodes {

                node_counter += 1;

                global_uuid_to_short.insert(node.id.clone(), format!("n{}", node_counter));

            }

        }



        stages.iter().map(|stage| {

            // 阶段内节点编号仍按数组顺序，用于边的 source/target

            let stage_node_ids: std::collections::HashMap<String, usize> =

                stage.nodes.iter().enumerate()

                    .map(|(i, node)| (node.id.clone(), i))

                    .collect();

            let nodes: Vec<ExportNode> = stage.nodes.iter().map(|node| {

                // Subflow 节点：替换 params 中的 definitionId 为 refCode

                let params = if node.node_type == workflow::WorkflowNodeType::Subflow {

                    Self::remap_subflow_params(&node.params, def_id_to_ref)

                } else {

                    node.params.clone()

                };

                ExportNode {

                    node_type: node.node_type.clone(),

                    label: node.label.clone(),

                    params,

                    delay_ms: node.delay_ms,

                    timeout_ms: node.timeout_ms,

                    retry_count: node.retry_count,

                    retry_delay_ms: node.retry_delay_ms,

                    input_mapping: Self::remap_mapping_short_ids(&node.input_mapping, &global_uuid_to_short),

                    output_mapping: Self::remap_mapping_short_ids(&node.output_mapping, &global_uuid_to_short),

                    position: node.position.clone(),

                }

            }).collect();

            let edges: Vec<ExportEdge> = stage.edges.iter().map(|edge| {

                let src_idx = stage_node_ids.get(&edge.source).copied().unwrap_or(0);

                let tgt_idx = stage_node_ids.get(&edge.target).copied().unwrap_or(0);

                ExportEdge {

                    source: format!("n{}", src_idx + 1),

                    target: format!("n{}", tgt_idx + 1),

                    label: edge.label.clone(),

                    condition: edge.condition.clone(),

                }

            }).collect();

            ExportStage {

                name: stage.name.clone(),

                order: stage.order,

                nodes,

                edges,

                gate: stage.gate.clone(),

                collapsed: stage.collapsed,

                offset_x: stage.offset_x,

                offset_y: stage.offset_y,

            }

        }).collect()

    }



    /// 转换阶段连线为导出格式

    fn convert_stage_edges(

        stages: &[workflow::Stage],

        stage_short_ids: &std::collections::HashMap<String, String>,

    ) -> Vec<ExportEdge> {

        stages.iter().flat_map(|stage| {

            stage.stage_edges.iter().filter_map(|se| {

                let src = stage_short_ids.get(&se.source).cloned();

                let tgt = stage_short_ids.get(&se.target).cloned();

                match (src, tgt) {

                    (Some(source), Some(target)) => Some(ExportEdge {

                        source, target, label: None, condition: None,

                    }),

                    _ => None,

                }

            })

        }).collect()

    }



    /// 替换 Subflow 节点的 params：definitionId -> refCode + subflowFileName

    fn remap_subflow_params(

        params: &Option<serde_json::Value>,

        def_id_to_ref: &std::collections::HashMap<String, String>,

    ) -> Option<serde_json::Value> {

        params.as_ref().and_then(|p| p.as_object()).map(|obj| {

            let mut new_obj = serde_json::Map::new();

            // 保留非 definitionId 的字段

            for (k, v) in obj {

                if k != "definitionId" {

                    new_obj.insert(k.clone(), v.clone());

                }

            }

            // 获取 definitionId 并替换为 refCode + subflowFileName

            if let Some(def_id) = obj.get("definitionId").and_then(|v| v.as_str()) {

                if let Some(ref_code) = def_id_to_ref.get(def_id) {

                    new_obj.insert("refCode".to_string(), serde_json::json!(ref_code));

                    // 文件名后续在导出文件写入时根据工作流名称 + refCode 生成



                }

            }

            serde_json::Value::Object(new_obj)

        })

    }



    /// 导入时重建完整 WorkflowDefinition（生成新 UUID）

    /// ref_code_to_uuid: 关联码 -> 已写入数据库的子工作流新 UUID（用于替换 Subflow 节点的 refCode）

    pub fn into_definition_with_subflows(

        self,

        ref_code_to_uuid: &std::collections::HashMap<String, String>,

    ) -> workflow::WorkflowDefinition {

        let now_ts = crate::utils::now();

        let wf_id = crate::utils::new_id();



        let mut stages: Vec<workflow::Stage> = self.stages.into_iter().map(|stage| {

            let nodes: Vec<workflow::WorkflowNode> = stage.nodes.into_iter().map(|en| {

                // Subflow 节点：将 refCode 替换回 definitionId

                let params = if en.node_type == workflow::WorkflowNodeType::Subflow {

                    Self::restore_subflow_params(&en.params, ref_code_to_uuid)

                } else {

                    en.params

                };

                workflow::WorkflowNode {

                    id: String::new(),

                    node_type: en.node_type,

                    label: en.label,

                    plugin_id: None,

                    command_id: None,

                    params,

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



            // 节点 mapping 替换在下方二次遍历中统一处理（需要全局映射）



            workflow::Stage {

                id: crate::utils::new_id(),

                name: stage.name,

                order: stage.order,

                nodes,

                edges,

                stage_edges: vec![],

                gate: stage.gate,

                collapsed: stage.collapsed,

                offset_x: stage.offset_x,

                offset_y: stage.offset_y,

            }

        }).collect();



        // 构建阶段短标识符 -> 新UUID 映射

        let stage_short_to_uuid: std::collections::HashMap<String, String> =

            stages.iter().enumerate()

                .map(|(i, s)| (format!("s{}", i + 1), s.id.clone()))

                .collect();



        // 二次遍历：构建全局节点短标识 -> 新 UUID 映射，替换所有 mapping

        // 全局短标识是跨阶段连续编号（n1, n2, n3...）

        let mut global_short_to_uuid: std::collections::HashMap<String, String> = stage_short_to_uuid.clone();

        let mut n_counter: usize = 0;

        for stage in &stages {

            for node in &stage.nodes {

                n_counter += 1;

                global_short_to_uuid.insert(format!("n{}", n_counter), node.id.clone());

            }

        }

        for stage in &mut stages {

            for node in &mut stage.nodes {

                if let Some(ref mapping) = node.input_mapping {

                    if let Some(val) = mapping.as_object() {

                        let new_mapping: serde_json::Map<String, serde_json::Value> = val.iter()

                            .map(|(k, v)| {

                                let new_v = if let Some(s) = v.as_str() {

                                    Self::remap_mapping_value_uuids(s, &global_short_to_uuid, &std::collections::HashMap::new())

                                        .map(serde_json::Value::String)

                                        .unwrap_or_else(|| v.clone())

                                } else {

                                    v.clone()

                                };

                                (k.clone(), new_v)

                            })

                            .collect();

                        node.input_mapping = Some(serde_json::Value::Object(new_mapping));

                    }

                }

                if let Some(ref mapping) = node.output_mapping {

                    if let Some(val) = mapping.as_object() {

                        let new_mapping: serde_json::Map<String, serde_json::Value> = val.iter()

                            .map(|(k, v)| {

                                let new_v = if let Some(s) = v.as_str() {

                                    Self::remap_mapping_value_uuids(s, &global_short_to_uuid, &std::collections::HashMap::new())

                                        .map(serde_json::Value::String)

                                        .unwrap_or_else(|| v.clone())

                                } else {

                                    v.clone()

                                };

                                (k.clone(), new_v)

                            })

                            .collect();

                        node.output_mapping = Some(serde_json::Value::Object(new_mapping));

                    }

                }

            }

        }



        // 恢复阶段连线

        let all_stage_edges: Vec<workflow::WorkflowEdge> = self.stage_edges.into_iter().map(|se| {

            workflow::WorkflowEdge {

                id: crate::utils::new_id(),

                source: stage_short_to_uuid.get(&se.source).cloned().unwrap_or_default(),

                target: stage_short_to_uuid.get(&se.target).cloned().unwrap_or_default(),

                label: None,

                condition: None,

            }

        }).collect();

        for stage in &mut stages {

            stage.stage_edges = all_stage_edges.iter()

                .filter(|e| e.source == stage.id)

                .cloned()

                .collect();

        }



        workflow::WorkflowDefinition {

            id: wf_id,

            name: self.name,

            version: self.version,

            description: self.description,

            trigger: self.trigger,

            stages,

            input_schema: None,

            output_schema: None,

            created_at: now_ts,

            updated_at: now_ts,

            enabled: self.enabled,

        }

    }



    /// 向后兼容：不带子工作流映射的导入

    pub fn into_definition(self) -> workflow::WorkflowDefinition {

        self.into_definition_with_subflows(&std::collections::HashMap::new())

    }



    /// 替换 mapping 中的节点 UUID 为短 ID（n1/n2）和阶段 UUID 为短 ID（s1/s2）

    fn remap_mapping_short_ids(

        mapping: &Option<serde_json::Value>,

        global_uuid_to_short: &std::collections::HashMap<String, String>,

    ) -> Option<serde_json::Value> {

        mapping.as_ref().and_then(|m| m.as_object()).map(|obj| {

            let new_obj: serde_json::Map<String, serde_json::Value> = obj.iter()

                .map(|(k, v)| {

                    let new_v = if let Some(s) = v.as_str() {

                        let mut result = s.to_string();

                        // 替换所有 UUID（节点和阶段）为全局短标识

                        for (uuid, short) in global_uuid_to_short {

                            result = result.replace(uuid, short);

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



    /// 替换 mapping 中的短 ID（n1/n2）为新 UUID（节点维度）

    fn remap_mapping_value_uuids(

        value: &str,

        short_to_node_uuid: &std::collections::HashMap<String, String>,

        _short_to_stage_uuid: &std::collections::HashMap<String, String>,

    ) -> Option<String> {

        let mut result = value.to_string();

        for (short_id, new_uuid) in short_to_node_uuid {

            result = result.replace(short_id, new_uuid);

        }

        Some(result)

    }







    /// 恢复 Subflow 节点的 params：refCode -> definitionId

    fn restore_subflow_params(

        params: &Option<serde_json::Value>,

        ref_code_to_uuid: &std::collections::HashMap<String, String>,

    ) -> Option<serde_json::Value> {

        params.as_ref().and_then(|p| p.as_object()).map(|obj| {

            let mut new_obj = serde_json::Map::new();

            for (k, v) in obj {

                if k != "refCode" {

                    new_obj.insert(k.clone(), v.clone());

                }

            }

            if let Some(ref_code) = obj.get("refCode").and_then(|v| v.as_str()) {

                if let Some(new_uuid) = ref_code_to_uuid.get(ref_code) {

                    new_obj.insert("definitionId".to_string(), serde_json::json!(new_uuid));

                }

            }

            serde_json::Value::Object(new_obj)

        })

    }

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

    dir_path: String,

) -> Result<(), String> {

    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;

    let def = workflow::get_definition(&conn, &id)

        .map_err(|e| format!("查询失败: {}", e))?

        .ok_or_else(|| "工作流不存在".to_string())?;



    // 判断是否包含子工作流

    let has_subflows = def.stages.iter().any(|stage| {

        stage.nodes.iter().any(|n| n.node_type == workflow::WorkflowNodeType::Subflow)

    });



    if !has_subflows {

        // 简单导出：无子工作流，使用基础 From impl

        let export_def: ExportWorkflowDefinition = def.into();

        // 自动创建以工作流名称命名的子文件夹

        let workflow_dir = std::path::Path::new(&dir_path).join(&export_def.name);

        std::fs::create_dir_all(&workflow_dir)

            .map_err(|e| format!("创建文件夹失败: {}", e))?;



        let file_name = format!("[主]{}.json", export_def.name);

        let file_path = workflow_dir.join(&file_name);

        let json = serde_json::to_string_pretty(&export_def)

            .map_err(|e| format!("序列化失败: {}", e))?;

        std::fs::write(&file_path, json)

            .map_err(|e| format!("写入文件失败: {}", e))?;

    } else {

        // 含子工作流导出：使用 from_with_subflows

        let (main_export, subflow_exports) = ExportWorkflowDefinition::from_with_subflows(def, &conn)?;



        // 自动创建以工作流名称命名的子文件夹

        let workflow_dir = std::path::Path::new(&dir_path).join(&main_export.name);

        std::fs::create_dir_all(&workflow_dir)

            .map_err(|e| format!("创建文件夹失败: {}", e))?;



        // 写入主文件

        let main_file_name = format!("[主]{}.json", main_export.name);

        let main_file_path = workflow_dir.join(&main_file_name);

        let main_json = serde_json::to_string_pretty(&main_export)

            .map_err(|e| format!("序列化失败: {}", e))?;

        std::fs::write(&main_file_path, main_json)

            .map_err(|e| format!("写入主文件失败: {}", e))?;



        // 写入各子工作流文件

        for (ref_code, sub_def) in &subflow_exports {

            let file_name = format!("[子]{}({}).json", sub_def.name, ref_code);

            let sub_file_path = workflow_dir.join(&file_name);



            let sub_json = serde_json::to_string_pretty(&sub_def)

                .map_err(|e| format!("序列化失败: {}", e))?;

            std::fs::write(&sub_file_path, sub_json)

                .map_err(|e| format!("写入子工作流文件失败: {}", e))?;

        }

    }



    Ok(())

}



/// 从文件导入工作流（支持子工作流捆绑导入）

#[tauri::command]

pub fn import_workflow_from_file(

    state: tauri::State<'_, crate::DbState>,

    file_path: String,

) -> Result<workflow::WorkflowDefinition, String> {

    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;



    // 读取主文件

    let main_json = std::fs::read_to_string(&file_path)

        .map_err(|e| format!("读取文件失败: {}", e))?;

    let main_export: ExportWorkflowDefinition = serde_json::from_str(&main_json)

        .map_err(|e| format!("JSON 解析失败: {}", e))?;



    // 检查主文件是否包含子工作流引用

    let has_subflow_refs = main_export.stages.iter().any(|stage| {

        stage.nodes.iter().any(|n| {

            n.node_type == workflow::WorkflowNodeType::Subflow

                && n.params.as_ref()

                    .and_then(|p| p.get("refCode"))

                    .is_some()

        })

    });



    if !has_subflow_refs {

        // 无子工作流：直接导入

        let def = main_export.into_definition();

        workflow::create_definition(&conn, &def).map_err(|e| format!("导入失败: {}", e))?;

        Ok(def)

    } else {

        // 收集同目录下的 [子] 文件

        let dir = std::path::Path::new(&file_path)

            .parent()

            .ok_or_else(|| "无法获取文件所在目录".to_string())?;

        let mut subflow_files: std::collections::HashMap<String, String> = std::collections::HashMap::new(); // refCode -> 文件路径

        if let Ok(entries) = std::fs::read_dir(dir) {

            for entry in entries.flatten() {

                let file_name = entry.file_name().to_string_lossy().to_string();

                // 文件名格式：[子]{name}(ref_N).json

                if file_name.starts_with("[子]") && file_name.ends_with(".json") {

                    // 提取关联码

                    if let Some(rest) = file_name.strip_prefix("[子]") {

                        if let Some(rest) = rest.strip_suffix(".json") {

                            // rest = "{name}(ref_N)"

                            if let Some(start) = rest.rfind("(ref_") {

                                if rest.ends_with(")") {

                                    let ref_code = &rest[start+1..rest.len()-1]; // "ref_N"

                                    subflow_files.insert(ref_code.to_string(), entry.path().to_string_lossy().to_string());

                                }

                            }

                        }

                    }

                }

            }

        }



        // 读取所有子工作流文件

        let mut subflow_defs: std::collections::HashMap<String, ExportWorkflowDefinition> = std::collections::HashMap::new();

        for (ref_code, path) in &subflow_files {

            let json = std::fs::read_to_string(path)

                .map_err(|e| format!("读取子工作流文件失败: {}", e))?;

            let def: ExportWorkflowDefinition = serde_json::from_str(&json)

                .map_err(|e| format!("解析子工作流文件失败: {}", e))?;

            subflow_defs.insert(ref_code.clone(), def);

        }



        // 按拓扑序导入：递归获取依赖深度，先导入深层再浅层

        fn get_depth(

            ref_code: &str,

            defs: &std::collections::HashMap<String, ExportWorkflowDefinition>,

            cache: &mut std::collections::HashMap<String, usize>,

        ) -> usize {

            if let Some(&d) = cache.get(ref_code) { return d; }

            let def = match defs.get(ref_code) {

                Some(d) => d,

                None => { cache.insert(ref_code.to_string(), 0); return 0; }

            };

            let max_child = def.stages.iter().flat_map(|s| s.nodes.iter())

                .filter(|n| n.node_type == workflow::WorkflowNodeType::Subflow)

                .filter_map(|n| n.params.as_ref()?.get("refCode")?.as_str().map(String::from))

                .map(|rc| get_depth(&rc, defs, cache))

                .max()

                .unwrap_or(0);

            let depth = max_child + 1;

            cache.insert(ref_code.to_string(), depth);

            depth

        }



        let mut depth_cache: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        let mut sorted_refs: Vec<String> = subflow_files.keys().cloned().collect();

        sorted_refs.sort_by(|a, b| {

            let da = get_depth(a, &subflow_defs, &mut depth_cache);

            let db = get_depth(b, &subflow_defs, &mut depth_cache);

            db.cmp(&da) // 深度大的先导入

        });



        // 逐层写入子工作流，建立 refCode -> 新 UUID 映射

        let mut ref_code_to_uuid: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        for ref_code in &sorted_refs {

            let sub_export = subflow_defs.get(ref_code)

                .ok_or_else(|| format!("子工作流 {} 未找到", ref_code))?;

            let def = sub_export.clone().into_definition_with_subflows(&ref_code_to_uuid);

            let new_id = def.id.clone();

            workflow::create_definition(&conn, &def)

                .map_err(|e| format!("导入子工作流 {} 失败: {}", ref_code, e))?;

            ref_code_to_uuid.insert(ref_code.clone(), new_id);

        }



        // 导入主工作流

        let main_def = main_export.into_definition_with_subflows(&ref_code_to_uuid);

        workflow::create_definition(&conn, &main_def)

            .map_err(|e| format!("导入主工作流失败: {}", e))?;

        Ok(main_def)

    }

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

    Ok(value.parse::<usize>().unwrap_or(10))

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

/// 删除指定版本快照
#[tauri::command]
pub fn delete_workflow_version(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
    version: i64,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;
    crate::workflow::delete_workflow_version(&conn, &workflow_id, version)
        .map_err(|e| format!("删除版本失败: {}", e))
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

            "SELECT id, definition_id, definition_name, status, context,

                    trigger, trigger_detail,

                    started_at, completed_at, completion_rate, error, created_at

             FROM workflow_instances WHERE id = ?1"

        ).map_err(|e| format!("查询失败: {}", e))?;

        stmt.query_row(rusqlite::params![execution_id], |row| crate::workflow::instance_from_row(row))

            .map_err(|e| format!("查询失败: {}", e))?

    };

    let def = crate::workflow::get_definition(&conn, &instance.definition_id)

        .map_err(|e| format!("查询失败: {}", e))?

        .ok_or_else(|| "工作流定义不存在".to_string())?;

    let executor = app_handle.state::<std::sync::Arc<crate::workflow::executor::NodeExecutor>>();



    // 从 app_settings 读取最大并发数（与 start_workflow 保持一致）

    let max_concurrency: usize = conn

        .query_row(

            "SELECT value FROM app_settings WHERE key = 'workflow_max_concurrency'",

            [],

            |row| row.get::<_, String>(0),

        )

        .ok()

        .and_then(|v| v.parse().ok())

        .unwrap_or(10);



    crate::workflow::engine::WorkflowEngine::recover_execution(

        &executor.inner(),

        &def,

        &execution_id,

        serde_json::Value::Object(serde_json::Map::new()),

        &app_handle,

        max_concurrency,

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



/// 工作流执行前校验

///

/// 前端调用此命令获取校验结果，根据 ok 字段决定是否继续执行。

/// 后端 start_workflow 内部也会执行兜底校验（安全冗余）。

#[tauri::command]

pub fn validate_workflow(

    state: tauri::State<'_, crate::DbState>,

    workflow_id: Option<String>,

    definition: Option<serde_json::Value>,

) -> Result<crate::workflow::ValidationResult, String> {

    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;

    let def = if let Some(def_val) = definition {

        serde_json::from_value::<crate::workflow::WorkflowDefinition>(def_val)

            .map_err(|e| format!("工作流定义解析失败: {}", e))?

    } else {

        let wf_id = workflow_id.ok_or_else(|| "必须提供 workflow_id 或 definition 参数".to_string())?;

        workflow::get_definition(&conn, &wf_id)

            .map_err(|e| format!("查询失败: {}", e))?

            .ok_or_else(|| "工作流不存在".to_string())?

    };

    let mode = crate::workflow::ExecutionMode::default();

    crate::workflow::engine::WorkflowEngine::validate_workflow_for_execution(&def, &conn, &mode)

        .map_err(|e| format!("校验失败: {}", e))

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





/// 检查子工作流是否会形成闭环（供前端下拉过滤使用）

#[tauri::command]

pub fn check_subflow_cycle(

    state: tauri::State<'_, crate::DbState>,

    parent_id: String,

    candidate_id: String,

) -> Result<bool, String> {

    use std::collections::HashSet;



    let conn = state.get_conn().map_err(|e| format!("数据库连接失败: {}", e))?;



    fn check_cycle(

        conn: &rusqlite::Connection,

        current_id: &str,

        target_id: &str,

        visited: &mut HashSet<String>,

    ) -> Result<bool, String> {

        if current_id == target_id {

            return Ok(true);

        }

        if !visited.insert(current_id.to_string()) {

            return Ok(false);

        }



        // 加载当前工作流定义

        let def = crate::workflow::get_definition(conn, current_id)

            .map_err(|e| format!("查询失败: {}", e))?

            .ok_or_else(|| format!("工作流不存在: {}", current_id))?;



        // 遍历所有 Subflow 节点

        for stage in &def.stages {

            for node in &stage.nodes {

                if node.node_type != crate::workflow::WorkflowNodeType::Subflow {

                    continue;

                }

                let subflow_id = node.params

                    .as_ref()

                    .and_then(|p| p.get("definitionId"))

                    .and_then(|v| v.as_str())

                    .map(|s| s.to_string());

                if let Some(sid) = subflow_id {

                    if check_cycle(conn, &sid, target_id, visited)? {

                        return Ok(true);

                    }

                }

            }

        }



        Ok(false)

    }



    let mut visited = HashSet::new();

    check_cycle(&conn, &candidate_id, &parent_id, &mut visited)

}

