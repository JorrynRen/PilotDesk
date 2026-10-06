//! 云同步（个人向跨设备同步）——产品端同步引擎。
//!
//! 平台接口（`requireAuth`，产品端用 Bearer；令牌复用 [`current_access_token`]）：
//! ```text
//! GET  /api/v1/me/sync/watermark
//! GET  /api/v1/me/sync/objects?since=&kind=          → { watermark, items, hasMore }
//! POST /api/v1/me/sync/push   body { kind, items:[{ key, payload?, deleted?, baseVersion }] }
//!                             → { accepted:[{key,version,seq}], conflicts:[{key,version,updatedAt,deleted}] }
//! ```
//! 语义：`baseVersion` = 本地已知的**远端**版本（新建用 0）；等于远端当前版本才接受（远端 version+1），
//! 小于则冲突。
//!
//! ## 本地优先 + 双向自动同步（先 pull 后 push，冲突不自动选边）
//!
//! - **本地是否真改过**：以内容规范化哈希判据（`sync_state.last_synced_hash`）为准——本地当前内容
//!   哈希 ≠ `last_synced_hash` 才算有真实改动。**不使用** `updated_at` 等易变信号兜底。
//! - **冲突**：pull 应用远端时，若「本地有真实改动 + 远端也变了」→ 先把本地内容留档为本地版本
//!   （origin = `cloud_sync`），主对象**跟随远端**，并记录冲突（对象清单）供 UI 提示；
//!   push 收到 409 同理（留档本地 → 跟随远端 → 记录冲突），**不再自动重提本地内容（撤回「本地赢」）**。
//!   用户若想保留自己的版本，从「版本」回滚即可，届时内容哈希变化会被判为「有真实改动」，
//!   下轮 push 正常上传（baseVersion 已是远端最新，会被接受）。
//! - **删除传播**：push 的删除候选**收窄**为「本地回收站中已跟踪的条目」（`deleted_at IS NOT NULL`）；
//!   本地从未见过 / 已彻底删除（purge）的条目**不推删除**，靠 pull 增量对齐。
//! - **墓碑**：`purge_workflow`（从回收站彻底删除）保留 `sync_state` 行并置 `tombstone = 1`；
//!   墓碑存在期间**阻止该 key 被 pull 复活**（远端 version ≤ 墓碑记录 version → 忽略；
//!   \> 墓碑记录 version → 视为远端发生新事件，清墓碑并接受）。墓碑永久保留。
//! - **墓碑推送标志 `tombstone_pushed`**：墓碑只保证「不被复活」，但**不能保证删除意图已送达云端**——
//!   若彻底删除时离线 / push 失败，云端会永远留着一份活跃副本。为此给墓碑加一个推送标志，语义是
//!   「该墓碑的 `deleted` 是否已成功送达云端」，用于**只补送未送达的删除**（既避免每轮重复推送，
//!   也避免「推失败后下次不再推」）。三种状态组合（`tombstone` / `tombstone_pushed`）：
//!   - 未 purge（活跃或回收站中）：`tombstone = 0`，标志无意义（恒 0）；
//!   - 已 purge 未送达：`tombstone = 1` 且 `tombstone_pushed = 0` → **补送候选**，每轮尝试推 `deleted`，
//!     推成功置 1，失败（网络错误 / 409）保持 0 → 下一轮继续尝试；
//!   - 已 purge 已送达：`tombstone = 1` 且 `tombstone_pushed = 1` → 不再重复推送。
//!   墓碑**永久保留**，仅此标志用于去重推送；pull 拉到该 key 的 `deleted = true` 时视为删除意图已达成，
//!   一并置 `tombstone_pushed = 1` 并推进版本。
//!
//! ## 本次范围：只同步「工作流」（kind = "workflow"）
//!
//! 插件与灵感**暂不同步**：两者的本地主键（插件 id / 灵感 id）是否**跨设备稳定**尚未确认——
//! 若同一资产在两台设备上生成了不同 id，按 id 对齐会产生重复条目或误覆盖；工作流用自身 UUID 作
//! 主键、天然稳定，所以先只做工作流。（若后续确认灵感 id 稳定且改动很小，可扩展 `kind`，见交付报告建议。）
//!
//! 工作流 payload 复用「导出 / 组织共享」的同一套打包/解析逻辑（[`build_workflow_export_json`] /
//! [`apply_workflow_bundle_in_place`]），含子流捆绑，保证跨设备还原一致。
//!
//! ## 同步对象集合：只同步「根工作流」
//!
//! payload 已把子流递归**捆绑**进父级，若子流再作为独立对象同步，会在对端产生重复副本。
//! 因此只把「未被任何工作流作为子流引用」的工作流当作同步对象；子流内容随父级 payload 一起走。
//! 写入子流时会把其**祖先**一并标脏（见 [`mark_workflow_dirty`]），保证子流改动最终随父级上传。
//!
//! 安全：只上传工作流定义（payload 仅工作流 JSON），**不含任何设置 / 密钥 / 凭据**。

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::commands::account::{current_access_token, has_capability};
use crate::commands::app_settings::{get_setting, set_setting};
use crate::commands::workflow::{
    apply_workflow_bundle_in_place, build_workflow_export_json, payload_matches_local_definition,
};
use crate::utils::errors::AppError;
use crate::utils::platform;

/// 云同步开关（**默认关闭**，用户显式开启才同步）：仅 `1` / `true` 视为开启。
pub const SYNC_ENABLED_KEY: &str = "cloud_sync_enabled";
/// 上次同步游标（平台 watermark）
pub const SYNC_CURSOR_KEY: &str = "sync_cursor";
/// 上次同步时间（本地可读字符串，供 UI 展示）
pub const SYNC_LAST_AT_KEY: &str = "sync_last_at";
/// 上次同步结果摘要（含失败原因，供 UI 展示）
pub const SYNC_LAST_RESULT_KEY: &str = "sync_last_result";
/// 云同步能力键（pro / team 才有）
pub const CAP_CLOUD_SYNC: &str = "sync.cloud";

/// 同步对象类型（当前只做工作流）
const KIND_WORKFLOW: &str = "workflow";
/// 单次 push 条目上限（平台契约，见平台 `app/src/services/sync/service.ts::PUSH_MAX_ITEMS`）
const PUSH_MAX_ITEMS: usize = 100;
/// 自动同步间隔设置键（**分钟**）：缺省 10，范围 1..=1440（clamp），循环每轮读取。
pub const SYNC_INTERVAL_KEY: &str = "cloud_sync_interval_minutes";
/// 自动同步间隔默认值（分钟）。
const SYNC_INTERVAL_DEFAULT_MINUTES: u64 = 10;
/// 自动同步间隔下界（分钟）——1 分钟为最小可配粒度。
const SYNC_INTERVAL_MIN_MINUTES: u64 = 1;
/// 自动同步间隔上界（分钟）——1440 分钟 = 24 小时。
const SYNC_INTERVAL_MAX_MINUTES: u64 = 1440;

// ════════════════════════════════════════════════════════════
// 设置读取
// ════════════════════════════════════════════════════════════

/// 是否开启云同步：缺省 / 非 `1`|`true` → 关闭（默认关闭）。
pub fn sync_enabled(conn: &Connection) -> bool {
    match get_setting(conn, SYNC_ENABLED_KEY) {
        Ok(Some(v)) => {
            let t = v.trim().to_ascii_lowercase();
            t == "1" || t == "true"
        }
        _ => false,
    }
}

/// 写开关设置。
pub fn set_sync_enabled(conn: &Connection, enabled: bool) -> Result<(), AppError> {
    set_setting(conn, SYNC_ENABLED_KEY, if enabled { "1" } else { "0" })
}

/// 读自动同步间隔（分钟）：缺省 10，缺省/非法回退默认，越界 clamp 到 1..=1440。
///
/// 由 [`run_auto_sync_loop`] 每轮读取一次 → 在设置页改完无需重启，下一轮即生效。
pub fn sync_interval_minutes(conn: &Connection) -> u64 {
    get_setting(conn, SYNC_INTERVAL_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|m| m.clamp(SYNC_INTERVAL_MIN_MINUTES, SYNC_INTERVAL_MAX_MINUTES))
        .unwrap_or(SYNC_INTERVAL_DEFAULT_MINUTES)
}

/// 读同步游标（上次 watermark；缺省/非法 → 0）。
fn read_cursor(conn: &Connection) -> i64 {
    get_setting(conn, SYNC_CURSOR_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// 写同步游标。
fn write_cursor(conn: &Connection, cursor: i64) -> Result<(), AppError> {
    set_setting(conn, SYNC_CURSOR_KEY, &cursor.to_string())
}

// ════════════════════════════════════════════════════════════
// sync_state 元数据（每个同步对象一行）
// ════════════════════════════════════════════════════════════

/// 读某对象的本地已知远端版本（无记录 → 0，视为新建）。
fn server_version_of(conn: &Connection, key: &str) -> Result<i64, AppError> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT server_version FROM sync_state WHERE kind = ?1 AND object_key = ?2",
            params![KIND_WORKFLOW, key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.unwrap_or(0))
}

/// 工作流定义的**内容规范化哈希**（走导出结构、剔除时间戳/版本号等易变字段）。
///
/// 与 [`crate::commands::workflow::payload_matches_local_definition`] 同一套归一（导出结构 +
/// [`crate::workflow::normalized_content_hash`]），因此「哈希相等」等价于「内容等价」。
fn definition_content_hash(def: &crate::workflow::WorkflowDefinition) -> u64 {
    let v = serde_json::to_value(
        crate::commands::workflow::ExportWorkflowDefinition::from(def.clone()),
    )
    .unwrap_or(serde_json::Value::Null);
    crate::workflow::normalized_content_hash(&v)
}

/// 本地**活跃**（非软删除）工作流当前内容的规范化哈希；本地不存在 → `None`。
fn local_content_hash(conn: &Connection, key: &str) -> Result<Option<u64>, AppError> {
    match crate::workflow::get_active_definition(conn, key)? {
        Some(def) => Ok(Some(definition_content_hash(&def))),
        None => Ok(None),
    }
}

/// 读某对象 `last_synced_hash`（原样字符串；无记录 → `None`）。
fn last_synced_hash_of(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT last_synced_hash FROM sync_state WHERE kind = ?1 AND object_key = ?2",
        params![KIND_WORKFLOW, key],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .unwrap_or(None)
    .flatten()
}

/// 本地当前内容是否**偏离上次成功同步的内容**（即「本地有真实改动」）：
/// - 本地不存在（无活跃定义）→ false（删除另行在删除候选里处理）；
/// - `last_synced_hash` 为空但本地存在 → true（从未同步过 / 被判定需要重推，如从回收站恢复）；
/// - 否则比较「本地当前内容哈希 ≠ last_synced_hash」。
///
/// **判据只依赖内容哈希**，不依赖 `updated_at` / `dirty` 等易变信号。
fn local_differs_from_synced(conn: &Connection, key: &str) -> Result<bool, AppError> {
    let Some(h) = local_content_hash(conn, key)? else {
        return Ok(false);
    };
    Ok(last_synced_hash_of(conn, key).as_deref() != Some(h.to_string().as_str()))
}

/// 读某对象的墓碑状态 `(是否墓碑, 墓碑记录的远端版本)`。
/// 墓碑版本复用 `server_version`（= 彻底删除时本地已知的远端版本）；删除事件被平台接受后该列会推进，
/// 从而本机自己的删除事件不会误触发「清墓碑复活」。
fn tombstone_of(conn: &Connection, key: &str) -> Result<(bool, i64), AppError> {
    let row: Option<(i64, i64)> = conn
        .query_row(
            "SELECT tombstone, server_version FROM sync_state WHERE kind = ?1 AND object_key = ?2",
            params![KIND_WORKFLOW, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((t, v)) => (t != 0, v),
        None => (false, 0),
    })
}

/// 清除某对象的墓碑（远端发生了更高版本的新事件 → 接受复活）。同时复位 `tombstone_pushed`
/// （墓碑已清，推送标志随之失效；下次若再 purge 会由 [`mark_workflow_purged`] 重新置 0）。
fn clear_tombstone(conn: &Connection, key: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE sync_state SET tombstone = 0, tombstone_pushed = 0
         WHERE kind = ?1 AND object_key = ?2",
        params![KIND_WORKFLOW, key],
    )?;
    Ok(())
}

/// 读某墓碑的「删除意图是否已送达云端」标志（无记录 → false）。
/// 仅单测直接读取该列；生产路径由 `list_push_candidates` 的 SQL 判据消费，故只在测试构建编译。
#[cfg(test)]
fn tombstone_pushed_of(conn: &Connection, key: &str) -> Result<bool, AppError> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT tombstone_pushed FROM sync_state WHERE kind = ?1 AND object_key = ?2",
            params![KIND_WORKFLOW, key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.unwrap_or(0) != 0)
}

/// 标记某墓碑的删除意图**已送达云端**（`tombstone_pushed = 1`），并可选地把已知远端版本推进到
/// `server_version`（`Some(v)` 且 `v` 更大时用 `MAX` 前推；`None` = 不动版本，用于「push 接受回执
/// 已由 [`upsert_sync_state`] 更新版本」的场景）。
///
/// **仅作用于墓碑行**（`tombstone = 1`）——对普通条目是 no-op，因此可安全地在本轮 push 的每个
/// accepted 回执上调用，不会污染常规同步对象。
fn mark_tombstone_pushed(
    conn: &Connection,
    key: &str,
    server_version: Option<i64>,
) -> Result<(), AppError> {
    match server_version {
        Some(v) => conn.execute(
            "UPDATE sync_state
                SET tombstone_pushed = 1, server_version = MAX(server_version, ?3)
              WHERE kind = ?1 AND object_key = ?2 AND tombstone = 1",
            params![KIND_WORKFLOW, key, v],
        )?,
        None => conn.execute(
            "UPDATE sync_state SET tombstone_pushed = 1
              WHERE kind = ?1 AND object_key = ?2 AND tombstone = 1",
            params![KIND_WORKFLOW, key],
        )?,
    };
    Ok(())
}

/// 写 sync_state（存在则覆盖 `server_version` / `dirty` / `last_synced_hash`；**不动 `tombstone`**）。
///
/// `last_synced_hash` 只在「本地内容与远端已一致」时写入（pull 应用完成 / push 成功）——
/// 因此「哈希相等」等价于「与远端一致」。
fn upsert_sync_state(
    conn: &Connection,
    key: &str,
    server_version: i64,
    dirty: bool,
    last_synced_hash: Option<u64>,
) -> Result<(), AppError> {
    let hash = last_synced_hash.map(|h| h.to_string());
    conn.execute(
        "INSERT INTO sync_state (kind, object_key, server_version, local_updated_at, dirty, last_synced_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(kind, object_key)
         DO UPDATE SET server_version = ?3, local_updated_at = ?4, dirty = ?5, last_synced_hash = ?6",
        params![
            KIND_WORKFLOW,
            key,
            server_version,
            crate::utils::now(),
            dirty as i64,
            hash
        ],
    )?;
    Ok(())
}

/// 从回收站**恢复**工作流后的同步语义：该 key 视为「本地有真实改动」，确保下轮 push 会把它
/// 作为新版本推上去（**云端复活**）。做法：置 `dirty = 1` 且清空 `last_synced_hash`
/// （哈希为空 + 本地存在 → `local_differs_from_synced` 为真）。
/// **best-effort**：表缺失 / 写失败静默忽略。
pub fn mark_workflow_restored(conn: &Connection, id: &str) {
    if id.is_empty() {
        return;
    }
    if let Err(e) = conn.execute(
        "UPDATE sync_state SET dirty = 1, last_synced_hash = NULL WHERE kind = ?1 AND object_key = ?2",
        params![KIND_WORKFLOW, id],
    ) {
        log::debug!("[CloudSync] 恢复标脏失败 {}：{}", id, e);
    }
}

/// **彻底删除**（purge）工作流后的同步语义：保留 `sync_state` 行并置 `tombstone = 1`、
/// `tombstone_pushed = 0`，同时清空 `last_synced_hash`；**不删除** `sync_state` 行，**不动**
/// `server_version`。
///
/// - `tombstone = 1`：阻止该 key 被 pull 复活；
/// - `tombstone_pushed = 0`：删除意图尚未送达云端 → 进入**补送候选**（每轮尝试推 `deleted`，
///   成功后才置 1），从而「离线 / 失败时不会永久丢删除、送达后不重复推送」；
/// - 保留 `server_version`：作为补送 `deleted` 时的 `baseVersion`。
///
/// 墓碑**永久保留**（不做自动清理），仅 `tombstone_pushed` 用于去重推送。
/// **best-effort**：无该行（从未跟踪）/ 表缺失 / 写失败静默忽略。
pub fn mark_workflow_purged(conn: &Connection, id: &str) {
    if id.is_empty() {
        return;
    }
    if let Err(e) = conn.execute(
        "UPDATE sync_state SET tombstone = 1, tombstone_pushed = 0, dirty = 0, last_synced_hash = NULL
         WHERE kind = ?1 AND object_key = ?2",
        params![KIND_WORKFLOW, id],
    ) {
        log::debug!("[CloudSync] 置墓碑失败 {}：{}", id, e);
    }
}

// ════════════════════════════════════════════════════════════
// 脏标记（由工作流的集中写入点调用）
// ════════════════════════════════════════════════════════════

/// 标记某工作流本地已改动：置 `dirty = 1`，并把「把它作为子流引用的祖先」也一并标脏。
///
/// 为什么连祖先一起标：子流不是独立同步对象，其内容只能随父级 payload 上传，所以改了子流必须
/// 让根工作流变脏，否则子流改动永远传不出去。
///
/// **best-effort**：sync_state 表不存在（老库未就绪 / 单测内存库）或写失败时静默忽略，
/// 绝不影响本地工作流的正常读写。
pub fn mark_workflow_dirty(conn: &Connection, id: &str) {
    if id.is_empty() {
        return;
    }
    if let Err(e) = mark_dirty_row(conn, id) {
        log::debug!("[CloudSync] 标记脏位失败 {}：{}", id, e);
        return;
    }
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(id.to_string());
    if let Err(e) = mark_ancestors_dirty(conn, id, &mut visited) {
        log::debug!("[CloudSync] 传播祖先脏位失败 {}：{}", id, e);
    }
}

/// 只置本行脏位（不新建为 0 版本则保留原 server_version）。
fn mark_dirty_row(conn: &Connection, id: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO sync_state (kind, object_key, server_version, local_updated_at, dirty)
         VALUES (?1, ?2, 0, ?3, 1)
         ON CONFLICT(kind, object_key) DO UPDATE SET dirty = 1, local_updated_at = ?3",
        params![KIND_WORKFLOW, id, crate::utils::now()],
    )?;
    Ok(())
}

/// 递归把引用 `child_id` 的工作流标脏（`visited` 去重，防环）。
fn mark_ancestors_dirty(
    conn: &Connection,
    child_id: &str,
    visited: &mut HashSet<String>,
) -> Result<(), AppError> {
    for parent in workflows_referencing(conn, child_id)? {
        if !visited.insert(parent.clone()) {
            continue;
        }
        mark_dirty_row(conn, &parent)?;
        mark_ancestors_dirty(conn, &parent, visited)?;
    }
    Ok(())
}

/// 找出把 `child_id` 作为子流（Subflow 节点 `params.definitionId`）引用的工作流 id 列表。
pub(crate) fn workflows_referencing(
    conn: &Connection,
    child_id: &str,
) -> Result<Vec<String>, AppError> {
    let defs = crate::workflow::list_definitions(conn)?;
    let mut out = Vec::new();
    for d in defs {
        if d.id == child_id {
            continue;
        }
        let referenced = d.stages.iter().any(|s| {
            s.nodes.iter().any(|n| {
                n.node_type == crate::workflow::WorkflowNodeType::Subflow
                    && n.params
                        .as_ref()
                        .and_then(|p| p.get("definitionId"))
                        .and_then(|v| v.as_str())
                        == Some(child_id)
            })
        });
        if referenced {
            out.push(d.id);
        }
    }
    Ok(out)
}

/// 当前被任何工作流作为子流引用的全部 id（这些不做独立同步对象）。
fn referenced_workflow_ids(conn: &Connection) -> Result<HashSet<String>, AppError> {
    let defs = crate::workflow::list_definitions(conn)?;
    let mut set = HashSet::new();
    for d in &defs {
        for s in &d.stages {
            for n in &s.nodes {
                if n.node_type == crate::workflow::WorkflowNodeType::Subflow {
                    if let Some(id) = n
                        .params
                        .as_ref()
                        .and_then(|p| p.get("definitionId"))
                        .and_then(|v| v.as_str())
                    {
                        set.insert(id.to_string());
                    }
                }
            }
        }
    }
    Ok(set)
}

// ════════════════════════════════════════════════════════════
// 平台响应 / 请求结构
// ════════════════════════════════════════════════════════════

/// 远端对象条目（GET /objects 的 items 元素）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct RemoteItem {
    #[serde(default)]
    key: String,
    #[serde(default)]
    version: i64,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    payload: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

/// GET /objects 响应。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullResp {
    #[serde(default)]
    watermark: i64,
    #[serde(default)]
    items: Vec<RemoteItem>,
    #[serde(default)]
    has_more: bool,
}

/// push 请求元素。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PushItem {
    key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<String>,
    /// 软删除标记（仅删除时下发，保留时省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted: Option<bool>,
    base_version: i64,
}

/// push 请求体。
#[derive(Debug, Serialize)]
struct PushBody {
    kind: &'static str,
    items: Vec<PushItem>,
}

/// push 响应。
#[derive(Debug, Deserialize)]
struct PushResp {
    #[serde(default)]
    accepted: Vec<AcceptedItem>,
    #[serde(default)]
    conflicts: Vec<ConflictItem>,
}

/// push 被接受的条目（`seq` 平台内部序号，本地不落库——游标用 watermark，见模块注释）。
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AcceptedItem {
    #[serde(default)]
    key: String,
    #[serde(default)]
    version: i64,
    #[serde(default)]
    seq: Option<i64>,
}

/// push 冲突条目。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ConflictItem {
    #[serde(default)]
    key: String,
    #[serde(default)]
    version: i64,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    deleted: bool,
}

// ════════════════════════════════════════════════════════════
// 结果 / 状态视图
// ════════════════════════════════════════════════════════════

/// 一个冲突对象（本地与远端同时改动）：仅在 UI 提示用（id + 名称）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    pub id: String,
    pub name: String,
}

/// 一轮同步结果（失败不抛异常，原因放 `error` 文案）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSyncResult {
    pub pulled: usize,
    pub pushed: usize,
    pub conflicts: usize,
    /// 本轮冲突对象清单（本地改动已留档为本地版本，主对象已跟随远端；供 UI 提示）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflict_items: Vec<SyncConflict>,
    /// 打包失败（本轮未能上传）的对象数
    pub failed: usize,
    /// 失败对象与原因（供 UI 展示；如「体积超限」）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CloudSyncResult {
    fn failure(msg: impl Into<String>) -> Self {
        Self {
            error: Some(msg.into()),
            ..Default::default()
        }
    }
}

/// 同步状态（只读，供 UI）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSyncStatus {
    pub enabled: bool,
    pub capability_ok: bool,
    pub last_at: Option<String>,
    pub last_result: Option<String>,
    /// 本地已登记（被跟踪）的同步对象数
    pub object_count: i64,
    /// 本地根工作流数（未被任何工作流作为子流引用）
    pub root_count: i64,
    /// 本地子工作流数（被其它工作流以 Subflow 节点引用，不单独同步，随主工作流一并上传）
    pub subflow_count: i64,
    pub cursor: i64,
}

// ════════════════════════════════════════════════════════════
// 纯决策函数（便于单测）
// ════════════════════════════════════════════════════════════

/// 是否用远端条目覆盖本地：远端版本严格大于本地已知版本才应用（软删除同理）。
fn pull_should_apply(remote_version: i64, known_version: i64) -> bool {
    remote_version > known_version
}

/// push 时对某脏对象采取的动作。
#[derive(Debug, PartialEq, Eq)]
enum PushAction {
    /// 打包上传
    Upload,
    /// 推送软删除
    Delete,
    /// 跳过（子流不独立同步，随父级 payload 走）
    Skip,
}

/// push 决策：本地存在且是根工作流 → 上传；本地不存在 → 删除；本地存在但是子流 → 跳过。
fn decide_push(local_exists: bool, is_referenced_as_subflow: bool) -> PushAction {
    if local_exists {
        if is_referenced_as_subflow {
            PushAction::Skip
        } else {
            PushAction::Upload
        }
    } else {
        PushAction::Delete
    }
}

// ════════════════════════════════════════════════════════════
// pull / push 主体
// ════════════════════════════════════════════════════════════

/// 把多页 pull 响应折叠为 `(最终 watermark, 全部条目)`（**纯函数**，便于单测翻页取全）。
///
/// 语义与 [`fetch_all_objects`] 的循环一致：逐页追加条目；`watermark` 取**最后一页**返回的值
/// （平台按 `watermark` 继续拉，最后一页即全局最大水位）；`has_more` 只用于决定是否继续请求，
/// 折叠本身不依赖它。无页时返回 `since`。
fn fold_pull_pages(since: i64, pages: Vec<(i64, Vec<RemoteItem>, bool)>) -> (i64, Vec<RemoteItem>) {
    let mut watermark = since;
    let mut out = Vec::new();
    for (page_watermark, items, _has_more) in pages {
        watermark = page_watermark;
        out.extend(items);
    }
    (watermark, out)
}

/// 拉取自 `since` 之后的全部对象（自动翻页直到 `hasMore = false`）。
/// 返回 `(最终 watermark, 全部条目)`。
async fn fetch_all_objects(token: &str, since: i64) -> Result<(i64, Vec<RemoteItem>), AppError> {
    let mut pages: Vec<(i64, Vec<RemoteItem>, bool)> = Vec::new();
    let mut cursor = since;
    loop {
        let path = format!(
            "/api/v1/me/sync/objects?since={}&kind={}",
            cursor, KIND_WORKFLOW
        );
        let resp: PullResp = platform::get_json(&path, Some(token)).await?;
        cursor = resp.watermark;
        let has_more = resp.has_more;
        pages.push((cursor, resp.items, has_more));
        if !has_more {
            break;
        }
    }
    Ok(fold_pull_pages(since, pages))
}

/// 应用单条远端条目的结果。
#[derive(Debug, Default)]
struct ApplyOutcome {
    /// 是否真的落地（写了内容 / 改了删除态）
    applied: bool,
    /// 判定为冲突时记录的对象（本地有真实改动 + 远端也变了）
    conflict: Option<SyncConflict>,
}

/// 应用一批远端条目，返回 `(实际落地条数, 本轮冲突清单)`。
fn apply_remote_items(conn: &Connection, items: &[RemoteItem]) -> (usize, Vec<SyncConflict>) {
    let mut applied = 0usize;
    let mut conflicts = Vec::new();
    for item in items {
        match apply_one_remote_item(conn, item) {
            Ok(o) => {
                if o.applied {
                    applied += 1;
                }
                if let Some(c) = o.conflict {
                    conflicts.push(c);
                }
            }
            Err(e) => log::warn!("[CloudSync] 应用远端对象失败 {}：{}", item.key, e),
        }
    }
    (applied, conflicts)
}

/// 应用单条远端条目。
///
/// - **墓碑守卫**（`tombstone = 1`，本机已彻底删除该 key）：
///   - 远端 `deleted = true` → 云端已是删除态 → 删除意图**已达成**：置 `tombstone_pushed = 1` 并推进
///     `server_version`，本机早已真删、无需其它动作（返回 `applied = false`，未改本地数据）；
///   - 远端 `deleted = false` 且 `remote_version > 墓碑记录 version` → 远端发生更高版本新事件 →
///     **清墓碑并接受复活**（尊重远端显式事件，沿用既有规则）；
///   - 远端 `deleted = false` 且 `remote_version ≤ 墓碑记录 version` → **保持墓碑、等待补推**（不复活）；
/// - 版本判据保持：仅 `remote_version > known_version` 才应用；
/// - 本地已有该工作流且内容与远端规范化后**相同** → 直接跳过（不写内容、不建快照），只推进 `sync_state`；
/// - 本地**有真实改动**（`local_differs_from_synced`）且远端也变了 → 判定**冲突**：先
///   `snapshot_before_overwrite`（origin = `cloud_sync`）把本地内容留档为本地版本，再应用远端内容
///   （主对象**跟随远端**），并返回冲突记录；本地无真实改动 → 直接应用（正常跟随）；
/// - 应用成功后把 `last_synced_hash` 更新为**远端内容哈希**（此处本地内容已等于远端），
///   因此「哈希相等 = 与远端一致」；
/// - 软删除分支：软删除前同样先打快照（便于从回收站恢复后回滚），`last_synced_hash` 置空。
fn apply_one_remote_item(conn: &Connection, item: &RemoteItem) -> Result<ApplyOutcome, AppError> {
    if item.key.is_empty() {
        return Ok(ApplyOutcome::default());
    }

    // 墓碑守卫：墓碑记录的版本 = server_version（本地已知的远端版本）
    let (tombstone, tomb_version) = tombstone_of(conn, &item.key)?;
    if tombstone {
        if item.deleted {
            // 云端已是删除态 → 删除意图已达成：标记已送达并推进版本；本机已真删，无其它动作。
            mark_tombstone_pushed(conn, &item.key, Some(item.version))?;
            return Ok(ApplyOutcome::default());
        }
        if item.version <= tomb_version {
            return Ok(ApplyOutcome::default()); // 未产生更高版本新事件 → 保持墓碑、等待补推（不复活）
        }
        // 远端版本 > 墓碑记录版本且 deleted=false → 远端发生了更高版本的新事件（另一设备显式恢复/更新）
        // → 清墓碑并接受复活，继续按正常流程应用。
        clear_tombstone(conn, &item.key)?;
    }

    if !pull_should_apply(item.version, server_version_of(conn, &item.key)?) {
        return Ok(ApplyOutcome::default());
    }

    if item.deleted {
        // 远端删除传播 → 本地**软删除**（进回收站），不真删数据。删除前给本地内容留一个版本快照，
        // 便于用户从回收站恢复后仍可回滚到删除前的状态。
        if let Err(e) = crate::workflow::snapshot_before_overwrite(
            conn,
            &item.key,
            crate::workflow::VERSION_ORIGIN_CLOUD_SYNC,
        ) {
            log::warn!("[CloudSync] 软删除前保存版本快照失败 {}：{}", item.key, e);
        }
        crate::workflow::soft_delete_definition(conn, &item.key)
            .map_err(|e| AppError::Db(format!("删除失败: {}", e)))?;
        // 本地内容已软删除 → 与远端一致（都处于删除态），哈希置空
        upsert_sync_state(conn, &item.key, item.version, false, None)?;
        return Ok(ApplyOutcome {
            applied: true,
            conflict: None,
        });
    }

    match item.payload.as_deref() {
        Some(payload) if !payload.trim().is_empty() => {
            // 无改动 → 跳过内容写入与快照，仅推进 sync_state（避免版本表膨胀）
            if payload_matches_local_definition(conn, &item.key, payload)? {
                let h = local_content_hash(conn, &item.key)?;
                upsert_sync_state(conn, &item.key, item.version, false, h)?;
                return Ok(ApplyOutcome::default());
            }
            // 本地有真实改动 + 远端也变了 → 冲突：先给本地内容留档为本地版本，再应用远端（主对象跟随远端）
            let conflict = local_differs_from_synced(conn, &item.key)?;
            let conflict_record = if conflict {
                let name = crate::workflow::get_definition(conn, &item.key)?
                    .map(|d| d.name)
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| item.key.clone());
                if let Err(e) = crate::workflow::snapshot_before_overwrite(
                    conn,
                    &item.key,
                    crate::workflow::VERSION_ORIGIN_CLOUD_SYNC,
                ) {
                    log::warn!("[CloudSync] 冲突留档本地版本失败 {}：{}", item.key, e);
                }
                Some(SyncConflict {
                    id: item.key.clone(),
                    name,
                })
            } else {
                None
            };
            apply_workflow_bundle_in_place(conn, &item.key, payload)?;
            // 应用后本地内容 = 远端内容 → 记录为已同步
            let h = local_content_hash(conn, &item.key)?;
            upsert_sync_state(conn, &item.key, item.version, false, h)?;
            Ok(ApplyOutcome {
                applied: true,
                conflict: conflict_record,
            })
        }
        // payload 缺失（协议异常）：只推进版本、保留原哈希，避免反复拉取（无法判定本地是否与远端一致）
        _ => {
            let h = last_synced_hash_of(conn, &item.key).and_then(|s| s.parse::<u64>().ok());
            upsert_sync_state(conn, &item.key, item.version, false, h)?;
            Ok(ApplyOutcome {
                applied: true,
                conflict: None,
            })
        }
    }
}

/// 本轮推送的候选对象 `(key, baseVersion)`：
/// - **上传候选**（本地存在）：本地内容哈希 ≠ `last_synced_hash`（[`local_differs_from_synced`]）
///   才纳入——即「本地有真实改动」的唯一可靠判据；从未跟踪（无 `sync_state` 行）时哈希为空、
///   本地存在 → 视为有改动，baseVersion = 0（首次整体上传）；
/// - **删除候选**（本地已删除）：**仅收窄为「本地回收站中的、已跟踪的根工作流」**
///   （`deleted_at IS NOT NULL`，且远端已知 `server_version > 0`、且未推送 `dirty = 1`）；
///   本地从未见过（远端无此对象）的条目**不推删除**，靠 pull 增量对齐；
/// - **墓碑补送候选**（已 purge 但删除意图未送达）：`tombstone = 1` 且 `tombstone_pushed = 0`
///   且远端已知（`server_version > 0`）→ 推 `deleted`（baseVersion = 该行 `server_version`）；
///   送达后置 `tombstone_pushed = 1` → 此后不再进入候选（不重复推送）；失败则保持 0 → 下一轮继续。
///
/// 注：`dirty` 仅作「回收站删除意图是否已推送」的快速判据（删除无内容哈希可比），上传判据一律以内容哈希为准；
/// 墓碑补送的「已送达」判据用独立的 `tombstone_pushed`（见模块注释），与 ② 的 `dirty` 互不干扰。
fn list_push_candidates(conn: &Connection) -> Result<Vec<(String, i64)>, AppError> {
    let mut candidates: HashMap<String, i64> = HashMap::new();

    // ① 上传候选：活跃定义且本地内容偏离上次同步内容
    for d in crate::workflow::list_definitions(conn)? {
        if local_differs_from_synced(conn, &d.id)? {
            let base = server_version_of(conn, &d.id)?;
            candidates.insert(d.id, base);
        }
    }

    // ② 删除候选：回收站中的根工作流（已跟踪、远端已知、删除未推送）
    let referenced = referenced_workflow_ids(conn)?;
    let tracked: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT object_key FROM sync_state WHERE kind = ?1")?;
        let keys: Vec<String> = stmt
            .query_map(params![KIND_WORKFLOW], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        keys.into_iter().collect()
    };
    for d in crate::workflow::list_deleted_workflows(conn)? {
        if candidates.contains_key(&d.id) || !tracked.contains(&d.id) || referenced.contains(&d.id) {
            continue;
        }
        let (is_tombstone, server_version) = tombstone_of(conn, &d.id)?;
        // 墓碑（已彻底删除过）不推删除；远端未知（server_version = 0）不推删除；删除已推送（dirty=0）不重复推
        if is_tombstone || server_version <= 0 || !is_dirty(conn, &d.id) {
            continue;
        }
        candidates.insert(d.id, server_version);
    }

    // ③ 墓碑补送候选：已彻底删除（tombstone = 1）但删除意图尚未送达云端（tombstone_pushed = 0）。
    //    本地定义已真删 → 不会出现在 ①；此类只能靠本通道把 `deleted` 补送到云端。
    //    远端未知（server_version = 0，云端本就无副本）无需补送。
    //    与 ② 互不干扰：② 只看「仍在回收站的活跃定义行」（deleted_at IS NOT NULL），本通道只看墓碑行，
    //    两者 key 集合不相交（同一 key 不可能既在回收站又已 purge）。
    {
        let mut stmt = conn.prepare(
            "SELECT object_key, server_version FROM sync_state
              WHERE kind = ?1 AND tombstone = 1 AND tombstone_pushed = 0",
        )?;
        let rows: Vec<(String, i64)> = stmt
            .query_map(params![KIND_WORKFLOW], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        for (key, server_version) in rows {
            if server_version > 0 {
                candidates.entry(key).or_insert(server_version);
            }
        }
    }

    Ok(candidates.into_iter().collect())
}

/// 读某对象的 `dirty` 标记（无记录 → false）。
fn is_dirty(conn: &Connection, key: &str) -> bool {
    conn.query_row(
        "SELECT dirty FROM sync_state WHERE kind = ?1 AND object_key = ?2",
        params![KIND_WORKFLOW, key],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .unwrap_or(None)
    .map(|d| d != 0)
    .unwrap_or(false)
}

/// 收集本地待推送对象 → push items（分批前）。
///
/// 返回 `(push items, 打包失败原因)`：打包失败（如超过平台体积上限、定义缺失）**不静默**——
/// 除记日志外，原因汇总进第二个返回值，由调用方放进本轮结果让 UI 展示。
fn build_push_items(conn: &Connection) -> Result<(Vec<PushItem>, Vec<String>), AppError> {
    let referenced = referenced_workflow_ids(conn)?;
    let mut items = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for (key, server_version) in list_push_candidates(conn)? {
        // 「本地是否存在」用活跃定义判定：已软删除（回收站）⇒ 视为不存在 ⇒ 推送 deleted 动作，
        // 把本地删除传播到远端；否则会把一份已删除的工作流又当内容上传。
        // 墓碑补送候选（已 purge）同样走此路径：本地定义已真删 → exists=false → 推 deleted。
        let def = crate::workflow::get_active_definition(conn, &key)
            .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?;
        let exists = def.is_some();
        match decide_push(exists, referenced.contains(&key)) {
            PushAction::Skip => {}
            PushAction::Delete => items.push(PushItem {
                key,
                payload: None,
                deleted: Some(true),
                base_version: server_version,
            }),
            PushAction::Upload => match build_workflow_export_json(conn, &key) {
                Ok((_name, json)) => items.push(PushItem {
                    key,
                    payload: Some(json),
                    deleted: None,
                    base_version: server_version,
                }),
                Err(e) => {
                    log::warn!("[CloudSync] 打包工作流失败，跳过推送 {}：{}", key, e);
                    let label = def
                        .as_ref()
                        .map(|d| d.name.clone())
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| key.clone());
                    failures.push(format!("{}：{}", label, e));
                }
            },
        }
    }
    Ok((items, failures))
}

/// 按平台单次推送上限把待推送项分批（每批 ≤ [`PUSH_MAX_ITEMS`] 条；**纯函数**，便于单测）。
fn chunk_push_items(items: &[PushItem]) -> Vec<Vec<PushItem>> {
    items.chunks(PUSH_MAX_ITEMS).map(<[PushItem]>::to_vec).collect()
}

// ════════════════════════════════════════════════════════════
// 冲突处理（不自动选边：留档本地 + 跟随远端）
// ════════════════════════════════════════════════════════════

/// 拆分 push 冲突：把**墓碑补送**（已彻底删除对象补推 `deleted`）的 409 单独摘出去。
///
/// 墓碑 409 只表示「远端版本已变」，本机对象已真删、**不进入常规冲突处理**（否则会把远端内容
/// 落回本地 = 复活墓碑）。把它们从 `conflicts` 计数中剔除后，摘要里的「冲突 N」与
/// `conflictItems.len()` 口径一致（都只反映真正留档并跟随远端的冲突）。
///
/// 返回 `(需处理的常规冲突, 墓碑 409 数量)`；墓碑数量仅用于日志，不计入 `conflicts`。
fn split_tombstone_conflicts(
    conn: &Connection,
    conflicts: Vec<ConflictItem>,
) -> Result<(Vec<ConflictItem>, usize), AppError> {
    let mut real = Vec::new();
    let mut tombstone = 0usize;
    for c in conflicts {
        if c.key.is_empty() {
            continue;
        }
        if tombstone_of(conn, &c.key)?.0 {
            tombstone += 1;
        } else {
            real.push(c);
        }
    }
    Ok((real, tombstone))
}

/// 处理 push 冲突：**不自动选边**（撤回「本地赢」），把「冲突恢复权」交给用户。
///
/// 对每个冲突对象：
/// 1. 取回**远端最新内容**；
/// 2. 先把**本地内容**留档为本地版本（origin = `cloud_sync`，挂在原对象上）——用户改动不丢；
/// 3. 主对象**跟随远端**：远端软删除 → 本地软删除；否则就地应用远端内容；
/// 4. 更新 `sync_state`（server_version = 远端版本、last_synced_hash = 应用后本地内容哈希）；
/// 5. 记录冲突（id + 名称）供 UI 提示。
///
/// **不再以远端版本为 baseVersion 自动重提本地内容**。用户若想保留自己的版本，从「版本」回滚即可，
/// 届时内容哈希不同 → 下轮 push 视为有真实改动 → 正常上传（baseVersion 已是远端最新，会被接受）。
///
/// 返回 `(本轮冲突清单, 失败原因列表)`。
async fn resolve_conflicts(
    state: &crate::DbState,
    token: &str,
    conflicts: &[ConflictItem],
) -> Result<(Vec<SyncConflict>, Vec<String>), AppError> {
    if conflicts.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    // 取回远端最新内容（since=0），按 key 归集（用于「跟随远端」）
    let (_watermark, items) = fetch_all_objects(token, 0).await?;
    let remote: HashMap<String, RemoteItem> = items
        .into_iter()
        .filter(|i| !i.key.is_empty())
        .map(|i| (i.key.clone(), i))
        .collect();

    let mut records: Vec<SyncConflict> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for c in conflicts {
        if c.key.is_empty() {
            continue;
        }
        let Ok(conn) = state.get_conn() else {
            continue;
        };
        // 墓碑（已彻底删除）的 push 冲突（补送 `deleted` 时 409：远端刚被改动）**不做任何本地动作**。
        //
        // 自洽性说明：此类 409 只表示「远端版本已变」，本机无需在此选边——**下一轮 pull 会重新判定**：
        // 若远端已产生更高版本的新事件（deleted=false）→ 清墓碑复活；否则（含 deleted=true）→ 保持墓碑，
        // 继续由补送候选重推（tombstone_pushed 保持 0，不会被永久跳过）。因此不需要额外的特殊重试逻辑。
        // 这里必须跳过「跟随远端」流程：本机对象已真删，套用常规冲突处理会把远端内容重新落回本地
        // （等于复活墓碑），违背墓碑语义。
        if tombstone_of(&conn, &c.key)?.0 {
            continue;
        }
        // 名称（取本地工作流名；取不到则用 key）
        let name = crate::workflow::get_definition(&conn, &c.key)?
            .map(|d| d.name)
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| c.key.clone());
        // ① 先留档本地版本（用户改动不丢）
        if let Err(e) = crate::workflow::snapshot_before_overwrite(
            &conn,
            &c.key,
            crate::workflow::VERSION_ORIGIN_CLOUD_SYNC,
        ) {
            log::warn!("[CloudSync] 冲突留档本地版本失败 {}：{}", c.key, e);
        }
        // ② 主对象跟随远端
        match remote.get(&c.key) {
            Some(r) if r.deleted => {
                if let Err(e) =
                    crate::workflow::soft_delete_definition(&conn, &c.key)
                {
                    failures.push(format!("{}：跟随远端删除失败（{}）", name, e));
                } else if let Err(e) = upsert_sync_state(&conn, &c.key, r.version, false, None) {
                    log::warn!("[CloudSync] 冲突后更新 sync_state 失败 {}：{}", c.key, e);
                }
            }
            Some(r) => {
                let payload = r.payload.as_deref().filter(|p| !p.trim().is_empty());
                match payload {
                    Some(payload) => {
                        match apply_workflow_bundle_in_place(&conn, &c.key, payload) {
                            Ok(_) => {
                                let h = local_content_hash(&conn, &c.key)?;
                                if let Err(e) =
                                    upsert_sync_state(&conn, &c.key, r.version, false, h)
                                {
                                    log::warn!(
                                        "[CloudSync] 冲突后更新 sync_state 失败 {}：{}",
                                        c.key,
                                        e
                                    );
                                }
                            }
                            Err(e) => failures.push(format!("{}：跟随远端内容失败（{}）", name, e)),
                        }
                    }
                    // 远端无内容（协议异常）：仅推进版本、保留原哈希
                    None => {
                        let h = last_synced_hash_of(&conn, &c.key)
                            .and_then(|s| s.parse::<u64>().ok());
                        let _ = upsert_sync_state(&conn, &c.key, r.version, false, h);
                    }
                }
            }
            // 远端查不到该对象（异常）：无可跟随内容，仅记录冲突
            None => {}
        }
        records.push(SyncConflict {
            id: c.key.clone(),
            name,
        });
    }
    Ok((records, failures))
}

// ════════════════════════════════════════════════════════════
// 单轮同步
// ════════════════════════════════════════════════════════════

/// 执行一轮云同步（失败不抛异常，原因在返回结果的 `error` 里）。
pub async fn run_sync_round(state: &crate::DbState) -> CloudSyncResult {
    // ① 开关
    let enabled = match state.get_conn() {
        Ok(c) => sync_enabled(&c),
        Err(e) => return CloudSyncResult::failure(format!("数据库连接失败：{}", e)),
    };
    if !enabled {
        return CloudSyncResult::failure("云同步未开启");
    }

    // ② 登录（未登录不覆盖上次结果，仅返回提示）
    let token = match current_access_token(state).await {
        Ok(Some(t)) => t,
        Ok(None) => return CloudSyncResult::failure("未登录平台账号"),
        Err(e) => return CloudSyncResult::failure(format!("获取登录状态失败：{}", e)),
    };

    // ③ 能力（本地缓存判定；缓存缺失时交由服务端 403 裁决）
    match state.get_conn() {
        Ok(c) => {
            if has_capability(&c, CAP_CLOUD_SYNC) == Some(false) {
                return CloudSyncResult::failure("云同步为付费功能，当前等级不含此能力");
            }
        }
        Err(e) => return CloudSyncResult::failure(format!("数据库连接失败：{}", e)),
    }

    let mut result = CloudSyncResult::default();

    // ── pull ──（连接作用域内完成，避免跨 await 持有）
    let cursor = match state.get_conn() {
        Ok(c) => read_cursor(&c),
        Err(_) => 0,
    };
    match fetch_all_objects(&token, cursor).await {
        Ok((watermark, items)) => {
            if let Ok(conn) = state.get_conn() {
                let (applied, conflicts) = apply_remote_items(&conn, &items);
                result.pulled = applied;
                // 冲突计数统一在末尾由 `conflict_items` 派生，保证「冲突 N」与清单条数一致
                result.conflict_items.extend(conflicts);
                let _ = write_cursor(&conn, watermark);
            }
        }
        Err(e) => {
            let r = CloudSyncResult::failure(format!("拉取失败：{}", e));
            save_round_meta(state, &r);
            return r;
        }
    }

    // ── push ──
    let (push_items, failures) = match state.get_conn() {
        Ok(conn) => match build_push_items(&conn) {
            Ok(v) => v,
            Err(e) => {
                let r = CloudSyncResult::failure(format!("收集本地变更失败：{}", e));
                save_round_meta(state, &r);
                return r;
            }
        },
        Err(e) => {
            let r = CloudSyncResult::failure(format!("数据库连接失败：{}", e));
            save_round_meta(state, &r);
            return r;
        }
    };
    // 打包失败的对象计入本轮结果（不静默）：UI 据此提示「N 个失败（原因）」
    result.failed = failures.len();
    result.failures = failures;

    for chunk in chunk_push_items(&push_items) {
        let body = PushBody {
            kind: KIND_WORKFLOW,
            items: chunk,
        };
        let resp: PushResp = match platform::post_json("/api/v1/me/sync/push", &body, Some(&token))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                // 保留本轮已收集的打包失败信息，避免被整轮错误覆盖而丢失
                let mut r = CloudSyncResult::failure(format!("推送失败：{}", e));
                r.failed = result.failed;
                r.failures = result.failures.clone();
                save_round_meta(state, &r);
                return r;
            }
        };
        // accepted 落版本（连接作用域内完成，不跨 await）：本地内容此刻已与远端一致 → 记录哈希
        if let Ok(conn) = state.get_conn() {
            for acc in &resp.accepted {
                let h = local_content_hash(&conn, &acc.key).unwrap_or(None);
                if let Err(e) = upsert_sync_state(&conn, &acc.key, acc.version, false, h) {
                    log::warn!("[CloudSync] 更新已推送对象版本失败 {}：{}", acc.key, e);
                }
                // 若该 key 是墓碑（补送 `deleted` 被接受）→ 删除意图已送达：置 tombstone_pushed = 1，
                // 之后不再进入补送候选。版本已由上面的 upsert 更新，此处无需再动版本。
                // 对普通条目（tombstone = 0）是 no-op。
                if let Err(e) = mark_tombstone_pushed(&conn, &acc.key, None) {
                    log::warn!("[CloudSync] 标记墓碑已推送失败 {}：{}", acc.key, e);
                }
                result.pushed += 1;
            }
        }
        // 冲突处理（不自动选边：留档本地 + 跟随远端；内部自行取连接，不在此持有）。
        // 先摘出墓碑补送的 409：它们不进入常规冲突处理，也不计入 `conflicts`
        // （保证「冲突 N」与 `conflictItems` 始终一致）。
        if !resp.conflicts.is_empty() {
            let (real_conflicts, tombstone_409) = match state.get_conn() {
                Ok(conn) => split_tombstone_conflicts(&conn, resp.conflicts).unwrap_or_else(|e| {
                    log::warn!("[CloudSync] 拆分冲突失败，本轮按无冲突处理：{}", e);
                    (Vec::new(), 0)
                }),
                // 取不到连接：保守起见不处理冲突（与原行为一致：连接失败时不改本地）
                Err(_) => (Vec::new(), 0),
            };
            if tombstone_409 > 0 {
                log::debug!(
                    "[CloudSync] 墓碑补送的 {} 条 409 不计入冲突（等下一轮 pull 重新判定）",
                    tombstone_409
                );
            }
            match resolve_conflicts(state, &token, &real_conflicts).await {
                Ok((records, failures)) => {
                    result.conflict_items.extend(records);
                    result.failed += failures.len();
                    result.failures.extend(failures);
                }
                Err(e) => log::warn!("[CloudSync] 冲突处理失败：{}", e),
            }
        }
    }

    // 冲突计数与清单同源：`conflicts` 恒等于 `conflictItems.len()`（墓碑 409 不进入清单也不计数）
    result.conflicts = result.conflict_items.len();
    save_round_meta(state, &result);
    result
}

/// 保存本轮摘要（时间 + 结果）到设置里，供 UI 展示。
fn save_round_meta(state: &crate::DbState, result: &CloudSyncResult) {
    let Ok(conn) = state.get_conn() else {
        return;
    };
    let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let _ = set_setting(&conn, SYNC_LAST_AT_KEY, &at);
    let text = match &result.error {
        Some(e) => format!("失败：{}", e),
        None => {
            let mut text = format!(
                "拉取 {} / 推送 {} / 冲突 {}",
                result.pulled, result.pushed, result.conflicts
            );
            // 打包失败（如体积超限）不静默：摘要里带上数量与首个原因
            if result.failed > 0 {
                text.push_str(&format!(" / 失败 {}", result.failed));
                if let Some(first) = result.failures.first() {
                    text.push_str(&format!("（{}）", first));
                }
            }
            // 冲突不自动选边：明确提示「改动已留档为本地版本，可在「版本」中查看并回滚」
            if result.conflicts > 0 {
                text.push_str(&format!(
                    "；检测到 {} 个冲突：你的改动已保存为本地版本，可在工作流的「版本」中查看并回滚",
                    result.conflicts
                ));
            }
            text
        }
    };
    let _ = set_setting(&conn, SYNC_LAST_RESULT_KEY, &text);
    log::info!("[CloudSync] {}", text);
}

// ════════════════════════════════════════════════════════════
// Tauri 命令
// ════════════════════════════════════════════════════════════

/// 立即同步一轮。
#[tauri::command]
pub async fn cloud_sync_now(state: tauri::State<'_, crate::DbState>) -> Result<CloudSyncResult, String> {
    Ok(run_sync_round(state.inner()).await)
}

/// 读取同步状态（只读，供 UI）。
#[tauri::command]
pub fn cloud_sync_status(
    state: tauri::State<'_, crate::DbState>,
) -> Result<CloudSyncStatus, String> {
    let conn = state.get_conn().map_err(|e| format!("数据库连接失败：{}", e))?;
    let object_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_state WHERE kind = ?1",
            params![KIND_WORKFLOW],
            |r| r.get(0),
        )
        .unwrap_or(0);
    // 本地根 / 子工作流数（只读，供界面说明「子流随主工作流同步」）：
    // 子流集合复用与同步决策同一判据 [`referenced_workflow_ids`]，保证口径一致。
    let total_defs = crate::workflow::list_definitions(&conn)
        .map(|v| v.len() as i64)
        .unwrap_or(0);
    let subflow_count = referenced_workflow_ids(&conn)
        .map(|s| s.len() as i64)
        .unwrap_or(0)
        .min(total_defs);
    let root_count = (total_defs - subflow_count).max(0);
    Ok(CloudSyncStatus {
        enabled: sync_enabled(&conn),
        capability_ok: has_capability(&conn, CAP_CLOUD_SYNC).unwrap_or(false),
        last_at: get_setting(&conn, SYNC_LAST_AT_KEY).ok().flatten(),
        last_result: get_setting(&conn, SYNC_LAST_RESULT_KEY).ok().flatten(),
        object_count,
        root_count,
        subflow_count,
        cursor: read_cursor(&conn),
    })
}

/// 设置开关；开启时**立即触发一轮**同步并返回该轮结果（前端据此提示）。
///
/// 选择「后端立即触发」而非「前端随后再调 cloud_sync_now」：开启与首轮同步原子化，
/// 避免前端漏调导致「开了但没同步」；关闭时不做任何网络请求。
#[tauri::command]
pub async fn cloud_sync_set_enabled(
    state: tauri::State<'_, crate::DbState>,
    enabled: bool,
) -> Result<CloudSyncResult, String> {
    {
        let conn = state.get_conn().map_err(|e| format!("数据库连接失败：{}", e))?;
        set_sync_enabled(&conn, enabled).map_err(String::from)?;
    }
    if enabled {
        Ok(run_sync_round(state.inner()).await)
    } else {
        Ok(CloudSyncResult::default())
    }
}

// ════════════════════════════════════════════════════════════
// 自动同步
// ════════════════════════════════════════════════════════════

/// 后台自动同步循环：应用启动后挂上，每轮同步结束时读一次间隔设置（分钟）再休眠。
///
/// 间隔由设置 [`SYNC_INTERVAL_KEY`] 控制（缺省 10 分钟，clamp 1..=1440），
/// **每轮重新读取** → 设置页改完无需重启，本次休眠结束后的下一轮即按新间隔。
/// 仅「开关开启 + 已登录 + 有能力」时才真正联网；其余静默跳过；失败只记日志，
/// **不阻塞 UI、不影响任何本地功能**（离线可用）。
pub async fn run_auto_sync_loop(state: crate::DbState) {
    loop {
        let enabled = state.get_conn().map(|c| sync_enabled(&c)).unwrap_or(false);
        if enabled {
            let r = run_sync_round(&state).await;
            match &r.error {
                Some(e) => log::debug!("[CloudSync] 自动同步跳过：{}", e),
                None => log::info!(
                    "[CloudSync] 自动同步完成：拉取 {} / 推送 {} / 冲突 {} / 失败 {}",
                    r.pulled,
                    r.pushed,
                    r.conflicts,
                    r.failed
                ),
            }
        }
        // 每轮读取最新间隔（分钟 → 秒）；读不到回退默认值
        let minutes = state
            .get_conn()
            .map(|c| sync_interval_minutes(&c))
            .unwrap_or(SYNC_INTERVAL_DEFAULT_MINUTES);
        tokio::time::sleep(std::time::Duration::from_secs(minutes * 60)).await;
    }
}

// ════════════════════════════════════════════════════════════
// 单测（纯函数 / 决策分支）
// ════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// pull 决策：仅远端版本严格更大才应用；相等/更小不应用。
    #[test]
    fn pull_apply_only_when_remote_newer() {
        assert!(pull_should_apply(1, 0));
        assert!(pull_should_apply(5, 4));
        assert!(!pull_should_apply(3, 3));
        assert!(!pull_should_apply(2, 9));
        assert!(!pull_should_apply(0, 0));
    }

    /// push 决策：本地存在且根 → 上传；本地不存在 → 删除；本地存在但是子流 → 跳过。
    #[test]
    fn push_decision_branches() {
        assert_eq!(decide_push(true, false), PushAction::Upload);
        assert_eq!(decide_push(true, true), PushAction::Skip);
        assert_eq!(decide_push(false, false), PushAction::Delete);
        assert_eq!(decide_push(false, true), PushAction::Delete);
    }

    /// 开关判据：缺省关闭；只认 '1' / 'true' 为开启。
    #[test]
    fn sync_enabled_default_off() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL);",
        )
        .unwrap();
        assert!(!sync_enabled(&conn), "缺省应关闭");
        set_sync_enabled(&conn, true).unwrap();
        assert!(sync_enabled(&conn));
        set_sync_enabled(&conn, false).unwrap();
        assert!(!sync_enabled(&conn));
        set_setting(&conn, SYNC_ENABLED_KEY, "TRUE").unwrap();
        assert!(sync_enabled(&conn));
        set_setting(&conn, SYNC_ENABLED_KEY, "yes").unwrap();
        assert!(!sync_enabled(&conn), "非 1/true 一律视为关闭");
    }

    /// 游标读写：缺省 0，写入后可读回。
    #[test]
    fn cursor_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL);",
        )
        .unwrap();
        assert_eq!(read_cursor(&conn), 0);
        write_cursor(&conn, 42).unwrap();
        assert_eq!(read_cursor(&conn), 42);
    }

    /// sync_state：初次 upsert 记为新建（version 0），随后可推进版本、清脏并记录内容哈希。
    #[test]
    fn sync_state_upsert_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sync_state (
                kind TEXT NOT NULL, object_key TEXT NOT NULL,
                server_version INTEGER NOT NULL DEFAULT 0,
                local_updated_at INTEGER NOT NULL DEFAULT 0,
                dirty INTEGER NOT NULL DEFAULT 0,
                last_synced_hash TEXT,
                tombstone INTEGER NOT NULL DEFAULT 0,
                tombstone_pushed INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (kind, object_key));",
        )
        .unwrap();
        assert_eq!(server_version_of(&conn, "w1").unwrap(), 0);
        upsert_sync_state(&conn, "w1", 3, true, Some(11)).unwrap();
        assert_eq!(server_version_of(&conn, "w1").unwrap(), 3);
        assert_eq!(last_synced_hash_of(&conn, "w1").as_deref(), Some("11"));
        // 再写一次：版本、脏位、哈希都覆盖
        upsert_sync_state(&conn, "w1", 4, false, Some(22)).unwrap();
        assert_eq!(server_version_of(&conn, "w1").unwrap(), 4);
        assert_eq!(last_synced_hash_of(&conn, "w1").as_deref(), Some("22"));
        let dirty: i64 = conn
            .query_row(
                "SELECT dirty FROM sync_state WHERE kind='workflow' AND object_key='w1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dirty, 0);
    }

    /// 脏标记 best-effort：sync_state 表不存在时不 panic（老库 / 单测内存库场景）。
    #[test]
    fn mark_dirty_is_best_effort_without_table() {
        let conn = Connection::open_in_memory().unwrap();
        // 故意不建 sync_state，也不建 workflow_definitions
        mark_workflow_dirty(&conn, "w1");
        // 不 panic 即通过
    }

    /// 体积超限的工作流：**不静默丢弃**——进不了 push 列表，但原因汇总到 `failures`（供 UI 展示）。
    #[test]
    fn oversized_workflow_reported_not_silently_dropped() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE workflow_definitions (
                id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '', version TEXT NOT NULL DEFAULT '1.0.0',
                description TEXT NOT NULL DEFAULT '',
                trigger TEXT NOT NULL DEFAULT '{\"triggerType\":\"manual\"}',
                stages TEXT NOT NULL DEFAULT '[]', input_schema TEXT, output_schema TEXT, icon TEXT,
                created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 1, deleted_at INTEGER);
             CREATE TABLE sync_state (
                kind TEXT NOT NULL, object_key TEXT NOT NULL,
                server_version INTEGER NOT NULL DEFAULT 0,
                local_updated_at INTEGER NOT NULL DEFAULT 0,
                dirty INTEGER NOT NULL DEFAULT 0,
                last_synced_hash TEXT,
                tombstone INTEGER NOT NULL DEFAULT 0,
                tombstone_pushed INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (kind, object_key));",
        )
        .unwrap();
        // 300K 字符描述 > 256K 上限 → 打包必失败
        let big = "x".repeat(300 * 1024);
        conn.execute(
            "INSERT INTO workflow_definitions (id, name, description) VALUES ('w-big', '超大工作流', ?1)",
            params![big],
        )
        .unwrap();

        let (items, failures) = build_push_items(&conn).unwrap();
        assert!(items.is_empty(), "打包失败的对象不应进入 push 列表");
        assert_eq!(failures.len(), 1);
        assert!(
            failures[0].contains("超大工作流"),
            "失败原因应含对象名：{:?}",
            failures[0]
        );
        assert!(
            failures[0].contains("体积过大"),
            "失败原因应说明体积超限：{:?}",
            failures[0]
        );
    }

    /// 完整 schema + sync_state 的内存库（云同步单测用）。
    fn full_sync_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::init::FINAL_SCHEMA_SQL)
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE sync_state (
                kind TEXT NOT NULL, object_key TEXT NOT NULL,
                server_version INTEGER NOT NULL DEFAULT 0,
                local_updated_at INTEGER NOT NULL DEFAULT 0,
                dirty INTEGER NOT NULL DEFAULT 0,
                last_synced_hash TEXT,
                tombstone INTEGER NOT NULL DEFAULT 0,
                tombstone_pushed INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (kind, object_key));",
        )
        .unwrap();
        conn
    }

    /// 把某对象标记为「已与远端同步」：记录当前本地内容哈希（= 与远端一致）。
    fn mark_synced(conn: &Connection, key: &str, server_version: i64) {
        let h = local_content_hash(conn, key).unwrap();
        upsert_sync_state(conn, key, server_version, false, h).unwrap();
    }

    fn sync_def(id: &str, name: &str) -> crate::workflow::WorkflowDefinition {
        crate::workflow::WorkflowDefinition {
            id: id.into(),
            name: name.into(),
            version: "1.0.0".into(),
            description: String::new(),
            trigger: crate::workflow::TriggerConfig {
                trigger_type: crate::workflow::TriggerType::Manual,
                cron: None,
                event_name: None,
            },
            stages: vec![],
            icon: None,
            input_schema: None,
            output_schema: None,
            created_at: 1,
            updated_at: 1,
            enabled: true,
        }
    }

    /// 远端删除传播到本地 → 本地**软删除**（进回收站），不真删。
    #[test]
    fn remote_delete_soft_deletes_locally() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "被远端删除的工作流")).unwrap();
        // 已跟踪且内容与远端一致（软删除前会先给本地内容留版本快照，此处不影响删除断言）
        mark_synced(&conn, "w1", 1);

        let item = RemoteItem {
            key: "w1".to_string(),
            version: 2,
            deleted: true,
            payload: None,
            updated_at: None,
        };
        let outcome = apply_one_remote_item(&conn, &item).unwrap();
        assert!(outcome.applied, "远端删除应落地");
        assert!(outcome.conflict.is_none(), "本地无改动 → 非冲突");

        assert!(
            crate::workflow::get_definition(&conn, "w1").unwrap().is_some(),
            "软删除应保留定义行"
        );
        assert!(crate::workflow::is_definition_deleted(&conn, "w1").unwrap());
        assert!(
            crate::workflow::list_definitions(&conn).unwrap().is_empty(),
            "已删除项应从工作流列表隐藏"
        );
        assert_eq!(
            crate::workflow::list_deleted_workflows(&conn).unwrap().len(),
            1,
            "远端删除应落到回收站"
        );
        // 软删除后本地内容已不存在 → last_synced_hash 置空
        assert_eq!(last_synced_hash_of(&conn, "w1"), None);
    }

    /// 本地软删除的**已跟踪**工作流 → push 选择「删除」动作（deleted=true），而不是把已删内容又上传。
    #[test]
    fn soft_deleted_local_pushes_delete_action() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "本地删除的工作流")).unwrap();
        // 先与远端同步过（server_version > 0），再本地删除 → 应作为删除候选推送
        mark_synced(&conn, "w1", 3);
        crate::workflow::soft_delete_definition(&conn, "w1").unwrap();

        let (items, _failures) = build_push_items(&conn).unwrap();
        let item = items
            .iter()
            .find(|i| i.key == "w1")
            .expect("回收站中的已跟踪对象应进入 push 删除候选");
        assert_eq!(item.deleted, Some(true), "本地已软删除应推送删除动作");
        assert!(item.payload.is_none(), "删除动作不应携带内容 payload");
        assert_eq!(item.base_version, 3, "baseVersion 应为已登记的远端版本");
    }

    // ── 批次 3 / 4 / 5：版本备份、覆盖语义、哈希判据、删除传播与墓碑 ──

    /// pull：本地内容与远端相同 → 跳过（不写、不建快照），只推进 sync_state。
    #[test]
    fn pull_skips_when_content_unchanged() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "内容稳定的工作流")).unwrap();
        let (_n, payload) = build_workflow_export_json(&conn, "w1").unwrap();
        mark_synced(&conn, "w1", 1);

        let item = RemoteItem {
            key: "w1".to_string(),
            version: 2,
            deleted: false,
            payload: Some(payload),
            updated_at: None,
        };
        let outcome = apply_one_remote_item(&conn, &item).unwrap();
        assert!(!outcome.applied, "内容相同 → 跳过（未落地）");
        assert!(outcome.conflict.is_none(), "内容相同 → 非冲突");
        assert!(
            crate::workflow::list_workflow_versions(&conn, "w1")
                .unwrap()
                .is_empty(),
            "无改动不应建快照"
        );
        assert_eq!(
            server_version_of(&conn, "w1").unwrap(),
            2,
            "只推进 sync_state"
        );
    }

    /// 本地内容哈希未变（仅 updated_at / dirty 变）→ 不算本地改动、不进 push 候选。
    #[test]
    fn local_unchanged_even_if_updated_at_changes_not_candidate() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "稳定的工作流")).unwrap();
        mark_synced(&conn, "w1", 3);
        // 模拟「保存但内容没变」：updated_at 被推后、dirty 被标脏
        conn.execute(
            "UPDATE workflow_definitions SET updated_at = updated_at + 100 WHERE id = 'w1'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE sync_state SET dirty = 1 WHERE object_key = 'w1'", [])
            .unwrap();

        assert!(
            !local_differs_from_synced(&conn, "w1").unwrap(),
            "内容未变 → 不算本地改动（判据不依赖 updated_at / dirty）"
        );
        let candidates = list_push_candidates(&conn).unwrap();
        assert!(
            !candidates.iter().any(|(k, _)| k == "w1"),
            "内容未变不应进入 push 候选"
        );
    }

    /// 本地内容变了 → 视为有真实改动、进入 push 候选（baseVersion = 已登记远端版本）。
    #[test]
    fn local_content_change_enters_candidates() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "原名")).unwrap();
        mark_synced(&conn, "w1", 3);
        crate::workflow::update_definition(&conn, &sync_def("w1", "改过名")).unwrap();

        assert!(
            local_differs_from_synced(&conn, "w1").unwrap(),
            "内容变了 → 有真实改动"
        );
        let candidates = list_push_candidates(&conn).unwrap();
        let entry = candidates
            .iter()
            .find(|(k, _)| k == "w1")
            .expect("内容变化的对象应进入 push 候选");
        assert_eq!(entry.1, 3, "baseVersion 应为已登记的远端版本");
    }

    /// pull：本地与远端内容不同（本地未改）→ 覆盖前给本地旧内容建 cloud_sync 快照，再跟随远端。
    #[test]
    fn pull_snapshots_before_overwrite_when_changed() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "本地内容")).unwrap();
        crate::workflow::create_definition(&conn, &sync_def("w2", "远端内容")).unwrap();
        let (_n, payload) = build_workflow_export_json(&conn, "w2").unwrap();
        mark_synced(&conn, "w1", 1);

        let item = RemoteItem {
            key: "w1".to_string(),
            version: 2,
            deleted: false,
            payload: Some(payload),
            updated_at: None,
        };
        let outcome = apply_one_remote_item(&conn, &item).unwrap();
        assert!(outcome.applied, "内容不同 → 跟随远端");
        assert!(outcome.conflict.is_none(), "本地未改 → 非冲突");

        let vs = crate::workflow::list_workflow_versions(&conn, "w1").unwrap();
        assert_eq!(vs.len(), 1, "覆盖前应给本地旧内容建一个快照");
        assert_eq!(vs[0].origin, crate::workflow::VERSION_ORIGIN_CLOUD_SYNC);
        let snap: crate::workflow::WorkflowDefinition =
            serde_json::from_str(&vs[0].snapshot).unwrap();
        assert_eq!(snap.name, "本地内容", "快照应是被覆盖前的本地旧内容");
        assert_eq!(
            crate::workflow::get_active_definition(&conn, "w1")
                .unwrap()
                .unwrap()
                .name,
            "远端内容",
            "本地内容应被远端覆盖"
        );
    }

    /// 冲突：本地真改 + 远端更新 → 本地内容留档为本地版本、主对象跟随远端、计入冲突清单，且**未被自动重提**。
    #[test]
    fn pull_conflict_keeps_local_version_and_follows_remote() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "本地旧内容")).unwrap();
        mark_synced(&conn, "w1", 1);
        // 本地又改了（真实改动）
        crate::workflow::update_definition(&conn, &sync_def("w1", "本地新改动")).unwrap();
        assert!(local_differs_from_synced(&conn, "w1").unwrap());
        // 远端也有新版内容
        crate::workflow::create_definition(&conn, &sync_def("w2", "远端内容")).unwrap();
        let (_n, payload) = build_workflow_export_json(&conn, "w2").unwrap();

        let item = RemoteItem {
            key: "w1".to_string(),
            version: 2,
            deleted: false,
            payload: Some(payload),
            updated_at: None,
        };
        let outcome = apply_one_remote_item(&conn, &item).unwrap();
        assert!(outcome.applied, "应应用远端内容（主对象跟随远端）");
        let conflict = outcome.conflict.expect("应判定为冲突");
        assert_eq!(conflict.id, "w1");
        assert_eq!(conflict.name, "本地新改动", "冲突记录应带上本地名称");

        // 本地改动被留档为一个本地版本（origin = cloud_sync）
        let vs = crate::workflow::list_workflow_versions(&conn, "w1").unwrap();
        assert_eq!(vs.len(), 1, "应给本地改动留一个版本");
        assert_eq!(vs[0].origin, crate::workflow::VERSION_ORIGIN_CLOUD_SYNC);
        let snap: crate::workflow::WorkflowDefinition =
            serde_json::from_str(&vs[0].snapshot).unwrap();
        assert_eq!(snap.name, "本地新改动", "留档的是本地改动");
        // 主对象跟随远端
        assert_eq!(
            crate::workflow::get_active_definition(&conn, "w1")
                .unwrap()
                .unwrap()
                .name,
            "远端内容",
            "主对象应跟随远端"
        );
        // 跟随远端后哈希 = 远端内容 → 不再视为本地改动，**未被自动重提**
        assert!(!local_differs_from_synced(&conn, "w1").unwrap());
        let candidates = list_push_candidates(&conn).unwrap();
        assert!(
            !candidates.iter().any(|(k, _)| k == "w1"),
            "冲突后主对象跟随远端，不应自动重提本地内容"
        );
    }

    /// 本地未改 + 远端更新 → 直接跟随、无冲突。
    #[test]
    fn pull_without_local_change_follows_remote_no_conflict() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "本地内容")).unwrap();
        mark_synced(&conn, "w1", 1);
        crate::workflow::create_definition(&conn, &sync_def("w2", "远端内容")).unwrap();
        let (_n, payload) = build_workflow_export_json(&conn, "w2").unwrap();

        let item = RemoteItem {
            key: "w1".to_string(),
            version: 2,
            deleted: false,
            payload: Some(payload),
            updated_at: None,
        };
        let outcome = apply_one_remote_item(&conn, &item).unwrap();
        assert!(outcome.applied, "应跟随远端");
        assert!(outcome.conflict.is_none(), "本地未改 → 无冲突");
        assert_eq!(
            crate::workflow::get_active_definition(&conn, "w1")
                .unwrap()
                .unwrap()
                .name,
            "远端内容"
        );
    }

    /// 删除候选收窄：仅回收站中的**已跟踪**条目推 deleted；从未同步（server_version=0）的回收站条目不推。
    #[test]
    fn delete_candidate_only_recycle_bin_tracked() {
        let conn = full_sync_conn();
        // 已跟踪并同步过的条目 → 进回收站后应推删除
        crate::workflow::create_definition(&conn, &sync_def("w1", "已同步后删除")).unwrap();
        mark_synced(&conn, "w1", 3);
        crate::workflow::soft_delete_definition(&conn, "w1").unwrap();
        // 从未同步（远端未知）的条目 → 进回收站后**不**推删除
        crate::workflow::create_definition(&conn, &sync_def("w2", "从未同步即删除")).unwrap();
        crate::workflow::soft_delete_definition(&conn, "w2").unwrap();

        let (items, _failures) = build_push_items(&conn).unwrap();
        assert!(
            items.iter().any(|i| i.key == "w1" && i.deleted == Some(true)),
            "已跟踪的回收站条目应推 deleted"
        );
        assert!(
            !items.iter().any(|i| i.key == "w2"),
            "从未同步（远端未知）的条目不应推 deleted"
        );
    }

    /// purge：保留 sync_state 行并置 `tombstone=1` / `tombstone_pushed=0`，保留 `server_version`；
    /// 且作为**补送候选**推 `deleted`（把删除意图送达云端，避免云端永远留着活跃副本）。
    #[test]
    fn purge_keeps_tombstone_and_enters_retry_candidate() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "待彻底删除")).unwrap();
        mark_synced(&conn, "w1", 3);
        crate::workflow::soft_delete_definition(&conn, "w1").unwrap();
        crate::workflow::purge_workflow(&conn, "w1").unwrap();

        // sync_state 行保留、tombstone=1、记录当时的远端版本、删除意图尚未送达
        let (tombstone, version) = tombstone_of(&conn, "w1").unwrap();
        assert!(tombstone, "彻底删除后应保留墓碑");
        assert_eq!(version, 3, "墓碑记录当时已知的远端版本");
        assert!(
            !tombstone_pushed_of(&conn, "w1").unwrap(),
            "刚 purge 时删除意图尚未送达"
        );
        assert!(
            crate::workflow::get_definition(&conn, "w1").unwrap().is_none(),
            "彻底删除应移除本地定义"
        );
        // 补送候选：推 deleted，baseVersion = 墓碑记录的 server_version
        let (items, _failures) = build_push_items(&conn).unwrap();
        let item = items
            .iter()
            .find(|i| i.key == "w1")
            .expect("已 purge 的条目应作为补送候选推 deleted");
        assert_eq!(item.deleted, Some(true));
        assert!(item.payload.is_none());
        assert_eq!(item.base_version, 3);
    }

    /// 墓碑守卫：pull 同版本/更低版本的 deleted → 忽略（不复活）；更高版本且 deleted=false → 清墓碑并接受。
    #[test]
    fn tombstone_blocks_pull_resurrection_until_newer_version() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "本地工作流")).unwrap();
        mark_synced(&conn, "w1", 5);
        crate::workflow::soft_delete_definition(&conn, "w1").unwrap();
        crate::workflow::purge_workflow(&conn, "w1").unwrap();

        // 远端同版本删除事件 → 忽略，不复活
        let same = RemoteItem {
            key: "w1".to_string(),
            version: 5,
            deleted: true,
            payload: None,
            updated_at: None,
        };
        let o = apply_one_remote_item(&conn, &same).unwrap();
        assert!(!o.applied, "同/更低版本的墓碑事件应忽略");
        assert!(
            crate::workflow::get_definition(&conn, "w1").unwrap().is_none(),
            "墓碑存在期间不得被 pull 复活"
        );
        assert!(tombstone_of(&conn, "w1").unwrap().0, "墓碑应仍存在");

        // 远端更高版本的新事件（另一设备显式恢复/更新）→ 清墓碑并接受复活
        crate::workflow::create_definition(&conn, &sync_def("w2", "远端复活内容")).unwrap();
        let (_n, payload) = build_workflow_export_json(&conn, "w2").unwrap();
        let newer = RemoteItem {
            key: "w1".to_string(),
            version: 6,
            deleted: false,
            payload: Some(payload),
            updated_at: None,
        };
        let o2 = apply_one_remote_item(&conn, &newer).unwrap();
        assert!(o2.applied, "更高版本应接受复活");
        assert!(!tombstone_of(&conn, "w1").unwrap().0, "墓碑应被清除");
        assert_eq!(
            crate::workflow::get_active_definition(&conn, "w1")
                .unwrap()
                .unwrap()
                .name,
            "远端复活内容"
        );
    }

    /// 恢复：restore 后该 key 视为「本地有真实改动」→ 进入 push 候选（云端复活的关键）。
    #[test]
    fn restore_marks_local_change_and_enters_candidates() {
        let conn = full_sync_conn();
        crate::workflow::create_definition(&conn, &sync_def("w1", "被删后恢复")).unwrap();
        mark_synced(&conn, "w1", 3);
        crate::workflow::soft_delete_definition(&conn, "w1").unwrap();

        crate::workflow::restore_workflow(&conn, "w1").unwrap();
        assert!(!crate::workflow::is_definition_deleted(&conn, "w1").unwrap());
        assert!(
            local_differs_from_synced(&conn, "w1").unwrap(),
            "恢复视为本地有真实改动（last_synced_hash 已清空）"
        );
        assert_eq!(
            last_synced_hash_of(&conn, "w1"),
            None,
            "恢复应清空 last_synced_hash"
        );
        let candidates = list_push_candidates(&conn).unwrap();
        let entry = candidates
            .iter()
            .find(|(k, _)| k == "w1")
            .expect("恢复后的对象应进入 push 候选");
        assert_eq!(entry.1, 3, "baseVersion 应为已登记的远端版本");
    }

    // ── 墓碑推送标志 tombstone_pushed：删除意图的补送与去重 ──

    /// 造一个「已彻底删除」的墓碑对象：创建 → 同步过 v3 → 软删除 → purge。
    fn purged_tombstone(conn: &Connection, id: &str, server_version: i64) {
        crate::workflow::create_definition(conn, &sync_def(id, "待彻底删除")).unwrap();
        mark_synced(conn, id, server_version);
        crate::workflow::soft_delete_definition(conn, id).unwrap();
        crate::workflow::purge_workflow(conn, id).unwrap();
    }

    /// purge 后进入「补送候选」（推 deleted，baseVersion = 墓碑记录的 server_version）；
    /// 模拟推送**成功一次**（accepted 落版本 + 置标志）后，**不再出现在候选**（不重复推送）。
    #[test]
    fn tombstone_push_candidate_sent_once_then_gone() {
        let conn = full_sync_conn();
        purged_tombstone(&conn, "w1", 3);

        // purge 即置 tombstone=1 / tombstone_pushed=0，保留 server_version 作 baseVersion
        assert!(tombstone_of(&conn, "w1").unwrap().0, "应保留墓碑");
        assert!(!tombstone_pushed_of(&conn, "w1").unwrap(), "刚 purge 未送达");
        let (items, _f) = build_push_items(&conn).unwrap();
        let item = items
            .iter()
            .find(|i| i.key == "w1")
            .expect("未送达的墓碑应进入补送候选");
        assert_eq!(item.deleted, Some(true), "补送动作为删除");
        assert!(item.payload.is_none(), "删除动作不带内容");
        assert_eq!(item.base_version, 3, "baseVersion = 墓碑记录的 server_version");

        // 模拟 push 被接受：accepted 回执落版本（upsert）+ 置 tombstone_pushed = 1
        upsert_sync_state(&conn, "w1", 4, false, None).unwrap();
        mark_tombstone_pushed(&conn, "w1", None).unwrap();
        assert!(tombstone_pushed_of(&conn, "w1").unwrap(), "送达后应置标志");
        assert!(
            !list_push_candidates(&conn)
                .unwrap()
                .iter()
                .any(|(k, _)| k == "w1"),
            "已送达的墓碑不应再进入补送候选（不重复推送）"
        );
        assert!(
            !build_push_items(&conn).unwrap().0.iter().any(|i| i.key == "w1"),
            "已送达的墓碑不应再被推送"
        );
    }

    /// 推送**失败**（未收到 accepted）：保持 tombstone_pushed = 0 → **下一轮仍在候选**（不会永久跳过）。
    #[test]
    fn tombstone_push_failure_remains_candidate() {
        let conn = full_sync_conn();
        purged_tombstone(&conn, "w1", 3);

        // 模拟整批 push 失败：不调用任何 accepted 逻辑 → 状态原样
        assert!(
            list_push_candidates(&conn)
                .unwrap()
                .iter()
                .any(|(k, _)| k == "w1"),
            "推送失败后仍应在候选中，等待下一轮继续推送"
        );
        assert!(
            !tombstone_pushed_of(&conn, "w1").unwrap(),
            "失败不得把标志置为已送达"
        );
        // 下一轮再次构造 → 仍能拿到同一个补送项（baseVersion 未回退）
        let (items, _f) = build_push_items(&conn).unwrap();
        let item = items.iter().find(|i| i.key == "w1").expect("下一轮仍应补送");
        assert_eq!(item.base_version, 3);
        assert_eq!(item.deleted, Some(true));
    }

    /// pull 拉到该 key 的 `deleted = true` → 云端已是删除态（删除意图已达成）：
    /// 置 `tombstone_pushed = 1`、推进 server_version，本机已真删、不复活，且不再进入补送候选。
    #[test]
    fn pull_deleted_marks_tombstone_pushed() {
        let conn = full_sync_conn();
        purged_tombstone(&conn, "w1", 3);

        let item = RemoteItem {
            key: "w1".to_string(),
            version: 5,
            deleted: true,
            payload: None,
            updated_at: None,
        };
        let o = apply_one_remote_item(&conn, &item).unwrap();
        assert!(!o.applied, "本机无本地数据可改，不算落地");
        assert!(tombstone_of(&conn, "w1").unwrap().0, "墓碑应保留");
        assert!(tombstone_pushed_of(&conn, "w1").unwrap(), "删除意图已达成 → 置标志");
        assert_eq!(server_version_of(&conn, "w1").unwrap(), 5, "推进 server_version");
        assert!(
            crate::workflow::get_definition(&conn, "w1").unwrap().is_none(),
            "不得复活"
        );
        assert!(
            !list_push_candidates(&conn)
                .unwrap()
                .iter()
                .any(|(k, _)| k == "w1"),
            "已送达的墓碑不再补送"
        );
    }

    /// pull 拉到 `deleted = false` 且版本**更高** → 清墓碑、接受复活（尊重远端更新的显式事件），
    /// 之后不再补推（内容已与远端一致）。
    #[test]
    fn pull_newer_alive_clears_tombstone_and_resurrects() {
        let conn = full_sync_conn();
        purged_tombstone(&conn, "w1", 3);
        crate::workflow::create_definition(&conn, &sync_def("w2", "远端复活内容")).unwrap();
        let (_n, payload) = build_workflow_export_json(&conn, "w2").unwrap();

        let item = RemoteItem {
            key: "w1".to_string(),
            version: 6,
            deleted: false,
            payload: Some(payload),
            updated_at: None,
        };
        let o = apply_one_remote_item(&conn, &item).unwrap();
        assert!(o.applied, "更高版本 → 接受复活");
        assert!(!tombstone_of(&conn, "w1").unwrap().0, "墓碑应被清除");
        assert!(!tombstone_pushed_of(&conn, "w1").unwrap(), "清墓碑时标志应复位");
        assert_eq!(
            crate::workflow::get_active_definition(&conn, "w1")
                .unwrap()
                .unwrap()
                .name,
            "远端复活内容"
        );
        assert!(
            !list_push_candidates(&conn)
                .unwrap()
                .iter()
                .any(|(k, _)| k == "w1"),
            "复活后内容已与远端一致，不应再补推"
        );
    }

    /// pull 拉到 `deleted = false` 且版本**不高于**墓碑记录 → 保持墓碑、不复活，仍进入补送候选。
    #[test]
    fn pull_stale_alive_keeps_tombstone_and_candidate() {
        let conn = full_sync_conn();
        purged_tombstone(&conn, "w1", 3);
        crate::workflow::create_definition(&conn, &sync_def("w2", "旧远端内容")).unwrap();
        let (_n, payload) = build_workflow_export_json(&conn, "w2").unwrap();

        // 版本等于墓碑记录（不更高）→ 视为过期事件，保持墓碑、等待补推
        let item = RemoteItem {
            key: "w1".to_string(),
            version: 3,
            deleted: false,
            payload: Some(payload),
            updated_at: None,
        };
        let o = apply_one_remote_item(&conn, &item).unwrap();
        assert!(!o.applied, "版本不高于墓碑 → 不复活");
        assert!(tombstone_of(&conn, "w1").unwrap().0, "墓碑应保持");
        assert!(!tombstone_pushed_of(&conn, "w1").unwrap(), "仍未送达");
        assert!(
            crate::workflow::get_definition(&conn, "w1").unwrap().is_none(),
            "不得复活"
        );
        let candidates = list_push_candidates(&conn).unwrap();
        let entry = candidates
            .iter()
            .find(|(k, _)| k == "w1")
            .expect("保持墓碑 → 仍在补送候选");
        assert_eq!(entry.1, 3, "baseVersion 仍为墓碑记录的 server_version");
    }

    /// 墓碑补送候选与「常规删除候选（回收站条目）」互不干扰：两者各自独立进入/退出候选。
    #[test]
    fn tombstone_candidate_isolated_from_recycle_bin_candidate() {
        let conn = full_sync_conn();
        // w1：回收站中的已跟踪条目 → 常规删除候选（走 dirty 判据）
        crate::workflow::create_definition(&conn, &sync_def("w1", "回收站条目")).unwrap();
        mark_synced(&conn, "w1", 3);
        crate::workflow::soft_delete_definition(&conn, "w1").unwrap();
        // w2：彻底删除 → 墓碑补送候选（走 tombstone_pushed 判据）
        purged_tombstone(&conn, "w2", 4);

        let (items, _f) = build_push_items(&conn).unwrap();
        let i1 = items.iter().find(|i| i.key == "w1").expect("回收站条目应推删除");
        assert_eq!(i1.deleted, Some(true));
        assert_eq!(i1.base_version, 3);
        let i2 = items.iter().find(|i| i.key == "w2").expect("墓碑应补送删除");
        assert_eq!(i2.deleted, Some(true));
        assert_eq!(i2.base_version, 4);

        // 墓碑补送标志只作用于 w2：置 1 后 w2 退出候选，而 w1 的常规删除候选不受影响
        mark_tombstone_pushed(&conn, "w2", None).unwrap();
        let (items2, _f) = build_push_items(&conn).unwrap();
        assert!(
            !items2.iter().any(|i| i.key == "w2"),
            "已送达的墓碑应退出补送候选"
        );
        assert!(
            items2.iter().any(|i| i.key == "w1" && i.deleted == Some(true)),
            "常规删除候选不受墓碑标志影响"
        );
        // 反向：w1 的 dirty 与 w2 无关
        assert!(!tombstone_of(&conn, "w1").unwrap().0, "w1 非墓碑");
        assert!(tombstone_of(&conn, "w2").unwrap().0, "w2 仍是墓碑");
    }

    // ── 分页 / 分批（纯函数，无需真实大数据量）──

    /// 造一条远端条目（仅 key/version 有意义）。
    fn ri(key: &str, version: i64) -> RemoteItem {
        RemoteItem {
            key: key.to_string(),
            version,
            deleted: false,
            payload: None,
            updated_at: None,
        }
    }

    /// 翻页合并：多页（hasMore=true→false）响应能取全全部条目，watermark 取**最后一页**；
    /// 无页时 watermark 回退 `since`。对应 `fetch_all_objects` 的循环语义。
    #[test]
    fn pull_pages_merge_all_and_take_last_watermark() {
        let pages = vec![
            (500i64, vec![ri("a", 1), ri("b", 2)], true),
            (800i64, vec![ri("c", 3)], false),
        ];
        let (watermark, items) = fold_pull_pages(0, pages);
        assert_eq!(watermark, 800, "watermark 应取最后一页的值");
        assert_eq!(items.len(), 3, "多页条目应取全");
        assert_eq!(
            items.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "条目顺序应与翻页顺序一致"
        );

        // 空页：不推进 watermark，返回 since
        let (wm, empty) = fold_pull_pages(42, Vec::new());
        assert_eq!(wm, 42);
        assert!(empty.is_empty());
    }

    /// 分批：>100 个候选按 ≤100 分批，不丢项，且每批不超过平台上限。
    #[test]
    fn push_items_chunked_by_platform_limit() {
        assert_eq!(PUSH_MAX_ITEMS, 100, "平台单次推送上限应为 100");
        let items: Vec<PushItem> = (0..250)
            .map(|i| PushItem {
                key: format!("k{}", i),
                payload: Some("p".to_string()),
                deleted: None,
                base_version: 1,
            })
            .collect();
        let chunks = chunk_push_items(&items);
        assert_eq!(chunks.len(), 3, "250 条应分成 3 批");
        assert!(chunks.iter().all(|c| c.len() <= PUSH_MAX_ITEMS), "每批不得超过上限");
        assert_eq!(
            chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
            vec![100, 100, 50]
        );
        assert_eq!(
            chunks.iter().map(|c| c.len()).sum::<usize>(),
            250,
            "分批不得丢项"
        );
    }

    // ── 墓碑 409 的计数口径 ──

    /// 墓碑补送的 409（本机已彻底删除，补推 `deleted` 撞版本）**不计入常规冲突**：
    /// 只有非墓碑冲突进入处理与计数，从而 `conflicts` 恒等于 `conflictItems.len()`。
    #[test]
    fn tombstone_409_excluded_from_conflicts() {
        let conn = full_sync_conn();
        // w-tomb：墓碑行；w-live：普通行
        conn.execute(
            "INSERT INTO sync_state (kind, object_key, tombstone, tombstone_pushed)
             VALUES (?1, 'w-tomb', 1, 0)",
            params![KIND_WORKFLOW],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_state (kind, object_key) VALUES (?1, 'w-live')",
            params![KIND_WORKFLOW],
        )
        .unwrap();

        let conflicts = vec![
            ConflictItem {
                key: "w-tomb".to_string(),
                version: 7,
                updated_at: None,
                deleted: false,
            },
            ConflictItem {
                key: "w-live".to_string(),
                version: 3,
                updated_at: None,
                deleted: false,
            },
        ];
        let (real, tombstone) = split_tombstone_conflicts(&conn, conflicts).unwrap();
        assert_eq!(tombstone, 1, "墓碑补送的 409 单独计数");
        assert_eq!(real.len(), 1, "只有非墓碑冲突进入常规处理");
        assert_eq!(real[0].key, "w-live");
        assert!(
            !real.iter().any(|c| c.key == "w-tomb"),
            "墓碑冲突不得进入 conflicts / conflictItems"
        );
    }

    // ── 自动同步间隔可配置 ──

    /// 间隔读设置并 clamp：缺省 10；越界夹到 1..=1440；非法回退默认。
    #[test]
    fn sync_interval_minutes_default_and_clamp() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL);",
        )
        .unwrap();
        assert_eq!(sync_interval_minutes(&conn), 10, "缺省 10 分钟");
        set_setting(&conn, SYNC_INTERVAL_KEY, "1").unwrap();
        assert_eq!(sync_interval_minutes(&conn), 1);
        set_setting(&conn, SYNC_INTERVAL_KEY, "30").unwrap();
        assert_eq!(sync_interval_minutes(&conn), 30);
        set_setting(&conn, SYNC_INTERVAL_KEY, "1440").unwrap();
        assert_eq!(sync_interval_minutes(&conn), 1440);
        // 越界：下界夹到 1、上界夹到 1440
        set_setting(&conn, SYNC_INTERVAL_KEY, "0").unwrap();
        assert_eq!(sync_interval_minutes(&conn), 1);
        set_setting(&conn, SYNC_INTERVAL_KEY, "99999").unwrap();
        assert_eq!(sync_interval_minutes(&conn), 1440);
        // 非法：回退默认
        set_setting(&conn, SYNC_INTERVAL_KEY, "abc").unwrap();
        assert_eq!(sync_interval_minutes(&conn), 10);
    }
}
