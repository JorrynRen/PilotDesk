//! 飞地业务命令（M3：任务主链）。汇报 / 文件 / 组织知识库在后续里程碑补入本文件。
//!
//! 命令层只做薄适配：授权与领域规则全在 [`crate::enclave::tasks`] 与 [`crate::enclave::authz`]。

use std::sync::Arc;

use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use tauri::State;

use crate::enclave::authz;
use crate::enclave::files::{self, FileAssetView};
use crate::enclave::reports::{self, FeedbackView, ReportView};
use crate::enclave::tasks::{self, CreateTaskInput, DepsPreview, TaskView, UpdateTaskInput};
use crate::enclave::EnclaveState;

/// 成员端命令转发：owner 走本地库，成员经主机 WS 执行同一套领域逻辑。
///
/// `local` 闭包必须使用传入的 `actor`（成员 user id），不要自己读 `local_identity`。
async fn run_command<T, F>(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    command: &str,
    payload: serde_json::Value,
    local: F,
) -> Result<T, String>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce(&Connection, &str) -> Result<T, String>,
{
    let role = state.store.with_conn(|c| authz::role_for_org(c, org_id))?;
    match role {
        Some(authz::Role::Owner) => state.store.with_conn(|c| {
            let actor = authz::current_actor_id(c)?;
            local(c, &actor)
        }),
        Some(_) => {
            let data =
                crate::enclave::host::call_host(state, db, org_id, command, payload).await?;
            serde_json::from_value(data).map_err(|e| format!("解析主机响应失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// 任务列表（整组织，前端按 `parentTaskId` 组树）。
///
/// 主机（owner）走本地库；成员走局域网 WebSocket 实时拉取（M7，不缓存）。
#[tauri::command]
pub async fn enclave_task_list(
    db: State<'_, crate::DbState>,
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<TaskView>, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.list",
        json!({}),
        |c, actor| tasks::list_tasks(c, &org_id, actor),
    )
    .await
}

/// 单任务详情（含 `blocked` 与 `effectiveActions`）。
#[tauri::command]
pub async fn enclave_task_get(
    org_id: String,
    task_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    let payload = json!({ "taskId": task_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.get",
        payload,
        |c, actor| tasks::get_task(c, &org_id, &task_id, actor),
    )
    .await
}

/// 新建任务 / 子任务。
#[tauri::command]
pub async fn enclave_task_create(
    org_id: String,
    input: CreateTaskInput,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    let input_value =
        serde_json::to_value(&input).map_err(|e| format!("任务参数序列化失败：{}", e))?;
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.create",
        json!({ "input": input_value }),
        |c, actor| tasks::create_task(c, &org_id, actor, &input),
    )
    .await
}

/// 编辑任务字段。
#[tauri::command]
pub async fn enclave_task_update(
    org_id: String,
    task_id: String,
    input: UpdateTaskInput,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    let input_value =
        serde_json::to_value(&input).map_err(|e| format!("任务参数序列化失败：{}", e))?;
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.update",
        json!({ "taskId": task_id.clone(), "input": input_value }),
        |c, actor| tasks::update_task(c, &org_id, actor, &task_id, &input),
    )
    .await
}

/// 状态流转（状态机 + 前置依赖校验）。
#[tauri::command]
pub async fn enclave_task_set_status(
    org_id: String,
    task_id: String,
    status: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    let payload = json!({ "taskId": task_id.clone(), "status": status.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.set_status",
        payload,
        |c, actor| tasks::set_status_checked(c, &org_id, actor, &task_id, &status),
    )
    .await
}

/// 汇总确认：子任务全部收敛后，由创建者 / 负责人收口完成（容器不单独沉淀知识库）。
#[tauri::command]
pub async fn enclave_task_container_confirm(
    org_id: String,
    task_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    let payload = json!({ "taskId": task_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.container_confirm",
        payload,
        |c, actor| tasks::confirm_container(c, &org_id, actor, &task_id),
    )
    .await
}

/// 设置前置依赖（纯 AND；DFS 环检测）。
#[tauri::command]
pub async fn enclave_task_set_deps(
    org_id: String,
    task_id: String,
    blocked_by: Vec<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    let payload = json!({ "taskId": task_id.clone(), "blockedBy": &blocked_by });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.set_deps",
        payload,
        |c, actor| tasks::set_deps(c, &org_id, actor, &task_id, &blocked_by),
    )
    .await
}

/// 依赖变更预览：先看影响（成环 / 新阻塞 / 新解锁），再确认保存。
#[tauri::command]
pub async fn enclave_task_deps_preview(
    org_id: String,
    task_id: String,
    blocked_by: Vec<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<DepsPreview, String> {
    let payload = json!({ "taskId": task_id.clone(), "blockedBy": &blocked_by });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.deps_preview",
        payload,
        |c, _actor| tasks::deps_preview(c, &org_id, &task_id, &blocked_by),
    )
    .await
}

// ════════════════════════════════════════════════════════════
// 过程记录 / 参与者
// ════════════════════════════════════════════════════════════

/// 追加一条进度说明（创建者 / 负责人 / 参与者；提交完成汇报后锁定）。
///
/// 成员端先把附件上传到主机临时区，再以**令牌**随记录一次性提交；主机只接受令牌，
/// 不接受客户端指定的任意本机路径。
#[tauri::command]
pub async fn enclave_task_progress_add(
    org_id: String,
    task_id: String,
    content: String,
    attachments: Vec<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::progress::ProgressView, String> {
    let role = state.store.with_conn(|c| authz::role_for_org(c, &org_id))?;
    match role {
        Some(authz::Role::Owner) => state.store.with_conn(|c| {
            let actor = authz::current_actor_id(c)?;
            crate::enclave::progress::add_progress(
                c,
                &org_id,
                &actor,
                &task_id,
                &content,
                &attachments,
            )
        }),
        Some(_) => {
            let mut tokens = Vec::with_capacity(attachments.len());
            for path in &attachments {
                tokens.push(
                    crate::enclave::host::upload_temp_file(
                        state.inner(),
                        db.inner(),
                        &org_id,
                        path,
                    )
                    .await?,
                );
            }
            let data = crate::enclave::host::call_host(
                state.inner(),
                db.inner(),
                &org_id,
                "task.progress_add",
                json!({
                    "taskId": task_id,
                    "content": content,
                    "attachments": tokens,
                }),
            )
            .await?;
            serde_json::from_value(data).map_err(|e| format!("解析主机响应失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// 过程记录列表 + 锁定状态。
#[tauri::command]
pub async fn enclave_task_progress_list(
    org_id: String,
    task_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::progress::ProgressList, String> {
    let payload = json!({ "taskId": task_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.progress_list",
        payload,
        |c, _actor| crate::enclave::progress::list_progress(c, &org_id, &task_id),
    )
    .await
}

/// 设置任务参与者（仅创建者 / 负责人）。
#[tauri::command]
pub async fn enclave_task_set_collaborators(
    org_id: String,
    task_id: String,
    collaborators: Vec<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    let payload = json!({
        "taskId": task_id.clone(),
        "collaborators": &collaborators,
    });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.set_collaborators",
        payload,
        |c, actor| tasks::set_collaborators(c, &org_id, actor, &task_id, &collaborators),
    )
    .await
}

// ════════════════════════════════════════════════════════════
// M4：完成汇报
// ════════════════════════════════════════════════════════════

#[tauri::command]
pub async fn enclave_report_list(
    org_id: String,
    task_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<ReportView>, String> {
    let payload = json!({ "taskId": task_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "report.list",
        payload,
        |c, _actor| reports::list_reports(c, &org_id, &task_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_report_save_draft(
    org_id: String,
    task_id: String,
    content: String,
    materials: Option<Vec<String>>,
    deliverables: Option<Vec<String>>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    let payload = json!({
        "taskId": task_id.clone(),
        "content": content.clone(),
        "materials": &materials,
        "deliverables": &deliverables,
    });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "report.save_draft",
        payload,
        |c, actor| {
            reports::save_draft(
                c,
                &org_id,
                actor,
                &task_id,
                &content,
                &materials.unwrap_or_default(),
                &deliverables.unwrap_or_default(),
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_report_submit(
    org_id: String,
    task_id: String,
    content: String,
    materials: Option<Vec<String>>,
    deliverables: Option<Vec<String>>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    let payload = json!({
        "taskId": task_id.clone(),
        "content": content.clone(),
        "materials": &materials,
        "deliverables": &deliverables,
    });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "report.submit",
        payload,
        |c, actor| {
            reports::submit_report(
                c,
                &org_id,
                actor,
                &task_id,
                &content,
                &materials.unwrap_or_default(),
                &deliverables.unwrap_or_default(),
            )
        },
    )
    .await
}

/// 基于「已退回」的汇报继续修改：新建草稿并复制正文与交付物挂载，原记录保留为日志。
#[tauri::command]
pub async fn enclave_report_revise(
    org_id: String,
    task_id: String,
    report_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    let payload = json!({ "taskId": task_id.clone(), "reportId": report_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "report.revise",
        payload,
        |c, actor| reports::revise_report(c, &org_id, actor, &task_id, &report_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_report_confirm(
    org_id: String,
    task_id: String,
    report_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    let payload = json!({ "taskId": task_id.clone(), "reportId": report_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "report.confirm",
        payload,
        |c, actor| reports::confirm_report(c, &org_id, actor, &task_id, &report_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_report_return(
    org_id: String,
    task_id: String,
    report_id: String,
    reason: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    let payload = json!({
        "taskId": task_id.clone(),
        "reportId": report_id.clone(),
        "reason": reason.clone(),
    });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "report.return",
        payload,
        |c, actor| {
            reports::return_report(c, &org_id, actor, &task_id, &report_id, &reason)
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_report_feedback_list(
    org_id: String,
    report_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<FeedbackView>, String> {
    let payload = json!({ "reportId": report_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "report.feedback_list",
        payload,
        |c, _actor| reports::list_feedback(c, &report_id),
    )
    .await
}

// ════════════════════════════════════════════════════════════
// M5：组织知识库
// ════════════════════════════════════════════════════════════

/// 组织成员才可读组织知识库（默认拒绝：无快照 → 拒绝）。
fn ensure_kb_view(conn: &rusqlite::Connection, org_id: &str) -> Result<(), String> {
    authz::role_for_org(conn, org_id)?.map(|_| ()).ok_or_else(|| {
        "尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()
    })
}

#[tauri::command]
pub async fn enclave_kb_list(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::knowledge::KbView>, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.list",
        json!({}),
        |c, _actor| {
            ensure_kb_view(c, &org_id)?;
            crate::enclave::knowledge::list_kbs(c, &org_id)
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_kb_search(
    org_id: String,
    kb_id: Option<String>,
    query: String,
    limit: Option<i64>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::knowledge::KbEntryView>, String> {
    let payload = json!({
        "kbId": &kb_id,
        "query": query.clone(),
        "limit": limit,
    });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.search",
        payload,
        |c, _actor| {
            ensure_kb_view(c, &org_id)?;
            crate::enclave::knowledge::search_entries(
                c,
                &org_id,
                kb_id.as_deref(),
                &query,
                limit.unwrap_or(50),
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_kb_entry_get(
    org_id: String,
    kb_id: String,
    memory_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Option<crate::enclave::knowledge::KbEntryView>, String> {
    let payload = json!({ "kbId": kb_id.clone(), "memoryId": memory_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.entry_get",
        payload,
        |c, _actor| {
            ensure_kb_view(c, &org_id)?;
            crate::enclave::knowledge::get_entry(c, &org_id, &kb_id, &memory_id)
        },
    )
    .await
}

/// 沉淀重试（确认成功但归档失败时使用）。
#[tauri::command]
pub fn enclave_task_sediment_retry(
    org_id: String,
    task_id: String,
    report_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        crate::enclave::knowledge::retry_sediment(c, &org_id, &task_id, &report_id, &actor)
    })
}

/// 人工编辑知识条目（owner / admin）：标题、正文、标签。
#[tauri::command]
pub fn enclave_kb_entry_update(
    org_id: String,
    kb_id: String,
    memory_id: String,
    title: String,
    content: String,
    tags: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::knowledge::KbEntryView, String> {
    state.store.with_conn(|c| {
        let actor = authz::authorize(c, &org_id, authz::Action::KbEdit)?;
        crate::enclave::knowledge::update_entry(
            c,
            &org_id,
            &kb_id,
            &memory_id,
            &title,
            &content,
            &tags,
            &actor,
        )
    })
}

/// 移除知识条目（owner / admin）。
#[tauri::command]
pub fn enclave_kb_entry_remove(
    org_id: String,
    kb_id: String,
    memory_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    state.store.with_conn(|c| {
        let _actor = authz::authorize(c, &org_id, authz::Action::KbEdit)?;
        crate::enclave::knowledge::remove_entry(c, &org_id, &kb_id, &memory_id)
    })
}

/// 修改知识库信息（名称 / 描述，owner / admin）。
#[tauri::command]
pub fn enclave_kb_update(
    org_id: String,
    kb_id: String,
    name: String,
    description: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::knowledge::KbView, String> {
    state.store.with_conn(|c| {
        authz::authorize(c, &org_id, authz::Action::KbEdit)?;
        crate::enclave::knowledge::update_kb(c, &org_id, &kb_id, &name, &description)
    })
}

/// 知识库「文件」页：去重后的交付物列表（成员可读）。
#[tauri::command]
pub async fn enclave_kb_file_list(
    org_id: String,
    kb_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::knowledge::KbFileListView>, String> {
    let payload = json!({ "kbId": kb_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.file_list",
        payload,
        |c, _actor| {
            ensure_kb_view(c, &org_id)?;
            crate::enclave::knowledge::list_files(c, &org_id, &kb_id)
        },
    )
    .await
}

// ════════════════════════════════════════════════════════════
// M4：文件资产
// ════════════════════════════════════════════════════════════

#[tauri::command]
pub async fn enclave_file_upload(
    org_id: String,
    task_id: String,
    src_path: String,
    report_id: Option<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<FileAssetView, String> {
    let role = state.store.with_conn(|c| authz::role_for_org(c, &org_id))?;
    match role {
        Some(authz::Role::Owner) => state.store.with_conn(|c| {
            let actor = authz::current_actor_id(c)?;
            files::attach_file(
                c,
                &org_id,
                &actor,
                &task_id,
                &src_path,
                report_id.as_deref(),
            )
        }),
        Some(_) => {
            let data = crate::enclave::host::upload_file(
                state.inner(),
                db.inner(),
                &org_id,
                &task_id,
                &src_path,
                report_id.as_deref(),
            )
            .await?;
            serde_json::from_value(data).map_err(|e| format!("解析上传结果失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

#[tauri::command]
pub async fn enclave_file_detach(
    org_id: String,
    task_id: String,
    file_id: String,
    report_id: Option<String>,
    progress_id: Option<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    let payload = json!({
        "taskId": task_id.clone(),
        "fileId": file_id.clone(),
        "reportId": &report_id,
        "progressId": &progress_id,
    });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "file.detach",
        payload,
        |c, actor| {
            files::detach_file(
                c,
                &org_id,
                actor,
                &task_id,
                &file_id,
                report_id.as_deref(),
                progress_id.as_deref(),
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_task_file_list(
    org_id: String,
    task_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<FileAssetView>, String> {
    let payload = json!({ "taskId": task_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "file.task_list",
        payload,
        |c, _actor| files::list_task_files(c, &org_id, &task_id),
    )
    .await
}

/// 返回文件绝对路径，前端再调用既有的 `open_path` 打开。
///
/// 主机直接返回本地路径；成员先从主机下载到内容寻址缓存（sha256 校验后返回缓存路径）。
#[tauri::command]
pub async fn enclave_file_abs_path(
    org_id: String,
    file_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<String, String> {
    let role = state.store.with_conn(|c| authz::role_for_org(c, &org_id))?;
    match role {
        Some(authz::Role::Owner) => state
            .store
            .with_conn(|c| files::file_abs_path(c, &org_id, &file_id)),
        Some(_) => {
            crate::enclave::host::fetch_file_to_cache(
                state.inner(),
                db.inner(),
                &org_id,
                &file_id,
            )
            .await
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// 本机团队文件根目录视图（§13）。
///
/// `base` = 用户选择的基础目录；`root` = 本机实际文件根（`<base>/enclave/files`）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamFilesRootView {
    pub base: String,
    pub root: String,
    pub custom: bool,
    pub available: bool,
}

#[tauri::command]
pub fn enclave_files_root_get(
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamFilesRootView, String> {
    state.store.with_conn(|c| {
        let base = files::resolve_team_base(c)?;
        let root = files::resolve_team_files_root(c)?;
        let custom = c
            .query_row(
                "SELECT 1 FROM enclave_settings WHERE key = ?1",
                rusqlite::params![files::TEAM_FILES_ROOT_KEY],
                |_| Ok(()),
            )
            .is_ok();
        let available = std::fs::create_dir_all(&root).is_ok();
        Ok(TeamFilesRootView {
            base: base.to_string_lossy().to_string(),
            root: root.to_string_lossy().to_string(),
            custom,
            available,
        })
    })
}

/// 设置本机团队文件根目录的基础目录（**全局设置**，与具体组织无关；仅主机 owner）：
/// 校验 → 搬迁校验 → 落库 → 审计（审计挂在主机所属的第一个组织下）。
#[tauri::command]
pub fn enclave_files_root_set(
    path: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamFilesRootView, String> {
    use crate::enclave::authz::{current_actor_id, role_for_org, Role};
    state.store.with_conn(|c| {
        let actor = current_actor_id(c)?;
        // 全局目录属于主机：要求本机身份至少是一个组织的 owner
        let org_ids: Vec<String> = {
            let mut stmt = c
                .prepare("SELECT org_id FROM org_snapshot ORDER BY org_id")
                .map_err(|e| format!("读取组织快照失败：{}", e))?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| format!("读取组织快照失败：{}", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| format!("读取组织快照失败：{}", e))?
        };
        let host_org = org_ids
            .into_iter()
            .find(|id| matches!(role_for_org(c, id), Ok(Some(Role::Owner))))
            .ok_or_else(|| "仅组织所有者（主机）可修改本机团队文件根目录".to_string())?;

        let before = files::resolve_team_base(c)?;
        let new_base = files::validate_base(c, &path)?;
        files::set_base(c, &new_base)?;
        crate::enclave::org::audit(
            c,
            &host_org,
            &actor,
            "files.root",
            &serde_json::json!({
                "from": before.to_string_lossy(),
                "to": new_base.to_string_lossy()
            })
            .to_string(),
        )?;
        let root = files::resolve_team_files_root(c)?;
        Ok(TeamFilesRootView {
            base: new_base.to_string_lossy().to_string(),
            root: root.to_string_lossy().to_string(),
            custom: true,
            available: true,
        })
    })
}
