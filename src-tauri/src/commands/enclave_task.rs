//! 飞地业务命令（M3：任务主链）。汇报 / 文件 / 组织知识库在后续里程碑补入本文件。
//!
//! 命令层只做薄适配：授权与领域规则全在 [`crate::enclave::tasks`] 与 [`crate::enclave::authz`]。

use std::sync::Arc;

use tauri::State;

use crate::enclave::authz;
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
