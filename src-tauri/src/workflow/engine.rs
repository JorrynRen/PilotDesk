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
use super::{WorkflowDefinition, WorkflowNode, WorkflowEdge, Stage, MergeStrategy};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NodeRuntimeState {
    pub status: NodeStatus,
    pub output: Option<Value>,
    pub error: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum NodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[allow(dead_code)]
pub struct ExecutionContext {
    pub execution_id: String,
    pub cancelled: Arc<AtomicBool>,
    pub node_states: Arc<AsyncMutex<HashMap<String, NodeRuntimeState>>>,
    pub completed_count: Arc<AtomicUsize>,
    pub total_count: usize,
}

#[allow(dead_code)]
const DEFAULT_MAX_CONCURRENCY: usize = 5;

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
    ) -> Result<(), AppError> {
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
                        // 条件不满足，跳过此节点
                        completed_count.fetch_add(1, Ordering::SeqCst);
                        emitter.emit("workflow:node-status", serde_json::json!({
                            "execution_id": execution_id,
                            "node_id": node_id,
                            "status": "skipped",
                        })).ok();
                        continue;
                    }
                }

                let exec_id = execution_id.to_string();
                let nid = node_id.clone();
                let emitter = emitter.clone();
                let context = context.clone();
                let completed_count = completed_count.clone();
                let total = total_count;
                let _cancel = cancelled.clone();
                let exec = executor.clone();
                let permit = semaphore.clone().acquire_owned().await;
                let _permit = match permit {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                
                let handle = tokio::spawn(async move {
                    emitter.emit("workflow:node-status", serde_json::json!({
                        "execution_id": exec_id, "node_id": nid, "status": "running",
                    })).ok();

                    let ctx_snapshot = context.lock().await.clone();
                    let resolved_input = resolve_node_input(&node, &ctx_snapshot);

                    let node_def = NodeDef {
                        id: node.id.clone(),
                        node_type: format!("{:?}", node.node_type).to_lowercase(),
                        label: node.label.clone(),
                        config: node.params.clone().unwrap_or(Value::Object(serde_json::Map::new())),
                        plugin_id: node.plugin_id.clone(),
                        command_id: node.command_id.clone(),
                        timeout_seconds: node.timeout_ms.map(|ms| ms / 1000),
                        retry_count: node.retry_count,
                        retry_interval_ms: node.retry_delay_ms,
                    };

                    let result = exec.execute(&node_def, resolved_input.clone(), &exec_id, &emitter).await;

                    match result {
                        Ok(output) => {
                            let node_output = output.output.clone();
                            context.lock().await.insert(nid.clone(), node_output.clone());
                            completed_count.fetch_add(1, Ordering::SeqCst);

                            emitter.emit("workflow:node-status", serde_json::json!({
                                "execution_id": exec_id, "node_id": nid, "status": "completed", "output": node_output,
                            })).ok();
                            emitter.emit("workflow:progress", serde_json::json!({
                                "execution_id": exec_id, "completed": completed_count.load(Ordering::SeqCst), "total": total,
                            })).ok();
                        }
                        Err(e) => {
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

        Ok(())
    }

    /// 评估边上的条件表达式
    fn evaluate_condition(condition: &str, source_output: Option<&Value>) -> bool {
        match source_output {
            Some(output) => {
                let output_str = output.to_string();
                // 简单条件评估：支持 ==, !=, >, <, contains
                if let Some(val) = condition.strip_prefix("==") {
                    return output_str.trim_matches('"') == val.trim();
                }
                if let Some(val) = condition.strip_prefix("!=") {
                    return output_str.trim_matches('"') != val.trim();
                }
                if let Some(val) = condition.strip_prefix("contains") {
                    return output_str.contains(val.trim());
                }
                // 默认：条件表达式作为 JavaScript 表达式（通过 transform executor 评估）
                // 简单场景：非空即真
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
                // 自定义合并脚本（由 transform executor 执行）
                Value::String(format!("custom_merge:{}", script))
            }
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
    ) -> Result<(), AppError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let _node_states: Arc<AsyncMutex<HashMap<String, NodeRuntimeState>>> = Arc::new(AsyncMutex::new(HashMap::new()));
        let total_count: usize = def.stages.iter().map(|s| s.nodes.len()).sum();
        let completed_count = Arc::new(AtomicUsize::new(0));

        let context: Arc<AsyncMutex<HashMap<String, Value>>> = Arc::new(AsyncMutex::new(HashMap::new()));
        context.lock().await.insert("__input__".to_string(), input_data.clone());

        let semaphore = Arc::new(Semaphore::new(max_concurrency));

        // 外层调度：阶段串行
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

            // 内层调度：阶段内 DAG
            Self::execute_stage(
                executor, stage, execution_id,
                &context, emitter, &cancelled,
                &completed_count, total_count, &semaphore,
            ).await?;

            // 执行 Gate 合并
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

        emitter.emit("workflow:execution-status", serde_json::json!({
            "execution_id": execution_id,
            "status": "completed",
        })).ok();

        Ok(())
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
    ) -> Result<(), AppError> {
        let db_conn = match emitter.state::<crate::DbState>().get_conn() {
            Ok(conn) => conn,
            Err(_) => return Err(AppError::External("无法获取数据库连接".into())),
        };

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

        // 过滤每个阶段中未完成的节点
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
            return Ok(());
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

        Self::execute_with_concurrency(executor, &recovered_def, execution_id, input_data, emitter, max_concurrency).await
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
    Value::Null
}
