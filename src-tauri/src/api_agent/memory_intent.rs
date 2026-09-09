//! KV 记忆「意图路由注入」：每轮组装 system prompt 前，用一次轻量 LLM 调用把
//! 用户最新消息解析为 分类数组 + 关键词数组，再在 key_memories 上做只读分域检索
//! （复用 db::select_memories 的统一评分排序），命中即注入、0 命中则不注入。
//!
//! 开关：app_settings `memory_intent_enabled`（缺省视为启用；置 "0"/"false" 关闭）。
//! 模型覆盖：app_settings `memory_intent_model`（缺省用当前会话模型）。
//! 失败降级：LLM/网络/格式不支持 → 回退 ranked-top 注入（format_for_prompt）；
//! 解析成功但意图无记忆需求（空 categories 且空 keywords）→ 0 命中，不注入。

use crate::api_agent::db::{format_memories_block, MemoryStore};
use crate::api_agent::types::UsageRecord;

/// app_settings key：意图路由开关（"1"/"true" 开；其余关；缺省视为开）。
pub const SETTING_ENABLED: &str = "memory_intent_enabled";
/// app_settings key：意图路由模型覆盖（缺省使用当前会话模型）。
pub const SETTING_MODEL: &str = "memory_intent_model";

const ALLOWED_CATEGORIES: [&str; 4] = ["fact", "preference", "skill", "event"];
/// 每次注入最多返回的记忆条数。
const INJECT_LIMIT: usize = 5;
/// 意图路由短调用的最大输出 token。
const ROUTE_MAX_TOKENS: u32 = 300;
/// 意图路由请求超时（秒），超时即按失败降级。
const ROUTE_TIMEOUT_SECS: u64 = 8;

/// 意图路由 prompt（稳定不变，输入仅替换为最新用户消息）。
const ROUTE_SYSTEM: &str = "\
你是记忆检索意图路由器。给定用户的最新消息，判断此刻需要回忆哪些长期记忆（KV 记忆，存于 key/tags/分类）。
输出要求：
- categories：从 fact/preference/skill/event 中选，无倾向则为空数组；
- keywords：列出 1~12 个“能在记忆 key 或 tags 中出现的实体词/缩写/技术栈/路径片段”，
  要求词面具体、贴近存储命名（如 db、typescript、npm），不要输出概括性描述或完整句子。
只输出 JSON：{\"categories\": [...], \"keywords\": [...]}，不要任何其它文字。";

#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
}

impl Intent {
    fn is_empty(&self) -> bool {
        self.categories.is_empty() && self.keywords.is_empty()
    }
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

/// 意图路由模型覆盖（同步读取；None 表示使用当前会话模型）。
pub fn read_model_override(conn: &rusqlite::Connection) -> Option<String> {
    crate::commands::app_settings::get_setting(conn, SETTING_MODEL)
        .ok()
        .flatten()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

fn parse_response(raw: &str) -> Option<Intent> {
    let text = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    // 容错：若模型夹带说明文字，取首个 `{` 到末尾 `}` 的片段
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let json: serde_json::Value = serde_json::from_str(&text[start..=end]).ok()?;

    // 字段存在但类型错误（非数组）视为解析失败，避免把损坏输出当空意图
    let cats_field = json.get("categories");
    if cats_field.is_some() && cats_field.and_then(|v| v.as_array()).is_none() {
        return None;
    }
    let kws_field = json.get("keywords");
    if kws_field.is_some() && kws_field.and_then(|v| v.as_array()).is_none() {
        return None;
    }

    let mut categories: Vec<String> = cats_field
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| ALLOWED_CATEGORIES.contains(&s.as_str()))
                .collect()
        })
        .unwrap_or_default();

    let mut keywords: Vec<String> = kws_field
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s.chars().count() <= 40)
                .collect()
        })
        .unwrap_or_default();

    // 去重（大小写不敏感）保序并截断数量上限
    let mut seen = std::collections::HashSet::new();
    keywords.retain(|k| seen.insert(k.to_lowercase()));
    keywords.truncate(12);
    categories.truncate(4);

    Some(Intent { categories, keywords })
}

/// 从 OpenAI 兼容非流式响应解析用量（`total_tokens` 缺失时以 prompt+completion 兜底；
/// 缓存读取兼容顶层 prompt_cache_hit_tokens 与 prompt_tokens_details.cached_tokens 两种位置）。
fn usage_from_openai_value(v: &serde_json::Value) -> Option<UsageRecord> {
    let prompt = v["prompt_tokens"].as_u64()? as u32;
    let completion = v["completion_tokens"].as_u64().unwrap_or(0) as u32;
    let cache_read = v["prompt_cache_hit_tokens"]
        .as_u64()
        .or_else(|| v["prompt_tokens_details"]["cached_tokens"].as_u64())
        .unwrap_or(0) as u32;
    Some(UsageRecord {
        prompt,
        completion,
        total: v["total_tokens"].as_u64().unwrap_or((prompt + completion) as u64) as u32,
        cache_read,
        cache_write: 0,
    })
}

/// 调用 OpenAI 兼容非流式接口获取意图 JSON；anthropic 格式或请求失败返回 (None, None)（走降级）。
/// 返回 (意图, 本次调用的用量)：请求成功但内容解析失败时仍回传用量，供调用方如实计费。
async fn route_via_llm(
    api_format: &str,
    endpoint: &str,
    api_key: &str,
    model: &str,
    user_text: &str,
) -> (Option<Intent>, Option<UsageRecord>) {
    if api_format.to_lowercase().contains("anthropic") || endpoint.trim().is_empty() || model.is_empty() {
        return (None, None);
    }
    let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "temperature": 0.0,
        "max_tokens": ROUTE_MAX_TOKENS,
        "messages": [
            { "role": "system", "content": ROUTE_SYSTEM },
            { "role": "user", "content": user_text }
        ]
    });

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(ROUTE_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(_) => return (None, None),
    };
    if !resp.status().is_success() {
        return (None, None);
    }
    let value: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let usage = value.get("usage").and_then(usage_from_openai_value);
    let content = value["choices"][0]["message"]["content"].as_str();
    match content {
        Some(c) => (parse_response(c), usage),
        None => (None, usage),
    }
}

/// 组装 KV 记忆注入块（异步部分只接收标量与 store，避免连接引用跨 await 导致 !Send）：
/// - 未启用 / anthropic 格式 / 路由失败 → fallback（ranked-top 注入）；
/// - 路由成功但无意图（空 categories 且空 keywords）→ 0 命中，不注入；
/// - 有意图 → select_memories 只读检索 + 统一排序，注入块或 0 命中不注入。
///
/// 返回 (注入块, 意图路由调用用量)：调用方据用量把本次短调用计入会话用量统计。
pub async fn memory_injection_block(
    enabled: bool,
    model_override: Option<String>,
    api_format: &str,
    endpoint: &str,
    api_key: &str,
    session_model: &str,
    user_text: &str,
    store: &MemoryStore,
) -> (Option<String>, Option<UsageRecord>) {
    if user_text.trim().is_empty() || !enabled {
        return (store.format_for_prompt(INJECT_LIMIT), None);
    }

    let model = model_override.unwrap_or_else(|| session_model.to_string());

    let (intent, usage) = route_via_llm(api_format, endpoint, api_key, &model, user_text).await;
    let Some(intent) = intent else {
        // 路由不可用：回退统一评分 top-N，保证基础记忆能力不因路由失败而丢
        log::info!("[MemoryIntent] 意图路由不可用，回退 ranked-top 注入");
        return (store.format_for_prompt(INJECT_LIMIT), usage);
    };

    if intent.is_empty() {
        // 意图明确表示无需回忆记忆：0 命中，不注入
        return (None, usage);
    }

    let query = if intent.keywords.is_empty() {
        None
    } else {
        Some(intent.keywords.join(" "))
    };
    let categories: Option<Vec<String>> = if intent.categories.is_empty() {
        None
    } else {
        Some(intent.categories)
    };

    let matched = store.select_memories(query.as_deref(), categories.as_deref(), Some(INJECT_LIMIT));
    (format_memories_block(&matched), usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_response_normal_and_fenced() {
        let intent = parse_response(
            "```json\n{\"categories\": [\"preference\", \"fact\"], \"keywords\": [\"db\", \"typescript\", \"连接\"]}\n```",
        )
        .unwrap();
        assert_eq!(intent.categories, vec!["preference".to_string(), "fact".to_string()]);
        assert_eq!(intent.keywords, vec!["db".to_string(), "typescript".to_string(), "连接".to_string()]);
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
        let intent = parse_response(r#"{"categories": [], "keywords": []}"#).unwrap();
        assert!(intent.is_empty());
    }

    #[test]
    fn test_parse_response_invalid() {
        assert!(parse_response("not json at all").is_none());
        assert!(parse_response(r#"{"categories": 1}"#).is_none());
    }
}
