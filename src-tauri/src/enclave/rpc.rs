//! 成员端命令转发（主机侧分发器）。
//!
//! 成员端不缓存团队数据，所有业务读写都经局域网 WS 转发到主机执行。这里按
//! **命令名 + JSON 参数**分发到与命令层相同的领域函数，并统一使用连接握手确认的
//! `actor`（成员 user id）做授权——绝不使用主机本机的 `local_identity`。

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

use super::{files, knowledge, progress, reports, stats, tasks};

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

/// 专属属性参数：JSON 对象字符串（缺省 = 空对象）。
fn fields_value(payload: &Value) -> Result<Value, String> {
    let raw = opt_str(payload, "fields").unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&raw).map_err(|_| "专属属性格式不正确".to_string())
}

/// 管理角色校验（与命令层 `Action::KbEdit` 同口径：owner / admin）。
fn require_kb_edit(conn: &Connection, org_id: &str, actor: &str) -> Result<(), String> {
    let role = crate::enclave::authz::role_for_member(conn, org_id, actor)?;
    if matches!(
        role,
        Some(crate::enclave::authz::Role::Owner) | Some(crate::enclave::authz::Role::Admin)
    ) {
        Ok(())
    } else {
        Err("仅组织所有者/管理员可管理知识库文件".to_string())
    }
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
        // ── 治理只读：成员端 Agent 工具 / 面板按主机策略复核出境闸门 ──
        "policy.get" => to_value(super::org::policy_view(conn, org_id)?),
        "emergency.get" => to_value(super::org::emergency_view(conn, org_id)?),

        // ── 报告 / 绩效（只读聚合）──
        "stats.team" => {
            let from = payload
                .get("from")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "缺少参数：from".to_string())?;
            let to = payload
                .get("to")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "缺少参数：to".to_string())?;
            to_value(stats::team_stats(conn, org_id, from, to)?)
        }

        // ── 通知（沟通层）：成员端读主机权威通知 ──
        "notification.list" => {
            let limit = payload.get("limit").and_then(|v| v.as_i64()).unwrap_or(30);
            let unread_only = payload
                .get("unreadOnly")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            to_value(super::notify::list(
                conn,
                org_id,
                actor,
                limit,
                unread_only,
            )?)
        }
        "notification.mark_read" => {
            let ids = str_vec(payload, "ids");
            to_value(super::notify::mark_read(conn, org_id, actor, &ids)?)
        }
        // ids 为空 = 清空已读
        "notification.delete" => {
            let ids = str_vec(payload, "ids");
            to_value(super::notify::delete(conn, org_id, actor, &ids)?)
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
        "task.delete" => {
            let task_id = req_str(payload, "taskId")?;
            tasks::delete_task(conn, org_id, actor, &task_id)?;
            Ok(Value::Null)
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
        "task.sediment_retry" => {
            let task_id = req_str(payload, "taskId")?;
            let report_id = req_str(payload, "reportId")?;
            ensure_report_in_org(conn, org_id, &report_id)?;
            knowledge::retry_sediment(conn, org_id, &task_id, &report_id, actor)?;
            Ok(Value::Null)
        }

        // ── 任务讨论（沟通层）────────────────────────────────
        "comment.list" => {
            let task_id = req_str(payload, "taskId")?;
            to_value(super::comments::list_comments(conn, org_id, &task_id)?)
        }
        "comment.add" => {
            let task_id = req_str(payload, "taskId")?;
            let content = req_str(payload, "content")?;
            let reply_to = opt_str(payload, "replyToId");
            let mentions = str_vec(payload, "mentions");
            to_value(super::comments::add_comment(
                conn,
                org_id,
                actor,
                &task_id,
                &content,
                reply_to.as_deref(),
                &mentions,
            )?)
        }
        "comment.delete" => {
            let comment_id = req_str(payload, "commentId")?;
            super::comments::delete_comment(conn, org_id, actor, &comment_id)?;
            Ok(Value::Null)
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
        // ── 组织知识库：人工投喂与待整理队列 ──
        // 主机侧只负责**入队**：模型整理需要主库里的 Provider 配置，由主机后台循环执行
        // （成员端不缓存团队数据，也不该解析主机 Provider）。
        "kb.ingest.capability" => {
            let cached = knowledge::load_ingest_capability(conn, org_id);
            let pending = knowledge::ingest_queue_pending(conn, org_id);
            let mut cap: Value = cached
                .and_then(|json| serde_json::from_str::<Value>(&json).ok())
                .unwrap_or_else(|| {
                    serde_json::json!({
                        "canProcess": "none",
                        "providerName": "",
                        "model": "",
                        "trustDomain": "",
                        "reason": "主机尚未上报加工能力",
                        "pending": pending,
                    })
                });
            if let Some(obj) = cap.as_object_mut() {
                obj.insert("pending".into(), Value::from(pending));
            }
            Ok(cap)
        }
        "kb.queue.list" => {
            let kb_id = opt_str(payload, "kbId");
            to_value(knowledge::list_ingest_queue(
                conn,
                org_id,
                kb_id.as_deref(),
            )?)
        }
        "kb.queue.retry" => {
            let id = req_str(payload, "queueId")?;
            knowledge::retry_ingest(conn, org_id, &id)?;
            Ok(Value::Null)
        }
        "kb.queue.drop" => {
            let id = req_str(payload, "queueId")?;
            to_value(knowledge::drop_ingest(conn, org_id, &id)?)
        }
        "kb.ingest.snippet" | "kb.ingest.url" => {
            let kb_id = req_str(payload, "kbId")?;
            let source_ref = req_str(payload, "sourceRef")?;
            let body = payload.get("payload").cloned().unwrap_or(Value::Null);
            let kind = if command == "kb.ingest.snippet" { "snippet" } else { "url" };
            let queue_id = knowledge::enqueue_ingest(
                conn,
                org_id,
                &kb_id,
                kind,
                &source_ref,
                &body.to_string(),
            )?;
            to_value(serde_json::json!({
                "queued": true,
                "queueId": queue_id,
                "entries": 0,
            }))
        }
        "kb.queue.run" => {
            // 成员无法在本地执行；后台循环会接手
            let pending = knowledge::ingest_queue_pending(conn, org_id);
            to_value(serde_json::json!({
                "done": 0,
                "failed": 0,
                "stopped": true,
                "remaining": pending,
            }))
        }

        // 手工整理第一步：把队列项原文提取出来给前端回填表单（不写库）
        "kb.ingest.preview" => {
            require_kb_edit(conn, org_id, actor)?;
            let id = req_str(payload, "queueId")?;
            let row = knowledge::get_ingest(conn, org_id, &id)?;
            let p: Value = serde_json::from_str(&row.payload).unwrap_or(Value::Null);
            let (title, content, _) =
                knowledge::manual_ingest_text(conn, org_id, &row.kind, &p)?;
            to_value(serde_json::json!({ "title": title, "content": content }))
        }
        // 手工整理第二步：用户确认（可改标题/正文/标签，补专属属性）后写入组织库
        "kb.ingest.commit" => {
            require_kb_edit(conn, org_id, actor)?;
            let id = req_str(payload, "queueId")?;
            let title = opt_str(payload, "title").unwrap_or_default();
            let content = req_str(payload, "content")?;
            let tags = opt_str(payload, "tags").unwrap_or_default();
            let fields = fields_value(payload)?;
            let row = knowledge::get_ingest(conn, org_id, &id)?;
            knowledge::commit_manual_ingest(
                conn,
                org_id,
                &row,
                &title,
                &content,
                &tags,
                &fields,
                actor,
            )?;
            to_value(serde_json::json!({
                "queued": false,
                "queueId": row.id,
                "entries": 1,
            }))
        }

        "kb.ingest.file" => {
            let kb_id = req_str(payload, "kbId")?;
            let token = req_str(payload, "token")?;
            let file_name = opt_str(payload, "fileName").unwrap_or_else(|| "file".to_string());
            let temp = super::host::resolve_upload_token(org_id, actor, &token)?;
            let bytes = std::fs::read(&temp).map_err(|e| format!("读取上传文件失败：{}", e))?;
            let _ = std::fs::remove_file(&temp);
            let imported = files::import_bytes(conn, org_id, &file_name, &bytes)?;
            let body = serde_json::json!({
                "fileId": imported.file_id,
                "fileName": imported.file_name,
                "relPath": imported.rel_path,
            });
            let queue_id = knowledge::enqueue_ingest(
                conn,
                org_id,
                &kb_id,
                "file",
                &imported.rel_path,
                &body.to_string(),
            )?;
            to_value(serde_json::json!({
                "queued": true,
                "queueId": queue_id,
                "entries": 0,
            }))
        }
        "kb.candidate.add" => {
            let kb_id = req_str(payload, "kbId")?;
            let title = opt_str(payload, "title").unwrap_or_default();
            let content = opt_str(payload, "content").unwrap_or_default();
            let tags = opt_str(payload, "tags").unwrap_or_default();
            let metadata = opt_str(payload, "metadata").unwrap_or_default();
            let origin = opt_str(payload, "origin").unwrap_or_else(|| "ai".to_string());
            to_value(knowledge::add_candidate(
                conn,
                org_id,
                &kb_id,
                actor,
                &title,
                &content,
                &tags,
                &metadata,
                &origin,
            )?)
        }
        "kb.candidate.list" => {
            let kb_id = req_str(payload, "kbId")?;
            to_value(knowledge::list_candidates(conn, org_id, &kb_id)?)
        }
        "kb.candidate.adopt" => {
            let kb_id = req_str(payload, "kbId")?;
            let id = req_str(payload, "candidateId")?;
            // 采纳前可修正标题 / 正文 / 标签 / 专属属性（缺省 = 用模型原值）
            let title = opt_str(payload, "title");
            let content = opt_str(payload, "content");
            let tags = opt_str(payload, "tags");
            let fields = fields_value(payload)?;
            to_value(knowledge::adopt_candidate(
                conn,
                org_id,
                &kb_id,
                &id,
                title.as_deref(),
                content.as_deref(),
                tags.as_deref(),
                &fields,
                actor,
            )?)
        }
        "kb.candidate.reject" => {
            let kb_id = req_str(payload, "kbId")?;
            let id = req_str(payload, "candidateId")?;
            to_value(knowledge::reject_candidate(conn, org_id, &kb_id, &id)?)
        }

        "kb.list" => to_value(knowledge::list_kbs(conn, org_id)?),
        // 库定义管理：与「编辑条目」同档（owner / admin）
        "kb.update" => {
            require_kb_edit(conn, org_id, actor)?;
            let kb_id = req_str(payload, "kbId")?;
            let name = req_str(payload, "name")?;
            let description = opt_str(payload, "description").unwrap_or_default();
            let field_schema = opt_str(payload, "fieldSchema").unwrap_or_default();
            to_value(knowledge::update_kb(
                conn,
                org_id,
                &kb_id,
                &name,
                &description,
                &field_schema,
            )?)
        }
        "kb.create" => {
            require_kb_edit(conn, org_id, actor)?;
            let name = req_str(payload, "name")?;
            let description = opt_str(payload, "description").unwrap_or_default();
            let field_schema = opt_str(payload, "fieldSchema").unwrap_or_default();
            to_value(knowledge::create_kb(
                conn,
                org_id,
                &name,
                &description,
                &field_schema,
                actor,
            )?)
        }
        "kb.delete" => {
            require_kb_edit(conn, org_id, actor)?;
            let kb_id = req_str(payload, "kbId")?;
            knowledge::delete_kb(conn, org_id, &kb_id)?;
            to_value(serde_json::json!({ "deleted": true }))
        }
        "kb.search" => {
            let query = req_str(payload, "query")?;
            let kb_id = opt_str(payload, "kbId");
            let limit = payload.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let offset = payload.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
            to_value(knowledge::search_entries(
                conn,
                org_id,
                kb_id.as_deref(),
                &query,
                limit,
                offset,
            )?)
        }
        "kb.entry_get" => {
            let kb_id = req_str(payload, "kbId")?;
            let memory_id = req_str(payload, "memoryId")?;
            to_value(knowledge::get_entry(conn, org_id, &kb_id, &memory_id)?)
        }
        // 条目编辑 / 移除：与「编辑库定义」同档（owner / admin）
        "kb.entry_update" => {
            require_kb_edit(conn, org_id, actor)?;
            let kb_id = req_str(payload, "kbId")?;
            let memory_id = req_str(payload, "memoryId")?;
            let title = req_str(payload, "title")?;
            let content = opt_str(payload, "content").unwrap_or_default();
            let tags = opt_str(payload, "tags").unwrap_or_default();
            let fields = fields_value(payload)?;
            to_value(knowledge::update_entry(
                conn,
                org_id,
                &kb_id,
                &memory_id,
                &title,
                &content,
                &tags,
                &fields,
                actor,
            )?)
        }
        "kb.entry_remove" => {
            require_kb_edit(conn, org_id, actor)?;
            let kb_id = req_str(payload, "kbId")?;
            let memory_id = req_str(payload, "memoryId")?;
            knowledge::remove_entry(conn, org_id, &kb_id, &memory_id)?;
            Ok(Value::Null)
        }
        "kb.file_list" => {
            let kb_id = req_str(payload, "kbId")?;
            // 可选分页：成员端「加载更多」带 limit/offset；不带时保持整库口径
            let limit = payload.get("limit").and_then(|v| v.as_i64());
            let offset = payload.get("offset").and_then(|v| v.as_i64());
            to_value(knowledge::list_files_page(conn, org_id, &kb_id, limit, offset)?)
        }
        // 文件之间的关系（正文—附件 / 新版—旧版）：读成员可看，写仅 owner / admin
        "kb.file_relations" => {
            let kb_id = req_str(payload, "kbId")?;
            crate::enclave::authz::role_for_member(conn, org_id, actor)?
                .ok_or_else(|| "当前账号不是该组织成员".to_string())?;
            to_value(knowledge::list_file_relations(conn, org_id, &kb_id)?)
        }
        "kb.file_link" => {
            require_kb_edit(conn, org_id, actor)?;
            let kb_id = req_str(payload, "kbId")?;
            let kind = req_str(payload, "kind")?;
            let from = req_str(payload, "fromFileId")?;
            let to = req_str(payload, "toFileId")?;
            knowledge::link_file_relation(conn, org_id, &kb_id, actor, &kind, &from, &to)?;
            Ok(Value::Null)
        }
        "kb.file_unlink" => {
            require_kb_edit(conn, org_id, actor)?;
            let kb_id = req_str(payload, "kbId")?;
            let kind = req_str(payload, "kind")?;
            let from = req_str(payload, "fromFileId")?;
            let to = req_str(payload, "toFileId")?;
            to_value(knowledge::unlink_file_relation(
                conn, org_id, &kb_id, &kind, &from, &to,
            )?)
        }

        // ── 文件回收（管理角色）──
        "kb.entry_file_detach" => {
            require_kb_edit(conn, org_id, actor)?;
            let kb_id = req_str(payload, "kbId")?;
            let memory_id = req_str(payload, "memoryId")?;
            let file_id = req_str(payload, "fileId")?;
            knowledge::detach_entry_file(conn, org_id, &kb_id, &memory_id, &file_id)?;
            Ok(Value::Null)
        }
        "kb.file_delete" => {
            require_kb_edit(conn, org_id, actor)?;
            let file_id = req_str(payload, "fileId")?;
            files::delete_asset(conn, org_id, &file_id)?;
            Ok(Value::Null)
        }
        "kb.orphan_files" => {
            // 只读列表：成员即可查看
            crate::enclave::authz::role_for_member(conn, org_id, actor)?
                .ok_or_else(|| "当前账号不是该组织成员".to_string())?;
            to_value(files::list_orphan_files(conn, org_id)?)
        }
        "kb.orphan_cleanup" => {
            require_kb_edit(conn, org_id, actor)?;
            to_value(files::cleanup_orphan_files(conn, org_id)?)
        }

        other => Err(format!("未知的飞地命令：{}", other)),
    }
}
