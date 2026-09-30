use super::executor::NodeExecutor;
use super::registry::NodeDef;
use super::template::{infer_typed_value, TemplateEngine};
use super::{
    GateStrategy, MergeStrategy, Stage, WorkflowDefinition, WorkflowEdge, WorkflowNode,
    WorkflowNodeType,
};
use crate::utils::errors::AppError;
use async_recursion::async_recursion;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

/// 从 AppHandle 获取数据库连接

fn get_db_conn(
    app_handle: &tauri::AppHandle,
) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, AppError> {
    app_handle
        .state::<crate::DbState>()
        .get_conn()
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

    // 事件化：节点执行开始（node/start；旧 node_executions 行已由事件日志取代）。
    let input_preview: Option<String> = input_data.map(|s| s.chars().take(500).collect::<String>());
    let event = serde_json::json!({
        "executionId": execution_id,
        "nodeId": node_id,
        "status": status,
        "inputPreview": input_preview,
        "timestamp": now,
    });
    crate::eventlog::append_workflow_event(conn, execution_id, "node/start", &event, true)?;
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
    _input_data: Option<&str>,
    artifacts_path: Option<&str>,
) -> Result<(), AppError> {
    let now = crate::utils::now();

    // agent_session_id 由调用方直接传入（从 NodeOutput.session_id 获取）

    // input_data 由调用方传入（completed 时覆盖 running 时写入的输入映射内容）

    // artifacts_path 由调用方传入（节点执行工件路径）

    // 事件化：节点状态迁移（error 截断便于审计；状态事件不含 output）。
    let err_preview: Option<String> =
        error_message.map(|s| s.chars().take(2000).collect::<String>());
    let event = serde_json::json!({
        "executionId": execution_id,
        "nodeId": node_id,
        "status": status,
        "errorMessage": err_preview,
        "agentSessionId": agent_session_id,
        "finishedAt": now,
        "timestamp": now,
    });
    crate::eventlog::append_workflow_event(conn, execution_id, "node/status", &event, true)?;

    // 事件化：成功节点另落 node/result（携带 output 与 artifacts_path）。
    // 完整事件源需要输出供下游引用 / rebuild_context_from_history 重建。
    if matches!(status, "completed" | "success") {
        if let Some(output) = output_data {
            let result_event = serde_json::json!({
                "executionId": execution_id,
                "nodeId": node_id,
                "output": output,
                "artifactsPath": artifacts_path,
                "timestamp": now,
            });
            crate::eventlog::append_workflow_event(
                conn,
                execution_id,
                "node/result",
                &result_event,
                true,
            )?;
        }
    }

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
        emitter,
        execution_id,
        definition_id,
        mode,
        Some(node_payload),
        None,
        None,
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
            log::warn!(
                "[WorkflowEngine] 获取数据库连接失败，跳过节点执行记录: {}",
                e
            );
            return;
        }
    };
    let result = match status {
        "skipped" | "running" => {
            insert_node_execution(&conn, execution_id, node_id, status, input_data)
        }

        _ => update_node_execution(
            &conn,
            execution_id,
            node_id,
            status,
            output_data,
            error_message,
            agent_session_id,
            input_data_override,
            artifacts_path,
        ),
    };
    if let Err(e) = result {
        log::warn!(
            "[WorkflowEngine] 写入节点执行记录失败 (execution={}, node={}, status={}): {}",
            execution_id,
            node_id,
            status,
            e
        );
    }

    // 写入节点执行日志

    let log_level = match status {
        "failed" | "skipped" => "warn",
        _ => "info",
    };
    let log_message = format!(
        "节点 {} 执行{}",
        node_id,
        match status {
            "running" => "开始",
            "completed" => "完成",
            "failed" => "失败",
            "skipped" => "跳过",
            "cancelled" => "取消",
            _ => status,
        }
    );
    let log_metadata = if let Some(err) = error_message {
        Some(serde_json::json!({"error": err}).to_string())
    } else {
        None
    };
    insert_node_execution_log(
        &conn,
        execution_id,
        node_id,
        log_level,
        &log_message,
        log_metadata.as_deref(),
    );
}

/// 追加节点执行日志（node/log 事件；读侧按节点日志投影返回）。

fn insert_node_execution_log(
    conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    execution_id: &str,
    node_id: &str,
    level: &str,
    message: &str,
    metadata: Option<&str>,
) {
    let node_execution_id = format!("{}_{}", execution_id, node_id);
    let event = serde_json::json!({
        "executionId": execution_id,
        "nodeExecutionId": node_execution_id,
        "nodeId": node_id,
        "level": level,
        "message": message,
        "metadata": metadata,
    });
    if let Err(e) =
        crate::eventlog::append_workflow_event(conn, execution_id, "node/log", &event, false)
    {
        log::warn!("[WorkflowEngine] 写入节点执行日志失败: {}", e);
    }
}

/// 更新实例进度（completion_rate + skippedCount + context 持久化）
///
/// `settled` = 已结算节点数（**完成 + 跳过**）：条件分支未走的节点不该拖住完成度，
/// 否则带条件的工作流永远到不了 100%（详见 [`resolve_terminal_status`]）。
/// `skipped` 单列出来供 UI 显示「跳过 N」，避免"100% 但有些节点没跑"被误读。
fn update_instance_progress(
    conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    execution_id: &str,
    total: usize,
    settled: usize,
    skipped: usize,
    context: Option<&serde_json::Value>,
) {
    let rate = if total > 0 {
        settled as f64 / total as f64
    } else {
        0.0
    };
    let now = crate::utils::now();
    let mut event = serde_json::json!({
        "executionId": execution_id,
        "completionRate": rate,
        "skippedCount": skipped,
        "timestamp": now,
    });
    if let Some(ctx) = context {
        event["context"] = ctx.clone();
    }
    if let Err(e) = crate::eventlog::append_workflow_event(
        conn,
        execution_id,
        "execution/progress",
        &event,
        false,
    ) {
        log::warn!("[WorkflowEngine] 追加执行进度事件失败: {}", e);
    }
}

/// 门控判定结果。
///
/// 「策略不满足」是**正常的业务结果**（如 All 策略下有节点失败），不是故障；只有
/// 配置/表达式本身有问题才算异常（走 `Err`）。区分开才能：日志用 warn 而不是 error、
/// 前端文案直说原因，而不是把正常结果套上「检查异常」的壳。
enum GateDecision {
    /// 满足策略，放行
    Pass,
    /// 被策略拦下，`String` 是给用户看的原因（已含阶段名与判定细节）
    Blocked(String),
}

/// 两层调度引擎：阶段串行 → 阶段内 DAG

pub struct WorkflowEngine;

/// 执行模式（预留：支持完整执行、单点执行、断点执行）
///
/// 序列化为带 `type` 的对象：`{"type":"full"}` / `{"type":"single_node","nodeId":"…"}` /
/// `{"type":"chain","nodeId":"…"}` / `{"type":"completion"}`。进度事件的 `mode` 字段统一用该对象
/// （首帧与后续帧同形），`nodeId` 的字段名与前端 `ExecutionMode` 声明一致。
///

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ExecutionMode {
    /// 完整执行（默认）：从 start 节点执行到 end 节点
    Full,

    /// 单点执行：仅执行选中的单个节点（预留）
    SingleNode {
        #[serde(rename = "nodeId")]
        node_id: String,
    },

    /// 链式执行：从选中节点开始，执行含选中节点后所有链上的后序节点
    Chain {
        #[serde(rename = "nodeId")]
        node_id: String,
    },

    /// 补全执行：跳过已标记为 completed 的节点，按拓扑顺序执行所有未完成节点
    Completion,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::Full
    }
}

/// 断点执行（单点 / 链式 / 补全）需要、全量执行不需要的补充信息。
///
/// 断点执行只跑 scope 子集，但进度分母、门控判定与 `gate_output` 必须与全量执行同口径，
/// 因此把完整定义的相关信息随执行一起传入，而不是让子集定义自己近似：
/// - [`Self::total_reachable`]：进度分母（用子集数会把进度提前算满）；
/// - [`Self::history_statuses`] + [`Self::full_stages`]：门控按完整阶段节点集统计分析；
/// - [`Self::gate_output_baseline`]：上一轮 `gate_output.<stage>` 值，作为本轮基线合并；
/// - [`Self::boundary_edges`]：按阶段分组的边界入边（`target` 在 scope 内、`source` 在 scope 外），
///   仅参与入边条件判定，不参与拓扑分层。
pub(crate) struct RecoveryContext {
    total_reachable: usize,
    history_statuses: HashMap<String, String>,
    full_stages: HashMap<String, Stage>,
    gate_output_baseline: HashMap<String, Value>,
    boundary_edges: HashMap<String, Vec<WorkflowEdge>>,
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

        let start_nodes: Vec<&WorkflowNode> = def
            .stages
            .iter()
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
        if !start_ok {
            has_error = true;
        }

        // ── 2. end 节点唯一性 ──

        let end_nodes: Vec<&WorkflowNode> = def
            .stages
            .iter()
            .flat_map(|s| s.nodes.iter())
            .filter(|n| n.node_type == WorkflowNodeType::End)
            .collect();
        let end_ok = end_nodes.len() <= 1; // End 节点可选，0个或1个均可;
        checks.push(ValidationCheck {
            check_type: "boundary_end".into(),
            severity: if end_ok { "info".into() } else { "warning".into() },
                message: if end_nodes.len() == 0 {
                    "未配置结束节点（可选，不影响执行）".into()
                } else if end_ok {
                format!("结束节点唯一 (id={})", end_nodes[0].id)
            } else {
                format!("结束节点数量异常: 发现 {} 个，建议仅保留1个", end_nodes.len())
            },
            details: if !end_ok {
                Some(serde_json::json!({ "ids": end_nodes.iter().map(|n| &n.id).collect::<Vec<_>>() }))
            } else { None },
        });

        // End 节点不再作为强制校验，不设置 has_error

        // ── 3. 完整路径（start → end 可达）──

        if start_ok && end_nodes.len() == 1 {
            let plan = Self::compute_execution_plan(def);
            let end_reachable = plan.reachable_node_ids.contains(&end_nodes[0].id);
            checks.push(ValidationCheck {
                check_type: "path_complete".into(),
                severity: if end_reachable {
                    "info".into()
                } else {
                    "error".into()
                },
                message: if end_reachable {
                    "从起始节点到结束节点存在完整执行路径".into()
                } else {
                    "结束节点不可达（End 节点为可选节点，不影响执行）".into()
                },
                details: None,
            });

            // End 可达性不再作为强制校验，不设置 has_error

            // ── 4. 未就绪节点统计 ──

            let all_node_ids: std::collections::HashSet<String> = def
                .stages
                .iter()
                .flat_map(|s| s.nodes.iter().map(|n| n.id.clone()))
                .collect();
            let reachable_set: std::collections::HashSet<String> =
                plan.reachable_node_ids.iter().cloned().collect();
            let unreachable_count = all_node_ids.len().saturating_sub(reachable_set.len());
            checks.push(ValidationCheck {
                check_type: "unreachable_nodes".into(),
                severity: if unreachable_count == 0 {
                    "info".into()
                } else {
                    "warning".into()
                },
                message: if unreachable_count == 0 {
                    "所有节点均可达".into()
                } else {
                    format!("存在 {} 个未就绪节点（不在执行路径上）", unreachable_count)
                },
                details: if unreachable_count > 0 {
                    let unreachable_ids: Vec<String> = all_node_ids
                        .into_iter()
                        .filter(|id| !reachable_set.contains(id))
                        .collect();
                    Some(serde_json::json!({ "count": unreachable_count, "ids": unreachable_ids }))
                } else {
                    None
                },
            });
        }

        // ── 5. 阶段内无循环依赖（Kahn 算法）──

        let mut has_cycle = false;
        let mut cycle_stages = Vec::new();
        for stage in &def.stages {
            if stage.nodes.len() <= 1 {
                continue;
            }

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
            severity: if has_cycle {
                "error".into()
            } else {
                "info".into()
            },
            message: if has_cycle {
                format!("{} 个阶段内存在节点循环依赖", cycle_stages.len())
            } else {
                "所有阶段内均无循环依赖".into()
            },
            details: if has_cycle {
                Some(serde_json::json!({ "stage_ids": cycle_stages }))
            } else {
                None
            },
        });
        if has_cycle {
            has_error = true;
        }

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

            let only_boundary = stage.nodes.len() > 0
                && stage.nodes.iter().all(|n| {
                    n.node_type == WorkflowNodeType::Start || n.node_type == WorkflowNodeType::End
                });
            if only_boundary {
                continue;
            }

            let non_boundary = stage
                .nodes
                .iter()
                .filter(|n| {
                    n.node_type != WorkflowNodeType::Start && n.node_type != WorkflowNodeType::End
                })
                .count();
            if non_boundary == 0 {
                empty_stages.push(stage.id.clone());
            }
        }

        checks.push(ValidationCheck {
            check_type: "empty_stages".into(),
            severity: if empty_stages.is_empty() {
                "info".into()
            } else {
                "warning".into()
            },
            message: if empty_stages.is_empty() {
                "所有阶段均包含非边界节点".into()
            } else {
                format!(
                    "{} 个阶段仅包含边界节点（无实际工作节点）",
                    empty_stages.len()
                )
            },
            details: if !empty_stages.is_empty() {
                Some(serde_json::json!({ "stage_ids": empty_stages }))
            } else {
                None
            },
        });

        // ── 8. 子工作流全链路预检（深度 + 循环引用）──

        let max_depth: usize = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'workflow_max_subflow_depth'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        let subflow_checks = Self::check_all_subflow_chains(def, conn, max_depth);
        for sc in subflow_checks {
            if sc.severity == "error" {
                has_error = true;
            }

            checks.push(sc);
        }

        /// 从 Option<Value> 中提取字符串参数
        fn get_param_str<'a>(params: &'a Option<serde_json::Value>, key: &str) -> Option<&'a str> {
            params
                .as_ref()
                .and_then(|p| p.get(key))
                .and_then(|v| v.as_str())
        }

        // ── 9. 节点必要参数校验（人性化提示） ──
        for stage in &def.stages {
            for node in &stage.nodes {
                if node.node_type == WorkflowNodeType::Start
                    || node.node_type == WorkflowNodeType::End
                {
                    continue;
                }
                let type_str = format!("{:?}", node.node_type).to_lowercase();
                // 节点类型中文映射
                let type_label = match type_str.as_str() {
                    "agent" => "Agent",
                    "api" => "API",
                    "transform" => "代码转换",
                    "interact" => "人工交互",
                    "subflow" => "子工作流",
                    _ => &type_str,
                };
                // 参数名中文映射
                fn param_label(key: &str) -> String {
                    match key {
                        "agent_type" => "Agent 类型".to_string(),
                        "prompt_template" => "提示模板".to_string(),
                        "url" => "请求地址".to_string(),
                        "script" => "脚本代码".to_string(),
                        "prompt" => "交互提示".to_string(),
                        "definitionId" => "子工作流".to_string(),
                        _ => key.to_string(),
                    }
                }
                let missing: Vec<&str> = match type_str.as_str() {
                    "agent" => {
                        let mut m = Vec::new();
                        if get_param_str(&node.params, "agent_type").is_none() {
                            m.push("agent_type");
                        }
                        if get_param_str(&node.params, "prompt_template")
                            .map_or(true, |s| s.is_empty())
                        {
                            m.push("prompt_template");
                        }
                        m
                    }
                    "api" => {
                        let mut m = Vec::new();
                        if get_param_str(&node.params, "url").map_or(true, |s| s.is_empty()) {
                            m.push("url");
                        }
                        m
                    }
                    "transform" => {
                        let mut m = Vec::new();
                        if get_param_str(&node.params, "script").map_or(true, |s| s.is_empty()) {
                            m.push("script");
                        }
                        m
                    }
                    "interact" => {
                        let mut m = Vec::new();
                        if get_param_str(&node.params, "prompt").map_or(true, |s| s.is_empty()) {
                            m.push("prompt");
                        }
                        m
                    }
                    "subflow" => {
                        let mut m = Vec::new();
                        if get_param_str(&node.params, "definitionId")
                            .map_or(true, |s| s.is_empty())
                        {
                            m.push("definitionId");
                        }
                        m
                    }
                    _ => Vec::new(),
                };
                if !missing.is_empty() {
                    has_error = true;
                    let readable_missing: Vec<String> =
                        missing.iter().map(|k| param_label(k).to_string()).collect();
                    checks.push(ValidationCheck {
                        check_type: "node_config".into(),
                        severity: "error".into(),
                        message: format!("【{}节点】{} 缺少配置：{}", type_label, node.label, readable_missing.join("、")),
                        details: Some(serde_json::json!({ "node_id": &node.id, "node_type": &type_str, "missing": &missing })),
                    });
                }
            }
        }

        Ok(ValidationResult {
            ok: !has_error,
            checks,
        })
    }

    /// 阶段连线环路检测（Kahn 算法）

    fn check_stage_edge_cycle(def: &WorkflowDefinition) -> ValidationCheck {
        let stage_ids: std::collections::HashSet<String> =
            def.stages.iter().map(|s| s.id.clone()).collect();
        let mut in_degree: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut adj: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
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

        let mut queue: Vec<String> = in_degree
            .iter()
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
            severity: if has_cycle {
                "error".into()
            } else {
                "info".into()
            },
            message: if has_cycle {
                format!(
                    "阶段连线存在环路（{} 个阶段不可达）",
                    def.stages.len() - processed
                )
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
            def,
            conn,
            max_depth,
            &mut Vec::new(),
            &mut std::collections::HashSet::new(),
            &mut checks,
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
                if node.node_type != WorkflowNodeType::Subflow {
                    continue;
                }

                let subflow_def_id = node
                    .params
                    .as_ref()
                    .and_then(|p| p.get("definitionId"))
                    .and_then(|v| v.as_str());
                let subflow_def_id = match subflow_def_id {
                    Some(id) => id.to_string(),
                    None => {
                        checks.push(ValidationCheck {
                            check_type: "subflow_config".into(),
                            severity: "warning".into(),
                            message: format!(
                                "阶段 '{}' 的 Subflow 节点 '{}' 缺少 definitionId 参数",
                                stage.name, node.id
                            ),
                            details: Some(
                                serde_json::json!({ "stageId": &stage.id, "nodeId": &node.id }),
                            ),
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
                        message: format!("检测到子工作流循环引用: {}", chain_display.join(" → ")),
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
                            chain.len() + 1,
                            max_depth,
                            chain.join(" → "),
                            subflow_def_id
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
                            &sub_def,
                            conn,
                            max_depth,
                            chain,
                            visited_global,
                            checks,
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

/// 断点分支收尾时的终态进度事件（与执行实现内部的进度事件同形）。
///
/// `mode` 用真实执行模式的对象形态，`execution.status` 取终态字面量，使前端既有的事件监听
/// 能感知执行结束与失败原因，而不必依赖命令返回值。
///
/// # Arguments
///
/// * `emitter` - 事件发送端
/// * `execution_id` - 实例 id
/// * `def` - 工作流定义（取 id 与 name）
/// * `mode` - 本次执行模式
/// * `status` - 终态：`success` / `failed` / `cancelled`
/// * `error` - 失败原因；成功时为 `None`
/// * `progress` - 进度统计（completed, total）；不适用时为 `None`
fn emit_execution_terminal_progress(
    emitter: &tauri::AppHandle,
    execution_id: &str,
    def: &WorkflowDefinition,
    mode: &ExecutionMode,
    status: &str,
    error: Option<&str>,
    progress: Option<(usize, usize)>,
) {
    let mut execution = serde_json::json!({
        "status": status,
        "definition_name": &def.name,
    });
    if let Some(err) = error {
        execution["error"] = Value::String(err.to_string());
    }

    emit_progress(
        emitter,
        execution_id,
        &def.id,
        mode,
        None,
        None,
        Some(execution),
        progress,
    );
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
            s.nodes
                .iter()
                .find(|n| n.node_type == WorkflowNodeType::Start)
                .map(|_| s.id.clone())
        });
        let start_node_id = def.stages.iter().find_map(|s| {
            s.nodes
                .iter()
                .find(|n| n.node_type == WorkflowNodeType::Start)
                .map(|n| n.id.clone())
        });
        let stage_id_set: std::collections::HashSet<String> =
            def.stages.iter().map(|s| s.id.clone()).collect();
        let stage_map: std::collections::HashMap<String, &Stage> =
            def.stages.iter().map(|s| (s.id.clone(), s)).collect();

        // 构建阶段连线快速查找（从各阶段的 stage_edges 读取）

        let se_down: std::collections::HashMap<String, Vec<String>> = {
            let mut m: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for stage in &def.stages {
                for edge in &stage.stage_edges {
                    m.entry(edge.source.clone())
                        .or_default()
                        .push(edge.target.clone());
                }
            }

            m
        };
        match (start_stage_id, start_node_id) {
            (Some(sid), Some(nid)) => {
                // BFS：同时收集可达阶段（按拓扑顺序分层）和可达节点

                let mut visited_nodes = std::collections::HashSet::new();
                let mut visited_stages = std::collections::HashSet::new();
                let mut stage_depth: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                let mut node_queue = std::collections::VecDeque::new();
                visited_nodes.insert(nid.clone());
                visited_stages.insert(sid.clone());
                stage_depth.insert(sid.clone(), 0);
                node_queue.push_back(nid.clone());
                while let Some(cur) = node_queue.pop_front() {
                    if let Some(stage) = stage_map
                        .values()
                        .find(|s| s.nodes.iter().any(|n| n.id == cur))
                    {
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
                // 没有 start 节点，按 stage_edges 拓扑排序阶段
                // 构建入度表
                let mut se_up: std::collections::HashMap<String, Vec<String>> =
                    std::collections::HashMap::new();
                for stage in &def.stages {
                    for edge in &stage.stage_edges {
                        se_up
                            .entry(edge.target.clone())
                            .or_default()
                            .push(stage.id.clone());
                    }
                }
                let non_empty_ids: std::collections::HashSet<String> = def
                    .stages
                    .iter()
                    .filter(|s| !s.nodes.is_empty())
                    .map(|s| s.id.clone())
                    .collect();
                // BFS 遍历所有阶段（含空阶段），只将非空阶段加入执行计划
                let mut visited_stages: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut queue = std::collections::VecDeque::new();
                let mut stage_depth: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for stage in &def.stages {
                    if stage_id_set.contains(&stage.id) && !se_up.contains_key(&stage.id) {
                        queue.push_back(stage.id.clone());
                        visited_stages.insert(stage.id.clone());
                        stage_depth.insert(stage.id.clone(), 0);
                    }
                }
                if queue.is_empty() {
                    for stage in &def.stages {
                        if stage_id_set.contains(&stage.id) {
                            queue.push_back(stage.id.clone());
                            visited_stages.insert(stage.id.clone());
                            stage_depth.insert(stage.id.clone(), 0);
                            break;
                        }
                    }
                }
                while let Some(cur) = queue.pop_front() {
                    let cur_depth = *stage_depth.get(&cur).unwrap_or(&0);
                    if let Some(downstream) = se_down.get(&cur) {
                        for ds_id in downstream {
                            if stage_id_set.contains(ds_id) && !visited_stages.contains(ds_id) {
                                let new_depth = cur_depth + 1;
                                visited_stages.insert(ds_id.clone());
                                // 所有阶段都记录 depth（用于 BFS 传播），non_empty 决定是否加入 layer
                                stage_depth.insert(ds_id.clone(), new_depth);
                                queue.push_back(ds_id.clone());
                            }
                        }
                    }
                }
                let max_depth = stage_depth.values().max().copied().unwrap_or(0);
                let mut layers: Vec<Vec<String>> = (0..=max_depth).map(|_| Vec::new()).collect();
                for (sid, depth) in &stage_depth {
                    if non_empty_ids.contains(sid) {
                        layers[*depth].push(sid.clone());
                    }
                }
                ExecutionPlan {
                    reachable_node_ids: def
                        .stages
                        .iter()
                        .flat_map(|s| s.nodes.iter().map(|n| n.id.clone()))
                        .collect(),
                    ordered_stage_ids: layers,
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
            adjacency
                .get_mut(&edge.source)
                .unwrap_or(&mut vec![])
                .push(edge.target.clone());
            *in_degree.get_mut(&edge.target).unwrap_or(&mut 0) += 1;
        }

        let mut layers: Vec<Vec<String>> = Vec::new();
        let mut queue: Vec<String> = in_degree
            .iter()
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

    ///

    /// `boundary_edges`: 仅用于入边条件判定的边（`target` 在本次执行范围内、`source` 在范围外）。

    /// 断点执行时阶段定义被裁剪成执行范围子集，这些边不在 `stage.edges` 里；不参与拓扑分层，

    /// 只与阶段内入边一起做条件判定，避免条件性跳过的节点被重跑。

    /// `external_incoming_edges`: 来自其它阶段的节点连线（`target` 在本阶段、`source` 在其它阶段）。
    ///
    /// 连线按来源阶段存放在上游阶段里，若不做归集，本阶段的目标节点会被当成"阶段入口节点"无条件执行，
    /// 导致条件分支跳过的链路下游仍被按层级执行。只参与入边连通性判定，不影响阶段内拓扑分层。
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
        success_count: &Arc<AtomicUsize>,
        // 条件分支未命中 / 上游无产出而跳过的节点数（计入"已结算"，见 resolve_terminal_status）
        skipped_count: &Arc<AtomicUsize>,
        total_count: usize,
        semaphore: &Arc<Semaphore>,
        max_concurrency: usize,
        visited_def_ids: &[String],
        reachable_nodes: &std::collections::HashSet<String>,
        boundary_edges: &[WorkflowEdge],
        external_incoming_edges: &[WorkflowEdge],
    ) -> Result<(HashMap<String, String>, Value), AppError> {
        let layers = Self::topological_sort(&stage.nodes, &stage.edges)?;
        let node_map: HashMap<String, &WorkflowNode> =
            stage.nodes.iter().map(|n| (n.id.clone(), n)).collect();

        // 节点执行状态追踪（用于门控策略判断），Arc<AsyncMutex> 支持跨 spawn 共享

        let node_statuses: Arc<AsyncMutex<HashMap<String, String>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));
        for layer in &layers {
            // 句柄与节点 id 成对保存：panic 时要能结算对应节点（JoinError 自身不带节点信息）
            let mut handles: Vec<(String, tokio::task::JoinHandle<Result<(), AppError>>)> =
                Vec::new();
            let _stage_id = stage.id.clone(); // 用于 async move 闭包

            for node_id in layer {
                let node = match node_map.get(node_id) {
                    Some(n) => (*n).clone(),
                    None => continue,
                };
                if cancelled.load(Ordering::SeqCst) {
                    return Err(AppError::External("工作流已被取消".into()));
                }

                // 跳过全局不可达节点（定义改动后的残留节点、不在 start 可达路径上的孤立节点）。
                // 必须显式发一条状态：否则界面上该节点没有任何状态、也不知道为何整个执行被判"跳过"，
                // 用户只能看到"节点一直没动 + 工作流失败/跳过"。不计入已结算（分母是可达节点数）。
                if !reachable_nodes.contains(&node.id) {
                    let reason = "节点不在执行路径上（没有从起始节点连通过来的入边）";
                    log::info!("[WorkflowEngine] 节点 {} 不在执行路径上，跳过", node.id);
                    node_statuses
                        .lock()
                        .await
                        .insert(node.id.clone(), "skipped".to_string());
                    record_node_execution(
                        emitter,
                        execution_id,
                        &node.id,
                        "skipped",
                        None,
                        None,
                        Some(reason),
                        None,
                        None,
                        None,
                    );
                    emit_node_status(
                        emitter,
                        execution_id,
                        definition_id,
                        &node.id,
                        "skipped",
                        None,
                        Some(reason),
                        None,
                        None,
                        mode,
                    );
                    continue;
                }

                // 检查入边条件

                let incoming_edges: Vec<&WorkflowEdge> = {
                    // 去重：断点边界入边与跨阶段入边可能指向同一条连线
                    let mut seen_ids: std::collections::HashSet<&str> =
                        std::collections::HashSet::new();
                    stage
                        .edges
                        .iter()
                        .chain(boundary_edges.iter())
                        .chain(external_incoming_edges.iter())
                        .filter(|e| e.target == *node_id)
                        .filter(|e| seen_ids.insert(e.id.as_str()))
                        .collect()
                };
                if !incoming_edges.is_empty() {
                    let ctx = context.lock().await.clone();

                    // 多入边取 OR：任一入边链路连通即可执行
                    let any_edge_active = Self::any_incoming_edge_active(&incoming_edges, &ctx);
                    if !any_edge_active {
                        // 条件分支未命中 / 上游无产出：节点不再执行，但**计入已结算**——
                        // 它是设计上的结果，不是失败，也不该让完成度永远到不了 100%。
                        // 注意：只有「计划内（可达）但条件未命中」才算跳过；计划外不可达节点
                        // 已在上面提前 continue，不计入任何分子（否则分子会超过分母）。
                        // 原因里带上上游节点名：常见的"莫名跳过"就是上游 Agent 没产出/未执行，
                        // 用户需要一眼看出是谁没产出，而不是只看到"跳过"。
                        let upstream_names: Vec<String> = incoming_edges
                            .iter()
                            .map(|e| {
                                node_map
                                    .get(&e.source)
                                    .map(|n| n.label.clone())
                                    .unwrap_or_else(|| e.source.clone())
                            })
                            .collect();
                        let reason = format!(
                            "上游节点未产出，本节点跳过（上游：{}）",
                            upstream_names.join("、")
                        );
                        log::info!(
                            "[WorkflowEngine] 节点 {} 无可用上游产出，跳过：{}",
                            node.id,
                            reason
                        );
                        skipped_count.fetch_add(1, Ordering::SeqCst);
                        node_statuses
                            .lock()
                            .await
                            .insert(node_id.clone(), "skipped".to_string());
                        record_node_execution(
                            emitter,
                            execution_id,
                            node_id,
                            "skipped",
                            None,
                            None,
                            Some(&reason),
                            None,
                            None,
                            None,
                        );
                        let settled = completed_count.load(Ordering::SeqCst)
                            + skipped_count.load(Ordering::SeqCst);
                        emit_node_status(
                            emitter,
                            execution_id,
                            definition_id,
                            node_id,
                            "skipped",
                            None,
                            Some(&reason),
                            Some(settled),
                            Some(total_count),
                            mode,
                        );
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
                    let (resolved_input, unresolved_input) =
                        resolve_node_input(&node, &ctx_snapshot);
                    log_unresolved_input(emitter, execution_id, node_id, &unresolved_input);
                    record_node_execution(
                        emitter,
                        execution_id,
                        node_id,
                        "running",
                        serde_json::to_string(&resolved_input).ok().as_deref(),
                        None,
                        None,
                        None,
                        None,
                        None,
                    );
                    emit_node_status(
                        emitter,
                        execution_id,
                        definition_id,
                        node_id,
                        "running",
                        None,
                        None,
                        None,
                        None,
                        mode,
                    );

                    // delay 支持：执行前等待 delay_ms 毫秒

                    if let Some(delay_ms) = node.delay_ms {
                        if delay_ms > 0 {
                            log::info!(
                                "[WorkflowEngine] 节点 {} 延迟 {}ms 后执行",
                                node_id,
                                delay_ms
                            );
                            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                        }
                    }

                    // 子工作流同样受节点超时约束：超时按该节点失败处理，并取消子执行
                    let subflow_timeout_ms = node.timeout_ms.filter(|ms| *ms > 0);
                    let subflow_future = Self::execute_subflow_node(
                        executor,
                        &node,
                        resolved_input,
                        execution_id,
                        emitter,
                        max_concurrency,
                        visited_def_ids,
                    );
                    let sub_result = match subflow_timeout_ms {
                        Some(ms) => match tokio::time::timeout(
                            tokio::time::Duration::from_millis(ms),
                            subflow_future,
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                // 子执行 id 规则与 execute_subflow_node 内部一致：`{父执行}_{节点}`
                                let sub_execution_id = format!("{}_{}", execution_id, node.id);
                                executor.cancel_execution(&sub_execution_id);
                                log::warn!(
                                    "[WorkflowEngine] 子工作流节点 {} 执行超时（{}ms），已取消子执行 {}",
                                    node_id, ms, sub_execution_id
                                );
                                Err(AppError::External(format!("节点执行超时（{}ms）", ms)))
                            }
                        },
                        None => subflow_future.await,
                    };
                    match sub_result {
                        Ok(output) => {
                            node_statuses
                                .lock()
                                .await
                                .insert(node_id.clone(), "completed".to_string());

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
                                            // 宽松解析：未解析的占位符按空处理（不回退成 `{{...}}` 原文）
                                            let resolved = if crate::workflow::template::TemplateEngine::has_placeholder(s) {
                                                let (text, unresolved) = crate::workflow::template::TemplateEngine::resolve_lossy(s, &sub_ctx);
                                                if !unresolved.is_empty() {
                                                    log::warn!(

                                                        "[WorkflowEngine] 子工作流节点 {} 输出映射[{}] 未解析（该值按空处理）：{:?}",
                                                        node_id, k, unresolved

                                                    );
                                                }

                                                text
                                            } else {
                                                s.to_string()
                                            };
                                            result_map.insert(k.clone(), Value::String(resolved));
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
                            context
                                .lock()
                                .await
                                .insert(node_id.clone(), mapped_output.clone());
                            raw_outputs
                                .lock()
                                .await
                                .insert(node_id.clone(), output.clone());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            success_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(
                                emitter,
                                execution_id,
                                node_id,
                                "completed",
                                None,
                                serde_json::to_string(&mapped_output).ok().as_deref(),
                                None,
                                None,
                                None,
                                None,
                            );

                            // 实时更新实例完成率

                            if let Ok(conn) = get_db_conn(emitter) {
                                let settled = completed_count.load(Ordering::SeqCst)
                                    + skipped_count.load(Ordering::SeqCst);
                                update_instance_progress(
                                    &conn,
                                    execution_id,
                                    total_count,
                                    settled,
                                    skipped_count.load(Ordering::SeqCst),
                                    None,
                                );
                            }

                            let settled = completed_count.load(Ordering::SeqCst)
                                + skipped_count.load(Ordering::SeqCst);
                            emit_node_status(
                                emitter,
                                execution_id,
                                definition_id,
                                node_id,
                                "completed",
                                Some(&mapped_output),
                                None,
                                Some(settled),
                                Some(total_count),
                                mode,
                            );
                        }

                        Err(e) => {
                            node_statuses
                                .lock()
                                .await
                                .insert(node_id.clone(), "failed".to_string());
                            record_node_execution(
                                emitter,
                                execution_id,
                                node_id,
                                "failed",
                                None,
                                None,
                                Some(&e.to_string()),
                                None,
                                None,
                                None,
                            );
                            emit_node_status(
                                emitter,
                                execution_id,
                                definition_id,
                                node_id,
                                "failed",
                                None,
                                Some(&e.to_string()),
                                None,
                                None,
                                mode,
                            );
                            // 不再 return Err，让后续节点继续执行
                        }
                    }

                    // _permit 在此处 drop，释放并发许可

                    continue;
                }

                let exec_id = execution_id.to_string();
                let nid = node_id.clone();
                // 句柄侧留一份节点 id：任务 panic/被中断时 JoinError 不带节点信息，没有它就无法结算该节点
                let nid_for_join = nid.clone();
                let emitter = emitter.clone();
                let context = context.clone();
                let raw_outputs = raw_outputs.clone();
                let completed_count = completed_count.clone();
                let success_count = success_count.clone();
                let skipped_count = skipped_count.clone();
                let total = total_count;
                let exec = executor.clone();

                // 开始节点：提前提取 output_mapping，执行后用它替代空结果

                let start_output_mapping = if node.node_type == WorkflowNodeType::Start {
                    node.output_mapping
                        .as_ref()
                        .and_then(|m| m.as_object().cloned())
                } else {
                    None
                };
                let node_statuses_clone = node_statuses.clone();
                let def_id_owned = definition_id.to_string();
                let mode_owned = mode.clone();
                let cancelled = cancelled.clone();
                let handle = tokio::spawn(async move {
                    let ctx_snapshot = context.lock().await.clone();
                    log::info!(
                        "[WorkflowEngine] Executing node {} (type={:?}), context keys: {:?}",
                        nid,
                        node.node_type,
                        context.lock().await.keys().collect::<Vec<_>>()
                    );
                    let (resolved_input, unresolved_input) =
                        resolve_node_input(&node, &ctx_snapshot);
                    log_unresolved_input(&emitter, &exec_id, &nid, &unresolved_input);

                    // [DEBUG] transform 节点执行前打印 resolved_input 实际内容
                    if node.node_type == WorkflowNodeType::Transform {
                        log::info!(
                            "[TransformExecutor][DEBUG] 节点 {} 执行前 resolved_input = {} (raw: {:?})",
                            nid,
                            serde_json::to_string_pretty(&resolved_input).unwrap_or_else(|_| "<序列化失败>".to_string()),
                            resolved_input
                        );
                        // 检查是否存在常见字段名
                        if let Some(obj) = resolved_input.as_object() {
                            for (k, v) in obj {
                                log::info!(
                                    "[TransformExecutor][DEBUG]   input.{} = {:?} (type={})",
                                    k,
                                    v,
                                    match v {
                                        Value::Null => "null",
                                        Value::Bool(_) => "bool",
                                        Value::Number(_) => "number",
                                        Value::String(s) =>
                                            if s.is_empty() {
                                                "string(EMPTY!)"
                                            } else {
                                                "string"
                                            },
                                        Value::Array(_) => "array",
                                        Value::Object(_) => "object",
                                    }
                                );
                            }
                            if obj.is_empty() {
                                log::warn!("[TransformExecutor][DEBUG]   ⚠️ resolved_input 是空对象！inputMapping 可能未正确解析或上游节点未暴露输出");
                            }
                        }
                    }

                    record_node_execution(
                        &emitter,
                        &exec_id,
                        &nid,
                        "running",
                        serde_json::to_string(&resolved_input).ok().as_deref(),
                        None,
                        None,
                        None,
                        None,
                        None,
                    );
                    emit_node_status(
                        &emitter,
                        &exec_id,
                        &def_id_owned,
                        &nid,
                        "running",
                        None,
                        None,
                        None,
                        None,
                        &mode_owned,
                    );

                    // delay 支持：执行前等待 delay_ms 毫秒

                    if let Some(delay_ms) = node.delay_ms {
                        if delay_ms > 0 {
                            log::info!("[WorkflowEngine] 节点 {} 延迟 {}ms 后执行", nid, delay_ms);
                            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                        }
                    }

                    // 取消检查：delay 后执行前
                    if cancelled.load(Ordering::SeqCst) {
                        log::info!(
                            "[WorkflowEngine] 节点 {} 在 delay 后检测到取消信号，跳过执行",
                            nid
                        );
                        node_statuses_clone
                            .lock()
                            .await
                            .insert(nid.clone(), "cancelled".to_string());
                        emit_node_status(
                            &emitter,
                            &exec_id,
                            &def_id_owned,
                            &nid,
                            "cancelled",
                            None,
                            None,
                            None,
                            None,
                            &mode_owned,
                        );
                        return Ok(());
                    }

                    let mut node_def = node_to_node_def(&node);

                    // Agent 节点：解析 resume_session_ref 模板（从 context 获取实际 session_id）

                    if node_def.node_type == "agent" {
                        if let Some(ref_mode) =
                            node_def.config.get("session_mode").and_then(|v| v.as_str())
                        {
                            if ref_mode == "resume" {
                                if let Some(ref_tmpl) = node_def
                                    .config
                                    .get("resume_session_ref")
                                    .and_then(|v| v.as_str())
                                {
                                    if !ref_tmpl.is_empty() {
                                        match TemplateEngine::resolve(ref_tmpl, &ctx_snapshot) {
                                            Ok(resolved_sid) => {
                                                log::info!("[WorkflowEngine] Agent node {} resume session resolved: {} -> {}", nid, ref_tmpl, resolved_sid);
                                                node_def.config.as_object_mut().map(|map| {
                                                    map.insert(
                                                        "resume_session_id".to_string(),
                                                        Value::String(resolved_sid),
                                                    )
                                                });
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

                    // 使用 tokio::select! 同时监听执行结果、取消信号与节点超时。
                    // 超时（控制属性的「超时 (ms)」）是防卡死的硬上限：没有它，跑飞的命令或挂起的
                    // 请求会让执行一直挂着，只能手动终止。超时按"这个节点失败"处理——走 `abort_node`
                    // 做节点级收尾（停子进程、撤等待登记），不取消整个工作流。
                    let node_timeout_ms = node.timeout_ms.filter(|ms| *ms > 0);
                    let result = tokio::select! {
                        r = exec.execute(&node_def, resolved_input.clone(), &exec_id, &emitter) => r,
                        _ = async {
                            // 轮询取消信号（200ms 间隔），正常执行路径几乎零开销
                            while !cancelled.load(Ordering::SeqCst) {
                                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                            }
                        } => {
                            log::info!("[WorkflowEngine] 节点 {} 执行被取消信号中断", nid);
                            Err(AppError::External("工作流已被取消".into()))
                        }
                        _ = async {
                            match node_timeout_ms {
                                Some(ms) => tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await,
                                None => std::future::pending::<()>().await,
                            }
                        } => {
                            let ms = node_timeout_ms.unwrap_or(0);
                            // 该闭包是 spawn 出来的 'static 任务，只能用提前 clone 的 Arc（`exec`）
                            exec.abort_node(&exec_id, &nid);
                            log::warn!("[WorkflowEngine] 节点 {} 执行超时（{}ms），已中断该节点", nid, ms);
                            Err(AppError::External(format!("节点执行超时（{}ms）", ms)))
                        }
                    };
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

                                context.lock().await.insert(
                                    format!("__session_id__{}", nid),
                                    Value::String(sid.clone()),
                                );
                            }

                            // 根据 outputMapping 构建暴露给后续节点的 context

                            let output_mapping = node.output_mapping.as_ref();
                            if node.node_type == WorkflowNodeType::Start {
                                // Start 节点：预填充阶段已写入完整 context，跳过覆盖

                                log::info!("[WorkflowEngine] Start node {} context preserved from pre-population", nid);
                            } else if let Some(mapping) = output_mapping.and_then(|m| m.as_object())
                            {
                                // 非 Start 节点：暴露 outputMapping 声明的字段

                                // 匹配逻辑：判断 path（下拉选择的值）而非 key（用户自定义字段名）

                                log::info!(
                                    "[WorkflowEngine][DEBUG] 节点 {} (type={:?}) outputMapping 映射内容: {:?} | node_output.content = {:?}",
                                    nid, node.node_type, mapping, node_output.get("content")
                                );
                                let exposed = expose_node_output(
                                    mapping,
                                    &node_output,
                                    output.session_id.as_deref(),
                                );
                                for (key, path) in mapping {
                                    log::info!(
                                        "[WorkflowEngine][DEBUG]   outputMapping[{}] -> {:?}, exposed_value = {:?}",
                                        key, path, exposed.get(key)
                                    );
                                }

                                let exposed_keys: Vec<String> = exposed.keys().cloned().collect();
                                let exposed_clone = exposed.clone(); // 在移动前先 clone 供日志用

                                context
                                    .lock()
                                    .await
                                    .insert(nid.clone(), Value::Object(exposed));
                                log::info!(
                                    "[WorkflowEngine][DEBUG] Node {} context (outputMapping): keys={:?}, context_value={:?}",
                                    nid, exposed_keys, Value::Object(exposed_clone)
                                );
                            } else if node.node_type == WorkflowNodeType::End {
                                // End 节点：无 outputMapping，输出 = resolved_input（所有 inputMapping 组成的对象）

                                context
                                    .lock()
                                    .await
                                    .insert(nid.clone(), resolved_input.clone());
                                log::info!(
                                    "[WorkflowEngine] End node {} context: resolved_input = {:?}",
                                    nid,
                                    resolved_input
                                );
                            } else {
                                // 无 outputMapping：
                                // - interact 节点：从 { content: xxx } 中提取 content 暴露为 output
                                // - agent 节点：裸字符串直接暴露为 output
                                // - 其他节点：默认不暴露任何字段（空对象）

                                let exposed = if node.node_type == WorkflowNodeType::Interact {
                                    let mut m = serde_json::Map::new();
                                    let content_val = output.output.get("content");
                                    let output_keys: Vec<String> = output
                                        .output
                                        .as_object()
                                        .map(|o| o.keys().cloned().collect())
                                        .unwrap_or_default();
                                    log::info!(
                                        "[WorkflowEngine][DEBUG] interact 节点 {} 无 outputMapping，开始自动暴露 | node_output.content = {:?} | node_output 完整 keys: {:?}",
                                        nid, content_val, output_keys
                                    );
                                    if let Some(val) = content_val {
                                        m.insert("output".into(), val.clone());
                                        log::info!(
                                            "[WorkflowEngine][DEBUG]   ✅ interact 节点 {} 自动暴露 output = {:?} (type={})",
                                            nid, val,
                                            match val {
                                                Value::Null => "null",
                                                Value::Bool(_) => "bool",
                                                Value::Number(_) => "number",
                                                Value::String(s) => if s.is_empty() { "string(EMPTY!)" } else { "string" },
                                                Value::Array(_) => "array",
                                                Value::Object(_) => "object",
                                            }
                                        );
                                    } else {
                                        log::warn!(
                                            "[WorkflowEngine][DEBUG]   ⚠️ interact 节点 {} node_output 中找不到 'content' 字段！完整 output: {:?}",
                                            nid, output.output
                                        );
                                    }
                                    m
                                } else if node.node_type == WorkflowNodeType::Agent {
                                    // Agent 节点输出是裸字符串（Value::String），无 outputMapping 时直接暴露为 output
                                    let mut m = serde_json::Map::new();
                                    log::info!(
                                        "[WorkflowEngine][DEBUG] agent 节点 {} 无 outputMapping，自动暴露裸字符串为 output",
                                        nid
                                    );
                                    m.insert("output".into(), node_output.clone());
                                    m
                                } else {
                                    serde_json::Map::new()
                                };
                                let exposed_keys: Vec<String> = exposed.keys().cloned().collect();
                                let exposed_clone = exposed.clone(); // 在移动前先 clone 供日志用
                                context
                                    .lock()
                                    .await
                                    .insert(nid.clone(), Value::Object(exposed));
                                log::info!(
                                    "[WorkflowEngine][DEBUG] 节点 {} (type={:?}) 无 outputMapping，暴露到 context 的 keys: {:?} | 值: {:?}",
                                    nid, node.node_type, exposed_keys,
                                    if exposed_keys.is_empty() { Value::Object(serde_json::Map::new()) } else { Value::Object(exposed_clone) }
                                );
                            }

                            // 保存原始执行结果到 raw_outputs（供门控合并使用）

                            raw_outputs
                                .lock()
                                .await
                                .insert(nid.clone(), output.output.clone());
                            log::info!(
                                "[WorkflowEngine] Node {} output written to context: {:?}",
                                nid,
                                node_output
                            );
                            node_statuses_clone
                                .lock()
                                .await
                                .insert(nid.clone(), "completed".to_string());
                            completed_count.fetch_add(1, Ordering::SeqCst);
                            success_count.fetch_add(1, Ordering::SeqCst);
                            record_node_execution(
                                &emitter,
                                &exec_id,
                                &nid,
                                "completed",
                                None,
                                serde_json::to_string(&node_output).ok().as_deref(),
                                None,
                                output.session_id.as_deref(),
                                output.input_data.as_deref(),
                                output.artifacts_path.as_deref(),
                            );
                            let settled = completed_count.load(Ordering::SeqCst)
                                + skipped_count.load(Ordering::SeqCst);
                            emit_node_status(
                                &emitter,
                                &exec_id,
                                &def_id_owned,
                                &nid,
                                "completed",
                                Some(&node_output),
                                None,
                                Some(settled),
                                Some(total),
                                &mode_owned,
                            );

                            // 实时更新实例完成率

                            if let Ok(conn) = get_db_conn(&emitter) {
                                update_instance_progress(
                                    &conn,
                                    &exec_id,
                                    total,
                                    settled,
                                    skipped_count.load(Ordering::SeqCst),
                                    None,
                                );
                            }

                            return Ok(());
                        }

                        Err(e) => {
                            let final_status = if cancelled.load(Ordering::SeqCst) {
                                "cancelled"
                            } else {
                                "failed"
                            };
                            let error_str = e.to_string();
                            let error_msg: &str = if final_status == "cancelled" {
                                "用户中止"
                            } else {
                                &error_str
                            };
                            node_statuses_clone
                                .lock()
                                .await
                                .insert(nid.clone(), final_status.to_string());
                            record_node_execution(
                                &emitter,
                                &exec_id,
                                &nid,
                                final_status,
                                None,
                                None,
                                Some(error_msg),
                                None,
                                None,
                                None,
                            );
                            let settled = completed_count.load(Ordering::SeqCst)
                                + skipped_count.load(Ordering::SeqCst);
                            emit_node_status(
                                &emitter,
                                &exec_id,
                                &def_id_owned,
                                &nid,
                                final_status,
                                None,
                                Some(error_msg),
                                Some(settled),
                                Some(total),
                                &mode_owned,
                            );

                            // 不再 return Err，让同层其他节点继续执行

                            // 实时更新实例完成率（分子仍是「已结算」：失败节点不算完成，
                            // 跳过节点算——与终态判定同口径）
                            if let Ok(conn) = get_db_conn(&emitter) {
                                update_instance_progress(
                                    &conn,
                                    &exec_id,
                                    total,
                                    settled,
                                    skipped_count.load(Ordering::SeqCst),
                                    None,
                                );
                            }

                            return Ok(());
                        }
                    }
                });
                handles.push((nid_for_join, handle));
            }

            for (nid, handle) in handles {
                match handle.await {
                    Ok(Err(e)) => {
                        // 节点执行失败，状态已记录在 node_statuses 中，不终止阶段

                        log::warn!("[WorkflowEngine] 节点执行失败(已容忍): {}", e);
                    }

                    // 任务 panic / 被运行时中断：节点只会留下 "running" 记录，界面上表现为
                    // "节点一直转圈且工作流已结束"。必须补一条终态，否则用户永远看不到结论。
                    Err(join_err) => {
                        log::warn!(
                            "[WorkflowEngine] 节点 {} 执行任务异常退出: {}",
                            nid,
                            join_err
                        );
                        node_statuses
                            .lock()
                            .await
                            .insert(nid.clone(), "failed".to_string());
                        let reason = format!("节点执行任务异常退出（{}）", join_err);
                        record_node_execution(
                            emitter,
                            execution_id,
                            &nid,
                            "failed",
                            None,
                            None,
                            Some(&reason),
                            None,
                            None,
                            None,
                        );
                        let settled = completed_count.load(Ordering::SeqCst)
                            + skipped_count.load(Ordering::SeqCst);
                        emit_node_status(
                            emitter,
                            execution_id,
                            definition_id,
                            &nid,
                            "failed",
                            None,
                            Some(&reason),
                            Some(settled),
                            Some(total_count),
                            mode,
                        );
                    }

                    _ => {}
                }
            }
        }

        let statuses = node_statuses.lock().await.clone();
        Ok((statuses, Value::Null))
    }

    /// 评估边上的条件表达式

    /// 支持：==, !=, >, <, >=, <=, contains, starts_with, ends_with, is_empty, is_not_empty

    /// 入边条件判定。
    ///
    /// 条件格式为 `[字段] 运算符 值`（与前端条件编辑器一致）：
    /// - `score >= 60`：从上游输出对象中取 `score` 字段参与比较
    /// - `>= 60`：省略字段，取上游输出的第一个值（兼容旧数据）
    fn evaluate_condition(condition: &str, source_output: Option<&Value>) -> bool {
        // 解析出字段名，并把 condition 归一化为 `运算符 值`，后续分支沿用原有解析逻辑
        let cond_parts: Vec<&str> = condition.split_whitespace().collect();
        let has_field = cond_parts.len() >= 2
            && !CONDITION_OPERATORS.contains(&cond_parts[0])
            && CONDITION_OPERATORS.contains(&cond_parts[1]);
        let (cond_field, condition): (Option<&str>, String) = if has_field {
            (Some(cond_parts[0]), cond_parts[1..].join(" "))
        } else {
            (None, condition.to_string())
        };
        match source_output {
            Some(output) => {
                // 指定字段时按字段名取值；未指定字段时对 Array 取第一个元素、对 Object 取第一个 value
                let resolved = match cond_field {
                    Some(field) => resolve_condition_field(output, field),
                    None => match output {
                        Value::Array(arr) => arr.first().cloned().unwrap_or(Value::Null),
                        Value::Object(map) => map.values().next().cloned().unwrap_or(Value::Null),
                        other => other.clone(),
                    },
                };

                // 取值归一化：JSON 文本解析 + 单值解包，与输入映射/模板取值保持同一语义
                let resolved = TemplateEngine::normalize_value(&resolved);
                let output_str = resolved.to_string();
                let trimmed = condition_value_text(&resolved);

                // 取值缺失（字段不存在或为 null）时无法比较大小，直接判不满足
                if resolved.is_null()
                    && matches!(
                        condition.split_whitespace().next(),
                        Some(">") | Some(">=") | Some("<") | Some("<=")
                    )
                {
                    return false;
                }

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

                    return trimmed.as_str() >= val.trim();
                }

                if let Some(val) = condition.strip_prefix("<=") {
                    if let (Ok(a), Ok(b)) = (trimmed.parse::<f64>(), val.trim().parse::<f64>()) {
                        return a <= b;
                    }

                    return trimmed.as_str() <= val.trim();
                }

                if let Some(val) = condition.strip_prefix(">") {
                    if let (Ok(a), Ok(b)) = (trimmed.parse::<f64>(), val.trim().parse::<f64>()) {
                        return a > b;
                    }

                    return trimmed.as_str() > val.trim();
                }

                if let Some(val) = condition.strip_prefix("<") {
                    if let (Ok(a), Ok(b)) = (trimmed.parse::<f64>(), val.trim().parse::<f64>()) {
                        return a < b;
                    }

                    return trimmed.as_str() < val.trim();
                }

                if let Some(val) = condition.strip_prefix("contains") {
                    return output_str.contains(val.trim());
                }

                if let Some(val) = condition.strip_prefix("starts_with") {
                    return condition_value_text(&resolved).starts_with(val.trim());
                }
                if let Some(val) = condition.strip_prefix("ends_with") {
                    return condition_value_text(&resolved).ends_with(val.trim());
                }
                if condition.trim() == "is_empty" {
                    return is_condition_value_empty(&resolved);
                }
                if condition.trim() == "is_not_empty" {
                    return !is_condition_value_empty(&resolved);
                }
                !output.is_null() && !output_str.is_empty()
            }

            None => false,
        }
    }

    /// 入边链路连通性判定：多入边取 OR，任一入边连通即可执行该节点。
    ///
    /// 一条连线连通需同时满足：
    /// - 上游节点真的执行过：执行成功会把产出写入 `ctx`，被条件跳过或未执行的上游没有产出，
    ///   说明这条链路根本没走到，不能激活下游（否则条件分支跳过的链路会被按层级继续执行）；
    /// - 连线无条件，或条件成立。
    fn any_incoming_edge_active(edges: &[&WorkflowEdge], ctx: &HashMap<String, Value>) -> bool {
        edges.iter().any(|edge| {
            let Some(source_output) = ctx.get(&edge.source) else {
                return false;
            };
            match &edge.condition {
                Some(cond) => Self::evaluate_condition(cond, Some(source_output)),
                None => true,
            }
        })
    }

    /// 检查门控策略是否满足进入下一阶段的条件

    ///

    /// 统一流程：先执行 merge，再检查策略。

    /// - All: 阶段内所有非边界节点均成功完成 → 放行；任一失败 → 中止

    /// - count: 至少 `gate.threshold` 指定的节点数成功完成 → 放行；不足 → 中止

    /// - threshold: 基于 merge 后的值做条件判断，表达式取自 `gate.threshold`（如 ">= 60"）

    ///   不满足 → 中止

    fn check_gate_strategy(
        stage: &Stage,
        node_statuses: &HashMap<String, String>,
        merged_value: &Value,
    ) -> Result<GateDecision, AppError> {
        // 收集阶段内非边界节点的执行状态

        let non_boundary_statuses: Vec<(&String, &String)> = stage
            .nodes
            .iter()
            .filter(|n| {
                n.node_type != WorkflowNodeType::Start && n.node_type != WorkflowNodeType::End
            })
            .filter_map(|n| node_statuses.get(&n.id).map(|s| (&n.id, s)))
            .collect();
        let total = non_boundary_statuses.len();
        let success_count = non_boundary_statuses
            .iter()
            .filter(|(_, s)| *s == "completed")
            .count();
        let failed_count = non_boundary_statuses
            .iter()
            .filter(|(_, s)| *s == "failed")
            .count();
        log::info!(
            "[check_gate_strategy] stage={}, strategy={:?}, total={}, success={}, failed={}",
            stage.name,
            stage.gate.strategy,
            total,
            success_count,
            failed_count
        );
        match &stage.gate.strategy {
            GateStrategy::All => {
                // 全部完成：所有非边界节点必须成功

                if failed_count > 0 {
                    // 正常门控结果：原因原样带出，由调用方按「策略未通过」上报
                    Ok(GateDecision::Blocked(format!(
                        "阶段 '{}' 门控策略 All 不满足: {}/{} 个节点失败",
                        stage.name, failed_count, total
                    )))
                } else {
                    Ok(GateDecision::Pass)
                }
            }

            GateStrategy::Count => {
                // 指定数量完成：至少 n 个节点成功；n 取自 gate.threshold（前端独立字段）
                let required = stage
                    .gate
                    .threshold
                    .as_deref()
                    .and_then(|v| v.trim().parse::<usize>().ok());
                match required {
                    Some(n) if success_count >= n => Ok(GateDecision::Pass),
                    Some(n) => Ok(GateDecision::Blocked(format!(
                        "阶段 '{}' 门控策略 count 不满足: 仅 {}/{} 个节点成功（需要 {}）",
                        stage.name, success_count, total, n
                    ))),
                    // 配置本身有问题（没填门槛数）：这才是真异常
                    None => Err(AppError::External(format!(
                        "阶段 '{}' 门控策略 count 缺少有效的完成节点数（threshold={:?}）",
                        stage.name, stage.gate.threshold
                    ))),
                }
            }

            GateStrategy::Threshold => {
                // 按条件判断：基于 merge 后的值做条件判断（表达式取自 gate.threshold）
                // 复用 evaluate_condition 逻辑，将 merged_value 作为 source_output
                let expr = stage.gate.threshold.clone().unwrap_or_default();
                if expr.trim().is_empty() {
                    return Err(AppError::External(format!(
                        "阶段 '{}' 门控策略 threshold 缺少条件表达式（threshold 为空）",
                        stage.name
                    )));
                }
                let passed = Self::evaluate_condition(&expr, Some(merged_value));
                if passed {
                    Ok(GateDecision::Pass)
                } else {
                    Ok(GateDecision::Blocked(format!(
                        "阶段 '{}' 门控策略 threshold 不满足: 合并值未满足条件 '{}'",
                        stage.name, expr
                    )))
                }
            }
        }
    }

    /// 执行 Gate 合并逻辑

    fn merge_stage_outputs(stage: &Stage, raw_outputs: &HashMap<String, Value>) -> Value {
        let node_outputs: Vec<(&String, &Value)> = stage
            .nodes
            .iter()
            .filter_map(|n| raw_outputs.get(&n.id).map(|v| (&n.id, v)))
            // 无实质产出的节点（空对象：开始/结束这类不做实际工作的边界节点、本轮无产出的节点）
            // 不参与合并。否则合并结果里会多出一批 `"<节点ID>": {}` 噪音键，
            // 下游 `{{gate_output.<阶段ID>.<节点ID>}}` 还会取到"存在但为空"的假值。
            .filter(|(_, value)| !is_empty_contribution(value))
            .collect();
        match stage.gate.merge_strategy {
            MergeStrategy::Merge => {
                // 按「节点ID → 该节点的执行结果数据」整值成键。
                // 键名只由节点ID决定（不随值形态在 `<ID>_<字段>` 与 `<ID>` 之间抖动），
                // 有产出的节点必然出现；下游 `{{gate_output.<阶段ID>.<节点ID>}}`
                // 与 `{{gate_output.<阶段ID>.<节点ID>.<字段>}}` 都能稳定取值。
                let mut merged = serde_json::Map::new();
                for (node_id, output) in &node_outputs {
                    merged.insert(sanitize_context_key(node_id), (*output).clone());
                }

                Value::Object(merged)
            }

            MergeStrategy::Concat => {
                let arr: Vec<Value> = node_outputs.iter().map(|(_, v)| (*v).clone()).collect();
                Value::Array(arr)
            }

            MergeStrategy::PickFirst => node_outputs
                .first()
                .map(|(_, v)| (*v).clone())
                .unwrap_or(Value::Null),

            MergeStrategy::PickLast => node_outputs
                .last()
                .map(|(_, v)| (*v).clone())
                .unwrap_or(Value::Null),

            MergeStrategy::Custom => {
                // 自定义脚本取自 gate.custom_script（前端独立字段）

                let script = stage.gate.custom_script.as_deref().unwrap_or("");

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
                            if let Some((_, node_val)) =
                                node_outputs.iter().find(|(id, _)| id.as_str() == node_id)
                            {
                                let extracted = extract_by_path(node_val, field_path);
                                if extracted != Value::Null {
                                    let key = field_path
                                        .split('.')
                                        .last()
                                        .map(|s| s.to_string())
                                        .unwrap_or_default();
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
                        if let Some((_, val)) =
                            node_outputs.iter().find(|(id, _)| id.as_str() == node_id)
                        {
                            extract_by_path(val, field_path)
                        } else {
                            Value::Null
                        }
                    } else {
                        node_outputs
                            .iter()
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
                            "success" => true, // node_outputs 已是已完成节点

                            _ => true,
                        };
                        if !include {
                            continue;
                        }

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
                            if let Some(&max_val) = numeric_values.iter().max_by(|a, b| {
                                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                            }) {
                                Value::Number(
                                    serde_json::Number::from_f64(max_val)
                                        .unwrap_or(serde_json::Number::from(0)),
                                )
                            } else {
                                Value::Null
                            }
                        }

                        "min" => {
                            if let Some(&min_val) = numeric_values.iter().min_by(|a, b| {
                                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                            }) {
                                Value::Number(
                                    serde_json::Number::from_f64(min_val)
                                        .unwrap_or(serde_json::Number::from(0)),
                                )
                            } else {
                                Value::Null
                            }
                        }

                        "avg" => {
                            if !numeric_values.is_empty() {
                                let avg = numeric_values.iter().sum::<f64>()
                                    / numeric_values.len() as f64;
                                Value::Number(
                                    serde_json::Number::from_f64(avg)
                                        .unwrap_or(serde_json::Number::from(0)),
                                )
                            } else {
                                Value::Null
                            }
                        }

                        "sum" => {
                            let sum = numeric_values.iter().sum::<f64>();
                            Value::Number(
                                serde_json::Number::from_f64(sum)
                                    .unwrap_or(serde_json::Number::from(0)),
                            )
                        }

                        "count" => Value::Number(serde_json::Number::from(numeric_values.len())),

                        "first" => all_values.first().cloned().unwrap_or(Value::Null),

                        "last" => all_values.last().cloned().unwrap_or(Value::Null),

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

        let subflow_def_id = node
            .params
            .as_ref()
            .and_then(|p| p.get("definitionId"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::InvalidInput("Subflow 节点缺少 definitionId 参数".into()))?;

        // 加载子工作流定义

        let conn = get_db_conn(emitter)?;
        let subflow_def = super::get_definition(&conn, subflow_def_id)?.ok_or_else(|| {
            AppError::InvalidInput(format!("子工作流定义不存在: {}", subflow_def_id))
        })?;

        // 检查 maxDepth（默认 10）

        let max_depth = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'workflow_max_subflow_depth'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        if visited_def_ids.len() >= max_depth {
            return Err(AppError::InvalidInput(format!(
                "子工作流嵌套深度 {} 超过最大限制 {}（maxDepth）。工作流: \"{}\" -> \"{}\"",
                visited_def_ids.len() + 1,
                max_depth,
                subflow_def.name,
                subflow_def.name
            )));
        }

        // 检查循环引用

        if visited_def_ids.contains(&subflow_def_id.to_string()) {
            let chain = visited_def_ids
                .iter()
                .chain(std::iter::once(&subflow_def_id.to_string()))
                .cloned()
                .collect::<Vec<_>>()
                .join(" → ");
            return Err(AppError::InvalidInput(format!(
                "检测到工作流循环引用：{}。请检查工作流定义中的 Subflow 节点配置。",
                chain
            )));
        }

        // 创建子工作流的 execution_id（使用父 execution_id + 节点 id 作为子 execution_id）

        let sub_execution_id = format!("{}_{}", execution_id, node.id);

        // 创建子工作流实例记录

        let now = crate::utils::now();

        // 事件化：子工作流实例创建（execution/created，随父流程级联记录）。
        let event = serde_json::json!({
            "executionId": &sub_execution_id,
            "definitionId": &subflow_def_id,
            "definitionName": &subflow_def.name,
            "status": "running",
            "trigger": "subflow",
            "startedAt": now,
            "createdAt": now,
        });
        let _ = crate::eventlog::append_workflow_event(
            &conn,
            &sub_execution_id,
            "execution/created",
            &event,
            true,
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
            &ExecutionMode::Full,
            None,
        )
        .await;

        // 子工作流实例状态已由 execute_with_concurrency_impl 内部的 final_status 统一处理，
        // 此处不再二次覆盖，避免将 cancelled 错误覆盖为 success/failed

        // 子工作流节点的「执行结果数据」与主工作流节点同源：取子流程执行返回的最终产出。
        // 只做键名净化——内部 `gate_output.<阶段ID>` 这类带点键在模板里点号是路径分隔符，
        // 不净化就无法被下游整键引用。
        result.map(sanitize_result_keys)
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
        Self::execute_with_concurrency_impl(
            executor,
            def,
            execution_id,
            input_data,
            emitter,
            max_concurrency,
            &[],
            &ExecutionMode::Full,
            None,
        )
        .await
    }

    /// 内部实现：支持 visited_def_ids 循环引用检测

    ///

    /// `mode`: 本次执行模式，随进度事件原样上报（全量执行为 [`ExecutionMode::Full`]）。

    /// `recovery`: 断点执行的补充信息（进度分母、门控历史状态与基线）；全量执行传 `None`。

    async fn execute_with_concurrency_impl(
        executor: &Arc<NodeExecutor>,
        def: &WorkflowDefinition,
        execution_id: &str,
        input_data: Value,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
        visited_def_ids: &[String],
        mode: &ExecutionMode,
        recovery: Option<&RecoveryContext>,
    ) -> Result<Value, AppError> {
        let _guard = ExecutionGuard {
            executor: executor.as_ref(),
            execution_id: execution_id.to_string(),
        };
        let definition_id = def.id.clone();

        // -- inputSchema：先注入默认值，再校验 --
        //
        // 顶层执行（手动 / 定时 / 事件触发）没有上游节点可以补齐入参，所以：
        //   1. schema 里写了 `default` 的字段，缺失时注入默认值——这是顶层唯一的入参来源；
        //   2. 仍然缺失的 `required` 字段只记警告，不再让执行失败（否则"给定义标了 required"
        //      等于把这个工作流彻底锁死，这也是它此前形同虚设却又能把人卡住的原因）。
        // 子工作流（Subflow 节点调用）仍按契约强校验：入参来自上游 input_mapping，缺了就是定义错了。
        let mut input_data = input_data;
        apply_input_schema_defaults(&mut input_data, &def.input_schema);
        let is_top_level = visited_def_ids.is_empty();

        if let Some(schema) = &def.input_schema {
            if let Some(schema_obj) = schema.as_object() {
                for (field, rules) in schema_obj {
                    if let Some(rules_obj) = rules.as_object() {
                        let required = rules_obj
                            .get("required")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if required {
                            let has_value = match input_data.get(field) {
                                Some(v) => !v.is_null(),
                                None => false,
                            };
                            if !has_value {
                                let msg = format!(
                                    "工作流 \"{}\" 缺少必填输入参数: {}（类型: {}）",
                                    def.name,
                                    field,
                                    rules_obj
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown")
                                );
                                if is_top_level {
                                    log::warn!(
                                        "[WorkflowEngine] {}（顶层执行无上游可补齐，模板引用该字段会得到空值）",
                                        msg
                                    );
                                } else {
                                    return Err(AppError::InvalidInput(msg));
                                }
                            }
                        }

                        if let Some(expected_type) = rules_obj.get("type").and_then(|v| v.as_str())
                        {
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
        log::info!(
            "[WorkflowEngine] 开始执行: id={}, stages={}, total_nodes={}",
            execution_id,
            def.stages.len(),
            total_count
        );
        let context: Arc<AsyncMutex<HashMap<String, Value>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));
        let raw_outputs: Arc<AsyncMutex<HashMap<String, Value>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));
        context
            .lock()
            .await
            .insert("__input__".to_string(), input_data.clone());

        // 断点执行（单点/链式/补全）的种子 = 实例上下文 + 从事件重建的前序节点输出，它们的键必须
        // **平铺在 context 顶层**：节点输入里的模板形如 {{<节点id>.output}}，解析走的是顶层键
        // （context.get(节点id)），只在 __input__ 下是命中不了的（__input__ 回退只覆盖无点号的
        // 简单变量名）。实时执行里这些键本就是顶层键（节点完成时逐个写入），这里补齐同一形态。
        if let Some(obj) = input_data.as_object() {
            let mut ctx = context.lock().await;
            for (k, v) in obj {
                if k == "__input__" {
                    continue;
                }
                ctx.insert(k.clone(), v.clone());
            }
        }

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
                                    // 宽松解析：未解析的占位符按空处理，不回退成 `{{...}}` 原文
                                    let (resolved, unresolved_refs) =
                                        TemplateEngine::resolve_lossy(s, &ctx);
                                    if !unresolved_refs.is_empty() {
                                        log::warn!(
                                            "[WorkflowEngine] 开始节点 {} 输出映射[{}] 未解析（该值按空处理）：{:?}",
                                            node.id, k, unresolved_refs
                                        );
                                    }

                                    output_obj.insert(k.clone(), Value::String(resolved.clone()));

                                    // 设置扁平键，支持简单 {{参数名}} 格式

                                    context
                                        .lock()
                                        .await
                                        .insert(k.clone(), Value::String(resolved));
                                } else {
                                    output_obj.insert(k.clone(), v.clone());
                                    context.lock().await.insert(k.clone(), v.clone());
                                }
                            }

                            context
                                .lock()
                                .await
                                .insert(node.id.clone(), Value::Object(output_obj));
                        }
                    }

                    break;
                }
            }
        }

        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        log::info!(
            "[WorkflowEngine] Context after start node pre-population: {:?}",
            context.lock().await.keys().collect::<Vec<_>>()
        );

        // 使用统一的执行计划（消除重复 BFS 逻辑）

        let plan = Self::compute_execution_plan(def);
        let reachable_nodes: std::collections::HashSet<String> =
            plan.reachable_node_ids.iter().cloned().collect();

        // 进度分母：断点模式用完整定义的可达节点数（见 progress_total）。
        let total_count = progress_total(recovery, reachable_nodes.len());
        let success_count = Arc::new(AtomicUsize::new(0));
        // 条件分支未命中 / 上游无产出而跳过的节点数：计入"已结算"（分子），
        // 否则带条件的工作流完成度永远到不了 100%（见 resolve_terminal_status）。
        let skipped_count = Arc::new(AtomicUsize::new(0));
        log::info!(
            "[WorkflowEngine] 执行计划: stages layers={:?}, reachable_nodes={}",
            plan.ordered_stage_ids,
            total_count
        );

        // 执行中断的原因（门控拦住 / 节点失败 / 阶段任务异常 / 用户取消）。
        // 这里**不能**直接 return：下面那段"写终态事件"会被跳过，实例就永远停在 running
        // （表现为：定时任务已因节点失败终止，定义页与实例页却一直显示"运行中"）。
        let mut abort_reason: Option<String> = None;
        'layers: for (layer_idx, layer) in plan.ordered_stage_ids.iter().enumerate() {
            // 同层阶段并行执行（无依赖关系的阶段可同时运行）

            let mut handles: Vec<tokio::task::JoinHandle<Result<Option<String>, AppError>>> =
                Vec::new();
            for stage_id in layer {
                let stage = def.stages.iter().find(|s| s.id == *stage_id).unwrap();
                if stage.nodes.is_empty() {
                    log::info!(
                        "[WorkflowEngine] 跳过空阶段: id={}, name={}",
                        stage.id,
                        stage.name
                    );
                    continue;
                }

                log::info!(
                    "[WorkflowEngine] 执行阶段: id={}, name={}, nodes={}, edges={}",
                    stage.id,
                    stage.name,
                    stage.nodes.len(),
                    stage.edges.len()
                );
                if cancelled.load(Ordering::SeqCst) {
                    abort_reason = Some("工作流已被取消".into());
                    break 'layers;
                }

                // clone shared state for spawn

                let exec_id = execution_id.to_string();
                let stage_clone = stage.clone();

                // 边界入边（仅条件判定用）：断点执行时阶段被裁剪成 scope 子集，
                // 来自 scope 外节点的入边条件不该因此丢失。
                let stage_boundary_edges: Vec<WorkflowEdge> = recovery
                    .and_then(|r| r.boundary_edges.get(&stage.id))
                    .cloned()
                    .unwrap_or_default();

                // 跨阶段入边：连线按来源阶段存放，需归集到目标阶段才能参与"链路是否连通"的判定，
                // 否则目标节点会被当成阶段入口节点无条件执行（条件分支跳过的链路仍被按层级执行）。
                // 只取来源阶段位于更早层（已执行完毕）的连线；同层阶段并行，时序不确定，维持原有不判定行为。
                let stage_node_ids: std::collections::HashSet<&str> =
                    stage.nodes.iter().map(|n| n.id.as_str()).collect();
                let prior_layer_node_ids: std::collections::HashSet<&str> = plan
                    .ordered_stage_ids
                    .iter()
                    .take(layer_idx)
                    .flat_map(|l| l.iter())
                    .filter_map(|sid| def.stages.iter().find(|s| s.id == *sid))
                    .flat_map(|s| s.nodes.iter())
                    .map(|n| n.id.as_str())
                    .collect();
                let stage_external_edges: Vec<WorkflowEdge> = def
                    .stages
                    .iter()
                    .flat_map(|s| s.edges.iter())
                    .filter(|e| {
                        stage_node_ids.contains(e.target.as_str())
                            && !stage_node_ids.contains(e.source.as_str())
                    })
                    .filter(|e| prior_layer_node_ids.contains(e.source.as_str()))
                    .cloned()
                    .collect();

                // 门控与 gate_output 的断点口径数据在 spawn 前取出为自有值（异步任务要求 'static）。
                let gate_stage_full: Option<Stage> =
                    recovery.and_then(|r| r.full_stages.get(&stage.id)).cloned();
                let gate_history_statuses: HashMap<String, String> = recovery
                    .map(|r| r.history_statuses.clone())
                    .unwrap_or_default();
                let gate_output_baseline: Option<Value> = recovery
                    .and_then(|r| r.gate_output_baseline.get(&stage.id))
                    .cloned();
                let context = context.clone();
                let raw_outputs = raw_outputs.clone();
                let emitter = emitter.clone();
                let cancelled = cancelled.clone();
                let completed_count = completed_count.clone();
                let success_count = success_count.clone();
                let skipped_count = skipped_count.clone();
                let semaphore = semaphore.clone();
                let executor = executor.clone();
                let reachable_nodes = reachable_nodes.clone();
                let visited_def_ids = visited_def_ids.to_vec();
                let definition_id = definition_id.clone();
                let mode = mode.clone();
                handles.push(tokio::spawn(async move {
                    emit_progress(
                        &emitter,
                        &exec_id,
                        &definition_id,
                        &mode,
                        None,
                        Some(serde_json::json!({
                            "id": stage_clone.id.clone(),
                            "name": stage_clone.name.clone(),
                            "status": "running",
                        })),
                        None,
                        None,
                    );
                    let (node_statuses, _) = WorkflowEngine::execute_stage(
                        &executor,
                        &stage_clone,
                        &exec_id,
                        &definition_id,
                        &mode,
                        &context,
                        &raw_outputs,
                        &emitter,
                        &cancelled,
                        &completed_count,
                        &success_count,
                        &skipped_count,
                        total_count,
                        &semaphore,
                        max_concurrency,
                        &visited_def_ids,
                        &reachable_nodes,
                        &stage_boundary_edges,
                        &stage_external_edges,
                    )
                    .await?;

                    // 合并策略

                    let raw_ctx = raw_outputs.lock().await.clone();
                    let merged = WorkflowEngine::merge_stage_outputs(&stage_clone, &raw_ctx);

                    // 门控策略检查

                    // 断点执行只跑 scope 子集，判定要按完整阶段的节点集，并纳入 scope 外节点的
                    // 上一轮状态（本轮状态优先），否则「全部成功」这类策略会因少算节点而误判通过。
                    let gate_stage = gate_stage_full.as_ref().unwrap_or(&stage_clone);
                    let gate_statuses =
                        gate_statuses_with_history(&node_statuses, &gate_history_statuses);
                    match WorkflowEngine::check_gate_strategy(gate_stage, &gate_statuses, &merged) {
                        Ok(GateDecision::Pass) => {
                            log::info!(
                                "[WorkflowEngine] 阶段 '{}' 门控策略检查通过，合并结果已写入",
                                stage_clone.name
                            );

                            // 断点执行本轮只产出 scope 内节点的合并结果，与上一轮的
                            // gate_output.<stage> 合并，避免下游引用上一轮已完成节点时取值缺失。
                            let gate_output =
                                merge_gate_output_baseline(gate_output_baseline.as_ref(), merged);
                            context
                                .lock()
                                .await
                                .insert(format!("gate_output.{}", stage_clone.id), gate_output);
                            emit_progress(
                                &emitter,
                                &exec_id,
                                &definition_id,
                                &mode,
                                None,
                                Some(serde_json::json!({
                                    "id": stage_clone.id.clone(),
                                    "name": stage_clone.name.clone(),
                                    "status": "completed",
                                })),
                                None,
                                None,
                            );
                            Ok(None)
                        }

                        Ok(GateDecision::Blocked(reason)) => {
                            // 正常门控结果（如 All 策略下有节点失败）：warn + 原因原样上报。
                            // 不要在这里再套一层前缀 —— 会造成「检查异常: …… 不满足: ……」这种
                            // 同一句话出现两遍、还把正常拦截说成故障的文案。
                            let msg = format!("{}，工作流中止", reason);
                            log::warn!("[WorkflowEngine] {}", msg);
                            emit_progress(
                                &emitter,
                                &exec_id,
                                &definition_id,
                                &mode,
                                None,
                                Some(serde_json::json!({
                                    "id": stage_clone.id.clone(),
                                    "name": stage_clone.name.clone(),
                                    "status": "gate_failed",
                                    "reason": msg.clone(),
                                })),
                                None,
                                None,
                            );
                            cancelled.store(true, Ordering::SeqCst);
                            // 把精确原因回传给调度方：终端错误与通知都用它，避免另造一句笼统的文案
                            Ok(Some(msg))
                        }

                        // 真异常（配置缺失、条件表达式解析失败等）才走这里，报「检查异常」才算准确
                        Err(e) => {
                            log::error!(
                                "[WorkflowEngine] 阶段 '{}' 门控策略检查失败: {}",
                                stage_clone.name,
                                e
                            );
                            emit_progress(
                                &emitter,
                                &exec_id,
                                &definition_id,
                                &mode,
                                None,
                                Some(serde_json::json!({
                                    "id": stage_clone.id.clone(),
                                    "name": stage_clone.name.clone(),
                                    "status": "gate_failed",
                                    "error": e.to_string(),
                                })),
                                None,
                                None,
                            );
                            Err(AppError::InvalidInput(format!(
                                "阶段 '{}' 门控策略检查异常: {}",
                                stage_clone.name, e
                            )))
                        }
                    }
                }));
            }

            // 等待当前层所有阶段执行完成

            for handle in handles {
                match handle.await {
                    // 阶段任务返回 Some(原因) = 该阶段门控未通过（原因里已含阶段名与判定细节）
                    Ok(Ok(Some(reason))) => {
                        cancelled.store(true, Ordering::SeqCst);
                        // 原因原样带进实例终端错误与通知；
                        // 不再用笼统的「阶段 'X' 门控策略未通过」覆盖掉精确原因
                        abort_reason = Some(reason);
                        break 'layers;
                    }
                    Ok(Ok(None)) => {}

                    Ok(Err(e)) => {
                        cancelled.store(true, Ordering::SeqCst);
                        abort_reason = Some(e.to_string());
                        break 'layers;
                    }

                    Err(join_err) => {
                        cancelled.store(true, Ordering::SeqCst);
                        abort_reason = Some(format!("阶段执行任务异常: {}", join_err));
                        break 'layers;
                    }
                }
            }
        }

        // 更新实例状态为终态，持久化 context 和完成率
        let total = reachable_nodes.len();
        let success = success_count.load(Ordering::SeqCst);
        let skipped = skipped_count.load(Ordering::SeqCst);
        // 「已结算」= 完成 + 跳过：条件分支未走的节点是设计上的结果，不是失败，
        // 也不该拖住完成度（否则带条件的工作流永远到不了 100%，还会被判成 failed）。
        let settled = success + skipped;
        let rate = if total > 0 {
            settled as f64 / total as f64
        } else {
            1.0
        };
        // 判定最终状态优先级：
        // 1. 未被中断且 settled == total → "success"（所有应结算的节点都结算了，即使后续收到取消
        //    信号也视为成功）
        // 2. 用户取消 → "cancelled"
        // 3. 被中断（门控拦住 / 节点失败 / 任务异常）或 settled < total → "failed"
        let final_status = resolve_run_terminal_status(
            settled,
            total,
            abort_reason.as_deref(),
            cancelled.load(Ordering::SeqCst),
        );
        let now = crate::utils::now();

        if let Ok(conn) = get_db_conn(emitter) {
            let final_ctx = context.lock().await.clone();
            // 最终产出：End 节点收集的字段 → End 上游节点产出 → 最后一个有产出的节点。
            // 归一化（解包包装键、识别主正文）与渲染都在前端，后端只判定"取哪个节点的值"。
            let resolved_output = resolve_final_output(def, &final_ctx);
            let final_ctx_json = serde_json::Value::Object(
                final_ctx
                    .into_iter()
                    .filter(|(k, _)| !k.starts_with("__"))
                    .collect(),
            );

            // 事件化：实例进入终态（success/failed/cancelled）。
            let mut event = serde_json::json!({
                "executionId": execution_id,
                "status": final_status,
                "completionRate": rate,
                "skippedCount": skipped,
                "context": &final_ctx_json,
                "completedAt": now,
                "timestamp": now,
            });
            apply_final_output(&mut event, resolved_output);
            // 中断原因落进实例错误：实例详情/「查看结果」能直接说明为什么停在这儿
            if let Some(reason) = &abort_reason {
                event["errorMessage"] = Value::String(reason.chars().take(2000).collect());
            }
            crate::eventlog::append_workflow_event(
                &conn,
                execution_id,
                "execution/status",
                &event,
                true,
            )?;
        }

        emit_progress(
            emitter,
            execution_id,
            &definition_id,
            &mode,
            None,
            None,
            Some(serde_json::json!({
                // 按真实终态上报：原先无论成败都报 "completed"，会让订阅方（编辑器画布）把
                // 失败收尾时仍停在 running 的节点收敛成"成功"，也与实例状态不一致
                "status": final_status,
                "definition_name": &def.name,
                // 用引用而不是移动：下面还要拿 abort_reason 决定返回 Err
                "error": &abort_reason,
            })),
            None,
        );
        let ctx = context.lock().await.clone();

        // 过滤内部变量（__ 前缀），仅返回用户数据作为工作流输出

        let output: serde_json::Map<String, Value> = ctx
            .into_iter()
            .filter(|(k, _)| !k.starts_with("__"))
            .collect();
        let output_value = Value::Object(output);

        // 中断（门控拦住 / 节点失败 / 任务异常）仍要按 Err 上抛：调用方据此决定终态帧与提示。
        // 注意终态事件已在上面写好，所以实例不会再停在 running。
        if let Some(reason) = abort_reason {
            return Err(AppError::External(reason));
        }

        // -- outputSchema 格式化 --

        Ok(Self::format_output(output_value, &def.output_schema))
    }

    // ════════════════════════════════════════════════════════════
    // 断点执行基础设施（单点执行 / 链式执行 / 补全执行）
    // ════════════════════════════════════════════════════════════

    /// 从 `workflow_events`（node/result / node/status）重建前序已完成节点的 context
    ///
    /// 读取每个已完成节点的最新 output（node/result），按定义中的 outputMapping 解析出与实时执行
    /// 一致的暴露值后以 node_id 为 key 注入 HashMap；`{{session_id}}` 取自 node/status 的
    /// agentSessionId。用于断点执行时恢复前序执行上下文。
    ///
    /// `scope` 是本轮要重跑的节点集合：它们的上一轮输出与 `__session_id__` 都不注入（否则本轮
    /// 会读到旧值），scope 外节点的值保持可用。`seed` 是本次执行的种子（实例上下文），
    /// 作为 Start 节点复现预填充语义时的解析基准。
    fn rebuild_context_from_history(
        execution_id: &str,
        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
        def: &WorkflowDefinition,
        scope: &std::collections::HashSet<String>,
        seed: &serde_json::Value,
    ) -> Result<std::collections::HashMap<String, serde_json::Value>, AppError> {
        // 前序已完成节点输出改经事件派生（读切点迁移③）：node/result 每节点保留最新 output。
        let outputs = crate::workflow::events::derive_node_outputs(conn, execution_id)
            .map_err(|e| AppError::Db(e.to_string()))?;
        let agent_sessions =
            crate::workflow::events::derive_node_agent_sessions(conn, execution_id)
                .map_err(|e| AppError::Db(e.to_string()))?;
        let context = rebuild_context_from_records(&outputs, &agent_sessions, def, scope, seed);
        log::info!(
            "[WorkflowEngine] rebuild_context: execution_id={}, recovered {} nodes, scope={}",
            execution_id,
            context.len(),
            scope.len()
        );
        Ok(context)
    }

    /// 计算断点执行的节点范围
    ///
    /// 返回需要执行的节点 ID 集合。
    /// - Full: 所有可达节点
    /// - SingleNode: 仅指定节点
    /// - Chain: 从指定节点 BFS 到所有后序可达节点
    /// - Completion: 所有非 completed 状态的可达节点
    fn compute_execution_scope(
        def: &WorkflowDefinition,
        mode: &ExecutionMode,
        execution_id: &str,
        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    ) -> Result<std::collections::HashSet<String>, AppError> {
        let plan = Self::compute_execution_plan(def);
        let all_reachable: std::collections::HashSet<String> =
            plan.reachable_node_ids.iter().cloned().collect();
        let scope = match mode {
            ExecutionMode::Full => all_reachable,
            ExecutionMode::SingleNode { node_id } => {
                if !all_reachable.contains(node_id) {
                    return Err(AppError::InvalidInput(format!(
                        "节点 {} 不可达，无法执行",
                        node_id
                    )));
                }
                let mut set = std::collections::HashSet::new();
                set.insert(node_id.clone());
                set
            }

            ExecutionMode::Chain { node_id } => {
                if !all_reachable.contains(node_id) {
                    return Err(AppError::InvalidInput(format!(
                        "节点 {} 不可达，无法执行",
                        node_id
                    )));
                }
                // BFS 从指定节点沿节点连线 + 阶段连线遍历所有后序可达节点。
                // 遍历逻辑与 compute_execution_plan 保持一致，支持跨阶段链式传播：
                //   - 沿当前节点所在阶段的 stage.edges 找阶段内下游节点
                //   - 沿该阶段的 stage_edges 找下游阶段，并将其入口节点（阶段内入度为 0 的节点）加入队列
                let stage_map: std::collections::HashMap<String, &Stage> =
                    def.stages.iter().map(|s| (s.id.clone(), s)).collect();
                let se_down = stage_downstream_targets(&def.stages);
                let mut downstream: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                downstream.insert(node_id.clone());
                let mut queue: std::collections::VecDeque<String> =
                    std::collections::VecDeque::new();
                queue.push_back(node_id.clone());
                while let Some(cur) = queue.pop_front() {
                    let cur_stage = def
                        .stages
                        .iter()
                        .find(|s| s.nodes.iter().any(|n| n.id == cur));
                    let Some(stage) = cur_stage else {
                        continue;
                    };
                    // 阶段内节点连线
                    for edge in &stage.edges {
                        if edge.source == cur
                            && all_reachable.contains(&edge.target)
                            && downstream.insert(edge.target.clone())
                        {
                            queue.push_back(edge.target.clone());
                        }
                    }
                    // 阶段连线 → 下游阶段的入口节点
                    if let Some(downstreams) = se_down.get(&stage.id) {
                        for ds_id in downstreams {
                            let Some(ds) = stage_map.get(ds_id) else {
                                continue;
                            };
                            // has_incoming = 阶段内存在入边的节点集合（即 stage.edge 的 target）
                            let has_incoming: std::collections::HashSet<&str> =
                                ds.edges.iter().map(|e| e.target.as_str()).collect();
                            // 下游阶段的入口节点：不在 has_incoming 中（阶段内入度为 0）
                            // （与 compute_execution_plan 的入口判定一致）
                            for node in &ds.nodes {
                                if all_reachable.contains(&node.id)
                                    && !has_incoming.contains(node.id.as_str())
                                    && downstream.insert(node.id.clone())
                                {
                                    queue.push_back(node.id.clone());
                                }
                            }
                        }
                    }
                }
                log::info!(
                    "[WorkflowEngine] Chain scope: from {}, downstream {} nodes",
                    node_id,
                    downstream.len()
                );
                downstream
            }

            ExecutionMode::Completion => {
                // 已完成节点改经事件派生（读切点迁移②）。
                let completed: std::collections::HashSet<String> =
                    crate::workflow::events::derive_node_statuses(conn, execution_id)
                        .map_err(|e| AppError::Db(e.to_string()))?
                        .into_iter()
                        .filter(|(_, status)| status == "completed")
                        .map(|(nid, _)| nid)
                        .collect();

                // 非完成的可达节点
                let pending: std::collections::HashSet<String> =
                    all_reachable.difference(&completed).cloned().collect();
                log::info!(
                    "[WorkflowEngine] Completion scope: reachable={}, completed={}, pending={}",
                    all_reachable.len(),
                    completed.len(),
                    pending.len()
                );
                pending
            }
        };
        Ok(scope)
    }

    /// 重置目标范围内节点的执行状态
    ///
    /// 记录本次重置范围（node/reset 事件）；重跑后节点状态由事件投影回到 pending。
    /// scope 外的已完成节点保持不变。
    fn reset_nodes_for_reexecution(
        execution_id: &str,
        scope: &std::collections::HashSet<String>,
        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    ) -> Result<(), AppError> {
        log::info!(
            "[WorkflowEngine] reset_nodes: execution_id={}, scope={}, affected_rows={}",
            execution_id,
            scope.len(),
            scope.len()
        );

        // 事件化：重跑不抹历史——记录本次重置范围，供审计与回放。
        let mut nodes: Vec<String> = scope.iter().cloned().collect();
        nodes.sort();
        let event = serde_json::json!({
            "executionId": execution_id,
            "nodes": nodes,
            "affectedRows": scope.len(),
        });
        crate::eventlog::append_workflow_event(conn, execution_id, "node/reset", &event, true)?;
        Ok(())
    }

    /// 重置实例状态为 running
    fn reset_instance_for_reexecution(
        execution_id: &str,
        conn: &r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>,
    ) -> Result<(), AppError> {
        // 事件化：记录实例重跑，保留上一轮完成的审计痕迹。
        let event = serde_json::json!({
            "executionId": execution_id,
            "status": "running",
            "timestamp": crate::utils::now(),
        });
        crate::eventlog::append_workflow_event(
            conn,
            execution_id,
            "execution/reset",
            &event,
            true,
        )?;
        log::info!(
            "[WorkflowEngine] reset_instance: execution_id={}",
            execution_id
        );
        Ok(())
    }

    /// 统一断点执行入口
    ///
    /// 支持 Full / SingleNode / Chain / Completion 四种模式。
    ///
    /// 四种模式共用同一条路径：计算执行范围 → 重置范围内节点与实例 → 重建 context → 裁剪定义
    /// → 执行 → 按完整定义重算终态。Full 的范围就是全部可达节点，因此对已有实例的 full 重跑
    /// 同样先重置上一轮状态，不再带着上一轮的节点状态直接执行。
    pub async fn execute_with_mode(
        executor: &Arc<NodeExecutor>,
        def: &WorkflowDefinition,
        execution_id: &str,
        input_data: Value,
        emitter: &tauri::AppHandle,
        max_concurrency: usize,
        mode: ExecutionMode,
    ) -> Result<Value, AppError> {
        let conn = get_db_conn(emitter)?;

        // 完整定义的可达节点：终态重算、进度分母与门控节点集都以它为准。
        let all_reachable: Vec<String> = Self::compute_execution_plan(def).reachable_node_ids;
        let all_total = all_reachable.len();

        // 1. 计算执行范围
        let scope = Self::compute_execution_scope(def, &mode, execution_id, &conn)?;

        // 2. 空范围（如补全执行时已无未完成节点）：直接收尾为 success，不留下 running。
        if scope.is_empty() {
            log::info!("[WorkflowEngine] execute_with_mode: scope 为空，无需执行");
            let now = crate::utils::now();
            // 此路径下所有节点都已结算，产出只存在于历史记录里：全量重建上下文再取。
            let all_scope: std::collections::HashSet<String> =
                all_reachable.iter().cloned().collect();
            let resolved_output = match Self::rebuild_context_from_history(
                execution_id,
                &conn,
                def,
                &all_scope,
                &input_data,
            ) {
                Ok(ctx) => resolve_final_output(def, &ctx),
                Err(_) => (None, "none", None),
            };
            // 事件化：空 scope 断点续跑，实例直接收尾为 success（终态字段齐全）。
            let mut event = serde_json::json!({
                "executionId": execution_id,
                "status": "success",
                "completionRate": 1.0,
                "completedAt": now,
                "timestamp": now,
            });
            apply_final_output(&mut event, resolved_output);
            crate::eventlog::append_workflow_event(
                &conn,
                execution_id,
                "execution/status",
                &event,
                true,
            )?;
            emit_execution_terminal_progress(
                emitter,
                execution_id,
                def,
                &mode,
                "success",
                None,
                Some((all_total, all_total)),
            );
            return Ok(Value::Null);
        }

        // 3. 重置目标范围内节点状态
        Self::reset_nodes_for_reexecution(execution_id, &scope, &conn)?;

        // 4. 重置实例状态
        Self::reset_instance_for_reexecution(execution_id, &conn)?;

        // 5. 重建 context：scope 内节点不注入上一轮输出，scope 外节点保持可用
        let history_context =
            Self::rebuild_context_from_history(execution_id, &conn, def, &scope, &input_data)?;

        // 6. 断点口径的补充信息：进度分母、门控历史状态与 gate_output 基线、边界入边
        let recovery = RecoveryContext {
            total_reachable: all_total,
            history_statuses: crate::workflow::events::derive_node_statuses(&conn, execution_id)
                .map_err(|e| AppError::Db(e.to_string()))?
                .into_iter()
                .filter(|(node_id, _)| !scope.contains(node_id))
                .collect(),
            full_stages: def
                .stages
                .iter()
                .map(|stage| (stage.id.clone(), stage.clone()))
                .collect(),
            gate_output_baseline: collect_gate_output_baseline(&input_data),
            boundary_edges: boundary_condition_edges(&def.stages, &scope),
        };

        // 7. 构建 recovered_def：仅包含目标范围内的节点
        let recovered_stages: Vec<Stage> = def
            .stages
            .iter()
            .map(|stage| {
                let pending_nodes: Vec<WorkflowNode> = stage
                    .nodes
                    .iter()
                    .filter(|n| scope.contains(&n.id))
                    .cloned()
                    .collect();
                let pending_ids: std::collections::HashSet<&str> =
                    pending_nodes.iter().map(|n| n.id.as_str()).collect();
                let filtered_edges: Vec<WorkflowEdge> = stage
                    .edges
                    .iter()
                    .filter(|e| {
                        pending_ids.contains(e.source.as_str())
                            && pending_ids.contains(e.target.as_str())
                    })
                    .cloned()
                    .collect();
                Stage {
                    id: stage.id.clone(),
                    name: stage.name.clone(),
                    order: stage.order,
                    nodes: pending_nodes,
                    edges: filtered_edges,
                    stage_edges: stage.stage_edges.clone(),
                    gate: stage.gate.clone(),
                    collapsed: stage.collapsed,
                    offset_x: stage.offset_x,
                    offset_y: stage.offset_y,
                }
            })
            .filter(|s| !s.nodes.is_empty())
            .collect();
        let recovered_def = WorkflowDefinition {
            id: def.id.clone(),
            name: format!(
                "{} ({})",
                def.name,
                match &mode {
                    ExecutionMode::SingleNode { .. } => "单点执行",
                    ExecutionMode::Chain { .. } => "链式执行",
                    ExecutionMode::Completion => "补全执行",
                    _ => "执行",
                }
            ),
            version: def.version.clone(),
            description: def.description.clone(),
            trigger: def.trigger.clone(),
            stages: recovered_stages,
            input_schema: def.input_schema.clone(),
            output_schema: def.output_schema.clone(),
            icon: def.icon.clone(),
            created_at: def.created_at,
            updated_at: crate::utils::now(),
            enabled: def.enabled,
        };

        // 8. 发送开始事件（mode 与后续帧同形：带 type 的对象）
        emit_progress(
            emitter,
            execution_id,
            &def.id,
            &mode,
            None,
            None,
            Some(serde_json::json!({
                "status": "running",
                "definition_name": &def.name,
            })),
            None,
        );

        // 9. 把重建的历史 context 并入实例上下文作为执行种子：impl 会把种子的顶层键平铺进执行
        // context，而节点引用形如 {{<节点id>.output}}，只在 __input__ 下是命中不了的。
        let mut seed_input = input_data.clone();
        if let Some(obj) = seed_input.as_object_mut() {
            for (k, v) in history_context {
                obj.insert(k, v);
            }
        }

        let impl_result = Self::execute_with_concurrency_impl(
            executor,
            &recovered_def,
            execution_id,
            seed_input,
            emitter,
            max_concurrency,
            &[],
            &mode,
            Some(&recovery),
        )
        .await;

        // 10. 按完整定义重算终态：断点执行已结束（impl 已返回），实例不能停在 running。
        let reachable_set: std::collections::HashSet<&str> =
            all_reachable.iter().map(|s| s.as_str()).collect();
        let mut completed_count: usize = 0;
        let mut skipped_count: usize = 0;
        let mut failed_count: usize = 0;
        let mut cancelled_count: usize = 0;
        {
            // 状态投影改经事件派生（workflow_events 为唯一事实源，读切点迁移①）。
            let statuses = crate::workflow::events::derive_node_statuses(&conn, execution_id)
                .map_err(|e| AppError::Db(e.to_string()))?;
            for (nid, node_status) in &statuses {
                if !reachable_set.contains(nid.as_str()) {
                    continue;
                }
                match node_status.as_str() {
                    "completed" => {
                        completed_count += 1;
                    }
                    // 条件分支未命中的节点同样算「已结算」：不拖完成度，也不判失败
                    "skipped" => {
                        skipped_count += 1;
                    }
                    "failed" => {
                        failed_count += 1;
                    }
                    "cancelled" => {
                        cancelled_count += 1;
                    }
                    _ => {}
                }
            }
        }

        let settled_count = completed_count + skipped_count;
        let rate = if all_total > 0 {
            settled_count as f64 / all_total as f64
        } else {
            1.0
        };
        // 执行已结束，实例必须落终态（见 resolve_terminal_status）。
        let terminal_status = resolve_terminal_status(
            settled_count,
            all_total,
            failed_count,
            cancelled_count,
            impl_result.is_err(),
        );
        let now = crate::utils::now();
        let error_message = impl_result.as_ref().err().map(|e| e.to_string());
        // 产出重算：impl 若在中途失败，就没写过终态事件（也就没落产出），这里补算一次；
        // 已写过时重算结果与之一致，不写字段的分支也不会覆盖（见 apply_final_output）。
        let all_scope: std::collections::HashSet<String> = all_reachable.iter().cloned().collect();
        let resolved_output = match Self::rebuild_context_from_history(
            execution_id,
            &conn,
            def,
            &all_scope,
            &input_data,
        ) {
            Ok(ctx) => resolve_final_output(def, &ctx),
            Err(_) => (None, "none", None),
        };
        // 事件化：断点执行后实例状态重算（终态）。
        let mut event = serde_json::json!({
            "executionId": execution_id,
            "status": terminal_status,
            "completionRate": rate,
            "skippedCount": skipped_count,
            "completedAt": now,
            "timestamp": now,
        });
        apply_final_output(&mut event, resolved_output);
        if let Some(err) = &error_message {
            event["errorMessage"] = Value::String(err.chars().take(2000).collect());
        }
        crate::eventlog::append_workflow_event(
            &conn,
            execution_id,
            "execution/status",
            &event,
            true,
        )?;
        emit_execution_terminal_progress(
            emitter,
            execution_id,
            def,
            &mode,
            terminal_status,
            error_message.as_deref(),
            Some((settled_count, all_total)),
        );
        log::info!(
            "[WorkflowEngine] 断点执行后重算: total={}, completed={}, skipped={}, failed={}, cancelled={}, rate={:.2}, status={}",
            all_total, completed_count, skipped_count, failed_count, cancelled_count, rate, terminal_status
        );
        impl_result
    }
}

/// 将 WorkflowNode 转换为 NodeDef（执行器上下文）

fn node_to_node_def(node: &WorkflowNode) -> NodeDef {
    NodeDef {
        id: node.id.clone(),
        node_type: format!("{:?}", node.node_type).to_lowercase(),
        label: node.label.clone(),
        config: node
            .params
            .clone()
            .unwrap_or(Value::Object(serde_json::Map::new())),
        plugin_id: node.plugin_id.clone(),
        command_id: node.command_id.clone(),
        timeout_ms: node.timeout_ms,
    }
}

/// 调试用：返回 Value 的类型名
fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "number(int)"
            } else {
                "number(float)"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 上下文键名净化：`.` 是模板引擎的路径分隔符（含点号的键无法作为整键被引用），
/// 空键也无法命中，统一替换为 `_`。合并（gate_output）与子工作流写入 context 时共用。
fn sanitize_context_key(key: &str) -> String {
    let sanitized = key.replace('.', "_");
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

/// 是否为空产出（空对象）：门控合并时这类值不参与。
///
/// 开始/结束这类边界节点没有实际工作，执行结果数据就是空对象；未产出内容的节点同理。
/// 把它们合并进去只会得到 `"<节点ID>": {}` 噪音键，让下游误以为"取到了值"。
fn is_empty_contribution(value: &Value) -> bool {
    value.as_object().map_or(false, |obj| obj.is_empty())
}

/// 按 inputSchema 的 `default` 补全缺失入参。
///
/// 顶层执行（手动 / 定时 / 事件触发）没有上游节点提供入参，`default` 是唯一的兜底来源：
/// 缺字段（或值为 null）时写入默认值，已提供的值一律不覆盖。入参为 null 时升格为空对象，
/// 否则连"写入"的位置都没有（前端未传输入的路径传的就是 null）。
fn apply_input_schema_defaults(input: &mut Value, schema: &Option<Value>) {
    let Some(schema_obj) = schema.as_ref().and_then(|s| s.as_object()) else {
        return;
    };
    let defaults: Vec<(String, Value)> = schema_obj
        .iter()
        .filter_map(|(field, rules)| {
            let default = rules.as_object()?.get("default")?;
            if default.is_null() {
                return None;
            }
            Some((field.clone(), default.clone()))
        })
        .collect();
    if defaults.is_empty() {
        return;
    }

    if input.is_null() {
        *input = Value::Object(serde_json::Map::new());
    }
    let Some(obj) = input.as_object_mut() else {
        return;
    };
    for (field, default) in defaults {
        if matches!(obj.get(&field), None | Some(Value::Null)) {
            obj.insert(field, default);
        }
    }
}

/// 子工作流结果键名净化：对象顶层键统一走 [`sanitize_context_key`]。
///
/// 子流程上下文里的 `gate_output.<阶段ID>` 等带点键在模板中无法整键引用，净化后
/// 与主工作流节点的数据形态一致（键名可被 `{{子流程节点ID.<键>}}` 取到）。
fn sanitize_result_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (sanitize_context_key(&key), value))
                .collect(),
        ),
        other => other,
    }
}

/// 记录输入映射未解析的占位符（应用日志 + 节点日志），便于排查“取不到值”的问题。
///
/// 以前解析失败会把 `{{...}}` 原文当值静默传下去，现在按空/`null` 处理后必须可见。
fn log_unresolved_input(
    emitter: &tauri::AppHandle,
    execution_id: &str,
    node_id: &str,
    unresolved: &[String],
) {
    if unresolved.is_empty() {
        return;
    }
    let message = format!("输入映射未解析（该值按空处理）：{}", unresolved.join("、"));
    log::warn!("[WorkflowEngine] 节点 {} {}", node_id, message);
    if let Ok(conn) = get_db_conn(emitter) {
        insert_node_execution_log(&conn, execution_id, node_id, "warn", &message, None);
    }
}

/// 解析节点输入（模板变量替换）
///
/// 返回解析结果与**未解析的占位符列表**（形如 `input1: {{gate_output.stage_x}}`）：
/// - 未含模板的字符串保持字面量（用户输入的常量）；
/// - 含模板但解析不到的占位符按**空**处理（整串是纯占位符时归一为 `Null`），
///   不再把 `{{...}}` 原文当值传给下游。
fn resolve_node_input(
    node: &WorkflowNode,
    context: &HashMap<String, Value>,
) -> (Value, Vec<String>) {
    if let Some(mapping) = &node.input_mapping {
        if let Ok(map) = serde_json::from_value::<HashMap<String, String>>(mapping.clone()) {
            let mut resolved = serde_json::Map::new();
            let mut unresolved: Vec<String> = Vec::new();
            log::info!(
                "[resolve_node_input][DEBUG] 节点 {} (type={:?}) 开始解析 inputMapping: {:?} | context keys: {:?}",
                node.id, node.node_type, map, context.keys().collect::<Vec<_>>()
            );
            for (key, template) in &map {
                // 纯字面量（无占位符）原样保留；含模板的用宽松解析：未解析的占位符按空处理
                let (resolve_result, missing): (Result<String, AppError>, Vec<String>) =
                    if TemplateEngine::has_placeholder(template) {
                        let (text, missing) = TemplateEngine::resolve_lossy(template, context);
                        (Ok(text), missing)
                    } else {
                        (Ok(template.clone()), Vec::new())
                    };
                for expression in &missing {
                    unresolved.push(format!("{}: {{{{{}}}}}", key, expression));
                }

                match resolve_result {
                    Ok(value) => {
                        // 【优化 A】智能推断原始类型，而非一律转字符串
                        // 目的：transform 节点脚本中 input.x * 3 能直接得到数字结果，
                        // 而不会因为 "6" * 3 = NaN 导致用户困惑。
                        // 随后做取值归一化（JSON 文本解析 + 单值解包），输入映射在全节点类型下语义一致。
                        let typed_value =
                            TemplateEngine::normalize_value(&infer_typed_value(&value));
                        log::info!(
                            "[resolve_node_input][DEBUG]   ✅ inputMapping[{}] 解析成功: {:?} -> raw_str=\"{}\" | 类型推断后: {:?} (type={})",
                            key, template,
                            // 必须按字符截断：原先的 &value[..50] 在中文值上会把下标切进字符内部直接 panic
                            crate::utils::text::elide_head(&value, 50),
                            typed_value,
                            value_type_name(&typed_value)
                        );
                        resolved.insert(key.clone(), typed_value);
                    }

                    Err(e) => {
                        log::warn!(

                                "[resolve_node_input] 节点 {} 的 inputMapping[{}] 模板解析失败: {} (模板=\"{}\")",
                                node.id, key, e, template

                            );

                        // 未解析 → 无值（不再把模板原文当值）
                        resolved.insert(key.clone(), Value::Null);
                    }
                }
            }

            return (Value::Object(resolved), unresolved);
        }
    }

    (Value::Object(serde_json::Map::new()), Vec::new())
}

/// 按 outputMapping 计算节点输出需要暴露给后续节点的上下文对象。
///
/// 节点执行后的实时写入与断点执行的历史上下文重建共用本实现，两条路径不允许分叉。
/// 分支依据是 path（下拉选择的取值表达式），而非 key（用户自定义字段名）：
/// - `{{content}}`：取 `node_output.content`，缺失时回退为整个 `node_output`；
/// - `{{session_id}}`：取 `session_id`；缺失时不暴露该字段；
/// - `{{}}`：整个 `node_output`；
/// - `{{content.score}}` / `{{content[0].name}}` / `{{字段名.子字段}}`：按点号与 `[N]`
///   逐层下钻（见 [`resolve_output_path`]），取不到时回退为整个 `node_output`；
/// - 非 `{{}}` 字面量：不暴露（视为用户常量）。
///
/// # Arguments
///
/// * `mapping` - 节点的 outputMapping（字段名 → 取值表达式）
/// * `node_output` - 节点执行的原始输出
/// * `session_id` - 该节点的 agent 会话 id；无会话时为 `None`
///
/// # Returns
///
/// 写入上下文的暴露对象；键集合是 `mapping` 的子集（未命中的字段不写入）。
fn expose_node_output(
    mapping: &serde_json::Map<String, Value>,
    node_output: &Value,
    session_id: Option<&str>,
) -> serde_json::Map<String, Value> {
    let mut exposed = serde_json::Map::new();
    for (key, path) in mapping {
        match path.as_str() {
            Some("{{session_id}}") => {
                if let Some(sid) = session_id {
                    exposed.insert(key.clone(), Value::String(sid.to_string()));
                }
            }
            Some(path_str) => {
                // 引用路径：{{content}}、{{content.score}}、{{content[0].name}}、{{字段名}}
                if path_str.starts_with("{{") && path_str.ends_with("}}") {
                    let lookup = path_str[2..path_str.len() - 2].trim();
                    exposed.insert(key.clone(), resolve_output_path(node_output, lookup));
                }
            }
            None => {}
        }
    }
    exposed
}

/// 按引用路径从节点原始输出中取值，支持点号下钻与 `[N]` 下标。
///
/// - `content`：执行结果本体（输出对象里没有 `content` 字段时即整个输出）
/// - `content.score` / `content[0].name`：在 `content` 基础上继续下钻；上游把 JSON 作为
///   文本输出（如 Agent 返回的结构化结果）时会自动解析，下游无需再加转换节点取值
/// - 其它路径：按同名逐层下钻，取不到时回退整个输出（保持原有的宽松语义）
pub(crate) fn resolve_output_path(node_output: &Value, path: &str) -> Value {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return node_output.clone();
    }

    // `content` 特指执行结果本体：输出对象里没有 content 字段时，去掉该前缀直接下钻
    let mut normalized = trimmed.to_string();
    if node_output.get("content").is_none() {
        if normalized == "content" {
            return node_output.clone();
        }
        if let Some(rest) = normalized.strip_prefix("content.") {
            normalized = rest.to_string();
        } else if let Some(rest) = normalized.strip_prefix("content[") {
            normalized = format!("[{}", rest);
        }
    }

    TemplateEngine::extract_value(node_output, &normalized).unwrap_or_else(|_| node_output.clone())
}

/// 探测可用的字段路径时最多展开的层数（3 层可覆盖到 `key.items[0].name`，避免候选列表过长）
const PROBE_MAX_DEPTH: usize = 3;

/// 探测候选路径的数量上限
const PROBE_MAX_PATHS: usize = 40;

/// 探测节点产出中可供条件/映射选择的字段路径（如 `output.result`、`output[0].name`）。
///
/// 与 [`expose_node_output`] 共用同一套取值语义：先按 outputMapping 暴露的值展开，
/// 无输出映射时用整个产出；数组取首元素。只做结构展开、不做单值解包，
/// 保证给出的路径与求值时真正能取到值的路径一致。
pub(crate) fn probe_output_field_paths(
    node_output: &Value,
    mapping: Option<&serde_json::Map<String, Value>>,
) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    match mapping {
        Some(map) if !map.is_empty() => {
            for (key, path) in map {
                let Some(path_str) = path.as_str() else {
                    continue;
                };
                if !(path_str.starts_with("{{") && path_str.ends_with("}}")) {
                    continue;
                }
                let lookup = path_str[2..path_str.len() - 2].trim();
                let value = resolve_output_path(node_output, lookup);
                // 映射键本身也是可选字段（单值场景直接用它比较）
                paths.push(key.clone());
                collect_probe_paths(&value, key, 1, &mut paths);
            }
        }
        _ => collect_probe_paths(node_output, "", 1, &mut paths),
    }
    paths.sort();
    paths.dedup();
    paths
}

/// 递归收集字段路径（对象字段 / 数组首元素），带深度与数量上限
fn collect_probe_paths(value: &Value, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > PROBE_MAX_DEPTH || out.len() >= PROBE_MAX_PATHS {
        return;
    }
    match TemplateEngine::as_structural(value) {
        Value::Object(map) => {
            for (key, child) in map {
                if out.len() >= PROBE_MAX_PATHS {
                    return;
                }
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                out.push(path.clone());
                collect_probe_paths(&child, &path, depth + 1, out);
            }
        }
        Value::Array(arr) => {
            if let Some(first) = arr.first() {
                let path = if prefix.is_empty() {
                    "[0]".to_string()
                } else {
                    format!("{}[0]", prefix)
                };
                out.push(path.clone());
                collect_probe_paths(first, &path, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// [`WorkflowEngine::rebuild_context_from_history`] 的纯函数部分：由事件投影算出重建 context。
///
/// - scope 内节点（本轮要重跑）不注入上一轮输出与 `__session_id__`，避免本轮读到旧值；
/// - Start 节点复现预填充语义（node/result 记的是定义原文）；
/// - 定义里已不存在的节点（被删过）无法判定类型，保留原始输出。
///
/// # Arguments
///
/// * `outputs` - node/result 投影（node_id → (output, artifactsPath)）
/// * `agent_sessions` - node/status 投影（node_id → agentSessionId）
/// * `def` - 完整定义（判定节点类型与 outputMapping）
/// * `scope` - 本轮要重跑的节点集合
/// * `seed` - 本次执行的种子（实例上下文）
///
/// # Returns
///
/// node_id → 暴露值，另含 scope 外节点的 `__session_id__<nodeId>` 与 `__input__` 占位。
fn rebuild_context_from_records(
    outputs: &std::collections::BTreeMap<String, (Option<String>, Option<String>)>,
    agent_sessions: &std::collections::BTreeMap<String, String>,
    def: &WorkflowDefinition,
    scope: &std::collections::HashSet<String>,
    seed: &Value,
) -> HashMap<String, Value> {
    let nodes_by_id: HashMap<&str, &WorkflowNode> = def
        .stages
        .iter()
        .flat_map(|stage| stage.nodes.iter())
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut context: HashMap<String, Value> = HashMap::new();
    // node_id → node/result 原文（执行结果数据）：门控合并按同一数据来源重算时需要
    let mut raw_outputs: HashMap<String, Value> = HashMap::new();
    for (node_id, (output_data_opt, _artifacts)) in outputs {
        // 本轮要重跑的节点不注入上一轮输出：scope 内节点互相读到的必须是本轮结果。
        if scope.contains(node_id) {
            continue;
        }
        let Some(output_data_str) = output_data_opt else {
            continue;
        };
        let Ok(node_output) = serde_json::from_str::<Value>(output_data_str) else {
            continue;
        };
        // 门控合并重算用的执行结果数据（node/result 原文）
        raw_outputs.insert(node_id.clone(), node_output.clone());
        // 重建取值规则见 rebuild_node_context_value / rebuild_applies_output_mapping。
        let node_def = nodes_by_id.get(node_id.as_str()).copied();
        let value = match node_def {
            // Start 在实时执行里走预填充（node/result 是定义原文），重建需重算。
            Some(node) if node.node_type == WorkflowNodeType::Start => {
                let base = rebuild_resolution_base(&context, seed);
                rebuild_start_node_value(node, &base).unwrap_or(node_output)
            }
            Some(node) => {
                let mapping = rebuild_applies_output_mapping(node)
                    .then_some(node.output_mapping.as_ref())
                    .flatten()
                    .and_then(|mapping| mapping.as_object());
                rebuild_node_context_value(
                    &node.node_type,
                    mapping,
                    &node_output,
                    agent_sessions.get(node_id).map(String::as_str),
                )
            }
            None => node_output,
        };
        context.insert(node_id.clone(), value);
    }

    // 门控合并值（`gate_output.<阶段ID>`）：实时执行在阶段通过门控时写入，断点/单点重跑
    // 若不补建，下游 `{{gate_output.<阶段ID>...}}` 就取不到值。按同一数据来源
    // （节点执行结果数据）与同一合并策略重算。
    for (key, value) in rebuild_gate_outputs(def, scope, &raw_outputs) {
        context.insert(key, value);
    }

    // 会话延续：实时执行把 agent 会话 id 以 __ 前缀写在 context 顶层（供 session_mode=resume 的
    // `{{__session_id__<nodeId>}}` 引用），node/status 留了痕迹，这里还原同一形态，
    // 使断点执行也能延续 scope 外已完成节点的会话。
    for (node_id, session_id) in agent_sessions {
        if scope.contains(node_id) {
            continue;
        }
        context.insert(
            format!("__session_id__{}", node_id),
            Value::String(session_id.clone()),
        );
    }

    // 注入 __input__ 占位（兼容 TemplateEngine 回退逻辑）
    context
        .entry("__input__".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    context
}

/// 断点重建时补建 `gate_output.<阶段ID>`（阶段门控合并值）。
///
/// 实时执行在阶段通过门控时，按该阶段的合并策略把阶段内节点的**执行结果数据**
/// 合并进上下文；断点/单点重跑只重建节点输出、没有重跑门控，若不补建这层键，
/// 下游 `{{gate_output.<阶段ID>...}}` 就会解析不到。
///
/// 口径与实时执行完全一致：同一数据来源（node/result 原文）、同一合并策略
/// （[`WorkflowEngine::merge_stage_outputs`]）。含本轮重跑节点的阶段不补建——
/// 它们会在本轮门控通过时按本轮结果重算；阶段内没有任何执行结果数据（门控没跑过）
/// 也不补建，避免凭空造出一个空合并值。
///
/// # Arguments
///
/// * `def` - 完整工作流定义
/// * `scope` - 本轮要重跑的节点集合
/// * `raw_outputs` - 节点 ID → node/result 原文（已解析）
///
/// # Returns
///
/// `gate_output.<阶段ID>` 键值对；没有可补建的阶段时为空表。
fn rebuild_gate_outputs(
    def: &WorkflowDefinition,
    scope: &std::collections::HashSet<String>,
    raw_outputs: &HashMap<String, Value>,
) -> HashMap<String, Value> {
    let mut rebuilt = HashMap::new();
    for stage in &def.stages {
        if stage.nodes.iter().any(|node| scope.contains(&node.id)) {
            continue;
        }
        if !stage
            .nodes
            .iter()
            .any(|node| raw_outputs.contains_key(&node.id))
        {
            continue;
        }
        let merged = WorkflowEngine::merge_stage_outputs(stage, raw_outputs);
        if merged
            .as_object()
            .map(|obj| obj.is_empty())
            .unwrap_or(false)
        {
            continue;
        }
        rebuilt.insert(format!("gate_output.{}", stage.id), merged);
    }
    rebuilt
}

/// 进度分母：断点执行的 def 只含本轮 scope，用子集数会把进度提前算满，
/// 因此有 [`RecoveryContext`] 时用完整定义的可达节点数（与结束时重算 completionRate 同口径）。
///
/// # Arguments
///
/// * `recovery` - 断点执行补充信息；全量执行为 `None`
/// * `scope_reachable` - 本轮定义的可达节点数
///
/// # Returns
///
/// 写入进度事件的 `total`。
fn progress_total(recovery: Option<&RecoveryContext>, scope_reachable: usize) -> usize {
    recovery
        .map(|r| r.total_reachable)
        .unwrap_or(scope_reachable)
}

/// 门控判定的状态集合：本轮状态优先，scope 外节点的上一轮状态补位。
///
/// 断点执行只统计 scope 内节点，缺了 scope 外节点会让「全部成功」这类策略少算节点。
///
/// # Arguments
///
/// * `current` - 本轮 execute_stage 返回的状态（node_id → status）
/// * `history` - scope 外节点的上一轮状态（node_id → status）
///
/// # Returns
///
/// 合并后的状态集合。
fn gate_statuses_with_history(
    current: &HashMap<String, String>,
    history: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut statuses = current.clone();
    for (node_id, status) in history {
        statuses
            .entry(node_id.clone())
            .or_insert_with(|| status.clone());
    }
    statuses
}

/// 执行结束后的实例终态判定。
///
/// 执行已结束，实例必须落终态（写 `running` 会让实例永久卡住）。
/// 「已结算」= 完成 + **跳过**：条件分支未走的节点是设计上的结果，不是失败，
/// 也不该拖住完成度 —— 否则带条件的工作流永远到不了 100%，还会被判成 failed。
/// 1. 有 failed 节点、或 impl 返回错误（如门控未通过）→ `failed`；
/// 2. 全部可达节点都已结算 → `success`（有取消信号但无节点被取消时也算成功）；
/// 3. 存在 cancelled 节点 → `cancelled`；
/// 4. 其余（无失败但仍有可达节点未结算，如链式只跑了后半段）→ `failed`，
///    与执行实现「settled < total 即 failed」的判定一致。
///
/// # Arguments
///
/// * `settled` - 已结算节点数（完成 + 跳过）
/// * `total` - 完整定义范围内的可达节点数
/// * `failed` - 失败节点数
/// * `cancelled` - 取消节点数
/// * `impl_failed` - 执行实现是否返回错误
///
/// # Returns
///
/// 实例终态字面量。
fn resolve_terminal_status(
    settled: usize,
    total: usize,
    failed: usize,
    cancelled: usize,
    impl_failed: bool,
) -> &'static str {
    // 失败判定先于"全部结算"：门控未通过时所有节点可能都已结算，但执行是终止的，
    // 不能因为分子凑满就报成功。
    if failed > 0 || impl_failed {
        "failed"
    } else if settled == total {
        "success"
    } else if cancelled > 0 {
        "cancelled"
    } else {
        "failed"
    }
}

/// 执行实现（`execute_with_concurrency_impl`）收尾时的终态判定。
///
/// 与 [`resolve_terminal_status`]（断点路径按节点状态计数判定）互补：这里只有"已结算数"和
/// "是否被中断"两个信息。
///
/// 关键点：**内部叫停标志不等于用户取消**。门控拦住时会 `cancelled.store(true)` 用来叫停同层
/// 阶段；若照它把"因失败而终止"记成 `cancelled`，实例就变成"用户取消"，语义完全不对。
///
/// # Arguments
///
/// * `settled` - 已结算节点数（完成 + 跳过）
/// * `total` - 本次执行范围的可达节点数
/// * `abort_reason` - 中断原因（门控未通过 / 节点任务失败 / 用户取消…）；未中断为 `None`
/// * `cancel_flag` - 进程内的取消标志（可能只是内部叫停，见上）
///
/// # Returns
///
/// 实例终态字面量。
fn resolve_run_terminal_status(
    settled: usize,
    total: usize,
    abort_reason: Option<&str>,
    cancel_flag: bool,
) -> &'static str {
    // 用户取消的统一原因是这句（见各处 `工作流已被取消`）
    let user_cancelled = abort_reason
        .map(|r| r.contains("已被取消"))
        .unwrap_or(false);
    if abort_reason.is_none() && settled == total {
        "success"
    } else if user_cancelled || (abort_reason.is_none() && cancel_flag) {
        "cancelled"
    } else {
        "failed"
    }
}

/// 边条件支持的运算符，与前端条件编辑器保持一致（`is_*` 为无值运算符）。
const CONDITION_OPERATORS: [&str; 11] = [
    "==",
    "!=",
    ">=",
    "<=",
    ">",
    "<",
    "contains",
    "starts_with",
    "ends_with",
    "is_empty",
    "is_not_empty",
];

/// 按字段/路径从上游节点输出中取值。
///
/// 支持点号路径与下标（`output.status`、`output.items[0].name`），值本身是 JSON 文本时自动解析；
/// 取不到时回退原有宽松语义：对象取同名字段、数组取首元素同名字段、标量返回自身。
fn resolve_condition_field(output: &Value, field: &str) -> Value {
    if let Ok(value) = TemplateEngine::extract_value(output, field) {
        return value;
    }
    match output {
        Value::Object(map) => map.get(field).cloned().unwrap_or(Value::Null),
        Value::Array(arr) => arr
            .first()
            .and_then(|v| v.as_object())
            .and_then(|m| m.get(field).cloned())
            .unwrap_or(Value::Null),
        other => other.clone(),
    }
}

/// 条件比较用的文本形式：字符串去掉引号，Null 视为空串。
fn condition_value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 判空：Null / 空串 / 空数组 / 空对象视为空。
fn is_condition_value_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(arr) => arr.is_empty(),
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

/// 按阶段分组收集边界入边：`target` 在本次执行范围内、`source` 在范围外的边。
///
/// 断点执行时阶段定义被裁剪成执行范围子集，这些边会随之消失；入边条件判定仍需要它们，
/// 否则上一轮因条件被跳过的节点会被重跑（有真实副作用）。返回值只用于条件判定，
/// 不改变本轮的拓扑分层与可执行节点集合。
///
/// # Arguments
///
/// * `stages` - 阶段的完整列表
/// * `scope` - 本次执行的节点 id 集合
///
/// # Returns
///
/// 阶段 id → 该阶段的边界入边；没有边界入边的阶段不出现。
fn boundary_condition_edges(
    stages: &[Stage],
    scope: &std::collections::HashSet<String>,
) -> HashMap<String, Vec<WorkflowEdge>> {
    let mut map: HashMap<String, Vec<WorkflowEdge>> = HashMap::new();
    for stage in stages {
        let edges: Vec<WorkflowEdge> = stage
            .edges
            .iter()
            .filter(|e| scope.contains(&e.target) && !scope.contains(&e.source))
            .cloned()
            .collect();
        if !edges.is_empty() {
            map.insert(stage.id.clone(), edges);
        }
    }
    map
}

/// 从实例上下文收集上一轮各阶段的 `gate_output.<stage>`（stage id → 值）。
///
/// 断点执行本轮只产出 scope 内节点的合并结果，需要以它作为基线，
/// 避免下游引用上一轮已完成节点时取值缺失。
///
/// # Arguments
///
/// * `context` - 实例上下文（上一轮收尾时持久化的 context）
///
/// # Returns
///
/// stage id → 上一轮的 `gate_output` 值；上下文中没有这类键时为空表。
fn collect_gate_output_baseline(context: &Value) -> HashMap<String, Value> {
    let Some(obj) = context.as_object() else {
        return HashMap::new();
    };
    obj.iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("gate_output.")
                .map(|stage_id| (stage_id.to_string(), value.clone()))
        })
        .collect()
}

/// 把上一轮的 `gate_output.<stage>` 作为基线合并进本轮结果。
///
/// 断点执行只跑 scope 子集，本轮 `merge_stage_outputs` 只含本轮节点的输出；直接覆盖会让
/// 下游引用上一轮已完成节点的 `gate_output` 字段解析失败。仅当两侧都是对象（Merge 策略）时
/// 按「本轮覆盖基线」合并；其他合并策略（Concat / PickFirst / PickLast / Custom）的取值不按
/// 节点键组织，整体替换以保持原有语义。
///
/// # Arguments
///
/// * `baseline` - 上一轮的 `gate_output.<stage>` 值；无历史时为 `None`
/// * `merged` - 本轮阶段合并结果
///
/// # Returns
///
/// 写入 `gate_output.<stage>` 的最终值。
fn merge_gate_output_baseline(baseline: Option<&Value>, merged: Value) -> Value {
    let Value::Object(current) = &merged else {
        return merged;
    };
    let Some(Value::Object(previous)) = baseline else {
        return merged;
    };
    let mut out: serde_json::Map<String, Value> = previous
        .iter()
        // 基线里的空产出键也不保留：上一轮遗留的 `"<节点ID>": {}` 噪音不能靠基线复活
        .filter(|(_, value)| !is_empty_contribution(value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for (key, value) in current {
        out.insert(key.clone(), value.clone());
    }
    Value::Object(out)
}

/// 阶段连线邻接表（上游阶段 id → 下游阶段 id 列表）。
///
/// `stage_edges` 归**来源阶段**所有，因此只采纳 `source` 等于该边所在阶段 id 的连线：
/// 其它阶段（或改阶段后残留）里同名 `source` 的边不能把无关阶段纳入链上范围。
///
/// # Arguments
///
/// * `stages` - 阶段的完整列表
///
/// # Returns
///
/// 上游阶段 id → 下游阶段 id 列表（可能含重复项，调用方按集合去重）。
fn stage_downstream_targets(stages: &[Stage]) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for stage in stages {
        for edge in &stage.stage_edges {
            if edge.source != stage.id {
                continue;
            }
            map.entry(edge.source.clone())
                .or_default()
                .push(edge.target.clone());
        }
    }
    map
}

/// 节点产出是否"有内容"（空对象 / 空数组 / 空串 / Null / 全空字段都算无产出）
///
/// 容器按"任一子项有内容"判定：End 节点产出是 inputMapping 组成的对象，
/// `{"result": ""}` 这种"壳非空、值全空"的情形必须算无产出——否则会把它当成最终结果，
/// 把真正的内容挡在展示之外（同时也让 End → 上游的兜底取不到东西）。
fn output_is_meaningful(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => a.iter().any(output_is_meaningful),
        Value::Object(o) => o.values().any(output_is_meaningful),
        _ => true,
    }
}

/// 解析本次执行的「最终产出」（产出卡片的取值来源）。
///
/// 顺序：
/// 1. **End 节点收集的字段**（其 `input_mapping` 解析结果）——定义期声明的产出；
/// 2. **End 的上游节点产出**——"会话/群聊转工作流"自动生成的图里 End 没有 input_mapping，
///    产出恒为空对象；此时按连线取收口节点的产出。用连线而非"最后完成"是为了稳定：
///    并行分支下"最后完成"是竞态结果，同一工作流两次执行的产出可能不同；
/// 3. 兜底：定义顺序里最后一个有产出的节点。
///
/// 只判定"取哪个节点的值"，归一化（解包包装键、识别主正文）与渲染交给前端，
/// 这样调整展示规则不需要重编译。
///
/// `pub(crate)`：读侧（`events::list_instances`）要给**本功能上线前**跑的旧实例补算产出，
/// 用同一口径才不会新旧两套规则。
pub(crate) fn resolve_final_output(
    def: &WorkflowDefinition,
    ctx: &HashMap<String, Value>,
) -> (Option<Value>, &'static str, Option<String>) {
    let nodes: Vec<&WorkflowNode> = def.stages.iter().flat_map(|s| s.nodes.iter()).collect();
    let end_nodes: Vec<&WorkflowNode> = nodes
        .iter()
        .copied()
        .filter(|n| n.node_type == WorkflowNodeType::End)
        .collect();

    // 1) End 节点
    for end in &end_nodes {
        if let Some(v) = ctx.get(&end.id) {
            if output_is_meaningful(v) {
                return (Some(v.clone()), "end", Some(end.label.clone()));
            }
        }
    }

    // 2) End 的上游节点（按连线，取定义顺序里最后一个有产出的）
    for end in &end_nodes {
        let upstream: Vec<&str> = def
            .stages
            .iter()
            .flat_map(|s| s.edges.iter())
            .filter(|e| e.target == end.id)
            .map(|e| e.source.as_str())
            .collect();
        let mut hit: Option<(&WorkflowNode, Value)> = None;
        for node in &nodes {
            if !upstream.contains(&node.id.as_str()) {
                continue;
            }
            if let Some(v) = ctx.get(&node.id) {
                if output_is_meaningful(v) {
                    hit = Some((node, v.clone()));
                }
            }
        }
        if let Some((node, v)) = hit {
            return (Some(v), "end-upstream", Some(node.label.clone()));
        }
    }

    // 3) 兜底：最后一个有产出的节点（跳过 Start，它只携带初始入参）
    let mut last: Option<(&WorkflowNode, Value)> = None;
    for node in &nodes {
        if node.node_type == WorkflowNodeType::Start {
            continue;
        }
        if let Some(v) = ctx.get(&node.id) {
            if output_is_meaningful(v) {
                last = Some((node, v.clone()));
            }
        }
    }
    match last {
        Some((node, v)) => (Some(v), "last-node", Some(node.label.clone())),
        None => (None, "none", None),
    }
}

/// 把 [`resolve_final_output`] 的结果写进终态事件。
///
/// 本次无产出时**一个字段都不写**：断点执行会先后写多条 `execution/status`，
/// 后写的事件若带 `output: null` 会把前面算好的产出覆盖掉。
fn apply_final_output(event: &mut Value, resolved: (Option<Value>, &'static str, Option<String>)) {
    let (output, source, label) = resolved;
    let Some(output) = output else { return };
    event["output"] = output;
    event["outputSource"] = Value::String(source.to_string());
    if let Some(label) = label {
        event["outputNodeLabel"] = Value::String(label);
    }
}

/// 重建上下文时，是否按 outputMapping 解析该节点 node/result 里的输出。
///
/// Start 与 Subflow 在实时执行中都不走 [`expose_node_output`]：
/// - Start：outputMapping 在预填充阶段解析后写入 context，node/result 记的是定义原文；
/// - Subflow：outputMapping 在执行时解析，解析结果同时写入 context 与 node/result。
///
/// 因此这两类节点在断点重建时原样使用 node/result（对 Subflow 再套一次映射会二次解析）。
///
/// # Arguments
///
/// * `node` - 定义中的节点
///
/// # Returns
///
/// `true` 表示需要按 outputMapping 解析 node/result。
fn rebuild_applies_output_mapping(node: &WorkflowNode) -> bool {
    !matches!(
        node.node_type,
        WorkflowNodeType::Start | WorkflowNodeType::Subflow
    )
}

/// 断点执行重建上下文时，单个已完成节点写入 context 的取值。
///
/// - 有 outputMapping（且 [`rebuild_applies_output_mapping`] 为真）：按 [`expose_node_output`]
///   解析，与实时执行写入的值一致；
/// - 无 outputMapping：与实时执行的「无 outputMapping」分支对齐 —— Agent 节点裸输出暴露为
///   `output`，Interact 节点把 `{content: …}` 的 content 暴露为 `output`，其余类型写入空对象；
///   End 在实时执行里写的是输入映射的解析结果，无法从 node/result 还原，保留原始输出。
///   两侧口径一致，断点执行与全量执行的模板可解析性才相同。
///
/// # Arguments
///
/// * `node_type` - 节点类型（决定无 outputMapping 时的暴露口径）
/// * `mapping` - 节点 outputMapping 的对象形式；无 outputMapping、非对象或该节点不按映射解析时为 `None`
/// * `node_output` - node/result 记录的原始输出
/// * `session_id` - 该节点 agent 会话 id（取自 node/status 的 agentSessionId）
///
/// # Returns
///
/// 写入重建上下文的取值。
fn rebuild_node_context_value(
    node_type: &WorkflowNodeType,
    mapping: Option<&serde_json::Map<String, Value>>,
    node_output: &Value,
    session_id: Option<&str>,
) -> Value {
    if let Some(mapping) = mapping {
        return Value::Object(expose_node_output(mapping, node_output, session_id));
    }

    match node_type {
        WorkflowNodeType::Agent => serde_json::json!({ "output": node_output }),
        WorkflowNodeType::Interact => {
            serde_json::json!({ "output": node_output.get("content").unwrap_or(node_output) })
        }
        WorkflowNodeType::End => node_output.clone(),
        _ => Value::Object(serde_json::Map::new()),
    }
}

/// Start 节点在断点重建时写入 context 的值：复现实时执行的预填充语义。
///
/// 实时执行在预填充阶段逐字段 [`TemplateEngine::resolve`] Start 的 outputMapping 得到暴露对象，
/// node/result 记的是定义原文；断点重建必须重算，否则 `{{<startId>.<field>}}` 取到的是
/// `{{topic}}` 这样的原文，而不是解析后的值。
///
/// # Arguments
///
/// * `node` - Start 节点定义
/// * `context` - 解析基准，与实时预填充同形（`__input__` + 种子顶层键 + 已重建的历史输出）
///
/// # Returns
///
/// `Some(object)` 为解析后的暴露对象；节点没有 outputMapping 对象时为 `None`，
/// 由调用方退回 node/result 原文。
fn rebuild_start_node_value(
    node: &WorkflowNode,
    context: &HashMap<String, Value>,
) -> Option<Value> {
    let mapping = node.output_mapping.as_ref()?.as_object()?;
    let mut output_obj = serde_json::Map::new();
    for (key, value) in mapping {
        // 与实时预填充一致：未解析的占位符按空处理（不回退成 `{{...}}` 原文）；
        // 未含占位符的字面量原样保留。
        let resolved = match value.as_str() {
            Some(template) if TemplateEngine::has_placeholder(template) => {
                let (text, unresolved) = TemplateEngine::resolve_lossy(template, context);
                if !unresolved.is_empty() {
                    log::warn!(
                        "[WorkflowEngine] 断点重建：开始节点输出映射[{}] 未解析（该值按空处理）：{:?}",
                        key, unresolved
                    );
                }
                Value::String(text)
            }
            Some(template) => Value::String(template.to_string()),
            None => value.clone(),
        };
        output_obj.insert(key.clone(), resolved);
    }
    Some(Value::Object(output_obj))
}

/// 重建上下文时 Start 节点 outputMapping 的解析基准。
///
/// 与执行实现的种子形态一致：`__input__` 取「实例上下文 + 已重建的历史输出」，
/// 同时把这些顶层键平铺进基准（`{{topic}}` 这类无点号引用走精确匹配）。
fn rebuild_resolution_base(
    context: &HashMap<String, Value>,
    seed: &Value,
) -> HashMap<String, Value> {
    let mut input_obj = seed.as_object().cloned().unwrap_or_default();
    for (key, value) in context {
        if key.starts_with("__") {
            continue;
        }
        input_obj.insert(key.clone(), value.clone());
    }

    let input_value = Value::Object(input_obj);
    let mut base: HashMap<String, Value> = HashMap::new();
    base.insert("__input__".to_string(), input_value.clone());
    if let Some(obj) = input_value.as_object() {
        for (key, value) in obj {
            base.insert(key.clone(), value.clone());
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 outputMapping 对象形式（测试输入）
    fn mapping(value: Value) -> serde_json::Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    /// inputSchema 的 default 是顶层执行唯一的入参来源：缺失/为 null 时补默认值，
    /// 已提供的值不覆盖；入参为 null 时也能升格成对象再写入。
    #[test]
    fn input_schema_defaults_fill_missing_fields() {
        let schema = Some(serde_json::json!({
            "topic": { "type": "string", "required": true, "default": "人工智能" },
            "count": { "type": "integer", "default": 3 },
            "note":  { "type": "string" },
        }));

        let mut input = Value::Null;
        apply_input_schema_defaults(&mut input, &schema);
        assert_eq!(input.get("topic"), Some(&serde_json::json!("人工智能")));
        assert_eq!(input.get("count"), Some(&serde_json::json!(3)));
        assert!(
            input.get("note").is_none(),
            "没有 default 的字段不应被凭空造出来"
        );

        // 显式传值不被默认值覆盖
        let mut provided = serde_json::json!({ "topic": "量子计算", "count": null });
        apply_input_schema_defaults(&mut provided, &schema);
        assert_eq!(provided.get("topic"), Some(&serde_json::json!("量子计算")));
        assert_eq!(
            provided.get("count"),
            Some(&serde_json::json!(3)),
            "null 视为缺失"
        );
    }

    #[test]
    fn content_mapping_extracts_content_field() {
        let output = serde_json::json!({"content": "hello"});
        let exposed = expose_node_output(
            &mapping(serde_json::json!({"text": "{{content}}"})),
            &output,
            None,
        );
        assert_eq!(exposed.get("text"), Some(&serde_json::json!("hello")));
    }

    #[test]
    fn content_mapping_on_plugin_output_falls_back_to_whole_output() {
        let output = serde_json::json!({"result": 1});
        let exposed = expose_node_output(
            &mapping(serde_json::json!({"output": "{{content}}"})),
            &output,
            None,
        );
        assert_eq!(
            exposed,
            mapping(serde_json::json!({"output": {"result": 1}}))
        );
    }

    #[test]
    fn session_id_mapping_exposes_agent_session() {
        let output = serde_json::json!({"content": "hi"});
        let m = mapping(serde_json::json!({"sid": "{{session_id}}"}));
        let with_session = expose_node_output(&m, &output, Some("agent-session-1"));
        let sid = serde_json::json!("agent-session-1");
        assert_eq!(with_session.get("sid"), Some(&sid));

        // 无会话时不暴露该字段
        assert!(expose_node_output(&m, &output, None).is_empty());
    }

    #[test]
    fn generic_path_mapping_extracts_field_and_falls_back_when_missing() {
        let output = serde_json::json!({"result": 0.5});
        let found = expose_node_output(
            &mapping(serde_json::json!({"value": "{{result}}"})),
            &output,
            None,
        );
        let expected = serde_json::json!(0.5);
        assert_eq!(found.get("value"), Some(&expected));
        let missing = expose_node_output(
            &mapping(serde_json::json!({"value": "{{missing}}"})),
            &output,
            None,
        );
        assert_eq!(missing.get("value"), Some(&output));
    }

    #[test]
    fn empty_braces_mapping_exposes_whole_output_and_literals_are_skipped() {
        let output = serde_json::json!({"result": 1});
        let whole = expose_node_output(&mapping(serde_json::json!({"all": "{{}}"})), &output, None);
        assert_eq!(whole.get("all"), Some(&output));

        // 非 {{}} 字面量视为用户常量，不进入上下文
        let literal = expose_node_output(
            &mapping(serde_json::json!({"const": "literal"})),
            &output,
            None,
        );
        assert!(literal.is_empty());
    }

    #[test]
    fn rebuild_value_matches_live_exposure_without_mapping() {
        let output = serde_json::json!({"result": 1});

        // 无 outputMapping 的其他类型：实时执行写入空对象（不暴露任何字段），重建同口径
        assert_eq!(
            rebuild_node_context_value(&WorkflowNodeType::Plugin, None, &output, None),
            serde_json::json!({})
        );

        // End 实时写的是输入映射解析结果，无法从 node/result 还原，保留原始输出
        assert_eq!(
            rebuild_node_context_value(&WorkflowNodeType::End, None, &output, None),
            output
        );
        let m = mapping(serde_json::json!({"output": "{{content}}"}));
        assert_eq!(
            rebuild_node_context_value(&WorkflowNodeType::Plugin, Some(&m), &output, None),
            serde_json::json!({"output": {"result": 1}})
        );
    }

    #[test]
    fn rebuild_value_mirrors_live_exposure_for_agent_and_interact() {
        // 实时执行里：Agent 节点无 outputMapping 时把裸输出暴露为 output，
        // Interact 节点把 {content: …} 的 content 暴露为 output。
        assert_eq!(
            rebuild_node_context_value(
                &WorkflowNodeType::Agent,
                None,
                &serde_json::json!("答案"),
                None
            ),
            serde_json::json!({"output": "答案"})
        );
        assert_eq!(
            rebuild_node_context_value(
                &WorkflowNodeType::Interact,
                None,
                &serde_json::json!({"content": "用户输入"}),
                None
            ),
            serde_json::json!({"output": "用户输入"})
        );
        // 没有 content 包装时退回整个输出
        assert_eq!(
            rebuild_node_context_value(
                &WorkflowNodeType::Interact,
                None,
                &serde_json::json!("裸值"),
                None
            ),
            serde_json::json!({"output": "裸值"})
        );
    }

    #[test]
    fn only_mapping_exposing_node_types_are_rebuilt_through_output_mapping() {
        let node = |node_type: &str| -> WorkflowNode {
            serde_json::from_value(serde_json::json!({"id": "n1", "type": node_type, "label": "N"}))
                .unwrap()
        };
        assert!(rebuild_applies_output_mapping(&node("agent")));
        assert!(rebuild_applies_output_mapping(&node("plugin")));
        // Start 与 Subflow 的 node/result 不是「待套用映射的原始输出」
        assert!(!rebuild_applies_output_mapping(&node("start")));
        assert!(!rebuild_applies_output_mapping(&node("subflow")));
    }

    // ── 断点执行（单点/链式/补全）──

    #[test]
    fn merge_keys_are_stable_node_ids_with_sanitized_names() {
        // 合并结果按「节点ID → 该节点执行结果数据」整值成键：
        // 键名不随值形态变化，且带点键被净化（点号在模板里是路径分隔符）
        let s = stage(
            "s1",
            vec![
                node("n1", "plugin"),
                node("n2", "agent"),
                node("n.4", "plugin"),
            ],
        );
        let mut raw: HashMap<String, Value> = HashMap::new();
        raw.insert("n1".to_string(), serde_json::json!({ "result": 1 }));
        raw.insert("n2".to_string(), serde_json::json!("裸字符串"));
        raw.insert("n.4".to_string(), serde_json::json!({ "a.b": 2 }));
        assert_eq!(
            WorkflowEngine::merge_stage_outputs(&s, &raw),
            serde_json::json!({
                "n1": { "result": 1 },
                "n2": "裸字符串",
                "n_4": { "a.b": 2 },
            })
        );
    }

    #[test]
    fn merge_skips_nodes_without_output() {
        // 无实质产出（空对象）的节点不参与合并：开始/结束这类不做实际工作的边界节点
        // 不会在合并结果里留下 `"<节点ID>": {}` 噪音键
        let s = stage(
            "s1",
            vec![
                node("n_start", "start"),
                node("n1", "plugin"),
                node("n_end", "end"),
            ],
        );
        let mut raw: HashMap<String, Value> = HashMap::new();
        raw.insert("n_start".to_string(), serde_json::json!({}));
        raw.insert("n1".to_string(), serde_json::json!({ "result": 1 }));
        raw.insert("n_end".to_string(), serde_json::json!({}));
        assert_eq!(
            WorkflowEngine::merge_stage_outputs(&s, &raw),
            serde_json::json!({ "n1": { "result": 1 } })
        );
    }

    #[test]
    fn subflow_result_keys_are_sanitized() {
        // 子流程上下文里的带点键（如内部 gate_output.<阶段ID>）净化成可整键引用的形态
        assert_eq!(
            sanitize_result_keys(serde_json::json!({
                "gate_output.stage_inner": { "n_inner": { "result": 1 } },
                "": 2,
                "n1": 3,
            })),
            serde_json::json!({
                "gate_output_stage_inner": { "n_inner": { "result": 1 } },
                "_": 2,
                "n1": 3,
            })
        );
    }

    #[test]
    fn rebuild_gate_outputs_recomputes_only_stages_outside_scope() {
        // 前序阶段（不在本轮 scope 内）按同一合并口径重算 gate_output；含本轮节点的阶段
        // 交由本轮门控重算，不在这里补建。
        let s1 = stage("s1", vec![node("n1", "plugin")]);
        let s2 = stage("s2", vec![node("n2", "plugin")]);
        let def = definition(vec![s1, s2]);
        let mut raw: HashMap<String, Value> = HashMap::new();
        raw.insert("n1".to_string(), serde_json::json!({ "result": 1 }));
        raw.insert("n2".to_string(), serde_json::json!({ "result": 2 }));
        let rebuilt = rebuild_gate_outputs(&def, &scope(&["n2"]), &raw);
        assert_eq!(
            rebuilt.get("gate_output.s1"),
            Some(&serde_json::json!({ "n1": { "result": 1 } }))
        );
        assert!(!rebuilt.contains_key("gate_output.s2"));

        // 阶段内没有任何执行结果数据（门控没跑过）不补建
        assert!(rebuild_gate_outputs(&def, &scope(&[]), &HashMap::new()).is_empty());
    }

    /// 构造节点（仅测试关心的字段）
    fn node(id: &str, node_type: &str) -> WorkflowNode {
        serde_json::from_value(serde_json::json!({"id": id, "type": node_type, "label": id}))
            .unwrap()
    }

    /// 构造带 outputMapping 的节点
    fn node_with_mapping(id: &str, node_type: &str, output_mapping: Value) -> WorkflowNode {
        serde_json::from_value(serde_json::json!({
            "id": id, "type": node_type, "label": id, "outputMapping": output_mapping,
        }))
        .unwrap()
    }

    /// 构造阶段（仅测试关心的字段，门控取默认 All + Merge）
    fn stage(id: &str, nodes: Vec<WorkflowNode>) -> Stage {
        Stage {
            id: id.to_string(),
            name: id.to_string(),
            order: 0,
            nodes,
            edges: Vec::new(),
            stage_edges: Vec::new(),
            gate: crate::workflow::GateConfig::default(),
            collapsed: false,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }

    /// 构造边
    fn edge(id: &str, source: &str, target: &str, condition: Option<&str>) -> WorkflowEdge {
        WorkflowEdge {
            id: id.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            label: None,
            condition: condition.map(str::to_string),
        }
    }

    /// 构造定义（仅测试关心的字段）
    fn definition(stages: Vec<Stage>) -> WorkflowDefinition {
        WorkflowDefinition {
            id: "wf1".to_string(),
            name: "wf".to_string(),
            version: "1".to_string(),
            description: String::new(),
            trigger: crate::workflow::TriggerConfig {
                trigger_type: crate::workflow::TriggerType::Manual,
                cron: None,
                event_name: None,
            },
            stages,
            icon: None,
            input_schema: None,
            output_schema: None,
            created_at: 0,
            updated_at: 0,
            enabled: true,
        }
    }

    fn scope(ids: &[&str]) -> std::collections::HashSet<String> {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    fn outputs(
        entries: &[(&str, Value)],
    ) -> std::collections::BTreeMap<String, (Option<String>, Option<String>)> {
        entries
            .iter()
            .map(|(id, value)| ((*id).to_string(), (Some(value.to_string()), None)))
            .collect()
    }

    fn sessions(entries: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        entries
            .iter()
            .map(|(id, session)| ((*id).to_string(), (*session).to_string()))
            .collect()
    }

    #[test]
    fn execution_mode_serializes_as_tagged_object_with_camel_case_node_id() {
        assert_eq!(
            serde_json::to_value(ExecutionMode::Full).unwrap(),
            serde_json::json!({"type": "full"})
        );
        assert_eq!(
            serde_json::to_value(ExecutionMode::SingleNode {
                node_id: "n1".to_string()
            })
            .unwrap(),
            serde_json::json!({"type": "single_node", "nodeId": "n1"})
        );
        assert_eq!(
            serde_json::to_value(ExecutionMode::Chain {
                node_id: "n2".to_string()
            })
            .unwrap(),
            serde_json::json!({"type": "chain", "nodeId": "n2"})
        );
        assert_eq!(
            serde_json::to_value(ExecutionMode::Completion).unwrap(),
            serde_json::json!({"type": "completion"})
        );
    }

    #[test]
    fn rebuild_start_node_resolves_mapping_against_rebuilt_seed() {
        let start = node_with_mapping(
            "n_start",
            "start",
            serde_json::json!({"topic": "{{topic}}", "count": 3}),
        );
        let context =
            rebuild_resolution_base(&HashMap::new(), &serde_json::json!({"topic": "真实主题"}));

        // 模板字段按种子解析，非字符串字段原样保留（与实时预填充一致）
        assert_eq!(
            rebuild_start_node_value(&start, &context).unwrap(),
            serde_json::json!({"topic": "真实主题", "count": 3})
        );

        // 无 outputMapping：调用方保持使用 node/result 原文
        assert!(rebuild_start_node_value(&node("n_start", "start"), &context).is_none());
    }

    #[test]
    fn rebuild_start_node_injects_resolved_value_not_definition_text() {
        let start = node_with_mapping(
            "n_start",
            "start",
            serde_json::json!({"topic": "{{topic}}"}),
        );
        let def = definition(vec![stage("s1", vec![start, node("n1", "agent")])]);
        // node/result 里是定义原文（实时执行的预填充不写 node/result）
        let outputs = outputs(&[("n_start", serde_json::json!({"topic": "{{topic}}"}))]);
        let context = rebuild_context_from_records(
            &outputs,
            &sessions(&[]),
            &def,
            &scope(&[]),
            &serde_json::json!({"topic": "真实主题"}),
        );
        assert_eq!(
            context.get("n_start"),
            Some(&serde_json::json!({"topic": "真实主题"}))
        );
    }

    #[test]
    fn rebuild_skips_scope_nodes_and_restores_sessions_for_the_rest() {
        let def = definition(vec![stage(
            "s1",
            vec![node("n1", "agent"), node("n2", "agent")],
        )]);
        let outputs = outputs(&[
            ("n1", serde_json::json!("旧值1")),
            ("n2", serde_json::json!("旧值2")),
        ]);
        let sessions = sessions(&[("n1", "sess-1"), ("n2", "sess-2")]);
        let context = rebuild_context_from_records(
            &outputs,
            &sessions,
            &def,
            &scope(&["n1"]),
            &serde_json::json!({}),
        );

        // scope 内节点本轮重跑：不注入上一轮输出，也不注入上一轮会话
        assert!(!context.contains_key("n1"));
        assert!(!context.contains_key("__session_id__n1"));
        // scope 外节点保持可用
        assert_eq!(
            context.get("n2"),
            Some(&serde_json::json!({"output": "旧值2"}))
        );
        assert_eq!(
            context.get("__session_id__n2"),
            Some(&serde_json::json!("sess-2"))
        );
    }

    #[test]
    fn boundary_condition_edges_keep_only_out_of_scope_sources() {
        let mut s1 = stage("s1", vec![node("n1", "agent"), node("n2", "agent")]);
        s1.edges = vec![
            edge("e1", "n_out", "n1", Some("==ok")),
            edge("e2", "n1", "n2", Some("==ok")),
            edge("e3", "n2", "n_out2", None),
        ];
        let map = boundary_condition_edges(&[s1], &scope(&["n1", "n2"]));
        let kept = map.get("s1").expect("s1 应保留 source 在 scope 外的入边");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "e1");
        // 没有边界入边的阶段不出现
        assert!(boundary_condition_edges(
            &[stage("s2", vec![node("n3", "agent")])],
            &scope(&["n3"])
        )
        .is_empty());
    }

    #[test]
    fn stage_downstream_targets_ignore_foreign_source_edges() {
        let mut s1 = stage("s1", vec![node("n1", "agent")]);
        let mut s2 = stage("s2", vec![node("n2", "agent")]);
        s1.stage_edges = vec![edge("se1", "s1", "s2", None)];
        // 阶段声明的连线 source 不是本阶段（残留/异常数据）：不纳入链上范围
        s2.stage_edges = vec![edge("se2", "s1", "s3", None)];
        let map = stage_downstream_targets(&[s1, s2]);
        assert_eq!(map.get("s1"), Some(&vec!["s2".to_string()]));
    }

    #[test]
    fn gate_output_baseline_is_collected_and_merged_with_current_round() {
        let context = serde_json::json!({
            "n1": {"output": "值"},
            "gate_output.s1": {"n1_output": "旧"},
            "gate_output.s2": 3,
        });
        let baseline = collect_gate_output_baseline(&context);
        assert_eq!(baseline.len(), 2);
        assert_eq!(
            baseline.get("s1"),
            Some(&serde_json::json!({"n1_output": "旧"}))
        );
        assert_eq!(baseline.get("s2"), Some(&serde_json::json!(3)));

        // 本轮结果覆盖同名键，基线中的其余键保留
        assert_eq!(
            merge_gate_output_baseline(
                baseline.get("s1"),
                serde_json::json!({"n1_output": "新", "n9_output": 1})
            ),
            serde_json::json!({"n1_output": "新", "n9_output": 1})
        );
        // 非对象结果（Concat / PickFirst 等）整体替换，无历史时原样返回
        assert_eq!(
            merge_gate_output_baseline(baseline.get("s1"), serde_json::json!([1, 2])),
            serde_json::json!([1, 2])
        );
        // 基线中的空产出键（无实质产出的节点）不保留
        assert_eq!(
            merge_gate_output_baseline(
                Some(&serde_json::json!({"n_start": {}, "n1_output": "旧"})),
                serde_json::json!({})
            ),
            serde_json::json!({"n1_output": "旧"})
        );
        assert_eq!(
            merge_gate_output_baseline(None, serde_json::json!({"a": 1})),
            serde_json::json!({"a": 1})
        );
    }

    #[test]
    fn gate_statuses_keep_current_round_over_history() {
        let current: HashMap<String, String> = [("n1".to_string(), "completed".to_string())]
            .into_iter()
            .collect();
        let history: HashMap<String, String> = [
            ("n1".to_string(), "failed".to_string()),
            ("n2".to_string(), "failed".to_string()),
        ]
        .into_iter()
        .collect();
        let merged = gate_statuses_with_history(&current, &history);
        assert_eq!(merged.get("n1"), Some(&"completed".to_string()));
        assert_eq!(merged.get("n2"), Some(&"failed".to_string()));
    }

    #[test]
    fn gate_all_uses_full_stage_and_scope_outside_history() {
        // 默认门控策略是 All + Merge；裁剪后的阶段只剩 scope 内的 n1
        let full = stage("s1", vec![node("n1", "agent"), node("n2", "agent")]);
        let recovered = stage("s1", vec![node("n1", "agent")]);
        let merged = serde_json::json!({});
        let current: HashMap<String, String> = [("n1".to_string(), "completed".to_string())]
            .into_iter()
            .collect();
        let history: HashMap<String, String> = [("n2".to_string(), "failed".to_string())]
            .into_iter()
            .collect();

        // 只看本轮子集：少算 n2，误判通过（修复前的行为）
        assert!(matches!(
            WorkflowEngine::check_gate_strategy(&recovered, &current, &merged).unwrap(),
            GateDecision::Pass
        ));

        // 完整阶段 + scope 外历史状态：All 策略按 n2 失败判定 → 被策略拦下（正常结果，不是 Err）
        let statuses = gate_statuses_with_history(&current, &history);
        assert!(matches!(
            WorkflowEngine::check_gate_strategy(&full, &statuses, &merged).unwrap(),
            GateDecision::Blocked(_)
        ));
    }

    #[test]
    fn progress_total_uses_full_reachable_count_in_breakpoint_modes() {
        let recovery = RecoveryContext {
            total_reachable: 9,
            history_statuses: HashMap::new(),
            full_stages: HashMap::new(),
            gate_output_baseline: HashMap::new(),
            boundary_edges: HashMap::new(),
        };

        // 断点执行：分母是完整定义的可达节点数，不是本轮 scope 数
        assert_eq!(progress_total(Some(&recovery), 2), 9);
        // 全量执行：分母就是本轮可达节点数
        assert_eq!(progress_total(None, 2), 2);
    }

    #[test]
    fn terminal_status_never_stays_running() {
        assert_eq!(resolve_terminal_status(4, 4, 0, 0, false), "success");
        // 门控失败等 impl 错误：无 failed/cancelled 节点也必须落 failed
        assert_eq!(resolve_terminal_status(2, 4, 0, 0, true), "failed");
        assert_eq!(resolve_terminal_status(2, 4, 1, 0, false), "failed");
        assert_eq!(resolve_terminal_status(2, 4, 0, 1, false), "cancelled");
        // 部分完成且无失败（如链式只跑了后半段）不再停在 running
        assert_eq!(resolve_terminal_status(2, 4, 0, 0, false), "failed");
    }

    #[test]
    fn terminal_status_counts_skipped_as_settled() {
        // settled = 完成 + 跳过：条件分支未命中的节点是设计上的结果，不算失败、不拖完成度
        assert_eq!(resolve_terminal_status(2 + 2, 4, 0, 0, false), "success");
        // 有节点失败时 settled < total → failed（跳过不会把失败"盖"成成功）
        assert_eq!(resolve_terminal_status(2 + 1, 4, 1, 0, false), "failed");
        // 门控未通过（impl 错误）仍 failed，即使其他节点都结算了
        assert_eq!(resolve_terminal_status(4, 4, 0, 0, true), "failed");
    }

    #[test]
    fn evaluate_condition_parses_field_prefix_and_all_operators() {
        let output = serde_json::json!({ "score": 72, "level": "high", "note": "" });

        // 前端结构化编辑器写入的格式：`字段 运算符 值`
        assert!(WorkflowEngine::evaluate_condition(
            "score >= 60",
            Some(&output)
        ));
        assert!(!WorkflowEngine::evaluate_condition(
            "score > 80",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "level == high",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "level starts_with hi",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "level ends_with gh",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "level contains ig",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "note is_empty",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "level is_not_empty",
            Some(&output)
        ));

        // 字段不存在：大小比较判不满足，存在性判断判为空
        assert!(!WorkflowEngine::evaluate_condition(
            "missing >= 0",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "missing is_empty",
            Some(&output)
        ));

        // 兼容省略字段的旧格式（取输出的第一个值）
        let scalar = serde_json::json!("yes");
        assert!(WorkflowEngine::evaluate_condition("== yes", Some(&scalar)));
        assert!(WorkflowEngine::evaluate_condition(
            "contains ye",
            Some(&scalar)
        ));

        // 无上游输出时条件不成立
        assert!(!WorkflowEngine::evaluate_condition("== yes", None));
    }

    #[test]
    fn incoming_edges_require_executed_source_with_or_semantics() {
        let edges = vec![
            edge("e1", "a", "c", None),
            edge("e2", "b", "c", Some("score >= 60")),
        ];
        let refs: Vec<&WorkflowEdge> = edges.iter().collect();

        // 上游都没执行（被条件跳过）：链路没走到，下游不能执行
        assert!(!WorkflowEngine::any_incoming_edge_active(
            &refs,
            &HashMap::new()
        ));

        // 上游 a 执行过：无条件入边连通 → 多入边取 OR，可执行
        let mut ctx_a: HashMap<String, Value> = HashMap::new();
        ctx_a.insert("a".to_string(), serde_json::json!({}));
        assert!(WorkflowEngine::any_incoming_edge_active(&refs, &ctx_a));

        // 只有 b 执行过，且条件不满足 → 不执行
        let mut ctx_bad: HashMap<String, Value> = HashMap::new();
        ctx_bad.insert("b".to_string(), serde_json::json!({ "score": 10 }));
        assert!(!WorkflowEngine::any_incoming_edge_active(&refs, &ctx_bad));

        // 只有 b 执行过且条件满足 → 执行
        let mut ctx_ok: HashMap<String, Value> = HashMap::new();
        ctx_ok.insert("b".to_string(), serde_json::json!({ "score": 90 }));
        assert!(WorkflowEngine::any_incoming_edge_active(&refs, &ctx_ok));
    }

    #[test]
    fn output_mapping_path_drills_into_json_text() {
        let mapping: serde_json::Map<String, Value> = serde_json::from_value(serde_json::json!({
            "score": "{{content.score}}",
            "first": "{{content[0].name}}",
            "whole": "{{content}}",
        }))
        .unwrap();

        // Agent 节点：执行结果是 JSON 文本，按路径可直接取到字段
        let agent_output = Value::String("{\"score\": 60}".to_string());
        let exposed = expose_node_output(&mapping, &agent_output, None);
        assert_eq!(exposed.get("score"), Some(&serde_json::json!(60)));
        assert_eq!(exposed.get("whole"), Some(&agent_output));
        // 取不到时回退整个输出（保持原有宽松语义）
        assert_eq!(exposed.get("first"), Some(&agent_output));

        // 输出本就是对象：content 字段存在时从它下钻
        let obj_output = serde_json::json!({ "content": { "score": 7 }, "other": 1 });
        assert_eq!(
            expose_node_output(&mapping, &obj_output, None).get("score"),
            Some(&serde_json::json!(7))
        );

        // 数组文本 + content[N].字段
        let arr_output = Value::String("[{\"name\":\"alice\"}]".to_string());
        let mut arr_mapping = serde_json::Map::new();
        arr_mapping.insert(
            "first".to_string(),
            Value::String("{{content[0].name}}".to_string()),
        );
        assert_eq!(
            expose_node_output(&arr_mapping, &arr_output, None).get("first"),
            Some(&serde_json::json!("alice"))
        );
    }

    #[test]
    fn input_mapping_normalizes_json_text_value() {
        // 输入映射解析统一走取值归一化：JSON 文本 → 单值解包 / 多字段成结构
        let mut n = node("n1", "transform");
        n.input_mapping = Some(serde_json::json!({ "input1": "{{out.up.stage1}}" }));
        let mut single: HashMap<String, Value> = HashMap::new();
        single.insert(
            "up".to_string(),
            serde_json::json!({ "out": "{\"result\": 0.98}" }),
        );
        assert_eq!(
            resolve_node_input(&n, &single).0,
            serde_json::json!({ "input1": 0.98 })
        );
        let mut multi: HashMap<String, Value> = HashMap::new();
        multi.insert(
            "up".to_string(),
            serde_json::json!({ "out": "{\"a\": 1, \"b\": 2}" }),
        );
        assert_eq!(
            resolve_node_input(&n, &multi).0,
            serde_json::json!({ "input1": { "a": 1, "b": 2 } })
        );
    }

    #[test]
    fn unresolved_input_mapping_becomes_null_and_is_reported() {
        // 模板未解析 → 无值（null），而不是把 {{...}} 原文当值传给下游；同时回报未解析表达式
        let mut n = node("n1", "transform");
        n.input_mapping = Some(serde_json::json!({ "input1": "{{gate_output.stage_x}}" }));
        let (value, unresolved) = resolve_node_input(&n, &HashMap::new());
        assert_eq!(value, serde_json::json!({ "input1": Value::Null }));
        assert_eq!(
            unresolved,
            vec!["input1: {{gate_output.stage_x}}".to_string()]
        );

        // 纯字面量（无占位符）不受影响，且不产生未解析记录
        let mut lit = node("n2", "transform");
        lit.input_mapping = Some(serde_json::json!({ "input1": "常量" }));
        let (value, unresolved) = resolve_node_input(&lit, &HashMap::new());
        assert_eq!(value, serde_json::json!({ "input1": "常量" }));
        assert!(unresolved.is_empty());
    }

    #[test]
    fn shorthand_keeps_gate_output_and_builtin_prefix_intact() {
        // {{gate_output.阶段.字段}} 属于引擎内建变量，不能按输入映射简写改写；
        // 输入映射简写仍按 `参数名.节点ID.阶段ID` 正常展开。
        assert_eq!(
            TemplateEngine::expand_shorthand_for_test("gate_output.stage1.result"),
            "gate_output.stage1.result"
        );
        assert_eq!(
            TemplateEngine::expand_shorthand_for_test("__input__.a"),
            "__input__.a"
        );
        assert_eq!(
            TemplateEngine::expand_shorthand_for_test("out.node1.stage1"),
            "node1.out"
        );
        assert_eq!(
            TemplateEngine::expand_shorthand_for_test("out.result.node1.stage1"),
            "node1.out.result"
        );
    }

    #[test]
    fn gate_config_accepts_frontend_wire_format() {
        // 前端保存格式：strategy / mergeStrategy 为字符串，数量与条件表达式放在同级 threshold
        let gate: crate::workflow::GateConfig = serde_json::from_value(serde_json::json!({
            "strategy": "count", "mergeStrategy": "merge", "threshold": "3"
        }))
        .expect("count 门控必须能反序列化");
        assert_eq!(gate.strategy, crate::workflow::GateStrategy::Count);
        assert_eq!(gate.threshold.as_deref(), Some("3"));
        let gate2: crate::workflow::GateConfig = serde_json::from_value(serde_json::json!({
            "strategy": "threshold", "mergeStrategy": "custom",
            "threshold": ">= 60", "customScript": "calc:success:array:avg"
        }))
        .expect("threshold 门控必须能反序列化");
        assert_eq!(gate2.strategy, crate::workflow::GateStrategy::Threshold);
        assert_eq!(gate2.merge_strategy, crate::workflow::MergeStrategy::Custom);
        assert_eq!(
            gate2.custom_script.as_deref(),
            Some("calc:success:array:avg")
        );
    }

    #[test]
    fn condition_uses_normalized_value() {
        // 边条件取值与输入映射同语义：JSON 文本先归一化再比较
        let output = serde_json::json!({ "out": "{\"result\": 0.98}" });
        assert!(WorkflowEngine::evaluate_condition(
            "out >= 0",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "out == 0.98",
            Some(&output)
        ));
        assert!(!WorkflowEngine::evaluate_condition(
            "out < 0",
            Some(&output)
        ));
    }

    #[test]
    fn condition_field_supports_paths() {
        // 多字段 JSON 文本：条件字段支持点号路径与数组下标
        let output = serde_json::json!({
            "out": "{\"status\": \"ok\", \"items\": [{\"name\": \"a\"}]}"
        });
        assert!(WorkflowEngine::evaluate_condition(
            "out.status == ok",
            Some(&output)
        ));
        assert!(WorkflowEngine::evaluate_condition(
            "out.items[0].name == a",
            Some(&output)
        ));
        assert!(!WorkflowEngine::evaluate_condition(
            "out.status == bad",
            Some(&output)
        ));
    }

    #[test]
    fn probe_output_field_paths_lists_mapping_and_nested_fields() {
        let mapping: serde_json::Map<String, Value> = serde_json::from_value(serde_json::json!({
            "output": "{{content}}",
        }))
        .unwrap();
        let output =
            Value::String("{\"result\": 0.98, \"items\": [{\"name\": \"a\"}]}".to_string());
        let paths = probe_output_field_paths(&output, Some(&mapping));
        for expected in [
            "output",
            "output.result",
            "output.items",
            "output.items[0]",
            "output.items[0].name",
        ] {
            assert!(
                paths.contains(&expected.to_string()),
                "缺少候选路径 {}",
                expected
            );
        }
    }

    #[test]
    fn final_output_prefers_end_node_fields() {
        let mut s = stage("s1", vec![node("n1", "agent"), node("n2", "end")]);
        s.edges = vec![edge("e1", "n1", "n2", None)];
        let def = definition(vec![s]);
        let ctx: HashMap<String, Value> = [
            ("n1".to_string(), serde_json::json!({"result": "上游正文"})),
            (
                "n2".to_string(),
                serde_json::json!({"summary": "结束节点汇总"}),
            ),
        ]
        .into_iter()
        .collect();

        let (output, source, label) = resolve_final_output(&def, &ctx);
        assert_eq!(source, "end");
        assert_eq!(label.as_deref(), Some("n2"));
        assert_eq!(output, Some(serde_json::json!({"summary": "结束节点汇总"})));
    }

    #[test]
    fn final_output_falls_back_to_end_upstream_when_end_has_no_mapping() {
        // 自动生成图（会话/群聊转工作流）的 End 没有 inputMapping，产出恒为 {}：
        // 此时取收口节点的产出，否则用户拿不到任何结果。
        let mut s = stage("s1", vec![node("n1", "agent"), node("n2", "end")]);
        s.edges = vec![edge("e1", "n1", "n2", None)];
        let def = definition(vec![s]);
        let ctx: HashMap<String, Value> = [
            ("n1".to_string(), serde_json::json!({"result": "上游正文"})),
            ("n2".to_string(), serde_json::json!({})),
        ]
        .into_iter()
        .collect();

        let (output, source, label) = resolve_final_output(&def, &ctx);
        assert_eq!(source, "end-upstream");
        assert_eq!(label.as_deref(), Some("n1"));
        assert_eq!(output, Some(serde_json::json!({"result": "上游正文"})));
    }

    #[test]
    fn final_output_ignores_end_fields_that_resolved_empty() {
        // `{"summary": ""}` 壳非空、值全空：必须继续往下找，否则会把真正的内容挡在展示之外
        let mut s = stage("s1", vec![node("n1", "agent"), node("n2", "end")]);
        s.edges = vec![edge("e1", "n1", "n2", None)];
        let def = definition(vec![s]);
        let ctx: HashMap<String, Value> = [
            ("n1".to_string(), serde_json::json!("上游正文")),
            (
                "n2".to_string(),
                serde_json::json!({"summary": "", "extra": null}),
            ),
        ]
        .into_iter()
        .collect();

        let (output, source, _) = resolve_final_output(&def, &ctx);
        assert_eq!(source, "end-upstream");
        assert_eq!(output, Some(serde_json::json!("上游正文")));
    }

    #[test]
    fn final_output_falls_back_to_last_node_without_end() {
        let def = definition(vec![stage(
            "s1",
            vec![node("n_start", "start"), node("n1", "agent")],
        )]);
        let ctx: HashMap<String, Value> = [
            (
                "n_start".to_string(),
                serde_json::json!({"topic": "初始入参"}),
            ),
            ("n1".to_string(), serde_json::json!({"result": "节点产出"})),
        ]
        .into_iter()
        .collect();

        let (output, source, label) = resolve_final_output(&def, &ctx);
        assert_eq!(source, "last-node");
        assert_eq!(label.as_deref(), Some("n1"));
        assert_eq!(output, Some(serde_json::json!({"result": "节点产出"})));
    }

    #[test]
    fn final_output_is_none_when_nothing_produced() {
        let def = definition(vec![stage("s1", vec![node("n1", "agent")])]);
        let ctx: HashMap<String, Value> = [("n1".to_string(), serde_json::json!("   "))]
            .into_iter()
            .collect();

        let (output, source, label) = resolve_final_output(&def, &ctx);
        assert_eq!(source, "none");
        assert!(output.is_none());
        assert!(label.is_none());
    }

    #[test]
    fn apply_final_output_never_writes_empty_result() {
        // 无产出时不写任何字段：断点执行会先后写多条终态事件，null 不能覆盖已有产出
        let mut event = serde_json::json!({"status": "success"});
        apply_final_output(&mut event, (None, "none", None));
        assert!(event.get("output").is_none());
        assert!(event.get("outputSource").is_none());

        apply_final_output(
            &mut event,
            (
                Some(serde_json::json!("正文")),
                "end",
                Some("结束".to_string()),
            ),
        );
        assert_eq!(event["output"], serde_json::json!("正文"));
        assert_eq!(event["outputSource"], serde_json::json!("end"));
        assert_eq!(event["outputNodeLabel"], serde_json::json!("结束"));
    }

    #[test]
    fn run_terminal_status_gate_abort_is_failed_not_cancelled() {
        // 门控拦住：内部把 cancelled 置真只是为了叫停同层阶段，
        // 实例终态必须是 failed（曾因此把"因失败而终止"显示成"用户取消"）
        assert_eq!(
            resolve_run_terminal_status(
                1,
                2,
                Some("阶段 '群聊任务' 门控策略 All 不满足: 1/2 个节点失败，工作流中止"),
                true,
            ),
            "failed"
        );
        // 用户取消
        assert_eq!(
            resolve_run_terminal_status(1, 2, Some("工作流已被取消"), true),
            "cancelled"
        );
        // 全部结算且未中断 → success（取消信号来得晚也算成功，与原判据一致）
        assert_eq!(resolve_run_terminal_status(2, 2, None, false), "success");
        assert_eq!(resolve_run_terminal_status(2, 2, None, true), "success");
        // 有节点未结算且未被中断 → failed
        assert_eq!(resolve_run_terminal_status(1, 2, None, false), "failed");
    }
}
