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
        "保存一条键值对记忆到持久化知识库。格式：key=名称，value=内容，category=分类（fact/preference/skill/event）"
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
        let entry = self.store.save_memory(key, value, category);
        Ok(format!(
            "记忆已保存: [{}] {} = {}",
            entry.category, entry.key, entry.value
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
        "在记忆库中搜索键值对。返回匹配的所有记忆，按访问频率排序。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词（匹配 key 和 value）"
                }
            },
            "required": ["query"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let query = arguments["query"].as_str().ok_or("缺少 query 参数")?;
        let results = self.store.search_memory(query);
        if results.is_empty() {
            Ok("未找到匹配的记忆。".to_string())
        } else {
            let formatted: Vec<String> = results
                .iter()
                .map(|e| format!("- [{}] {}: {}", e.category, e.key, e.value))
                .collect();
            Ok(format!(
                "找到 {} 条记忆：\n{}",
                results.len(),
                formatted.join("\n")
            ))
        }
    }
}
