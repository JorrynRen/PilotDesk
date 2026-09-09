//! 图像编辑工具：基于已有图片做可控修改（/images/edits）。
//!
//! 自 `api_agent/image_to_image.rs` 拆出（工具架构统一 v1.0，命名规约对齐），
//! 共享原语见 `image_common.rs`。

use crate::tools::image_common::{resolve_image_bytes, upload_to_images_endpoint};
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::Arc;

/// 图像编辑工具
pub struct EditImageTool {
    api_endpoint: String,
    api_key: String,
    /// 跨 provider 解析（providerId → (endpoint, key, api_format)）；None 表示仅支持当前会话提供商。
    /// key 只用于请求构造，绝不进入工具返回值/LLM 上下文。
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
}

impl EditImageTool {
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
    ) -> Self {
        Self { api_endpoint, api_key, resolve, get_models }
    }

    /// 校验 model 是否在当前 provider 的合法模型列表中，不在则返回引导错误。
    fn validate_model(&self, model: &str, provider_id: &str) -> Result<(), String> {
        if let Some(get_models) = &self.get_models {
            let valid: Vec<String> = get_models(provider_id.to_string());
            if !valid.is_empty() && !valid.iter().any(|m| m == model) {
                return Err(format!(
                    "模型 '{}' 不是提供商 '@{}' 的有效模型。\
                     有效模型：{}\n请重新调用 list_models，从返回结果中完整复制模型名（含 namespace 前缀如 TeleAI/xxx）后重试。",
                    model, provider_id, valid.join(", ")
                ));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ToolHandler for EditImageTool {
    fn name(&self) -> &str {
        "edit_image"
    }

    fn description(&self) -> &str {
        "编辑/局部修改已有图片（图生图）。使用 OpenAI 兼容的 /images/edits 接口。模型需经 list_models 选择支持图像编辑的模型。若 /images/edits 端点不可用（HTTP 404/405/501），说明该服务商/模型不支持图像编辑接口，应改用 generate_image 并传入 image 数组完成图生图/图像编辑，而非继续重试本工具。若连续 2 次调用失败，请停止自动重试，向用户确认三选一：继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
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
                    "description": "可选：目标提供商 id（先用 list_models 查看可用提供商）；省略时使用当前会话提供商。跨提供商时需与 model 配合指定。"
                },
                "model": {
                    "type": "string",
                    "description": "图像编辑模型名（必填，原样复制自 list_models 清单，含完整 namespace 前缀，禁止自行缩短或编造）"
                }
            },
            "required": ["image", "prompt"]
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
        let prompt = arguments["prompt"].as_str().ok_or("缺少 prompt 参数")?;
        let size = arguments["size"].as_str().unwrap_or("1024x1024");
        let n = arguments["n"].as_u64().unwrap_or(1).clamp(1, 10);
        // 模型名必须显式提供（各服务商可用模型不同，无通用默认值）：剥离前导 @ 与空白后为空则报错引导。
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名（含 namespace 前缀）传入，不要自行编造")?;

        let image_png = resolve_image_bytes(image).await?;
        let mask_png = match arguments["mask"].as_str() {
            Some(mask) => Some(resolve_image_bytes(mask).await?),
            None => None,
        };

        // 跨提供商解析：指定 provider 时运行时查库取 endpoint/key（key 仅用于请求构造，不进返回值）。
        let provider_arg = arguments["provider"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let (endpoint_base, api_key) = match (&provider_arg, &self.resolve) {
            (Some(pid), Some(resolve)) => match resolve(pid) {
                Some((ep, key, _fmt)) => (ep, key),
                None => return Err(format!("提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商", pid)),
            },
            _ => (self.api_endpoint.clone(), self.api_key.clone()),
        };

        // 校验 model 是否在合法清单内
        let provider_id = provider_arg.as_deref().unwrap_or("__default__");
        self.validate_model(&model, provider_id)?;

        upload_to_images_endpoint(
            "edits",
            &endpoint_base,
            &api_key,
            &model,
            image_png,
            mask_png,
            Some(prompt),
            size,
            n,
        )
        .await
    }
}
