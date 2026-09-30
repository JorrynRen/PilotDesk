//! Director：全知协调者（非自主发言者）。依赖 LlmClient 输出结构化决策。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::models::{summarize_confirmation_title, ConfirmationItem, ConfirmationRequest, TaskRow};
use super::participant::{Attitude, ChatMessage, LlmClient, Stance};

/// 组装「各参与者当前立场」区块文本（含态度标签）。
fn format_stances(stances: &[Stance]) -> String {
    if stances.is_empty() {
        return "（暂无）".to_string();
    }
    stances
        .iter()
        .map(|st| {
            let tag = match st.attitude {
                Attitude::Agree => "支持",
                Attitude::Disagree => "反对",
                Attitude::Neutral => "中立",
            };
            format!("- [@{}]: [{}] {}", st.participant_id, tag, st.stance)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 开场编排产出的任务草稿。
#[derive(Debug, Clone)]
pub struct TaskDraft {
    pub description: String,
    /// 前置任务编号（0 基）。
    pub depends_on: Vec<usize>,
    /// 负责执行该任务的参与者 id（由 Director 指派，可为空）。
    pub assignee: Option<String>,
}

/// Director 按需补充的新参与者草稿（全自动组建名册：主持人发现参与者不足/缺角色时提出）。
#[derive(Debug, Clone)]
pub struct NewParticipantDraft {
    /// 唯一 id（仅含字母/数字/下划线/连字符，如 api_3、designer_1）。
    pub id: String,
    pub display_name: String,
    /// 角色定位：简洁、只写专业方向，禁止携带具体任务属性（任务协调统一由主持人负责）。
    pub system_role: String,
    /// "api"=通用 LLM 参与者（provider/model 必填）；"cli"=本地 CLI Agent（agent_type 必填）。
    pub participant_type: String,
    /// 提供商 id（api 类型必填，必须存在于可用模型清单）。
    pub provider: String,
    /// 文本生成模型名（api 类型必填，必须从模型清单逐字复制，禁止编造）。
    pub model: String,
    /// 已注册 CLI Agent 类型（cli 类型必填，必须从可用 CLI Agent 清单逐字复制，禁止编造；
    /// 同一房间同一 agent_type 只允许 1 个）。
    pub agent_type: String,
    /// 补充原因（为什么需要该参与者，如"名册缺少视觉设计能力"；供调度消息可追溯展示，可为空）。
    pub reason: String,
}

/// 开场编排结果：任务草稿 + 角色分配 + 按需补充的参与者。
#[derive(Debug, Clone)]
pub struct KickoffPlan {
    pub tasks: Vec<TaskDraft>,
    pub roles: HashMap<String, String>,
    pub new_participants: Vec<NewParticipantDraft>,
    /// 未被 Director 覆盖角色、由代码兜底补"未分配"的参与者 id（调用方据此落提示）。
    pub missing_roles: Vec<String>,
}

/// 阶段A 目标确定结果：主持人理解整理后的总目标 + 是否需要向用户确认。
#[derive(Debug, Clone)]
pub struct GoalClarification {
    /// 整理后的总目标（主持人视角：目标 + 关键约束 + 可验证验收标准；不得复制用户原文）。
    pub goal: String,
    /// 目标存在歧义 / 缺关键基础信息，需先向用户确认再编排。
    pub need_confirmation: bool,
    /// 需要向用户确认的具体问题（need_confirmation=true 时非空）。
    pub questions: Vec<String>,
    /// 主持人取的简短产物目录名（≤20 字符，贴切概括；空则回退按主题截断）。
    pub output_dir_name: String,
}

/// 阶段A 确认回复消化结果（v3.5c）：主持人理解并消化用户回复（可能含材料核验简报），
/// 动态更新问题答案/问题项/目标方向，并判定是否仍需继续向用户确认。
#[derive(Debug, Clone)]
pub struct GoalClarifyReply {
    /// 消化后的目标（基于回复/材料更新约束、方向与验收；不得复制用户口语原文）。
    pub adjusted_goal: String,
    /// 本轮回合已确定/已落实的信息要点（含从用户指向材料核验得到的项，供追溯）。
    pub resolved: Vec<String>,
    /// 仍缺失、影响任务拆分、且回复/材料/简报均无法确定的项（need_more=true 时非空）。
    pub remaining_questions: Vec<String>,
    /// 是否仍需继续向用户确认（仅当 remaining_questions 非空时为 true）。
    pub need_more: bool,
    /// 更新后的简短产物目录名（未变化时为空串表示沿用）。
    pub output_dir_name: String,
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
    /// 需调整角色的参与者（仅包含角色变化的；无调整需求时为空）。
    pub roles: HashMap<String, String>,
    /// 讨论中发现的缺失角色（按需补充参与者；无补充需求时为空）。
    pub new_participants: Vec<NewParticipantDraft>,
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
    /// retry / reassign / skip / abort / ask_user / self_execute / add_participant
    pub action: String,
    /// reassign 时的目标参与者 id
    pub assignee: Option<String>,
    /// 决策理由
    pub reason: String,
    /// ask_user 时向用户发起的确认请求
    pub confirmation: Option<ConfirmationRequest>,
    /// add_participant 时建议补充的新参与者（接盘失败任务，可同时满足"重派"效果）
    pub new_participant: Option<NewParticipantDraft>,
    /// 同时移除的僵尸参与者 id（反复失败/能力不可用占位，可与其他 action 组合，如替换）
    pub remove_participant: Option<String>,
}

/// Director 选中的下一位发言者及其选择理由。
#[derive(Debug, Clone)]
pub struct SpeakerDecision {
    pub speaker: String,
    pub reason: String,
    /// 本轮发言者的具体工作内容（v3.4ao）：主持人"派活"——讨论中则告知
    /// "就 X 的方案/分工/验收标准发表意见"，方案已定则告知"执行 X：具体步骤与产出要求"。
    /// 空串时调用方回退原"围绕任务发言"语义。
    pub work_content: String,
    /// 名册缺身份/能力时的补人提案（v3.4bb）：由系统创建专用参与者承接该身份，
    /// **不会**让现有参与者临时扮演他人（身份纪律由提示词层约束）。空数组表示不需补人。
    pub new_participants: Vec<NewParticipantDraft>,
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
    /// 主持人收口裁决（v3.4ao）：next_action=done 时，判定为"讨论中已被实质完成、执行阶段应跳过"的
    /// discussing 任务 id 列表（主持人最终权威；产物/发言仅为裁决依据）。
    pub completed_tasks: Vec<String>,
}

/// 立场预处理结果（LLM 从单次发言提取的归一化立场）。
#[derive(Debug, Clone)]
pub struct StanceExtraction {
    /// 态度（支持/反对/中立）。
    pub attitude: Attitude,
    /// 归一化立场陈述（≤100 字，第一人称）。
    pub stance: String,
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
    /// / amend_goal（目标范围扩展，修正初始目标）/ temp_task（临时性一次性额外任务，不改目标）
    /// / switch_director（更换主持人，结构身份变更）
    pub intent: String,
    /// change_goal / new_goal 时的新目标文本（其余为空）
    pub new_goal: String,
    /// amend_goal 时追加到目标锚点的补充文本（其余为空）
    pub amend: String,
    /// refine 时提炼出的补充/细化约束（可空；change_goal/new_goal/amend_goal/temp_task 时忽略）
    pub note: String,
    /// temp_task 时的临时额外任务（其余为 None）
    pub temp_task: Option<TempTaskDraft>,
    /// switch_director 时的新主持人参与者 id（其余为 None）
    pub director: Option<String>,
}

/// 用户插队的临时性、一次性额外任务（不更新初始目标，执行完即弃）。
#[derive(Debug, Clone)]
pub struct TempTaskDraft {
    pub description: String,
    pub assignee: Option<String>,
}

/// Director 结构化决策输出。
pub struct Director {
    llm: Arc<dyn LlmClient>,
    /// 主持人自身的参与者 id（通信标识 [@id]），供自我身份感知与引用规范。
    director_id: String,
    /// 最近一次 LLM 调用的思考链（reasoning_content），供决策消息落库、前端无差异展示。
    last_reasoning: Mutex<String>,
}

impl Director {
    pub fn new(llm: Arc<dyn LlmClient>, director_id: String) -> Self {
        Self {
            llm,
            director_id,
            last_reasoning: Mutex::new(String::new()),
        }
    }

    /// 调用 LLM 完成一次决策：捕获思考链（reasoning_content）供落库展示，返回决策正文。
    /// 与参与者一致使用 complete_with_tool_calls，使主持人推理也能被前端无差异渲染。
    async fn complete_decision(&self, prompt: &str) -> Result<String, String> {
        let (content, _tool_calls, reasoning) = self
            .llm
            .complete_with_tool_calls(
                &self.system_prompt(),
                &[ChatMessage::user(prompt)],
                None,
                None,
            )
            .await?;
        if let Ok(mut g) = self.last_reasoning.lock() {
            *g = reasoning;
        }
        Ok(content)
    }

    /// 取走最近一次决策的思考链（取后清空，避免重复落库）。
    pub fn take_last_reasoning(&self) -> String {
        self.last_reasoning
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }

    fn system_prompt(&self) -> String {
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_system_prompt.prompt.md。
        super::prompts::render(
            super::prompts::blocks::DIRECTOR_SYSTEM_PROMPT.body,
            &super::prompts::PromptCtx::new().var("director_id", &self.director_id),
        )
    }

    /// 阶段A 目标确定：主持人理解并整理用户目标（生成总目标），判定是否需要先向用户确认。
    /// `topic` 为当前目标锚点（首次整理时可为空或占位），`latest` 为用户最新指令原文，
    /// `roster` 为参与者名册（id/显示名/角色，供理解用户提及的人称）。失败返回 `None`（调用方降级）。
    pub async fn clarify_goal(
        &self,
        topic: &str,
        latest: &str,
        notes: &str,
        roster: &str,
    ) -> Option<GoalClarification> {
        let topic_section = if topic.trim().is_empty() {
            "（房间尚无既定目标，本次为初始目标整理）".to_string()
        } else {
            topic.to_string()
        };
        let notes_section = if notes.trim().is_empty() {
            "（暂无补充约束）".to_string()
        } else {
            notes.to_string()
        };
        let roster_section = if roster.trim().is_empty() {
            "（暂无参与者名册）".to_string()
        } else {
            roster.to_string()
        };

        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_clarify_goal.prompt.md，
        // 此处仅组装输入数据段（latest/topic/notes/roster），文本全部来自块资产。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_CLARIFY_GOAL.body,
            &super::prompts::PromptCtx::new()
                .section("latest", latest)
                .section("topic", topic_section)
                .section("notes", notes_section)
                .section("roster", roster_section),
        );

        let raw = self.complete_decision(&prompt).await.ok()?;
        let json = strip_fences(&raw);
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let goal = v["goal"]
            .as_str()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if goal.is_empty() {
            return None;
        }
        let need_confirmation = v["need_confirmation"].as_bool().unwrap_or(false);
        let questions = v["questions"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|q| q.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let output_dir_name = v["output_dir_name"]
            .as_str()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        Some(GoalClarification {
            goal,
            need_confirmation,
            questions,
            output_dir_name,
        })
    }

    /// 阶段A 确认回复消化（v3.5c）：主持人理解并消化用户对确认问题的回复，
    /// 结合已解析输入物/约束（notes）与材料核验简报（evidence），输出：消化后的目标、
    /// 已确定信息要点、仍需向用户追问的剩余问题，以及是否需继续确认。
    /// `goal` 为当前整理中的目标，`reply` 为用户本轮回复原文。失败返回 `None`（调用方降级为摘要兜底）。
    pub async fn clarify_reply(
        &self,
        goal: &str,
        reply: &str,
        notes: &str,
        roster: &str,
        evidence: &str,
    ) -> Option<GoalClarifyReply> {
        let notes_section = if notes.trim().is_empty() {
            "（暂无补充约束）".to_string()
        } else {
            notes.to_string()
        };
        let evidence_section = if evidence.trim().is_empty() {
            "（本轮未执行材料核验）".to_string()
        } else {
            evidence.to_string()
        };
        let roster_section = if roster.trim().is_empty() {
            "（暂无参与者名册）".to_string()
        } else {
            roster.to_string()
        };

        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_CLARIFY_REPLY.body,
            &super::prompts::PromptCtx::new()
                .section("goal", goal)
                .section("reply", reply)
                .section("notes", notes_section)
                .section("evidence", evidence_section)
                .section("roster", roster_section),
        );

        let raw = self.complete_decision(&prompt).await.ok()?;
        let json = strip_fences(&raw);
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let adjusted_goal = v["adjusted_goal"]
            .as_str()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if adjusted_goal.is_empty() {
            return None;
        }
        let strings = |key: &str| {
            v[key]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|q| q.as_str().map(|s| s.trim().to_string()))
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let need_more = v["need_more"].as_bool().unwrap_or(false);
        let output_dir_name = v["output_dir_name"]
            .as_str()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        Some(GoalClarifyReply {
            adjusted_goal,
            resolved: strings("resolved"),
            remaining_questions: strings("remaining_questions"),
            need_more,
            output_dir_name,
        })
    }

    /// 开场编排：产出子任务/议程 + 为参与者分配角色 + 按需补充缺失角色参与者。
    /// `topic` 为初始目标（不可变锚点），`latest` 为本轮新增指令（首轮两者相同，
    /// 追加/确认轮为用户最新输入），`catalog` 为可用模型 + CLI Agent 清单（供按需组建参与者选模型/agent），
    /// `max_new_participants` 为本场可补充空位（剩余空位与批量上限取小，0 表示禁止新增）。
    /// 返回任务草稿、角色分配与建议新增的参与者；失败返回 `None`。
    pub async fn kickoff(
        &self,
        topic: &str,
        latest: &str,
        notes: &str,
        participants: &[(String, String, String)],
        catalog: &str,
        max_new_participants: usize,
        failures: &str,
    ) -> Option<KickoffPlan> {
        // 名册三字段：唯一 id / 显示名（仅作角色分配时参考用户定位倾向）/ 当前角色。
        let roster = participants
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

        // 初始目标与新增指令分离：防止追加/确认轮次脱离原始目标。
        let latest_section = if topic == latest {
            "（本轮无新增指令，直接围绕初始目标编排）".to_string()
        } else {
            format!(
                "【本轮新增指令（用户刚刚补充/确认，需纳入编排）】\n{}",
                latest
            )
        };
        let notes_section = if notes.trim().is_empty() {
            "（暂无补充约束）".to_string()
        } else {
            format!("【已累积的补充/细化约束（必须一并满足）】\n{}", notes)
        };
        let catalog_section = if catalog.trim().is_empty() {
            "（无可用模型/CLI Agent）".to_string()
        } else {
            catalog.to_string()
        };
        let failures_section = if failures.trim().is_empty() {
            "（暂无失败记录）".to_string()
        } else {
            failures.to_string()
        };

        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_kickoff.prompt.md，
        // 此处仅组装输入数据段（topic/notes/latest/roster/catalog/failures）与单值变量。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_KICKOFF.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("notes", notes_section)
                .section("latest", latest_section)
                .section("roster", roster)
                .section("catalog", catalog_section)
                .section("failures", failures_section)
                .var("max_new_participants", max_new_participants.to_string()),
        );

        let raw = self.complete_decision(&prompt).await.ok()?;
        let json = strip_fences(&raw);
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;

        let tasks = v["tasks"]
            .as_array()?
            .iter()
            .map(|t| TaskDraft {
                description: t["description"]
                    .as_str()
                    .unwrap_or("未命名任务")
                    .to_string(),
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
                // 归一化 key：LLM 可能回显 [@id]/@id，直接作为 id 写库会匹配不到裸 id 行（角色分配不落库）。
                roles.insert(
                    normalize_ref(k),
                    val.as_str().unwrap_or("参与者").to_string(),
                );
            }
        }

        // v3.4as：roles 完整性兜底——Director 未覆盖的参与者补"未分配"，不阻断编排，交由调用方落提示。
        let mut missing_roles = Vec::new();
        for (id, _, _) in participants {
            if !roles.contains_key(id) {
                roles.insert(id.clone(), "未分配".to_string());
                missing_roles.push(id.clone());
            }
        }

        Some(KickoffPlan {
            tasks,
            roles,
            new_participants: parse_new_participants(&v),
            missing_roles,
        })
    }

    /// 追加轮次对账重排：把当前任务清单（含状态）交给 Director，输出 keep/remove/add 操作。
    /// 运行时据此删减未执行任务、重分配负责人、新增必要子任务，从而替代盲目累加。
    /// `catalog` 为可用模型 + CLI Agent 清单（供按需组建参与者选模型/agent），`max_new_participants` 为本场
    /// 可补充空位（与剩余空位取小，0 表示禁止新增）。
    /// 失败返回 `None`（由调用方按保守策略处理：不新增、不删减）。
    pub async fn replan(
        &self,
        topic: &str,
        latest: &str,
        notes: &str,
        existing: &[TaskRow],
        participants: &[(String, String, String)],
        catalog: &str,
        max_new_participants: usize,
        failures: &str,
    ) -> Option<ReplanPlan> {
        // 名册三字段：唯一 id / 显示名（仅作理解用户指令中的人称引用）/ 当前角色。
        let roster = participants
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
        let catalog_section = if catalog.trim().is_empty() {
            "（无可用模型/CLI Agent）".to_string()
        } else {
            catalog.to_string()
        };
        let failures_section = if failures.trim().is_empty() {
            "（暂无失败记录）".to_string()
        } else {
            failures.to_string()
        };

        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_replan.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_REPLAN.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("notes", notes_section)
                .section("latest", latest)
                .section("roster", roster)
                .section("catalog", catalog_section)
                .section("failures", failures_section)
                .section("existing_tasks", existing_text)
                .var("max_new_participants", max_new_participants.to_string()),
        );

        let raw = self.complete_decision(&prompt).await.ok()?;
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
                            .filter_map(|x| {
                                x.as_str()
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                            })
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

        let mut roles = HashMap::new();
        if let Some(obj) = v["roles"].as_object() {
            for (k, val) in obj {
                // 归一化 key：LLM 可能回显 [@id]/@id，直接作为 id 写库会匹配不到裸 id 行（角色分配不落库）。
                roles.insert(
                    normalize_ref(k),
                    val.as_str().unwrap_or("参与者").to_string(),
                );
            }
        }

        Some(ReplanPlan {
            operations,
            roles,
            new_participants: parse_new_participants(&v),
        })
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
                let role = if role.trim().is_empty() {
                    "未分配".to_string()
                } else {
                    role.clone()
                };
                format!("- [@{}]（显示名：{}）（角色：{}）", id, name, role)
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
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(" | 说明={}", detail)
                    }
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

        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_replan_for_roster_change.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_REPLAN_FOR_ROSTER_CHANGE.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("notes", notes_section)
                .section("roster", roster)
                .section("existing_tasks", existing_text),
        );

        let raw = self.complete_decision(&prompt).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;

        let mut roles = HashMap::new();
        if let Some(obj) = v["roles"].as_object() {
            for (k, val) in obj {
                // 归一化 key：LLM 可能回显 [@id]/@id，直接作为 id 写库会匹配不到裸 id 行（角色分配不落库）。
                roles.insert(
                    normalize_ref(k),
                    val.as_str().unwrap_or("参与者").to_string(),
                );
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
                            .filter_map(|x| {
                                x.as_str()
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                            })
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

    /// 判断用户最新输入对初始目标的意图类型（五分类）。
    /// 默认保守：仅在语义明确时才判 `change_goal` / `new_goal` / `amend_goal` / `temp_task`，
    /// 其余按 `refine`；但「用户明确改变目标/方向」不得被降级吞并。失败返回 `None`（由调用方按 `refine` 处理）。
    pub async fn classify_goal_intent(
        &self,
        topic: &str,
        latest: &str,
        notes: &str,
        roster: &[(String, String, String)],
    ) -> Option<GoalIntent> {
        let notes_display = if notes.trim().is_empty() {
            "（暂无）".to_string()
        } else {
            notes.to_string()
        };
        let roster_section = if roster.is_empty() {
            "（暂无参与者）".to_string()
        } else {
            roster
                .iter()
                .map(|(id, name, role)| {
                    let role = if role.trim().is_empty() {
                        "未分配"
                    } else {
                        role.as_str()
                    };
                    format!("- [@{}]（显示名：{}）（角色：{}）", id, name, role)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_classify_goal_intent.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_CLASSIFY_GOAL_INTENT.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("notes", notes_display)
                .section("roster", roster_section)
                .section("latest", latest),
        );
        let raw = self.complete_decision(&prompt).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let intent = match v["intent"].as_str().unwrap_or("refine") {
            "change_goal" => "change_goal".to_string(),
            "new_goal" => "new_goal".to_string(),
            "amend_goal" => "amend_goal".to_string(),
            "temp_task" => "temp_task".to_string(),
            "switch_director" => "switch_director".to_string(),
            _ => "refine".to_string(),
        };
        let new_goal = v["new_goal"].as_str().unwrap_or("").trim().to_string();
        let amend = v["amend"].as_str().unwrap_or("").trim().to_string();
        let note = v["note"].as_str().unwrap_or("").trim().to_string();
        let temp_task = if v["temp_task"].is_object() {
            let d = v["temp_task"].clone();
            Some(TempTaskDraft {
                description: d["description"].as_str().unwrap_or("").trim().to_string(),
                assignee: d["assignee"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            })
        } else {
            None
        };
        let director = v["director"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(GoalIntent {
            intent,
            new_goal,
            amend,
            note,
            temp_task,
            director,
        })
    }

    /// 判断用户对确认请求的回复语义：`confirm`（回复/补充确认项）或 `directive`（新指令/改方向）。
    /// 完全由 LLM 依据内容语义判断，不约定硬编码规则；失败默认按 `confirm` 处理。
    pub async fn classify_reply_intent(
        &self,
        confirmation_prompt: &str,
        reply: &str,
    ) -> Option<String> {
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_classify_reply_intent.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_CLASSIFY_REPLY_INTENT.body,
            &super::prompts::PromptCtx::new()
                .section("confirmation_prompt", confirmation_prompt)
                .section("reply", reply),
        );
        let raw = self.complete_decision(&prompt).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        Some(match v["intent"].as_str().unwrap_or("confirm") {
            "directive" => "directive".to_string(),
            _ => "confirm".to_string(),
        })
    }

    /// 选择下一发言者（依据角色匹配/立场分歧/话题相关性/@提及/用户最新指令）。
    /// 失败返回 `None`（由 RuleEngine 轮询兜底）。
    /// `recent_speeches` 为最近发言（发言者 + 内容），供 Director 判断任务是否已被实质推进。
    pub async fn next_speaker(
        &self,
        topic: &str,
        summary: &str,
        participants: &[(String, String, String)],
        task_manifest: &str,
        stances: &[Stance],
        mention: Option<&str>,
        latest_directive: &str,
        notes: &str,
        failures: &str,
        recent_speeches: &str,
        catalog: &str,
        max_new_participants: usize,
    ) -> Option<SpeakerDecision> {
        // 名册三字段（与 kickoff/replan 一致）：唯一 id / 显示名 / 当前角色。
        // 身份信息缺失会迫使主持人靠发言反推身份、导致臆造角色（如把正方当成评委组长）。
        let roster = participants
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
        let mention_hint = mention
            .map(|m| format!("用户 @ 了：[@{}]", m))
            .unwrap_or_default();
        let task_section = if task_manifest.trim().is_empty() {
            "（暂无任务清单）".to_string()
        } else {
            task_manifest.to_string()
        };
        let stance_section = format_stances(stances);
        let directive_section = if latest_directive.trim().is_empty() {
            "（暂无）".to_string()
        } else {
            latest_directive.to_string()
        };
        let notes_section = if notes.trim().is_empty() {
            "（暂无）".to_string()
        } else {
            notes.to_string()
        };
        let failures_section = if failures.trim().is_empty() {
            "（暂无失败记录）".to_string()
        } else {
            failures.to_string()
        };
        let recent_speeches_section = if recent_speeches.trim().is_empty() {
            "（暂无最近发言）".to_string()
        } else {
            recent_speeches.to_string()
        };
        let catalog_section = if catalog.trim().is_empty() {
            "（无可用模型/CLI Agent）".to_string()
        } else {
            catalog.to_string()
        };

        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_next_speaker.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_NEXT_SPEAKER.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("summary", summary)
                .section("task_manifest", task_section)
                .section("roster", roster)
                .section("mention", mention_hint)
                .section("stances", stance_section)
                .section("latest", directive_section)
                .section("notes", notes_section)
                .section("failures", failures_section)
                .section("recent_speeches", recent_speeches_section)
                .section("catalog", catalog_section)
                .var("max_new_participants", max_new_participants.to_string()),
        );

        let raw = self.complete_decision(&prompt).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let speaker = v["next_speaker"]
            .as_str()
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let new_participants = parse_new_participants(&v);
        if speaker.is_empty() && new_participants.is_empty() {
            None
        } else {
            let reason = v["reason"].as_str().unwrap_or("").trim().to_string();
            let work_content = v["work_content"].as_str().unwrap_or("").trim().to_string();
            Some(SpeakerDecision {
                speaker,
                reason,
                work_content,
                new_participants,
            })
        }
    }

    /// 立场预处理：从单次发言提取归一化立场（态度 + ≤100 字陈述）。
    /// LLM 失败或输出不合法时返回 None，由调用方回退到机械截断 + 关键词分类。
    pub async fn extract_stance(&self, speaker: &str, speech: &str) -> Option<StanceExtraction> {
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_extract_stance.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_EXTRACT_STANCE.body,
            &super::prompts::PromptCtx::new()
                .section("speech", speech)
                .var("speaker", speaker),
        );
        let raw = self.complete_decision(&prompt).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let attitude = match v["attitude"].as_str().unwrap_or("").trim() {
            "支持" => Attitude::Agree,
            "反对" => Attitude::Disagree,
            "中立" => Attitude::Neutral,
            _ => return None,
        };
        let stance = v["stance"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        Some(StanceExtraction { attitude, stance })
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
        stances: &[Stance],
        latest_directive: &str,
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
        let stance_section = format_stances(stances);
        let directive_section = if latest_directive.trim().is_empty() {
            "（暂无）".to_string()
        } else {
            latest_directive.to_string()
        };
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_review.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_REVIEW.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("notes", notes_display)
                .section("latest", directive_section)
                .section("task_manifest", task_display)
                .section("summary", old_summary)
                .section("transcript", transcript)
                .section("stances", stance_section),
        );
        let raw = self.complete_decision(&prompt).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let summary = v["summary"].as_str().unwrap_or("").trim().to_string();
        let next_action = match v["next_action"].as_str().unwrap_or("discuss").trim() {
            "confirm" => "confirm".to_string(),
            "replan" => "replan".to_string(),
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
        let completed_tasks = v["completed_tasks"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        x.as_str()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(ReviewOutcome {
            summary,
            next_action,
            reason,
            confirmation,
            completed_tasks,
        })
    }

    /// 语义解析用户输入物引用（v3.4an LLM 层）：只输出锚点枚举 + 路径段结构，
    /// 不允许输出路径字符串（防幻觉）；路径拼接与存在性校验由调用方代码完成。
    /// 失败/超时返回 None（调用方仅用确定性解析结果，不阻塞编排）。
    pub async fn parse_input_refs(&self, texts: &str) -> Option<Vec<(String, Vec<String>)>> {
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_parse_input_refs.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_PARSE_INPUT_REFS.body,
            &super::prompts::PromptCtx::new().section("texts", texts),
        );
        let raw = self.complete_decision(&prompt).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;
        let arr = v.as_array()?;
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for item in arr {
            let anchor = item["anchor"].as_str().unwrap_or("").to_string();
            let subpath: Vec<String> = item["subpath"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if !subpath.is_empty() {
                out.push((anchor, subpath));
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// 收敛结论。
    pub async fn conclude(
        &self,
        topic: &str,
        summary: &str,
        transcript: &str,
        task_manifest: &str,
    ) -> Option<String> {
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_conclude.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_CONCLUDE.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("summary", summary)
                .section("task_manifest", task_manifest)
                .section("transcript", transcript),
        );
        let raw = self.complete_decision(&prompt).await.ok()?;
        Some(raw.trim().to_string())
    }

    /// 任务执行失败后的兜底裁决：返回结构化处置决策（重试/换人/跳过/终止/向用户确认/主持人亲自执行/补充参与者接盘）。
    /// `topic` 为初始目标锚点，确保失败处置与确认请求不偏离原始目标。
    /// `attempt` 为累计尝试次数（含重试与换人）；`previous_executors` 为已失败执行者（不得再指派）。
    /// `catalog` 为可用模型 + CLI Agent 清单（供 add_participant 选模型/agent），`max_new_participants` 为本场可补充空位。
    /// 失败返回 `None`（由调用方走确定性兜底：轮询换人/跳过）。
    pub async fn handle_task_failure(
        &self,
        topic: &str,
        task_id: &str,
        task_desc: &str,
        error: &str,
        roster: &[(String, String)],
        attempt: usize,
        previous_executors: &[String],
        catalog: &str,
        max_new_participants: usize,
    ) -> Option<FailureDecision> {
        let roster_text = if roster.is_empty() {
            "（无其他可指派执行者）".to_string()
        } else {
            roster
                .iter()
                .map(|(id, role)| {
                    let role = if role.trim().is_empty() {
                        "未分配"
                    } else {
                        role.as_str()
                    };
                    format!("- [@{}]({})", id, role)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let previous_text = if previous_executors.is_empty() {
            "（无）".to_string()
        } else {
            previous_executors
                .iter()
                .map(|e| format!("[@{}]", e))
                .collect::<Vec<_>>()
                .join("、")
        };
        let catalog_section = if catalog.trim().is_empty() {
            "（无可用模型/CLI Agent）".to_string()
        } else {
            catalog.to_string()
        };
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_handle_task_failure.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_HANDLE_TASK_FAILURE.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("task_desc", task_desc)
                .section("error", error)
                .section("previous_executors", previous_text)
                .section("roster", roster_text)
                .section("catalog", catalog_section)
                .var("attempt", attempt.to_string())
                .var("max_new_participants", max_new_participants.to_string()),
        );
        let raw = self.complete_decision(&prompt).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(strip_fences(&raw)).ok()?;

        let action = match v["action"].as_str().unwrap_or("skip").trim() {
            "retry" => "retry".to_string(),
            "reassign" => "reassign".to_string(),
            "abort" => "abort".to_string(),
            "ask_user" => "ask_user".to_string(),
            "self_execute" => "self_execute".to_string(),
            "add_participant" => "add_participant".to_string(),
            _ => "skip".to_string(),
        };
        let assignee = v["assignee"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let reason = v["reason"].as_str().unwrap_or("").trim().to_string();
        let remove_participant = v["remove_participant"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let confirmation = if action == "ask_user" {
            Some(build_confirmation(topic, task_id, error, &v))
        } else {
            None
        };
        let new_participant = if action == "add_participant" {
            parse_new_participants(&v).into_iter().next()
        } else {
            None
        };

        Some(FailureDecision {
            action,
            assignee,
            reason,
            confirmation,
            new_participant,
            remove_participant,
        })
    }

    /// 工具授权裁决：对中/高风险工具调用做轻量结构化裁决，返回是否放行及理由。
    /// 注入当前讨论目标作为「任务必要性」依据；请求失败/解析失败时按风险分级回退
    /// （High 拒绝，Medium/Low 放行），避免频繁工具调用因裁决不稳定而批量卡死。
    pub async fn authorize_tool(
        &self,
        topic: &str,
        tool_name: &str,
        args: &str,
        risk: &str,
    ) -> ToolDecision {
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_authorize_tool.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_AUTHORIZE_TOOL.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("tool_name", tool_name)
                .section("risk", risk)
                .section("args", args),
        );

        let raw = match self.complete_decision(&prompt).await {
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

        let allow = v["allow"]
            .as_bool()
            .unwrap_or_else(|| fallback_decision(risk));
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
        // v3.4aw 提示词资产化：模板位于 prompts/blocks/director_decide_stall_continue.prompt.md。
        let prompt = super::prompts::render(
            super::prompts::blocks::DIRECTOR_DECIDE_STALL_CONTINUE.body,
            &super::prompts::PromptCtx::new()
                .section("topic", topic)
                .section("summary", summary),
        );

        let raw = self.complete_decision(&prompt).await.ok()?;

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

/// 从 Director 的 ask_user 决策 JSON 中构建确认请求；解析失败则退化为开放式确认。
fn build_confirmation(
    topic: &str,
    task_id: &str,
    error: &str,
    v: &serde_json::Value,
) -> ConfirmationRequest {
    let fallback = format!(
        "初始目标：{}\n任务执行失败：{}\n请确认如何继续。",
        topic, error
    );
    build_confirmation_from_json(&v["confirmation"], task_id, &fallback)
}

/// 从 JSON 解析确认请求（title/reply_mode/prompt/items）；prompt 缺失时用 fallback。
fn build_confirmation_from_json(
    c: &serde_json::Value,
    task_id: &str,
    fallback_prompt: &str,
) -> ConfirmationRequest {
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
            .map(|arr| {
                arr.iter()
                    .map(|it| ConfirmationItem::from_json(it, "input_type"))
                    .collect()
            })
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

/// 宽容归一化参与者引用：剥离 `[@...]` / `@` 前缀/后缀得到裸 id。
/// 角色分配（roles）的 key 由 LLM 输出，可能回显带括号/前缀的引用；
/// 若未经归一化就作为 id 写库（UPDATE WHERE id=原始key），将因匹配不到裸 id 行而静默失败——
/// 这正是「角色分配消息显示变更、落库却未生效」的根因。与 room.rs 的 normalize_participant_ref 同语义。
fn normalize_ref(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix("[@") {
        s = rest.strip_suffix(']').unwrap_or(rest).trim();
    } else if let Some(rest) = s.strip_prefix('@') {
        s = rest.trim();
    }
    s.to_string()
}

/// 解析 Director 输出中的 new_participants 数组（按需组建参与者）。
/// 兼容两种输出形态：数组 `new_participants`（kickoff/replan 批量）与
/// 单对象 `new_participant`（失败裁决 add_participant 单点补人）。
/// 逐字段宽容解析：id/provider/model 缺失时丢弃该条；显示名/角色缺失时回退。
fn parse_new_participants(v: &serde_json::Value) -> Vec<NewParticipantDraft> {
    let arr = match v["new_participants"].as_array() {
        Some(arr) => arr.clone(),
        None => match v["new_participant"].as_object() {
            Some(_) => vec![v["new_participant"].clone()],
            None => return Vec::new(),
        },
    };
    arr.iter()
        .filter_map(|p| {
            let id = p["id"]
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())?;
            let display_name = p["display_name"]
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                // V3.5bp：display_name 缺失时不再用裸 id 兜底（否则名册显示 [@api_1] 无显示名、
                // 前端名册与调度消息读不出身份），改用可读兜底「参与者{id}」，避免与 id 混淆。
                .unwrap_or_else(|| format!("参与者{}", id));
            let system_role = p["system_role"]
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| display_name.clone());
            // cli 参与者：agent_type 必填（provider/model 忽略）；api 参与者（缺省）：provider/model 必填。
            let participant_type = match p["participant_type"].as_str().map(|s| s.trim()) {
                Some("cli") => "cli",
                _ => "api",
            };
            let reason = p["reason"]
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            if participant_type == "cli" {
                let agent_type = p["agent_type"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())?;
                Some(NewParticipantDraft {
                    id,
                    display_name,
                    system_role,
                    participant_type: "cli".into(),
                    provider: String::new(),
                    model: String::new(),
                    agent_type,
                    reason,
                })
            } else {
                let provider = p["provider"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())?;
                let model = p["model"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())?;
                Some(NewParticipantDraft {
                    id,
                    display_name,
                    system_role,
                    participant_type: "api".into(),
                    provider,
                    model,
                    agent_type: String::new(),
                    reason,
                })
            }
        })
        .collect()
}
