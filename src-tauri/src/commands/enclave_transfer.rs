//! 主机转移命令（M8）：状态查询 / 加密迁移包导出导入 / 状态推进 / 旧机销毁。
//!
//! 控制面（云端 `transfer.request / accept / complete`）的平台接口尚未提供，
//! 本层先把**数据面 + 状态机**打通；`verified → completed` 由 owner 在完成云端
//! 转让后显式推进（接口接入后改为后端自动调用）。

use std::sync::Arc;

use tauri::State;

use crate::enclave::host;
use crate::enclave::transfer::{
    self, ExportOutcome, ImportOutcome, LanOfferOutcome, TransferView,
};
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

// ── 局域网自动迁移（数据面；云端所有权变更仍待平台接口）─────────

/// 发起局域网转移：生成迁移包与一次性口令，等待受让人确认。仅 owner。
#[tauri::command]
pub fn enclave_transfer_lan_offer(
    org_id: String,
    target_user_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<LanOfferOutcome, String> {
    state
        .store
        .with_conn(|c| transfer::create_lan_offer(c, &org_id, &target_user_id))
}

/// 拉取发给当前账号的转移请求（经主机 WS）。
#[tauri::command]
pub async fn enclave_transfer_lan_pending(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<TransferView>, String> {
    let data = host::fetch_pending_transfers(state.inner(), db.inner(), &org_id).await?;
    serde_json::from_value(data).map_err(|e| format!("解析转移请求失败：{}", e))
}

/// 接受并自动完成局域网迁移：接受 → 分块下载 → 校验 → 合并 → 告知主机已校验。
#[tauri::command]
pub async fn enclave_transfer_lan_accept(
    org_id: String,
    transfer_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ImportOutcome, String> {
    let accepted =
        host::accept_lan_transfer(state.inner(), db.inner(), &org_id, &transfer_id).await?;
    let passphrase = accepted["passphrase"].as_str().unwrap_or("").to_string();
    if passphrase.is_empty() {
        return Err("主机未返回本次迁移口令".to_string());
    }
    let expected_hash = accepted["packageHash"].as_str().unwrap_or("").to_string();

    let dest = std::env::temp_dir().join(format!("pilotdesk-lan-{}.pdesk", transfer_id));
    let actual_hash = match host::download_lan_package(
        state.inner(),
        db.inner(),
        &org_id,
        &transfer_id,
        &dest,
    )
    .await
    {
        Ok(h) => h,
        Err(e) => {
            let _ = std::fs::remove_file(&dest);
            return Err(e);
        }
    };
    if !expected_hash.is_empty() && actual_hash != expected_hash {
        let _ = std::fs::remove_file(&dest);
        return Err("迁移包校验失败（哈希不一致），已中止".to_string());
    }

    let imported = state.store.with_conn(|c| {
        transfer::import_package(c, dest.to_string_lossy().as_ref(), &passphrase)
    });
    let _ = std::fs::remove_file(&dest);
    let outcome = imported?;

    host::confirm_lan_verified(state.inner(), db.inner(), &org_id, &transfer_id)
        .await
        .map_err(|e| format!("数据已导入并校验，但主机未确认迁移完成：{}", e))?;
    Ok(outcome)
}
