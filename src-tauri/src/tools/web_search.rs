//! 联网搜索工具：将关键词查询分发给 web.rs 中的搜索后端，返回结构化文本。
//!
//! 自 `api_agent/web_search.rs` 迁入（工具架构统一 v1.0，轮 6），逻辑不变。

use async_trait::async_trait;

use crate::api_agent::web::{search_web, SearchConfig};
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

/// 联网搜索工具
pub struct WebSearchTool {
    config: SearchConfig,
}

impl WebSearchTool {
    pub fn new(config: SearchConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ToolHandler for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "联网搜索，返回相关网页的标题、URL 与摘要。适用于需要最新信息、事实核查、\
查询当前事件或网页资料等场景；默认使用 Bing 中国版（无需配置），也可在设置中切换到 \
Tavily 或 Bing API。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词（自然语言或关键词均可）"
                },
                "num": {
                    "type": "integer",
                    "description": "返回结果数量，默认 5，最大 10"
                }
            },
            "required": ["query"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Web, ToolTag::Network, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let query = arguments["query"].as_str().ok_or("缺少 query 参数")?;
        let num = arguments["num"].as_u64().unwrap_or(5).clamp(1, 10) as usize;

        let results = search_web(&self.config, query, num).await?;
        if results.is_empty() {
            return Ok("未找到相关搜索结果。".to_string());
        }

        let text = results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let snippet = if r.snippet.is_empty() { "(无摘要)" } else { &r.snippet };
                format!("{}. {}\nURL: {}\n{}\n", i + 1, r.title, r.url, snippet)
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(text)
    }
}
