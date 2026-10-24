//! 飞地（Enclave）：治理与主机类命令（快照读取/刷新、成员、策略、应急、审计、主机状态、转移）。
//!
//! 业务类命令（任务/汇报/文件/组织知识库）在 [`super::enclave_task`]。
//!
//! 命令层**只做薄适配**：权限与领域规则全在 [`crate::enclave`] 领域层。

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
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

/// 只刷新本机登录身份（`/api/v1/me`）。
///
/// 账号切换后快照轮询可能尚未执行，若沿用旧身份会让新账号继承上一账号的任务权限；
/// 前端进入飞地时先调此命令，再拉任务 / 能力。
#[tauri::command]
pub async fn enclave_identity_refresh(
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    crate::enclave::snapshot::refresh_local_identity(db.inner(), state.inner()).await
}

/// 平台联调自检：登录凭证 / `/me` / 组织列表 / 成员名单；可选顺带重登记主机地址。
#[tauri::command]
pub async fn enclave_platform_check(
    org_id: String,
    register_host: Option<bool>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::snapshot::PlatformCheck>, String> {
    Ok(crate::enclave::snapshot::platform_check(
        db.inner(),
        state.inner(),
        &org_id,
        register_host.unwrap_or(false),
    )
    .await)
}

/// 最近一次治理快照成功同步时间（unix 秒，0 = 从未同步）：供前端判断是否 stale。
#[tauri::command]
pub fn enclave_snapshot_freshness(state: State<'_, Arc<EnclaveState>>) -> i64 {
    state.last_snapshot_at()
}

/// 主机服务状态（M7）：本机是主机还是成员、监听端口、发现到的主机。
#[tauri::command]
pub fn enclave_host_status(
    org_id: Option<String>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::host::HostStatusView, String> {
    crate::enclave::host::host_status(state.inner(), org_id.as_deref())
}

/// 成员端手动填写主机地址（`主机:端口`；留空清除，回到自动发现）。
#[tauri::command]
pub fn enclave_host_addr_set(
    org_id: String,
    addr: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    crate::enclave::host::set_manual_addr(state.inner(), &org_id, &addr)
}

/// 清除成员端固定的主机证书指纹（主机重装 / 更换主机后使用）。
#[tauri::command]
pub fn enclave_host_fp_reset(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<(), String> {
    crate::enclave::host::reset_fingerprint(state.inner(), &org_id)
}

/// 将本机登记为该组织主机（仅 owner）：首次登记 / 同设备续登直接生效。
///
/// 若已被其他设备登记，返回明确错误；换机请走 `enclave_host_takeover`（先迁移数据）。
#[tauri::command]
pub async fn enclave_host_claim(
    org_id: String,
    takeover: Option<bool>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::host::HostStatusView, String> {
    let role = state
        .store
        .with_conn(|c| crate::enclave::authz::role_for_org(c, &org_id))?;
    if role != Some(crate::enclave::authz::Role::Owner) {
        return Err("仅组织所有者可登记主机".to_string());
    }
    let reg = crate::enclave::host::register_with_platform(
        db.inner(),
        state.inner(),
        &org_id,
        takeover.unwrap_or(false),
    )
    .await?;
    let host = reg
        .host
        .ok_or_else(|| "平台未返回主机信息".to_string())?;
    if !reg.claimed {
        return Err(format!(
            "该组织已由「{}」登记为主机；换机请使用「从旧主机接管」",
            host.device_name
        ));
    }
    crate::enclave::host::apply_host_info(state.inner(), &org_id, &host)?;
    crate::enclave::host::host_status(state.inner(), Some(&org_id))
}

/// 从旧主机在线接管（同账号换机）：迁移数据 → 校验 → 导入 → 登记本机。
#[tauri::command]
pub async fn enclave_host_takeover(
    org_id: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::host::HandoverOutcome, String> {
    let role = state
        .store
        .with_conn(|c| crate::enclave::authz::role_for_org(c, &org_id))?;
    if role != Some(crate::enclave::authz::Role::Owner) {
        return Err("仅组织所有者可接管主机".to_string());
    }
    crate::enclave::host::takeover_from_host(state.inner(), db.inner(), &org_id).await
}

/// 强制接管（旧设备已丢失 / 不可修复）：不迁移数据，直接登记本机为空库主机。
///
/// 仅 owner 可用，必须输入组织 ID 二次确认；不可撤销。
#[tauri::command]
pub async fn enclave_host_takeover_force(
    org_id: String,
    confirmation: String,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::host::HostStatusView, String> {
    if confirmation.trim() != org_id {
        return Err("请输入组织 ID 以确认强制接管".to_string());
    }
    let role = state
        .store
        .with_conn(|c| crate::enclave::authz::role_for_org(c, &org_id))?;
    if role != Some(crate::enclave::authz::Role::Owner) {
        return Err("仅组织所有者可接管主机".to_string());
    }
    let reg = crate::enclave::host::register_with_platform(
        db.inner(),
        state.inner(),
        &org_id,
        true,
    )
    .await?;
    let host = reg
        .host
        .ok_or_else(|| "平台未返回主机信息".to_string())?;
    if !reg.claimed {
        return Err("主机登记未生效".to_string());
    }
    crate::enclave::host::apply_host_info(state.inner(), &org_id, &host)?;
    crate::enclave::host::host_status(state.inner(), Some(&org_id))
}

/// 本机文件缓存占用与上限（成员端打开/下载团队文件时产生）。
#[tauri::command]
pub fn enclave_cache_info(
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::host::CacheInfoView, String> {
    state.store.with_conn(|c| Ok(crate::enclave::host::cache_info(c)))
}

/// 调整本机文件缓存上限（单位 MB；后端按 256 MB ~ 100 GB 夹取并立即淘汰）。
#[tauri::command]
pub fn enclave_cache_set_limit(
    max_mb: u64,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::host::CacheInfoView, String> {
    let bytes = max_mb.saturating_mul(1024 * 1024);
    state
        .store
        .with_conn(|c| crate::enclave::host::set_cache_limit(c, bytes))
}

/// 清空本机文件缓存（不影响主机上的团队数据）。
#[tauri::command]
pub fn enclave_cache_clear(
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::host::CacheInfoView, String> {
    state.store.with_conn(crate::enclave::host::clear_cache)
}

// ════════════════════════════════════════════════════════════
// M2：授权器 + 策略 / 应急 / 审计
// ════════════════════════════════════════════════════════════

/// 组织级能力视图：当前身份、角色、可执行动作，以及快照新鲜度与应急状态。
///
/// 前端**只按 `actions` 渲染治理按钮**（`effectiveActions` 是唯一来源）；默认拒绝时这里为空。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnclaveOrgCaps {
    pub org_id: String,
    /// 本机登录身份（未登录为 null）
    pub identity: Option<crate::enclave::org::LocalIdentityView>,
    /// 当前用户在该组织中的角色（owner / admin / member；未同步为 null）
    pub role: Option<String>,
    /// 可执行的组织级动作
    pub actions: Vec<String>,
    /// 治理快照是否新鲜（超过 `SNAPSHOT_STALE_MAX_SECS` 即 false）
    pub snapshot_fresh: bool,
    /// 快照同步时间（unix 秒）
    pub synced_at: i64,
    /// 飞地对外服务是否已临时关闭
    pub disabled: bool,
}

#[tauri::command]
pub fn enclave_org_caps(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<EnclaveOrgCaps, String> {
    let store = &state.store;
    let now = chrono::Utc::now().timestamp();
    store.with_conn(|c| {
        let identity = crate::enclave::org::local_identity(c)?;
        let role = crate::enclave::authz::role_for_org(c, &org_id)?.map(|r| r.as_str().to_string());
        let actions = crate::enclave::authz::effective_actions_for_org(c, &org_id)?
            .into_iter()
            .map(str::to_string)
            .collect();
        let synced_at: i64 = c
            .query_row(
                "SELECT synced_at FROM org_snapshot WHERE org_id = ?1",
                rusqlite::params![org_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(EnclaveOrgCaps {
            org_id: org_id.clone(),
            identity,
            role,
            actions,
            snapshot_fresh: synced_at > 0
                && now.saturating_sub(synced_at) < crate::enclave::SNAPSHOT_STALE_MAX_SECS,
            synced_at,
            disabled: state.is_disabled(),
        })
    })
}

/// 读数据出境总闸（默认关闭）。成员端以主机策略为准。
#[tauri::command]
pub async fn enclave_policy_get(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::org::PolicyView, String> {
    let role = state
        .store
        .with_conn(|c| crate::enclave::authz::role_for_org(c, &org_id))?;
    match role {
        Some(crate::enclave::authz::Role::Owner) => state
            .store
            .with_conn(|c| crate::enclave::org::policy_view(c, &org_id)),
        Some(_) => {
            let data = crate::enclave::host::call_host_without_token(
                state.inner(),
                &org_id,
                "policy.get",
                serde_json::json!({}),
            )
            .await?;
            serde_json::from_value(data).map_err(|e| format!("解析主机策略失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// 设置数据出境总闸（仅 owner）：写库 + 审计。
///
/// 语义：`true` = 允许把团队数据发送至云端模型。**本开关是主机本地策略**（随库走），
/// 断网也可修改；但授权仍依据最近一次治理快照中的角色。
#[tauri::command]
pub fn enclave_policy_set(
    org_id: String,
    allow: bool,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::org::PolicyView, String> {
    use crate::enclave::authz::{authorize, Action};
    let view = state.store.with_conn(|c| {
        let actor = authorize(c, &org_id, Action::PolicyWrite)?;
        let before = crate::enclave::org::get_allow_cloud_model(c, &org_id)?.unwrap_or(false);
        crate::enclave::org::set_allow_cloud_model(c, &org_id, allow, &actor)?;
        crate::enclave::org::audit(
            c,
            &org_id,
            &actor,
            "policy.allow_cloud_model",
            &serde_json::json!({ "from": before, "to": allow }).to_string(),
        )?;
        crate::enclave::org::policy_view(c, &org_id)
    })?;
    // 出境闸门变更：在线成员的界面要同步（本机事件 + WS 广播）
    crate::enclave::host::notify_change(state.inner(), &org_id, "org");
    Ok(view)
}

/// 在线成员（主机侧订阅列表）。主机本地读取；成员经主机 WS 的 `org.online` 取。
///
/// 只读信息：成员之间互相可见，不涉及治理权限。
#[tauri::command]
pub async fn enclave_online_members(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<String>, String> {
    let role = state
        .store
        .with_conn(|c| crate::enclave::authz::role_for_org(c, &org_id))?;
    match role {
        Some(crate::enclave::authz::Role::Owner) => {
            Ok(crate::enclave::host::online_member_ids(state.inner(), &org_id))
        }
        Some(_) => {
            let data = crate::enclave::host::call_host_without_token(
                state.inner(),
                &org_id,
                "org.online",
                serde_json::json!({}),
            )
            .await?;
            serde_json::from_value(data).map_err(|e| format!("解析在线成员失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// 读主机应急开关。成员端以主机状态为准。
#[tauri::command]
pub async fn enclave_emergency_get(
    org_id: String,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::org::EmergencyView, String> {
    let role = state
        .store
        .with_conn(|c| crate::enclave::authz::role_for_org(c, &org_id))?;
    match role {
        Some(crate::enclave::authz::Role::Owner) => state
            .store
            .with_conn(|c| crate::enclave::org::emergency_view(c, &org_id)),
        Some(_) => {
            let data = crate::enclave::host::call_host_without_token(
                state.inner(),
                &org_id,
                "emergency.get",
                serde_json::json!({}),
            )
            .await?;
            serde_json::from_value(data).map_err(|e| format!("解析主机应急状态失败：{}", e))
        }
        None => Err("尚未同步该组织的治理快照，请先在「组织与权限」页刷新".to_string()),
    }
}

/// 设置主机应急开关（仅 owner）：写库 + 审计 + 更新运行态。
///
/// 语义：`true` = **临时关闭飞地对外服务**；本机命令照常可用、断网可用、不改云端数据。
#[tauri::command]
pub fn enclave_emergency_set(
    org_id: String,
    disabled: bool,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::org::EmergencyView, String> {
    use crate::enclave::authz::{authorize, Action};
    let view = state.store.with_conn(|c| {
        let actor = authorize(c, &org_id, Action::EmergencySet)?;
        let before = crate::enclave::org::get_emergency(c, &org_id)?.unwrap_or(false);
        crate::enclave::org::set_emergency(c, &org_id, disabled, &actor)?;
        crate::enclave::org::audit(
            c,
            &org_id,
            &actor,
            "host.emergency",
            &serde_json::json!({ "from": before, "to": disabled }).to_string(),
        )?;
        crate::enclave::org::emergency_view(c, &org_id)
    })?;
    state.set_disabled(view.disabled);
    // 应急开关变化：让在线成员的治理视图跟着刷新（对外服务随后会拒绝新连接）
    crate::enclave::host::notify_change(state.inner(), &org_id, "org");
    Ok(view)
}

/// 读通知列表（沟通层）。主机走本地库；成员经主机 WS 拉取（不缓存）。
#[tauri::command]
pub async fn enclave_notification_list(
    org_id: String,
    limit: Option<i64>,
    unread_only: Option<bool>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::notify::NotificationView>, String> {
    let limit = limit.unwrap_or(30);
    let unread_only = unread_only.unwrap_or(false);
    let payload = json!({ "limit": limit, "unreadOnly": unread_only });
    crate::commands::enclave_task::run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "notification.list",
        payload,
        |c, actor| crate::enclave::notify::list(c, &org_id, actor, limit, unread_only),
    )
    .await
}

/// 标记通知已读：`ids` 为空 = 全部未读。
#[tauri::command]
pub async fn enclave_notification_mark_read(
    org_id: String,
    ids: Option<Vec<String>>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<usize, String> {
    let ids = ids.unwrap_or_default();
    let payload = json!({ "ids": &ids });
    crate::commands::enclave_task::run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "notification.mark_read",
        payload,
        |c, actor| crate::enclave::notify::mark_read(c, &org_id, actor, &ids),
    )
    .await
}

/// 删除通知：`ids` 为空 = 清空本账号全部已读通知。
#[tauri::command]
pub async fn enclave_notification_delete(
    org_id: String,
    ids: Option<Vec<String>>,
    db: State<'_, crate::DbState>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<usize, String> {
    let ids = ids.unwrap_or_default();
    let payload = json!({ "ids": &ids });
    crate::commands::enclave_task::run_command(
        state.inner(),
        db.inner(),
        &org_id,
        "notification.delete",
        payload,
        |c, actor| crate::enclave::notify::delete(c, &org_id, actor, &ids),
    )
    .await
}

/// 主机自动备份状态（本机设置，仅主机有数据可备）。
#[tauri::command]
pub fn enclave_backup_status(
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::backup::BackupStatusView, String> {
    state.store.with_conn(|c| crate::enclave::backup::status(c))
}

/// 修改自动备份设置：开关 / 间隔小时 / 保留份数。
#[tauri::command]
pub fn enclave_backup_config_set(
    enabled: bool,
    interval_hours: i64,
    keep: i64,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::backup::BackupStatusView, String> {
    state.store.with_conn(|c| {
        crate::enclave::backup::set_config(c, enabled, interval_hours, keep)
    })
}

/// 立即备份一次（阻塞线程池执行，避免卡住 UI）。
#[tauri::command]
pub async fn enclave_backup_now(
    state: State<'_, Arc<EnclaveState>>,
) -> Result<crate::enclave::backup::BackupInfo, String> {
    let st = state.inner().clone();
    tokio::task::spawn_blocking(move || st.store.with_conn(|c| crate::enclave::backup::run_backup(c)))
        .await
        .map_err(|e| format!("备份线程异常：{}", e))?
}

/// 读策略 / 应急变更审计流水（按时间倒序）。
///
/// 权限：仅 owner / admin（治理审计不对全员开放）。其中 `files.root` 的绝对路径
/// 属于主机本机隐私，**仅 owner 可见**；admin 只能看到「已变更」。
#[tauri::command]
pub fn enclave_audit_list(
    org_id: String,
    limit: Option<i64>,
    offset: Option<i64>,
    state: State<'_, Arc<EnclaveState>>,
) -> Result<Vec<crate::enclave::org::AuditView>, String> {
    use crate::enclave::authz::{self, Action, Role};

    let limit = limit.unwrap_or(50).clamp(1, 200);
    let offset = offset.unwrap_or(0).max(0);
    state.store.with_conn(|c| {
        let role = authz::role_for_org(c, &org_id)?.ok_or_else(|| {
            "尚未同步该组织的治理快照，请先在「组织与权限」页点击刷新".to_string()
        })?;
        if !authz::org_allows(role, Action::OrgAdmin) {
            return Err("仅组织所有者或管理员可查看变更审计".to_string());
        }
        let mut rows = crate::enclave::org::list_audit(c, &org_id, limit, offset)?;
        if role != Role::Owner {
            for row in &mut rows {
                if row.action == "files.root" {
                    row.detail = r#"{"redacted":true}"#.to_string();
                }
            }
        }
        Ok(rows)
    })
}
