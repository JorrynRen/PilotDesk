//! 任务抽象与调度：Kahn 拓扑排序与依赖环检测。

use std::collections::{HashSet, VecDeque};

use super::models::TaskRow;

/// 任务终态：不再参与就绪/讨论判定（与 room.rs 调度判据同源）。
fn is_terminal_status(status: &str) -> bool {
    matches!(status, "success" | "skipped" | "failed" | "aborted")
}

/// 把「前置任务 id」列表解析为任务数组下标 —— `depends_on` 的存储语义是**绝对下标**
/// （见 room.rs 编排写入：kickoff 偏移成全局下标、replan 由 id 映射）。
///
/// `target_id` 为被编辑任务自身 id（自依赖直接拒绝）；找不到的 id 报错而不是静默忽略，
/// 否则界面会显示"已保存"但实际依赖丢了。
pub fn resolve_dep_indices(
    tasks: &[TaskRow],
    target_id: &str,
    dep_ids: &[String],
) -> Result<Vec<usize>, String> {
    let mut indices: Vec<usize> = Vec::new();
    for raw in dep_ids {
        let id = raw.trim();
        if id.is_empty() {
            continue;
        }
        if id == target_id {
            return Err("任务不能依赖自身".to_string());
        }
        match tasks.iter().position(|t| t.id == id) {
            Some(idx) => {
                if !indices.contains(&idx) {
                    indices.push(idx);
                }
            }
            None => return Err("前置任务不存在（可能已被移除）".to_string()),
        }
    }
    indices.sort_unstable();
    Ok(indices)
}

/// 转换用：把依赖展开成"剔除作废任务后"的有效前置下标。
///
/// 背景：`aborted` 在群聊里表示该任务已被**作废/移除**（目标变更归档、Director 重排删减），
/// 它不该被转进工作流重新执行；但后继任务的 `depends_on` 仍指向它，直接丢弃会让索引指错人。
/// 这里做一次收缩：作废前置 → 用它自己的有效前置替代（传递展开），于是返回的下标里
/// 永远不会出现作废任务，调用方按 `status != "aborted"` 过滤节点即可保持依赖语义不变。
pub fn effective_deps(tasks: &[TaskRow]) -> Vec<Vec<usize>> {
    let raw: Vec<Vec<usize>> = tasks
        .iter()
        .map(|t| serde_json::from_str::<Vec<usize>>(&t.depends_on).unwrap_or_default())
        .collect();
    let aborted: Vec<bool> = tasks.iter().map(|t| t.status == "aborted").collect();
    let mut memo: Vec<Option<Vec<usize>>> = vec![None; tasks.len()];
    (0..tasks.len())
        .map(|i| resolve_effective(i, &raw, &aborted, &mut memo))
        .collect()
}

/// `effective_deps` 的递归实现（记忆化 + 环保护：先占位空集，成环时退化为"无前置"）。
fn resolve_effective(
    idx: usize,
    raw: &[Vec<usize>],
    aborted: &[bool],
    memo: &mut Vec<Option<Vec<usize>>>,
) -> Vec<usize> {
    if let Some(cached) = memo.get(idx).and_then(|m| m.clone()) {
        return cached;
    }
    if let Some(slot) = memo.get_mut(idx) {
        *slot = Some(Vec::new());
    }
    let mut out: Vec<usize> = Vec::new();
    for &d in raw.get(idx).map(Vec::as_slice).unwrap_or_default() {
        if d >= raw.len() || d == idx {
            continue;
        }
        let resolved = if aborted.get(d).copied().unwrap_or(false) {
            resolve_effective(d, raw, aborted, memo)
        } else {
            vec![d]
        };
        for r in resolved {
            if r != idx && !out.contains(&r) {
                out.push(r);
            }
        }
    }
    out.sort_unstable();
    if let Some(slot) = memo.get_mut(idx) {
        *slot = Some(out.clone());
    }
    out
}

/// 依赖变更预览（纯计算，不改任何状态）：确认卡据此展示"改完会发生什么"。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepChangePreview {
    /// 变更后陷入依赖环的任务编号（分层调度下会被判死锁失败，必须先拦住）
    pub cycle_task_nos: Vec<i64>,
    /// 变更后由等待变为就绪（解除阻塞）的任务编号
    pub unblocked_task_nos: Vec<i64>,
    /// 变更后由就绪变为等待（新增阻塞）的任务编号
    pub blocked_task_nos: Vec<i64>,
}

/// 计算把 `target_idx` 的前置改为 `new_deps` 之后的影响。
/// 就绪判定复用调度同一实现（`ready_indices`），环检测复用 `cycle_indices`，
/// 保证预览结论与运行期行为一致。
pub fn preview_dep_change(
    tasks: &[TaskRow],
    target_idx: usize,
    new_deps: &[usize],
) -> DepChangePreview {
    let deps_before: Vec<Vec<usize>> = tasks
        .iter()
        .map(|t| serde_json::from_str::<Vec<usize>>(&t.depends_on).unwrap_or_default())
        .collect();
    let mut deps_after = deps_before.clone();
    if let Some(slot) = deps_after.get_mut(target_idx) {
        *slot = new_deps.to_vec();
    }
    let done: Vec<bool> = tasks
        .iter()
        .map(|t| is_terminal_status(&t.status))
        .collect();

    let before: HashSet<usize> = ready_indices(&deps_before, &done).into_iter().collect();
    let after: Vec<usize> = ready_indices(&deps_after, &done);
    let after_set: HashSet<usize> = after.iter().copied().collect();
    let cycle: HashSet<usize> = cycle_indices(&deps_after, &done).into_iter().collect();

    // 环成员单列（它们既非"新阻塞"也非"新解锁"，而是会被判死锁失败）
    let unblocked: Vec<usize> = after
        .iter()
        .copied()
        .filter(|i| !before.contains(i) && !cycle.contains(i))
        .collect();
    let mut blocked: Vec<usize> = before
        .iter()
        .copied()
        .filter(|i| !after_set.contains(i) && !cycle.contains(i))
        .collect();
    blocked.sort_unstable();
    let mut cycle_members: Vec<usize> = cycle.iter().copied().collect();
    cycle_members.sort_unstable();

    let task_nos = |ids: Vec<usize>| -> Vec<i64> {
        ids.into_iter()
            .filter_map(|i| tasks.get(i).map(|t| t.task_no))
            .collect()
    };

    DepChangePreview {
        cycle_task_nos: task_nos(cycle_members),
        unblocked_task_nos: task_nos(unblocked),
        blocked_task_nos: task_nos(blocked),
    }
}

/// 对 `deps[i]`（i 的前置任务下标集合）做 Kahn 拓扑排序，返回执行顺序。
/// 存在环时返回 `None`。
#[allow(dead_code)]
pub fn topological_order(deps: &[Vec<usize>]) -> Option<Vec<usize>> {
    let n = deps.len();
    let mut indegree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];

    for (i, parents) in deps.iter().enumerate() {
        for &p in parents {
            if p < n && p != i {
                indegree[i] += 1;
                dependents[p].push(i);
            }
        }
    }

    let mut ready: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);

    while let Some(i) = ready.pop() {
        order.push(i);
        for &next in &dependents[i] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push(next);
            }
        }
    }

    if order.len() == n {
        Some(order)
    } else {
        None
    }
}

/// 在未终态任务组成的子图中检测依赖环，返回环内任务下标（升序）；无环返回空集。
/// 已终态任务（`done[i]`）视作已放行、不参与环判定，因此跨"终态-未终态"的边不会误判为环。
/// 分层调度下，环成员互相等待、永远不会就绪，调用方应显式失败而不是静默收敛。
pub fn cycle_indices(deps: &[Vec<usize>], done: &[bool]) -> Vec<usize> {
    let n = deps.len().min(done.len());
    let active: HashSet<usize> = (0..n).filter(|&i| !done[i]).collect();

    // 只统计未终态子图内的入边；终态父任务视作已放行，不构成等待。
    let mut indegree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &i in active.iter() {
        for &p in deps[i].iter() {
            if p < n && p != i && active.contains(&p) {
                indegree[i] += 1;
                dependents[p].push(i);
            }
        }
    }

    // Kahn 剥除：能剥掉的全剥掉，剩下的（active 且入度从未归零）即环成员。
    let mut removed = vec![false; n];
    let mut queue: VecDeque<usize> = active
        .iter()
        .copied()
        .filter(|&i| indegree[i] == 0)
        .collect();
    while let Some(i) = queue.pop_front() {
        removed[i] = true;
        for &next in dependents[i].iter() {
            indegree[next] -= 1;
            if indegree[next] == 0 && !removed[next] {
                queue.push_back(next);
            }
        }
    }

    let mut cycles: Vec<usize> = (0..n)
        .filter(|&i| active.contains(&i) && !removed[i])
        .collect();
    cycles.sort_unstable();
    cycles
}

/// 就绪任务下标（升序）：自身未终态（`!done[i]`）且所有前置均已终态（视为已放行）。
/// 越界（`p >= n`）或自依赖（`p == i`）的前置不入等待集，语义与调用方 `dep_blocked` 一致。
/// 阻塞任务（前置终态但非 success）同样落在就绪列——由调用方按 dep_blocked 判 skipped 排空，
/// 避免"父任务失败 → 子任务永久滞留"的悬挂。
/// 分层调度每轮从 DB 重建 `done` 后调用；就绪集不会再增长（无连锁释放）。
pub fn ready_indices(deps: &[Vec<usize>], done: &[bool]) -> Vec<usize> {
    let n = deps.len().min(done.len());
    (0..n)
        .filter(|&i| !done[i] && deps[i].iter().all(|&p| p >= n || p == i || done[p]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topo_order_linear() {
        // 0 -> 1 -> 2
        let deps = vec![vec![], vec![0], vec![1]];
        let order = topological_order(&deps).unwrap();
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn topo_order_parallel() {
        // 0 与 1 独立，2 依赖两者
        let deps = vec![vec![], vec![], vec![0, 1]];
        let order = topological_order(&deps).unwrap();
        assert_eq!(order.len(), 3);
        assert!(order[2] == 2);
    }

    #[test]
    fn topo_cycle_detected() {
        let deps = vec![vec![1], vec![0]];
        assert!(topological_order(&deps).is_none());
    }

    #[test]
    fn cycle_two_nodes_all_active() {
        // 0 <-> 1，均未终态 → 两者皆环成员。
        let deps = vec![vec![1], vec![0]];
        let done = vec![false, false];
        assert_eq!(cycle_indices(&deps, &done), vec![0, 1]);
    }

    #[test]
    fn cycle_members_exclude_done_parent() {
        // 0 -> 1（1 已终态）-> 2，且 2 -> 0：因 1 已终态被放行，未终态子图 0/2 不成环。
        let deps = vec![vec![2], vec![0], vec![1]];
        let done = vec![false, true, false];
        assert!(cycle_indices(&deps, &done).is_empty());
    }

    #[test]
    fn cycle_with_side_chain_keeps_only_cycle() {
        // 1 <-> 2 成环；0 未终态、无依赖，可正常剥除 → 仅 1/2 入环。
        let deps = vec![vec![], vec![2], vec![1]];
        let done = vec![false, false, false];
        assert_eq!(cycle_indices(&deps, &done), vec![1, 2]);
    }

    #[test]
    fn acyclic_all_active_empty() {
        let deps = vec![vec![], vec![0], vec![1]];
        let done = vec![false, false, false];
        assert!(cycle_indices(&deps, &done).is_empty());
    }

    #[test]
    fn ready_includes_free_and_released_only() {
        // 0 已终态；1 依赖 0 → 放行；2 无依赖未终态 → 就绪。3 依赖 1（未终态）→ 等待。
        let deps = vec![vec![], vec![0], vec![], vec![1]];
        let done = vec![true, false, false, false];
        assert_eq!(ready_indices(&deps, &done), vec![1, 2]);
    }

    #[test]
    fn ready_includes_blocked_by_failed_parent() {
        // 0 失败（终态）；1 依赖 0 → 就绪（调用方 dep_blocked 判 skipped，防止悬挂）。
        let deps = vec![vec![], vec![0]];
        let done = vec![true, false];
        assert_eq!(ready_indices(&deps, &done), vec![1]);
    }

    #[test]
    fn ready_waits_pending_parent() {
        let deps = vec![vec![], vec![0]];
        let done = vec![false, false];
        assert_eq!(ready_indices(&deps, &done), vec![0]);
    }

    // ── 人工干预：依赖 id 解析与变更预览 ──

    fn task(id: &str, no: i64, deps: &str, status: &str) -> TaskRow {
        TaskRow {
            id: id.to_string(),
            room_id: "r".to_string(),
            task_no: no,
            description: format!("任务{}", no),
            assignee: None,
            depends_on: deps.to_string(),
            status: status.to_string(),
            result_summary: None,
            error: None,
            started_at: None,
            completed_at: None,
        }
    }

    #[test]
    fn resolve_dep_indices_maps_ids_and_rejects_bad_input() {
        let tasks = vec![task("a", 1, "[]", "success"), task("b", 2, "[]", "pending")];
        assert_eq!(
            resolve_dep_indices(&tasks, "b", &["a".to_string()]).unwrap(),
            vec![0]
        );
        // 自依赖
        assert!(resolve_dep_indices(&tasks, "b", &["b".to_string()]).is_err());
        // 不存在的 id 必须报错，不能静默丢弃
        assert!(resolve_dep_indices(&tasks, "b", &["ghost".to_string()]).is_err());
    }

    #[test]
    fn preview_reports_unblocked_when_deps_cleared() {
        // T1 已成功；T2 就绪；T3 依赖 T2（未终态）→ 等待。
        let tasks = vec![
            task("t1", 1, "[]", "success"),
            task("t2", 2, "[]", "pending"),
            task("t3", 3, "[1]", "pending"),
        ];
        // 清空 T3 的前置 → T3 由等待变为就绪。
        let preview = preview_dep_change(&tasks, 2, &[]);
        assert_eq!(preview.unblocked_task_nos, vec![3]);
        assert!(preview.blocked_task_nos.is_empty());
        assert!(preview.cycle_task_nos.is_empty());
    }

    #[test]
    fn preview_reports_newly_blocked() {
        // T1 成功；T2 与 T3 均无依赖、就绪。
        let tasks = vec![
            task("t1", 1, "[]", "success"),
            task("t2", 2, "[]", "pending"),
            task("t3", 3, "[]", "pending"),
        ];
        // 让 T2 依赖 T3（T3 未终态）→ T2 由就绪变为等待。
        let preview = preview_dep_change(&tasks, 1, &[2]);
        assert_eq!(preview.blocked_task_nos, vec![2]);
        assert!(preview.unblocked_task_nos.is_empty());
        assert!(preview.cycle_task_nos.is_empty());
    }

    #[test]
    fn preview_reports_cycle_members_separately() {
        // T2 依赖 T1；把 T1 改为依赖 T2 → 成环。
        let tasks = vec![
            task("t1", 1, "[]", "pending"),
            task("t2", 2, "[0]", "pending"),
        ];
        let preview = preview_dep_change(&tasks, 0, &[1]);
        assert_eq!(preview.cycle_task_nos, vec![1, 2]);
        // 环成员不计入"新阻塞"，避免同一后果被说两遍。
        assert!(preview.blocked_task_nos.is_empty());
    }

    #[test]
    fn effective_deps_keeps_intact_without_aborted() {
        let tasks = vec![
            task("t1", 1, "[]", "success"),
            task("t2", 2, "[0]", "success"),
        ];
        assert_eq!(effective_deps(&tasks), vec![vec![], vec![0]]);
    }

    #[test]
    fn effective_deps_contracts_aborted_parent() {
        // T1 作废（无前置）、T2 依赖 T1 → T2 收缩为无前置（不能指向已剔除的节点）。
        let tasks = vec![
            task("t1", 1, "[]", "aborted"),
            task("t2", 2, "[0]", "success"),
        ];
        assert_eq!(
            effective_deps(&tasks),
            vec![Vec::<usize>::new(), Vec::new()]
        );
    }

    #[test]
    fn effective_deps_contracts_through_aborted_chain() {
        // T1 正常、T2 作废（依赖 T1）、T3 依赖 T2 → T3 应继承 T1。
        let tasks = vec![
            task("t1", 1, "[]", "success"),
            task("t2", 2, "[0]", "aborted"),
            task("t3", 3, "[1]", "pending"),
        ];
        let deps = effective_deps(&tasks);
        assert_eq!(deps[2], vec![0]);
        // 作废节点自身不会出现在任何有效前置里
        assert!(deps.iter().all(|d| !d.contains(&1)));
    }
}
