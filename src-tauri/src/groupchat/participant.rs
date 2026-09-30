//! 参与者抽象层（framework 层，provider/agent 无关）
//!
//! 对外只暴露三个 trait：`LlmClient` / `CliRunner` / `Participant`。
//! 二次开发者通常只需实现前两者之一即可接入自有 Agent。

use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

use super::models::{summarize_confirmation_title, ConfirmationItem, ConfirmationRequest};

/// 流式增量回调：框架把每个增量片段实时推给前端 `token_stream`。
pub type DeltaFn = dyn Fn(&str) + Send + Sync;

/// 框架层 Chat 消息（群聊语义：`name` 标识发言者，单会话可省略）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: String,
    /// 多模态图片（base64 data URL 或 http(s) URL），仅用于 user 消息。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,
    /// 思考模式（DeepSeek 等）：assistant 消息携带上轮的 reasoning_content，随请求原样回传。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
}

impl ChatMessage {
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".into(),
            name: None,
            content: content.into(),
            images: None,
            reasoning_content: None,
        }
    }
    #[allow(dead_code)]
    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".into(),
            name: None,
            content: content.into(),
            images: None,
            reasoning_content: None,
        }
    }
    /// assistant 消息携带思考链（reasoning_content），供思考模式模型原样回传。
    #[allow(dead_code)]
    pub fn assistant_with_reasoning(content: &str, reasoning: &str) -> Self {
        Self {
            role: "assistant".into(),
            name: None,
            content: content.into(),
            images: None,
            reasoning_content: if reasoning.is_empty() {
                None
            } else {
                Some(reasoning.to_string())
            },
        }
    }
    #[allow(dead_code)]
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".into(),
            name: None,
            content: content.into(),
            images: None,
            reasoning_content: None,
        }
    }
    #[allow(dead_code)]
    pub fn named(role: &str, name: &str, content: &str) -> Self {
        Self {
            role: role.into(),
            name: Some(name.into()),
            content: content.into(),
            images: None,
            reasoning_content: None,
        }
    }
}

/// 立场态度（由立场文本派生，与前端快照图标/聚合统计共用同一规则）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Attitude {
    #[default]
    Neutral,
    Agree,
    Disagree,
}

impl Attitude {
    pub fn as_str(&self) -> &'static str {
        match self {
            Attitude::Agree => "agree",
            Attitude::Disagree => "disagree",
            Attitude::Neutral => "neutral",
        }
    }

    /// 从 DB/事件字符串解析（未知值归为 Neutral）。
    pub fn parse(s: &str) -> Attitude {
        match s {
            "agree" => Attitude::Agree,
            "disagree" => Attitude::Disagree,
            _ => Attitude::Neutral,
        }
    }
}

/// 依据关键词分类立场态度（否定感知：关键词前 1-2 字符含 不/没/未 时态度反转；agree 优先判定）。
pub fn classify_attitude(text: &str) -> Attitude {
    if let Some(a) = match_attitude(
        text,
        &["支持", "同意", "赞成"],
        Attitude::Agree,
        Attitude::Disagree,
    ) {
        return a;
    }
    match_attitude(
        text,
        &["反对", "质疑", "拒绝"],
        Attitude::Disagree,
        Attitude::Agree,
    )
    .unwrap_or(Attitude::Neutral)
}

/// 扫描一组关键词：全部命中均带否定时返回 `negative`，存在无否定命中时返回 `positive`，未命中返回 None。
fn match_attitude(
    text: &str,
    keywords: &[&str],
    positive: Attitude,
    negative: Attitude,
) -> Option<Attitude> {
    let mut found = false;
    let mut all_negated = true;
    for kw in keywords {
        let mut from = 0;
        while let Some(rel) = text[from..].find(kw) {
            found = true;
            let abs = from + rel;
            if !is_negated_before(text, abs) {
                all_negated = false;
            }
            from = abs + kw.len();
        }
    }
    if !found {
        return None;
    }
    Some(if all_negated { negative } else { positive })
}

/// 判断关键词命中位置（字节偏移，位于字符边界）前 1-2 个字符是否含否定词。
fn is_negated_before(text: &str, pos: usize) -> bool {
    text[..pos]
        .chars()
        .rev()
        .take(2)
        .any(|c| matches!(c, '不' | '没' | '未'))
}

/// 解析立场态度：优先读取发言开头的严格立场声明（「立场：支持/反对/中立」，支持【】、[]、全/半角冒号），
/// 未声明时回退到否定感知的关键词分类。
pub fn resolve_attitude(text: &str) -> Attitude {
    let t = text.trim_start();
    let mut head = t;
    for open in ["【", "["] {
        if let Some(r) = head.strip_prefix(open) {
            head = r;
            break;
        }
    }
    if let Some(rest) = head.strip_prefix("立场") {
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix('：')
            .or_else(|| rest.strip_prefix(':'))
            .map(str::trim_start);
        if let Some(rest) = rest {
            let attitude = match rest.chars().next() {
                Some('支') => Some(Attitude::Agree),    // 支持
                Some('反') => Some(Attitude::Disagree), // 反对
                Some('中') => Some(Attitude::Neutral),  // 中立
                _ => None,
            };
            if let Some(a) = attitude {
                return a;
            }
        }
    }
    classify_attitude(t)
}

/// 立场快照（L2 收敛裁决依据）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stance {
    pub participant_id: String,
    pub stance: String,
    /// 立场态度（由 stance 文本派生）。
    #[serde(default)]
    pub attitude: Attitude,
}

/// 一次发言的输入视图（memory.rs 组装，即 L3 定向视图）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnView {
    /// 当前议程/目标
    pub topic: String,
    /// L1 全局摘要
    pub summary: String,
    /// L2 立场快照
    pub stances: Vec<Stance>,
    /// L3 定向消息（已按 recipients 过滤；name 保留发言者）
    pub messages: Vec<ChatMessage>,
    /// 角色设定
    pub system_role: String,
    /// 讨论阶段注入：当前发言者被指派的任务上下文（执行阶段为空串）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub task_context: String,
    /// 参与者名册（id、显示名、角色）：LLM 据此刻画 id ↔ 显示名 ↔ 角色映射，
    /// 正文引用他人一律使用唯一 id（前端再映射为显示名）。空时按无名册处理。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roster: Vec<(String, String, String)>,
    /// 用户主导语言（中文/英文）：产物命名语言约束。空串表示未判定，不注入约束。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_language: String,
    /// 房间统一产物目录（v3.4ao，绝对路径）：参与者产出的文件必须写入该目录。
    /// 空串表示未约定，不注入位置约束。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output_dir: String,
}

/// 一次发言的输出（`content` 落库为 `groupchat_messages.content`）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnResult {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// 本轮工具调用链（JSON 数组字符串，与 `groupchat_messages.tool_calls` 对齐；无工具调用为空串）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_calls: String,
    /// 思考模式（DeepSeek 等）的 reasoning_content；随 assistant 消息落库，下一轮回传。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning_content: String,
    /// 失败原因；有值时表示本轮执行失败（content 通常为空）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 参与者运行中主动发起的用户确认请求；有值表示本轮需要用户确认而非普通发言/结果。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<ConfirmationRequest>,
}

/// CLI Agent 配置。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CliConfig {
    pub command: String,
    /// 首次启动参数（按空白切分）
    pub args_template: String,
    /// 续接参数模板，如 `--resume {session_id}`
    pub resume_arg_template: String,
}

/// CLI 一次执行输出。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CliOutput {
    pub stdout: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// LLM 调用抽象：Director 与 LLM 参与者共用，provider 无关。
#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<String, String>;

    /// 返回 `(回复内容, 工具调用链 JSON, 思考链 reasoning_content)`。
    /// 默认仅调用 `complete` 并置空工具调用链与思考链；
    /// 支持工具的适配器（如 `PilotDeskLlmClient`）可覆写以透出本轮工具调用过程与思考链。
    /// `on_progress` 为"实质产出进展"心跳（每完成一轮有产出的迭代触发一次），与
    /// `Participant::run_turn` 的 `on_progress` 对齐；无进展感知需求时传 None。
    async fn complete_with_tool_calls(
        &self,
        system: &str,
        messages: &[ChatMessage],
        on_delta: Option<Arc<DeltaFn>>,
        _on_progress: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(String, String, String), String> {
        let content = self.complete(system, messages, on_delta).await?;
        Ok((content, String::new(), String::new()))
    }
}

/// CLI 执行抽象：外部 CLI Agent 的通用执行入口。
#[async_trait::async_trait]
pub trait CliRunner: Send + Sync {
    async fn run(
        &self,
        config: &CliConfig,
        prompt: &str,
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<CliOutput, String>;
}

/// 参与者抽象（核心框架唯一耦合点）。
#[async_trait::async_trait]
pub trait Participant: Send + Sync {
    #[allow(dead_code)]
    fn id(&self) -> &str;
    /// `on_progress`：任务级"进展心跳"（每完成一轮有实质产出的迭代触发一次），供上层
    /// 停滞秒表重置——区分"长任务正常推进"与"任务停滞"（有产出不误杀）。参数随 future
    /// 传递，天然适配同一参与者被多个并行任务共用；讨论/侦查等无需进展感知的场景传 None。
    async fn run_turn(
        &self,
        view: TurnView,
        on_delta: Option<Arc<DeltaFn>>,
        on_progress: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> TurnResult;
    #[allow(dead_code)]
    async fn cancel(&self) {}
    /// 运行时更新角色设定（Director 开场指派后同步，默认无操作）。
    async fn set_role(&self, _role: &str) {}
}

/// API 参与者：无状态，每轮由 L3 视图注入上下文。
pub struct LlmParticipant {
    pub id: String,
    pub llm: Arc<dyn LlmClient>,
    pub role_prompt: Arc<RwLock<String>>,
    /// 主持人（Director）的参与者 id，供自我身份感知与建议前缀（【建议[@主持人ID]】）。
    pub director_id: String,
    /// 群聊作用域被禁技能名（构造时从 app_settings 读取的快照）。
    /// 只决定 `<available_skills>` 目录是否向模型披露；load_skill 仍可点名加载被隐藏技能。
    pub disabled_skills: Vec<String>,
}

#[async_trait::async_trait]
impl Participant for LlmParticipant {
    fn id(&self) -> &str {
        &self.id
    }

    async fn set_role(&self, role: &str) {
        if let Ok(mut w) = self.role_prompt.write() {
            *w = role.to_string();
        }
    }

    async fn run_turn(
        &self,
        view: TurnView,
        on_delta: Option<Arc<DeltaFn>>,
        on_progress: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> TurnResult {
        let role = self
            .role_prompt
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let system = build_llm_system_prompt(
            &view,
            &role,
            &self.id,
            &self.director_id,
            &self.disabled_skills,
        );
        match self
            .llm
            .complete_with_tool_calls(&system, &view.messages, on_delta, on_progress)
            .await
        {
            Ok((content, tool_calls, reasoning_content)) => {
                // 参与者运行中主动发起确认：若输出为确认请求 JSON，则转确认而非普通发言。
                if let Some(confirmation) = parse_confirmation_request(&content) {
                    TurnResult {
                        content: String::new(),
                        tool_calls: String::new(),
                        reasoning_content: String::new(),
                        metadata: None,
                        error: None,
                        confirmation: Some(confirmation),
                    }
                } else {
                    TurnResult {
                        content,
                        tool_calls,
                        reasoning_content,
                        metadata: None,
                        error: None,
                        confirmation: None,
                    }
                }
            }
            Err(e) => TurnResult {
                content: String::new(),
                tool_calls: String::new(),
                reasoning_content: String::new(),
                metadata: None,
                error: Some(format!("[{} 发言失败] {}", self.id, e)),
                confirmation: None,
            },
        }
    }
}

/// CLI 参与者：跨轮用 `--resume` 持续会话（session_id 由 CliRunner 内部管理）。
pub struct CliParticipant {
    pub id: String,
    pub runner: Arc<dyn CliRunner>,
    pub config: CliConfig,
    /// 角色设定（与 LlmParticipant 一致，Director 开场指派后可同步更新）。
    pub role_prompt: Arc<RwLock<String>>,
    /// 主持人（Director）的参与者 id，供身份感知与建议前缀（【建议[@主持人ID]】）。
    pub director_id: String,
}

#[async_trait::async_trait]
impl Participant for CliParticipant {
    fn id(&self) -> &str {
        &self.id
    }

    async fn set_role(&self, role: &str) {
        if let Ok(mut w) = self.role_prompt.write() {
            *w = role.to_string();
        }
    }

    async fn run_turn(
        &self,
        view: TurnView,
        on_delta: Option<Arc<DeltaFn>>,
        _on_progress: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> TurnResult {
        let role = self
            .role_prompt
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let prompt = build_cli_prompt(&view, &role, &self.id, &self.director_id);
        match self.runner.run(&self.config, &prompt, on_delta).await {
            Ok(out) => TurnResult {
                content: out.stdout,
                tool_calls: String::new(),
                reasoning_content: String::new(),
                metadata: None,
                error: None,
                confirmation: None,
            },
            Err(e) => TurnResult {
                content: String::new(),
                tool_calls: String::new(),
                reasoning_content: String::new(),
                metadata: None,
                error: Some(format!("[{} 执行失败] {}", self.id, e)),
                confirmation: None,
            },
        }
    }
}

/// 用户参与者：不自动发言，用户指令由 RoomRuntime 直接注入并广播。
pub struct UserParticipant {
    #[allow(dead_code)]
    pub id: String,
}

#[async_trait::async_trait]
impl Participant for UserParticipant {
    fn id(&self) -> &str {
        &self.id
    }

    async fn run_turn(
        &self,
        _view: TurnView,
        _on_delta: Option<Arc<DeltaFn>>,
        _on_progress: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> TurnResult {
        TurnResult {
            content: String::new(),
            tool_calls: String::new(),
            reasoning_content: String::new(),
            metadata: None,
            error: None,
            confirmation: None,
        }
    }
}

/// 参与者 system prompt 的公共数据段（名册/摘要/立场/任务/产物规范/统一产物目录），
/// LLM 与 CLI 双版本共用同一构建逻辑（v3.4aw 去重：消除双份维护）。
struct ParticipantSections {
    roster: String,
    summary: String,
    stances: String,
    task_context: String,
    product_norm: String,
    output_dir: String,
}

fn build_participant_sections(view: &TurnView) -> ParticipantSections {
    let roster = if view.roster.is_empty() {
        String::new()
    } else {
        let mut s = String::from("\n[参与者名册]\n");
        for (id, _name, role) in &view.roster {
            let role = if role.trim().is_empty() {
                "未分配"
            } else {
                role.as_str()
            };
            s.push_str(&format!("- [@{}]（角色：{}）\n", id, role));
        }
        s
    };
    let summary = if view.summary.is_empty() {
        String::new()
    } else {
        format!(
            "\n[主持人进度摘要]（由主持人维护的进度总结，**不是用户发言或新指令**；用户新指令只会作为新消息出现）\n{}\n",
            view.summary
        )
    };
    let stances = if view.stances.is_empty() {
        String::new()
    } else {
        let mut s = String::from("\n[各参与者当前立场]\n");
        for st in &view.stances {
            let tag = match st.attitude {
                Attitude::Agree => "支持",
                Attitude::Disagree => "反对",
                Attitude::Neutral => "中立",
            };
            s.push_str(&format!(
                "- {}: [{}] {}\n",
                st.participant_id, tag, st.stance
            ));
        }
        s
    };
    let task_context = if view.task_context.is_empty() {
        String::new()
    } else {
        format!("\n[你负责的任务]\n{}\n", view.task_context)
    };
    let product_norm = if view.user_language.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n【产物规范】本群聊用户使用{}交流：你生成/落盘的产物（文档、图片、脚本、配置文件等文件名）\
             优先使用该语言命名；提及产物文件时给出**绝对路径**（如 E:\\PilotDesk\\Foo.tsx），\
             方便用户直接点击打开；若以表格形式统计展示多个产物（如列：文件名/大小/状态），可保持简洁不附完整路径；\
             代码/工程文件（如 main.py、package.json、src/ 目录结构等）命名遵循技术规范，不强行替换。",
            view.user_language
        )
    };
    let output_dir = if view.output_dir.is_empty() {
        String::new()
    } else {
        format!(
            "\n【产物目录】本房间所有参与者产出的文件必须统一写入目录：`{}`（绝对路径）。\
             该目录由系统创建好，写文件时缺失的上级目录会自动创建，**无需**用 mkdir 创建\
             （尤其不要写 Unix 风格的 `mkdir -p`：本机是 cmd.exe，会把 `-p` 当成目录名，\
             在工作目录里多出一个 `-p` 目录）；\
             禁止随意选择其他位置（桌面、工作区根目录等）；\
             报告产物时仍按【产物规范】给出该目录下的绝对路径。",
            view.output_dir
        )
    };
    ParticipantSections {
        roster,
        summary,
        stances,
        task_context,
        product_norm,
        output_dir,
    }
}

/// 组装 API 参与者的 system prompt（身份 + 角色 + 议题 + 名册 + 摘要 + 立场）。
/// v3.4ax 提示词资产化：LLM/CLI 共用 blocks/participant.prompt.md，工具纪律按模式经语义变体注入。
/// 追加技能目录 `<available_skills>` 块（与单 Agent 会话一致：Progressive Disclosure，
/// 仅列 name+description，完整内容经 load_skill 按需加载）。`disabled_skills` 为群聊作用域
/// 被禁技能名，只从目录块中隐藏（load_skill 仍可点名加载）。
fn build_llm_system_prompt(
    view: &TurnView,
    role_prompt: &str,
    own_id: &str,
    director_id: &str,
    disabled_skills: &[String],
) -> String {
    let base = build_participant_prompt(
        view,
        role_prompt,
        own_id,
        director_id,
        super::prompts::fragments::RULE_TOOL_DISCIPLINE_LLM
            .body
            .trim(),
    );
    match available_skills_block(disabled_skills) {
        Some(block) => format!("{}\n\n{}", base, block),
        None => base,
    }
}

/// 扫描全局 API Agent 技能目录（与群聊 load_skill 工具注册同源：均取
/// `crate::get_api_agent_skills_dir()`），按 `disabled_skills` 过滤后渲染
/// `<available_skills>` 注入块。禁用集只隐藏目录披露，不改任何工具注册表。
/// 目录不存在/扫描为空/全部技能被禁时静默返回 None（无可见技能则无块），不 panic。
fn available_skills_block(disabled_skills: &[String]) -> Option<String> {
    let loader = crate::api_agent::skills::SkillLoader::new(crate::get_api_agent_skills_dir());
    let visible: Vec<_> = loader
        .list_skills()
        .into_iter()
        .filter(|e| !disabled_skills.contains(&e.name))
        .collect();
    render_skills_block(&visible)
}

/// 渲染技能目录块（Progressive Disclosure：仅 name+description，与单 Agent 会话同构）。
/// `skills` 为空时返回 None；description 为空时省略 `: ` 后缀，避免悬空冒号。
fn render_skills_block(skills: &[crate::api_agent::system_prompt::SkillEntry]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let mut block = String::from("<available_skills>\n");
    block.push_str("以下是可用的技能列表。调用 load_skill 工具加载完整技能内容：\n\n");
    for s in skills {
        if s.description.is_empty() {
            block.push_str(&format!("- **{}**\n", s.name));
        } else {
            block.push_str(&format!("- **{}**: {}\n", s.name, s.description));
        }
    }
    block.push_str("</available_skills>");
    Some(block)
}

/// 组装 CLI 参与者的 prompt（与 LLM 共用模板，仅工具纪律变体不同：CLI 无内置工具）。
/// v3.4ax 提示词资产化：模板位于 prompts/blocks/participant.prompt.md。
fn build_cli_prompt(view: &TurnView, role_prompt: &str, own_id: &str, director_id: &str) -> String {
    build_participant_prompt(
        view,
        role_prompt,
        own_id,
        director_id,
        super::prompts::fragments::RULE_TOOL_DISCIPLINE_CLI
            .body
            .trim(),
    )
}

/// 参与者提示词公共组装：共用模板 + 按模式注入工具纪律变体（{{VARIANT}}）。
fn build_participant_prompt(
    view: &TurnView,
    role_prompt: &str,
    own_id: &str,
    director_id: &str,
    discipline: &str,
) -> String {
    let s = build_participant_sections(view);
    super::prompts::render(
        super::prompts::blocks::PARTICIPANT.body,
        &super::prompts::PromptCtx::new()
            .var(
                "role",
                if role_prompt.is_empty() {
                    "参与者"
                } else {
                    role_prompt
                },
            )
            .var("own_id", own_id)
            .var("director_id", director_id)
            .section("topic", &view.topic)
            .section("roster", s.roster)
            .section("summary", s.summary)
            .section("stances", s.stances)
            .section("task_context", s.task_context)
            .section("product_norm", s.product_norm)
            .section("output_dir", s.output_dir)
            .variant(discipline),
    )
}

/// 从 LLM 输出中提取 JSON 对象：优先整段解析，失败则截取确认 JSON（`{"askUser"` 到末 `}`）子串再解析。
fn extract_json(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();
    // 去掉可能的 markdown 代码块围栏
    let cleaned = {
        let mut s = trimmed;
        if let Some(x) = s.strip_prefix("```json") {
            s = x;
        } else if let Some(x) = s.strip_prefix("```") {
            s = x;
        }
        if let Some(x) = s.strip_suffix("```") {
            s = x;
        }
        s.trim()
    };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(cleaned) {
        return Some(v);
    }
    // 混合文本（自然语言 + JSON）：定位确认 JSON 起点，截取到最后一个 "}"。
    let start = cleaned.find("{\"askUser\"").or_else(|| cleaned.find('{'))?;
    let end = cleaned.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&cleaned[start..=end]).ok()
}

/// 解析 LLM 输出中的确认请求 JSON；非确认输出返回 `None`。
/// 确认约定：`{"askUser":true,"prompt":"...","replyMode":"open|structured","items":[...]}`。
/// task_id 由调用方（执行阶段）回填；讨论阶段发起时保持空串表示无关联任务。
fn parse_confirmation_request(raw: &str) -> Option<ConfirmationRequest> {
    let v: serde_json::Value = extract_json(raw)?;
    if v["askUser"].as_bool() != Some(true) {
        return None;
    }
    let prompt = v["prompt"].as_str().unwrap_or("").trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    let reply_mode = match v["replyMode"].as_str().unwrap_or("open") {
        "structured" => "structured".to_string(),
        _ => "open".to_string(),
    };
    // 标题优先用 LLM 生成的摘要（≤22 字），缺失时从 prompt 首行截取兜底。
    let title = v["title"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| summarize_confirmation_title(&s))
        .unwrap_or_else(|| summarize_confirmation_title(&prompt));
    let items = if reply_mode == "structured" {
        v["items"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|it| ConfirmationItem::from_json(it, "inputType"))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(ConfirmationRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        task_id: String::new(),
        title,
        prompt,
        reply_mode,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_agent::system_prompt::SkillEntry;

    /// 无技能（目录不存在/未配置/为空）时技能块静默返回 None：与单会话一致——无技能则无块。
    #[test]
    fn test_skills_block_none_when_no_skills() {
        // SkillLoader::new(None)：scan 直接返回 → 空列表 → 无块。
        let loader = crate::api_agent::skills::SkillLoader::new(None);
        assert!(render_skills_block(&loader.list_skills()).is_none());
        // 空临时目录同样不产生任何技能。
        let tmp = std::env::temp_dir().join(format!(
            "pilotdesk_gc_skills_empty_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let loader =
            crate::api_agent::skills::SkillLoader::new(Some(tmp.to_string_lossy().to_string()));
        assert!(render_skills_block(&loader.list_skills()).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 技能块与单 Agent 会话同构：包裹标签 + 说明行，每项一行 `- **name**: description`。
    #[test]
    fn test_skills_block_renders_entries() {
        let block = render_skills_block(&[
            SkillEntry {
                name: "web_search".into(),
                description: "联网搜索".into(),
            },
            SkillEntry {
                name: "read_file".into(),
                description: String::new(),
            },
        ])
        .unwrap();
        assert!(block.starts_with(
            "<available_skills>\n以下是可用的技能列表。调用 load_skill 工具加载完整技能内容：\n\n"
        ));
        assert!(block.contains("- **web_search**: 联网搜索\n"));
        // description 为空时省略 `: ` 后缀。
        assert!(block.contains("- **read_file**\n"));
        assert!(block.ends_with("</available_skills>"));
        assert!(!block.contains("- **read_file**: "));
    }
}
