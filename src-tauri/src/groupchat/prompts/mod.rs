//! 提示词资产化骨架（v3.4aw）：代码专注逻辑，提示词独立为功能块资产。
//!
//! 三维组织：
//! - 业务环节（Flow）：按业务流程组织（接收消息/目标确认/编排/讨论/执行/评估/结论）
//! - 场景（Scene）：环节内的一次 LLM 调用，声明输入数据段/记忆层与引用的提示词块
//! - 提示词块（Block）：静态提示词资产（.prompt.md，include_str! 嵌入），被场景引用
//!
//! 运行时值分层注入（提示词块只含占位符，不含动态值）：
//! - `{{VAR:x}}`：单值变量（director_id、max_new_participants、round 等）
//! - `{{SECTION:x}}`：数据段（roster、task_manifest、transcript、四层记忆等）
//!
//! 压缩原则：LLM 输入侧不做硬截断；压缩只允许发生在 LLM 输出侧（L1 摘要、执行者结论）。
//!
//! 迁移状态（v3.4aw）：director 全部决策点 + participant（LLM/CLI）+ room 讨论轮约束
//! 已迁移为块资产；scenes/flows/sections 提供注册表校验与场景矩阵，测试兜底渲染完整性。

use std::collections::HashMap;

pub mod flows;
pub mod scenes;
pub mod sections;

/// 组装上下文：单值变量 + 数据段（均为渲染后的字符串）+ 块级语义变体。
#[derive(Default)]
pub struct PromptCtx {
    pub vars: HashMap<&'static str, String>,
    pub sections: HashMap<&'static str, String>,
    /// 块级语义变体值（替换块 body 中的 {{VARIANT}}，如参与者模式 llm/cli）。
    pub variant: Option<String>,
}

impl PromptCtx {
    pub fn new() -> Self {
        Self::default()
    }
    /// 注册单值变量。
    pub fn var(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.vars.insert(key, value.into());
        self
    }
    /// 注册数据段（整段文本）。
    pub fn section(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.sections.insert(key, value.into());
        self
    }
    /// 注入块级语义变体值（替换块 body 中的 {{VARIANT}}）。
    pub fn variant(mut self, value: impl Into<String>) -> Self {
        self.variant = Some(value.into());
        self
    }
}

/// 渲染模板：递归展开 `{{FRAGMENT:id|variant}}` → 替换 `{{SECTION:x}}`/`{{VAR:x}}`。
/// 数据注入（VAR/SECTION/FRAGMENT）从集合硬替换；`{{VARIANT}}` 为语义变体（场景引用时覆写）。
/// 未解析占位符在 debug 构建下告警。
pub fn render(template: &str, ctx: &PromptCtx) -> String {
    // 1. 递归展开片段引用（迭代上限防循环；上限须大于任何块的全部片段引用数，
    //    如 kickoff/replan 现含 11 个片段引用，8 曾不够导致尾部未展开）。
    let mut out = template.to_string();
    let mut depth = 0;
    while depth < 64 {
        match find_fragment_placeholder(&out) {
            Some((placeholder, id, variant)) => {
                let frag = fragments::ALL
                    .iter()
                    .find(|f| f.id == id)
                    .unwrap_or_else(|| panic!("[prompts] 引用了未注册片段: {}", id));
                if frag.variant.is_some() && variant.is_none() {
                    panic!(
                        "[prompts] 片段 {} 声明变体 `{}` 但引用未提供值",
                        id,
                        frag.variant.unwrap()
                    );
                }
                let mut body = frag.body.trim().to_string();
                if let Some(val) = variant {
                    body = body.replace("{{VARIANT}}", val);
                }
                out = out.replacen(placeholder, &body, 1);
            }
            None => break,
        }
        depth += 1;
    }
    debug_assert!(
        !out.contains("{{FRAGMENT:"),
        "[prompts] 存在未解析片段引用：\n{}",
        out
    );
    // 2. 块级语义变体（{{VARIANT}}，值由调用方经 PromptCtx.variant 注入）。
    if let Some(v) = &ctx.variant {
        out = out.replace("{{VARIANT}}", v);
    }
    // 3. 数据注入（VAR/SECTION，集合）。
    for (k, v) in &ctx.sections {
        out = out.replace(&format!("{{{{SECTION:{}}}}}", k), v);
    }
    for (k, v) in &ctx.vars {
        out = out.replace(&format!("{{{{VAR:{}}}}}", k), v);
    }
    debug_assert!(
        !out.contains("{{SECTION:") && !out.contains("{{VAR:"),
        "[prompts] 存在未解析占位符：\n{}",
        out
    );
    debug_assert!(
        !out.contains("{{VARIANT}}"),
        "[prompts] 存在未注入的语义变体 {{VARIANT}}：\n{}",
        out
    );
    out
}

/// 查找模板中第一个 `{{FRAGMENT:id|variant}}` 占位符，返回 (完整占位符, id, variant)。
fn find_fragment_placeholder(template: &str) -> Option<(&str, &str, Option<&str>)> {
    let start = template.find("{{FRAGMENT:")?;
    let rest = &template[start + 11..];
    let end = rest.find("}}")?;
    let inner = &rest[..end];
    let placeholder = &template[start..start + 11 + end + 2];
    match inner.split_once('|') {
        Some((id, v)) => Some((placeholder, id, Some(v))),
        None => Some((placeholder, inner, None)),
    }
}

/// 扫描模板中全部 `{{FRAGMENT:id}}`（或带变体）引用的片段 id（用于注册校验）。
fn collect_fragment_refs(template: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for (idx, _) in template.match_indices("{{FRAGMENT:") {
        let rest = &template[idx + 11..];
        if let Some(end) = rest.find("}}") {
            let inner = &rest[..end];
            let id = inner.split('|').next().unwrap_or(inner);
            out.push(id);
        }
    }
    out
}

/// 扫描模板中全部 `{{VAR:x}}` / `{{SECTION:x}}` 的占位符（kind, name）（用于注册校验）。
fn collect_var_section_names(template: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < template.len() {
        if let Some(rel) = template[i..].find("{{") {
            let start = i + rel;
            let rest = &template[start + 2..];
            if let Some(end) = rest.find("}}") {
                let inner = &rest[..end];
                if let Some(name) = inner.strip_prefix("SECTION:") {
                    out.push(("SECTION".to_string(), name.to_string()));
                } else if let Some(name) = inner.strip_prefix("VAR:") {
                    out.push(("VAR".to_string(), name.to_string()));
                }
                i = start + 2 + end + 2;
                continue;
            }
        }
        break;
    }
    out
}

/// 提示词块资产（静态模板 + 元数据）。
pub struct BlockSpec {
    /// 唯一 id（如 "director.clarify_goal"）。
    pub id: &'static str,
    /// 用途说明（审查/去重用）。
    pub usage: &'static str,
    /// 适用业务环节。
    pub flow: &'static str,
    /// 模板（含 `{{VAR:x}}` / `{{SECTION:x}}` / `{{VARIANT}}` 占位符）。
    pub body: &'static str,
    /// 块级语义变体声明（body 中最多 1 个 {{VARIANT}}，渲染时由调用方经 PromptCtx.variant 注入值）。
    pub variant: Option<&'static str>,
}

pub const fn block(id: &'static str, usage: &'static str, flow: &'static str, body: &'static str) -> BlockSpec {
    BlockSpec { id, usage, flow, body, variant: None }
}

pub const fn block_variant(id: &'static str, usage: &'static str, flow: &'static str, body: &'static str, variant: &'static str) -> BlockSpec {
    BlockSpec { id, usage, flow, body, variant: Some(variant) }
}

/// 提示词块注册表（单一真相源）。
pub mod blocks {
    use super::{BlockSpec, block, block_variant};

    pub const DIRECTOR_CLARIFY_GOAL: BlockSpec = block(
        "director.clarify_goal",
        "阶段A 目标确定：理解整理用户目标 + 判定是否需要确认",
        "flow.clarify",
        include_str!("blocks/director_clarify_goal.prompt.md"),
    );

    pub const DIRECTOR_CLARIFY_REPLY: BlockSpec = block(
        "director.clarify_reply",
        "阶段A 确认回复消化：理解回复/材料核验，更新答案与问题项、调整目标方向，判定是否继续追问",
        "flow.clarify",
        include_str!("blocks/director_clarify_reply.prompt.md"),
    );

    pub const DIRECTOR_KICKOFF: BlockSpec = block(
        "director.kickoff",
        "阶段B 任务编排：划分子任务/依赖/指派 + 按需组建参与者",
        "flow.orchestrate",
        include_str!("blocks/director_kickoff.prompt.md"),
    );

    pub const DIRECTOR_REVIEW: BlockSpec = block(
        "director.review",
        "阶段D 评估：更新 L1 摘要 + 判定下一步（discuss/confirm/done）+ completed_tasks 裁决",
        "flow.review",
        include_str!("blocks/director_review.prompt.md"),
    );

    pub const DIRECTOR_NEXT_SPEAKER: BlockSpec = block(
        "director.next_speaker",
        "阶段C 选发言者：判断任务推进 + 派发本轮工作内容（讨论/执行）",
        "flow.discuss",
        include_str!("blocks/director_next_speaker.prompt.md"),
    );

    pub const DIRECTOR_REPLAN: BlockSpec = block(
        "director.replan",
        "阶段B 对账重排：keep/remove/add 操作 + 角色调整 + 按需组建参与者",
        "flow.orchestrate",
        include_str!("blocks/director_replan.prompt.md"),
    );

    pub const DIRECTOR_CONCLUDE: BlockSpec = block(
        "director.conclude",
        "阶段E 最终结论：对照目标输出四部分结论（已完成/未完成/失败/最终结论）",
        "flow.conclude",
        include_str!("blocks/director_conclude.prompt.md"),
    );

    pub const DIRECTOR_CLASSIFY_GOAL_INTENT: BlockSpec = block(
        "director.classify_goal_intent",
        "接收用户消息：判定用户输入对初始目标的意图（六分类）",
        "flow.receive",
        include_str!("blocks/director_classify_goal_intent.prompt.md"),
    );

    pub const DIRECTOR_HANDLE_TASK_FAILURE: BlockSpec = block(
        "director.handle_task_failure",
        "失败处置：retry/reassign/skip/abort/ask_user/self_execute/add_participant + 僵尸移除",
        "flow.execute",
        include_str!("blocks/director_handle_task_failure.prompt.md"),
    );

    pub const DIRECTOR_CLASSIFY_REPLY_INTENT: BlockSpec = block(
        "director.classify_reply_intent",
        "确认回复语义：confirm（答复确认）或 directive（新指令/改方向）",
        "flow.receive",
        include_str!("blocks/director_classify_reply_intent.prompt.md"),
    );

    pub const DIRECTOR_EXTRACT_STANCE: BlockSpec = block(
        "director.extract_stance",
        "立场预处理：从发言提取态度 + ≤100 字立场陈述",
        "flow.discuss",
        include_str!("blocks/director_extract_stance.prompt.md"),
    );

    pub const DIRECTOR_REPLAN_FOR_ROSTER_CHANGE: BlockSpec = block(
        "director.replan_for_roster_change",
        "名册变更后重排：角色连续性 + 未完成任务重派",
        "flow.orchestrate",
        include_str!("blocks/director_replan_for_roster_change.prompt.md"),
    );

    pub const DIRECTOR_PARSE_INPUT_REFS: BlockSpec = block(
        "director.parse_input_refs",
        "语义解析用户输入物引用（锚点枚举 + 路径段，防幻觉）",
        "flow.receive",
        include_str!("blocks/director_parse_input_refs.prompt.md"),
    );

    pub const DIRECTOR_AUTHORIZE_TOOL: BlockSpec = block(
        "director.authorize_tool",
        "工具授权裁决：安全性 + 任务必要性（中高风险放行判定）",
        "flow.execute",
        include_str!("blocks/director_authorize_tool.prompt.md"),
    );

    pub const DIRECTOR_DECIDE_STALL_CONTINUE: BlockSpec = block(
        "director.decide_stall_continue",
        "空转裁决：连续无进展时是否再给推进机会",
        "flow.execute",
        include_str!("blocks/director_decide_stall_continue.prompt.md"),
    );

    pub const DIRECTOR_SYSTEM_PROMPT: BlockSpec = block(
        "director.system_prompt",
        "主持人全局 system：身份/职责分层/五阶段工作流/引用规范",
        "flow.*",
        include_str!("blocks/director_system_prompt.prompt.md"),
    );

    /// 主持人失活恢复决策：由现有 API 参与者（角色不变）输出新主持人配置建议。
    pub const EXECUTOR_RECOVER_DIRECTOR: BlockSpec = block(
        "executor.recover_director",
        "主持人失活恢复：现有 API 参与者决策创建新主持人（角色定义保持不变，非轮流接管）",
        "flow.*",
        include_str!("blocks/executor_recover_director.prompt.md"),
    );

    pub const PARTICIPANT: BlockSpec = block_variant(
        "participant",
        "参与者 system：LLM/CLI 共用（身份/职责分层/引用规范/立场声明/产物规范），工具纪律按参与者模式经 {{VARIANT}} 注入",
        "flow.discuss",
        include_str!("blocks/participant.prompt.md"),
        "participant_mode",
    );

    pub const ROOM_DISCUSSION_TURN: BlockSpec = block(
        "room.discussion_turn",
        "讨论轮强约束：所有指令经主持人 + 本轮不执行（注入参与者 task_context）",
        "flow.discuss",
        include_str!("blocks/room_discussion_turn.prompt.md"),
    );

    /// 全部已注册块（供场景引用校验 / 快照测试遍历）。
    pub const ALL: &[BlockSpec] = &[
        DIRECTOR_CLARIFY_GOAL,
        DIRECTOR_CLARIFY_REPLY,
        DIRECTOR_KICKOFF,
        DIRECTOR_REVIEW,
        DIRECTOR_NEXT_SPEAKER,
        DIRECTOR_REPLAN,
        DIRECTOR_CONCLUDE,
        DIRECTOR_CLASSIFY_GOAL_INTENT,
        DIRECTOR_HANDLE_TASK_FAILURE,
        DIRECTOR_CLASSIFY_REPLY_INTENT,
        DIRECTOR_EXTRACT_STANCE,
        DIRECTOR_REPLAN_FOR_ROSTER_CHANGE,
        DIRECTOR_PARSE_INPUT_REFS,
        DIRECTOR_AUTHORIZE_TOOL,
        DIRECTOR_DECIDE_STALL_CONTINUE,
        DIRECTOR_SYSTEM_PROMPT,
        EXECUTOR_RECOVER_DIRECTOR,
        PARTICIPANT,
        ROOM_DISCUSSION_TURN,
    ];
}

/// 提示词原子片段（v3.4ax）：颗粒度细到"语义原子"，跨场景单一来源。
/// - `body` 可含数据占位符 `{{VAR:x}}`/`{{SECTION:x}}`/`{{FRAGMENT:y}}`（硬替换，集合提供，不限数量）；
/// - `{{VARIANT}}` 为语义变体（body 中最多 1 个，与 `variant` 字段对应），由场景引用时覆写措辞。
pub struct FragmentSpec {
    pub id: &'static str,
    pub usage: &'static str,
    pub body: &'static str,
    pub variant: Option<&'static str>,
}

/// 原子片段注册表（单一真相源）。
pub mod fragments {
    use super::FragmentSpec;

    pub const IDENTITY_DIRECTOR: FragmentSpec = FragmentSpec {
        id: "identity.director",
        usage: "主持人身份句（13 个 director 决策点公共前缀）",
        body: include_str!("blocks/fragments/identity_director.prompt.md"),
        variant: None,
    };

    pub const RULE_STRICT_JSON: FragmentSpec = FragmentSpec {
        id: "rule.strict_json",
        usage: "严格 JSON 输出约束（所有结构化决策点）",
        body: include_str!("blocks/fragments/rule_strict_json.prompt.md"),
        variant: None,
    };

    pub const RULE_GOAL_ANCHOR: FragmentSpec = FragmentSpec {
        id: "rule.goal_anchor",
        usage: "初始目标锚定标题（语义变体：各场景锚定强度措辞不同）",
        body: include_str!("blocks/fragments/rule_goal_anchor.prompt.md"),
        variant: Some("goal_anchor"),
    };

    pub const RULE_SLOT_LIMIT: FragmentSpec = FragmentSpec {
        id: "rule.slot_limit",
        usage: "按需组建参与者的本场空位上限约束",
        body: include_str!("blocks/fragments/rule_slot_limit.prompt.md"),
        variant: None,
    };

    pub const RULE_NEW_PARTICIPANT_SPEC: FragmentSpec = FragmentSpec {
        id: "rule.new_participant_spec",
        usage: "新参与者选型（api/cli）与 id 格式约束（统一措辞标准）",
        body: include_str!("blocks/fragments/rule_new_participant_spec.prompt.md"),
        variant: None,
    };

    pub const RULE_TASK_DESCRIPTION: FragmentSpec = FragmentSpec {
        id: "rule.task_description",
        usage: "任务描述规范：目标+验收+边界+不写死细节+禁止空洞（统一措辞标准）",
        body: include_str!("blocks/fragments/rule_task_description.prompt.md"),
        variant: None,
    };

    pub const RULE_PRIOR_PRODUCT: FragmentSpec = FragmentSpec {
        id: "rule.prior_product",
        usage: "前置产物编号引用约束",
        body: include_str!("blocks/fragments/rule_prior_product.prompt.md"),
        variant: None,
    };

    pub const RULE_INPUT_REF: FragmentSpec = FragmentSpec {
        id: "rule.input_ref",
        usage: "用户输入物定位约束",
        body: include_str!("blocks/fragments/rule_input_ref.prompt.md"),
        variant: None,
    };

    pub const RULE_TOOL_DISCIPLINE_LLM: FragmentSpec = FragmentSpec {
        id: "rule.tool_discipline_llm",
        usage: "API 参与者工具纪律 + ask_user 确认禁则（LLM 模式 {{VARIANT}} 注入值）",
        body: include_str!("blocks/fragments/rule_tool_discipline_llm.prompt.md"),
        variant: None,
    };

    pub const RULE_TOOL_DISCIPLINE_CLI: FragmentSpec = FragmentSpec {
        id: "rule.tool_discipline_cli",
        usage: "CLI 参与者工具能力声明：无法调用内置工具（CLI 模式 {{VARIANT}} 注入值）",
        body: include_str!("blocks/fragments/rule_tool_discipline_cli.prompt.md"),
        variant: None,
    };

    pub const RULE_CATALOG_HEADING: FragmentSpec = FragmentSpec {
        id: "rule.catalog_heading",
        usage: "【可用模型/CLI Agent 清单】段标题（kickoff/replan/失败处置共用，逐字复制禁则）",
        body: include_str!("blocks/fragments/rule_catalog_heading.prompt.md"),
        variant: None,
    };

    pub const RULE_FAILURES_HEADING: FragmentSpec = FragmentSpec {
        id: "rule.failures_heading",
        usage: "【参与者近期失败记录】段标题（kickoff/replan/选人共用，统一措辞）",
        body: include_str!("blocks/fragments/rule_failures_heading.prompt.md"),
        variant: None,
    };

    pub const RULE_MINIMAL_ROSTER: FragmentSpec = FragmentSpec {
        id: "rule.minimal_roster",
        usage: "按需组建参与者最小化原则（能不加就不加，人数越少意见越容易收敛）",
        body: include_str!("blocks/fragments/rule_minimal_roster.prompt.md"),
        variant: None,
    };

    /// 全部已注册片段（供引用校验 / 无孤立片段检测 / 渲染遍历）。
    pub const ALL: &[FragmentSpec] = &[
        IDENTITY_DIRECTOR,
        RULE_STRICT_JSON,
        RULE_GOAL_ANCHOR,
        RULE_SLOT_LIMIT,
        RULE_NEW_PARTICIPANT_SPEC,
        RULE_TASK_DESCRIPTION,
        RULE_PRIOR_PRODUCT,
        RULE_INPUT_REF,
        RULE_TOOL_DISCIPLINE_LLM,
        RULE_TOOL_DISCIPLINE_CLI,
        RULE_CATALOG_HEADING,
        RULE_FAILURES_HEADING,
        RULE_MINIMAL_ROSTER,
    ];

    /// 仅供运行时（Rust 调用方）引用的片段：不出现在任何块模板的 `{{FRAGMENT:}}` 占位中，
    /// 由代码作为块级语义变体值（PromptCtx.variant）注入，如参与者工具纪律按 LLM/CLI 模式选择。
    pub const RUNTIME_REFERENCED: &[&str] = &[
        "rule.tool_discipline_llm",
        "rule.tool_discipline_cli",
    ];
}

/// 场景（环节内的一次 LLM 调用）声明：输入数据段/记忆层 + 引用块。
pub struct SceneSpec {
    pub id: &'static str,
    pub flow: &'static str,
    /// 该场景需要的输入数据段（校验注入完整性）。
    pub input_sections: &'static [&'static str],
    /// 该场景引用的提示词块 id。
    pub blocks: &'static [&'static str],
}

/// 业务环节（主干，可读性）。
pub struct FlowSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub scenes: &'static [&'static str],
}

/// 场景注册表（全部已定义的 LLM 调用点）。
pub mod scene_registry {
    use super::scenes::*;
    use super::SceneSpec;

    pub const ALL_SCENES: &[&SceneSpec] = &[
        &CLARIFY_GOAL,
        &KICKOFF,
        &REPLAN,
        &REPLAN_FOR_ROSTER_CHANGE,
        &NEXT_SPEAKER,
        &REVIEW,
        &CONCLUDE,
        &CLASSIFY_GOAL_INTENT,
        &CLASSIFY_REPLY_INTENT,
        &EXTRACT_STANCE,
        &PARSE_INPUT_REFS,
        &HANDLE_TASK_FAILURE,
        &AUTHORIZE_TOOL,
        &DECIDE_STALL_CONTINUE,
        &PARTICIPANT_LLM,
        &PARTICIPANT_CLI,
        &ROOM_DISCUSSION_TURN,
    ];
}

/// 注册表自检：块 id 唯一、场景引用的块已注册、场景声明/环节引用的数据段与环节均已注册。
/// 供单元测试与运行时（接入前由测试驱动）保证提示词资产与代码一致。
#[allow(dead_code)]
pub fn validate() -> Result<(), String> {
    let known_sections: HashMap<&str, ()> = sections::SECTIONS.iter().map(|s| (s.id, ())).collect();
    let flows: HashMap<&str, ()> = flows::FLOWS.iter().map(|f| (f.id, ())).collect();
    let block_ids: HashMap<&str, ()> = blocks::ALL.iter().map(|b| (b.id, ())).collect();

    // 1. 块 id 唯一。
    if block_ids.len() != blocks::ALL.len() {
        return Err("提示词块 id 存在重复".into());
    }
    // 1.1 数据段描述完整性（source/usage 必填）。
    for s in sections::SECTIONS {
        if s.source.is_empty() || s.usage.is_empty() {
            return Err(format!("数据段 {} 缺少 source/usage 描述", s.id));
        }
    }
    // 1.2 块环节归属校验（"flow.*" 表示全环节通用）+ 用途描述完整性 + 块级变体一致性。
    for b in blocks::ALL {
        if b.usage.is_empty() {
            return Err(format!("块 {} 缺少用途说明", b.id));
        }
        if b.flow != "flow.*" && !flows.contains_key(b.flow) {
            return Err(format!("块 {} 的环节 {} 未注册", b.id, b.flow));
        }
        let variant_count = b.body.matches("{{VARIANT}}").count();
        if variant_count > 1 {
            return Err(format!("块 {} 含 {} 个 {{VARIANT}}（最多 1 个，请拆分）", b.id, variant_count));
        }
        if b.variant.is_some() && variant_count != 1 {
            return Err(format!("块 {} 声明变体但 body 无 {{VARIANT}} 占位", b.id));
        }
        if b.variant.is_none() && variant_count != 0 {
            return Err(format!("块 {} body 含 {{VARIANT}} 但未声明 variant 字段", b.id));
        }
    }
    // 1.3 片段自检：id 唯一、{{VARIANT}}≤1 且与 variant 字段一致、数据占位符已注册。
    let mut fragment_ids: HashMap<&str, ()> = HashMap::new();
    for f in fragments::ALL {
        if fragment_ids.insert(f.id, ()).is_some() {
            return Err(format!("片段 id 重复: {}", f.id));
        }
        if f.usage.is_empty() {
            return Err(format!("片段 {} 缺少用途说明", f.id));
        }
        let variant_count = f.body.matches("{{VARIANT}}").count();
        if variant_count > 1 {
            return Err(format!("片段 {} 含 {} 个 {{VARIANT}}（最多 1 个，请拆分）", f.id, variant_count));
        }
        if f.variant.is_some() && variant_count != 1 {
            return Err(format!("片段 {} 声明变体但 body 无 {{VARIANT}} 占位", f.id));
        }
        if f.variant.is_none() && variant_count != 0 {
            return Err(format!("片段 {} body 含 {{VARIANT}} 但未声明 variant 字段", f.id));
        }
        for (kind, name) in collect_var_section_names(f.body) {
            // 仅 SECTION 数据段需注册；VAR 单值由调用方提供。
            if kind == "SECTION" && !known_sections.contains_key(name.as_str()) {
                return Err(format!("片段 {} 引用了未注册数据段: {}", f.id, name));
            }
        }
    }
    // 1.4 块引用的片段存在 + 无孤立片段（未被任何块引用）。
    let mut referenced_fragments: HashMap<&str, ()> = HashMap::new();
    for b in blocks::ALL {
        for fid in collect_fragment_refs(b.body) {
            if !fragment_ids.contains_key(fid) {
                return Err(format!("块 {} 引用了未注册片段: {}", b.id, fid));
            }
            referenced_fragments.insert(fid, ());
        }
    }
    for f in fragments::ALL {
        // 运行时引用片段（RUNTIME_REFERENCED）由 Rust 调用方作为变体值注入，不在块模板占位中，跳过孤立检测。
        if !referenced_fragments.contains_key(f.id) && !fragments::RUNTIME_REFERENCED.contains(&f.id) {
            return Err(format!("片段 {} 未被任何块引用（孤立冗余）", f.id));
        }
    }
    // 2. 场景自检。
    let mut scene_ids: HashMap<&str, ()> = HashMap::new();
    for s in scene_registry::ALL_SCENES {
        if scene_ids.insert(s.id, ()).is_some() {
            return Err(format!("场景 id 重复: {}", s.id));
        }
        if !flows.contains_key(s.flow) {
            return Err(format!("场景 {} 的环节 {} 未注册", s.id, s.flow));
        }
        for bid in s.blocks {
            if !block_ids.contains_key(bid) {
                return Err(format!("场景 {} 引用了未注册块: {}", s.id, bid));
            }
        }
        for sec in s.input_sections {
            if !known_sections.contains_key(sec) {
                return Err(format!("场景 {} 声明了未注册数据段: {}", s.id, sec));
            }
        }
    }
    // 3. 环节引用的场景均存在。
    for f in flows::FLOWS {
        if f.name.is_empty() {
            return Err(format!("环节 {} 缺少名称", f.id));
        }
        for sid in f.scenes {
            if !scene_ids.contains_key(sid) {
                return Err(format!("环节 {} 引用了未注册场景: {}", f.id, sid));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注册表自检：块/场景/数据段/环节引用一致。
    #[test]
    fn registry_is_consistent() {
        validate().expect("提示词资产注册表校验失败");
    }

    /// 扫描模板中的占位符（{{SECTION:x}} / {{VAR:x}}），并递归进被引用的片段 body
    /// （{{FRAGMENT:id}} 引用的原子内占位符同样需要注入假值）。{{VARIANT}} 由引用时提供，不收集。
    fn collect_placeholders(body: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut stack: Vec<String> = vec![body.to_string()];
        let mut seen_fragments: std::collections::HashSet<String> = std::collections::HashSet::new();
        while let Some(text) = stack.pop() {
            // match_indices 保证多字节字符（中文）边界安全。
            for (idx, _) in text.match_indices("{{") {
                let rest = &text[idx + 2..];
                if let Some(end) = rest.find("}}") {
                    let inner = &rest[..end];
                    if let Some(name) = inner.strip_prefix("SECTION:") {
                        out.push(("SECTION".to_string(), name.to_string()));
                    } else if let Some(name) = inner.strip_prefix("VAR:") {
                        out.push(("VAR".to_string(), name.to_string()));
                    } else if let Some(id) = inner.strip_prefix("FRAGMENT:") {
                        let fid = id.split('|').next().unwrap_or(id);
                        if seen_fragments.insert(fid.to_string()) {
                            if let Some(f) = fragments::ALL.iter().find(|f| f.id == fid) {
                                stack.push(f.body.to_string());
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// 每个块用假值渲染后无未解析占位符（防模板新增占位符但调用方未注入）。
    #[test]
    fn all_blocks_render_complete() {
        validate().unwrap();
        for b in blocks::ALL {
            let ph = collect_placeholders(b.body);
            let mut ctx = PromptCtx::new();
            for (kind, name) in ph {
                let key: &'static str = Box::leak(name.into_boxed_str());
                match kind.as_str() {
                    "SECTION" => {
                        ctx.sections.insert(key, format!("<SECTION {}>", key));
                    }
                    _ => {
                        ctx.vars.insert(key, format!("<VAR {}>", key));
                    }
                }
            }
            // 块级语义变体：声明了变体的块注入假值。
            if let Some(v) = b.variant {
                ctx.variant = Some(format!("<VARIANT {}>", v));
            }
            let rendered = render(b.body, &ctx);
            assert!(
                !rendered.contains("{{SECTION:") && !rendered.contains("{{VAR:") && !rendered.contains("{{VARIANT}}"),
                "块 {} 存在未解析占位符",
                b.id
            );
        }
    }

    /// 快照：关键决策点渲染结构与关键规则文本存在性（提示词回归护栏）。
    #[test]
    fn key_blocks_snapshot() {
        // kickoff 必须包含任务拆分强制与阶段B 定位。
        let kickoff = render(
            blocks::DIRECTOR_KICKOFF.body,
            &PromptCtx::new()
                .section("topic", "<topic>")
                .section("notes", "<notes>")
                .section("latest", "<latest>")
                .section("roster", "<roster>")
                .section("catalog", "<catalog>")
                .section("failures", "<failures>")
                .var("max_new_participants", "2"),
        );
        assert!(kickoff.contains("阶段B（任务编排）"));
        assert!(kickoff.contains("非单原子目标必须拆分为多个可独立完成的子任务"));
        assert!(kickoff.contains("禁止将用户指令原样作为单一任务"));
        assert!(kickoff.contains("T{i+1}"));

        // 讨论轮约束必须含"不执行"强约束。
        let discuss = render(
            blocks::ROOM_DISCUSSION_TURN.body,
            &PromptCtx::new().section("work_content", "<wc>"),
        );
        assert!(discuss.contains("讨论轮"));
        assert!(discuss.contains("不得执行任务"));

        // 参与者块按模式注入工具纪律：LLM 模式含工具纪律与 ask_user 禁则，CLI 模式含无内置工具声明且无工具纪律。
        let render_participant = |discipline: &str| {
            render(
                blocks::PARTICIPANT.body,
                &PromptCtx::new()
                    .var("role", "<role>")
                    .var("own_id", "<own>")
                    .var("director_id", "<dir>")
                    .section("topic", "<t>")
                    .section("roster", "<r>")
                    .section("summary", "<s>")
                    .section("stances", "<st>")
                    .section("task_context", "<tc>")
                    .section("product_norm", "<pn>")
                    .section("output_dir", "<od>")
                    .variant(discipline),
            )
        };
        let llm_p = render_participant(fragments::RULE_TOOL_DISCIPLINE_LLM.body.trim());
        assert!(llm_p.contains("【工具使用纪律】"));
        assert!(llm_p.contains("不要调用 ask_user 工具"));
        assert!(llm_p.contains("【职责分层】"));
        let cli_p = render_participant(fragments::RULE_TOOL_DISCIPLINE_CLI.body.trim());
        assert!(cli_p.contains("无法调用本群聊的内置工具"));
        assert!(!cli_p.contains("【工具使用纪律】"));
        assert!(cli_p.contains("【职责分层】"));
    }
}
