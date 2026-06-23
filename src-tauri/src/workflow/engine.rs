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
use super::{WorkflowDefinition, WorkflowNode, WorkflowNodeType, WorkflowEdge, Stage, MergeStrategy};

/// 从 AppHandle 获取数据库连接
fn get_db_conn(app_handle: &tauri::AppHandle) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, AppError> {
    app_handle.state::<crate::DbState>().get_conn()
        .map_err(|e| AppError::External(format!("数据库连接失败: {}", e)))
}

/// 写入节点执行记录
fn insert_node_execution(
    conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    execution_id: &str,
    node_id: &str,
    status: &str,
    input_data: Option<&str>,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "INSERT OR REPLACE INTO node_executions (id, execution_id, node_id, status, input_data, started_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
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
    conn.execute(
        "UPDATE node_executions SET status = ?1, output_data = COALESCE(?2, output_data),
         error_message = ?3, finished_at = ?4
         WHERE execution_id = ?5 AND node_id = ?6",
        rusqlite::params![status, output_data, error_message, now, execution_id, node_id],
    )?;
    Ok(())
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
    ) -> Result<Value, AppError> {
        let layers = Self::topological_sort(&stage.nodes, &stage.edges)?;
        let node_map: HashMap<String, &WorkflowNode> = stage.nodes.iter().map(|n| (n.id.clone(), n)).collect();

        for layer in &layers {
            let mut handles = Vec::new();

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
                        completed_count.fetch_add(1, Ordering::SeqCst);
                        if let Ok(conn) = get_db_conn(emitter) {
                            let _ = insert_node_execution(&conn, execution_id, node_id, "skipped", None);
                        }
                        emitter.emit("workflow:node-status", serde_json::json!({
                            "execution_id": execution_id,
                            "node_id": node_id,
                            "status": "skipped",
                        })).ok();
                        continue;
                    }
                }

                // Subflow 节点：在 spawn 之外同步执行（避免 Send 约束问题）
                if node.node_type == WorkflowNodeType::Subflow {
                    let ctx_snapshot = context.lock().await.clone();
                    let resolved_input = resolve_node_input(&node, &ctx_snapshot);

                    // 写入开始记录
                    if let Ok(conn) = get_db_conn(emitter) {
                        let input_str = serde_json::to_string(&resolved_input).ok();
                        let _ = insert_node_execution(&conn, execution_id, node_id, "running", input_str.as_deref());
                    }

                    emitter.emit("workflow:node-status", serde_json::json!({
                        "execution_id": execution_id, "node_id": node_id, "status": "running",
                    })).ok();

                    match Self::execute_subflow_node(
                        executor, &node, resolved_input,
                        execution_id, emitter, max_concurrency,
                        visited_def_ids,
                    ).await {
                        Ok(output) => {
                            context.lock().await.insert(node_id.clone(), output.clone());
                            completed_count.fetch_add(1, Ordering::SeqCst);

                            if let Ok(conn) = get_db_conn(emitter) {
                                let output_str = serde_json::to_string(&output).ok();
                                let _ = update_node_execution(&conn, execution_id, node_id, "completed", output_str.as_deref(), None);
                            }

                            emitter.emit("workflow:node-status", serde_json::json!({
                                "execution_id": execution_id, "node_id": node_id, "status": "completed", "output": output,
                            })).ok();
                            emitter.emit("workflow:progress", serde_json::json!({
                                "execution_id": execution_id, "completed": completed_count.load(Ordering::SeqCst), "total": total_count,
                            })).ok();
                        }
                        Err(e) => {
                            if let Ok(conn) = get_db_conn(emitter) {
                                let _ = update_node_execution(&conn, execution_id, node_id, "failed", None, Some(&e.to_string()));
                            }

                            emitter.emit("workflow:node-status", serde_json::json!({
                                "execution_id": execution_id, "node_id": node_id, "status": "failed", "error": e.to_string(),
                            })).ok();
                        }
                    }
                    continue;
                }

                let exec_id = execution_id.to_string();
                let nid = node_id.clone();
                let emitter = emitter.clone();
                let context = context.clone();
                let completed_count = completed_count.clone();
                let total = total_count;
                let exec = executor.clone();
                let permit = semaphore.clone().acquire_owned().await;
                let _permit = match permit {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let handle = tokio::spawn(async move {
                    let ctx_snapshot = context.lock().await.clone();
                    let resolved_input = resolve_node_input(&node, &ctx_snapshot);

                    // 写入开始记录
                    if let Ok(conn) = get_db_conn(&emitter) {
                        let input_str = serde_json::to_string(&resolved_input).ok();
                        let _ = insert_node_execution(&conn, &exec_id, &nid, "running", input_str.as_deref());
                    }

                    emitter.emit("workflow:node-status", serde_json::json!({
                        "execution_id": exec_id, "node_id": nid, "status": "running",
                    })).ok();

                    let node_def = node_to_node_def(&node);

                    let result = exec.execute(&node_def, resolved_input.clone(), &exec_id, &emitter).await;

                    match result {
                        Ok(output) => {
                            let node_output = output.output.clone();
                            context.lock().await.insert(nid.clone(), node_output.clone());
                            completed_count.fetch_add(1, Ordering::SeqCst);

                            if let Ok(conn) = get_db_conn(&emitter) {
                                let output_str = serde_json::to_string(&node_output).ok();
                                let _ = update_node_execution(&conn, &exec_id, &nid, "completed", output_str.as_deref(), None);
                            }

                            emitter.emit("workflow:node-status", serde_json::json!({
                                "execution_id": exec_id, "node_id": nid, "status": "completed", "output": node_output,
                            })).ok();
                            emitter.emit("workflow:progress", serde_json::json!({
                                "execution_id": exec_id, "completed": completed_count.load(Ordering::SeqCst), "total": total,
                            })).ok();
                        }
                        Err(e) => {
                            if let Ok(conn) = get_db_conn(&emitter) {
                                let _ = update_node_execution(&conn, &exec_id, &nid, "failed", None, Some(&e.to_string()));
                            }

                            emitter.emit("workflow:node-status", serde_json::json!({
                                "execution_id": exec_id, "node_id": nid, "status": "failed", "error": e.to_string(),
                            })).ok();
                        }
                    }
                });

                handles.push(handle);
            }

            for handle in handles {
                let _ = handle.await;
            }
        }

        Ok(Value::Null)
    }

    /// 评估边上的条件表达式
    /// 支持：==, !=, >, <, >=, <=, contains
    fn evaluate_condition(condition: &str, source_output: Option<&Value>) -> bool {
        match source_output {
            Some(output) => {
                let output_str = output.to_string();
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
            MergeStrategy::Custom(ref script) => {
                Value::String(format!("custom_merge:{}", script))
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
        let cancelled = Arc::new(AtomicBool::new(false));
        let total_count: usize = def.stages.iter().map(|s| s.nodes.len()).sum();
        let completed_count = Arc::new(AtomicUsize::new(0));

        let context: Arc<AsyncMutex<HashMap<String, Value>>> = Arc::new(AsyncMutex::new(HashMap::new()));
        context.lock().await.insert("__input__".to_string(), input_data.clone());

        let semaphore = Arc::new(Semaphore::new(max_concurrency));

        for stage in &def.stages {
            if cancelled.load(Ordering::SeqCst) {
                return Err(AppError::External("工作流已被取消".into()));
            }

            emitter.emit("workflow:stage-status", serde_json::json!({
                "execution_id": execution_id,
                "stage_id": stage.id,
                "stage_name": stage.name,
                "status": "running",
            })).ok();

            Self::execute_stage(
                executor, stage, execution_id,
                &context, emitter, &cancelled,
                &completed_count, total_count, &semaphore,
                max_concurrency,
                visited_def_ids,
            ).await?;

            let ctx = context.lock().await.clone();
            let merged = Self::merge_stage_outputs(stage, &ctx);
            context.lock().await.insert(format!("__stage_{}_output__", stage.order), merged);

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
            "status": "completed",
        })).ok();

        let ctx = context.lock().await.clone();
        Ok(Value::Object(ctx.into_iter().collect()))
    }

    /// 从 checkpoint 恢复执行
    #[allow(dead_code)]
    pub async fn recover_execution(
        executor: &Arc<NodeExecutor>,
        def: &WorkflowDefinition,
        execution_id: &str,
        input_data: Value,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
    ) -> Result<Value, AppError> {
        let db_conn = get_db_conn(emitter)?;

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

            if pending_nodes.is_empty() {
                continue;
            }

            let pending_ids: std::collections::HashSet<&str> = pending_nodes.iter()
                .map(|n| n.id.as_str()).collect();
            let filtered_edges: Vec<WorkflowEdge> = stage.edges.iter()
                .filter(|e| pending_ids.contains(e.source.as_str()) && pending_ids.contains(e.target.as_str()))
                .cloned()
                .collect();

            recovered_stages.push(Stage {
                id: stage.id.clone(),
                name: stage.name.clone(),
                order: stage.order,
                nodes: pending_nodes,
                edges: filtered_edges,
                gate: stage.gate.clone(),
            });
        }

        if recovered_stages.is_empty() {
            emitter.emit("workflow:execution-status", serde_json::json!({
                "execution_id": execution_id, "status": "completed", "message": "所有节点已完成",
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
                let value = TemplateEngine::resolve(template, context).unwrap_or_else(|_| template.clone());
                resolved.insert(key.clone(), Value::String(value));
            }
            return Value::Object(resolved);
        }
    }
    Value::Object(serde_json::Map::new())
}
