//! 主机转移命令（M8）：状态查询 / 加密迁移包导出导入 / 状态推进 / 旧机销毁。
//!
//! 数据面与本地状态机在本层闭环；`verified → completed` 的云端生效由
//! `enclave_transfer_advance` 在推进前调用平台组织转让接口完成（旧主机 owner 触发），
//! 新主机则依据治理快照中「owner 已变更为本机」自动把转入记录推进到 `completed`。

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
///
/// 推进到 `completed` 前先落实云端生效：
/// - **转出（旧主机 owner）**：调用平台 `POST /organizations/:id/transfer` 完成 ownership 变更；
/// - **接管（新主机）**：要求治理快照已显示本机为 owner（快照轮询也会自动推进，这里兜底）。
#[tauri::command]
pub async fn enclave_transfer_advance(
    transfer_id: String,
    to_phase: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TransferView, String> {
    let cur = state.store.with_conn(|c| transfer::get(c, &transfer_id))?;
    if to_phase == transfer::PHASE_COMPLETED && cur.phase == transfer::PHASE_VERIFIED {
        let role =
            state
                .store
                .with_conn(|c| crate::enclave::authz::role_for_org(c, &cur.org_id))?;
        if role != Some(crate::enclave::authz::Role::Owner) {
            return Err("云端所有权尚未转移（本机还不是组织所有者），请先刷新治理快照".to_string());
        }
        if cur.direction == transfer::DIRECTION_OUT {
            let to_user_id: i64 = cur
                .peer_device_id
                .trim()
                .parse()
                .map_err(|_| "转移记录缺少有效的受让人账号".to_string())?;
            let token = crate::commands::org_common::require_token(db.inner()).await?;
            let path = format!("/api/v1/organizations/{}/transfer", cur.org_id);
            let transferred = crate::utils::platform::post_json_with_status::<_, serde_json::Value>(
                &path,
                &serde_json::json!({ "toUserId": to_user_id }),
                Some(&token),
            )
            .await;
            if let Err(e) = transferred {
                // 幂等兜底：请求可能已在平台生效（超时后重试 / 网页端已转让），以详情中的 owner 为准
                let detail_path = format!("/api/v1/organizations/{}", cur.org_id);
                let already = crate::utils::platform::get_json_with_status::<serde_json::Value>(
                    &detail_path,
                    Some(&token),
                )
                .await
                .ok()
                .and_then(|v| {
                    v.get("organization")
                        .and_then(|o| o.get("ownerId"))
                        .and_then(|x| x.as_i64())
                })
                .map(|owner| owner == to_user_id)
                .unwrap_or(false);
                if !already {
                    return Err(format!("云端组织转让失败：{}", e.message));
                }
            }
        }
    }
    let view = state
        .store
        .with_conn(|c| transfer::advance(c, &transfer_id, &to_phase))?;
    if to_phase == transfer::PHASE_COMPLETED && cur.direction == transfer::DIRECTION_OUT {
        // 旧主机：云端已转让，立即降级本地角色，主机服务下一轮复核时关闭监听
        state
            .store
            .with_conn(|c| crate::enclave::org::demote_after_transfer(c, &cur.org_id))?;
    }
    Ok(view)
}

/// 销毁旧主机数据：仅 `completed` 的转出记录；需输入组织 ID 二次确认。
#[tauri::command]
pub fn enclave_transfer_destroy(
    transfer_id: String,
    org_id: String,
    confirmation: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    if confirmation.trim() != org_id {
        return Err("请输入组织 ID 以确认销毁旧主机数据".to_string());
    }
    let view = state.store.with_conn(|c| transfer::get(c, &transfer_id))?;
    if view.org_id != org_id {
        return Err("转移记录与组织不匹配".to_string());
    }
    state
        .store
        .with_conn(|c| transfer::destroy_org_data(c, &org_id, &transfer_id))?;

    // 终态日志落**主库**：`enclave.db` 的审计可能随旧库删除/重建而消失（计划 §9）
    let main = db.get_conn().map_err(|e| e.to_string())?;
    let detail = serde_json::json!({
        "direction": view.direction,
        "peerDeviceId": view.peer_device_id,
        "packageHash": view.package_hash,
        "phase": view.phase,
    })
    .to_string();
    main.execute(
        "INSERT INTO enclave_transfer_log (org_id, transfer_id, action, detail, created_at)
         VALUES (?1, ?2, 'data_destroyed', ?3, ?4)",
        rusqlite::params![org_id, transfer_id, detail, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| format!("写入转移终态日志失败：{}", e))?;
    Ok(())
}

// ── 局域网自动迁移（数据面 + 云端所有权变更）───────────────

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

/// 拒绝局域网转移请求（受让人侧）：请求作废，旧主机继续权威。
#[tauri::command]
pub async fn enclave_transfer_lan_reject(
    org_id: String,
    transfer_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<TransferView, String> {
    let data =
        host::reject_lan_transfer(state.inner(), db.inner(), &org_id, &transfer_id).await?;
    serde_json::from_value(data).map_err(|e| format!("解析拒绝结果失败：{}", e))
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
