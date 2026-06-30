use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use crate::utils::errors::AppError;
use tauri::{Emitter, Manager};
use super::executor::NodeExecutor;
use super::registry::NodeDef;
use super::template::TemplateEngine;
use async_recursion::async_recursion;
use super::{WorkflowDefinition, WorkflowNode, WorkflowNodeType, WorkflowEdge, Stage, MergeStrategy, GateStrategy};

/// 从 AppHandle 获取数据库连接
fn get_db_conn(app_handle: &tauri::AppHandle) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, AppError> {
    app_handle.state::<crate::DbState>().get_conn()
        .map_err(|e| AppError::External(format!("数据库连接失败: {}", e)))
}

/// 写入节点执行记录
///
/// ID 策略: "{execution_id}_{node_id}"，INSERT OR REPLACE 保证同一节点重试时覆盖旧记录。
/// 如需保留重试历史，应改用独立 ID 并添加 attempt 字段。
fn insert_node_execution(
    conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    execution_id: &str,
    node_id: &str,
    status: &str,
    input_data: Option<&str>,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "INSERT OR REPLACE INTO node_executions (id, execution_id, node_id, status, input_data, started_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        rusqlite::params![
            format!("{}_{}", execution_id, node_id),
            execution_id, node_id, status, input_data, now, now,
        ],
    )?;
    Ok(())
}

/// 更新节点执行状态
fn update_node_execution(
    conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    execution_id: &str,
    node_id: &str,
    status: &str,
    output_data: Option<&str>,
    error_message: Option<&str>,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    // 从 output_data 中提取 agent_session_id
    let agent_session_id = output_data.and_then(|s| {
        serde_json::from_str::<serde_json::Value>(s).ok()
            .and_then(|v| v.get("agent_session_id").and_then(|v| v.as_str().map(|s| s.to_string())))
    });
    // 计算 duration_ms = finished_at - started_at
    let duration_ms: Option<i64> = conn.query_row(
        "SELECT started_at FROM node_executions WHERE execution_id = ?1 AND node_id = ?2",
        rusqlite::params![execution_id, node_id],
        |row| row.get::<_, Option<i64>>(0),
    ).ok().flatten().map(|started| now - started);

    conn.execute(
        "UPDATE node_executions SET status = ?1, output_data = COALESCE(?2, output_data),
         error_message = ?3, finished_at = ?4, updated_at = ?4,
         duration_ms = COALESCE(?5, duration_ms),
         agent_session_id = COALESCE(?6, agent_session_id)
         WHERE execution_id = ?7 AND node_id = ?8",
        rusqlite::params![
            status, output_data, error_message, now,
            duration_ms, agent_session_id,
            execution_id, node_id,
        ],
    )?;
    Ok(())
}

/// 写入节点执行记录并发送状态事件
fn emit_node_status(
    emitter: &tauri::AppHandle,
    execution_id: &str,
    node_id: &str,
    status: &str,
    output: Option<&Value>,
    error: Option<&str>,
    completed_count: Option<usize>,
    total: Option<usize>,
) {
    let mut payload = serde_json::json!({
        "execution_id": execution_id,
        "node_id": node_id,
        "status": status,
    });
    if let Some(out) = output {
        payload["output"] = out.clone();
    }
    if let Some(err) = error {
        payload["error"] = err.into();
    }
    emitter.emit("workflow:node-status", payload).ok();

    if let (Some(cc), Some(t)) = (completed_count, total) {
        emitter.emit("workflow:progress", serde_json::json!({
            "execution_id": execution_id,
            "completed": cc,
            "total": t,
        })).ok();
    }
}

/// 写入节点执行记录到 DB 并发送事件
///
/// 注意：写入失败仅记录日志，不中断工作流执行（节点执行不应因记录失败而中止）。
fn record_node_execution(
    emitter: &tauri::AppHandle,
    execution_id: &str,
    node_id: &str,
    status: &str,
    input_data: Option<&str>,
    output_data: Option<&str>,
    error_message: Option<&str>,
) {
    let conn = match get_db_conn(emitter) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[WorkflowEngine] 获取数据库连接失败，跳过节点执行记录: {}", e);
            return;
        }
    };
    let result = match status {
        "skipped" | "running" => {
            insert_node_execution(&conn, execution_id, node_id, status, input_data)
        }
        _ => {
            update_node_execution(&conn, execution_id, node_id, status, output_data, error_message)
        }
    };
    if let Err(e) = result {
        log::warn!("[WorkflowEngine] 写入节点执行记录失败 (execution={}, node={}, status={}): {}",
            execution_id, node_id, status, e);
    }
}

/// 两层调度引擎：阶段串行 → 阶段内 DAG
pub struct WorkflowEngine;

impl WorkflowEngine {
    /// 拓扑排序（Kahn 算法）
    pub fn topological_sort(
        nodes: &[WorkflowNode],
        edges: &[WorkflowEdge],
    ) -> Result<Vec<Vec<String>>, AppError> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();

        for node in nodes {
            in_degree.entry(node.id.clone()).or_insert(0);
            adjacency.entry(node.id.clone()).or_default();
        }

        for edge in edges {
            adjacency.get_mut(&edge.source).unwrap_or(&mut vec![]).push(edge.target.clone());
            *in_degree.get_mut(&edge.target).unwrap_or(&mut 0) += 1;
        }

        let mut layers: Vec<Vec<String>> = Vec::new();
        let mut queue: Vec<String> = in_degree.iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut processed = 0;

        while !queue.is_empty() {
            layers.push(queue.clone());
            let mut next_queue = Vec::new();

            for node_id in &queue {
                processed += 1;
                for neighbor in adjacency.get(node_id).unwrap_or(&vec![]) {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            next_queue.push(neighbor.clone());
                        }
                    }
                }
            }
            queue = next_queue;
        }

        if processed != nodes.len() {
            return Err(AppError::InvalidInput("工作流包含循环依赖".into()));
        }

        Ok(layers)
    }

    /// 执行单个阶段内的 DAG
    /// 执行单个阶段内的 DAG
    /// 返回 (节点状态映射, 阶段值)
    /// 节点状态映射: node_id -> "completed" | "failed" | "skipped" | "running"
    async fn execute_stage(
        executor: &Arc<NodeExecutor>,
        stage: &Stage,
        execution_id: &str,
        context: &Arc<AsyncMutex<HashMap<String, Value>>>,
        emitter: &tauri::AppHandle,
        cancelled: &Arc<AtomicBool>,
        completed_count: &Arc<AtomicUsize>,
        total_count: usize,
        semaphore: &Arc<Semaphore>,
        max_concurrency: usize,
        visited_def_ids: &[String],
    ) -> Result<(HashMap<String, String>, Value), AppError> {
        let layers = Self::topological_sort(&stage.nodes, &stage.edges)?;
        let node_map: HashMap<String, &WorkflowNode> = stage.nodes.iter().map(|n| (n.id.clone(), n)).collect();
        // 节点执行状态追踪（用于门控策略判断），Arc<AsyncMutex> 支持跨 spawn 共享
        let node_statuses: Arc<AsyncMutex<HashMap<String, String>>> = Arc::new(AsyncMutex::new(HashMap::new()));

        for layer in &layers {
            let mut handles: Vec<tokio::task::JoinHandle<Result<(), AppError>>> = Vec::new();

            let stage_id = stage.id.clone(); // 用于 async move 闭包
            for node_id in layer {
                let node = match node_map.get(node_id) {
                    Some(n) => (*n).clone(),
                    None => continue,
                };

                if cancelled.load(Ordering::SeqCst) {
                    return Err(AppError::External("工作流已被取消".into()));
                }

                // 检查入边条件
                let incoming_edges: Vec<&WorkflowEdge> = stage.edges.iter()
                    .filter(|e| e.target == *node_id).collect();

                if !incoming_edges.is_empty() {
                    let ctx = context.lock().await.clone();
                    let mut all_conditions_met = true;

                    for edge in &incoming_edges {
                        if let Some(cond) = &edge.condition {
                            let source_output = ctx.get(&edge.source);
                            if !Self::evaluate_condition(cond, source_output) {
                                all_conditions_met = false;
                                break;
                            }
                        }
                    }

                    if !all_conditions_met {
                        node_statuses.lock().await.insert(node_id.clone(), "skipped".to_string());
                        completed_count.fetch_add(1, Ordering::SeqCst);
                        record_node_execution(emitter, execution_id, node_id, "skipped", None, None, None);
                        emit_node_status(emitter, execution_id, node_id, "skipped", None, None,
                            Some(completed_count.load(Ordering::SeqCst)), Some(total_count));
                        continue;
                    }
                }

                // 获取并发许可（Subflow 和普通节点都受控）
                let permit = semaphore.clone().acquire_owned().await;
                let _permit = match permit {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Subflow 节点：在 spawn 之外同步执行（避免 Send 约束问题）
                if node.node_type == WorkflowNodeType::Subflow {
                    let ctx_snapshot = context.lock().await.clone();
                    let resolved_input = resolve_node_input(&node, &ctx_snapshot);

                    record_node_execution(emitter, execution_id, node_id, "running",
                        serde_json::to_string(&resolved_input).ok().as_deref(), None, None);
                    emit_node_status(emitter, execution_id, node_id, "running", None, None, None, None);

                    match Self::execute_subflow_node(
                        executor, &node, resolved_input,
                        execution_id, emitter, max_concurrency,
                        visited_def_ids,
                    ).await {
                        Ok(output) => {
                            node_statuses.lock().await.insert(node_id.clone(), "completed".to_string());
                            context.lock().await.insert(node_id.clone(), output.clone());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(emitter, execution_id, node_id, "completed", None,
                                serde_json::to_string(&output).ok().as_deref(), None);
                            emit_node_status(emitter, execution_id, node_id, "completed", Some(&output), None,
                                Some(completed_count.load(Ordering::SeqCst)), Some(total_count));
                        }
                        Err(e) => {
                            node_statuses.lock().await.insert(node_id.clone(), "failed".to_string());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(emitter, execution_id, node_id, "failed", None, None, Some(&e.to_string()));
                            emit_node_status(emitter, execution_id, node_id, "failed", None, Some(&e.to_string()), None, None);
                            // 不再 return Err，让后续节点继续执行
                        }
                    }
                    // _permit 在此处 drop，释放并发许可
                    continue;
                }

                let exec_id = execution_id.to_string();
                let nid = node_id.clone();
                let emitter = emitter.clone();
                let context = context.clone();
                let completed_count = completed_count.clone();
                let total = total_count;
                let exec = executor.clone();

                // 开始节点：提前提取 output_mapping，执行后用它替代空结果
                let start_output_mapping = if node.node_type == WorkflowNodeType::Start {
                    node.output_mapping.as_ref().and_then(|m| m.as_object().cloned())
                } else {
                    None
                };
                let node_statuses_clone = node_statuses.clone();
                let stage_id_clone = stage_id.clone();
                let handle = tokio::spawn(async move {
                    let ctx_snapshot = context.lock().await.clone();
                    log::info!("[WorkflowEngine] Executing node {} (type={:?}), context keys: {:?}", nid, node.node_type, context.lock().await.keys().collect::<Vec<_>>());
                    let resolved_input = resolve_node_input(&node, &ctx_snapshot);

                    record_node_execution(&emitter, &exec_id, &nid, "running",
                        serde_json::to_string(&resolved_input).ok().as_deref(), None, None);
                    emit_node_status(&emitter, &exec_id, &nid, "running", None, None, None, None);

                    let mut node_def = node_to_node_def(&node);

                    // Agent 节点：解析 resume_session_ref 模板（从 context 获取实际 session_id）
                    if node_def.node_type == "agent" {
                        if let Some(ref_mode) = node_def.config.get("session_mode").and_then(|v| v.as_str()) {
                            if ref_mode == "resume" {
                                if let Some(ref_tmpl) = node_def.config.get("resume_session_ref").and_then(|v| v.as_str()) {
                                    if !ref_tmpl.is_empty() {
                                        match TemplateEngine::resolve(ref_tmpl, &ctx_snapshot) {
                                            Ok(resolved_sid) => {
                                                log::info!("[WorkflowEngine] Agent node {} resume session resolved: {} -> {}", nid, ref_tmpl, resolved_sid);
                                                node_def.config.as_object_mut()
                                                    .map(|map| map.insert("resume_session_id".to_string(), Value::String(resolved_sid)));
                                            }
                                            Err(e) => {
                                                log::warn!("[WorkflowEngine] Agent node {} resume session_ref template resolve failed: {}", nid, e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let result = exec.execute(&node_def, resolved_input.clone(), &exec_id, &emitter).await;

                    match result {
                        Ok(output) => {
                            // 开始节点使用 output_mapping 作为输出，而非空执行结果
                            let node_output = if let Some(mapping) = &start_output_mapping {
                                Value::Object(mapping.clone())
                            } else {
                                output.output.clone()
                            };
                            // Agent 节点：将 content 和 session_id 组装为对象存入 context
                            // 使模板 {{key.output.nodeId}} 可解析 content / session_id 字段
                            // 同时保留 session_id 到独立 key 供直接引用
                            if let Some(ref sid) = output.session_id {
                                let ctx_value = serde_json::json!({
                                    "content": node_output,
                                    "session_id": sid,
                                });
                                context.lock().await.insert(nid.clone(), ctx_value);
                                // 旧格式兼容
                                context.lock().await.insert(format!("__session_id__{}", nid), Value::String(sid.clone()));
                                // 新格式：session_id.{nodeId}.{stageId}
                                context.lock().await.insert(format!("session_id.{}.{}", nid, stage_id_clone), Value::String(sid.clone()));
                                log::info!("[WorkflowEngine] Agent node {} context: content + session_id({})", nid, sid);
                            } else {
                                context.lock().await.insert(nid.clone(), node_output.clone());
                            }
                            log::info!("[WorkflowEngine] Node {} output written to context: {:?}", nid, node_output);
                            node_statuses_clone.lock().await.insert(nid.clone(), "completed".to_string());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(&emitter, &exec_id, &nid, "completed", None,
                                serde_json::to_string(&node_output).ok().as_deref(), None);
                            emit_node_status(&emitter, &exec_id, &nid, "completed", Some(&node_output), None,
                                Some(completed_count.load(Ordering::SeqCst)), Some(total));
                            return Ok(());
                        }
                        Err(e) => {
                            node_statuses_clone.lock().await.insert(nid.clone(), "failed".to_string());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(&emitter, &exec_id, &nid, "failed", None, None, Some(&e.to_string()));
                            emit_node_status(&emitter, &exec_id, &nid, "failed", None, Some(&e.to_string()),
                                Some(completed_count.load(Ordering::SeqCst)), Some(total));
                            // 不再 return Err，让同层其他节点继续执行
                            return Ok(());
                        }
                    }
                });

                handles.push(handle);
            }

            for handle in handles {
                match handle.await {
                    Ok(Err(e)) => {
                        // 节点执行失败，状态已记录在 node_statuses 中，不终止阶段
                        log::warn!("[WorkflowEngine] 节点执行失败(已容忍): {}", e);
                    }
                    Err(join_err) => {
                        log::warn!("[WorkflowEngine] 节点执行任务异常(已容忍): {}", join_err);
                    }
                    _ => {}
                }
            }
        }

        let statuses = node_statuses.lock().await.clone();
        Ok((statuses, Value::Null))
    }

    /// 评估边上的条件表达式
    /// 支持：==, !=, >, <, >=, <=, contains
    fn evaluate_condition(condition: &str, source_output: Option<&Value>) -> bool {
        match source_output {
            Some(output) => {
                // 对 Array 取第一个元素，对 Object 取第一个 value
                let resolved = match output {
                    Value::Array(arr) => arr.first().cloned().unwrap_or(Value::Null),
                    Value::Object(map) => map.values().next().cloned().unwrap_or(Value::Null),
                    other => other.clone(),
                };
                let output_str = resolved.to_string();
                let trimmed = output_str.trim_matches('"');

                if let Some(val) = condition.strip_prefix("==") {
                    return trimmed == val.trim();
                }
                if let Some(val) = condition.strip_prefix("!=") {
                    return trimmed != val.trim();
                }
                if let Some(val) = condition.strip_prefix(">=") {
                    if let (Ok(a), Ok(b)) = (trimmed.parse::<f64>(), val.trim().parse::<f64>()) {
                        return a >= b;
                    }
                    return trimmed >= val.trim();
                }
                if let Some(val) = condition.strip_prefix("<=") {
                    if let (Ok(a), Ok(b)) = (trimmed.parse::<f64>(), val.trim().parse::<f64>()) {
                        return a <= b;
                    }
                    return trimmed <= val.trim();
                }
                if let Some(val) = condition.strip_prefix(">") {
                    if let (Ok(a), Ok(b)) = (trimmed.parse::<f64>(), val.trim().parse::<f64>()) {
                        return a > b;
                    }
                    return trimmed > val.trim();
                }
                if let Some(val) = condition.strip_prefix("<") {
                    if let (Ok(a), Ok(b)) = (trimmed.parse::<f64>(), val.trim().parse::<f64>()) {
                        return a < b;
                    }
                    return trimmed < val.trim();
                }
                if let Some(val) = condition.strip_prefix("contains") {
                    return output_str.contains(val.trim());
                }
                !output.is_null() && !output_str.is_empty()
            }
            None => false,
        }
    }

    /// 检查门控策略是否满足进入下一阶段的条件
    ///
    /// 统一流程：先执行 merge，再检查策略。
    /// - All: 阶段内所有非边界节点均成功完成 → 放行；任一失败 → 中止
    /// - Count(n): 至少 n 个非边界节点成功完成 → 放行；不足 → 中止
    /// - Threshold(expr): 基于 merge 后的值做条件判断（如 "avg_score >= 60"）
    ///   不满足 → 中止
    fn check_gate_strategy(
        stage: &Stage,
        node_statuses: &HashMap<String, String>,
        merged_value: &Value,
    ) -> Result<bool, AppError> {
        // 收集阶段内非边界节点的执行状态
        let non_boundary_statuses: Vec<(&String, &String)> = stage.nodes.iter()
            .filter(|n| n.node_type != WorkflowNodeType::Start && n.node_type != WorkflowNodeType::End)
            .filter_map(|n| node_statuses.get(&n.id).map(|s| (&n.id, s)))
            .collect();

        let total = non_boundary_statuses.len();
        let success_count = non_boundary_statuses.iter()
            .filter(|(_, s)| *s == "completed")
            .count();
        let failed_count = non_boundary_statuses.iter()
            .filter(|(_, s)| *s == "failed")
            .count();

        log::info!(
            "[check_gate_strategy] stage={}, strategy={:?}, total={}, success={}, failed={}",
            stage.name, stage.gate.strategy, total, success_count, failed_count
        );

        match &stage.gate.strategy {
            GateStrategy::All => {
                // 全部完成：所有非边界节点必须成功
                if failed_count > 0 {
                    Err(AppError::External(format!(
                        "阶段 '{}' 门控策略 All 不满足: {}/{} 个节点失败",
                        stage.name, failed_count, total
                    )))
                } else {
                    Ok(true)
                }
            }
            GateStrategy::Count(n) => {
                // 指定数量完成：至少 n 个节点成功
                if success_count >= *n {
                    Ok(true)
                } else {
                    Err(AppError::External(format!(
                        "阶段 '{}' 门控策略 Count({}) 不满足: 仅 {}/{} 个节点成功",
                        stage.name, n, success_count, total
                    )))
                }
            }
            GateStrategy::Threshold(expr) => {
                // 按条件判断：基于 merge 后的值做条件判断
                // 复用 evaluate_condition 逻辑，将 merged_value 作为 source_output
                let passed = Self::evaluate_condition(expr, Some(merged_value));
                if passed {
                    Ok(true)
                } else {
                    Err(AppError::External(format!(
                        "阶段 '{}' 门控策略 Threshold 不满足: 合并值未满足条件 '{}'",
                        stage.name, expr
                    )))
                }
            }
        }
    }



    /// 执行 Gate 合并逻辑
    fn merge_stage_outputs(
        stage: &Stage,
        context: &HashMap<String, Value>,
    ) -> Value {
        let node_outputs: Vec<(&String, &Value)> = stage.nodes.iter()
            .filter_map(|n| context.get(&n.id).map(|v| (&n.id, v)))
            .collect();

        match stage.gate.merge_strategy {
            MergeStrategy::Merge => {
                let mut merged = serde_json::Map::new();
                for (node_id, output) in &node_outputs {
                    if let Some(obj) = output.as_object() {
                        for (k, v) in obj {
                            merged.insert(format!("{}_{}", node_id, k), v.clone());
                        }
                    } else {
                        merged.insert((*node_id).clone(), (*output).clone());
                    }
                }
                Value::Object(merged)
            }
            MergeStrategy::Concat => {
                let arr: Vec<Value> = node_outputs.iter().map(|(_, v)| (*v).clone()).collect();
                Value::Array(arr)
            }
            MergeStrategy::PickFirst => {
                node_outputs.first().map(|(_, v)| (*v).clone()).unwrap_or(Value::Null)
            }
            MergeStrategy::PickLast => {
                node_outputs.last().map(|(_, v)| (*v).clone()).unwrap_or(Value::Null)
            }
            MergeStrategy::Custom(ref script) => {
                // Helper: 从 Value 中按路径提取字段
                fn extract_by_path(val: &Value, path: &str) -> Value {
                    if path.is_empty() {
                        return val.clone();
                    }
                    let parts: Vec<&str> = path.split('.').collect();
                    let mut current: Value = val.clone();
                    for part in &parts {
                        if let Some(obj) = current.as_object() {
                            current = obj.get(*part).cloned().unwrap_or(Value::Null);
                        } else {
                            return Value::Null;
                        }
                    }
                    current
                }

                if let Some(rest) = script.strip_prefix("merge:") {
                    let mut merged = serde_json::Map::new();
                    for item in rest.split(',') {
                        let item = item.trim();
                        if let Some(dot_pos) = item.find('.') {
                            let node_id = &item[..dot_pos];
                            let field_path = &item[dot_pos + 1..];
                            if let Some((_, node_val)) = node_outputs.iter()
                                .find(|(id, _)| id.as_str() == node_id)
                            {
                                let extracted = extract_by_path(node_val, field_path);
                                if extracted != Value::Null {
                                    let key = field_path.split('.').last().map(|s| s.to_string()).unwrap_or_default();
                                    merged.insert(key, extracted);
                                }
                            }
                        }
                    }
                    Value::Object(merged)
                } else if let Some(rest) = script.strip_prefix("pick:") {
                    let rest = rest.trim();
                    if let Some(dot_pos) = rest.find('.') {
                        let node_id = &rest[..dot_pos];
                        let field_path = &rest[dot_pos + 1..];
                        if let Some((_, val)) = node_outputs.iter()
                            .find(|(id, _)| id.as_str() == node_id)
                        {
                            extract_by_path(val, field_path)
                        } else {
                            Value::Null
                        }
                    } else {
                        node_outputs.iter()
                            .find(|(id, _)| id.as_str() == rest)
                            .map(|(_, v)| (*v).clone())
                            .unwrap_or(Value::Null)
                    }
                } else if let Some(rest) = script.strip_prefix("wrap:") {
                    let mut parts = rest.splitn(3, ':');
                    let prefix = parts.next().unwrap_or("");
                    let suffix = parts.next().unwrap_or("");
                    let key = format!("{}{}{}", prefix, "output", suffix);
                    if let Some((_, val)) = node_outputs.first() {
                        let mut wrapped = serde_json::Map::new();
                        wrapped.insert(key, (*val).clone());
                        Value::Object(wrapped)
                    } else {
                        Value::Object(serde_json::Map::new())
                    }
                } else if let Some(rest) = script.strip_prefix("calc:") {
                    // 内置聚合运算，格式: calc:<filter>:<merge_as>:<value_op>
                    // filter: all | success（只保留成功节点）
                    // merge_as: none | object | array | flat
                    // value_op: none | max | min | avg | sum | first | last | count
                    let parts: Vec<&str> = rest.splitn(4, ':').collect();
                    let filter = parts.get(0).unwrap_or(&"all");
                    let _merge_as = parts.get(1).unwrap_or(&"none");
                    let value_op = parts.get(2).unwrap_or(&"none");

                    // 从 node_outputs 中提取可运算的数值列表
                    let mut numeric_values: Vec<f64> = Vec::new();
                    let mut string_values: Vec<String> = Vec::new();
                    let mut all_values: Vec<Value> = Vec::new();

                    for (_node_id, output) in &node_outputs {
                        // 过滤：只保留成功节点（此处所有 node_outputs 中的值均为已完成节点的输出）
                        let include = match *filter {
                            "success" => true,  // node_outputs 已是已完成节点
                            _ => true,
                        };
                        if !include { continue; }

                        // 尝试提取数值
                        if let Some(num) = output.as_f64() {
                            numeric_values.push(num);
                            all_values.push((*output).clone());
                        } else if let Some(s) = output.as_str() {
                            if let Ok(num) = s.trim().parse::<f64>() {
                                numeric_values.push(num);
                            } else {
                                string_values.push(s.to_string());
                            }
                            all_values.push((*output).clone());
                        } else {
                            all_values.push((*output).clone());
                        }
                    }

                    match *value_op {
                        "max" => {
                            if let Some(&max_val) = numeric_values.iter().max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)) {
                                Value::Number(serde_json::Number::from_f64(max_val).unwrap_or(serde_json::Number::from(0)))
                            } else {
                                Value::Null
                            }
                        }
                        "min" => {
                            if let Some(&min_val) = numeric_values.iter().min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)) {
                                Value::Number(serde_json::Number::from_f64(min_val).unwrap_or(serde_json::Number::from(0)))
                            } else {
                                Value::Null
                            }
                        }
                        "avg" => {
                            if !numeric_values.is_empty() {
                                let avg = numeric_values.iter().sum::<f64>() / numeric_values.len() as f64;
                                Value::Number(serde_json::Number::from_f64(avg).unwrap_or(serde_json::Number::from(0)))
                            } else {
                                Value::Null
                            }
                        }
                        "sum" => {
                            let sum = numeric_values.iter().sum::<f64>();
                            Value::Number(serde_json::Number::from_f64(sum).unwrap_or(serde_json::Number::from(0)))
                        }
                        "count" => {
                            Value::Number(serde_json::Number::from(numeric_values.len()))
                        }
                        "first" => {
                            all_values.first().cloned().unwrap_or(Value::Null)
                        }
                        "last" => {
                            all_values.last().cloned().unwrap_or(Value::Null)
                        }
                        _ => {
                            // none: 保留原始值数组（默认行为）
                            Value::Array(all_values)
                        }
                    }
                } else {
                    Value::Object(serde_json::Map::new())
                }
            }
        }
    }

    /// 执行 Subflow 节点：递归加载子工作流定义并执行
    #[async_recursion]
    async fn execute_subflow_node(
        executor: &Arc<NodeExecutor>,
        node: &WorkflowNode,
        input_data: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
        visited_def_ids: &[String],
    ) -> Result<Value, AppError> {
        // 从节点参数中获取子工作流定义 ID
        let subflow_def_id = node.params
            .as_ref()
            .and_then(|p| p.get("definitionId"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::InvalidInput("Subflow 节点缺少 definitionId 参数".into()))?;

        // 加载子工作流定义
        let conn = get_db_conn(emitter)?;
        let subflow_def = super::get_definition(&conn, subflow_def_id)?
            .ok_or_else(|| AppError::InvalidInput(format!("子工作流定义不存在: {}", subflow_def_id)))?;

        // 检查 maxDepth（默认 10）
        let max_depth = subflow_def.max_depth.unwrap_or(10) as usize;
        if visited_def_ids.len() >= max_depth {
            return Err(AppError::InvalidInput(format!(
                "子工作流嵌套深度 {} 超过最大限制 {}（maxDepth）。工作流: \"{}\" -> \"{}\"",
                visited_def_ids.len() + 1, max_depth, subflow_def.name, subflow_def.name
            )));
        }

        // 检查循环引用
        if visited_def_ids.contains(&subflow_def_id.to_string()) {
            let chain = visited_def_ids.iter().chain(std::iter::once(&subflow_def_id.to_string()))
                .cloned().collect::<Vec<_>>().join(" → ");
            return Err(AppError::InvalidInput(format!(
                "检测到工作流循环引用：{}。请检查工作流定义中的 Subflow 节点配置。", chain
            )));
        }

        // 创建子工作流的 execution_id（使用父 execution_id + 节点 id 作为子 execution_id）
        let sub_execution_id = format!("{}_{}", execution_id, node.id);

        // 创建子工作流实例记录
        let now = crate::utils::now();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO workflow_instances (id, definition_id, definition_name, status, context, trigger, started_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'running', '{}', 'subflow', ?4, ?4, ?4)",
            rusqlite::params![sub_execution_id, subflow_def_id, subflow_def.name, now],
        );

        // 构建新的 visited_def_ids 链
        let mut new_visited = visited_def_ids.to_vec();
        new_visited.push(subflow_def_id.to_string());

        // 递归执行子工作流
        let result = Self::execute_with_concurrency_impl(
            executor,
            &subflow_def,
            &sub_execution_id,
            input_data,
            emitter,
            max_concurrency,
            &new_visited,
        ).await;

        // 更新子工作流实例状态
        match &result {
            Ok(_) => {
                let _ = conn.execute(
                    "UPDATE workflow_instances SET status = 'success', completed_at = ?1, updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![crate::utils::now(), sub_execution_id],
                );
            }
            Err(e) => {
                let _ = conn.execute(
                    "UPDATE workflow_instances SET status = 'failed', error = ?1, completed_at = ?2, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![e.to_string(), crate::utils::now(), sub_execution_id],
                );
            }
        }

        result
    }

    /// 根据 outputSchema 格式化输出结果
    fn format_output(result: Value, schema: &Option<serde_json::Value>) -> Value {
        let Some(schema_obj) = schema.as_ref().and_then(|s| s.as_object()) else {
            return result;
        };
        if schema_obj.is_empty() {
            return result;
        }
        if let Some(result_obj) = result.as_object() {
            let mut formatted = serde_json::Map::new();
            for (field, rules) in schema_obj {
                if let Some(rules_obj) = rules.as_object() {
                    if let Some(value) = result_obj.get(field) {
                        formatted.insert(field.clone(), value.clone());
                    } else if let Some(default_val) = rules_obj.get("default") {
                        formatted.insert(field.clone(), default_val.clone());
                    } else {
                        formatted.insert(field.clone(), Value::Null);
                    }
                }
            }
            Value::Object(formatted)
        } else {
            result
        }
    }

    /// 启动工作流执行（两层调度）
    pub async fn execute_with_concurrency(
        executor: &Arc<NodeExecutor>,
        def: &WorkflowDefinition,
        execution_id: &str,
        input_data: Value,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
    ) -> Result<Value, AppError> {
        Self::execute_with_concurrency_impl(executor, def, execution_id, input_data, emitter, max_concurrency, &[]).await
    }

    /// 内部实现：支持 visited_def_ids 循环引用检测
    async fn execute_with_concurrency_impl(
        executor: &Arc<NodeExecutor>,
        def: &WorkflowDefinition,
        execution_id: &str,
        input_data: Value,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
        visited_def_ids: &[String],
    ) -> Result<Value, AppError> {
        // -- inputSchema 校验 --
        if let Some(schema) = &def.input_schema {
            if let Some(schema_obj) = schema.as_object() {
                for (field, rules) in schema_obj {
                    if let Some(rules_obj) = rules.as_object() {
                        let required = rules_obj.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
                        if required {
                            let has_value = match input_data.get(field) {
                                Some(v) => !v.is_null(),
                                None => false,
                            };
                            if !has_value {
                                return Err(AppError::InvalidInput(format!(
                                    "工作流 \"{}\" 缺少必填输入参数: {}（类型: {}）",
                                    def.name, field,
                                    rules_obj.get("type").and_then(|v| v.as_str()).unwrap_or("unknown")
                                )));
                            }
                        }
                        if let Some(expected_type) = rules_obj.get("type").and_then(|v| v.as_str()) {
                            if let Some(value) = input_data.get(field) {
                                if !value.is_null() {
                                    let type_ok = match expected_type {
                                        "string" => value.is_string(),
                                        "number" => value.is_number(),
                                        "integer" => value.is_i64() || value.is_u64(),
                                        "boolean" => value.is_boolean(),
                                        "array" => value.is_array(),
                                        "object" => value.is_object(),
                                        _ => true,
                                    };
                                    if !type_ok {
                                        return Err(AppError::InvalidInput(format!(
                                            "工作流 \"{}\" 输入参数 \"{}\" 类型错误: 期望 {}",
                                            def.name, field, expected_type
                                        )));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        let total_count: usize = def.stages.iter().map(|s| s.nodes.len()).sum();
        let completed_count = Arc::new(AtomicUsize::new(0));

        log::info!("[WorkflowEngine] 开始执行: id={}, stages={}, total_nodes={}", execution_id, def.stages.len(), total_count);

        let context: Arc<AsyncMutex<HashMap<String, Value>>> = Arc::new(AsyncMutex::new(HashMap::new()));
        context.lock().await.insert("__input__".to_string(), input_data.clone());

        // 开始节点：将输出映射作为初始上下文值
        for stage in &def.stages {
            for node in &stage.nodes {
                if node.node_type == WorkflowNodeType::Start {
                    if let Some(mapping) = &node.output_mapping {
                        if let Some(obj) = mapping.as_object() {
                            // 新架构：outputMapping 的 key 是用户自定义参数名，value 是 content（用户输入值）
                            // 以 nodeId 为 key 存储全部输出，支持 {{参数名.节点ID.阶段ID}} 格式
                            // 开始节点直接将 outputMapping 的 key-value 作为输出数据
                            let output_obj: serde_json::Map<String, Value> = obj.iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                            context.lock().await.insert(node.id.clone(), Value::Object(output_obj));
                            // 也设置扁平键，支持简单 {{参数名}} 格式
                            for (k, v) in obj {
                                context.lock().await.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    break;
                }
            }
        }

        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        log::info!("[WorkflowEngine] Context after start node pre-population: {:?}", context.lock().await.keys().collect::<Vec<_>>());

        for stage in &def.stages {
            log::info!("[WorkflowEngine] 执行阶段: id={}, name={}, nodes={}, edges={}", stage.id, stage.name, stage.nodes.len(), stage.edges.len());
            if cancelled.load(Ordering::SeqCst) {
                return Err(AppError::External("工作流已被取消".into()));
            }

            emitter.emit("workflow:stage-status", serde_json::json!({
                "execution_id": execution_id,
                "stage_id": stage.id,
                "stage_name": stage.name,
                "status": "running",
            })).ok();

            let (node_statuses, _) = Self::execute_stage(
                executor, stage, execution_id,
                &context, emitter, &cancelled,
                &completed_count, total_count, &semaphore,
                max_concurrency,
                visited_def_ids,
            ).await?;

            // ── 步骤 1: 执行合并策略（始终执行） ──
            let ctx = context.lock().await.clone();
            let merged = Self::merge_stage_outputs(stage, &ctx);

            // ── 步骤 2: 门控策略检查（合并完成后检查） ──
            //  - All: 全部非边界节点成功 → 放行；任一失败 → 中止
            //  - Count(n): 成功数 >= n → 放行；成功数 < n → 中止
            //  - Threshold: 合并后的值满足条件 → 放行；不满足 → 中止
            match Self::check_gate_strategy(stage, &node_statuses, &merged) {
                Ok(true) => {
                    log::info!("[WorkflowEngine] 阶段 '{}' 门控策略检查通过，合并结果已写入", stage.name);
                    context.lock().await.insert(format!("gate_output.{}", stage.id), merged);
                }
                Ok(false) => {
                    // 策略不满足（Count 成功数不足 / Threshold 条件不满足），中止工作流
                    let msg = format!(
                        "阶段 '{}' 门控策略未通过（strategy={:?}），工作流中止",
                        stage.name, stage.gate.strategy
                    );
                    log::warn!("[WorkflowEngine] {}", msg);
                    emitter.emit("workflow:stage-status", serde_json::json!({
                        "execution_id": execution_id,
                        "stage_id": stage.id,
                        "stage_name": stage.name,
                        "status": "gate_failed",
                        "reason": msg.clone(),
                    })).ok();
                    return Err(AppError::External(msg));
                }
                Err(e) => {
                    // 策略判定失败（如 All 策略下有节点失败），中止工作流
                    log::error!("[WorkflowEngine] 阶段 '{}' 门控策略检查失败: {}", stage.name, e);
                    emitter.emit("workflow:stage-status", serde_json::json!({
                        "execution_id": execution_id,
                        "stage_id": stage.id,
                        "stage_name": stage.name,
                        "status": "gate_failed",
                        "error": e.to_string(),
                    })).ok();
                    return Err(e);
                }
            }

            emitter.emit("workflow:stage-status", serde_json::json!({
                "execution_id": execution_id,
                "stage_id": stage.id,
                "stage_name": stage.name,
                "status": "completed",
            })).ok();
        }

        // 更新实例状态为成功
        if let Ok(conn) = get_db_conn(emitter) {
            let _ = conn.execute(
                "UPDATE workflow_instances SET status = 'success', completed_at = ?1, updated_at = ?1 WHERE id = ?2",
                rusqlite::params![crate::utils::now(), execution_id],
            );
        }

        emitter.emit("workflow:execution-status", serde_json::json!({
            "execution_id": execution_id,
            "definition_id": &def.id,
            "definition_name": &def.name,
            "status": "completed",
        })).ok();

        let ctx = context.lock().await.clone();
        // 过滤内部变量（__ 前缀），仅返回用户数据作为工作流输出
        let output: serde_json::Map<String, Value> = ctx.into_iter()
            .filter(|(k, _)| !k.starts_with("__"))
            .collect();
        let output_value = Value::Object(output);
        // -- outputSchema 格式化 --
        Ok(Self::format_output(output_value, &def.output_schema))
    }

    /// 从 checkpoint 恢复执行
    pub async fn recover_execution(
        executor: &Arc<NodeExecutor>,
        def: &WorkflowDefinition,
        execution_id: &str,
        input_data: Value,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
    ) -> Result<Value, AppError> {
        let db_conn = get_db_conn(emitter)?;

        let (_completed_nodes, recovered_stages) = {
            let mut stmt = db_conn.prepare(
                "SELECT node_id, status FROM node_executions WHERE execution_id = ?1"
            ).map_err(|e| AppError::Db(e.to_string()))?;

            let node_results: Vec<(String, String)> = stmt.query_map(
                rusqlite::params![execution_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            ).map_err(|e| AppError::Db(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Db(e.to_string()))?;

            let completed_nodes: std::collections::HashSet<String> = node_results.iter()
                .filter(|(_, status)| status == "completed")
                .map(|(id, _)| id.clone())
                .collect();

            let mut recovered_stages = Vec::new();
            for stage in &def.stages {
                let pending_nodes: Vec<WorkflowNode> = stage.nodes.iter()
                    .filter(|n| !completed_nodes.contains(&n.id))
                    .cloned()
                    .collect();
                if pending_nodes.is_empty() { continue; }
                let pending_ids: std::collections::HashSet<&str> = pending_nodes.iter()
                    .map(|n| n.id.as_str()).collect();
                let filtered_edges: Vec<WorkflowEdge> = stage.edges.iter()
                    .filter(|e| pending_ids.contains(e.source.as_str()) && pending_ids.contains(e.target.as_str()))
                    .cloned()
                    .collect();
                recovered_stages.push(Stage {
                    id: stage.id.clone(), name: stage.name.clone(), order: stage.order,
                    nodes: pending_nodes, edges: filtered_edges, gate: stage.gate.clone(),
                });
            }
            (completed_nodes, recovered_stages)
        };


        if recovered_stages.is_empty() {
            emitter.emit("workflow:execution-status", serde_json::json!({
                "execution_id": execution_id, "definition_id": &def.id, "definition_name": &def.name,
                "status": "completed", "message": "所有节点已完成",
            })).ok();
            return Ok(Value::Null);
        }

        let recovered_def = WorkflowDefinition {
            id: def.id.clone(),
            name: format!("{} (恢复)", def.name),
            version: def.version.clone(),
            description: def.description.clone(),
            trigger: def.trigger.clone(),
            stages: recovered_stages,
            input_schema: def.input_schema.clone(),
            output_schema: def.output_schema.clone(),
            max_depth: def.max_depth,
            created_at: def.created_at,
            updated_at: crate::utils::now(),
            enabled: def.enabled,
        };

        let _ = db_conn.execute(
            "UPDATE workflow_instances SET status = 'running', error = NULL, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![crate::utils::now(), execution_id],
        );

        Self::execute_with_concurrency_impl(executor, &recovered_def, execution_id, input_data, emitter, max_concurrency, &[]).await
    }
}

/// 将 WorkflowNode 转换为 NodeDef（执行器上下文）
fn node_to_node_def(node: &WorkflowNode) -> NodeDef {
    NodeDef {
        id: node.id.clone(),
        node_type: format!("{:?}", node.node_type).to_lowercase(),
        label: node.label.clone(),
        config: node.params.clone().unwrap_or(Value::Object(serde_json::Map::new())),
        plugin_id: node.plugin_id.clone(),
        command_id: node.command_id.clone(),
        timeout_seconds: node.timeout_ms.map(|ms| ms / 1000),
        retry_count: node.retry_count,
        retry_interval_ms: node.retry_delay_ms,
    }
}

/// 解析节点输入（模板变量替换）
fn resolve_node_input(node: &WorkflowNode, context: &HashMap<String, Value>) -> Value {
    if let Some(mapping) = &node.input_mapping {
        if let Ok(map) = serde_json::from_value::<HashMap<String, String>>(mapping.clone()) {
            let mut resolved = serde_json::Map::new();
            for (key, template) in &map {
                let resolve_result = TemplateEngine::resolve(template, context);
                match resolve_result {
                    Ok(value) => {
                        resolved.insert(key.clone(), Value::String(value));
                    }
                    Err(e) => {
                        log::warn!(
                            "[resolve_node_input] 节点 {} 的 inputMapping[{}] 模板解析失败: {} (模板=\"{}\")",
                            node.id, key, e, template
                        );
                        // 解析失败时保留原始模板字符串，便于排查
                        resolved.insert(key.clone(), Value::String(template.clone()));
                    }
                }
            }
            return Value::Object(resolved);
        }
    }
    Value::Object(serde_json::Map::new())
}
