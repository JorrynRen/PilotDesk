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
use super::{WorkflowDefinition, WorkflowNode, WorkflowEdge};

#[allow(dead_code)]
/// 节点运行时状态
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
/// 执行上下文
pub struct ExecutionContext {
    pub execution_id: String,
    pub cancelled: Arc<AtomicBool>,
    pub node_states: Arc<AsyncMutex<HashMap<String, NodeRuntimeState>>>,
    pub completed_count: Arc<AtomicUsize>,
    pub total_count: usize,
}

/// 默认最大并行节点数
#[allow(dead_code)]
const DEFAULT_MAX_CONCURRENCY: usize = 5;

/// DAG 调度引擎
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

    /// 启动工作流执行（可指定最大并行数）
    pub async fn execute_with_concurrency(
        executor: &Arc<NodeExecutor>,
        def: &WorkflowDefinition,
        execution_id: &str,
        input_data: Value,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
    ) -> Result<(), AppError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let node_states: Arc<AsyncMutex<HashMap<String, NodeRuntimeState>>> = Arc::new(AsyncMutex::new(HashMap::new()));
        let total_count = def.nodes.len();
        let completed_count = Arc::new(AtomicUsize::new(0));

        // 拓扑排序
        let layers = Self::topological_sort(&def.nodes, &def.edges)?;

        // 共享上下文（Arc<Mutex> 以支持并行节点写入）
        let context: Arc<AsyncMutex<HashMap<String, Value>>> = Arc::new(AsyncMutex::new(HashMap::new()));
        context.lock().await.insert("__input__".to_string(), input_data.clone());

        // 构建节点映射
        let node_map: HashMap<String, &WorkflowNode> = def.nodes.iter().map(|n| (n.id.clone(), n)).collect();

        // 创建信号量控制并发度
        let semaphore = Arc::new(Semaphore::new(max_concurrency));

        // 获取数据库连接用于记录节点执行日志
        let db_conn = match emitter.state::<crate::DbState>().get_conn() {
            Ok(conn) => Some(conn),
            Err(_) => None,
        };
        let db_conn = Arc::new(AsyncMutex::new(db_conn));

        // 逐层执行
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

                let exec_id = execution_id.to_string();
                let nid = node_id.clone();
                let emitter = emitter.clone();
                let node_states = node_states.clone();
                let context = context.clone();
                let completed_count = completed_count.clone();
                let total = total_count;
                let _cancel = cancelled.clone();
                let exec = executor.clone();

                // 获取信号量许可（控制并发度）
                let permit = semaphore.clone().acquire_owned().await;
                let _permit = match permit {
                    Ok(p) => p,
                    Err(_) => {
                        emitter.emit("workflow:log", serde_json::json!({
                            "execution_id": exec_id,
                            "node_execution_id": nid,
                            "level": "warn",
                            "message": "并发控制信号量关闭，跳过节点执行",
                        })).ok();
                        continue;
                    }
                };

                let db_conn_clone = db_conn.clone();
                let handle = tokio::spawn(async move {
                    // 更新状态为 running
                    node_states.lock().await.insert(nid.clone(), NodeRuntimeState {
                        status: NodeStatus::Running,
                        output: None,
                        error: None,
                    });

                    emitter.emit("workflow:node-status", serde_json::json!({
                        "execution_id": exec_id,
                        "node_id": nid,
                        "status": "running",
                    })).ok();

                    // 解析输入（从共享上下文读取）
                    let ctx_snapshot = context.lock().await.clone();
                    let resolved_input = resolve_node_input(&node, &ctx_snapshot);

                    // 构建 NodeDef
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

                    // 执行节点
                    let input_clone = resolved_input.clone();
                    let result = exec.execute(&node_def, resolved_input, &exec_id, &emitter).await;
                    let resolved_input = input_clone;

                    match result {
                        Ok(output) => {
                            let node_output = output.output.clone();
                            // 写入共享上下文
                            context.lock().await.insert(nid.clone(), output.output);
                            completed_count.fetch_add(1, Ordering::SeqCst);

                            let mut states = node_states.lock().await;
                            if let Some(state) = states.get_mut(&nid) {
                                state.status = NodeStatus::Completed;
                                state.output = Some(node_output.clone());
                            }

                            // 记录节点执行到数据库
                            if let Some(ref conn) = *db_conn_clone.lock().await {
                                let node_exec_id = crate::utils::new_id();
                                let now_ts = crate::utils::now();
                                let _ = conn.execute(
                                    "INSERT INTO node_executions (id, execution_id, node_id, status, input_data, output_data, started_at, finished_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                                    rusqlite::params![
                                        node_exec_id, exec_id, nid,
                                        "completed",
                                        serde_json::to_string(&resolved_input).unwrap_or_default(),
                                        serde_json::to_string(&node_output).unwrap_or_default(),
                                        now_ts, now_ts, now_ts, now_ts,
                                    ],
                                );
                                let _ = conn.execute(
                                    "INSERT INTO node_execution_logs (execution_id, node_execution_id, timestamp, level, message) VALUES (?1, ?2, ?3, ?4, ?5)",
                                    rusqlite::params![exec_id, node_exec_id, now_ts, "info", format!("节点 '{}' 执行成功", node.label)],
                                );
                            }

                            emitter.emit("workflow:node-status", serde_json::json!({
                                "execution_id": exec_id,
                                "node_id": nid,
                                "status": "completed",
                                "output": node_output,
                            })).ok();

                            emitter.emit("workflow:progress", serde_json::json!({
                                "execution_id": exec_id,
                                "completed": completed_count.load(Ordering::SeqCst),
                                "total": total,
                            })).ok();
                        }
                        Err(e) => {
                            let mut states = node_states.lock().await;
                            if let Some(state) = states.get_mut(&nid) {
                                state.status = NodeStatus::Failed;
                                state.error = Some(e.to_string());
                            }

                            // 记录节点执行失败到数据库
                            if let Some(ref conn) = *db_conn_clone.lock().await {
                                let node_exec_id = crate::utils::new_id();
                                let now_ts = crate::utils::now();
                                let _ = conn.execute(
                                    "INSERT INTO node_executions (id, execution_id, node_id, status, input_data, output_data, error_message, started_at, finished_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                                    rusqlite::params![
                                        node_exec_id, exec_id, nid,
                                        "failed",
                                        serde_json::to_string(&resolved_input).unwrap_or_default(),
                                        "null",
                                        e.to_string(),
                                        now_ts, now_ts, now_ts, now_ts,
                                    ],
                                );
                                let _ = conn.execute(
                                    "INSERT INTO node_execution_logs (execution_id, node_execution_id, timestamp, level, message) VALUES (?1, ?2, ?3, ?4, ?5)",
                                    rusqlite::params![exec_id, node_exec_id, now_ts, "error", format!("节点 '{}' 执行失败: {}", node.label, e)],
                                );
                            }

                            emitter.emit("workflow:node-status", serde_json::json!({
                                "execution_id": exec_id,
                                "node_id": nid,
                                "status": "failed",
                                "error": e.to_string(),
                            })).ok();
                        }
                    }
                });

                handles.push(handle);
            }

            // 等待当前层所有节点完成
            for handle in handles {
                let _ = handle.await;
            }
        }

        // 完成
        emitter.emit("workflow:execution-status", serde_json::json!({
            "execution_id": execution_id,
            "status": "completed",
        })).ok();

        Ok(())
    }

    /// 从 checkpoint 恢复执行
    /// 读取已完成的 node_executions，跳过已完成节点，重试失败节点
    #[allow(dead_code)]
    pub async fn recover_execution(
        executor: &Arc<NodeExecutor>,
        def: &WorkflowDefinition,
        execution_id: &str,
        input_data: Value,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
    ) -> Result<(), AppError> {
        // 获取数据库连接，查询已完成的节点
        let db_conn = match emitter.state::<crate::DbState>().get_conn() {
            Ok(conn) => conn,
            Err(_) => {
                return Err(AppError::External("无法获取数据库连接".into()));
            }
        };

        // 查询该执行实例的所有节点执行记录
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

        let _failed_nodes: std::collections::HashSet<String> = node_results.iter()
            .filter(|(_, status)| status == "failed")
            .map(|(id, _)| id.clone())
            .collect();

        // 过滤出未完成的节点（排除已完成节点）
        let pending_nodes: Vec<&super::WorkflowNode> = def.nodes.iter()
            .filter(|n| !completed_nodes.contains(&n.id))
            .collect();

        if pending_nodes.is_empty() {
            emitter.emit("workflow:execution-status", serde_json::json!({
                "execution_id": execution_id,
                "status": "completed",
                "message": "所有节点已完成，无需恢复",
            })).ok();
            return Ok(());
        }

        // 从 pending_nodes 重建边（只保留两端都在 pending 中的边）
        let pending_ids: std::collections::HashSet<&str> = pending_nodes.iter()
            .map(|n| n.id.as_str()).collect();
        let filtered_edges: Vec<&super::WorkflowEdge> = def.edges.iter()
            .filter(|e| pending_ids.contains(e.source.as_str()) && pending_ids.contains(e.target.as_str()))
            .collect();

        // 构建过滤后的 DAG 定义
        let filtered_def = super::WorkflowDefinition {
            id: def.id.clone(),
            name: format!("{} (恢复)", def.name),
            version: def.version.clone(),
            description: def.description.clone(),
            nodes: pending_nodes.into_iter().cloned().collect(),
            edges: filtered_edges.into_iter().cloned().collect(),
            input_schema: def.input_schema.clone(),
            output_schema: def.output_schema.clone(),
            max_depth: def.max_depth,
            created_at: def.created_at,
            updated_at: crate::utils::now(),
            enabled: def.enabled,
        };

        // 更新实例状态为 running
        let _ = db_conn.execute(
            "UPDATE workflow_instances SET status = 'running', error = NULL, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![crate::utils::now(), execution_id],
        );

        // 使用过滤后的 DAG 重新执行
        Self::execute_with_concurrency(
            executor,
            &filtered_def,
            execution_id,
            input_data,
            emitter,
            max_concurrency,
        ).await
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{WorkflowNode, WorkflowEdge, WorkflowNodeType};

    fn make_node(id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            node_type: WorkflowNodeType::PluginCommand,
            label: format!("节点 {}", id),
            plugin_id: None,
            command_id: None,
            params: None,
            condition: None,
            cron: None,
            event_name: None,
            delay_ms: None,
            timeout_ms: None,
            retry_count: None,
            retry_delay_ms: None,
            input_mapping: None,
            output_mapping: None,
            position: None,
        }
    }

    fn make_edge(source: &str, target: &str) -> WorkflowEdge {
        WorkflowEdge {
            id: format!("{}-{}", source, target),
            source: source.to_string(),
            target: target.to_string(),
            label: None,
            condition: None,
        }
    }

    #[test]
    fn test_topological_sort_simple() {
        let nodes = vec![make_node("a"), make_node("b"), make_node("c")];
        let edges = vec![make_edge("a", "b"), make_edge("b", "c")];
        let layers = WorkflowEngine::topological_sort(&nodes, &edges).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["a"]);
        assert_eq!(layers[1], vec!["b"]);
        assert_eq!(layers[2], vec!["c"]);
    }

    #[test]
    fn test_topological_sort_parallel() {
        let nodes = vec![make_node("a"), make_node("b"), make_node("c")];
        let edges = vec![make_edge("a", "c"), make_edge("b", "c")];
        let layers = WorkflowEngine::topological_sort(&nodes, &edges).unwrap();
        assert_eq!(layers.len(), 2);
        // a 和 b 在同一层（并行）
        assert_eq!(layers[0].len(), 2);
        assert!(layers[0].contains(&"a".to_string()));
        assert!(layers[0].contains(&"b".to_string()));
        assert_eq!(layers[1], vec!["c"]);
    }

    #[test]
    fn test_topological_sort_diamond() {
        let nodes = vec![make_node("a"), make_node("b"), make_node("c"), make_node("d")];
        let edges = vec![
            make_edge("a", "b"),
            make_edge("a", "c"),
            make_edge("b", "d"),
            make_edge("c", "d"),
        ];
        let layers = WorkflowEngine::topological_sort(&nodes, &edges).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["a"]);
        assert_eq!(layers[1].len(), 2); // b 和 c 并行
        assert_eq!(layers[2], vec!["d"]);
    }

    #[test]
    fn test_topological_sort_cycle_detection() {
        let nodes = vec![make_node("a"), make_node("b")];
        let edges = vec![make_edge("a", "b"), make_edge("b", "a")];
        let result = WorkflowEngine::topological_sort(&nodes, &edges);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("循环依赖"));
    }

    #[test]
    fn test_topological_sort_single_node() {
        let nodes = vec![make_node("a")];
        let edges: Vec<WorkflowEdge> = vec![];
        let layers = WorkflowEngine::topological_sort(&nodes, &edges).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0], vec!["a"]);
    }

    #[test]
    fn test_topological_sort_disconnected() {
        let nodes = vec![make_node("a"), make_node("b"), make_node("c")];
        let edges = vec![make_edge("a", "b")]; // c 是孤立的
        let layers = WorkflowEngine::topological_sort(&nodes, &edges).unwrap();
        // 第一层应包含 a 和 c（入度都为 0）
        assert!(layers[0].contains(&"a".to_string()));
        assert!(layers[0].contains(&"c".to_string()));
        assert_eq!(layers[1], vec!["b"]);
    }

    #[test]
    fn test_resolve_node_input_no_mapping() {
        let node = make_node("a");
        let context = HashMap::new();
        let result = resolve_node_input(&node, &context);
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_resolve_node_input_with_mapping() {
        let mut node = make_node("a");
        let mut map = serde_json::Map::new();
        map.insert("key1".to_string(), Value::String("{{__input__.output}}".to_string()));
        node.input_mapping = Some(Value::Object(map));

        let mut context = HashMap::new();
        let mut input_obj = serde_json::Map::new();
        input_obj.insert("output".to_string(), Value::String("hello".to_string()));
        context.insert("__input__".to_string(), Value::Object(input_obj));

        let result = resolve_node_input(&node, &context);
        if let Value::Object(obj) = result {
            assert_eq!(obj.get("key1").and_then(|v| v.as_str()), Some("hello"));
        } else {
            panic!("期望 Object，得到 {:?}", result);
        }
    }
}
