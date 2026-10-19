//! 主机转移命令（M8）：状态查询 / 加密迁移包导出导入 / 状态推进 / 旧机销毁。
//!
//! 控制面（云端 `transfer.request / accept / complete`）的平台接口尚未提供，
//! 本层先把**数据面 + 状态机**打通；`verified → completed` 由 owner 在完成云端
//! 转让后显式推进（接口接入后改为后端自动调用）。

use std::sync::Arc;

use tauri::State;

use crate::enclave::transfer::{self, ExportOutcome, ImportOutcome, TransferView};
use crate::enclave::EnclaveState;

#[tauri::command]
pub fn enclave_transfer_list(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<TransferView>, String> {
    state.store.with_conn(|c| transfer::list(c, &org_id))
}

/// 导出线下加密迁移包（仅组织 owner）。`out_path` 为空时落到应用 `enclave/transfer/`。
#[tauri::command]
pub fn enclave_transfer_export(
    org_id: String,
    passphrase: String,
    out_path: Option<String>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ExportOutcome, String> {
    state.store.with_conn(|c| {
        transfer::export_package(c, &org_id, out_path.as_deref(), &passphrase)
    })
}

/// 导入加密迁移包（接收方必须是该组织成员）。成功后记录为 `verified`。
#[tauri::command]
pub fn enclave_transfer_import(
    path: String,
    passphrase: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ImportOutcome, String> {
    state
        .store
        .with_conn(|c| transfer::import_package(c, &path, &passphrase))
}

/// 推进转移状态（真值表见 `transfer::can_advance`）。
#[tauri::command]
pub fn enclave_transfer_advance(
    transfer_id: String,
    to_phase: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TransferView, String> {
    state
        .store
        .with_conn(|c| transfer::advance(c, &transfer_id, &to_phase))
}

/// 销毁旧主机数据：仅 `completed` 的转出记录；需输入组织 ID 二次确认。
#[tauri::command]
pub fn enclave_transfer_destroy(
    transfer_id: String,
    org_id: String,
    confirmation: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    if confirmation.trim() != org_id {
        return Err("请输入组织 ID 以确认销毁旧主机数据".to_string());
    }
    state
        .store
        .with_conn(|c| transfer::destroy_org_data(c, &org_id, &transfer_id))
}
