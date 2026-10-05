use super::super::workflow;
use crate::utils::errors::AppError;
use crate::utils::{new_id, now};
use crate::workflow::executor::NodeExecutor;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tauri::{Emitter, Manager};

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
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
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
        icon: None,
        created_at: ts,
        updated_at: ts,
        enabled: true,
    };
    workflow::create_definition(&conn, &def)
        .map_err(|e| AppError::Db(format!("创建失败: {}", e)))?;
    Ok(def)
}

/// 获取工作流列表

#[tauri::command]
pub fn list_workflows(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<workflow::WorkflowDefinition>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    workflow::list_definitions(&conn).map_err(|e| AppError::Db(format!("查询失败: {}", e)).into())
}

/// 获取工作流详情

#[tauri::command]
pub fn get_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<Option<workflow::WorkflowDefinition>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    workflow::get_definition(&conn, &id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)).into())
}

/// 更新工作流

#[tauri::command]
pub fn update_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    name: Option<String>,
    description: Option<String>,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let mut def = workflow::get_definition(&conn, &id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?;
    if let Some(n) = name {
        def.name = n;
    }

    if let Some(d) = description {
        def.description = d;
    }

    workflow::update_definition(&conn, &def)
        .map_err(|e| AppError::Db(format!("更新失败: {}", e)).into())
}

/// 删定义前的守卫：仍有未结束实例时拒绝删除（不删任何数据）。
///
/// 删定义会级联清理其 Agent 节点会话与用量，正在跑的执行会因此失去会话行
/// （后续 `resume_session_ref` 延续会话时报「要延续的会话不存在」、产物也失去追溯入口），
/// 故要求先把执行停掉再删。
fn ensure_no_unfinished_executions(
    conn: &rusqlite::Connection,
    definition_id: &str,
) -> Result<(), AppError> {
    let unfinished = workflow::events::derive_instances(conn)
        .map_err(|e| AppError::Db(format!("读取执行记录失败: {}", e)))?
        .into_iter()
        .filter(|i| i.definition_id == definition_id && !is_terminal_instance(&i.status))
        .count();
    if unfinished > 0 {
        return Err(AppError::InvalidInput(format!(
            "该工作流仍有 {} 个未结束的执行，请先停止后再删除",
            unfinished
        )));
    }
    Ok(())
}

/// 删除工作流（连带清理其节点会话与用量、以及该工作流**已结束**的执行记录）
///
/// 未结束的实例刻意保留：删掉它们的事件会让实例从列表消失却仍在跑，且再也取消不掉
/// （见 [`delete_executions_inner`]）。因此删定义前先由 [`ensure_no_unfinished_executions`]
/// 把关：仍有未结束执行时直接拒绝，避免正在跑的执行失去会话。
/// 返回被清理的执行记录数。
#[tauri::command]
pub fn delete_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<DeleteExecutionsResult, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    ensure_no_unfinished_executions(&conn, &id)?;
    workflow::delete_definition(&conn, &id)
        .map_err(|e| AppError::Db(format!("删除失败: {}", e)))?;
    let ids: Vec<String> = workflow::events::derive_instances(&conn)
        .map_err(|e| AppError::Db(format!("读取执行记录失败: {}", e)))?
        .into_iter()
        .filter(|i| i.definition_id == id)
        .map(|i| i.id)
        .collect();
    delete_executions_inner(&conn, &ids).map_err(String::from)
}

/// 保存完整工作流定义（全量对象，匹配前端 store 调用）

#[tauri::command]
pub fn save_workflow_definition(
    state: tauri::State<'_, crate::DbState>,
    definition: crate::workflow::WorkflowDefinition,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;

    // 检查是否存在

    let existing = crate::workflow::get_definition(&conn, &definition.id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?;
    if existing.is_some() {
        crate::workflow::update_definition(&conn, &definition)
            .map_err(|e| AppError::Db(format!("更新失败: {}", e)).into())
    } else {
        crate::workflow::create_definition(&conn, &definition)
            .map_err(|e| AppError::Db(format!("创建失败: {}", e)).into())
    }
}

/// 保存工作流（阶段结构）

#[tauri::command]
pub fn save_workflow_dag(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    stages: Vec<workflow::Stage>,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let mut def = workflow::get_definition(&conn, &id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?;
    def.stages = stages;
    workflow::update_definition(&conn, &def)
        .map_err(|e| AppError::Db(format!("保存失败: {}", e)).into())
}

// ════════════════════════════════════════════════════════════

// 执行控制命令

// ════════════════════════════════════════════════════════════

/// 启动工作流执行
///
/// **刻意没有 `version` 参数**：历史版本走「恢复版本」再启动（`restore_workflow_version`），
/// 而不是"按指定版本运行一次"。后者会让实例长期与当前定义不一致（编辑、触发器、
/// 后续节点解析都以当前定义为准），语义上是个陷阱；而且前端从来没有传过它。
#[tauri::command]
pub async fn start_workflow(
    state: tauri::State<'_, crate::DbState>,
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    app_handle: tauri::AppHandle,
    workflow_id: String,
    input_data: Option<Value>,
    // 前端预生成的实例 ID（解决快速工作流的竞态条件）
    instance_id: Option<String>,
) -> Result<workflow::WorkflowInstance, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;

    // 1. 获取工作流定义

    let def = workflow::get_definition(&conn, &workflow_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?;

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
        skipped_count: 0,
        output: None,
        output_source: None,
        output_node_label: None,
        error: None,
        created_at: now_ts,
    };
    workflow::create_instance(&conn, &instance)
        .map_err(|e| AppError::Db(format!("创建实例失败: {}", e)))?;

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
        log::info!(
            "[WorkflowEngine] 开始执行工作流: id={}, name={}",
            instance_id_clone,
            def_clone.name
        );

        // ── 兜底校验（前端已预检，此处为安全冗余）──

        if let Some(conn) = app_handle_clone
            .try_state::<crate::DbState>()
            .and_then(|s| s.get_conn().ok())
        {
            match crate::workflow::engine::WorkflowEngine::validate_workflow_for_execution(
                &def_clone,
                &conn,
                &crate::workflow::ExecutionMode::default(),
            ) {
                Ok(result) if !result.ok => {
                    log::error!("[WorkflowEngine] 工作流校验未通过: {:?}", result.checks);
                    let errors: Vec<String> = result
                        .checks
                        .iter()
                        .filter(|c| c.severity == "error")
                        .map(|c| c.message.clone())
                        .collect();
                    if let Some(conn) = app_handle_clone
                        .try_state::<crate::DbState>()
                        .and_then(|s| s.get_conn().ok())
                    {
                        let err_preview = errors.join("; ");
                        let now = crate::utils::now();

                        // 事件化：实例进入 failed（校验失败路径）。
                        let event = serde_json::json!({
                            "executionId": instance_id_clone,
                            "status": "failed",
                            "errorMessage": err_preview.chars().take(2000).collect::<String>(),
                            "completionRate": 0.0,
                            "completedAt": now,
                            "timestamp": now,
                        });
                        let _ = crate::eventlog::append_workflow_event(
                            &conn,
                            &instance_id_clone,
                            "execution/status",
                            &event,
                            true,
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
            // 统一入口：起始帧 + 执行 + 终态派发 workflow.completed / workflow.failed（事件触发用）
            crate::workflow::triggers::run_definition_top_level(
                &inner_executor,
                &inner_def,
                &inner_id,
                inner_input,
                &inner_emitter,
                inner_concurrency,
                crate::workflow::triggers::TRIGGER_MANUAL,
            )
            .await
        });

        // 外层 await JoinHandle：捕获内层 panic

        match inner_handle.await {
            Ok(Ok(_output)) => {
                log::info!("[WorkflowEngine] 工作流执行成功: id={}", instance_id_clone);
                let _ = app_handle_clone.emit(
                    "workflow:execution-progress",
                    serde_json::json!({
                        "execution_id": instance_id_clone,
                        "definition_id": def_clone.id,
                        "mode": "full",
                        "execution": {
                            "status": "completed",
                            "definition_name": def_clone.name,
                        },
                    }),
                );
            }

            Ok(Err(e)) => {
                let err_str = e.to_string();
                let is_cancelled = err_str.contains("取消") || err_str.contains("cancel");
                log::error!(
                    "[WorkflowEngine] 工作流执行{}: id={}, error={}",
                    if is_cancelled { "已取消" } else { "失败" },
                    instance_id_clone,
                    err_str
                );

                // 兜底事件化：引擎内部 final_status 已写终态事件，此处仅当引擎异常退出时补 cancelled/failed 事件。
                if let Some(conn) = app_handle_clone
                    .try_state::<crate::DbState>()
                    .and_then(|s| s.get_conn().ok())
                {
                    let now = crate::utils::now();
                    if is_cancelled {
                        let event = serde_json::json!({
                            "executionId": &instance_id_clone,
                            "status": "cancelled",
                            "errorMessage": "用户中止",
                            "completionRate": 0.0,
                            "completedAt": now,
                            "timestamp": now,
                        });
                        let _ = crate::eventlog::append_workflow_event(
                            &conn,
                            &instance_id_clone,
                            "execution/status",
                            &event,
                            true,
                        );
                    } else {
                        let event = serde_json::json!({
                            "executionId": instance_id_clone,
                            "status": "failed",
                            "errorMessage": err_str.chars().take(2000).collect::<String>(),
                            "completionRate": 0.0,
                            "completedAt": now,
                            "timestamp": now,
                        });
                        let _ = crate::eventlog::append_workflow_event(
                            &conn,
                            &instance_id_clone,
                            "execution/status",
                            &event,
                            true,
                        );
                    }
                }

                let _ = app_handle_clone.emit(
                    "workflow:execution-progress",
                    serde_json::json!({
                        "execution_id": instance_id_clone,
                        "definition_id": def_clone.id,
                        "mode": "full",
                        "execution": {
                            "status": if is_cancelled { "cancelled" } else { "failed" },
                            "error": err_str,
                            "definition_name": def_clone.name,
                        },
                    }),
                );
            }

            Err(join_err) => {
                let msg = if join_err.is_panic() {
                    join_err
                        .into_panic()
                        .downcast::<String>()
                        .map(|s| *s)
                        .unwrap_or_else(|_| "未知 panic".to_string())
                } else {
                    "任务被取消".to_string()
                };
                log::error!(
                    "[WorkflowEngine] 工作流执行 panic: id={}, error={}",
                    instance_id_clone,
                    msg
                );

                // 回滚：panic 兜底追加 failed 事件。

                if let Some(conn) = app_handle_clone
                    .try_state::<crate::DbState>()
                    .and_then(|s| s.get_conn().ok())
                {
                    let now = crate::utils::now();

                    // 事件化：panic 回滚路径实例 failed。
                    let event = serde_json::json!({
                        "executionId": &instance_id_clone,
                        "status": "failed",
                        "errorMessage": msg.chars().take(2000).collect::<String>(),
                        "completionRate": 0.0,
                        "completedAt": now,
                        "timestamp": now,
                    });
                    let _ = crate::eventlog::append_workflow_event(
                        &conn,
                        &instance_id_clone,
                        "execution/status",
                        &event,
                        true,
                    );
                }

                let _ = app_handle_clone.emit(
                    "workflow:execution-progress",
                    serde_json::json!({
                        "execution_id": instance_id_clone,
                        "definition_id": def_clone.id,
                        "mode": "full",
                        "execution": {
                            "status": "failed",
                            "error": format!("工作流引擎内部错误: {}", msg),
                            "definition_name": def_clone.name,
                        },
                    }),
                );
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
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;

    // 1. 事件化：用户中止 → cancelled（workflow_events 唯一事实源）。
    //    仅当实例仍处于 Running 状态时才追加 cancelled 事件，
    //    避免编辑器卸载时的兜底取消调用覆盖已完成的 success/failed/cancelled 终态。

    let now = crate::utils::now();
    // 0. 撤销该执行下所有待裁决的工具审批：等待者随即收到通道关闭，按拒绝继续。
    //    否则被取消的 Agent 循环会一直阻塞在审批等待上（默认 30 分钟）才结束。
    executor
        .tool_approval_manager
        .forget_execution(&execution_id);
    let is_running = crate::workflow::events::derive_instance(&conn, &execution_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .map(|inst| inst.status == crate::workflow::WorkflowInstanceStatus::Running)
        .unwrap_or(false);
    if !is_running {
        log::info!(
            "[cancel_workflow] 实例 {} 非运行中状态，跳过取消事件",
            execution_id
        );
    } else {
        let event = serde_json::json!({
            "executionId": &execution_id,
            "status": "cancelled",
            "errorMessage": "用户中止",
            "completionRate": 0.0,
            "completedAt": now,
            "timestamp": now,
        });
        let _ = crate::eventlog::append_workflow_event(
            &conn,
            &execution_id,
            "execution/status",
            &event,
            true,
        );
    }

    // 2. 取消执行并立即终止 Agent 子进程（不获取 AgentManager 大锁）
    //    先设置 cancelled AtomicBool 使 tokio::select! 立即响应，
    //    再通过 processes HashMap 直接 kill 子进程（绕过 AsyncMutex 锁竞争）
    let node_ids: Vec<String> = {
        // 节点列表改经事件派生（workflow_events → 已知节点集合）。
        crate::workflow::events::derive_node_statuses(&conn, &execution_id)?
            .into_keys()
            .collect()
    };
    executor
        .inner()
        .cancel_execution_and_kill_agents(&execution_id, &node_ids);
    log::info!(
        "[cancel_workflow] 已取消执行: {}, 关联节点: {:?}",
        execution_id,
        node_ids
    );
    Ok(())
}

/// 批量删除执行记录的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteExecutionsResult {
    /// 实际删除的实例数
    pub deleted: usize,
    /// 因尚未结束而跳过的实例（运行中/待触发/已暂停）
    pub skipped: Vec<SkippedExecution>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedExecution {
    pub id: String,
    /// 状态字面量（running / pending / paused），前端据此提示原因
    pub status: String,
}

/// 实例是否已终态：只有终态实例的执行记录才允许删除。
fn is_terminal_instance(status: &workflow::WorkflowInstanceStatus) -> bool {
    use workflow::WorkflowInstanceStatus::*;
    matches!(status, Success | Failed | Cancelled | Timeout)
}

/// 删除一批执行记录（事件为唯一事实源，删的就是 `workflow_events` 里这些 execution_id 的全部行）。
///
/// **只删已终态的实例**：未结束实例的事件一旦删掉，实例会从列表消失但任务仍在跑，
/// 且取消链路（cancel_workflow → derive_node_statuses 取节点 id → 杀 agent 进程）
/// 再也取不到节点 id，等于把一个跑着的任务丢成孤儿。
/// 不存在的 id 视为已删除（幂等），便于前端重试。
///
/// **不触碰用量**：用量归因绑定的是工作流**定义**（会话 origin=`workflow:{定义id}`），
/// 删执行记录不改变定义的存在性，故 `api_usage_log` 保留（只有 [`workflow::delete_definition`]
/// 才级联清理会话与用量）。
pub fn delete_executions_inner(
    conn: &rusqlite::Connection,
    execution_ids: &[String],
) -> Result<DeleteExecutionsResult, AppError> {
    if execution_ids.is_empty() {
        return Ok(DeleteExecutionsResult {
            deleted: 0,
            skipped: Vec::new(),
        });
    }

    // 一次派生全表实例状态，避免按 id 逐个 derive（实例规模在百级以内）
    let statuses: std::collections::HashMap<String, workflow::WorkflowInstanceStatus> =
        workflow::events::derive_instances(conn)
            .map_err(|e| AppError::Db(format!("读取实例状态失败: {}", e)))?
            .into_iter()
            .map(|i| (i.id, i.status))
            .collect();
    let mut deletable: Vec<&str> = Vec::new();
    let mut skipped: Vec<SkippedExecution> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for id in execution_ids {
        if !seen.insert(id.as_str()) {
            continue; // 同一 id 重复出现只处理一次
        }
        match statuses.get(id) {
            None => {} // 实例不存在：视为已删除，幂等
            Some(s) if is_terminal_instance(s) => deletable.push(id.as_str()),
            Some(s) => skipped.push(SkippedExecution {
                id: id.clone(),
                status: workflow::events::status_code(s).to_string(),
            }),
        }
    }

    // 一条 IN + 一个事务：要么全删，要么全不删
    if !deletable.is_empty() {
        let placeholders = vec!["?"; deletable.len()].join(", ");
        let sql = format!(
            "DELETE FROM workflow_events WHERE execution_id IN ({})",
            placeholders
        );
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Db(format!("开启事务失败: {}", e)))?;
        // 事件为唯一事实源：这里删掉的行数远多于实例数，对外只报"实例数"
        tx.execute(&sql, rusqlite::params_from_iter(deletable.iter()))
            .map_err(|e| AppError::Db(format!("删除执行记录失败: {}", e)))?;
        tx.commit()
            .map_err(|e| AppError::Db(format!("提交事务失败: {}", e)))?;
    }

    Ok(DeleteExecutionsResult {
        deleted: deletable.len(),
        skipped,
    })
}

/// 批量删除执行记录（单条删除即长度为 1，无需另设单条入口）
#[tauri::command]
pub fn delete_executions(
    state: tauri::State<'_, crate::DbState>,
    execution_ids: Vec<String>,
) -> Result<DeleteExecutionsResult, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    delete_executions_inner(&conn, &execution_ids).map_err(String::from)
}

/// 获取执行状态

#[tauri::command]
pub fn get_execution(
    state: tauri::State<'_, crate::DbState>,
    execution_id: String,
) -> Result<Option<workflow::WorkflowInstance>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;

    // 按事件派生实例（workflow_events 为唯一事实源）。
    crate::workflow::events::derive_instance(&conn, &execution_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)).into())
}

/// 获取执行历史

#[tauri::command]
pub fn list_executions(
    state: tauri::State<'_, crate::DbState>,
    definition_id: Option<String>,
) -> Result<Vec<workflow::WorkflowInstance>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    workflow::list_instances(&conn, definition_id.as_deref())
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)).into())
}

/// 获取节点执行详情

#[tauri::command]
pub fn get_node_executions(
    state: tauri::State<'_, crate::DbState>,
    execution_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;

    // 节点执行详情改经事件派生（workflow_events → 折叠行；读切点迁移）。
    crate::workflow::events::derive_node_details(&conn, &execution_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)).into())
}

/// 探测某节点最近一次执行产出里可供条件/映射选择的字段路径（如 `output.result`、`output[0].name`）
///
/// 供边条件编辑器的「探测字段」使用：没有执行记录、或该节点无产出时返回空列表。
/// 取值语义与引擎的 outputMapping 暴露逻辑一致，保证给出的路径在条件里能真正取到值。
#[tauri::command]
pub fn probe_node_output_fields(
    state: tauri::State<'_, crate::DbState>,
    definition_id: String,
    node_id: String,
) -> Result<Vec<String>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;

    // 取该工作流最近一次执行的产出样本
    let instances = workflow::list_instances(&conn, Some(&definition_id))
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?;
    let Some(latest) = instances.into_iter().max_by_key(|i| i.created_at) else {
        return Ok(Vec::new());
    };
    let details = crate::workflow::events::derive_node_details(&conn, &latest.id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?;
    let node_output = details
        .iter()
        .find(|row| row.get("nodeId").and_then(|v| v.as_str()) == Some(node_id.as_str()))
        .and_then(|row| row.get("output"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if node_output.is_null() {
        return Ok(Vec::new());
    }

    let def = workflow::get_definition(&conn, &definition_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?;
    let mapping = def
        .stages
        .iter()
        .flat_map(|s| s.nodes.iter())
        .find(|n| n.id == node_id)
        .and_then(|n| n.output_mapping.as_ref())
        .and_then(|m| m.as_object());
    Ok(crate::workflow::engine::probe_output_field_paths(
        &node_output,
        mapping,
    ))
}

/// 响应人工介入

#[tauri::command]
pub async fn respond_human_input(
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    execution_id: String,
    node_id: String,
    response: String,
) -> Result<(), String> {
    executor
        .human_input_manager
        .resolve(&execution_id, &node_id, response)
        .map_err(|e| AppError::External(format!("响应失败: {}", e)).into())
}

/// 读取指定文件路径的文本内容（供人工交互 file 类型使用）
///
/// 用户在 interact 节点运行时选择文件后，前端调用此命令读取文件内容，
/// 将文本填入响应输入框，提交时把"文件内容"（而非路径）作为响应值传给工作流。
#[tauri::command]
pub fn read_file_content(path: String) -> Result<String, String> {
    if path.trim().is_empty() {
        return Err(AppError::InvalidInput("文件路径为空".to_string()).into());
    }
    // 限制读取大小（10MB），避免误读大文件导致 OOM
    const MAX_BYTES: u64 = 10 * 1024 * 1024;
    let metadata =
        std::fs::metadata(&path).map_err(|e| AppError::Io(format!("无法读取文件信息: {}", e)))?;
    if metadata.len() > MAX_BYTES {
        return Err(AppError::InvalidInput(format!(
            "文件过大（{} 字节，最大支持 {} 字节），请选择小于 10MB 的文本文件",
            metadata.len(),
            MAX_BYTES
        ))
        .into());
    }
    std::fs::read_to_string(&path).map_err(|e| AppError::Io(format!("读取文件失败: {}", e)).into())
}

/// 响应插件命令执行结果（前端执行完插件命令后回传）

#[tauri::command]
pub async fn respond_plugin_execute(
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    execution_id: String,
    node_id: String,
    result: crate::workflow::executors::plugin_executor::PluginExecuteResult,
) -> Result<(), String> {
    executor
        .plugin_execute_manager
        .resolve(&execution_id, &node_id, result)
        .map_err(|e| AppError::External(format!("响应失败: {}", e)).into())
}

/// 创建定时调度
///
/// Cron 表达式在此处**先校验再落库**：非法表达式若落库，调度器每次轮询都会判定"已到期"，
/// 于是每分钟重复触发（旧实现正是如此）。首次执行时间按表达式真实计算，不再固定为 now+60。
#[tauri::command]
pub fn create_schedule(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
    cron_expression: String,
    input_data: Option<String>,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let cron_expression = cron_expression.trim().to_string();
    crate::workflow::scheduler::validate_cron(&cron_expression).map_err(AppError::InvalidInput)?;
    let next_run_at = crate::workflow::scheduler::next_run_after(&cron_expression, now())
        .ok_or_else(|| AppError::InvalidInput("无法计算下一次执行时间".to_string()))?;
    // 会员配额：定时任务数量上限（未配置 / 未登录回落 free 兜底 5；-1 = 不限）。
    // 保存定义时会先删掉本工作流的旧调度再重建，所以这里按「总数」校验是幂等安全的。
    let schedule_limit = crate::commands::account::quota_limit(&conn, "workflow.schedules", 5);
    if schedule_limit != usize::MAX {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM workflow_schedules", [], |r| r.get(0))
            .unwrap_or(0);
        if count as usize >= schedule_limit {
            return Err(AppError::InvalidInput(format!(
                "已达当前等级的定时任务上限（{} 个），升级后可创建更多",
                schedule_limit
            ))
            .into());
        }
    }
    let sched = crate::workflow::scheduler::WorkflowSchedule {
        id: crate::utils::new_id(),
        workflow_id,
        cron_expression,
        enabled: true,
        input_data: input_data.unwrap_or_default(),
        last_run_at: None,
        next_run_at: Some(next_run_at),
        created_at: crate::utils::now(),
        updated_at: crate::utils::now(),
    };
    crate::workflow::scheduler::create_schedule(&conn, &sched)
        .map_err(|e| AppError::Db(format!("创建调度失败: {}", e)).into())
}

/// 启停一条定时调度
#[tauri::command]
pub fn set_schedule_enabled(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let pool = &state.pool;
    crate::workflow::scheduler::set_schedule_enabled(pool, &id, enabled)
        .map_err(|e| AppError::Db(format!("更新调度状态失败: {}", e)).into())
}

/// 平台内置事件清单（工作流「事件触发」可选的事件名，见 workflow::triggers）
#[tauri::command]
pub fn list_workflow_events() -> Vec<serde_json::Value> {
    crate::workflow::triggers::builtin_events()
}

/// 获取调度列表

#[tauri::command]
pub fn list_schedules(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<crate::workflow::scheduler::WorkflowSchedule>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::scheduler::list_schedules(&conn)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)).into())
}

/// 删除调度

#[tauri::command]
pub fn delete_schedule(state: tauri::State<'_, crate::DbState>, id: String) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::scheduler::delete_schedule(&conn, &id)
        .map_err(|e| AppError::Db(format!("删除失败: {}", e)).into())
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

// ── 组织共享工作流 payload（产品间协议，版本化） ──────────────────────

/// 共享 payload 的格式标识（写入 `format` 字段；导入端据此区分新旧格式）。
pub(crate) const SHARED_WORKFLOW_FORMAT: &str = "pilotdesk.shared.workflow";

/// 共享 payload 当前版本。
pub(crate) const SHARED_WORKFLOW_VERSION: u32 = 1;

/// 共享 payload 字符数上限（与平台 `payload` 约束一致）。
///
/// 超过该上限时**拒绝共享并给出明确中文提示**，绝不静默截断（截断会产生失配的子流引用）。
pub(crate) const SHARED_WORKFLOW_PAYLOAD_MAX_CHARS: usize = 256 * 1024;

/// 组织共享工作流 payload —— **产品间协议，版本化**：
///
/// ```json
/// {
///   "format": "pilotdesk.shared.workflow",
///   "version": 1,
///   "main":     { ...主工作流导出 JSON（ExportWorkflowDefinition）... },
///   "subflows": [ { ...子工作流导出 JSON（带 refCode）... }, ... ]
/// }
/// ```
///
/// 子流引用约定（沿用文件导出/导入的既有约定）：
/// - 导出侧：Subflow 节点 `params.definitionId` 被替换为 `params.refCode`
///   （见 [`ExportWorkflowDefinition::remap_subflow_params`]），`refCode` 形如 `ref_N`，
///   与 `subflows[i].refCode` 一一对应；子流集合由 `from_with_subflows` **递归**收集（含嵌套）。
/// - 导入侧：按拓扑序新建子流后建立 `refCode → 新 definitionId` 映射，再回写主/子流的引用
///   （见 [`ExportWorkflowDefinition::restore_subflow_params`]）。
///
/// 向后兼容：**旧格式**是不带 `format` 字段的裸单工作流 JSON（历史已共享的资源），
/// 导入端仍按 [`ExportWorkflowDefinition`] 解析（见 [`parse_shared_workflow_payload`]）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWorkflowBundle {
    /// 固定为 [`SHARED_WORKFLOW_FORMAT`]；缺失即为旧格式。
    pub format: String,
    /// 格式版本，当前为 [`SHARED_WORKFLOW_VERSION`]。
    pub version: u32,
    /// 主工作流。
    pub main: ExportWorkflowDefinition,
    /// 全部子工作流（递归展开，含嵌套子流）。
    pub subflows: Vec<ExportWorkflowDefinition>,
}

impl From<workflow::WorkflowDefinition> for ExportWorkflowDefinition {
    /// 基础转换（不含子工作流处理，用于向后兼容的 export_workflow 命令）

    fn from(def: workflow::WorkflowDefinition) -> Self {
        let stages = Self::convert_stages(&def.stages, &Default::default());
        let stage_short_ids: std::collections::HashMap<String, String> = def
            .stages
            .iter()
            .enumerate()
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
        // 用 `&rusqlite::Connection` 而非 `&PooledConnection`，以便组织共享等**不落盘**场景
        // 复用同一套转换逻辑（`PooledConnection` 会通过 Deref 自动强转为 `&Connection`）。
        conn: &rusqlite::Connection,
    ) -> Result<(Self, Vec<(String, Self)>), AppError> {
        let mut subflow_defs: Vec<(String, Self)> = Vec::new(); // (ref_code, export_def)

        let mut def_id_to_ref: std::collections::HashMap<String, String> =
            std::collections::HashMap::new(); // original defId -> ref_code

        let mut ref_counter: usize = 0;

        // 递归收集子工作流

        Self::collect_subflows_recursive(
            &def,
            conn,
            &mut subflow_defs,
            &mut def_id_to_ref,
            &mut ref_counter,
        )?;

        // 转换主工作流（替换 Subflow 节点的 params）

        let stage_short_ids: std::collections::HashMap<String, String> = def
            .stages
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), format!("s{}", i + 1)))
            .collect();
        let stages =
            Self::convert_stages_with_subflow_remap(&def.stages, &stage_short_ids, &def_id_to_ref);
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
        conn: &rusqlite::Connection,
        subflow_defs: &mut Vec<(String, Self)>,
        def_id_to_ref: &mut std::collections::HashMap<String, String>,
        ref_counter: &mut usize,
    ) -> Result<(), AppError> {
        for stage in &def.stages {
            for node in &stage.nodes {
                if node.node_type != workflow::WorkflowNodeType::Subflow {
                    continue;
                }

                let subflow_def_id = node
                    .params
                    .as_ref()
                    .and_then(|p| p.get("definitionId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if subflow_def_id.is_empty() {
                    continue;
                }

                // 如果已处理过此子工作流，跳过（同一子工作流被多个节点引用）

                if def_id_to_ref.contains_key(subflow_def_id) {
                    continue;
                }

                // 加载子工作流定义

                let sub_def = workflow::get_definition(conn, subflow_def_id)
                    .map_err(|e| AppError::Db(format!("加载子工作流失败: {}", e)))?
                    .ok_or_else(|| {
                        AppError::NotFound(format!("子工作流定义不存在: {}", subflow_def_id))
                    })?;
                *ref_counter += 1;
                let ref_code = format!("ref_{}", ref_counter);
                def_id_to_ref.insert(subflow_def_id.to_string(), ref_code.clone());

                // 递归收集更深层子工作流

                Self::collect_subflows_recursive(
                    &sub_def,
                    conn,
                    subflow_defs,
                    def_id_to_ref,
                    ref_counter,
                )?;

                // 转换子工作流本身（替换其 Subflow 节点的 params）

                let sub_stage_short_ids: std::collections::HashMap<String, String> = sub_def
                    .stages
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (s.id.clone(), format!("s{}", i + 1)))
                    .collect();
                let sub_stages = Self::convert_stages_with_subflow_remap(
                    &sub_def.stages,
                    &sub_stage_short_ids,
                    def_id_to_ref,
                );
                let sub_stage_edges =
                    Self::convert_stage_edges(&sub_def.stages, &sub_stage_short_ids);
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

        let mut global_uuid_to_short: std::collections::HashMap<String, String> =
            stage_uuid_to_short.clone();
        let mut node_counter: usize = 0;
        for stage in stages {
            for node in &stage.nodes {
                node_counter += 1;
                global_uuid_to_short.insert(node.id.clone(), format!("n{}", node_counter));
            }
        }

        stages
            .iter()
            .map(|stage| {
                // 阶段内节点编号仍按数组顺序（n1, n2...），用于边的 source/target

                let stage_node_ids: std::collections::HashMap<String, usize> = stage
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(i, node)| (node.id.clone(), i))
                    .collect();
                let nodes: Vec<ExportNode> = stage
                    .nodes
                    .iter()
                    .map(|node| ExportNode {
                        node_type: node.node_type.clone(),
                        label: node.label.clone(),
                        params: node.params.clone(),
                        delay_ms: node.delay_ms,
                        timeout_ms: node.timeout_ms,
                        input_mapping: Self::remap_mapping_short_ids(
                            &node.input_mapping,
                            &global_uuid_to_short,
                        ),
                        output_mapping: Self::remap_mapping_short_ids(
                            &node.output_mapping,
                            &global_uuid_to_short,
                        ),
                        position: node.position.clone(),
                    })
                    .collect();
                let edges: Vec<ExportEdge> = stage
                    .edges
                    .iter()
                    .map(|edge| {
                        let src_idx = stage_node_ids.get(&edge.source).copied().unwrap_or(0);
                        let tgt_idx = stage_node_ids.get(&edge.target).copied().unwrap_or(0);
                        ExportEdge {
                            source: format!("n{}", src_idx + 1),
                            target: format!("n{}", tgt_idx + 1),
                            label: edge.label.clone(),
                            condition: edge.condition.clone(),
                        }
                    })
                    .collect();
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
            })
            .collect()
    }

    /// 转换阶段列表（含子工作流节点 params 替换）

    /// 预构建全局节点 UUID → 短标识映射（跨阶段连续编号 n1, n2, n3...）

    fn convert_stages_with_subflow_remap(
        stages: &[workflow::Stage],
        stage_uuid_to_short: &std::collections::HashMap<String, String>,
        def_id_to_ref: &std::collections::HashMap<String, String>,
    ) -> Vec<ExportStage> {
        // 预构建全局节点 UUID → 短标识映射（跨阶段连续编号）

        let mut global_uuid_to_short: std::collections::HashMap<String, String> =
            stage_uuid_to_short.clone();
        let mut node_counter: usize = 0;
        for stage in stages {
            for node in &stage.nodes {
                node_counter += 1;
                global_uuid_to_short.insert(node.id.clone(), format!("n{}", node_counter));
            }
        }

        stages
            .iter()
            .map(|stage| {
                // 阶段内节点编号仍按数组顺序，用于边的 source/target

                let stage_node_ids: std::collections::HashMap<String, usize> = stage
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(i, node)| (node.id.clone(), i))
                    .collect();
                let nodes: Vec<ExportNode> = stage
                    .nodes
                    .iter()
                    .map(|node| {
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
                            input_mapping: Self::remap_mapping_short_ids(
                                &node.input_mapping,
                                &global_uuid_to_short,
                            ),
                            output_mapping: Self::remap_mapping_short_ids(
                                &node.output_mapping,
                                &global_uuid_to_short,
                            ),
                            position: node.position.clone(),
                        }
                    })
                    .collect();
                let edges: Vec<ExportEdge> = stage
                    .edges
                    .iter()
                    .map(|edge| {
                        let src_idx = stage_node_ids.get(&edge.source).copied().unwrap_or(0);
                        let tgt_idx = stage_node_ids.get(&edge.target).copied().unwrap_or(0);
                        ExportEdge {
                            source: format!("n{}", src_idx + 1),
                            target: format!("n{}", tgt_idx + 1),
                            label: edge.label.clone(),
                            condition: edge.condition.clone(),
                        }
                    })
                    .collect();
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
            })
            .collect()
    }

    /// 转换阶段连线为导出格式

    fn convert_stage_edges(
        stages: &[workflow::Stage],
        stage_short_ids: &std::collections::HashMap<String, String>,
    ) -> Vec<ExportEdge> {
        stages
            .iter()
            .flat_map(|stage| {
                stage.stage_edges.iter().filter_map(|se| {
                    let src = stage_short_ids.get(&se.source).cloned();
                    let tgt = stage_short_ids.get(&se.target).cloned();
                    match (src, tgt) {
                        (Some(source), Some(target)) => Some(ExportEdge {
                            source,
                            target,
                            label: None,
                            condition: None,
                        }),
                        _ => None,
                    }
                })
            })
            .collect()
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
        let mut stages: Vec<workflow::Stage> = self
            .stages
            .into_iter()
            .map(|stage| {
                let nodes: Vec<workflow::WorkflowNode> = stage
                    .nodes
                    .into_iter()
                    .map(|en| {
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
                            input_schema: None,
                            output_schema: None,
                            input_mapping: en.input_mapping,
                            output_mapping: en.output_mapping,
                            position: en.position,
                        }
                    })
                    .collect();

                // Assign new UUIDs to nodes

                let nodes: Vec<workflow::WorkflowNode> = nodes
                    .into_iter()
                    .map(|mut n| {
                        n.id = crate::utils::new_id();
                        n
                    })
                    .collect();

                // Build node short_id -> new_uuid map for this stage

                let stage_node_map: std::collections::HashMap<String, String> = nodes
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (format!("n{}", i + 1), n.id.clone()))
                    .collect();
                let edges: Vec<workflow::WorkflowEdge> = stage
                    .edges
                    .into_iter()
                    .map(|ee| {
                        let new_source = stage_node_map
                            .get(&ee.source)
                            .cloned()
                            .unwrap_or_else(crate::utils::new_id);
                        let new_target = stage_node_map
                            .get(&ee.target)
                            .cloned()
                            .unwrap_or_else(crate::utils::new_id);
                        workflow::WorkflowEdge {
                            id: crate::utils::new_id(),
                            source: new_source,
                            target: new_target,
                            label: ee.label,
                            condition: ee.condition,
                        }
                    })
                    .collect();

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
            })
            .collect();

        // 构建阶段短标识符 -> 新UUID 映射

        let stage_short_to_uuid: std::collections::HashMap<String, String> = stages
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("s{}", i + 1), s.id.clone()))
            .collect();

        // 二次遍历：构建全局节点短标识 -> 新 UUID 映射，替换所有 mapping

        // 全局短标识是跨阶段连续编号（n1, n2, n3...）

        let mut global_short_to_uuid: std::collections::HashMap<String, String> =
            stage_short_to_uuid.clone();
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
                        let new_mapping: serde_json::Map<String, serde_json::Value> = val
                            .iter()
                            .map(|(k, v)| {
                                let new_v = if let Some(s) = v.as_str() {
                                    Self::remap_mapping_value_uuids(
                                        s,
                                        &global_short_to_uuid,
                                        &std::collections::HashMap::new(),
                                    )
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
                        let new_mapping: serde_json::Map<String, serde_json::Value> = val
                            .iter()
                            .map(|(k, v)| {
                                let new_v = if let Some(s) = v.as_str() {
                                    Self::remap_mapping_value_uuids(
                                        s,
                                        &global_short_to_uuid,
                                        &std::collections::HashMap::new(),
                                    )
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

        let all_stage_edges: Vec<workflow::WorkflowEdge> = self
            .stage_edges
            .into_iter()
            .map(|se| workflow::WorkflowEdge {
                id: crate::utils::new_id(),
                source: stage_short_to_uuid
                    .get(&se.source)
                    .cloned()
                    .unwrap_or_default(),
                target: stage_short_to_uuid
                    .get(&se.target)
                    .cloned()
                    .unwrap_or_default(),
                label: None,
                condition: None,
            })
            .collect();
        for stage in &mut stages {
            stage.stage_edges = all_stage_edges
                .iter()
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
            icon: None,
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
            let new_obj: serde_json::Map<String, serde_json::Value> = obj
                .iter()
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

#[allow(dead_code)]
#[tauri::command]
pub fn import_workflow(
    state: tauri::State<'_, crate::DbState>,
    json_data: String,
) -> Result<workflow::WorkflowDefinition, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let export_def: ExportWorkflowDefinition = serde_json::from_str(&json_data)
        .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;
    let def = export_def.into_definition();
    workflow::create_definition(&conn, &def)
        .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
    Ok(def)
}

/// 导出工作流到文件

#[tauri::command]
pub fn export_workflow_to_file(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    dir_path: String,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let def = workflow::get_definition(&conn, &id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?;
    write_workflow_export(&conn, def, &dir_path)
}

/// 批量导出工作流到文件：对每个 id 各自建子目录写出（与单个导出一致）。
///
/// 返回成功导出的工作流数量；个别失败不中断其余导出，全部失败时返回首个错误。
#[tauri::command]
pub fn export_workflows_to_file(
    state: tauri::State<'_, crate::DbState>,
    ids: Vec<String>,
    dir_path: String,
) -> Result<usize, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let mut exported = 0usize;
    let mut first_err: Option<String> = None;
    for id in &ids {
        let def = match workflow::get_definition(&conn, id) {
            Ok(Some(def)) => def,
            Ok(None) => {
                if first_err.is_none() {
                    first_err = Some(format!("工作流不存在: {}", id));
                }
                continue;
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(format!("查询失败: {}", e));
                }
                continue;
            }
        };
        match write_workflow_export(&conn, def, &dir_path) {
            Ok(()) => exported += 1,
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    if exported == 0 && first_err.is_some() {
        return Err(first_err.unwrap());
    }
    Ok(exported)
}

/// 单个工作流的导出主体（供单个导出与批量导出共用，行为与原先一致）：
/// 在 `<dir_path>/<工作流名>/` 下写出 `[主]` / `[子]` JSON 文件。
fn write_workflow_export(
    conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    def: workflow::WorkflowDefinition,
    dir_path: &str,
) -> Result<(), String> {
    // 判断是否包含子工作流

    let has_subflows = def.stages.iter().any(|stage| {
        stage
            .nodes
            .iter()
            .any(|n| n.node_type == workflow::WorkflowNodeType::Subflow)
    });
    if !has_subflows {
        // 简单导出：无子工作流，使用基础 From impl

        let export_def: ExportWorkflowDefinition = def.into();

        // 自动创建以工作流名称命名的子文件夹

        let workflow_dir = std::path::Path::new(&dir_path).join(&export_def.name);
        std::fs::create_dir_all(&workflow_dir)
            .map_err(|e| AppError::Io(format!("创建文件夹失败: {}", e)))?;
        let file_name = format!("[主]{}.json", export_def.name);
        let file_path = workflow_dir.join(&file_name);
        let json = serde_json::to_string_pretty(&export_def)
            .map_err(|e| AppError::Json(format!("序列化失败: {}", e)))?;
        std::fs::write(&file_path, json)
            .map_err(|e| AppError::Io(format!("写入文件失败: {}", e)))?;
    } else {
        // 含子工作流导出：使用 from_with_subflows

        let (main_export, subflow_exports) =
            ExportWorkflowDefinition::from_with_subflows(def, conn)?;

        // 自动创建以工作流名称命名的子文件夹

        let workflow_dir = std::path::Path::new(&dir_path).join(&main_export.name);
        std::fs::create_dir_all(&workflow_dir)
            .map_err(|e| AppError::Io(format!("创建文件夹失败: {}", e)))?;

        // 写入主文件

        let main_file_name = format!("[主]{}.json", main_export.name);
        let main_file_path = workflow_dir.join(&main_file_name);
        let main_json = serde_json::to_string_pretty(&main_export)
            .map_err(|e| AppError::Json(format!("序列化失败: {}", e)))?;
        std::fs::write(&main_file_path, main_json)
            .map_err(|e| AppError::Io(format!("写入主文件失败: {}", e)))?;

        // 写入各子工作流文件

        for (ref_code, sub_def) in &subflow_exports {
            let file_name = format!("[子]{}({}).json", sub_def.name, ref_code);
            let sub_file_path = workflow_dir.join(&file_name);
            let sub_json = serde_json::to_string_pretty(&sub_def)
                .map_err(|e| AppError::Json(format!("序列化失败: {}", e)))?;
            std::fs::write(&sub_file_path, sub_json)
                .map_err(|e| AppError::Io(format!("写入子工作流文件失败: {}", e)))?;
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
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    import_workflow_from_file_with_conn(&conn, &file_path)
}

/// 导入主体（连接维度实现，文件系统入口）。
///
/// 拆出来是为了让市场安装复用同一套 [主]/[子] 收集逻辑：安装要在**同一个连接里**连续完成
/// "先删旧定义、再导入新的一份"（见 utils/market.rs 的 workflow_market_install）。
///
/// 收集 [子] 文件后交给与文件系统解耦的 [`import_export_bundle_with_conn`] 完成写库与引用回写。
pub(crate) fn import_workflow_from_file_with_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
) -> Result<workflow::WorkflowDefinition, String> {
    // 读取主文件

    let main_json = std::fs::read_to_string(file_path)
        .map_err(|e| AppError::Io(format!("读取文件失败: {}", e)))?;
    let main_export: ExportWorkflowDefinition = serde_json::from_str(&main_json)
        .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;

    // 含子工作流引用时才扫描同目录的 [子] 文件
    let subflow_defs = if export_has_subflow_refs(&main_export) {
        collect_subflow_defs_from_dir(file_path)?
    } else {
        std::collections::HashMap::new()
    };

    let (def, _subflow_count) = import_export_bundle_with_conn(conn, main_export, subflow_defs, None)
        .map_err(String::from)?;
    Ok(def)
}

/// 判断导出定义中的 Subflow 节点是否带有子工作流引用（`refCode`）。
fn export_has_subflow_refs(def: &ExportWorkflowDefinition) -> bool {
    def.stages.iter().any(|stage| {
        stage.nodes.iter().any(|n| {
            n.node_type == workflow::WorkflowNodeType::Subflow
                && n.params.as_ref().and_then(|p| p.get("refCode")).is_some()
        })
    })
}

/// 计算某子工作流（`refCode`）的依赖深度：其直接/间接引用的子流深度最大值 + 1。
///
/// 供导入时拓扑排序使用——深度大的（依赖更多的）先导入，保证被引用者先建库拿到新 id。
fn subflow_depth(
    ref_code: &str,
    defs: &std::collections::HashMap<String, ExportWorkflowDefinition>,
    cache: &mut std::collections::HashMap<String, usize>,
) -> usize {
    if let Some(&d) = cache.get(ref_code) {
        return d;
    }

    let def = match defs.get(ref_code) {
        Some(d) => d,
        None => {
            cache.insert(ref_code.to_string(), 0);
            return 0;
        }
    };
    let max_child = def
        .stages
        .iter()
        .flat_map(|s| s.nodes.iter())
        .filter(|n| n.node_type == workflow::WorkflowNodeType::Subflow)
        .filter_map(|n| n.params.as_ref()?.get("refCode")?.as_str().map(String::from))
        .map(|rc| subflow_depth(&rc, defs, cache))
        .max()
        .unwrap_or(0);
    let depth = max_child + 1;
    cache.insert(ref_code.to_string(), depth);
    depth
}

/// 收集主文件同目录下的 `[子]{name}(ref_N).json` 文件，返回 `refCode → 导出定义`。
fn collect_subflow_defs_from_dir(
    file_path: &str,
) -> Result<std::collections::HashMap<String, ExportWorkflowDefinition>, String> {
    let dir = std::path::Path::new(file_path)
        .parent()
        .ok_or_else(|| AppError::InvalidInput("无法获取文件所在目录".to_string()))?;
    let mut subflow_files: std::collections::HashMap<String, String> =
        std::collections::HashMap::new(); // refCode -> 文件路径

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();

            // 文件名格式：[子]{name}(ref_N).json
            if file_name.starts_with("[子]") && file_name.ends_with(".json") {
                if let Some(rest) = file_name.strip_prefix("[子]") {
                    if let Some(rest) = rest.strip_suffix(".json") {
                        // rest = "{name}(ref_N)"
                        if let Some(start) = rest.rfind("(ref_") {
                            if rest.ends_with(")") {
                                let ref_code = &rest[start + 1..rest.len() - 1]; // "ref_N"
                                subflow_files.insert(
                                    ref_code.to_string(),
                                    entry.path().to_string_lossy().to_string(),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // 读取所有子工作流文件
    let mut subflow_defs: std::collections::HashMap<String, ExportWorkflowDefinition> =
        std::collections::HashMap::new();
    for (ref_code, path) in &subflow_files {
        let json = std::fs::read_to_string(path)
            .map_err(|e| AppError::Io(format!("读取子工作流文件失败: {}", e)))?;
        let def: ExportWorkflowDefinition = serde_json::from_str(&json)
            .map_err(|e| AppError::Json(format!("解析子工作流文件失败: {}", e)))?;
        subflow_defs.insert(ref_code.clone(), def);
    }
    Ok(subflow_defs)
}

/// 把「主 + 子工作流（已解析）」按拓扑序写入数据库，并**重建子流引用**。
///
/// 与文件系统解耦：调用方负责从文件目录（[`collect_subflow_defs_from_dir`]）或
/// 共享 payload（[`parse_shared_workflow_payload`]）收集 `main` 与 `subflows`。这样
/// 组织共享导入无需落临时目录即可复用同一套逻辑。
///
/// 子流引用回写规则：导出侧的 `refCode` → 本次新建子流的 `definitionId`（见
/// [`ExportWorkflowDefinition::restore_subflow_params`]）。返回
/// `(主工作流定义, 实际导入的子工作流数量)`。
fn import_export_bundle_with_conn(
    conn: &rusqlite::Connection,
    main_export: ExportWorkflowDefinition,
    subflow_defs: std::collections::HashMap<String, ExportWorkflowDefinition>,
    // 主工作流的落库 id：`None` = 普通导入（新建、生成新 UUID）；
    // `Some(id)` = 就地导入（复用该 id，存在则更新——云同步 pull 覆盖同一 object_key 用）。
    main_id_override: Option<&str>,
) -> Result<(workflow::WorkflowDefinition, usize), AppError> {
    // 无子工作流引用：直接导入主工作流
    if !export_has_subflow_refs(&main_export) {
        let mut def = main_export.into_definition();
        store_main_definition(conn, &mut def, main_id_override)?;
        return Ok((def, 0));
    }

    // 按拓扑序导入：深度大的（依赖更多的）子工作流先导入
    let mut depth_cache: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut sorted_refs: Vec<String> = subflow_defs.keys().cloned().collect();
    sorted_refs.sort_by(|a, b| {
        let da = subflow_depth(a, &subflow_defs, &mut depth_cache);
        let db = subflow_depth(b, &subflow_defs, &mut depth_cache);
        db.cmp(&da) // 深度大的先导入
    });

    // 逐层写入子工作流，建立 refCode -> 新 UUID 映射
    let mut ref_code_to_uuid: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for ref_code in &sorted_refs {
        let sub_export = subflow_defs
            .get(ref_code)
            .ok_or_else(|| AppError::NotFound(format!("子工作流 {} 未找到", ref_code)))?;
        let def = sub_export
            .clone()
            .into_definition_with_subflows(&ref_code_to_uuid);
        let new_id = def.id.clone();
        workflow::create_definition(conn, &def)
            .map_err(|e| AppError::Db(format!("导入子工作流 {} 失败: {}", ref_code, e)))?;
        ref_code_to_uuid.insert(ref_code.clone(), new_id);
    }

    // 导入主工作流（回写 Subflow 引用的 definitionId）
    let mut main_def = main_export.into_definition_with_subflows(&ref_code_to_uuid);
    store_main_definition(conn, &mut main_def, main_id_override)?;
    Ok((main_def, sorted_refs.len()))
}

/// 主工作流落库：
/// - `override_id` 为 `None` → 一律新建（普通导入，保持既有行为）；
/// - 为 `Some(id)` → 改 id 后 upsert（存在则更新、不存在则新建）——云同步 pull 就地覆盖用，
///   目的是保持同一 object_key，而不是每轮同步都新建副本。
fn store_main_definition(
    conn: &rusqlite::Connection,
    def: &mut workflow::WorkflowDefinition,
    override_id: Option<&str>,
) -> Result<(), AppError> {
    match override_id {
        Some(id) => {
            def.id = id.to_string();
            if workflow::get_definition(conn, id)
                .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
                .is_some()
            {
                workflow::update_definition(conn, def)
                    .map_err(|e| AppError::Db(format!("更新工作流失败: {}", e)))
            } else {
                workflow::create_definition(conn, def)
                    .map_err(|e| AppError::Db(format!("导入失败: {}", e)))
            }
        }
        None => workflow::create_definition(conn, def)
            .map_err(|e| AppError::Db(format!("导入失败: {}", e))),
    }
}

/// 打包「组织共享工作流 payload」：主工作流 + 其全部子工作流（递归、含嵌套），统一使用
/// 带 `format` 字段的新版格式 [`SharedWorkflowBundle`]（**即使没有子流也带 `subflows: []`**，
/// 便于导入端走统一解析路径）。返回 `(主工作流名, payload JSON)`。
///
/// 复用文件导出的同一套转换（`from_with_subflows`：`definitionId → refCode` + 递归收集子流），
/// 但不落盘。超过 [`SHARED_WORKFLOW_PAYLOAD_MAX_CHARS`] 时返回明确中文错误，**绝不截断**。
pub(crate) fn build_workflow_export_json(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<(String, String), AppError> {
    let def = workflow::get_definition(conn, id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?;
    let name = def.name.clone();

    let (main, subflows) = ExportWorkflowDefinition::from_with_subflows(def, conn)?;
    let bundle = SharedWorkflowBundle {
        format: SHARED_WORKFLOW_FORMAT.to_string(),
        version: SHARED_WORKFLOW_VERSION,
        main,
        subflows: subflows.into_iter().map(|(_, def)| def).collect(),
    };
    let json = serde_json::to_string(&bundle)
        .map_err(|e| AppError::Json(format!("序列化失败: {}", e)))?;

    let chars = json.chars().count();
    if chars > SHARED_WORKFLOW_PAYLOAD_MAX_CHARS {
        return Err(AppError::InvalidInput(format!(
            "工作流含子流程后体积过大（{} 字符，上限 {}），暂不支持共享",
            chars, SHARED_WORKFLOW_PAYLOAD_MAX_CHARS
        )));
    }
    Ok((name, json))
}

/// 共享 payload 的解析结果。
enum ParsedSharedPayload {
    /// 新版：主 + 子流捆绑格式
    Bundle(SharedWorkflowBundle),
    /// 旧版：不带 `format` 字段的裸单工作流 JSON（向后兼容历史已共享的资源）
    Legacy(ExportWorkflowDefinition),
}

/// 识别并解析共享 payload：
/// - 带 `"format": "pilotdesk.shared.workflow"` → 新版捆绑格式；
/// - 否则 → 旧版裸单工作流 JSON（向后兼容）。
fn parse_shared_workflow_payload(payload: &str) -> Result<ParsedSharedPayload, AppError> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;
    if value.get("format").and_then(|v| v.as_str()) == Some(SHARED_WORKFLOW_FORMAT) {
        let bundle: SharedWorkflowBundle = serde_json::from_value(value)
            .map_err(|e| AppError::Json(format!("共享工作流解析失败: {}", e)))?;
        Ok(ParsedSharedPayload::Bundle(bundle))
    } else {
        let single: ExportWorkflowDefinition = serde_json::from_value(value)
            .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;
        Ok(ParsedSharedPayload::Legacy(single))
    }
}

/// 从共享 payload 新建一份本地工作流（自动识别新/旧格式；与
/// [`build_workflow_export_json`] 成对，供组织共享空间导入复用）。
///
/// - 新版（含 `format` 字段）：导入主工作流 + 全部子流，并重建子流引用（`refCode → 新 id`）；
/// - 旧版（裸单工作流 JSON）：等价于旧版 `import_workflow`，仅导入主工作流（向后兼容）。
///
/// 返回 `(新建的主工作流, 实际导入的子工作流数量)`。**同名工作流不会覆盖也不会改名**，
/// 而是新建一份同名副本。
pub(crate) fn import_workflow_from_json_with_conn(
    conn: &rusqlite::Connection,
    json: &str,
) -> Result<(workflow::WorkflowDefinition, usize), AppError> {
    match parse_shared_workflow_payload(json)? {
        ParsedSharedPayload::Bundle(bundle) => {
            let mut subflow_defs: std::collections::HashMap<String, ExportWorkflowDefinition> =
                std::collections::HashMap::new();
            for sub in bundle.subflows {
                let ref_code = sub.ref_code.clone().ok_or_else(|| {
                    AppError::Json("共享子工作流缺少 refCode，无法重建引用".to_string())
                })?;
                subflow_defs.insert(ref_code, sub);
            }
            import_export_bundle_with_conn(conn, bundle.main, subflow_defs, None)
        }
        ParsedSharedPayload::Legacy(single) => {
            let def = single.into_definition();
            workflow::create_definition(conn, &def)
                .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
            Ok((def, 0))
        }
    }
}

/// 就地把一份共享 payload（主 + 子流捆绑）应用到指定 object_key 的工作流上——云同步 pull 用。
///
/// 与 [`import_workflow_from_json_with_conn`] 共用同一套解析/转换逻辑，但：
/// - 主工作流**复用 `target_id`**（若不存在则新建），保持跨设备同步的 object_key 稳定，
///   而不是每轮同步都新建一份副本；
/// - 应用前清理该工作流**旧的子流定义**（递归、去重），避免每轮同步堆积孤儿子流；
///   仅清理「只被该工作流引用」的子流——被其他工作流共享引用的子流不动，避免误伤别的父级。
///
/// 返回 `(主工作流定义, 实际导入的子工作流数量)`。
pub(crate) fn apply_workflow_bundle_in_place(
    conn: &rusqlite::Connection,
    target_id: &str,
    json: &str,
) -> Result<(workflow::WorkflowDefinition, usize), AppError> {
    // 1. 清理旧的、仅被目标工作流引用的子流（其内容会由本次 payload 重建）
    if let Some(existing) = workflow::get_definition(conn, target_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
    {
        let mut visited = std::collections::HashSet::new();
        let mut old_subflow_ids = Vec::new();
        collect_referenced_subflow_ids(conn, &existing, &mut old_subflow_ids, &mut visited);
        for sid in old_subflow_ids {
            // 同时被别的父级引用 → 保留，不要删（会破坏那些父级的子流引用）
            let parents = crate::commands::cloud_sync::workflows_referencing(conn, &sid)
                .unwrap_or_default();
            let only_me = !parents.is_empty() && parents.iter().all(|p| p == target_id);
            if only_me {
                if let Err(e) = workflow::delete_definition(conn, &sid) {
                    log::warn!("[CloudSync] 清理旧子流失败 {}：{}", sid, e);
                }
            }
        }
    }

    // 2. 解析并就地落库（主工作流复用 target_id）
    match parse_shared_workflow_payload(json)? {
        ParsedSharedPayload::Bundle(bundle) => {
            let mut subflow_defs: std::collections::HashMap<String, ExportWorkflowDefinition> =
                std::collections::HashMap::new();
            for sub in bundle.subflows {
                let code = sub.ref_code.clone().ok_or_else(|| {
                    AppError::Json("共享子工作流缺少 refCode，无法重建引用".to_string())
                })?;
                subflow_defs.insert(code, sub);
            }
            import_export_bundle_with_conn(conn, bundle.main, subflow_defs, Some(target_id))
        }
        ParsedSharedPayload::Legacy(single) => {
            import_export_bundle_with_conn(
                conn,
                single,
                std::collections::HashMap::new(),
                Some(target_id),
            )
        }
    }
}

/// 递归收集某工作流定义引用的全部子流 id（去重；引用缺失的定义忽略）。
fn collect_referenced_subflow_ids(
    conn: &rusqlite::Connection,
    def: &workflow::WorkflowDefinition,
    out: &mut Vec<String>,
    visited: &mut std::collections::HashSet<String>,
) {
    for stage in &def.stages {
        for node in &stage.nodes {
            if node.node_type != workflow::WorkflowNodeType::Subflow {
                continue;
            }
            let Some(sid) = node
                .params
                .as_ref()
                .and_then(|p| p.get("definitionId"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            if !visited.insert(sid.to_string()) {
                continue;
            }
            out.push(sid.to_string());
            if let Ok(Some(sub)) = workflow::get_definition(conn, sid) {
                collect_referenced_subflow_ids(conn, &sub, out, visited);
            }
        }
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
    days: Option<i64>,
) -> Result<crate::workflow::WorkflowStats, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::get_workflow_stats(&conn, workflow_id.as_deref(), days)
        .map_err(|e| AppError::Db(format!("查询统计失败: {}", e)).into())
}

/// 获取执行时间线

#[tauri::command]
pub fn get_execution_timeline(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: Option<String>,
    days: Option<i64>,
) -> Result<Vec<crate::workflow::ExecutionTimelinePoint>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::get_execution_timeline(&conn, workflow_id.as_deref(), days.unwrap_or(30))
        .map_err(|e| AppError::Db(format!("查询时间线失败: {}", e)).into())
}

/// 获取节点类型使用统计

#[tauri::command]
pub fn get_node_type_stats(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: Option<String>,
    days: Option<i64>,
) -> Result<Vec<crate::workflow::NodeTypeStat>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::get_node_type_stats(&conn, workflow_id.as_deref(), days)
        .map_err(|e| AppError::Db(format!("查询节点类型统计失败: {}", e)).into())
}

/// 获取执行最频繁 / 失败率最高 Top N 工作流

#[tauri::command]
pub fn get_top_workflows(
    state: tauri::State<'_, crate::DbState>,
    days: Option<i64>,
    limit: Option<i64>,
    sort_by: Option<String>,
) -> Result<Vec<crate::workflow::TopWorkflowStat>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::get_top_workflows(&conn, days, limit, sort_by.as_deref())
        .map_err(|e| AppError::Db(format!("查询 Top 工作流失败: {}", e)).into())
}

/// 获取失败实例最常见的错误文本 Top N

#[tauri::command]
pub fn get_top_errors(
    state: tauri::State<'_, crate::DbState>,
    days: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<crate::workflow::TopErrorStat>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::get_top_errors(&conn, days, limit)
        .map_err(|e| AppError::Db(format!("查询 Top 错误失败: {}", e)).into())
}

/// 获取工作流最大并发数

#[tauri::command]
pub fn get_workflow_max_concurrency(
    state: tauri::State<'_, crate::DbState>,
) -> Result<usize, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let value: String = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'workflow_max_concurrency'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "5".to_string());
    Ok(value.parse::<usize>().unwrap_or(10))
}

/// 按会员配额夹取设置值：结果为 `[1, min(hard_max, quota)]`。
/// `quota == usize::MAX` 表示不限 → 只用 `hard_max`；`hard_max` 为产品硬上限。
fn clamp_with_quota(value: i64, quota: usize, hard_max: i64) -> i64 {
    let quota_max = if quota == usize::MAX {
        hard_max
    } else {
        (quota as i64).min(hard_max)
    };
    let upper = quota_max.max(1);
    value.clamp(1, upper)
}

/// 设置工作流最大并发数

#[tauri::command]
pub fn set_workflow_max_concurrency(
    state: tauri::State<'_, crate::DbState>,
    max_concurrency: usize,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    // 上限取 min(产品硬上限 20, 会员配额)；未登录 / 未下发回落 free 档兜底 10。
    let quota = crate::commands::account::quota_limit(&conn, "workflow.concurrency", 10);
    let clamped = clamp_with_quota(max_concurrency as i64, quota, 20);
    conn.execute(

        "INSERT OR REPLACE INTO app_settings (key, value, updated_at) VALUES ('workflow_max_concurrency', ?1, ?2)",
        rusqlite::params![clamped.to_string(), crate::utils::now()],
    )
    .map_err(|e| AppError::Db(format!("保存失败: {}", e)))?;
    Ok(())
}

/// 设置子工作流最大嵌套深度

#[tauri::command]
pub fn set_workflow_max_subflow_depth(
    state: tauri::State<'_, crate::DbState>,
    max_depth: i64,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    // 上限取 min(产品硬上限 10, 会员配额)；未登录 / 未下发回落 free 档兜底 3。
    let quota = crate::commands::account::quota_limit(&conn, "workflow.subflowDepth", 3);
    let clamped = clamp_with_quota(max_depth, quota, 10);
    conn.execute(
        "INSERT OR REPLACE INTO app_settings (key, value, updated_at) VALUES ('workflow_max_subflow_depth', ?1, ?2)",
        rusqlite::params![clamped.to_string(), crate::utils::now()],
    )
    .map_err(|e| AppError::Db(format!("保存失败: {}", e)))?;
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
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::duplicate_definition(&conn, &id, &new_name)
        .map_err(|e| AppError::Db(format!("复制失败: {}", e)).into())
}

/// 列出工作流版本历史

#[tauri::command]
pub fn list_workflow_versions(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
) -> Result<Vec<crate::workflow::WorkflowVersion>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::list_workflow_versions(&conn, &workflow_id)
        .map_err(|e| AppError::Db(format!("查询版本失败: {}", e)).into())
}

/// 保存工作流版本快照

#[tauri::command]
pub fn save_workflow_version(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
    snapshot: String,
) -> Result<crate::workflow::WorkflowVersion, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::save_workflow_version(&conn, &workflow_id, &snapshot)
        .map_err(|e| AppError::Db(format!("保存版本失败: {}", e)).into())
}

/// 恢复到指定版本

#[tauri::command]
pub fn restore_workflow_version(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
    version: i64,
) -> Result<crate::workflow::WorkflowVersion, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::restore_workflow_version(&conn, &workflow_id, version)
        .map_err(|e| AppError::Db(format!("恢复版本失败: {}", e)).into())
}

/// 删除指定版本快照
#[tauri::command]
pub fn delete_workflow_version(
    state: tauri::State<'_, crate::DbState>,
    workflow_id: String,
    version: i64,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::delete_workflow_version(&conn, &workflow_id, version)
        .map_err(|e| AppError::Db(format!("删除版本失败: {}", e)).into())
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
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    crate::workflow::get_node_execution_logs(&conn, &node_execution_id)
        .map_err(|e| AppError::Db(format!("查询日志失败: {}", e)).into())
}

// ════════════════════════════════════════════════════════════

// 执行恢复命令

// ════════════════════════════════════════════════════════════

/// 列出**中断的**执行（进程重启后残留的 running/paused，可用于补全或清理）。
///
/// 必须按进程内的运行登记表过滤：事件日志只能说"状态是 running"，
/// 而后台正在跑的实例同样是 running —— 不过滤就会把"正在后台跑"当成"中断残留"报给用户
/// （曾出现"检测到 1 个未完成的执行记录"却在正常运行的误报）。
#[tauri::command]
pub fn list_recoverable_executions(
    state: tauri::State<'_, crate::DbState>,
    executor: tauri::State<'_, Arc<NodeExecutor>>,
) -> Result<Vec<crate::workflow::RecoverableExecution>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let live: std::collections::HashSet<String> =
        executor.live_execution_ids().into_iter().collect();
    let mut list = crate::workflow::list_recoverable_executions(&conn)
        .map_err(|e| AppError::Db(format!("查询可恢复执行失败: {}", e)))?;
    list.retain(|r| !live.contains(&r.execution.id));
    Ok(list)
}

/// 进程内正在执行的实例 id（运行登记表）。
///
/// 前端据此区分"真的在跑"与"进程重启后的残留 running"：例如打开编辑器时接上后台运行的画布，
/// 而不是显示成"未运行"。
#[tauri::command]
pub fn list_live_executions(executor: tauri::State<'_, Arc<NodeExecutor>>) -> Vec<String> {
    executor.live_execution_ids()
}

// ════════════════════════════════════════════════════════════

// ════════════════════════════════════════════════════════════
// 断点执行（单点执行 / 链式执行 / 补全执行）
// ════════════════════════════════════════════════════════════

/// 统一断点执行命令
///
/// mode: "full" | "single" | "chain" | "completion"
/// 断点模式下 execution_id 对应的实例必须已存在执行记录。
#[tauri::command]
pub async fn execute_workflow_mode(
    app_handle: tauri::AppHandle,
    executor: tauri::State<'_, Arc<crate::workflow::executor::NodeExecutor>>,
    execution_id: String,
    mode: String,
    node_id: Option<String>,
) -> Result<(), String> {
    use crate::workflow::engine::ExecutionMode;
    let exec_mode = match mode.as_str() {
        "single" => {
            let nid =
                node_id.ok_or_else(|| AppError::InvalidInput("单点执行需要指定 node_id".into()))?;
            ExecutionMode::SingleNode { node_id: nid }
        }
        "chain" => {
            let nid =
                node_id.ok_or_else(|| AppError::InvalidInput("链式执行需要指定 node_id".into()))?;
            ExecutionMode::Chain { node_id: nid }
        }
        "completion" => ExecutionMode::Completion,
        "full" => ExecutionMode::Full,
        _ => return Err(AppError::InvalidInput(format!("不支持的执行模式: {}", mode)).into()),
    };
    let conn = app_handle
        .state::<crate::DbState>()
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let instance = crate::workflow::events::derive_instance(&conn, &execution_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("实例不存在".to_string()))?;

    // 断点模式校验：实例必须已有执行记录
    if !matches!(exec_mode, ExecutionMode::Full) {
        let node_count: i64 = crate::workflow::events::derive_node_statuses(&conn, &execution_id)
            .map_err(|e| AppError::Db(format!("查询执行记录失败: {}", e)))?
            .len() as i64;
        if node_count == 0 {
            return Err(AppError::InvalidInput(
                "该实例无执行记录，不支持断点执行。请先执行一次工作流。".into(),
            )
            .into());
        }
    }

    let def = crate::workflow::get_definition(&conn, &instance.definition_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("工作流定义不存在".to_string()))?;
    let max_concurrency: usize = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'workflow_max_concurrency'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let input_data = serde_json::from_str::<serde_json::Value>(&instance.context.to_string())
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let executor = executor.inner().clone();
    let exec_id = execution_id.clone();
    let def_clone = def.clone();
    let inner_handle = tokio::spawn(async move {
        crate::workflow::engine::WorkflowEngine::execute_with_mode(
            &executor,
            &def_clone,
            &exec_id,
            input_data,
            &app_handle,
            max_concurrency,
            exec_mode,
        )
        .await
    });
    match inner_handle.await {
        Ok(Ok(_)) => {
            log::info!(
                "[execute_workflow_mode] 执行成功: id={}, mode={}",
                execution_id,
                mode
            );
            Ok(())
        }
        Ok(Err(e)) => {
            let err_str = e.to_string();
            log::error!(
                "[execute_workflow_mode] 执行失败: id={}, mode={}, error={}",
                execution_id,
                mode,
                err_str
            );
            // 上抛失败：实例终态事件已由引擎写入并广播，返回值让调用方同样能感知失败。
            Err(err_str)
        }
        Err(join_err) => {
            let msg = if join_err.is_panic() {
                "任务 panic".to_string()
            } else {
                "任务被取消".to_string()
            };
            log::error!(
                "[execute_workflow_mode] panic: id={}, error={}",
                execution_id,
                msg
            );
            Err(msg)
        }
    }
}

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
            .map_err(|e| AppError::Json(format!("工作流定义解析失败: {}", e)))?
    } else {
        let wf_id = workflow_id.ok_or_else(|| {
            AppError::InvalidInput("必须提供 workflow_id 或 definition 参数".into())
        })?;
        let conn = state
            .get_conn()
            .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
        workflow::get_definition(&conn, &wf_id)
            .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
            .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?
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
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let def = if let Some(def_val) = definition {
        serde_json::from_value::<crate::workflow::WorkflowDefinition>(def_val)
            .map_err(|e| AppError::Json(format!("工作流定义解析失败: {}", e)))?
    } else {
        let wf_id = workflow_id.ok_or_else(|| {
            AppError::InvalidInput("必须提供 workflow_id 或 definition 参数".into())
        })?;
        workflow::get_definition(&conn, &wf_id)
            .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
            .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?
    };
    let mode = crate::workflow::ExecutionMode::default();
    crate::workflow::engine::WorkflowEngine::validate_workflow_for_execution(&def, &conn, &mode)
        .map_err(|e| AppError::InvalidInput(format!("校验失败: {}", e)).into())
}

// ════════════════════════════════════════════════════════════

// 人工介入查询命令

// ════════════════════════════════════════════════════════════

/// 获取所有待响应的人工介入节点

#[tauri::command]
pub fn get_pending_human_inputs(
    state: tauri::State<'_, crate::DbState>,
    executor: tauri::State<'_, Arc<NodeExecutor>>,
) -> Result<Vec<crate::workflow::PendingHumanInput>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let mut list = crate::workflow::get_pending_human_inputs(&conn)
        .map_err(|e| AppError::Db(format!("查询待响应请求失败: {}", e)))?;
    // 只保留**此刻真有等待者**的项：事件日志只能说明"曾经在等"，进程异常退出后
    // interact 节点会永远停在 running —— 日志推导出来的待办就成了点也点不动的幽灵。
    // 进程内的等待登记表是唯一权威（随进程生灭，页面切换不丢）。
    let live = executor.human_input_manager.pending_keys();
    list.retain(|p| live.contains(&format!("{}:{}", p.execution_id, p.node_id)));
    Ok(list)
}

/// 待用户裁决的工作流工具审批。
///
/// 两类合并返回，靠 `stale` 区分：
/// - `stale = false`：进程内还有等待者 —— 可操作卡片（点批准/拒绝会唤醒挂起的节点）；
/// - `stale = true`：进程重启后残留的失效记录（事件流有请求、等待登记表已空）—— 只读，
///   前端提示"该审批已失效"，并引导用户停止该执行。
///
/// 判据与人工输入一致：以**进程内等待登记表**为权威，事件流只作证据。
#[tauri::command]
pub fn get_pending_tool_approvals(
    state: tauri::State<'_, crate::DbState>,
    executor: tauri::State<'_, Arc<NodeExecutor>>,
) -> Result<Vec<crate::workflow::executors::agent_executor::PendingToolApproval>, String> {
    let live = executor.tool_approval_manager.pending_items();
    let live_keys: std::collections::HashSet<String> = live
        .iter()
        .map(|i| format!("{}:{}", i.execution_id, i.call_id))
        .collect();
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let stale = crate::workflow::list_unresolved_tool_approvals(&conn)
        .map_err(|e| AppError::Db(format!("查询失效审批失败: {}", e)))?
        .into_iter()
        .filter(|i| !live_keys.contains(&format!("{}:{}", i.execution_id, i.call_id)));
    Ok(live.into_iter().chain(stale).collect())
}

/// 用户裁决一次工作流工具审批（唤醒挂起的 Agent 节点）。
///
/// 未命中等待登记表时返回错误：审批可能已超时（按拒绝处理并已续跑）或进程已重启，
/// 前端据此提示"审批已失效"，而不是让用户以为提交成功。
#[tauri::command]
pub fn respond_tool_approval(
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    call_id: String,
    approved: bool,
) -> Result<(), String> {
    executor.tool_approval_manager.resolve(&call_id, approved)?;
    Ok(())
}

/// 检查子工作流是否会形成闭环（供前端下拉过滤使用）

#[tauri::command]
pub fn check_subflow_cycle(
    state: tauri::State<'_, crate::DbState>,
    parent_id: String,
    candidate_id: String,
) -> Result<bool, String> {
    use std::collections::HashSet;
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    fn check_cycle(
        conn: &rusqlite::Connection,
        current_id: &str,
        target_id: &str,
        visited: &mut HashSet<String>,
    ) -> Result<bool, AppError> {
        if current_id == target_id {
            return Ok(true);
        }

        if !visited.insert(current_id.to_string()) {
            return Ok(false);
        }

        // 加载当前工作流定义

        let def = crate::workflow::get_definition(conn, current_id)
            .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
            .ok_or_else(|| AppError::NotFound(format!("工作流不存在: {}", current_id)))?;

        // 遍历所有 Subflow 节点

        for stage in &def.stages {
            for node in &stage.nodes {
                if node.node_type != crate::workflow::WorkflowNodeType::Subflow {
                    continue;
                }

                let subflow_id = node
                    .params
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
    check_cycle(&conn, &candidate_id, &parent_id, &mut visited).map_err(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 建一个只含 workflow_events 的内存库（执行记录删除的唯一依赖）。
    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::init::FINAL_SCHEMA_SQL)
            .unwrap();
        conn
    }

    /// 造一个实例：created 事件 + 可选的终态 status 事件。
    fn seed_instance(conn: &rusqlite::Connection, id: &str, def_id: &str, terminal: Option<&str>) {
        crate::eventlog::append_workflow_event(
            conn,
            id,
            "execution/created",
            &json!({
                "executionId": id,
                "definitionId": def_id,
                "definitionName": "wf",
                "trigger": "manual",
                "status": "running",
            }),
            true,
        )
        .unwrap();
        if let Some(status) = terminal {
            crate::eventlog::append_workflow_event(
                conn,
                id,
                "execution/status",
                &json!({ "executionId": id, "status": status }),
                true,
            )
            .unwrap();
        }
    }

    fn count_events(conn: &rusqlite::Connection, id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM workflow_events WHERE execution_id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// 终态可删、未终态跳过、不存在的 id 幂等、重复 id 只算一次。
    #[test]
    fn delete_executions_removes_only_terminal_instances() {
        let conn = test_conn();
        seed_instance(&conn, "e-running", "d1", None);
        seed_instance(&conn, "e-success", "d1", Some("success"));
        seed_instance(&conn, "e-failed", "d1", Some("failed"));
        let ids = vec![
            "e-running".to_string(),
            "e-success".to_string(),
            "e-success".to_string(), // 重复：只算一次
            "e-failed".to_string(),
            "e-missing".to_string(), // 不存在：幂等，不报错也不计数
        ];
        let result = delete_executions_inner(&conn, &ids).unwrap();
        assert_eq!(result.deleted, 2, "只应删除两个终态实例");
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].id, "e-running");
        assert_eq!(result.skipped[0].status, "running");

        // 未结束实例的事件必须原样保留，否则实例会消失却仍在跑、且再也取消不掉
        assert!(count_events(&conn, "e-running") > 0);
        assert_eq!(count_events(&conn, "e-success"), 0);
        assert_eq!(count_events(&conn, "e-failed"), 0);
    }

    /// 空列表是安全的 no-op。
    #[test]
    fn delete_executions_with_empty_list_is_noop() {
        let conn = test_conn();
        seed_instance(&conn, "e1", "d1", Some("success"));
        let result = delete_executions_inner(&conn, &[]).unwrap();
        assert_eq!(result.deleted, 0);
        assert!(result.skipped.is_empty());
        assert!(count_events(&conn, "e1") > 0);
    }

    /// 删定义守卫：仍有未结束实例时拒绝（且只统计本定义的实例）；全部终态后放行。
    #[test]
    fn delete_guard_refuses_while_definition_has_unfinished_executions() {
        let conn = test_conn();
        seed_instance(&conn, "e-running", "d1", None);
        seed_instance(&conn, "e-success", "d1", Some("success"));
        seed_instance(&conn, "e-other-running", "d2", None); // 其它定义：不计入 d1 的守卫

        let err = ensure_no_unfinished_executions(&conn, "d1")
            .unwrap_err()
            .to_string();
        assert!(err.contains("未结束"), "错误应说明未结束: {}", err);
        assert!(err.contains('1'), "应报出未结束实例数量: {}", err);

        // 同一守卫对「其它定义」放行（其自身未结束不影响别的定义）
        let conn2 = test_conn();
        seed_instance(&conn2, "e-other-running", "d2", None);
        assert!(ensure_no_unfinished_executions(&conn2, "d1").is_ok());

        // 全部终态后放行
        let conn3 = test_conn();
        seed_instance(&conn3, "e-success", "d1", Some("success"));
        assert!(ensure_no_unfinished_executions(&conn3, "d1").is_ok());
    }

    /// 配额夹取：配额足够 → 用硬上限；配额更小 → 用配额；不限 → 用硬上限；值越界 → 被夹取。
    #[test]
    fn clamp_with_quota_respects_quota_and_hard_max() {
        // 配额足够（>= 硬上限）→ 保持原值；超过硬上限 → 夹到硬上限
        assert_eq!(clamp_with_quota(15, 20, 20), 15);
        assert_eq!(clamp_with_quota(25, 20, 20), 20);
        // 配额更小 → 夹到配额
        assert_eq!(clamp_with_quota(15, 8, 20), 8);
        // 不限（usize::MAX）→ 用硬上限
        assert_eq!(clamp_with_quota(15, usize::MAX, 20), 15);
        assert_eq!(clamp_with_quota(99, usize::MAX, 20), 20);
        // 下限 1
        assert_eq!(clamp_with_quota(0, 10, 20), 1);
        assert_eq!(clamp_with_quota(-5, 10, 20), 1);
        // 子流程深度：兜底配额 3，硬上限 10
        assert_eq!(clamp_with_quota(5, 3, 10), 3);
        assert_eq!(clamp_with_quota(5, 10, 10), 5);
    }

    // ── 组织共享工作流：打包 / 解包 / 子流引用回写 ─────────────────────

    /// 造一个工作流定义；`subflow_def_id` 非空时附带一个引用它的 Subflow 节点。
    fn make_share_def(
        id: &str,
        name: &str,
        subflow_def_id: Option<&str>,
    ) -> workflow::WorkflowDefinition {
        let make_node =
            |t: workflow::WorkflowNodeType, label: &str, params: Option<serde_json::Value>| {
                workflow::WorkflowNode {
                    id: crate::utils::new_id(),
                    node_type: t,
                    label: label.to_string(),
                    plugin_id: None,
                    command_id: None,
                    params,
                    delay_ms: None,
                    timeout_ms: None,
                    input_schema: None,
                    output_schema: None,
                    input_mapping: None,
                    output_mapping: None,
                    position: None,
                }
            };
        let mut nodes = vec![make_node(workflow::WorkflowNodeType::Start, "开始", None)];
        if let Some(sid) = subflow_def_id {
            nodes.push(make_node(
                workflow::WorkflowNodeType::Subflow,
                "子流程",
                Some(json!({ "definitionId": sid })),
            ));
        }
        workflow::WorkflowDefinition {
            id: id.to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            trigger: workflow::TriggerConfig {
                trigger_type: workflow::TriggerType::Manual,
                cron: None,
                event_name: None,
            },
            stages: vec![workflow::Stage {
                id: crate::utils::new_id(),
                name: "默认阶段".to_string(),
                order: 0,
                nodes,
                edges: vec![],
                stage_edges: vec![],
                gate: workflow::GateConfig::default(),
                collapsed: false,
                offset_x: 0.0,
                offset_y: 0.0,
            }],
            icon: None,
            input_schema: None,
            output_schema: None,
            created_at: 0,
            updated_at: 0,
            enabled: true,
        }
    }

    /// 打包「主 + 子流」→ 新格式 payload；解包导入 → 重建子流引用（旧 id → 新 id）。
    #[test]
    fn share_payload_bundles_subflows_and_remaps_on_import() {
        let conn = test_conn();
        // 子工作流
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        // 主工作流（Subflow 节点引用子工作流）
        let main_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();

        // 打包：应为新格式，主工作流 Subflow 节点带 refCode，subflows 恰好 1 条
        let (name, payload) = build_workflow_export_json(&conn, &main_id).unwrap();
        assert_eq!(name, "主流程");
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["format"], SHARED_WORKFLOW_FORMAT);
        assert_eq!(value["version"], SHARED_WORKFLOW_VERSION);
        assert_eq!(value["subflows"].as_array().unwrap().len(), 1);

        let main_nodes = value["main"]["stages"][0]["nodes"].as_array().unwrap();
        let sub_node = main_nodes.iter().find(|n| n["type"] == "subflow").unwrap();
        let main_ref = sub_node["params"]["refCode"].as_str().unwrap();
        let sub_ref = value["subflows"][0]["refCode"].as_str().unwrap();
        assert_eq!(main_ref, sub_ref, "主 Subflow 节点与子流的 refCode 必须一致");

        // 解包导入：主 + 子流各新建一份，主 Subflow 节点回写到新子流 id
        let (imported, count) = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        assert_eq!(count, 1, "应随主流程导入 1 个子工作流");
        assert_ne!(imported.id, main_id, "导入应生成新的主工作流 id");
        let new_def_id = imported
            .stages
            .iter()
            .flat_map(|s| s.nodes.iter())
            .find(|n| n.node_type == workflow::WorkflowNodeType::Subflow)
            .and_then(|n| n.params.as_ref()?.get("definitionId")?.as_str())
            .expect("导入后应回写 Subflow 节点的 definitionId");
        assert_ne!(new_def_id, sub_id, "子流应获得新 id");
        assert!(
            workflow::get_definition(&conn, new_def_id).unwrap().is_some(),
            "Subflow 节点的 definitionId 必须指向已导入的子工作流"
        );
    }

    /// 无子流时 payload 仍为新格式（`subflows: []`），且能被正常解析导入。
    #[test]
    fn share_payload_without_subflows_uses_bundle_format() {
        let conn = test_conn();
        let id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&id, "独立流程", None)).unwrap();
        let (_name, payload) = build_workflow_export_json(&conn, &id).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["format"], SHARED_WORKFLOW_FORMAT);
        assert!(value["subflows"].as_array().unwrap().is_empty());
        let (_def, count) = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        assert_eq!(count, 0);
    }

    /// 格式识别：旧格式（裸单工作流 JSON）与新格式都能被正确区分。
    #[test]
    fn parse_shared_payload_recognizes_new_and_legacy_formats() {
        // 旧格式：裸 ExportWorkflowDefinition，无 format 字段
        let legacy = ExportWorkflowDefinition::from(make_share_def("d1", "旧格式", None));
        let legacy_json = serde_json::to_string(&legacy).unwrap();
        match parse_shared_workflow_payload(&legacy_json).unwrap() {
            ParsedSharedPayload::Legacy(d) => assert_eq!(d.name, "旧格式"),
            ParsedSharedPayload::Bundle(_) => panic!("裸单工作流 JSON 应识别为旧格式"),
        }

        // 新格式：带 format 字段
        let bundle = SharedWorkflowBundle {
            format: SHARED_WORKFLOW_FORMAT.to_string(),
            version: SHARED_WORKFLOW_VERSION,
            main: ExportWorkflowDefinition::from(make_share_def("d2", "新格式主", None)),
            subflows: vec![],
        };
        let bundle_json = serde_json::to_string(&bundle).unwrap();
        match parse_shared_workflow_payload(&bundle_json).unwrap() {
            ParsedSharedPayload::Bundle(b) => {
                assert_eq!(b.main.name, "新格式主");
                assert!(b.subflows.is_empty());
            }
            ParsedSharedPayload::Legacy(_) => panic!("带 format 字段的 payload 应识别为新格式"),
        }
    }

    /// 子流引用的「旧 id → refCode」（导出）与「refCode → 新 id」（导入）纯函数回写。
    #[test]
    fn subflow_param_ref_roundtrip_replaces_id() {
        let old_id = "old-def-uuid";
        let params = Some(json!({ "definitionId": old_id, "foo": 1 }));

        let mut id_to_ref = std::collections::HashMap::new();
        id_to_ref.insert(old_id.to_string(), "ref_1".to_string());
        let remapped = ExportWorkflowDefinition::remap_subflow_params(&params, &id_to_ref).unwrap();
        assert_eq!(
            remapped.get("refCode").and_then(|v| v.as_str()),
            Some("ref_1")
        );
        assert!(remapped.get("definitionId").is_none(), "definitionId 应被移除");
        assert_eq!(remapped.get("foo").and_then(|v| v.as_i64()), Some(1));

        let mut ref_to_uuid = std::collections::HashMap::new();
        ref_to_uuid.insert("ref_1".to_string(), "new-def-uuid".to_string());
        let restored =
            ExportWorkflowDefinition::restore_subflow_params(&Some(remapped), &ref_to_uuid)
                .unwrap();
        assert_eq!(
            restored.get("definitionId").and_then(|v| v.as_str()),
            Some("new-def-uuid")
        );
        assert!(restored.get("refCode").is_none(), "refCode 应被移除");
    }
}
