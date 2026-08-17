//! 网页抓取工具：抓取指定 URL 的网页并提取可读文本。
//!
//! 自 `api_agent/web_fetch.rs` 迁入（工具架构统一 v1.0，轮 6），逻辑不变。

use async_trait::async_trait;

use crate::api_agent::web::fetch_web_text;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

/// 网页抓取工具
pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "抓取指定 URL 的网页内容并提取可读文本。适用于阅读文章、提取网页正文、\
获取某个链接的详细内容等场景；无法绕过登录或验证码。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "要抓取的网页 URL，必须以 http:// 或 https:// 开头"
                }
            },
            "required": ["url"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Web, ToolTag::Network, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let url = arguments["url"].as_str().ok_or("缺少 url 参数")?;
        fetch_web_text(url).await
    }
}
