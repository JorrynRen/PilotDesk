//! 视频模型能力查询：只读、无网络、无副作用。
//!
//! 把「这个模型吃什么参数」的本地硬失败前移：模型在发请求前先查一次，
//! 避免用不支持的槽位反复试错。数据完全来自当前生效的视频清单
//! （[`super::video_manifest::capability`]），不内联任何厂商能力。

use async_trait::async_trait;
use serde_json::Value;

use crate::tools::video_manifest;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

/// 视频模型能力查询工具
pub struct ListVideoCapabilitiesTool;

#[async_trait]
impl ToolHandler for ListVideoCapabilitiesTool {
    fn name(&self) -> &str {
        "list_video_capabilities"
    }

    fn description(&self) -> &str {
        "查询某个视频模型支持的参数槽位与限制（时长档位、参考图上限、画幅/分辨率）。\
         只读、无网络。在不确定模型是否支持某参数时先调用本工具，再调用 generate_video；\
         返回的 unsupported 列表即该模型会被本地拒绝的参数。\
         能力随视频清单版本演进，不要凭记忆假设。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "model": {
                    "type": "string",
                    "description": "视频模型名，原样取自 list_models（含命名空间前缀时需完整保留）"
                }
            },
            "required": ["model"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Video, ToolTag::Read]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let model = arguments
            .get("model")
            .and_then(|value| value.as_str())
            .map(|text| text.trim().trim_start_matches('@').to_string())
            .filter(|text| !text.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models，从清单中复制完整的视频模型名")?;

        let capability = video_manifest::capability(&model)?;
        serde_json::to_string_pretty(&capability)
            .map_err(|e| format!("序列化能力摘要失败: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn run(model: &str) -> Value {
        let out = ListVideoCapabilitiesTool
            .execute(json!({ "model": model }))
            .await
            .expect("查询应成功");
        serde_json::from_str(&out).unwrap()
    }

    #[tokio::test]
    async fn reports_supported_and_unsupported_slots() {
        let value = run("kling-v2.5").await;
        assert_eq!(value["model"], "kling-v2.5");
        assert_eq!(value["matchedRule"], "*kling*");
        // 文档化的能力：首尾帧 + 时长档位
        assert_eq!(value["supported"]["firstFrame"]["transform"], "data_uri_to_base64");
        assert_eq!(value["supported"]["duration"]["allowed"], json!([5, 10]));
        // 未映射的槽位必须出现在 unsupported
        assert!(value["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|slot| slot == "aspectRatio"));
        assert!(value["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|slot| slot == "seed"));
        // limits.durations 是 f64（清单里可能是 4.5 这类小数）
        assert_eq!(value["limits"]["durations"], json!([5.0, 10.0]));
    }

    #[tokio::test]
    async fn missing_or_empty_model_is_an_error() {
        assert!(ListVideoCapabilitiesTool.execute(json!({})).await.is_err());
        assert!(ListVideoCapabilitiesTool
            .execute(json!({ "model": "  " }))
            .await
            .is_err());
    }
}
