//! 飞地 API 共享与一对一传讯命令层。
//!
//! 领域规则在 `enclave::api_share` / `enclave::messenger`；这里只把 Tauri 参数
//! 适配成主机 RPC payload。owner 本机走本地库，其他成员统一经主机执行。

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use crate::enclave::api_share::{
    self, ApiShareAccessInfo, ApiShareDistributionView, ApiShareMemberAccessView,
    ApiShareProviderView, ApiShareSourceView, ApiShareTokenSecret, ApiShareTokenView,
    ApiShareUsageView, CreateApiShareDistributionInput, CreateApiShareSourceInput,
    CreateApiShareTokenInput, CreatedApiShareDistributions, IssuedApiShareToken,
    OpenedApiShareConnection, UpdateApiShareSourceInput, UpdateApiShareTokenInput,
};
use crate::enclave::authz;
use crate::enclave::messenger::{
    self, MessageView, MessengerFileView, MessengerMemberView, MessengerSettingsView,
    SendMessageInput,
};
use crate::enclave::EnclaveState;

use super::enclave_task::run_command;

fn value_of<T: serde::Serialize>(value: &T, label: &str) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| format!("{}参数序列化失败：{}", label, e))
}

#[tauri::command]
pub async fn enclave_api_share_access_info(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareAccessInfo, String> {
    let enclave = state.inner().clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.access_info",
        json!({}),
        |c, _actor| Ok(api_share::access_info_with_conn(&enclave, c, &org_id)),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_provider_catalog(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<ApiShareProviderView>, String> {
    let enclave = state.inner().clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.provider_catalog",
        json!({}),
        |c, actor| api_share::provider_catalog_with_conn(&enclave, c, &org_id, actor),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_members_list(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<ApiShareMemberAccessView>, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.members",
        json!({}),
        |c, actor| api_share::list_member_access(c, &org_id, actor),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_member_block(
    org_id: String,
    user_id: String,
    reason: Option<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareMemberAccessView, String> {
    let reason_arg = reason.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.member_block",
        json!({ "userId": user_id, "reason": reason }),
        |c, actor| {
            api_share::block_member(
                c,
                &org_id,
                actor,
                &user_id,
                reason_arg.as_deref().unwrap_or_default(),
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_member_unblock(
    org_id: String,
    user_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareMemberAccessView, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.member_unblock",
        json!({ "userId": user_id }),
        |c, actor| api_share::unblock_member(c, &org_id, actor, &user_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_sources_list(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<ApiShareSourceView>, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.sources",
        json!({}),
        |c, actor| api_share::list_sources(c, &org_id, actor),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_source_create(
    org_id: String,
    input: CreateApiShareSourceInput,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareSourceView, String> {
    let input_value = value_of(&input, "API 共享")?;
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.source_create",
        json!({ "input": input_value }),
        |c, actor| api_share::create_source(c, &org_id, actor, &input),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_source_update(
    org_id: String,
    source_id: String,
    input: UpdateApiShareSourceInput,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareSourceView, String> {
    let input_value = value_of(&input, "API 共享")?;
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.source_update",
        json!({ "sourceId": source_id, "input": input_value }),
        |c, actor| api_share::update_source(c, &org_id, actor, &source_id, &input),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_source_delete(
    org_id: String,
    source_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.source_delete",
        json!({ "sourceId": source_id }),
        |c, actor| api_share::delete_source(c, &org_id, actor, &source_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_tokens_list(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<ApiShareTokenView>, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.tokens",
        json!({}),
        |c, actor| api_share::list_tokens(c, &org_id, actor),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_token_create(
    org_id: String,
    input: CreateApiShareTokenInput,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<IssuedApiShareToken, String> {
    let input_value = value_of(&input, "共享令牌")?;
    let enclave = state.inner().clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.token_create",
        json!({ "input": input_value }),
        |c, actor| api_share::create_token(&enclave, c, &org_id, actor, &input),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_token_update(
    org_id: String,
    token_id: String,
    input: UpdateApiShareTokenInput,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareTokenView, String> {
    let input_value = value_of(&input, "共享令牌")?;
    let enclave = state.inner().clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.token_update",
        json!({ "tokenId": token_id, "input": input_value }),
        |c, actor| api_share::update_token(&enclave, c, &org_id, actor, &token_id, &input),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_token_secret(
    org_id: String,
    token_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareTokenSecret, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.token_secret",
        json!({ "tokenId": token_id }),
        |c, actor| api_share::token_secret(c, &org_id, actor, &token_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_token_revoke(
    org_id: String,
    token_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.token_revoke",
        json!({ "tokenId": token_id }),
        |c, actor| api_share::revoke_token(c, &org_id, actor, &token_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_token_archive(
    org_id: String,
    token_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareTokenView, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.token_archive",
        json!({ "tokenId": token_id }),
        |c, actor| api_share::archive_token(c, &org_id, actor, &token_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_token_delete(
    org_id: String,
    token_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.token_delete",
        json!({ "tokenId": token_id }),
        |c, actor| api_share::delete_token(c, &org_id, actor, &token_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_distributions_list(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<ApiShareDistributionView>, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.distributions",
        json!({}),
        |c, actor| api_share::list_distributions(c, &org_id, actor),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_distribution_create(
    org_id: String,
    input: CreateApiShareDistributionInput,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<CreatedApiShareDistributions, String> {
    let input_value = value_of(&input, "API 共享分发")?;
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.distribution_create",
        json!({ "input": input_value }),
        |c, actor| api_share::create_distributions(c, &org_id, actor, &input),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_distribution_mark_sent(
    org_id: String,
    distribution_id: String,
    message_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareDistributionView, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.distribution_mark_sent",
        json!({ "distributionId": distribution_id, "messageId": message_id }),
        |c, actor| {
            api_share::mark_distribution_sent(c, &org_id, actor, &distribution_id, &message_id)
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_distribution_revoke(
    org_id: String,
    distribution_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.distribution_revoke",
        json!({ "distributionId": distribution_id }),
        |c, actor| api_share::revoke_distribution(c, &org_id, actor, &distribution_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_distribution_archive(
    org_id: String,
    distribution_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareDistributionView, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.distribution_archive",
        json!({ "distributionId": distribution_id }),
        |c, actor| api_share::archive_distribution(c, &org_id, actor, &distribution_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_distribution_delete(
    org_id: String,
    distribution_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.distribution_delete",
        json!({ "distributionId": distribution_id }),
        |c, actor| api_share::delete_distribution(c, &org_id, actor, &distribution_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_distribution_open(
    org_id: String,
    distribution_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<OpenedApiShareConnection, String> {
    let enclave = state.inner().clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.distribution_open",
        json!({ "distributionId": distribution_id }),
        |c, actor| api_share::open_distribution(&enclave, c, &org_id, actor, &distribution_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_distribution_mark_applied(
    org_id: String,
    distribution_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareDistributionView, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.distribution_mark_applied",
        json!({ "distributionId": distribution_id }),
        |c, actor| api_share::mark_distribution_applied(c, &org_id, actor, &distribution_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_api_share_usage(
    org_id: String,
    source_id: Option<String>,
    token_id: Option<String>,
    from_day: Option<String>,
    to_day: Option<String>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<ApiShareUsageView, String> {
    let source_id_arg = source_id.clone();
    let token_id_arg = token_id.clone();
    let from_day_arg = from_day.clone();
    let to_day_arg = to_day.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "api_share.usage",
        json!({
            "sourceId": source_id,
            "tokenId": token_id,
            "fromDay": from_day,
            "toDay": to_day,
        }),
        |c, actor| {
            api_share::usage(
                c,
                &org_id,
                actor,
                source_id_arg.as_deref(),
                token_id_arg.as_deref(),
                from_day_arg.as_deref(),
                to_day_arg.as_deref(),
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_messenger_settings_get(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<MessengerSettingsView, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.settings_get",
        json!({}),
        |c, actor| messenger::settings(c, &org_id, actor),
    )
    .await
}

#[tauri::command]
pub async fn enclave_messenger_settings_set(
    org_id: String,
    retention_days: i64,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<MessengerSettingsView, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.retention_set",
        json!({ "days": retention_days }),
        |c, actor| messenger::set_retention(c, &org_id, actor, retention_days),
    )
    .await
}

#[tauri::command]
pub async fn enclave_messenger_members(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<MessengerMemberView>, String> {
    let enclave = state.inner().clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.members",
        json!({}),
        |c, actor| {
            let online = crate::enclave::host::online_member_ids(&enclave, &org_id);
            messenger::members(c, &org_id, actor, &online)
        },
    )
    .await
}

#[tauri::command]
pub async fn enclave_messenger_messages(
    org_id: String,
    peer_id: String,
    limit: Option<usize>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<MessageView>, String> {
    let peer_arg = peer_id.clone();
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.messages",
        json!({ "peerId": peer_id, "limit": limit }),
        |c, actor| messenger::list_messages(c, &org_id, actor, &peer_arg, limit),
    )
    .await
}

#[tauri::command]
pub async fn enclave_messenger_send(
    org_id: String,
    input: SendMessageInput,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<MessageView, String> {
    let input_value = value_of(&input, "传讯")?;
    let peer_id = input.peer_id.clone();
    let row = run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.send",
        json!({ "input": input_value }),
        |c, actor| {
            messenger::send(c, &org_id, actor, &input).and_then(|row| {
                messenger::list_messages(c, &org_id, actor, &row.to_user_id, Some(1)).and_then(
                    |mut rows| {
                        rows.pop()
                            .ok_or_else(|| "发送传讯后读取消息失败".to_string())
                    },
                )
            })
        },
    )
    .await?;
    let actor = state
        .inner()
        .store
        .with_conn(|c| authz::current_actor_id(c))?;
    crate::enclave::host::notify_messenger_change(state.inner(), &org_id, &actor, &peer_id, None);
    Ok(row)
}

#[tauri::command]
pub async fn enclave_messenger_send_file(
    org_id: String,
    peer_id: String,
    file_name: String,
    mime: String,
    data_base64: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<MessageView, String> {
    let peer_arg = peer_id.clone();
    let file_arg = file_name.clone();
    let mime_arg = mime.clone();
    let data_arg = data_base64.clone();
    let row = run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.send_file",
        json!({
            "peerId": peer_id,
            "fileName": file_name,
            "mime": mime,
            "dataBase64": data_base64,
        }),
        |c, actor| {
            messenger::send_file(
                c, &org_id, actor, &peer_arg, &file_arg, &mime_arg, &data_arg,
            )
            .and_then(|row| {
                messenger::list_messages(c, &org_id, actor, &row.to_user_id, Some(1)).and_then(
                    |mut rows| {
                        rows.pop()
                            .ok_or_else(|| "发送文件后读取消息失败".to_string())
                    },
                )
            })
        },
    )
    .await?;
    let actor = state
        .inner()
        .store
        .with_conn(|c| authz::current_actor_id(c))?;
    crate::enclave::host::notify_messenger_change(state.inner(), &org_id, &actor, &peer_arg, None);
    Ok(row)
}

#[tauri::command]
pub async fn enclave_messenger_file_get(
    org_id: String,
    file_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<MessengerFileView, String> {
    run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.file_get",
        json!({ "fileId": file_id }),
        |_c, actor| messenger::get_ephemeral_file(&org_id, actor, &file_id),
    )
    .await
}

#[tauri::command]
pub async fn enclave_messenger_recall(
    org_id: String,
    message_id: String,
    peer_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<MessageView, String> {
    let message_arg = message_id.clone();
    let peer_arg = peer_id.clone();
    let row = run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.recall",
        json!({ "messageId": message_id, "peerId": peer_id }),
        |c, actor| messenger::recall(c, &org_id, actor, &peer_arg, &message_arg),
    )
    .await?;
    let actor = state
        .inner()
        .store
        .with_conn(|c| authz::current_actor_id(c))?;
    crate::enclave::host::notify_messenger_change(state.inner(), &org_id, &actor, &peer_arg, None);
    Ok(row)
}

#[tauri::command]
pub async fn enclave_messenger_clear(
    org_id: String,
    peer_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<usize, String> {
    let peer_arg = peer_id.clone();
    let removed = run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "im.clear",
        json!({ "peerId": peer_id }),
        |c, actor| messenger::clear_thread(c, &org_id, actor, &peer_arg),
    )
    .await?;
    let actor = state
        .inner()
        .store
        .with_conn(|c| authz::current_actor_id(c))?;
    crate::enclave::host::notify_messenger_change(state.inner(), &org_id, &actor, &peer_arg, None);
    Ok(removed)
}
