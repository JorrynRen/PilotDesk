//! 成员端实时通道：常驻连接主机，把变更事件转成前端 Tauri 事件（`enclave:changed`）。
//!
//! 边界与「不缓存」一致：事件只带**主题**（task / kb / notify / org），不带业务数据；
//! 前端收到后按主题重拉对应数据，团队数据仍然只在主机上。
//!
//! 连接策略：
//! - 只为本机是**成员**（非 owner）的组织建连（owner 就是主机，本机写入直接走 Tauri 事件）；
//! - 每 30s 复核一次成员身份（被移出组织后通道自动断开）；
//! - 断线按 3s → 30s 退避重连；空闲 45s 发一次 `ping` 保活（主机 180s 无请求会断开）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::host::HostSession;
use super::EnclaveState;

/// 成员端实时通道主循环（应用启动时拉起一次）。
///
/// `ui_tx` 是本机 UI 的事件出口：事件由 lib.rs 侧统一转成 Tauri 事件，
/// 本模块刻意不引用 `AppHandle`（测试二进制不链接 GUI 运行时）。
pub async fn run_member_loop(
    state: Arc<EnclaveState>,
    pool: crate::DbPool,
    ui_tx: mpsc::UnboundedSender<Value>,
) {
    let mut tasks: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    loop {
        let orgs = member_orgs(&state);
        tasks.retain(|org, handle| {
            if orgs.iter().any(|o| o == org) {
                !handle.is_finished()
            } else {
                // 已不是该组织成员 / 快照被清空：停止监听
                handle.abort();
                false
            }
        });
        for org in orgs {
            if tasks.contains_key(&org) {
                continue;
            }
            let st = state.clone();
            let pool = pool.clone();
            let tx = ui_tx.clone();
            let org_id = org.clone();
            tasks.insert(
                org,
                tokio::spawn(async move { watch_org(st, pool, tx, org_id).await }),
            );
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

/// 本机以「成员」身份参与的组织（owner 组织由主机本机直接处理，不需要常驻通道）。
fn member_orgs(state: &EnclaveState) -> Vec<String> {
    state
        .store
        .with_conn(|c| {
            let mut stmt = c
                .prepare("SELECT org_id FROM org_snapshot WHERE my_role <> 'owner'")
                .map_err(|e| format!("读取组织快照失败：{}", e))?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| format!("读取组织快照失败：{}", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| format!("读取组织快照失败：{}", e))
        })
        .unwrap_or_default()
}

async fn watch_org(
    state: Arc<EnclaveState>,
    pool: crate::DbPool,
    ui_tx: mpsc::UnboundedSender<Value>,
    org_id: String,
) {
    let mut backoff = 3u64;
    loop {
        if !member_orgs(&state).iter().any(|o| o == &org_id) {
            return;
        }
        let db = crate::DbState { pool: pool.clone() };
        match watch_once(&state, &db, &ui_tx, &org_id).await {
            Ok(()) => backoff = 3,
            Err(e) => log::debug!("[enclave] 实时通道断开（{}）：{}", org_id, e),
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

async fn watch_once(
    state: &EnclaveState,
    db: &crate::DbState,
    ui_tx: &mpsc::UnboundedSender<Value>,
    org_id: &str,
) -> Result<(), String> {
    let mut session = HostSession::connect(state, db, org_id).await?;
    loop {
        let ev = session.wait_event().await?;
        if ev["orgId"].as_str() != Some(org_id) {
            continue;
        }
        let topic = ev["topic"].as_str().unwrap_or("task");
        let _ = ui_tx.send(json!({ "orgId": org_id, "topic": topic }));
    }
}
