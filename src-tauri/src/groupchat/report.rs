//! 汇报适配器：任务结果 → 一句话 `task_result` 发言。

use super::models::TaskRow;

/// 将任务执行结果转为可回房间的一句话汇报（L3 可直接引用的成果）。
pub fn to_task_result(task: &TaskRow) -> String {
    match task.status.as_str() {
        "success" => {
            let body = task.result_summary.clone().unwrap_or_default();
            if body.is_empty() {
                format!("[任务完成] {}", task.description)
            } else {
                format!("[任务完成] {}：{}", task.description, body)
            }
        }
        "failed" => {
            let err = task.error.clone().unwrap_or_default();
            if err.is_empty() {
                format!("[任务失败] {}", task.description)
            } else {
                format!("[任务失败] {}：{}", task.description, err)
            }
        }
        "skipped" => format!("[任务跳过] {}", task.description),
        _ => format!("[任务] {}", task.description),
    }
}
