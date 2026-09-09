//! 模型能力查询工具：把各 API Provider 的模型清单（含用户备注）返回给 LLM。
//!
//! 设计要点：
//! - **决策交给 LLM**：不做程序化能力过滤，返回全量清单，由当前会话 LLM 结合模型名与备注
//!   自行选择目标模型，再传给对应的生成工具（generate_image 等）。
//! - **安全红线**：返回内容只含 provider 名称/endpoint/模型名/备注，**绝不含 API key**；
//!   key 由各生成工具在后端内部持有并使用。

use crate::tools::{ProviderModelInfo, RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::Arc;

/// 模型能力查询工具
pub struct ListModelsTool {
    list: Arc<dyn Fn() -> Vec<ProviderModelInfo> + Send + Sync>,
}

impl ListModelsTool {
    pub fn new(list: Arc<dyn Fn() -> Vec<ProviderModelInfo> + Send + Sync>) -> Self {
        Self { list }
    }
}

#[async_trait]
impl ToolHandler for ListModelsTool {
    fn name(&self) -> &str {
        "list_models"
    }

    fn description(&self) -> &str {
        "列出用户配置的所有可用模型（含提供商、接口地址、模型名与用途备注）。当需要生成图片/视频/音频等而当前模型可能不支持时，先调用本工具查看可用的专用模型，再结合任务类型选择最合适的模型名，并传给对应的生成工具（如 generate_image 的 model/provider 参数）。返回内容不含任何密钥。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Read, ToolTag::Network]
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<String, String> {
        let providers = (self.list)();
        if providers.is_empty() {
            return Ok("当前未配置任何可用模型。".to_string());
        }
        let mut out = String::from(
            "可用模型清单（provider 参数必须逐字使用 provider_id 原始值；provider_name 仅为显示名，禁止用于任何参数）：\n",
        );
        for p in &providers {
            out.push_str(&format!(
                "■ provider_id: {}（provider_name: {}）| api_format: {} | 接口: {}\n",
                p.provider_id, p.provider_name, p.api_format, p.endpoint
            ));
            for m in &p.models {
                let desc = m.description.as_deref().unwrap_or("（未备注）");
                let mark = if is_non_text_model(&m.name, desc) {
                    " ⚠️非文本（疑似图片/视频/音频/向量类，严禁用于文本/对话任务）"
                } else {
                    ""
                };
                out.push_str(&format!("    - 模型: {} | 备注: {}{}\n", m.name, desc, mark));
            }
        }
        out.push_str(
            "提示：调用生成/语音/向量化等工具时，provider 参数必须逐字使用上方 provider_id（禁止使用 provider_name/中文名/序号/联想值）；model 参数必须与上方模型名完全一致（包括 organization/ 等 namespace 前缀），禁止自行缩短、合并或省略前缀；仅可使用清单中列出的模型，绝对不要编造清单外的模型名；标注 ⚠️非文本 的模型严禁用于文本/对话任务。\n",
        );
        Ok(out)
    }
}

/// 启发式识别疑似非文本模型（名称或备注命中图片/视频/音频/向量关键词）。
/// 仅用于清单标注警示，不作为运行时硬校验。
fn is_non_text_model(name: &str, desc: &str) -> bool {
    const NON_TEXT_KEYWORDS: &[&str] = &[
        "image", "img", "vision", "dall", "dalle", "sd-", "flux", "video",
        "tts", "stt", "audio", "speech", "embed", "vector",
        "图片", "图像", "视频", "音频", "语音", "向量", "嵌入",
    ];
    let haystack = format!("{} {}", name, desc).to_lowercase();
    NON_TEXT_KEYWORDS.iter().any(|k| haystack.contains(k))
}
