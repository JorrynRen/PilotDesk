//! 飞地（Enclave）：治理与主机类命令（快照读取/刷新、成员、策略、应急、审计、主机状态、转移）。
//!
//! 业务类命令（任务/汇报/文件/组织知识库）在 [`super::enclave_task`]。
//!
//! 命令层**只做薄适配**：权限与领域规则全在 [`crate::enclave`] 领域层。

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::enclave::EnclaveState;

/// 飞地模块状态：骨架阶段用于确认库已就绪（主机/快照字段随里程碑补齐）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnclaveStatus {
    /// 飞地数据库是否已打开并就绪。
    pub ready: bool,
    /// `enclave.db` 的绝对路径（便于用户确认团队数据落点）。
    pub db_path: String,
    /// 打开时确定的结构版本。
    pub schema_version: i64,
}

/// 飞地模块状态：确认 `enclave.db` 已生成、表结构就绪。
#[tauri::command]
pub fn enclave_status(state: State<'_, Arc<EnclaveState>>) -> Result<EnclaveStatus, String> {
    Ok(EnclaveStatus {
        ready: true,
        db_path: state.store.path().to_string_lossy().to_string(),
        schema_version: state.store.schema_version(),
    })
}

/// 本机快照中的组织列表（只读）。
#[tauri::command]
pub fn enclave_org_list(
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::org::OrgSnapshotView>, String> {
    state.store.with_conn(crate::enclave::org::list_orgs)
}

/// 某组织详情（组织信息 + 成员列表，只读）；不存在返回 `None`。
#[tauri::command]
pub fn enclave_org_detail(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Option<crate::enclave::org::OrgDetailView>, String> {
    state
        .store
        .with_conn(|c| crate::enclave::org::org_detail(c, &org_id))
}

/// 触发一轮治理快照刷新（全量）。
///
/// `org_id` 一期忽略（全量刷新）；参数保留以便后续做「单组织刷新」。
#[tauri::command]
pub async fn enclave_snapshot_refresh(
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
    org_id: Option<String>,
) -> Result<crate::enclave::snapshot::SnapshotOutcome, String> {
    let _ = org_id; // 一期全量刷新，忽略组织范围
    crate::enclave::snapshot::run_snapshot_round(db.inner(), state.inner()).await
}

/// 最近一次治理快照成功同步时间（unix 秒，0 = 从未同步）：供前端判断是否 stale。
#[tauri::command]
pub fn enclave_snapshot_freshness(state: State<'_, Arc<EnclaveState>>) -> i64 {
    state.last_snapshot_at()
}
