//! 房间运行时（RoomRuntime Actor）+ 房间注册表（RoomRegistry）。

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as AsyncMutex, Notify};

use crate::agent::AgentManager;
use crate::api_agent::agent_loop::{ApprovalHandler, ContinueHandler, RiskLevel, SecurityMode};
use crate::api_agent::types::ApiFormat;
use crate::db::init::DbPool;
use crate::db::models::Attachment;
use crate::utils::errors::AppError;
use tauri::Emitter;

use super::adapter::tools::RoomMcpAssets;
use super::adapter::{cli::PilotDeskCliRunner, llm::PilotDeskLlmClient};
use super::director::{
    Director, KickoffPlan, NewParticipantDraft, ReplanOperation, SpeakerDecision,
};
use super::event::GroupChatEvent;
use super::floor::FloorManager;
use super::memory::LayeredMemory;
use super::models::{
    ConfirmationItem, ConfirmationRequest, MessageRow, ParticipantRow, Room, TaskRow,
};
use super::participant::{
    resolve_attitude, Attitude, ChatMessage, CliConfig, CliParticipant, LlmClient, LlmParticipant,
    Participant, TurnResult, TurnView, UserParticipant,
};
use super::rules::RuleEngine;
use super::state::{RoomStateMachine, RoomStatus};
use super::{report, store, task};

// ── Director 失活熔断（主持人 LLM 失败后的恢复流程）──

/// Director 单次 LLM 决策调用的确定性超时（超时视为失败走兜底，不依赖 reqwest 300s）。
const DIRECTOR_CALL_TIMEOUT_SECS: u64 = 90;
/// 主持人单次决策调用失败/超时后的重试次数（同一主持人，避免瞬时抖动误判失活）。
/// 重试仍全部失败才计入连续失败；总尝试上限 = 1 + DIRECTOR_RETRY_LIMIT。
const DIRECTOR_RETRY_LIMIT: u32 = 2;
/// 参与者单轮 turn 的确定性超时（含工具循环；超时视为该轮发言失败）。
const TURN_TIMEOUT_SECS: u64 = 300;
/// 单个任务执行的总硬上限（v3.5c R1）：覆盖全部重试/重派/恢复尝试，超过即按"执行超时"失败并
/// 中断该任务（防执行层任何漏网步骤把任务永久卡在 running，拖住同波次与后继依赖层）。
const EXEC_TASK_TIMEOUT_SECS: u64 = 900;
/// 熔断阈值：连续 director_review 失败（已含重试耗尽）达到该值进入主持人恢复流程。
const DIRECTOR_FAIL_THRESHOLD: u32 = 3;
/// 恢复流程尝试的现有 API 参与者数（每人 1 次决策机会，创建新主持人；非轮询，超限即诚实收敛结束）。
const DIRECTOR_RECOVER_LIMIT: usize = 2;
/// 失败裁决连续返回 retry 达到该阈值后，由代码强制更换执行者（不依赖 LLM 自觉）。
const RETRY_FORCE_LIMIT: u32 = 2;
/// replan（讨论中补角色）单次补充的最大数量（kickoff 初始批量不设单次上限，仅受房间容量约束）。
const MAX_AUTO_NEW_PARTICIPANTS_REPLAN: usize = 2;
/// 阶段A 确认门单次入室最多轮次（v3.5c 自适应消化：每轮回复消化后仅当仍缺关键信息才追问，防死循环）。
const MAX_CLARIFY_ROUNDS: usize = 3;
/// 房间内 api/cli 参与者总数上限（按需组建名册时校验，防成本失控）。
/// 房间总席位 = api/cli 上限 + 用户(1) + 主持人(1) = 15（V3.5bp：与用户确认的
/// 「含用户和主持的总席位 15」对齐）。
const MAX_API_CLI_PARTICIPANTS: usize = 13;

/// 给 Director 的异步 LLM 调用加确定性超时：超时按失败（None）处理。
async fn with_director_timeout<T>(fut: impl Future<Output = Option<T>>) -> Option<T> {
    match tokio::time::timeout(Duration::from_secs(DIRECTOR_CALL_TIMEOUT_SECS), fut).await {
        Ok(v) => v,
        Err(_) => {
            log::warn!(
                "[GroupChat] Director 调用超时（{}s），按失败处理",
                DIRECTOR_CALL_TIMEOUT_SECS
            );
            None
        }
    }
}

/// 带重试的 Director 决策调用：单次调用经 `with_director_timeout` 失败/超时后，
/// 对同一主持人按退避重试（≤DIRECTOR_RETRY_LIMIT 次），全部失败才返回 None（进入熔断计数）。
/// 避免一次瞬时抖动（网络抖动/暂态错误）就被误判为失活而误走恢复流程。
async fn call_director_retry<T, Fut>(mut make_call: impl FnMut() -> Fut) -> Option<T>
where
    Fut: Future<Output = Option<T>>,
{
    let mut attempt: u32 = 0;
    loop {
        if let Some(v) = with_director_timeout(make_call()).await {
            return Some(v);
        }
        attempt += 1;
        if attempt >= DIRECTOR_RETRY_LIMIT {
            return None;
        }
        // 退避等待：第 1 次失败等 2s，第 2 次失败等 4s。
        tokio::time::sleep(Duration::from_secs(u64::from(attempt) * 2)).await;
    }
}

/// 格式化可用模型清单（provider / 模型 / 备注），供 Director 提示词选模型；仅含模型名与备注，不含 key。
fn format_models_catalog_db(conn: &rusqlite::Connection) -> String {
    let infos = crate::commands::api_provider::collect_provider_models(conn);
    if infos.is_empty() {
        return "（无可用模型）".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    for info in &infos {
        for m in &info.models {
            let note = m.description.as_deref().unwrap_or("").trim();
            let note = if note.is_empty() { "无备注" } else { note };
            lines.push(format!(
                "- provider={} | 模型: {} | 备注: {}",
                info.provider_id, m.name, note
            ));
        }
    }
    lines.join("\n")
}

/// 格式化已注册 CLI Agent 清单（agent_type / 显示名 / 描述），供 Director 选 cli 参与者。
/// 白名单来源（禁止编造 agent_type）；无已注册 agent 时返回空字符串（调用方省略该段）。
fn format_agents_catalog_db(conn: &rusqlite::Connection) -> String {
    let Ok(agents) = crate::commands::agents::list_agents_inner(conn) else {
        return String::new();
    };
    if agents.is_empty() {
        return String::new();
    }
    agents
        .iter()
        .map(|a| {
            let desc = a.description.trim();
            let desc = if desc.is_empty() { "无描述" } else { desc };
            format!(
                "- agent_type={} | 显示名: {} | 描述: {}",
                a.agent_type, a.display_name, desc
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 合并模型清单 + CLI Agent 清单（各带标题；无内容的分段省略），供 Director 按需组建参与者。
/// `allow_cli`=false 时省略 CLI Agent 段（房间级开关：按任务差异化限制主持人自动补 CLI）。
fn format_roster_catalog_db(conn: &rusqlite::Connection, allow_cli: bool) -> String {
    let models = format_models_catalog_db(conn);
    let mut sections: Vec<String> = Vec::new();
    sections.push("【可用模型清单（provider / 模型 / 备注）】".to_string());
    sections.push(models);
    if allow_cli {
        let agents = format_agents_catalog_db(conn);
        if !agents.is_empty() {
            sections.push(String::new());
            sections.push("【可用 CLI Agent 清单（agent_type / 显示名 / 描述）】".to_string());
            sections.push(agents);
        }
    }
    sections.join("\n")
}

/// 房间剩余可自动新增的参与者空位（房间内 api/cli 总数上限扣减现有数，最小 0）。
fn remaining_participant_slots_db(conn: &rusqlite::Connection, room_id: &str) -> usize {
    let count = store::list_participants(conn, room_id)
        .map(|rows| {
            rows.iter()
                .filter(|p| p.participant_type == "api" || p.participant_type == "cli")
                .count()
        })
        .unwrap_or(0);
    MAX_API_CLI_PARTICIPANTS.saturating_sub(count)
}

/// V3.5bp 确认回复摘要化：确认门通过后的兜底路径（Director 二次 clarify 失败/无主持人）中，
/// 不再把用户确认回复原文全文拼入目标锚点——只摘录可读要点（单行化 + 截断 120 字），
/// 避免口语/冗余原文污染目标与后续子任务描述。成功路径由 Director 消化整理，不经过此函数。
pub(crate) fn summarize_for_goal(reply_content: &str) -> String {
    const MAX_GOAL_REPLY_CHARS: usize = 120;
    let text = reply_content.trim();
    if text.is_empty() {
        return String::new();
    }
    // 单行化：折叠连续空白，避免换行/缩进把口语回复撑爆目标锚点。
    let single_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= MAX_GOAL_REPLY_CHARS {
        return single_line;
    }
    let cut: String = single_line.chars().take(MAX_GOAL_REPLY_CHARS).collect();
    format!("{}…", cut)
}

/// 记录一次参与者失败（讨论发言/任务执行均计入；每个参与者保留最近 5 条）。
/// 供 Director 感知反复失败：反复失败者不再被指派任务/发言，避免空转循环。
fn record_participant_failure_shared(
    map: &Arc<Mutex<HashMap<String, Vec<(i64, String)>>>>,
    participant_id: &str,
    reason: &str,
) {
    if let Ok(mut m) = map.lock() {
        let entry = m.entry(participant_id.to_string()).or_default();
        entry.push((crate::utils::now(), reason.to_string()));
        if entry.len() > 5 {
            entry.drain(..entry.len() - 5);
        }
    }
}

/// 判断参与者是否为"慢性失败者"（失败记录 ≥ 2 次）：运行时硬过滤，不依赖 LLM 自觉。
fn is_chronic_failer_map(
    map: &Arc<Mutex<HashMap<String, Vec<(i64, String)>>>>,
    participant_id: &str,
) -> bool {
    map.lock()
        .map(|m| m.get(participant_id).map(|f| f.len() >= 2).unwrap_or(false))
        .unwrap_or(false)
}

/// 校验并落库自动补充的参与者（api: agent_config={provider,model}；cli: agent_config={agent_type}）：
/// 校验 id 格式/唯一性、人数上限；api 额外校验 provider 存在且已配 key、model 命中该 provider 的模型清单；
/// cli 额外校验 agent_type 已注册（白名单）且同一房间同一 agent_type 只允许 1 个（去重）；
/// `allow_cli`=false 时拒绝一切 cli 草稿（房间级开关：按任务差异化限制主持人自动补 CLI）。
/// 不发送事件（由调用方决定）。返回 (创建成功 id, 拒绝原因)。
fn create_auto_participants_db(
    conn: &rusqlite::Connection,
    room_id: &str,
    drafts: &[NewParticipantDraft],
    allow_cli: bool,
    max_new: usize,
) -> (Vec<String>, Vec<String>) {
    let mut created: Vec<String> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    let existing = store::list_participants(conn, room_id).unwrap_or_default();
    let mut existing_ids: HashSet<String> = existing.iter().map(|p| p.id.clone()).collect();
    let api_cli_count = existing
        .iter()
        .filter(|p| p.participant_type == "api" || p.participant_type == "cli")
        .count();
    // 已占用 cli agent_type（同一房间去重：同 agent_type 只允许 1 个）。
    let mut cli_types: HashSet<String> = existing
        .iter()
        .filter(|p| p.participant_type == "cli")
        .filter_map(|p| {
            serde_json::from_str::<serde_json::Value>(&p.agent_config)
                .ok()
                .and_then(|v| v["agent_type"].as_str().map(|s| s.to_string()))
        })
        .collect();

    for d in drafts {
        if created.len() >= max_new {
            rejected.push(format!("本次新增参与者超过单次上限（{}），已截断", max_new));
            break;
        }
        if api_cli_count + created.len() >= MAX_API_CLI_PARTICIPANTS {
            rejected.push(format!(
                "参与者总数已达上限（{}），无法继续新增",
                MAX_API_CLI_PARTICIPANTS
            ));
            break;
        }
        let id = d.id.trim();
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            rejected.push(format!(
                "参与者 id '{}' 非法（仅允许字母/数字/下划线/连字符）",
                id
            ));
            continue;
        }
        if existing_ids.contains(id) {
            rejected.push(format!("参与者 id '{}' 已存在，已跳过", id));
            continue;
        }
        if d.participant_type == "cli" {
            // 房间级开关：本房间禁止主持人自动补充 CLI 参与者时，拒绝一切 cli 草稿。
            if !allow_cli {
                rejected.push(format!(
                    "本房间已关闭主持人自动补充 CLI 参与者，[@{}] 已跳过",
                    id
                ));
                continue;
            }
            // cli：白名单校验（已注册 agent 才允许）+ 同房间同 agent_type 去重。
            let agent_type = d.agent_type.trim();
            if agent_type.is_empty() {
                rejected.push(format!(
                    "参与者 '{}' 缺少 agent_type（cli 类型必填），已跳过",
                    id
                ));
                continue;
            }
            let Ok(Some(_agent)) = crate::commands::agents::get_agent_inner(conn, agent_type)
            else {
                rejected.push(format!(
                    "CLI Agent '{}' 未注册，已跳过（请从清单逐字复制 agent_type）",
                    agent_type
                ));
                continue;
            };
            if cli_types.contains(agent_type) {
                rejected.push(format!(
                    "同一房间已存在 CLI Agent '{}'（同 agent_type 只允许 1 个），已跳过",
                    agent_type
                ));
                continue;
            }
            let agent_config = serde_json::json!({ "agent_type": agent_type }).to_string();
            let row = ParticipantRow {
                id: id.to_string(),
                room_id: room_id.to_string(),
                participant_type: "cli".into(),
                agent_config,
                display_name: d.display_name.clone(),
                system_role: d.system_role.clone(),
                status: "active".into(),
            };
            if store::insert_participant(conn, &row).is_ok() {
                existing_ids.insert(id.to_string());
                cli_types.insert(agent_type.to_string());
                created.push(id.to_string());
            } else {
                rejected.push(format!("参与者 '{}' 落库失败，已跳过", id));
            }
            continue;
        }
        // api：provider 存在且有 key、model 命中白名单。
        let Ok(Some(provider)) = crate::commands::api_provider::get_api_provider(conn, &d.provider)
        else {
            rejected.push(format!("提供商 '{}' 不存在或未配置，已跳过", d.provider));
            continue;
        };
        // 模型白名单：provider.models 为空时跳过校验（与 generate_image 的宽容语义一致），
        // 非空时模型必须命中清单，防止 Director 编造/截断模型名。
        if !provider.models.is_empty() && !provider.models.iter().any(|m| m == &d.model) {
            rejected.push(format!(
                "模型 '{}' 不在提供商 [@{}] 的模型清单中，已跳过（请从清单逐字复制）",
                d.model, d.provider
            ));
            continue;
        }
        let key_ok = crate::commands::api_provider::get_api_key(conn, &d.provider)
            .ok()
            .flatten()
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        if !key_ok {
            rejected.push(format!("提供商 [@{}] 未配置 API Key，已跳过", d.provider));
            continue;
        }
        let agent_config =
            serde_json::json!({ "provider": d.provider, "model": d.model }).to_string();
        let row = ParticipantRow {
            id: id.to_string(),
            room_id: room_id.to_string(),
            participant_type: "api".into(),
            agent_config,
            display_name: d.display_name.clone(),
            system_role: d.system_role.clone(),
            status: "active".into(),
        };
        if store::insert_participant(conn, &row).is_ok() {
            existing_ids.insert(id.to_string());
            created.push(id.to_string());
        } else {
            rejected.push(format!("参与者 '{}' 落库失败，已跳过", id));
        }
    }
    (created, rejected)
}

/// Director 失活恢复流程状态（主持人 LLM 失败熔断后的恢复）。
/// 与旧三级兜底的区别：不轮流让现有参与者接管主持人身份（其角色定义保持不变），
/// 也不轮询兜底（无主持人即无法调度，轮询不科学）。
/// 恢复路径唯一：达到熔断阈值后，由现有 API 参与者决策创建一名新主持人并热替换，
/// 成功即继续收敛，失败即诚实收敛结束。
#[derive(Default)]
struct DirectorFallback {
    /// 连续 director_review 失败次数（≥DIRECTOR_FAIL_THRESHOLD 触发主持人恢复流程）。
    failures: u32,
}

impl DirectorFallback {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

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
/// camelCase 序列化（与前端 GroupChatConfirmationResponseInput 对齐），供 extra 落库恢复控件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmationResponse {
    pub item_id: String,
    pub value: String,
}

/// 用户对确认请求的回复分类（确认等待期消费）。
enum ConfirmationReply {
    /// 结构化确认回复（必然是对确认项的答复）；携带原始 responses 供结构化落库（恢复控件用）。
    Structured {
        content: String,
        responses: Vec<ConfirmationResponse>,
    },
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
        /// 会话安全模式（本波次有效，不持久化；None 默认标准）
        security_mode: Option<SecurityMode>,
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
    /// 房间统一产物目录变更（命令层已落库，运行时同步字段使后续上下文生效）。
    SetOutputDir(String),
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
    /// 目标范围扩展（amend_goal），携带追加到目标锚点的补充文本（不归档任务、不重置记忆，走对账重排）。
    Amended(String),
    /// 临时性一次性额外任务（temp_task），不更新目标锚点与约束，直接插入任务执行。
    TempTask {
        description: String,
        assignee: Option<String>,
    },
    /// 更换主持人（switch_director）：结构身份变更，走 set_director 热替换 + 全量重排。
    SwitchDirector(String),
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
    /// 投递命令；返回是否成功（false = receiver 已关闭，Actor 已退出）。
    pub fn send(&self, cmd: RoomCommand) -> bool {
        self.cmd_tx.send(cmd).is_ok()
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
        Self {
            rooms: std::sync::Mutex::new(HashMap::new()),
        }
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
        let participant_failures: Arc<Mutex<HashMap<String, Vec<(i64, String)>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let control = Control::new();
        let handle = RoomHandle {
            cmd_tx,
            control: control.clone(),
        };
        // 协作式取消令牌：与房间 control.abort 共享，参与者工具循环在停止/暂停时迭代边界提前结束。
        let built = build_participants(
            &conn,
            &pool,
            &room,
            app.clone(),
            approval_log.clone(),
            control.abort.clone(),
            None,
            None,
        )?;

        // 应用重启后懒恢复：读取 DB 真实状态与轮次，供 run() 判断是否续跑未完成任务。
        let room_status = RoomStatus::from_str(&room.status);
        let round = store::max_message_round(&conn, room_id);
        let topic = room.topic.clone();

        // 房间产物目录（v3.4ay）：显式指定优先；空则先以 `<工作目录>/outputs/<房间标题>/` 兜底，
        // 待目标理解阶段由 resolve_and_persist_output_dir 按主题目录名确定、创建并落库。
        let output_dir = if room.output_dir.trim().is_empty() {
            let cwd = crate::utils::paths::resolve_workspace_path(None, "", &conn)
                .to_string_lossy()
                .to_string();
            let title = sanitize_room_title(&room.title);
            format!("{}\\outputs\\{}", cwd.trim_end_matches(['\\', '/']), title)
        } else {
            room.output_dir.clone()
        };

        let runtime = RoomRuntime {
            room_id: room_id.to_string(),
            app,
            pool,
            cmd_rx,
            control,
            room,
            participants: built.participants,
            director: built.director,
            shared_director: built.shared_director,
            director_fallback: DirectorFallback::default(),
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
            last_user_directive: String::new(),
            // MCP 资产在 run() 启动时异步构建（spawn 为同步入口，无法 await）。
            mcp: None,
            // 会话安全模式：每次 Send 时注入（默认标准）。
            security_mode: None,
            participant_failures,
            input_refs: Vec::new(),
            output_dir,
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

    /// 投递命令并确保 Actor 存活（自愈残留句柄）：句柄缺失→spawn；存在但 send 失败
    ///（Actor 异常退出/panic 后 receiver 已关闭、句柄残留）→ 移除并重建后重投一次。
    /// `make` 闭包每次调用生成一条新命令（RoomCommand 不实现 Clone，字段由闭包内克隆重建）。
    pub fn send_resilient<F>(
        &self,
        pool: DbPool,
        app: tauri::AppHandle,
        room_id: &str,
        make: F,
    ) -> Result<(), AppError>
    where
        F: Fn() -> RoomCommand,
    {
        for attempt in 0..2 {
            let handle = self.get_or_spawn(pool.clone(), app.clone(), room_id)?;
            if handle.send(make()) {
                return Ok(());
            }
            log::warn!(
                "[GroupChat] 房间 {} Actor 句柄失效（投递失败），移除并重建…",
                room_id
            );
            self.remove(room_id);
            if attempt == 1 {
                return Err(AppError::External(format!(
                    "房间 {} Actor 无法启动/投递",
                    room_id
                )));
            }
        }
        Ok(())
    }

    pub fn remove(&self, room_id: &str) {
        self.rooms.lock().unwrap().remove(room_id);
    }
}

/// 从 DB 构建参与者与 Director。
struct BuiltParticipants {
    participants: HashMap<String, Arc<dyn Participant>>,
    director: Option<Arc<Director>>,
    /// 参与者工具审批/续跑裁决读取的共享主持人引用（Director 热切换后自动跟随）。
    shared_director: Arc<RwLock<Option<Arc<Director>>>>,
    director_id: Option<String>,
    floor_order: Vec<String>,
    executor_order: Vec<String>,
    user_id: String,
}

/// 解析端宽容归一化：把 LLM 输出中可能带 `[@...]` 括号、`@` 前缀或空白的参与者引用还原为裸 id。
/// 即使提示词要求输出裸 id，模型仍可能回显带括号的引用；匹配前统一剥离，避免静默失败（重派/选人被吞）。
fn normalize_participant_ref(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix("[@") {
        s = rest.strip_suffix(']').unwrap_or(rest).trim();
    } else if let Some(rest) = s.strip_prefix('@') {
        s = rest.trim();
    }
    s.to_string()
}

/// 协调类角色判定：主持人/主席/协调者等协调身份是「结构身份」（director 参与者）专属语义，
/// 禁止作为语义角色赋给普通参与者，避免出现「双重主持人」导致的归因错乱。
fn is_coordinator_role(role: &str) -> bool {
    let r = role.trim();
    !r.is_empty()
        && (r.contains("主持人")
            || r.contains("主席")
            || r.contains("协调者")
            || r.contains("协调员")
            || r.contains("协调/裁决")
            || r.eq_ignore_ascii_case("coordinator")
            || r.eq_ignore_ascii_case("director")
            || r.eq_ignore_ascii_case("host")
            || r.eq_ignore_ascii_case("moderator"))
}

fn build_participants(
    conn: &rusqlite::Connection,
    pool: &DbPool,
    room: &Room,
    app: tauri::AppHandle,
    approval_log: Arc<Mutex<Vec<ApprovalRecord>>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    mcp: Option<Arc<RoomMcpAssets>>,
    security_mode: Option<SecurityMode>,
) -> Result<BuiltParticipants, AppError> {
    let rows = store::list_participants(conn, &room.id)?;
    // 群聊作用域技能禁用集（构造时读一次的快照，随后 rebuild_participants 时刷新）：
    // 只从 `<available_skills>` 目录隐藏，load_skill 仍可点名加载被隐藏技能。
    let disabled_skills = crate::commands::app_settings::load_skill_scope_disabled(conn).groupchat;
    // 房间工作区根 = 产物目录（用户选定项目/工作空间根）；未选时回退全局工作空间。
    let cwd = if room.output_dir.trim().is_empty() {
        crate::utils::paths::resolve_workspace_path(None, "", conn)
            .to_string_lossy()
            .to_string()
    } else {
        room.output_dir.clone()
    };

    let mut participants: HashMap<String, Arc<dyn Participant>> = HashMap::new();
    let mut director: Option<Arc<Director>> = None;
    let mut floor_order: Vec<String> = Vec::new();
    let mut executor_order: Vec<String> = Vec::new();
    let mut user_id = String::from("user");
    let director_id = room.director_id.clone();

    // 先构造 Director：优先 participant_type='director' 行；缺失时按 room.director_id 指向的
    // 参与者配置兜底（热替换/历史数据可能把 director_id 指向普通参与者），避免重启后协调者丢失。
    let director_row = rows
        .iter()
        .find(|r| r.participant_type == "director")
        .or_else(|| {
            rows.iter()
                .find(|r| Some(r.id.as_str()) == director_id.as_deref())
        });
    if let Some(row) = director_row {
        if let Some(llm) = build_llm_client(
            conn,
            pool,
            &room.id,
            &row.agent_config,
            &app,
            &cwd,
            false,
            None,
            None,
            mcp.clone(),
            security_mode,
        ) {
            director = Some(Arc::new(Director::new(Arc::new(llm), row.id.clone())));
        }
    }
    // 共享主持人引用：参与者审批/续跑裁决读取它，Director 热切换（降级换人）后自动跟随。
    let shared_director = Arc::new(RwLock::new(director.clone()));

    for row in &rows {
        match row.participant_type.as_str() {
            "director" => {}
            "api" => {
                let event_session_id = format!("groupchat:{}:{}", room.id, row.id);
                if let Some(llm) = build_llm_client(
                    conn,
                    pool,
                    &room.id,
                    &row.agent_config,
                    &app,
                    &cwd,
                    true,
                    Some(event_session_id),
                    Some(cancel.clone()),
                    mcp.clone(),
                    security_mode,
                ) {
                    let llm = if shared_director.read().unwrap().clone().is_some() {
                        llm.with_approval_handler(make_director_approval(
                            shared_director.clone(),
                            approval_log.clone(),
                            room.topic.clone(),
                        ))
                        .with_continue_handler(make_director_continue(
                            shared_director.clone(),
                            room.topic.clone(),
                        ))
                    } else {
                        llm
                    };
                    let p = super::participant::LlmParticipant {
                        id: row.id.clone(),
                        llm: Arc::new(llm),
                        role_prompt: Arc::new(RwLock::new(row.system_role.clone())),
                        director_id: director_row.map(|r| r.id.clone()).unwrap_or_default(),
                        disabled_skills: disabled_skills.clone(),
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
                    role_prompt: Arc::new(RwLock::new(row.system_role.clone())),
                    director_id: director_row.map(|r| r.id.clone()).unwrap_or_default(),
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
        shared_director,
        director_id,
        floor_order,
        executor_order,
        user_id,
    })
}

fn build_llm_client(
    conn: &rusqlite::Connection,
    pool: &DbPool,
    room_id: &str,
    agent_config_json: &str,
    app: &tauri::AppHandle,
    cwd: &str,
    with_tools: bool,
    event_session_id: Option<String>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    mcp: Option<Arc<RoomMcpAssets>>,
    security_mode: Option<SecurityMode>,
) -> Option<PilotDeskLlmClient> {
    let v: serde_json::Value = serde_json::from_str(agent_config_json).ok()?;
    let provider_id = v["provider"].as_str()?;
    let model = v["model"].as_str().unwrap_or_default().to_string();
    let provider = crate::commands::api_provider::get_api_provider(conn, provider_id).ok()??;
    let api_key = crate::commands::api_provider::get_api_key(conn, provider_id).ok()??;
    let format = provider.api_format.parse::<ApiFormat>().unwrap_or_default();

    let mut client = PilotDeskLlmClient::new(
        provider.api_endpoint.clone(),
        api_key.clone(),
        model.clone(),
        format.clone(),
    )
    // 用量归因元数据（写 api_usage_log + `usage-recorded` 脏标记所需）：
    // provider=provider id；usage_key=房间级占位 scope；app_handle=房间 emitter/handle。
    .with_usage_meta(
        provider_id.to_string(),
        Some(format!("groupchat:{}", room_id)),
        app.clone(),
    );
    // 持久化权限规则：群聊与会话共用同一清单分类体系（高风险/风险/安全）
    client = client
        .with_permission_rules(crate::commands::permission::load_rules(conn).unwrap_or_default());
    // 工作区目录（工作区内路径 = 安全路径）
    client = client.with_workspace(Some(cwd.to_string()));
    // 会话安全模式（本波次有效，不持久化）
    client = client.with_security_mode(security_mode);
    // 流式 chunk 空闲超时：全局 app_settings 可配置（与会话共用同一键）
    client =
        client.with_stream_idle_secs(crate::commands::app_settings::load_stream_idle_secs(conn));
    if with_tools {
        // 图像生成/编辑工具复用参与者 provider 的端点/密钥（走 OpenAI 专有 /images 端点，
        // Anthropic 格式置 None）；read_image 走 chat 多模态，两种协议通用，不受该限制。
        let endpoint_key = (provider.api_endpoint.clone(), api_key.clone());
        let image = if matches!(format, ApiFormat::Anthropic) {
            None
        } else {
            Some(endpoint_key.clone())
        };
        let registry = super::adapter::tools::build_groupchat_tool_registry(
            conn,
            cwd,
            &model,
            format,
            image,
            Some(endpoint_key),
            app,
            pool,
            room_id,
            mcp.as_deref(),
        );
        client = client.with_tools(registry, app.clone());
        if let Some(sid) = event_session_id {
            client = client.with_event_session_id(sid);
        }
        // 协作式取消：注入房间停止/暂停信号，工具循环在迭代边界提前结束。
        if let Some(c) = cancel {
            client = client.with_cancel(c);
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
    /// 最终执行者的完整输出文本（v3.4n：task_result 消息落全文；无文本结论为空串）。
    result_content: String,
    /// Director 裁决 ask_user 或执行者主动发起时产生的确认请求。
    confirmation: Option<ConfirmationRequest>,
    /// 确认请求发起者 id（Director 或执行者），供确认请求消息落库时正确归属。
    confirmation_sender: Option<String>,
    /// 执行过程中主持人按需补充了新参与者（add_participant 裁决成功落地），
    /// 调用方需重建运行时名册，使新参与者进入后续任务指派的候选。
    roster_changed: bool,
}

/// 把 Director 包装成同步的 `ApprovalHandler`。读取共享主持人引用（热切换后跟随新主持人），
/// 内部用 block_in_place + block_on 调用异步的 `Director::authorize_tool`，并加确定性超时。
/// `topic` 为房间讨论目标，注入裁决以提供「任务必要性」依据。
fn make_director_approval(
    director_ref: Arc<RwLock<Option<Arc<Director>>>>,
    approval_log: Arc<Mutex<Vec<ApprovalRecord>>>,
    topic: String,
) -> ApprovalHandler {
    Box::new(
        move |_call_id: &str, tool_name: &str, args: &str, risk: RiskLevel| {
            let director = director_ref.read().unwrap().clone();
            let approval_log = approval_log.clone();
            let tool_name = tool_name.to_string();
            let args = args.to_string();
            let risk = risk.description().to_string();
            let topic = topic.clone();
            tokio::task::block_in_place(move || {
                tokio::runtime::Handle::current().block_on(async move {
                    let Some(director) = director else {
                        return false;
                    }; // 无主持人 → 默认拒绝（保守）
                    let decision = match tokio::time::timeout(
                        Duration::from_secs(DIRECTOR_CALL_TIMEOUT_SECS),
                        director.authorize_tool(&topic, &tool_name, &args, &risk),
                    )
                    .await
                    {
                        Ok(d) => d,
                        Err(_) => return false, // 裁决超时 → 拒绝
                    };
                    let allow = decision.allow;
                    let reason = decision.reason;
                    if let Ok(mut log) = approval_log.lock() {
                        log.push(ApprovalRecord {
                            tool_name,
                            risk,
                            allow,
                            reason,
                        });
                    }
                    allow
                })
            })
        },
    )
}

/// 把 Director 包装成同步的 `ContinueHandler`（停滞/上限时裁决是否继续），
/// 读取共享主持人引用；内部用 block_in_place + block_on 调用异步的
/// `Director::decide_stall_continue`，并加确定性超时。
/// 限制续命次数（最多 2 次），防止 Director 反复放行导致无限循环。
fn make_director_continue(
    director_ref: Arc<RwLock<Option<Arc<Director>>>>,
    topic: String,
) -> ContinueHandler {
    // Arc 包裹计数，Fn 闭包可多次 clone 使用（AtomicUsize 无 Copy，直接 move 会编译失败）。
    let max_continue = Arc::new(std::sync::atomic::AtomicUsize::new(2));
    Box::new(move |_current: usize, _max: usize| {
        if max_continue.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            return false;
        }
        let director = director_ref.read().unwrap().clone();
        let topic = topic.clone();
        let max_continue = max_continue.clone();
        tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(async move {
                let Some(director) = director else {
                    return false;
                };
                let keep = match tokio::time::timeout(
                    Duration::from_secs(DIRECTOR_CALL_TIMEOUT_SECS),
                    director.decide_stall_continue(&topic, ""),
                )
                .await
                {
                    Ok(v) => v.unwrap_or(false),
                    Err(_) => false,
                };
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
        Some(format!(
            "已完成以下工作（工具调用记录）：\n{}",
            lines.join("\n")
        ))
    }
}

/// 产物引用：本地文件或网络资源（前置成果注入用）。
#[derive(Debug, Clone)]
enum ArtifactKind {
    File,
    Url,
}

#[derive(Debug, Clone)]
struct ArtifactRef {
    kind: ArtifactKind,
    value: String,
}

/// 前置任务成果（注入后置任务执行者 prompt，保证任务延续性）。
#[derive(Debug, Clone)]
struct PriorResult {
    task_no: i64,
    description: String,
    /// 执行者输出全文（LLM 输出侧产物，完整透传，不做输入侧硬截断）。
    result_summary: String,
    /// 可引用产物（本地文件 / 网络 URL）。
    artifacts: Vec<ArtifactRef>,
    /// 成果正文（完整透传执行者 LLM 输出，不做输入侧硬截断——压缩只允许发生在 LLM 输出侧）。
    snippet: String,
}

/// 房间标题清理为安全目录名（v3.4ap 增强兜底）：
/// 1) 过滤 Windows 非法文件名字符；2) 剥离结尾点/空格（Windows 会剥离导致路径漂移）；
/// 3) 长度截断（64 字符，避免路径超长）；4) Windows 保留设备名兜底（CON/PRN/AUX/NUL/COM1-9/LPT1-9）。
fn sanitize_room_title(title: &str) -> String {
    crate::utils::paths::sanitize_dir_name(title, "room")
}

/// 收集任务在讨论阶段被执行/推进的证据摘要（v3.4ao）：仅作主持人裁决的**依据输入**，
/// 最终是否跳过执行由 Director（review.completed_tasks）裁决。证据 = 负责人讨论发言
/// 中携带的写盘类产物路径；无证据返回空串。
fn discussion_artifacts_summary(pool: &DbPool, room_id: &str, task: &TaskRow) -> String {
    let assignee = task.assignee.as_deref().unwrap_or("");
    if assignee.is_empty() {
        return String::new();
    }
    let Ok(conn) = pool.get() else {
        return String::new();
    };
    let Ok(rows) = store::list_messages(&conn, room_id, None, None) else {
        return String::new();
    };
    let mut paths: Vec<String> = Vec::new();
    for m in rows {
        if m.kind == "statement" && m.sender == assignee && !m.tool_calls.trim().is_empty() {
            for a in extract_artifacts_from_tool_calls(&m.tool_calls) {
                let p = match a.kind {
                    ArtifactKind::File => format!("文件 {}", a.value),
                    ArtifactKind::Url => format!("网络 {}", a.value),
                };
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
        }
    }
    if paths.is_empty() {
        String::new()
    } else {
        format!("（讨论阶段已产出：{}）", paths.join("；"))
    }
}

/// 用户输入物引用解析结果（v3.4an）：raw 为原文引用；resolved 为解析后的绝对路径/URL，
/// None=未能确定性解析（注入层显式降级为"定位要求"，避免执行者臆测路径）。
#[derive(Debug, Clone)]
struct ResolvedRef {
    raw: String,
    resolved: Option<String>,
}

/// LLM 语义层输出的输入物结构：anchor=锚点枚举（桌面/文档/下载/工作目录/上级目录…），
/// subpath=路径段数组。LLM 不允许输出路径字符串（防幻觉），路径拼接与存在性校验由代码完成。
#[derive(Debug, Clone)]
struct InputRefDraft {
    anchor: String,
    subpath: Vec<String>,
}

/// 锚点词 → 基准目录（用户主目录下的 Known Folder；工作目录类用 cwd）。
fn anchor_base_dir(anchor: &str, cwd: &str) -> Option<std::path::PathBuf> {
    let home = || {
        std::env::var_os("USERPROFILE")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
    };
    match anchor.trim() {
        "桌面" | "桌面文件夹" | "我的桌面" => home().map(|h| h.join("Desktop")),
        "文档" | "我的文档" => home().map(|h| h.join("Documents")),
        "下载" | "下载目录" => home().map(|h| h.join("Downloads")),
        "工作目录" | "项目目录" | "当前目录" => Some(std::path::PathBuf::from(cwd)),
        "上级目录" | "上一级" => std::path::Path::new(cwd).parent().map(|p| p.to_path_buf()),
        _ => None,
    }
}

/// 解析用户文本中的输入物引用（v3.4an）：
/// 1) 显式形态：URL / 绝对路径 / UNC / 含分隔符相对路径（相对 cwd 解析）；
/// 2) 锚点词形态：桌面/文档/下载/工作目录 等 + 子路径段（拼接后存在性校验）；
/// 3) LLM 语义层结构（anchor+subpath，同样拼接 + 存在性校验）。
/// 无法解析或存在性校验不通过的引用 → resolved=None，交由注入层显式降级。
fn resolve_input_refs(
    texts: &[String],
    cwd: &str,
    llm_drafts: &[InputRefDraft],
) -> Vec<ResolvedRef> {
    let mut out: Vec<ResolvedRef> = Vec::new();
    let mut push = |raw: String, resolved: Option<String>| {
        let raw = raw.trim().to_string();
        if raw.is_empty() || out.iter().any(|r| r.raw == raw) {
            return;
        }
        out.push(ResolvedRef { raw, resolved });
    };

    // ── 1) 显式形态（正则，确定性）──
    let url_re = regex::Regex::new(r#"https?://[^\s，。；、""'（）()<>|?*]+"#).unwrap();
    let abs_re = regex::Regex::new(r#"[A-Za-z]:[\\/][^\s，。；、""'（）()<>|?*]+"#).unwrap();
    let unc_re = regex::Regex::new(r#"\\\\[^\s，。；、""'（）()<>|?*]+"#).unwrap();
    // 至少两级路径的相对引用（如 web/images/a.png、docs/readme.md）
    let rel_re = regex::Regex::new(r#"[A-Za-z0-9_\-\u4e00-\u9fff]+[\\/][^\s，。；、""'（）()<>|?*]*[\\/][^\s，。；、""'（）()<>|?*]+"#).unwrap();
    for text in texts {
        for cap in url_re.captures_iter(text) {
            if let Some(m) = cap.get(0) {
                let v = m
                    .as_str()
                    .trim_end_matches(['.', '，', '。', ';', '；', '、'])
                    .to_string();
                push(v.clone(), Some(v));
            }
        }
        for cap in abs_re.captures_iter(text) {
            if let Some(m) = cap.get(0) {
                let v = m
                    .as_str()
                    .trim_end_matches(['\\', '/', '.', '，', '。', ';', '；', '、'])
                    .to_string();
                push(v.clone(), Some(v));
            }
        }
        for cap in unc_re.captures_iter(text) {
            if let Some(m) = cap.get(0) {
                push(m.as_str().to_string(), Some(m.as_str().to_string()));
            }
        }
        for cap in rel_re.captures_iter(text) {
            if let Some(m) = cap.get(0) {
                let rel = m.as_str();
                let joined = std::path::Path::new(cwd).join(rel);
                let resolved = if joined.exists() {
                    Some(joined.to_string_lossy().to_string())
                } else {
                    None
                };
                push(rel.to_string(), resolved);
            }
        }
    }

    // ── 2) 锚点词形态（启发式：锚点 + 子路径段，存在性校验；长锚点优先避免子串误匹配）──
    let anchor_keys = [
        "桌面文件夹",
        "我的文档",
        "下载目录",
        "工作目录",
        "项目目录",
        "当前目录",
        "上级目录",
        "桌面",
        "文档",
        "下载",
        "上一级",
    ];
    for text in texts {
        for key in anchor_keys {
            if let Some(pos) = text.find(key) {
                let Some(base) = anchor_base_dir(key, cwd) else {
                    continue;
                };
                let rest = &text[pos + key.len()..];
                let rest =
                    rest.trim_start_matches(['的', '下', '里', '中', '：', ':', ' ', '\u{3000}']);
                let mut sub = rest
                    .split(['，', '。', '；', ';', '、', '\n', '\r'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                sub = sub
                    .trim_end_matches("文件夹")
                    .trim_end_matches("目录")
                    .trim_end_matches("文件")
                    .trim_end_matches([' ', '的', '下', '里', '中', '\u{3000}'])
                    .to_string();
                if sub.is_empty() {
                    continue;
                }
                // 子路径可能含空格（如"2024 项目资料"），保守取首段
                let first_seg = sub.split(' ').next().unwrap_or(&sub).to_string();
                let joined = base.join(&first_seg);
                let resolved = if joined.exists() {
                    Some(joined.to_string_lossy().to_string())
                } else {
                    None
                };
                push(format!("{} {}", key, first_seg), resolved);
            }
        }
    }

    // ── 3) LLM 语义层结构（anchor+subpath，拼接 + 存在性校验）──
    for d in llm_drafts {
        let Some(base) = anchor_base_dir(&d.anchor, cwd) else {
            continue;
        };
        let mut joined = base;
        for seg in &d.subpath {
            joined = joined.join(seg);
        }
        let resolved = if joined.exists() {
            Some(joined.to_string_lossy().to_string())
        } else {
            None
        };
        push(d.subpath.join("/"), resolved);
    }

    out
}

/// 输入物引用注入段文本（C 层执行者 / B 层 Director 共用格式）。
fn format_input_refs_section(input_refs: &[ResolvedRef]) -> String {
    if input_refs.is_empty() {
        return String::new();
    }
    let mut lines = vec!["【用户输入物（已解析）】".to_string()];
    for r in input_refs {
        match &r.resolved {
            Some(p) => lines.push(format!("- [路径] {} → {}", r.raw, p)),
            None => lines.push(format!(
                "- [未解析] {} → 未能解析为具体路径，执行前请先用 list_files/glob 定位确认，必要时向主持人请求澄清，不得臆测路径",
                r.raw
            )),
        }
    }
    lines.join("\n")
}

/// 从任务工具调用链中确定性提取「可引用产物标识」（v3.4an）：
/// - 本地文件：write_file / edit_file / edit_image / create_document 等写入类工具的参数 path（绝对路径）；
/// - 网络资源：generate_image / generate_video / fetch_web 返回的 Markdown 图片/链接 URL（尖括号界定边界）。
/// 排除 data: base64 URI（体积爆炸且不可直接引用）；result 文本不扫裸本地路径（防误报库路径/噪音）。
fn extract_artifacts_from_tool_calls(tool_calls: &str) -> Vec<ArtifactRef> {
    let v: Option<serde_json::Value> = serde_json::from_str(tool_calls).ok();
    let Some(arr) = v.and_then(|v| v.as_array().cloned()) else {
        return Vec::new();
    };
    let mut out: Vec<ArtifactRef> = Vec::new();
    let mut push_unique = |kind: ArtifactKind, value: String| {
        if value.trim().is_empty() || value.starts_with("data:") {
            return;
        }
        if out.iter().any(|a| a.value == value) {
            return;
        }
        out.push(ArtifactRef { kind, value });
    };
    const WRITE_TOOLS: &[&str] = &["write_file", "edit_file", "edit_image", "create_document"];
    const URL_TOOLS: &[&str] = &["generate_image", "generate_video", "fetch_web"];
    for step in arr {
        let t = step["type"].as_str().unwrap_or("");
        let name = step["toolName"].as_str().unwrap_or("");
        match t {
            "tool_start" => {
                if WRITE_TOOLS.contains(&name) {
                    if let Some(args) = step["args"].as_str() {
                        if let Ok(a) = serde_json::from_str::<serde_json::Value>(args) {
                            if let Some(p) = a["path"].as_str() {
                                push_unique(ArtifactKind::File, p.to_string());
                            }
                        }
                    }
                }
            }
            "tool_result" => {
                if URL_TOOLS.contains(&name) {
                    if let Some(r) = step["result"].as_str() {
                        // 优先 Markdown 图片语法 ![alt](<url>)（generate_image 规范输出，尖括号界定 URL 边界）
                        if let Ok(re) = regex::Regex::new(r"!\[[^\]]*\]\(<([^>]+)>\)") {
                            for cap in re.captures_iter(r) {
                                if let Some(m) = cap.get(1) {
                                    push_unique(ArtifactKind::Url, m.as_str().trim().to_string());
                                }
                            }
                        }
                        // 兜底裸 URL（已提取的 Markdown 包裹形态去重后不再重复）
                        if let Ok(re) = regex::Regex::new(r#"https?://[^\s\)\]<>"']+"#) {
                            for cap in re.captures_iter(r) {
                                if let Some(m) = cap.get(0) {
                                    push_unique(ArtifactKind::Url, m.as_str().trim().to_string());
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
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
    /// 参与者审批/续跑裁决读取的共享主持人引用（Director 热切换后同步更新）。
    shared_director: Arc<RwLock<Option<Arc<Director>>>>,
    /// Director 降级兜底状态机（主持人 LLM 失败熔断）。
    director_fallback: DirectorFallback,
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
    /// 用户最近一条指令原文（供 Director 选人/审阅时优先服务用户最新要求）。
    last_user_directive: String,
    /// 房间级 MCP 资产（连接池 + 已枚举工具；run() 启动时异步构建一次，参与者共享）。
    mcp: Option<Arc<RoomMcpAssets>>,
    /// 会话安全模式（本波次有效，Send 命令注入；None 默认标准）。
    security_mode: Option<SecurityMode>,
    /// 参与者失败记录（participant_id → (时间戳, 原因)，保留最近 5 条）；
    /// 供 Director 感知反复失败、避免反复指派其执行任务。
    participant_failures: Arc<Mutex<HashMap<String, Vec<(i64, String)>>>>,
    /// 用户输入物引用解析结果（v3.4an）：用户指令/目标/约束中的路径与 URL 引用，
    /// 注入 Director 上下文（B 层）与执行者 prompt（C 层），避免执行者臆测路径。
    input_refs: Vec<ResolvedRef>,
    /// 房间统一产物目录（v3.4ao）：所有参与者产物必须写入该目录（绝对路径）。
    /// 来源：room.output_dir；空时回退 `<工作目录>/outputs/<房间标题>/`。
    output_dir: String,
}

impl RoomRuntime {
    fn emit(&self, event: GroupChatEvent) {
        let _ = self.app.emit("groupchat-event", &event);
    }

    fn conn(
        &self,
    ) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, AppError> {
        Ok(self.pool.get()?)
    }

    async fn run(mut self) {
        log::info!("[GroupChat] 房间 Actor 启动: {}", self.room_id);
        self.emit(GroupChatEvent::started(&self.room_id));

        // 群聊 MCP 解禁：房间级连接池（异步枚举一次），装配 MCP 工具后重建参与者（若有）。
        // 需在「懒恢复续跑」之前执行，保证续跑的任务同样具备 MCP 工具。
        if let Ok(conn) = self.conn() {
            // 同步读取 MCP 服务器配置（避免 &Connection 跨 await 导致非 Send），再异步装配。
            let servers = crate::commands::mcp::load_servers(&conn).unwrap_or_default();
            let assets = super::adapter::tools::build_room_mcp_assets(servers).await;
            if !assets.entries.is_empty() {
                log::info!(
                    "[GroupChat] 房间 {} 装配 MCP 工具（{} 个服务器）",
                    self.room_id,
                    assets.entries.len()
                );
                self.mcp = Some(Arc::new(assets));
                self.rebuild_participants().await;
            }
        }

        // 应用重启后懒恢复：房间仍标记 running（意外退出）且已有任务 → 自动续跑。
        if self.room.status == "running" && self.has_any_tasks() {
            self.resume_execution().await;
        }

        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                RoomCommand::Send {
                    sender,
                    content,
                    recipients,
                    reply_to,
                    mention,
                    attachments,
                    security_mode,
                } => {
                    if self.control.abort.load(Ordering::SeqCst) {
                        // 手动终止后，用户的新消息视为"重启"意图：清除终止标记，让 handle_send
                        // 重新进入编排（handle_send 会恢复 running 状态并处理该指令）。
                        self.control.abort.store(false, Ordering::SeqCst);
                    }
                    // 会话安全模式：本波次有效，注入运行时（None 默认标准）
                    self.security_mode = security_mode;
                    self.control.wait_if_paused().await;
                    self.handle_send(
                        &sender,
                        &content,
                        &recipients,
                        reply_to.as_deref(),
                        mention.as_deref(),
                        &attachments,
                    )
                    .await;
                }
                RoomCommand::SetDirector { director_id } => {
                    self.set_director(&director_id).await;
                }
                RoomCommand::SetOutputDir(output_dir) => {
                    self.room.output_dir = output_dir.clone();
                    self.output_dir = output_dir;
                }
                RoomCommand::ParticipantChanged => {
                    self.handle_participant_changed().await;
                }
                RoomCommand::Pause => {
                    self.persist_control_notice("已暂停：正在完成当前执行中的任务，之后将暂停调度新任务。点击「继续」可恢复。").await;
                    self.set_status(RoomStatus::Paused).await;
                }
                RoomCommand::Resume => {
                    // 恢复/重启：先解除暂停并**清除手动终止标记**——否则对已 aborted 房间点"继续/重启"
                    // 时残留的 abort 标志会让 resume_execution/finalize 立刻再次收敛回 Aborted，
                    // 表现为"停止后不能再重启"。清除后：有任务则续跑未完成；无任务则置 Running 等新指令。
                    self.control.resume();
                    self.control.abort.store(false, Ordering::SeqCst);
                    if self.has_any_tasks() {
                        self.resume_execution().await;
                    } else {
                        self.set_status(RoomStatus::Running).await;
                    }
                }
                RoomCommand::Abort => {
                    self.persist_control_notice(
                        "已停止：正在完成当前执行中的任务（若有），之后不再调度新任务。",
                    )
                    .await;
                    self.set_status(RoomStatus::Aborted).await;
                }
                // 非等待状态下收到确认回复（历史确认块补交 / 房间已结束 / Actor 重启后未进入确认等待），
                // 落库 confirmation_response 保证前端刷新后能恢复「已提交」态；
                // v3.4m 恢复闭环：命中仍 pending 的确认任务时注入回复并立即重跑剩余任务（无需再发指令）。
                RoomCommand::RespondConfirmation {
                    request_id,
                    responses,
                } => {
                    let rerun = self
                        .persist_confirmation_response_only(&request_id, &responses)
                        .await;
                    if rerun && !self.control.abort.load(Ordering::SeqCst) {
                        self.control.wait_if_paused().await;
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
                }
            }
        }
    }

    /// 参与者增删后，从 DB 重建参与者/Director/发言顺序，使后续讨论生效。
    async fn rebuild_participants(&mut self) {
        let Ok(conn) = self.conn() else {
            return;
        };
        let Ok(built) = build_participants(
            &conn,
            &self.pool,
            &self.room,
            self.app.clone(),
            self.approval_log.clone(),
            self.control.abort.clone(),
            self.mcp.clone(),
            self.security_mode,
        ) else {
            return;
        };
        self.participants = built.participants;
        self.director = built.director;
        self.director_id = built.director_id;
        self.user_id = built.user_id;
        // 同步共享主持人引用：参与者的审批/续跑裁决读取的是 build_participants 新建的 Arc，
        // 不更新会导致热切换后自我引用旧主持人（失败补人接线同样依赖它）。
        self.shared_director = built.shared_director;
        self.floor = FloorManager::new(built.floor_order.clone());
        self.floor_order = built.floor_order;
        self.executor_order = built.executor_order;
    }

    /// 格式化可用模型 + CLI Agent 清单，供 Director 按需组建参与者时选模型/agent。
    /// 仅含模型名与备注，不含 key（沿用 list_models 的展示纪律）。
    fn format_roster_catalog(&self) -> String {
        let Ok(conn) = self.conn() else {
            return "（无可用模型/CLI Agent）".to_string();
        };
        format_roster_catalog_db(&conn, self.room.allow_auto_cli != 0)
    }

    /// 当前房间剩余可自动新增的参与者空位（房间内 api/cli 总数上限扣减现有数，最小 0）。
    /// 供注入 Director 提示词，避免其反复提出超量的补充被运行时拒绝、浪费调用轮次。
    fn remaining_participant_slots(&self) -> usize {
        let Ok(conn) = self.conn() else {
            return 0;
        };
        remaining_participant_slots_db(&conn, &self.room_id)
    }

    /// 记录一次参与者失败（讨论发言/任务执行均计入）。
    fn record_participant_failure(&self, participant_id: &str, reason: &str) {
        record_participant_failure_shared(&self.participant_failures, participant_id, reason);
    }

    /// 参与者是否为慢性失败者（失败 ≥ 2 次）：运行时硬过滤，不依赖 LLM 自觉。
    fn is_chronic_failer(&self, participant_id: &str) -> bool {
        is_chronic_failer_map(&self.participant_failures, participant_id)
    }

    /// 格式化参与者近期失败记录（供 Director 提示词注入）；无记录返回空字符串。
    /// 每个参与者显示失败次数与最近一次原因（截断 120 字）。
    fn format_participant_failures(&self) -> String {
        let Ok(m) = self.participant_failures.lock() else {
            return String::new();
        };
        if m.is_empty() {
            return String::new();
        }
        let mut ids: Vec<&String> = m.keys().collect();
        ids.sort();
        let mut lines: Vec<String> = Vec::new();
        for id in ids {
            let fails = &m[id];
            let latest = fails
                .last()
                .map(|(_, r)| r.chars().take(120).collect::<String>())
                .unwrap_or_default();
            lines.push(format!(
                "- [@{}]：失败 {} 次（最近：{}）",
                id,
                fails.len(),
                latest
            ));
        }
        lines.join("\n")
    }

    /// 格式化最近发言（供 next_speaker 判断任务是否已被实质推进——有产出/完成即视为已推进，
    /// 不得再次指派该任务）。取最近 `limit` 条发言类消息（statement），完整输出。
    /// v3.4av：不再输入侧硬截断——判断方案是否收敛必须基于完整发言，压缩只允许发生在
    /// LLM 输出侧（如 L1 摘要）；150 字截断曾导致主持人误判"发言未完成"而重复点名。
    fn format_recent_speeches(&self, limit: usize) -> String {
        let all = self.memory.all_messages();
        let speeches: Vec<&MessageRow> = all
            .iter()
            .filter(|m| m.kind == "statement" && !m.content.trim().is_empty())
            .collect();
        if speeches.is_empty() {
            return String::new();
        }
        let take = speeches.len().min(limit);
        let recent = &speeches[speeches.len() - take..];
        let mut lines: Vec<String> = Vec::new();
        for m in recent {
            lines.push(format!("- [@{}]：{}", m.sender, m.content));
        }
        lines.join("\n")
    }

    /// 按需自动创建参与者（全自动组建名册，主持人提出后直接生效）：复用自由函数校验落库，
    /// 并对每个创建成功的参与者广播 participant_updated 事件。返回 (创建成功 id, 拒绝原因)。
    /// `max_new` 为本次单次补充上限（kickoff 初始批量=剩余空位，replan=场景上限）。
    fn create_auto_participants(
        &self,
        drafts: &[NewParticipantDraft],
        max_new: usize,
    ) -> (Vec<String>, Vec<String>) {
        let Ok(conn) = self.conn() else {
            return (
                Vec::new(),
                vec!["数据库连接失败，无法自动补充参与者".to_string()],
            );
        };
        let (created, rejected) = create_auto_participants_db(
            &conn,
            &self.room_id,
            drafts,
            self.room.allow_auto_cli != 0,
            max_new,
        );
        for c in &created {
            self.emit(GroupChatEvent::participant_updated(&self.room_id));
            log::info!(
                "[GroupChat] 主持人自动补充参与者: room={} id={}",
                self.room_id,
                c
            );
        }
        (created, rejected)
    }

    async fn set_status(&mut self, status: RoomStatus) {
        // RoomStateMachine 看门狗：DB（groupchat_rooms.status）仍为唯一权威，
        // 这里把状态同步镜像进 self.state.status，并对迁移记日志便于审计。
        // 不 panic：同状态重复（Running→Running、Finished 后重启）与 Aborted→Running
        // （停止后"继续/重启"）都是合法高频路径，非法迁移先以日志暴露，数据积累后再收紧。
        let prev = self.state.status;
        self.state.status = status;
        if prev != status {
            log::info!(
                "[GroupChat] room {} 状态迁移 {:?} -> {:?}",
                self.room_id,
                prev,
                status
            );
        }
        if let Ok(conn) = self.conn() {
            let _ = store::update_room_status(&conn, &self.room_id, status.as_str());
        }
        self.emit(GroupChatEvent::room_status(&self.room_id, status.as_str()));
    }

    /// 主持人热替换（结构身份变更）：交换目标参与者与现任主持人的 participant_type、
    /// 更新 room.director_id、重建运行时（Director 实例 + 参与者 + 发言顺序），并重排角色与任务。
    /// 新主持人构建失败时回滚并保留原主持人。
    async fn set_director(&mut self, director_id: &str) {
        let target = self
            .conn()
            .ok()
            .and_then(|c| store::get_participant(&c, &self.room_id, director_id).ok())
            .flatten();
        let Some(target) = target else {
            self.persist_control_notice(&format!(
                "无法切换主持人：参与者 [@{}] 不存在",
                director_id
            ))
            .await;
            return;
        };
        if target.participant_type == "director" {
            // 已是主持人，无需切换。
            return;
        }
        let old_director = self
            .director_id
            .clone()
            .unwrap_or_else(|| "director".to_string());

        // 1) 交换结构身份：目标参与者 → director；原主持人 → api（沿用其模型配置作为普通参与者）。
        if let Ok(conn) = self.conn() {
            let _ = store::update_participant_type(&conn, &self.room_id, director_id, "director");
            let _ = store::update_participant_type(&conn, &self.room_id, &old_director, "api");
            let _ = store::update_room_director(&conn, &self.room_id, director_id);
        }
        self.director_id = Some(director_id.to_string());

        // 2) 重建运行时（Director 实例 + 参与者 + 发言顺序）。
        self.rebuild_participants().await;

        // 3) 新主持人不可用 → 回滚。
        if self.director.is_none() {
            if let Ok(conn) = self.conn() {
                let _ = store::update_participant_type(&conn, &self.room_id, director_id, "api");
                let _ =
                    store::update_participant_type(&conn, &self.room_id, &old_director, "director");
                let _ = store::update_room_director(&conn, &self.room_id, &old_director);
            }
            self.director_id = Some(old_director);
            self.rebuild_participants().await;
            self.persist_control_notice(&format!(
                "主持人切换失败：新主持人 [@{}] 不可用，已恢复原主持人",
                director_id
            ))
            .await;
            return;
        }

        // 4) 落可追溯提示 + 全量重排角色与任务。
        self.persist_control_notice(&format!(
            "主持人已切换为 [@{}]，正在重新编排角色与任务…",
            director_id
        ))
        .await;
        self.replan_after_roster_change(&format!(
            "【调度】主持人已切换为 [@{}]，已重新编排",
            director_id
        ))
        .await;
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
        self.process_user_directive(sender, content, recipients, reply_to, mention, attachments)
            .await;

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

            // v3.4bp 分层执行：execute_tasks 只执行完当前就绪层（依赖已解锁且未终态）；
            // 若仍有后续层（依赖当前层产物、尚未执行的任务）则回 discuss 讨论下一层，
            // 全部任务终态才 break 收尾——已收敛层不再积压到末尾统一执行。
            if self.all_tasks_terminal() {
                break;
            }
            continue;
        }

        // 收敛结论
        self.finalize().await;
    }

    /// 是否存在任何任务（终态或未终态）。用于判断重启后是否需要续跑/补结论。
    fn has_any_tasks(&self) -> bool {
        let Ok(conn) = self.conn() else {
            return false;
        };
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
        for m in &messages {
            memory.apply_message(m, &pids);
        }
        if let Ok(stances) = store::list_stances(&conn, &self.room_id) {
            for s in stances {
                memory.update_stance(&s.participant_id, &s.stance, Attitude::parse(&s.attitude));
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
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else {
            return;
        };
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

    /// 恢复执行：重建记忆 → running 任务重新排队重试 → 与 handle_send 同款主循环
    /// （先讨论收敛再执行，不再跳过讨论直接执行——v3.4ao 消除"重启绕过讨论"的硬编排）。
    async fn resume_execution(&mut self) {
        self.rebuild_memory().await;
        self.requeue_running_tasks().await;
        self.set_status(RoomStatus::Running).await;

        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                break;
            }
            if self.discuss().await {
                if self.orchestration_done() {
                    break;
                }
                continue;
            }
            if self.control.abort.load(Ordering::SeqCst) {
                break;
            }
            if self.execute_tasks().await {
                if self.orchestration_done() {
                    break;
                }
                continue;
            }
            // v3.4bp 分层执行：execute_tasks 只执行完当前就绪层，回 discuss 讨论后续层，
            // 全部任务终态才 break 收尾。
            if self.all_tasks_terminal() {
                break;
            }
            continue;
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
        // 主席结论落库消息表（可追溯）；携带主持人生成结论时的思考链。
        let director_sender = self.director_sender();
        let concl_reasoning = self.take_director_reasoning();
        let concl_msg = self
            .persist_message(
                &director_sender,
                &[],
                "conclusion",
                None,
                &conclusion,
                &[],
                "",
                &concl_reasoning,
            )
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
        let user_msg = self
            .persist_user_directive(sender, content, recipients, reply_to, mention, attachments)
            .await;
        self.after_user_directive(content, &user_msg).await;
    }

    /// 用户指令落库 + 前端显示（立即部分）：归一化 @显示名、落库 directive 消息、发事件、
    /// 记录 @ 指定与最新指令。编排（after_user_directive）由调用方决定时机：
    /// 波次间隙立即执行；执行波次中途插队时延迟到波次结束后执行。
    async fn persist_user_directive(
        &mut self,
        sender: &str,
        content: &str,
        recipients: &[String],
        reply_to: Option<&str>,
        mention: Option<&str>,
        attachments: &[Attachment],
    ) -> MessageRow {
        // 用户消息落库 + 事件（正文中 @显示名 归一化为 [@id]，保持 id 唯一引用；显示名仅前端展示）
        let normalized = self.normalize_user_mentions(content);
        let user_msg = self
            .persist_message(
                sender,
                recipients,
                "directive",
                reply_to,
                &normalized,
                attachments,
                "",
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, user_msg.clone()));

        // 记录用户 @ 指定的参与者，供本轮下一发言者选择消费。
        self.pending_mention = mention.filter(|m| !m.is_empty()).map(|m| m.to_string());
        // 记录用户最新指令（v3.4ap：拼接当前真实时钟锚点），供 Director 选人/审阅时优先服务
        // 用户最新要求，并正确处理相对时间表达。仅作用于 LLM 上下文，不影响落库原文与展示。
        self.last_user_directive = format!("{}\n\n{}", crate::utils::current_clock_cn(), content);
        user_msg
    }

    /// 把用户正文中的 `@显示名` 归一化为 `[@id]`（显示名长度降序替换，避免子串误替换）。
    /// 仅替换用户明确 @ 提及的显示名；id 唯一，保证 LLM 视图与前端展示的确定性映射。
    fn normalize_user_mentions(&self, content: &str) -> String {
        let Ok(conn) = self.conn() else {
            return content.to_string();
        };
        let Ok(rows) = store::list_participants(&conn, &self.room_id) else {
            return content.to_string();
        };
        let mut pairs: Vec<(String, String)> = rows
            .iter()
            .filter(|p| !p.display_name.is_empty() && p.display_name != p.id)
            .map(|p| (p.display_name.clone(), p.id.clone()))
            .collect();
        pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        let mut out = content.to_string();
        for (name, id) in pairs {
            let needle = format!("@{}", name);
            if out.contains(&needle) {
                out = out
                    .split(&needle)
                    .collect::<Vec<_>>()
                    .join(&format!("[@{}]", id));
            }
        }
        out
    }

    /// 用户指令落库后的统一后续：按意图编排任务、写入记忆、重置收敛计数。
    async fn after_user_directive(&mut self, content: &str, user_msg: &MessageRow) {
        // v3.4at：初始目标不再直接复制用户消息原文——由阶段A（目标确认门）先理解整理、
        // 按需向用户确认后，topic 才落位为主持人整理后的总目标。用户原文由消息历史/记忆承载。

        // 用户消息拼接当前真实时钟锚点（v3.4ap）：Director 编排（意图分类/kickoff/replan）
        // 处理"今天/最近N天"等相对时间时以真实时钟为准。仅作用于编排链路，不影响落库与展示。
        let content_llm = format!("{}\n\n{}", crate::utils::current_clock_cn(), content);
        let content: &str = &content_llm;

        // 意图判定：首轮/目标变更 → 全量编排；refine → 对账式重排（可删减/重分配/新增）。
        let disposition = self.maybe_update_goal(content).await;

        // 刷新用户输入物解析（B/C 层注入数据源）：新指令携带的路径/URL 引用先解析再编排。
        self.refresh_input_refs().await;

        let result = match disposition {
            GoalDisposition::FirstRound => {
                self.clarify_and_kickoff(content, &user_msg.content).await
            }
            GoalDisposition::Changed(goal) => self.clarify_and_kickoff(&goal, &goal).await,
            GoalDisposition::Amended(supplement) => {
                // 目标范围扩展：合并进目标锚点后对账重排（不归档旧任务）。
                self.apply_goal_amend(&supplement).await;
                self.replan(content).await
            }
            GoalDisposition::TempTask {
                description,
                assignee,
            } => {
                // 临时性额外任务：直插执行，不触碰目标锚点与约束。
                self.insert_temp_task(&description, assignee.as_deref())
                    .await;
                KickoffResult { added: 1 }
            }
            GoalDisposition::SwitchDirector(target) => {
                // 更换主持人：热替换结构身份 + 全量重排角色与任务。
                self.set_director(&target).await;
                KickoffResult { added: 0 }
            }
            GoalDisposition::Refined => self.replan(content).await,
        };
        self.last_kickoff_added = result.added;

        // 讨论前把用户指令写入记忆，确保首个发言者的 view.messages 至少含一条 user 消息，
        // 否则上游 API 会因 "No user query found in messages" 返回 400。
        let pids = self.all_participant_ids();
        self.memory.apply_message(user_msg, &pids);

        // 新指令意味着讨论主题可能变化，重置收敛计数，避免插队后过早收敛。
        self.rules.reset();
    }

    /// 刷新用户输入物解析结果（v3.4an）：确定性解析 + LLM 语义层（失败/超时降级为仅确定性）。
    /// 解析来源：初始目标 + 用户最新指令 + 累积约束。
    async fn refresh_input_refs(&mut self) {
        let mut texts: Vec<String> = Vec::new();
        if !self.room.topic.trim().is_empty() {
            texts.push(self.room.topic.clone());
        }
        if !self.last_user_directive.trim().is_empty() {
            texts.push(self.last_user_directive.clone());
        }
        if !self.room.goal_notes.trim().is_empty() {
            texts.push(self.room.goal_notes.clone());
        }
        let cwd = self
            .conn()
            .ok()
            .map(|c| {
                crate::utils::paths::resolve_workspace_path(None, "", &c)
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_default();
        // LLM 语义层：仅输出 anchor+subpath 结构（防幻觉），拼接与存在性校验交给 resolve_input_refs。
        let mut llm_drafts: Vec<InputRefDraft> = Vec::new();
        if let Some(director) = self.director.clone() {
            let combined = texts.join("\n");
            if !combined.trim().is_empty() {
                if let Some(drafts) =
                    call_director_retry(|| director.parse_input_refs(&combined)).await
                {
                    llm_drafts = drafts
                        .into_iter()
                        .map(|(anchor, subpath)| InputRefDraft { anchor, subpath })
                        .collect();
                }
            }
        }
        self.input_refs = resolve_input_refs(&texts, &cwd, &llm_drafts);
    }

    /// B 层注入：把用户输入物解析段、统一产物目录与累积约束合并，作为 Director 上下文的 notes。
    fn combined_notes(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let input_section = format_input_refs_section(&self.input_refs);
        if !input_section.is_empty() {
            parts.push(input_section);
        }
        if !self.output_dir.is_empty() {
            parts.push(format!(
                "【产物目录】本房间所有参与者产出的文件必须统一写入：`{}`（绝对路径）。\
                 该目录已由系统创建，写文件会自动补齐缺失的上级目录，无需（也不要用 mkdir）创建。\
                 任务描述要求产出物时请指明该目录；主持人裁决/总结引用产物时同样基于该目录。",
                self.output_dir
            ));
        }
        if !self.room.goal_notes.trim().is_empty() {
            parts.push(self.room.goal_notes.clone());
        }
        parts.join("\n")
    }

    /// 目标理解阶段确定产物目录（v3.4ay）：用户已填则校验合法性并尝试创建；
    /// 未填/不合法/创建失败则回退 `<工作目录>/outputs/<目录名>` 并创建（目录名优先取
    /// 主持人命名的 `dir_name_hint`，为空才按目标主题截断兜底）；
    /// 结果落库 room.output_dir 并发事件，前端右侧面板可动态显示当前值。
    async fn resolve_and_persist_output_dir(&mut self, goal: &str, dir_name_hint: &str) -> String {
        let conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return self.output_dir.clone(),
        };
        let cwd = crate::utils::paths::resolve_workspace_path(None, "", &conn)
            .to_string_lossy()
            .to_string();
        // 目录名：主持人命名优先（简短贴切），否则按目标主题截断兜底；均去除换行等空白。
        let source = if dir_name_hint.trim().is_empty() {
            goal
        } else {
            dir_name_hint
        };
        let dir_name = sanitize_room_title(&source.replace(['\n', '\r', '\t'], ""));

        let resolved = if self.room.output_dir.trim().is_empty() {
            let fallback = format!(
                "{}\\outputs\\{}",
                cwd.trim_end_matches(['\\', '/']),
                dir_name
            );
            let _ = fs::create_dir_all(&fallback);
            fallback
        } else {
            let p = self.room.output_dir.trim().to_string();
            if Path::new(&p).is_absolute()
                && (Path::new(&p).exists() || fs::create_dir_all(&p).is_ok())
            {
                p
            } else {
                // 用户填写的路径不合法/无法创建：按未提供处理，回退默认产物目录。
                let fallback = format!(
                    "{}\\outputs\\{}",
                    cwd.trim_end_matches(['\\', '/']),
                    dir_name
                );
                let _ = fs::create_dir_all(&fallback);
                fallback
            }
        };

        if self.room.output_dir != resolved {
            let _ = store::update_room_output_dir(&conn, &self.room_id, &resolved);
            self.room.output_dir = resolved.clone();
        }
        self.output_dir = resolved.clone();
        self.emit(GroupChatEvent::output_dir_updated(&self.room_id, &resolved));
        resolved
    }

    /// 当前房间任务总数。
    fn task_count(&self) -> usize {
        self.conn()
            .ok()
            .map(|c| {
                store::list_tasks(&c, &self.room_id)
                    .unwrap_or_default()
                    .len()
            })
            .unwrap_or(0)
    }

    /// 编排收敛谓词：最近一次编排未新增任务，且所有任务均处于终态。
    /// 用于主循环在插队/确认后判断是否应结束，避免「追加 0 个仍空转」。
    fn orchestration_done(&self) -> bool {
        if self.last_kickoff_added != 0 {
            return false;
        }
        let Ok(conn) = self.conn() else {
            return false;
        };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else {
            return false;
        };
        tasks.iter().all(|t| {
            matches!(
                t.status.as_str(),
                "success" | "skipped" | "failed" | "aborted"
            )
        })
    }

    /// 任务终态即收敛的确定性兜底：存在任务且所有任务均处于终态。
    fn all_tasks_terminal(&self) -> bool {
        let Ok(conn) = self.conn() else {
            return false;
        };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else {
            return false;
        };
        !tasks.is_empty()
            && tasks.iter().all(|t| {
                matches!(
                    t.status.as_str(),
                    "success" | "skipped" | "failed" | "aborted"
                )
            })
    }

    /// 当前可执行层（就绪层）任务 id：前置任务全部 success 且自身未终态的任务。
    /// 与 execute_tasks 中波次计算共用同一依赖语义：`depends_on` 的绝对下标指向的前置全部 success。
    /// v3.4bp 分层执行的核心判据——讨论只聚焦就绪层，收敛后立即执行该层，再解锁后续层。
    fn ready_layer_ids(&self) -> Vec<String> {
        let Ok(conn) = self.conn() else {
            return Vec::new();
        };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else {
            return Vec::new();
        };
        let statuses: Vec<&str> = tasks.iter().map(|t| t.status.as_str()).collect();
        let deps: Vec<Vec<usize>> = tasks
            .iter()
            .map(|t| serde_json::from_str::<Vec<usize>>(&t.depends_on).unwrap_or_default())
            .collect();
        tasks
            .iter()
            .enumerate()
            .filter(|(i, _t)| {
                // 未终态（discussing/pending/running）且前置全部 success → 就绪可讨论/执行。
                let current = statuses[*i];
                let not_terminal = !matches!(current, "success" | "skipped" | "failed" | "aborted");
                let deps_ok = deps[*i]
                    .iter()
                    .all(|&d| d < statuses.len() && statuses[d] == "success");
                not_terminal && deps_ok
            })
            .map(|(_, t)| t.id.clone())
            .collect()
    }

    /// 就绪层是否为空：没有可讨论/可执行的任务（全部任务已终态，或存在依赖环/阻塞）。
    /// 讨论过程中就绪层为空意味着当前层已全部收敛并执行完，需回到 execute 解锁下一层或结束。
    fn ready_layer_empty(&self) -> bool {
        self.ready_layer_ids().is_empty()
    }

    /// 判定并应用目标意图。默认目标不可变；仅在用户明确要求变更目标或提出
    /// 全新无关目标时更新目标锚点；refine 的补充/细化约束累积进 goal_notes。
    async fn maybe_update_goal(&mut self, content: &str) -> GoalDisposition {
        // 首轮无既有任务时无需判定（content 为目标输入，随后由阶段A确认门整理后落位 topic）。
        let existing_count = self.task_count();
        if existing_count == 0 {
            return GoalDisposition::FirstRound;
        }

        let director = match self.director.clone() {
            Some(d) => d,
            None => return GoalDisposition::Refined,
        };

        // 名册（id / 显示名 / 当前角色）供意图分类解析用户指令中的人称引用（如"让张三当主持人"）。
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

        let g = match call_director_retry(|| {
            director.classify_goal_intent(&self.room.topic, content, &self.room.goal_notes, &roster)
        })
        .await
        {
            Some(g) => g,
            None => return GoalDisposition::Refined,
        };

        match g.intent.as_str() {
            "switch_director" => {
                // 更换主持人：结构身份变更，交由 set_director 热替换。
                let target = g
                    .director
                    .clone()
                    .map(|id| normalize_participant_ref(&id))
                    .unwrap_or_default();
                if target.is_empty() || !self.participants.contains_key(&target) {
                    return GoalDisposition::Refined;
                }
                GoalDisposition::SwitchDirector(target)
            }
            "change_goal" | "new_goal" => {
                let new_goal = g.new_goal.trim().to_string();
                if new_goal.is_empty() || new_goal == self.room.topic {
                    return GoalDisposition::Refined;
                }
                self.apply_goal_change(&new_goal).await;
                GoalDisposition::Changed(new_goal)
            }
            "amend_goal" => {
                // 目标范围扩展：修正初始目标锚点（追加合并），不归档任务、不重置记忆。
                let supplement = g.amend.trim().to_string();
                if supplement.is_empty() {
                    return GoalDisposition::Refined;
                }
                GoalDisposition::Amended(supplement)
            }
            "temp_task" => {
                // 临时性一次性额外任务：不更新目标，直接插入执行。
                let desc = g
                    .temp_task
                    .as_ref()
                    .map(|t| t.description.trim().to_string())
                    .unwrap_or_default();
                if desc.is_empty() {
                    return GoalDisposition::Refined;
                }
                GoalDisposition::TempTask {
                    description: desc,
                    assignee: g.temp_task.as_ref().and_then(|t| t.assignee.clone()),
                }
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

    /// 目标追加合并（amend_goal）：把用户补充的目标内容并入目标锚点。
    /// 区别于 apply_goal_change 的全量归档/重置——不归档任务、不重置记忆，随后走对账重排。
    async fn apply_goal_amend(&mut self, supplement: &str) {
        let new_topic = format!("{}\n\n【补充目标】\n{}", self.room.topic, supplement.trim());
        if let Ok(conn) = self.conn() {
            let _ = store::update_room_topic(&conn, &self.room_id, &new_topic);
        }
        self.room.topic = new_topic;
        // 系统提示，目标扩展可追溯。
        let director_sender = self.director_sender();
        let sys_msg = self
            .persist_message(
                &director_sender,
                &[],
                "system",
                None,
                &format!("用户已补充目标内容：\n{}", supplement),
                &[],
                "",
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, sys_msg));
    }

    /// 插入临时性额外任务（temp_task）：不更新目标锚点与约束，执行完即弃。
    async fn insert_temp_task(&mut self, description: &str, assignee: Option<&str>) {
        let task_no = (self.task_count() + 1) as i64;
        let assignee = assignee
            .map(normalize_participant_ref)
            .filter(|id| self.participants.contains_key(id));
        let task = TaskRow {
            id: uuid::Uuid::new_v4().to_string(),
            room_id: self.room_id.clone(),
            task_no,
            description: format!("[临时] {}", description.trim()),
            assignee: assignee.clone(),
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
        let director_sender = self.director_sender();
        let assignee_text = assignee
            .map(|a| format!("[@{}]", a))
            .unwrap_or_else(|| "待指派".into());
        let msg = self
            .persist_message(
                &director_sender,
                &[],
                "task_assignment",
                None,
                &format!(
                    "临时任务：T{}. {}（负责人：{}）",
                    task.task_no, task.description, assignee_text
                ),
                &[],
                "",
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, msg));
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
                Ok(RoomCommand::SetDirector { director_id }) => {
                    self.set_director(&director_id).await
                }
                Ok(RoomCommand::SetOutputDir(output_dir)) => {
                    self.room.output_dir = output_dir.clone();
                    self.output_dir = output_dir;
                }
                Ok(RoomCommand::ParticipantChanged) => self.handle_participant_changed().await,
                Ok(RoomCommand::Pause) => {
                    self.persist_control_notice("已暂停：正在完成当前执行中的任务，之后将暂停调度新任务。点击「继续」可恢复。").await;
                    self.set_status(RoomStatus::Paused).await;
                }
                Ok(RoomCommand::Resume) => self.set_status(RoomStatus::Running).await,
                Ok(RoomCommand::Abort) => {
                    self.persist_control_notice(
                        "已停止：正在完成当前执行中的任务（若有），之后不再调度新任务。",
                    )
                    .await;
                    self.set_status(RoomStatus::Aborted).await;
                }
                Ok(RoomCommand::RespondConfirmation {
                    request_id,
                    responses,
                }) => {
                    // 波次间隙补交的确认回复（历史确认块）：落库记录，不打断当前执行。
                    let _ = self
                        .persist_confirmation_response_only(&request_id, &responses)
                        .await;
                }
                Err(_) => return None,
            }
        }
    }

    /// 阶段A 目标确定 + 阶段B 编排（v3.4at）：主持人先理解整理用户目标（生成总目标），
    /// 若目标存在歧义/缺关键基础信息则先向用户确认（复用确认机制），确认后 topic 才落位为
    /// 整理后的总目标，再进入 kickoff 编排；目标清晰则直接落位并编排。
    /// `latest_llm` 为带时钟锚点的最新指令（供 LLM 相对时间理解），`raw` 为无时钟原文（降级落位用）。
    async fn clarify_and_kickoff(&mut self, latest_llm: &str, raw: &str) -> KickoffResult {
        let director_sender = self.director_sender();
        let roster = self.participant_roster();
        let roster_text = roster
            .iter()
            .map(|(id, name, role)| {
                let role = if role.trim().is_empty() {
                    "未分配"
                } else {
                    role.as_str()
                };
                format!("- [@{}]（显示名：{}）（当前角色：{}）", id, name, role)
            })
            .collect::<Vec<_>>()
            .join("\n");

        // 阶段A1 目标整理：主持人理解并生成总目标 + 产物目录名；调用失败/超时降级为按用户原文编排（不阻塞）。
        let notes = self.combined_notes();
        let (goal, need_confirmation, questions, dir_name) = match self.director.clone() {
            Some(d) => call_director_retry(|| {
                d.clarify_goal(&self.room.topic, latest_llm, &notes, &roster_text)
            })
            .await
            .map(|c| (c.goal, c.need_confirmation, c.questions, c.output_dir_name))
            .unwrap_or((raw.to_string(), false, Vec::new(), String::new())),
            None => (raw.to_string(), false, Vec::new(), String::new()),
        };
        // 主持人本次目标理解决策的思考链（供落库展示）。
        let clarify_reasoning = self.take_director_reasoning();

        if need_confirmation {
            // 阶段A2 确认门（v3.5c 自适应多轮消化）：主持人依据用户每轮回复内容 + 材料核验简报
            // 智能消化——更新问题答案、裁剪/增补问题项、调整目标与方向；仅当仍缺「影响任务拆分
            // 且回复/材料都无法确定」的关键信息时才继续追问（上限轮次防死循环）；信息满足即落位
            // topic 进入编排。不再把不完整的单轮回复当完整确认。
            let mut current_goal = goal.clone();
            let mut current_dir_name = dir_name.clone();
            let mut open_questions = questions;
            let mut reasoning = clarify_reasoning.clone();
            // 逐轮消化确认的要点累积（在后续追问文案中回显，让用户感知主持人"消化了什么"）。
            let mut resolved_acc: Vec<String> = Vec::new();
            let mut settled = false;
            let mut rounds = 0usize;
            while !settled && rounds < MAX_CLARIFY_ROUNDS {
                rounds += 1;
                let questions_text = open_questions
                    .iter()
                    .enumerate()
                    .map(|(i, q)| format!("{}. {}", i + 1, q))
                    .collect::<Vec<_>>()
                    .join("\n");
                let digest_line = if resolved_acc.is_empty() {
                    String::new()
                } else {
                    format!("已根据你的回复消化确认：\n{}\n", resolved_acc.join("\n"))
                };
                let notice = format!(
                    "【目标理解】我已理解你的目标，以下信息需要与你确认后再开始编排：\n目标：{}\n{}待确认问题：\n{}",
                    current_goal, digest_line, questions_text
                );
                let msg = self
                    .persist_message(
                        &director_sender,
                        &[],
                        "director_notice",
                        None,
                        &notice,
                        &[],
                        "",
                        &reasoning,
                    )
                    .await;
                self.memory.apply_message(&msg, &self.all_participant_ids());
                self.emit(GroupChatEvent::message(&self.room_id, msg));

                // 构造开放式确认请求（无关联任务），落库后阻塞等待用户回复。
                let request_id = uuid::Uuid::new_v4().to_string();
                let items: Vec<ConfirmationItem> = open_questions
                    .iter()
                    .enumerate()
                    .map(|(i, q)| ConfirmationItem {
                        id: format!("q{}", i + 1),
                        label: q.clone(),
                        input_type: "text".to_string(),
                        options: Vec::new(),
                        required: false,
                        placeholder: None,
                    })
                    .collect();
                let conf = ConfirmationRequest {
                    request_id: request_id.clone(),
                    task_id: String::new(),
                    title: "目标确认".to_string(),
                    // 确认请求 prompt 仅作简短引导；目标与待确认问题由 notice 承载。
                    prompt: "请确认以上【目标理解】中的待确认问题，或直接补充说明（若已提供材料位置，主持人会先核验材料再判断）。".to_string(),
                    reply_mode: "open".to_string(),
                    items,
                };
                let extra = serde_json::to_string(&conf).unwrap_or_else(|_| "{}".into());
                let req_msg = self
                    .persist_message_extra(
                        &director_sender,
                        &[],
                        "confirmation_request",
                        None,
                        &conf.prompt,
                        &[],
                        "",
                        "",
                        &extra,
                    )
                    .await;
                self.emit(GroupChatEvent::message(&self.room_id, req_msg));

                let reply = self
                    .await_confirmation_response(&request_id, &conf.items)
                    .await;
                // 结构化/开放式回复统一取其内容；等待被中断则为 None。
                let reply_content: Option<String> = match reply {
                    Some(ConfirmationReply::Structured { content, .. }) => Some(content),
                    Some(ConfirmationReply::Open { content, .. }) => Some(content),
                    None => None,
                };
                let Some(reply_content) = reply_content else {
                    // 等待被中断（停止/超时）：break 退出消化循环，以当前理解落位并编排，不阻塞用户。
                    break;
                };
                // 用户回复落库 + 写入记忆（供后续上下文引用）。
                let resp_extra = serde_json::json!({ "responses": [] }).to_string();
                let user_sender = self.user_id.clone();
                let resp_msg = self
                    .persist_message_extra(
                        &user_sender,
                        &[],
                        "confirmation_response",
                        Some(request_id.as_str()),
                        &reply_content,
                        &[],
                        "",
                        "",
                        &resp_extra,
                    )
                    .await;
                self.emit(GroupChatEvent::message(&self.room_id, resp_msg.clone()));
                let pids = self.all_participant_ids();
                self.memory.apply_message(&resp_msg, &pids);

                let notes_ctx = self.combined_notes();
                // 意图分派：用户可能并非逐条回答，而是给出新指令/改方向 → 重新进入目标理解。
                let intent = match self.director.clone() {
                    Some(d) => call_director_retry(|| {
                        d.classify_reply_intent(&questions_text, &reply_content)
                    })
                    .await
                    .unwrap_or_else(|| "confirm".to_string()),
                    None => "confirm".to_string(),
                };
                if intent == "directive" {
                    // 用户改方向/追加指令：以回复为最新输入重新理解目标（可能再次需要确认）。
                    let rc = match self.director.clone() {
                        Some(d) => {
                            call_director_retry(|| {
                                d.clarify_goal(
                                    &self.room.topic,
                                    &reply_content,
                                    &notes_ctx,
                                    &roster_text,
                                )
                            })
                            .await
                        }
                        None => None,
                    };
                    match rc {
                        Some(c) => {
                            current_goal = c.goal;
                            if !c.output_dir_name.trim().is_empty() {
                                current_dir_name = c.output_dir_name;
                            }
                            if c.need_confirmation && !c.questions.is_empty() {
                                open_questions = c.questions;
                                reasoning.clear();
                                continue; // 按新方向的问题继续追问
                            }
                            settled = true;
                        }
                        None => settled = true, // 重理解失败：以当前目标落位（不阻塞）。
                    }
                    continue;
                }

                // confirm 消化（v3.5c）：先做材料核验侦查（回复指向目录/文件时主持人只读核验
                // 并取回简报），再把回复 + 简报交由 Director 智能消化：更新答案/问题项/目标方向，
                // 并判定是否需要继续向用户追问剩余关键问题。
                let evidence = self
                    .director_scout(&format!(
                        "目标消化核验：用户为确认目标给出的回复可能指向了材料位置。请核验回复中提到的目录/文件/仓库，读取必要材料，判断其中是否包含回答以下待确认问题所需的信息：\n当前目标：{}\n用户回复：{}\n待确认问题：{}",
                        current_goal, reply_content, questions_text
                    ))
                    .await;
                let cr = match self.director.clone() {
                    Some(d) => {
                        call_director_retry(|| {
                            d.clarify_reply(
                                &current_goal,
                                &reply_content,
                                &notes_ctx,
                                &roster_text,
                                &evidence,
                            )
                        })
                        .await
                    }
                    None => None,
                };
                match cr {
                    Some(c) => {
                        // 消化成功：更新目标与目录名；消化确认要点累积供后续追问回显；
                        // 仅当仍缺关键信息且模型判定需继续才再追问。
                        current_goal = c.adjusted_goal;
                        if !c.resolved.is_empty() {
                            resolved_acc.extend(c.resolved);
                        }
                        if !c.output_dir_name.trim().is_empty() {
                            current_dir_name = c.output_dir_name;
                        }
                        if c.need_more && !c.remaining_questions.is_empty() {
                            open_questions = c.remaining_questions;
                            reasoning.clear();
                            continue; // 下一轮仅追问剩余问题（不再重复已确认项）
                        }
                        settled = true;
                    }
                    None => {
                        // Director 消化失败兜底：只摘录摘要并入目标，不无限打扰用户。
                        current_goal = format!(
                            "{}\n\n【用户补充】\n{}",
                            current_goal,
                            summarize_for_goal(&reply_content)
                        );
                        settled = true;
                    }
                }
            }
            // 消化循环结束：落位整理后的总目标并进入编排。
            self.set_room_topic(&current_goal).await;
            self.resolve_and_persist_output_dir(&current_goal, &current_dir_name)
                .await;
            self.kickoff_first(&current_goal).await
        } else {
            // 目标清晰：落【目标理解】notice 供用户可见可纠正，落位 topic 后直接编排。
            let notice = format!("【目标理解】我已理解你的目标：\n{}", goal);
            let msg = self
                .persist_message(
                    &director_sender,
                    &[],
                    "director_notice",
                    None,
                    &notice,
                    &[],
                    "",
                    &clarify_reasoning,
                )
                .await;
            self.memory.apply_message(&msg, &self.all_participant_ids());
            self.emit(GroupChatEvent::message(&self.room_id, msg));

            self.set_room_topic(&goal).await;
            self.resolve_and_persist_output_dir(&goal, &dir_name).await;
            self.kickoff_first(&goal).await
        }
    }

    /// 落位房间目标锚点（topic）：阶段A 整理确认后的总目标，不覆盖为原文。
    async fn set_room_topic(&mut self, topic: &str) {
        let topic = topic.trim().to_string();
        if topic.is_empty() || topic == self.room.topic {
            return;
        }
        if let Ok(conn) = self.conn() {
            let _ = store::update_room_topic(&conn, &self.room_id, &topic);
        }
        self.room.topic = topic;
    }

    /// 主持人侦查（v3.5c B 层）：让主持人以工具方式核验材料/产物/工作区，为决策收集证据。
    /// 复用「主持人亲自执行」的工具化执行器：审批由当前安全模式驱动，需要审批的工具一律
    /// 拒绝（不自审），无需审批的工具可调用；并注入只读约束。返回 ≤SCOUT_MAX_CHARS 的事实简报；
    /// 无主持人/失败/超时返回空串（调用方降级为不注入证据，不影响决策主链路）。
    async fn director_scout(&mut self, mission: &str) -> String {
        const SCOUT_MAX_CHARS: usize = 4000;
        const SCOUT_TIMEOUT_SECS: u64 = 90;
        if self.director.is_none() {
            return String::new();
        }
        let executor = {
            let Ok(conn) = self.conn() else {
                return String::new();
            };
            Self::build_director_executor(
                &conn,
                &self.pool,
                &self.room_id,
                &self.app,
                &self
                    .director_id
                    .clone()
                    .unwrap_or_else(|| "director".to_string()),
                Some(self.control.abort.clone()),
                self.mcp.clone(),
                self.security_mode,
                &self.output_dir,
            )
        };
        let Some(executor) = executor else {
            return String::new();
        };
        let prompt = format!(
            "【主持人侦查任务（事实核验）】\n{}\n\n核验完成后直接输出客观简报：列出你实际查看到的材料内容要点、关键文件与事实信息，不要给出建议或结论。",
            mission
        );
        let view = TurnView {
            topic: mission.to_string(),
            summary: String::new(),
            stances: Vec::new(),
            messages: vec![ChatMessage::user(&prompt)],
            system_role: "主持人侦查".into(),
            task_context: String::new(),
            roster: Vec::new(),
            user_language: String::new(),
            output_dir: self.output_dir.clone(),
        };
        match tokio::time::timeout(
            Duration::from_secs(SCOUT_TIMEOUT_SECS),
            executor.run_turn(view, None, None),
        )
        .await
        {
            Ok(r) if r.error.is_none() && !r.content.trim().is_empty() => {
                let text = r.content.trim().to_string();
                if text.chars().count() > SCOUT_MAX_CHARS {
                    text.chars().take(SCOUT_MAX_CHARS).collect()
                } else {
                    text
                }
            }
            Ok(_) => String::new(),
            Err(_) => {
                log::warn!(
                    "[GroupChat] 主持人侦查超时（{}s），降级为无证据",
                    SCOUT_TIMEOUT_SECS
                );
                String::new()
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

        let mut tasks = Vec::new();
        let mut role_map: HashMap<String, String> = HashMap::new();
        let mut rejected_roles: Vec<(String, String)> = Vec::new();
        let mut roster_lines: Vec<String> = Vec::new();
        let roster_catalog = self.format_roster_catalog();
        let participant_failures = self.format_participant_failures();
        // 初始批量组建名册：不设单次数量上限，仅受房间剩余空位约束（注入空位避免超量提议浪费轮次）。
        let max_new_participants = self.remaining_participant_slots();
        // B 层注入：用户输入物解析结果并入 Director 上下文（notes）。
        let mut notes = self.combined_notes();
        // v3.5c 编排取证：正式拆分任务/指派角色前先侦查工作区与用户材料（含半成品位置/输入物），
        // 收集影响任务拆分与角色定位的事实（已有内容/技术栈/缺口），简报并入 notes 供 kickoff 更准。
        let scout_mission = format!(
            "任务编排取证：在划分任务与指派角色前，请核验工作区与用户提供的材料（含半成品目录/输入物位置/产物目录），读取必要文件，收集影响任务拆分与角色定位的关键事实（已有内容、技术栈、依赖、缺口等）。用户目标：{}",
            goal
        );
        let evidence = self.director_scout(&scout_mission).await;
        if !evidence.is_empty() {
            notes.push_str(&format!(
                "\n\n【主持人侦查简报】（编排取证，只读核验所得事实）\n{}",
                evidence
            ));
        }
        if let Some(director) = &self.director {
            if let Some(plan) = call_director_retry(|| {
                director.kickoff(
                    &self.room.topic,
                    goal,
                    &notes,
                    &roster,
                    &roster_catalog,
                    max_new_participants,
                    &participant_failures,
                )
            })
            .await
            {
                let KickoffPlan {
                    tasks: drafts,
                    roles,
                    new_participants,
                    missing_roles,
                } = plan;
                // v3.4as：Director 未覆盖角色的参与者由代码兜底补"未分配"，并落提示便于追溯。
                if !missing_roles.is_empty() {
                    let names = missing_roles
                        .iter()
                        .map(|id| format!("[@{}]", id))
                        .collect::<Vec<_>>()
                        .join("、");
                    let notice = format!(
                        "编排提示：以下参与者未获得角色分配，已按\"未分配\"兜底：{}",
                        names
                    );
                    let director_sender = self.director_id.clone().unwrap_or_default();
                    let msg = self
                        .persist_message(
                            &director_sender,
                            &[],
                            "director_notice",
                            None,
                            &notice,
                            &[],
                            "",
                            "",
                        )
                        .await;
                    self.memory.apply_message(&msg, &self.all_participant_ids());
                    self.emit(GroupChatEvent::message(&self.room_id, msg));
                }
                // 按需组建名册：主持人发现参与者不足时自动补充（先落库 + 重建运行时，
                // 再应用任务/角色，确保 assignee/role 校验能命中新参与者）。
                if !new_participants.is_empty() {
                    let (created, rejected) =
                        self.create_auto_participants(&new_participants, max_new_participants);
                    if !created.is_empty() {
                        self.rebuild_participants().await;
                        for c in &created {
                            let draft = new_participants.iter().find(|p| p.id == *c);
                            let role = draft.map(|p| p.system_role.clone()).unwrap_or_default();
                            let reason = draft.map(|p| p.reason.clone()).unwrap_or_default();
                            let reason_part = if reason.is_empty() {
                                String::new()
                            } else {
                                format!("（原因：{}）", reason)
                            };
                            roster_lines
                                .push(format!("- [@{}]（角色：{}）{}", c, role, reason_part));
                        }
                    }
                    for r in &rejected {
                        roster_lines.push(format!("- 拒绝：{}", r));
                    }
                }
                role_map = roles;
                for (i, d) in drafts.iter().enumerate() {
                    let assignee = d
                        .assignee
                        .clone()
                        .map(|id| normalize_participant_ref(&id))
                        .filter(|id| {
                            self.participants.contains_key(id) && !self.is_chronic_failer(id)
                        });
                    // Director 返回批内下标，偏移为全量任务表绝对下标（归档的旧任务仍占下标）。
                    let depends_abs: Vec<usize> =
                        d.depends_on.iter().map(|p| p + existing_count).collect();
                    let task = TaskRow {
                        id: uuid::Uuid::new_v4().to_string(),
                        room_id: self.room_id.clone(),
                        task_no: (existing_count + i + 1) as i64,
                        description: d.description.clone(),
                        assignee,
                        depends_on: serde_json::to_string(&depends_abs)
                            .unwrap_or_else(|_| "[]".into()),
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
                // 应用角色分配（白名单校验：协调类角色仅限 Director 持有）。
                rejected_roles = self.apply_role_assignments(&role_map).await;
            }

            // 主席开场调度信息以正式发言方式显示在消息流中（自动补充参与者 + 任务清单 + 角色指派）。
            if !tasks.is_empty() || !roster_lines.is_empty() {
                let mut lines: Vec<String> = vec!["【调度】".to_string()];
                if !roster_lines.is_empty() {
                    lines.push(String::new());
                    lines.push("自动补充参与者：".to_string());
                    lines.extend(roster_lines.iter().cloned());
                }
                if !tasks.is_empty() {
                    lines.push(String::new());
                    lines.push("任务指派：".to_string());
                    for t in &tasks {
                        let assignee = t
                            .assignee
                            .clone()
                            .map(|a| format!("[@{}]", a))
                            .unwrap_or_else(|| "待指派".to_string());
                        lines.push(format!(
                            "- T{}. {}（负责人：{}）",
                            t.task_no, t.description, assignee
                        ));
                    }
                }
                if !role_map.is_empty() {
                    lines.push(String::new());
                    lines.push("角色指派：".to_string());
                    for (pid, role) in &role_map {
                        if rejected_roles.iter().any(|(rp, _)| rp == pid) {
                            continue;
                        }
                        lines.push(format!("- [@{}] → {}", pid, role));
                    }
                    if !rejected_roles.is_empty() {
                        lines.push(String::new());
                        lines.push("被拒绝（协调类角色仅限 Director 持有）：".to_string());
                        for (pid, role) in &rejected_roles {
                            lines.push(format!("- [@{}] → {}", pid, role));
                        }
                    }
                }
                let schedule_text = lines.join("\n");
                let director_sender = self
                    .director_id
                    .clone()
                    .unwrap_or_else(|| "director".to_string());
                let schedule_msg = self
                    .persist_message(
                        &director_sender,
                        &[],
                        "task_assignment",
                        None,
                        &schedule_text,
                        &[],
                        "",
                        "",
                    )
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
            // 理论不会发生（Refined 仅在已有任务时进入）；兜底走首轮（含阶段A目标整理）。
            return self.clarify_and_kickoff(latest, latest).await;
        }

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

        let roster_catalog = self.format_roster_catalog();
        let participant_failures = self.format_participant_failures();
        // 讨论中补充以 replan 上限为准（与剩余空位取小），注入提示词避免超量提议浪费轮次。
        let max_new_participants = self
            .remaining_participant_slots()
            .min(MAX_AUTO_NEW_PARTICIPANTS_REPLAN);
        // B 层注入：用户输入物解析结果并入 Director 上下文（notes）。
        let notes = self.combined_notes();
        let plan = match &self.director {
            Some(d) => {
                call_director_retry(|| {
                    d.replan(
                        &self.room.topic,
                        latest,
                        &notes,
                        &existing,
                        &roster,
                        &roster_catalog,
                        max_new_participants,
                        &participant_failures,
                    )
                })
                .await
            }
            None => None,
        };

        let Some(plan) = plan else {
            // Director 不可用或对账失败：保守策略，不新增、不删减、不重分配。
            return KickoffResult { added: 0 };
        };

        // 按需组建名册：讨论中发现缺角色时主持人自动补充（先落库 + 重建运行时，
        // 再应用对账操作/角色，确保 reassign/role 校验能命中新参与者）。
        let mut roster_lines: Vec<String> = Vec::new();
        if !plan.new_participants.is_empty() {
            let (created, rejected) = self
                .create_auto_participants(&plan.new_participants, MAX_AUTO_NEW_PARTICIPANTS_REPLAN);
            if !created.is_empty() {
                self.rebuild_participants().await;
                for c in &created {
                    let draft = plan.new_participants.iter().find(|p| p.id == *c);
                    let role = draft.map(|p| p.system_role.clone()).unwrap_or_default();
                    let reason = draft.map(|p| p.reason.clone()).unwrap_or_default();
                    let reason_part = if reason.is_empty() {
                        String::new()
                    } else {
                        format!("（原因：{}）", reason)
                    };
                    roster_lines.push(format!("- [@{}]（角色：{}）{}", c, role, reason_part));
                }
            }
            for r in &rejected {
                roster_lines.push(format!("- 拒绝：{}", r));
            }
        }

        let app = self
            .apply_replan_operations(&existing, &plan.operations)
            .await;

        // 应用角色调整（仅 replan 明确要求时）：统一走白名单校验，协调类角色仅限 Director。
        let mut role_lines: Vec<String> = Vec::new();
        let rejected_roles = self.apply_role_assignments(&plan.roles).await;
        for (pid, role) in &plan.roles {
            if rejected_roles.iter().any(|(rp, _)| rp == pid) {
                continue;
            }
            role_lines.push(format!("- [@{}] → {}", pid, role));
        }
        for (pid, role) in &rejected_roles {
            role_lines.push(format!(
                "- [@{}] → {}（已拒绝：协调类角色仅限 Director 持有）",
                pid, role
            ));
        }

        // 对账调度可追溯消息（有变化时）
        let had_role_changes = !role_lines.is_empty();
        if !app.added.is_empty()
            || app.removed != 0
            || app.reassigned != 0
            || had_role_changes
            || !roster_lines.is_empty()
        {
            let mut lines: Vec<String> = vec!["【调度】对账".to_string()];
            if !roster_lines.is_empty() {
                lines.push(String::new());
                lines.push("自动补充参与者：".to_string());
                lines.extend(roster_lines.iter().cloned());
            }
            if had_role_changes {
                lines.push(String::new());
                lines.push("角色调整：".to_string());
                lines.extend(role_lines);
            }
            if app.removed != 0 {
                lines.push(String::new());
                lines.push(format!("移除冗余任务 {} 项", app.removed));
            }
            if app.reassigned != 0 {
                lines.push(String::new());
                lines.push(format!("重分配任务 {} 项", app.reassigned));
            }
            if !app.added.is_empty() {
                lines.push(String::new());
                lines.push("新增任务：".to_string());
                for t in &app.added {
                    let assignee = t
                        .assignee
                        .clone()
                        .map(|a| format!("[@{}]", a))
                        .unwrap_or_else(|| "待指派".to_string());
                    lines.push(format!(
                        "- T{}. {}（负责人：{}）",
                        t.task_no, t.description, assignee
                    ));
                }
            }
            let director_sender = self
                .director_id
                .clone()
                .unwrap_or_else(|| "director".to_string());
            let msg = self
                .persist_message(
                    &director_sender,
                    &[],
                    "task_assignment",
                    None,
                    &lines.join("\n"),
                    &[],
                    "",
                    "",
                )
                .await;
            self.emit(GroupChatEvent::message(&self.room_id, msg));
        }

        // 空变化自检：用户明确提出新指令但重排无任何任务调整时，落可追溯提示，
        // 避免「主持人无视插队」的静默（refine 类规范约束本身可能不引起任务变化，仅提示不阻断）。
        if app.added.is_empty()
            && app.removed == 0
            && app.reassigned == 0
            && !had_role_changes
            && roster_lines.is_empty()
            && !latest.trim().is_empty()
            && latest != self.room.topic
        {
            let director_sender = self
                .director_id
                .clone()
                .unwrap_or_else(|| "director".to_string());
            let text = format!(
                "用户最新指令未引起任务调整：\n{}\n（若该指令需要落实为具体任务，请再次明确指示）",
                latest
            );
            let msg = self
                .persist_message(
                    &director_sender,
                    &[],
                    "director_notice",
                    None,
                    &text,
                    &[],
                    "",
                    "",
                )
                .await;
            self.emit(GroupChatEvent::message(&self.room_id, msg));
        }

        if let Some(first) = app.added.first() {
            if let Ok(conn) = self.conn() {
                let _ = store::update_room_current_task(&conn, &self.room_id, &first.id);
            }
        }

        KickoffResult {
            added: app.added.len(),
        }
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
                            // 宽容归一化：模型可能回显带 [@] 括号的引用，剥离后匹配参与者。
                            if let Some(ra) = op
                                .reassign
                                .clone()
                                .map(|id| normalize_participant_ref(&id))
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
            let Some(&root_idx) = id_to_index.get(root_id) else {
                continue;
            };
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

        // 3) 重分配（仅 discussing/pending/running，且未被级联删减；running 重派在指令中断执行重跑后生效）
        for (idx, new_assignee) in &reassigns {
            if removed.contains(idx) {
                continue;
            }
            let mut t = existing[*idx].clone();
            if t.status == "discussing" || t.status == "pending" || t.status == "running" {
                t.assignee = Some(new_assignee.clone());
                if let Ok(conn) = self.conn() {
                    let _ = store::update_task(&conn, &t);
                }
                self.emit(GroupChatEvent::task_updated(&self.room_id, t));
            }
        }

        // 4) 新增任务（depends_on 可引用已有任务 id，或本批 add 中排在本条之前的任务 id——v3.4an 放开本批依赖）。
        // 先预创建本批任务（生成 id / task_no 并登记批内索引），再统一解析依赖，最后落库。
        let mut new_tasks: Vec<TaskRow> = Vec::new();
        let mut batch_index: HashMap<String, usize> = HashMap::new();
        for (i, op) in adds.iter().enumerate() {
            let task_id = uuid::Uuid::new_v4().to_string();
            batch_index.insert(task_id.clone(), existing.len() + i);
            new_tasks.push(TaskRow {
                id: task_id,
                room_id: self.room_id.clone(),
                task_no: (existing.len() + i + 1) as i64,
                description: op.description.clone(),
                assignee: op
                    .assignee
                    .clone()
                    .map(|id| normalize_participant_ref(&id))
                    .filter(|id| self.participants.contains_key(id) && !self.is_chronic_failer(id)),
                depends_on: "[]".into(),
                status: "discussing".into(),
                result_summary: None,
                error: None,
                started_at: None,
                completed_at: None,
            });
        }
        for (i, op) in adds.iter().enumerate() {
            let mut depends_abs: Vec<usize> = Vec::new();
            for dep_id in &op.depends_on {
                let mut dep_idx = id_to_index.get(dep_id).copied();
                if dep_idx.is_none() {
                    // 本批内依赖：仅允许引用批内位置小于当前任务的先发任务，防前向/环形依赖。
                    if let Some(&idx) = batch_index.get(dep_id) {
                        if idx < existing.len() + i {
                            dep_idx = Some(idx);
                        }
                    }
                }
                if let Some(idx) = dep_idx {
                    if !depends_abs.contains(&idx) {
                        depends_abs.push(idx);
                    }
                }
            }
            let mut t = new_tasks[i].clone();
            t.depends_on = serde_json::to_string(&depends_abs).unwrap_or_else(|_| "[]".into());
            new_tasks[i] = t;
        }
        for task in &new_tasks {
            if let Ok(conn) = self.conn() {
                let _ = store::insert_task(&conn, task);
            }
            self.emit(GroupChatEvent::task_updated(&self.room_id, task.clone()));
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
        self.replan_after_roster_change("【调度】参与者名册变更，已重新编排")
            .await;
    }

    /// 名册/主持人变化后的统一重排（调用方需先 rebuild_participants）：
    /// Director 对账重排角色与任务，并落可追溯调度消息。
    async fn replan_after_roster_change(&mut self, notice: &str) {
        // 名册变化改变讨论结构，重置收敛计数，避免旧名册结论过早收敛。
        self.rules.reset();

        // 尚无任务：下一次 kickoff_first 自然纳入新名册，无需单独重排。
        if self.task_count() == 0 {
            return;
        }

        let Some(director) = self.director.clone() else {
            return;
        };
        let Ok(conn) = self.conn() else {
            return;
        };
        let Ok(existing) = store::list_tasks(&conn, &self.room_id) else {
            return;
        };

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

        // 应用角色分配（白名单校验：协调类角色仅限 Director 持有）
        let mut role_lines: Vec<String> = Vec::new();
        let rejected_roles = self.apply_role_assignments(&plan.roles).await;
        for (pid, role) in &plan.roles {
            if rejected_roles.iter().any(|(rp, _)| rp == pid) {
                continue;
            }
            role_lines.push(format!("- [@{}] → {}", pid, role));
        }
        for (pid, role) in &rejected_roles {
            role_lines.push(format!(
                "- [@{}] → {}（已拒绝：协调类角色仅限 Director 持有）",
                pid, role
            ));
        }

        // 应用任务重排
        let app = self
            .apply_replan_operations(&existing, &plan.operations)
            .await;

        // 名册变更调度消息（可追溯）
        // V3.5c：每段前补空行分隔（「新增任务：」不再直接拼接在最后一行角色分配后）。
        let mut lines: Vec<String> = vec![notice.to_string()];
        if !role_lines.is_empty() {
            lines.push(String::new());
            lines.push("角色分配：".to_string());
            lines.extend(role_lines);
        }
        if app.removed != 0 {
            lines.push(String::new());
            lines.push(format!("移除冗余任务 {} 项", app.removed));
        }
        if app.reassigned != 0 {
            lines.push(String::new());
            lines.push(format!("重分配任务 {} 项", app.reassigned));
        }
        if !app.added.is_empty() {
            lines.push(String::new());
            lines.push("新增任务：".to_string());
            for t in &app.added {
                let assignee = t
                    .assignee
                    .clone()
                    .map(|a| format!("[@{}]", a))
                    .unwrap_or_else(|| "待指派".to_string());
                lines.push(format!(
                    "- T{}. {}（负责人：{}）",
                    t.task_no, t.description, assignee
                ));
            }
        }
        let director_sender = self.director_sender();
        let msg = self
            .persist_message(
                &director_sender,
                &[],
                "task_assignment",
                None,
                &lines.join("\n"),
                &[],
                "",
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, msg));

        if let Some(first) = app.added.first() {
            if let Ok(conn) = self.conn() {
                let _ = store::update_room_current_task(&conn, &self.room_id, &first.id);
            }
        }
    }

    /// 主持人失活恢复：由现有任意 API 参与者（角色定义保持不变）决策创建一名新主持人并热替换，
    /// 恢复房间主持人调度能力。不将任何现有参与者原地升任为主持人（其角色会因此失效），
    /// 也不做轮询兜底（无主持人即无调度，轮询不科学）。尝试上限 DIRECTOR_RECOVER_LIMIT。
    ///
    /// 流程：为每个候选 API 参与者（按名册顺序，最多 DIRECTOR_RECOVER_LIMIT 人，每人 1 次机会）：
    /// 1) 用其 agent_config 构建一次性无工具决策客户端（不进入其会话、不改其身份）；
    /// 2) 注入房间目标/任务清单/可用模型目录/失败记录，请其输出新主持人配置（api 类型）；
    /// 3) 校验 provider/key/模型白名单（复用与补人的同一套校验语义）；
    /// 4) 新增一条 participant_type="director" 参与者行，原主持人行保留（不删除、不降级；
    ///    仅通过 update_room_director 把房间指向新行），热替换并重建运行时。
    /// 任一候选成功即返回可追溯提示；全部失败返回失败提示（调用方落库后诚实收敛结束）。
    async fn recover_director_via_api_agent(&mut self) -> String {
        let Ok(conn) = self.conn() else {
            return "主持人失活且数据库不可用，无法触发恢复流程".to_string();
        };
        let Ok(rows) = store::list_participants(&conn, &self.room_id) else {
            return "主持人失活且名册读取失败，无法触发恢复流程".to_string();
        };
        // 候选执行者：现有 api 参与者（非慢性失败者）。
        let executors: Vec<ParticipantRow> = rows
            .iter()
            .filter(|r| r.participant_type == "api")
            .filter(|r| !self.is_chronic_failer(&r.id))
            .cloned()
            .collect();
        if executors.is_empty() {
            return "主持人失活，且房间内无可用 API 参与者执行恢复决策，讨论诚实收敛结束"
                .to_string();
        }

        let cwd = crate::utils::paths::resolve_workspace_path(None, "", &conn)
            .to_string_lossy()
            .to_string();
        // 名册段文本（id / 显示名 / 角色），供恢复决策避免重复 provider 的单点故障。
        let roster_text = self
            .participant_roster()
            .into_iter()
            .map(|(id, name, role)| {
                let role = if role.trim().is_empty() {
                    "未分配"
                } else {
                    role.as_str()
                };
                format!("- [@{}]（显示名：{}）（角色：{}）", id, name, role)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let roster_text = if roster_text.is_empty() {
            "（暂无参与者）".to_string()
        } else {
            roster_text
        };
        self.ensure_task_assignees();
        let task_manifest = self.build_task_manifest();
        let catalog = self.format_roster_catalog();
        let failures_text = self.format_participant_failures();
        let old_director_id = self
            .director_id
            .clone()
            .unwrap_or_else(|| "director".to_string());

        let mut reasons: Vec<String> = Vec::new();
        for (idx, exec) in executors.iter().take(DIRECTOR_RECOVER_LIMIT).enumerate() {
            // 1) 用执行者自身配置构建一次性决策客户端（无工具；不加载审批/续跑回调，避免自我裁决）。
            let Some(llm) = build_llm_client(
                &conn,
                &self.pool,
                &self.room.id,
                &exec.agent_config,
                &self.app,
                &cwd,
                false,
                None,
                None,
                self.mcp.clone(),
                self.security_mode,
            ) else {
                reasons.push(format!("[@{}] 决策客户端构建失败", exec.id));
                continue;
            };

            // 2) 组装恢复决策提示词（提示词块资产）。
            let prompt = super::prompts::render(
                super::prompts::blocks::EXECUTOR_RECOVER_DIRECTOR.body,
                &super::prompts::PromptCtx::new()
                    .var("executor_id", &exec.id)
                    .section("topic", &self.room.topic)
                    .section("task_manifest", &task_manifest)
                    .section("roster", &roster_text)
                    .section("catalog", &catalog)
                    .section("failures", &failures_text),
            );
            // 单次恢复决策调用（与 Director 调用一致：确定性超时 + 同客户端重试；返回 Option 便于熔断计数）。
            let raw = call_director_retry(|| async {
                llm.complete("", &[ChatMessage::user(&prompt)], None)
                    .await
                    .ok()
            })
            .await;
            let Some(raw) = raw else {
                reasons.push(format!("[@{}] 恢复决策调用失败/超时", exec.id));
                continue;
            };

            // 3) 解析新主持人配置草案（宽松提取；不合法则跳过该候选）。
            let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
                reasons.push(format!("[@{}] 恢复决策输出非 JSON", exec.id));
                continue;
            };
            let Some(provider) = v["provider"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                reasons.push(format!("[@{}] 未提供新主持人 provider", exec.id));
                continue;
            };
            let Some(model) = v["model"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                reasons.push(format!("[@{}] 未提供新主持人 model", exec.id));
                continue;
            };
            let Some(display_name) = v["display_name"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                reasons.push(format!("[@{}] 未提供新主持人显示名", exec.id));
                continue;
            };
            let system_role = v["system_role"].as_str().map(str::trim).unwrap_or("");

            // provider/key/模型白名单校验（与 create_auto_participants 的 api 校验同语义）。
            let Ok(Some(provider_row)) =
                crate::commands::api_provider::get_api_provider(&conn, provider)
            else {
                reasons.push(format!(
                    "[@{}] 提供商 '{}' 不存在或未配置",
                    exec.id, provider
                ));
                continue;
            };
            let key_ok = crate::commands::api_provider::get_api_key(&conn, provider)
                .ok()
                .flatten()
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            if !key_ok {
                reasons.push(format!("[@{}] 提供商 '{}' 未配置 key", exec.id, provider));
                continue;
            }
            if !provider_row.models.is_empty() && !provider_row.models.iter().any(|m| m == model) {
                reasons.push(format!(
                    "[@{}] 模型 '{}' 不在提供商 '{}' 的模型清单中",
                    exec.id, model, provider
                ));
                continue;
            }

            // 4) 落库：新增 director 行（id 唯一性校验），原主持人行保留。房间 director_id 指向新行。
            let new_id = format!("new_director_{}", idx + 1);
            if rows.iter().any(|r| r.id == new_id) {
                reasons.push(format!("[@{}] 新主持人 id '{}' 冲突", exec.id, new_id));
                continue;
            }
            let agent_config =
                serde_json::json!({ "provider": provider, "model": model }).to_string();
            let new_row = ParticipantRow {
                id: new_id.clone(),
                room_id: self.room_id.clone(),
                participant_type: "director".into(),
                agent_config: agent_config.clone(),
                display_name: display_name.to_string(),
                system_role: system_role.to_string(),
                status: "active".into(),
            };
            // 落库单点：任一失败视为该候选不可用（新行/原行/房间指向全部成功才算）。
            let mut db_ok = store::insert_participant(&conn, &new_row).is_ok();
            if db_ok {
                db_ok = store::update_room_director(&conn, &self.room_id, &new_id).is_ok();
            }
            if !db_ok {
                let _ = store::delete_participant(&conn, &self.room_id, &new_id);
                reasons.push(format!("[@{}] 新主持人落库失败", exec.id));
                continue;
            }

            // 5) 热替换：重建运行时（新行成为唯一 director），同步共享引用。
            self.rebuild_participants().await;
            if self.director.is_none() || self.director_id.as_deref() != Some(&new_id) {
                // 新主持人构建失败：回滚（删新行，房间 director_id 还原原行）。
                let _ = store::delete_participant(&conn, &self.room_id, &new_id);
                let _ = store::update_room_director(&conn, &self.room_id, &old_director_id);
                self.director_id = Some(old_director_id.clone());
                self.rebuild_participants().await;
                reasons.push(format!("[@{}] 新主持人构建不可用，已回滚", exec.id));
                continue;
            }

            return format!(
                "主持人连续失败 {} 次，已由现有 API 参与者 [@{}] 创建并任命新主持人 [@{}]（provider={}），调度已恢复",
                DIRECTOR_FAIL_THRESHOLD, exec.id, new_id, provider
            );
        }

        // 全部候选失败：无主持人意味着所有调度逻辑被破坏，诚实收敛结束（不轮询）。
        format!(
            "主持人无法恢复（连续失败 {} 次；尝试 {{}} 名候选，均失败），讨论诚实收敛结束",
            DIRECTOR_FAIL_THRESHOLD
        )
        .replacen("{{}}", &reasons.join("；"), 1)
    }

    async fn discuss(&mut self) -> bool {
        // B 层注入：统一产物目录/用户输入物并入 Director 上下文（本轮讨论期间稳定）。
        let notes = self.combined_notes();
        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                return false;
            }
            self.control.wait_if_paused().await;

            // 下一检查点：检测用户插队，发现新指令则处理并中断讨论，交由上层重跑。
            if let Some(RoomCommand::Send {
                sender,
                content,
                recipients,
                reply_to,
                mention,
                attachments,
                security_mode,
            }) = self.drain_pending_commands().await
            {
                self.security_mode = security_mode;
                self.process_user_directive(
                    &sender,
                    &content,
                    &recipients,
                    reply_to.as_deref(),
                    mention.as_deref(),
                    &attachments,
                )
                .await;
                return true;
            }

            // 确定性收敛兜底：所有任务均已终态且无新增，直接结束讨论，避免空转。
            // v3.4bp 分层执行：存在任务但就绪层为空（依赖环/前置未成功导致无可讨论）时同样退出讨论，
            // 交由 execute_tasks 执行就绪层或按依赖失败将阻塞任务置 skipped，避免讨论阶段空转。
            // 空任务集不算就绪层空（初始化/编排尚未产生任务时讨论照常进行）。
            if self.all_tasks_terminal() || (self.has_any_tasks() && self.ready_layer_empty()) {
                return false;
            }

            // 选下一发言者：用户 @ 指定优先；否则 Director 决策（带重试），失败走轮询兜底。
            // 注入最近发言上下文，供 Director 判断任务是否已被实质推进（已推进则不再指派该任务）。
            // 注：主持人失活不再走「候选轮流接管/轮询」，恢复流程在审阅失败分支内一次性完成。
            let mention = self.pending_mention.take();
            self.ensure_task_assignees();
            let task_manifest = self.build_task_manifest();
            // 预绑定临时值（闭包内自属临时会被 future 借用导致 E0515；提前绑定后闭包只引用局部）。
            let memory_summary = self.memory.summary();
            let stances = self.memory.stances();
            let failures_text = self.format_participant_failures();
            let recent_speeches = self.format_recent_speeches(3);
            // 补人清单注入：主持人发现名册缺角色/立场时可提出补人提案（new_participants）。
            let catalog_text = self.format_roster_catalog();
            let max_new_participants = self
                .remaining_participant_slots()
                .min(MAX_AUTO_NEW_PARTICIPANTS_REPLAN);
            // 名册三字段（id/显示名/角色）预绑定：选人必须感知参与者固定身份，避免主持人臆造角色。
            let roster = self.participant_roster();
            let decision = match mention.as_ref() {
                Some(m) if self.floor_order.iter().any(|id| id == m) => Some(SpeakerDecision {
                    speaker: m.clone(),
                    reason: "用户 @ 指定".into(),
                    work_content: String::new(),
                    new_participants: Vec::new(),
                }),
                _ => match &self.director {
                    Some(d) => {
                        call_director_retry(|| {
                            d.next_speaker(
                                &self.room.topic,
                                &memory_summary,
                                &roster,
                                &task_manifest,
                                &stances,
                                mention.as_deref(),
                                &self.last_user_directive,
                                &notes,
                                &failures_text,
                                &recent_speeches,
                                &catalog_text,
                                max_new_participants,
                            )
                        })
                        .await
                    }
                    None => None,
                },
            };
            // 主持人提出补人提案（名册缺角色/立场）→ 由系统创建专用参与者并重排，替代"让现有参与者临时扮演"。
            if let Some(dec) = &decision {
                if !dec.new_participants.is_empty() {
                    let (created, rejected) =
                        self.create_auto_participants(&dec.new_participants, max_new_participants);
                    let mut lines: Vec<String> = vec!["【调度】名册缺少必要角色/立场，已创建专用参与者（身份保持各自固定，不相互扮演）".to_string()];
                    for c in &created {
                        lines.push(format!("- 新增 [@{}]", c));
                    }
                    for r in &rejected {
                        lines.push(format!("- 拒绝补充参与者：{}", r));
                    }
                    let director_sender = self.director_sender();
                    let text = lines.join("\n");
                    let msg = self
                        .persist_message(
                            &director_sender,
                            &[],
                            "director_notice",
                            None,
                            &text,
                            &[],
                            "",
                            "",
                        )
                        .await;
                    self.emit(GroupChatEvent::message(&self.room_id, msg));
                    if !created.is_empty() {
                        // 名册变化：重建运行时并按新名册重排角色/任务，回到循环顶部重新选人。
                        self.rebuild_participants().await;
                        self.replan_after_roster_change(&format!(
                            "【调度】已新增参与者 {}，重新编排角色与任务",
                            created.join("、")
                        ))
                        .await;
                        continue;
                    }
                }
            }
            // 主持人本次选人决策的思考链（供落库展示）。
            let director_reasoning = self.take_director_reasoning();
            // 主席发言者选择决策落库（可追溯）。参与者一律以唯一 id 落库，
            // 显示名由前端渲染时统一映射为 @显示名。
            // 宽容归一化：模型可能回显带 [@] 括号的引用，先还原为裸 id 再落库与匹配。
            let speaker_ref = decision
                .as_ref()
                .map(|d| normalize_participant_ref(&d.speaker));
            // 本轮工作内容（v3.4ao）：主持人"派活"——讨论中派讨论议题、可执行派执行指令；
            // 未提供时回退原"围绕任务发言与推进"语义（build_speaker_task_context）。
            let work_content = decision
                .as_ref()
                .map(|d| d.work_content.trim().to_string())
                .filter(|s| !s.is_empty());
            if let Some(dec) = &decision {
                let director_sender = self.director_sender();
                let speaker_norm = normalize_participant_ref(&dec.speaker);
                let text = if dec.reason.is_empty() {
                    format!("【调度】\n选择 [@{}] 发言", speaker_norm)
                } else {
                    format!("【调度】\n选择 [@{}] 发言（{}）", speaker_norm, dec.reason)
                };
                let text = match &work_content {
                    Some(wc) => format!("{}\n\n本轮工作内容：{}", text, wc),
                    None => text,
                };
                let msg = self
                    .persist_message(
                        &director_sender,
                        &[],
                        "scheduling",
                        None,
                        &text,
                        &[],
                        "",
                        &director_reasoning,
                    )
                    .await;
                // v3.4au：调度消息写入 GroupMemory（与 statement/用户消息一致）——调度承载本轮
                // 工作内容与理由，不进 memory 则参与者上下文缺失"主持人指派"，只能靠 system prompt
                // 的 task_context，导致参与者困惑"是否有主持人指派"（群聊协调上下文断裂）。
                self.memory.apply_message(&msg, &self.all_participant_ids());
                self.emit(GroupChatEvent::message(&self.room_id, msg));
            }
            let speaker = match self.floor.next(speaker_ref.as_deref()) {
                Some(s) => s,
                None => return false,
            };
            self.state.next_round();
            let round = self.state.round;
            self.emit(GroupChatEvent::floor_granted(
                &self.room_id,
                &speaker,
                round,
            ));

            let Some(participant) = self.participants.get(&speaker).cloned() else {
                continue;
            };

            let mut view = self.memory.build_view(&speaker, "", false, &self.user_id);
            // v3.4ao：主持人明确"派活"（讨论议题/执行指令）时优先注入本轮工作内容；
            // 未提供（用户 @ 指定/降级轮询）回退原"围绕任务发言与推进"语义。
            view.task_context = match &work_content {
                Some(wc) => {
                    // v3.4aq：讨论轮（主持人要求发表意见/方案）注入强约束——所有指令经主持人
                    // 统一发出 + 本轮不执行；执行轮（要求执行/修正/完成）正常指派。
                    if Self::is_discussion_work(wc) {
                        // v3.4aw 提示词资产化：讨论轮强约束模板位于
                        // prompts/blocks/room_discussion_turn.prompt.md。
                        super::prompts::render(
                            super::prompts::blocks::ROOM_DISCUSSION_TURN.body,
                            &super::prompts::PromptCtx::new().section("work_content", wc),
                        )
                    } else {
                        format!("本轮主持人指派工作：{}", wc)
                    }
                }
                None => self.build_speaker_task_context(&speaker),
            };
            // v3.4ao：房间统一产物目录注入讨论发言视图。
            view.output_dir = self.output_dir.clone();
            // 注入参与者名册（id、显示名、角色），供 LLM 刻画 id ↔ 语义角色映射。
            view.roster = self.participant_roster();

            let app = self.app.clone();
            let room_id = self.room_id.clone();
            let speaker_id = speaker.clone();
            let delta_fn = move |delta: &str| {
                let _ = app.emit(
                    "groupchat-event",
                    &GroupChatEvent::token_stream(&room_id, &speaker_id, delta),
                );
            };
            // 发言执行期间并发监听命令通道：用户插队消息立即预显示（编排留待发言结束后），
            // 暂停/停止/换主持人等控制命令即时生效，不依赖发言间隙。
            let director_sender = self.director_sender();
            let turn_fut = tokio::time::timeout(
                Duration::from_secs(TURN_TIMEOUT_SECS),
                participant.run_turn(view, Some(Arc::new(delta_fn)), None),
            );
            tokio::pin!(turn_fut);
            let mut staged_directive: Option<(String, MessageRow)> = None;
            let result = loop {
                tokio::select! {
                    r = &mut turn_fut => break match r {
                        Ok(r) => r,
                        Err(_) => {
                            log::warn!("[GroupChat] 参与者 {} 发言超时（{}s），按失败处理", speaker, TURN_TIMEOUT_SECS);
                            TurnResult {
                                content: String::new(),
                                metadata: None,
                                tool_calls: String::new(),
                                reasoning_content: String::new(),
                                error: Some(format!("发言超时（{}s）", TURN_TIMEOUT_SECS)),
                                confirmation: None,
                            }
                        }
                    },
                    cmd = self.cmd_rx.recv() => {
                        match cmd {
                            Some(RoomCommand::Send { sender, content, recipients, reply_to, mention, attachments, security_mode }) => {
                                self.security_mode = security_mode;
                                // 讨论中插队：立即预显示用户消息 + 等待提示，编排留待当前发言结束后。
                                let msg = self.persist_user_directive(&sender, &content, &recipients, reply_to.as_deref(), mention.as_deref(), &attachments).await;
                                if staged_directive.is_none() {
                                    let notice = self
                                        .persist_message(&director_sender, &[], "director_notice", None,
                                            "已收到你的指令，当前发言结束后将立即编排。", &[], "", "")
                                        .await;
                                    self.emit(GroupChatEvent::message(&self.room_id, notice));
                                }
                                staged_directive = Some((content, msg));
                            }
                            Some(RoomCommand::SetDirector { director_id }) => self.set_director(&director_id).await,
                            Some(RoomCommand::SetOutputDir(output_dir)) => {
                                self.room.output_dir = output_dir.clone();
                                self.output_dir = output_dir;
                            }
                            Some(RoomCommand::ParticipantChanged) => self.handle_participant_changed().await,
                            Some(RoomCommand::Pause) => {
                                self.persist_control_notice("已暂停：正在完成当前发言，之后将暂停讨论。点击「继续」可恢复。").await;
                                self.set_status(RoomStatus::Paused).await;
                            }
                            Some(RoomCommand::Resume) => self.set_status(RoomStatus::Running).await,
                            Some(RoomCommand::Abort) => {
                                self.persist_control_notice("已停止：正在完成当前发言（若有），之后将结束讨论。").await;
                                self.set_status(RoomStatus::Aborted).await;
                            }
                            Some(RoomCommand::RespondConfirmation { .. }) => {}
                            // 命令通道关闭（房间被移除）：中断发言并按异常收尾。
                            None => break TurnResult {
                                content: String::new(),
                                metadata: None,
                                tool_calls: String::new(),
                                reasoning_content: String::new(),
                                error: Some("房间已关闭，发言中断".into()),
                                confirmation: None,
                            },
                        }
                        continue;
                    }
                }
            };
            // 发言失败记录（供 Director 感知反复失败、避免反复指派其发言/任务）。
            if let Some(err) = result.error.as_deref() {
                self.record_participant_failure(&speaker, err);
            }
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

            let msg = self
                .persist_message(
                    &speaker,
                    &[],
                    "statement",
                    None,
                    &speech,
                    &[],
                    &result.tool_calls,
                    &result.reasoning_content,
                )
                .await;
            self.emit(GroupChatEvent::message(&self.room_id, msg.clone()));
            self.memory.apply_message(&msg, &self.all_participant_ids());

            // 讨论中插队指令：当前发言已落库，立即执行编排并交还上层重跑（预显示已完成）。
            if let Some((content, msg)) = staged_directive {
                self.after_user_directive(&content, &msg).await;
                return true;
            }

            // 立场快照：LLM 预处理归一化立场（态度 + ≤100 字陈述）；失败/超时或无 Director 时回退机械截断 + 文本分类。
            let (stance, attitude) = match &self.director {
                Some(d) => {
                    match call_director_retry(|| d.extract_stance(&speaker, &speech)).await {
                        Some(se) => (se.stance, se.attitude),
                        None => (
                            speech.chars().take(200).collect::<String>(),
                            resolve_attitude(&speech),
                        ),
                    }
                }
                _ => (
                    speech.chars().take(200).collect::<String>(),
                    resolve_attitude(&speech),
                ),
            };
            self.memory.update_stance(&speaker, &stance, attitude);
            if let Ok(conn) = self.conn() {
                let _ = store::upsert_stance(&conn, &self.room_id, &speaker, &stance, attitude);
            }
            self.emit(GroupChatEvent::stance_updated(
                &self.room_id,
                &speaker,
                &stance,
                attitude,
            ));

            // 语义收敛审查：Director 审阅进度、更新 L1 摘要，并判定 discuss/confirm/done。
            // 主持人 LLM 失败熔断：连续失败（已含同主持人重试耗尽）触发恢复流程——
            // 由现有 API 参与者决策创建新主持人并热替换，成功继续收敛、失败诚实收敛结束。
            // 不再轮流接管主持人身份（破坏参与者角色定义），也不再轮询兜底（无主持人即无调度）。
            let review = match &self.director {
                Some(director) => {
                    let transcript = self.build_review_transcript(30);
                    // 预绑定临时值（避免闭包内自属临时被 future 借用导致 E0515）。
                    let memory_summary = self.memory.summary();
                    let stances = self.memory.stances();
                    call_director_retry(|| {
                        director.director_review(
                            &self.room.topic,
                            &memory_summary,
                            &transcript,
                            &self.room.goal_notes,
                            &task_manifest,
                            &stances,
                            &self.last_user_directive,
                        )
                    })
                    .await
                }
                None => None,
            };

            let Some(review) = review else {
                self.director_fallback.failures += 1;
                log::warn!(
                    "[GroupChat] Director 审阅失败（连续 {} 次）",
                    self.director_fallback.failures
                );

                // 达到熔断阈值 → 触发主持人恢复流程（同步完成：成功继续、失败收敛结束）。
                if self.director_fallback.failures >= DIRECTOR_FAIL_THRESHOLD {
                    let notice = self.recover_director_via_api_agent().await;
                    let director_sender = self.director_sender();
                    let msg = self
                        .persist_message(
                            &director_sender,
                            &[],
                            "director_notice",
                            None,
                            &notice,
                            &[],
                            "",
                            "",
                        )
                        .await;
                    self.emit(GroupChatEvent::message(&self.room_id, msg));
                    if self.director.is_some() {
                        // 新主持人已就位：清零失败计数，继续收敛。
                        log::info!("[GroupChat] 主持人恢复成功，继续讨论");
                        self.director_fallback.reset();
                        continue;
                    }
                    // 主持人无法恢复：没有主持人意味着所有调度逻辑被破坏，诚实收敛结束（不轮询）。
                    let text =
                        "主持人无法恢复，讨论已诚实收敛结束（无主持人即无调度，不采用轮询兜底）";
                    let msg = self
                        .persist_message(
                            &director_sender,
                            &[],
                            "director_notice",
                            None,
                            text,
                            &[],
                            "",
                            "",
                        )
                        .await;
                    self.emit(GroupChatEvent::message(&self.room_id, msg));
                    return false;
                }
                // 未达阈值：继续下一轮（下一轮对同一主持人重试）。
                continue;
            };

            // review 成功：清零失败计数，恢复收敛。
            self.director_fallback.reset();
            log::info!(
                "[GroupChat] Director 审阅: next={} reason={}",
                review.next_action,
                review.reason
            );

            // 更新 L1 摘要（非空才落库，避免覆盖已有有效摘要）。
            if !review.summary.is_empty() {
                self.memory.set_summary(&review.summary);
                let director_sender = self.director_sender();
                let review_reasoning = self.take_director_reasoning();
                let summary_msg = self
                    .persist_message(
                        &director_sender,
                        &[],
                        "summary",
                        None,
                        &review.summary,
                        &[],
                        "",
                        &review_reasoning,
                    )
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
                "replan" => {
                    // 审阅发现需重新编排（新需求/约束/方案或技术选型变更）：先经主持人触发
                    // 对账式重排，再继续调度，确保发言/指令中的新信息被消费进任务清单。
                    let reason = if review.reason.trim().is_empty() {
                        "主持人审阅：最近发言/指令揭示了需重新编排任务清单的新信息".to_string()
                    } else {
                        review.reason.clone()
                    };
                    self.replan(&reason).await;
                    return true;
                }
                "done" => {
                    // 主持人收口裁决（v3.4ao）：把主持裁决为"讨论中已实质完成"的任务置 success，
                    // 执行阶段跳过（主持人最终权威，产物/发言仅作裁决依据）。
                    if !review.completed_tasks.is_empty() {
                        self.apply_completed_tasks(&review.completed_tasks).await;
                    }
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

    /// 应用主持人收口裁决（v3.4aq）：`completed_tasks` 为任务编号（如 1、2 或 T1），
    /// 匹配仍 discussing 的任务置 success，执行阶段跳过（主持人最终权威；产物/发言仅是其裁决依据）。
    async fn apply_completed_tasks(&mut self, task_nos: &[String]) {
        let conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return,
        };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else {
            return;
        };
        for t in tasks {
            let matched = task_nos.iter().any(|no| {
                let no = no.trim().trim_start_matches(['T', 't']).trim();
                no.parse::<i64>().ok() == Some(t.task_no)
            });
            if t.status == "discussing" && matched {
                let mut nt = t.clone();
                nt.status = "success".into();
                nt.result_summary = Some("主持人裁决：讨论阶段已完成".to_string());
                nt.completed_at = Some(crate::utils::now());
                let _ = store::update_task(&conn, &nt);
                self.emit(GroupChatEvent::task_updated(&self.room_id, nt));
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
        reasoning_content: &str,
    ) -> MessageRow {
        self.persist_message_extra(
            sender,
            recipients,
            kind,
            reply_to,
            content,
            attachments,
            tool_calls,
            reasoning_content,
            "{}",
        )
        .await
    }

    /// 落库消息，支持附加结构化数据（extra，如用户确认请求 JSON）与思考链（reasoning_content）。
    async fn persist_message_extra(
        &mut self,
        sender: &str,
        recipients: &[String],
        kind: &str,
        reply_to: Option<&str>,
        content: &str,
        attachments: &[Attachment],
        tool_calls: &str,
        reasoning_content: &str,
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
            reasoning_content: reasoning_content.to_string(),
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

    /// 统一应用角色分配（kickoff / replan / 名册变更共用）：
    /// 落库 + 运行时 set_role + 前端 role_updated；协调类角色仅允许 Director 参与者持有，
    /// 赋给普通参与者时拒绝（保留原角色，避免「双重主持人」）。返回被拒绝的 (id, 角色) 供调用方落可追溯消息。
    async fn apply_role_assignments(
        &mut self,
        roles: &HashMap<String, String>,
    ) -> Vec<(String, String)> {
        let mut rejected: Vec<(String, String)> = Vec::new();
        for (raw_pid, role) in roles {
            // 宽容归一化：LLM 输出/上游解析可能带 [@id]/@id 前缀，直接作为 id 落库
            // 会匹配不到裸 id 行（UPDATE 影响行数为 0，落库静默失败）。兜底归一化保证落库生效。
            let pid = normalize_participant_ref(raw_pid);
            let is_director = self
                .conn()
                .ok()
                .and_then(|c| store::get_participant(&c, &self.room_id, &pid).ok())
                .flatten()
                .map(|p| p.participant_type == "director")
                .unwrap_or(false);
            if is_coordinator_role(role) && !is_director {
                rejected.push((pid.clone(), role.clone()));
                continue;
            }
            if let Ok(conn) = self.conn() {
                let _ = store::update_participant_role(&conn, &self.room_id, &pid, role);
            }
            self.emit(GroupChatEvent::role_updated(&self.room_id, &pid, role));
            if let Some(p) = self.participants.get(&pid) {
                p.set_role(role).await;
            }
        }
        rejected
    }

    /// Director 发言者 id（消息落库时的 sender）。
    fn director_sender(&self) -> String {
        self.director_id
            .clone()
            .unwrap_or_else(|| "director".to_string())
    }

    /// 取走主持人最近一次决策的思考链（供决策消息落库、前端无差异展示）。
    fn take_director_reasoning(&self) -> String {
        self.director
            .as_ref()
            .map(|d| d.take_last_reasoning())
            .unwrap_or_default()
    }

    /// 控制操作（暂停/停止等）的可追溯提示：说明"正在收尾"，避免用户误以为停止未生效。
    async fn persist_control_notice(&mut self, text: &str) {
        let director_sender = self.director_sender();
        let msg = self
            .persist_message(
                &director_sender,
                &[],
                "director_notice",
                None,
                text,
                &[],
                "",
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, msg));
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

    /// 参与者名册（id、显示名、角色），注入 LLM 视图供语义关联。
    fn participant_roster(&self) -> Vec<(String, String, String)> {
        self.conn()
            .ok()
            .and_then(|c| store::list_participants(&c, &self.room_id).ok())
            .map(|rows| {
                rows.into_iter()
                    .map(|p| (p.id, p.display_name, p.system_role))
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn execute_tasks(&mut self) -> bool {
        const RETRY_MAX: usize = 2; // 失败后额外重试次数
        const REASSIGN_MAX: usize = 2; // Director 重派次数上限
        const BASE_BACKOFF_MS: u64 = 500; // 重试指数退避基数

        let tasks = match self.conn() {
            Ok(c) => store::list_tasks(&c, &self.room_id).unwrap_or_default(),
            Err(_) => return false,
        };
        if tasks.is_empty() {
            return false;
        }
        let n = tasks.len();

        let deps: Vec<Vec<usize>> = tasks
            .iter()
            .map(|t| serde_json::from_str::<Vec<usize>>(&t.depends_on).unwrap_or_default())
            .collect();
        let max_parallel = self.room.max_parallel.max(1) as usize;

        // 下标 -> 最终状态（依赖检查用，下标与 tasks 数组一致）
        let mut final_status: Vec<String> = tasks.iter().map(|t| t.status.clone()).collect();
        // 终态任务（成功/跳过/失败/归档）不再重复执行，也不计入后继的前置数。
        let done: Vec<bool> = final_status
            .iter()
            .map(|s| matches!(s.as_str(), "success" | "skipped" | "failed" | "aborted"))
            .collect();

        // 就绪过滤收敛到 task::ready_indices（单一实现）：自身未终态且所有前置均已终态。
        // 阻塞任务（前置终态但非 success）也在就绪列，由下方 dep_blocked 判 skipped 排空。
        let mut ready: Vec<usize> = task::ready_indices(&deps, &done);
        let summary = self.memory.summary().to_string();
        let director_sender = self.director_sender();

        // 前置成果数据源（v3.4an）：
        // 1) db_prior：一次性扫描消息表，按 task_id 建索引（跨轮/重启后反查 task_result 全文与工具链）；
        // 2) prior_cache：本轮波次内已完成任务的实际成果（含最新 result_content/tool_calls，最准确）。
        let db_prior: HashMap<String, (String, String)> = {
            let mut map = HashMap::new();
            if let Ok(conn) = self.pool.get() {
                if let Ok(rows) = store::list_messages(&conn, &self.room_id, None, None) {
                    for m in rows {
                        if m.kind == "task_result" && !m.extra.is_empty() {
                            let content = m
                                .content
                                .splitn(2, '：')
                                .nth(1)
                                .unwrap_or(&m.content)
                                .to_string();
                            map.entry(m.extra.clone())
                                .or_insert_with(|| (content, m.tool_calls.clone()));
                        }
                    }
                }
            }
            map
        };
        let mut prior_cache: HashMap<usize, PriorResult> = HashMap::new();

        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                break;
            }
            self.control.wait_if_paused().await;

            // 下一检查点：检测用户插队，发现新指令则处理并中断执行，交由上层重跑。
            if let Some(RoomCommand::Send {
                sender,
                content,
                recipients,
                reply_to,
                mention,
                attachments,
                security_mode,
            }) = self.drain_pending_commands().await
            {
                self.security_mode = security_mode;
                self.process_user_directive(
                    &sender,
                    &content,
                    &recipients,
                    reply_to.as_deref(),
                    mention.as_deref(),
                    &attachments,
                )
                .await;
                return true;
            }

            if ready.is_empty() {
                break;
            }

            // 取当前波次（受 max_parallel 限制）
            let wave: Vec<usize> = ready.drain(..ready.len().min(max_parallel)).collect();

            let mut handles = Vec::with_capacity(wave.len());
            // 每任务一条"实质产出进展"通道：AgentLoop 每完成一轮有产出的迭代即 try_send，
            // 汇聚端据此重置停滞秒表（区分长任务正常推进与真停滞，有产出不误杀）。
            let mut progress_rxs = Vec::with_capacity(wave.len());
            for &idx in &wave {
                // 依赖感知跳过：任一前置非 success → skipped，不再无意义执行
                let dep_blocked = deps[idx]
                    .iter()
                    .any(|&d| d >= final_status.len() || final_status[d] != "success");

                // 首次执行者：优先 Director 指派的 assignee，否则轮询兜底
                let initial_assignee = tasks[idx]
                    .assignee
                    .clone()
                    .map(|id| normalize_participant_ref(&id))
                    .filter(|id| self.participants.contains_key(id))
                    .or_else(|| {
                        self.executor_order
                            .get(idx % self.executor_order.len().max(1))
                            .cloned()
                    })
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
                let pool = self.pool.clone();
                let mcp = self.mcp.clone();
                let security_mode = self.security_mode;
                let approval_log = self.approval_log.clone();
                let shared_director = self.shared_director.clone();
                let allow_auto_cli = self.room.allow_auto_cli;
                let participant_failures = self.participant_failures.clone();
                let user_language = self.memory.detect_user_language(&self.user_id);
                let input_refs = self.input_refs.clone();
                let output_dir = self.output_dir.clone();

                // 该任务的进展心跳通道（AgentLoop 实质产出 → try_send；汇聚端重置停滞秒表）。
                let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<()>(32);

                // 任务延续性注入：收集该任务的前置成功任务成果（优先本轮内存缓存，其次 DB 反查），
                // 使执行者基于前置成果继续，不重复执行前置子任务的工作。
                let prior_results: Vec<PriorResult> = deps[idx]
                    .iter()
                    .filter(|&&d| d < n && final_status[d] == "success")
                    .filter_map(|&d| {
                        if let Some(p) = prior_cache.get(&d) {
                            return Some(p.clone());
                        }
                        let t = &tasks[d];
                        let (content, tool_calls) =
                            db_prior.get(&t.id).cloned().unwrap_or_default();
                        let artifacts = if tool_calls.trim().is_empty() {
                            Vec::new()
                        } else {
                            extract_artifacts_from_tool_calls(&tool_calls)
                        };
                        // v3.4av：前置成果片段完整透传执行者输出（LLM 输出侧产物），
                        // 不做输入侧硬截断——后继任务判断依赖完整成果；摘要压缩仅由 LLM 输出承担。
                        let snippet = if content.trim().is_empty() {
                            t.result_summary.clone().unwrap_or_default()
                        } else {
                            content
                        };
                        Some(PriorResult {
                            task_no: t.task_no,
                            description: t.description.clone(),
                            result_summary: t.result_summary.clone().unwrap_or_default(),
                            artifacts,
                            snippet,
                        })
                    })
                    .collect();

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
                        pool,
                        room_id,
                        control,
                        summary,
                        director_sender,
                        RETRY_MAX,
                        REASSIGN_MAX,
                        BASE_BACKOFF_MS,
                        mcp,
                        security_mode,
                        approval_log,
                        shared_director,
                        allow_auto_cli,
                        participant_failures,
                        user_language,
                        prior_results,
                        input_refs,
                        output_dir,
                        progress_tx,
                    )
                    .await
                });
                handles.push((idx, handle));
                progress_rxs.push(progress_rx);
            }

            // 汇聚波次结果，回到 Actor 主循环按顺序统一落库。
            let mut pending_confirmation: Option<(String, ConfirmationRequest)> = None;
            // 波次中途到达的用户指令：立即落库显示 + 等待提示，编排延迟到波次结束后执行。
            // 不做 abort/detach —— 波次任务整体完整跑完并落库，避免打断原子工作与重复执行。
            let mut staged_directive: Option<(String, MessageRow)> = None;
            for ((idx, mut handle), mut progress_rx) in
                handles.into_iter().zip(progress_rxs.into_iter())
            {
                // 执行波次汇聚期插队检查：任务执行中并发监听命令通道，用户消息立即落库显示（预输出），
                // 编排留待波次结束后；暂停/停止/换主持人等控制命令同步即时生效，不依赖任务间隙。
                // R1（V3.6 进展感知任务级停滞秒表）：不再按"固定总时长"秒杀——合法长任务（多次
                // 工具迭代、总时长可远超固定上限）每轮"实质产出"都经 progress_rx 重置秒表；
                // 仅当距上次实质产出超过 EXEC_TASK_TIMEOUT_SECS 仍无产出（任一步骤漏网真停滞）时，
                // 才按"任务停滞超时"失败并 abort，防止任务永久 running 拖住同波次与后继依赖层。
                let mut stall_sleep = Box::pin(tokio::time::sleep(Duration::from_secs(
                    EXEC_TASK_TIMEOUT_SECS,
                )));
                let outcome = loop {
                    tokio::select! {
                        biased;
                        r = &mut handle => break match r {
                            Ok(o) => o,
                            Err(e) => {
                                let mut t = tasks[idx].clone();
                                t.status = "failed".into();
                                t.error = Some(format!("任务执行异常: {}", e));
                                t.completed_at = Some(crate::utils::now());
                                TaskOutcome { task: t, messages: Vec::new(), should_abort: false, tool_calls: String::new(), result_content: String::new(), confirmation: None, confirmation_sender: None, roster_changed: false }
                            }
                        },
                        cmd = self.cmd_rx.recv() => {
                            match cmd {
                                Some(RoomCommand::Send { sender, content, recipients, reply_to, mention, attachments, security_mode }) => {
                                    self.security_mode = security_mode;
                                    // 立即预显示用户消息；编排（after_user_directive）延迟到波次结束后，
                                    // 取最近一条（多条插队时后者覆盖前者）。
                                    let msg = self.persist_user_directive(&sender, &content, &recipients, reply_to.as_deref(), mention.as_deref(), &attachments).await;
                                    if staged_directive.is_none() {
                                        // 显式告知用户"指令已收到、正在收尾"，消除"消息被吞没/停止未生效"的错觉。
                                        let notice = self
                                            .persist_message(&director_sender, &[], "director_notice", None,
                                                "已收到你的指令，当前执行中的任务完成后将立即编排。", &[], "", "")
                                            .await;
                                        self.emit(GroupChatEvent::message(&self.room_id, notice));
                                    }
                                    staged_directive = Some((content, msg));
                                }
                                Some(RoomCommand::SetDirector { director_id }) => self.set_director(&director_id).await,
                                Some(RoomCommand::SetOutputDir(output_dir)) => {
                                    self.room.output_dir = output_dir.clone();
                                    self.output_dir = output_dir;
                                }
                                Some(RoomCommand::ParticipantChanged) => self.handle_participant_changed().await,
                                Some(RoomCommand::Pause) => {
                                    self.persist_control_notice("已暂停：正在完成当前执行中的任务，之后将暂停调度新任务。点击「继续」可恢复。").await;
                                    self.set_status(RoomStatus::Paused).await;
                                }
                                Some(RoomCommand::Resume) => self.set_status(RoomStatus::Running).await,
                                Some(RoomCommand::Abort) => {
                                    self.persist_control_notice("已停止：正在完成当前执行中的任务（若有），之后不再调度新任务。").await;
                                    self.set_status(RoomStatus::Aborted).await;
                                }
                                Some(RoomCommand::RespondConfirmation { request_id, responses }) => {
                                    // 波次执行中补交的确认回复（历史确认块，如重启恢复期间）：
                                    // 落库记录（命中仍 pending 的任务时注入回复待重跑），不打断当前波次。
                                    let _ = self.persist_confirmation_response_only(&request_id, &responses).await;
                                }
                                // 命令通道关闭（房间被移除）：中断当前任务并按异常收尾。
                                None => {
                                    let mut t = tasks[idx].clone();
                                    t.status = "pending".into();
                                    t.error = Some("房间已关闭，任务中断".into());
                                    break TaskOutcome { task: t, messages: Vec::new(), should_abort: true, tool_calls: String::new(), result_content: String::new(), confirmation: None, confirmation_sender: None, roster_changed: false };
                                }
                            }
                            continue;
                        }
                        // 实质产出进展心跳：AgentLoop 每完成一轮有产出的迭代即通知 → 重置停滞秒表。
                        // 长任务持续产出即永不超时；仅零产出且任务未返回才触发下方停滞失败。
                        _ = progress_rx.recv() => {
                            stall_sleep = Box::pin(tokio::time::sleep(Duration::from_secs(EXEC_TASK_TIMEOUT_SECS)));
                            continue;
                        }
                        // 任务级停滞超时（R1/V3.6）：距上次实质产出超过 EXEC_TASK_TIMEOUT_SECS 仍无产出
                        // → 按停滞超时失败并 abort 底层 task（进入 failed 终态，同波次与后继层继续收敛）。
                        _ = &mut stall_sleep => {
                            log::warn!("[GroupChat] 任务 T{} 停滞超时（距上次实质产出 >{}s 无进展），已中断", tasks[idx].task_no, EXEC_TASK_TIMEOUT_SECS);
                            handle.abort();
                            let mut t = tasks[idx].clone();
                            t.status = "failed".into();
                            t.error = Some(format!("任务停滞超时（距上次实质产出超 {} 秒无进展），已中断", EXEC_TASK_TIMEOUT_SECS));
                            t.completed_at = Some(crate::utils::now());
                            break TaskOutcome { task: t, messages: Vec::new(), should_abort: false, tool_calls: String::new(), result_content: String::new(), confirmation: None, confirmation_sender: None, roster_changed: false };
                        }
                    }
                };

                // 先落库 Director 在重试/重派过程中的裁决消息
                for pm in outcome.messages {
                    let msg = self
                        .persist_message(&pm.sender, &[], &pm.kind, None, &pm.content, &[], "", "")
                        .await;
                    self.emit(GroupChatEvent::message(&self.room_id, msg));
                }

                if outcome.should_abort {
                    self.control.abort.store(true, Ordering::SeqCst);
                }

                // 失败补人成功落地后重建运行时名册，使新参与者进入后续任务指派的候选。
                if outcome.roster_changed {
                    self.rebuild_participants().await;
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

                // ask_user 的任务暂不落库 task_result，由确认请求消息承载交互；
                // 协作式取消标 pending 的任务同样不落终态汇报（恢复重跑后再报）。
                if !awaiting_user && task.status != "pending" {
                    self.persist_task_message(&task, &outcome.tool_calls, &outcome.result_content)
                        .await;
                }

                // 成功后写入本轮前置成果缓存（供后续波次/后继任务注入，含最新成果与工具链）。
                if task.status == "success" {
                    // v3.4av：成果片段完整透传执行者输出，不做输入侧硬截断。
                    let snippet = if outcome.result_content.trim().is_empty() {
                        task.result_summary.clone().unwrap_or_default()
                    } else {
                        outcome.result_content.clone()
                    };
                    prior_cache.insert(
                        idx,
                        PriorResult {
                            task_no: task.task_no,
                            description: task.description.clone(),
                            result_summary: task.result_summary.clone().unwrap_or_default(),
                            artifacts: extract_artifacts_from_tool_calls(&outcome.tool_calls),
                            snippet,
                        },
                    );
                }

                // v3.4bp 分层执行：波次只执行「当前就绪层」（前置已 success 的任务）。
                // 不再连锁释放后继层——后继层待下一轮 discuss 讨论收敛后再执行，
                // 由主循环交替 discuss/execute_tasks 逐层推进（避免未讨论即被执行）。
                // 依赖解锁靠下一轮调用从 DB 重建就绪层（task::ready_indices）自然完成。
            }

            // 本轮工具授权裁决消息统一落库
            self.flush_approval_logs().await;

            // 有任务需要用户确认：进入确认等待，用户回复后重新编排并重跑。
            // 若同时有波次中途插队指令，确认流程结束后再执行其编排（两者均触发重跑）。
            if let Some((sender, conf)) = pending_confirmation {
                self.handle_user_confirmation(&sender, conf).await;
                if let Some((content, msg)) = staged_directive.take() {
                    self.after_user_directive(&content, &msg).await;
                }
                return true;
            }
            // 波次中途插队指令：波次已完整落库，现在执行编排并交还主循环重跑。
            if let Some((content, msg)) = staged_directive {
                self.after_user_directive(&content, &msg).await;
                return true;
            }
        }

        // 依赖环显式失败（v3.5c）：剩余未终态任务若恰为依赖环成员（互相等待、永不就绪），
        // 分层调度不会经过 dep_blocked 跳过路径覆盖它们；若静默收敛会永久卡 pending 且不可见。
        // 仅在「剩余未终态 == 环成员」时判死锁——环外仍有未终态层时留待后续层推进/重排后重查。
        if !self.control.abort.load(Ordering::SeqCst) {
            self.fail_deadlocked_cycles().await;
        }

        // 执行结束：把执行结果回流到 L1 摘要，供后续结论/讨论使用。
        self.refresh_summary().await;

        false
    }

    /// 依赖环死锁检测：当剩余未终态任务恰好构成依赖环时，显式将其标 failed（不吞错误、防上层空转）。
    async fn fail_deadlocked_cycles(&mut self) {
        let Ok(conn) = self.conn() else { return };
        let Ok(cur) = store::list_tasks(&conn, &self.room_id) else {
            return;
        };
        let cur_deps: Vec<Vec<usize>> = cur
            .iter()
            .map(|t| serde_json::from_str::<Vec<usize>>(&t.depends_on).unwrap_or_default())
            .collect();
        let cur_done: Vec<bool> = cur
            .iter()
            .map(|t| {
                matches!(
                    t.status.as_str(),
                    "success" | "skipped" | "failed" | "aborted"
                )
            })
            .collect();
        let cyclic: HashSet<usize> = task::cycle_indices(&cur_deps, &cur_done)
            .into_iter()
            .collect();
        if cyclic.is_empty() {
            return;
        }
        let non_terminal: HashSet<usize> = (0..cur.len()).filter(|&i| !cur_done[i]).collect();
        if cyclic != non_terminal {
            // 环外仍有未终态任务（后续层尚未解锁）：本轮先不判死锁，交由后续层推进后重查。
            return;
        }
        let mut failed_nos: Vec<i64> = Vec::new();
        for (i, t) in cur.iter().enumerate() {
            if !cyclic.contains(&i) {
                continue;
            }
            let mut t = t.clone();
            t.status = "failed".into();
            t.error =
                Some("任务依赖形成环（互相等待），无法就绪执行，已终止；请调整依赖后重试".into());
            t.completed_at = Some(crate::utils::now());
            self.save_and_emit_task(&t);
            failed_nos.push(t.task_no);
        }
        if failed_nos.is_empty() {
            return;
        }
        let nos: Vec<String> = failed_nos.iter().map(|n| format!("T{}", n)).collect();
        let director_sender = self.director_sender();
        let text = format!(
            "【调度】检测到任务依赖环，已终止相关任务（{}）：互相等待无法就绪，请调整依赖后重试。",
            nos.join("、")
        );
        let msg = self
            .persist_message(
                &director_sender,
                &[],
                "director_notice",
                None,
                &text,
                &[],
                "",
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, msg));
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
                "",
                &extra,
            )
            .await;
        let req_msg_id = req_msg.id.clone();
        self.emit(GroupChatEvent::message(&self.room_id, req_msg));

        // 2. 阻塞等待用户回复。
        let Some(reply) = self
            .await_confirmation_response(&confirmation.request_id, &confirmation.items)
            .await
        else {
            return;
        };

        // 3. 结构化回复直接按确认处理；开放式消息由 Director 判断语义（confirm / directive）。
        match reply {
            ConfirmationReply::Structured {
                content: reply_content,
                responses,
            } => {
                self.apply_confirmation_flow(
                    &confirmation,
                    &req_msg_id,
                    &reply_content,
                    &responses,
                )
                .await;
            }
            ConfirmationReply::Open {
                sender,
                content,
                recipients,
                reply_to,
                mention,
                attachments,
            } => {
                let intent = match self.director.clone() {
                    Some(d) => call_director_retry(|| {
                        d.classify_reply_intent(&confirmation.prompt, &content)
                    })
                    .await
                    .unwrap_or_else(|| "confirm".to_string()),
                    None => "confirm".to_string(),
                };
                if intent == "directive" {
                    // 用户插入的新指令：按正常用户指令处理（落库 + 重新编排）。
                    self.process_user_directive(
                        &sender,
                        &content,
                        &recipients,
                        reply_to.as_deref(),
                        mention.as_deref(),
                        &attachments,
                    )
                    .await;
                } else {
                    // 确认回复：落库 + 注入 pending 任务重跑（开放式回复无结构化 responses）。
                    self.apply_confirmation_flow(&confirmation, &req_msg_id, &content, &[])
                        .await;
                }
            }
        }
    }

    /// 确认回复的统一处理：落库 confirmation_response 消息（extra 携带结构化 responses 供前端恢复控件），
    /// 并注入原 pending 任务重跑。`confirmation_msg_id` 为 confirmation_request 消息 id
    /// （reply_to 契约：指向被回复消息 id，而非 requestId）。
    async fn apply_confirmation_flow(
        &mut self,
        confirmation: &ConfirmationRequest,
        confirmation_msg_id: &str,
        reply_content: &str,
        responses: &[ConfirmationResponse],
    ) {
        let user_sender = self.user_id.clone();
        let extra = serde_json::json!({ "responses": responses }).to_string();
        let resp_msg = self
            .persist_message_extra(
                &user_sender,
                &[],
                "confirmation_response",
                Some(confirmation_msg_id),
                reply_content,
                &[],
                "",
                "",
                &extra,
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, resp_msg.clone()));

        // 有明确关联任务时，把回复写回原 pending 任务并恢复为 discussing 以便重跑；
        // 讨论阶段发起（无关联任务）时：确认回复也必须先经主持人消费——并入最新指令并按
        // 用户输入链路编排（意图分类→合并/重排），避免回复被下一位发言者直接消费。
        if !confirmation.task_id.is_empty() {
            self.apply_confirmation_reply(&confirmation.task_id, reply_content)
                .await;
            let pids = self.all_participant_ids();
            self.memory.apply_message(&resp_msg, &pids);
            // 重置收敛计数，避免确认后过早收敛。
            self.rules.reset();
        } else {
            let content_llm = format!("{}\n\n{}", crate::utils::current_clock_cn(), reply_content);
            self.last_user_directive = content_llm.clone();
            self.after_user_directive(&content_llm, &resp_msg).await;
        }
    }

    /// 非确认等待状态下的兜底落库：持久化 confirmation_response 消息（reply_to 指向
    /// 对应 confirmation_request 消息 id；extra 携带结构化 responses），并尝试恢复确认任务的执行现场。
    /// 用于历史确认块补交 / Actor 重启后未进入确认等待等场景，保证前端刷新后能恢复「已提交」态。
    ///
    /// v3.4m 恢复闭环：若对应确认请求仍关联 pending 任务（如重启后确认等待未恢复、
    /// 讨论中补交回复等），把回复合并进任务描述并恢复为 discussing（apply_confirmation_reply）。
    /// 返回 true 表示已注入回复（任务待重跑），调用方据此立即执行剩余任务，无需等待用户下一条指令。
    async fn persist_confirmation_response_only(
        &mut self,
        request_id: &str,
        responses: &[ConfirmationResponse],
    ) -> bool {
        let content = self.format_confirmation_reply(request_id, responses);
        let user_sender = self.user_id.clone();
        let extra = serde_json::json!({ "responses": responses }).to_string();
        // 先定位确认请求消息 id（reply_to 指向被回复消息 id，而非 requestId）；未命中则 reply_to 置空。
        let req_msg_id = self
            .conn()
            .ok()
            .and_then(|conn| store::list_messages(&conn, &self.room_id, None, None).ok())
            .and_then(|rows| {
                rows.into_iter()
                    .find(|m| m.kind == "confirmation_request" && m.extra.contains(request_id))
                    .map(|m| m.id)
            });
        let resp_msg = self
            .persist_message_extra(
                &user_sender,
                &[],
                "confirmation_response",
                req_msg_id.as_deref(),
                &content,
                &[],
                "",
                "",
                &extra,
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, resp_msg.clone()));

        // 定位确认请求（extra 内嵌 ConfirmationRequest，含 task_id），命中仍 pending 的任务则注入回复。
        let Ok(conn) = self.conn() else {
            return false;
        };
        let Ok(rows) = store::list_messages(&conn, &self.room_id, None, None) else {
            return false;
        };
        let Some(m) = rows
            .iter()
            .find(|m| m.kind == "confirmation_request" && m.extra.contains(request_id))
        else {
            return false;
        };
        let Ok(conf) = serde_json::from_str::<ConfirmationRequest>(&m.extra) else {
            return false;
        };
        if conf.task_id.is_empty() {
            return false;
        }
        self.apply_confirmation_reply(&conf.task_id, &content).await
    }

    /// 把结构化确认回复格式化为可读文本：优先用对应确认请求中的 item label 映射，缺失时回退 item_id。
    fn format_confirmation_reply(
        &self,
        request_id: &str,
        responses: &[ConfirmationResponse],
    ) -> String {
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
    async fn await_confirmation_response(
        &mut self,
        request_id: &str,
        items: &[ConfirmationItem],
    ) -> Option<ConfirmationReply> {
        loop {
            if self.control.abort.load(Ordering::SeqCst) {
                return None;
            }
            match self.cmd_rx.recv().await {
                Some(RoomCommand::RespondConfirmation {
                    request_id: rid,
                    responses,
                }) => {
                    if rid != request_id {
                        // 用户回复的是历史/错配的确认块：仅落库标记已提交，继续等待当前确认。
                        self.persist_confirmation_response_only(&rid, &responses)
                            .await;
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
                    return Some(ConfirmationReply::Structured { content, responses });
                }
                Some(RoomCommand::Send {
                    sender,
                    content,
                    recipients,
                    reply_to,
                    mention,
                    attachments,
                    security_mode,
                }) => {
                    // 确认等待期间的开放文本回复：同步更新本波次模式
                    self.security_mode = security_mode;
                    return Some(ConfirmationReply::Open {
                        sender,
                        content,
                        recipients,
                        reply_to,
                        mention,
                        attachments,
                    });
                }
                Some(RoomCommand::SetDirector { director_id }) => {
                    self.set_director(&director_id).await
                }
                Some(RoomCommand::SetOutputDir(output_dir)) => {
                    self.room.output_dir = output_dir.clone();
                    self.output_dir = output_dir;
                }
                Some(RoomCommand::ParticipantChanged) => self.handle_participant_changed().await,
                Some(RoomCommand::Pause) => self.set_status(RoomStatus::Paused).await,
                Some(RoomCommand::Resume) => self.set_status(RoomStatus::Running).await,
                Some(RoomCommand::Abort) => return None,
                None => return None,
            }
        }
    }

    /// 用户已回复确认：把回复合并进原 pending 任务的描述，并将状态恢复为 discussing 以便重跑。
    /// 返回是否成功注入（仅当任务仍存在且状态为 pending 时返回 true）。
    async fn apply_confirmation_reply(&mut self, task_id: &str, reply: &str) -> bool {
        if let Ok(conn) = self.conn() {
            if let Ok(Some(mut t)) = store::get_task(&conn, &self.room_id, task_id) {
                // 仅当任务仍处于 pending（等待确认）时才注入并重跑；否则保持不变。
                if t.status == "pending" {
                    t.description = format!(
                        "{}\n\n【用户补充/确认信息】\n{}",
                        t.description.trim_end(),
                        reply
                    );
                    t.status = "discussing".into();
                    t.error = None;
                    t.started_at = None;
                    t.completed_at = None;
                    let _ = store::update_task(&conn, &t);
                    self.emit(GroupChatEvent::task_updated(&self.room_id, t));
                    return true;
                }
            }
        }
        false
    }

    /// 确定性兜底：在名册中轮询选择下一个可用执行者（≠ 当前执行人，且必须真实存在于 participants）。
    fn next_executor_fallback(
        executor_order: &[String],
        participants: &HashMap<String, Arc<dyn Participant>>,
        current: &str,
    ) -> Option<String> {
        if executor_order.is_empty() {
            return None;
        }
        let start = executor_order
            .iter()
            .position(|id| id == current)
            .map(|i| i + 1)
            .unwrap_or(0);
        for offset in 0..executor_order.len() {
            let id = &executor_order[(start + offset) % executor_order.len()];
            if id != current && participants.contains_key(id) {
                return Some(id.clone());
            }
        }
        None
    }

    /// 构建「主持人亲自执行 / 侦查」的执行器：读 director 参与者的 agent_config，构建工具化
    /// LLM 客户端，包成 LlmParticipant。
    /// 授权策略（v3.5c+）：不 consult 主持人 LLM（避免自我授权死循环）。是否需要审批由当前
    /// 安全模式驱动：当前模式下需要审批的工具一律拒绝执行，无需审批（Low）的工具正常调用。
    #[allow(clippy::too_many_arguments)]
    fn build_director_executor(
        conn: &rusqlite::Connection,
        pool: &DbPool,
        room_id: &str,
        app: &tauri::AppHandle,
        director_id: &str,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        mcp: Option<Arc<RoomMcpAssets>>,
        security_mode: Option<SecurityMode>,
        output_dir: &str,
    ) -> Option<Arc<dyn Participant>> {
        let rows = store::list_participants(conn, room_id).ok()?;
        let row = rows.iter().find(|r| r.participant_type == "director")?;
        // 执行器工作区根 = 房间产物目录（选定项目/工作空间根）；未选时回退全局工作空间。
        let cwd = if output_dir.trim().is_empty() {
            crate::utils::paths::resolve_workspace_path(None, "", conn)
                .to_string_lossy()
                .to_string()
        } else {
            output_dir.to_string()
        };
        let event_session_id = format!("groupchat:{}:director", room_id);
        // 群聊作用域技能禁用集：主持人亲自执行/侦查同样处于群聊注入面，须隐藏被禁技能目录。
        let disabled_skills =
            crate::commands::app_settings::load_skill_scope_disabled(conn).groupchat;
        let llm = build_llm_client(
            conn,
            pool,
            room_id,
            &row.agent_config,
            app,
            &cwd,
            true,
            Some(event_session_id),
            cancel,
            mcp,
            security_mode,
        )?;
        Some(Arc::new(LlmParticipant {
            id: director_id.to_string(),
            llm: Arc::new(llm),
            role_prompt: Arc::new(RwLock::new("主持人（亲自执行）".to_string())),
            director_id: director_id.to_string(),
            disabled_skills,
        }))
    }

    /// 单个任务的完整执行（含重试/重派/补充参与者接盘/主持人亲自执行），作为关联函数以便并发 spawn。
    /// 不写库、不发事件（补充参与者落库除外），只返回最终任务状态与需要落库的裁决消息。
    #[allow(clippy::too_many_arguments)]
    async fn execute_single_task(
        mut task: TaskRow,
        initial_assignee: Option<String>,
        dep_blocked: bool,
        mut participants: HashMap<String, Arc<dyn Participant>>,
        director: Option<Arc<Director>>,
        topic: String,
        mut executor_order: Vec<String>,
        app: tauri::AppHandle,
        pool: DbPool,
        room_id: String,
        control: Control,
        summary: String,
        director_sender: String,
        retry_max: usize,
        reassign_max: usize,
        base_backoff_ms: u64,
        mcp: Option<Arc<RoomMcpAssets>>,
        security_mode: Option<SecurityMode>,
        approval_log: Arc<Mutex<Vec<ApprovalRecord>>>,
        shared_director: Arc<RwLock<Option<Arc<Director>>>>,
        allow_auto_cli: i64,
        participant_failures: Arc<Mutex<HashMap<String, Vec<(i64, String)>>>>,
        user_language: String,
        prior_results: Vec<PriorResult>,
        input_refs: Vec<ResolvedRef>,
        output_dir: String,
        progress_tx: tokio::sync::mpsc::Sender<()>,
    ) -> TaskOutcome {
        let mut messages: Vec<PendingMessage> = Vec::new();
        let mut should_abort = false;
        // 执行中主持人补充了新参与者（add_participant 落地）→ 调用方需重建名册。
        let mut roster_changed = false;

        if dep_blocked {
            task.status = "skipped".into();
            task.error = Some("前置任务失败或未完成".into());
            task.completed_at = Some(crate::utils::now());
            return TaskOutcome {
                task,
                messages,
                should_abort,
                tool_calls: String::new(),
                result_content: String::new(),
                confirmation: None,
                confirmation_sender: None,
                roster_changed,
            };
        }

        let Some(mut current_assignee) = initial_assignee else {
            task.status = "skipped".into();
            task.error = Some("无可用执行者".into());
            task.completed_at = Some(crate::utils::now());
            return TaskOutcome {
                task,
                messages,
                should_abort,
                tool_calls: String::new(),
                result_content: String::new(),
                confirmation: None,
                confirmation_sender: None,
                roster_changed,
            };
        };

        let mut retries = 0usize;
        let mut reassigns = 0usize;
        // 累计尝试次数（供 Director 裁决判断"已反复失败"）。
        let mut attempts = 0usize;
        // 连续 Director retry 决策次数（达到阈值由代码强制换人，不依赖 LLM 自觉）。
        let mut consecutive_retries: u32 = 0;
        // 已失败执行者（不得再次指派）。
        let mut failed_executors: Vec<String> = Vec::new();
        let mut last_error: Option<String> = None;
        let mut last_tool_calls = String::new();
        // 成功时执行者的完整最终文本（v3.4n：随 task_result 消息落全文）。
        let mut result_content = String::new();
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
                control.clone(),
                last_error.clone(),
                last_work_hint.clone(),
                user_language.clone(),
                prior_results.clone(),
                input_refs.clone(),
                output_dir.clone(),
                progress_tx.clone(),
            )
            .await;

            last_tool_calls = result.tool_calls.clone();
            attempts += 1;

            // 协作式取消（房间停止/暂停）：工具循环已在迭代边界提前结束（当前工具原子完成），
            // 本任务按「中断」处理——标 pending（非终态，恢复后可重跑），记录进度提示，
            // 不再进入重试/重派/裁决，避免停止后仍继续推进。
            if control.abort.load(Ordering::SeqCst) {
                task.status = "pending".into();
                task.error = Some("用户已停止执行，中断前已完成部分工作".into());
                break;
            }

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
            let failed = result.error.is_some()
                || (result.content.is_empty() && result.tool_calls.is_empty());
            if !failed {
                task.status = "success".into();
                if result.content.is_empty() {
                    // 工具已执行但无最终文本：用工具调用链摘要作为成果（消息体同样回退该摘要）。
                    task.result_summary = Some(
                        work_hint_from_tool_calls(&result.tool_calls)
                            .unwrap_or_else(|| "已执行工具调用（无文本结论）".to_string()),
                    );
                    result_content = String::new();
                } else {
                    // v3.4av：result_summary 完整保留执行者输出（LLM 输出侧产物），不做输入侧硬截断；
                    // 前端面板的"短摘要"观感由前端展示层自行折叠，任务清单/成果引用注入完整成果。
                    result_content = result.content.clone();
                    task.result_summary = Some(result.content.clone());
                }
                task.error = None;
                break;
            }

            let err = result.error.unwrap_or_else(|| "空输出".to_string());
            // 记录参与者失败（供主持人感知反复失败、避免跨任务反复指派同一人）。
            record_participant_failure_shared(&participant_failures, &current_assignee, &err);
            last_error = Some(err.clone());
            // 重试时携带上次已完成的工作摘要（仅在确有工具记录时注入）。
            last_work_hint = work_hint_from_tool_calls(&last_tool_calls);
            if !failed_executors.contains(&current_assignee) {
                failed_executors.push(current_assignee.clone());
            }

            // 1) 原地重试（指数退避）
            if retries < retry_max {
                retries += 1;
                let shift = retries.min(6) as u32;
                let backoff = base_backoff_ms * (1u64 << shift);
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                continue;
            }

            // 2) Director 兜底裁决：换人/跳过/终止/询问用户/主持人亲自执行。
            //    裁决缺失（无主持人/超时/解析失败）或决策未能应用（assignee 非法/确认缺失/
            //    self_execute 构建失败）时不静默：落可见消息并轮询换下一个执行者试 1 次。
            if reassigns < reassign_max {
                let mut need_fallback = true;
                if let Some(director) = &director {
                    let roster: Vec<(String, String)> = pool
                        .get()
                        .ok()
                        .and_then(|c| store::list_participants(&c, &room_id).ok())
                        .map(|rows| {
                            rows.iter()
                                .filter(|r| {
                                    r.participant_type == "api" || r.participant_type == "cli"
                                })
                                .map(|r| (r.id.clone(), r.system_role.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let roster_catalog = pool
                        .get()
                        .ok()
                        .map(|c| format_roster_catalog_db(&c, allow_auto_cli != 0))
                        .unwrap_or_default();
                    let max_new = pool
                        .get()
                        .ok()
                        .map(|c| {
                            remaining_participant_slots_db(&c, &room_id)
                                .min(MAX_AUTO_NEW_PARTICIPANTS_REPLAN)
                        })
                        .unwrap_or(0);
                    // 全局失败参与者（≥2 次）并入"不得再指派"名单，防止主持人跨任务反复指派同一人。
                    let mut all_failed = failed_executors.clone();
                    if let Ok(m) = participant_failures.lock() {
                        for (id, fails) in m.iter() {
                            if fails.len() >= 2 && !all_failed.contains(id) {
                                all_failed.push(id.clone());
                            }
                        }
                    }
                    let decision = call_director_retry(|| {
                        director.handle_task_failure(
                            &topic,
                            &task.id,
                            &task.description,
                            &err,
                            &roster,
                            attempts,
                            &all_failed,
                            &roster_catalog,
                            max_new,
                        )
                    })
                    .await;
                    if let Some(d) = decision {
                        need_fallback = false; // 已获得裁决
                        let action_desc = match d.action.as_str() {
                            "retry" => "重试",
                            "reassign" => "重派",
                            "skip" => "跳过",
                            "abort" => "终止",
                            "ask_user" => "向用户确认",
                            "self_execute" => "主持人亲自执行",
                            "add_participant" => "补充参与者接盘",
                            _ => d.action.as_str(),
                        };
                        let assignee_desc = d
                            .assignee
                            .clone()
                            .map(|a| format!("[@{}]", a))
                            .unwrap_or_else(|| "-".into());
                        let reason = if d.reason.is_empty() {
                            "未说明".to_string()
                        } else {
                            d.reason.clone()
                        };
                        let text = format!(
                            "失败裁决：任务「{}」\n失败原因：{}\n处置：{}（目标：{}）\n理由：{}",
                            task.description, err, action_desc, assignee_desc, reason
                        );
                        messages.push(PendingMessage {
                            sender: director_sender.clone(),
                            kind: "failure_decision".into(),
                            content: text,
                        });

                        // 前置处理：移除僵尸参与者（可与其他 action 组合，如 add_participant/reassign 一并输出）。
                        // 只允许移除 api/cli 参与者（禁止主持人/用户），移除后从运行时名册与执行顺序剔除，
                        // 释放席位；若移除的是当前执行者，retry 分支会走兜底换人。
                        if let Some(rid) = d.remove_participant.clone() {
                            let rid = normalize_participant_ref(&rid);
                            let is_api_cli = pool
                                .get()
                                .ok()
                                .and_then(|c| store::get_participant(&c, &room_id, &rid).ok())
                                .flatten()
                                .map(|p| p.participant_type == "api" || p.participant_type == "cli")
                                .unwrap_or(false);
                            if is_api_cli && participants.contains_key(&rid) {
                                if let Some(conn) = pool.get().ok() {
                                    let _ = store::delete_participant(&conn, &room_id, &rid);
                                }
                                participants.remove(&rid);
                                executor_order.retain(|e| e != &rid);
                                if !failed_executors.contains(&rid) {
                                    failed_executors.push(rid.clone());
                                }
                                let _ = app.emit(
                                    "groupchat-event",
                                    &GroupChatEvent::participant_updated(&room_id),
                                );
                                roster_changed = true;
                                messages.push(PendingMessage {
                                    sender: director_sender.clone(),
                                    kind: "failure_decision".into(),
                                    content: format!(
                                        "已移除反复失败的参与者 [@{}]（原因：{}），释放席位",
                                        rid,
                                        if d.reason.is_empty() {
                                            "反复失败"
                                        } else {
                                            d.reason.as_str()
                                        }
                                    ),
                                });
                            }
                        }

                        match d.action.as_str() {
                            "retry" => {
                                // 当前执行者已被移除（僵尸移除场景）→ 不可原地重试，走兜底换人。
                                if !participants.contains_key(&current_assignee) {
                                    need_fallback = true;
                                } else {
                                    // 计入 Director 兜底介入次数，避免反复 retry 死循环。
                                    consecutive_retries += 1;
                                    if consecutive_retries >= RETRY_FORCE_LIMIT {
                                        // 连续 retry 达阈值：由代码强制换人，不依赖 LLM 自觉。
                                        if let Some(next) = Self::next_executor_fallback(
                                            &executor_order,
                                            &participants,
                                            &current_assignee,
                                        ) {
                                            messages.push(PendingMessage {
                                                sender: director_sender.clone(),
                                                kind: "failure_decision".into(),
                                                content: format!(
                                                    "连续重试仍失败，强制更换执行者 → [@{}]",
                                                    next
                                                ),
                                            });
                                            current_assignee = next;
                                            task.assignee = Some(current_assignee.clone());
                                            let _ = app.emit(
                                                "groupchat-event",
                                                &GroupChatEvent::task_updated(
                                                    &room_id,
                                                    task.clone(),
                                                ),
                                            );
                                        }
                                        consecutive_retries = 0;
                                    }
                                    reassigns += 1;
                                }
                                retries = 0;
                                if !need_fallback {
                                    continue;
                                }
                            }
                            "reassign" => {
                                if let Some(new_id) = d
                                    .assignee
                                    .clone()
                                    .map(|id| normalize_participant_ref(&id))
                                    .filter(|id| {
                                        participants.contains_key(id)
                                            && *id != current_assignee
                                            && !failed_executors.contains(id)
                                            && !is_chronic_failer_map(&participant_failures, id)
                                    })
                                {
                                    current_assignee = new_id;
                                    task.assignee = Some(current_assignee.clone());
                                    // 实时推送 assignee 变更，前端 executingIds 据此点亮呼吸圆点。
                                    let _ = app.emit(
                                        "groupchat-event",
                                        &GroupChatEvent::task_updated(&room_id, task.clone()),
                                    );
                                    reassigns += 1;
                                    retries = 0;
                                    consecutive_retries = 0;
                                    continue;
                                }
                                // assignee 非法/已失败/同人 → 轮询换人兜底。
                                need_fallback = true;
                            }
                            "self_execute" => {
                                if let Some(conn) = pool.get().ok() {
                                    if let Some(executor) = Self::build_director_executor(
                                        &conn,
                                        &pool,
                                        &room_id,
                                        &app,
                                        &director_sender,
                                        Some(control.abort.clone()),
                                        mcp.clone(),
                                        security_mode,
                                        &output_dir,
                                    ) {
                                        participants.insert(director_sender.clone(), executor);
                                        current_assignee = director_sender.clone();
                                        task.assignee = Some(current_assignee.clone());
                                        // 实时推送主持人接管，前端主持人头像呼吸点据此点亮。
                                        let _ = app.emit(
                                            "groupchat-event",
                                            &GroupChatEvent::task_updated(&room_id, task.clone()),
                                        );
                                        reassigns += 1;
                                        retries = 0;
                                        consecutive_retries = 0;
                                        continue;
                                    }
                                }
                                // 主持人执行器构建失败 → 轮询换人兜底。
                                need_fallback = true;
                            }
                            "add_participant" => {
                                // 主持人补充参与者接盘：校验落库 → 构建运行时实例（与 build_participants
                                // 一致的审批/续跑接线）→ 重派任务 → 标记名册变更（调用方重建名册，
                                // 使新参与者进入后续任务指派的候选）。
                                if let Some(draft) = d.new_participant.clone() {
                                    if let Some(conn) = pool.get().ok() {
                                        let (created, rejected) = create_auto_participants_db(
                                            &conn,
                                            &room_id,
                                            std::slice::from_ref(&draft),
                                            allow_auto_cli != 0,
                                            MAX_AUTO_NEW_PARTICIPANTS_REPLAN,
                                        );
                                        if let Some(new_id) = created.first().cloned() {
                                            let cwd = crate::utils::paths::resolve_workspace_path(
                                                None, "", &conn,
                                            )
                                            .to_string_lossy()
                                            .to_string();
                                            let mut built_ok = false;
                                            if let Some(row) =
                                                store::get_participant(&conn, &room_id, &new_id)
                                                    .ok()
                                                    .flatten()
                                            {
                                                if row.participant_type == "cli" {
                                                    // cli 参与者：本地 CLI Agent 执行器（与 build_participants 一致，每参与者独立 AgentManager）。
                                                    let agent_type =
                                                        agent_type_from_config(&row.agent_config);
                                                    let runner = Arc::new(PilotDeskCliRunner::new(
                                                        Arc::new(AsyncMutex::new(
                                                            AgentManager::new(),
                                                        )),
                                                        pool.clone(),
                                                        cwd.clone(),
                                                    ));
                                                    let p = CliParticipant {
                                                        id: new_id.clone(),
                                                        runner,
                                                        config: CliConfig {
                                                            command: agent_type,
                                                            args_template: String::new(),
                                                            resume_arg_template: String::new(),
                                                        },
                                                        role_prompt: Arc::new(RwLock::new(
                                                            row.system_role.clone(),
                                                        )),
                                                        director_id: director_sender.clone(),
                                                    };
                                                    participants
                                                        .insert(new_id.clone(), Arc::new(p));
                                                    built_ok = true;
                                                } else {
                                                    // api 参与者：LLM 客户端 + 审批/续跑接线（与 build_participants 一致）。
                                                    // 群聊作用域技能禁用集与 build_participants 同源：动态补员同样隐藏被禁技能目录。
                                                    let disabled_skills = crate::commands::app_settings::load_skill_scope_disabled(&conn).groupchat;
                                                    let event_session_id =
                                                        format!("groupchat:{}:{}", room_id, new_id);
                                                    if let Some(llm) = build_llm_client(
                                                        &conn,
                                                        &pool,
                                                        &room_id,
                                                        &row.agent_config,
                                                        &app,
                                                        &cwd,
                                                        true,
                                                        Some(event_session_id),
                                                        Some(control.abort.clone()),
                                                        mcp.clone(),
                                                        security_mode,
                                                    ) {
                                                        let llm = if shared_director
                                                            .read()
                                                            .unwrap()
                                                            .clone()
                                                            .is_some()
                                                        {
                                                            llm.with_approval_handler(
                                                                make_director_approval(
                                                                    shared_director.clone(),
                                                                    approval_log.clone(),
                                                                    topic.clone(),
                                                                ),
                                                            )
                                                            .with_continue_handler(
                                                                make_director_continue(
                                                                    shared_director.clone(),
                                                                    topic.clone(),
                                                                ),
                                                            )
                                                        } else {
                                                            llm
                                                        };
                                                        let p = LlmParticipant {
                                                            id: new_id.clone(),
                                                            llm: Arc::new(llm),
                                                            role_prompt: Arc::new(RwLock::new(
                                                                row.system_role.clone(),
                                                            )),
                                                            director_id: director_sender.clone(),
                                                            disabled_skills: disabled_skills
                                                                .clone(),
                                                        };
                                                        participants
                                                            .insert(new_id.clone(), Arc::new(p));
                                                        built_ok = true;
                                                    }
                                                }
                                            }
                                            if built_ok {
                                                if !executor_order.contains(&new_id) {
                                                    executor_order.push(new_id.clone());
                                                }
                                                current_assignee = new_id.clone();
                                                task.assignee = Some(current_assignee.clone());
                                                let _ = app.emit(
                                                    "groupchat-event",
                                                    &GroupChatEvent::participant_updated(&room_id),
                                                );
                                                let _ = app.emit(
                                                    "groupchat-event",
                                                    &GroupChatEvent::task_updated(
                                                        &room_id,
                                                        task.clone(),
                                                    ),
                                                );
                                                messages.push(PendingMessage {
                                                    sender: director_sender.clone(),
                                                    kind: "failure_decision".into(),
                                                    content: format!(
                                                        "已补充参与者 [@{}] 接盘该任务（原因：{}）",
                                                        new_id,
                                                        if d.reason.is_empty() {
                                                            "任务需要该能力"
                                                        } else {
                                                            d.reason.as_str()
                                                        }
                                                    ),
                                                });
                                                reassigns += 1;
                                                retries = 0;
                                                consecutive_retries = 0;
                                                roster_changed = true;
                                                continue;
                                            }
                                            // 构建失败：新参与者行已落库但无法运行 → 回滚该行，避免名册僵尸参与者。
                                            let _ =
                                                store::delete_participant(&conn, &room_id, &new_id);
                                        }
                                        for r in &rejected {
                                            messages.push(PendingMessage {
                                                sender: director_sender.clone(),
                                                kind: "failure_decision".into(),
                                                content: format!("补充参与者被拒绝：{}", r),
                                            });
                                        }
                                    }
                                    // 创建/构建失败 → 轮询换人兜底。
                                    need_fallback = true;
                                } else {
                                    need_fallback = true;
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
                                // 未携带确认请求：不静默跳过，轮询换人兜底。
                                need_fallback = true;
                            }
                            _ => {} // skip 等：接受默认最终失败，不兜底。
                        }
                    }
                    // decision None（无主持人裁决/超时/解析失败）→ need_fallback 保持 true。
                }

                if need_fallback {
                    messages.push(PendingMessage {
                        sender: director_sender.clone(),
                        kind: "failure_decision".into(),
                        content: "主持人裁决不可用或未生效（缺失/超时/解析失败），自动切换到下一执行者重试".into(),
                    });
                    if let Some(next) = Self::next_executor_fallback(
                        &executor_order,
                        &participants,
                        &current_assignee,
                    ) {
                        current_assignee = next;
                        task.assignee = Some(current_assignee.clone());
                        let _ = app.emit(
                            "groupchat-event",
                            &GroupChatEvent::task_updated(&room_id, task.clone()),
                        );
                        reassigns += 1;
                        retries = 0;
                        consecutive_retries = 0;
                        continue;
                    }
                }
            }

            // 3) 最终失败
            task.status = "failed".into();
            task.error = Some(err);
            break;
        }

        if confirmation.is_none() && task.status != "pending" {
            task.completed_at = Some(crate::utils::now());
        }
        TaskOutcome {
            task,
            messages,
            should_abort,
            tool_calls: last_tool_calls,
            result_content,
            confirmation,
            confirmation_sender,
            roster_changed,
        }
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
        control: Control,
        error_hint: Option<String>,
        work_hint: Option<String>,
        user_language: String,
        prior_results: Vec<PriorResult>,
        input_refs: Vec<ResolvedRef>,
        output_dir: String,
        progress_tx: tokio::sync::mpsc::Sender<()>,
    ) -> TurnResult {
        let mut prompt = format!("请完成以下任务并给出结果：{}", task_desc);
        // 前置任务成果参考（v3.4an）：任务延续性核心——注入依赖前置任务的实际成果与产物，
        // 使执行者基于前置成果继续，不重复执行前置子任务的工作。
        if !prior_results.is_empty() {
            let mut block = String::from("\n\n【前置任务成果参考】（已完成，勿重复执行）");
            for p in &prior_results {
                block.push_str(&format!("\nT{}. {}", p.task_no, p.description));
                if !p.artifacts.is_empty() {
                    let items: Vec<String> = p
                        .artifacts
                        .iter()
                        .map(|a| match a.kind {
                            ArtifactKind::File => format!("  · [文件] {}", a.value),
                            ArtifactKind::Url => format!("  · [网络] {}", a.value),
                        })
                        .collect();
                    block.push_str(&format!("\n- 产物：\n{}", items.join("\n")));
                }
                let snippet = if p.snippet.trim().is_empty() {
                    &p.result_summary
                } else {
                    &p.snippet
                };
                if !snippet.trim().is_empty() {
                    block.push_str(&format!("\n- 成果片段：{}", snippet));
                }
            }
            block.push_str("\n请基于以上前置成果继续，不要重复已完成的工作。");
            prompt.push_str(&block);
        }
        // 用户输入物引用（v3.4an）：显式/锚点词/LLM 语义解析结果注入执行者，未解析引用显式降级为定位要求。
        if !input_refs.is_empty() {
            let section = format_input_refs_section(&input_refs);
            if !section.is_empty() {
                prompt.push_str(&format!("\n\n{}", section));
            }
        }
        if let Some(work) = work_hint {
            prompt.push_str(&format!(
                "\n\n（注意：本次为续接执行，你此前已完成了以下工作：\n{}\n请基于这些已完成的工作继续推进，不要从头重复执行。）",
                work
            ));
        }
        if let Some(hint) = error_hint {
            prompt.push_str(&format!(
                "\n\n（上次执行失败原因：{}，请修正后重试。）",
                hint
            ));
        }
        let view = TurnView {
            topic: task_desc,
            summary,
            stances: vec![],
            messages: vec![ChatMessage::user(&prompt)],
            system_role: "执行者".into(),
            task_context: String::new(),
            roster: Vec::new(),
            user_language,
            output_dir,
        };
        // delta_fn move 闭包捕获 speaker_id 后不可再借用。
        let delta_fn = move |delta: &str| {
            // 暂停/停止后不再推送流式内容：波次仍自然收尾并落库，仅停止实时展示，
            // 消除"点了停止却还在输出"的错觉。
            if control.paused.load(Ordering::SeqCst) || control.abort.load(Ordering::SeqCst) {
                return;
            }
            let _ = app.emit(
                "groupchat-event",
                &GroupChatEvent::token_stream(&room_id, &speaker_id, delta),
            );
        };
        // 任务级"实质产出"进展心跳：AgentLoop 每完成一轮有产出的迭代即触发，汇聚端据此
        // 重置任务停滞秒表（见 execute_tasks R1）——长任务正常推进不会被固定时长误杀。
        let progress_tick: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = progress_tx.try_send(());
        });
        // V3.6：不再对单轮执行设固定秒级超时（R2 撤销）——单次 run_turn 是 AgentLoop 的完整
        // 多迭代执行（最多 24 次工具迭代），合法长任务总时长可远超固定上限，300s 秒杀会砍断
        // 任务。长任务/停滞统一治理改为：①任务级进展感知停滞秒表（execute_tasks 汇聚端按本轮
        // 心跳重置）；②AgentLoop 内置停滞检测（连续无进展收敛提示 → 主持人裁决续命/收尾）；
        // ③各层独立有界（LLM 300s 流 / 本地命令活性 600s / 同步工具 60s 兜底）。漏网真卡死由
        // 任务级停滞秒表兜底，而正常推进的迭代永不触发。
        participant
            .run_turn(view, Some(Arc::new(delta_fn)), Some(progress_tick))
            .await
    }

    fn save_and_emit_task(&self, task: &TaskRow) {
        if let Ok(conn) = self.conn() {
            let _ = store::update_task(&conn, task);
        }
        self.emit(GroupChatEvent::task_updated(&self.room_id, task.clone()));
    }

    async fn persist_task_message(
        &mut self,
        task: &TaskRow,
        tool_calls: &str,
        result_content: &str,
    ) {
        let sender = task
            .assignee
            .clone()
            .unwrap_or_else(|| self.user_id.clone());
        let task_result = report::to_task_result(task, result_content);
        // extra 写入 task.id（v3.4an：供跨轮/重启后按 task_id 精确反查任务成果全文与工具调用链，
        // 支撑后置任务的前置成果注入）。
        let msg = self
            .persist_message_extra(
                &sender,
                &[],
                "task_result",
                None,
                &task_result,
                &[],
                tool_calls,
                "",
                &task.id,
            )
            .await;
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
            .persist_message(
                &director_sender,
                &[],
                "tool_decision",
                None,
                &text,
                &[],
                "",
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, msg));
    }

    /// 构建 Director 审阅用讨论记录：保留最近 `limit` 条完整消息（结论轮使用更大窗口），
    /// 更早消息不再全量注入（由 L1 摘要承载），避免长上下文注意力稀释与 token 成本线性增长。
    /// 缓存友好（v3.5d）：转录按原始消息逐字回放（不再在渲染期注入实时时钟），头部省略
    /// 说明使用固定文案（不随被省略条数变化），避免同一历史前缀随秒/条数漂移导致整段 miss。
    fn build_review_transcript(&self, limit: usize) -> String {
        let all = self.memory.all_messages();
        let start = all.len().saturating_sub(limit);
        let mut out = String::new();
        if start > 0 {
            out.push_str("（更早的讨论已浓缩进已有进度摘要，以下为最近记录）\n");
        }
        for m in &all[start..] {
            out.push_str(&format!("[@{}]: {}", m.sender, m.content));
            out.push('\n');
        }
        out
    }

    /// 构建任务执行清单文本（编号、状态、描述、负责人、结果/错误），供 Director 选人/总结复用。
    /// 给就绪层里"未指派"的任务补上负责人（按执行者顺序轮转），并落库 + 广播。
    ///
    /// 会话转换或手动新增的任务可能没有负责人（assignee=None）。这种任务在主持人眼里"无人认领"：
    /// 既推不进讨论（没有发言者推进），又容易被重排判为冗余而**作废**、或被评审判为已完成而跳过。
    /// 执行阶段本就有轮询兜底（execute_tasks），这里把兜底提前到讨论/评审之前并持久化，
    /// 让主持人与参与者都能看到负责人，避免"未指派 → 跳过/作废"。
    fn ensure_task_assignees(&mut self) {
        if self.executor_order.is_empty() {
            return;
        }
        let Ok(conn) = self.conn() else { return };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else {
            return;
        };
        let mut changed = 0usize;
        for (i, t) in tasks.iter().enumerate() {
            if t.status != "discussing" {
                continue;
            }
            let has_assignee = t
                .assignee
                .as_deref()
                .map(|a| !a.trim().is_empty())
                .unwrap_or(false);
            if has_assignee {
                continue;
            }
            let mut nt = t.clone();
            nt.assignee = self
                .executor_order
                .get(i % self.executor_order.len())
                .cloned();
            if let Ok(c) = self.conn() {
                let _ = store::update_task(&c, &nt);
            }
            self.emit(GroupChatEvent::task_updated(&self.room_id, nt));
            changed += 1;
        }
        if changed > 0 {
            log::info!(
                "[Room {}] 已为 {} 个未指派任务补齐负责人",
                self.room_id,
                changed
            );
        }
    }

    fn build_task_manifest(&self) -> String {
        let pool = self.pool.clone();
        let room_id = self.room_id.clone();
        self.conn()
            .ok()
            .map(|c| store::list_tasks(&c, &self.room_id).unwrap_or_default())
            .unwrap_or_default()
            .iter()
            .map(|t| {
                let assignee = t
                    .assignee
                    .clone()
                    .map(|a| format!("[@{}]", a))
                    .unwrap_or_else(|| "-".into());
                let detail = if t.status == "success" {
                    t.result_summary.clone().unwrap_or_default()
                } else {
                    t.error.clone().unwrap_or_default()
                };
                // v3.4ao：discussing 任务附"讨论阶段执行证据"（主持人裁决依据，非判定本身）。
                let evidence = if t.status == "discussing" {
                    discussion_artifacts_summary(&pool, &room_id, t)
                } else {
                    String::new()
                };
                format!(
                    "T{}. [{}] {}（负责人：{}）{}{}",
                    t.task_no,
                    t.status,
                    t.description,
                    assignee,
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(" - {}", detail)
                    },
                    evidence
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 判定主持人指派的工作是否为"讨论轮"（要求发表意见/方案，而非直接执行）。
    /// 特征词命中任一即视为讨论轮；执行轮（执行/修正/完成）正常指派执行。
    fn is_discussion_work(wc: &str) -> bool {
        const DISCUSSION_MARKS: &[&str] = &[
            "发表意见",
            "方案",
            "讨论",
            "达成一致",
            "意见",
            "分工",
            "暂不执行",
        ];
        DISCUSSION_MARKS.iter().any(|k| wc.contains(k))
    }

    /// 构建当前发言者被指派的任务上下文（讨论阶段注入 participant 视图，明确任务锚点）。
    fn build_speaker_task_context(&self, speaker: &str) -> String {
        let Ok(conn) = self.conn() else {
            return String::new();
        };
        let Ok(tasks) = store::list_tasks(&conn, &self.room_id) else {
            return String::new();
        };
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
                &self.memory.stances(),
                &self.last_user_directive,
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
            .persist_message(
                &director_sender,
                &[],
                "summary",
                None,
                &review.summary,
                &[],
                "",
                "",
            )
            .await;
        self.emit(GroupChatEvent::message(&self.room_id, summary_msg));
    }

    async fn conclude(&mut self) -> String {
        // 结论轮使用更大的讨论窗口（60 条），确保最终结论基于充分的参与者发言与成果材料
        let transcript = self.build_review_transcript(60);

        // 任务执行清单：供主席在结论中区分已完成/未完成/失败
        let task_manifest = self.build_task_manifest();

        let conclusion = match &self.director {
            Some(d) => {
                // 预绑定临时值（避免闭包内自属临时被 future 借用导致 E0515）。
                let memory_summary = self.memory.summary();
                call_director_retry(|| {
                    d.conclude(
                        &self.room.topic,
                        &memory_summary,
                        &transcript,
                        &task_manifest,
                    )
                })
                .await
            }
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
