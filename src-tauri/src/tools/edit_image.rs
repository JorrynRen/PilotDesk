//! 图像编辑工具（过渡转发壳）：内部走统一的 `generate_image` 实现。
//!
//! 模型差异由图片模型清单决定，本工具只负责把 `image` + `mask` 参数
//! 映射为 `referenceImages` + `mask`，下一版将移除。

use crate::tools::generate_image::GenerateImageTool;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::Arc;

/// 图像编辑工具（转发）
pub struct EditImageTool {
    inner: GenerateImageTool,
}

impl EditImageTool {
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
        cwd: String,
    ) -> Self {
        Self {
            inner: GenerateImageTool::new(api_endpoint, api_key, resolve, None, get_models, cwd),
        }
    }
}

#[async_trait]
impl ToolHandler for EditImageTool {
    fn name(&self) -> &str {
        "edit_image"
    }

    fn description(&self) -> &str {
        "编辑/局部修改已有图片（过渡兼容入口，等价于 generate_image 传入 referenceImages）。新调用请直接使用 generate_image。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "原图来源：本地绝对路径、http(s) URL 或 base64 data URL"
                },
                "prompt": {
                    "type": "string",
                    "description": "编辑描述（英文效果最佳，说明要如何修改图片）"
                },
                "mask": {
                    "type": "string",
                    "description": "可选遮罩图（与 image 同尺寸，透明区域将被编辑），来源同 image"
                },
                "size": {
                    "type": "string",
                    "description": "输出尺寸，如 1024x1024、1792x1024、1024x1792 等，默认 1024x1024（具体支持取决于模型）"
                },
                "n": {
                    "type": "integer",
                    "description": "生成数量，默认 1，最大 10"
                },
                "provider": {
                    "type": "string",
                    "description": "必填：目标提供商 id，逐字取 list_models 清单里的 provider_id（标 ★ 的是当前会话提供商）；provider 与 model 必须分别传入，禁止拼成 provider_id/model"
                },
                "model": {
                    "type": "string",
                    "description": "图像编辑模型名（必填，原样复制自 list_models 清单；若模型名自身含斜杠/命名空间（如 TeleAI/xxx），必须原样保留；禁止自行缩短、编造，也禁止把 provider_id 拼进来）"
                }
            },
            "required": ["image", "prompt", "model", "provider"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Image, ToolTag::Write, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let image = arguments["image"].as_str().ok_or("缺少 image 参数")?;
        let mut forwarded = arguments.clone();
        forwarded["referenceImages"] = serde_json::json!([image]);
        if let Some(object) = forwarded.as_object_mut() {
            object.remove("image");
        }
        self.inner.execute(forwarded).await
    }
}
