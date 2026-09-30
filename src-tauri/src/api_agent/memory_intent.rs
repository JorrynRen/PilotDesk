//! 上下文注入：每轮组装 system prompt 前，用一次轻量 LLM 调用把用户最新消息解析为
//! 分类数组 + 关键词数组，然后**走两条互斥的链路**：
//!   1. **会话记忆**（`select_session_memories`，匹配面 key / tags，受路由分类约束）；
//!   2. **知识库**（`search_knowledge`，按**正文**走 `kb_entry_fts`，不受分类约束 ——
//!      `knowledge` 不在分类白名单里，靠分类过滤只会把整库静默排除）。
//! 命中即注入、两边都 0 命中则不注入。
//!
//! 开关：app_settings `memory_intent_enabled`（缺省视为启用；置 "0"/"false" 关闭；
//! 设置 → 记忆管理 → KV 记忆 中暴露为"启用意图检索注入"）。
//! 模型覆盖：app_settings `memory_intent_model` =
//! `{"providerId":"...","model":"..."}`（providerId 留空 = 只覆盖模型名、沿用当前会话的
//! 提供商；整体缺省 = 提供商与模型都跟随当前会话。见 `parse_model_override`）。
//! 请求超时：app_settings `memory_intent_timeout_secs`（缺省 60 秒，合法 1~60；同页可填）。
//! 该值是**上限而非等待时长**：模型返回即继续，只有上游卡住才会等满。
//! 路由请求复用 `ApiClient` 的流式路径（与会话主请求同一套连接参数与请求体构造），
//! 仅用外层 `tokio::time::timeout` 按配置秒数截断。
//! 请求形态：用户消息包成 `{"message": "..."}` 放在**数据位**（对话位会诱发模型"回答消息"），
//! 并附带 `response_format: json_object` 把"输出必须是合法 JSON"变成请求约束；
//! provider 不支持（HTTP 400/422）时降级为普通请求并记下冷却期标记，不每轮重复撞错。
//! 响应解析三层容错：首个括号配平的 JSON 对象 → 截断抢救（预算被思考吃光时）→ 纯 JSON 数组；
//! 三者都不成立即判解析失败，绝不把"一段人话"当关键词。
//! 失败处理：路由请求失败 / 响应不可解析 → 本轮不注入（连续失败达阈值进入熔断冷却期，
//! 冷却期内不再发起路由请求；本地超时属首字节延迟抖动，不计入熔断）；
//! 用户关闭开关或输入为空时同样不注入。
//! 解析成功但未给出任何关键词 → 不检索、不注入（非错误：消息可能确实没有可检索主题；
//! 此时不按分类或热度兜底注入）。
//! 不注入不影响按需检索：`search_memory` 始终作为普通工具可用。

use crate::api_agent::client::{ApiClient, ChatResponse};
use crate::api_agent::db::{
    format_memories_block, meta_compact, source_label, MemoryStore, SourceKind, SourcedEntry,
};
use crate::api_agent::types::{ApiFormat, ChatMessage, ChatRequest, UsageRecord};

/// app_settings key：意图路由开关（"1"/"true" 开；其余关；缺省视为开）。
pub const SETTING_ENABLED: &str = "memory_intent_enabled";
/// app_settings key：意图路由模型覆盖（缺省使用当前会话模型）。
pub const SETTING_MODEL: &str = "memory_intent_model";
/// app_settings key：意图路由请求超时（秒）。
pub const SETTING_TIMEOUT_SECS: &str = "memory_intent_timeout_secs";

const ALLOWED_CATEGORIES: [&str; 4] = ["fact", "preference", "skill", "event"];
/// 每次注入最多返回的**会话记忆**条数。
const INJECT_LIMIT: usize = 5;
/// 每次注入最多返回的**知识库**条数。与 `INJECT_LIMIT` **各自独立**：知识条目不是会话记忆，
/// 共用一个额度只会让两边互相挤占（而且知识库一旦建起来，命中数往往比会话记忆多得多）。
const KNOWLEDGE_INJECT_LIMIT: usize = 4;
/// 会话记忆的单条预览上限：它是短事实卡片（见 `format_memories_block` 的说明）。
const MEMORY_PREVIEW_CHARS: usize = 600;
/// 知识条目的单条预览上限：知识分块的正文常 400~1200 字，用 600 会把它砍掉一半 ——
/// 而"带出来的知识只有半截"比不带更糟（模型会照着半句话作答）。
const KNOWLEDGE_PREVIEW_CHARS: usize = 1200;
/// 意图路由短调用的最大输出 token。
/// 不能贴着 JSON 本身的大小给（只说 JSON 本身约 100~200 token）：思考型模型会先产出
/// reasoning 再产出 content，预算只够思考时会以 finish_reason=length 结束且 content 为空，
/// 表现为"路由返回空"。这里给一段简短思考 + JSON 留余量，同时仍把最坏时长压在几秒量级。
const ROUTE_MAX_TOKENS: u32 = 1024;
/// 意图路由请求超时（秒）—— **上限，不是等待时长**：模型返回即继续，不会补足到该秒数。
/// 路由在主请求之前同步执行，这个上限因此等于"provider 卡住时每条消息最多被拖多久"。
/// 缺省取合法上界（见 ROUTE_TIMEOUT_MAX_SECS）：正常往返实测约 2 秒，留足余量避免慢窗口
/// 白丢一次注入；需要更紧的失败前置延迟时在设置页调小。
pub const DEFAULT_ROUTE_TIMEOUT_SECS: u64 = 60;
/// 路由超时的合法下界（秒）：再低会连正常往返都框不住。
const ROUTE_TIMEOUT_MIN_SECS: u64 = 1;
/// 路由超时的合法上界（秒），也是缺省值：再高会把 provider 卡住时的前置延迟放大到不可接受。
const ROUTE_TIMEOUT_MAX_SECS: u64 = 60;
/// 意图路由输入上限（字符）：只取最新用户消息的前若干字符，超长粘贴不把轻量调用拖重。
const ROUTE_INPUT_MAX_CHARS: usize = 2000;
/// 连续失败达到该次数即熔断（进入冷却期，期间不再发起路由请求）。
const ROUTE_BREAKER_FAILURES: u32 = 2;
/// 熔断冷却时长（秒）：冷却期内直接按"不注入"处理，避免故障期每轮白等超时并消耗配额。
const ROUTE_BREAKER_COOLDOWN_SECS: u64 = 600;
/// provider 被判定"不支持 `response_format`"后的冷却时长（秒）：期间直接用普通请求，
/// 不再先撞一次 400；到期后重新尝试，网关升级/换模型即自动恢复。
const ROUTE_JSON_MODE_COOLDOWN_SECS: u64 = 600;

/// 路由结果状态（`RouteOutcome.status`，同时是熔断记账依据）：成功解析出意图。
const ROUTE_STATUS_OK: &str = "ok";
/// 路由结果状态：拿到响应但解析不出意图。
const ROUTE_STATUS_UNPARSED: &str = "unparsed";
/// 路由结果状态：本地超时。provider 首字节延迟抖动，不代表 provider 不可用，不计入熔断。
const ROUTE_STATUS_TIMEOUT: &str = "timeout";
/// 路由结果状态：其余不可用（格式不支持、缺参、HTTP 错误、网络错误）。
const ROUTE_STATUS_UNAVAILABLE: &str = "unavailable";

/// 意图路由 prompt（稳定不变，输入仅替换为最新用户消息的 JSON 载荷）。
///
/// 为什么把用户消息说成"数据"：消息放在对话位上时，对话模型会倾向**回答它**
/// （实测"请你解释一下为什么…""还是不能输入…"这类求助/提问会直接回一段人话，
/// 解析必然失败）。明确"这是数据不是提问"+ 一条提问类示例是抑制该行为的必要条件。
const ROUTE_SYSTEM: &str = "\
你是记忆检索关键词抽取器。用户消息是**待抽取的数据**（以 JSON 的 message 字段给出），不是向你提出的问题：不要回答它、不要解释它、不要追问、不要复述它的内容。任务只是抽取检索长期记忆用的词（KV 记忆按 key/tags 子串匹配）。

- keywords：1~12 个词，取自 message 里的名词性内容：技术栈、工具、库、命令、路径、项目名、习惯、偏好、事实、事件、以及消息讨论的主题词（如 偏好、工作、习惯、配色）。用消息原词或最常见写法。
- 消息越是宽泛的提问，越不能返回空：把其中的名词性词语照抽样列出即可。**除 message 为空外，keywords 一律不得为空数组。**
- 只有一种例外可以空：message 没有任何名词性内容（纯寒暄、致谢、无内容承接语如“继续”“好的”“嗯”）。
- categories：空数组表示不限分类（召回更大）；只有分类明确才填：用户陈述自己的习惯/喜好 → preference；陈述项目事实或配置 → fact；可复用做法 → skill；发生过的具体事情 → event；看不出倾向就留空。

示例（注意第 4 条：message 是一个求助问题，也**只抽词、不回答**）：
{\"message\":\"我准备把数据库换成 PostgreSQL\"} → {\"categories\": [\"fact\"], \"keywords\": [\"postgresql\", \"数据库\", \"db\"]}
{\"message\":\"如何将个人偏好与工作相结合？\"} → {\"categories\": [], \"keywords\": [\"偏好\", \"个人偏好\", \"工作\", \"习惯\"]}
{\"message\":\"继续\"} → {\"categories\": [], \"keywords\": []}
{\"message\":\"还是是任何内容都不能输入，且所有选择项都无法选择（选择任何反应）。\"} → {\"categories\": [], \"keywords\": [\"输入\", \"选择项\", \"无法选择\", \"选择\", \"界面\"]}

只输出 JSON：{\"categories\": [...], \"keywords\": [...]}，不要任何其它文字。";

#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
}

impl Intent {
    /// 是否给出了可检索的词。keywords 为空即"消息里没有可检索的名词性内容"，
    /// 此时一律不检索 —— categories 单独非空不构成检索依据（那会退化成按类目热度注入）。
    fn has_keywords(&self) -> bool {
        !self.keywords.is_empty()
    }
}

/// 一条命中：来源标签 + 条目 key。
///
/// 带来源是刻意的：用户要能看出"这条回答引用了哪个知识库"。只报"命中 3 条"的话，
/// 自动注入对用户仍是黑盒 —— 不知道命中的是自己存的事实，还是某份资料里的内容。
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalHit {
    /// `记忆·fact` / `知识库·小说创作`（多归属时 `知识库·小说创作 等 2 个库`）
    pub source: String,
    pub key: String,
}

/// 本轮自动记忆检索的可观测轨迹：调用方据此向前端显式展示"是否调用意图路由、
/// 用什么模型与参数、命中哪些 key、是否注入"，使用户对自动注入不再无感。
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalTrace {
    /// disabled（功能关闭）/ empty_input / route_breaker_open（熔断冷却中）/ route_unavailable /
    /// route_unparsed / no_keywords（未返回任何关键词，不检索也不注入）/ matched（已按意图检索）
    pub status: &'static str,
    /// 意图路由实际使用的模型；未发起调用时为空串。
    pub model: String,
    /// 路由解析出的分类过滤（空表示未限分类）。
    pub categories: Vec<String>,
    /// 路由解析出的关键词（空表示未给关键词）。
    pub keywords: Vec<String>,
    /// 最终命中并（可能）注入的条目（带来源标签）。
    pub matched: Vec<RetrievalHit>,
    /// 是否确实注入了记忆块。
    pub injected: bool,
    /// 未注入时的具体原因（成功路径为空串）：路由请求失败 / 响应解析失败 / 熔断冷却。
    /// 随轨迹一并展示，使用户无需翻日志即可归因。
    pub reason: String,
}

fn is_enabled(conn: &rusqlite::Connection) -> bool {
    match crate::commands::app_settings::get_setting(conn, SETTING_ENABLED) {
        Ok(Some(v)) => {
            let v = v.trim().to_lowercase();
            v != "0" && v != "false" && v != "off"
        }
        // 缺省视为启用（灰度后可通过设置关闭）
        _ => true,
    }
}

/// 意图路由开关（同步读取，供调用方在 await 前取好标量，避免连接引用跨 await）。
pub fn read_enabled(conn: &rusqlite::Connection) -> bool {
    is_enabled(conn)
}

/// 意图路由模型覆盖（同步读取；None 表示使用当前会话的提供商与模型）。
///
/// `provider_id` 为空 = 沿用当前会话的提供商，只换模型名（这也是旧值形态的语义）；
/// 非空 = 连 endpoint / api_format / api_key 一起换成该提供商 —— 与知识库「指定模型」
/// 同一套口径，允许"路由走便宜的小模型，主对话走大模型"。
#[derive(Debug, Clone, PartialEq)]
pub struct IntentModelOverride {
    pub provider_id: String,
    pub model: String,
}

/// 解析覆盖项：`{"providerId":"...","model":"..."}`；
/// 兼容旧值 —— 早期这里只存一个裸模型名，按"仅覆盖模型名"解析（provider_id 留空）。
pub fn parse_model_override(raw: &str) -> Option<IntentModelOverride> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        let provider_id = v["providerId"].as_str().unwrap_or("").trim().to_string();
        let model = v["model"].as_str().unwrap_or("").trim().to_string();
        if provider_id.is_empty() && model.is_empty() {
            return None;
        }
        return Some(IntentModelOverride { provider_id, model });
    }
    Some(IntentModelOverride {
        provider_id: String::new(),
        model: raw.to_string(),
    })
}

pub fn read_model_override(conn: &rusqlite::Connection) -> Option<IntentModelOverride> {
    crate::commands::app_settings::get_setting(conn, SETTING_MODEL)
        .ok()
        .flatten()
        .and_then(|raw| parse_model_override(&raw))
}

/// 解析路由超时设置：非数字或超出合法区间一律回退缺省，避免填错把前置延迟放大或缩到无效。
fn parse_timeout_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| (ROUTE_TIMEOUT_MIN_SECS..=ROUTE_TIMEOUT_MAX_SECS).contains(s))
        .unwrap_or(DEFAULT_ROUTE_TIMEOUT_SECS)
}

/// 意图路由请求超时（同步读取，供调用方在 await 前取好标量，避免连接引用跨 await）。
pub fn read_timeout_secs(conn: &rusqlite::Connection) -> u64 {
    let raw = crate::commands::app_settings::get_setting(conn, SETTING_TIMEOUT_SECS)
        .ok()
        .flatten();
    parse_timeout_secs(raw.as_deref())
}

/// 解析路由响应为意图。三层容错，任一成功即返回：
/// 1. **首个括号配平的 JSON 对象** —— 模型常"先解释再给 JSON"或"JSON 后再补一段话"，
///    取"首个 `{` 到末尾 `}`"会把两段拼成非法 JSON；
/// 2. **截断抢救** —— 思考型模型把预算烧在 reasoning 上时 JSON 会被截在数组中间
///    （finish_reason=length），此时从已输出的字符串字面量里回收完整项；
/// 3. **纯 JSON 数组** —— 模型偶尔只给关键词列表。
///
/// 三者都不成立即判解析失败：**绝不能**把"一段人话"当关键词（那会按句子注入无关记忆）。
fn parse_response(raw: &str) -> Option<Intent> {
    let text = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    if let Some(object) = first_balanced_object(text) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(object) {
            if let Some(intent) = intent_from_json(&json) {
                return Some(intent);
            }
        }
    }

    if let Some(intent) = salvage_truncated(text) {
        return Some(intent);
    }

    if let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(text) {
        let keywords: Vec<String> = values
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if !keywords.is_empty() {
            return Some(sanitize_intent(Vec::new(), keywords));
        }
    }

    None
}

/// 取首个括号配平的 JSON 对象（跳过字符串字面量里的花括号与转义）。
fn first_balanced_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + offset + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 从被截断的 JSON 里回收完整字符串项（仅用于数组未闭合的中间截断）。
fn salvage_truncated(text: &str) -> Option<Intent> {
    let keywords = collect_string_literals_after(text, "keywords")?;
    if keywords.is_empty() {
        return None;
    }
    let categories = collect_string_literals_after(text, "categories").unwrap_or_default();
    Some(sanitize_intent(categories, keywords))
}

/// 收集 `"<field>"` 之后方括号内的完整字符串字面量；数组未闭合时收到文本末尾为止。
fn collect_string_literals_after(text: &str, field: &str) -> Option<Vec<String>> {
    let marker = format!("\"{}\"", field);
    let field_at = text.find(&marker)?;
    let rest = &text[field_at + marker.len()..];
    let bracket = rest.find('[')?;

    let mut items: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in rest[bracket + 1..].chars() {
        if in_string {
            if escaped {
                current.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                items.push(std::mem::take(&mut current));
                in_string = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            ']' => break,
            _ => {}
        }
    }
    Some(items)
}

/// 从 JSON 对象取意图；字段存在但类型错误（非数组）视为解析失败，避免把损坏输出当空意图。
fn intent_from_json(json: &serde_json::Value) -> Option<Intent> {
    let cats_field = json.get("categories");
    if cats_field.is_some() && cats_field.and_then(|v| v.as_array()).is_none() {
        return None;
    }
    let kws_field = json.get("keywords");
    if kws_field.is_some() && kws_field.and_then(|v| v.as_array()).is_none() {
        return None;
    }

    let categories: Vec<String> = cats_field
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let keywords: Vec<String> = kws_field
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Some(sanitize_intent(categories, keywords))
}

/// 归一化：分类只留白名单内取值，关键词去空/去长词、去重（大小写不敏感）保序并截断上限。
fn sanitize_intent(categories: Vec<String>, keywords: Vec<String>) -> Intent {
    let mut categories: Vec<String> = categories
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| ALLOWED_CATEGORIES.contains(&s.as_str()))
        .collect();
    let mut keywords: Vec<String> = keywords
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().count() <= 40)
        .collect();

    let mut seen = std::collections::HashSet::new();
    keywords.retain(|k| seen.insert(k.to_lowercase()));
    keywords.truncate(12);
    categories.truncate(4);

    Intent {
        categories,
        keywords,
    }
}

/// 意图路由调用的结果：请求成功但解析失败时仍回传用量（如实计费）。
struct RouteOutcome {
    intent: Option<Intent>,
    usage: Option<UsageRecord>,
    /// ok（解析成功）/ unparsed（有响应但解析失败）/ timeout（本地超时）/ unavailable（格式不支持或请求失败）
    status: &'static str,
    /// 非 ok 时的具体原因（HTTP 状态与响应片段 / 网络超时 / 格式不支持）。
    reason: String,
}

/// 按字符边界截取前 max 个字符（避免截断多字节字符）。
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

/// 意图路由的熔断状态（按 provider endpoint + model 分键）。
#[derive(Default)]
struct BreakerState {
    /// 连续失败次数（成功即清零）
    failures: u32,
    /// 冷却截止时刻；None 表示未熔断
    open_until: Option<std::time::Instant>,
}

/// 冷却期内返回 true（应跳过路由直接降级）；冷却结束则清零失败计数并恢复尝试。
fn breaker_should_skip(state: &mut BreakerState, now: std::time::Instant) -> bool {
    match state.open_until {
        Some(until) if now < until => true,
        Some(_) => {
            state.open_until = None;
            state.failures = 0;
            false
        }
        None => false,
    }
}

/// 按结果状态记账熔断：成功清零并撤销熔断；真实错误累计，达阈值即开启冷却期；
/// 本地超时不改变状态 —— 它只说明这次首字节来晚了，下一轮很可能正常，
/// 计入会让一次冷启动把路由熄火整个冷却期。
fn record_route_result(state: &mut BreakerState, status: &str, now: std::time::Instant) {
    if status == ROUTE_STATUS_TIMEOUT {
        return;
    }
    if status == ROUTE_STATUS_OK {
        state.failures = 0;
        state.open_until = None;
        return;
    }
    state.failures = state.failures.saturating_add(1);
    if state.failures >= ROUTE_BREAKER_FAILURES {
        state.open_until = Some(now + std::time::Duration::from_secs(ROUTE_BREAKER_COOLDOWN_SECS));
    }
}

/// 全局熔断状态表（进程级；按 provider 端点 + 模型分键，切换 provider 互不影响）。
fn breaker_states() -> &'static std::sync::Mutex<std::collections::HashMap<String, BreakerState>> {
    static STATES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, BreakerState>>,
    > = std::sync::OnceLock::new();
    STATES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 熔断键：同一 endpoint 下不同模型独立熔断。
fn breaker_key(endpoint: &str, model: &str) -> String {
    format!("{}|{}", endpoint.trim_end_matches('/'), model)
}

/// 截断并压平文本用于日志（避免把整段响应体写进日志）。
fn log_snippet(text: &str) -> String {
    const MAX: usize = 300;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX {
        return flat;
    }
    format!("{}…", flat.chars().take(MAX).collect::<String>())
}

/// 组装路由请求：固定的路由 system 提示 + 截断后的最新用户消息（JSON 数据位），不含历史消息、
/// 工具、技能或记忆注入；不发送 temperature（deepseek-reasoner 等推理模型不支持该参数）。
///
/// 用户消息包成 `{"message": "..."}` 而不是直接放进 user 位：同一段文本放在对话位上时，
/// 对话模型会倾向回答它（提问/求助类消息尤甚），抽取必然失败。
///
/// `stream` 固定为 true：路由与会话主请求走同一条流式路径 `chat_stream`。部分 provider
/// 在 `stream: false` 下不结束响应（连接只维持 keepalive、永不返回响应体），非流式读取
/// 会一直挂到外层超时。
///
/// `json_mode` 打开时附带 `response_format: json_object`：把"必须是合法 JSON"从提示词
/// 自觉升级为请求约束（实测该约束能拦住被指令式文本带偏的输出）。provider 不支持时
/// 由调用方降级重试。
fn route_request(model: &str, user_text: &str, json_mode: bool) -> ChatRequest {
    let payload = serde_json::json!({
        "message": truncate_chars(user_text, ROUTE_INPUT_MAX_CHARS),
    })
    .to_string();
    ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::system(ROUTE_SYSTEM),
            ChatMessage::user(&payload),
        ],
        tools: None,
        tool_choice: None,
        stream: true,
        temperature: None,
        // 预算不收紧：思考型模型会把预算烧在 reasoning 上（实测 256 时 JSON 被截在数组中间），
        // 截断比"多花几十个 token"更贵。
        max_tokens: Some(ROUTE_MAX_TOKENS),
        response_format: json_mode.then(|| serde_json::json!({ "type": "json_object" })),
    }
}

/// 不支持 `response_format` 的 provider 标记（key = endpoint|model）→ 冷却截止时刻。
/// 命中后直接跳过 json 模式，避免每轮都先撞一次 400 再重试（路由同步跑在主请求之前，
/// 白撞一次就是纯粹的额外延迟）。冷却而非永久：网关偶发 400 不该永久失去该能力。
fn json_mode_unsupported(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static UNSUPPORTED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    > = std::sync::OnceLock::new();
    UNSUPPORTED.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 该 provider 当前是否被标记为不支持 json 模式。
fn json_mode_disabled(key: &str) -> bool {
    let mut states = match json_mode_unsupported().lock() {
        Ok(states) => states,
        Err(_) => return false,
    };
    match states.get(key) {
        Some(until) if std::time::Instant::now() < *until => true,
        Some(_) => {
            states.remove(key);
            false
        }
        None => false,
    }
}

/// 记下"该 provider 不支持 json 模式"，进入 `ROUTE_JSON_MODE_COOLDOWN_SECS` 冷却。
fn mark_json_mode_unsupported(key: &str) {
    if let Ok(mut states) = json_mode_unsupported().lock() {
        states.insert(
            key.to_string(),
            std::time::Instant::now()
                + std::time::Duration::from_secs(ROUTE_JSON_MODE_COOLDOWN_SECS),
        );
    }
}

/// 错误是否像"provider 不支持 response_format"：请求形态问题通常回 400/422。
/// 宁可偶尔多降级一次（有冷却兜底），也不要让 json 模式因偶发 400 永久失效。
fn looks_like_unsupported_json_mode(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("http 400") || lower.contains("http 422")
}

/// 把"上游已送回多少数据"写成失败原因里的一句话。
/// 超时究竟是"网关一直没给数据"还是"上游在持续输出思考"，全看这两个数字。
fn received_progress(received: &std::sync::Mutex<(usize, usize)>) -> String {
    let (reasoning_chars, content_chars) = received.lock().map(|g| *g).unwrap_or((0, 0));
    if reasoning_chars == 0 && content_chars == 0 {
        "期间上游未返回任何数据".to_string()
    } else {
        format!(
            "期间上游已返回思考 {} 字符、正文 {} 字符",
            reasoning_chars, content_chars
        )
    }
}

/// 单次路由尝试：`json_mode` 决定是否附带结构化输出约束。
/// 同时把上游实际返回的正文/思考字符数累加进 `received`（失败归因用）。
async fn route_attempt(
    client: &ApiClient,
    model: &str,
    user_text: &str,
    json_mode: bool,
    received: &std::sync::Mutex<(usize, usize)>,
) -> Result<ChatResponse, String> {
    let request = route_request(model, user_text, json_mode);
    client
        .chat_stream(
            &request,
            |c| {
                if let Ok(mut r) = received.lock() {
                    r.1 += c.chars().count();
                }
            },
            |r| {
                if let Ok(mut s) = received.lock() {
                    s.0 += r.chars().count();
                }
            },
            |_, _, _| {},
            |_, _, _, _, _| {},
        )
        .await
}

/// 调用 OpenAI 兼容接口获取意图 JSON；anthropic 格式或请求失败返回 unavailable。
///
/// 失败一律记录 warn 日志（跳过原因 / HTTP 状态码与响应片段 / 网络超时），
/// 否则意图路由不可用时用户与开发者都无从归因。
async fn route_via_llm(
    api_format: &str,
    endpoint: &str,
    api_key: &str,
    model: &str,
    timeout_secs: u64,
    user_text: &str,
) -> RouteOutcome {
    let fail = |status: &'static str, reason: String| {
        log::warn!("[MemoryIntent] 意图路由不可用: {}", reason);
        RouteOutcome {
            intent: None,
            usage: None,
            status,
            reason,
        }
    };
    if api_format.to_lowercase().contains("anthropic") {
        return fail(
            ROUTE_STATUS_UNAVAILABLE,
            format!("仅支持 OpenAI 兼容格式，当前 api_format={}", api_format),
        );
    }
    if endpoint.trim().is_empty() || model.is_empty() {
        return fail(
            ROUTE_STATUS_UNAVAILABLE,
            format!(
                "缺少 api_endpoint 或 model（endpoint 为空={}，model 为空={}）",
                endpoint.trim().is_empty(),
                model.is_empty()
            ),
        );
    }

    // 与会话主请求用同一个 ApiClient 构造与同一条流式路径：连接参数（600s 总时长 /
    // 10s 连接超时）、连接池、请求体构造、429 重试与错误文案全部一致。
    let client = ApiClient::new(
        endpoint.trim().to_string(),
        api_key.to_string(),
        ApiFormat::OpenAI,
    );
    let provider_key = breaker_key(endpoint, model);
    // 结构化输出优先；该 provider 已被标记不支持（冷却期内）则直接用普通请求发一次。
    let try_json_mode = !json_mode_disabled(&provider_key);

    let started = std::time::Instant::now();
    // 超时用外层 tokio 计时：ApiClient 自身总时长上限是 600s（为主请求设计），
    // 路由必须按用户配置的秒数截断，否则慢 provider 会把每条消息的前置延迟拖长。
    // 降级重试也在同一段预算内，避免"撞 400 + 重试"把前置延迟翻倍。
    //
    // 同时累计上游实际送回的数据量。"上游迟迟不回"有两种成因完全不同的形态：一直不吐任何
    // 数据（网关把请求挂住）与持续吐思考内容却不出正文（请求形态诱发了长思考）。失败原因里
    // 带上这两个数字，归因不必再靠猜。
    let received: std::sync::Arc<std::sync::Mutex<(usize, usize)>> =
        std::sync::Arc::new(std::sync::Mutex::new((0, 0)));
    let response = match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), async {
        let first = route_attempt(&client, model, user_text, try_json_mode, &received).await;
        match first {
            Ok(response) => Ok(response),
            Err(err) if try_json_mode && looks_like_unsupported_json_mode(&err) => {
                log::warn!(
                    "[MemoryIntent] provider 可能不支持 response_format，降级重试一次: {}",
                    err
                );
                mark_json_mode_unsupported(&provider_key);
                route_attempt(&client, model, user_text, false, &received).await
            }
            Err(err) => Err(err),
        }
    })
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return fail(
                ROUTE_STATUS_UNAVAILABLE,
                format!(
                    "请求失败（耗时 {}ms，{}）: {}",
                    started.elapsed().as_millis(),
                    received_progress(&received),
                    e
                ),
            )
        }
        Err(_) => {
            return fail(
                ROUTE_STATUS_TIMEOUT,
                format!(
                    "请求失败（耗时 {}ms，本地超时，上限 {}s；{}）",
                    started.elapsed().as_millis(),
                    timeout_secs,
                    received_progress(&received)
                ),
            )
        }
    };

    log::info!(
        "[MemoryIntent] 意图路由请求成功（model={}, 耗时 {}ms, finish_reason={}, content {} 字符, reasoning {} 字符）",
        model,
        started.elapsed().as_millis(),
        response.finish_reason,
        response.content.chars().count(),
        response.reasoning_content.chars().count()
    );
    let usage = response.usage;

    // 思考型模型可能把内容都放在 reasoning_content，或在思维链上耗尽预算导致 content 为空
    // （finish_reason=length）。先解析 content，解析不出再退回 reasoning：
    // parse_response 要求取出的片段是结构合法的意图 JSON，因此误采思维链里举例文本的概率很低。
    let parsed = parse_intent_response(&response.content, &response.reasoning_content);

    match parsed {
        Some(intent) => RouteOutcome {
            intent: Some(intent),
            usage,
            status: ROUTE_STATUS_OK,
            reason: String::new(),
        },
        None => {
            let content_len = response.content.chars().count();
            let reasoning_len = response.reasoning_content.chars().count();
            let reason = if content_len == 0 && reasoning_len == 0 {
                format!("响应为空（finish_reason={}）", response.finish_reason)
            } else if content_len == 0 {
                format!(
                    "响应 content 为空、只有思考内容（finish_reason={}，reasoning {} 字符）",
                    response.finish_reason, reasoning_len
                )
            } else {
                format!(
                    "响应不是预期 JSON（finish_reason={}，content {} 字符）: {}",
                    response.finish_reason,
                    content_len,
                    log_snippet(&response.content)
                )
            };
            log::warn!("[MemoryIntent] 路由响应无法解析: {}", reason);
            RouteOutcome {
                intent: None,
                usage,
                status: ROUTE_STATUS_UNPARSED,
                reason,
            }
        }
    }
}

/// 从响应中解析意图：正文优先；正文解析不出时退回思考内容。
///
/// 退回是必要的：思考型模型会先产出 reasoning_content 再产出 content，预算吃紧时
/// 整段输出都在 reasoning 里、content 为空（finish_reason=length），只看正文就会把
/// 一次有效的路由判成"响应为空"。
fn parse_intent_response(content: &str, reasoning: &str) -> Option<Intent> {
    parse_response(content).or_else(|| {
        if reasoning.trim().is_empty() {
            None
        } else {
            parse_response(reasoning)
        }
    })
}

/// 组装注入块（异步部分只接收标量与 store，避免连接引用跨 await 导致 !Send）。
/// 注入只有一种成立路径：意图命中。此外一律不注入：
/// - 未启用（用户关闭开关）/ 输入为空 → 不发起路由，不注入；
/// - 路由不可用 / 本地超时 / 解析失败（含熔断冷却期）→ 不注入；
/// - 路由成功但无意图（空 categories 且空 keywords）→ 0 命中，不注入；
/// - 有意图 → 两条链路各检索一次，命中即注入，两边都 0 命中不注入。
///
/// 不注入不代表模型失去记忆能力：`search_memory` 始终作为普通工具可用，模型可按需调用。
///
/// 返回 (注入块, 意图路由调用用量, 检索轨迹)：调用方据用量计费、据轨迹向前端显式
/// 展示自动检索（是否调用、参数、命中 key、是否注入）。
pub async fn memory_injection_block(
    enabled: bool,
    model_override: Option<String>,
    timeout_secs: u64,
    api_format: &str,
    endpoint: &str,
    api_key: &str,
    session_model: &str,
    user_text: &str,
    store: &MemoryStore,
) -> (Option<String>, Option<UsageRecord>, RetrievalTrace) {
    let bare_trace = |status: &'static str| RetrievalTrace {
        status,
        model: String::new(),
        categories: Vec::new(),
        keywords: Vec::new(),
        matched: Vec::new(),
        injected: false,
        reason: String::new(),
    };

    if user_text.trim().is_empty() || !enabled {
        // 关闭开关或空输入：不注入任何记忆（模型仍可调用 search_memory 按需检索）。
        let status = if enabled { "empty_input" } else { "disabled" };
        log::info!("[MemoryIntent] 本轮不注入记忆（{}）", status);
        return (None, None, bare_trace(status));
    }

    let model = model_override.unwrap_or_else(|| session_model.to_string());

    // 熔断：连续失败达阈值后进入冷却期，期间直接按"不注入"处理，
    // 避免故障期每轮白等超时并持续消耗 provider 配额。
    let route_key = breaker_key(endpoint, &model);
    {
        let mut states = breaker_states().lock().unwrap();
        let state = states.entry(route_key.clone()).or_default();
        if breaker_should_skip(state, std::time::Instant::now()) {
            log::info!("[MemoryIntent] 意图路由处于熔断冷却期，本轮跳过路由且不注入记忆");
            return (
                None,
                None,
                RetrievalTrace {
                    status: "route_breaker_open",
                    model,
                    categories: Vec::new(),
                    keywords: Vec::new(),
                    matched: Vec::new(),
                    injected: false,
                    reason: format!(
                        "熔断冷却中：此前连续 {} 次路由失败，{}s 内不再发起路由",
                        ROUTE_BREAKER_FAILURES, ROUTE_BREAKER_COOLDOWN_SECS
                    ),
                },
            );
        }
    }

    let RouteOutcome {
        intent,
        usage,
        status,
        reason,
    } = route_via_llm(
        api_format,
        endpoint,
        api_key,
        &model,
        timeout_secs,
        user_text,
    )
    .await;
    {
        let mut states = breaker_states().lock().unwrap();
        record_route_result(
            states.entry(route_key).or_default(),
            status,
            std::time::Instant::now(),
        );
    }

    let Some(intent) = intent else {
        // 路由不可用/解析失败：本轮不注入任何记忆（避免与当前话题无关的 top-N 噪声），
        // 模型需要用户偏好或事实时由 search_memory 主动检索（其工具描述已说明）。
        log::info!(
            "[MemoryIntent] 意图路由不可用（{}），本轮不注入记忆",
            status
        );
        return (
            None,
            usage,
            RetrievalTrace {
                status: if status == ROUTE_STATUS_UNPARSED {
                    "route_unparsed"
                } else {
                    "route_unavailable"
                },
                model,
                categories: Vec::new(),
                keywords: Vec::new(),
                matched: Vec::new(),
                injected: false,
                reason,
            },
        );
    };

    let keywords = intent.keywords.clone();
    let categories_vec = intent.categories.clone();
    if !intent.has_keywords() {
        // 没有关键词即"消息里没有可检索的名词性内容"：本轮不检索、不注入。
        // 不按分类或热度兜底注入（categories 单独非空不构成检索依据 —— 那等于把该类目下
        // 最热的几条塞给用户，与本轮话题无关）。非错误：只是没有可检索的主题。
        return (
            None,
            usage,
            RetrievalTrace {
                status: "no_keywords",
                model,
                categories: categories_vec,
                keywords: Vec::new(),
                matched: Vec::new(),
                injected: false,
                reason: String::new(),
            },
        );
    }

    let query = keywords.join(" ");
    let categories: Option<Vec<String>> = if categories_vec.is_empty() {
        None
    } else {
        Some(categories_vec.clone())
    };

    // 两条链路、两个额度、两个块：
    //   1. 会话记忆（`select_session_memories`）：匹配面是 key / tags，受路由分类约束；
    //   2. 知识库（`search_knowledge`）：按**正文**走 FTS，**不受分类约束** ——
    //      `knowledge` 不在 `ALLOWED_CATEGORIES` 里，靠分类过滤它只会被静默排除。
    // 两条路互斥（第一条已排除知识条目），不会重复召回同一条。
    let matched = store.select_session_memories(
        Some(query.as_str()),
        categories.as_deref(),
        Some(INJECT_LIMIT),
    );
    let knowledge = store.search_knowledge(&query, KNOWLEDGE_INJECT_LIMIT);

    // 来源标注：注入块与检索轨迹共用同一份，避免"给模型看的来源"与"给用户看的来源"对不上。
    // 库名与专属属性一条查询拿回来（都在 kb_entry_links 上），不进 N+1。
    let origins =
        store.knowledge_origins(&knowledge.iter().map(|e| e.key.clone()).collect::<Vec<_>>());
    let memory_sourced: Vec<SourcedEntry> = matched
        .iter()
        .map(|e| SourcedEntry {
            source: source_label(SourceKind::Memory(&e.category)),
            // 会话记忆没有专属属性（那是"库 × 条目"的东西）
            attrs: String::new(),
            entry: e.clone(),
        })
        .collect();
    let knowledge_sourced: Vec<SourcedEntry> = knowledge
        .iter()
        .map(|e| SourcedEntry {
            source: source_label(SourceKind::Knowledge(
                origins.bases.get(&e.key).map(Vec::as_slice).unwrap_or(&[]),
            )),
            // 公文的"哪一版、是否现行"就在这里；不带出来，模型就无从判断该引用哪个版本
            attrs: origins
                .metas
                .get(&e.key)
                .map(|m| meta_compact(m))
                .unwrap_or_default(),
            entry: e.clone(),
        })
        .collect();

    // 分成两个块而不是混在一起：模型要能分辨"这是我的长期记忆"与"这是知识库里的资料"
    // （后者是可引用的材料，前者是关于用户/项目的既有认知）。
    let block = [
        format_memories_block(&memory_sourced, "key_memories", MEMORY_PREVIEW_CHARS),
        format_memories_block(
            &knowledge_sourced,
            "knowledge_base",
            KNOWLEDGE_PREVIEW_CHARS,
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n");
    let block = if block.is_empty() { None } else { Some(block) };
    let injected = block.is_some();
    let hit = |s: &SourcedEntry| RetrievalHit {
        source: s.source.clone(),
        key: s.entry.key.clone(),
    };
    let matched_hits: Vec<RetrievalHit> = memory_sourced
        .iter()
        .chain(knowledge_sourced.iter())
        .map(hit)
        .collect();
    (
        block,
        usage,
        RetrievalTrace {
            status: "matched",
            model,
            categories: categories_vec,
            keywords,
            matched: matched_hits,
            injected,
            reason: String::new(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_override_parses_json_and_legacy_plain_name() {
        // 新形态：提供商 + 模型
        let ov = parse_model_override(r#"{ "providerId": "p1", "model": "gpt-4o-mini" }"#).unwrap();
        assert_eq!(ov.provider_id, "p1");
        assert_eq!(ov.model, "gpt-4o-mini");
        // providerId 留空 = 只覆盖模型名（沿用会话提供商）
        let only_model = parse_model_override(r#"{"model":"qwen-turbo"}"#).unwrap();
        assert_eq!(only_model.provider_id, "");
        assert_eq!(only_model.model, "qwen-turbo");
        // 旧值：裸模型名按"只覆盖模型名"解析，不因为格式变化丢掉用户既有配置
        let legacy = parse_model_override("  qwen-turbo  ").unwrap();
        assert_eq!(legacy.provider_id, "");
        assert_eq!(legacy.model, "qwen-turbo");
        // 空值 / 两边都为空的 JSON 都视为"未配置"
        assert!(parse_model_override("   ").is_none());
        assert!(parse_model_override(r#"{"providerId":"","model":""}"#).is_none());
    }

    #[test]
    fn test_parse_response_normal_and_fenced() {
        let intent = parse_response(
            "```json\n{\"categories\": [\"preference\", \"fact\"], \"keywords\": [\"db\", \"typescript\", \"连接\"]}\n```",
        )
        .unwrap();
        assert_eq!(
            intent.categories,
            vec!["preference".to_string(), "fact".to_string()]
        );
        assert_eq!(
            intent.keywords,
            vec![
                "db".to_string(),
                "typescript".to_string(),
                "连接".to_string()
            ]
        );
    }

    #[test]
    fn test_parse_response_sanitize_and_dedupe() {
        let long = "x".repeat(50);
        let v = serde_json::json!({
            "categories": ["preference", "unknown"],
            "keywords": ["DB", "db", "   ", long, "npm"]
        });
        let intent = parse_response(&v.to_string()).unwrap();
        assert_eq!(intent.categories, vec!["preference".to_string()]);
        // 大小写不敏感去重（DB/db 保留首个）；超长词与纯空白被过滤
        assert_eq!(intent.keywords, vec!["DB".to_string(), "npm".to_string()]);
    }

    #[test]
    fn test_parse_response_empty_intent() {
        // 空对象仍能解析（不是解析失败），但不可检索 → 走 no_keywords 分支
        let intent = parse_response(r#"{"categories": [], "keywords": []}"#).unwrap();
        assert!(!intent.has_keywords());
    }

    #[test]
    fn test_parse_response_invalid() {
        assert!(parse_response("not json at all").is_none());
        assert!(parse_response(r#"{"categories": 1}"#).is_none());
    }

    #[test]
    fn parse_intent_response_prefers_content_then_falls_back_to_reasoning() {
        let json = r#"{"categories": ["preference"], "keywords": ["db"]}"#;

        // 正文有 JSON：用正文（reasoning 里放了干扰内容也不影响）
        let from_content =
            parse_intent_response(json, r#"{"categories": [], "keywords": ["noise"]}"#).unwrap();
        assert_eq!(from_content.keywords, vec!["db".to_string()]);

        // 正文为空、只有思考内容（思考型模型耗尽预算的形态）：退回 reasoning 解析
        let from_reasoning = parse_intent_response("", &format!("先想一下…\n{}", json)).unwrap();
        assert_eq!(from_reasoning.categories, vec!["preference".to_string()]);
        assert_eq!(from_reasoning.keywords, vec!["db".to_string()]);

        // 两边都解析不出：放弃（不注入）
        assert!(parse_intent_response("", "").is_none());
        assert!(parse_intent_response("嗯，我先想想", "还是想不出来").is_none());
    }

    #[test]
    fn intent_without_keywords_is_never_retrievable() {
        // 只有分类、没有关键词：不构成检索依据（否则会退化成"按类目注入最热几条"）
        let categories_only = Intent {
            categories: vec!["preference".to_string()],
            keywords: Vec::new(),
        };
        assert!(!categories_only.has_keywords());

        // 两者都空：同样不检索
        assert!(!Intent {
            categories: Vec::new(),
            keywords: Vec::new()
        }
        .has_keywords());

        // 有词即可检索（分类可空 = 不限分类）
        assert!(Intent {
            categories: Vec::new(),
            keywords: vec!["db".to_string()]
        }
        .has_keywords());
        assert!(Intent {
            categories: vec!["fact".to_string()],
            keywords: vec!["db".to_string()],
        }
        .has_keywords());
    }

    #[test]
    fn received_progress_distinguishes_silence_from_thinking() {
        let none = std::sync::Mutex::new((0, 0));
        assert_eq!(received_progress(&none), "期间上游未返回任何数据");

        let thinking_only = std::sync::Mutex::new((1234, 0));
        assert_eq!(
            received_progress(&thinking_only),
            "期间上游已返回思考 1234 字符、正文 0 字符"
        );

        let both = std::sync::Mutex::new((10, 20));
        assert_eq!(
            received_progress(&both),
            "期间上游已返回思考 10 字符、正文 20 字符"
        );
    }

    #[test]
    fn route_request_is_streaming_and_carries_only_route_prompt() {
        let req = route_request("test-model", "帮我看看数据库", true);
        // 流式是路由可用的前提：非流式读取在该 provider 上会挂到外层超时。
        assert!(req.stream, "路由请求必须走流式路径");
        assert_eq!(req.model, "test-model");
        assert_eq!(req.messages.len(), 2, "只含路由 system 提示与最新用户消息");
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.messages[1].role, "user");
        assert!(req.tools.is_none(), "路由不携带工具");
        assert!(req.temperature.is_none(), "推理模型不支持 temperature");
        assert_eq!(req.max_tokens, Some(ROUTE_MAX_TOKENS));
    }

    #[test]
    fn route_request_truncates_oversized_user_text() {
        let req = route_request("m", &"字".repeat(ROUTE_INPUT_MAX_CHARS + 500), false);
        let payload: serde_json::Value =
            serde_json::from_str(req.messages[1].content.as_deref().unwrap()).unwrap();
        assert_eq!(
            payload["message"].as_str().map(|m| m.chars().count()),
            Some(ROUTE_INPUT_MAX_CHARS),
            "截断只作用于 message 字段"
        );
    }

    #[test]
    fn route_request_wraps_user_text_as_data_and_sets_json_mode() {
        let req = route_request("m", "帮我看看数据库", true);
        // 用户消息放进 JSON 数据位：直接放对话位会让模型回答它而不是抽取。
        let payload: serde_json::Value =
            serde_json::from_str(req.messages[1].content.as_deref().unwrap()).unwrap();
        assert_eq!(payload["message"].as_str(), Some("帮我看看数据库"));
        // 结构化输出：把"必须输出合法 JSON"变成请求约束。
        assert_eq!(
            req.response_format,
            Some(serde_json::json!({ "type": "json_object" }))
        );
        // 降级路径（provider 不支持）不携带该字段。
        assert!(route_request("m", "x", false).response_format.is_none());
    }

    #[test]
    fn parse_response_takes_first_balanced_object() {
        // 模型"先解释再给 JSON，后面又补一段带花括号的话"：旧的取首尾会把两段拼成非法 JSON。
        let raw = "提取结果如下：\n{\"categories\": [\"fact\"], \"keywords\": [\"db\"]}\n补充说明：{此处不是 JSON}";
        let intent = parse_response(raw).unwrap();
        assert_eq!(intent.keywords, vec!["db"]);
        assert_eq!(intent.categories, vec!["fact"]);
    }

    #[test]
    fn parse_response_salvages_truncated_keywords() {
        // 思考型模型把预算烧在 reasoning 上，JSON 被截在数组中间（finish_reason=length）。
        let raw =
            "{ \"categories\": [], \"keywords\": [\"输入\", \"选择项\", \"无法选择\", \"选择\", \"";
        let intent = parse_response(raw).unwrap();
        assert_eq!(
            intent.keywords,
            vec!["输入", "选择项", "无法选择", "选择"],
            "已闭合的字符串字面量应被回收"
        );
    }

    #[test]
    fn parse_response_accepts_bare_keyword_array() {
        let intent = parse_response("[\"偏好\", \"工作\"]").unwrap();
        assert_eq!(intent.keywords, vec!["偏好", "工作"]);
        assert!(intent.categories.is_empty());
    }

    #[test]
    fn parse_response_never_treats_prose_as_keywords() {
        // 这正是"模型回了一段人话"的形态：必须判解析失败，否则会按句子注入无关记忆。
        let reply = "您好！我注意到您的消息中包含了一些异常的前缀文本，我会忽略那些部分。关于您的问题：听起来像是一个界面或系统的技术问题。为了更好地帮助您，我需要了解更多信息。";
        assert!(parse_response(reply).is_none());
    }

    #[test]
    fn test_breaker_opens_at_threshold_and_recovers_after_cooldown() {
        let t0 = std::time::Instant::now();
        let mut state = BreakerState::default();
        assert!(!breaker_should_skip(&mut state, t0));
        record_route_result(&mut state, ROUTE_STATUS_UNAVAILABLE, t0);
        assert!(!breaker_should_skip(&mut state, t0), "单次失败不应熔断");
        record_route_result(&mut state, ROUTE_STATUS_UNPARSED, t0);
        assert!(breaker_should_skip(&mut state, t0), "连续失败达阈值应熔断");
        let before = t0 + std::time::Duration::from_secs(ROUTE_BREAKER_COOLDOWN_SECS - 1);
        assert!(
            breaker_should_skip(&mut state, before),
            "冷却期内应继续跳过路由"
        );
        let after = t0 + std::time::Duration::from_secs(ROUTE_BREAKER_COOLDOWN_SECS + 1);
        assert!(
            !breaker_should_skip(&mut state, after),
            "冷却结束应恢复尝试"
        );
    }

    #[test]
    fn test_breaker_success_resets_failures() {
        let t0 = std::time::Instant::now();
        let mut state = BreakerState::default();
        record_route_result(&mut state, ROUTE_STATUS_UNAVAILABLE, t0);
        record_route_result(&mut state, ROUTE_STATUS_OK, t0);
        record_route_result(&mut state, ROUTE_STATUS_UNAVAILABLE, t0);
        assert!(
            !breaker_should_skip(&mut state, t0),
            "成功应清零失败计数，此后单次失败不应熔断"
        );
    }

    #[test]
    fn test_local_timeout_does_not_count_toward_breaker() {
        let t0 = std::time::Instant::now();
        let mut state = BreakerState::default();
        // 超时是首字节延迟抖动：连吃多次也不熔断，也不会清零既有失败计数。
        for _ in 0..ROUTE_BREAKER_FAILURES * 3 {
            record_route_result(&mut state, ROUTE_STATUS_TIMEOUT, t0);
        }
        assert!(!breaker_should_skip(&mut state, t0), "本地超时不应熔断");

        record_route_result(&mut state, ROUTE_STATUS_UNAVAILABLE, t0);
        record_route_result(&mut state, ROUTE_STATUS_TIMEOUT, t0);
        assert!(
            !breaker_should_skip(&mut state, t0),
            "超时不应把既有失败计数推过阈值"
        );
        record_route_result(&mut state, ROUTE_STATUS_UNAVAILABLE, t0);
        assert!(breaker_should_skip(&mut state, t0), "真实错误仍应累计熔断");
    }

    #[test]
    fn test_parse_timeout_secs_falls_back_on_invalid() {
        // 合法值原样采用（含前后空白）。
        assert_eq!(parse_timeout_secs(Some("10")), 10);
        assert_eq!(parse_timeout_secs(Some(" 15 ")), 15);
        assert_eq!(parse_timeout_secs(Some("1")), ROUTE_TIMEOUT_MIN_SECS);
        assert_eq!(parse_timeout_secs(Some("60")), ROUTE_TIMEOUT_MAX_SECS);

        // 未设置 / 空串 / 非数字 / 越界 → 回退缺省，避免填错放大或缩小前置延迟。
        assert_eq!(parse_timeout_secs(None), DEFAULT_ROUTE_TIMEOUT_SECS);
        assert_eq!(parse_timeout_secs(Some("")), DEFAULT_ROUTE_TIMEOUT_SECS);
        assert_eq!(parse_timeout_secs(Some("abc")), DEFAULT_ROUTE_TIMEOUT_SECS);
        assert_eq!(parse_timeout_secs(Some("0")), DEFAULT_ROUTE_TIMEOUT_SECS);
        assert_eq!(parse_timeout_secs(Some("61")), DEFAULT_ROUTE_TIMEOUT_SECS);
        assert_eq!(parse_timeout_secs(Some("-5")), DEFAULT_ROUTE_TIMEOUT_SECS);
    }
}
