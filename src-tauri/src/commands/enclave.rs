//! 飞地（Enclave）：治理与主机类命令（快照读取/刷新、成员、策略、应急、审计、主机状态、转移）。
//!
//! 业务类命令（任务/汇报/文件/组织知识库）在 [`super::enclave_task`]。
//!
//! 命令层**只做薄适配**：权限与领域规则全在 [`crate::enclave`] 领域层。

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
pub fn enclave_status(state: State<'_, EnclaveState>) -> Result<EnclaveStatus, String> {
    Ok(EnclaveStatus {
        ready: true,
        db_path: state.store.path().to_string_lossy().to_string(),
        schema_version: state.store.schema_version(),
    })
}
