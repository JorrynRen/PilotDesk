//! 参与者抽象层（framework 层，provider/agent 无关）
//!
//! 对外只暴露三个 trait：`LlmClient` / `CliRunner` / `Participant`。
//! 二次开发者通常只需实现前两者之一即可接入自有 Agent。

use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

use super::models::{ConfirmationItem, ConfirmationRequest, summarize_confirmation_title};

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
}

impl ChatMessage {
    pub fn user(content: &str) -> Self {
        Self { role: "user".into(), name: None, content: content.into(), images: None }
    }
    #[allow(dead_code)]
    pub fn assistant(content: &str) -> Self {
        Self { role: "assistant".into(), name: None, content: content.into(), images: None }
    }
    #[allow(dead_code)]
    pub fn system(content: &str) -> Self {
        Self { role: "system".into(), name: None, content: content.into(), images: None }
    }
    #[allow(dead_code)]
    pub fn named(role: &str, name: &str, content: &str) -> Self {
        Self { role: role.into(), name: Some(name.into()), content: content.into(), images: None }
    }
}

/// 立场快照（L2 收敛裁决依据）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stance {
    pub participant_id: String,
    pub stance: String,
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

    /// 返回 `(回复内容, 工具调用链 JSON)`。默认仅调用 `complete` 并置空工具调用链；
    /// 支持工具的适配器（如 `PilotDeskLlmClient`）可覆写以透出本轮工具调用过程。
    async fn complete_with_tool_calls(
        &self,
        system: &str,
        messages: &[ChatMessage],
        on_delta: Option<Arc<DeltaFn>>,
    ) -> Result<(String, String), String> {
        let content = self.complete(system, messages, on_delta).await?;
        Ok((content, String::new()))
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
    async fn run_turn(&self, view: TurnView, on_delta: Option<Arc<DeltaFn>>) -> TurnResult;
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

    async fn run_turn(&self, view: TurnView, on_delta: Option<Arc<DeltaFn>>) -> TurnResult {
        let role = self.role_prompt.read().map(|g| g.clone()).unwrap_or_default();
        let system = build_llm_system_prompt(&view, &role);
        match self.llm.complete_with_tool_calls(&system, &view.messages, on_delta).await {
            Ok((content, tool_calls)) => {
                // 参与者运行中主动发起确认：若输出为确认请求 JSON，则转确认而非普通发言。
                if let Some(confirmation) = parse_confirmation_request(&content) {
                    TurnResult {
                        content: String::new(),
                        tool_calls: String::new(),
                        metadata: None,
                        error: None,
                        confirmation: Some(confirmation),
                    }
                } else {
                    TurnResult { content, tool_calls, metadata: None, error: None, confirmation: None }
                }
            }
            Err(e) => TurnResult {
                content: String::new(),
                tool_calls: String::new(),
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
}

#[async_trait::async_trait]
impl Participant for CliParticipant {
    fn id(&self) -> &str {
        &self.id
    }

    async fn run_turn(&self, view: TurnView, on_delta: Option<Arc<DeltaFn>>) -> TurnResult {
        let prompt = build_cli_prompt(&view);
        match self.runner.run(&self.config, &prompt, on_delta).await {
            Ok(out) => TurnResult { content: out.stdout, tool_calls: String::new(), metadata: None, error: None, confirmation: None },
            Err(e) => TurnResult {
                content: String::new(),
                tool_calls: String::new(),
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

    async fn run_turn(&self, _view: TurnView, _on_delta: Option<Arc<DeltaFn>>) -> TurnResult {
        TurnResult { content: String::new(), tool_calls: String::new(), metadata: None, error: None, confirmation: None }
    }
}

/// 组装 API 参与者的 system prompt（角色 + 议题 + 摘要 + 立场）。
fn build_llm_system_prompt(view: &TurnView, role_prompt: &str) -> String {
    let mut s = format!(
        "你在一个多 Agent 群聊中协作解决问题，你的角色是：{}\n\n当前讨论目标：{}\n",
        if role_prompt.is_empty() { "参与者" } else { role_prompt },
        view.topic
    );
    if !view.summary.is_empty() {
        s.push_str(&format!("\n[全局摘要]\n{}\n", view.summary));
    }
    if !view.stances.is_empty() {
        s.push_str("\n[各参与者当前立场]\n");
        for st in &view.stances {
            s.push_str(&format!("- {}: {}\n", st.participant_id, st.stance));
        }
    }
    if !view.task_context.is_empty() {
        s.push_str(&format!("\n[你负责的任务]\n{}\n", view.task_context));
    }
    s.push_str(
        "\n请基于以上信息推进当前议题：若你被指派了任务，优先围绕【你负责的任务】给出具体方案与推进意见；\
         否则给出你的观点、补充或质疑。发言要具体、有建设性，避免空泛复述或复述无关文件内容；\
         如果你有不同意见，请直接说明理由。\n\n\
         【工具使用纪律】只有在你被指派的任务明确涉及某个具体文件、目录或代码片段时，才调用文件类工具\
         （read_file/list_files/glob/grep/execute_command）；禁止为“了解环境”而无差别扫描整个工作区，\
         禁止 list_files 全量列出后再逐一读取所有文件。需要定位信息时，优先用 glob/grep 精确匹配目标，\
         再按需 read_file 单个文件；若任务本身与工作区文件无关，直接基于已有信息推进即可。\n\n\
         【重要】如果你在推进过程中需要向用户提问、请求决策或等待用户输入才能继续，\
         不要输出普通发言，而是只输出一个 JSON 对象（不要 markdown 代码块）：\n\
         {\"askUser\":true,\"title\":\"不超过22字的确认事项摘要\",\"prompt\":\"向用户提出的问题\",\"replyMode\":\"open\",\"items\":[]}\n\
         replyMode 为 \"open\"（开放式回复）或 \"structured\"（结构化多项）；title 为给用户看的确认事项摘要标题（不超过22字）；structured 时 items 为数组，\
         每项格式：{\"id\":\"唯一标识\",\"label\":\"问题\",\"inputType\":\"text|select|confirm\",\"options\":[],\"required\":false,\"placeholder\":\"\"}。\
         options 仅 inputType=select 时填写；inputType=confirm 表示是/否选择。\
         只有确实需要用户确认时才输出该 JSON；否则正常输出你的观点或结果。",
    );
    s
}

/// 组装 CLI 参与者的 prompt。
fn build_cli_prompt(view: &TurnView) -> String {
    let mut s = format!("当前讨论目标：{}\n", view.topic);
    if !view.summary.is_empty() {
        s.push_str(&format!("\n[全局摘要]\n{}\n", view.summary));
    }
    if !view.stances.is_empty() {
        s.push_str("\n[各参与者当前立场]\n");
        for st in &view.stances {
            s.push_str(&format!("- {}: {}\n", st.participant_id, st.stance));
        }
    }
    if !view.task_context.is_empty() {
        s.push_str(&format!("\n[你负责的任务]\n{}\n", view.task_context));
    }
    s.push_str(
        "\n请基于以上信息推进当前议题：若你被指派了任务，优先围绕【你负责的任务】给出具体方案与推进意见；\
         否则给出你的观点、补充或质疑，直接输出结论即可。",
    );
    s
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
            .map(|arr| arr.iter().map(|it| ConfirmationItem::from_json(it, "inputType")).collect())
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
