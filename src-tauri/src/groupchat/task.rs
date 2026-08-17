//! 任务抽象与调度：Kahn 拓扑排序。

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
}
