//! 会话级 API Agent 运行的进程内注册表。
//!
//! 一次会话运行由 `run_api_agent` 在后台 tokio task 中执行，生命周期与前端组件无关。
//! 本表同时承担两件事：
//!
//! 1. **「运行中」的真相来源**：进程活着才有记录，运行结束或应用重启即自动为空，
//!    不会像落库标记那样在崩溃后留下假的 running（同 `groupchat_room_actor_alive`
//!    的存活探测思路：用进程内事实回答"到底还在不在跑"）。
//! 2. **取消通道**：`agent_stop_generation` 对 API 会话置取消标志；Agent Loop 在
//!    迭代边界与流式读取期间检查该标志，使"停止生成"对 API 会话同样有效。
//!
//! 同一会话在任一时刻至多登记一次运行；重复运行以最后一次登记为准。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static RUNS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();

/// 运行别名（别名 id → 真实会话 id）：工作流 Agent 节点以 `wf_{execution_id}_{node_id}`
/// 作为取消入口（CLI 路径的进程表键），而 API 路径的运行登记在真实会话 id 上。
/// 登记别名后 `cancel(别名)` 作用于目标会话的运行，`cancel_workflow` 对两种 Agent 节点
/// 用同一个键取消即可。运行被中途丢弃时别名可能残留，但 `cancel` 只在目标会话确有
/// 运行记录时生效，残留别名无副作用。
static ALIASES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn runs() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn aliases() -> &'static Mutex<HashMap<String, String>> {
    ALIASES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 登记运行别名：此后 `cancel(alias_id)` 作用于 `session_id` 的运行。
pub fn set_alias(alias_id: &str, session_id: &str) {
    aliases()
        .lock()
        .unwrap()
        .insert(alias_id.to_string(), session_id.to_string());
}

/// 注销运行别名（调用方结束时的清理；未登记时为无操作）。
pub fn clear_alias(alias_id: &str) {
    aliases().lock().unwrap().remove(alias_id);
}

/// 登记一次运行并返回其取消标志；运行结束时必须调用 `finish` 注销。
pub fn begin(session_id: &str) -> Arc<AtomicBool> {
    let token = Arc::new(AtomicBool::new(false));
    runs()
        .lock()
        .unwrap()
        .insert(session_id.to_string(), Arc::clone(&token));
    token
}

/// 注销运行记录。仅当登记的仍是本次运行的标志时才移除，
/// 避免"前一次运行的收尾"误删"后一次运行"的记录。
pub fn finish(session_id: &str, token: &Arc<AtomicBool>) {
    let mut map = runs().lock().unwrap();
    if map
        .get(session_id)
        .is_some_and(|cur| Arc::ptr_eq(cur, token))
    {
        map.remove(session_id);
    }
}

/// 请求取消该会话（或指向该会话的别名）的运行；没有运行记录时返回 false（无从取消）。
pub fn cancel(session_id: &str) -> bool {
    let target = aliases().lock().unwrap().get(session_id).cloned();
    let key = target.as_deref().unwrap_or(session_id);
    let map = runs().lock().unwrap();
    match map.get(key) {
        Some(token) => {
            token.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

/// 该会话当前是否有运行中的后台任务。生产路径用 `running_sessions` 一次性对齐前端状态、
/// 用 `cancel` 停止运行，都不需要单点查询，故仅编译于测试目标。
#[cfg(test)]
pub fn is_running(session_id: &str) -> bool {
    runs().lock().unwrap().contains_key(session_id)
}

/// 全部运行中的会话 id：前端启动/重入时一次性对齐列表状态。
pub fn running_sessions() -> Vec<String> {
    runs().lock().unwrap().keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_marks_running_and_finish_clears_it() {
        let sid = "test-begin-finish";
        assert!(!is_running(sid));
        let token = begin(sid);
        assert!(is_running(sid));
        assert!(running_sessions().contains(&sid.to_string()));
        finish(sid, &token);
        assert!(!is_running(sid));
    }

    #[test]
    fn cancel_sets_flag_only_for_running_session() {
        let sid = "test-cancel-flag";
        // 无运行记录时无从取消
        assert!(!cancel(sid));

        let token = begin(sid);
        assert!(!token.load(Ordering::SeqCst), "登记后不应处于已取消状态");
        assert!(cancel(sid));
        assert!(token.load(Ordering::SeqCst), "取消应置位该次运行的标志");
        finish(sid, &token);
    }

    #[test]
    fn finish_from_previous_run_keeps_newer_run_registered() {
        let sid = "test-finish-stale";
        let first = begin(sid);
        let second = begin(sid);
        // 前一次运行收尾时不得注销后一次运行的记录
        finish(sid, &first);
        assert!(is_running(sid), "后一次运行的登记应保留");
        assert!(cancel(sid), "取消应作用于仍登记的那一次运行");
        assert!(second.load(Ordering::SeqCst));
        finish(sid, &second);
        assert!(!is_running(sid));
    }

    #[test]
    fn cancel_through_alias_stops_target_run() {
        let alias = "test-alias-exec-node";
        let sid = "test-alias-target";
        // 无别名、无运行记录时无从取消
        assert!(!cancel(alias));

        let token = begin(sid);
        set_alias(alias, sid);
        assert!(cancel(alias), "取消别名应作用于目标会话的运行");
        assert!(token.load(Ordering::SeqCst));

        finish(sid, &token);
        assert!(!cancel(alias), "运行结束后别名不再有可取消的目标");
        clear_alias(alias);
    }
}
