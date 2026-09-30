//! 工作流事件读侧派生（`workflow_events` → 节点状态/输出的纯投影）。
//!
//! 节点写路径已收敛为事件（node/start · node/status · node/result · node/reset），
//! 旧表 `node_executions` 将按"事件为唯一事实源"物理删除；调度/看板/重建经本层读取。
//! 派生规则与旧表语义对齐：
//! - 状态：以事件 seq 顺序叠加、后到者胜（node/start 记 running/skipped 初态，
//!   node/status 记迁移，node/reset 把列出的节点置回 pending）；
//! - 输出：node/result 每节点保留最新一条（output / artifactsPath）。
//!
//! 本层为生产读切点（列表/看板/恢复/日志均经此处折叠）；单测在文件底部。
//! 个别仅测试/旧接口使用项按需标注 #[allow(dead_code)]。

use crate::utils::errors::AppError;
use crate::workflow::{
    ExecutionTimelinePoint, NodeTypeStat, Stage, TopErrorStat, TopWorkflowStat, WorkflowDefinition,
    WorkflowInstance, WorkflowInstanceStatus, WorkflowStats,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

/// [`WorkflowInstanceStatus`] → 旧表/事件 payload 的状态字面量（测试/工具用）。
#[allow(dead_code)]
pub fn status_code(status: &WorkflowInstanceStatus) -> &'static str {
    match status {
        WorkflowInstanceStatus::Pending => "pending",
        WorkflowInstanceStatus::Running => "running",
        WorkflowInstanceStatus::Paused => "paused",
        WorkflowInstanceStatus::Success => "success",
        WorkflowInstanceStatus::Failed => "failed",
        WorkflowInstanceStatus::Cancelled => "cancelled",
        WorkflowInstanceStatus::Timeout => "timeout",
    }
}

fn parse_status(s: &str) -> Option<WorkflowInstanceStatus> {
    Some(match s {
        "pending" => WorkflowInstanceStatus::Pending,
        "running" => WorkflowInstanceStatus::Running,
        "paused" => WorkflowInstanceStatus::Paused,
        "success" => WorkflowInstanceStatus::Success,
        "failed" => WorkflowInstanceStatus::Failed,
        "cancelled" => WorkflowInstanceStatus::Cancelled,
        "timeout" => WorkflowInstanceStatus::Timeout,
        _ => return None,
    })
}

/// 节点最终状态投影（node_id → status）。
/// 处理顺序即事件 seq：node/start（初态 running/skipped）、node/status（迁移）、
/// node/reset（把 payload.nodes 列出的节点全部置回 pending）。
pub fn derive_node_statuses(
    conn: &Connection,
    execution_id: &str,
) -> Result<BTreeMap<String, String>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT kind, payload FROM workflow_events
         WHERE execution_id = ?1 AND kind IN ('node/start', 'node/status', 'node/reset')
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![execution_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for row in rows {
        let (kind, payload) = row?;
        let v: Value = serde_json::from_str(&payload)?;
        if kind == "node/reset" {
            if let Some(nodes) = v.get("nodes").and_then(|n| n.as_array()) {
                for node in nodes {
                    if let Some(id) = node.as_str() {
                        map.insert(id.to_string(), "pending".to_string());
                    }
                }
            }
        } else if let (Some(id), Some(status)) = (
            v.get("nodeId").and_then(|n| n.as_str()),
            v.get("status").and_then(|s| s.as_str()),
        ) {
            map.insert(id.to_string(), status.to_string());
        }
    }
    Ok(map)
}

/// 节点输出投影（node_id → (output, artifacts_path)），node/result 每节点最新一条胜出。
pub fn derive_node_outputs(
    conn: &Connection,
    execution_id: &str,
) -> Result<BTreeMap<String, (Option<String>, Option<String>)>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM workflow_events
         WHERE execution_id = ?1 AND kind = 'node/result'
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![execution_id], |r| r.get::<_, String>(0))?;
    let mut map: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    for row in rows {
        let v: Value = serde_json::from_str(&row?)?;
        if let Some(id) = v.get("nodeId").and_then(|n| n.as_str()) {
            map.insert(
                id.to_string(),
                (
                    v.get("output").and_then(|o| o.as_str()).map(str::to_string),
                    v.get("artifactsPath")
                        .and_then(|a| a.as_str())
                        .map(str::to_string),
                ),
            );
        }
    }
    Ok(map)
}

/// 节点 agent 会话 id 投影（node_id → agentSessionId）。
///
/// 只有成功收尾的节点才在 node/status 携带非空 `agentSessionId`（running/failed 落 null），
/// 因此取每节点最新的非空值：既反映最近一次成功执行的会话，也不会被后续的空值抹掉。
pub fn derive_node_agent_sessions(
    conn: &Connection,
    execution_id: &str,
) -> Result<BTreeMap<String, String>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM workflow_events
         WHERE execution_id = ?1 AND kind = 'node/status'
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![execution_id], |r| r.get::<_, String>(0))?;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for row in rows {
        let v: Value = serde_json::from_str(&row?)?;
        if let (Some(id), Some(sid)) = (
            v.get("nodeId").and_then(|n| n.as_str()),
            v.get("agentSessionId").and_then(|s| s.as_str()),
        ) {
            map.insert(id.to_string(), sid.to_string());
        }
    }
    Ok(map)
}

/// 节点最近一次执行开始的 `timestamp`（node/start 最新一条；尝试被 reset 后由新一轮 start 覆盖）。
#[allow(dead_code)]
pub fn node_started_at(
    conn: &Connection,
    execution_id: &str,
    node_id: &str,
) -> Result<Option<i64>, AppError> {
    let timestamp: Option<i64> = conn
        .query_row(
            "SELECT json_extract(payload, '$.timestamp') FROM workflow_events
             WHERE execution_id = ?1 AND kind = 'node/start' AND json_extract(payload, '$.nodeId') = ?2
             ORDER BY seq DESC LIMIT 1",
            params![execution_id, node_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(timestamp)
}

/// 折叠出某执行的节点详情行（对齐旧 `get_node_executions` 输出，按 startedAt 升序）。
/// 每节点字段取各自最新事件（start 的 startedAt/inputPreview、status 的 finishedAt/error、
/// result 的 output），最终 status 以 [`derive_node_statuses`] 为准（含 reset 置 pending）。
pub fn derive_node_details(
    conn: &Connection,
    execution_id: &str,
) -> Result<Vec<serde_json::Value>, AppError> {
    let statuses = derive_node_statuses(conn, execution_id)?;
    let outputs = derive_node_outputs(conn, execution_id)?;
    let mut starts: BTreeMap<String, (Option<i64>, Option<String>)> = BTreeMap::new();
    let mut finishes: BTreeMap<String, (Option<i64>, Option<String>)> = BTreeMap::new();
    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut stmt = conn.prepare(
        "SELECT kind, payload FROM workflow_events
         WHERE execution_id = ?1 AND kind IN ('node/start', 'node/status', 'node/result')
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![execution_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (kind, payload) = row?;
        let v: Value = serde_json::from_str(&payload)?;
        let Some(nid) = v.get("nodeId").and_then(|n| n.as_str()) else {
            continue;
        };
        ids.insert(nid.to_string());
        match kind.as_str() {
            "node/start" => {
                starts.insert(
                    nid.to_string(),
                    (
                        v.get("timestamp").and_then(|t| t.as_i64()),
                        v.get("inputPreview")
                            .and_then(|i| i.as_str())
                            .map(str::to_string),
                    ),
                );
            }
            "node/status" => {
                finishes.insert(
                    nid.to_string(),
                    (
                        v.get("finishedAt")
                            .and_then(|t| t.as_i64())
                            .or_else(|| v.get("timestamp").and_then(|t| t.as_i64())),
                        v.get("errorMessage")
                            .and_then(|e| e.as_str())
                            .map(str::to_string),
                    ),
                );
            }
            _ => {}
        }
    }

    // node/start 的 inputPreview 在写入时被截断到 500 字符，长输入（Agent 提示词动辄几千字）
    // 必然不是合法 JSON：解析失败时退回原文（前端按纯文本展示），
    // 否则"输入"这一块对真实节点几乎永远是空的。
    let parse_val = |raw: Option<String>| match raw {
        Some(s) => serde_json::from_str::<Value>(&s).unwrap_or(Value::String(s)),
        None => Value::Null,
    };
    let mut out: Vec<serde_json::Value> = Vec::new();
    for nid in ids {
        let status = statuses
            .get(&nid)
            .cloned()
            .unwrap_or_else(|| "pending".to_string());
        let (started, input_preview) = starts.get(&nid).cloned().unwrap_or((None, None));
        let (finished, error) = finishes.get(&nid).cloned().unwrap_or((None, None));
        let output = outputs.get(&nid).and_then(|(o, _)| o.clone());
        out.push(serde_json::json!({
            "nodeId": nid,
            "status": status,
            "input": parse_val(input_preview),
            "output": parse_val(output),
            "error": error,
            "startedAt": started,
            "finishedAt": finished,
        }));
    }
    out.sort_by_key(|r| {
        r.get("startedAt")
            .and_then(|s| s.as_i64())
            .unwrap_or(i64::MAX)
    });
    Ok(out)
}

/// 单节点执行投影行（对齐旧 `node_executions` 行语义；供恢复、节点统计等读切点折叠）。
#[derive(Debug, Clone)]
#[allow(dead_code)] // node_id 与 map key 冗余，暂保留供对账。
pub struct NodeExecRow {
    pub node_id: String,
    pub status: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub input_preview: Option<String>,
    pub error_message: Option<String>,
    pub output: Option<String>,
    pub artifacts_path: Option<String>,
}

/// 折叠某执行的节点行：node/start（初态/开始时间/输入预览）、node/status（终态/结束时间/错误）、
/// node/result（输出/产物路径）、node/reset（将列出的节点置回 pending 并清空时间/错误/输出）。
pub fn derive_node_rows(
    conn: &Connection,
    execution_id: &str,
) -> Result<BTreeMap<String, NodeExecRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT kind, payload FROM workflow_events
         WHERE execution_id = ?1 AND kind IN ('node/start', 'node/status', 'node/result', 'node/reset')
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![execution_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let default_row = |node_id: &str| NodeExecRow {
        node_id: node_id.to_string(),
        status: "pending".to_string(),
        started_at: None,
        finished_at: None,
        input_preview: None,
        error_message: None,
        output: None,
        artifacts_path: None,
    };
    let mut map: BTreeMap<String, NodeExecRow> = BTreeMap::new();
    for row in rows {
        let (kind, payload) = row?;
        let v: Value = serde_json::from_str(&payload)?;
        if kind == "node/reset" {
            if let Some(nodes) = v.get("nodes").and_then(|n| n.as_array()) {
                for node in nodes {
                    if let Some(id) = node.as_str() {
                        let row = map.entry(id.to_string()).or_insert_with(|| default_row(id));
                        row.status = "pending".to_string();
                        row.started_at = None;
                        row.finished_at = None;
                        row.error_message = None;
                        row.output = None;
                        row.artifacts_path = None;
                    }
                }
            }
            continue;
        }
        let Some(nid) = v.get("nodeId").and_then(|n| n.as_str()) else {
            continue;
        };
        let row = map
            .entry(nid.to_string())
            .or_insert_with(|| default_row(nid));
        match kind.as_str() {
            "node/start" => {
                if let Some(s) = v.get("status").and_then(|s| s.as_str()) {
                    row.status = s.to_string();
                }
                row.started_at = v.get("timestamp").and_then(|t| t.as_i64());
                row.input_preview = v
                    .get("inputPreview")
                    .and_then(|i| i.as_str())
                    .map(str::to_string);
            }
            "node/status" => {
                if let Some(s) = v.get("status").and_then(|s| s.as_str()) {
                    row.status = s.to_string();
                }
                row.finished_at = v
                    .get("finishedAt")
                    .and_then(|t| t.as_i64())
                    .or_else(|| v.get("timestamp").and_then(|t| t.as_i64()));
                row.error_message = v
                    .get("errorMessage")
                    .and_then(|e| e.as_str())
                    .map(str::to_string);
            }
            "node/result" => {
                row.output = v.get("output").and_then(|o| o.as_str()).map(str::to_string);
                row.artifacts_path = v
                    .get("artifactsPath")
                    .and_then(|a| a.as_str())
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    Ok(map)
}

/// 实例折叠中间态（对齐旧 `workflow_instances` 行字段）。
struct InstanceFold {
    exec_id: String,
    created_seq: Option<i64>,
    created_at: i64,
    definition_id: String,
    definition_name: String,
    trigger: String,
    trigger_detail: Option<String>,
    status: WorkflowInstanceStatus,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    completion_rate: f64,
    /// 跳过的节点数（条件分支未命中 / 上游无产出）：与 completion_rate 同源事件携带
    skipped_count: u32,
    /// 最终产出与来源（终态事件携带；取值规则见 engine 的 `resolve_final_output`）
    output: Option<Value>,
    output_source: Option<String>,
    output_node_label: Option<String>,
    error: Option<String>,
    context: Value,
}

impl InstanceFold {
    fn new(exec_id: &str, seq: i64, created_at: i64, v: &Value) -> Self {
        let definition_id = v
            .get("definitionId")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        let definition_name = v
            .get("definitionName")
            .and_then(|n| n.as_str())
            .unwrap_or(&definition_id)
            .to_string();
        let status = v
            .get("status")
            .and_then(|s| s.as_str())
            .and_then(parse_status)
            .unwrap_or(WorkflowInstanceStatus::Running);
        Self {
            exec_id: exec_id.to_string(),
            created_seq: Some(seq),
            created_at,
            definition_id,
            definition_name,
            trigger: v
                .get("trigger")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            trigger_detail: v
                .get("triggerDetail")
                .and_then(|t| t.as_str())
                .map(str::to_string),
            status,
            started_at: v.get("startedAt").and_then(|t| t.as_i64()),
            completed_at: None,
            completion_rate: v
                .get("completionRate")
                .and_then(|r| r.as_f64())
                .unwrap_or(0.0),
            skipped_count: v.get("skippedCount").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
            output: None,
            output_source: None,
            output_node_label: None,
            error: v
                .get("errorMessage")
                .and_then(|e| e.as_str())
                .map(str::to_string),
            context: Value::Object(serde_json::Map::new()),
        }
    }

    fn apply(&mut self, kind: &str, v: &Value) {
        match kind {
            "execution/status" => {
                if let Some(s) = v
                    .get("status")
                    .and_then(|s| s.as_str())
                    .and_then(parse_status)
                {
                    self.status = s;
                }
                if let Some(c) = v.get("completedAt").and_then(|c| c.as_i64()) {
                    self.completed_at = Some(c);
                } else if let Some(ts) = v.get("timestamp").and_then(|t| t.as_i64()) {
                    if matches!(
                        self.status,
                        WorkflowInstanceStatus::Success
                            | WorkflowInstanceStatus::Failed
                            | WorkflowInstanceStatus::Cancelled
                            | WorkflowInstanceStatus::Timeout
                    ) {
                        self.completed_at = Some(ts);
                    }
                }
                if let Some(rate) = v.get("completionRate").and_then(|r| r.as_f64()) {
                    self.completion_rate = rate;
                }
                if let Some(c) = v.get("skippedCount").and_then(|c| c.as_u64()) {
                    self.skipped_count = c as u32;
                }
                if let Some(c) = v.get("context") {
                    if c.is_object() {
                        self.context = c.clone();
                    }
                }
                if let Some(e) = v.get("errorMessage").and_then(|e| e.as_str()) {
                    self.error = Some(e.to_string());
                }
                // 最终产出：只有真带了产出才覆盖（断点执行会先后写多条终态事件，
                // 无产出的事件带 `output: null`，不能让它清掉前面算好的结果）。
                if let Some(o) = v.get("output") {
                    if !o.is_null() {
                        self.output = Some(o.clone());
                        self.output_source = v
                            .get("outputSource")
                            .and_then(|s| s.as_str())
                            .map(str::to_string);
                        self.output_node_label = v
                            .get("outputNodeLabel")
                            .and_then(|s| s.as_str())
                            .map(str::to_string);
                    }
                }
            }
            "execution/progress" => {
                if let Some(rate) = v.get("completionRate").and_then(|r| r.as_f64()) {
                    self.completion_rate = rate;
                }
                if let Some(c) = v.get("skippedCount").and_then(|c| c.as_u64()) {
                    self.skipped_count = c as u32;
                }
                if let Some(c) = v.get("context") {
                    if c.is_object() {
                        self.context = c.clone();
                    }
                }
            }
            "execution/reset" => {
                self.status = WorkflowInstanceStatus::Running;
                self.completed_at = None;
                self.error = None;
                // 重跑：清掉上一轮的产出，否则运行中会一直显示旧结果
                self.output = None;
                self.output_source = None;
                self.output_node_label = None;
            }
            _ => {}
        }
    }

    fn into_instance(self) -> WorkflowInstance {
        WorkflowInstance {
            id: self.exec_id,
            definition_id: self.definition_id,
            definition_name: self.definition_name,
            status: self.status,
            context: self.context,
            trigger: self.trigger,
            trigger_detail: self.trigger_detail,
            started_at: self.started_at,
            completed_at: self.completed_at,
            completion_rate: self.completion_rate,
            skipped_count: self.skipped_count,
            output: self.output,
            output_source: self.output_source,
            output_node_label: self.output_node_label,
            error: self.error,
            created_at: self.created_at,
        }
    }
}

/// 从事件日志折叠出全部实例的最新视图（按 created_at 降序；列表/详情/看板读切点）。
pub fn derive_instances(conn: &Connection) -> Result<Vec<WorkflowInstance>, AppError> {
    derive_instances_filtered(conn, None)
}

/// 折叠单个执行的最新视图（详情/中止判断用）。
pub fn derive_instance(
    conn: &Connection,
    execution_id: &str,
) -> Result<Option<WorkflowInstance>, AppError> {
    let mut list = derive_instances_filtered(conn, Some(execution_id))?;
    Ok(list.pop())
}

fn derive_instances_filtered(
    conn: &Connection,
    execution_id: Option<&str>,
) -> Result<Vec<WorkflowInstance>, AppError> {
    let row_map = |r: &rusqlite::Row| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    };
    let tuples: Vec<(i64, String, String, String, i64)> = match execution_id {
        Some(id) => {
            let mut stmt = conn.prepare(
                "SELECT seq, execution_id, kind, payload, created_at FROM workflow_events
                 WHERE kind IN ('execution/created','execution/status','execution/progress','execution/reset')
                   AND execution_id = ?1
                 ORDER BY seq ASC",
            )?;
            let x = stmt
                .query_map(params![id], row_map)?
                .collect::<Result<Vec<_>, _>>()?;
            x
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT seq, execution_id, kind, payload, created_at FROM workflow_events
                 WHERE kind IN ('execution/created','execution/status','execution/progress','execution/reset')
                 ORDER BY seq ASC",
            )?;
            let x = stmt
                .query_map([], row_map)?
                .collect::<Result<Vec<_>, _>>()?;
            x
        }
    };
    let mut folds: BTreeMap<String, InstanceFold> = BTreeMap::new();
    for (seq, exec_id, kind, payload, created_at) in tuples {
        let v: Value = serde_json::from_str(&payload)?;
        match kind.as_str() {
            "execution/created" => {
                folds.insert(
                    exec_id.clone(),
                    InstanceFold::new(&exec_id, seq, created_at, &v),
                );
            }
            _ => {
                let Some(fold) = folds.get_mut(&exec_id) else {
                    continue;
                };
                fold.apply(kind.as_str(), &v);
            }
        }
    }

    let mut list: Vec<InstanceFold> = folds.into_values().collect();
    list.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.created_seq.unwrap_or(0).cmp(&a.created_seq.unwrap_or(0)))
    });
    Ok(list.into_iter().map(InstanceFold::into_instance).collect())
}

/// 实例级派生摘要：由 `execution/created` + `execution/status` 折叠出每个实例的
/// (definition_id / created_at / 最新 status)。供列表与看板聚合读切点复用。
#[derive(Debug, Clone)]
#[allow(dead_code)] // 保留轻量摘要查询（测试/工具用）；主读侧用 derive_instances。
pub struct InstanceSummary {
    pub execution_id: String,
    pub definition_id: Option<String>,
    pub created_at: Option<i64>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub status: String,
}

/// 轻量摘要折叠（供 InstanceSummary 查询）。
#[allow(dead_code)]
pub fn derive_instance_summaries(conn: &Connection) -> Result<Vec<InstanceSummary>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT execution_id, kind, payload FROM workflow_events
         WHERE kind IN ('execution/created', 'execution/status')
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut by_exec: BTreeMap<String, InstanceSummary> = BTreeMap::new();
    for row in rows {
        let (execution_id, kind, payload) = row?;
        let v: Value = serde_json::from_str(&payload)?;
        let status = v
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let entry = by_exec
            .entry(execution_id.clone())
            .or_insert_with(|| InstanceSummary {
                execution_id: execution_id.clone(),
                definition_id: None,
                created_at: None,
                started_at: None,
                completed_at: None,
                status: String::new(),
            });
        match kind.as_str() {
            "execution/created" => {
                entry.definition_id = v
                    .get("definitionId")
                    .and_then(|d| d.as_str())
                    .map(str::to_string);
                entry.created_at = v.get("createdAt").and_then(|t| t.as_i64());
                entry.started_at = v.get("startedAt").and_then(|t| t.as_i64());
                entry.status = status;
            }
            _ => {
                if !status.is_empty() {
                    entry.status = status;
                }
                if let Some(c) = v.get("completedAt").and_then(|t| t.as_i64()) {
                    entry.completed_at = Some(c);
                }
            }
        }
    }
    Ok(by_exec.into_values().collect())
}

// ════════════════════════════════════════════════════════════
// 看板/列表/恢复聚合（读侧唯一入口：全部经 workflow_events 折叠）
// ════════════════════════════════════════════════════════════

fn status_slot(status: &WorkflowInstanceStatus) -> usize {
    match status {
        WorkflowInstanceStatus::Pending => 0,
        WorkflowInstanceStatus::Running => 1,
        WorkflowInstanceStatus::Paused => 2,
        WorkflowInstanceStatus::Success => 3,
        WorkflowInstanceStatus::Failed => 4,
        WorkflowInstanceStatus::Cancelled => 5,
        WorkflowInstanceStatus::Timeout => 6,
    }
}

/// 执行列表（按 created_at 降序；definition_id 过滤）。
pub fn list_instances(
    conn: &Connection,
    definition_id: Option<&str>,
) -> Result<Vec<WorkflowInstance>, AppError> {
    let mut instances = derive_instances(conn)?;
    if let Some(did) = definition_id {
        instances.retain(|i| i.definition_id == did);
    }
    backfill_output(conn, &mut instances);
    Ok(instances)
}

/// 终态实例补算「最终产出」。
///
/// 产出是本功能上线后才落进终态事件的，此前的执行记录里没有这个字段——不补算的话，
/// 用户翻历史记录只会看到"本次无产出"，而这恰恰是这次要解决的痛点。
/// 补算用与实时执行同一个 [`resolve_final_output`]，代价是每个定义多读一次定义表（带缓存）。
///
/// 注意：定义可能在执行后被改过，补算结果以**当前**定义的连线/节点类型为准；
/// 只补算终态实例，运行中的实例不显示"半截产出"。
fn backfill_output(conn: &Connection, instances: &mut [WorkflowInstance]) {
    let mut defs: std::collections::HashMap<String, Option<WorkflowDefinition>> =
        std::collections::HashMap::new();
    for inst in instances.iter_mut() {
        if inst.output.is_some() || !is_terminal_status(&inst.status) {
            continue;
        }
        let Some(ctx_obj) = inst.context.as_object() else {
            continue;
        };
        if ctx_obj.is_empty() {
            continue;
        }
        let def = defs.entry(inst.definition_id.clone()).or_insert_with(|| {
            super::get_definition(conn, &inst.definition_id)
                .ok()
                .flatten()
        });
        let Some(def) = def else { continue };
        let ctx: std::collections::HashMap<String, Value> = ctx_obj.clone().into_iter().collect();
        let (output, source, label) = super::engine::resolve_final_output(def, &ctx);
        if let Some(output) = output {
            inst.output = Some(output);
            inst.output_source = Some(source.to_string());
            inst.output_node_label = label;
        }
    }
}

/// 是否已是终态（只有终态才谈得上"最终产出"）
fn is_terminal_status(status: &WorkflowInstanceStatus) -> bool {
    matches!(
        status,
        WorkflowInstanceStatus::Success
            | WorkflowInstanceStatus::Failed
            | WorkflowInstanceStatus::Cancelled
            | WorkflowInstanceStatus::Timeout
    )
}

fn instance_duration_ms(inst: &WorkflowInstance) -> Option<i64> {
    match (inst.completed_at, inst.started_at) {
        (Some(c), Some(s)) if c >= s => Some((c - s) * 1000),
        _ => None,
    }
}

struct StatBucket {
    counts: [i64; 7],
    dur_sum: i64,
    dur_max: i64,
    dur_min: i64,
    dur_cnt: i64,
}

impl StatBucket {
    fn new() -> Self {
        Self {
            counts: [0; 7],
            dur_sum: 0,
            dur_max: 0,
            dur_min: i64::MAX,
            dur_cnt: 0,
        }
    }

    fn add(&mut self, inst: &WorkflowInstance) {
        self.counts[status_slot(&inst.status)] += 1;
        if let Some(d) = instance_duration_ms(inst) {
            self.dur_sum += d;
            self.dur_max = self.dur_max.max(d);
            self.dur_min = self.dur_min.min(d);
            self.dur_cnt += 1;
        }
    }

    /// 无样本返回 None（与"耗时恰好为 0ms"区分开）。
    ///
    /// 事件时间戳是**秒级**的（`utils::now()`），一次不足 1 秒的执行会得到 0ms——
    /// 若用 0 表示"无数据"，最小值聚合会被这一条压成 0 并显示为空，故统一用 Option 表达。
    fn avg_ms(&self) -> Option<f64> {
        if self.dur_cnt > 0 {
            Some(self.dur_sum as f64 / self.dur_cnt as f64)
        } else {
            None
        }
    }

    fn max_ms(&self) -> Option<i64> {
        if self.dur_cnt > 0 {
            Some(self.dur_max)
        } else {
            None
        }
    }

    fn min_ms(&self) -> Option<i64> {
        if self.dur_cnt > 0 {
            Some(self.dur_min)
        } else {
            None
        }
    }

    fn total(&self) -> i64 {
        self.counts.iter().sum()
    }
}

/// 看板总览（workflow_instances/node_executions 聚合并行读切点）。
pub fn get_workflow_stats(
    conn: &Connection,
    workflow_id: Option<&str>,
    days: Option<i64>,
) -> Result<WorkflowStats, AppError> {
    let now_ts = crate::utils::now();
    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;
    let n_days_ago = if is_all {
        0
    } else {
        now_ts - days_value * 24 * 3600
    };
    let seven_ago = now_ts - 7 * 24 * 3600;
    let thirty_ago = now_ts - 30 * 24 * 3600;
    let in_def = |inst: &WorkflowInstance| -> bool {
        workflow_id.map_or(true, |wid| inst.definition_id == wid)
    };
    let instances = derive_instances(conn)?;
    let mut total = StatBucket::new();
    let mut range = StatBucket::new();
    let mut last7 = 0i64;
    let mut last30 = 0i64;
    let mut lastn = 0i64;
    for inst in &instances {
        if !in_def(inst) {
            continue;
        }
        total.add(inst);
        if inst.created_at >= seven_ago {
            last7 += 1;
        }
        if inst.created_at >= thirty_ago {
            last30 += 1;
        }
        if is_all || inst.created_at >= n_days_ago {
            lastn += 1;
            range.add(inst);
        }
    }

    // 节点执行总数/失败数：按 workflow 域（无时间窗口，等价旧 COUNT(node_executions)）。
    let mut total_node_executions = 0i64;
    let mut node_failed_count = 0i64;
    for inst in &instances {
        if !in_def(inst) {
            continue;
        }
        let rows = derive_node_rows(conn, &inst.id)?;
        total_node_executions += rows.len() as i64;
        node_failed_count += rows.values().filter(|r| r.status == "failed").count() as i64;
    }

    let rate_of = |good: i64, total_cnt: i64| -> f64 {
        if total_cnt > 0 {
            good as f64 / total_cnt as f64 * 100.0
        } else {
            0.0
        }
    };
    Ok(WorkflowStats {
        total_executions: total.total(),
        success_count: total.counts[3],
        failed_count: total.counts[4],
        cancelled_count: total.counts[5],
        running_count: total.counts[1],
        pending_count: total.counts[0],
        paused_count: total.counts[2],
        timeout_count: total.counts[6],
        success_rate: rate_of(total.counts[3], total.total()),
        avg_duration_ms: total.avg_ms(),
        max_duration_ms: total.max_ms(),
        min_duration_ms: total.min_ms(),
        total_node_executions,
        node_failed_count,
        last_7_days_count: last7,
        last_30_days_count: last30,
        last_n_days_count: lastn,
        days: days_value,
        range_total: range.total(),
        range_success: range.counts[3],
        range_failed: range.counts[4],
        range_cancelled: range.counts[5],
        range_running: range.counts[1],
        range_pending: range.counts[0],
        range_paused: range.counts[2],
        range_timeout: range.counts[6],
        range_success_rate: rate_of(range.counts[3], range.total()),
        range_avg_duration_ms: range.avg_ms(),
        range_max_duration_ms: range.max_ms(),
        range_min_duration_ms: range.min_ms(),
    })
}

fn bucket_expr(granularity: &str) -> &'static str {
    match granularity {
        "day" => "DATE(?1, 'unixepoch')",
        "week" => "strftime('%Y-W%W', ?1, 'unixepoch')",
        "month" => "strftime('%Y-%m', ?1, 'unixepoch')",
        _ => "DATE(?1, 'unixepoch')",
    }
}

fn bucket_label(conn: &Connection, granularity: &str, ts: i64) -> Result<String, AppError> {
    let sql = format!("SELECT {} ", bucket_expr(granularity));
    let label = conn.query_row(&sql, params![ts], |r| r.get::<_, String>(0))?;
    Ok(label)
}

/// 执行时间线（日期分桶；与旧 SQL 的 DATE/strftime 标签保持一致）。
pub fn get_execution_timeline(
    conn: &Connection,
    workflow_id: Option<&str>,
    days: i64,
) -> Result<Vec<ExecutionTimelinePoint>, AppError> {
    let effective_days = if days <= 0 { 365 } else { days };
    let granularity: &str = if effective_days <= 90 {
        "day"
    } else if effective_days <= 365 {
        "week"
    } else {
        "month"
    };
    let now_ts = crate::utils::now();
    let start_ts = now_ts - effective_days * 24 * 3600;
    struct DayAgg {
        total: i64,
        success: i64,
        failed: i64,
        cancelled: i64,
        dur_sum: i64,
        dur_cnt: i64,
    }
    let mut buckets: BTreeMap<String, DayAgg> = BTreeMap::new();
    for inst in derive_instances(conn)? {
        if let Some(wid) = workflow_id {
            if inst.definition_id != wid {
                continue;
            }
        }
        if inst.created_at < start_ts {
            continue;
        }
        let label = bucket_label(conn, granularity, inst.created_at)?;
        let agg = buckets.entry(label).or_insert(DayAgg {
            total: 0,
            success: 0,
            failed: 0,
            cancelled: 0,
            dur_sum: 0,
            dur_cnt: 0,
        });
        agg.total += 1;
        match inst.status {
            WorkflowInstanceStatus::Success => agg.success += 1,
            WorkflowInstanceStatus::Failed => agg.failed += 1,
            WorkflowInstanceStatus::Cancelled => agg.cancelled += 1,
            _ => {}
        }
        if let Some(d) = instance_duration_ms(&inst) {
            agg.dur_sum += d;
            agg.dur_cnt += 1;
        }
    }

    let mut points: Vec<ExecutionTimelinePoint> = buckets
        .into_iter()
        .map(|(date, agg)| {
            let avg = if agg.dur_cnt > 0 {
                Some(agg.dur_sum as f64 / agg.dur_cnt as f64)
            } else {
                None
            };
            ExecutionTimelinePoint {
                date,
                total: agg.total,
                success: agg.success,
                failed: agg.failed,
                cancelled: agg.cancelled,
                avg_duration_ms: avg,
                granularity: granularity.to_string(),
            }
        })
        .collect();
    points.sort_by_key(|p| p.date.clone());
    Ok(points)
}

/// 节点类型统计（经事件折叠；node_type 映射取自 workflow_definitions.stages）。
pub fn get_node_type_stats(
    conn: &Connection,
    workflow_id: Option<&str>,
    days: Option<i64>,
) -> Result<Vec<NodeTypeStat>, AppError> {
    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;
    let now_ts = crate::utils::now();
    let start_ts = if is_all {
        0
    } else {
        now_ts - days_value * 24 * 3600
    };

    // 1. (definition_id, node_id) → node_type：仅解析本域相关定义（含老定义的兜底全量）。
    let def_sql = match workflow_id {
        Some(_) => "SELECT id, stages FROM workflow_definitions WHERE id = ?1",
        None => "SELECT id, stages FROM workflow_definitions",
    };
    let mut node_type_map: std::collections::HashMap<(String, String), String> =
        std::collections::HashMap::new();
    {
        let mut stmt = conn.prepare(def_sql)?;
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = match workflow_id {
            Some(wid) => vec![Box::new(wid.to_string())],
            None => vec![],
        };
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (def_id, stages_str) in rows {
            if let Ok(stages) = serde_json::from_str::<Vec<Stage>>(&stages_str) {
                for stage in &stages {
                    for node in &stage.nodes {
                        let nt = format!("{:?}", node.node_type).to_lowercase();
                        node_type_map.insert((def_id.clone(), node.id.clone()), nt);
                    }
                }
            }
        }
    }

    // 2. 折叠命中窗口/域的节点行并聚合。
    let mut agg: std::collections::HashMap<String, (i64, i64, f64)> =
        std::collections::HashMap::new();
    for inst in derive_instances(conn)? {
        if let Some(wid) = workflow_id {
            if inst.definition_id != wid {
                continue;
            }
        }
        if inst.created_at < start_ts {
            continue;
        }
        let rows = derive_node_rows(conn, &inst.id)?;
        for (node_id, row) in rows {
            let Some(nt) = node_type_map.get(&(inst.definition_id.clone(), node_id.clone())) else {
                continue;
            };
            let entry = agg.entry(nt.clone()).or_insert((0, 0, 0.0));
            entry.0 += 1;
            if row.status == "failed" {
                entry.1 += 1;
            }
            let dur = match (row.finished_at, row.started_at) {
                (Some(f), Some(s)) if f >= s => ((f - s) * 1000) as f64,
                _ => 0.0,
            };
            entry.2 += dur;
        }
    }

    let stats: Vec<NodeTypeStat> = agg
        .into_iter()
        .map(|(node_type, (count, failed_count, sum_duration))| {
            let avg_duration_ms = if count > 0 {
                sum_duration / count as f64
            } else {
                0.0
            };
            NodeTypeStat {
                node_type,
                count,
                failed_count,
                avg_duration_ms,
            }
        })
        .collect();
    Ok(stats)
}

/// Top 工作流排行（读切点：事件折叠）。
pub fn get_top_workflows(
    conn: &Connection,
    days: Option<i64>,
    limit: Option<i64>,
    sort_by: Option<&str>,
) -> Result<Vec<TopWorkflowStat>, AppError> {
    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;
    let limit_value = limit.unwrap_or(5).max(1) as usize;
    let now_ts = crate::utils::now();
    let start_ts = if is_all {
        0
    } else {
        now_ts - days_value * 24 * 3600
    };
    struct DefAgg {
        name: String,
        total: i64,
        success: i64,
        failed: i64,
        cancelled: i64,
    }
    let mut by_def: std::collections::HashMap<String, DefAgg> = std::collections::HashMap::new();
    for inst in derive_instances(conn)? {
        if inst.definition_id.is_empty() || inst.created_at < start_ts {
            continue;
        }
        let agg = by_def.entry(inst.definition_id.clone()).or_insert(DefAgg {
            name: String::new(),
            total: 0,
            success: 0,
            failed: 0,
            cancelled: 0,
        });
        agg.name = inst.definition_name.clone();
        agg.total += 1;
        match inst.status {
            WorkflowInstanceStatus::Success => agg.success += 1,
            WorkflowInstanceStatus::Failed => agg.failed += 1,
            WorkflowInstanceStatus::Cancelled => agg.cancelled += 1,
            _ => {}
        }
    }

    let mut list: Vec<(String, TopWorkflowStat)> = by_def
        .into_iter()
        .map(|(definition_id, agg)| {
            let failed_rate = if agg.total > 0 {
                agg.failed as f64 / agg.total as f64 * 100.0
            } else {
                0.0
            };
            let sort_key = definition_id.clone();
            (
                sort_key,
                TopWorkflowStat {
                    definition_id,
                    definition_name: agg.name,
                    total: agg.total,
                    success: agg.success,
                    failed: agg.failed,
                    cancelled: agg.cancelled,
                    failed_rate,
                },
            )
        })
        .collect();
    // 主排序按 sort_by（failed 或 total），并列时按 definition_id 兜底保证稳定。
    list.sort_by(|a, b| {
        let cmp = if sort_by == Some("failed") {
            b.1.failed.cmp(&a.1.failed).then(b.1.total.cmp(&a.1.total))
        } else {
            b.1.total.cmp(&a.1.total)
        };
        cmp.then(a.0.cmp(&b.0))
    });
    Ok(list.into_iter().take(limit_value).map(|(_, v)| v).collect())
}

/// Top 错误聚合（读切点：事件折叠；error 取自 failed 实例的 errorMessage）。
pub fn get_top_errors(
    conn: &Connection,
    days: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<TopErrorStat>, AppError> {
    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;
    let limit_value = limit.unwrap_or(5).max(1) as usize;
    let now_ts = crate::utils::now();
    let start_ts = if is_all {
        0
    } else {
        now_ts - days_value * 24 * 3600
    };
    struct ErrAgg {
        count: i64,
        last_at: i64,
    }
    let mut by_err: std::collections::HashMap<String, ErrAgg> = std::collections::HashMap::new();
    for inst in derive_instances(conn)? {
        if inst.status != WorkflowInstanceStatus::Failed || inst.created_at < start_ts {
            continue;
        }
        let raw = inst.error.as_deref().unwrap_or("");
        let text = if raw.trim().is_empty() {
            "(无错误信息)".to_string()
        } else {
            raw.trim().to_string()
        };
        let agg = by_err.entry(text).or_insert(ErrAgg {
            count: 0,
            last_at: inst.created_at,
        });
        agg.count += 1;
        agg.last_at = agg.last_at.max(inst.created_at);
    }

    let mut list: Vec<(i64, TopErrorStat)> = by_err
        .into_iter()
        .map(|(error, agg)| {
            (
                agg.count,
                TopErrorStat {
                    error,
                    count: agg.count,
                    last_occurred_at: agg.last_at,
                },
            )
        })
        .collect();
    list.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(list.into_iter().take(limit_value).map(|(_, v)| v).collect())
}

/// 可恢复执行列表（paused/running 实例 + 节点状态；读切点）。
pub fn list_recoverable_executions(
    conn: &Connection,
) -> Result<Vec<crate::workflow::RecoverableExecution>, AppError> {
    let mut recoverable = Vec::new();
    for inst in derive_instances(conn)? {
        if !matches!(
            inst.status,
            WorkflowInstanceStatus::Paused | WorkflowInstanceStatus::Running
        ) {
            continue;
        }
        let mut completed = Vec::new();
        let mut failed = Vec::new();
        let mut pending = Vec::new();
        for (node_id, row) in derive_node_rows(conn, &inst.id)? {
            match row.status.as_str() {
                "completed" | "success" => completed.push(node_id),
                "failed" => failed.push(node_id),
                _ => pending.push(node_id),
            }
        }
        if !failed.is_empty() || !pending.is_empty() {
            recoverable.push(crate::workflow::RecoverableExecution {
                execution: inst,
                completed_nodes: completed,
                failed_nodes: failed,
                pending_nodes: pending,
            });
        }
    }
    Ok(recoverable)
}

/// 待人工输入（running 的 interact 节点；prompt/input_type 优先事件内完整 input，
/// 事件缺失时回退到定义参数）。
pub fn get_pending_human_inputs(
    conn: &Connection,
) -> Result<Vec<crate::workflow::PendingHumanInput>, AppError> {
    let mut pending = Vec::new();
    for inst in derive_instances(conn)? {
        if !matches!(
            inst.status,
            WorkflowInstanceStatus::Running | WorkflowInstanceStatus::Paused
        ) {
            continue;
        }
        // 从定义里找 interact 节点（用于回退 prompt/input_type）。
        let def = crate::workflow::get_definition(conn, &inst.definition_id)
            .ok()
            .flatten()
            .map(|d| d.stages)
            .unwrap_or_default();
        let interact_params = |node_id: &str| -> Option<(String, String)> {
            for stage in &def {
                for node in &stage.nodes {
                    if node.id == node_id
                        && node.node_type == crate::workflow::WorkflowNodeType::Interact
                    {
                        let params = node.params.as_ref()?;
                        let prompt = params
                            .get("prompt")
                            .and_then(|p| p.as_str())
                            .unwrap_or("请输入")
                            .to_string();
                        let input_type = params
                            .get("input_type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("text")
                            .to_string();
                        return Some((prompt, input_type));
                    }
                }
            }
            None
        };
        // 节点名：界面按「阶段名·节点名」展示（阶段名由前端反查），查不到则退回节点 id
        let node_label_of = |node_id: &str| -> String {
            for stage in &def {
                for node in &stage.nodes {
                    if node.id == node_id {
                        return node.label.clone();
                    }
                }
            }
            String::new()
        };
        for (node_id, row) in derive_node_rows(conn, &inst.id)? {
            if row.status != "running" {
                continue;
            }
            // 先尝试事件 inputPreview（JSON），失败回退定义参数。
            let mut prompt = String::from("请输入");
            let mut input_type = String::from("text");
            if let Some(raw) = row.input_preview.as_deref() {
                if let Ok(v) = serde_json::from_str::<Value>(raw) {
                    if let Some(p) = v.get("prompt").and_then(|p| p.as_str()) {
                        prompt = p.to_string();
                    }
                    if let Some(t) = v.get("inputType").and_then(|t| t.as_str()) {
                        input_type = t.to_string();
                    }
                }
            }
            if let Some((dp, dt)) = interact_params(&node_id) {
                if prompt == "请输入" && row.input_preview.is_none() {
                    prompt = dp;
                }
                if input_type == "text" {
                    input_type = dt;
                }
            }
            pending.push(crate::workflow::PendingHumanInput {
                execution_id: inst.id.clone(),
                node_id: node_id.clone(),
                node_label: {
                    let label = node_label_of(&node_id);
                    if label.is_empty() {
                        node_id
                    } else {
                        label
                    }
                },
                prompt,
                input_type,
                created_at: row.started_at.unwrap_or(0),
            });
        }
    }
    Ok(pending)
}

/// 未闭环的工具审批请求（写过 `approval/requested` 但没有对应的 `approval/resolved`），
/// 且所属执行仍处于 Running / Paused。
///
/// 这些是**进程重启后的失效记录**：等待登记表随进程清空，谁也不可能再裁决，
/// 所以 `stale = true`，只用于告诉用户"执行卡在哪次审批上"（并可据此停止该执行），
/// 不作为可操作卡片——与幽灵人工输入同一判据。
pub fn list_unresolved_tool_approvals(
    conn: &Connection,
) -> Result<Vec<crate::workflow::executors::agent_executor::PendingToolApproval>, AppError> {
    let key = |execution_id: &str, call_id: &str| format!("{}:{}", execution_id, call_id);

    let mut stmt = conn.prepare(
        "SELECT execution_id, kind, payload, created_at FROM workflow_events
         WHERE kind IN ('approval/requested', 'approval/resolved') ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;

    let mut requested: Vec<crate::workflow::executors::agent_executor::PendingToolApproval> =
        Vec::new();
    let mut resolved: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows {
        let (execution_id, kind, payload, created_at) = row?;
        let Ok(v) = serde_json::from_str::<Value>(&payload) else {
            continue;
        };
        let call_id = v
            .get("callId")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();
        if call_id.is_empty() {
            continue;
        }
        if kind == "approval/resolved" {
            resolved.insert(key(&execution_id, &call_id));
            continue;
        }
        requested.push(
            crate::workflow::executors::agent_executor::PendingToolApproval {
                call_id,
                execution_id: execution_id.clone(),
                node_id: v
                    .get("nodeId")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string(),
                node_label: v
                    .get("nodeLabel")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string(),
                tool_name: v
                    .get("toolName")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string(),
                arguments: v
                    .get("arguments")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string(),
                risk: v
                    .get("risk")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string(),
                created_at,
                stale: true,
            },
        );
    }

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in requested {
        let k = key(&item.execution_id, &item.call_id);
        if resolved.contains(&k) || !seen.insert(k) {
            continue;
        }
        // 执行已结束：这次审批再无人等，也不值得展示
        if let Some(inst) = derive_instance(conn, &item.execution_id)? {
            if !matches!(
                inst.status,
                WorkflowInstanceStatus::Running | WorkflowInstanceStatus::Paused
            ) {
                continue;
            }
        }
        out.push(item);
    }
    Ok(out)
}

/// 节点执行日志（node/log 事件派生；读切点）。
pub fn get_node_execution_logs(
    conn: &Connection,
    node_execution_id: &str,
) -> Result<Vec<crate::workflow::NodeExecutionLogEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT seq, payload, created_at FROM workflow_events
         WHERE kind = 'node/log' AND json_extract(payload, '$.nodeExecutionId') = ?1
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![node_execution_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut logs = Vec::new();
    for row in rows {
        let (seq, payload, created_at) = row?;
        let v: Value = serde_json::from_str(&payload)?;
        logs.push(crate::workflow::NodeExecutionLogEntry {
            id: seq,
            execution_id: v
                .get("executionId")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string(),
            node_execution_id: node_execution_id.to_string(),
            timestamp: created_at,
            level: v
                .get("level")
                .and_then(|l| l.as_str())
                .unwrap_or("info")
                .to_string(),
            message: v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string(),
            metadata: v
                .get("metadata")
                .and_then(|m| m.as_str())
                .map(str::to_string),
        });
    }
    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eventlog::{
        append_workflow_event, insert_workflow_event_at, WORKFLOW_EVENTS_SCHEMA,
    };
    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(WORKFLOW_EVENTS_SCHEMA).unwrap();
        conn
    }

    fn append(conn: &Connection, kind: &str, payload: serde_json::Value) {
        append_workflow_event(conn, "ex1", kind, &payload, true).unwrap();
    }

    #[test]
    fn status_projection_respects_event_order_and_reset() {
        let conn = mem_conn();
        append(
            &conn,
            "node/start",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"running","timestamp":1}),
        );
        append(
            &conn,
            "node/status",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"success","timestamp":2}),
        );
        append(
            &conn,
            "node/start",
            serde_json::json!({"executionId":"ex1","nodeId":"b","status":"skipped","timestamp":3}),
        );
        append(
            &conn,
            "node/reset",
            serde_json::json!({"executionId":"ex1","nodes":["a"],"affectedRows":1}),
        );
        let statuses = derive_node_statuses(&conn, "ex1").unwrap();
        assert_eq!(statuses.get("a").map(String::as_str), Some("pending"));
        assert_eq!(statuses.get("b").map(String::as_str), Some("skipped"));
    }

    #[test]
    fn output_projection_keeps_latest_per_node() {
        let conn = mem_conn();
        append(
            &conn,
            "node/result",
            serde_json::json!({"executionId":"ex1","nodeId":"a","output":"{\"x\":1}","timestamp":1}),
        );
        append(
            &conn,
            "node/result",
            serde_json::json!({"executionId":"ex1","nodeId":"a","output":"{\"x\":2}","timestamp":2}),
        );
        let outputs = derive_node_outputs(&conn, "ex1").unwrap();
        let (output, _artifacts) = outputs.get("a").unwrap();
        assert_eq!(output.as_deref(), Some("{\"x\":2}"));
    }

    #[test]
    fn agent_session_projection_keeps_latest_non_null_per_node() {
        let conn = mem_conn();
        append(
            &conn,
            "node/status",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"running","agentSessionId":null,"timestamp":1}),
        );
        append(
            &conn,
            "node/status",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"completed","agentSessionId":"s1","timestamp":2}),
        );
        append(
            &conn,
            "node/status",
            serde_json::json!({"executionId":"ex1","nodeId":"b","status":"failed","agentSessionId":null,"timestamp":3}),
        );
        // 重跑：running 落 null 不抹掉上一次成功执行的会话。
        append(
            &conn,
            "node/status",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"running","agentSessionId":null,"timestamp":4}),
        );
        let sessions = derive_node_agent_sessions(&conn, "ex1").unwrap();
        assert_eq!(sessions.get("a").map(String::as_str), Some("s1"));
        assert_eq!(sessions.get("b"), None);
    }

    #[test]
    fn started_at_returns_latest_start_attempt() {
        let conn = mem_conn();
        append(
            &conn,
            "node/start",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"running","timestamp":100}),
        );
        append(
            &conn,
            "node/status",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"completed","timestamp":200}),
        );
        append(
            &conn,
            "node/start",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"running","timestamp":300}),
        );
        assert_eq!(node_started_at(&conn, "ex1", "a").unwrap(), Some(300));
        assert_eq!(node_started_at(&conn, "ex1", "nope").unwrap(), None);
    }

    #[test]
    fn details_fold_start_status_result_into_row() {
        let conn = mem_conn();
        append(
            &conn,
            "node/start",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"running","inputPreview":"{\"v\":1}","timestamp":100}),
        );
        append(
            &conn,
            "node/status",
            serde_json::json!({"executionId":"ex1","nodeId":"a","status":"completed","errorMessage":null,"finishedAt":200,"timestamp":200}),
        );
        append(
            &conn,
            "node/result",
            serde_json::json!({"executionId":"ex1","nodeId":"a","output":"{\"r\":2}","timestamp":200}),
        );
        let rows = derive_node_details(&conn, "ex1").unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row["nodeId"].as_str(), Some("a"));
        assert_eq!(row["status"].as_str(), Some("completed"));
        assert_eq!(row["input"]["v"].as_i64(), Some(1));
        assert_eq!(row["output"]["r"].as_i64(), Some(2));
        assert_eq!(row["startedAt"].as_i64(), Some(100));
        assert_eq!(row["finishedAt"].as_i64(), Some(200));
    }

    #[test]
    fn instance_summaries_fold_created_and_status() {
        let conn = mem_conn();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        // 实例 A：created running → status success
        push(
            "a",
            "execution/created",
            serde_json::json!({"executionId":"a","definitionId":"d1","definitionName":"D1","status":"running","startedAt":1,"createdAt":1}),
        );
        push(
            "a",
            "execution/status",
            serde_json::json!({"executionId":"a","status":"success","completionRate":1.0,"completedAt":5}),
        );
        // 实例 B：created running（无终态事件）
        push(
            "b",
            "execution/created",
            serde_json::json!({"executionId":"b","definitionId":"d2","definitionName":"D2","status":"running","startedAt":2,"createdAt":2}),
        );
        let summaries = derive_instance_summaries(&conn).unwrap();
        let by_id: std::collections::BTreeMap<&str, &InstanceSummary> = summaries
            .iter()
            .map(|s| (s.execution_id.as_str(), s))
            .collect();
        assert_eq!(by_id.len(), 2);
        assert_eq!(by_id["a"].status, "success");
        assert_eq!(by_id["a"].definition_id.as_deref(), Some("d1"));
        assert_eq!(by_id["a"].created_at, Some(1));
        assert_eq!(by_id["a"].started_at, Some(1));
        assert_eq!(by_id["a"].completed_at, Some(5));
        assert_eq!(by_id["b"].status, "running");
    }

    #[test]
    fn instance_fold_tracks_context_rate_and_terminal_reset() {
        let conn = mem_conn();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        push(
            "x",
            "execution/created",
            serde_json::json!({"executionId":"x","definitionId":"d1","definitionName":"D1","status":"running","startedAt":10,"createdAt":10}),
        );
        push(
            "x",
            "execution/progress",
            serde_json::json!({"executionId":"x","completionRate":0.5,"context":{"a":1},"timestamp":20}),
        );
        let inst = derive_instance(&conn, "x").unwrap().unwrap();
        assert_eq!(status_code(&inst.status), "running");
        assert_eq!(inst.completion_rate, 0.5);
        assert_eq!(inst.context["a"], 1);
        assert_eq!(inst.started_at, Some(10));
        assert_eq!(inst.definition_name, "D1");
        push(
            "x",
            "execution/status",
            serde_json::json!({"executionId":"x","status":"success","completionRate":1.0,"context":{"a":2},"completedAt":30,"timestamp":30}),
        );
        let inst = derive_instance(&conn, "x").unwrap().unwrap();
        assert_eq!(status_code(&inst.status), "success");
        assert_eq!(inst.completed_at, Some(30));
        assert_eq!(inst.context["a"], 2);

        // 重跑：execution/reset 重新打开终态。
        push(
            "x",
            "execution/reset",
            serde_json::json!({"executionId":"x","status":"running","timestamp":31}),
        );
        let inst = derive_instance(&conn, "x").unwrap().unwrap();
        assert_eq!(status_code(&inst.status), "running");
        assert_eq!(inst.completed_at, None);
    }

    #[test]
    fn stats_counts_statuses_durations_and_nodes() {
        let conn = mem_conn();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        let base = crate::utils::now() - 3 * 86400;
        // 实例 A：success，50s 完成，节点 n1 completed。
        push(
            "a",
            "execution/created",
            serde_json::json!({"executionId":"a","definitionId":"d1","definitionName":"D1","status":"running","startedAt":base,"createdAt":base}),
        );
        push(
            "a",
            "node/start",
            serde_json::json!({"executionId":"a","nodeId":"n1","status":"running","timestamp":base}),
        );
        push(
            "a",
            "node/status",
            serde_json::json!({"executionId":"a","nodeId":"n1","status":"completed","finishedAt":base + 50,"timestamp":base + 50}),
        );
        push(
            "a",
            "execution/status",
            serde_json::json!({"executionId":"a","status":"success","completionRate":1.0,"completedAt":base + 50,"timestamp":base + 50}),
        );
        // 实例 B：failed，70s 完成，节点 n2 failed。
        push(
            "b",
            "execution/created",
            serde_json::json!({"executionId":"b","definitionId":"d2","definitionName":"D2","status":"running","startedAt":base,"createdAt":base}),
        );
        push(
            "b",
            "node/start",
            serde_json::json!({"executionId":"b","nodeId":"n2","status":"running","timestamp":base}),
        );
        push(
            "b",
            "node/status",
            serde_json::json!({"executionId":"b","nodeId":"n2","status":"failed","errorMessage":"boom","finishedAt":base + 70,"timestamp":base + 70}),
        );
        push(
            "b",
            "execution/status",
            serde_json::json!({"executionId":"b","status":"failed","completionRate":0.5,"errorMessage":"boom","completedAt":base + 70,"timestamp":base + 70}),
        );
        let stats = get_workflow_stats(&conn, None, None).unwrap();
        assert_eq!(stats.total_executions, 2);
        assert_eq!(stats.success_count, 1);
        assert_eq!(stats.failed_count, 1);
        assert_eq!(stats.cancelled_count, 0);
        assert_eq!(stats.success_rate, 50.0);
        assert_eq!(stats.total_node_executions, 2);
        assert_eq!(stats.node_failed_count, 1);
        assert_eq!(stats.avg_duration_ms, Some(60_000.0));
        assert_eq!(stats.max_duration_ms, Some(70_000));
        assert_eq!(stats.min_duration_ms, Some(50_000));
        // 按 def 过滤只统计 D1。
        let stats_d1 = get_workflow_stats(&conn, Some("d1"), None).unwrap();
        assert_eq!(stats_d1.total_executions, 1);
        assert_eq!(stats_d1.total_node_executions, 1);
        assert_eq!(stats_d1.node_failed_count, 0);
    }

    /// 零耗时执行（同秒内完成）必须与"无样本"区分：前者是有效样本（短于 1s），后者才是空态。
    ///
    /// 事件时间戳是秒级的，不足 1 秒的执行 completedAt == startedAt → 0ms。
    /// 修复前这个 0 与"无数据"共用同一个值，最短耗时被压成 0 并被前端渲染成 "--"。
    #[test]
    fn stats_min_duration_distinguishes_zero_sample_from_no_sample() {
        let conn = mem_conn();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        let base = crate::utils::now() - 3600;
        // A：同秒完成（0ms）；B：5s 完成。
        push(
            "a",
            "execution/created",
            serde_json::json!({"executionId":"a","definitionId":"d1","definitionName":"D1","status":"running","startedAt":base,"createdAt":base}),
        );
        push(
            "a",
            "execution/status",
            serde_json::json!({"executionId":"a","status":"success","completionRate":1.0,"completedAt":base,"timestamp":base}),
        );
        push(
            "b",
            "execution/created",
            serde_json::json!({"executionId":"b","definitionId":"d1","definitionName":"D1","status":"running","startedAt":base,"createdAt":base}),
        );
        push(
            "b",
            "execution/status",
            serde_json::json!({"executionId":"b","status":"success","completionRate":1.0,"completedAt":base + 5,"timestamp":base + 5}),
        );
        let stats = get_workflow_stats(&conn, None, None).unwrap();
        assert_eq!(
            stats.min_duration_ms,
            Some(0),
            "0ms 是有效样本，不应变成空态"
        );
        assert_eq!(stats.range_min_duration_ms, Some(0));
        assert_eq!(stats.max_duration_ms, Some(5_000));
        assert_eq!(stats.avg_duration_ms, Some(2_500.0));

        // 无终态执行（无耗时样本）→ 全 None，前端显示 "--"。
        let conn2 = mem_conn();
        append_workflow_event(
            &conn2,
            "c",
            "execution/created",
            &serde_json::json!({"executionId":"c","definitionId":"d1","definitionName":"D1","status":"running","startedAt":base,"createdAt":base}),
            true,
        )
        .unwrap();
        let no_sample = get_workflow_stats(&conn2, None, None).unwrap();
        assert_eq!(no_sample.min_duration_ms, None);
        assert_eq!(no_sample.max_duration_ms, None);
        assert_eq!(no_sample.avg_duration_ms, None);
    }

    #[test]
    fn execution_timeline_buckets_by_created_day() {
        let conn = mem_conn();
        // 用显式 created_at 落两条事件，分属两个自然日，验证按天分桶。
        let now = crate::utils::now();
        insert_workflow_event_at(
            &conn,
            "a",
            "execution/created",
            &serde_json::json!({"executionId":"a","definitionId":"d1","definitionName":"D1","status":"running","startedAt":now - 86400,"createdAt":now - 86400}),
            true,
            now - 86400,
        )
        .unwrap();
        insert_workflow_event_at(
            &conn,
            "a",
            "execution/status",
            &serde_json::json!({"executionId":"a","status":"success","completionRate":1.0,"completedAt":now - 80000,"timestamp":now - 80000}),
            true,
            now - 80000,
        )
        .unwrap();
        insert_workflow_event_at(
            &conn,
            "b",
            "execution/created",
            &serde_json::json!({"executionId":"b","definitionId":"d1","definitionName":"D1","status":"running","startedAt":now - 2 * 86400,"createdAt":now - 2 * 86400}),
            true,
            now - 2 * 86400,
        )
        .unwrap();
        insert_workflow_event_at(
            &conn,
            "b",
            "execution/status",
            &serde_json::json!({"executionId":"b","status":"failed","errorMessage":"x","completedAt":now - 2 * 86400 + 10,"timestamp":now - 2 * 86400 + 10}),
            true,
            now - 2 * 86400 + 10,
        )
        .unwrap();
        let points = get_execution_timeline(&conn, None, 7).unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points.iter().map(|p| p.total).sum::<i64>(), 2);
        assert_eq!(points.iter().map(|p| p.success).sum::<i64>(), 1);
        assert_eq!(points.iter().map(|p| p.failed).sum::<i64>(), 1);
        assert!(points.iter().all(|p| p.granularity == "day"));
    }

    #[test]
    fn top_workflows_groups_and_sorts_by_total() {
        let conn = mem_conn();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        let now = crate::utils::now();
        for (id, def, name, status) in [
            ("a", "w1", "W1", "success"),
            ("b", "w1", "W1", "failed"),
            ("c", "w2", "W2", "success"),
        ] {
            push(
                id,
                "execution/created",
                serde_json::json!({"executionId":id,"definitionId":def,"definitionName":name,"status":"running","startedAt":now - 1000,"createdAt":now - 1000}),
            );
            push(
                id,
                "execution/status",
                serde_json::json!({"executionId":id,"status":status,"completionRate":1.0,"completedAt":now - 500,"timestamp":now - 500}),
            );
        }
        let top = get_top_workflows(&conn, None, Some(10), None).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].definition_id, "w1");
        assert_eq!(top[0].total, 2);
        assert_eq!(top[0].failed, 1);
        assert_eq!(top[1].definition_id, "w2");
        assert_eq!(top[1].total, 1);
    }

    #[test]
    fn top_errors_groups_failed_messages() {
        let conn = mem_conn();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        let now = crate::utils::now();
        for (id, err) in [("a", "boom A"), ("b", "boom A"), ("c", "boom B")] {
            push(
                id,
                "execution/created",
                serde_json::json!({"executionId":id,"definitionId":"d","definitionName":"D","status":"running","startedAt":now - 1000,"createdAt":now - 1000}),
            );
            push(
                id,
                "execution/status",
                serde_json::json!({"executionId":id,"status":"failed","errorMessage":err,"completionRate":0.0,"completedAt":now - 500,"timestamp":now - 500}),
            );
        }
        let top = get_top_errors(&conn, None, Some(10)).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].error, "boom A");
        assert_eq!(top[0].count, 2);
        assert_eq!(top[1].count, 1);
    }

    #[test]
    fn node_type_stats_smoke_empty_defs_ok() {
        let conn = mem_conn();
        conn.execute_batch(
            "CREATE TABLE workflow_definitions (id TEXT PRIMARY KEY, stages TEXT NOT NULL DEFAULT '[]');",
        )
        .unwrap();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        let now = crate::utils::now();
        push(
            "a",
            "execution/created",
            serde_json::json!({"executionId":"a","definitionId":"d1","definitionName":"D1","status":"success","startedAt":now - 1000,"createdAt":now - 1000}),
        );
        push(
            "a",
            "node/start",
            serde_json::json!({"executionId":"a","nodeId":"n1","status":"running","timestamp":now - 1000}),
        );
        push(
            "a",
            "node/status",
            serde_json::json!({"executionId":"a","nodeId":"n1","status":"completed","finishedAt":now - 500,"timestamp":now - 500}),
        );
        push(
            "a",
            "execution/status",
            serde_json::json!({"executionId":"a","status":"success","completionRate":1.0,"completedAt":now - 500,"timestamp":now - 500}),
        );
        // defs 表无 stages（未映射 node_type）→ 返回空，不 panic。
        let stats = get_node_type_stats(&conn, Some("d1"), None).unwrap();
        assert!(stats.is_empty());
    }

    #[test]
    fn node_type_stats_maps_types_from_definitions() {
        let conn = mem_conn();
        conn.execute_batch(
            "CREATE TABLE workflow_definitions (id TEXT PRIMARY KEY, stages TEXT NOT NULL DEFAULT '[]');",
        )
        .unwrap();
        let stages = r#"[{"id":"s1","name":"S1","order":0,"nodes":[
            {"id":"n1","type":"agent","label":"A"},
            {"id":"n2","type":"api","label":"B"}
          ],"edges":[],"gate":{"strategy":"all","mergeStrategy":"merge"}}]"#;
        conn.execute(
            "INSERT INTO workflow_definitions (id, stages) VALUES (?1, ?2)",
            params!["d1", stages],
        )
        .unwrap();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        let base = crate::utils::now() - 1000;
        push(
            "a",
            "execution/created",
            serde_json::json!({"executionId":"a","definitionId":"d1","definitionName":"D1","status":"running","startedAt":base,"createdAt":base}),
        );
        push(
            "a",
            "node/start",
            serde_json::json!({"executionId":"a","nodeId":"n1","status":"running","timestamp":base}),
        );
        push(
            "a",
            "node/start",
            serde_json::json!({"executionId":"a","nodeId":"n2","status":"running","timestamp":base}),
        );
        push(
            "a",
            "node/status",
            serde_json::json!({"executionId":"a","nodeId":"n1","status":"completed","finishedAt":base + 60,"timestamp":base + 60}),
        );
        push(
            "a",
            "node/status",
            serde_json::json!({"executionId":"a","nodeId":"n2","status":"failed","errorMessage":"e","finishedAt":base + 120,"timestamp":base + 120}),
        );
        push(
            "a",
            "execution/status",
            serde_json::json!({"executionId":"a","status":"failed","completionRate":0.5,"completedAt":base + 120,"timestamp":base + 120}),
        );
        let stats = get_node_type_stats(&conn, Some("d1"), None).unwrap();
        let by_type: std::collections::BTreeMap<String, &NodeTypeStat> =
            stats.iter().map(|s| (s.node_type.clone(), s)).collect();
        assert_eq!(by_type.len(), 2);
        assert_eq!(by_type["agent"].count, 1);
        assert_eq!(by_type["agent"].failed_count, 0);
        assert_eq!(by_type["agent"].avg_duration_ms, 60_000.0);
        assert_eq!(by_type["api"].count, 1);
        assert_eq!(by_type["api"].failed_count, 1);
        assert_eq!(by_type["api"].avg_duration_ms, 120_000.0);
    }

    #[test]
    fn node_logs_read_back_in_order() {
        let conn = mem_conn();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, false).unwrap();
        };
        push(
            "ex",
            "node/log",
            serde_json::json!({"executionId":"ex","nodeExecutionId":"ex_n1","level":"info","message":"开始","metadata":null}),
        );
        push(
            "ex",
            "node/log",
            serde_json::json!({"executionId":"ex","nodeExecutionId":"ex_n1","level":"warn","message":"重试","metadata":"{\"attempt\":2}"}),
        );
        push(
            "ex",
            "node/log",
            serde_json::json!({"executionId":"ex","nodeExecutionId":"ex_n2","level":"info","message":"其它节点","metadata":null}),
        );
        let logs = get_node_execution_logs(&conn, "ex_n1").unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].message, "开始");
        assert_eq!(logs[0].level, "info");
        assert_eq!(logs[1].message, "重试");
        assert_eq!(logs[1].level, "warn");
        assert!(logs.iter().all(|l| l.execution_id == "ex"));
        assert!(logs.iter().all(|l| l.node_execution_id == "ex_n1"));
    }

    #[test]
    fn recoverable_executions_list_running_with_pending_or_failed() {
        let conn = mem_conn();
        let push = |execution_id: &str, kind: &str, payload: serde_json::Value| {
            append_workflow_event(&conn, execution_id, kind, &payload, true).unwrap();
        };
        let base = crate::utils::now() - 1000;
        // running 实例：n1 完成、n2 失败 → 可恢复。
        push(
            "r",
            "execution/created",
            serde_json::json!({"executionId":"r","definitionId":"d1","definitionName":"D1","status":"running","startedAt":base,"createdAt":base}),
        );
        push(
            "r",
            "node/start",
            serde_json::json!({"executionId":"r","nodeId":"n1","status":"running","timestamp":base}),
        );
        push(
            "r",
            "node/status",
            serde_json::json!({"executionId":"r","nodeId":"n1","status":"completed","finishedAt":base + 10,"timestamp":base + 10}),
        );
        push(
            "r",
            "node/start",
            serde_json::json!({"executionId":"r","nodeId":"n2","status":"running","timestamp":base + 10}),
        );
        push(
            "r",
            "node/status",
            serde_json::json!({"executionId":"r","nodeId":"n2","status":"failed","errorMessage":"e","finishedAt":base + 20,"timestamp":base + 20}),
        );
        // success 实例：不参与恢复。
        push(
            "s",
            "execution/created",
            serde_json::json!({"executionId":"s","definitionId":"d2","definitionName":"D2","status":"running","startedAt":base,"createdAt":base}),
        );
        push(
            "s",
            "node/start",
            serde_json::json!({"executionId":"s","nodeId":"m1","status":"running","timestamp":base}),
        );
        push(
            "s",
            "node/status",
            serde_json::json!({"executionId":"s","nodeId":"m1","status":"completed","finishedAt":base + 10,"timestamp":base + 10}),
        );
        push(
            "s",
            "execution/status",
            serde_json::json!({"executionId":"s","status":"success","completionRate":1.0,"completedAt":base + 10,"timestamp":base + 10}),
        );
        let recoverable = list_recoverable_executions(&conn).unwrap();
        assert_eq!(recoverable.len(), 1);
        assert_eq!(recoverable[0].execution.id, "r");
        assert_eq!(recoverable[0].completed_nodes, vec!["n1".to_string()]);
        assert_eq!(recoverable[0].failed_nodes, vec!["n2".to_string()]);
    }

    #[test]
    fn list_instances_filters_and_sorts_by_created_desc() {
        let conn = mem_conn();
        let now = crate::utils::now();
        let ins = |execution_id: &str, kind: &str, payload: serde_json::Value, ts: i64| {
            insert_workflow_event_at(&conn, execution_id, kind, &payload, true, ts).unwrap();
        };
        ins(
            "old",
            "execution/created",
            serde_json::json!({"executionId":"old","definitionId":"d1","definitionName":"D1","status":"running","startedAt":now - 2000,"createdAt":now - 2000}),
            now - 2000,
        );
        ins(
            "other",
            "execution/created",
            serde_json::json!({"executionId":"other","definitionId":"d2","definitionName":"D2","status":"running","startedAt":now - 1500,"createdAt":now - 1500}),
            now - 1500,
        );
        ins(
            "new",
            "execution/created",
            serde_json::json!({"executionId":"new","definitionId":"d1","definitionName":"D1","status":"running","startedAt":now - 1000,"createdAt":now - 1000}),
            now - 1000,
        );
        ins(
            "new",
            "execution/status",
            serde_json::json!({"executionId":"new","status":"success","completionRate":1.0,"completedAt":now - 900,"timestamp":now - 900}),
            now - 900,
        );
        let all = list_instances(&conn, None).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, "new"); // created_at 降序
        assert_eq!(all[1].id, "other");
        let d1 = list_instances(&conn, Some("d1")).unwrap();
        assert_eq!(d1.len(), 2);
        assert!(d1.iter().all(|i| i.definition_id == "d1"));
    }
}
