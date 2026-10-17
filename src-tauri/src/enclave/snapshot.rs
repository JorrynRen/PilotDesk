//! 飞地治理快照同步：从平台拉取「组织 + 成员」，只读落库到 `enclave.db`。
//!
//! 权威分工：组织 / 成员 / 角色 / 席位**以平台为准**，本模块只写只读快照；
//! 本地不写成员治理数据（写策略开关的是 [`super::org`]）。
//!
//! 平台接口（`requireAuth`，Bearer）：
//! ```text
//! GET /api/v1/organizations/mine          → { organizations: [...] }（字段与 org_share 的 OrgRow 同口径）
//! GET /api/v1/organizations/:id/members    → 成员列表（**接口未定稿，按防御式解析**）
//! ```
//!
//! 一条硬约束贯穿全文件：**网络请求全部做完拿到数据，再进 `with_conn` 同步块写库**；
//! 同步块内绝不 `.await`。

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::org::{self, MemberSnapshotRow, OrgSnapshotRow};
use super::{EnclaveState, SNAPSHOT_DEFAULT_INTERVAL_SECS, SNAPSHOT_STALE_MAX_SECS};
use crate::commands::org_common::require_token;
use crate::utils::platform;

/// 主库设置键：治理快照后台轮询间隔（秒）。
const SNAPSHOT_INTERVAL_KEY: &str = "enclave_snapshot_interval_secs";

/// 失败退避的起始时长（秒）。
const BACKOFF_START_SECS: u64 = 30;
/// 失败退避的上限（秒，15 分钟）。
const BACKOFF_MAX_SECS: u64 = 15 * 60;

/// 一轮治理快照同步的结果（供前端刷新后提示）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotOutcome {
    /// 本轮同步的组织数（仅 active 组织）。
    pub org_count: usize,
    /// 本轮同步的成员总数。
    pub member_count: usize,
    /// 本轮同步完成时间（unix 秒）。
    pub synced_at: i64,
    /// 成员列表拉取失败的原因（仅首个；组织快照本身仍会正常写入）。
    ///
    /// 平台成员列表接口尚属待对齐项（见设计 §16.1），该接口缺失时**不应拖垮整轮同步**：
    /// 组织信息照常落库，成员表留空并把这个原因透出给前端提示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_error: Option<String>,
}

// ── 平台响应结构 ────────────────────────────────────────────────

#[derive(Deserialize)]
struct OrgsResp {
    organizations: Vec<OrgRow>,
}

/// 平台组织行（**与 org_share 的 `OrgRow` 同口径**；`MyOrg` 字段就是组织快照需要的数据）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrgRow {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    seat_status: String,
    #[serde(default)]
    plan_key: String,
    #[serde(default)]
    plan_expires_at: Option<String>,
    #[serde(default)]
    seats: i64,
    #[serde(default)]
    member_count: i64,
    #[serde(default)]
    status: String,
    /// 平台 owner（即主机身份来源）。平台 `OrgRow` **当前无 owner 字段**，
    /// 这里防御式兼容 `ownerUserId`；平台补齐前恒为 `None` → 落库为空字符串（详见下方注释）。
    #[serde(default)]
    owner_user_id: Option<String>,
}

// ── 一轮同步 ────────────────────────────────────────────────────

/// 执行一轮治理快照同步：拉组织与成员 → 只读落库。
///
/// 未登录直接返回 `Err("请先登录平台账号")`（来自 [`require_token`]）。
pub async fn run_snapshot_round(
    db: &crate::DbState,
    state: &EnclaveState,
) -> Result<SnapshotOutcome, String> {
    let token = require_token(db).await?;

    // 1) 我所属的组织：只保留 seatStatus=active 且 status=active 的组织。
    let resp: OrgsResp = platform::get_json_with_status("/api/v1/organizations/mine", Some(&token))
        .await
        .map_err(|e| e.message)?;
    let orgs: Vec<OrgRow> = resp
        .organizations
        .into_iter()
        .filter(|o| o.seat_status == "active" && o.status == "active")
        .collect();

    // 2) 逐组织拉成员（网络请求先行，await 全在同步块之外）。
    //
    // 成员列表接口是**待对齐项**：缺失/报错时只记原因、跳过该组织的成员写入，
    // **不拖垮整轮**（组织快照照常落库）。注意此时**绝不能**调 `upsert_member_snapshot`——
    // 它会把这个组织的存量成员全部标记为 `removed`。
    let mut org_rows: Vec<OrgSnapshotRow> = Vec::with_capacity(orgs.len());
    let mut member_sets: Vec<(String, Vec<MemberSnapshotRow>)> = Vec::with_capacity(orgs.len());
    let mut member_error: Option<String> = None;
    for o in &orgs {
        org_rows.push(OrgSnapshotRow {
            org_id: o.id.to_string(),
            name: o.name.clone(),
            // 平台 ownerUserId 待补：当前为空字符串，M2 起用于确定主机身份。
            owner_user_id: o.owner_user_id.clone().unwrap_or_default(),
            plan_key: o.plan_key.clone(),
            plan_expires_at: o.plan_expires_at.clone(),
            seats: o.seats,
            member_count: o.member_count,
            status: o.status.clone(),
        });
        let path = format!("/api/v1/organizations/{}/members", o.id);
        match fetch_members(&path, &token).await {
            Ok(members) => member_sets.push((o.id.to_string(), members)),
            Err(e) => {
                log::debug!("[Enclave] 组织 {} 成员快照拉取失败：{}", o.id, e);
                if member_error.is_none() {
                    member_error = Some(e);
                }
            }
        }
    }

    let org_count = org_rows.len();
    let member_count: usize = member_sets.iter().map(|(_, v)| v.len()).sum();

    // 3) 同步块：数据已全部到手，写库（闭包内绝不 await）。
    state.store.with_conn(|c| {
        for r in &org_rows {
            org::upsert_org_snapshot(c, r)?;
        }
        for (org_id, members) in &member_sets {
            org::upsert_member_snapshot(c, org_id, members)?;
        }
        Ok(())
    })?;

    let synced_at = chrono::Utc::now().timestamp();
    state.last_snapshot_at.store(synced_at, Ordering::Relaxed);
    Ok(SnapshotOutcome {
        org_count,
        member_count,
        synced_at,
        member_error,
    })
}

/// 拉某组织的成员列表，做**防御式解析**。
///
/// **平台 members 接口尚未定稿**，响应体可能是 `{ members: [...] }`，也可能是裸数组 `[...]`；
/// 因此这里先用 `serde_json::Value` 兼容两种形状，再由 [`parse_member`] 逐条宽松取值。
async fn fetch_members(path: &str, token: &str) -> Result<Vec<MemberSnapshotRow>, String> {
    let v: serde_json::Value = platform::get_json_with_status(path, Some(token))
        .await
        .map_err(|e| match e.status {
            // 404：平台尚未提供该接口（待对齐项），给出可读原因而不是网关的「未注册的项目：api」
            404 => "平台暂未提供成员列表接口（待平台补齐）".to_string(),
            401 => "请先登录平台账号".to_string(),
            _ => e.message,
        })?;
    let list = match &v {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(o) => o
            .get("members")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default(),
        // 其它形状（null / 数字等）一律视为空列表，不阻塞整轮同步。
        _ => Vec::new(),
    };
    Ok(list
        .iter()
        .filter_map(parse_member)
        .collect::<Vec<MemberSnapshotRow>>())
}

/// 把一条成员 JSON 宽松解析为 [`MemberSnapshotRow`]（缺失字段用默认值）。
///
/// 字段口径：`userId`（兼容数字）/ `role` / `displayName` / `email` / `seatStatus`（缺省视为 "active"）。
/// `userId` 缺失（或空）的条目无法作为主键，直接丢弃。
fn parse_member(v: &serde_json::Value) -> Option<MemberSnapshotRow> {
    let user_id = value_str(v, "userId")
        .or_else(|| value_str(v, "id"))
        .unwrap_or_default();
    if user_id.is_empty() {
        return None;
    }
    Some(MemberSnapshotRow {
        user_id,
        role: non_empty(value_str(v, "role").unwrap_or_default(), "member"),
        display_name: value_str(v, "displayName").unwrap_or_default(),
        email: value_str(v, "email").unwrap_or_default(),
        seat_status: non_empty(value_str(v, "seatStatus").unwrap_or_default(), "active"),
    })
}

/// 取字符串字段（兼容数字值：转成字符串）；字段缺失或类型不符返回 `None`。
fn value_str(v: &serde_json::Value, key: &str) -> Option<String> {
    match v.get(key) {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// 空串回退默认值。
fn non_empty(s: String, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s
    }
}

// ── 强刷（治理操作前置）─────────────────────────────────────────

/// 治理操作前置强刷：快照过期（或从未同步）才真正联网拉取，否则直接放行。
///
/// 超时 / 失败统一返回「云端不可用，成员治理已冻结」——治理类操作必须在新鲜快照上进行。
/// **M2（治理写操作）才会调用**，故此处标记 `allow(dead_code)`。
#[allow(dead_code)] // M2 治理操作前置强刷才会调用
pub async fn try_refresh_blocking(
    db: &crate::DbState,
    state: &EnclaveState,
    timeout: Duration,
) -> Result<(), String> {
    let latest = state.store.with_conn(org::latest_synced_at)?;
    let now = chrono::Utc::now().timestamp();
    // 从未同步（latest=0）或已过期（gap ≥ SNAPSHOT_STALE_MAX_SECS）才强刷。
    if latest != 0 && now.saturating_sub(latest) < SNAPSHOT_STALE_MAX_SECS {
        return Ok(());
    }
    match tokio::time::timeout(timeout, run_snapshot_round(db, state)).await {
        Ok(Ok(_)) => Ok(()),
        _ => Err("云端不可用，成员治理已冻结".to_string()),
    }
}

// ── 后台自动同步 ────────────────────────────────────────────────

/// 后台自动治理快照循环：应用启动后挂上，每轮结束时读一次间隔设置再休眠。
///
/// 成功 → 按主库设置 [`SNAPSHOT_INTERVAL_KEY`] 休眠；失败 → 指数退避
/// （30s 起，翻倍至上限 15min，**成功即重置**）。失败只 `log::debug!`，
/// **绝不 panic、不阻塞 UI**（未登录会持续退避重试，登录后即恢复）。
pub async fn run_auto_snapshot_loop(db: crate::DbState, state: Arc<EnclaveState>) {
    let mut backoff = Duration::from_secs(BACKOFF_START_SECS);
    loop {
        match run_snapshot_round(&db, &state).await {
            Ok(o) => {
                backoff = Duration::from_secs(BACKOFF_START_SECS);
                log::info!(
                    "[Enclave] 治理快照同步完成：组织 {} / 成员 {}",
                    o.org_count,
                    o.member_count
                );
            }
            Err(e) => {
                log::debug!("[Enclave] 治理快照同步跳过：{}", e);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(BACKOFF_MAX_SECS));
                continue;
            }
        }
        // 每轮重新读间隔（主库 app_settings）；读不到用默认值。
        let secs = db
            .get_conn()
            .map(|c| snapshot_interval_secs(&c))
            .unwrap_or(SNAPSHOT_DEFAULT_INTERVAL_SECS);
        tokio::time::sleep(Duration::from_secs(secs.max(1) as u64)).await;
    }
}

/// 读治理快照轮询间隔（秒）：主库 `app_settings` 的 `enclave_snapshot_interval_secs`；
/// 缺省 / 非法 / 非正数 → [`SNAPSHOT_DEFAULT_INTERVAL_SECS`]。
fn snapshot_interval_secs(conn: &rusqlite::Connection) -> i64 {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        rusqlite::params![SNAPSHOT_INTERVAL_KEY],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.trim().parse::<i64>().ok())
    .filter(|s| *s > 0)
    .unwrap_or(SNAPSHOT_DEFAULT_INTERVAL_SECS)
}
