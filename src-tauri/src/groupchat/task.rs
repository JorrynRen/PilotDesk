//! 任务抽象与调度：Kahn 拓扑排序与依赖环检测。

use std::collections::{HashSet, VecDeque};

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
    let mut queue: VecDeque<usize> = active.iter().copied().filter(|&i| indegree[i] == 0).collect();
    while let Some(i) = queue.pop_front() {
        removed[i] = true;
        for &next in dependents[i].iter() {
            indegree[next] -= 1;
            if indegree[next] == 0 && !removed[next] {
                queue.push_back(next);
            }
        }
    }

    let mut cycles: Vec<usize> = (0..n).filter(|&i| active.contains(&i) && !removed[i]).collect();
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
        .filter(|&i| {
            !done[i]
                && deps[i]
                    .iter()
                    .all(|&p| p >= n || p == i || done[p])
        })
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
}
