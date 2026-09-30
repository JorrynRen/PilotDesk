use serde::{Deserialize, Serialize};

/// 群聊房间（groupchat_rooms）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub title: String,
    /// 讨论目标（L1 摘要的锚点）
    pub topic: String,
    /// idle/running/paused/finished/aborted
    pub status: String,
    /// 发言策略（round_robin 等）
    pub strategy: String,
    pub max_rounds: i64,
    /// 执行阶段并行上限
    pub max_parallel: i64,
    /// 当前 Director 参与者 id（可热替换）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub director_id: Option<String>,
    /// 当前讨论中的议程/子任务 id（发言权角色匹配锚点 + 恢复进度定位）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// 累积的补充/细化约束（JSON 数组字符串，随初始目标一起注入 Director 决策）
    #[serde(default = "default_goal_notes")]
    pub goal_notes: String,
    /// 是否允许主持人在自动补人时添加 CLI 参与者（1=允许，0=禁止；按任务差异化限制，默认允许）
    #[serde(default = "default_allow_auto_cli")]
    pub allow_auto_cli: i64,
    /// 房间统一产物目录（绝对路径；空则运行时回退 `<工作目录>/outputs/<房间标题>/`）。
    /// 所有参与者产出的文件必须写入该目录，保证位置一致、房间内全局感知（v3.4ao）。
    #[serde(default)]
    pub output_dir: String,
}

fn default_goal_notes() -> String {
    "[]".to_string()
}

fn default_allow_auto_cli() -> i64 {
    1
}

/// 群聊参与者（groupchat_participants）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantRow {
    pub id: String,
    pub room_id: String,
    /// api/cli/user/director
    pub participant_type: String,
    /// JSON 字符串（api:{provider,model} / cli:{agent_type}）
    pub agent_config: String,
    pub display_name: String,
    /// 统一角色体系；用户可指定，未指定由 Director 开场分配，运行中可随时重配
    pub system_role: String,
    /// active/paused/finished
    pub status: String,
}

/// 群聊消息（groupchat_messages，L3/L4 的数据源）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub id: String,
    pub room_id: String,
    pub round: i64,
    pub seq: i64,
    /// participant_id（user/director 均为 participant 记录）
    pub sender: String,
    /// JSON 数组；[]=广播（只控可见性，发言权由 floor 串行）
    pub recipients: String,
    /// statement/question/reply/proposal/vote/task_assignment/task_result/summary/conclusion/directive/system
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub content: String,
    /// 附件元数据（图片/文件，JSON 数组字符串，结构复用 `crate::db::models::Attachment`）
    pub attachments: String,
    /// 参与者本轮工具调用链（reasoning/tool_start/tool_result 步骤，JSON 数组字符串，可溯源）
    pub tool_calls: String,
    /// 思考模式（DeepSeek 等）的 reasoning_content；assistant 消息下一轮请求原样回传
    pub reasoning_content: String,
    /// 附加结构化数据（如用户确认请求/回复，JSON 对象字符串），普通消息为 "{}"
    pub extra: String,
    pub timestamp: i64,
}

/// 立场快照（groupchat_stances，L2 收敛裁决依据）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StanceRow {
    pub room_id: String,
    pub participant_id: String,
    /// 该参与者当前立场（支持/反对/提案要点）
    pub stance: String,
    /// 立场态度（agree/disagree/neutral，由 stance 文本派生）
    pub attitude: String,
    pub updated_at: i64,
}

/// 任务（groupchat_tasks，开场编排产出，全程贯穿讨论与执行）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    pub id: String,
    pub room_id: String,
    pub task_no: i64,
    /// 高层任务描述（非工作流 JSON）
    pub description: String,
    /// participant_id；讨论阶段可空，进入执行前由 Director 指派
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// JSON 数组（前置任务 id）
    pub depends_on: String,
    /// discussing/pending/running/success/failed/skipped
    pub status: String,
    /// 一句话汇报（L3 可直接引用的成果）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}

/// 向用户发起的确认请求（Director 裁决 ask_user 时产生，或参与者运行中主动发起）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmationRequest {
    /// 唯一标识，供前端回复时关联
    pub request_id: String,
    /// 关联任务 id（用户回复后由运行时恢复 pending 任务重跑）
    pub task_id: String,
    /// 确认事项摘要标题（≤22 字，由 LLM 生成；空则前端兜底取 prompt 首行）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// 确认的问题/说明
    pub prompt: String,
    /// open=开放式回复；structured=结构化确认项列表
    pub reply_mode: String,
    /// structured 时的确认项
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ConfirmationItem>,
}

/// 结构化确认项（前端按 input_type 渲染对应控件）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmationItem {
    pub id: String,
    pub label: String,
    /// select / confirm / text
    pub input_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl ConfirmationItem {
    /// 从 LLM 输出的确认项 JSON 解析，做字段兜底归一化，避免前端控件失效。
    /// `input_key` 区分 Director（snake_case: input_type）与参与者（camelCase: inputType）输出。
    pub fn from_json(it: &serde_json::Value, input_key: &str) -> Self {
        let label = it["label"].as_str().unwrap_or("").trim().to_string();
        // id 兜底：缺省时用 label 去空白，避免多确认项共用默认 "q" 导致前端勾选状态错乱。
        let id = it["id"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if label.is_empty() {
                    "q".to_string()
                } else {
                    label.chars().filter(|c| !c.is_whitespace()).collect()
                }
            });
        let input_type = match it[input_key].as_str().unwrap_or("text") {
            "confirm" => "confirm".to_string(),
            "select" => "select".to_string(),
            _ => "text".to_string(),
        };
        ConfirmationItem {
            id,
            label,
            input_type,
            options: parse_options(&it["options"]),
            required: it["required"].as_bool().unwrap_or(false),
            placeholder: it["placeholder"]
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        }
    }
}

/// 生成 ≤22 字的确认事项摘要标题：取首行非空文本，按字符截断（无省略号）。
pub(crate) fn summarize_confirmation_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("");
    line.chars().take(22).collect()
}

/// 解析选项字段：兼容字符串数组、数字数组、`{label,value}` 对象数组，以及逗号分隔字符串。
/// 防止模型输出格式偏差导致 options 缺失（前端无选项可选）。
fn parse_options(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|x| match x {
                serde_json::Value::String(s) => Some(s.trim().to_string()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                serde_json::Value::Object(o) => o
                    .get("label")
                    .or_else(|| o.get("value"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.trim().to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect(),
        serde_json::Value::String(s) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}
