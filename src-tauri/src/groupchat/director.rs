//! Director：全知协调者（非自主发言者）。依赖 LlmClient 输出结构化决策。

use std::collections::HashMap;
use std::sync::Arc;

use super::models::{ConfirmationItem, ConfirmationRequest, TaskRow, summarize_confirmation_title};
use super::participant::{ChatMessage, LlmClient};

/// 开场编排产出的任务草稿。
#[derive(Debug, Clone)]
pub struct TaskDraft {
    pub description: String,
    /// 前置任务编号（0 基）。
    pub depends_on: Vec<usize>,
    /// 负责执行该任务的参与者 id（由 Director 指派，可为空）。
    pub assignee: Option<String>,
}

/// 追加轮次对账重排时的单个操作。
#[derive(Debug, Clone)]
pub struct ReplanOperation {
    /// keep（保留/重分配） / remove（删减未执行任务） / add（新增）。
    pub action: String,
    /// keep/remove 时目标已有任务 id；add 时为空。
    pub task_id: Option<String>,
    /// keep 时的重分配目标参与者 id；空表示保持原负责人。
    pub reassign: Option<String>,
    /// remove 时的移除理由。
    pub reason: String,
    /// add 时的任务描述。
    pub description: String,
    /// add 时的前置任务 id 列表（仅引用「已有任务 id」）。
    pub depends_on: Vec<String>,
    /// add 时的负责人。
    pub assignee: Option<String>,
}

/// 追加轮次对账重排计划。
#[derive(Debug, Clone)]
pub struct ReplanPlan {
    pub operations: Vec<ReplanOperation>,
}

/// 名册变更（加入/移除参与者）后的重排结果：角色分配 + 任务重派/新增。
#[derive(Debug, Clone)]
pub struct RosterReplan {
    pub roles: HashMap<String, String>,
    pub operations: Vec<ReplanOperation>,
}

/// 任务执行失败后的处置决策。
#[derive(Debug, Clone)]
pub struct FailureDecision {
    /// retry / reassign / skip / abort / ask_user
    pub action: String,
    /// reassign 时的目标参与者 id
    pub assignee: Option<String>,
    /// 决策理由
    pub reason: String,
    /// ask_user 时向用户发起的确认请求
    pub confirmation: Option<ConfirmationRequest>,
}

/// Director 选中的下一位发言者及其选择理由。
#[derive(Debug, Clone)]
pub struct SpeakerDecision {
    pub speaker: String,
    pub reason: String,
}

/// Director 讨论收敛审查结果。
#[derive(Debug, Clone)]
pub struct ReviewOutcome {
    /// 更新后的 L1 摘要。
    pub summary: String,
    /// discuss=继续讨论 / confirm=需向用户确认 / done=已收敛。
    pub next_action: String,
    /// 判定理由。
    pub reason: String,
    /// next_action=confirm 时的确认请求。
    pub confirmation: Option<ConfirmationRequest>,
}

/// 工具授权裁决结果。
#[derive(Debug, Clone)]
pub struct ToolDecision {
    pub allow: bool,
    pub reason: String,
}

/// 用户指令对初始目标的意图分类。
#[derive(Debug, Clone)]
pub struct GoalIntent {
    /// refine（补充/确认，保持目标）/ change_goal（明确变更目标）/ new_goal（全新无关目标）
    pub intent: String,
    /// change_goal / new_goal 时的新目标文本（refine 时为空）
    pub new_goal: String,
    /// refine 时提炼出的补充/细化约束（可空；change_goal/new_goal 时忽略）
    pub note: String,
}

/// Director 结构化决策输出。
pub struct Director {
    llm: Arc<dyn LlmClient>,
}

impl Director {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }

    /// 开场编排：产出子任务/议程 + 为参与者分配角色。
    /// `topic` 为初始目标（不可变锚点），`latest` 为本轮新增指令（首轮两者相同，
    /// 追加/确认轮为用户最新输入）。返回 `(任务草稿, participant_id -> 角色)`；失败返回 `None`。
    pub async fn kickoff(
        &self,
        topic: &str,
        latest: &str,
        notes: &str,
        participants: &[(String, String)],
    ) -> Option<(Vec<TaskDraft>, HashMap<String, String>)> {
        let roster = participants
            .iter()
            .map(|(id, name)| format!("- {} ({})", id, name))
            .collect::<Vec<_>>()
            .join("\n");

        // 初始目标与新增指令分离：防止追加/确认轮次脱离原始目标。
        let latest_section = if topic == latest {
            "（本轮无新增指令，直接围绕初始目标编排）".to_string()
        } else {
            format!("【本轮新增指令（用户刚刚补充/确认，需纳入编排）】\n{}", latest)
        };
        let notes_section = if notes.trim().is_empty() {
            "（暂无补充约束）".to_string()
        } else {
            format!("【已累积的补充/细化约束（必须一并满足）】\n{}", notes)
        };

        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。请针对以下目标进行开场编排。\n\
             【初始目标（始终不变，必须优先服务）】\n{topic}\n\n\
             {notes_section}\n\n\
             {latest_section}\n\n\
             参与者列表：\n{roster}\n\n\
             请输出严格 JSON（不要 markdown 代码块），格式：\n\
             {{\"tasks\":[{{\"description\":\"子任务描述\",\"depends_on\":[前置任务下标数组],\"assignee\":\"负责执行的参与者id\"}}],\
             \"roles\":{{\"参与者id\":\"角色名\"}}}}\n\
             要求：\n\
             1. 任务个数由你根据目标复杂度自行决定，roles 覆盖每个参与者；\n\
             2. 每个任务都必须用 assignee 明确指派给一个参与者（使用参与者列表中的 id）；\n\
             3. 所有任务都必须服务于【初始目标】，并满足【已累积的补充/细化约束】，同时响应【本轮新增指令】；若新增指令与初始目标冲突，以初始目标为准；\n\
             4. 任务必须是可被参与者执行的实质性子任务，禁止包含“总结观点/归纳共识分歧/形成最终结论/收口”类任务——最终结论由 Director 在讨论结束后统一负责；\n\
             5. depends_on 表示该任务必须等待哪些前置任务完成后才能执行：若任务 B 依赖任务 A（A 在 tasks 数组中的下标为 i），则 B.depends_on 必须包含整数 i（下标从 0 开始）；无依赖则用空数组 []；\n\
             6. 每个任务的 description 必须写明可验证的产出物/目标，并在涉及文件时指明具体文件、目录或搜索模式；禁止“了解/熟悉/分析项目”“查看工作区”等空洞、泛化的任务描述，禁止让参与者无差别扫描整个工作区。"
        );

        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        let json = strip_fences(&raw);
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;

        let tasks = v["tasks"]
            .as_array()?
            .iter()
            .map(|t| TaskDraft {
                description: t["description"].as_str().unwrap_or("未命名任务").to_string(),
                depends_on: t["depends_on"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_u64().map(|n| n as usize))
                            .collect()
                    })
                    .unwrap_or_default(),
                assignee: t["assignee"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            })
            .collect();

        let mut roles = HashMap::new();
        if let Some(obj) = v["roles"].as_object() {
            for (k, val) in obj {
                roles.insert(k.clone(), val.as_str().unwrap_or("参与者").to_string());
            }
        }

        Some((tasks, roles))
    }

    /// 追加轮次对账重排：把当前任务清单（含状态）交给 Director，输出 keep/remove/add 操作。
    /// 运行时据此删减未执行任务、重分配负责人、新增必要子任务，从而替代盲目累加。
    /// 失败返回 `None`（由调用方按保守策略处理：不新增、不删减）。
    pub async fn replan(
        &self,
        topic: &str,
        latest: &str,
        notes: &str,
        existing: &[TaskRow],
        participants: &[(String, String)],
    ) -> Option<ReplanPlan> {
        let roster = participants
            .iter()
            .map(|(id, name)| format!("- {} ({})", id, name))
            .collect::<Vec<_>>()
            .join("\n");

        let existing_lines: Vec<String> = existing
            .iter()
            .map(|t| {
                let assignee = t.assignee.clone().unwrap_or_else(|| "未指派".into());
                format!(
                    "- id={} | 状态={} | 描述={} | 负责人={}",
                    t.id, t.status, t.description, assignee
                )
            })
            .collect();
        let existing_text = if existing_lines.is_empty() {
            "（暂无已有任务）".to_string()
        } else {
            existing_lines.join("\n")
        };

        let notes_section = if notes.trim().is_empty() {
            "（暂无补充约束）".to_string()
        } else {
            format!("【已累积的补充/细化约束（必须一并满足）】\n{}", notes)
        };

        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。用户对初始目标补充了新的指令，请对「当前任务清单」进行对账式重排。\n\
             【初始目标（始终不变，必须优先服务）】\n{topic}\n\n\
             {notes_section}\n\n\
             【本轮新增指令（用户最新输入）】\n{latest}\n\n\
             参与者列表：\n{roster}\n\n\
             当前任务清单（id 必须原样保留；仅未执行任务可删减或重分配）：\n{existing_text}\n\n\
             请输出严格 JSON（不要 markdown 代码块），格式：\n\
             {{\"operations\":[\
             {{\"action\":\"keep\",\"task_id\":\"已存在任务id\",\"reassign\":\"新负责人id或省略\"}},\
             {{\"action\":\"remove\",\"task_id\":\"已存在任务id\",\"reason\":\"移除理由\"}},\
             {{\"action\":\"add\",\"description\":\"新任务描述\",\"depends_on\":[\"前置任务id\"],\"assignee\":\"负责人id\"}}\
             ]}}\n\
             要求：\n\
             1. 状态为 success/failed/skipped 的任务一律 keep 且 reassign 留空，不得改动；\n\
             2. 状态为 discussing（尚未执行）的任务：若本轮指令使其不再必要，用 remove 并说明理由；若需更换负责人，用 keep 且填 reassign；\n\
             3. 状态为 pending（等待用户确认）的任务不要 remove，用 keep 保持；\n\
             4. 本轮指令若需要新的子任务，用 add 新增；depends_on 只能引用「已有任务 id」，本批新增任务之间不建立依赖；\n\
             5. 若本轮指令并未要求新增任务、且已有任务已覆盖目标，则不要输出 add 操作，operations 只保留必要的 keep；\n\
             6. 所有任务都必须服务于【初始目标】；\n\
             7. add 的任务 description 必须写明可验证的产出物/目标，并在涉及文件时指明具体文件、目录或搜索模式；禁止“了解/熟悉/分析项目”“查看工作区”等空洞任务描述，禁止让参与者无差别扫描整个工作区。"
        );

        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;

        let operations = v["operations"]
            .as_array()?
            .iter()
            .map(|op| {
                let action = op["action"].as_str().unwrap_or("keep").to_string();
                let task_id = op["task_id"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let reassign = op["reassign"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let reason = op["reason"].as_str().unwrap_or("").trim().to_string();
                let description = op["description"].as_str().unwrap_or("").trim().to_string();
                let depends_on = op["depends_on"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
                            .collect()
                    })
                    .unwrap_or_default();
                let assignee = op["assignee"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                ReplanOperation {
                    action,
                    task_id,
                    reassign,
                    reason,
                    description,
                    depends_on,
                    assignee,
                }
            })
            .collect();

        Some(ReplanPlan { operations })
    }

    /// 名册变更后的重排：结合当前角色与任务完成情况，输出角色分配与任务重派。
    /// 关键约束：保持已有参与者的根本角色定位（仅可调整职责/任务分工）、
    /// 已完成任务（success/failed/skipped）不得重派。失败返回 `None`（由调用方保守处理）。
    pub async fn replan_for_roster_change(
        &self,
        topic: &str,
        notes: &str,
        existing: &[TaskRow],
        participants: &[(String, String, String)],
    ) -> Option<RosterReplan> {
        let roster_lines: Vec<String> = participants
            .iter()
            .map(|(id, name, role)| {
                let role = if role.trim().is_empty() { "未分配".to_string() } else { role.clone() };
                format!("- {}（{}）角色：{}", id, name, role)
            })
            .collect();
        let roster = if roster_lines.is_empty() {
            "（暂无参与者）".to_string()
        } else {
            roster_lines.join("\n")
        };

        let existing_lines: Vec<String> = existing
            .iter()
            .map(|t| {
                let assignee = t.assignee.clone().unwrap_or_else(|| "未指派".into());
                let detail = if t.status == "success" {
                    t.result_summary.clone().unwrap_or_default()
                } else {
                    t.error.clone().unwrap_or_default()
                };
                format!(
                    "- id={} | 状态={} | 描述={} | 负责人={}{}",
                    t.id,
                    t.status,
                    t.description,
                    assignee,
                    if detail.is_empty() { String::new() } else { format!(" | 说明={}", detail) }
                )
            })
            .collect();
        let existing_text = if existing_lines.is_empty() {
            "（暂无任务）".to_string()
        } else {
            existing_lines.join("\n")
        };

        let notes_section = if notes.trim().is_empty() {
            "（暂无补充约束）".to_string()
        } else {
            format!("【已累积的补充/细化约束（必须一并满足）】\n{}", notes)
        };

        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。有参与者加入/离开了房间，请对「角色分配」和「任务清单」进行对账式重排。\n\
             【初始目标（始终不变，必须优先服务）】\n{topic}\n\n\
             {notes_section}\n\n\
             【当前参与者名册（id、显示名、当前角色）】\n{roster}\n\n\
             【当前任务清单（id、状态、描述、负责人、结果/错误）】\n{existing_text}\n\n\
             请输出严格 JSON（不要 markdown 代码块），格式：\n\
             {{\"roles\":{{\"参与者id\":\"角色名\"}},\"operations\":[\
             {{\"action\":\"keep\",\"task_id\":\"已存在任务id\",\"reassign\":\"新负责人id或省略\"}},\
             {{\"action\":\"add\",\"description\":\"新任务描述\",\"depends_on\":[\"前置任务id\"],\"assignee\":\"负责人id\"}}\
             ]}}\n\
             要求：\n\
             1. 角色连续性：已有参与者保持其根本角色定位，仅可调整职责边界/任务分工，禁止根本性变更角色（如 架构师→视觉工程师）；新加入的参与者分配一个合适的角色；已离开的参与者不再出现在 roles 中；\n\
             2. 已完成任务不可动：状态为 success/failed/skipped 的任务一律 keep 且 reassign 留空，不得重派或重新分配；\n\
             3. 仅 discussing/pending/running（未完成/未执行）的任务可重派、拆分或新增；优先把其中适合的部分指派给新参与者，不重复已完成工作；\n\
             4. 已离开参与者名下未完成的任务需重派给仍在名册中的参与者；\n\
             5. 所有重排必须服务【初始目标】并满足补充约束；\n\
             6. roles 覆盖名册中每个仍在册的参与者。"
        );

        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;

        let mut roles = HashMap::new();
        if let Some(obj) = v["roles"].as_object() {
            for (k, val) in obj {
                roles.insert(k.clone(), val.as_str().unwrap_or("参与者").to_string());
            }
        }

        let operations = v["operations"]
            .as_array()?
            .iter()
            .map(|op| {
                let action = op["action"].as_str().unwrap_or("keep").to_string();
                let task_id = op["task_id"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let reassign = op["reassign"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let reason = op["reason"].as_str().unwrap_or("").trim().to_string();
                let description = op["description"].as_str().unwrap_or("").trim().to_string();
                let depends_on = op["depends_on"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
                            .collect()
                    })
                    .unwrap_or_default();
                let assignee = op["assignee"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                ReplanOperation {
                    action,
                    task_id,
                    reassign,
                    reason,
                    description,
                    depends_on,
                    assignee,
                }
            })
            .collect();

        Some(RosterReplan { roles, operations })
    }

    /// 判断用户最新输入对初始目标的意图类型。
    /// 默认保守：仅在语义明确时才判 `change_goal` / `new_goal`，其余一律 `refine`。
    /// 失败返回 `None`（由调用方按 `refine` 处理）。
    pub async fn classify_goal_intent(&self, topic: &str, latest: &str, notes: &str) -> Option<GoalIntent> {
        let notes_display = if notes.trim().is_empty() {
            "（暂无）".to_string()
        } else {
            notes.to_string()
        };
        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。请判断用户最新输入对「初始目标」的意图类型。\n\
             【初始目标】\n{topic}\n\n\
             【已累积的补充/细化约束】\n{notes_display}\n\n\
             【用户最新输入】\n{latest}\n\n\
             请输出严格 JSON（不要 markdown 代码块），格式：\
             {{\"intent\":\"refine|change_goal|new_goal\",\"new_goal\":\"新目标文本(仅 change_goal/new_goal 时填写)\",\"note\":\"补充/细化约束(仅 refine 且确有新增约束时填写)\"}}\n\
             判定规则：\n\
             1. 若用户是对当前目标的补充、细化、确认、追问或对讨论内容的反馈 → intent=refine；若其中蕴含新的约束或需求，请提炼为一句简洁的 note（否则 note 留空）；new_goal 留空；\n\
             2. 若用户明确表示要修改/更换当前目标（如“改成…”“换成…”“重新设定目标为…”“目标改为…”）→ intent=change_goal，new_goal 填新目标，note 留空；\n\
             3. 若用户提出了与当前目标完全无关的全新目标/话题（如“算了，我们来做…”“换一个完全不同的任务…”“新任务：…”）→ intent=new_goal，new_goal 填新目标，note 留空。\n\
             默认优先判 refine，只有在语义明确时才判 change_goal/new_goal；note 只在 refine 且确有新约束时填写，避免重复已有约束。"
        );
        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let intent = match v["intent"].as_str().unwrap_or("refine") {
            "change_goal" => "change_goal".to_string(),
            "new_goal" => "new_goal".to_string(),
            _ => "refine".to_string(),
        };
        let new_goal = v["new_goal"].as_str().unwrap_or("").trim().to_string();
        let note = v["note"].as_str().unwrap_or("").trim().to_string();
        Some(GoalIntent { intent, new_goal, note })
    }

    /// 判断用户对确认请求的回复语义：`confirm`（回复/补充确认项）或 `directive`（新指令/改方向）。
    /// 完全由 LLM 依据内容语义判断，不约定硬编码规则；失败默认按 `confirm` 处理。
    pub async fn classify_reply_intent(&self, confirmation_prompt: &str, reply: &str) -> Option<String> {
        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。系统正在等待用户对以下确认请求的回复。\n\
             【确认请求】\n{confirmation_prompt}\n\n\
             【用户回复】\n{reply}\n\n\
             请判断这条用户回复的意图：\n\
             1. 若用户是在回答、确认、补充上述确认请求的内容，或提供确认请求所需的决策信息 → intent=confirm；\n\
             2. 若用户提出了新的要求、新的指令、更换了方向，或与当前确认请求无直接关联 → intent=directive。\n\
             请输出严格 JSON（不要 markdown 代码块），格式：{{\"intent\":\"confirm|directive\"}}。默认优先判 confirm。"
        );
        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        Some(match v["intent"].as_str().unwrap_or("confirm") {
            "directive" => "directive".to_string(),
            _ => "confirm".to_string(),
        })
    }

    /// 选择下一发言者（依据角色匹配/立场分歧/话题相关性/@提及）。
    /// 失败返回 `None`（由 RuleEngine 轮询兜底）。
    pub async fn next_speaker(
        &self,
        topic: &str,
        summary: &str,
        participants: &[String],
        task_manifest: &str,
        mention: Option<&str>,
    ) -> Option<SpeakerDecision> {
        let roster = participants.join(", ");
        let mention_hint = mention.map(|m| format!("用户 @ 了：{m}")).unwrap_or_default();
        let task_section = if task_manifest.trim().is_empty() {
            "（暂无任务清单）".to_string()
        } else {
            task_manifest.to_string()
        };

        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。请选择下一位发言者。\n\
             当前议题（初始目标，不可偏离）：{topic}\n全局摘要：{summary}\n\
             任务清单（编号、状态、描述、负责人）：\n{task_section}\n\
             可发言参与者：{roster}\n{mention_hint}\n\n\
             请输出严格 JSON（不要 markdown 代码块），格式：{{\"next_speaker\":\"参与者id\",\"reason\":\"选择理由\"}}。\
             选择依据必须服务于上述初始目标：优先选择仍有未完成任务（状态为 discussing/pending/running）的负责人发言推进，\
             避免重复指派已完成或无需推进的参与者；若参与者为空，next_speaker 设为空字符串。"
        );

        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let speaker = v["next_speaker"].as_str()?.trim().to_string();
        if speaker.is_empty() {
            None
        } else {
            let reason = v["reason"].as_str().unwrap_or("").trim().to_string();
            Some(SpeakerDecision { speaker, reason })
        }
    }

    /// 讨论收敛审查：审阅进度，更新 L1 摘要并判定下一步（discuss/confirm/done）。
    /// 语义收敛替代字符串比较：把「关键信息获取方式」（查文件/自行分析/向用户确认）
    /// 作为第一层判据，并对照任务清单判断子任务是否已明确。
    pub async fn director_review(
        &self,
        topic: &str,
        old_summary: &str,
        transcript: &str,
        notes: &str,
        task_manifest: &str,
    ) -> Option<ReviewOutcome> {
        let notes_display = if notes.trim().is_empty() {
            "（暂无）".to_string()
        } else {
            notes.to_string()
        };
        let task_display = if task_manifest.trim().is_empty() {
            "（暂无任务清单）".to_string()
        } else {
            task_manifest.to_string()
        };
        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。请审阅当前讨论进度，更新摘要并判定下一步。\n\
             【初始目标（不可变锚点，必须始终保留其语义）】\n{topic}\n\n\
             【已累积的补充/细化约束】\n{notes_display}\n\n\
             【任务清单（编号、状态、描述、负责人）】\n{task_display}\n\n\
             【已有进度摘要】\n{old_summary}\n\n\
             【最近讨论记录】\n{transcript}\n\n\
             请输出严格 JSON（不要 markdown 代码块），格式：\
             {{\"summary\":\"更新后的中文摘要(≤300字，锚定初始目标)\",\"next_action\":\"discuss|confirm|done\",\"reason\":\"判定理由\",\"confirmation\":{{...}}}}\n\n\
             next_action 判定规则（按顺序判断）：\n\
             1. 若继续推进任务仍需关键信息，且该信息只能由用户（人类）提供或拍板（例如需求歧义、不可逆决策、方案取舍、用户偏好），则 next_action=confirm，并用 confirmation 字段发起确认请求；\n\
             2. 若仍缺的信息可由参与者通过查文件/工具/自行分析获取，或仍有未完成任务需要继续讨论细化，则 next_action=discuss；\n\
             3. 仅当所有子任务方案均已明确、无需再向用户确认、也无需继续讨论时，才 next_action=done。\n\
             注意：不要因参与者表述方式变化或换措辞就重复讨论；信息已充分、任务已明确就直接 done，不要机械重复指派。\n\n\
             confirmation 字段格式（仅 next_action=confirm 时填写，其余省略）：\n\
             - 开放式：{{\"title\":\"不超过22字的确认事项摘要\",\"reply_mode\":\"open\",\"prompt\":\"请说明…\",\"items\":[]}}\n\
             - 结构化：{{\"title\":\"不超过22字的确认事项摘要\",\"reply_mode\":\"structured\",\"prompt\":\"请确认…\",\"items\":[{{\"id\":\"q1\",\"label\":\"问题\",\"input_type\":\"confirm|select|text\",\"options\":[],\"required\":true,\"placeholder\":\"\"}}]}}"
        );
        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let summary = v["summary"].as_str().unwrap_or("").trim().to_string();
        let next_action = match v["next_action"].as_str().unwrap_or("discuss").trim() {
            "confirm" => "confirm".to_string(),
            "done" => "done".to_string(),
            _ => "discuss".to_string(),
        };
        let reason = v["reason"].as_str().unwrap_or("").trim().to_string();
        let confirmation = if next_action == "confirm" {
            Some(build_confirmation_from_json(
                &v["confirmation"],
                "",
                "请确认以下事项以便继续。",
            ))
        } else {
            None
        };
        Some(ReviewOutcome { summary, next_action, reason, confirmation })
    }

    /// 收敛结论。
    pub async fn conclude(&self, topic: &str, summary: &str, transcript: &str, task_manifest: &str) -> Option<String> {
        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。讨论已收敛，请给出最终结论。\n\
             【初始目标（结论必须对照此目标，不可偏离）】\n{topic}\n\
             全局摘要：{summary}\n\n任务执行清单：\n{task_manifest}\n\n完整讨论记录：\n{transcript}\n\n\
             请严格按以下四部分输出结论正文，每部分以 `## 标题` 开头（标题必须一字不差），\
             正文写在标题下方。不要使用 `---` 分隔线、不要代码块、不要任何前缀或额外说明。\n\n\
             四部分要求各不相同，各写各的，互不套用：\n\n\
             ## 已完成\n【要求】简洁罗列已成功完成的任务与成果要点，每条一行直接陈述即可；不要综合叙述、不要展开论证。\n\n\
             ## 未完成\n【要求】简洁罗列尚未完成的任务及原因，每条一行直接陈述即可。\n\n\
             ## 失败\n【要求】简洁罗列失败的任务及失败原因，每条一行直接陈述即可。\n\n\
             ## 最终结论\n【要求】这一部分才需要完整综合，前面的已完成/未完成/失败部分保持简洁即可。\
             请综合以上讨论记录、执行成果与任务清单，整理成意见收敛的完整总结：\
             覆盖关键决策、方案要点与执行成果，不遗漏关键信息，避免摘要式总结，但不可冗余；\
             应呈现讨论收敛后的共识与最终确定的方案，而不是复述各方发言过程或简单罗列要点。\
             此完整总结要求仅适用于「最终结论」部分，不要影响前面三个部分的简洁风格。"
        );
        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        Some(raw.trim().to_string())
    }

    /// 任务执行失败后的兜底裁决：返回结构化处置决策（重试/换人/跳过/终止/向用户确认）。
    /// `topic` 为初始目标锚点，确保失败处置与确认请求不偏离原始目标。
    /// 失败返回 `None`（由调用方走默认跳过策略）。
    pub async fn handle_task_failure(
        &self,
        topic: &str,
        task_id: &str,
        task_desc: &str,
        error: &str,
        participants: &[String],
    ) -> Option<FailureDecision> {
        let roster = participants.join(", ");
        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。执行者执行以下子任务失败，请给出处置决策。\n\
             【初始目标（所有处置必须服务于此目标，不可偏离）】\n{topic}\n\
             子任务：{task_desc}\n失败原因：{error}\n可重新指派的参与者：{roster}\n\n\
             请输出严格 JSON（不要 markdown 代码块），格式：\
             {{\"action\":\"retry|reassign|skip|abort|ask_user\",\"assignee\":\"参与者id(仅 reassign 时必填)\",\"reason\":\"决策理由\",\"confirmation\":{{...}}}}\n\
             要求：\n\
             1. 若失败可能因瞬时故障且该参与者仍合适，action=retry；\n\
             2. 若应换人继续，action=reassign 并指定 assignee；\n\
             3. 若该任务已无必要或无法完成，action=skip；\n\
             4. 若失败严重到应终止整个执行，action=abort；\n\
             5. 若该任务的关键决策需要用户（人类）拍板才能继续（例如涉及不可逆操作、关键信息缺失需用户补充、方案取舍），action=ask_user，并通过 confirmation 字段发起确认请求。\n\n\
             confirmation 字段格式：\n\
             - 开放式回复（reply_mode=open，items 为空数组）：\
             {{\"title\":\"不超过22字的确认事项摘要\",\"reply_mode\":\"open\",\"prompt\":\"请说明你的决定或补充信息\",\"items\":[]}}\n\
             - 结构化确认列表（reply_mode=structured）：\
             {{\"title\":\"不超过22字的确认事项摘要\",\"reply_mode\":\"structured\",\"prompt\":\"请确认以下事项\",\"items\":[\
             {{\"id\":\"q1\",\"label\":\"问题标题\",\"input_type\":\"confirm\",\"options\":[],\"required\":true}},\
             {{\"id\":\"q2\",\"label\":\"问题标题\",\"input_type\":\"select\",\"options\":[\"选项A\",\"选项B\"],\"required\":true}},\
             {{\"id\":\"q3\",\"label\":\"问题标题\",\"input_type\":\"text\",\"required\":false,\"placeholder\":\"请输入\"}}\
             ]}}\n\
             input_type 可选：confirm（确认，固定为 是/否）、select（单选，需 options）、text（文本填写）。\n\
             6. 无论选择何种 action，处置与确认问题都必须始终围绕上述【初始目标】。"
        );
        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;

        let action = match v["action"].as_str().unwrap_or("skip").trim() {
            "retry" => "retry".to_string(),
            "reassign" => "reassign".to_string(),
            "abort" => "abort".to_string(),
            "ask_user" => "ask_user".to_string(),
            _ => "skip".to_string(),
        };
        let assignee = v["assignee"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let reason = v["reason"].as_str().unwrap_or("").trim().to_string();

        let confirmation = if action == "ask_user" {
            Some(build_confirmation(topic, task_id, error, &v))
        } else {
            None
        };

        Some(FailureDecision { action, assignee, reason, confirmation })
    }

    /// 工具授权裁决：对中/高风险工具调用做轻量结构化裁决，返回是否放行及理由。
    /// 注入当前讨论目标作为「任务必要性」依据；请求失败/解析失败时按风险分级回退
    /// （High 拒绝，Medium/Low 放行），避免频繁工具调用因裁决不稳定而批量卡死。
    pub async fn authorize_tool(&self, topic: &str, tool_name: &str, args: &str, risk: &str) -> ToolDecision {
        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。执行者发起了一次工具调用，需要你裁决是否放行。\n\
             【当前讨论目标】\n{topic}\n\n\
             工具名：{tool_name}\n风险等级：{risk}\n参数：{args}\n\n\
             请基于安全性与任务必要性进行裁决：若该工具调用服务于【当前讨论目标】且参数合理则放行；\
             若明显危险、越权或与目标无关则拒绝。输出严格 JSON（不要 markdown 代码块），格式：\
             {{\"allow\":true或false,\"reason\":\"简要理由\"}}"
        );

        let raw = match self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log::warn!(
                    "[GroupChat] Director 工具授权裁决调用失败: tool={} err={}，按风险等级回退",
                    tool_name,
                    e
                );
                let fallback = fallback_decision(risk);
                return ToolDecision {
                    allow: fallback,
                    reason: "裁决调用失败，按风险等级回退".to_string(),
                };
            }
        };

        let v: serde_json::Value = match serde_json::from_str(strip_fences(&raw)) {
            Ok(v) => v,
            Err(_) => {
                log::warn!(
                    "[GroupChat] Director 工具授权裁决解析失败: tool={}，按风险等级回退",
                    tool_name
                );
                let fallback = fallback_decision(risk);
                return ToolDecision {
                    allow: fallback,
                    reason: "裁决结果解析失败，按风险等级回退".to_string(),
                };
            }
        };

        let allow = v["allow"].as_bool().unwrap_or_else(|| fallback_decision(risk));
        let reason = v["reason"].as_str().unwrap_or("").trim().to_string();
        log::info!(
            "[GroupChat] Director 工具授权裁决: tool={} allow={} reason={}",
            tool_name,
            allow,
            reason
        );
        ToolDecision { allow, reason }
    }

    /// 执行者连续无进展时，裁决是否再给一次推进机会（true=继续，false=收尾）。
    /// 成功返回 `Some(bool)`；LLM 不可用/解析失败返回 `None`（调用方按"不继续"处理）。
    pub async fn decide_stall_continue(&self, topic: &str, summary: &str) -> Option<bool> {
        let prompt = format!(
            "你是多 Agent 群聊的协调者（Director）。执行者已连续多轮无实质进展（未成功执行工具且未输出内容）。\n\
             初始目标：{topic}\n当前摘要：{summary}\n\n\
             请裁决是否给执行者最后一次推进机会。若任务方向仍清晰、继续有价值则 continue 为 true；\
             若已陷入空转则 continue 为 false（进入收尾总结）。输出严格 JSON（不要 markdown 代码块），格式：\
             {{\"continue\":true或false,\"reason\":\"简要理由\"}}"
        );

        let raw = self
            .llm
            .complete(&system_prompt(), &[ChatMessage::user(&prompt)], None)
            .await
            .ok()?;

        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let continue_loop = v["continue"].as_bool().unwrap_or(false);
        let reason = v["reason"].as_str().unwrap_or("").trim().to_string();
        log::info!(
            "[GroupChat] Director 停滞裁决: continue={} reason={}",
            continue_loop,
            reason
        );
        Some(continue_loop)
    }
}

/// 裁决失败时的风险分级回退：High 拒绝，Medium/Low 放行（避免工具链因裁决不稳定而批量卡死）。
fn fallback_decision(risk: &str) -> bool {
    !risk.eq_ignore_ascii_case("high")
}

fn system_prompt() -> String {
    "你是多 Agent 群聊的协调者（Director）。你的职责是编排讨论、分配角色、选择发言者、总结与裁决。\
     你只输出结构化决策，不作为普通参与者发言。".to_string()
}

/// 从 Director 的 ask_user 决策 JSON 中构建确认请求；解析失败则退化为开放式确认。
fn build_confirmation(topic: &str, task_id: &str, error: &str, v: &serde_json::Value) -> ConfirmationRequest {
    let fallback = format!("初始目标：{}\n任务执行失败：{}\n请确认如何继续。", topic, error);
    build_confirmation_from_json(&v["confirmation"], task_id, &fallback)
}

/// 从 JSON 解析确认请求（title/reply_mode/prompt/items）；prompt 缺失时用 fallback。
fn build_confirmation_from_json(c: &serde_json::Value, task_id: &str, fallback_prompt: &str) -> ConfirmationRequest {
    let reply_mode = match c["reply_mode"].as_str().unwrap_or("open") {
        "structured" => "structured".to_string(),
        _ => "open".to_string(),
    };
    let prompt = c["prompt"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_prompt.to_string());
    // 标题优先用 LLM 生成的摘要（≤22 字），缺失时从 prompt 首行截取兜底。
    let title = c["title"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| summarize_confirmation_title(&s))
        .unwrap_or_else(|| summarize_confirmation_title(&prompt));

    let items = if reply_mode == "structured" {
        c["items"]
            .as_array()
            .map(|arr| arr.iter().map(|it| ConfirmationItem::from_json(it, "input_type")).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    ConfirmationRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        task_id: task_id.to_string(),
        title,
        prompt,
        reply_mode,
        items,
    }
}

/// 去除 LLM 输出可能包裹的 ```json ... ``` 代码块围栏。
fn strip_fences(raw: &str) -> &str {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("```json").unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix("```").unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix("```").unwrap_or(trimmed);
    trimmed.trim()
}
