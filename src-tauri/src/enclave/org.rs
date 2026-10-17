//! 飞地治理域：组织 / 成员快照的读写，以及主机本地策略（出云总闸、应急开关）与审计。
//!
//! **纯同步、无 `await`**：全部函数直接吃 `&rusqlite::Connection`，由调用方在
//! [`crate::enclave::db::EnclaveStore::with_conn`] 的同步块内调用（闭包内绝不 await）。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// 组织快照的写入行（来自平台 `/organizations/mine`，字段与 `org_snapshot` 表对齐）。
pub struct OrgSnapshotRow {
    pub org_id: String,
    pub name: String,
    pub owner_user_id: String,
    pub plan_key: String,
    pub plan_expires_at: Option<String>,
    pub seats: i64,
    pub member_count: i64,
    pub status: String,
}

/// 成员快照的写入行（来自平台 `/organizations/:id/members`）。
pub struct MemberSnapshotRow {
    pub user_id: String,
    pub role: String,
    pub display_name: String,
    pub email: String,
    pub seat_status: String,
}

// ── 视图（序列化给前端）────────────────────────────────────────

/// 组织快照视图（只读）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgSnapshotView {
    pub org_id: String,
    pub name: String,
    pub owner_user_id: String,
    pub plan_key: String,
    pub plan_expires_at: Option<String>,
    pub seats: i64,
    pub member_count: i64,
    pub status: String,
    pub synced_at: i64,
}

/// 成员快照视图（只读）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberView {
    pub user_id: String,
    pub role: String,
    pub display_name: String,
    pub email: String,
    pub seat_status: String,
    pub synced_at: i64,
}

/// 组织详情视图：组织信息 + 成员列表（按 owner → admin → member 排序）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgDetailView {
    pub org: OrgSnapshotView,
    pub members: Vec<MemberView>,
}

/// 审计流水视图。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditView {
    pub id: String,
    pub org_id: String,
    pub actor_id: String,
    pub action: String,
    pub detail: String,
    pub created_at: i64,
}

// ── 争夺写入 ────────────────────────────────────────────────────

/// 写入 / 覆盖一个组织快照（按 `org_id` upsert，刷新 `synced_at`）。
pub fn upsert_org_snapshot(conn: &Connection, row: &OrgSnapshotRow) -> Result<(), String> {
    let synced_at = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO org_snapshot
            (org_id, name, owner_user_id, plan_key, plan_expires_at, seats, member_count, status, synced_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(org_id) DO UPDATE SET
            name = excluded.name,
            owner_user_id = excluded.owner_user_id,
            plan_key = excluded.plan_key,
            plan_expires_at = excluded.plan_expires_at,
            seats = excluded.seats,
            member_count = excluded.member_count,
            status = excluded.status,
            synced_at = excluded.synced_at",
        params![
            row.org_id,
            row.name,
            row.owner_user_id,
            row.plan_key,
            row.plan_expires_at,
            row.seats,
            row.member_count,
            row.status,
            synced_at
        ],
    )
    .map_err(|e| format!("写入组织快照失败：{}", e))?;
    Ok(())
}

/// 全量覆盖某组织的成员快照：
/// 1. upsert 传入的成员（刷新 `synced_at`、`seat_status`）；
/// 2. 把该组织下**不在本次列表里**的成员置 `seat_status = 'removed'`（不物理删，保留历史）。
pub fn upsert_member_snapshot(
    conn: &Connection,
    org_id: &str,
    members: &[MemberSnapshotRow],
) -> Result<(), String> {
    let synced_at = chrono::Utc::now().timestamp();
    for m in members {
        conn.execute(
            "INSERT INTO member_snapshot
                (org_id, user_id, role, display_name, email, seat_status, synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(org_id, user_id) DO UPDATE SET
                role = excluded.role,
                display_name = excluded.display_name,
                email = excluded.email,
                seat_status = excluded.seat_status,
                synced_at = excluded.synced_at",
            params![
                org_id,
                m.user_id,
                m.role,
                m.display_name,
                m.email,
                m.seat_status,
                synced_at
            ],
        )
        .map_err(|e| format!("写入成员快照失败：{}", e))?;
    }

    // 本次未出现在列表里的成员 → 置 removed（成员端已离席）。
    // 用动态占位符构造 NOT IN；本次列表为空时把该组织下所有成员都置 removed。
    use rusqlite::types::Value;
    let mut values: Vec<Value> = vec![Value::Text(org_id.to_string())];
    let mut sql =
        String::from("UPDATE member_snapshot SET seat_status = 'removed' WHERE org_id = ?");
    if !members.is_empty() {
        let placeholders = vec!["?"; members.len()].join(", ");
        sql.push_str(&format!(" AND user_id NOT IN ({})", placeholders));
        for m in members {
            values.push(Value::Text(m.user_id.clone()));
        }
    }
    conn.execute(&sql, rusqlite::params_from_iter(values))
        .map_err(|e| format!("标记离席成员失败：{}", e))?;
    Ok(())
}

// ── 读取 ────────────────────────────────────────────────────────

fn map_org_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<OrgSnapshotView> {
    Ok(OrgSnapshotView {
        org_id: r.get(0)?,
        name: r.get(1)?,
        owner_user_id: r.get(2)?,
        plan_key: r.get(3)?,
        plan_expires_at: r.get(4)?,
        seats: r.get(5)?,
        member_count: r.get(6)?,
        status: r.get(7)?,
        synced_at: r.get(8)?,
    })
}

/// 列出本机快照中的全部组织（按名称排序）。
pub fn list_orgs(conn: &Connection) -> Result<Vec<OrgSnapshotView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT org_id, name, owner_user_id, plan_key, plan_expires_at,
                    seats, member_count, status, synced_at
             FROM org_snapshot ORDER BY name",
        )
        .map_err(|e| format!("查询组织快照失败：{}", e))?;
    let rows = stmt
        .query_map([], map_org_row)
        .map_err(|e| format!("查询组织快照失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取组织快照失败：{}", e))
}

/// 取某组织详情（组织信息 + 成员列表）；不存在返回 `None`。
/// 成员按角色排序：owner → admin → member（其余角色排最后）。
pub fn org_detail(
    conn: &Connection,
    org_id: &str,
) -> Result<Option<OrgDetailView>, String> {
    let org = conn
        .query_row(
            "SELECT org_id, name, owner_user_id, plan_key, plan_expires_at,
                    seats, member_count, status, synced_at
             FROM org_snapshot WHERE org_id = ?1",
            params![org_id],
            map_org_row,
        )
        .optional()
        .map_err(|e| format!("查询组织失败：{}", e))?;
    let org = match org {
        Some(o) => o,
        None => return Ok(None),
    };

    let mut stmt = conn
        .prepare(
            "SELECT user_id, role, display_name, email, seat_status, synced_at
             FROM member_snapshot WHERE org_id = ?1
             ORDER BY CASE role
                        WHEN 'owner' THEN 0
                        WHEN 'admin' THEN 1
                        WHEN 'member' THEN 2
                        ELSE 3
                      END,
                      display_name",
        )
        .map_err(|e| format!("查询成员快照失败：{}", e))?;
    let members = stmt
        .query_map(params![org_id], |r| {
            Ok(MemberView {
                user_id: r.get(0)?,
                role: r.get(1)?,
                display_name: r.get(2)?,
                email: r.get(3)?,
                seat_status: r.get(4)?,
                synced_at: r.get(5)?,
            })
        })
        .map_err(|e| format!("查询成员快照失败：{}", e))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取成员快照失败：{}", e))?;

    Ok(Some(OrgDetailView { org, members }))
}

/// 组织快照中最近一次同步时间（`synced_at` 最大值；无组织返回 0）。
pub fn latest_synced_at(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT MAX(synced_at) FROM org_snapshot", [], |r| {
        r.get::<_, Option<i64>>(0)
    })
    .map(|v| v.unwrap_or(0))
    .map_err(|e| format!("读取组织快照时间失败：{}", e))
}

// ── 主机本地策略 ────────────────────────────────────────────────

/// 读「数据出境」总闸：`Some(bool)`；组织无策略行返回 `None`（视为默认不允许）。
#[allow(dead_code)] // 后续里程碑（策略开关 UI）才会调用
pub fn get_allow_cloud_model(conn: &Connection, org_id: &str) -> Result<Option<bool>, String> {
    conn.query_row(
        "SELECT allow_cloud_model FROM org_policy WHERE org_id = ?1",
        params![org_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map(|o| o.map(|v| v != 0))
    .map_err(|e| format!("读取出云策略失败：{}", e))
}

/// 写「数据出境」总闸（无行则先 INSERT）。
#[allow(dead_code)] // 后续里程碑（策略开关 UI）才会调用
pub fn set_allow_cloud_model(
    conn: &Connection,
    org_id: &str,
    allow: bool,
    actor: &str,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO org_policy (org_id, allow_cloud_model, updated_by, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(org_id) DO UPDATE SET
            allow_cloud_model = excluded.allow_cloud_model,
            updated_by = excluded.updated_by,
            updated_at = excluded.updated_at",
        params![org_id, allow as i64, actor, now],
    )
    .map_err(|e| format!("写入出云策略失败：{}", e))?;
    Ok(())
}

/// 读主机应急开关：`Some(true)` = 飞地对外服务已临时关闭；无行返回 `None`（视为未关闭）。
#[allow(dead_code)] // 后续里程碑（应急开关 UI）才会调用
pub fn get_emergency(conn: &Connection, org_id: &str) -> Result<Option<bool>, String> {
    conn.query_row(
        "SELECT enclave_disabled FROM host_emergency WHERE org_id = ?1",
        params![org_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map(|o| o.map(|v| v != 0))
    .map_err(|e| format!("读取应急开关失败：{}", e))
}

/// 写主机应急开关（无行则先 INSERT）。
#[allow(dead_code)] // 后续里程碑（应急开关 UI）才会调用
pub fn set_emergency(
    conn: &Connection,
    org_id: &str,
    disabled: bool,
    actor: &str,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO host_emergency (org_id, enclave_disabled, updated_by, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(org_id) DO UPDATE SET
            enclave_disabled = excluded.enclave_disabled,
            updated_by = excluded.updated_by,
            updated_at = excluded.updated_at",
        params![org_id, disabled as i64, actor, now],
    )
    .map_err(|e| format!("写入应急开关失败：{}", e))?;
    Ok(())
}

// ── 审计 ────────────────────────────────────────────────────────

/// 写一条审计（**唯一写审计入口**）：策略 / 应急 / 转移变更一律经此落库，便于统一口径与扩展。
/// `detail_json` 为结构化明细的 JSON 字符串（无明细传 `"{}"`）。
#[allow(dead_code)] // 后续里程碑（策略 / 应急 / 转移）才会调用
pub fn audit(
    conn: &Connection,
    org_id: &str,
    actor_id: &str,
    action: &str,
    detail_json: &str,
) -> Result<(), String> {
    let id = crate::utils::new_id();
    let created_at = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO policy_audit (id, org_id, actor_id, action, detail, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, org_id, actor_id, action, detail_json, created_at],
    )
    .map_err(|e| format!("写入审计失败：{}", e))?;
    Ok(())
}

/// 读某组织的审计流水（按 `created_at` 倒序，`limit` 条）。
#[allow(dead_code)] // 后续里程碑（审计视图）才会调用
pub fn list_audit(conn: &Connection, org_id: &str, limit: i64) -> Result<Vec<AuditView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, org_id, actor_id, action, detail, created_at
             FROM policy_audit WHERE org_id = ?1
             ORDER BY created_at DESC LIMIT ?2",
        )
        .map_err(|e| format!("查询审计失败：{}", e))?;
    let rows = stmt
        .query_map(params![org_id, limit], |r| {
            Ok(AuditView {
                id: r.get(0)?,
                org_id: r.get(1)?,
                actor_id: r.get(2)?,
                action: r.get(3)?,
                detail: r.get(4)?,
                created_at: r.get(5)?,
            })
        })
        .map_err(|e| format!("查询审计失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取审计失败：{}", e))
}
