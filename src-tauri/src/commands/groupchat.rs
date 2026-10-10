//! 群聊 Tauri 命令（统一前缀 `groupchat_`）。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::api_agent::agent_loop::SecurityMode;
use crate::db::models::Attachment;
use crate::groupchat::models::{MessageRow, ParticipantRow, Room, TaskRow};
use crate::groupchat::room::{ConfirmationResponse, RoomCommand, RoomRegistry};
use crate::groupchat::store;
use crate::utils::errors::AppError;
use crate::workflow::{
    GateConfig, Stage, TriggerConfig, TriggerType, WorkflowDefinition, WorkflowEdge, WorkflowNode,
    WorkflowNodeType,
};
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
    let conn = state.get_conn()?;
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
        // 派生字段：只有 list_rooms 会填
        current_round: None,
        task_total: None,
        task_finished: None,
    };

    store::insert_room(&conn, &room)?;
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
        store::insert_participant(&conn, &row)?;
    }

    // 启动房间 Actor
    registry.spawn(state.pool.clone(), app.clone(), &room.id)?;

    Ok(room)
}

#[tauri::command]
pub fn groupchat_join_room(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
    participant: ParticipantInput,
) -> Result<ParticipantRow, String> {
    let conn = state.get_conn()?;
    let row = ParticipantRow {
        id: participant.id.clone(),
        room_id: room_id.clone(),
        participant_type: participant.participant_type,
        agent_config: participant.agent_config,
        display_name: participant.display_name,
        system_role: participant.system_role,
        status: "active".into(),
    };
    store::insert_participant(&conn, &row)?;
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
    let conn = state.get_conn()?;
    store::delete_room(&conn, &room_id)?;
    Ok(())
}

/// 移除参与者（数据库 + 运行时重建）。
#[tauri::command]
pub fn groupchat_remove_participant(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
    participant_id: String,
) -> Result<(), String> {
    let conn = state.get_conn()?;
    // 保护结构性参与者：Director 与用户不可移除。
    if let Some(p) = store::get_participant(&conn, &room_id, &participant_id)? {
        if p.participant_type == "director" || p.participant_type == "user" {
            return Err(AppError::InvalidInput("Director 与用户参与者不可移除".to_string()).into());
        }
    }
    store::delete_participant(&conn, &room_id, &participant_id)?;
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
    let security_mode = input
        .security_mode
        .as_deref()
        .and_then(SecurityMode::from_str);
    registry.send_resilient(state.pool.clone(), app.clone(), &input.room_id, move || {
        RoomCommand::Send {
            sender: "user".into(),
            content: content.clone(),
            recipients: recipients.clone(),
            reply_to: reply_to.clone(),
            mention: mention.clone(),
            attachments: attachments.clone(),
            security_mode,
        }
    })?;
    Ok(())
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
    registry.send_resilient(state.pool.clone(), app.clone(), &input.room_id, move || {
        RoomCommand::RespondConfirmation {
            request_id: request_id.clone(),
            responses: responses.clone(),
        }
    })?;
    Ok(())
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
    let conn = state.get_conn()?;
    store::update_room_director(&conn, &room_id, &new_director_id)?;
    if let Some(handle) = registry.get(&room_id) {
        handle.send(RoomCommand::SetDirector {
            director_id: new_director_id,
        });
    }
    let room = store::get_room(&conn, &room_id)?
        .ok_or_else(|| AppError::NotFound("房间不存在".to_string()))?;
    Ok(room)
}

#[tauri::command]
pub fn groupchat_set_output_dir(
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    room_id: String,
    output_dir: String,
) -> Result<Room, String> {
    let conn = state.get_conn()?;
    store::update_room_output_dir(&conn, &room_id, &output_dir)?;
    if let Some(handle) = registry.get(&room_id) {
        handle.send(RoomCommand::SetOutputDir(output_dir));
    }
    Ok(store::get_room(&conn, &room_id)?
        .ok_or_else(|| AppError::NotFound("房间不存在".to_string()))?)
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
    let conn = state.get_conn()?;
    store::update_room_status(&conn, &room_id, "paused")?;
    Ok(store::get_room(&conn, &room_id)?.unwrap())
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
        let handle = registry.get_or_spawn(state.pool.clone(), app.clone(), &room_id)?;
        if !handle.send(RoomCommand::Resume) {
            // Actor 已异常退出但句柄残留：移除并重建后重投。
            log::warn!(
                "[GroupChat] 房间 {} resume 投递失败，移除残留句柄并重建 Actor",
                room_id
            );
            registry.remove(&room_id);
            let handle = registry.spawn(state.pool.clone(), app.clone(), &room_id)?;
            (handle, true)
        } else {
            (handle, false)
        }
    };
    let _ = respawned;
    handle.resume(); // 解除暂停并 notify（actor 若阻塞在 wait_if_paused 则立即恢复）
    let conn = state.get_conn()?;
    store::update_room_status(&conn, &room_id, "running")?;
    Ok(store::get_room(&conn, &room_id)?.unwrap())
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
    let conn = state.get_conn()?;
    store::update_room_status(&conn, &room_id, "aborted")?;
    Ok(store::get_room(&conn, &room_id)?.unwrap())
}

#[tauri::command]
pub fn groupchat_get_room(state: State<'_, DbState>, room_id: String) -> Result<Room, String> {
    let conn = state.get_conn()?;
    Ok(store::get_room(&conn, &room_id)?
        .ok_or_else(|| AppError::NotFound("房间不存在".to_string()))?)
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
    let conn = state.get_conn()?;
    let limit = limit.unwrap_or(100).max(1).min(500);
    let total = store::count_messages(&conn, &room_id)?;
    let messages = store::list_messages_page(&conn, &room_id, before_seq, limit)?;
    Ok(MessagePage { messages, total })
}

#[tauri::command]
pub fn groupchat_get_tasks(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<Vec<TaskRow>, String> {
    let conn = state.get_conn()?;
    Ok(store::list_tasks(&conn, &room_id)?)
}

// ── 任务级人工干预（手动跳过 / 手动新增 / 修改依赖）──
//
// 任务表是事件源（`room_events` 的 `task/created|task/status`），故干预直接写库 + 广播
// `task_updated`：Actor 的调度每轮都从库里重建任务与依赖（`execute_tasks` / `ready_layer_ids`
// 均调 `list_tasks`），写库即对调度生效 —— 无需新增 RoomCommand 变体（那要在 6 处命令 match
// 上重复铺分支，收益不抵复杂度）。
//
// 边界：只允许干预**未进入执行**的任务（discussing/pending）；running 与终态一律拒绝，
// 避免与 Actor 正在写入的状态互相踩踏。

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddTaskInput {
    pub room_id: String,
    pub description: String,
    /// 前置任务 **id** 列表（存储层下标由后端换算，见 `task::resolve_dep_indices`）
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub assignee: Option<String>,
}

/// 广播任务变更（形态与 Actor 的 `task_updated` 完全一致，前端按 id 归并）。
fn emit_task_updated(app: &AppHandle, task: &TaskRow) {
    use tauri::Emitter;
    let _ = app.emit(
        "groupchat-event",
        &crate::groupchat::event::GroupChatEvent::task_updated(&task.room_id, task.clone()),
    );
}

/// 人工干预的状态前置条件：仅讨论中/待执行可改。
fn ensure_intervenable(task: &TaskRow) -> Result<(), AppError> {
    match task.status.as_str() {
        "discussing" | "pending" => Ok(()),
        "running" => Err(AppError::InvalidInput(
            "任务正在执行中，请先暂停或停止房间后再操作".to_string(),
        )),
        _ => Err(AppError::InvalidInput("任务已结束，无法再修改".to_string())),
    }
}

/// 任务编号列表 → `T1、T3` 文案（错误/提示信息复用）。
fn task_nos_text(nos: &[i64]) -> String {
    nos.iter()
        .map(|n| format!("T{}", n))
        .collect::<Vec<_>>()
        .join("、")
}

/// 手动跳过子任务：置 skipped 并说明原因（跳过是终态，依赖它的后续任务会因前置未成功而被调度跳过）。
#[tauri::command]
pub fn groupchat_task_skip(
    app: AppHandle,
    state: State<'_, DbState>,
    room_id: String,
    task_id: String,
) -> Result<TaskRow, String> {
    let conn = state.get_conn()?;
    let mut task = store::get_task(&conn, &room_id, &task_id)?
        .ok_or_else(|| AppError::NotFound("任务不存在".to_string()))?;
    ensure_intervenable(&task)?;
    task.status = "skipped".to_string();
    task.error = Some("用户手动跳过".to_string());
    task.completed_at = Some(crate::utils::now());
    store::update_task(&conn, &task)?;
    emit_task_updated(&app, &task);
    Ok(task)
}

/// 手动追加子任务（只增不改：不改动既有任务与目标锚点，与临时任务同语义）。
#[tauri::command]
pub fn groupchat_task_add(
    app: AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, RoomRegistry>,
    input: AddTaskInput,
) -> Result<TaskRow, String> {
    let conn = state.get_conn()?;
    let room = store::get_room(&conn, &input.room_id)?
        .ok_or_else(|| AppError::NotFound("房间不存在".to_string()))?;
    let description = input.description.trim().to_string();
    if description.is_empty() {
        return Err(AppError::InvalidInput("任务描述不能为空".to_string()).into());
    }
    let tasks = store::list_tasks(&conn, &input.room_id)?;
    // 新任务尚无 id，target_id 传空（自依赖不可能成立）
    let deps = crate::groupchat::task::resolve_dep_indices(&tasks, "", &input.depends_on)?;
    let assignee = match input
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(id) => {
            let participants = store::list_participants(&conn, &input.room_id)?;
            // 只有 api/cli 参与者能执行任务；user/director 指派过去等于调度侧的"无可用执行者"。
            let ok = participants
                .iter()
                .any(|p| p.id == id && matches!(p.participant_type.as_str(), "api" | "cli"));
            if !ok {
                return Err(AppError::InvalidInput(
                    "负责人必须是房间内的 Agent 参与者（API/CLI）".to_string(),
                )
                .into());
            }
            Some(id.to_string())
        }
        None => None,
    };
    let task = TaskRow {
        id: uuid::Uuid::new_v4().to_string(),
        room_id: input.room_id.clone(),
        task_no: tasks.iter().map(|t| t.task_no).max().unwrap_or(0) + 1,
        description,
        assignee,
        depends_on: serde_json::to_string(&deps).unwrap_or_else(|_| "[]".to_string()),
        // 起始状态与既有新增路径（kickoff/replan/临时任务）一致：先讨论定性，再进入执行
        status: "discussing".to_string(),
        result_summary: None,
        error: None,
        started_at: None,
        completed_at: None,
    };
    store::insert_task(&conn, &task)?;
    emit_task_updated(&app, &task);

    // 房间已收敛（finished/idle）时无人再调度：唤醒 Actor 走一遍讨论→执行→收尾。
    // paused/aborted 刻意不唤醒 —— 用户的暂停/停止意图优先，恢复时 `resume_execution` 自会读到新任务。
    if matches!(room.status.as_str(), "finished" | "idle") {
        registry.send_resilient(state.pool.clone(), app.clone(), &input.room_id, || {
            RoomCommand::Resume
        })?;
    }
    Ok(task)
}

/// 修改子任务依赖（仅 discussing/pending；成环直接拒绝）。
#[tauri::command]
pub fn groupchat_task_update_deps(
    app: AppHandle,
    state: State<'_, DbState>,
    room_id: String,
    task_id: String,
    depends_on: Vec<String>,
) -> Result<TaskRow, String> {
    let conn = state.get_conn()?;
    let mut task = store::get_task(&conn, &room_id, &task_id)?
        .ok_or_else(|| AppError::NotFound("任务不存在".to_string()))?;
    ensure_intervenable(&task)?;
    let tasks = store::list_tasks(&conn, &room_id)?;
    let idx = tasks
        .iter()
        .position(|t| t.id == task_id)
        .ok_or_else(|| AppError::NotFound("任务不存在".to_string()))?;
    let deps = crate::groupchat::task::resolve_dep_indices(&tasks, &task_id, &depends_on)?;
    let preview = crate::groupchat::task::preview_dep_change(&tasks, idx, &deps);
    if !preview.cycle_task_nos.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "任务依赖存在环（涉及 {}），请调整依赖后重试",
            task_nos_text(&preview.cycle_task_nos)
        ))
        .into());
    }
    task.depends_on = serde_json::to_string(&deps).unwrap_or_else(|_| "[]".to_string());
    store::update_task(&conn, &task)?;
    emit_task_updated(&app, &task);
    Ok(task)
}

/// 依赖变更预览（纯计算，不落库）：确认卡据此展示"改完会阻塞/解锁谁、是否成环"。
#[tauri::command]
pub fn groupchat_task_deps_preview(
    state: State<'_, DbState>,
    room_id: String,
    task_id: String,
    depends_on: Vec<String>,
) -> Result<crate::groupchat::task::DepChangePreview, String> {
    let conn = state.get_conn()?;
    let tasks = store::list_tasks(&conn, &room_id)?;
    let idx = tasks
        .iter()
        .position(|t| t.id == task_id)
        .ok_or_else(|| AppError::NotFound("任务不存在".to_string()))?;
    let deps = crate::groupchat::task::resolve_dep_indices(&tasks, &task_id, &depends_on)?;
    Ok(crate::groupchat::task::preview_dep_change(
        &tasks, idx, &deps,
    ))
}

/// 房间「消息事件」全局水位：该房间最大**业务消息序号**（payload.seq，无记录为 0）。
/// 前端在整页加载后把投影水位同步到该值，使已应用/已加载的消息在重复/乱序
/// 派发时可按 seq 直接丢弃（投影水位阶段②/③）。
///
/// 口径必须与前端比较的 `event.message.seq` 一致，即 payload 内的业务消息序号；
/// 不可用 `room_events.seq`（跨房间、跨事件类型的全局自增主键）——两者混用会把所有
/// 新消息误判为「早于水位」而丢弃，表现为消息不实时、刷新后才出现。
#[tauri::command]
pub fn groupchat_message_watermark(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<i64, String> {
    let conn = state.get_conn()?;
    Ok(store::max_message_seq(&conn, &room_id))
}

#[tauri::command]
pub fn groupchat_list_rooms(state: State<'_, DbState>) -> Result<Vec<Room>, String> {
    let conn = state.get_conn()?;
    Ok(store::list_rooms(&conn)?)
}

#[tauri::command]
pub fn groupchat_get_participants(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<Vec<ParticipantRow>, String> {
    let conn = state.get_conn()?;
    Ok(store::list_participants(&conn, &room_id)?)
}

#[tauri::command]
pub fn groupchat_get_stances(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<Vec<crate::groupchat::models::StanceRow>, String> {
    let conn = state.get_conn()?;
    Ok(store::list_stances(&conn, &room_id)?)
}

// ── 群聊任务 DAG → 工作流定义（精准转换）──
//
// 两个出口共用同一份定义：
// - `groupchat_export_workflow`：返回定义 JSON（前端预览/另存）；
// - `groupchat_promote_workflow`：直接把定义落库 —— 工作流列表可见、可直接运行。
//
// 相比旧的"草稿导出"，这里修掉了三处会导致"能打开但跑不出结果"的问题：
// ① 节点参数改用运行期真名（旧稿写的 `params.prompt`/`assignee` 无任何读取点，
//    `prompt_template`/`agent_type` 缺失会让运行前校验直接判 error）；
// ② 依赖不再是线性链，而是任务真实 DAG；
// ③ 补上 outputMapping/inputMapping —— 引擎不会自动把上游产出拼进提示词，
//    且按"入边源节点在上下文里是否有产出"判定边是否连通（engine.rs `any_incoming_edge_active`）。

/// 参与者 `agent_config` → agent 节点参数。
///
/// 运行期只认 `agent_type` / `prompt_template`（API 模式另需 `api_provider`/`api_model`，
/// 非 resume 场景两者必填），解析不出可用配置时返回 None，由调用方继续按回退链尝试。
fn agent_node_params(agent_config: &str, prompt_template: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(agent_config).ok()?;
    let mut params = serde_json::Map::new();
    params.insert(
        "prompt_template".to_string(),
        serde_json::Value::String(prompt_template.to_string()),
    );
    let provider = v["provider"].as_str().unwrap_or_default();
    let model = v["model"].as_str().unwrap_or_default();
    if !provider.is_empty() && !model.is_empty() {
        params.insert(
            "agent_type".to_string(),
            serde_json::Value::String("api".to_string()),
        );
        params.insert(
            "api_provider".to_string(),
            serde_json::Value::String(provider.to_string()),
        );
        params.insert(
            "api_model".to_string(),
            serde_json::Value::String(model.to_string()),
        );
    } else if let Some(agent_type) = v["agent_type"].as_str().filter(|s| !s.is_empty()) {
        params.insert(
            "agent_type".to_string(),
            serde_json::Value::String(agent_type.to_string()),
        );
    } else {
        return None;
    }
    Some(serde_json::Value::Object(params))
}

/// 任务节点提示词：总目标 + 本任务 + 前置产出占位符。
/// 占位符 `{{t<n>}}` 必须与 `build_task_mapping` 的键一一对应（引擎只按 inputMapping 的键取值）。
fn build_task_prompt(task: &TaskRow, tasks: &[TaskRow], deps: &[usize]) -> String {
    let mut prompt = format!("总目标：{{{{goal}}}}\n\n你的子任务：{}\n", task.description);
    let upstream: Vec<&TaskRow> = deps.iter().filter_map(|&d| tasks.get(d)).collect();
    if !upstream.is_empty() {
        prompt.push_str("\n【前置任务产出】\n");
        for up in upstream {
            prompt.push_str(&format!(
                "- T{}（{}）：\n{{{{t{}}}}}\n",
                up.task_no,
                truncate_chars(&up.description, 40),
                up.task_no
            ));
        }
    }
    prompt
}

/// 规范引用表达式 `{{<字段名>.<节点ID>.<阶段ID>}}`（与编辑器节点面板生成的形态一致）。
fn reference(field: &str, node_id: &str, stage_id: &str) -> String {
    format!("{{{{{}.{}.{}}}}}", field, node_id, stage_id)
}

/// 任务节点 inputMapping：总目标 + 各前置任务的产出引用。
///
/// 引用写成编辑器规范短写 `{{<上游暴露字段>.<上游节点ID>.<阶段ID>}}`，模板引擎会把它展开成
/// `{{<上游节点ID>.<上游暴露字段>}}` 再取值。首段必须是**上游节点 outputMapping 的字段名**
/// （本生成器的 agent 节点暴露 `result`），不能写本地变量名——写成 `{{t3.<节点ID>.<阶段ID>}}`
/// 会展开成 `{{<节点ID>.t3}}`，而该节点只暴露 result，取值落空（且编辑器节点面板的下拉里
/// 只有 `result.*` 形态，认不出这条引用，显示为"引用不存在"）。
fn build_task_mapping(
    deps: &[usize],
    tasks: &[TaskRow],
    node_ids: &[String],
    start_id: &str,
    stage_id: &str,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "goal".to_string(),
        serde_json::Value::String(reference("goal", start_id, stage_id)),
    );
    for &d in deps {
        if let (Some(t), Some(nid)) = (tasks.get(d), node_ids.get(d)) {
            map.insert(
                format!("t{}", t.task_no),
                serde_json::Value::String(reference("result", nid, stage_id)),
            );
        }
    }
    serde_json::Value::Object(map)
}

/// 节点在阶段内容区（480×500）内的排布。
///
/// 编辑器里阶段内容区是 `overflow: hidden` 的固定画框（节点可视范围 x ∈ [5,315]、y ∈ [5,435]），
/// 超出就得靠"阶段内平移"才看得到。旧实现用固定 140px 间距，第 4 个任务就被顶出可视区。
/// 这里按可视范围自适应：
/// - ≤6 个节点 → 单列；更多 → 两列（内容区宽度只容得下两列 160px 节点，列距 200）；
/// - 垂直间距 80（节点高 60 + 间距 20），行数超过 6 时压到 60（节点等高）；
/// - 可视区内最多容纳 14 个节点（起始 + 12 任务 + 结束），更多时尾部节点需阶段内平移查看。
fn node_position(index: usize, total: usize) -> (i64, i64) {
    let columns: usize = if total <= 6 { 1 } else { 2 };
    let rows = total.div_ceil(columns);
    let pitch: i64 = if rows <= 6 { 80 } else { 60 };
    let col = (index % columns) as i64;
    let row = (index / columns) as i64;
    let x = if columns == 1 { 20 } else { 20 + col * 200 };
    (x, 20 + row * pitch)
}

/// 按字符（而非字节）截断，避免中文被截半。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}

fn new_edge(source: &str, target: &str) -> WorkflowEdge {
    WorkflowEdge {
        id: uuid::Uuid::new_v4().to_string(),
        source: source.to_string(),
        target: target.to_string(),
        label: None,
        condition: None,
    }
}

/// 群聊任务 DAG → 可直接运行的工作流定义。
///
/// 组装依据（每条都对应运行期真实语义，错一条就变成"能跑但拿不到数据"）：
/// - 每个节点都要 outputMapping：引擎按"入边源节点是否在上下文里有产出"判定边是否连通，
///   没有映射的节点会让整条下游链被跳过；
/// - 起始节点用 outputMapping 承载总目标（引擎会把起始节点的映射字段平铺进上下文），
///   于是任意节点都能以 `{{goal}}` 取到总目标；
/// - 依赖即任务 DAG：无前置 → 起始节点，有前置 → 各前置任务节点，无后继 → 结束节点；
/// - 前置产出靠 inputMapping + 提示词占位符注入（引擎不自动拼接上游产出）；
/// - 作废（aborted）任务不转进工作流，其依赖由后继任务继承（见 `task::effective_deps`）。
fn build_groupchat_workflow(
    conn: &rusqlite::Connection,
    room: &Room,
) -> Result<WorkflowDefinition, AppError> {
    let tasks = store::list_tasks(conn, &room.id)?;
    let kept: Vec<bool> = tasks.iter().map(|t| t.status != "aborted").collect();
    if !kept.iter().any(|k| *k) {
        return Err(AppError::InvalidInput(
            "房间没有可转换的子任务（任务都已作废，或尚未完成编排）".to_string(),
        ));
    }
    let participants = store::list_participants(conn, &room.id)?;

    // 依赖收缩掉作废任务，保证下标不指向被剔除的节点
    let deps = crate::groupchat::task::effective_deps(&tasks);
    let aborted_mask: Vec<bool> = tasks.iter().map(|t| t.status == "aborted").collect();
    let cyclic = crate::groupchat::task::cycle_indices(&deps, &aborted_mask);
    if !cyclic.is_empty() {
        let nos = cyclic
            .iter()
            .filter_map(|&i| tasks.get(i).map(|t| format!("T{}", t.task_no)))
            .collect::<Vec<_>>()
            .join("、");
        return Err(AppError::InvalidInput(format!(
            "任务依赖存在环（涉及 {}），请调整依赖后重试",
            nos
        )));
    }

    // 结论统一取自消息表 `kind='conclusion'`（房间表 summary 字段已移除）。
    let conclusion = store::list_messages(conn, &room.id, None, None)?
        .into_iter()
        .rev()
        .find(|m| m.kind == "conclusion")
        .map(|m| m.content)
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| {
            format!(
                "群聊「{}」讨论产出，共 {} 项任务。",
                room.title,
                tasks.len()
            )
        });

    // 先给每个任务分配节点 id：连边与 inputMapping 都要引用前置节点 id
    let node_ids: Vec<String> = tasks
        .iter()
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect();
    let start_id = uuid::Uuid::new_v4().to_string();
    let end_id = uuid::Uuid::new_v4().to_string();
    // 阶段 id 必须提前生成：inputMapping 的规范引用表达式里要带阶段 id
    let stage_id = uuid::Uuid::new_v4().to_string();
    // 排布按"起始 + 保留任务 + 结束"的完整序列算，保证所有节点都在内容区可视范围内
    let total_nodes = kept.iter().filter(|k| **k).count() + 2;
    let positions: Vec<(i64, i64)> = (0..total_nodes)
        .map(|i| node_position(i, total_nodes))
        .collect();
    let mut nodes: Vec<WorkflowNode> = Vec::new();
    let mut edges: Vec<WorkflowEdge> = Vec::new();

    nodes.push(WorkflowNode {
        id: start_id.clone(),
        node_type: WorkflowNodeType::Start,
        label: "开始".to_string(),
        plugin_id: None,
        command_id: None,
        params: None,
        delay_ms: None,
        timeout_ms: None,
        input_schema: None,
        output_schema: None,
        input_mapping: None,
        output_mapping: Some(serde_json::json!({ "goal": room.topic })),
        position: Some(serde_json::json!({ "x": positions[0].0, "y": positions[0].1 })),
    });

    // 负责人回退链：任务负责人 → 主持人 → 首个 API 参与者 → 首个 CLI 参与者
    let mut fallback: Vec<&str> = Vec::new();
    for kind in ["director", "api", "cli"] {
        if let Some(p) = participants.iter().find(|p| p.participant_type == kind) {
            fallback.push(p.agent_config.as_str());
        }
    }

    let mut kept_indices: Vec<usize> = Vec::new();
    for (i, t) in tasks.iter().enumerate() {
        if !kept[i] {
            continue;
        }
        let pos = positions[kept_indices.len() + 1];
        let prompt = build_task_prompt(t, &tasks, &deps[i]);
        let assignee_config = t
            .assignee
            .as_deref()
            .and_then(|id| participants.iter().find(|p| p.id == id))
            .map(|p| p.agent_config.as_str());
        let params = assignee_config
            .into_iter()
            .chain(fallback.iter().copied())
            .find_map(|cfg| agent_node_params(cfg, &prompt))
            .ok_or_else(|| {
                AppError::Config(format!(
                    "任务 T{} 找不到可用的 Agent 配置：房间内没有可用的 API/CLI 参与者",
                    t.task_no
                ))
            })?;
        nodes.push(WorkflowNode {
            id: node_ids[i].clone(),
            node_type: WorkflowNodeType::Agent,
            label: format!("T{} {}", t.task_no, truncate_chars(&t.description, 24)),
            plugin_id: None,
            command_id: None,
            params: Some(params),
            delay_ms: None,
            timeout_ms: None,
            input_schema: None,
            output_schema: None,
            input_mapping: Some(build_task_mapping(
                &deps[i], &tasks, &node_ids, &start_id, &stage_id,
            )),
            // 暴露产出：既是下游 {{...result}} 的数据源，也让本节点的出边被判定为连通
            output_mapping: Some(serde_json::json!({ "result": "{{content}}" })),
            position: Some(serde_json::json!({ "x": pos.0, "y": pos.1 })),
        });
        kept_indices.push(i);
    }

    // 连边：无前置 → 起始节点；有前置 → 各前置任务节点；无后继 → 结束节点
    let kept_set: std::collections::HashSet<usize> = kept_indices.iter().copied().collect();
    let mut has_dependent = vec![false; tasks.len()];
    for &i in &kept_indices {
        let upstream: Vec<usize> = deps[i]
            .iter()
            .copied()
            .filter(|d| kept_set.contains(d))
            .collect();
        if upstream.is_empty() {
            edges.push(new_edge(&start_id, &node_ids[i]));
        } else {
            for d in upstream {
                has_dependent[d] = true;
                edges.push(new_edge(&node_ids[d], &node_ids[i]));
            }
        }
    }
    for &i in &kept_indices {
        if !has_dependent[i] {
            edges.push(new_edge(&node_ids[i], &end_id));
        }
    }

    nodes.push(WorkflowNode {
        id: end_id.clone(),
        node_type: WorkflowNodeType::End,
        label: "结束".to_string(),
        plugin_id: None,
        command_id: None,
        params: None,
        delay_ms: None,
        timeout_ms: None,
        input_schema: None,
        output_schema: None,
        input_mapping: None,
        output_mapping: None,
        position: Some(serde_json::json!({
            "x": positions[total_nodes - 1].0,
            "y": positions[total_nodes - 1].1,
        })),
    });

    let now = crate::utils::now();
    Ok(WorkflowDefinition {
        id: uuid::Uuid::new_v4().to_string(),
        name: room.title.clone(),
        version: "1.0.0".to_string(),
        description: format!(
            "{}\n\n— 由群聊「{}」转换：{} 个子任务，任务依赖与前置产出引用已保留。",
            conclusion,
            room.title,
            kept_indices.len()
        ),
        trigger: TriggerConfig {
            trigger_type: TriggerType::Manual,
            cron: None,
            event_name: None,
        },
        stages: vec![Stage {
            id: stage_id,
            name: "群聊任务".to_string(),
            order: 0,
            nodes,
            edges,
            stage_edges: Vec::new(),
            gate: GateConfig::default(),
            collapsed: false,
            offset_x: 0.0,
            offset_y: 0.0,
        }],
        icon: None,
        input_schema: None,
        output_schema: None,
        created_at: now,
        updated_at: now,
        enabled: true,
    })
}

/// 转换后的工作流定义 JSON（与落库所用定义逐字一致，供前端预览/另存）。
#[tauri::command]
pub fn groupchat_export_workflow(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<serde_json::Value, String> {
    let conn = state.get_conn()?;
    let room = store::get_room(&conn, &room_id)?
        .ok_or_else(|| AppError::NotFound("房间不存在".to_string()))?;
    let def = build_groupchat_workflow(&conn, &room)?;
    Ok(serde_json::to_value(&def).map_err(|e| AppError::Json(e.to_string()))?)
}

/// 精准转换：把群聊任务议程落库为**可直接运行**的工作流定义（工作流列表可见、无需再编辑）。
#[tauri::command]
pub fn groupchat_promote_workflow(
    state: State<'_, DbState>,
    room_id: String,
) -> Result<WorkflowDefinition, String> {
    let conn = state.get_conn()?;
    let room = store::get_room(&conn, &room_id)?
        .ok_or_else(|| AppError::NotFound("房间不存在".to_string()))?;
    let def = build_groupchat_workflow(&conn, &room)?;
    crate::workflow::create_definition(&conn, &def)?;
    Ok(def)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::init::FINAL_SCHEMA_SQL)
            .unwrap();
        conn
    }

    fn seed_room(conn: &rusqlite::Connection) -> Room {
        let now = crate::utils::now();
        let room = Room {
            id: "room-1".to_string(),
            title: "测试房间".to_string(),
            topic: "把产品做出来".to_string(),
            status: "finished".to_string(),
            strategy: "round_robin".to_string(),
            max_rounds: 5,
            max_parallel: 2,
            director_id: Some("director".to_string()),
            current_task_id: None,
            created_at: now,
            updated_at: now,
            goal_notes: "[]".to_string(),
            allow_auto_cli: 1,
            output_dir: String::new(),
            // 派生字段：只有 list_rooms 会填
            current_round: None,
            task_total: None,
            task_finished: None,
        };
        store::insert_room(conn, &room).unwrap();
        for (id, kind, config) in [
            ("director", "director", r#"{"provider":"p1","model":"m1"}"#),
            ("api-a", "api", r#"{"provider":"p2","model":"m2"}"#),
            ("cli-b", "cli", r#"{"agent_type":"claude"}"#),
        ] {
            store::insert_participant(
                conn,
                &ParticipantRow {
                    id: id.to_string(),
                    room_id: room.id.clone(),
                    participant_type: kind.to_string(),
                    agent_config: config.to_string(),
                    display_name: id.to_string(),
                    system_role: "参与者".to_string(),
                    status: "active".to_string(),
                },
            )
            .unwrap();
        }
        room
    }

    fn add_task(
        conn: &rusqlite::Connection,
        id: &str,
        no: i64,
        desc: &str,
        deps: &[usize],
        status: &str,
        assignee: Option<&str>,
    ) {
        store::insert_task(
            conn,
            &TaskRow {
                id: id.to_string(),
                room_id: "room-1".to_string(),
                task_no: no,
                description: desc.to_string(),
                assignee: assignee.map(str::to_string),
                depends_on: serde_json::to_string(deps).unwrap(),
                status: status.to_string(),
                result_summary: None,
                error: None,
                started_at: None,
                completed_at: None,
            },
        )
        .unwrap();
    }

    /// 转换后的定义必须：参数用运行期真名、依赖按 DAG 还原（不是线性链）、
    /// 前置产出有取值引用、作废任务被剔除且依赖被后继继承。
    #[test]
    fn workflow_conversion_preserves_dag_and_params() {
        let conn = test_conn();
        let room = seed_room(&conn);
        // T1、T2 并行无前置；T3 依赖 T1+T2；T4 依赖 T3 但已作废；T5 依赖 T4 → 应继承 T3
        add_task(&conn, "t1", 1, "调研市场", &[], "success", Some("api-a"));
        add_task(&conn, "t2", 2, "整理需求", &[], "success", Some("cli-b"));
        add_task(&conn, "t3", 3, "写方案", &[0, 1], "pending", Some("api-a"));
        add_task(&conn, "t4", 4, "废弃的旧步骤", &[2], "aborted", None);
        add_task(&conn, "t5", 5, "评审方案", &[3], "pending", None);

        let def = build_groupchat_workflow(&conn, &room).unwrap();
        assert!(def.enabled);
        assert_eq!(def.stages.len(), 1);
        let stage = &def.stages[0];

        let by_label: HashMap<&str, &WorkflowNode> =
            stage.nodes.iter().map(|n| (n.label.as_str(), n)).collect();
        let label_of = |id: &str| -> String {
            stage
                .nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.label.clone())
                .unwrap_or_default()
        };
        let node_id_of = |prefix: &str| -> String {
            stage
                .nodes
                .iter()
                .find(|n| n.label.starts_with(prefix))
                .map(|n| n.id.clone())
                .unwrap_or_default()
        };

        // 作废的 T4 不转进工作流：start + T1/T2/T3/T5 + end
        assert_eq!(stage.nodes.len(), 6);
        assert!(
            by_label.keys().all(|l| !l.starts_with("T4")),
            "作废任务不应成为节点"
        );

        // 每个 agent 节点的参数都是运行期真名，且产出可被下游引用
        for n in stage
            .nodes
            .iter()
            .filter(|n| n.node_type == WorkflowNodeType::Agent)
        {
            let params = n.params.as_ref().expect("agent 节点缺 params");
            assert!(!params["agent_type"].as_str().unwrap_or_default().is_empty());
            assert!(!params["prompt_template"]
                .as_str()
                .unwrap_or_default()
                .is_empty());
            if params["agent_type"] == "api" {
                assert!(!params["api_provider"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty());
                assert!(!params["api_model"].as_str().unwrap_or_default().is_empty());
            }
            assert_eq!(n.output_mapping.as_ref().unwrap()["result"], "{{content}}");
        }
        // 起始节点承载总目标，任意节点可用 {{goal}}
        let start = stage
            .nodes
            .iter()
            .find(|n| n.node_type == WorkflowNodeType::Start)
            .unwrap();
        assert_eq!(
            start.output_mapping.as_ref().unwrap()["goal"],
            "把产品做出来"
        );

        // 依赖按 DAG 还原：T1/T2 直接挂 start，T3 由两条入边（T1、T2）驱动
        let pair = |id: &str| label_of(id);
        let edges: HashSet<(String, String)> = stage
            .edges
            .iter()
            .map(|e| (pair(&e.source), pair(&e.target)))
            .collect();
        assert!(edges.contains(&("开始".to_string(), format!("T1 调研市场"))));
        assert!(edges.contains(&("开始".to_string(), "T2 整理需求".to_string())));
        assert!(edges.contains(&("T1 调研市场".to_string(), "T3 写方案".to_string())));
        assert!(edges.contains(&("T2 整理需求".to_string(), "T3 写方案".to_string())));
        // 线性链是旧草稿的形态：T2 不该依赖 T1
        assert!(!edges.contains(&("T1 调研市场".to_string(), "T2 整理需求".to_string())));

        // T5 的前置 T4 已作废 → 依赖被收缩为 T3（提示词与 inputMapping 都指向 T3）
        let t5 = by_label.get("T5 评审方案").expect("缺少 T5 节点");
        let prompt5 = t5.params.as_ref().unwrap()["prompt_template"]
            .as_str()
            .unwrap();
        assert!(
            prompt5.contains("{{t3}}"),
            "T5 提示词应含继承来的前置产出占位符: {prompt5}"
        );
        assert!(!prompt5.contains("{{t4}}"));
        let mapping5 = t5.input_mapping.as_ref().unwrap();
        // 引用必须是编辑器规范短写 {{上游暴露字段.节点ID.阶段ID}}（模板引擎展开为 {{节点ID.字段}}）
        assert_eq!(
            mapping5["t3"],
            serde_json::json!(format!("{{{{result.{}.{}}}}}", node_id_of("T3"), stage.id))
        );
        assert!(mapping5.get("t4").is_none());
        assert_eq!(
            mapping5["goal"],
            serde_json::json!(format!("{{{{goal.{}.{}}}}}", start.id, stage.id))
        );

        // 引用有效性总检：三段式引用的字段必须是该节点 outputMapping 里真实暴露的键。
        // （这条正是为了拦住"首段误写成局部变量名 t3、上游只暴露 result"那类错误）
        let by_id: HashMap<&str, &WorkflowNode> =
            stage.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        for n in &stage.nodes {
            let Some(map) = n.input_mapping.as_ref().and_then(|m| m.as_object()) else {
                continue;
            };
            for (key, val) in map {
                let Some(expr) = val.as_str() else { continue };
                for part in expr
                    .split("{{")
                    .skip(1)
                    .filter_map(|s| s.split("}}").next())
                {
                    let segs: Vec<&str> = part.split('.').collect();
                    if segs.len() != 3 {
                        continue;
                    }
                    let (field, nid, sid) = (segs[0], segs[1], segs[2]);
                    assert!(
                        by_id.contains_key(nid),
                        "节点 {} 引用了不存在的节点 {}",
                        key,
                        nid
                    );
                    assert_eq!(sid, stage.id, "节点 {} 引用了错误的阶段 {}", key, sid);
                    let exposed = by_id[nid]
                        .output_mapping
                        .as_ref()
                        .unwrap()
                        .as_object()
                        .unwrap();
                    assert!(
                        exposed.contains_key(field),
                        "节点 {} 引用的字段 {} 不在上游暴露字段 {:?} 里",
                        key,
                        field,
                        exposed.keys().collect::<Vec<_>>()
                    );
                }
            }
        }

        // 节点排布：全部落在内容区可视范围（x ≤ 315 / y ≤ 435）且对齐 20px 网格、互不重叠
        let mut seen_pos: HashSet<(i64, i64)> = HashSet::new();
        for n in &stage.nodes {
            let pos = n.position.as_ref().expect("节点缺 position");
            let (x, y) = (pos["x"].as_i64().unwrap(), pos["y"].as_i64().unwrap());
            assert!(
                (5..=315).contains(&x) && x % 20 == 0,
                "x 越界或未对齐网格: {x}"
            );
            assert!(
                (5..=435).contains(&y) && y % 20 == 0,
                "y 越界或未对齐网格: {y}"
            );
            assert!(seen_pos.insert((x, y)), "节点位置重复: ({x}, {y})");
        }

        // 唯一汇点 T5 → end；所有节点从 start 可达（引擎只执行可达节点）
        let into_end: Vec<&WorkflowEdge> = stage
            .edges
            .iter()
            .filter(|e| e.target == node_id_of("结束"))
            .collect();
        assert_eq!(into_end.len(), 1);
        assert_eq!(into_end[0].source, node_id_of("T5"));

        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = vec![start.id.clone()];
        while let Some(id) = queue.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            for e in stage.edges.iter().filter(|e| e.source == id) {
                queue.push(e.target.clone());
            }
        }
        assert_eq!(seen.len(), stage.nodes.len(), "存在从起始节点不可达的节点");
    }

    /// 任务全部作废（或尚未编排）时明确报错，而不是产出一个空壳工作流。
    #[test]
    fn workflow_conversion_rejects_empty_agenda() {
        let conn = test_conn();
        let room = seed_room(&conn);
        assert!(build_groupchat_workflow(&conn, &room).is_err());

        add_task(&conn, "t1", 1, "已作废", &[], "aborted", None);
        assert!(build_groupchat_workflow(&conn, &room).is_err());
    }

    /// 排布：全部落在内容区可视范围、对齐 20px 网格、互不重叠（≤14 个节点）。
    #[test]
    fn node_positions_stay_inside_content_area() {
        for total in 2..=14usize {
            let mut seen: HashSet<(i64, i64)> = HashSet::new();
            for i in 0..total {
                let (x, y) = node_position(i, total);
                assert!((5..=315).contains(&x) && x % 20 == 0, "total={total} x={x}");
                assert!((5..=435).contains(&y) && y % 20 == 0, "total={total} y={y}");
                assert!(seen.insert((x, y)), "total={total} 位置重复 ({x}, {y})");
            }
        }
    }
}
