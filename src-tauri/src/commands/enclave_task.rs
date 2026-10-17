//! 飞地业务命令（M3：任务主链）。汇报 / 文件 / 组织知识库在后续里程碑补入本文件。
//!
//! 命令层只做薄适配：授权与领域规则全在 [`crate::enclave::tasks`] 与 [`crate::enclave::authz`]。

use std::sync::Arc;

use tauri::State;

use crate::enclave::authz;
use crate::enclave::files::{self, FileAssetView};
use crate::enclave::reports::{self, FeedbackView, ReportView};
use crate::enclave::tasks::{self, CreateTaskInput, DepsPreview, TaskView, UpdateTaskInput};
use crate::enclave::EnclaveState;

/// 任务列表（整组织，前端按 `parentTaskId` 组树）。
#[tauri::command]
pub fn enclave_task_list(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<TaskView>, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        tasks::list_tasks(c, &org_id, &actor)
    })
}

/// 单任务详情（含 `blocked` 与 `effectiveActions`）。
#[tauri::command]
pub fn enclave_task_get(
    org_id: String,
    task_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        tasks::get_task(c, &org_id, &task_id, &actor)
    })
}

/// 新建任务 / 子任务。
#[tauri::command]
pub fn enclave_task_create(
    org_id: String,
    input: CreateTaskInput,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        tasks::create_task(c, &org_id, &actor, &input)
    })
}

/// 编辑任务字段。
#[tauri::command]
pub fn enclave_task_update(
    org_id: String,
    task_id: String,
    input: UpdateTaskInput,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        tasks::update_task(c, &org_id, &actor, &task_id, &input)
    })
}

/// 状态流转（状态机 + 前置依赖校验）。
#[tauri::command]
pub fn enclave_task_set_status(
    org_id: String,
    task_id: String,
    status: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        tasks::set_status(c, &org_id, &actor, &task_id, &status)
    })
}

/// 设置前置依赖（纯 AND；DFS 环检测）。
#[tauri::command]
pub fn enclave_task_set_deps(
    org_id: String,
    task_id: String,
    blocked_by: Vec<String>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        tasks::set_deps(c, &org_id, &actor, &task_id, &blocked_by)
    })
}

/// 依赖变更预览：先看影响（成环 / 新阻塞 / 新解锁），再确认保存。
#[tauri::command]
pub fn enclave_task_deps_preview(
    org_id: String,
    task_id: String,
    blocked_by: Vec<String>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<DepsPreview, String> {
    state
        .store
        .with_conn(|c| tasks::deps_preview(c, &org_id, &task_id, &blocked_by))
}

// ════════════════════════════════════════════════════════════
// 过程记录 / 参与者
// ════════════════════════════════════════════════════════════

/// 追加一条进度说明（创建者 / 负责人 / 参与者；提交完成汇报后锁定）。
#[tauri::command]
pub fn enclave_task_progress_add(
    org_id: String,
    task_id: String,
    content: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::progress::ProgressView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        crate::enclave::progress::add_progress(c, &org_id, &actor, &task_id, &content)
    })
}

/// 过程记录列表 + 锁定状态。
#[tauri::command]
pub fn enclave_task_progress_list(
    org_id: String,
    task_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::progress::ProgressList, String> {
    state
        .store
        .with_conn(|c| crate::enclave::progress::list_progress(c, &org_id, &task_id))
}

/// 设置任务参与者（仅创建者 / 负责人）。
#[tauri::command]
pub fn enclave_task_set_collaborators(
    org_id: String,
    task_id: String,
    collaborators: Vec<String>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TaskView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        tasks::set_collaborators(c, &org_id, &actor, &task_id, &collaborators)
    })
}

// ════════════════════════════════════════════════════════════
// M4：完成汇报
// ════════════════════════════════════════════════════════════

#[tauri::command]
pub fn enclave_report_list(
    org_id: String,
    task_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<ReportView>, String> {
    state.store.with_conn(|c| reports::list_reports(c, &org_id, &task_id))
}

#[tauri::command]
pub fn enclave_report_save_draft(
    org_id: String,
    task_id: String,
    content: String,
    materials: Option<Vec<String>>,
    deliverables: Option<Vec<String>>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        reports::save_draft(
            c,
            &org_id,
            &actor,
            &task_id,
            &content,
            &materials.unwrap_or_default(),
            &deliverables.unwrap_or_default(),
        )
    })
}

#[tauri::command]
pub fn enclave_report_submit(
    org_id: String,
    task_id: String,
    content: String,
    materials: Option<Vec<String>>,
    deliverables: Option<Vec<String>>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        reports::submit_report(
            c,
            &org_id,
            &actor,
            &task_id,
            &content,
            &materials.unwrap_or_default(),
            &deliverables.unwrap_or_default(),
        )
    })
}

#[tauri::command]
pub fn enclave_report_confirm(
    org_id: String,
    task_id: String,
    report_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        reports::confirm_report(c, &org_id, &actor, &task_id, &report_id)
    })
}

#[tauri::command]
pub fn enclave_report_return(
    org_id: String,
    task_id: String,
    report_id: String,
    reason: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ReportView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        reports::return_report(c, &org_id, &actor, &task_id, &report_id, &reason)
    })
}

#[tauri::command]
pub fn enclave_report_feedback_list(
    report_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<FeedbackView>, String> {
    state.store.with_conn(|c| reports::list_feedback(c, &report_id))
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
pub fn enclave_kb_list(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::knowledge::KbView>, String> {
    state.store.with_conn(|c| {
        ensure_kb_view(c, &org_id)?;
        crate::enclave::knowledge::list_kbs(c, &org_id)
    })
}

#[tauri::command]
pub fn enclave_kb_search(
    org_id: String,
    kb_id: Option<String>,
    query: String,
    limit: Option<i64>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::knowledge::KbEntryView>, String> {
    state.store.with_conn(|c| {
        ensure_kb_view(c, &org_id)?;
        crate::enclave::knowledge::search_entries(
            c,
            &org_id,
            kb_id.as_deref(),
            &query,
            limit.unwrap_or(50),
        )
    })
}

#[tauri::command]
pub fn enclave_kb_entry_get(
    org_id: String,
    kb_id: String,
    memory_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Option<crate::enclave::knowledge::KbEntryView>, String> {
    state.store.with_conn(|c| {
        ensure_kb_view(c, &org_id)?;
        crate::enclave::knowledge::get_entry(c, &org_id, &kb_id, &memory_id)
    })
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

// ════════════════════════════════════════════════════════════
// M4：文件资产
// ════════════════════════════════════════════════════════════

#[tauri::command]
pub fn enclave_file_upload(
    org_id: String,
    task_id: String,
    src_path: String,
    report_id: Option<String>,
    progress_id: Option<String>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<FileAssetView, String> {
    state.store.with_conn(|c| {
        let actor = authz::current_actor_id(c)?;
        files::attach_file(
            c,
            &org_id,
            &actor,
            &task_id,
            &src_path,
            report_id.as_deref(),
            progress_id.as_deref(),
        )
    })
}

#[tauri::command]
pub fn enclave_task_file_list(
    org_id: String,
    task_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<FileAssetView>, String> {
    state.store.with_conn(|c| files::list_task_files(c, &org_id, &task_id))
}

/// 返回文件绝对路径，前端再调用既有的 `open_path` 打开。
#[tauri::command]
pub fn enclave_file_abs_path(
    org_id: String,
    file_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<String, String> {
    state
        .store
        .with_conn(|c| files::file_abs_path(c, &org_id, &file_id))
}

/// 团队文件根视图（§13）。
///
/// `base` = 用户选择的基础目录；`root` = 实际文件根（`<base>/enclave/files`）。
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

/// 设置团队文件基础目录（**全局设置**，与具体组织无关；仅主机 owner）：
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
            .ok_or_else(|| "仅组织所有者（主机）可修改团队文件根".to_string())?;

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
