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
pub(crate) async fn run_command<T, F>(
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

/// 硬删除终态任务及其全部记录（创建者 / 组织管理员）。
#[tauri::command]
pub async fn enclave_task_delete(
    org_id: String,
    task_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    let payload = json!({ "taskId": task_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.delete",
        payload,
        |c, actor| tasks::delete_task(c, &org_id, actor, &task_id),
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
// 沟通层：任务讨论
// ════════════════════════════════════════════════════════════

/// 任务讨论列表（按时间正序）。
#[tauri::command]
pub async fn enclave_task_comment_list(
    org_id: String,
    task_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::comments::CommentView>, String> {
    let payload = json!({ "taskId": task_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "comment.list",
        payload,
        |c, _actor| crate::enclave::comments::list_comments(c, &org_id, &task_id),
    )
    .await
}

/// 发一条任务评论（可回复 / @ 成员）。
#[tauri::command]
pub async fn enclave_task_comment_add(
    org_id: String,
    task_id: String,
    content: String,
    reply_to_id: Option<String>,
    mentions: Option<Vec<String>>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::comments::CommentView, String> {
    let mentions = mentions.unwrap_or_default();
    let payload = json!({
        "taskId": task_id.clone(),
        "content": content.clone(),
        "replyToId": reply_to_id.clone(),
        "mentions": &mentions,
    });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "comment.add",
        payload,
        |c, actor| {
            crate::enclave::comments::add_comment(
                c,
                &org_id,
                actor,
                &task_id,
                &content,
                reply_to_id.as_deref(),
                &mentions,
            )
        },
    )
    .await
}

/// 删除评论（作者本人或组织管理者；软删）。
#[tauri::command]
pub async fn enclave_task_comment_delete(
    org_id: String,
    comment_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    let payload = json!({ "commentId": comment_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "comment.delete",
        payload,
        |c, actor| crate::enclave::comments::delete_comment(c, &org_id, actor, &comment_id),
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
    offset: Option<i64>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::knowledge::KbEntryView>, String> {
    let payload = json!({
        "kbId": &kb_id,
        "query": query.clone(),
        "limit": limit,
        "offset": offset,
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
                offset.unwrap_or(0),
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
///
/// 主机（owner）走本地库；成员转发到主机执行（知识条目只在主机侧）。
#[tauri::command]
pub async fn enclave_task_sediment_retry(
    org_id: String,
    task_id: String,
    report_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    let payload = json!({ "taskId": task_id.clone(), "reportId": report_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "task.sediment_retry",
        payload,
        |c, actor| {
            crate::enclave::knowledge::retry_sediment(c, &org_id, &task_id, &report_id, actor)
        },
    )
    .await
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

/// 解除知识条目与一份原件的关联（owner / admin；任务产出条目拒绝）。
#[tauri::command]
pub async fn enclave_kb_entry_file_detach(
    org_id: String,
    kb_id: String,
    memory_id: String,
    file_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    let payload = json!({
        "kbId": kb_id.clone(),
        "memoryId": memory_id.clone(),
        "fileId": file_id.clone(),
    });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.entry_file_detach",
        payload,
        |c, _actor| {
            authz::authorize(c, &org_id, authz::Action::KbEdit)?;
            crate::enclave::knowledge::detach_entry_file(
                c, &org_id, &kb_id, &memory_id, &file_id,
            )
        },
    )
    .await
}

/// 删除一份团队文件原件（owner / admin；仍有任务挂载或条目引用时拒绝）。
#[tauri::command]
pub async fn enclave_kb_file_delete(
    org_id: String,
    file_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    let payload = json!({ "fileId": file_id.clone() });
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.file_delete",
        payload,
        |c, _actor| {
            authz::authorize(c, &org_id, authz::Action::KbEdit)?;
            files::delete_asset(c, &org_id, &file_id)
        },
    )
    .await
}

/// 无引用文件（删除条目后留下的孤儿）。
#[tauri::command]
pub async fn enclave_kb_orphan_files(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<files::OrphanFileView>, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.orphan_files",
        json!({}),
        |c, _actor| {
            authz::authorize(c, &org_id, authz::Action::KbView)?;
            files::list_orphan_files(c, &org_id)
        },
    )
    .await
}

/// 批量清理无引用文件（owner / admin），返回删除数量。
#[tauri::command]
pub async fn enclave_kb_orphan_cleanup(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<usize, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.orphan_cleanup",
        json!({}),
        |c, _actor| {
            authz::authorize(c, &org_id, authz::Action::KbEdit)?;
            files::cleanup_orphan_files(c, &org_id)
        },
    )
    .await
}

/// 修改知识库定义（名称 / 描述 / 专属属性，owner / admin）。
#[tauri::command]
pub async fn enclave_kb_update(
    org_id: String,
    kb_id: String,
    name: String,
    description: String,
    field_schema: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::knowledge::KbView, String> {
    let payload = json!({
        "kbId": kb_id,
        "name": name,
        "description": description,
        "fieldSchema": field_schema,
    });
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.update",
        payload,
        move |c, _actor| {
            authz::authorize(c, &org_for_write, authz::Action::KbEdit)?;
            crate::enclave::knowledge::update_kb(
                c,
                &org_for_write,
                &kb_id,
                &name,
                &description,
                &field_schema,
            )
        },
    )
    .await
}

/// 新建自定义库（owner / admin）。默认库由系统自动创建，不在这里建。
#[tauri::command]
pub async fn enclave_kb_create(
    org_id: String,
    name: String,
    description: String,
    field_schema: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::knowledge::KbView, String> {
    let payload = json!({
        "name": name,
        "description": description,
        "fieldSchema": field_schema,
    });
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.create",
        payload,
        move |c, actor| {
            authz::authorize(c, &org_for_write, authz::Action::KbEdit)?;
            crate::enclave::knowledge::create_kb(
                c,
                &org_for_write,
                &name,
                &description,
                &field_schema,
                actor,
            )
        },
    )
    .await
}

/// 删除自定义库（默认库与有内容的库会被拒绝，owner / admin）。
#[tauri::command]
pub async fn enclave_kb_delete(
    org_id: String,
    kb_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    let payload = json!({ "kbId": kb_id });
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.delete",
        payload,
        move |c, _actor| {
            authz::authorize(c, &org_for_write, authz::Action::KbEdit)?;
            crate::enclave::knowledge::delete_kb(c, &org_for_write, &kb_id)
        },
    )
    .await
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

// ════════════════════════════════════════════════════════════
// 组织知识库：人工投喂与「待整理」队列
// ════════════════════════════════════════════════════════════

use std::time::Duration;

use crate::enclave::knowledge::{IngestEntryInput, KbIngestQueueView};

/// 团队库加工能力（主机算、成员读缓存）。
#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TeamIngestCapability {
    /// none | local | cloud
    pub can_process: String,
    pub provider_name: String,
    pub model: String,
    pub trust_domain: String,
    pub reason: String,
    pub pending: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamIngestOutcome {
    pub queued: bool,
    pub queue_id: String,
    pub entries: usize,
}

/// 手工整理第一步的回填内容（标题 + 正文，不落库）。
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamIngestPreview {
    pub title: String,
    pub content: String,
}

/// 手工入库 = 绕过模型直接写共享库，质量风险由管理角色兜底：
/// 与「编辑/移除条目」同档（owner / admin）。普通成员只能投喂排队。
fn require_ingest_editor(c: &Connection, org_id: &str, actor: &str) -> Result<(), String> {
    let role = authz::role_for_member(c, org_id, actor)?;
    if matches!(role, Some(authz::Role::Owner) | Some(authz::Role::Admin)) {
        Ok(())
    } else {
        Err("仅组织所有者/管理员可手工入库".to_string())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamQueueRunOutcome {
    pub done: usize,
    pub failed: usize,
    pub stopped: bool,
    pub remaining: i64,
}

fn empty_capability(reason: &str, pending: i64) -> TeamIngestCapability {
    TeamIngestCapability {
        can_process: "none".into(),
        provider_name: String::new(),
        model: String::new(),
        trust_domain: String::new(),
        reason: reason.to_string(),
        pending,
    }
}

/// 主机侧计算加工能力：Provider 档位 + 组织出境闸门。
fn compute_team_capability(
    db: &crate::DbState,
    state: &EnclaveState,
    org_id: &str,
) -> TeamIngestCapability {
    let pending = state
        .store
        .with_conn(|c| Ok(crate::enclave::knowledge::ingest_queue_pending(c, org_id)))
        .unwrap_or(0);
    let Ok(conn) = db.pool.get() else {
        return empty_capability("无法访问主库", pending);
    };
    let model = match crate::api_agent::kb_llm::resolve_kb_model(&conn) {
        Ok(m) => m,
        Err(e) => return empty_capability(&e, pending),
    };
    let trust = crate::commands::api_provider::provider_trust_domain(&conn, &model.provider_id);
    let is_internal = trust == crate::enclave::gate::TrustDomain::Internal;
    if !is_internal {
        let allow = state
            .store
            .with_conn(|c| crate::enclave::org::policy_view(c, org_id))
            .map(|p| p.allow_cloud_model)
            .unwrap_or(false);
        if !allow {
            return TeamIngestCapability {
                can_process: "none".into(),
                provider_name: model.provider_name,
                model: model.model,
                trust_domain: trust.as_str().into(),
                reason: "组织出境闸门关闭：团队数据不得发送至云端模型".into(),
                pending,
            };
        }
    }
    TeamIngestCapability {
        can_process: if is_internal { "local".into() } else { "cloud".into() },
        provider_name: model.provider_name,
        model: model.model,
        trust_domain: trust.as_str().into(),
        reason: String::new(),
        pending,
    }
}

fn cache_team_capability(state: &EnclaveState, org_id: &str, cap: &TeamIngestCapability) {
    let json = serde_json::to_string(cap).unwrap_or_default();
    let _ = state.store.with_conn(|c| {
        crate::enclave::knowledge::save_ingest_capability(c, org_id, &json)
    });
}

#[tauri::command]
pub async fn enclave_kb_ingest_capability(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamIngestCapability, String> {
    match state.store.with_conn(|c| authz::role_for_org(c, &org_id))? {
        Some(authz::Role::Owner) => {
            let cap = compute_team_capability(db.inner(), state.inner(), &org_id);
            cache_team_capability(state.inner(), &org_id, &cap);
            Ok(cap)
        }
        Some(_) => {
            let cached = state.store.with_conn(|c| {
                Ok(crate::enclave::knowledge::load_ingest_capability(c, &org_id))
            })?;
            let pending = state.store.with_conn(|c| {
                Ok(crate::enclave::knowledge::ingest_queue_pending(c, &org_id))
            })?;
            let mut cap = cached
                .and_then(|json| serde_json::from_str::<TeamIngestCapability>(&json).ok())
                .unwrap_or_else(|| empty_capability("主机尚未上报加工能力", pending));
            cap.pending = pending;
            Ok(cap)
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

#[tauri::command]
pub async fn enclave_kb_queue_list(
    org_id: String,
    kb_id: Option<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<KbIngestQueueView>, String> {
    let payload = json!({ "kbId": kb_id });
    let kb_for_query = kb_id.clone();
    let org_for_query = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.queue.list",
        payload,
        move |c, _actor| {
            crate::enclave::knowledge::list_ingest_queue(
                c,
                &org_for_query,
                kb_for_query.as_deref(),
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_kb_queue_retry(
    org_id: String,
    queue_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    let payload = json!({ "queueId": queue_id });
    let qid = queue_id.clone();
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.queue.retry",
        payload,
        move |c, _actor| crate::enclave::knowledge::retry_ingest(c, &org_for_write, &qid),
    )
    .await
}

#[tauri::command]
pub async fn enclave_kb_queue_drop(
    org_id: String,
    queue_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<bool, String> {
    let payload = json!({ "queueId": queue_id });
    let qid = queue_id.clone();
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.queue.drop",
        payload,
        move |c, _actor| crate::enclave::knowledge::drop_ingest(c, &org_for_write, &qid),
    )
    .await
}

#[tauri::command]
pub async fn enclave_kb_queue_run(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamQueueRunOutcome, String> {
    match state.store.with_conn(|c| authz::role_for_org(c, &org_id))? {
        Some(authz::Role::Owner) => {
            drain_team_queue(db.inner(), state.inner(), &org_id, 10).await
        }
        Some(_) => {
            // 成员无法在本地执行（不缓存团队数据）：主机后台循环会接手
            let remaining = state.store.with_conn(|c| {
                Ok(crate::enclave::knowledge::ingest_queue_pending(c, &org_id))
            })?;
            Ok(TeamQueueRunOutcome {
                done: 0,
                failed: 0,
                stopped: true,
                remaining,
            })
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// 手工整理第一步：把队列项的原文提取出来回填表单（只读，不写库）。
///
/// 提取失败（如网址还没抓到原文）不阻断流程：前端仍让用户手工填写后提交。
#[tauri::command]
pub async fn enclave_kb_ingest_preview(
    org_id: String,
    queue_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamIngestPreview, String> {
    let payload = json!({ "queueId": queue_id });
    let qid = queue_id.clone();
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.ingest.preview",
        payload,
        move |c, actor| {
            require_ingest_editor(c, &org_for_write, actor)?;
            let row = crate::enclave::knowledge::get_ingest(c, &org_for_write, &qid)?;
            let p: serde_json::Value =
                serde_json::from_str(&row.payload).unwrap_or(serde_json::Value::Null);
            let (title, content, _) = crate::enclave::knowledge::manual_ingest_text(
                c,
                &org_for_write,
                &row.kind,
                &p,
            )?;
            Ok(TeamIngestPreview { title, content })
        },
    )
    .await
}

/// 手工整理第二步：用户确认（标题/正文/标签可改）后写入组织库。
#[tauri::command]
pub async fn enclave_kb_ingest_commit(
    org_id: String,
    queue_id: String,
    title: String,
    content: String,
    tags: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamIngestOutcome, String> {
    let payload = json!({ "queueId": queue_id, "title": title, "content": content, "tags": tags });
    let qid = queue_id.clone();
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.ingest.commit",
        payload,
        move |c, actor| {
            require_ingest_editor(c, &org_for_write, actor)?;
            let row = crate::enclave::knowledge::get_ingest(c, &org_for_write, &qid)?;
            crate::enclave::knowledge::commit_manual_ingest(
                c,
                &org_for_write,
                &row,
                &title,
                &content,
                &tags,
                actor,
            )?;
            Ok(TeamIngestOutcome {
                queued: false,
                queue_id: row.id,
                entries: 1,
            })
        },
    )
    .await
}

/// 团队库文件投喂：主机直接读本机路径；成员先把文件经 WS 上传到主机临时区，再由主机导入。
#[tauri::command]
pub async fn enclave_kb_ingest_file(
    org_id: String,
    kb_id: Option<String>,
    path: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamIngestOutcome, String> {
    let role = state.store.with_conn(|c| authz::role_for_org(c, &org_id))?;
    let kb = match kb_id {
        Some(k) => k,
        None => state.store.with_conn(|c| {
            crate::enclave::knowledge::ensure_org_kb(c, &org_id, "")
        })?,
    };
    match role {
        Some(authz::Role::Owner) => {
            let imported = state
                .store
                .with_conn(|c| files::import_local_file(c, &org_id, &path))?;
            let payload = json!({
                "fileId": imported.file_id,
                "fileName": imported.file_name,
                "relPath": imported.rel_path,
            });
            let queue_id = state.store.with_conn(|c| {
                crate::enclave::knowledge::enqueue_ingest(
                    c,
                    &org_id,
                    &kb,
                    "file",
                    &imported.rel_path,
                    &payload.to_string(),
                )
            })?;
            let run = drain_team_queue(db.inner(), state.inner(), &org_id, 1).await?;
            Ok(TeamIngestOutcome {
                queued: run.done == 0,
                queue_id,
                entries: run.done,
            })
        }
        Some(_) => {
            let file_name = std::path::Path::new(&path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
                .to_string();
            let token = crate::enclave::host::upload_temp_file(
                state.inner(),
                db.inner(),
                &org_id,
                &path,
            )
            .await?;
            let data = crate::enclave::host::call_host(
                state.inner(),
                db.inner(),
                &org_id,
                "kb.ingest.file",
                json!({ "kbId": kb, "token": token, "fileName": file_name }),
            )
            .await?;
            serde_json::from_value(data).map_err(|e| format!("解析主机响应失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// AI 生成草稿（落「待确认」，人工采纳后才进库）。
///
/// 生成只把**主题**发给模型，不携带团队数据，因此不受出境闸门约束（与个人库同一语义）。
/// 模型在本机解析：成员用自己的 Provider 生成，结果再交给主机入库。
#[tauri::command]
pub async fn enclave_kb_generate(
    org_id: String,
    kb_id: Option<String>,
    topic: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::knowledge::KbCandidateView, String> {
    let topic = topic.trim().to_string();
    if topic.is_empty() {
        return Err("请先写一个主题或要求".into());
    }
    let kb = match kb_id {
        Some(k) => k,
        None => state.store.with_conn(|c| {
            crate::enclave::knowledge::ensure_org_kb(c, &org_id, "")
        })?,
    };
    let (kb_name, fields_json) = state.store.with_conn(|c| {
        crate::enclave::knowledge::kb_meta(c, &org_id, &kb)
    })?;
    let model = {
        let conn = db.pool.get().map_err(|e| format!("取主库连接失败：{}", e))?;
        crate::api_agent::kb_llm::resolve_kb_model(&conn)
            .map_err(|e| format!("没有可用模型：{}", e))?
    };
    let draft = crate::api_agent::kb_llm::generate_draft(&model, &topic, &kb_name, &fields_json)
        .await
        .map_err(|e| format!("生成失败：{}", e))?;
    if let Ok(conn) = db.pool.get() {
        crate::commands::knowledge::record_kb_usage_conn(
            &conn,
            &format!("enclave:{}", org_id),
            &model,
        );
    }
    let title = if draft.title.trim().is_empty() {
        let t: String = topic.chars().take(40).collect();
        if t.is_empty() {
            crate::api_agent::knowledge::derive_title(&draft.content)
        } else {
            t
        }
    } else {
        draft.title
    };
    let metadata = serde_json::to_string(&draft.meta).unwrap_or_else(|_| "{}".into());
    let tags = draft.tags.join(",");
    let org_for_write = org_id.clone();
    let kb_for_write = kb.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.candidate.add",
        json!({
            "kbId": kb, "title": &title, "content": &draft.content,
            "tags": &tags, "metadata": &metadata, "origin": "ai",
        }),
        move |c, actor| {
            crate::enclave::knowledge::add_candidate(
                c,
                &org_for_write,
                &kb_for_write,
                actor,
                &title,
                &draft.content,
                &tags,
                &metadata,
                "ai",
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_kb_candidate_list(
    org_id: String,
    kb_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::knowledge::KbCandidateView>, String> {
    let payload = json!({ "kbId": kb_id });
    let kb_for_query = kb_id.clone();
    let org_for_query = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.candidate.list",
        payload,
        move |c, _actor| {
            crate::enclave::knowledge::list_candidates(c, &org_for_query, &kb_for_query)
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_kb_candidate_adopt(
    org_id: String,
    kb_id: String,
    candidate_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::knowledge::KbEntryView, String> {
    let payload = json!({ "kbId": kb_id, "candidateId": candidate_id });
    let kb_for_write = kb_id.clone();
    let cand = candidate_id.clone();
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.candidate.adopt",
        payload,
        move |c, actor| {
            crate::enclave::knowledge::adopt_candidate(
                c,
                &org_for_write,
                &kb_for_write,
                &cand,
                actor,
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_kb_candidate_reject(
    org_id: String,
    kb_id: String,
    candidate_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<bool, String> {
    let payload = json!({ "kbId": kb_id, "candidateId": candidate_id });
    let kb_for_write = kb_id.clone();
    let cand = candidate_id.clone();
    let org_for_write = org_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "kb.candidate.reject",
        payload,
        move |c, _actor| {
            crate::enclave::knowledge::reject_candidate(c, &org_for_write, &kb_for_write, &cand)
        },
    )
    .await
}

/// 片段投喂：片段是"用户已经拿到的文本"，入库不依赖模型；整理才依赖。
#[tauri::command]
pub async fn enclave_kb_ingest_snippet(
    org_id: String,
    kb_id: Option<String>,
    title: String,
    text: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamIngestOutcome, String> {
    if text.trim().is_empty() {
        return Err("片段内容为空".into());
    }
    let source_ref = format!(
        "snippet:{}",
        &crate::api_agent::knowledge::sha256_hex(text.as_bytes())[..16]
    );
    let payload = json!({ "title": title, "text": text });
    let role = state.store.with_conn(|c| authz::role_for_org(c, &org_id))?;
    let kb = match kb_id {
        Some(k) => k,
        None => state.store.with_conn(|c| {
            crate::enclave::knowledge::ensure_org_kb(c, &org_id, "")
        })?,
    };
    match role {
        Some(authz::Role::Owner) => {
            let queue_id = state.store.with_conn(|c| {
                crate::enclave::knowledge::enqueue_ingest(
                    c,
                    &org_id,
                    &kb,
                    "snippet",
                    &source_ref,
                    &payload.to_string(),
                )
            })?;
            let run = drain_team_queue(db.inner(), state.inner(), &org_id, 1).await?;
            Ok(TeamIngestOutcome {
                queued: run.done == 0,
                queue_id,
                entries: run.done,
            })
        }
        Some(_) => {
            let data = crate::enclave::host::call_host(
                state.inner(),
                db.inner(),
                &org_id,
                "kb.ingest.snippet",
                json!({ "kbId": kb, "sourceRef": source_ref, "payload": payload }),
            )
            .await?;
            serde_json::from_value(data).map_err(|e| format!("解析主机响应失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

#[tauri::command]
pub async fn enclave_kb_ingest_url(
    org_id: String,
    kb_id: Option<String>,
    url: String,
    name: Option<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TeamIngestOutcome, String> {
    let target = url.trim().to_string();
    if !(target.starts_with("http://") || target.starts_with("https://")) {
        return Err("只支持 http/https 地址".into());
    }
    let source_ref = format!(
        "url:{}",
        &crate::api_agent::knowledge::sha256_hex(target.as_bytes())[..16]
    );
    let payload = json!({ "url": target, "name": name });
    let role = state.store.with_conn(|c| authz::role_for_org(c, &org_id))?;
    let kb = match kb_id {
        Some(k) => k,
        None => state.store.with_conn(|c| {
            crate::enclave::knowledge::ensure_org_kb(c, &org_id, "")
        })?,
    };
    match role {
        Some(authz::Role::Owner) => {
            let queue_id = state.store.with_conn(|c| {
                crate::enclave::knowledge::enqueue_ingest(
                    c,
                    &org_id,
                    &kb,
                    "url",
                    &source_ref,
                    &payload.to_string(),
                )
            })?;
            let run = drain_team_queue(db.inner(), state.inner(), &org_id, 1).await?;
            Ok(TeamIngestOutcome {
                queued: run.done == 0,
                queue_id,
                entries: run.done,
            })
        }
        Some(_) => {
            let data = crate::enclave::host::call_host(
                state.inner(),
                db.inner(),
                &org_id,
                "kb.ingest.url",
                json!({ "kbId": kb, "sourceRef": source_ref, "payload": payload }),
            )
            .await?;
            serde_json::from_value(data).map_err(|e| format!("解析主机响应失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// 网址投喂的「原文落盘」：抓页面 → 本地清洗成 markdown → 存成团队文件资产。
///
/// 与个人知识库同一口径：原文入库**不依赖模型**。模型不可用时也能在「文件」页看原文、
/// 也能由 owner / admin 手工入库；后续整理读这份快照，页面改版或下线都不影响已投喂内容。
///
/// 只处理 `pending` 且还没有 `fileId` 的行。抓取失败会把原因记进 payload（`snapshotError`），
/// 之后不再自动重抓 —— 否则一个挂掉的站点会被每轮后台循环反复请求；界面上的「重试」会清掉它。
async fn snapshot_pending_urls(
    state: &EnclaveState,
    org_id: &str,
    max: usize,
) -> Result<usize, String> {
    if max == 0 {
        return Ok(0);
    }
    let rows = state.store.with_conn(|c| {
        crate::enclave::knowledge::list_ingest_queue(c, org_id, None)
    })?;
    let mut done = 0usize;
    let mut tried = 0usize;
    for row in rows {
        // `max` 计的是**尝试次数**：抓取失败的站点也必须占额度，
        // 否则一次 drain 会把队列里所有失败地址依次请求一遍。
        if tried >= max {
            break;
        }
        if row.kind != "url" || row.status != "pending" {
            continue;
        }
        let payload: serde_json::Value =
            serde_json::from_str(&row.payload).unwrap_or(serde_json::Value::Null);
        let Some(obj) = payload.as_object() else { continue };
        let has_file = obj
            .get("fileId")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if has_file || obj.contains_key("snapshotError") {
            continue;
        }
        let url = obj
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            continue;
        }
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        tried += 1;
        let fetched: Result<(String, crate::api_agent::knowledge::IngestPrep), String> = async {
            let html = crate::commands::knowledge::load_page(&url).await?;
            let text = crate::tools::browser::html_to_markdown(&html);
            let title = name
                .unwrap_or_else(|| crate::commands::knowledge::extract_title(&html, &url));
            let prep = crate::api_agent::knowledge::prepare_web(&title, &text)?;
            Ok((title, prep))
        }
        .await;

        let mut next = payload.clone();
        match fetched {
            Ok((title, prep)) => {
                let imported = state.store.with_conn(|c| {
                    files::import_bytes(c, org_id, &prep.name, &prep.write_bytes)
                })?;
                if let Some(o) = next.as_object_mut() {
                    o.insert("fileId".into(), json!(imported.file_id));
                    o.insert("fileName".into(), json!(imported.file_name));
                    o.insert("relPath".into(), json!(imported.rel_path));
                    o.insert("title".into(), json!(title));
                }
                state.store.with_conn(|c| {
                    crate::enclave::knowledge::update_ingest_payload(
                        c,
                        org_id,
                        &row.id,
                        &next.to_string(),
                    )
                })?;
                done += 1;
            }
            Err(e) => {
                log::warn!("[Enclave][KB] 网址原文落盘失败（{}）：{}", url, e);
                if let Some(o) = next.as_object_mut() {
                    o.insert("snapshotError".into(), json!(e));
                }
                let _ = state.store.with_conn(|c| {
                    crate::enclave::knowledge::update_ingest_payload(
                        c,
                        org_id,
                        &row.id,
                        &next.to_string(),
                    )
                });
            }
        }
    }
    Ok(done)
}

/// 主机侧消费队列（单消费者，按入队顺序）。
async fn drain_team_queue(
    db: &crate::DbState,
    state: &EnclaveState,
    org_id: &str,
    max: usize,
) -> Result<TeamQueueRunOutcome, String> {
    let _ = state.store.with_conn(|c| {
        crate::enclave::knowledge::reset_running_ingest(c, org_id);
        Ok(())
    });
    // 网址先落原文（与个人库一致）：模型是否可用都要先有原文，后续整理只读这份快照
    if let Err(e) = snapshot_pending_urls(state, org_id, 3).await {
        log::warn!("[Enclave][KB] 网址原文落盘异常（{}）：{}", org_id, e);
    }
    let cap = compute_team_capability(db, state, org_id);
    if cap.can_process == "none" {
        return Ok(TeamQueueRunOutcome {
            done: 0,
            failed: 0,
            stopped: true,
            remaining: cap.pending,
        });
    }
    let mut done = 0usize;
    let mut failed = 0usize;
    let mut stopped = false;
    for _ in 0..max {
        let row = state.store.with_conn(|c| {
            crate::enclave::knowledge::claim_next_ingest(c, org_id)
        })?;
        let Some(row) = row else { break };
        match process_team_queue_row(db, state, org_id, &row).await {
            Ok(n) => {
                let _ = state.store.with_conn(|c| {
                    crate::enclave::knowledge::finish_ingest(c, &row.id);
                    Ok(())
                });
                done += n;
            }
            Err(e) => {
                log::warn!("[Enclave][KB] 待整理失败（{} {}）：{}", row.kind, row.id, e);
                let _ = state.store.with_conn(|c| {
                    crate::enclave::knowledge::fail_ingest(c, &row.id, &e);
                    Ok(())
                });
                failed += 1;
                if e.contains("模型") || e.contains("Provider") || e.contains("闸门") {
                    stopped = true;
                    break;
                }
            }
        }
    }
    let remaining = state.store.with_conn(|c| {
        Ok(crate::enclave::knowledge::ingest_queue_pending(c, org_id))
    })?;
    Ok(TeamQueueRunOutcome {
        done,
        failed,
        stopped,
        remaining,
    })
}

async fn process_team_queue_row(
    db: &crate::DbState,
    state: &EnclaveState,
    org_id: &str,
    row: &KbIngestQueueView,
) -> Result<usize, String> {
    let payload: serde_json::Value =
        serde_json::from_str(&row.payload).unwrap_or(serde_json::Value::Null);
    // 文件投喂：条目写入后要把源文件挂上（条目可溯源、文件页可见）
    let file_id_opt = payload
        .get("fileId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let (kb_name, fields_json) = state.store.with_conn(|c| {
        crate::enclave::knowledge::kb_meta(c, org_id, &row.kb_id)
    })?;
    let model = {
        let conn = db.pool.get().map_err(|e| format!("取主库连接失败：{}", e))?;
        crate::api_agent::kb_llm::resolve_kb_model(&conn).map_err(|e| format!("模型不可用：{}", e))?
    };
    let (trust, actor) = {
        let conn = db.pool.get().map_err(|e| format!("取主库连接失败：{}", e))?;
        let trust = crate::commands::api_provider::provider_trust_domain(&conn, &model.provider_id);
        drop(conn);
        let actor = state
            .store
            .with_conn(|c| authz::current_actor_id(c))
            .unwrap_or_default();
        (trust, actor)
    };
    if trust == crate::enclave::gate::TrustDomain::Cloud {
        let allow = state
            .store
            .with_conn(|c| crate::enclave::org::policy_view(c, org_id))
            .map(|p| p.allow_cloud_model)
            .unwrap_or(false);
        if !allow {
            return Err("组织出境闸门关闭，团队数据不得发送至云端模型".into());
        }
    }

    let entries: Vec<IngestEntryInput> = match row.kind.as_str() {
        "snippet" => {
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "待整理记录缺少片段正文".to_string())?;
            let title = payload.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let draft = crate::api_agent::kb_llm::process_snippet(
                &model,
                text,
                &kb_name,
                &fields_json,
                title,
            )
            .await
            .map_err(|e| format!("模型整理未生效：{}", e))?;
            let digest = &crate::api_agent::knowledge::sha256_hex(text.as_bytes())[..16];
            vec![IngestEntryInput {
                memory_id: format!("manual:{}", digest),
                title: if draft.title.trim().is_empty() {
                    crate::api_agent::knowledge::derive_title(text)
                } else {
                    draft.title
                },
                content: draft.content,
                tags: draft.tags.join(","),
                metadata: serde_json::to_string(&draft.meta).unwrap_or_else(|_| "{}".into()),
            }]
        }
        "file" => {
            // 只认 file_assets（fileId）：投喂后文件可能被重命名/移动，relPath 只是快照。
            let file_id = payload
                .get("fileId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let abs = if !file_id.is_empty() {
                state.store.with_conn(|c| {
                    files::file_meta(c, org_id, file_id).map(|m| m.abs_path)
                })?
            } else {
                let rel = payload
                    .get("relPath")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "待整理记录缺少文件路径".to_string())?;
                state.store.with_conn(|c| {
                    Ok(files::org_dir(c, org_id)?
                        .join(rel)
                        .to_string_lossy()
                        .to_string())
                })?
            };
            let prep = crate::api_agent::knowledge::prepare_file(
                std::path::Path::new(&abs),
                false,
            )?;
            if prep.blocks.is_empty() {
                return Err("该格式暂不支持抽取正文（原文已入库）".into());
            }
            let blocks = crate::api_agent::knowledge::merge_small_blocks(
                &prep.text,
                &prep.blocks,
                crate::api_agent::knowledge::MIN_MODEL_BLOCK,
            );
            let block_texts: Vec<String> = blocks.iter().map(|b| b.text.clone()).collect();
            let (doc_title, plans) = crate::api_agent::kb_llm::plan_chunks(
                &model,
                &block_texts,
                &kb_name,
                &fields_json,
                &prep.name,
                false,
            )
            .await
            .map_err(|e| format!("模型整理未生效：{}", e))?;
            let (chunks, _truncated) = crate::api_agent::knowledge::assemble_plans(
                &prep, &blocks, &plans,
            );
            let digest = &crate::api_agent::knowledge::sha256_hex(prep.sha.as_bytes())[..16];
            chunks
                .iter()
                .enumerate()
                .map(|(i, c)| IngestEntryInput {
                    memory_id: format!("manual:{}:{}", digest, i),
                    title: if c.title.trim().is_empty() {
                        format!("{} · {}", doc_title, i + 1)
                    } else {
                        c.title.clone()
                    },
                    content: c.text.clone(),
                    tags: c.tags.clone(),
                    metadata: if c.meta_json.trim().is_empty() {
                        "{}".into()
                    } else {
                        c.meta_json.clone()
                    },
                })
                .collect()
        }
        "url" => {
            let url = payload
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "待整理记录缺少网址".to_string())?;
            let name = payload.get("name").and_then(|v| v.as_str());
            // 投喂时已把原文快照存成团队文件：整理只读这份快照，页面改版/下线都不影响
            let (page_title, prep) = if let Some(fid) = file_id_opt.as_deref() {
                let meta = state.store.with_conn(|c| files::file_meta(c, org_id, fid))?;
                let text = std::fs::read_to_string(&meta.abs_path)
                    .map_err(|e| format!("读取网址原文快照失败：{}", e))?;
                let title = payload
                    .get("title")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| name.map(str::to_string))
                    .unwrap_or_else(|| meta.file_name.trim_end_matches(".md").to_string());
                let prep = crate::api_agent::knowledge::prepare_web(&title, &text)?;
                (title, prep)
            } else {
                let html = crate::commands::knowledge::load_page(url)
                    .await
                    .map_err(|e| format!("抓取网页失败：{}", e))?;
                let title = name
                    .map(str::to_string)
                    .unwrap_or_else(|| crate::commands::knowledge::extract_title(&html, url));
                let text = crate::tools::browser::html_to_markdown(&html);
                if text.trim().is_empty() {
                    return Err("页面没有可提取的正文（可能是纯 JS 渲染的站点）".into());
                }
                let prep = crate::api_agent::knowledge::prepare_web(&title, &text)?;
                (title, prep)
            };
            let blocks = crate::api_agent::knowledge::merge_small_blocks(
                &prep.text,
                &prep.blocks,
                crate::api_agent::knowledge::MIN_MODEL_BLOCK,
            );
            let block_texts: Vec<String> = blocks.iter().map(|b| b.text.clone()).collect();
            let (doc_title, sections) = crate::api_agent::kb_llm::organize_page(
                &model,
                &block_texts,
                &page_title,
                &kb_name,
                &fields_json,
                url,
            )
            .await
            .map_err(|e| format!("模型整理未生效：{}", e))?;
            let digest = &crate::api_agent::knowledge::sha256_hex(prep.sha.as_bytes())[..16];
            sections
                .iter()
                .enumerate()
                .map(|(i, s)| IngestEntryInput {
                    memory_id: format!("manual:{}:{}", digest, i),
                    title: if s.title.trim().is_empty() {
                        format!("{} · {}", doc_title, i + 1)
                    } else {
                        s.title.clone()
                    },
                    content: s.content.clone(),
                    tags: s.tags.join(","),
                    metadata: serde_json::to_string(&s.meta).unwrap_or_else(|_| "{}".into()),
                })
                .collect()
        }
        other => return Err(format!("不支持的待整理类型：{}", other)),
    };

    if let Ok(conn) = db.pool.get() {
        crate::commands::knowledge::record_kb_usage_conn(
            &conn,
            &format!("enclave:{}", org_id),
            &model,
        );
    }
    let origin = match row.kind.as_str() {
        "snippet" => "manual",
        "file" => "file",
        _ => "url",
    };
    state.store.with_conn(|c| {
        crate::enclave::knowledge::write_ingest_entries(
            c,
            org_id,
            &row.kb_id,
            origin,
            &row.source_ref,
            &actor,
            &entries,
            file_id_opt.as_deref(),
        )
    })
}

/// 主机后台消费循环（只处理本机承载的组织）。
pub async fn run_enclave_kb_queue_loop(db: crate::DbState, state: Arc<EnclaveState>) {
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        let hosts = state
            .store
            .with_conn(|c| {
                crate::enclave::org::host_org_ids(c, &crate::enclave::host::device_id())
            })
            .unwrap_or_default();
        for org_id in hosts {
            let cap = compute_team_capability(&db, &state, &org_id);
            cache_team_capability(&state, &org_id, &cap);
            if cap.pending > 0 {
                // 网址原文落盘不依赖模型：无模型时也要先有原文（文件页可见、可手工入库）
                let _ = snapshot_pending_urls(&state, &org_id, 3).await;
            }
            if cap.can_process != "none" && cap.pending > 0 {
                match drain_team_queue(&db, &state, &org_id, 3).await {
                    Ok(o) if o.done > 0 || o.failed > 0 => log::info!(
                        "[Enclave][KB] 待整理消费（{}）：完成 {}，失败 {}，剩余 {}",
                        org_id,
                        o.done,
                        o.failed,
                        o.remaining
                    ),
                    Ok(_) => {}
                    Err(e) => log::warn!("[Enclave][KB] 待整理消费异常（{}）：{}", org_id, e),
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(120)).await;
    }
}
