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
//! 小于则冲突（服务端不覆盖，客户端生成冲突副本）；`deleted` 为软删除。
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
    apply_workflow_bundle_in_place, build_workflow_export_json, import_workflow_from_json_with_conn,
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
/// 单次 push 条目上限（平台契约）
const PUSH_MAX_ITEMS: usize = 100;
/// 自动同步间隔（秒）：每 10 分钟一次
const AUTO_SYNC_INTERVAL_SECS: u64 = 10 * 60;

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

/// 写 sync_state（存在则覆盖 server_version / dirty / local_updated_at）。
fn upsert_sync_state(
    conn: &Connection,
    key: &str,
    server_version: i64,
    dirty: bool,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO sync_state (kind, object_key, server_version, local_updated_at, dirty)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(kind, object_key)
         DO UPDATE SET server_version = ?3, local_updated_at = ?4, dirty = ?5",
        params![
            KIND_WORKFLOW,
            key,
            server_version,
            crate::utils::now(),
            dirty as i64
        ],
    )?;
    Ok(())
}

/// 列举所有本地有未推送改动的对象 `(object_key, server_version)`。
fn list_dirty_objects(conn: &Connection) -> Result<Vec<(String, i64)>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT object_key, server_version FROM sync_state
         WHERE kind = ?1 AND dirty = 1",
    )?;
    let rows = stmt
        .query_map(params![KIND_WORKFLOW], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 本地该对象是否有未推送的改动：
/// - 本地存在且 sync_state 记录 `dirty = 1` → true；
/// - 本地存在但**从未跟踪**（无记录）→ true（首次同步时会作为新对象上传，属未推送）；
/// - 本地不存在 → false。
fn local_has_unpushed(conn: &Connection, key: &str) -> bool {
    match crate::workflow::get_definition(conn, key) {
        Ok(Some(_)) => {
            let dirty: Option<i64> = conn
                .query_row(
                    "SELECT dirty FROM sync_state WHERE kind = ?1 AND object_key = ?2",
                    params![KIND_WORKFLOW, key],
                    |r| r.get(0),
                )
                .optional()
                .unwrap_or(None);
            dirty.map(|d| d != 0).unwrap_or(true)
        }
        _ => false,
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

/// 一轮同步结果（失败不抛异常，原因放 `error` 文案）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSyncResult {
    pub pulled: usize,
    pub pushed: usize,
    pub conflicts: usize,
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

/// 冲突副本名：`原名（冲突副本 YYYY-MM-DD HH:mm）`。
fn conflict_copy_name(original: &str, at: chrono::DateTime<chrono::Local>) -> String {
    format!("{}（冲突副本 {}）", original, at.format("%Y-%m-%d %H:%M"))
}

// ════════════════════════════════════════════════════════════
// pull / push 主体
// ════════════════════════════════════════════════════════════

/// 拉取自 `since` 之后的全部对象（自动翻页直到 `hasMore = false`）。
/// 返回 `(最终 watermark, 全部条目)`。
async fn fetch_all_objects(token: &str, since: i64) -> Result<(i64, Vec<RemoteItem>), AppError> {
    let mut out = Vec::new();
    let mut cursor = since;
    let mut watermark = since;
    let mut has_more = true;
    while has_more {
        let path = format!(
            "/api/v1/me/sync/objects?since={}&kind={}",
            cursor, KIND_WORKFLOW
        );
        let resp: PullResp = platform::get_json(&path, Some(token)).await?;
        watermark = resp.watermark;
        out.extend(resp.items);
        has_more = resp.has_more;
        cursor = resp.watermark;
    }
    Ok((watermark, out))
}

/// 应用一批远端条目，返回实际落地条数。
fn apply_remote_items(conn: &Connection, items: &[RemoteItem]) -> usize {
    let mut applied = 0usize;
    for item in items {
        match apply_one_remote_item(conn, item) {
            Ok(true) => applied += 1,
            Ok(false) => {}
            Err(e) => log::warn!("[CloudSync] 应用远端对象失败 {}：{}", item.key, e),
        }
    }
    applied
}

/// 应用单条远端条目：deleted → 删本地；否则按 key 就地覆盖导入。
fn apply_one_remote_item(conn: &Connection, item: &RemoteItem) -> Result<bool, AppError> {
    if item.key.is_empty() {
        return Ok(false);
    }
    if !pull_should_apply(item.version, server_version_of(conn, &item.key)?) {
        return Ok(false);
    }
    // 本地有未推送改动、而远端又有更新（含软删除）→ 先把本地版本另存为冲突副本，绝不丢本地改动
    // （与本轮 push 冲突处理同一原则：两侧数据都要保住）
    if local_has_unpushed(conn, &item.key) {
        if let Err(e) = create_conflict_copy(conn, &item.key) {
            log::warn!("[CloudSync] 拉取前另存冲突副本失败 {}：{}", item.key, e);
        }
    }
    if item.deleted {
        if crate::workflow::get_definition(conn, &item.key)
            .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
            .is_some()
        {
            crate::workflow::delete_definition(conn, &item.key)
                .map_err(|e| AppError::Db(format!("删除失败: {}", e)))?;
        }
        upsert_sync_state(conn, &item.key, item.version, false)?;
        return Ok(true);
    }
    match item.payload.as_deref() {
        Some(payload) if !payload.trim().is_empty() => {
            apply_workflow_bundle_in_place(conn, &item.key, payload)?;
            upsert_sync_state(conn, &item.key, item.version, false)?;
            Ok(true)
        }
        // payload 缺失（协议异常）：只推进版本，避免反复拉取
        _ => {
            upsert_sync_state(conn, &item.key, item.version, false)?;
            Ok(true)
        }
    }
}

/// 本轮推送的候选对象 `(key, baseVersion)`：
/// - 已跟踪且 `dirty = 1` 的（本地改动 / 待推送的本地删除）；
/// - **从未跟踪过**的本地工作流（首次同步把已有工作流整体推上去；baseVersion = 0，视为新建）。
fn list_push_candidates(conn: &Connection) -> Result<Vec<(String, i64)>, AppError> {
    let mut candidates: HashMap<String, i64> = HashMap::new();
    for (k, v) in list_dirty_objects(conn)? {
        candidates.insert(k, v);
    }
    let tracked: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT object_key FROM sync_state WHERE kind = ?1")?;
        let keys: Vec<String> = stmt
            .query_map(params![KIND_WORKFLOW], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        keys.into_iter().collect()
    };
    for d in crate::workflow::list_definitions(conn)? {
        if !tracked.contains(&d.id) && !candidates.contains_key(&d.id) {
            candidates.insert(d.id, 0);
        }
    }
    Ok(candidates.into_iter().collect())
}

/// 收集本地待推送对象 → push items（分批前）。
fn build_push_items(conn: &Connection) -> Result<Vec<PushItem>, AppError> {
    let referenced = referenced_workflow_ids(conn)?;
    let mut items = Vec::new();
    for (key, server_version) in list_push_candidates(conn)? {
        let exists = crate::workflow::get_definition(conn, &key)
            .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
            .is_some();
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
                Err(e) => log::warn!("[CloudSync] 打包工作流失败，跳过推送 {}：{}", key, e),
            },
        }
    }
    Ok(items)
}

// ════════════════════════════════════════════════════════════
// 冲突处理
// ════════════════════════════════════════════════════════════

/// 处理 push 冲突：**不丢任何一侧数据**。
///
/// 策略（选择理由见交付报告）：
/// 1. 把**本地**当前版本另存为一份新工作流（名称追加「（冲突副本 时间戳）」），登记为新同步对象
///    （`server_version = 0`、`dirty = 1`）——本地改动由此保住，并会在下一轮作为新对象推上去；
/// 2. 再把**远端**最新版本拉回，覆盖本地原对象（同一 object_key）——远端那一侧也保住。
/// 这样两侧内容都在，只是被拆成两个工作流。若远端是软删除，则本地原对象随拉取被删，本地改动仍留在副本里。
async fn resolve_conflicts(
    state: &crate::DbState,
    token: &str,
    conflicts: &[ConflictItem],
) -> Result<usize, AppError> {
    if conflicts.is_empty() {
        return Ok(0);
    }
    // ① 先把本地版本另存为冲突副本（必须在覆盖原对象之前；连接用完即释放，不跨 await）
    {
        let conn = state.get_conn()?;
        for c in conflicts {
            if c.key.is_empty() {
                continue;
            }
            if let Err(e) = create_conflict_copy(&conn, &c.key) {
                log::warn!("[CloudSync] 生成冲突副本失败 {}：{}", c.key, e);
            }
            // 清掉原对象脏位：本地内容已另存副本；也避免下方「远端覆盖」时再复制一次
            let v = server_version_of(&conn, &c.key).unwrap_or(0);
            let _ = upsert_sync_state(&conn, &c.key, v, false);
        }
    }

    // ② 拉取远端最新，覆盖本地原对象（含软删除）
    let cursor = {
        let conn = state.get_conn()?;
        read_cursor(&conn)
    };
    let (watermark, items) = fetch_all_objects(token, cursor).await?;
    let applied = {
        let conn = state.get_conn()?;
        let applied = apply_remote_items(&conn, &items);
        write_cursor(&conn, watermark)?;
        // ③ 保险：冲突对象一律置 dirty = 0、版本对齐远端，避免反复冲突（本地内容已存副本）
        for c in conflicts {
            if c.key.is_empty() {
                continue;
            }
            let v = server_version_of(&conn, &c.key)?.max(c.version);
            upsert_sync_state(&conn, &c.key, v, false)?;
        }
        applied
    };
    Ok(applied)
}

/// 把本地某工作流另存为「冲突副本」：导入为新工作流、改名、登记为新同步对象（下轮推送）。
/// 返回新对象 id（无可复制时返回 `None`）。
fn create_conflict_copy(conn: &Connection, key: &str) -> Result<Option<String>, AppError> {
    let Some(def) = crate::workflow::get_definition(conn, key)
        .map_err(|e| AppError::Db(format!("查询失败: {}", e)))?
    else {
        return Ok(None);
    };
    let (_name, payload) = match build_workflow_export_json(conn, key) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[CloudSync] 冲突副本打包失败 {}：{}", key, e);
            return Ok(None);
        }
    };
    let (mut copy, _subflows) = import_workflow_from_json_with_conn(conn, &payload)?;
    copy.name = conflict_copy_name(&def.name, chrono::Local::now());
    crate::workflow::update_definition(conn, &copy)
        .map_err(|e| AppError::Db(format!("保存冲突副本失败: {}", e)))?;
    // 副本作为**新对象**：baseVersion 用 0（下一轮新建推送），dirty = 1
    upsert_sync_state(conn, &copy.id, 0, true)?;
    Ok(Some(copy.id))
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
                result.pulled = apply_remote_items(&conn, &items);
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
    let push_items = match state.get_conn() {
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

    for chunk in push_items.chunks(PUSH_MAX_ITEMS) {
        let body = PushBody {
            kind: KIND_WORKFLOW,
            items: chunk.to_vec(),
        };
        let resp: PushResp = match platform::post_json("/api/v1/me/sync/push", &body, Some(&token))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let r = CloudSyncResult::failure(format!("推送失败：{}", e));
                save_round_meta(state, &r);
                return r;
            }
        };
        // accepted 落版本（连接作用域内完成，不跨 await）
        if let Ok(conn) = state.get_conn() {
            for acc in &resp.accepted {
                if let Err(e) = upsert_sync_state(&conn, &acc.key, acc.version, false) {
                    log::warn!("[CloudSync] 更新已推送对象版本失败 {}：{}", acc.key, e);
                }
                result.pushed += 1;
            }
        }
        // 冲突处理（内部自行取连接，不在此持有）
        if !resp.conflicts.is_empty() {
            result.conflicts += resp.conflicts.len();
            match resolve_conflicts(state, &token, &resp.conflicts).await {
                Ok(applied) => result.pulled += applied,
                Err(e) => log::warn!("[CloudSync] 冲突处理失败：{}", e),
            }
        }
    }

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
        None => format!(
            "拉取 {} / 推送 {} / 冲突 {}",
            result.pulled, result.pushed, result.conflicts
        ),
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
    Ok(CloudSyncStatus {
        enabled: sync_enabled(&conn),
        capability_ok: has_capability(&conn, CAP_CLOUD_SYNC).unwrap_or(false),
        last_at: get_setting(&conn, SYNC_LAST_AT_KEY).ok().flatten(),
        last_result: get_setting(&conn, SYNC_LAST_RESULT_KEY).ok().flatten(),
        object_count,
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

/// 后台自动同步循环：应用启动后挂上，每 [`AUTO_SYNC_INTERVAL_SECS`] 执行一轮。
///
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
                    "[CloudSync] 自动同步完成：拉取 {} / 推送 {} / 冲突 {}",
                    r.pulled,
                    r.pushed,
                    r.conflicts
                ),
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(AUTO_SYNC_INTERVAL_SECS)).await;
    }
}

// ════════════════════════════════════════════════════════════
// 单测（纯函数 / 决策分支）
// ════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// 冲突副本命名：原名 + 「（冲突副本 YYYY-MM-DD HH:mm）」。
    #[test]
    fn conflict_copy_name_format() {
        use chrono::TimeZone;
        let at = chrono::Local
            .with_ymd_and_hms(2026, 10, 5, 9, 7, 0)
            .single()
            .unwrap();
        assert_eq!(
            conflict_copy_name("写小说", at),
            "写小说（冲突副本 2026-10-05 09:07）"
        );
    }

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

    /// sync_state：初次 upsert 记为新建（version 0, dirty 1），随后可推进版本并清脏。
    #[test]
    fn sync_state_upsert_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sync_state (
                kind TEXT NOT NULL, object_key TEXT NOT NULL,
                server_version INTEGER NOT NULL DEFAULT 0,
                local_updated_at INTEGER NOT NULL DEFAULT 0,
                dirty INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (kind, object_key));",
        )
        .unwrap();
        assert_eq!(server_version_of(&conn, "w1").unwrap(), 0);
        upsert_sync_state(&conn, "w1", 3, true).unwrap();
        assert_eq!(server_version_of(&conn, "w1").unwrap(), 3);
        // 再写一次：版本与脏位都覆盖
        upsert_sync_state(&conn, "w1", 4, false).unwrap();
        assert_eq!(server_version_of(&conn, "w1").unwrap(), 4);
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
}
