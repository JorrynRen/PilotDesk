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
    agent_session_id: Option<&str>,
    input_data: Option<&str>,
    artifacts_path: Option<&str>,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    // agent_session_id 由调用方直接传入（从 NodeOutput.session_id 获取）
    // input_data 由调用方传入（completed 时覆盖 running 时写入的输入映射内容）
    // artifacts_path 由调用方传入（节点执行工件路径）
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
         agent_session_id = COALESCE(?6, agent_session_id),
         input_data = COALESCE(?7, input_data),
         artifacts_path = COALESCE(?8, artifacts_path)
         WHERE execution_id = ?9 AND node_id = ?10",
        rusqlite::params![
            status, output_data, error_message, now,
            duration_ms, agent_session_id,
            input_data, artifacts_path,
            execution_id, node_id,
        ],
    )?;
    Ok(())
}

/// 写入节点执行记录并发送状态事件
fn emit_node_status(
    emitter: &tauri::AppHandle,
    execution_id: &str,
    definition_id: &str,
    node_id: &str,
    status: &str,
    output: Option<&Value>,
    error: Option<&str>,
    completed_count: Option<usize>,
    total: Option<usize>,
    mode: &ExecutionMode,
) {
    let mut node_payload = serde_json::json!({
        "id": node_id,
        "status": status,
    });
    if let Some(out) = output {
        node_payload["output"] = out.clone();
    }
    if let Some(err) = error {
        node_payload["error"] = err.into();
    }
    emit_progress(
        emitter, execution_id, definition_id, mode,
        Some(node_payload), None, None,
        completed_count.zip(total),
    );
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
    agent_session_id: Option<&str>,
    input_data_override: Option<&str>,
    artifacts_path: Option<&str>,
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
            update_node_execution(&conn, execution_id, node_id, status, output_data, error_message, agent_session_id, input_data_override, artifacts_path)
        }
    };
    if let Err(e) = result {
        log::warn!("[WorkflowEngine] 写入节点执行记录失败 (execution={}, node={}, status={}): {}",
            execution_id, node_id, status, e);
    }
}

/// 两层调度引擎：阶段串行 → 阶段内 DAG
pub struct WorkflowEngine;

/// 执行模式（预留：支持完整执行、单点执行、断点执行）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ExecutionMode {
    /// 完整执行（默认）：从 start 节点执行到 end 节点
    Full,
    /// 单点执行：仅执行选中的单个节点（预留）
    #[allow(dead_code)]
    SingleNode { node_id: String },
    /// 断点执行：从选中节点执行至 end 节点（预留）
    #[allow(dead_code)]
    FromNode { node_id: String },
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::Full
    }
}

/// 校验结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationResult {
    /// 是否通过（无 error 级别检查项）
    pub ok: bool,
    /// 校验详情列表
    pub checks: Vec<ValidationCheck>,
}

/// 单项校验结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationCheck {
    /// 校验类型标识
    pub check_type: String,
    /// 严重级别
    pub severity: String, // "error" | "warning" | "info"
    /// 可读消息
    pub message: String,
    /// 可选详情（如涉及的节点/阶段 ID）
    pub details: Option<serde_json::Value>,
}

/// 执行计划：描述工作流的拓扑执行顺序和可达节点
#[derive(serde::Serialize, Clone, Debug)]
pub struct ExecutionPlan {
    /// 从 start 节点可达的所有节点 ID
    pub reachable_node_ids: Vec<String>,
    /// 分层阶段 ID（同层无依赖关系，可并行执行）
    pub ordered_stage_ids: Vec<Vec<String>>,
}

/// Drop 时自动注销执行注册
struct ExecutionGuard<'a> {
    executor: &'a NodeExecutor,
    execution_id: String,
}

impl WorkflowEngine {
    /// 工作流执行前校验（统一前置检查）
    ///
    /// 检查项：start/end 唯一性、完整路径、阶段内循环依赖、阶段连线环路、
    ///         未就绪节点统计、子工作流全链路深度/循环引用预检、空阶段检测
    pub fn validate_workflow_for_execution(
        def: &WorkflowDefinition,
        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
        _mode: &ExecutionMode,
    ) -> Result<ValidationResult, AppError> {
        let mut checks: Vec<ValidationCheck> = Vec::new();
        let mut has_error = false;

        // ── 1. start 节点唯一性 ──
        let start_nodes: Vec<&WorkflowNode> = def.stages.iter()
            .flat_map(|s| s.nodes.iter())
            .filter(|n| n.node_type == WorkflowNodeType::Start)
            .collect();
        let start_ok = start_nodes.len() == 1;
        checks.push(ValidationCheck {
            check_type: "boundary_start".into(),
            severity: if start_ok { "info".into() } else { "error".into() },
            message: if start_ok {
                format!("起始节点唯一 (id={})", start_nodes[0].id)
            } else {
                format!("起始节点数量异常: 期望 1 个，实际 {} 个", start_nodes.len())
            },
            details: if !start_ok {
                Some(serde_json::json!({ "ids": start_nodes.iter().map(|n| &n.id).collect::<Vec<_>>() }))
            } else { None },
        });
        if !start_ok { has_error = true; }

        // ── 2. end 节点唯一性 ──
        let end_nodes: Vec<&WorkflowNode> = def.stages.iter()
            .flat_map(|s| s.nodes.iter())
            .filter(|n| n.node_type == WorkflowNodeType::End)
            .collect();
        let end_ok = end_nodes.len() == 1;
        checks.push(ValidationCheck {
            check_type: "boundary_end".into(),
            severity: if end_ok { "info".into() } else { "error".into() },
            message: if end_ok {
                format!("结束节点唯一 (id={})", end_nodes[0].id)
            } else {
                format!("结束节点数量异常: 期望 1 个，实际 {} 个", end_nodes.len())
            },
            details: if !end_ok {
                Some(serde_json::json!({ "ids": end_nodes.iter().map(|n| &n.id).collect::<Vec<_>>() }))
            } else { None },
        });
        if !end_ok { has_error = true; }

        // ── 3. 完整路径（start → end 可达）──
        if start_ok && end_ok {
            let plan = Self::compute_execution_plan(def);
            let end_reachable = plan.reachable_node_ids.contains(&end_nodes[0].id);
            checks.push(ValidationCheck {
                check_type: "path_complete".into(),
                severity: if end_reachable { "info".into() } else { "error".into() },
                message: if end_reachable {
                    "从起始节点到结束节点存在完整执行路径".into()
                } else {
                    "不存在从起始节点到结束节点的完整路径".into()
                },
                details: None,
            });
            if !end_reachable { has_error = true; }

            // ── 4. 未就绪节点统计 ──
            let all_node_ids: std::collections::HashSet<String> = def.stages.iter()
                .flat_map(|s| s.nodes.iter().map(|n| n.id.clone()))
                .collect();
            let reachable_set: std::collections::HashSet<String> =
                plan.reachable_node_ids.iter().cloned().collect();
            let unreachable_count = all_node_ids.len().saturating_sub(reachable_set.len());
            checks.push(ValidationCheck {
                check_type: "unreachable_nodes".into(),
                severity: if unreachable_count == 0 { "info".into() } else { "warning".into() },
                message: if unreachable_count == 0 {
                    "所有节点均可达".into()
                } else {
                    format!("存在 {} 个未就绪节点（不在执行路径上）", unreachable_count)
                },
                details: if unreachable_count > 0 {
                    let unreachable_ids: Vec<String> = all_node_ids.into_iter()
                        .filter(|id| !reachable_set.contains(id))
                        .collect();
                    Some(serde_json::json!({ "count": unreachable_count, "ids": unreachable_ids }))
                } else { None },
            });
        }

        // ── 5. 阶段内无循环依赖（Kahn 算法）──
        let mut has_cycle = false;
        let mut cycle_stages = Vec::new();
        for stage in &def.stages {
            if stage.nodes.len() <= 1 { continue; }
            match Self::topological_sort(&stage.nodes, &stage.edges) {
                Ok(_) => {}
                Err(_) => {
                    has_cycle = true;
                    cycle_stages.push(stage.id.clone());
                }
            }
        }
        checks.push(ValidationCheck {
            check_type: "node_cycle".into(),
            severity: if has_cycle { "error".into() } else { "info".into() },
            message: if has_cycle {
                format!("{} 个阶段内存在节点循环依赖", cycle_stages.len())
            } else {
                "所有阶段内均无循环依赖".into()
            },
            details: if has_cycle {
                Some(serde_json::json!({ "stage_ids": cycle_stages }))
            } else { None },
        });
        if has_cycle { has_error = true; }

        // ── 6. 阶段连线环路检测（Kahn 算法）──
        let stage_cycle_check = Self::check_stage_edge_cycle(def);
        if stage_cycle_check.severity == "error" {
            has_error = true;
        }
        checks.push(stage_cycle_check);

        // ── 7. 空阶段检测 ──
        let mut empty_stages = Vec::new();
        for stage in &def.stages {
            // 仅包含边界节点（开始/结束）的阶段是合法设计，不视为空阶段
            let only_boundary = stage.nodes.len() > 0 && stage.nodes.iter().all(|n|
                n.node_type == WorkflowNodeType::Start || n.node_type == WorkflowNodeType::End
            );
            if only_boundary { continue; }
            let non_boundary = stage.nodes.iter()
                .filter(|n| n.node_type != WorkflowNodeType::Start && n.node_type != WorkflowNodeType::End)
                .count();
            if non_boundary == 0 {
                empty_stages.push(stage.id.clone());
            }
        }
        checks.push(ValidationCheck {
            check_type: "empty_stages".into(),
            severity: if empty_stages.is_empty() { "info".into() } else { "warning".into() },
            message: if empty_stages.is_empty() {
                "所有阶段均包含非边界节点".into()
            } else {
                format!("{} 个阶段仅包含边界节点（无实际工作节点）", empty_stages.len())
            },
            details: if !empty_stages.is_empty() {
                Some(serde_json::json!({ "stage_ids": empty_stages }))
            } else { None },
        });

        // ── 8. 子工作流全链路预检（深度 + 循环引用）──
        let max_depth: usize = conn.query_row(
            "SELECT value FROM app_settings WHERE key = 'workflow_max_subflow_depth'",
            [],
            |row| row.get::<_, String>(0),
        ).ok().and_then(|v| v.parse().ok()).unwrap_or(3);

        let subflow_checks = Self::check_all_subflow_chains(def, conn, max_depth);
        for sc in subflow_checks {
            if sc.severity == "error" { has_error = true; }
            checks.push(sc);
        }

        Ok(ValidationResult { ok: !has_error, checks })
    }

    /// 阶段连线环路检测（Kahn 算法）
    fn check_stage_edge_cycle(def: &WorkflowDefinition) -> ValidationCheck {
        let stage_ids: std::collections::HashSet<String> =
            def.stages.iter().map(|s| s.id.clone()).collect();

        let mut in_degree: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut adj: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

        for s in &def.stages {
            in_degree.entry(s.id.clone()).or_insert(0);
            adj.entry(s.id.clone()).or_default();
        }
        for s in &def.stages {
            for edge in &s.stage_edges {
                if stage_ids.contains(&edge.source) && stage_ids.contains(&edge.target) {
                    adj.get_mut(&edge.source).unwrap().push(edge.target.clone());
                    *in_degree.entry(edge.target.clone()).or_insert(0) += 1;
                }
            }
        }

        let mut queue: Vec<String> = in_degree.iter()
            .filter(|(_, &d)| d == 0)
            .map(|(id, _)| id.clone())
            .collect();
        let mut processed = 0;

        while !queue.is_empty() {
            let mut next_queue = Vec::new();
            for id in &queue {
                processed += 1;
                if let Some(neighbors) = adj.get(id) {
                    for neighbor in neighbors {
                        if let Some(d) = in_degree.get_mut(neighbor) {
                            *d -= 1;
                            if *d == 0 {
                                next_queue.push(neighbor.clone());
                            }
                        }
                    }
                }
            }
            queue = next_queue;
        }

        let has_cycle = processed < def.stages.len();
        ValidationCheck {
            check_type: "stage_edge_cycle".into(),
            severity: if has_cycle { "error".into() } else { "info".into() },
            message: if has_cycle {
                format!("阶段连线存在环路（{} 个阶段不可达）", def.stages.len() - processed)
            } else {
                "阶段连线无环路".into()
            },
            details: None,
        }
    }

    /// 子工作流全链路预检（递归检查所有 Subflow 节点的嵌套深度和循环引用）
    fn check_all_subflow_chains(
        def: &WorkflowDefinition,
        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
        max_depth: usize,
    ) -> Vec<ValidationCheck> {
        let mut checks = Vec::new();
        Self::check_subflow_chain_recursive(
            def, conn, max_depth, &mut Vec::new(), &mut std::collections::HashSet::new(), &mut checks,
        );
        checks
    }

    fn check_subflow_chain_recursive(
        current_def: &WorkflowDefinition,
        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
        max_depth: usize,
        chain: &mut Vec<String>,
        visited_global: &mut std::collections::HashSet<String>,
        checks: &mut Vec<ValidationCheck>,
    ) {
        chain.push(current_def.id.clone());

        if visited_global.contains(&current_def.id) {
            chain.pop();
            return;
        }
        visited_global.insert(current_def.id.clone());

        for stage in &current_def.stages {
            for node in &stage.nodes {
                if node.node_type != WorkflowNodeType::Subflow { continue; }

                let subflow_def_id = node.params.as_ref()
                    .and_then(|p| p.get("definitionId"))
                    .and_then(|v| v.as_str());

                let subflow_def_id = match subflow_def_id {
                    Some(id) => id.to_string(),
                    None => {
                        checks.push(ValidationCheck {
                            check_type: "subflow_config".into(),
                            severity: "warning".into(),
                            message: format!("阶段 '{}' 的 Subflow 节点 '{}' 缺少 definitionId 参数",
                                stage.name, node.id),
                            details: Some(serde_json::json!({ "stageId": &stage.id, "nodeId": &node.id })),
                        });
                        continue;
                    }
                };

                // 检查循环引用
                if chain.contains(&subflow_def_id) {
                    let mut chain_display = chain.clone();
                    chain_display.push(subflow_def_id.clone());
                    checks.push(ValidationCheck {
                        check_type: "subflow_cycle".into(),
                        severity: "error".into(),
                        message: format!(
                            "检测到子工作流循环引用: {}",
                            chain_display.join(" → ")
                        ),
                        details: Some(serde_json::json!({
                            "chain": chain_display,
                            "nodeId": &node.id,
                            "stageId": &stage.id,
                        })),
                    });
                    continue;
                }

                // 检查嵌套深度
                if chain.len() >= max_depth {
                    checks.push(ValidationCheck {
                        check_type: "subflow_depth".into(),
                        severity: "error".into(),
                        message: format!(
                            "子工作流嵌套深度 {} 超过最大限制 {} (链路: {} → {})",
                            chain.len() + 1, max_depth, chain.join(" → "), subflow_def_id
                        ),
                        details: Some(serde_json::json!({
                            "depth": chain.len() + 1,
                            "maxDepth": max_depth,
                            "nodeId": &node.id,
                            "stageId": &stage.id,
                        })),
                    });
                    continue;
                }

                // 加载子工作流定义并递归检查
                match super::get_definition(conn, &subflow_def_id) {
                    Ok(Some(sub_def)) => {
                        Self::check_subflow_chain_recursive(
                            &sub_def, conn, max_depth, chain, visited_global, checks,
                        );
                    }
                    Ok(None) => {
                        checks.push(ValidationCheck {
                            check_type: "subflow_missing".into(),
                            severity: "error".into(),
                            message: format!("子工作流定义不存在: {}", subflow_def_id),
                            details: Some(serde_json::json!({
                                "definitionId": &subflow_def_id,
                                "nodeId": &node.id,
                                "stageId": &stage.id,
                            })),
                        });
                    }
                    Err(e) => {
                        checks.push(ValidationCheck {
                            check_type: "subflow_load_error".into(),
                            severity: "warning".into(),
                            message: format!("加载子工作流定义失败: {} ({})", subflow_def_id, e),
                            details: Some(serde_json::json!({
                                "definitionId": &subflow_def_id,
                                "nodeId": &node.id,
                                "error": e.to_string(),
                            })),
                        });
                    }
                }
            }
        }

        chain.pop();
    }
}

/// 统一执行进度事件发射
///
/// 将 node-status / stage-status / execution-status 三个事件统一为
/// workflow:execution-progress，前端只需注册一个 listen。
/// payload 为扁平结构，node/stage/execution 三个可选字段标识变更类型。
fn emit_progress(
    emitter: &tauri::AppHandle,
    execution_id: &str,
    definition_id: &str,
    mode: &ExecutionMode,
    node: Option<serde_json::Value>,
    stage: Option<serde_json::Value>,
    execution: Option<serde_json::Value>,
    progress: Option<(usize, usize)>,
) {
    let mut payload = serde_json::json!({
        "execution_id": execution_id,
        "definition_id": definition_id,
        "mode": mode,
    });
    if let Some(n) = node {
        payload["node"] = n;
    }
    if let Some(s) = stage {
        payload["stage"] = s;
    }
    if let Some(e) = execution {
        payload["execution"] = e;
    }
    if let Some((completed, total)) = progress {
        payload["progress"] = serde_json::json!({ "completed": completed, "total": total });
    }
    emitter.emit("workflow:execution-progress", payload).ok();
}

impl Drop for ExecutionGuard<'_> {
    fn drop(&mut self) {
        self.executor.unregister_execution(&self.execution_id);
    }
}

impl WorkflowEngine {
    /// 根据 workflow definition 计算 execution plan
    /// 从 start 节点出发，沿节点连线 + 阶段连线做全工作流 BFS，
    /// 返回可达节点集合和拓扑排序的阶段执行顺序
    pub fn compute_execution_plan(def: &WorkflowDefinition) -> ExecutionPlan {
        // 1. 找到 start 节点和所在阶段
        let start_stage_id = def.stages.iter().find_map(|s| {
            s.nodes.iter().find(|n| n.node_type == WorkflowNodeType::Start).map(|_| s.id.clone())
        });
        let start_node_id = def.stages.iter().find_map(|s| {
            s.nodes.iter().find(|n| n.node_type == WorkflowNodeType::Start).map(|n| n.id.clone())
        });

        match (start_stage_id, start_node_id) {
            (Some(sid), Some(nid)) => {
                let stage_id_set: std::collections::HashSet<String> =
                    def.stages.iter().map(|s| s.id.clone()).collect();
                let stage_map: std::collections::HashMap<String, &Stage> =
                    def.stages.iter().map(|s| (s.id.clone(), s)).collect();

                // 构建阶段连线快速查找（从各阶段的 stage_edges 读取）
                let se_down: std::collections::HashMap<String, Vec<String>> = {
                    let mut m: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
                    for stage in &def.stages {
                        for edge in &stage.stage_edges {
                            m.entry(edge.source.clone()).or_default().push(edge.target.clone());
                        }
                    }
                    m
                };

                // BFS：同时收集可达阶段（按拓扑顺序分层）和可达节点
                let mut visited_nodes = std::collections::HashSet::new();
                let mut visited_stages = std::collections::HashSet::new();
                let mut stage_depth: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                let mut node_queue = std::collections::VecDeque::new();

                visited_nodes.insert(nid.clone());
                visited_stages.insert(sid.clone());
                stage_depth.insert(sid.clone(), 0);
                node_queue.push_back(nid.clone());

                while let Some(cur) = node_queue.pop_front() {
                    if let Some(stage) = stage_map.values().find(|s| s.nodes.iter().any(|n| n.id == cur)) {
                        // 沿阶段内节点连线找下游节点
                        for edge in &stage.edges {
                            if edge.source == cur && !visited_nodes.contains(&edge.target) {
                                visited_nodes.insert(edge.target.clone());
                                node_queue.push_back(edge.target.clone());
                            }
                        }
                        // 沿阶段连线找下游阶段
                        if let Some(downstreams) = se_down.get(&stage.id) {
                            let cur_depth = *stage_depth.get(&stage.id).unwrap_or(&0);
                            for ds_id in downstreams {
                                if stage_id_set.contains(ds_id) && !visited_stages.contains(ds_id) {
                                    visited_stages.insert(ds_id.clone());
                                    stage_depth.insert(ds_id.clone(), cur_depth + 1);
                                    // 只将下游阶段的入口节点（入度为0的节点）加入队列，
                                    // BFS 会沿阶段内节点连线自然遍历到真正可达的节点
                                    if let Some(ds) = stage_map.get(ds_id) {
                                        let has_incoming: std::collections::HashSet<String> =
                                            ds.edges.iter().map(|e| e.target.clone()).collect();
                                        for node in &ds.nodes {
                                            if !visited_nodes.contains(&node.id)
                                                && !has_incoming.contains(&node.id)
                                            {
                                                visited_nodes.insert(node.id.clone());
                                                node_queue.push_back(node.id.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                let mut reachable_ids: Vec<String> = visited_nodes.into_iter().collect();
                reachable_ids.sort();
                // 按深度分组，同层阶段可并行执行
                let max_depth = stage_depth.values().max().copied().unwrap_or(0);
                let mut layers: Vec<Vec<String>> = (0..=max_depth).map(|_| Vec::new()).collect();
                for (sid, depth) in &stage_depth {
                    layers[*depth].push(sid.clone());
                }
                ExecutionPlan {
                    reachable_node_ids: reachable_ids,
                    ordered_stage_ids: layers,
                }
            }
            _ => {
                // 没有 start 节点，fallback：所有阶段和节点
                ExecutionPlan {
                    reachable_node_ids: def.stages.iter()
                        .flat_map(|s| s.nodes.iter().map(|n| n.id.clone()))
                        .collect(),
                    ordered_stage_ids: vec![def.stages.iter().map(|s| s.id.clone()).collect()],
                }
            }
        }
    }



}
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
        definition_id: &str,
        mode: &ExecutionMode,
        context: &Arc<AsyncMutex<HashMap<String, Value>>>,
        raw_outputs: &Arc<AsyncMutex<HashMap<String, Value>>>,
        emitter: &tauri::AppHandle,
        cancelled: &Arc<AtomicBool>,
        completed_count: &Arc<AtomicUsize>,
        total_count: usize,
        semaphore: &Arc<Semaphore>,
        max_concurrency: usize,
        visited_def_ids: &[String],
        reachable_nodes: &std::collections::HashSet<String>,
    ) -> Result<(HashMap<String, String>, Value), AppError> {
        let layers = Self::topological_sort(&stage.nodes, &stage.edges)?;
        let node_map: HashMap<String, &WorkflowNode> = stage.nodes.iter().map(|n| (n.id.clone(), n)).collect();
        // 节点执行状态追踪（用于门控策略判断），Arc<AsyncMutex> 支持跨 spawn 共享
        let node_statuses: Arc<AsyncMutex<HashMap<String, String>>> = Arc::new(AsyncMutex::new(HashMap::new()));

        for layer in &layers {
            let mut handles: Vec<tokio::task::JoinHandle<Result<(), AppError>>> = Vec::new();

            let _stage_id = stage.id.clone(); // 用于 async move 闭包
            for node_id in layer {
                let node = match node_map.get(node_id) {
                    Some(n) => (*n).clone(),
                    None => continue,
                };

                if cancelled.load(Ordering::SeqCst) {
                    return Err(AppError::External("工作流已被取消".into()));
                }

                // 跳过全局不可达节点
                if !reachable_nodes.contains(&node.id) {
                    node_statuses.lock().await.insert(node.id.clone(), "skipped".to_string());
                    completed_count.fetch_add(1, Ordering::SeqCst);
                    continue;
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
                        record_node_execution(emitter, execution_id, node_id, "skipped", None, None, None, None, None, None);
                        emit_node_status(emitter, execution_id, definition_id, node_id, "skipped", None, None, Some(completed_count.load(Ordering::SeqCst)), Some(total_count), mode);
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
                        serde_json::to_string(&resolved_input).ok().as_deref(), None, None, None, None, None);
                    emit_node_status(emitter, execution_id, definition_id, node_id, "running", None, None, None, None, mode);

                    // delay 支持：执行前等待 delay_ms 毫秒
                    if let Some(delay_ms) = node.delay_ms {
                        if delay_ms > 0 {
                            log::info!("[WorkflowEngine] 节点 {} 延迟 {}ms 后执行", node_id, delay_ms);
                            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                        }
                    }

                    match Self::execute_subflow_node(
                        executor, &node, resolved_input,
                        execution_id, emitter, max_concurrency,
                        visited_def_ids,
                    ).await {
                        Ok(output) => {
                            node_statuses.lock().await.insert(node_id.clone(), "completed".to_string());
                            // 子工作流节点：应用 output_mapping 提取字段（类似 Start 节点处理方式）
                            // output_mapping 值支持 {{endNodeId.content}} 格式从子工作流返回的上下文中提取
                            let mapped_output = if let Some(mapping) = &node.output_mapping {
                                if let Some(obj) = mapping.as_object() {
                                    // 构建临时 context：output 是子工作流返回的完整上下文
                                    let mut sub_ctx = std::collections::HashMap::new();
                                    sub_ctx.insert("__input__".to_string(), output.clone());
                                    let mut result_map = serde_json::Map::new();
                                    for (k, v) in obj {
                                        if let Some(s) = v.as_str() {
                                            match crate::workflow::template::TemplateEngine::resolve(s, &sub_ctx) {
                                                Ok(resolved) => {
                                                    result_map.insert(k.clone(), Value::String(resolved));
                                                }
                                                Err(_) => {
                                                    result_map.insert(k.clone(), v.clone());
                                                }
                                            }
                                        } else {
                                            result_map.insert(k.clone(), v.clone());
                                        }
                                    }
                                    Value::Object(result_map)
                                } else {
                                    output.clone()
                                }
                            } else {
                                output.clone()
                            };
                            context.lock().await.insert(node_id.clone(), mapped_output.clone());
                            raw_outputs.lock().await.insert(node_id.clone(), output.clone());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(emitter, execution_id, node_id, "completed", None,
                                serde_json::to_string(&mapped_output).ok().as_deref(), None, None, None, None);
                            emit_node_status(emitter, execution_id, definition_id, node_id, "completed", Some(&mapped_output), None, Some(completed_count.load(Ordering::SeqCst)), Some(total_count), mode);
                        }
                        Err(e) => {
                            node_statuses.lock().await.insert(node_id.clone(), "failed".to_string());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(emitter, execution_id, node_id, "failed", None, None, Some(&e.to_string()), None, None, None);
                            emit_node_status(emitter, execution_id, definition_id, node_id, "failed", None, Some(&e.to_string()), None, None, mode);
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
                let raw_outputs = raw_outputs.clone();
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
                let def_id_owned = definition_id.to_string();
                let mode_owned = mode.clone();
                let handle = tokio::spawn(async move {
                    let ctx_snapshot = context.lock().await.clone();
                    log::info!("[WorkflowEngine] Executing node {} (type={:?}), context keys: {:?}", nid, node.node_type, context.lock().await.keys().collect::<Vec<_>>());
                    let resolved_input = resolve_node_input(&node, &ctx_snapshot);

                    record_node_execution(&emitter, &exec_id, &nid, "running",
                        serde_json::to_string(&resolved_input).ok().as_deref(), None, None, None, None, None);
                    emit_node_status(&emitter, &exec_id, &def_id_owned, &nid, "running", None, None, None, None, &mode_owned);

                    // delay 支持：执行前等待 delay_ms 毫秒
                    if let Some(delay_ms) = node.delay_ms {
                        if delay_ms > 0 {
                            log::info!("[WorkflowEngine] 节点 {} 延迟 {}ms 后执行", nid, delay_ms);
                            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                        }
                    }

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
                            // Agent 节点：仅将通过 outputMapping 显式声明的字段暴露给 context
                            // session_id 以 __ 前缀内部保留（供会话延续），不暴露给模板引用
                            if let Some(ref sid) = output.session_id {
                                // 内部保留 session_id（__ 前缀隔离，不暴露给用户模板）
                                context.lock().await.insert(format!("__session_id__{}", nid), Value::String(sid.clone()));
                            }
                            // 根据 outputMapping 构建暴露给后续节点的 context
                            let output_mapping = node.output_mapping.as_ref();
                            if node.node_type == WorkflowNodeType::Start {
                                // Start 节点：预填充阶段已写入完整 context，跳过覆盖
                                log::info!("[WorkflowEngine] Start node {} context preserved from pre-population", nid);
                            } else if let Some(mapping) = output_mapping.and_then(|m| m.as_object()) {
                                // 非 Start 节点：暴露 outputMapping 声明的字段
                                // 匹配逻辑：判断 path（下拉选择的值）而非 key（用户自定义字段名）
                                let mut exposed = serde_json::Map::new();
                                for (key, path) in mapping {
                                    match path.as_str() {
                                        Some("{{content}}") => {
                                            exposed.insert(key.clone(), output.output.clone());
                                        }
                                        Some("{{session_id}}") => {
                                            if let Some(ref sid) = output.session_id {
                                                exposed.insert(key.clone(), Value::String(sid.clone()));
                                            }
                                        }
                                        _ => {
                                            // 仅支持 {{var}} 格式：去掉 {{}} 后按路径从 node_output 提取
                                            // 非 {{}} 格式视为用户常量字符串，不暴露
                                            if let Some(path_str) = path.as_str() {
                                                if path_str.starts_with("{{") && path_str.ends_with("}}") {
                                                    let lookup = &path_str[2..path_str.len()-2];
                                                    if !lookup.is_empty() {
                                                        if let Some(val) = node_output.get(lookup) {
                                                            exposed.insert(key.clone(), val.clone());
                                                        } else {
                                                            exposed.insert(key.clone(), node_output.clone());
                                                        }
                                                    } else {
                                                        exposed.insert(key.clone(), node_output.clone());
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                let exposed_keys: Vec<String> = exposed.keys().cloned().collect();
                                context.lock().await.insert(nid.clone(), Value::Object(exposed));
                                log::info!("[WorkflowEngine] Node {} context (outputMapping): {:?}", nid, exposed_keys);
                            } else if node.node_type == WorkflowNodeType::End {
                                // End 节点：无 outputMapping，输出 = resolved_input（所有 inputMapping 组成的对象）
                                context.lock().await.insert(nid.clone(), resolved_input.clone());
                                log::info!("[WorkflowEngine] End node {} context: resolved_input = {:?}", nid, resolved_input);
                            } else {
                                // 无 outputMapping：默认不暴露任何字段（空对象）
                                context.lock().await.insert(nid.clone(), Value::Object(serde_json::Map::new()));
                                log::info!("[WorkflowEngine] Node {} context: no outputMapping, empty", nid);
                            }
                            // 保存原始执行结果到 raw_outputs（供门控合并使用）
                            raw_outputs.lock().await.insert(nid.clone(), output.output.clone());
                            log::info!("[WorkflowEngine] Node {} output written to context: {:?}", nid, node_output);
                            node_statuses_clone.lock().await.insert(nid.clone(), "completed".to_string());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(&emitter, &exec_id, &nid, "completed", None,
                                serde_json::to_string(&node_output).ok().as_deref(), None,
                                output.session_id.as_deref(),
                                output.input_data.as_deref(),
                                output.artifacts_path.as_deref());
                            emit_node_status(&emitter, &exec_id, &def_id_owned, &nid, "completed", Some(&node_output), None, Some(completed_count.load(Ordering::SeqCst)), Some(total), &mode_owned);
                            return Ok(());
                        }
                        Err(e) => {
                            node_statuses_clone.lock().await.insert(nid.clone(), "failed".to_string());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(&emitter, &exec_id, &nid, "failed", None, None, Some(&e.to_string()), None, None, None);
                            emit_node_status(&emitter, &exec_id, &def_id_owned, &nid, "failed", None, Some(&e.to_string()), Some(completed_count.load(Ordering::SeqCst)), Some(total), &mode_owned);
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
        raw_outputs: &HashMap<String, Value>,
    ) -> Value {
        let node_outputs: Vec<(&String, &Value)> = stage.nodes.iter()
            .filter_map(|n| raw_outputs.get(&n.id).map(|v| (&n.id, v)))
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
        let max_depth = conn.query_row("SELECT value FROM app_settings WHERE key = 'workflow_max_subflow_depth'", [], |row| row.get::<_, String>(0)).ok().and_then(|v| v.parse().ok()).unwrap_or(3);
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
        let _guard = ExecutionGuard { executor: executor.as_ref(), execution_id: execution_id.to_string() };
        let definition_id = def.id.clone();
        let mode = ExecutionMode::default(); // 预留：未来可从参数获取
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

        let cancelled = executor.register_execution(execution_id);
        let total_count: usize = def.stages.iter().map(|s| s.nodes.len()).sum();
        let completed_count = Arc::new(AtomicUsize::new(0));

        log::info!("[WorkflowEngine] 开始执行: id={}, stages={}, total_nodes={}", execution_id, def.stages.len(), total_count);

        let context: Arc<AsyncMutex<HashMap<String, Value>>> = Arc::new(AsyncMutex::new(HashMap::new()));
        let raw_outputs: Arc<AsyncMutex<HashMap<String, Value>>> = Arc::new(AsyncMutex::new(HashMap::new()));
        context.lock().await.insert("__input__".to_string(), input_data.clone());

        // 开始节点：将输出映射作为初始上下文值
        for stage in &def.stages {
            for node in &stage.nodes {
                if node.node_type == WorkflowNodeType::Start {
                    if let Some(mapping) = &node.output_mapping {
                        if let Some(obj) = mapping.as_object() {
                            // 开始节点：通过 TemplateEngine 解析 output_mapping 值
                            // 支持 {{title}} 自动匹配 __input__.title（由 TemplateEngine 回退逻辑处理）
                            let ctx = context.lock().await.clone();
                            let mut output_obj = serde_json::Map::new();
                            for (k, v) in obj {
                                if let Some(s) = v.as_str() {
                                    let resolved = TemplateEngine::resolve(s, &ctx)
                                        .unwrap_or_else(|_| s.to_string());
                                    output_obj.insert(k.clone(), Value::String(resolved.clone()));
                                    // 设置扁平键，支持简单 {{参数名}} 格式
                                    context.lock().await.insert(k.clone(), Value::String(resolved));
                                } else {
                                    output_obj.insert(k.clone(), v.clone());
                                    context.lock().await.insert(k.clone(), v.clone());
                                }
                            }
                            context.lock().await.insert(node.id.clone(), Value::Object(output_obj));
                        }
                    }
                    break;
                }
            }
        }

        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        log::info!("[WorkflowEngine] Context after start node pre-population: {:?}", context.lock().await.keys().collect::<Vec<_>>());

        // 使用统一的执行计划（消除重复 BFS 逻辑）
        let plan = Self::compute_execution_plan(def);
        let reachable_nodes: std::collections::HashSet<String> =
            plan.reachable_node_ids.iter().cloned().collect();
        let total_count = reachable_nodes.len();

        log::info!("[WorkflowEngine] 执行计划: stages layers={:?}, reachable_nodes={}", plan.ordered_stage_ids, total_count);

        for layer in &plan.ordered_stage_ids {
            // 同层阶段并行执行（无依赖关系的阶段可同时运行）
            let mut handles: Vec<tokio::task::JoinHandle<Result<(String, HashMap<String, String>, bool), AppError>>> = Vec::new();

            for stage_id in layer {
                let stage = def.stages.iter().find(|s| s.id == *stage_id).unwrap();
                log::info!("[WorkflowEngine] 执行阶段: id={}, name={}, nodes={}, edges={}", stage.id, stage.name, stage.nodes.len(), stage.edges.len());

                if cancelled.load(Ordering::SeqCst) {
                    return Err(AppError::External("工作流已被取消".into()));
                }

                // clone shared state for spawn
                let exec_id = execution_id.to_string();
                let stage_clone = stage.clone();
                let context = context.clone();
                let raw_outputs = raw_outputs.clone();
                let emitter = emitter.clone();
                let cancelled = cancelled.clone();
                let completed_count = completed_count.clone();
                let semaphore = semaphore.clone();
                let executor = executor.clone();
                let reachable_nodes = reachable_nodes.clone();
                let visited_def_ids = visited_def_ids.to_vec();
                let definition_id = definition_id.clone();
                let mode = mode.clone();

                handles.push(tokio::spawn(async move {
                    emit_progress(
                        &emitter, &exec_id, &definition_id, &mode,
                        None,
                        Some(serde_json::json!({
                            "id": stage_clone.id.clone(),
                            "name": stage_clone.name.clone(),
                            "status": "running",
                        })),
                        None, None,
                    );

                    let (node_statuses, _) = WorkflowEngine::execute_stage(
                        &executor, &stage_clone, &exec_id,
                        &definition_id, &mode,
                        &context, &raw_outputs, &emitter, &cancelled,
                        &completed_count, total_count, &semaphore,
                        max_concurrency,
                        &visited_def_ids,
                        &reachable_nodes,
                    ).await?;

                    // 合并策略
                    let raw_ctx = raw_outputs.lock().await.clone();
                    let merged = WorkflowEngine::merge_stage_outputs(&stage_clone, &raw_ctx);

                    // 门控策略检查
                    match WorkflowEngine::check_gate_strategy(&stage_clone, &node_statuses, &merged) {
                        Ok(true) => {
                            log::info!("[WorkflowEngine] 阶段 '{}' 门控策略检查通过，合并结果已写入", stage_clone.name);
                            context.lock().await.insert(format!("gate_output.{}", stage_clone.id), merged);
                            emit_progress(
                                &emitter, &exec_id, &definition_id, &mode,
                                None,
                                Some(serde_json::json!({
                                    "id": stage_clone.id.clone(),
                                    "name": stage_clone.name.clone(),
                                    "status": "completed",
                                })),
                                None, None,
                            );
                            Ok((stage_clone.id.clone(), node_statuses, true))
                        }
                        Ok(false) => {
                            let msg = format!(
                                "阶段 '{}' 门控策略未通过（strategy={:?}），工作流中止",
                                stage_clone.name, stage_clone.gate.strategy
                            );
                            log::warn!("[WorkflowEngine] {}", msg);
                            emit_progress(
                                &emitter, &exec_id, &definition_id, &mode,
                                None,
                                Some(serde_json::json!({
                                    "id": stage_clone.id.clone(),
                                    "name": stage_clone.name.clone(),
                                    "status": "gate_failed",
                                    "reason": msg.clone(),
                                })),
                                None, None,
                            );
                            cancelled.store(true, Ordering::SeqCst);
                            Ok((stage_clone.id.clone(), node_statuses, false))
                        }
                        Err(e) => {
                            log::error!("[WorkflowEngine] 阶段 '{}' 门控策略检查失败: {}", stage_clone.name, e);
                            emit_progress(
                                &emitter, &exec_id, &definition_id, &mode,
                                None,
                                Some(serde_json::json!({
                                    "id": stage_clone.id.clone(),
                                    "name": stage_clone.name.clone(),
                                    "status": "gate_failed",
                                    "error": e.to_string(),
                                })),
                                None, None,
                            );
                            Err(AppError::InvalidInput(format!(
                                "阶段 '{}' 门控策略检查异常: {}", stage_clone.name, e
                            )))
                        }
                    }
                }));
            }

            // 等待当前层所有阶段执行完成
            for handle in handles {
                match handle.await {
                    Ok(Ok((sid, _ns, gate_pass))) => {
                        if !gate_pass {
                            cancelled.store(true, Ordering::SeqCst);
                            return Err(AppError::External(format!(
                                "阶段 '{}' 门控策略未通过，工作流中止", sid
                            )));
                        }
                    }
                    Ok(Err(e)) => {
                        cancelled.store(true, Ordering::SeqCst);
                        return Err(e);
                    }
                    Err(join_err) => {
                        cancelled.store(true, Ordering::SeqCst);
                        return Err(AppError::External(format!("阶段执行任务异常: {}", join_err)));
                    }
                }
            }
        }
        // 更新实例状态为成功
        if let Ok(conn) = get_db_conn(emitter) {
            let _ = conn.execute(
                "UPDATE workflow_instances SET status = 'success', completed_at = ?1, updated_at = ?1 WHERE id = ?2",
                rusqlite::params![crate::utils::now(), execution_id],
            );
        }

        emit_progress(
            emitter, execution_id, &definition_id, &mode,
            None, None,
            Some(serde_json::json!({
                "status": "completed",
                "definition_name": &def.name,
            })),
            None,
        );

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
                    nodes: pending_nodes, edges: filtered_edges, stage_edges: stage.stage_edges.clone(), gate: stage.gate.clone(),
                    collapsed: stage.collapsed, offset_x: stage.offset_x, offset_y: stage.offset_y,
                });
            }
            (completed_nodes, recovered_stages)
        };


        if recovered_stages.is_empty() {
            emit_progress(
                emitter, execution_id, &def.id, &ExecutionMode::default(),
                None, None,
                Some(serde_json::json!({
                    "status": "completed",
                    "definition_name": &def.name,
                    "message": "所有节点已完成",
                })),
                None,
            );
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
