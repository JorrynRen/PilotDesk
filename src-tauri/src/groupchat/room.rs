//! 房间运行时（RoomRuntime Actor）+ 房间注册表（RoomRegistry）。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::{mpsc, Mutex as AsyncMutex, Notify};

use crate::agent::AgentManager;
use crate::api_agent::agent_loop::{ApprovalHandler, ContinueHandler, RiskLevel};
use crate::api_agent::types::ApiFormat;
use crate::db::init::DbPool;
use crate::db::models::Attachment;
use crate::utils::errors::AppError;
use tauri::Emitter;

use super::adapter::{cli::PilotDeskCliRunner, llm::PilotDeskLlmClient};
use super::director::{Director, ReplanOperation, SpeakerDecision};
use super::event::GroupChatEvent;
use super::floor::FloorManager;
use super::memory::LayeredMemory;
use super::models::{ConfirmationItem, ConfirmationRequest, MessageRow, Room, TaskRow};
use super::participant::{ChatMessage, CliConfig, Participant, TurnResult, TurnView, UserParticipant};
use super::rules::RuleEngine;
use super::state::{RoomStateMachine, RoomStatus};
use super::{store, report};

/// 群聊消息正文落库前的归一化：把连续空行折叠为单个空行，并去除首尾空白。
/// 保留单个空行以维持 Markdown 的段落/代码块结构，仅消除冗余的连续空行。
/// 独立于主应用 utils，便于群聊模块整体开源。
fn normalize_message_content(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut pending_blank = false;
    let mut started = false;

    for line in content.lines() {
        if line.trim().is_empty() {
            if started {
                pending_blank = true;
            }
        } else {
            if pending_blank {
                out.push('\n');
                pending_blank = false;
            }
            out.push_str(line);
            out.push('\n');
            started = true;
        }
    }

    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// 工具调用链 JSON（ThinkingChainStep[]）落库前的归一化：对其中的文本字段
/// （content/result/fileDiff）逐项折叠空行。工具输出、文件 diff、推理文本可能含
/// 连续空行，若不处理，前端 whitespace-pre-wrap 直出时会原样显示多空行。
/// 用 serde_json::Value 泛化处理，避免依赖具体步骤类型，保持群聊模块可整体开源。
fn normalize_tool_calls(tool_calls: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(tool_calls) {
        if let Some(items) = value.as_array_mut() {
            for item in items.iter_mut() {
                if let Some(obj) = item.as_object_mut() {
                    for key in ["content", "result", "fileDiff"] {
                        if let Some(serde_json::Value::String(s)) = obj.get_mut(key) {
                            let normalized = normalize_message_content(s);
                            *s = normalized;
                        }
                    }
                }
            }
        }
        if let Ok(serialized) = serde_json::to_string(&value) {
            return serialized;
        }
    }
    tool_calls.to_string()
}

/// 用户对单个确认项的回复（结构化确认时前端逐项提交）。
#[derive(Debug, Clone)]
pub struct ConfirmationResponse {
    pub item_id: String,
    pub value: String,
}

/// 用户对确认请求的回复分类（确认等待期消费）。
enum ConfirmationReply {
    /// 结构化确认回复（必然是对确认项的答复）。
    Structured(String),
    /// 开放式用户消息（语义需 Director 判断：confirm 或 directive）。
    Open {
        sender: String,
        content: String,
        recipients: Vec<String>,
        reply_to: Option<String>,
        mention: Option<String>,
        attachments: Vec<Attachment>,
    },
}

/// 发往房间 Actor 的命令。
#[derive(Debug)]
pub enum RoomCommand {
    Send {
        sender: String,
        content: String,
        recipients: Vec<String>,
        reply_to: Option<String>,
        mention: Option<String>,
        attachments: Vec<Attachment>,
    },
    /// 用户对结构化确认请求的回复（开放式回复走 Send 命令）。
    RespondConfirmation {
        #[allow(dead_code)]
        request_id: String,
        responses: Vec<ConfirmationResponse>,
    },
    SetDirector {
        director_id: String,
    },
    /// 参与者名册发生变更（加入/移除），需要重建运行时并重新编排角色与任务。
    ParticipantChanged,
    Pause,
    Resume,
    Abort,
}

/// 用户指令对任务编排的处置分类（决定走首轮全量编排还是追加轮对账）。
enum GoalDisposition {
    /// 首轮：尚无任何任务，content 即初始目标。
    FirstRound,
    /// 目标已变更，携带新目标文本（旧任务已归档，需全量重新编排）。
    Changed(String),
    /// refine：目标未变，可累积补充约束，追加轮走对账重排。
    Refined,
}

/// 一次任务编排的结果（供主循环的编排收敛谓词使用）。
struct KickoffResult {
    /// 本次实际新增的任务数（对账重排时 = add 操作数）。
    added: usize,
}

/// 应用 Director 重排操作后的变更统计。
struct ReplanApplication {
    removed: usize,
    reassigned: usize,
    added: Vec<TaskRow>,
}

/// 房间运行时与命令层共享的控制标志。
#[derive(Clone)]
struct Control {
    abort: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    resume: Arc<Notify>,
}

impl Control {
    fn new() -> Self {
        Self {
            abort: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            resume: Arc::new(Notify::new()),
        }
    }

    async fn wait_if_paused(&self) {
        while self.paused.load(Ordering::SeqCst) {
            self.resume.notified().await;
        }
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.resume.notify_waiters();
    }
}

/// 房间句柄：供命令层发命令 / 直接置控制标志。
#[derive(Clone)]
pub struct RoomHandle {
    cmd_tx: mpsc::UnboundedSender<RoomCommand>,
    control: Control,
}

impl RoomHandle {
    pub fn send(&self, cmd: RoomCommand) {
        let _ = self.cmd_tx.send(cmd);
    }
    pub fn abort(&self) {
        self.control.abort.store(true, Ordering::SeqCst);
    }
    pub fn pause(&self) {
        self.control.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.control.paused.store(false, Ordering::SeqCst);
        self.control.resume.notify_waiters();
    }
}

/// 房间注册表（托管在 app state 中）。
#[derive(Default)]
pub struct RoomRegistry {
    rooms: std::sync::Mutex<HashMap<String, RoomHandle>>,
}

impl RoomRegistry {
    pub fn new() -> Self {
        Self { rooms: std::sync::Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, room_id: &str) -> Option<RoomHandle> {
        self.rooms.lock().unwrap().get(room_id).cloned()
    }

    /// 启动一个房间 Actor，返回其句柄。
    pub fn spawn(
        &self,
        pool: DbPool,
        app: tauri::AppHandle,
        room_id: &str,
    ) -> Result<RoomHandle, AppError> {
        let conn = pool.get()?;
        let room = store::get_room(&conn, room_id)?
            .ok_or_else(|| AppError::NotFound(format!("房间不存在: {}", room_id)))?;

        let approval_log: Arc<Mutex<Vec<ApprovalRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let built = build_participants(&conn, &pool, &room, app.clone(), approval_log.clone())?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let control = Control::new();
        let handle = RoomHandle { cmd_tx, control: control.clone() };

        // 应用重启后懒恢复：读取 DB 真实状态与轮次，供 run() 判断是否续跑未完成任务。
        let room_status = RoomStatus::from_str(&room.status);
        let round = store::max_message_round(&conn, room_id);
        let topic = room.topic.clone();

        let runtime = RoomRuntime {
            room_id: room_id.to_string(),
            app,
            pool,
            cmd_rx,
            control,
            room,
            participants: built.participants,
            director: built.director,
            director_id: built.director_id,
            user_id: built.user_id,
            floor: FloorManager::new(built.floor_order.clone()),
            floor_order: built.floor_order,
            executor_order: built.executor_order,
            memory: LayeredMemory::new(&topic, ""),
            state: {
                let mut s = RoomStateMachine::new(room_status);
                s.round = round;
                s
            },
            rules: RuleEngine::new(),
            seq: store::max_message_seq(&conn, room_id),
            approval_log,
            pending_mention: None,
            last_kickoff_added: 0,
        };

        // 使用 tauri::async_runtime::spawn 而非 tokio::spawn：
        // spawn 可能由同步命令（groupchat_create_room / groupchat_send_message）触发，
        // 此时当前线程不在 Tokio runtime 上下文中，tokio::spawn 会 panic（no reactor running）。
        tauri::async_runtime::spawn(async move { runtime.run().await });

        self.rooms
            .lock()
            .unwrap()
            .insert(room_id.to_string(), handle.clone());

        Ok(handle)
    }

    /// 若房间 Actor 已存在则复用，否则从 DB 重建并启动（用于应用重启后懒恢复）。
    pub fn get_or_spawn(
        &self,
        pool: DbPool,
        app: tauri::AppHandle,
        room_id: &str,
    ) -> Result<RoomHandle, AppError> {
        if let Some(handle) = self.get(room_id) {
            return Ok(handle);
        }
        self.spawn(pool, app, room_id)
    }

    pub fn remove(&self, room_id: &str) {
        self.rooms.lock().unwrap().remove(room_id);
    }
}

/// 从 DB 构建参与者与 Director。
struct BuiltParticipants {
    participants: HashMap<String, Arc<dyn Participant>>,
    director: Option<Arc<Director>>,
    director_id: Option<String>,
    floor_order: Vec<String>,
    executor_order: Vec<String>,
    user_id: String,
}

fn build_participants(
    conn: &rusqlite::Connection,
    pool: &DbPool,
    room: &Room,
    app: tauri::AppHandle,
    approval_log: Arc<Mutex<Vec<ApprovalRecord>>>,
) -> Result<BuiltParticipants, AppError> {
    let rows = store::list_participants(conn, &room.id)?;
    let cwd = crate::utils::paths::resolve_workspace_path(None, "", conn)
        .to_string_lossy()
        .to_string();

    let mut participants: HashMap<String, Arc<dyn Participant>> = HashMap::new();
    let mut director: Option<Arc<Director>> = None;
    let mut floor_order: Vec<String> = Vec::new();
    let mut executor_order: Vec<String> = Vec::new();
    let mut user_id = String::from("user");
    let director_id = room.director_id.clone();

    // 先构造 Director：API 参与者需要其 Arc 作为工具审批裁决来源。
    for row in &rows {
        if row.participant_type == "director" {
            if let Some(llm) = build_llm_client(conn, &row.agent_config, &app, &cwd, false, None) {
                director = Some(Arc::new(Director::new(Arc::new(llm))));
            }
        }
    }

    for row in &rows {
        match row.participant_type.as_str() {
            "director" => {}
            "api" => {
                let event_session_id = format!("groupchat:{}:{}", room.id, row.id);
                if let Some(llm) = build_llm_client(conn, &row.agent_config, &app, &cwd, true, Some(event_session_id)) {
                    let llm = match &director {
                        Some(d) => llm
                            .with_approval_handler(make_director_approval(d.clone(), approval_log.clone(), room.topic.clone()))
                            .with_continue_handler(make_director_continue(d.clone(), room.topic.clone())),
                        None => llm,
                    };
                    let p = super::participant::LlmParticipant {
                        id: row.id.clone(),
                        llm: Arc::new(llm),
                        role_prompt: Arc::new(RwLock::new(row.system_role.clone())),
                    };
                    participants.insert(row.id.clone(), Arc::new(p));
                    floor_order.push(row.id.clone());
                    executor_order.push(row.id.clone());
                }
            }
            "cli" => {
                let agent_type = agent_type_from_config(&row.agent_config);
                let runner = Arc::new(PilotDeskCliRunner::new(
                    Arc::new(AsyncMutex::new(AgentManager::new())),
                    pool.clone(),
                    cwd.clone(),
                ));
                let p = super::participant::CliParticipant {
                    id: row.id.clone(),
                    runner,
                    config: CliConfig {
                        command: agent_type,
                        args_template: String::new(),
                        resume_arg_template: String::new(),
                    },
                };
                participants.insert(row.id.clone(), Arc::new(p));
                floor_order.push(row.id.clone());
                executor_order.push(row.id.clone());
            }
            "user" => {
                user_id = row.id.clone();
                let p = UserParticipant { id: row.id.clone() };
                participants.insert(row.id.clone(), Arc::new(p));
            }
            _ => {}
        }
    }

    Ok(BuiltParticipants {
        participants,
        director,
        director_id,
        floor_order,
        executor_order,
        user_id,
    })
}

fn build_llm_client(
    conn: &rusqlite::Connection,
    agent_config_json: &str,
    app: &tauri::AppHandle,
    cwd: &str,
    with_tools: bool,
    event_session_id: Option<String>,
) -> Option<PilotDeskLlmClient> {
    let v: serde_json::Value = serde_json::from_str(agent_config_json).ok()?;
    let provider_id = v["provider"].as_str()?;
    let model = v["model"].as_str().unwrap_or_default().to_string();
    let provider = crate::commands::api_provider::get_api_provider(conn, provider_id).ok()??;
    let api_key = crate::commands::api_provider::get_api_key(conn, provider_id).ok()??;
    let format = provider.api_format.parse::<ApiFormat>().unwrap_or_default();

    let mut client = PilotDeskLlmClient::new(provider.api_endpoint, api_key, model, format);
    if with_tools {
        let registry = super::adapter::tools::build_groupchat_tool_registry(conn, cwd);
        client = client.with_tools(registry, app.clone());
        if let Some(sid) = event_session_id {
            client = client.with_event_session_id(sid);
        }
    }
    Some(client)
}

/// Director 工具授权裁决记录，用于在参与者 turn 结束后统一落库（避免同步回调内直接写库的 seq/round 竞态）。
struct ApprovalRecord {
    tool_name: String,
    risk: String,
    allow: bool,
    reason: String,
}

/// 待落库消息（并发任务执行阶段暂存，回到 Actor 主循环后按顺序写库）。
struct PendingMessage {
    sender: String,
    kind: String,
    content: String,
}

/// 单个任务并发执行后的结果。
struct TaskOutcome {
    task: TaskRow,
    /// 执行过程中的 Director 裁决消息（如 failure_decision），按发生顺序排列。
    messages: Vec<PendingMessage>,
    /// Director 裁决要求终止整个执行流程。
    should_abort: bool,
    /// 最终执行尝试的工具调用链（JSON 数组字符串），随 task_result 消息落库。
    tool_calls: String,
    /// Director 裁决 ask_user 或执行者主动发起时产生的确认请求。
    confirmation: Option<ConfirmationRequest>,
    /// 确认请求发起者 id（Director 或执行者），供确认请求消息落库时正确归属。
    confirmation_sender: Option<String>,
}

/// 把 Director 包装成同步的 `ApprovalHandler`，内部用 block_in_place + block_on
/// 调用异步的 `Director::authorize_tool`；裁决结果写入共享缓冲，由 RoomRuntime 落库。
/// `topic` 为房间讨论目标，注入裁决以提供「任务必要性」依据。
fn make_director_approval(
    director: Arc<Director>,
    approval_log: Arc<Mutex<Vec<ApprovalRecord>>>,
    topic: String,
) -> ApprovalHandler {
    Box::new(move |_call_id: &str, tool_name: &str, args: &str, risk: RiskLevel| {
        let director = director.clone();
        let approval_log = approval_log.clone();
        let tool_name = tool_name.to_string();
        let args = args.to_string();
        let risk = risk.description().to_string();
        let topic = topic.clone();
        tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(async move {
                let decision = director.authorize_tool(&topic, &tool_name, &args, &risk).await;
                let allow = decision.allow;
                let reason = decision.reason;
                if let Ok(mut log) = approval_log.lock() {
                    log.push(ApprovalRecord { tool_name, risk, allow, reason });
                }
                allow
            })
        })
    })
}

/// 把 Director 包装成同步的 `ContinueHandler`（停滞/上限时裁决是否继续），
/// 内部用 block_in_place + block_on 调用异步的 `Director::decide_stall_continue`。
/// 限制续命次数（最多 2 次），防止 Director 反复放行导致无限循环。
fn make_director_continue(director: Arc<Director>, topic: String) -> ContinueHandler {
    // Arc 包裹计数，Fn 闭包可多次 clone 使用（AtomicUsize 无 Copy，直接 move 会编译失败）。
    let max_continue = Arc::new(std::sync::atomic::AtomicUsize::new(2));
    Box::new(move |_current: usize, _max: usize| {
        if max_continue.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            return false;
        }
        let director = director.clone();
        let topic = topic.clone();
        let max_continue = max_continue.clone();
        tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(async move {
                let keep = director.decide_stall_continue(&topic, "").await.unwrap_or(false);
                if keep {
                    max_continue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                }
                keep
            })
        })
    })
}

fn agent_type_from_config(agent_config_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(agent_config_json)
        .ok()
        .and_then(|v| v["agent_type"].as_str().map(String::from))
        .unwrap_or_default()
}

/// 从工具调用链 JSON（`ThinkingChainStep[]`，与 `groupchat_messages.tool_calls` 格式一致）
/// 生成"已完成工作"摘要文本，用于失败续接提示与无文本成果兜底。
fn work_hint_from_tool_calls(tool_calls: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(tool_calls).ok()?;
    let arr = v.as_array()?;
    let mut lines: Vec<String> = Vec::new();
    for step in arr {
        if step["type"].as_str().unwrap_or("") != "tool_result" {
            continue;
        }
        let name = step["toolName"].as_str().unwrap_or("tool");
        let result = step["result"].as_str().unwrap_or("");
        let truncated: String = if result.chars().count() > 200 {
            result.chars().take(200).collect::<String>() + "…"
        } else {
            result.to_string()
        };
        lines.push(format!("- {}：{}", name, truncated));
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!("已完成以下工作（工具调用记录）：\n{}", lines.join("\n")))
    }
}

struct RoomRuntime {
    room_id: String,
    app: tauri::AppHandle,
    pool: DbPool,
    cmd_rx: mpsc::UnboundedReceiver<RoomCommand>,
    control: Control,
    room: Room,
    participants: HashMap<String, Arc<dyn Participant>>,
    director: Option<Arc<Director>>,
    director_id: Option<String>,
    user_id: String,
    floor: FloorManager,
    floor_order: Vec<String>,
    executor_order: Vec<String>,
    memory: LayeredMemory,
    state: RoomStateMachine,
    rules: RuleEngine,
    seq: i64,
    approval_log: Arc<Mutex<Vec<ApprovalRecord>>>,
    /// 用户最近一条指令 @ 指定的参与者 id（在下一轮发言者选择时消费后清除）。
    pending_mention: Option<String>,
    /// 最近一次任务编排实际新增的任务数（供编排收敛谓词使用）。
    last_kickoff_added: usize,
}

impl RoomRuntime {
    fn emit(&self, event: GroupChatEvent) {
        let _ = self.app.emit("groupchat-event", &event);
    }

    fn conn(&self) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, AppError> {
        Ok(self.pool.get()?)
    }

    async fn run(mut self) {
        log::info!("[GroupChat] 房间 Actor 启动: {}", self.room_id);
        self.emit(GroupChatEvent::started(&self.room_id));

        // 应用重启后懒恢复：房间仍标记 running（意外退出）且已有任务 → 自动续跑。
        if self.room.status == "running" && self.has_any_tasks() {
            self.resume_execution().await;
        }

        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                RoomCommand::Send { sender, content, recipients, reply_to, mention, attachments } => {
                    if self.control.abort.load(Ordering::SeqCst) {
                        continue;
                    }
                    self.control.wait_if_paused().await;
                    self.handle_send(&sender, &content, &recipients, reply_to.as_deref(), mention.as_deref(), &attachments).await;
                }
                RoomCommand::SetDirector { director_id } => {
                    self.set_director(&director_id).await;
                }
                RoomCommand::ParticipantChanged => {
                    self.handle_participant_changed().await;
                }
                RoomCommand::Pause => self.set_status(RoomStatus::Paused).await,
                RoomCommand::Resume => {
                    // 同一进程内解除暂停；跨进程（Actor 重启）且已有任务时，续跑未完成任务。
                    self.control.resume();
                    if self.has_any_tasks() {
                        self.resume_execution().await;
                    } else {
                        self.set_status(RoomStatus::Running).await;
                    }
                }
                RoomCommand::Abort => self.set_status(RoomStatus::Aborted).await,
                // 非等待状态下收到确认回复（历史确认块补交 / 房间已结束 / Actor 重启后未进入确认等待），
                // 仍落库 confirmation_response，保证前端刷新后能恢复「已提交」态。
                RoomCommand::RespondConfirmation { request_id, responses } => {
                    self.persist_confirmation_response_only(&request_id, &responses).await;
                }
            }
        }
    }

    /// 参与者增删后，从 DB 重建参与者/Director/发言顺序，使后续讨论生效。
    async fn rebuild_participants(&mut self) {
        let Ok(conn) = self.conn() else {
            return;
        };
        let Ok(built) = build_participants(&conn, &self.pool, &self.room, self.app.clone(), self.approval_log.clone()) else {
            return;
        };
        self.participants = built.participants;
        self.director = built.director;
        self.director_id = built.director_id;
        self.user_id = built.user_id;
        self.floor = FloorManager::new(built.floor_order.clone());
        self.floor_order = built.floor_order;
        self.executor_order = built.executor_order;
        self.memory.set_names(self.participant_names());
    }

    async fn set_status(&self, status: RoomStatus) {
        if let Ok(conn) = self.conn() {
            let _ = store::update_room_status(&conn, &self.room_id, status.as_str());
        }
        self.emit(GroupChatEvent::room_status(&self.room_id, status.as_str()));
    }

    async fn set_director(&self, director_id: &str) {
        if let Ok(conn) = self.conn() {
            let _ = store::update_room_director(&conn, &self.room_id, director_id);
        }
        // MVP：热替换需重建 Director 实例，此处仅落库 + 记录。
        log::info!("[GroupChat] director 已更新为 {}（重启后生效）", director_id);
        let _ = self.app.emit("groupchat-event", &GroupChatEvent::room_status(&self.room_id, "running"));
    }

    async fn handle_send(
        &mut self,
        sender: &str,
        content: &str,
        recipients: &[String],
        reply_to: Option<&str>,
        mention: Option<&str>,
        attachments: &[Attachment],
    ) {
        self.state.start();
        self.set_status(RoomStatus::Running).await;

        // 处理首条用户指令
        self.process_user_directive(sender, content, recipients, reply_to, mention, attachments).await;

        // 讨论 → 执行 主循环；在下一检查点检测到用户插队时，重新编排并重跑。
        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                break;
            }

            if self.discuss().await {
                // 插队后若编排已收敛（无新增任务且所有任务终态），结束而非空转。
                if self.orchestration_done() {
                    break;
                }
                continue;
            }

            if self.control.abort.load(Ordering::SeqCst) {
                break;
            }

            if self.execute_tasks().await {
                // 确认回复/插队后若编排已收敛，结束而非空转。
                if self.orchestration_done() {
                    break;
                }
                continue;
            }

            break;
        }

        // 收敛结论
        self.finalize().await;
    }

    /// 是否存在任何任务（终态或未终态）。用于判断重启后是否需要续跑/补结论。
    fn has_any_tasks(&self) -> bool {
        let Ok(conn) = self.conn() else { return false; };
        matches!(store::list_tasks(&conn, &self.room_id), Ok(t) if !t.is_empty())
    }

    /// 从 DB 重建分层记忆：重放消息、恢复立场、取最后一条 summary 作为 L1 摘要。
    async fn rebuild_memory(&mut self) {
        let conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return,
        };
        let messages = store::list_messages(&conn, &self.room_id, None, None).unwrap_or_default();
        let pids = self.all_participant_ids();

        let mut memory = LayeredMemory::new(&self.room.topic, "");
        memory.set_names(self.participant_names());
        for m in &messages {
            memory.apply_message(m, &pids);
        }
        if let Ok(stances) = store::list_stances(&conn, &self.room_id) {
            for s in stances {
                memory.update_stance(&s.participant_id, &s.stance);
            }
        }
        let summary = messages
            .iter()
            .rev()
            .find(|m| m.kind == "summary")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        memory.set_summary(&summary);

        self.memory = memory;
    }

    /// 恢复时处理 running 任务：崩溃时结果未知，按失败处理并重新排队（pending）走重试。
    /// 注意：不能标记为 failed（终态），否则 execute_tasks 会跳过、无法重跑；
    /// 改为重新排队 pending，交由 execute_tasks 重新执行（内含重试/重派）。
    async fn requeue_running_tasks(&self) {
        let conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return,
        };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else { return; };
        for mut t in tasks {
            if t.status == "running" {
                t.status = "pending".into();
                t.error = Some("应用重启，任务中断，已按失败重新排队重试".into());
                t.completed_at = None;
                let _ = store::update_task(&conn, &t);
                self.emit(GroupChatEvent::task_updated(&self.room_id, t));
            }
        }
    }

    /// 恢复执行：重建记忆 → running 任务重新排队重试 → 执行循环 → 收敛结论。
    /// 讨论收敛状态不持久化，恢复后跳过 discuss 直接执行（任务编排已持久化）。
    async fn resume_execution(&mut self) {
        self.rebuild_memory().await;
        self.requeue_running_tasks().await;
        self.set_status(RoomStatus::Running).await;

        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                break;
            }
            if self.execute_tasks().await {
                if self.orchestration_done() {
                    break;
                }
                continue;
            }
            break;
        }

        self.finalize().await;
    }

    /// 收敛结论：生成结论消息 + finished 事件 + 置 Finished（中止则置 Aborted）。
    async fn finalize(&mut self) {
        if self.control.abort.load(Ordering::SeqCst) {
            self.set_status(RoomStatus::Aborted).await;
            return;
        }
        let conclusion = self.conclude().await;
        // 主席结论落库消息表（可追溯）
        let director_sender = self.director_sender();
        let concl_msg = self
            .persist_message(&director_sender, &[], "conclusion", None, &conclusion, &[], "")
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, concl_msg));
        self.emit(GroupChatEvent::finished(&self.room_id));
        self.set_status(RoomStatus::Finished).await;
    }

    /// 处理一条用户指令：落库消息、重新编排任务、写入记忆。
    /// 首条指令与插队指令共用此入口，保证语义一致。
    async fn process_user_directive(
        &mut self,
        sender: &str,
        content: &str,
        recipients: &[String],
        reply_to: Option<&str>,
        mention: Option<&str>,
        attachments: &[Attachment],
    ) {
        // 用户消息落库 + 事件
        let user_msg = self.persist_message(sender, recipients, "directive", reply_to, content, attachments, "").await;
        self.emit(GroupChatEvent::message(&self.room_id, user_msg.clone()));

        // 记录用户 @ 指定的参与者，供本轮下一发言者选择消费。
        self.pending_mention = mention.filter(|m| !m.is_empty()).map(|m| m.to_string());

        self.after_user_directive(content, &user_msg).await;
    }

    /// 用户指令落库后的统一后续：按意图编排任务、写入记忆、重置收敛计数。
    async fn after_user_directive(&mut self, content: &str, user_msg: &MessageRow) {
        // 意图判定：首轮/目标变更 → 全量编排；refine → 对账式重排（可删减/重分配/新增）。
        let disposition = self.maybe_update_goal(content).await;

        let result = match disposition {
            GoalDisposition::FirstRound => self.kickoff_first(content).await,
            GoalDisposition::Changed(goal) => self.kickoff_first(&goal).await,
            GoalDisposition::Refined => self.replan(content).await,
        };
        self.last_kickoff_added = result.added;

        // 讨论前把用户指令写入记忆，确保首个发言者的 view.messages 至少含一条 user 消息，
        // 否则上游 API 会因 "No user query found in messages" 返回 400。
        let pids = self.all_participant_ids();
        self.memory.set_names(self.participant_names());
        self.memory.apply_message(user_msg, &pids);

        // 新指令意味着讨论主题可能变化，重置收敛计数，避免插队后过早收敛。
        self.rules.reset();
    }

    /// 当前房间任务总数。
    fn task_count(&self) -> usize {
        self.conn()
            .ok()
            .map(|c| store::list_tasks(&c, &self.room_id).unwrap_or_default().len())
            .unwrap_or(0)
    }

    /// 编排收敛谓词：最近一次编排未新增任务，且所有任务均处于终态。
    /// 用于主循环在插队/确认后判断是否应结束，避免「追加 0 个仍空转」。
    fn orchestration_done(&self) -> bool {
        if self.last_kickoff_added != 0 {
            return false;
        }
        let Ok(conn) = self.conn() else { return false; };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else { return false; };
        tasks
            .iter()
            .all(|t| matches!(t.status.as_str(), "success" | "skipped" | "failed" | "aborted"))
    }

    /// 任务终态即收敛的确定性兜底：存在任务且所有任务均处于终态。
    fn all_tasks_terminal(&self) -> bool {
        let Ok(conn) = self.conn() else { return false; };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else { return false; };
        !tasks.is_empty()
            && tasks
                .iter()
                .all(|t| matches!(t.status.as_str(), "success" | "skipped" | "failed" | "aborted"))
    }

    /// 判定并应用目标意图。默认目标不可变；仅在用户明确要求变更目标或提出
    /// 全新无关目标时更新目标锚点；refine 的补充/细化约束累积进 goal_notes。
    async fn maybe_update_goal(&mut self, content: &str) -> GoalDisposition {
        // 首轮无既有任务时无需判定（content 即初始目标）。
        let existing_count = self.task_count();
        if existing_count == 0 {
            return GoalDisposition::FirstRound;
        }

        let director = match self.director.clone() {
            Some(d) => d,
            None => return GoalDisposition::Refined,
        };

        let g = match director
            .classify_goal_intent(&self.room.topic, content, &self.room.goal_notes)
            .await
        {
            Some(g) => g,
            None => return GoalDisposition::Refined,
        };

        match g.intent.as_str() {
            "change_goal" | "new_goal" => {
                let new_goal = g.new_goal.trim().to_string();
                if new_goal.is_empty() || new_goal == self.room.topic {
                    return GoalDisposition::Refined;
                }
                self.apply_goal_change(&new_goal).await;
                GoalDisposition::Changed(new_goal)
            }
            _ => {
                // refine：若提炼出新的补充/细化约束，累积进 goal_notes 持久保留。
                let note = g.note.trim().to_string();
                if !note.is_empty() {
                    self.append_goal_note(&note);
                }
                GoalDisposition::Refined
            }
        }
    }

    /// 应用目标变更：归档旧任务、更新并清空补充约束、重置记忆、发系统提示。
    async fn apply_goal_change(&mut self, new_goal: &str) {
        // 1) 归档旧目标尚未完成的任务，避免继续执行旧目标。
        if let Ok(conn) = self.conn() {
            if let Ok(tasks) = store::list_tasks(&conn, &self.room_id) {
                for mut t in tasks {
                    if matches!(t.status.as_str(), "discussing" | "pending" | "running") {
                        t.status = "aborted".into();
                        t.error = Some("用户已变更目标，旧任务归档".into());
                        t.completed_at = Some(crate::utils::now());
                        let _ = store::update_task(&conn, &t);
                        self.emit(GroupChatEvent::task_updated(&self.room_id, t));
                    }
                }
            }
            let _ = store::update_room_topic(&conn, &self.room_id, new_goal);
            let _ = store::update_room_goal_notes(&conn, &self.room_id, "[]");
        }

        // 2) 同步内存目标 + 清空补充约束 + 重置记忆（旧目标摘要/立场/历史对新目标无意义）。
        self.room.topic = new_goal.to_string();
        self.room.goal_notes = "[]".to_string();
        self.memory = LayeredMemory::new(new_goal, "");

        // 3) 发系统提示，目标变更可追溯。
        let director_sender = self.director_sender();
        let sys_msg = self
            .persist_message(
                &director_sender,
                &[],
                "system",
                None,
                &format!("用户已变更讨论目标：\n{}", new_goal),
                &[],
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, sys_msg));
    }

    /// 累积一条补充/细化约束到 goal_notes（去重后持久化）。
    fn append_goal_note(&mut self, note: &str) {
        let mut notes: Vec<String> =
            serde_json::from_str(&self.room.goal_notes).unwrap_or_default();
        if !notes.iter().any(|n| n == note) {
            notes.push(note.to_string());
        }
        let serialized = serde_json::to_string(&notes).unwrap_or_else(|_| "[]".into());
        if let Ok(conn) = self.conn() {
            let _ = store::update_room_goal_notes(&conn, &self.room_id, &serialized);
        }
        self.room.goal_notes = serialized;
    }

    /// 非阻塞地消费命令队列中已到达的命令。
    /// 用户消息（Send）返回给调用方做插队处理；其余命令就地处理。
    async fn drain_pending_commands(&mut self) -> Option<RoomCommand> {
        loop {
            match self.cmd_rx.try_recv() {
                Ok(cmd @ RoomCommand::Send { .. }) => return Some(cmd),
                Ok(RoomCommand::SetDirector { director_id }) => self.set_director(&director_id).await,
                Ok(RoomCommand::ParticipantChanged) => self.handle_participant_changed().await,
                Ok(RoomCommand::Pause) => self.set_status(RoomStatus::Paused).await,
                Ok(RoomCommand::Resume) => self.set_status(RoomStatus::Running).await,
                Ok(RoomCommand::Abort) => self.set_status(RoomStatus::Aborted).await,
                Ok(RoomCommand::RespondConfirmation { .. }) => {}
                Err(_) => return None,
            }
        }
    }

    /// 首轮 / 目标变更后的全量编排：Director 输出完整任务清单，直接落库。
    async fn kickoff_first(&mut self, goal: &str) -> KickoffResult {
        // 首轮重置记忆；目标变更场景的旧记忆已由 apply_goal_change 重置。
        let existing_count = self.task_count();
        if existing_count == 0 {
            self.memory = LayeredMemory::new(&self.room.topic, "");
        }
        let names = self.participant_names();
        let roster: Vec<(String, String)> = self
            .floor_order
            .iter()
            .map(|id| {
                let name = names.get(id).cloned().unwrap_or_else(|| id.clone());
                (id.clone(), name)
            })
            .collect();

        let mut tasks = Vec::new();
        let mut role_map: HashMap<String, String> = HashMap::new();
        if let Some(director) = &self.director {
            if let Some((drafts, roles)) = director.kickoff(&self.room.topic, goal, &self.room.goal_notes, &roster).await {
                role_map = roles;
                for (i, d) in drafts.iter().enumerate() {
                    let assignee = d
                        .assignee
                        .clone()
                        .filter(|id| self.participants.contains_key(id));
                    // Director 返回批内下标，偏移为全量任务表绝对下标（归档的旧任务仍占下标）。
                    let depends_abs: Vec<usize> = d
                        .depends_on
                        .iter()
                        .map(|p| p + existing_count)
                        .collect();
                    let task = TaskRow {
                        id: uuid::Uuid::new_v4().to_string(),
                        room_id: self.room_id.clone(),
                        task_no: (existing_count + i + 1) as i64,
                        description: d.description.clone(),
                        assignee,
                        depends_on: serde_json::to_string(&depends_abs).unwrap_or_else(|_| "[]".into()),
                        status: "discussing".into(),
                        result_summary: None,
                        error: None,
                        started_at: None,
                        completed_at: None,
                    };
                    if let Ok(conn) = self.conn() {
                        let _ = store::insert_task(&conn, &task);
                    }
                    self.emit(GroupChatEvent::task_updated(&self.room_id, task.clone()));
                    tasks.push(task);
                }
                for (pid, role) in &role_map {
                    if let Ok(conn) = self.conn() {
                        let _ = store::update_participant_role(&conn, &self.room_id, pid, role);
                    }
                    self.emit(GroupChatEvent::role_updated(&self.room_id, pid, role));
                    // 同步运行时参与者角色，使后续发言的 system prompt 使用新指派角色。
                    if let Some(p) = self.participants.get(pid) {
                        p.set_role(role).await;
                    }
                }
            }

            // 主席开场调度信息以正式发言方式显示在消息流中（任务清单 + 角色指派）。
            if !tasks.is_empty() {
                let mut lines: Vec<String> = Vec::new();
                lines.push("主席调度".to_string());
                lines.push(String::new());
                lines.push("任务指派：".to_string());
                for t in &tasks {
                    let assignee = t
                        .assignee
                        .as_deref()
                        .map(|id| names.get(id).cloned().unwrap_or_else(|| id.to_string()))
                        .unwrap_or_else(|| "待指派".to_string());
                    lines.push(format!("- T{}. {}（负责人：{}）", t.task_no, t.description, assignee));
                }
                if !role_map.is_empty() {
                    lines.push(String::new());
                    lines.push("角色指派：".to_string());
                    for (pid, role) in &role_map {
                        let name = names.get(pid).cloned().unwrap_or_else(|| pid.clone());
                        lines.push(format!("- {} → {}", name, role));
                    }
                }
                let schedule_text = lines.join("\n");
                let director_sender = self.director_id.clone().unwrap_or_else(|| "director".to_string());
                let schedule_msg = self
                    .persist_message(&director_sender, &[], "task_assignment", None, &schedule_text, &[], "")
                    .await;
                self.emit(GroupChatEvent::message(&self.room_id, schedule_msg));
            }
        }

        // 无 Director 或编排失败 → 兜底生成单一任务（仅首轮）
        if tasks.is_empty() && existing_count == 0 {
            let task = TaskRow {
                id: uuid::Uuid::new_v4().to_string(),
                room_id: self.room_id.clone(),
                task_no: 1,
                description: self.room.topic.clone(),
                assignee: None,
                depends_on: "[]".into(),
                status: "discussing".into(),
                result_summary: None,
                error: None,
                started_at: None,
                completed_at: None,
            };
            if let Ok(conn) = self.conn() {
                let _ = store::insert_task(&conn, &task);
            }
            self.emit(GroupChatEvent::task_updated(&self.room_id, task.clone()));
            tasks.push(task);
        }

        if let Some(first) = tasks.first() {
            if let Ok(conn) = self.conn() {
                let _ = store::update_room_current_task(&conn, &self.room_id, &first.id);
            }
        }
        KickoffResult { added: tasks.len() }
    }

    /// 追加轮对账式重排：Director 输出 keep/remove/add 操作，运行时删减未执行任务、
    /// 重分配负责人、新增必要子任务。返回本次实际新增的任务数。
    async fn replan(&mut self, latest: &str) -> KickoffResult {
        let existing = match self.conn() {
            Ok(c) => store::list_tasks(&c, &self.room_id).unwrap_or_default(),
            Err(_) => return KickoffResult { added: 0 },
        };
        if existing.is_empty() {
            // 理论不会发生（Refined 仅在已有任务时进入）；兜底走首轮。
            return self.kickoff_first(latest).await;
        }

        let names = self.participant_names();
        let roster: Vec<(String, String)> = self
            .floor_order
            .iter()
            .map(|id| {
                let name = names.get(id).cloned().unwrap_or_else(|| id.clone());
                (id.clone(), name)
            })
            .collect();

        let plan = match &self.director {
            Some(d) => d
                .replan(&self.room.topic, latest, &self.room.goal_notes, &existing, &roster)
                .await,
            None => None,
        };

        let Some(plan) = plan else {
            // Director 不可用或对账失败：保守策略，不新增、不删减、不重分配。
            return KickoffResult { added: 0 };
        };

        let app = self.apply_replan_operations(&existing, &plan.operations).await;

        // 对账调度可追溯消息（有变化时）
        if !app.added.is_empty() || app.removed != 0 || app.reassigned != 0 {
            let mut lines: Vec<String> = vec!["主席对账调度".to_string(), String::new()];
            if app.removed != 0 {
                lines.push(format!("移除冗余任务 {} 项", app.removed));
            }
            if app.reassigned != 0 {
                lines.push(format!("重分配任务 {} 项", app.reassigned));
            }
            if !app.added.is_empty() {
                lines.push("新增任务：".to_string());
                for t in &app.added {
                    let assignee = t
                        .assignee
                        .as_deref()
                        .map(|id| names.get(id).cloned().unwrap_or_else(|| id.to_string()))
                        .unwrap_or_else(|| "待指派".to_string());
                    lines.push(format!("- T{}. {}（负责人：{}）", t.task_no, t.description, assignee));
                }
            }
            let director_sender = self.director_id.clone().unwrap_or_else(|| "director".to_string());
            let msg = self
                .persist_message(&director_sender, &[], "task_assignment", None, &lines.join("\n"), &[], "")
                .await;
            self.emit(GroupChatEvent::message(&self.room_id, msg));
        }

        if let Some(first) = app.added.first() {
            if let Ok(conn) = self.conn() {
                let _ = store::update_room_current_task(&conn, &self.room_id, &first.id);
            }
        }

        KickoffResult { added: app.added.len() }
    }

    /// 应用 Director 重排操作（级联删减未执行任务、重派、新增）并落库/发事件。
    /// 返回变更统计，供调用方生成调度消息。
    async fn apply_replan_operations(
        &mut self,
        existing: &[TaskRow],
        operations: &[ReplanOperation],
    ) -> ReplanApplication {
        let id_to_index: HashMap<String, usize> = existing
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i))
            .collect();

        // 1) 归类操作
        let mut remove_roots: Vec<String> = Vec::new();
        let mut remove_reasons: HashMap<String, String> = HashMap::new();
        let mut reassigns: HashMap<usize, String> = HashMap::new();
        let mut adds: Vec<ReplanOperation> = Vec::new();
        for op in operations {
            match op.action.as_str() {
                "remove" => {
                    if let Some(tid) = op.task_id.clone() {
                        remove_roots.push(tid.clone());
                        if !op.reason.trim().is_empty() {
                            remove_reasons.insert(tid, op.reason.trim().to_string());
                        }
                    }
                }
                "keep" => {
                    if let Some(tid) = op.task_id.clone() {
                        if let Some(idx) = id_to_index.get(&tid) {
                            if let Some(ra) = op
                                .reassign
                                .clone()
                                .filter(|id| self.participants.contains_key(id))
                            {
                                reassigns.insert(*idx, ra);
                            }
                        }
                    }
                }
                "add" => {
                    if !op.description.trim().is_empty() {
                        adds.push(op.clone());
                    }
                }
                _ => {}
            }
        }

        // 2) 级联删减未执行任务（root 及其未执行后继）
        let mut removed: HashSet<usize> = HashSet::new();
        for root_id in &remove_roots {
            let Some(&root_idx) = id_to_index.get(root_id) else { continue; };
            let mut stack = vec![root_idx];
            while let Some(idx) = stack.pop() {
                if removed.contains(&idx) {
                    continue;
                }
                // 仅删减「未执行」任务（discussing）；终态/pending 一律不动。
                if existing[idx].status != "discussing" {
                    continue;
                }
                removed.insert(idx);
                for (j, other) in existing.iter().enumerate() {
                    if j == idx || other.status != "discussing" {
                        continue;
                    }
                    if let Ok(deps) = serde_json::from_str::<Vec<usize>>(&other.depends_on) {
                        if deps.contains(&idx) {
                            stack.push(j);
                        }
                    }
                }
            }
        }

        let mut removed_sorted: Vec<usize> = removed.iter().copied().collect();
        removed_sorted.sort_unstable();

        for idx in &removed_sorted {
            let mut t = existing[*idx].clone();
            t.status = "aborted".into();
            let reason = remove_reasons
                .get(&t.id)
                .cloned()
                .unwrap_or_else(|| "Director 重排判定冗余".to_string());
            t.error = Some(format!("已移除：{reason}"));
            t.completed_at = Some(crate::utils::now());
            if let Ok(conn) = self.conn() {
                let _ = store::update_task(&conn, &t);
            }
            self.emit(GroupChatEvent::task_updated(&self.room_id, t));
        }

        // 3) 重分配（仅 discussing/pending，且未被级联删减）
        for (idx, new_assignee) in &reassigns {
            if removed.contains(idx) {
                continue;
            }
            let mut t = existing[*idx].clone();
            if t.status == "discussing" || t.status == "pending" {
                t.assignee = Some(new_assignee.clone());
                if let Ok(conn) = self.conn() {
                    let _ = store::update_task(&conn, &t);
                }
                self.emit(GroupChatEvent::task_updated(&self.room_id, t));
            }
        }

        // 4) 新增任务（depends_on 引用已有任务 id，映射为下标）
        let mut new_tasks: Vec<TaskRow> = Vec::new();
        for op in &adds {
            let mut depends_abs: Vec<usize> = Vec::new();
            for dep_id in &op.depends_on {
                if let Some(&idx) = id_to_index.get(dep_id) {
                    if !depends_abs.contains(&idx) {
                        depends_abs.push(idx);
                    }
                }
            }
            let assignee = op
                .assignee
                .clone()
                .filter(|id| self.participants.contains_key(id));
            let task_no = (existing.len() + new_tasks.len() + 1) as i64;
            let task = TaskRow {
                id: uuid::Uuid::new_v4().to_string(),
                room_id: self.room_id.clone(),
                task_no,
                description: op.description.clone(),
                assignee,
                depends_on: serde_json::to_string(&depends_abs).unwrap_or_else(|_| "[]".into()),
                status: "discussing".into(),
                result_summary: None,
                error: None,
                started_at: None,
                completed_at: None,
            };
            if let Ok(conn) = self.conn() {
                let _ = store::insert_task(&conn, &task);
            }
            self.emit(GroupChatEvent::task_updated(&self.room_id, task.clone()));
            new_tasks.push(task);
        }

        ReplanApplication {
            removed: removed.len(),
            reassigned: reassigns.len(),
            added: new_tasks,
        }
    }

    /// 名册变更（加入/移除参与者）后：重建运行时列表，并 Director 重排角色与任务。
    async fn handle_participant_changed(&mut self) {
        self.rebuild_participants().await;
        // 名册变化改变讨论结构，重置收敛计数，避免旧名册结论过早收敛。
        self.rules.reset();

        // 尚无任务：下一次 kickoff_first 自然纳入新名册，无需单独重排。
        if self.task_count() == 0 {
            return;
        }

        let Some(director) = self.director.clone() else { return; };
        let Ok(conn) = self.conn() else { return; };
        let Ok(existing) = store::list_tasks(&conn, &self.room_id) else { return; };

        let names = self.participant_names();
        let roles = self.participant_roles();
        let roster: Vec<(String, String, String)> = self
            .floor_order
            .iter()
            .map(|id| {
                let name = names.get(id).cloned().unwrap_or_else(|| id.clone());
                let role = roles.get(id).cloned().unwrap_or_default();
                (id.clone(), name, role)
            })
            .collect();

        let Some(plan) = director
            .replan_for_roster_change(&self.room.topic, &self.room.goal_notes, &existing, &roster)
            .await
        else {
            return;
        };

        // 应用角色分配
        let mut role_lines: Vec<String> = Vec::new();
        for (pid, role) in &plan.roles {
            if let Ok(conn) = self.conn() {
                let _ = store::update_participant_role(&conn, &self.room_id, pid, role);
            }
            self.emit(GroupChatEvent::role_updated(&self.room_id, pid, role));
            if let Some(p) = self.participants.get(pid) {
                p.set_role(role).await;
            }
            let name = names.get(pid).cloned().unwrap_or_else(|| pid.clone());
            role_lines.push(format!("- {} → {}", name, role));
        }

        // 应用任务重排
        let app = self.apply_replan_operations(&existing, &plan.operations).await;

        // 名册变更调度消息（可追溯）
        let mut lines: Vec<String> = vec!["主席调度：参与者名册变更，已重新编排".to_string()];
        if !role_lines.is_empty() {
            lines.push(String::new());
            lines.push("角色分配：".to_string());
            lines.extend(role_lines);
        }
        if app.removed != 0 {
            lines.push(format!("移除冗余任务 {} 项", app.removed));
        }
        if app.reassigned != 0 {
            lines.push(format!("重分配任务 {} 项", app.reassigned));
        }
        if !app.added.is_empty() {
            lines.push("新增任务：".to_string());
            for t in &app.added {
                let assignee = t
                    .assignee
                    .as_deref()
                    .map(|id| names.get(id).cloned().unwrap_or_else(|| id.to_string()))
                    .unwrap_or_else(|| "待指派".to_string());
                lines.push(format!("- T{}. {}（负责人：{}）", t.task_no, t.description, assignee));
            }
        }
        let director_sender = self.director_sender();
        let msg = self
            .persist_message(&director_sender, &[], "task_assignment", None, &lines.join("\n"), &[], "")
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, msg));

        if let Some(first) = app.added.first() {
            if let Ok(conn) = self.conn() {
                let _ = store::update_room_current_task(&conn, &self.room_id, &first.id);
            }
        }
    }

    async fn discuss(&mut self) -> bool {
        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                return false;
            }
            self.control.wait_if_paused().await;

            // 下一检查点：检测用户插队，发现新指令则处理并中断讨论，交由上层重跑。
            if let Some(RoomCommand::Send { sender, content, recipients, reply_to, mention, attachments }) = self.drain_pending_commands().await {
                self.process_user_directive(&sender, &content, &recipients, reply_to.as_deref(), mention.as_deref(), &attachments).await;
                return true;
            }

            // 确定性收敛兜底：所有任务均已终态且无新增，直接结束讨论，避免空转。
            if self.all_tasks_terminal() {
                return false;
            }

            // 选下一发言者：用户 @ 指定优先；否则 Director 决策，失败走轮询兜底。
            let mention = self.pending_mention.take();
            let task_manifest = self.build_task_manifest();
            let decision = match mention.as_ref() {
                Some(m) if self.floor_order.iter().any(|id| id == m) => {
                    Some(SpeakerDecision { speaker: m.clone(), reason: "用户 @ 指定".into() })
                }
                _ => match &self.director {
                    Some(d) => d
                        .next_speaker(&self.room.topic, self.memory.summary(), &self.floor_order, &task_manifest, mention.as_deref())
                        .await,
                    None => None,
                },
            };
            // 主席发言者选择决策落库（可追溯）
            if let Some(dec) = &decision {
                let director_sender = self.director_sender();
                // 发言者显示名：id（显示名），提升可读性；无显示名或与 id 相同时仅用 id。
                let names = self.participant_names();
                let speaker_label = match names.get(&dec.speaker) {
                    Some(n) if !n.is_empty() && n != &dec.speaker => format!("{}（{}）", dec.speaker, n),
                    _ => dec.speaker.clone(),
                };
                let text = if dec.reason.is_empty() {
                    format!("调度：选择 {} 发言", speaker_label)
                } else {
                    format!("调度：选择 {} 发言（{}）", speaker_label, dec.reason)
                };
                let msg = self.persist_message(&director_sender, &[], "scheduling", None, &text, &[], "").await;
                self.emit(GroupChatEvent::message(&self.room_id, msg));
            }
            let speaker = match self.floor.next(decision.as_ref().map(|d| d.speaker.as_str())) {
                Some(s) => s,
                None => return false,
            };
            self.state.next_round();
            let round = self.state.round;
            self.emit(GroupChatEvent::floor_granted(&self.room_id, &speaker, round));

            let Some(participant) = self.participants.get(&speaker).cloned() else {
                continue;
            };

            let mut view = self.memory.build_view(&speaker, "", false, &self.user_id);
            view.task_context = self.build_speaker_task_context(&speaker);

            let app = self.app.clone();
            let room_id = self.room_id.clone();
            let speaker_id = speaker.clone();
            let delta_fn = move |delta: &str| {
                let _ = app.emit(
                    "groupchat-event",
                    &GroupChatEvent::token_stream(&room_id, &speaker_id, delta),
                );
            };
            let result = participant.run_turn(view, Some(Arc::new(delta_fn))).await;
            self.flush_approval_logs().await;
            // 参与者主动发起确认：进入确认流程（讨论阶段无关联任务，task_id 为空）。
            if let Some(conf) = result.confirmation {
                self.handle_user_confirmation(&speaker, conf).await;
                return true;
            }
            // 讨论阶段失败归一化：把错误信息作为发言内容，保证消息流可读（执行阶段才走重试/重派）。
            let speech = if result.content.is_empty() {
                result.error.clone().unwrap_or_default()
            } else {
                result.content.clone()
            };

            let msg = self.persist_message(&speaker, &[], "statement", None, &speech, &[], &result.tool_calls).await;
            self.emit(GroupChatEvent::message(&self.room_id, msg.clone()));
            self.memory.apply_message(&msg, &self.all_participant_ids());

            // 立场快照（MVP：截取发言作为立场，后续可由 Director 判定）
            let stance = speech.chars().take(200).collect::<String>();
            self.memory.update_stance(&speaker, &stance);
            if let Ok(conn) = self.conn() {
                let _ = store::upsert_stance(&conn, &self.room_id, &speaker, &stance);
            }
            self.emit(GroupChatEvent::stance_updated(&self.room_id, &speaker, &stance));

            // 语义收敛审查：Director 审阅进度、更新 L1 摘要，并判定 discuss/confirm/done。
            let review = match &self.director {
                Some(director) => {
                    let transcript = self.build_review_transcript(30);
                    director
                        .director_review(
                            &self.room.topic,
                            self.memory.summary(),
                            &transcript,
                            &self.room.goal_notes,
                            &task_manifest,
                        )
                        .await
                }
                None => None,
            };

            let Some(review) = review else {
                // Director 不可用或裁决失败：无法语义收敛，依赖确定性兜底继续轮询。
                continue;
            };
            log::info!(
                "[GroupChat] Director 审阅: next={} reason={}",
                review.next_action,
                review.reason
            );

            // 更新 L1 摘要（非空才落库，避免覆盖已有有效摘要）。
            if !review.summary.is_empty() {
                self.memory.set_summary(&review.summary);
                let director_sender = self.director_sender();
                let summary_msg = self
                    .persist_message(&director_sender, &[], "summary", None, &review.summary, &[], "")
                    .await;
                self.emit(GroupChatEvent::message(&self.room_id, summary_msg));
            }

            match review.next_action.as_str() {
                "confirm" => {
                    if let Some(conf) = review.confirmation {
                        let director_sender = self.director_sender();
                        self.handle_user_confirmation(&director_sender, conf).await;
                        return true;
                    }
                    // 缺确认请求时退化为继续讨论。
                    self.rules.should_converge(false);
                }
                "done" => {
                    // done 连续 2 轮防抖收敛，避免单次误判。
                    if self.rules.should_converge(true) {
                        return false;
                    }
                }
                _ => {
                    self.rules.should_converge(false);
                }
            }
        }
    }

    async fn persist_message(
        &mut self,
        sender: &str,
        recipients: &[String],
        kind: &str,
        reply_to: Option<&str>,
        content: &str,
        attachments: &[Attachment],
        tool_calls: &str,
    ) -> MessageRow {
        self.persist_message_extra(sender, recipients, kind, reply_to, content, attachments, tool_calls, "{}").await
    }

    /// 落库消息，支持附加结构化数据（extra，如用户确认请求 JSON）。
    async fn persist_message_extra(
        &mut self,
        sender: &str,
        recipients: &[String],
        kind: &str,
        reply_to: Option<&str>,
        content: &str,
        attachments: &[Attachment],
        tool_calls: &str,
        extra: &str,
    ) -> MessageRow {
        self.seq += 1;
        let msg = MessageRow {
            id: uuid::Uuid::new_v4().to_string(),
            room_id: self.room_id.clone(),
            round: self.state.round,
            seq: self.seq,
            sender: sender.to_string(),
            recipients: serde_json::to_string(recipients).unwrap_or_else(|_| "[]".into()),
            kind: kind.to_string(),
            reply_to: reply_to.map(String::from),
            content: normalize_message_content(content),
            attachments: serde_json::to_string(attachments).unwrap_or_else(|_| "[]".into()),
            tool_calls: normalize_tool_calls(tool_calls),
            extra: extra.to_string(),
            timestamp: crate::utils::now(),
        };
        if let Ok(conn) = self.conn() {
            let _ = store::insert_message(&conn, &msg);
        }
        msg
    }

    fn all_participant_ids(&self) -> Vec<String> {
        self.participants.keys().cloned().collect()
    }

    /// Director 发言者 id（消息落库时的 sender）。
    fn director_sender(&self) -> String {
        self.director_id.clone().unwrap_or_else(|| "director".to_string())
    }

    /// participant_id -> display_name（供 Director 编排与 LLM 视图使用可读名称）。
    fn participant_names(&self) -> HashMap<String, String> {
        self.conn()
            .ok()
            .and_then(|c| store::list_participants(&c, &self.room_id).ok())
            .map(|rows| rows.into_iter().map(|p| (p.id, p.display_name)).collect())
            .unwrap_or_default()
    }

    /// participant_id -> system_role（供名册变更重排时把现有角色传给 Director）。
    fn participant_roles(&self) -> HashMap<String, String> {
        self.conn()
            .ok()
            .and_then(|c| store::list_participants(&c, &self.room_id).ok())
            .map(|rows| rows.into_iter().map(|p| (p.id, p.system_role)).collect())
            .unwrap_or_default()
    }

    async fn execute_tasks(&mut self) -> bool {
        const RETRY_MAX: usize = 2;         // 失败后额外重试次数
        const REASSIGN_MAX: usize = 2;      // Director 重派次数上限
        const BASE_BACKOFF_MS: u64 = 500;   // 重试指数退避基数

        let tasks = match self.conn() {
            Ok(c) => store::list_tasks(&c, &self.room_id).unwrap_or_default(),
            Err(_) => return false,
        };
        if tasks.is_empty() {
            return false;
        }

        let deps: Vec<Vec<usize>> = tasks
            .iter()
            .map(|t| serde_json::from_str::<Vec<usize>>(&t.depends_on).unwrap_or_default())
            .collect();
        let n = tasks.len();
        let max_parallel = self.room.max_parallel.max(1) as usize;

        // 下标 -> 最终状态（依赖检查用，下标与 tasks 数组一致）
        let mut final_status: Vec<String> = tasks.iter().map(|t| t.status.clone()).collect();
        // 终态任务（成功/跳过/失败/归档）不再重复执行，也不计入后继的前置数。
        let done: Vec<bool> = final_status
            .iter()
            .map(|s| matches!(s.as_str(), "success" | "skipped" | "failed" | "aborted"))
            .collect();

        // Kahn 调度结构：indegree + dependents，依赖完成后释放后继，恢复并行执行。
        // 已终态的任务不再重复执行，且不计入后继的前置数（视作已放行）。
        let mut indegree = vec![0usize; n];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, parents) in deps.iter().enumerate() {
            for &p in parents {
                if p < n && p != i {
                    if !done[p] {
                        indegree[i] += 1;
                    }
                    dependents[p].push(i);
                }
            }
        }

        let mut ready: Vec<usize> = (0..n)
            .filter(|&i| !done[i] && indegree[i] == 0)
            .collect();
        let summary = self.memory.summary().to_string();
        let director_sender = self.director_sender();

        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                break;
            }
            self.control.wait_if_paused().await;

            // 下一检查点：检测用户插队，发现新指令则处理并中断执行，交由上层重跑。
            if let Some(RoomCommand::Send { sender, content, recipients, reply_to, mention, attachments }) = self.drain_pending_commands().await {
                self.process_user_directive(&sender, &content, &recipients, reply_to.as_deref(), mention.as_deref(), &attachments).await;
                return true;
            }

            if ready.is_empty() {
                break;
            }

            // 取当前波次（受 max_parallel 限制）
            let wave: Vec<usize> = ready.drain(..ready.len().min(max_parallel)).collect();

            let mut handles = Vec::with_capacity(wave.len());
            for &idx in &wave {
                // 依赖感知跳过：任一前置非 success → skipped，不再无意义执行
                let dep_blocked = deps[idx]
                    .iter()
                    .any(|&d| d >= final_status.len() || final_status[d] != "success");

                // 首次执行者：优先 Director 指派的 assignee，否则轮询兜底
                let initial_assignee = tasks[idx]
                    .assignee
                    .clone()
                    .filter(|id| self.participants.contains_key(id))
                    .or_else(|| self.executor_order.get(idx % self.executor_order.len().max(1)).cloned())
                    .or_else(|| self.executor_order.first().cloned());

                let mut task = tasks[idx].clone();
                if !dep_blocked {
                    task.assignee = initial_assignee.clone();
                    task.status = "running".into();
                    task.started_at = Some(crate::utils::now());
                    self.save_and_emit_task(&task);
                }

                let participants = self.participants.clone();
                let director = self.director.clone();
                let executor_order = self.executor_order.clone();
                let app = self.app.clone();
                let room_id = self.room_id.clone();
                let topic = self.room.topic.clone();
                let control = self.control.clone();
                let summary = summary.clone();
                let director_sender = director_sender.clone();

                let handle = tokio::spawn(async move {
                    Self::execute_single_task(
                        task,
                        initial_assignee,
                        dep_blocked,
                        participants,
                        director,
                        topic,
                        executor_order,
                        app,
                        room_id,
                        control,
                        summary,
                        director_sender,
                        RETRY_MAX,
                        REASSIGN_MAX,
                        BASE_BACKOFF_MS,
                    )
                    .await
                });
                handles.push((idx, handle));
            }

            // 汇聚波次结果，回到 Actor 主循环按顺序统一落库。
            let mut pending_confirmation: Option<(String, ConfirmationRequest)> = None;
            for (idx, handle) in handles {
                // 执行波次汇聚期插队检查：检测到用户新消息立即处理并交还主循环。
                if let Some(RoomCommand::Send { sender, content, recipients, reply_to, mention, attachments }) =
                    self.drain_pending_commands().await
                {
                    self.process_user_directive(&sender, &content, &recipients, reply_to.as_deref(), mention.as_deref(), &attachments).await;
                    return true;
                }

                let outcome = match handle.await {
                    Ok(o) => o,
                    Err(e) => {
                        let mut t = tasks[idx].clone();
                        t.status = "failed".into();
                        t.error = Some(format!("任务执行异常: {}", e));
                        t.completed_at = Some(crate::utils::now());
                        TaskOutcome { task: t, messages: Vec::new(), should_abort: false, tool_calls: String::new(), confirmation: None, confirmation_sender: None }
                    }
                };

                // 先落库 Director 在重试/重派过程中的裁决消息
                for pm in outcome.messages {
                    let msg = self
                        .persist_message(&pm.sender, &[], &pm.kind, None, &pm.content, &[], "")
                        .await;
                    self.emit(GroupChatEvent::message(&self.room_id, msg));
                }

                if outcome.should_abort {
                    self.control.abort.store(true, Ordering::SeqCst);
                }

                let awaiting_user = outcome.confirmation.is_some();
                if let Some(conf) = outcome.confirmation {
                    let sender = outcome
                        .confirmation_sender
                        .unwrap_or_else(|| director_sender.clone());
                    pending_confirmation = Some((sender, conf));
                }

                let task = outcome.task;
                final_status[idx] = task.status.clone();
                self.save_and_emit_task(&task);

                // ask_user 的任务暂不落库 task_result，由确认请求消息承载交互。
                if !awaiting_user {
                    self.persist_task_message(&task, &outcome.tool_calls).await;
                }

                // 释放后继依赖
                for &next in &dependents[idx] {
                    indegree[next] -= 1;
                    if indegree[next] == 0 {
                        ready.push(next);
                    }
                }
            }

            // 本轮工具授权裁决消息统一落库
            self.flush_approval_logs().await;

            // 有任务需要用户确认：进入确认等待，用户回复后重新编排并重跑。
            if let Some((sender, conf)) = pending_confirmation {
                self.handle_user_confirmation(&sender, conf).await;
                return true;
            }
        }

        // 执行结束：把执行结果回流到 L1 摘要，供后续结论/讨论使用。
        self.refresh_summary().await;

        false
    }

    /// 处理 Director 或参与者发起的用户确认：落库确认请求、等待用户回复、把回复注入原任务重跑。
    async fn handle_user_confirmation(&mut self, sender: &str, confirmation: ConfirmationRequest) {
        // 1. 落库确认请求消息（extra 携带完整结构供前端渲染确认 UI）。
        let extra = serde_json::to_string(&confirmation).unwrap_or_else(|_| "{}".into());
        let req_msg = self
            .persist_message_extra(
                sender,
                &[],
                "confirmation_request",
                None,
                &confirmation.prompt,
                &[],
                "",
                &extra,
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, req_msg));

        // 2. 阻塞等待用户回复。
        let Some(reply) = self.await_confirmation_response(&confirmation.request_id, &confirmation.items).await else {
            return;
        };

        // 3. 结构化回复直接按确认处理；开放式消息由 Director 判断语义（confirm / directive）。
        match reply {
            ConfirmationReply::Structured(reply_content) => {
                self.apply_confirmation_flow(&confirmation, &reply_content).await;
            }
            ConfirmationReply::Open { sender, content, recipients, reply_to, mention, attachments } => {
                let intent = match self.director.clone() {
                    Some(d) => d
                        .classify_reply_intent(&confirmation.prompt, &content)
                        .await
                        .unwrap_or_else(|| "confirm".to_string()),
                    None => "confirm".to_string(),
                };
                if intent == "directive" {
                    // 用户插入的新指令：按正常用户指令处理（落库 + 重新编排）。
                    self.process_user_directive(&sender, &content, &recipients, reply_to.as_deref(), mention.as_deref(), &attachments).await;
                } else {
                    // 确认回复：落库 + 注入 pending 任务重跑。
                    self.apply_confirmation_flow(&confirmation, &content).await;
                }
            }
        }
    }

    /// 确认回复的统一处理：落库 confirmation_response 消息，并注入原 pending 任务重跑。
    async fn apply_confirmation_flow(&mut self, confirmation: &ConfirmationRequest, reply_content: &str) {
        let user_sender = self.user_id.clone();
        let resp_msg = self
            .persist_message(&user_sender, &[], "confirmation_response", Some(confirmation.request_id.as_str()), reply_content, &[], "")
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, resp_msg.clone()));

        // 有明确关联任务时，把回复写回原 pending 任务并恢复为 discussing 以便重跑；
        // 讨论阶段发起（无关联任务）时仅写入记忆，不注入任务。
        if !confirmation.task_id.is_empty() {
            self.apply_confirmation_reply(&confirmation.task_id, reply_content).await;
        }

        let pids = self.all_participant_ids();
        self.memory.set_names(self.participant_names());
        self.memory.apply_message(&resp_msg, &pids);

        // 重置收敛计数，避免确认后过早收敛。
        self.rules.reset();
    }

    /// 非确认等待状态下的兜底落库：仅持久化 confirmation_response 消息（reply_to=request_id），
    /// 不恢复任务重跑、不重置收敛计数（房间可能已结束或未在等待确认）。
    /// 用于历史确认块补交 / Actor 重启后未进入确认等待等场景，保证前端刷新后能恢复「已提交」态。
    async fn persist_confirmation_response_only(&mut self, request_id: &str, responses: &[ConfirmationResponse]) {
        let content = self.format_confirmation_reply(request_id, responses);
        let user_sender = self.user_id.clone();
        let resp_msg = self
            .persist_message(&user_sender, &[], "confirmation_response", Some(request_id), &content, &[], "")
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, resp_msg.clone()));
    }

    /// 把结构化确认回复格式化为可读文本：优先用对应确认请求中的 item label 映射，缺失时回退 item_id。
    fn format_confirmation_reply(&self, request_id: &str, responses: &[ConfirmationResponse]) -> String {
        let labels: HashMap<String, String> = self
            .conn()
            .ok()
            .and_then(|conn| store::list_messages(&conn, &self.room_id, None, None).ok())
            .and_then(|rows| {
                rows.into_iter()
                    .find(|m| m.kind == "confirmation_request" && m.extra.contains(request_id))
                    .and_then(|m| serde_json::from_str::<ConfirmationRequest>(&m.extra).ok())
            })
            .map(|c| {
                c.items
                    .iter()
                    .filter(|it| !it.label.is_empty())
                    .map(|it| (it.id.clone(), it.label.clone()))
                    .collect()
            })
            .unwrap_or_default();
        responses
            .iter()
            .map(|r| {
                let label = labels
                    .get(&r.item_id)
                    .cloned()
                    .unwrap_or_else(|| r.item_id.clone());
                format!("{}：{}", label, r.value)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 等待用户对确认请求的回复；通道关闭/中止返回 `None`。
    /// `request_id` 用于校验回复关联的确认块（错配/历史块仅落库标记，不中断当前等待）。
    /// `items` 用于把 item_id 映射为可读的 label，供后续理解用户选择。
    async fn await_confirmation_response(&mut self, request_id: &str, items: &[ConfirmationItem]) -> Option<ConfirmationReply> {
        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                return None;
            }
            match self.cmd_rx.recv().await {
                Some(RoomCommand::RespondConfirmation { request_id: rid, responses }) => {
                    if rid != request_id {
                        // 用户回复的是历史/错配的确认块：仅落库标记已提交，继续等待当前确认。
                        self.persist_confirmation_response_only(&rid, &responses).await;
                        continue;
                    }
                    let content = responses
                        .iter()
                        .map(|r| {
                            let label = items
                                .iter()
                                .find(|it| it.id == r.item_id)
                                .map(|it| it.label.as_str())
                                .filter(|s| !s.is_empty())
                                .unwrap_or(&r.item_id);
                            format!("{}：{}", label, r.value)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Some(ConfirmationReply::Structured(content));
                }
                Some(RoomCommand::Send { sender, content, recipients, reply_to, mention, attachments }) => {
                    return Some(ConfirmationReply::Open {
                        sender,
                        content,
                        recipients,
                        reply_to,
                        mention,
                        attachments,
                    });
                }
                Some(RoomCommand::SetDirector { director_id }) => self.set_director(&director_id).await,
                Some(RoomCommand::ParticipantChanged) => self.handle_participant_changed().await,
                Some(RoomCommand::Pause) => self.set_status(RoomStatus::Paused).await,
                Some(RoomCommand::Resume) => self.set_status(RoomStatus::Running).await,
                Some(RoomCommand::Abort) => return None,
                None => return None,
            }
        }
    }

    /// 用户已回复确认：把回复合并进原 pending 任务的描述，并将状态恢复为 discussing 以便重跑。
    /// 不再把回复当作新指令重新编排，避免无限追加子任务。
    async fn apply_confirmation_reply(&mut self, task_id: &str, reply: &str) {
        if let Ok(conn) = self.conn() {
            if let Ok(Some(mut t)) = store::get_task(&conn, &self.room_id, task_id) {
                // 仅当任务仍处于 pending（等待确认）时才注入并重跑；否则保持不变。
                if t.status == "pending" {
                    t.description = format!("{}\n\n【用户补充/确认信息】\n{}", t.description.trim_end(), reply);
                    t.status = "discussing".into();
                    t.error = None;
                    t.started_at = None;
                    t.completed_at = None;
                    let _ = store::update_task(&conn, &t);
                    self.emit(GroupChatEvent::task_updated(&self.room_id, t));
                }
            }
        }
    }

    /// 单个任务的完整执行（含重试/重派），作为关联函数以便并发 spawn。
    /// 不写库、不发事件，只返回最终任务状态与需要落库的裁决消息。
    #[allow(clippy::too_many_arguments)]
    async fn execute_single_task(
        mut task: TaskRow,
        initial_assignee: Option<String>,
        dep_blocked: bool,
        participants: HashMap<String, Arc<dyn Participant>>,
        director: Option<Arc<Director>>,
        topic: String,
        executor_order: Vec<String>,
        app: tauri::AppHandle,
        room_id: String,
        _control: Control,
        summary: String,
        director_sender: String,
        retry_max: usize,
        reassign_max: usize,
        base_backoff_ms: u64,
    ) -> TaskOutcome {
        let mut messages: Vec<PendingMessage> = Vec::new();
        let mut should_abort = false;

        if dep_blocked {
            task.status = "skipped".into();
            task.error = Some("前置任务失败或未完成".into());
            task.completed_at = Some(crate::utils::now());
            return TaskOutcome { task, messages, should_abort, tool_calls: String::new(), confirmation: None, confirmation_sender: None };
        }

        let Some(mut current_assignee) = initial_assignee else {
            task.status = "skipped".into();
            task.error = Some("无可用执行者".into());
            task.completed_at = Some(crate::utils::now());
            return TaskOutcome { task, messages, should_abort, tool_calls: String::new(), confirmation: None, confirmation_sender: None };
        };

        let mut retries = 0usize;
        let mut reassigns = 0usize;
        let mut last_error: Option<String> = None;
        let mut last_tool_calls = String::new();
        // 上次尝试已完成工作的摘要（由工具调用链生成），重试时注入续接提示，避免从头执行。
        let mut last_work_hint: Option<String> = None;
        let mut confirmation: Option<ConfirmationRequest> = None;
        let mut confirmation_sender: Option<String> = None;

        loop {
            let Some(participant) = participants.get(&current_assignee).cloned() else {
                task.status = "failed".into();
                task.error = Some(format!("执行者不存在: {}", current_assignee));
                break;
            };

            let result = Self::run_task_once(
                participant,
                task.description.clone(),
                summary.clone(),
                app.clone(),
                room_id.clone(),
                current_assignee.clone(),
                last_error.clone(),
                last_work_hint.clone(),
            )
            .await;

            last_tool_calls = result.tool_calls.clone();

            // 执行者主动发起确认：回填 task_id，任务转 pending，等待用户回复后重跑。
            if let Some(mut conf) = result.confirmation {
                conf.task_id = task.id.clone();
                confirmation = Some(conf);
                confirmation_sender = Some(current_assignee.clone());
                task.assignee = Some(current_assignee.clone());
                task.status = "pending".into();
                task.error = Some("执行者需用户确认".into());
                break;
            }

            // 失败判定：有错误，或既无内容又无工具调用链（工具已产生实质工作的空文本不判失败）。
            let failed = result.error.is_some() || (result.content.is_empty() && result.tool_calls.is_empty());
            if !failed {
                task.status = "success".into();
                task.result_summary = Some(if result.content.is_empty() {
                    // 工具已执行但无最终文本：用工具调用链摘要作为成果。
                    work_hint_from_tool_calls(&result.tool_calls)
                        .unwrap_or_else(|| "已执行工具调用（无文本结论）".to_string())
                } else {
                    result.content.chars().take(300).collect()
                });
                task.error = None;
                break;
            }

            let err = result.error.unwrap_or_else(|| "空输出".to_string());
            last_error = Some(err.clone());
            // 重试时携带上次已完成的工作摘要（仅在确有工具记录时注入）。
            last_work_hint = work_hint_from_tool_calls(&last_tool_calls);

            // 1) 原地重试（指数退避）
            if retries < retry_max {
                retries += 1;
                let shift = retries.min(6) as u32;
                let backoff = base_backoff_ms * (1u64 << shift);
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                continue;
            }

            // 2) Director 兜底重派
            if reassigns < reassign_max {
                if let Some(director) = &director {
                    let decision = director
                        .handle_task_failure(&topic, &task.id, &task.description, &err, &executor_order)
                        .await;
                    if let Some(d) = decision {
                        let action_desc = match d.action.as_str() {
                            "retry" => "重试",
                            "reassign" => "重派",
                            "skip" => "跳过",
                            "abort" => "终止",
                            "ask_user" => "向用户确认",
                            _ => d.action.as_str(),
                        };
                        let assignee_desc = d.assignee.clone().unwrap_or_else(|| "-".into());
                        let reason = if d.reason.is_empty() { "未说明".to_string() } else { d.reason.clone() };
                        let text = format!(
                            "失败裁决：任务「{}」\n失败原因：{}\n处置：{}（目标：{}）\n理由：{}",
                            task.description, err, action_desc, assignee_desc, reason
                        );
                        messages.push(PendingMessage {
                            sender: director_sender.clone(),
                            kind: "failure_decision".into(),
                            content: text,
                        });

                        match d.action.as_str() {
                            "retry" => {
                                // 与 reassign 一样计入 Director 兜底介入次数，避免反复 retry 导致死循环
                                // （否则 execute_single_task 永不返回，插队检查点永远无法到达）。
                                reassigns += 1;
                                retries = 0;
                                continue;
                            }
                            "reassign" => {
                                if let Some(new_id) = d.assignee.clone().filter(|id| {
                                    participants.contains_key(id) && *id != current_assignee
                                }) {
                                    current_assignee = new_id;
                                    task.assignee = Some(current_assignee.clone());
                                    reassigns += 1;
                                    retries = 0;
                                    continue;
                                }
                            }
                            "abort" => {
                                should_abort = true;
                                task.status = "failed".into();
                                task.error = Some(format!("{}（Director 裁决终止）", err));
                                break;
                            }
                            "ask_user" => {
                                if let Some(conf) = d.confirmation {
                                    confirmation = Some(conf);
                                    confirmation_sender = Some(director_sender.clone());
                                    task.status = "pending".into();
                                    task.error = Some(format!("需用户确认：{}", reason));
                                    break;
                                }
                                // 未携带确认请求时退化为跳过。
                            }
                            _ => {}
                        }
                    }
                }
            }

            // 3) 最终失败
            task.status = "failed".into();
            task.error = Some(err);
            break;
        }

        if confirmation.is_none() {
            task.completed_at = Some(crate::utils::now());
        }
        TaskOutcome { task, messages, should_abort, tool_calls: last_tool_calls, confirmation, confirmation_sender }
    }

    /// 执行单次任务尝试（关联函数，避免跨 await 借用 self 导致 future 非 Send）。
    ///
    /// `work_hint` 为上次尝试已完成工作的摘要（工具调用记录），重试/续接时注入提示，
    /// 避免二次执行时从头再来。
    async fn run_task_once(
        participant: Arc<dyn Participant>,
        task_desc: String,
        summary: String,
        app: tauri::AppHandle,
        room_id: String,
        speaker_id: String,
        error_hint: Option<String>,
        work_hint: Option<String>,
    ) -> TurnResult {
        let mut prompt = format!("请完成以下任务并给出结果：{}", task_desc);
        if let Some(work) = work_hint {
            prompt.push_str(&format!(
                "\n\n（注意：本次为续接执行，你此前已完成了以下工作：\n{}\n请基于这些已完成的工作继续推进，不要从头重复执行。）",
                work
            ));
        }
        if let Some(hint) = error_hint {
            prompt.push_str(&format!("\n\n（上次执行失败原因：{}，请修正后重试。）", hint));
        }
        let view = TurnView {
            topic: task_desc,
            summary,
            stances: vec![],
            messages: vec![ChatMessage::user(&prompt)],
            system_role: "执行者".into(),
            task_context: String::new(),
        };
        let delta_fn = move |delta: &str| {
            let _ = app.emit(
                "groupchat-event",
                &GroupChatEvent::token_stream(&room_id, &speaker_id, delta),
            );
        };
        participant.run_turn(view, Some(Arc::new(delta_fn))).await
    }

    fn save_and_emit_task(&self, task: &TaskRow) {
        if let Ok(conn) = self.conn() {
            let _ = store::update_task(&conn, task);
        }
        self.emit(GroupChatEvent::task_updated(&self.room_id, task.clone()));
    }

    async fn persist_task_message(&mut self, task: &TaskRow, tool_calls: &str) {
        let sender = task.assignee.clone().unwrap_or_else(|| self.user_id.clone());
        let task_result = report::to_task_result(task);
        let msg = self.persist_message(&sender, &[], "task_result", None, &task_result, &[], tool_calls).await;
        self.memory.apply_message(&msg, &self.all_participant_ids());
        self.emit(GroupChatEvent::message(&self.room_id, msg));
    }

    /// 将 Director 工具授权裁决从共享缓冲聚合落库为一条消息（在参与者 turn 结束后调用）。
    /// 每个发言者从开始发言到结束为一个聚合单元（flush 在每次 turn 结束后调用），
    /// 下一位发言者的裁决重新累积，避免每条裁决单独成消息造成消息流噪声。
    async fn flush_approval_logs(&mut self) {
        let records = self
            .approval_log
            .lock()
            .map(|mut log| std::mem::take(&mut *log))
            .unwrap_or_default();
        if records.is_empty() {
            return;
        }
        let director_sender = self.director_sender();
        let mut text = format!("本轮工具裁决（{} 项）：\n", records.len());
        for r in &records {
            text.push_str(&format!(
                "- {}（{}）→ {}{}\n",
                r.tool_name,
                r.risk,
                if r.allow { "放行" } else { "拒绝" },
                if r.reason.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", r.reason)
                }
            ));
        }
        let msg = self
            .persist_message(&director_sender, &[], "tool_decision", None, &text, &[], "")
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, msg));
    }

    /// 构建 Director 审阅用讨论记录：保留最近 `limit` 条完整消息（结论轮使用更大窗口），
    /// 更早消息不再全量注入（由 L1 摘要承载），避免长上下文注意力稀释与 token 成本线性增长。
    fn build_review_transcript(&self, limit: usize) -> String {
        let all = self.memory.all_messages();
        let start = all.len().saturating_sub(limit);
        let mut out = String::new();
        if start > 0 {
            out.push_str(&format!("（{} 条更早的讨论已浓缩进已有进度摘要）\n", start));
        }
        for m in &all[start..] {
            out.push_str(&format!("{}: {}", m.sender, m.content));
            out.push('\n');
        }
        out
    }

    /// 构建任务执行清单文本（编号、状态、描述、负责人、结果/错误），供 Director 选人/总结复用。
    fn build_task_manifest(&self) -> String {
        self.conn()
            .ok()
            .map(|c| store::list_tasks(&c, &self.room_id).unwrap_or_default())
            .unwrap_or_default()
            .iter()
            .map(|t| {
                let assignee = t.assignee.clone().unwrap_or_else(|| "-".into());
                let detail = if t.status == "success" {
                    t.result_summary.clone().unwrap_or_default()
                } else {
                    t.error.clone().unwrap_or_default()
                };
                format!(
                    "T{}. [{}] {}（负责人：{}）{}",
                    t.task_no,
                    t.status,
                    t.description,
                    assignee,
                    if detail.is_empty() { String::new() } else { format!(" - {}", detail) }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 构建当前发言者被指派的任务上下文（讨论阶段注入 participant 视图，明确任务锚点）。
    fn build_speaker_task_context(&self, speaker: &str) -> String {
        let Ok(conn) = self.conn() else { return String::new(); };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else { return String::new(); };
        let assigned: Vec<String> = tasks
            .iter()
            .filter(|t| {
                t.assignee.as_deref() == Some(speaker)
                    && matches!(t.status.as_str(), "discussing" | "pending" | "running")
            })
            .map(|t| format!("- [{}] {}", t.status, t.description))
            .collect();
        if assigned.is_empty() {
            String::new()
        } else {
            format!(
                "你被指派了以下任务，请围绕它们发言与推进（忽略与此无关的文件内容或话题）：\n{}",
                assigned.join("\n")
            )
        }
    }

    /// 执行结果回流 L1 摘要：执行结束后把任务执行成果压缩进摘要，补异步并行反馈缺口。
    async fn refresh_summary(&mut self) {
        let director = match self.director.clone() {
            Some(d) => d,
            None => return,
        };
        let transcript = self.build_review_transcript(30);
        let task_manifest = self.build_task_manifest();
        let Some(review) = director
            .director_review(
                &self.room.topic,
                self.memory.summary(),
                &transcript,
                &self.room.goal_notes,
                &task_manifest,
            )
            .await
        else {
            return;
        };
        if review.summary.is_empty() {
            return;
        }
        self.memory.set_summary(&review.summary);
        let director_sender = self.director_sender();
        let summary_msg = self
            .persist_message(&director_sender, &[], "summary", None, &review.summary, &[], "")
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, summary_msg));
    }

    async fn conclude(&mut self) -> String {
        // 结论轮使用更大的讨论窗口（60 条），确保最终结论基于充分的参与者发言与成果材料
        let transcript = self.build_review_transcript(60);

        // 任务执行清单：供主席在结论中区分已完成/未完成/失败
        let task_manifest = self.build_task_manifest();

        let conclusion = match &self.director {
            Some(d) => d
                .conclude(&self.room.topic, self.memory.summary(), &transcript, &task_manifest)
                .await,
            None => None,
        };

        conclusion.unwrap_or_else(|| {
            if self.memory.summary().is_empty() {
                format!("讨论结束，共 {} 轮。", self.state.round)
            } else {
                self.memory.summary().to_string()
            }
        })
    }
}
