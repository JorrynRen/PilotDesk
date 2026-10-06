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
/// （同为软删除、进回收站）；市场重装覆盖 / 旧子流清理 → [`workflow::hard_delete_definition`]
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

    /// 关联码（**仅老格式解析用**：子工作流有值，主工作流为 None）。
    ///
    /// 新格式（v2 共享 payload / 新文件包）不再输出该字段：引用一律由
    /// `params.definitionId`（工作流稳定 ID）直接表达。保留 `Option` 只为继续读取
    /// 老导出文件与历史 v1 共享 payload（见 [`ExportWorkflowDefinition::restore_subflow_params`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// 共享 payload 当前版本（**v2**：统一 `workflows` 数组 + 工作流稳定 ID 引用）。
pub(crate) const SHARED_WORKFLOW_VERSION: u32 = 2;

/// 共享 payload 字符数上限（与平台 `payload` 约束一致）。
///
/// 超过该上限时**拒绝共享并给出明确中文提示**，绝不静默截断（截断会产生失配的子流引用）。
pub(crate) const SHARED_WORKFLOW_PAYLOAD_MAX_CHARS: usize = 256 * 1024;

/// 共享工作流 payload **v1（历史格式，仅解析）** —— `main` + `subflows` 二元结构：
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
/// 子流引用约定（v1 老约定）：
/// - 导出侧：Subflow 节点 `params.definitionId` 被替换为 `params.refCode`，`refCode` 形如
///   `ref_N`，与 `subflows[i].refCode` 一一对应，子流集合递归收集（含嵌套）。
/// - 导入侧：按拓扑序新建子流后建立 `refCode → 新 definitionId` 映射，再回写主/子流的引用
///   （见 [`ExportWorkflowDefinition::restore_subflow_params`]）。
///
/// **新格式为 [`SharedWorkflowPayload`]（v2，稳定 ID 引用）**，本结构只用于读取平台/本地
/// 已存在的历史 v1 payload（见 [`parse_shared_workflow_payload`]）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWorkflowBundle {
    /// 固定为 [`SHARED_WORKFLOW_FORMAT`]；缺失即为旧格式。
    pub format: String,
    /// 格式版本（v1 = 1）。
    pub version: u32,
    /// 主工作流。
    pub main: ExportWorkflowDefinition,
    /// 全部子工作流（递归展开，含嵌套子流）。
    pub subflows: Vec<ExportWorkflowDefinition>,
}

/// 共享工作流 payload **v2（当前格式）** 的单条成员。
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

/// 共享工作流 payload **v2（当前格式）** —— 统一 `workflows` 数组（含主工作流）：
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
/// 导入端据此直接就地更新 / 新建，**不再做 refCode 重映射**，因此同一份 payload 在不同实例上
/// 得到相同的 ID。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWorkflowPayload {
    /// 固定为 [`SHARED_WORKFLOW_FORMAT`]。
    pub format: String,
    /// 格式版本，当前为 [`SHARED_WORKFLOW_VERSION`]。
    pub version: u32,
    /// 主工作流的稳定 ID（对应 `workflows` 中某条目的 `id`）。
    pub main_id: String,
    /// 主工作流 + 全部子工作流（递归展开，含嵌套子流）。
    pub workflows: Vec<SharedWorkflowEntry>,
}

// ── 工作流文件包（目录 / zip，产品间协议，版本化）──────────────────────

/// 文件包清单文件名（放在包根目录）。
pub(crate) const WORKFLOW_PACKAGE_MANIFEST_FILE: &str = "manifest.json";
/// 文件包格式标识（写入清单的 `format` 字段）。
pub(crate) const WORKFLOW_PACKAGE_FORMAT: &str = "pilotdesk.workflow.package";
/// 文件包当前版本。
pub(crate) const WORKFLOW_PACKAGE_VERSION: u32 = 1;

/// `manifest.json` 结构 —— 声明包内成员及其稳定 ID：
///
/// ```json
/// {
///   "format": "pilotdesk.workflow.package",
///   "version": 1,
///   "mainId": "<主工作流稳定 ID>",
///   "workflows": [ { "id": "<稳定 ID>", "name": "名称", "isMain": true, "file": "[主]名称.json" } ]
/// }
/// ```
///
/// 引用关系完全由各成员 JSON 里的 `params.definitionId`（稳定 ID）表达，文件名**不参与**解析。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowPackageManifest {
    format: String,
    version: u32,
    main_id: String,
    workflows: Vec<WorkflowPackageEntry>,
}

/// 包内单个成员：稳定 ID + 显示名 + 相对文件名。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowPackageEntry {
    id: String,
    name: String,
    /// 是否为主工作流（主工作流文件名仍保留 `[主]` 前缀，便于人类识别）。
    #[serde(default)]
    is_main: bool,
    /// 包内相对文件名（纯文件名，不含路径）。
    file: String,
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

/// 单个工作流的导出主体（供单个导出与批量导出共用）：
/// 在 `<dir_path>/<工作流名>/` 下写出**新文件包**——
/// `manifest.json`（清单）+ `[主]{名称}.json`（主工作流）+ `{名称}.json`（各子工作流，原名，
/// 重名时依次加 `(2)`、`(3)` 序号；文件名不参与引用解析）。
///
/// 引用一律由各 JSON 内的 `params.definitionId`（稳定 ID）表达，不再替换为 refCode。
fn write_workflow_export(
    conn: &rusqlite::Connection,
    def: workflow::WorkflowDefinition,
    dir_path: &str,
) -> Result<(), String> {
    // 自动创建以工作流名称命名的子文件夹

    let workflow_dir = std::path::Path::new(dir_path).join(&def.name);
    std::fs::create_dir_all(&workflow_dir)
        .map_err(|e| AppError::Io(format!("创建文件夹失败: {}", e)))?;

    // 递归收集子工作流（稳定 ID 引用，`definitionId` 原样保留）
    let subflows = collect_subflow_definitions(conn, &def)?;

    let mut used_files: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entries: Vec<WorkflowPackageEntry> = Vec::new();

    // 主工作流：文件名保留 `[主]` 前缀，便于人类在文件管理器里识别
    let main_file = format!("[主]{}.json", def.name);
    used_files.insert(main_file.clone());
    let main_export: ExportWorkflowDefinition = def.clone().into();
    entries.push(WorkflowPackageEntry {
        id: def.id.clone(),
        name: def.name.clone(),
        is_main: true,
        file: main_file.clone(),
    });
    write_export_file(&workflow_dir, &main_file, &main_export)?;

    // 子工作流：原名（无 `[子]` 前缀），同名时仅文件名加序号
    for sub in subflows {
        let file_name = unique_workflow_file_name(&sub.name, &mut used_files);
        let sub_export: ExportWorkflowDefinition = sub.clone().into();
        entries.push(WorkflowPackageEntry {
            id: sub.id.clone(),
            name: sub.name.clone(),
            is_main: false,
            file: file_name.clone(),
        });
        write_export_file(&workflow_dir, &file_name, &sub_export)?;
    }

    // 清单：引用完全由各 JSON 内的 definitionId 表达，文件名不参与解析
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

/// 从文件导入工作流（支持新文件包 / 老 `[主]`+`[子]` 目录 / 单文件 JSON）。
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
}

/// 导入主体（连接维度实现，文件系统入口）的返回结构。
pub(crate) struct WorkflowFileImport {
    pub definition: workflow::WorkflowDefinition,
    pub subflow_count: usize,
    pub missing_dependencies: Vec<String>,
    pub skipped: usize,
}

/// 导入主体（连接维度实现，文件系统入口）。
///
/// 识别顺序：
/// 1. **新文件包**：选中的是 `manifest.json`，或同目录存在 `manifest.json` → 按清单逐个以稳定 ID
///    就地更新 / 新建（引用直接由 `definitionId` 解析）。
/// 2. **老目录**：主文件里 Subflow 节点带 `refCode` → 扫描同目录 `[子]{name}(ref_N).json` 并重映射。
/// 3. **单文件 JSON**：按内容导入；引用缺失不阻断，仅回报给 UI。
///
/// 拆出来是为了让市场安装复用同一套收集逻辑：安装要在**同一个连接里**连续完成
/// "先删旧定义、再导入新的一份"（见 utils/market.rs 的 workflow_market_install）。
pub(crate) fn import_workflow_from_file_with_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
) -> Result<WorkflowFileImport, String> {
    let path = std::path::Path::new(file_path);

    // 1. 新文件包：选中的就是清单，或同目录另有 manifest.json
    let manifest_path = match path.file_name().and_then(|n| n.to_str()) {
        Some(WORKFLOW_PACKAGE_MANIFEST_FILE) => Some(path.to_path_buf()),
        _ => path
            .parent()
            .map(|d| d.join(WORKFLOW_PACKAGE_MANIFEST_FILE))
            .filter(|p| p.is_file()),
    };
    if let Some(manifest_path) = manifest_path {
        if let Some(outcome) = import_workflow_package(conn, &manifest_path)? {
            return Ok(outcome);
        }
    }

    // 2/3. 老格式：读取选中文件
    let main_json = std::fs::read_to_string(file_path)
        .map_err(|e| AppError::Io(format!("读取文件失败: {}", e)))?;
    let main_export: ExportWorkflowDefinition = serde_json::from_str(&main_json)
        .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;

    // 2. 老目录：主文件里的 Subflow 节点带 refCode → 扫描同目录 [子] 文件并重映射
    if export_has_subflow_refs(&main_export) {
        let subflow_defs = collect_subflow_defs_from_dir(file_path)?;
        let (definition, subflow_count, skipped) = import_export_bundle_with_conn(
            conn,
            main_export,
            subflow_defs,
            None,
            workflow::VERSION_ORIGIN_IMPORT,
        )
        .map_err(String::from)?;
        return Ok(WorkflowFileImport {
            definition,
            subflow_count,
            missing_dependencies: Vec::new(),
            skipped,
        });
    }

    // 3. 单文件（无 manifest、无容器）：按内容导入，引用按 definitionId 直接解析；
    //    本地缺失的依赖不阻断，仅回报给 UI。
    let missing_dependencies = collect_missing_subflow_dependencies(conn, &main_export);
    let definition = main_export.into_definition();
    workflow::create_definition(conn, &definition)
        .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
    Ok(WorkflowFileImport {
        definition,
        subflow_count: 0,
        missing_dependencies,
        skipped: 0,
    })
}

/// 按新文件包导入：解析 `manifest.json`，逐成员以**稳定 ID** 就地更新 / 新建。
///
/// 返回 `Ok(None)` 表示这不是一个可识别的新包（清单缺失 / 格式不符），调用方回退老格式路径。
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
        // 不是新格式清单 → 交由老格式路径
        Err(_) => return Ok(None),
    };
    if manifest.format != WORKFLOW_PACKAGE_FORMAT {
        return Ok(None);
    }

    let mut main_def: Option<workflow::WorkflowDefinition> = None;
    let mut subflow_count = 0usize;
    let mut skipped = 0usize;
    for entry in &manifest.workflows {
        // 防目录穿越：成员名必须是不含路径分隔符的纯文件名
        if std::path::Path::new(&entry.file)
            .file_name()
            .and_then(|n| n.to_str())
            != Some(entry.file.as_str())
        {
            return Err(AppError::InvalidInput(format!(
                "工作流包成员文件名非法: {}",
                entry.file
            ))
            .into());
        }
        let member_path = dir.join(&entry.file);
        let json = std::fs::read_to_string(&member_path)
            .map_err(|e| AppError::Io(format!("读取包内文件「{}」失败: {}", entry.file, e)))?;
        let export: ExportWorkflowDefinition = serde_json::from_str(&json)
            .map_err(|e| AppError::Json(format!("解析包内文件「{}」失败: {}", entry.file, e)))?;
        // 新格式引用直接由 definitionId 表达，无需 refCode 重映射
        let mut def = export.into_definition_with_subflows(&std::collections::HashMap::new());
        // 就地更新：本地已有同 ID → 复用该 id 覆盖（覆盖前先备份本地版本）；不存在 → 沿用该 id 新建；
        // 内容与本地相同 → 跳过（不写、不建快照）。
        let applied = store_main_definition(
            conn,
            &mut def,
            Some(&entry.id),
            workflow::VERSION_ORIGIN_IMPORT,
        )?;
        if !applied {
            skipped += 1;
        }
        if entry.is_main || entry.id == manifest.main_id {
            main_def = Some(def);
        } else {
            subflow_count += 1;
        }
    }

    let definition = main_def.ok_or_else(|| {
        AppError::Json("工作流包缺少主工作流（manifest.mainId 未匹配任何成员）".to_string())
    })?;
    Ok(Some(WorkflowFileImport {
        definition,
        subflow_count,
        missing_dependencies: Vec::new(),
        skipped,
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
/// `(主工作流定义, 实际导入的子工作流数量, 因内容相同被跳过的主工作流数)`。
fn import_export_bundle_with_conn(
    conn: &rusqlite::Connection,
    main_export: ExportWorkflowDefinition,
    subflow_defs: std::collections::HashMap<String, ExportWorkflowDefinition>,
    // 主工作流的落库 id：`None` = 普通导入（新建、生成新 UUID）；
    // `Some(id)` = 就地导入（复用该 id，存在则更新——云同步 pull 覆盖同一 object_key 用）。
    main_id_override: Option<&str>,
    origin: &str,
) -> Result<(workflow::WorkflowDefinition, usize, usize), AppError> {
    // 无子工作流引用：直接导入主工作流
    if !export_has_subflow_refs(&main_export) {
        let mut def = main_export.into_definition();
        let applied = store_main_definition(conn, &mut def, main_id_override, origin)?;
        return Ok((def, 0, usize::from(!applied)));
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
    let applied = store_main_definition(conn, &mut main_def, main_id_override, origin)?;
    Ok((main_def, sorted_refs.len(), usize::from(!applied)))
}

/// 主工作流落库：
/// - `override_id` 为 `None` → 一律新建（普通导入，保持既有行为）；
/// - 为 `Some(id)` → 改 id 后 upsert（存在则更新、不存在则新建）——云同步 pull 就地覆盖用，
///   目的是保持同一 object_key，而不是每轮同步都新建副本。
///
/// **覆盖本地已有工作流内容前先打版本快照**（`origin` 区分来源：`import` / `cloud_sync`）；
/// 若与本地内容规范化后完全相同 → 直接跳过（不写、不建快照）。返回是否真的写入（`false` = 跳过）。
fn store_main_definition(
    conn: &rusqlite::Connection,
    def: &mut workflow::WorkflowDefinition,
    override_id: Option<&str>,
    origin: &str,
) -> Result<bool, AppError> {
    match override_id {
        Some(id) => {
            def.id = id.to_string();
            match workflow::get_definition(conn, id)
                .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
            {
                Some(existing) => {
                    // 无改动 → 跳过（本地已存在相同内容，不写、不建快照）
                    if definitions_content_equal(&existing, def) {
                        return Ok(false);
                    }
                    // 有改动 → 先备份本地版本，再接受覆盖
                    if let Err(e) = workflow::snapshot_before_overwrite(conn, id, origin) {
                        log::warn!("[Workflow] 覆盖前保存版本快照失败 {}：{}", id, e);
                    }
                    workflow::update_definition(conn, def)
                        .map_err(|e| AppError::Db(format!("更新工作流失败: {}", e)))?;
                    Ok(true)
                }
                None => {
                    workflow::create_definition(conn, def)
                        .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
                    Ok(true)
                }
            }
        }
        None => {
            workflow::create_definition(conn, def)
                .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
            Ok(true)
        }
    }
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

/// 打包「工作流共享 payload」（v2）：主工作流 + 其全部子工作流（递归、含嵌套），统一使用
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

/// 共享 payload 的解析结果。
enum ParsedSharedPayload {
    /// 当前格式 v2：统一 `workflows` 数组 + 稳定 ID 引用
    V2(SharedWorkflowPayload),
    /// v1：`main` + `subflows`，节点参数用 `refCode`（历史兼容）
    V1(SharedWorkflowBundle),
    /// 旧版：不带 `format` 字段的裸单工作流 JSON（更早的历史资源）
    Legacy(ExportWorkflowDefinition),
}

/// 识别并解析共享 payload：
/// - 带 `format` 且 `version >= 2` → v2（[`SharedWorkflowPayload`]，稳定 ID）；
/// - 带 `format` 且 `version < 2` → v1 捆绑格式（[`SharedWorkflowBundle`]，refCode）；
/// - 否则 → 裸单工作流 JSON（向后兼容更早的历史资源）。
fn parse_shared_workflow_payload(payload: &str) -> Result<ParsedSharedPayload, AppError> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;
    if value.get("format").and_then(|v| v.as_str()) == Some(SHARED_WORKFLOW_FORMAT) {
        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        if version >= SHARED_WORKFLOW_VERSION as u64 {
            let payload: SharedWorkflowPayload = serde_json::from_value(value)
                .map_err(|e| AppError::Json(format!("共享工作流解析失败: {}", e)))?;
            Ok(ParsedSharedPayload::V2(payload))
        } else {
            let bundle: SharedWorkflowBundle = serde_json::from_value(value)
                .map_err(|e| AppError::Json(format!("共享工作流解析失败: {}", e)))?;
            Ok(ParsedSharedPayload::V1(bundle))
        }
    } else {
        let single: ExportWorkflowDefinition = serde_json::from_value(value)
            .map_err(|e| AppError::Json(format!("JSON 解析失败: {}", e)))?;
        Ok(ParsedSharedPayload::Legacy(single))
    }
}

/// 导入 v2 共享 payload：逐个工作流以**稳定 ID** 就地更新 / 新建（复用该 id，不新建副本）。
///
/// 引用直接由各定义的 `params.definitionId` 表达，无需 refCode 重映射。
/// 覆盖本地已有内容前先备份版本（origin = `import`）；内容相同则跳过。
///
/// 返回 `(主工作流定义, 实际导入的子工作流数量, 因内容相同被跳过的成员数)`。
fn import_shared_payload_v2(
    conn: &rusqlite::Connection,
    payload: SharedWorkflowPayload,
) -> Result<(workflow::WorkflowDefinition, usize, usize), AppError> {
    let main_id = payload.main_id.clone();
    let mut main_def: Option<workflow::WorkflowDefinition> = None;
    let mut subflow_count = 0usize;
    let mut skipped = 0usize;
    for entry in payload.workflows {
        let mut def = entry
            .definition
            .into_definition_with_subflows(&std::collections::HashMap::new());
        let applied = store_main_definition(
            conn,
            &mut def,
            Some(&entry.id),
            workflow::VERSION_ORIGIN_IMPORT,
        )?;
        if !applied {
            skipped += 1;
        }
        if entry.id == main_id {
            main_def = Some(def);
        } else {
            subflow_count += 1;
        }
    }
    let definition = main_def.ok_or_else(|| {
        AppError::Json("共享工作流缺少主工作流（mainId 未匹配任何成员）".to_string())
    })?;
    Ok((definition, subflow_count, skipped))
}

/// 从共享 payload 导入一份本地工作流（自动识别 v2 / v1 / 裸单格式；与
/// [`build_workflow_export_json`] 成对，供组织共享空间导入复用）。
///
/// - v2：按稳定 ID 逐个**就地更新 / 新建**（本地已存在同 ID 的 → 复用该 id 覆盖，不产生副本）；
/// - v1：按拓扑序新建子流并做 `refCode → 新 id` 重映射（历史兼容）；
/// - 裸单：等价于旧版 `import_workflow`，仅导入主工作流（更早的历史兼容）。
///
/// 覆盖本地已有内容前先备份版本（origin = `import`）；内容相同则跳过。
/// 返回 `(主工作流, 实际导入的子工作流数量, 因内容相同被跳过的成员数)`。
pub(crate) fn import_workflow_from_json_with_conn(
    conn: &rusqlite::Connection,
    json: &str,
) -> Result<(workflow::WorkflowDefinition, usize, usize), AppError> {
    match parse_shared_workflow_payload(json)? {
        ParsedSharedPayload::V2(payload) => import_shared_payload_v2(conn, payload),
        ParsedSharedPayload::V1(bundle) => {
            let mut subflow_defs: std::collections::HashMap<String, ExportWorkflowDefinition> =
                std::collections::HashMap::new();
            for sub in bundle.subflows {
                let ref_code = sub.ref_code.clone().ok_or_else(|| {
                    AppError::Json("共享子工作流缺少 refCode，无法重建引用".to_string())
                })?;
                subflow_defs.insert(ref_code, sub);
            }
            import_export_bundle_with_conn(
                conn,
                bundle.main,
                subflow_defs,
                None,
                workflow::VERSION_ORIGIN_IMPORT,
            )
        }
        ParsedSharedPayload::Legacy(single) => {
            let def = single.into_definition();
            workflow::create_definition(conn, &def)
                .map_err(|e| AppError::Db(format!("导入失败: {}", e)))?;
            Ok((def, 0, 0))
        }
    }
}

/// 取共享 payload 的**主工作流定义**（用于内容比对 / 云同步冲突留档）。
///
/// v2 取 `mainId` 命中的成员；v1 取 `main`；裸单即其本身。
pub(crate) fn payload_main_definition(
    payload: &str,
) -> Result<workflow::WorkflowDefinition, AppError> {
    match parse_shared_workflow_payload(payload)? {
        ParsedSharedPayload::V2(p) => {
            let main_id = p.main_id.clone();
            let entry = p
                .workflows
                .into_iter()
                .find(|e| e.id == main_id)
                .ok_or_else(|| {
                    AppError::Json("共享工作流缺少主工作流（mainId 未匹配任何成员）".to_string())
                })?;
            Ok(entry
                .definition
                .into_definition_with_subflows(&std::collections::HashMap::new()))
        }
        ParsedSharedPayload::V1(b) => Ok(b.main.into_definition()),
        ParsedSharedPayload::Legacy(s) => Ok(s.into_definition()),
    }
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
/// - v2（稳定 ID）：按各成员稳定 ID 就地 upsert；主工作流归一到 `target_id`（object_key），
///   不再每轮生成新子流 id，故**无需清理旧子流**（批次 2 会接管同步语义）；
/// - v1：先清理该工作流**旧的、仅被它引用的子流**（递归去重），再按 refCode 重映射导入
///   （主工作流复用 `target_id`）；
/// - 裸单：等价于就地覆盖主工作流。
///
/// 返回 `(主工作流定义, 实际导入的子工作流数量)`。
pub(crate) fn apply_workflow_bundle_in_place(
    conn: &rusqlite::Connection,
    target_id: &str,
    json: &str,
) -> Result<(workflow::WorkflowDefinition, usize), AppError> {
    match parse_shared_workflow_payload(json)? {
        ParsedSharedPayload::V2(payload) => {
            let original_main_id = payload.main_id.clone();
            let mut main_def: Option<workflow::WorkflowDefinition> = None;
            let mut subflow_count = 0usize;
            for entry in payload.workflows {
                let mut def = entry
                    .definition
                    .into_definition_with_subflows(&std::collections::HashMap::new());
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
        ParsedSharedPayload::V1(bundle) => {
            cleanup_in_place_subflows(conn, target_id);
            let mut subflow_defs: std::collections::HashMap<String, ExportWorkflowDefinition> =
                std::collections::HashMap::new();
            for sub in bundle.subflows {
                let code = sub.ref_code.clone().ok_or_else(|| {
                    AppError::Json("共享子工作流缺少 refCode，无法重建引用".to_string())
                })?;
                subflow_defs.insert(code, sub);
            }
            import_export_bundle_with_conn(
                conn,
                bundle.main,
                subflow_defs,
                Some(target_id),
                workflow::VERSION_ORIGIN_CLOUD_SYNC,
            )
            .map(|(def, n, _skipped)| (def, n))
        }
        ParsedSharedPayload::Legacy(single) => import_export_bundle_with_conn(
            conn,
            single,
            std::collections::HashMap::new(),
            Some(target_id),
            workflow::VERSION_ORIGIN_CLOUD_SYNC,
        )
        .map(|(def, n, _skipped)| (def, n)),
    }
}

/// v1 就地覆盖前清理旧的、仅被目标工作流引用的子流（其内容会由本次 payload 重建）。
///
/// 同时被别的父级引用的子流保留，避免破坏那些父级的子流引用。
fn cleanup_in_place_subflows(conn: &rusqlite::Connection, target_id: &str) {
    let existing = match workflow::get_definition(conn, target_id) {
        Ok(Some(d)) => d,
        _ => return,
    };
    let mut visited = std::collections::HashSet::new();
    let mut old_subflow_ids = Vec::new();
    collect_referenced_subflow_ids(conn, &existing, &mut old_subflow_ids, &mut visited);
    for sid in old_subflow_ids {
        let parents =
            crate::commands::cloud_sync::workflows_referencing(conn, &sid).unwrap_or_default();
        let only_me = !parents.is_empty() && parents.iter().all(|p| p == target_id);
        if only_me {
            // 内部替换：真删旧子流、不进回收站（其内容由本次 payload 重建）
            if let Err(e) = workflow::hard_delete_definition(conn, &sid) {
                log::warn!("[CloudSync] 清理旧子流失败 {}：{}", sid, e);
            }
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
        let (imported, count, _skipped) = import_workflow_from_json_with_conn(&conn2, &payload).unwrap();
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

        let (imported, count, _skipped) = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        assert_eq!(count, 1);
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
        let (_def, count, _skipped) = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        assert_eq!(count, 0);
    }

    /// 格式识别：v2 / v1 / 裸单三种形状都能被正确区分。
    #[test]
    fn parse_shared_payload_recognizes_v2_v1_and_legacy() {
        // 裸单：无 format 字段
        let legacy = ExportWorkflowDefinition::from(make_share_def("d1", "旧格式", None));
        match parse_shared_workflow_payload(&serde_json::to_string(&legacy).unwrap()).unwrap() {
            ParsedSharedPayload::Legacy(d) => assert_eq!(d.name, "旧格式"),
            _ => panic!("裸单工作流 JSON 应识别为裸单格式"),
        }

        // v1：带 format + version=1
        let v1 = SharedWorkflowBundle {
            format: SHARED_WORKFLOW_FORMAT.to_string(),
            version: 1,
            main: ExportWorkflowDefinition::from(make_share_def("d2", "v1主", None)),
            subflows: vec![],
        };
        match parse_shared_workflow_payload(&serde_json::to_string(&v1).unwrap()).unwrap() {
            ParsedSharedPayload::V1(b) => assert_eq!(b.main.name, "v1主"),
            _ => panic!("version=1 应识别为 v1"),
        }

        // v2：带 format + version=2
        let v2 = SharedWorkflowPayload {
            format: SHARED_WORKFLOW_FORMAT.to_string(),
            version: 2,
            main_id: "d3".to_string(),
            workflows: vec![SharedWorkflowEntry {
                id: "d3".to_string(),
                name: "v2主".to_string(),
                definition: ExportWorkflowDefinition::from(make_share_def("d3", "v2主", None)),
            }],
        };
        match parse_shared_workflow_payload(&serde_json::to_string(&v2).unwrap()).unwrap() {
            ParsedSharedPayload::V2(p) => assert_eq!(p.main_id, "d3"),
            _ => panic!("version=2 应识别为 v2"),
        }
    }

    /// 老格式子流引用的「refCode → 新 id」纯函数回写（v1 导入路径使用）。
    #[test]
    fn restore_subflow_params_replaces_refcode() {
        // 老格式导出侧留下的 params：只有 refCode（没有 definitionId）
        let params = Some(json!({ "refCode": "ref_1", "foo": 1 }));

        let mut ref_to_uuid = std::collections::HashMap::new();
        ref_to_uuid.insert("ref_1".to_string(), "new-def-uuid".to_string());
        let restored =
            ExportWorkflowDefinition::restore_subflow_params(&params, &ref_to_uuid).unwrap();
        assert_eq!(
            restored.get("definitionId").and_then(|v| v.as_str()),
            Some("new-def-uuid")
        );
        assert!(restored.get("refCode").is_none(), "refCode 应被移除");
        assert_eq!(restored.get("foo").and_then(|v| v.as_i64()), Some(1));
    }

    /// 老格式 v1 payload（带 refCode）仍能导入并重建引用。
    #[test]
    fn legacy_v1_payload_imports_with_refcode_remap() {
        let conn = test_conn();
        let sub = build_v1_subflow_export("旧子流", "ref_1");
        let main = build_v1_main_export("旧主流程", "ref_1");
        let v1 = SharedWorkflowBundle {
            format: SHARED_WORKFLOW_FORMAT.to_string(),
            version: 1,
            main,
            subflows: vec![sub],
        };
        let payload = serde_json::to_string(&v1).unwrap();

        let (imported, count, _skipped) = import_workflow_from_json_with_conn(&conn, &payload).unwrap();
        assert_eq!(count, 1, "应随主流程导入 1 个子工作流");
        let ref_id = imported
            .stages
            .iter()
            .flat_map(|s| s.nodes.iter())
            .find(|n| n.node_type == workflow::WorkflowNodeType::Subflow)
            .and_then(|n| n.params.as_ref()?.get("definitionId")?.as_str())
            .map(str::to_string)
            .expect("v1 导入应把 refCode 回写为 definitionId");
        assert!(
            workflow::get_definition(&conn, &ref_id).unwrap().is_some(),
            "回写后的 definitionId 必须指向已导入的子工作流"
        );
    }

    /// 老目录 `[主]xxx.json` + `[子]xxx(ref_N).json` 仍能导入并重建引用。
    #[test]
    fn legacy_directory_import_restores_references() {
        let conn = test_conn();
        let root = temp_workflow_dir();
        let pkg = root.join("老包");
        std::fs::create_dir_all(&pkg).unwrap();

        let main = build_v1_main_export("老主流程", "ref_1");
        std::fs::write(
            pkg.join("[主]老主流程.json"),
            serde_json::to_string_pretty(&main).unwrap(),
        )
        .unwrap();
        let sub = build_v1_subflow_export("老子流程", "ref_1");
        std::fs::write(
            pkg.join("[子]老子流程(ref_1).json"),
            serde_json::to_string_pretty(&sub).unwrap(),
        )
        .unwrap();

        let outcome = import_workflow_from_file_with_conn(
            &conn,
            pkg.join("[主]老主流程.json").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(outcome.subflow_count, 1, "老目录应收集到 1 个子工作流");
        assert!(outcome.missing_dependencies.is_empty());
        let ref_id = outcome
            .definition
            .stages
            .iter()
            .flat_map(|s| s.nodes.iter())
            .find(|n| n.node_type == workflow::WorkflowNodeType::Subflow)
            .and_then(|n| n.params.as_ref()?.get("definitionId")?.as_str())
            .map(str::to_string)
            .expect("老目录导入应把 refCode 回写为 definitionId");
        assert!(workflow::get_definition(&conn, &ref_id).unwrap().is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    /// 新文件包往返：导出目录结构正确，导入到干净库后 id 与引用均保持不变。
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

        // 目录结构：manifest + [主] 主文件 + 子流原名文件
        let pkg = root.join("主流程");
        assert!(pkg.join("manifest.json").is_file(), "应写出 manifest.json");
        assert!(pkg.join("[主]主流程.json").is_file());
        assert!(
            pkg.join("子流程A.json").is_file(),
            "新格式子工作流应为原名、无 [子] 前缀"
        );
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(pkg.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["format"], WORKFLOW_PACKAGE_FORMAT);
        assert_eq!(manifest["mainId"], main_id.as_str());
        assert_eq!(manifest["workflows"].as_array().unwrap().len(), 2);

        // 导入到干净库：id 保持不变、引用直接指向稳定子流 id
        let conn2 = test_conn();
        let outcome = import_workflow_from_file_with_conn(
            &conn2,
            pkg.join("[主]主流程.json").to_str().unwrap(),
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
            root.join("主流程").join("[主]主流程.json").to_str().unwrap(),
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

    /// 造一个老格式子工作流导出定义（带 refCode）。
    fn build_v1_subflow_export(name: &str, ref_code: &str) -> ExportWorkflowDefinition {
        let mut def = ExportWorkflowDefinition::from(make_share_def("old-sub", name, None));
        def.ref_code = Some(ref_code.to_string());
        def
    }

    /// 造一个老格式主工作流导出定义：Subflow 节点参数为 `refCode`（而非 definitionId）。
    fn build_v1_main_export(name: &str, ref_code: &str) -> ExportWorkflowDefinition {
        let mut def = ExportWorkflowDefinition::from(make_share_def("old-main", name, None));
        // 手工塞入一个用 refCode 引用的 Subflow 节点
        for stage in &mut def.stages {
            stage.nodes.push(ExportNode {
                node_type: workflow::WorkflowNodeType::Subflow,
                label: "子流程".to_string(),
                params: Some(json!({ "refCode": ref_code })),
                delay_ms: None,
                timeout_ms: None,
                input_mapping: None,
                output_mapping: None,
                position: None,
            });
        }
        def
    }

    /// 临时目录（测试结束由调用方清理）。
    fn temp_workflow_dir() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pilotdesk-wf-test-{}", crate::utils::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
