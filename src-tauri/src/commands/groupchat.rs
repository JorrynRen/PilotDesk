//! 群聊 Tauri 命令（统一前缀 `groupchat_`）。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::db::models::Attachment;
use crate::groupchat::models::{MessageRow, ParticipantRow, Room, TaskRow};
use crate::groupchat::room::{ConfirmationResponse, RoomCommand, RoomRegistry};
use crate::groupchat::store;
use crate::api_agent::agent_loop::SecurityMode;
use crate::DbState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRoomInput {
    pub title: String,
    pub topic: String,
    pub participants: Vec<ParticipantInput>,
    pub director_id: String,
    /// 是否允许主持人在自动补人时添加 CLI 参与者（缺省 1=允许；按任务差异化限制）
    #[serde(default = "default_allow_auto_cli")]
    pub allow_auto_cli: i64,
    /// 房间统一产物目录（绝对路径；缺省空=运行时回退 `<工作目录>/outputs/<房间标题>/`）
    #[serde(default)]
    pub output_dir: String,
}

fn default_allow_auto_cli() -> i64 {
    1
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantInput {
    pub id: String,
    pub participant_type: String,
    pub agent_config: String,
    pub display_name: String,
    pub system_role: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageInput {
    pub room_id: String,
    pub content: String,
    #[serde(default)]
    pub recipients: Option<Vec<String>>,
    pub reply_to: Option<String>,
    /// 用户 @ 指定的参与者 id（可为空）。
    #[serde(default)]
    pub mention: Option<String>,
    /// 已落盘的附件元数据（图片/文件）。
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// 会话安全模式（本波次有效；strict/standard/relaxed/unrestricted，缺省标准）。
    #[serde(default)]
    pub security_mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RespondConfirmationInput {
    pub room_id: String,
    pub request_id: String,
    pub responses: Vec<ConfirmationResponseInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmationResponseInput {
    pub item_id: String,
    pub value: String,
}

#[tauri::command]
pub fn groupchat_create_room(
    app: AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    input: CreateRoomInput,
) -> Result<Room, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    let now = crate::utils::now();

    let room = Room {
        id: uuid::Uuid::new_v4().to_string(),
        title: input.title.clone(),
        topic: input.topic.clone(),
        status: "idle".into(),
        strategy: "round_robin".into(),
        max_rounds: 5,
        max_parallel: 2,
        director_id: Some(input.director_id.clone()),
        current_task_id: None,
        created_at: now,
        updated_at: now,
        goal_notes: "[]".into(),
        allow_auto_cli: input.allow_auto_cli,
        output_dir: input.output_dir,
    };

    store::insert_room(&conn, &room).map_err(|e| e.to_string())?;
    for p in &input.participants {
        let row = ParticipantRow {
            id: p.id.clone(),
            room_id: room.id.clone(),
            participant_type: p.participant_type.clone(),
            agent_config: p.agent_config.clone(),
            display_name: p.display_name.clone(),
            system_role: p.system_role.clone(),
            status: "active".into(),
        };
        store::insert_participant(&conn, &row).map_err(|e| e.to_string())?;
    }

    // 启动房间 Actor
    registry
        .spawn(state.pool.clone(), app.clone(), &room.id)
        .map_err(|e| e.to_string())?;

    Ok(room)
}

#[tauri::command]
pub fn groupchat_join_room(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
    participant: ParticipantInput,
) -> Result<ParticipantRow, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    let row = ParticipantRow {
        id: participant.id.clone(),
        room_id: room_id.clone(),
        participant_type: participant.participant_type,
        agent_config: participant.agent_config,
        display_name: participant.display_name,
        system_role: participant.system_role,
        status: "active".into(),
    };
    store::insert_participant(&conn, &row).map_err(|e| e.to_string())?;
    if let Some(handle) = registry.get(&room_id) {
        handle.send(RoomCommand::ParticipantChanged);
    }
    Ok(row)
}

/// 删除房间（停止 Actor + 清理注册表 + 级联删除数据库数据）。
#[tauri::command]
pub fn groupchat_delete_room(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
) -> Result<(), String> {
    if let Some(handle) = registry.get(&room_id) {
        handle.abort();
        handle.send(RoomCommand::Abort);
    }
    registry.remove(&room_id);
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::delete_room(&conn, &room_id).map_err(|e| e.to_string())
}

/// 移除参与者（数据库 + 运行时重建）。
#[tauri::command]
pub fn groupchat_remove_participant(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
    participant_id: String,
) -> Result<(), String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    // 保护结构性参与者：Director 与用户不可移除。
    if let Some(p) = store::get_participant(&conn, &room_id, &participant_id)
        .map_err(|e| e.to_string())?
    {
        if p.participant_type == "director" || p.participant_type == "user" {
            return Err("Director 与用户参与者不可移除".to_string());
        }
    }
    store::delete_participant(&conn, &room_id, &participant_id).map_err(|e| e.to_string())?;
    if let Some(handle) = registry.get(&room_id) {
        handle.send(RoomCommand::ParticipantChanged);
    }
    Ok(())
}

#[tauri::command]
pub fn groupchat_send_message(
    app: AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    input: SendMessageInput,
) -> Result<(), String> {
    // 自愈投递：句柄缺失或失效（Actor 异常退出残留）时自动重建再投，杜绝"消息被静默丢弃"。
    let content = input.content.clone();
    let recipients = input.recipients.unwrap_or_default();
    let reply_to = input.reply_to.clone();
    let mention = input.mention.clone();
    let attachments = input.attachments.clone();
    let security_mode = input.security_mode.as_deref().and_then(SecurityMode::from_str);
    registry
        .send_resilient(state.pool.clone(), app.clone(), &input.room_id, move || {
            RoomCommand::Send {
                sender: "user".into(),
                content: content.clone(),
                recipients: recipients.clone(),
                reply_to: reply_to.clone(),
                mention: mention.clone(),
                attachments: attachments.clone(),
                security_mode,
            }
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn groupchat_respond_confirmation(
    app: AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    input: RespondConfirmationInput,
) -> Result<(), String> {
    let responses = input
        .responses
        .into_iter()
        .map(|r| ConfirmationResponse {
            item_id: r.item_id,
            value: r.value,
        })
        .collect::<Vec<_>>();
    let request_id = input.request_id.clone();
    registry
        .send_resilient(state.pool.clone(), app.clone(), &input.room_id, move || {
            RoomCommand::RespondConfirmation {
                request_id: request_id.clone(),
                responses: responses.clone(),
            }
        })
        .map_err(|e| e.to_string())
}

/// 房间 Actor（进程内运行实例）是否存活。用于区分"真 running"（Actor 在跑）与
/// "假 running"（DB 仍标 running 但进程内无 Actor，如异常退出后未恢复）。
#[tauri::command]
pub fn groupchat_room_actor_alive(registry: State<'_, RoomRegistry>, room_id: String) -> bool {
    registry.get(&room_id).is_some()
}

#[tauri::command]
pub fn groupchat_set_director(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
    new_director_id: String,
) -> Result<Room, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::update_room_director(&conn, &room_id, &new_director_id).map_err(|e| e.to_string())?;
    if let Some(handle) = registry.get(&room_id) {
        handle.send(RoomCommand::SetDirector {
            director_id: new_director_id,
        });
    }
    let room = store::get_room(&conn, &room_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "房间不存在".to_string())?;
    Ok(room)
}

#[tauri::command]
pub fn groupchat_set_output_dir(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
    output_dir: String,
) -> Result<Room, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::update_room_output_dir(&conn, &room_id, &output_dir).map_err(|e| e.to_string())?;
    if let Some(handle) = registry.get(&room_id) {
        handle.send(RoomCommand::SetOutputDir(output_dir));
    }
    store::get_room(&conn, &room_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "房间不存在".to_string())
}

#[tauri::command]
pub fn groupchat_pause(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
) -> Result<Room, String> {
    if let Some(handle) = registry.get(&room_id) {
        handle.pause();
        handle.send(RoomCommand::Pause);
    }
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::update_room_status(&conn, &room_id, "paused").map_err(|e| e.to_string())?;
    Ok(store::get_room(&conn, &room_id)
        .map_err(|e| e.to_string())?
        .unwrap())
}

#[tauri::command]
pub fn groupchat_resume(
    app: AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
) -> Result<Room, String> {
    // 恢复/重启：先确保 Actor 存活（缺失则 spawn；句柄残留失效则移除重建），再解除暂停
    // （外部 notify，覆盖 actor 卡在 wait_if_paused 的情形），最后投 Resume 触发续跑/清终止标记。
    let (handle, respawned) = {
        let handle = registry.get_or_spawn(state.pool.clone(), app.clone(), &room_id).map_err(|e| e.to_string())?;
        if !handle.send(RoomCommand::Resume) {
            // Actor 已异常退出但句柄残留：移除并重建后重投。
            log::warn!("[GroupChat] 房间 {} resume 投递失败，移除残留句柄并重建 Actor", room_id);
            registry.remove(&room_id);
            let handle = registry.spawn(state.pool.clone(), app.clone(), &room_id).map_err(|e| e.to_string())?;
            (handle, true)
        } else {
            (handle, false)
        }
    };
    let _ = respawned;
    handle.resume(); // 解除暂停并 notify（actor 若阻塞在 wait_if_paused 则立即恢复）
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::update_room_status(&conn, &room_id, "running").map_err(|e| e.to_string())?;
    Ok(store::get_room(&conn, &room_id)
        .map_err(|e| e.to_string())?
        .unwrap())
}

#[tauri::command]
pub fn groupchat_abort(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
) -> Result<Room, String> {
    if let Some(handle) = registry.get(&room_id) {
        handle.abort();
        handle.send(RoomCommand::Abort);
    }
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::update_room_status(&conn, &room_id, "aborted").map_err(|e| e.to_string())?;
    Ok(store::get_room(&conn, &room_id)
        .map_err(|e| e.to_string())?
        .unwrap())
}

#[tauri::command]
pub fn groupchat_get_room(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<Room, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::get_room(&conn, &room_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "房间不存在".to_string())
}

/// 消息分页结果：`messages` 为按 seq 升序的一页，`total` 为房间消息总数。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub messages: Vec<MessageRow>,
    pub total: i64,
}

#[tauri::command]
pub fn groupchat_get_messages(
    state: State<'_, DbState>,
    room_id: String,
    before_seq: Option<i64>,
    limit: Option<i64>,
) -> Result<MessagePage, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    let limit = limit.unwrap_or(100).max(1).min(500);
    let total = store::count_messages(&conn, &room_id).map_err(|e| e.to_string())?;
    let messages = store::list_messages_page(&conn, &room_id, before_seq, limit).map_err(|e| e.to_string())?;
    Ok(MessagePage { messages, total })
}

#[tauri::command]
pub fn groupchat_get_tasks(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<Vec<TaskRow>, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::list_tasks(&conn, &room_id).map_err(|e| e.to_string())
}

/// 房间「消息事件」全局水位：该房间最大 message 事件 seq（无记录为 0）。
/// 前端在整页加载后把投影水位同步到该值，使已应用/已加载的消息在重复/乱序
/// 派发时可按 seq 直接丢弃（投影水位阶段②/③）。
#[tauri::command]
pub fn groupchat_message_watermark(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<i64, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM room_events WHERE room_id = ?1 AND kind = 'message'",
        rusqlite::params![room_id],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn groupchat_list_rooms(state: State<'_, DbState>) -> Result<Vec<Room>, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::list_rooms(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn groupchat_get_participants(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<Vec<ParticipantRow>, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::list_participants(&conn, &room_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn groupchat_get_stances(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<Vec<crate::groupchat::models::StanceRow>, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    store::list_stances(&conn, &room_id).map_err(|e| e.to_string())
}

/// 终态导出：把群聊结论 + 任务议程导出为工作流定义草稿（模板化 JSON，供人工编辑）。
#[tauri::command]
pub fn groupchat_export_workflow(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<serde_json::Value, String> {
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    let room = store::get_room(&conn, &room_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "房间不存在".to_string())?;
    let tasks = store::list_tasks(&conn, &room_id).map_err(|e| e.to_string())?;

    // 结论统一取自消息表 `kind='conclusion'`（房间表 summary 字段已移除）。
    let conclusion_msg = store::list_messages(&conn, &room_id, None, None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .rev()
        .find(|m| m.kind == "conclusion")
        .map(|m| m.content);

    let conclusion = match conclusion_msg {
        Some(c) if !c.is_empty() => c,
        _ => format!("群聊「{}」讨论产出，共 {} 项任务。", room.title, tasks.len()),
    };
    let now = crate::utils::now();
    let stage_id = uuid::Uuid::new_v4().to_string();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    // 起始与结束边界节点
    nodes.push(serde_json::json!({
        "id": "start",
        "type": "start",
        "label": "开始",
        "isBoundary": true,
        "position": { "x": 80, "y": 80 },
    }));
    nodes.push(serde_json::json!({
        "id": "end",
        "type": "end",
        "label": "结束",
        "isBoundary": true,
        "position": { "x": 80, "y": 80 + tasks.len() as i64 * 120 + 120 },
    }));

    let mut prev_id = "start".to_string();
    for (i, t) in tasks.iter().enumerate() {
        let node_id = format!("task-{}", i + 1);
        let label = if t.description.chars().count() > 24 {
            format!("{}…", t.description.chars().take(24).collect::<String>())
        } else {
            t.description.clone()
        };
        nodes.push(serde_json::json!({
            "id": node_id,
            "type": "agent",
            "label": label,
            "params": {
                "prompt": t.description,
                "assignee": t.assignee,
            },
            "position": { "x": 80, "y": 80 + (i as i64 + 1) * 120 },
        }));
        edges.push(serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "source": prev_id,
            "target": node_id,
        }));
        prev_id = node_id;
    }
    edges.push(serde_json::json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "source": prev_id,
        "target": "end",
    }));

    Ok(serde_json::json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "name": format!("{}（群聊导出）", room.title),
        "version": "1.0.0",
        "description": conclusion,
        "trigger": { "triggerType": "manual" },
        "stages": [{
            "id": stage_id,
            "name": "群聊任务",
            "order": 0,
            "nodes": nodes,
            "edges": edges,
            "stageEdges": [],
            "gate": { "strategy": "all", "mergeStrategy": "merge" },
            "collapsed": false,
            "offsetX": 0,
            "offsetY": 0,
        }],
        "icon": null,
        "inputSchema": null,
        "outputSchema": null,
        "createdAt": now,
        "updatedAt": now,
        "enabled": false,
    }))
}
