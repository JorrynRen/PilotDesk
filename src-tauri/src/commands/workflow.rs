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

/// 删除工作流（**软删除**：移入回收站，不真删数据）
///
/// 批次 2 起用户主动删除一律走 [`workflow::soft_delete_definition`]：定义只是打上删除标记、
/// 从列表隐藏，其节点会话、用量与执行记录都保留；由回收站的「恢复」还原、「彻底删除」真删。
/// 因此这里不再清理执行记录，返回的 `deleted` 恒为 0（保留返回结构以兼容前端调用）。
///
/// 两道守卫仍然保留：
/// - [`ensure_not_referenced`]：仍被其它工作流作为子流引用时拒绝删除（批次 1 行为）；
/// - [`ensure_no_unfinished_executions`]：仍有未结束执行时拒绝，避免"删了又在跑"。
#[tauri::command]
pub fn delete_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<DeleteExecutionsResult, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    // 用户主动删除：若仍被其他工作流作为子流引用，拒绝删除，避免留下悬空引用
    // （见 [`ensure_not_referenced`]）。**仅**用户主动删除走此守卫；云同步的远端删除传播
    // 直接调用 `workflow::soft_delete_definition`，不受影响（否则同步会被历史引用卡死）。
    ensure_not_referenced(&conn, &id)?;
    ensure_no_unfinished_executions(&conn, &id)?;
    workflow::soft_delete_definition(&conn, &id)
        .map_err(|e| AppError::Db(format!("删除失败: {}", e)))?;
    Ok(DeleteExecutionsResult {
        deleted: 0,
        skipped: Vec::new(),
    })
}

/// 回收站恢复结果：`id` 为原稳定 ID，`name` 为恢复后的名称（重名时可能追加了序号）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredWorkflow {
    pub id: String,
    pub name: String,
}

/// 清空回收站结果：被彻底删除的工作流数量。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyRecycleBinResult {
    pub purged: usize,
}

/// 回收站：列出已软删除的工作流（按删除时间倒序）。
#[tauri::command]
pub fn list_deleted_workflows(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<workflow::DeletedWorkflow>, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    workflow::list_deleted_workflows(&conn)
        .map_err(|e| AppError::Db(format!("查询回收站失败: {}", e)).into())
}

/// 回收站：恢复一个工作流（与现有未删除工作流重名时自动追加序号）。
#[tauri::command]
pub fn restore_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<RestoredWorkflow, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let (id, name) = workflow::restore_workflow(&conn, &id)
        .map_err(|e| AppError::Db(format!("恢复失败: {}", e)))?;
    Ok(RestoredWorkflow { id, name })
}

/// 回收站：彻底删除一个工作流（真删，连同版本记录级联清理）。
#[tauri::command]
pub fn purge_workflow(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    workflow::purge_workflow(&conn, &id)
        .map_err(|e| AppError::Db(format!("彻底删除失败: {}", e)).into())
}

/// 回收站：清空（逐个彻底删除），返回清除数量。
#[tauri::command]
pub fn empty_recycle_bin(
    state: tauri::State<'_, crate::DbState>,
) -> Result<EmptyRecycleBinResult, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let purged = workflow::empty_recycle_bin(&conn)
        .map_err(|e| AppError::Db(format!("清空回收站失败: {}", e)))?;
    Ok(EmptyRecycleBinResult { purged })
}

/// 找出引用了 `child_id` 的工作流（`id` + 名称）。
///
/// 现有云同步侧的 `referenced_workflow_ids` 是**反向**的（查"哪些工作流被引用"）；
/// 这里需要「查谁引用了我」，故复用云同步侧的 `workflows_referencing`（同口径：
/// 遍历全部定义、看是否有 Subflow 节点 `params.definitionId == child_id`），再补上名称便于提示。
fn referencing_workflows(
    conn: &rusqlite::Connection,
    child_id: &str,
) -> Result<Vec<(String, String)>, AppError> {
    let ids = crate::commands::cloud_sync::workflows_referencing(conn, child_id)?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let defs = workflow::list_definitions(conn)?;
    let name_of: std::collections::HashMap<&str, &str> = defs
        .iter()
        .map(|d| (d.id.as_str(), d.name.as_str()))
        .collect();
    Ok(ids
        .into_iter()
        .map(|id| {
            let name = name_of.get(id.as_str()).copied().unwrap_or("").to_string();
            (id, name)
        })
        .collect())
}

/// 用户主动删除前的引用守卫：被其他工作流引用时拒绝删除。
///
/// **注意**：内部 / 自动化删除路径（云同步远端删除传播、市场重装的旧定义清理）**不经过**
/// 此守卫。批次 2 起那些路径已分派：云同步远端删除 → [`workflow::soft_delete_definition`]
/// （同为软删除、进回收站）；市场重装覆盖 → [`workflow::hard_delete_definition`]
/// （内部替换语义，真删、不进回收站）。若在此拦这些路径，同步 / 安装会被历史引用卡死。
fn ensure_not_referenced(conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
    let referrers = referencing_workflows(conn, id).map_err(String::from)?;
    if referrers.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = referrers
        .iter()
        .map(|(_, n)| {
            if n.trim().is_empty() {
                "（未命名）".to_string()
            } else {
                n.clone()
            }
        })
        .collect();
    Err(format!(
        "该工作流被 {} 个工作流引用（{}），无法删除。请先解除引用。",
        referrers.len(),
        names.join("、")
    ))
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

    // 1. 获取工作流定义（已软删除 / 进回收站的定义拒绝启动，给出明确中文提示）

    let def = match workflow::get_definition(&conn, &workflow_id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
    {
        Some(def) if !workflow::is_definition_deleted(&conn, &workflow_id)
            .map_err(|e| AppError::Db(format!("查询失败: {}", e)))? =>
        {
            def
        }
        Some(def) => {
            return Err(AppError::InvalidInput(format!(
                "工作流「{}」已被删除，无法运行；请先从回收站恢复",
                def.name
            ))
            .into());
        }
        None => return Err(AppError::NotFound("工作流不存在".to_string()).into()),
    };

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
/// 删执行记录不改变定义的存在性，故 `api_usage_log` 保留（只有 [`workflow::hard_delete_definition`]
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

/// 定时任务数量的**技术保护上限**：为避免过多定时任务影响本地稳定性而设，不是功能 / 档位限制。
const MAX_SCHEDULES: usize = 1000;

/// 创建定时调度
///
/// Cron 表达式在此处**先校验再落库**：非法表达式若落库，调度器每次轮询都会判定"已到期"，
/// 于是每分钟重复触发（旧实现正是如此）。首次执行时间按表达式真实计算，不再固定为 now+60。
/// 数量只受 `MAX_SCHEDULES`（技术保护）约束，不再受会员档位配额限制。
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
    // 技术保护上限：定时任务数量超过 MAX_SCHEDULES 就拒绝，避免过多定时任务拖垮本地稳定性。
    // 保存定义时会先删掉本工作流的旧调度再重建，所以这里按「总数」校验是幂等安全的。
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM workflow_schedules", [], |r| r.get(0))
        .unwrap_or(0);
    if count as usize >= MAX_SCHEDULES {
        return Err(AppError::InvalidInput(format!(
            "定时任务数量已达系统保护上限（{} 个）。这是为避免过多定时任务影响本地稳定性而设的保护，请先清理不再使用的定时任务。",
            MAX_SCHEDULES
        ))
        .into());
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

// ── 组织共享工作流 payload（产品间协议） ──────────────────────

/// 共享 payload 的格式标识（写入 `format` 字段；导入端据此识别格式）。
pub(crate) const SHARED_WORKFLOW_FORMAT: &str = "pilotdesk.shared.workflow";

/// 共享 payload 版本号（仅作标识写入，导入端不做分支兼容）。
pub(crate) const SHARED_WORKFLOW_VERSION: u32 = 2;

/// 共享 payload 字符数上限（与平台 `payload` 约束一致）。
///
/// 超过该上限时**拒绝共享并给出明确中文提示**，绝不静默截断（截断会产生失配的子流引用）。
pub(crate) const SHARED_WORKFLOW_PAYLOAD_MAX_CHARS: usize = 256 * 1024;

/// 共享工作流 payload 的单条成员。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWorkflowEntry {
    /// 工作流稳定 ID（`workflow_definitions.id`，UUIDv4）。
    pub id: String,
    /// 工作流名称（仅供展示，不参与引用解析）。
    pub name: String,
    /// 导出结构（Subflow 节点参数里保留 `params.definitionId`，即被引用工作流的稳定 ID）。
    pub definition: ExportWorkflowDefinition,
}

/// 共享工作流 payload —— 统一 `workflows` 数组（含主工作流）：
///
/// ```json
/// {
///   "format": "pilotdesk.shared.workflow",
///   "version": 2,
///   "mainId": "<主工作流稳定 ID>",
///   "workflows": [ { "id": "<稳定 ID>", "name": "名称", "definition": { ...导出结构... } } ]
/// }
/// ```
///
/// 引用关系完全由各 `definition` 里 Subflow 节点的 `params.definitionId`（稳定 ID）表达，
/// 导入端据此直接就地更新 / 新建，因此同一份 payload 在不同实例上得到相同的 ID。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWorkflowPayload {
    /// 固定为 [`SHARED_WORKFLOW_FORMAT`]。
    pub format: String,
    /// 格式版本（仅标识，见 [`SHARED_WORKFLOW_VERSION`]）。
    pub version: u32,
    /// 主工作流的稳定 ID（对应 `workflows` 中某条目的 `id`）。
    pub main_id: String,
    /// 主工作流 + 全部子工作流（递归展开，含嵌套子流）。
    pub workflows: Vec<SharedWorkflowEntry>,
}

// ── 工作流文件包（目录 / zip，产品间协议，版本化）──────────────────────

/// 文件包清单文件名（放在包根目录，是工作流包的**唯一入口**）。
pub(crate) const WORKFLOW_PACKAGE_MANIFEST_FILE: &str = "manifest.json";
/// 包内工作流文件所在子目录（主/子工作流一视同仁平铺于此）。
pub(crate) const WORKFLOWS_SUBDIR: &str = "workflows";
/// 文件包格式标识（写入清单的 `format` 字段）。
pub(crate) const WORKFLOW_PACKAGE_FORMAT: &str = "pilotdesk.workflow.package";
/// 文件包当前版本。
pub(crate) const WORKFLOW_PACKAGE_VERSION: u32 = 1;

/// `manifest.json` 结构 —— 声明包内成员及其稳定 ID，是工作流包的**唯一入口**：
///
/// ```json
/// {
///   "format": "pilotdesk.workflow.package",
///   "version": 1,
///   "mainId": "<主工作流稳定 ID>",
///   "workflows": [ { "id": "<稳定 ID>", "name": "名称", "file": "workflows/名称.json" } ]
/// }
/// ```
///
/// - 入口只由 `mainId` 唯一表达（不再有 `isMain` 字段，避免两处说法不一致）：
///   其语义收窄为「导入完成后默认打开哪一个」；
/// - `file` 是**相对包根的相对路径**（如 `workflows/名称.json`）；
/// - 引用关系完全由各成员 JSON 里的 `params.definitionId`（稳定 ID）表达，文件名**不参与**解析。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowPackageManifest {
    format: String,
    version: u32,
    main_id: String,
    workflows: Vec<WorkflowPackageEntry>,
}

/// 包内单个成员：稳定 ID + 显示名 + 相对包根的相对路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowPackageEntry {
    id: String,
    name: String,
    /// 相对包根的相对路径（如 `workflows/名称.json`）。
    file: String,
}

impl From<workflow::WorkflowDefinition> for ExportWorkflowDefinition {
    /// 基础转换（不含子工作流处理；供 export_workflow 命令使用）

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
        }
    }
}

impl ExportWorkflowDefinition {
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

    /// 导入时重建完整 WorkflowDefinition（生成新 UUID）
    ///
    /// Subflow 节点的引用直接由 `params.definitionId`（稳定 ID）表达，原样保留。
    pub fn into_definition_with_subflows(self) -> workflow::WorkflowDefinition {
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
                        workflow::WorkflowNode {
                            id: String::new(),
                            node_type: en.node_type,
                            label: en.label,
                            plugin_id: None,
                            command_id: None,
                            params: en.params,
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

    /// 导入（不带子工作流映射，等价于 [`Self::into_definition_with_subflows`]）
    pub fn into_definition(self) -> workflow::WorkflowDefinition {
        self.into_definition_with_subflows()
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

/// 单个工作流的导出主体（供单个导出与批量导出共用）：
/// 在 `<dir_path>/<工作流名>/` 下写出**工作流包**——
/// 包根 `manifest.json`（唯一入口）+ `workflows/` 子目录下的全部工作流（含主工作流，
/// 一视同仁平铺），文件名=工作流原名（重名时依次加 `(2)`、`(3)` 序号；文件名不参与引用解析）。
///
/// 引用一律由各 JSON 内的 `params.definitionId`（稳定 ID）表达，不再替换为 refCode。
/// 清单里不再有 `isMain`，入口只由 `mainId` 表达（语义：导入后默认打开哪一个）。
fn write_workflow_export(
    conn: &rusqlite::Connection,
    def: workflow::WorkflowDefinition,
    dir_path: &str,
) -> Result<(), String> {
    // 自动创建以工作流名称命名的子文件夹
    let workflow_dir = std::path::Path::new(dir_path).join(&def.name);
    // 全部工作流（含主工作流）都放在 workflows/ 子目录下，文件名不区分主/子
    let workflows_dir = workflow_dir.join(WORKFLOWS_SUBDIR);
    std::fs::create_dir_all(&workflows_dir)
        .map_err(|e| AppError::Io(format!("创建文件夹失败: {}", e)))?;

    // 递归收集子工作流（稳定 ID 引用，`definitionId` 原样保留）
    let subflows = collect_subflow_definitions(conn, &def)?;

    let mut used_files: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entries: Vec<WorkflowPackageEntry> = Vec::new();

    // 主工作流：与子工作流一视同仁，文件名=原名（无 `[主]` 前缀，重名仅加序号）
    let main_file = unique_workflow_file_name(&def.name, &mut used_files);
    let main_export: ExportWorkflowDefinition = def.clone().into();
    entries.push(WorkflowPackageEntry {
        id: def.id.clone(),
        name: def.name.clone(),
        file: format!("{}/{}", WORKFLOWS_SUBDIR, main_file),
    });
    write_export_file(&workflows_dir, &main_file, &main_export)?;

    // 子工作流：同样放在 workflows/ 下，文件名=原名（重名时仅文件名加序号）
    for sub in subflows {
        let file_name = unique_workflow_file_name(&sub.name, &mut used_files);
        let sub_export: ExportWorkflowDefinition = sub.clone().into();
        entries.push(WorkflowPackageEntry {
            id: sub.id.clone(),
            name: sub.name.clone(),
            file: format!("{}/{}", WORKFLOWS_SUBDIR, file_name),
        });
        write_export_file(&workflows_dir, &file_name, &sub_export)?;
    }

    // 清单：入口只由 mainId 表达；引用完全由各 JSON 内的 definitionId 表达，文件名不参与解析
    let manifest = WorkflowPackageManifest {
        format: WORKFLOW_PACKAGE_FORMAT.to_string(),
        version: WORKFLOW_PACKAGE_VERSION,
        main_id: def.id,
        workflows: entries,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Json(format!("序列化失败: {}", e)))?;
    std::fs::write(
        workflow_dir.join(WORKFLOW_PACKAGE_MANIFEST_FILE),
        manifest_json,
    )
    .map_err(|e| AppError::Io(format!("写入清单失败: {}", e)))?;

    Ok(())
}

/// 写出一个工作流成员文件（`ExportWorkflowDefinition` JSON）。
fn write_export_file(
    dir: &std::path::Path,
    file_name: &str,
    def: &ExportWorkflowDefinition,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(def)
        .map_err(|e| AppError::Json(format!("序列化失败: {}", e)))?;
    std::fs::write(dir.join(file_name), json)
        .map_err(|e| AppError::Io(format!("写入文件失败: {}", e)).to_string())
}

/// 生成包内不冲突的文件名：`名称.json`；重名时依次 `名称(2).json`、`名称(3).json`…
/// （仅影响文件名，引用不依赖文件名）。
fn unique_workflow_file_name(name: &str, used: &mut std::collections::HashSet<String>) -> String {
    let base = format!("{}.json", name);
    if used.insert(base.clone()) {
        return base;
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{}({}).json", name, n);
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// 递归收集某工作流以 Subflow 节点引用的全部子工作流定义（按 `definitionId` 去重，含嵌套）。
///
/// 新格式（稳定 ID）导出用：节点 `params.definitionId` 原样保留、不再替换为 `refCode`，
/// 因此这里只需按稳定 ID 收集定义本身。被引用定义缺失时报错（与老导出行为一致）。
fn collect_subflow_definitions(
    conn: &rusqlite::Connection,
    root: &workflow::WorkflowDefinition,
) -> Result<Vec<workflow::WorkflowDefinition>, AppError> {
    let mut out = Vec::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    collect_subflow_definitions_recursive(conn, root, &mut out, &mut visited)?;
    Ok(out)
}

fn collect_subflow_definitions_recursive(
    conn: &rusqlite::Connection,
    def: &workflow::WorkflowDefinition,
    out: &mut Vec<workflow::WorkflowDefinition>,
    visited: &mut std::collections::HashSet<String>,
) -> Result<(), AppError> {
    for stage in &def.stages {
        for node in &stage.nodes {
            if node.node_type != workflow::WorkflowNodeType::Subflow {
                continue;
            }
            let Some(sub_id) = node
                .params
                .as_ref()
                .and_then(|p| p.get("definitionId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            if !visited.insert(sub_id.to_string()) {
                continue;
            }
            let sub = workflow::get_definition(conn, sub_id)
                .map_err(|e| AppError::Db(format!("加载子工作流失败: {}", e)))?
                .ok_or_else(|| AppError::NotFound(format!("子工作流定义不存在: {}", sub_id)))?;
            collect_subflow_definitions_recursive(conn, &sub, out, visited)?;
            out.push(sub);
        }
    }
    Ok(())
}

/// 从文件导入工作流（支持新文件包 / 单文件 JSON）。
///
/// 返回结果含 `missingDependencies`：单文件导入时引用但本地不存在的子工作流描述，供 UI 提示
/// （不阻断导入）。
#[tauri::command]
pub fn import_workflow_from_file(
    state: tauri::State<'_, crate::DbState>,
    file_path: String,
) -> Result<ImportedWorkflowFile, String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let outcome = import_workflow_from_file_with_conn(&conn, &file_path)?;
    Ok(ImportedWorkflowFile {
        id: outcome.definition.id,
        name: outcome.definition.name,
        subflow_count: outcome.subflow_count,
        missing_dependencies: outcome.missing_dependencies,
        skipped: outcome.skipped,
        members: outcome.members,
    })
}

/// 文件导入结果（返回给前端的结构）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedWorkflowFile {
    /// 导入后主工作流的稳定 ID。
    pub id: String,
    /// 主工作流名称。
    pub name: String,
    /// 随主流程一并导入的子工作流数量（0 表示无子流）。
    pub subflow_count: usize,
    /// 缺失依赖描述（单文件导入且引用不存在时非空；名称缺失时用 ID 前 8 位）。
    pub missing_dependencies: Vec<String>,
    /// 因与本地内容相同而**跳过未写入**的成员数（导入幂等去重，供 UI 展示）。
    pub skipped: usize,
    /// 逐成员的导入结果明细（供 UI 展示「未新建」的覆盖 / 跳过提示）。
    pub members: Vec<ImportMemberOutcome>,
}

/// 单个成员的导入动作：用户有知情权——凡「未新建」（覆盖 / 跳过）都应可见。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportMemberAction {
    /// 本地无同 ID → 新建。
    Created,
    /// 本地有同 ID 且内容不同 → 就地覆盖（覆盖前已把本地版本存为快照）。
    Updated,
    /// 本地有同 ID 且内容相同 → 跳过（不写、不建快照）。
    Skipped,
}

/// 单个成员的导入结果明细。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportMemberOutcome {
    /// 成员的稳定 ID。
    pub id: String,
    /// 成员名称（用于提示文案）。
    pub name: String,
    /// 导入动作（新建 / 覆盖 / 跳过）。
    pub action: ImportMemberAction,
    /// 导入后被多少个其它工作流引用（用于提示「被引用的子流被更新了」）；无引用为 0。
    pub referencing_count: usize,
}

/// 导入主体（连接维度实现，文件系统入口）的返回结构。
#[derive(Debug)]
pub(crate) struct WorkflowFileImport {
    pub definition: workflow::WorkflowDefinition,
    pub subflow_count: usize,
    pub missing_dependencies: Vec<String>,
    pub skipped: usize,
    pub members: Vec<ImportMemberOutcome>,
}

/// 共享 payload 导入结果（含逐成员明细），供组织共享导入与 UI 复用。
pub(crate) struct SharedWorkflowImport {
    pub definition: workflow::WorkflowDefinition,
    pub subflow_count: usize,
    pub members: Vec<ImportMemberOutcome>,
}

/// 导入主体（连接维度实现，文件系统入口）。
///
/// 识别顺序：
/// 1. **工作流包**：选中的是 `manifest.json`，或该文件所在目录 / 其父目录存在 `manifest.json`
///    （向上最多查两级）→ 按清单逐个以稳定 ID 就地更新 / 新建（引用直接由 `definitionId` 解析）。
/// 2. **单文件 JSON**：按内容导入；引用缺失不阻断，仅回报给 UI。
///
/// 拆出来是为了让市场安装复用同一套收集逻辑：安装要在**同一个连接里**连续完成
/// "先删旧定义、再导入新的一份"（见 utils/market.rs 的 workflow_market_install）。
pub(crate) fn import_workflow_from_file_with_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
) -> Result<WorkflowFileImport, String> {
    let path = std::path::Path::new(file_path);

    // 1. 工作流包：定位包根清单（选中清单本身 / 同目录 / 父目录，向上最多两级）
    if let Some(manifest_path) = locate_package_manifest(path) {
        if let Some(outcome) = import_workflow_package(conn, &manifest_path)? {
            return Ok(outcome);
        }
    }

    // 2. 单文件：按内容导入，引用按 definitionId 直接解析；
    //    本地缺失的依赖不阻断，仅回报给 UI。
    let main_json = std::fs::read_to_string(file_path)
        .map_err(|e| AppError::Io(format!("读取文件失败: {}", e)))?;
    let main_export: ExportWorkflowDefinition = serde_json::from_str(&main_json)
        .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;
    let missing_dependencies = collect_missing_subflow_dependencies(conn, &main_export);
    let definition = main_export.into_definition();
    workflow::create_definition(conn, &definition)
        .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
    // 单文件导入按内容新建（无稳定 ID），动作恒为「新建」
    let members = import_member_outcomes(
        conn,
        vec![(
            definition.id.clone(),
            definition.name.clone(),
            ImportMemberAction::Created,
        )],
    )?;
    Ok(WorkflowFileImport {
        definition,
        subflow_count: 0,
        missing_dependencies,
        skipped: 0,
        members,
    })
}

/// 定位工作流包的清单路径（`file_path` 为调用方选中的文件）。
///
/// 规则（向上最多两级）：
/// 1. 选中的文件本身就是 `manifest.json` → 直接用；
/// 2. 否则看该文件**所在目录**是否有 `manifest.json`（包内工作流通常在同一目录）；
/// 3. 再看其**父目录**是否有 `manifest.json`（新布局：工作流在 `<包>/workflows/` 下，
///    清单在上一级 `<包>/`）。
///
/// 返回 `None` 表示未找到包清单，调用方按单文件处理。
fn locate_package_manifest(file_path: &std::path::Path) -> Option<std::path::PathBuf> {
    if file_path.file_name().and_then(|n| n.to_str()) == Some(WORKFLOW_PACKAGE_MANIFEST_FILE) {
        return Some(file_path.to_path_buf());
    }
    let mut dir = file_path.parent();
    // 最多向上查两级：当前文件所在目录 + 其父目录
    for _ in 0..2 {
        let d = dir?;
        let candidate = d.join(WORKFLOW_PACKAGE_MANIFEST_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// 解析包内成员的相对路径：必须是相对包根的**安全相对路径**。
///
/// 拒绝绝对路径、`..`、根前缀等一切可能越出包外的路径段（防目录穿越）。
fn resolve_package_member(
    root: &std::path::Path,
    rel: &str,
) -> Result<std::path::PathBuf, String> {
    use std::path::Component;
    let rel_path = std::path::Path::new(rel);
    let safe = !rel.is_empty()
        && rel_path
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if !safe {
        return Err(AppError::InvalidInput(format!(
            "工作流包成员路径非法（必须是包内相对路径，且不得为绝对路径或包含 `..`）: {}",
            rel
        ))
        .into());
    }
    Ok(root.join(rel_path))
}

/// 按工作流包导入：解析 `manifest.json`，逐成员以**稳定 ID** 就地更新 / 新建。
///
/// 返回 `Ok(None)` 表示这不是一个可识别的工作流包（清单缺失 / 非本格式），调用方按单文件处理。
fn import_workflow_package(
    conn: &rusqlite::Connection,
    manifest_path: &std::path::Path,
) -> Result<Option<WorkflowFileImport>, String> {
    let dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let raw = std::fs::read_to_string(manifest_path)
        .map_err(|e| AppError::Io(format!("读取清单失败: {}", e)))?;
    let manifest: WorkflowPackageManifest = match serde_json::from_str(&raw) {
        Ok(m) => m,
        // 不是本格式清单 → 交由调用方按单文件处理
        Err(_) => return Ok(None),
    };
    if manifest.format != WORKFLOW_PACKAGE_FORMAT {
        return Ok(None);
    }

    let mut main_def: Option<workflow::WorkflowDefinition> = None;
    let mut subflow_count = 0usize;
    // 逐成员记录 (id, name, action)，待全部落库后再统计引用数
    let mut raw_members: Vec<(String, String, ImportMemberAction)> = Vec::new();
    for entry in &manifest.workflows {
        // 防目录穿越：成员路径必须是包内安全相对路径（拒绝 `..` / 绝对路径）
        let member_path = resolve_package_member(dir, &entry.file)?;
        let json = std::fs::read_to_string(&member_path)
            .map_err(|e| AppError::Io(format!("读取包内文件「{}」失败: {}", entry.file, e)))?;
        let export: ExportWorkflowDefinition = serde_json::from_str(&json)
            .map_err(|e| AppError::Json(format!("解析包内文件「{}」失败: {}", entry.file, e)))?;
        // 引用直接由 definitionId 表达，原样保留
        let mut def = export.into_definition_with_subflows();
        // 就地更新：本地已有同 ID → 复用该 id 覆盖（覆盖前先备份本地版本）；不存在 → 沿用该 id 新建；
        // 内容与本地相同 → 跳过（不写、不建快照）。
        let action = store_main_definition(
            conn,
            &mut def,
            Some(&entry.id),
            workflow::VERSION_ORIGIN_IMPORT,
        )?;
        raw_members.push((def.id.clone(), def.name.clone(), action));
        // 入口只由 mainId 表达（不再有 isMain 字段）
        if entry.id == manifest.main_id {
            main_def = Some(def);
        } else {
            subflow_count += 1;
        }
    }

    let definition = main_def.ok_or_else(|| {
        AppError::Json("工作流包缺少主工作流（manifest.mainId 未匹配任何成员）".to_string())
    })?;
    // skipped 由逐成员明细派生（不另设计数器，避免两份事实）
    let skipped = raw_members
        .iter()
        .filter(|(_, _, action)| *action == ImportMemberAction::Skipped)
        .count();
    let members = import_member_outcomes(conn, raw_members)?;
    Ok(Some(WorkflowFileImport {
        definition,
        subflow_count,
        missing_dependencies: Vec::new(),
        skipped,
        members,
    }))
}

/// 单文件导入的缺失依赖检查：列出被引用、但本地不存在（或读取失败）的子工作流。
///
/// 单文件里没有子工作流的名称信息（名称只存在于被引用定义本身），故以 ID 前 8 位标识。
fn collect_missing_subflow_dependencies(
    conn: &rusqlite::Connection,
    export: &ExportWorkflowDefinition,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for stage in &export.stages {
        for node in &stage.nodes {
            if node.node_type != workflow::WorkflowNodeType::Subflow {
                continue;
            }
            let Some(id) = node
                .params
                .as_ref()
                .and_then(|p| p.get("definitionId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            if !seen.insert(id.to_string()) {
                continue;
            }
            if !matches!(workflow::get_definition(conn, id), Ok(Some(_))) {
                out.push(format!("未知子工作流（ID {}）", short_id(id)));
            }
        }
    }
    out
}

/// 取 ID 前 8 位用于提示（不足 8 位则原样返回）。
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// 主工作流落库：
/// - `override_id` 为 `None` → 一律新建（普通导入，保持既有行为）；
/// - 为 `Some(id)` → 改 id 后 upsert（存在则更新、不存在则新建）——云同步 pull 就地覆盖用，
///   目的是保持同一 object_key，而不是每轮同步都新建副本。
///
/// **覆盖本地已有工作流内容前先打版本快照**（`origin` 区分来源：`import` / `cloud_sync`）；
/// 若与本地内容规范化后完全相同 → 直接跳过（不写、不建快照）。
///
/// 返回本次落库动作（[`ImportMemberAction`]：新建 / 覆盖 / 跳过）。
fn store_main_definition(
    conn: &rusqlite::Connection,
    def: &mut workflow::WorkflowDefinition,
    override_id: Option<&str>,
    origin: &str,
) -> Result<ImportMemberAction, AppError> {
    match override_id {
        Some(id) => {
            def.id = id.to_string();
            match workflow::get_definition(conn, id)
                .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
            {
                Some(existing) => {
                    // 无改动 → 跳过（本地已存在相同内容，不写、不建快照）
                    if definitions_content_equal(&existing, def) {
                        return Ok(ImportMemberAction::Skipped);
                    }
                    // 有改动 → 先备份本地版本，再接受覆盖
                    if let Err(e) = workflow::snapshot_before_overwrite(conn, id, origin) {
                        log::warn!("[Workflow] 覆盖前保存版本快照失败 {}：{}", id, e);
                    }
                    workflow::update_definition(conn, def)
                        .map_err(|e| AppError::Db(format!("更新工作流失败: {}", e)))?;
                    Ok(ImportMemberAction::Updated)
                }
                None => {
                    workflow::create_definition(conn, def)
                        .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
                    Ok(ImportMemberAction::Created)
                }
            }
        }
        None => {
            workflow::create_definition(conn, def)
                .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
            Ok(ImportMemberAction::Created)
        }
    }
}

/// 把逐成员的 `(id, name, action)` 汇总为带引用计数的明细。
///
/// 引用计数在**全部成员落库后**统计，才能反映导入后的最终引用关系
/// （例如后落库的主流程引用先落库的子流）。
fn import_member_outcomes(
    conn: &rusqlite::Connection,
    members: Vec<(String, String, ImportMemberAction)>,
) -> Result<Vec<ImportMemberOutcome>, AppError> {
    let mut out = Vec::with_capacity(members.len());
    for (id, name, action) in members {
        let referencing_count = referencing_workflows(conn, &id)?.len();
        out.push(ImportMemberOutcome {
            id,
            name,
            action,
            referencing_count,
        });
    }
    Ok(out)
}

/// 两个工作流定义的「内容」是否等价。
///
/// 走导出结构比较（[`ExportWorkflowDefinition`] 已把阶段/节点/边的随机 UUID 归一为短标识），
/// 再经 [`workflow::normalized_content_hash`] 剔除时间戳 / 版本号等易变字段——因此「同一份内容」
/// 在本地重新解析（生成新 UUID）后也能判为等价。
fn definitions_content_equal(
    a: &workflow::WorkflowDefinition,
    b: &workflow::WorkflowDefinition,
) -> bool {
    let va = serde_json::to_value(ExportWorkflowDefinition::from(a.clone())).unwrap_or_default();
    let vb = serde_json::to_value(ExportWorkflowDefinition::from(b.clone())).unwrap_or_default();
    workflow::normalized_content_hash(&va) == workflow::normalized_content_hash(&vb)
}

/// 打包「工作流共享 payload」：主工作流 + 其全部子工作流（递归、含嵌套），统一使用
/// 带 `format` 字段的 [`SharedWorkflowPayload`]（**即使没有子流也带单元素 `workflows`**，
/// 便于导入端走统一解析路径）。引用一律由 `params.definitionId`（稳定 ID）表达。
/// 返回 `(主工作流名, payload JSON)`。
///
/// 超过 [`SHARED_WORKFLOW_PAYLOAD_MAX_CHARS`] 时返回明确中文错误，**绝不截断**。
pub(crate) fn build_workflow_export_json(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<(String, String), AppError> {
    let def = workflow::get_definition(conn, id)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
        .ok_or_else(|| AppError::NotFound("工作流不存在".to_string()))?;
    let name = def.name.clone();
    let main_id = def.id.clone();

    // 主 + 递归子流：稳定 ID 引用（节点 `definitionId` 原样保留，不做 refCode 重映射）
    let subflows = collect_subflow_definitions(conn, &def)?;
    let mut workflows: Vec<SharedWorkflowEntry> = Vec::with_capacity(subflows.len() + 1);
    workflows.push(SharedWorkflowEntry {
        id: def.id.clone(),
        name: def.name.clone(),
        definition: ExportWorkflowDefinition::from(def),
    });
    for sub in subflows {
        workflows.push(SharedWorkflowEntry {
            id: sub.id.clone(),
            name: sub.name.clone(),
            definition: ExportWorkflowDefinition::from(sub),
        });
    }

    let payload = SharedWorkflowPayload {
        format: SHARED_WORKFLOW_FORMAT.to_string(),
        version: SHARED_WORKFLOW_VERSION,
        main_id,
        workflows,
    };
    let json = serde_json::to_string(&payload)
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

/// 解析共享 payload：校验 `format` 标识后按 [`SharedWorkflowPayload`] 反序列化。
///
/// 仅支持当前唯一格式；无法识别的 payload 返回明确中文错误（不静默降级）。
fn parse_shared_workflow_payload(payload: &str) -> Result<SharedWorkflowPayload, AppError> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;
    if value.get("format").and_then(|v| v.as_str()) != Some(SHARED_WORKFLOW_FORMAT) {
        return Err(AppError::InvalidInput(
            "无法解析共享工作流：缺少 format 标识或格式不受支持".to_string(),
        ));
    }
    serde_json::from_value(value)
        .map_err(|e| AppError::Json(format!("共享工作流解析失败: {}", e)))
}

/// 导入共享 payload：逐个工作流以**稳定 ID** 就地更新 / 新建（复用该 id，不新建副本）。
///
/// 引用直接由各定义的 `params.definitionId` 表达。覆盖本地已有内容前先备份版本
/// （origin = `import`）；内容相同则跳过。
///
/// 返回主工作流定义、子工作流数量、跳过数量与逐成员明细。
fn import_shared_payload(
    conn: &rusqlite::Connection,
    payload: SharedWorkflowPayload,
) -> Result<SharedWorkflowImport, AppError> {
    let main_id = payload.main_id.clone();
    let mut main_def: Option<workflow::WorkflowDefinition> = None;
    let mut subflow_count = 0usize;
    // 逐成员记录 (id, name, action)，待全部落库后再统计引用数
    let mut raw_members: Vec<(String, String, ImportMemberAction)> = Vec::new();
    for entry in payload.workflows {
        let mut def = entry.definition.into_definition_with_subflows();
        let action = store_main_definition(
            conn,
            &mut def,
            Some(&entry.id),
            workflow::VERSION_ORIGIN_IMPORT,
        )?;
        raw_members.push((def.id.clone(), def.name.clone(), action));
        if entry.id == main_id {
            main_def = Some(def);
        } else {
            subflow_count += 1;
        }
    }
    let definition = main_def.ok_or_else(|| {
        AppError::Json("共享工作流缺少主工作流（mainId 未匹配任何成员）".to_string())
    })?;
    let members = import_member_outcomes(conn, raw_members)?;
    Ok(SharedWorkflowImport {
        definition,
        subflow_count,
        members,
    })
}

/// 从共享 payload 导入一份本地工作流（与 [`build_workflow_export_json`] 成对，供组织共享空间导入复用）。
///
/// 按稳定 ID 逐个**就地更新 / 新建**（本地已存在同 ID 的 → 复用该 id 覆盖，不产生副本）。
/// 覆盖本地已有内容前先备份版本（origin = `import`）；内容相同则跳过。
/// 返回结果含逐成员明细（供 UI 展示覆盖 / 跳过提示）。
pub(crate) fn import_workflow_from_json_with_conn(
    conn: &rusqlite::Connection,
    json: &str,
) -> Result<SharedWorkflowImport, AppError> {
    let payload = parse_shared_workflow_payload(json)?;
    import_shared_payload(conn, payload)
}

/// 取共享 payload 的**主工作流定义**（用于内容比对 / 云同步冲突留档）。
///
/// 取 `mainId` 命中的成员。
pub(crate) fn payload_main_definition(
    payload: &str,
) -> Result<workflow::WorkflowDefinition, AppError> {
    let p = parse_shared_workflow_payload(payload)?;
    let main_id = p.main_id.clone();
    let entry = p
        .workflows
        .into_iter()
        .find(|e| e.id == main_id)
        .ok_or_else(|| {
            AppError::Json("共享工作流缺少主工作流（mainId 未匹配任何成员）".to_string())
        })?;
    Ok(entry.definition.into_definition_with_subflows())
}

/// 远端 payload 的主工作流内容是否与本地当前内容等价（云同步 pull 的「无改动 → 跳过」判据）。
pub(crate) fn payload_matches_local_definition(
    conn: &rusqlite::Connection,
    key: &str,
    payload: &str,
) -> Result<bool, AppError> {
    let Some(local) = workflow::get_active_definition(conn, key)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
    else {
        return Ok(false);
    };
    let incoming = payload_main_definition(payload)?;
    Ok(definitions_content_equal(&local, &incoming))
}

/// 就地把一份共享 payload 应用到指定 object_key 的工作流上——云同步 pull 用。
///
/// 按各成员稳定 ID 就地 upsert；主工作流归一到 `target_id`（object_key），
/// 不每轮生成新子流 id，故**无需清理旧子流**。
///
/// **注意**：本路径为后台同步，`store_main_definition` 返回的逐成员动作明细在此**被丢弃**，
/// 不冒泡成 UI 弹窗（同步仅在冲突时已有提示）；解析 / 落库逻辑仍与导入共用，行为不变。
///
/// 返回 `(主工作流定义, 实际导入的子工作流数量)`。
pub(crate) fn apply_workflow_bundle_in_place(
    conn: &rusqlite::Connection,
    target_id: &str,
    json: &str,
) -> Result<(workflow::WorkflowDefinition, usize), AppError> {
    let payload = parse_shared_workflow_payload(json)?;
    let original_main_id = payload.main_id.clone();
    let mut main_def: Option<workflow::WorkflowDefinition> = None;
    let mut subflow_count = 0usize;
    for entry in payload.workflows {
        let mut def = entry.definition.into_definition_with_subflows();
        if entry.id == original_main_id {
            // 主工作流归一到 object_key，保证跨设备 key 稳定
            store_main_definition(
                conn,
                &mut def,
                Some(target_id),
                workflow::VERSION_ORIGIN_CLOUD_SYNC,
            )?;
            main_def = Some(def);
        } else {
            store_main_definition(
                conn,
                &mut def,
                Some(&entry.id),
                workflow::VERSION_ORIGIN_CLOUD_SYNC,
            )?;
            subflow_count += 1;
        }
    }
    let definition = main_def.ok_or_else(|| {
        AppError::Json("共享工作流缺少主工作流（mainId 未匹配任何成员）".to_string())
    })?;
    Ok((definition, subflow_count))
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
        .unwrap_or_else(|_| "10".to_string());
    Ok(value.parse::<usize>().unwrap_or(10))
}

/// 设置工作流最大并发数
///
/// 并发数是客户端自行调节的本地能力：默认 10，技术硬上限 30（避免过高明显占用本机资源）。
#[tauri::command]
pub fn set_workflow_max_concurrency(
    state: tauri::State<'_, crate::DbState>,
    max_concurrency: usize,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    // 固定范围 1..=30（技术硬上限），不再按会员档位配额夹取。
    let clamped = (max_concurrency as i64).clamp(1, 30);
    conn.execute(

        "INSERT OR REPLACE INTO app_settings (key, value, updated_at) VALUES ('workflow_max_concurrency', ?1, ?2)",
        rusqlite::params![clamped.to_string(), crate::utils::now()],
    )
    .map_err(|e| AppError::Db(format!("保存失败: {}", e)))?;
    Ok(())
}

/// 设置子工作流最大嵌套深度
///
/// 嵌套深度是客户端自行调节的本地能力：技术硬上限 10。
#[tauri::command]
pub fn set_workflow_max_subflow_depth(
    state: tauri::State<'_, crate::DbState>,
    max_depth: i64,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    // 固定范围 1..=10（技术硬上限），不再按会员档位配额夹取。
    let clamped = max_depth.clamp(1, 10);
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
    crate::workflow::save_workflow_version(
        &conn,
        &workflow_id,
        &snapshot,
        crate::workflow::VERSION_ORIGIN_MANUAL,
    )
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

    /// 新格式 v2 共享 payload 往返：导出 → 导入，工作流 id 保持不变、引用直接解析到同一 id。
    #[test]
    fn shared_payload_v2_roundtrip_preserves_ids() {
        let conn = test_conn();
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();

        let (name, payload) = build_workflow_export_json(&conn, &main_id).unwrap();
        assert_eq!(name, "主流程");
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["format"], SHARED_WORKFLOW_FORMAT);
        assert_eq!(value["version"], 2);
        assert_eq!(value["mainId"], main_id.as_str());
        assert_eq!(value["workflows"].as_array().unwrap().len(), 2);

        // 主工作流的 Subflow 节点保留 definitionId（稳定 ID），不再出现 refCode
        let main_entry = value["workflows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == main_id.as_str())
            .unwrap();
        let sub_node = main_entry["definition"]["stages"][0]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["type"] == "subflow")
            .unwrap();
        assert_eq!(sub_node["params"]["definitionId"], sub_id.as_str());
        assert!(
            sub_node["params"].get("refCode").is_none(),
            "新格式不应输出 refCode"
        );

        // 导入到干净库：id 保持不变，引用解析到同一子流 id
        let conn2 = test_conn();
        let outcome = import_workflow_from_json_with_conn(&conn2, &payload).unwrap();
        let imported = outcome.definition;
        let count = outcome.subflow_count;
        assert_eq!(count, 1);
        assert_eq!(imported.id, main_id, "主工作流 id 应保持不变");
        assert!(workflow::get_definition(&conn2, &sub_id).unwrap().is_some());
        let ref_id = imported
            .stages
            .iter()
            .flat_map(|s| s.nodes.iter())
            .find(|n| n.node_type == workflow::WorkflowNodeType::Subflow)
            .and_then(|n| n.params.as_ref()?.get("definitionId")?.as_str())
            .map(str::to_string)
            .expect("导入后 Subflow 节点应有 definitionId");
        assert_eq!(ref_id, sub_id, "引用应直接指向稳定子流 id");
    }

    /// v2 就地更新：本地已有同 ID → 导入不新建记录，仅覆盖内容。
    #[test]
    fn shared_payload_v2_updates_existing_in_place() {
        let conn = test_conn();
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();
        let (_name, payload) = build_workflow_export_json(&conn, &main_id).unwrap();
        let before = workflow::list_definitions(&conn).unwrap().len();

        // 本地改名后导入同一 payload → 应复用 id 覆盖回原内容，且不新增记录
        let mut d = workflow::get_definition(&conn, &main_id).unwrap().unwrap();
        d.name = "本地改名".to_string();
        workflow::update_definition(&conn, &d).unwrap();

        let outcome = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        let imported = outcome.definition;
        assert_eq!(outcome.subflow_count, 1);
        assert_eq!(imported.id, main_id);
        assert_eq!(
            workflow::list_definitions(&conn).unwrap().len(),
            before,
            "就地更新不应新增记录"
        );
        assert_eq!(
            workflow::get_definition(&conn, &main_id)
                .unwrap()
                .unwrap()
                .name,
            "主流程"
        );
        assert_eq!(
            workflow::get_definition(&conn, &sub_id).unwrap().unwrap().name,
            "子流程A"
        );
    }

    /// 无子流时 payload 仍为 v2（`workflows` 单元素），且能被正常解析导入。
    #[test]
    fn shared_payload_v2_without_subflows() {
        let conn = test_conn();
        let id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&id, "独立流程", None)).unwrap();
        let (_name, payload) = build_workflow_export_json(&conn, &id).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["version"], 2);
        assert_eq!(value["workflows"].as_array().unwrap().len(), 1);
        let outcome = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        assert_eq!(outcome.subflow_count, 0);
    }

    /// 工作流包往返：导出目录结构正确（manifest + workflows/ 平铺、无主/子前缀），
    /// 导入到干净库后 mainId 与各工作流 id、引用均保持不变。
    #[test]
    fn new_package_export_import_roundtrip() {
        let conn = test_conn();
        let sub_id = "11111111-1111-1111-1111-111111111111".to_string();
        workflow::create_definition(&conn, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = "22222222-2222-2222-2222-222222222222".to_string();
        workflow::create_definition(&conn, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();

        let root = temp_workflow_dir();
        let def = workflow::get_definition(&conn, &main_id).unwrap().unwrap();
        write_workflow_export(&conn, def, root.to_str().unwrap()).unwrap();

        // 目录结构：包根 manifest.json + workflows/ 下主/子一视同仁（原名、无 [主]/[子] 前缀）
        let pkg = root.join("主流程");
        assert!(pkg.join("manifest.json").is_file(), "应写出 manifest.json");
        assert!(
            pkg.join("workflows").join("主流程.json").is_file(),
            "主工作流应在 workflows/ 下、文件名为原名（无 [主] 前缀）"
        );
        assert!(
            pkg.join("workflows").join("子流程A.json").is_file(),
            "子工作流应在 workflows/ 下、文件名为原名（无 [子] 前缀）"
        );
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(pkg.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["format"], WORKFLOW_PACKAGE_FORMAT);
        assert_eq!(manifest["mainId"], main_id.as_str());
        let members = manifest["workflows"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        // file 一律是相对包根的相对路径；不再输出 isMain（入口只由 mainId 表达）
        for m in members {
            let file = m["file"].as_str().unwrap();
            assert!(file.starts_with("workflows/"), "file 应为相对路径: {}", file);
            assert!(m.get("isMain").is_none(), "不应再输出 isMain 字段");
        }
        let main_member = members
            .iter()
            .find(|m| m["id"] == main_id.as_str())
            .expect("清单应含主工作流成员");
        assert_eq!(main_member["file"], "workflows/主流程.json");

        // 导入到干净库：id 保持不变、引用直接指向稳定子流 id（选中包内子文件即可，
        // 导入端按「所在目录 / 父目录」向上找到包根 manifest.json）
        let conn2 = test_conn();
        let outcome = import_workflow_from_file_with_conn(
            &conn2,
            pkg.join("workflows").join("主流程.json").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(outcome.definition.id, main_id, "主工作流 id 应保持不变");
        assert_eq!(outcome.subflow_count, 1);
        assert!(outcome.missing_dependencies.is_empty());
        assert!(workflow::get_definition(&conn2, &sub_id).unwrap().is_some());
        let ref_id = outcome
            .definition
            .stages
            .iter()
            .flat_map(|s| s.nodes.iter())
            .find(|n| n.node_type == workflow::WorkflowNodeType::Subflow)
            .and_then(|n| n.params.as_ref()?.get("definitionId")?.as_str())
            .map(str::to_string)
            .expect("Subflow 节点应有 definitionId");
        assert_eq!(ref_id, sub_id, "引用应直接指向稳定子流 id");

        std::fs::remove_dir_all(&root).ok();
    }

    /// 新文件包就地更新：本地已有同 ID → 导入不新建记录，仅覆盖内容。
    #[test]
    fn new_package_import_updates_existing_in_place() {
        let conn = test_conn();
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();

        let root = temp_workflow_dir();
        let def = workflow::get_definition(&conn, &main_id).unwrap().unwrap();
        write_workflow_export(&conn, def, root.to_str().unwrap()).unwrap();
        let before = workflow::list_definitions(&conn).unwrap().len();

        // 本地改名后从包导入 → 复用同一 id 覆盖回原内容，不新增记录
        let mut d = workflow::get_definition(&conn, &main_id).unwrap().unwrap();
        d.name = "本地改名".to_string();
        workflow::update_definition(&conn, &d).unwrap();

        let outcome = import_workflow_from_file_with_conn(
            &conn,
            root.join("主流程")
                .join("workflows")
                .join("主流程.json")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome.definition.id, main_id);
        assert_eq!(outcome.subflow_count, 1);
        assert_eq!(
            workflow::list_definitions(&conn).unwrap().len(),
            before,
            "就地更新不应新增记录"
        );
        assert_eq!(
            workflow::get_definition(&conn, &main_id)
                .unwrap()
                .unwrap()
                .name,
            "主流程"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// 包导入明细：首次全为「新建」；本地改名后重导 → 该成员「覆盖」、其余「跳过」；
    /// 明细顺序与清单一致；被主流程引用的子流能拿到引用数；覆盖前留快照、跳过不建快照。
    #[test]
    fn package_import_member_actions_and_ordering() {
        // 源库：子流A（被主流程引用）+ 主流程
        let src = test_conn();
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&src, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = crate::utils::new_id();
        workflow::create_definition(&src, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();
        let root = temp_workflow_dir();
        let def = workflow::get_definition(&src, &main_id).unwrap().unwrap();
        write_workflow_export(&src, def, root.to_str().unwrap()).unwrap();
        let entry_file = root.join("主流程").join("workflows").join("主流程.json");

        // 1) 干净库首次导入 → 全部「新建」，顺序与清单一致（主流程在前）
        let conn = test_conn();
        let first =
            import_workflow_from_file_with_conn(&conn, entry_file.to_str().unwrap()).unwrap();
        assert_eq!(first.members.len(), 2);
        assert_eq!(first.members[0].id, main_id);
        assert_eq!(first.members[0].name, "主流程");
        assert_eq!(first.members[0].action, ImportMemberAction::Created);
        assert_eq!(first.members[0].referencing_count, 0, "主流程无引用方");
        assert_eq!(first.members[1].id, sub_id);
        assert_eq!(first.members[1].action, ImportMemberAction::Created);
        assert_eq!(first.members[1].referencing_count, 1, "子流被主流程引用");
        assert_eq!(first.skipped, 0);

        // 2) 本地把子流改名 → 重导：子流「覆盖」、主流程「跳过」
        let mut sub = workflow::get_definition(&conn, &sub_id).unwrap().unwrap();
        sub.name = "本地改名的子流".to_string();
        workflow::update_definition(&conn, &sub).unwrap();

        let second =
            import_workflow_from_file_with_conn(&conn, entry_file.to_str().unwrap()).unwrap();
        let by_id = |id: &str| second.members.iter().find(|m| m.id == id).unwrap();
        assert_eq!(by_id(&sub_id).action, ImportMemberAction::Updated);
        assert_eq!(by_id(&sub_id).name, "子流程A", "覆盖后名称为导入内容的名");
        assert_eq!(by_id(&sub_id).referencing_count, 1);
        assert_eq!(by_id(&main_id).action, ImportMemberAction::Skipped);
        assert_eq!(second.skipped, 1);

        // 保持既有语义：覆盖前备份本地版本（origin=import）；被跳过的成员不建快照
        let sub_versions = workflow::list_workflow_versions(&conn, &sub_id).unwrap();
        assert_eq!(sub_versions.len(), 1, "覆盖前应留 1 份本地版本快照");
        assert_eq!(sub_versions[0].origin, workflow::VERSION_ORIGIN_IMPORT);
        assert!(
            workflow::list_workflow_versions(&conn, &main_id)
                .unwrap()
                .is_empty(),
            "内容相同的成员跳过，不应建快照"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// 同一工作流包再次导入（内容未变）→ 全部「跳过」，且不建任何版本快照。
    #[test]
    fn package_reimport_unchanged_all_skipped() {
        let src = test_conn();
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&src, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = crate::utils::new_id();
        workflow::create_definition(&src, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();
        let root = temp_workflow_dir();
        let def = workflow::get_definition(&src, &main_id).unwrap().unwrap();
        write_workflow_export(&src, def, root.to_str().unwrap()).unwrap();
        let entry_file = root.join("主流程").join("workflows").join("主流程.json");

        let conn = test_conn();
        let first =
            import_workflow_from_file_with_conn(&conn, entry_file.to_str().unwrap()).unwrap();
        assert!(
            first
                .members
                .iter()
                .all(|m| m.action == ImportMemberAction::Created),
            "首次导入应全为「新建」: {:?}",
            first.members
        );

        // 同一包、内容未变 → 全部「跳过」
        let second =
            import_workflow_from_file_with_conn(&conn, entry_file.to_str().unwrap()).unwrap();
        assert_eq!(second.members.len(), 2);
        assert!(
            second
                .members
                .iter()
                .all(|m| m.action == ImportMemberAction::Skipped),
            "内容未变重导应全为「跳过」: {:?}",
            second.members
        );
        assert_eq!(second.skipped, 2, "skipped 应由逐成员明细派生");
        for id in [&main_id, &sub_id] {
            assert!(
                workflow::list_workflow_versions(&conn, id)
                    .unwrap()
                    .is_empty(),
                "跳过不应建版本快照: {}",
                id
            );
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// 改包内**主工作流**内容后重导 → 主流程「覆盖」、未变子流「跳过」；
    /// 且覆盖前主工作流已产生一条 origin=import 的本地版本备份（子流跳过不建快照）。
    #[test]
    fn package_import_main_changed_updates_and_snapshots() {
        let src = test_conn();
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&src, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = crate::utils::new_id();
        workflow::create_definition(&src, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();
        let root = temp_workflow_dir();
        let def = workflow::get_definition(&src, &main_id).unwrap().unwrap();
        write_workflow_export(&src, def, root.to_str().unwrap()).unwrap();
        let main_file = root
            .join("主流程")
            .join("workflows")
            .join("主流程.json");

        let conn = test_conn();
        import_workflow_from_file_with_conn(&conn, main_file.to_str().unwrap()).unwrap();

        // 只改包内主工作流的名称 → 主流程内容变化、子流未变
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&main_file).unwrap()).unwrap();
        v["name"] = serde_json::Value::String("主流程（新版）".to_string());
        std::fs::write(&main_file, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let outcome =
            import_workflow_from_file_with_conn(&conn, main_file.to_str().unwrap()).unwrap();
        let main_m = outcome.members.iter().find(|m| m.id == main_id).unwrap();
        assert_eq!(main_m.action, ImportMemberAction::Updated);
        assert_eq!(main_m.name, "主流程（新版）", "覆盖后应取导入内容的名");
        let sub_m = outcome.members.iter().find(|m| m.id == sub_id).unwrap();
        assert_eq!(sub_m.action, ImportMemberAction::Skipped, "未变的子流应跳过");

        // 覆盖前备份本地旧主流程（origin=import）；子流跳过不建快照
        let vs = workflow::list_workflow_versions(&conn, &main_id).unwrap();
        assert_eq!(vs.len(), 1, "主流程覆盖前应留 1 份本地版本快照");
        assert_eq!(vs[0].origin, workflow::VERSION_ORIGIN_IMPORT);
        let snap: workflow::WorkflowDefinition = serde_json::from_str(&vs[0].snapshot).unwrap();
        assert_eq!(snap.name, "主流程", "快照应是被覆盖前的本地旧内容");
        assert!(
            workflow::list_workflow_versions(&conn, &sub_id)
                .unwrap()
                .is_empty(),
            "跳过的子流不应建快照"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// 目录里没有 `manifest.json` → 按单文件导入（不报错、无子流、动作「新建」）。
    #[test]
    fn import_single_file_without_manifest_is_not_a_package() {
        let conn = test_conn();
        let root = temp_workflow_dir();
        let export = ExportWorkflowDefinition::from(make_share_def(
            "aaaaaaaa-1111-1111-1111-111111111111",
            "独立流程",
            None,
        ));
        let file = root.join("独立流程.json");
        std::fs::write(&file, serde_json::to_string_pretty(&export).unwrap()).unwrap();

        let outcome = import_workflow_from_file_with_conn(&conn, file.to_str().unwrap()).unwrap();
        assert_eq!(outcome.subflow_count, 0);
        assert!(outcome.missing_dependencies.is_empty());
        assert_eq!(outcome.members.len(), 1);
        assert_eq!(outcome.members[0].action, ImportMemberAction::Created);

        std::fs::remove_dir_all(&root).ok();
    }

    /// 包清单 `mainId` 未命中任何成员 → 明确中文错误（不静默降级）。
    #[test]
    fn package_import_errors_when_main_id_unmatched() {
        let conn = test_conn();
        let root = temp_workflow_dir();
        let pkg = root.join("缺主流程的包");
        std::fs::create_dir_all(&pkg).unwrap();
        let member_id = "88888888-8888-8888-8888-888888888888".to_string();
        let export = ExportWorkflowDefinition::from(make_share_def(&member_id, "成员", None));
        std::fs::write(
            pkg.join("成员.json"),
            serde_json::to_string_pretty(&export).unwrap(),
        )
        .unwrap();
        let manifest = serde_json::json!({
            "format": WORKFLOW_PACKAGE_FORMAT,
            "version": 1,
            "mainId": "99999999-9999-9999-9999-999999999999",
            "workflows": [
                { "id": member_id, "name": "成员", "file": "成员.json" },
            ],
        });
        std::fs::write(
            pkg.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let err = import_workflow_from_file_with_conn(
            &conn,
            pkg.join("manifest.json").to_str().unwrap(),
        )
        .unwrap_err();
        assert!(
            err.contains("主工作流") && err.contains("mainId"),
            "mainId 未命中应给出明确中文错误: {}",
            err
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// 共享 payload 导入返回逐成员明细：首次「新建」；本地改名后重导 → 该成员「覆盖」、其余「跳过」；
    /// 被引用子流的引用数正确。
    #[test]
    fn shared_payload_import_member_outcomes() {
        let src = test_conn();
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&src, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = crate::utils::new_id();
        workflow::create_definition(&src, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();
        let (_name, payload) = build_workflow_export_json(&src, &main_id).unwrap();

        let conn = test_conn();
        let first = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        assert_eq!(first.members.len(), 2);
        assert!(
            first
                .members
                .iter()
                .all(|m| m.action == ImportMemberAction::Created),
            "干净库首次导入应全为「新建」: {:?}",
            first.members
        );
        assert_eq!(
            first.members.iter().find(|m| m.id == sub_id).unwrap().referencing_count,
            1,
            "子流被主流程引用"
        );
        assert!(
            first.members.iter().all(|m| m.action != ImportMemberAction::Skipped),
            "首次导入不应有「跳过」"
        );

        // 本地改名后重导 → 子流覆盖、主流程跳过
        let mut d = workflow::get_definition(&conn, &sub_id).unwrap().unwrap();
        d.name = "本地改名".to_string();
        workflow::update_definition(&conn, &d).unwrap();

        let second = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        let skipped_count = second
            .members
            .iter()
            .filter(|m| m.action == ImportMemberAction::Skipped)
            .count();
        assert_eq!(skipped_count, 1);
        assert_eq!(
            second.members.iter().find(|m| m.id == sub_id).unwrap().action,
            ImportMemberAction::Updated
        );
        assert_eq!(
            second.members.iter().find(|m| m.id == main_id).unwrap().action,
            ImportMemberAction::Skipped
        );
    }

    /// 单文件导入：引用的子工作流本地不存在时不阻断，回报「缺失依赖」。
    #[test]
    fn single_file_import_reports_missing_dependencies() {
        let conn = test_conn();
        let missing_id = "33333333-3333-3333-3333-333333333333";
        let export = ExportWorkflowDefinition::from(make_share_def(
            "44444444-4444-4444-4444-444444444444",
            "孤立主流程",
            Some(missing_id),
        ));
        let root = temp_workflow_dir();
        let file = root.join("孤立主流程.json");
        std::fs::write(&file, serde_json::to_string_pretty(&export).unwrap()).unwrap();

        let outcome = import_workflow_from_file_with_conn(&conn, file.to_str().unwrap()).unwrap();
        assert_eq!(outcome.subflow_count, 0);
        assert_eq!(outcome.missing_dependencies.len(), 1, "应回报 1 个缺失依赖");
        assert!(
            outcome.missing_dependencies[0].contains(&short_id(missing_id)),
            "缺失依赖提示应含 ID 前 8 位: {:?}",
            outcome.missing_dependencies
        );
        // 不阻断：主工作流已导入
        assert!(workflow::get_definition(&conn, &outcome.definition.id)
            .unwrap()
            .is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    /// 引用检查（用户主动删除）：被引用的拒绝删除且错误含引用方名称；未被引用的可删。
    #[test]
    fn user_delete_guard_blocks_referenced_workflow() {
        let conn = test_conn();
        let sub_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&sub_id, "子流程A", None)).unwrap();
        let main_id = crate::utils::new_id();
        workflow::create_definition(&conn, &make_share_def(&main_id, "主流程", Some(&sub_id)))
            .unwrap();

        let err = ensure_not_referenced(&conn, &sub_id).unwrap_err();
        assert!(err.contains("无法删除"), "错误应说明无法删除: {}", err);
        assert!(err.contains("主流程"), "错误信息应含引用方名称: {}", err);
        assert!(err.contains('1'), "错误应含引用数量: {}", err);

        assert!(
            ensure_not_referenced(&conn, &main_id).is_ok(),
            "未被引用的工作流可删"
        );
    }

    /// 市场存量模板（已转换为工作流包布局）能被整包导入，且 mainId、各工作流 id 与引用正确。
    #[test]
    fn market_template_package_imports_with_stable_ids() {
        // 定位仓库内转换后的模板目录（src-tauri/../server/market/workflow/...）
        // 目录名已改为工作流 UUID，故按索引条目（name 命中）取其 dir，不写死目录名
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("仓库根目录");
        let market_root = repo_root.join("server").join("market").join("workflow");
        let index: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(market_root.join("workflow-index.json")).unwrap(),
        )
        .unwrap();
        let entry = index["templates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"].as_str() == Some("自动短篇小说写作工作流"))
            .expect("索引里应有「自动短篇小说写作工作流」模板");
        let pkg = market_root.join(entry["dir"].as_str().unwrap());
        assert!(
            pkg.join("manifest.json").is_file(),
            "模板应已转换为工作流包（含包根 manifest.json）"
        );

        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(pkg.join("manifest.json")).unwrap())
                .unwrap();
        let main_id = manifest["mainId"].as_str().unwrap().to_string();
        // 成员 file 一律是相对包根的相对路径（workflows/xxx.json）
        for m in manifest["workflows"].as_array().unwrap() {
            let file = m["file"].as_str().unwrap();
            assert!(file.starts_with("workflows/"), "file 应为相对路径: {}", file);
            assert!(pkg.join(file).is_file(), "清单声明的成员文件应存在: {}", file);
        }

        let conn = test_conn();
        // 直接选中包根清单导入整包
        let outcome = import_workflow_from_file_with_conn(
            &conn,
            pkg.join("manifest.json").to_str().unwrap(),
        )
        .unwrap();

        assert_eq!(
            outcome.definition.id, main_id,
            "主工作流 id 应与 manifest.mainId 一致"
        );
        assert_eq!(outcome.subflow_count, 3, "应导入 3 个子工作流");

        // 主流程 3 个 Subflow 节点的 definitionId 均应指向已导入的子工作流
        let ref_ids: Vec<String> = outcome
            .definition
            .stages
            .iter()
            .flat_map(|s| s.nodes.iter())
            .filter(|n| n.node_type == workflow::WorkflowNodeType::Subflow)
            .filter_map(|n| {
                n.params
                    .as_ref()?
                    .get("definitionId")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(ref_ids.len(), 3, "主流程应保留 3 个子流引用");
        for id in &ref_ids {
            assert!(
                workflow::get_definition(&conn, id).unwrap().is_some(),
                "子流引用 {} 应指向已导入的工作流",
                id
            );
        }
    }

    /// 工作流包识别：选中 `workflows/` 子目录下的成员文件时，靠**向上两级**找到包根
    /// `manifest.json`，按整包导入（而不是当单文件导入）。
    #[test]
    fn package_import_finds_manifest_two_levels_up() {
        let conn = test_conn();
        let sub_id = "55555555-5555-5555-5555-555555555555".to_string();
        let main_id = "66666666-6666-6666-6666-666666666666".to_string();

        let root = temp_workflow_dir();
        let pkg = root.join("某包");
        let wf_dir = pkg.join("workflows");
        std::fs::create_dir_all(&wf_dir).unwrap();

        let main_export =
            ExportWorkflowDefinition::from(make_share_def(&main_id, "主流程", Some(&sub_id)));
        let sub_export = ExportWorkflowDefinition::from(make_share_def(&sub_id, "子流程A", None));
        std::fs::write(
            wf_dir.join("主流程.json"),
            serde_json::to_string_pretty(&main_export).unwrap(),
        )
        .unwrap();
        std::fs::write(
            wf_dir.join("子流程A.json"),
            serde_json::to_string_pretty(&sub_export).unwrap(),
        )
        .unwrap();
        let manifest = serde_json::json!({
            "format": WORKFLOW_PACKAGE_FORMAT,
            "version": 1,
            "mainId": main_id,
            "workflows": [
                { "id": main_id, "name": "主流程", "file": "workflows/主流程.json" },
                { "id": sub_id, "name": "子流程A", "file": "workflows/子流程A.json" },
            ],
        });
        std::fs::write(
            pkg.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        // 选中 workflows/子流程A.json：所在目录无清单 → 父目录（包根）有 → 整包导入
        let outcome = import_workflow_from_file_with_conn(
            &conn,
            wf_dir.join("子流程A.json").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(outcome.definition.id, main_id, "应识别为整包并取 mainId 作主流程");
        assert_eq!(outcome.subflow_count, 1);
        assert!(workflow::get_definition(&conn, &sub_id).unwrap().is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    /// 工作流包安全：成员的 `file` 越界（`..` / 绝对路径）一律被拒绝。
    #[test]
    fn package_import_rejects_unsafe_member_path() {
        for bad in ["../evil.json", "workflows/../../evil.json", "/abs/evil.json"] {
            let conn = test_conn();
            let root = temp_workflow_dir();
            let pkg = root.join("坏包");
            std::fs::create_dir_all(&pkg).unwrap();
            let manifest = serde_json::json!({
                "format": WORKFLOW_PACKAGE_FORMAT,
                "version": 1,
                "mainId": "77777777-7777-7777-7777-777777777777",
                "workflows": [
                    { "id": "77777777-7777-7777-7777-777777777777", "name": "坏包", "file": bad }
                ],
            });
            std::fs::write(
                pkg.join("manifest.json"),
                serde_json::to_string_pretty(&manifest).unwrap(),
            )
            .unwrap();

            let result = import_workflow_from_file_with_conn(
                &conn,
                pkg.join("manifest.json").to_str().unwrap(),
            );
            let err = match result {
                Ok(_) => panic!("越界路径应被拒绝: {}", bad),
                Err(e) => e,
            };
            assert!(err.contains("非法"), "越界路径错误提示应含「非法」: {} ({})", err, bad);

            std::fs::remove_dir_all(&root).ok();
        }
    }

    /// 临时目录（测试结束由调用方清理）。
    fn temp_workflow_dir() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pilotdesk-wf-test-{}", crate::utils::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
