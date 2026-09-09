//! KV 记忆工具（会话模式专属）。
//!
//! 迁移自 `lib.rs`（工具架构统一 v1.0，轮 6）：从 `builtin_tool!` 宏改写为具名 struct。

use crate::api_agent::db::MemoryStore;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 保存键值对记忆
pub struct SaveMemoryTool {
    store: MemoryStore,
}

impl SaveMemoryTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for SaveMemoryTool {
    fn name(&self) -> &str {
        "save_memory"
    }

    fn description(&self) -> &str {
        "保存一条键值对记忆到持久化知识库。格式：key=名称，value=内容，category=分类（fact/preference/skill/event），important=是否重要（重要记忆不会被自动清理），tags=逗号分隔的检索标签（如 java,构建工具），便于日后按意图检索到该记忆"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "记忆的键（唯一标识，如 project_language）"
                },
                "value": {
                    "type": "string",
                    "description": "记忆的值（要保存的内容）"
                },
                "category": {
                    "type": "string",
                    "description": "记忆分类：fact（事实）、preference（偏好）、skill（技能）、event（事件）",
                    "enum": ["fact", "preference", "skill", "event"]
                },
                "important": {
                    "type": "boolean",
                    "description": "是否为重要记忆（默认 false；true 时记忆置顶保护，自动清理永不删除）"
                },
                "tags": {
                    "type": "string",
                    "description": "可选：检索标签，多个用逗号/顿号分隔（如 db,连接配置）；存储时统一为英文逗号"
                }
            },
            "required": ["key", "value", "category"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Write]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let key = arguments["key"].as_str().ok_or("缺少 key 参数")?;
        let value = arguments["value"].as_str().ok_or("缺少 value 参数")?;
        let category = arguments["category"].as_str().ok_or("缺少 category 参数")?;
        let important = arguments["important"].as_bool().unwrap_or(false);
        let tags = arguments["tags"].as_str().unwrap_or("");

        // 写入前校验（与存储/检索预算一致），防止模型写入超长或非法内容：
        // key/value/tags 长度、category 枚举（后续按此分域检索）。
        let key = key.trim();
        let value = value.trim();
        let category = category.trim();
        if key.is_empty() || key.chars().count() > 200 {
            return Err("记忆 key 不能为空且不超过 200 字符".to_string());
        }
        if value.is_empty() || value.chars().count() > 4000 {
            return Err(format!(
                "记忆 value 不能为空且不超过 4000 字符（当前 {} 字符）；请精简后再保存",
                value.chars().count()
            ));
        }
        if !matches!(category, "fact" | "preference" | "skill" | "event") {
            return Err(format!("记忆分类必须为 fact/preference/skill/event 之一（当前: {}）", category));
        }
        if tags.chars().count() > 200 {
            return Err("记忆 tags 不能超过 200 字符".to_string());
        }

        let entry = self.store.save_memory(key, value, category, important, tags);
        let tags_label = if entry.tags.is_empty() { "无".to_string() } else { entry.tags.clone() };
        let pin_label = if entry.pin { "是（自动清理永不删除）" } else { "否" };
        Ok(format!(
            "记忆已保存: [{}] {} = {}\n标签: {}；pin 保护: {}。\n提示：同名 key 再次保存会覆盖内容并刷新最近编辑时间，但不会增加访问热度。",
            entry.category, entry.key, entry.value, tags_label, pin_label
        ))
    }
}

/// 搜索键值对记忆
pub struct SearchMemoryTool {
    store: MemoryStore,
}

impl SearchMemoryTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for SearchMemoryTool {
    fn name(&self) -> &str {
        "search_memory"
    }

    fn description(&self) -> &str {
        "在记忆库中检索已保存的键值对记忆并返回内容。调用前必须先提供下列至少一个检索条件：query=话题关键词（匹配记忆的 key 与标签 tags，不使用 value 匹配），或 categories=分类数组（fact/preference/skill/event，仅在这些分类内检索）。未提供这两个参数时不会返回任何记忆内容，只会返回 key 索引并提示补充。命中按 重要置顶+热度(含30天半衰期)+命中加权 排序。默认最多返回 10 条（可用 limit 调整，上限 50），并受总字符预算保护以防撑爆上下文。如果不确定要查询的主题，不要省略参数调用，请直接询问用户想查询的偏好或事实。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "可选：检索关键词，匹配记忆的 key 与 tags（支持多个词，用空格或中英文逗号/顿号分隔）"
                },
                "categories": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["fact", "preference", "skill", "event"] },
                    "description": "可选：限定的分类数组（如 [\"preference\"]），只在这些分类内检索"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "可选：最多返回条数（默认不限制）"
                }
            },
            "required": []
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let query = arguments["query"].as_str().map(str::trim).filter(|q| !q.is_empty());
        let categories: Option<Vec<String>> = arguments["categories"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from))
                    .collect()
            })
            .filter(|c: &Vec<String>| !c.is_empty());
        // 无检索条件 = 模型漏参。不吐 value、不硬报错：仅返回 key+分类索引并引导补参，
        // 让模型能立即用正确参数重试，同时避免把无关个人记忆塞进上下文。
        if query.is_none() && categories.is_none() {
            const INDEX_CAP: usize = 30;
            let all = self.store.list_all(None, None);
            if all.is_empty() {
                return Ok("记忆库为空，暂无已保存的记忆；需要长期记住的信息可调用 save_memory 保存。".to_string());
            }
            let index: Vec<String> = all
                .iter()
                .take(INDEX_CAP)
                .map(|e| format!("- [{}] {}", e.category, e.key))
                .collect();
            let mut hint = format!(
                "search_memory 需提供 query（话题关键词）或 categories（如 [\"preference\"]）才会返回记忆内容；本次未提供，故仅列出 key 索引（不含内容）。记忆库共 {} 条：\n{}",
                self.store.count(),
                index.join("\n")
            );
            if all.len() > INDEX_CAP {
                hint.push_str(&format!("\n…其余 {} 条未列出。", all.len() - INDEX_CAP));
            }
            hint.push_str("\n请补充 query 或 categories 后重新调用；不确定查询主题时，请先询问用户想查询的偏好或事实。");
            return Ok(hint);
        }

        // 默认最多 10 条，防止未显式传 limit 时一次返回过多撑爆上下文
        const DEFAULT_LIMIT: usize = 10;
        const MAX_LIMIT: usize = 50;
        let limit = arguments["limit"]
            .as_u64()
            .map(|n| (n as usize).min(MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);

        let results = self.store.search_memory(query, categories.as_deref(), Some(limit));
        if results.is_empty() {
            return Ok("未找到匹配的记忆。".to_string());
        }

        // 返回文本预算：单条 value 预览 + 总字符上限，超出即截断并明示
        const VALUE_PREVIEW_CHARS: usize = 600;
        const MAX_OUTPUT_CHARS: usize = 8000;
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0usize;
        for e in &results {
            let preview: String = if e.value.chars().count() > VALUE_PREVIEW_CHARS {
                let head: String = e.value.chars().take(VALUE_PREVIEW_CHARS).collect();
                format!("{}…", head)
            } else {
                e.value.clone()
            };
            let tags = if e.tags.is_empty() { String::new() } else { format!(" [tags: {}]", e.tags) };
            let line = format!("- [{}] {}: {}{}", e.category, e.key, preview, tags);
            used += line.chars().count();
            if used > MAX_OUTPUT_CHARS && !lines.is_empty() {
                break;
            }
            lines.push(line);
        }

        let shown = lines.len();
        let mut out = format!("找到 {} 条记忆：\n{}", results.len(), lines.join("\n"));
        if shown < results.len() {
            out.push_str(&format!("\n（结果过长，仅显示前 {} 条，可缩小 limit 或细化关键词）", shown));
        }
        Ok(out)
    }
}
