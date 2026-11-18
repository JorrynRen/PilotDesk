//! 图片模型能力查询：只读、无网络、无副作用。
//!
//! 数据完全来自当前生效的图片清单（[`super::image_manifest::capability`]），
//! 不内联任何厂商能力。

use async_trait::async_trait;
use serde_json::Value;

use crate::tools::image_manifest;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

/// 图片模型能力查询工具
pub struct ListImageCapabilitiesTool;

#[async_trait]
impl ToolHandler for ListImageCapabilitiesTool {
    fn name(&self) -> &str {
        "list_image_capabilities"
    }

    fn description(&self) -> &str {
        "查询某个图片模型支持的参数槽位与限制（尺寸、生成数量、参考图与遮罩）。\
         只读、无网络。在不确定模型是否支持参考图/遮罩时先调用本工具，再调用 generate_image；\
         返回的 unsupported 列表即该模型会被本地拒绝的参数。\
         能力随图片清单版本演进，不要凭记忆假设。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "model": {
                    "type": "string",
                    "description": "图片模型名，原样取自 list_models（含命名空间前缀时需完整保留）"
                }
            },
            "required": ["model"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Image, ToolTag::Read]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let model = arguments
            .get("model")
            .and_then(|value| value.as_str())
            .map(|text| text.trim().trim_start_matches('@').to_string())
            .filter(|text| !text.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models，从清单中复制完整的图片模型名")?;

        let capability = image_manifest::capability(&model)?;
        serde_json::to_string_pretty(&capability)
            .map_err(|e| format!("序列化能力摘要失败: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn run(model: &str) -> Value {
        let out = ListImageCapabilitiesTool
            .execute(json!({ "model": model }))
            .await
            .expect("查询应成功");
        serde_json::from_str(&out).unwrap()
    }

    #[tokio::test]
    async fn default_rule_supports_generation_slots() {
        let value = run("gpt-image-1").await;
        assert_eq!(value["matchedRule"], "*");
        assert_eq!(value["output"], "markdown_url");
        assert_eq!(value["supported"]["prompt"]["transform"], json!(null));
        assert!(value["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|slot| slot == "mask"));
    }

    #[tokio::test]
    async fn edit_rule_reports_file_parts() {
        let value = run("seedream-image-edit").await;
        assert_eq!(value["matchedRule"], "*edit*");
        assert_eq!(
            value["supported"]["referenceImages"]["transform"],
            "file_part_png"
        );
        assert_eq!(value["limits"]["referenceImages"], 1);
    }

    #[tokio::test]
    async fn missing_or_empty_model_is_an_error() {
        assert!(ListImageCapabilitiesTool.execute(json!({})).await.is_err());
        assert!(ListImageCapabilitiesTool
            .execute(json!({ "model": "  " }))
            .await
            .is_err());
    }
}
