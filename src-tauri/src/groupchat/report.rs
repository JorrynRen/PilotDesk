//! 汇报适配器：任务结果 → 一句话 `task_result` 发言。

use super::models::TaskRow;

/// 将任务执行结果转为可回房间的汇报（L3 可直接引用的成果）。
/// `result_content` 为执行者最终输出的完整文本（v3.4n：task_result 消息落全文）；
/// 无最终文本（工具执行无结论）时回退用 result_summary（任务面板短摘要）。
pub fn to_task_result(task: &TaskRow, result_content: &str) -> String {
    match task.status.as_str() {
        "success" => {
            let body = if !result_content.trim().is_empty() {
                result_content.trim().to_string()
            } else {
                task.result_summary.clone().unwrap_or_default()
            };
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
