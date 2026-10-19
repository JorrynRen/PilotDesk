//! 成员端命令转发（主机侧分发器）。
//!
//! 成员端不缓存团队数据，所有业务读写都经局域网 WS 转发到主机执行。这里按
//! **命令名 + JSON 参数**分发到与命令层相同的领域函数，并统一使用连接握手确认的
//! `actor`（成员 user id）做授权——绝不使用主机本机的 `local_identity`。

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

use super::{files, knowledge, progress, reports, tasks};

fn to_value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| format!("序列化主机响应失败：{}", e))
}

fn req_str(payload: &Value, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少参数：{}", key))
}

fn opt_str(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn str_vec(payload: &Value, key: &str) -> Vec<String> {
    payload
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn ensure_report_in_org(conn: &Connection, org_id: &str, report_id: &str) -> Result<(), String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_reports r
             JOIN org_tasks t ON t.id = r.task_id
             WHERE r.id = ?1 AND t.org_id = ?2",
            params![report_id, org_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("校验汇报归属失败：{}", e))?;
    if n == 0 {
        return Err("汇报不存在或不属于当前组织".to_string());
    }
    Ok(())
}

/// 分发一条成员端命令。`actor` 必须是握手确认过的组织成员。
pub fn dispatch(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    command: &str,
    payload: &Value,
) -> Result<Value, String> {
    match command {
        // ── 治理只读：成员端 Agent 工具按主机策略复核出境闸门 ──
        "policy.get" => {
            let policy = super::org::get_allow_cloud_model(conn, org_id)?;
            Ok(serde_json::json!({
                "allowCloudModel": policy.unwrap_or(false),
                "explicit": policy.is_some(),
            }))
        }

        // ── 任务 ──────────────────────────────────────────────
        "task.get" => {
            let task_id = req_str(payload, "taskId")?;
            to_value(tasks::get_task(conn, org_id, &task_id, actor)?)
        }
        "task.create" => {
            let input: tasks::CreateTaskInput = serde_json::from_value(
                payload.get("input").cloned().unwrap_or(Value::Null),
            )
            .map_err(|e| format!("任务参数不正确：{}", e))?;
            to_value(tasks::create_task(conn, org_id, actor, &input)?)
        }
        "task.update" => {
            let task_id = req_str(payload, "taskId")?;
            let input: tasks::UpdateTaskInput = serde_json::from_value(
                payload.get("input").cloned().unwrap_or(Value::Null),
            )
            .map_err(|e| format!("任务参数不正确：{}", e))?;
            to_value(tasks::update_task(conn, org_id, actor, &task_id, &input)?)
        }
        "task.set_status" => {
            let task_id = req_str(payload, "taskId")?;
            let status = req_str(payload, "status")?;
            to_value(tasks::set_status_checked(
                conn, org_id, actor, &task_id, &status,
            )?)
        }
        "task.container_confirm" => {
            let task_id = req_str(payload, "taskId")?;
            to_value(tasks::confirm_container(conn, org_id, actor, &task_id)?)
        }
        "task.set_deps" => {
            let task_id = req_str(payload, "taskId")?;
            let blocked_by = str_vec(payload, "blockedBy");
            to_value(tasks::set_deps(
                conn,
                org_id,
                actor,
                &task_id,
                &blocked_by,
            )?)
        }
        "task.deps_preview" => {
            let task_id = req_str(payload, "taskId")?;
            let blocked_by = str_vec(payload, "blockedBy");
            to_value(tasks::deps_preview(conn, org_id, &task_id, &blocked_by)?)
        }
        "task.progress_add" => {
            let task_id = req_str(payload, "taskId")?;
            let content = req_str(payload, "content")?;
            // 成员端传的是上传令牌（不是主机路径）：解析到本机临时文件，
            // 禁止客户端直接指定任意路径（否则可借过程记录读取主机任意文件）。
            let tokens = str_vec(payload, "attachments");
            let mut paths = Vec::with_capacity(tokens.len());
            for token in &tokens {
                let path = super::host::resolve_upload_token(org_id, actor, token)?;
                paths.push(path.to_string_lossy().to_string());
            }
            let result =
                progress::add_progress(conn, org_id, actor, &task_id, &content, &paths);
            for path in &paths {
                let _ = std::fs::remove_file(path);
            }
            to_value(result?)
        }
        "task.progress_list" => {
            let task_id = req_str(payload, "taskId")?;
            to_value(progress::list_progress(conn, org_id, &task_id)?)
        }
        "task.set_collaborators" => {
            let task_id = req_str(payload, "taskId")?;
            let collaborators = str_vec(payload, "collaborators");
            to_value(tasks::set_collaborators(
                conn,
                org_id,
                actor,
                &task_id,
                &collaborators,
            )?)
        }

        // ── 完成汇报 ──────────────────────────────────────────
        "report.list" => {
            let task_id = req_str(payload, "taskId")?;
            to_value(reports::list_reports(conn, org_id, &task_id)?)
        }
        "report.save_draft" => {
            let task_id = req_str(payload, "taskId")?;
            let content = req_str(payload, "content")?;
            to_value(reports::save_draft(
                conn,
                org_id,
                actor,
                &task_id,
                &content,
                &str_vec(payload, "materials"),
                &str_vec(payload, "deliverables"),
            )?)
        }
        "report.submit" => {
            let task_id = req_str(payload, "taskId")?;
            let content = req_str(payload, "content")?;
            to_value(reports::submit_report(
                conn,
                org_id,
                actor,
                &task_id,
                &content,
                &str_vec(payload, "materials"),
                &str_vec(payload, "deliverables"),
            )?)
        }
        "report.revise" => {
            let task_id = req_str(payload, "taskId")?;
            let report_id = req_str(payload, "reportId")?;
            to_value(reports::revise_report(
                conn,
                org_id,
                actor,
                &task_id,
                &report_id,
            )?)
        }
        "report.confirm" => {
            let task_id = req_str(payload, "taskId")?;
            let report_id = req_str(payload, "reportId")?;
            to_value(reports::confirm_report(
                conn,
                org_id,
                actor,
                &task_id,
                &report_id,
            )?)
        }
        "report.return" => {
            let task_id = req_str(payload, "taskId")?;
            let report_id = req_str(payload, "reportId")?;
            let reason = req_str(payload, "reason")?;
            to_value(reports::return_report(
                conn,
                org_id,
                actor,
                &task_id,
                &report_id,
                &reason,
            )?)
        }
        "report.feedback_list" => {
            let report_id = req_str(payload, "reportId")?;
            ensure_report_in_org(conn, org_id, &report_id)?;
            to_value(reports::list_feedback(conn, &report_id)?)
        }

        // ── 文件（成员端只下发元数据；打开走 file.stat / file.fetch）──
        "file.task_list" => {
            let task_id = req_str(payload, "taskId")?;
            let mut list = files::list_task_files(conn, org_id, &task_id)?;
            // 主机本地绝对路径不外泄；成员端需要时由 enclave_file_abs_path 下载
            for f in &mut list {
                f.abs_path.clear();
            }
            to_value(list)
        }
        "file.detach" => {
            let task_id = req_str(payload, "taskId")?;
            let file_id = req_str(payload, "fileId")?;
            let report_id = opt_str(payload, "reportId");
            let progress_id = opt_str(payload, "progressId");
            files::detach_file(
                conn,
                org_id,
                actor,
                &task_id,
                &file_id,
                report_id.as_deref(),
                progress_id.as_deref(),
            )?;
            Ok(Value::Null)
        }

        // ── 组织知识库（成员只读）─────────────────────────────
        "kb.list" => to_value(knowledge::list_kbs(conn, org_id)?),
        "kb.search" => {
            let query = req_str(payload, "query")?;
            let kb_id = opt_str(payload, "kbId");
            let limit = payload.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            to_value(knowledge::search_entries(
                conn,
                org_id,
                kb_id.as_deref(),
                &query,
                limit,
            )?)
        }
        "kb.entry_get" => {
            let kb_id = req_str(payload, "kbId")?;
            let memory_id = req_str(payload, "memoryId")?;
            to_value(knowledge::get_entry(conn, org_id, &kb_id, &memory_id)?)
        }
        "kb.file_list" => {
            let kb_id = req_str(payload, "kbId")?;
            to_value(knowledge::list_files(conn, org_id, &kb_id)?)
        }

        other => Err(format!("未知的飞地命令：{}", other)),
    }
}
