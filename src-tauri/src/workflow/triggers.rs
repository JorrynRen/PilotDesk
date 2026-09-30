//! 工作流「事件触发」派发器。
//!
//! 触发模型（最小可用）：
//!   - 事件名取自**内置白名单**（[`BUILTIN_EVENTS`]），平台在关键节点调用 [`dispatch_event`]；
//!   - 定义属性「触发器类型 = 事件触发」+ 事件名即完成订阅（见 WorkflowPropertyDialog）；
//!   - 事件负载整体作为被触发工作流的**输入数据**，模板里可直接引用 `{{sessionId}}`、
//!     `{{definitionId}}`、`{{executionId}}` 等字段。
//!
//! 防护：
//!   - 白名单：未注册的事件名一律不派发，避免"订阅了永远不会响的名字"；
//!   - 自触发：`workflow.completed` 不触发"刚刚完成的那条定义"；
//!   - 事件链深度：[`EVENT_DEPTH_KEY`] 随负载与工作流输入传递，超过 [`MAX_EVENT_DEPTH`] 停止扩散，
//!     避免两条互相订阅的工作流互相点燃；
//!   - 用户会话过滤：只有 sessions 表里的用户会话才算"一轮生成结束"——工作流 Agent 节点的
//!     内部会话（`origin` 以 workflow 开头）与群聊参与者轮次都不参与，否则事件触发会变成噪声源。

use std::sync::Arc;

use tauri::Manager;

use super::engine::WorkflowEngine;
use super::executor::NodeExecutor;
use super::{TriggerType, WorkflowDefinition, WorkflowInstance, WorkflowInstanceStatus};
use crate::utils::errors::AppError;

/// 事件负载里记录「事件链深度」的键；由派发器写入，也随工作流输入传给下游
pub const EVENT_DEPTH_KEY: &str = "__eventDepth";

/// 事件链最大深度（同一轮事件扩散最多触发这么多层工作流）
const MAX_EVENT_DEPTH: u64 = 3;

/// 事件触发的工作流一律用这个并发上限（与定时调度保持一致）
const EVENT_RUN_CONCURRENCY: usize = 5;

/// 平台内置事件（UI 只提供这些；未在此表内的事件名永远不会被派发）
pub const BUILTIN_EVENTS: &[(&str, &str)] = &[
    ("session.completed", "会话：一轮生成结束"),
    ("workflow.completed", "工作流：执行成功"),
    ("workflow.failed", "工作流：执行失败"),
];

/// 供前端下拉展示的内置事件清单
pub fn builtin_events() -> Vec<serde_json::Value> {
    BUILTIN_EVENTS
        .iter()
        .map(|(name, label)| serde_json::json!({ "name": name, "label": label }))
        .collect()
}

pub fn is_builtin_event(name: &str) -> bool {
    BUILTIN_EVENTS.iter().any(|(n, _)| *n == name)
}

fn event_depth(payload: &serde_json::Value) -> u64 {
    payload
        .get(EVENT_DEPTH_KEY)
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// 触发来源（随"启动帧"下发给前端，用于分级提醒：手动运行本身有即时反馈，不提醒）
pub const TRIGGER_MANUAL: &str = "manual";
pub const TRIGGER_CRON: &str = "cron";
pub const TRIGGER_EVENT: &str = "event";

/// 发一条与终态同形的执行进度帧（`engine.rs` 的 `emit_progress` 是私有的，这里复刻同形结构）。
///
/// 完整执行原本只在**结束**时发 `execution.status`，起始帧只有断点执行路径才有——于是
/// "定时/事件触发的工作流已经跑起来了"前端毫无感知（卡片没有运行中、也没有任何提示）。
/// `trigger_kind` 让前端能区分手动与自动触发，据此决定是否提醒。调度器发失败帧时也复用这里。
pub fn emit_execution_frame(
    app: &tauri::AppHandle,
    def: &WorkflowDefinition,
    execution_id: &str,
    status: &str,
    trigger_kind: &str,
    error: Option<&str>,
) {
    use tauri::Emitter;
    let mut execution = serde_json::json!({
        "status": status,
        "definition_name": def.name,
    });
    if let Some(e) = error {
        execution["error"] = serde_json::json!(e);
    }
    let _ = app.emit(
        "workflow:execution-progress",
        serde_json::json!({
            "execution_id": execution_id,
            "definition_id": def.id,
            "mode": "full",
            "trigger_kind": trigger_kind,
            "execution": execution,
        }),
    );
}

/// 顶层执行一个工作流：起始帧 + 执行 + 终态事件派发（手动 / 定时 / 事件触发三条入口共用）。
///
/// 放在这里而不是引擎里的原因：这类事件只应代表**用户可见的一次工作流运行**，
/// 子工作流（Subflow 节点）走的是引擎内部的递归调用，不应向外派发事件。
pub async fn run_definition_top_level(
    executor: &Arc<NodeExecutor>,
    def: &WorkflowDefinition,
    execution_id: &str,
    input: serde_json::Value,
    app: &tauri::AppHandle,
    max_concurrency: usize,
    trigger_kind: &str,
) -> Result<serde_json::Value, AppError> {
    emit_execution_frame(app, def, execution_id, "running", trigger_kind, None);

    let result = WorkflowEngine::execute_with_concurrency(
        executor,
        def,
        execution_id,
        input.clone(),
        app,
        max_concurrency,
    )
    .await;

    let depth = event_depth(&input) + 1;
    match &result {
        Ok(_) => dispatch_event(
            app,
            "workflow.completed",
            serde_json::json!({
                "definitionId": def.id,
                "definitionName": def.name,
                "executionId": execution_id,
                EVENT_DEPTH_KEY: depth,
            }),
        ),
        Err(e) => {
            let text = e.to_string();
            // 取消不算失败事件：用户中止是主动行为，不该被当成"出错"去点燃下游
            let cancelled = text.contains("取消") || text.to_lowercase().contains("cancel");
            if !cancelled {
                dispatch_event(
                    app,
                    "workflow.failed",
                    serde_json::json!({
                        "definitionId": def.id,
                        "definitionName": def.name,
                        "executionId": execution_id,
                        "error": text,
                        EVENT_DEPTH_KEY: depth,
                    }),
                );
            }
        }
    }

    result
}

/// 派发一个平台事件（fire-and-forget，可在同步上下文调用）。
pub fn dispatch_event(app: &tauri::AppHandle, event_name: &str, payload: serde_json::Value) {
    if !is_builtin_event(event_name) {
        return;
    }
    let app = app.clone();
    let name = event_name.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = dispatch_event_inner(&app, &name, payload).await {
            log::error!("[WorkflowTriggers] 派发事件失败: event={}, err={}", name, e);
        }
    });
}

async fn dispatch_event_inner(
    app: &tauri::AppHandle,
    event_name: &str,
    payload: serde_json::Value,
) -> Result<(), AppError> {
    let depth = event_depth(&payload);
    if depth > MAX_EVENT_DEPTH {
        log::warn!(
            "[WorkflowTriggers] 事件链深度 {} 超过上限 {}，停止扩散: event={}",
            depth,
            MAX_EVENT_DEPTH,
            event_name
        );
        return Ok(());
    }

    let db = app
        .try_state::<crate::DbState>()
        .ok_or_else(|| AppError::External("DbState 未初始化".into()))?;
    let executor = app
        .try_state::<Arc<NodeExecutor>>()
        .ok_or_else(|| AppError::External("NodeExecutor 未初始化".into()))?
        .inner()
        .clone();

    // 只认"用户会话"：工作流 Agent 节点的内部会话（origin=workflow:*）与群聊参与者轮次
    // （会话 id 形如 `groupchat:房间:参与者`，sessions 表里没有行）都不是用户用完一轮，
    // 放它们进来会让事件触发变成噪声源。
    if event_name == "session.completed" {
        if let Some(sid) = payload.get("sessionId").and_then(|v| v.as_str()) {
            let conn = db.get_conn()?;
            if !is_user_session(&conn, sid) {
                return Ok(());
            }
        }
    }

    let matched: Vec<WorkflowDefinition> = {
        let conn = db.get_conn()?;
        super::list_definitions(&conn)?
            .into_iter()
            .filter(|d| {
                d.enabled
                    && d.trigger.trigger_type == TriggerType::Event
                    && d.trigger.event_name.as_deref() == Some(event_name)
            })
            .collect()
    };
    if matched.is_empty() {
        return Ok(());
    }

    // 自触发防护：完成事件不触发"刚刚完成的那条定义"
    let just_finished = payload.get("definitionId").and_then(|v| v.as_str());

    for def in matched {
        if just_finished == Some(def.id.as_str()) {
            log::info!(
                "[WorkflowTriggers] 跳过自触发: 工作流「{}」由自身完成事件触发",
                def.name
            );
            continue;
        }

        let instance_id = crate::utils::new_id();
        let now_ts = crate::utils::now();
        let instance = WorkflowInstance {
            id: instance_id.clone(),
            definition_id: def.id.clone(),
            definition_name: def.name.clone(),
            status: WorkflowInstanceStatus::Running,
            context: serde_json::json!({}),
            trigger: "event".to_string(),
            trigger_detail: Some(event_name.to_string()),
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
        {
            let conn = db.get_conn()?;
            super::create_instance(&conn, &instance)?;
        }

        log::info!(
            "[WorkflowTriggers] 事件命中，启动工作流: event={}, workflow={}, instance={}",
            event_name,
            def.name,
            instance_id
        );

        let exec = executor.clone();
        let app_for_run = app.clone();
        let input = payload.clone();
        tokio::spawn(async move {
            if let Err(e) = run_definition_top_level(
                &exec,
                &def,
                &instance_id,
                input,
                &app_for_run,
                EVENT_RUN_CONCURRENCY,
                TRIGGER_EVENT,
            )
            .await
            {
                log::error!("[WorkflowTriggers] 事件触发的工作流执行失败: {}", e);
            }
        });
    }

    Ok(())
}

/// 该会话是否是用户会话：sessions 表里有行、且不是工作流内部自动创建的
/// （工作流节点的会话 `origin` 形如 `workflow:{定义id}`）。
fn is_user_session(conn: &rusqlite::Connection, session_id: &str) -> bool {
    conn.query_row(
        "SELECT COALESCE(origin, '') FROM sessions WHERE id = ?1",
        rusqlite::params![session_id],
        |row| row.get::<_, String>(0),
    )
    .map(|origin| !origin.starts_with("workflow"))
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_depth_defaults_to_zero_and_reads_number() {
        assert_eq!(event_depth(&serde_json::json!({ "sessionId": "s1" })), 0);
        assert_eq!(event_depth(&serde_json::json!({ EVENT_DEPTH_KEY: 2 })), 2);
    }

    #[test]
    fn whitelist_rejects_unknown_event_names() {
        assert!(is_builtin_event("session.completed"));
        assert!(is_builtin_event("workflow.completed"));
        assert!(is_builtin_event("workflow.failed"));
        assert!(!is_builtin_event("session.created"));
        assert!(!is_builtin_event(""));
    }

    #[test]
    fn builtin_events_payload_shape() {
        let events = builtin_events();
        assert_eq!(events.len(), BUILTIN_EVENTS.len());
        for e in events {
            assert!(e.get("name").and_then(|v| v.as_str()).is_some());
            assert!(e.get("label").and_then(|v| v.as_str()).is_some());
        }
    }
}
