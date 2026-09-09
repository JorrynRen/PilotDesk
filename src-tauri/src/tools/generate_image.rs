//! 图片生成工具：调用 OpenAI 兼容的 `/images/generations` 接口。
//!
//! 复用当前 API Provider 的 endpoint 与 API Key，无需额外配置。
//! 自 `api_agent/image_gen.rs` 迁入（工具架构统一 v1.0，轮 7），文件名对齐工具名 `generate_image`。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use base64::Engine;
use std::sync::Arc;

/// 图片生成工具
pub struct GenerateImageTool {
    api_endpoint: String,
    api_key: String,
    /// 跨 provider 解析（providerId → (endpoint, key, api_format)）；None 表示仅支持当前会话提供商。
    /// key 只用于请求构造，绝不进入工具返回值/LLM 上下文。
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    /// 当前会话提供商 id（与 self.api_endpoint/api_key 对应；None 表示未知）。
    #[allow(dead_code)]
    current_provider: Option<String>,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
}

impl GenerateImageTool {
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        current_provider: Option<String>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
    ) -> Self {
        Self { api_endpoint, api_key, resolve, current_provider, get_models }
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

/// 将图片来源归一化为「公网 URL 或 base64 Data URI」字符串，供 JSON body 的 image 数组使用。
/// 本地文件路径会被读取并转换为 base64 Data URI；URL 经规范化清洗（剥离序号/标点污染）；Data URI 直接透传。
async fn normalize_image_ref(source: &str) -> Result<String, String> {
    if source.starts_with("data:") {
        return Ok(source.to_string());
    }
    if source.contains("http://") || source.contains("https://") {
        return crate::tools::image_common::normalize_http_url(source)
            .ok_or_else(|| "无法识别图片 URL".to_string());
    }
    let bytes = std::fs::read(source).map_err(|e| format!("读取图片文件失败: {}", e))?;
    let format =
        image::guess_format(&bytes).map_err(|e| format!("无法识别图片格式: {}", e))?;
    let mime = format.to_mime_type();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", mime, b64))
}

#[async_trait]
impl ToolHandler for GenerateImageTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        "生成或编辑图片。纯文生图时只传 prompt；传入 image 数组时进行图生图/图像编辑/变体（需当前模型支持）。使用 OpenAI 兼容的 /images/generations 接口。模型需经 list_models 选择支持图片生成的模型。若连续 2 次调用失败（如 HTTP 4xx/5xx、当前模型不支持图片生成），请停止自动重试，向用户确认三选一：继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "图片描述（英文效果最佳，尽可能详细）"
                },
                "provider": {
                    "type": "string",
                    "description": "可选：目标提供商 id（先用 list_models 查看可用提供商）；省略时使用当前会话提供商。跨提供商时需与 model 配合指定。"
                },
                "size": {
                    "type": "string",
                    "description": "输出尺寸，如 1024x1024、1792x1024、1K、2K 等，默认 1024x1024（具体支持取决于模型）"
                },
                "model": {
                    "type": "string",
                    "description": "图片生成模型名（必填，原样复制自 list_models 清单，含完整 namespace 前缀如 organization/model，禁止自行缩短或编造）"
                },
                "n": {
                    "type": "integer",
                    "description": "生成数量，默认 1，最大 4"
                },
                "image": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "可选：图生图/图像编辑/图像变体时传入的参考图片数组。每项可为公网 URL、base64 Data URI 或本地绝对路径。省略则为纯文生图。"
                }
            },
            "required": ["prompt"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Image, ToolTag::Write, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let prompt = arguments["prompt"]
            .as_str()
            .ok_or("缺少 prompt 参数")?;
        let size = arguments["size"].as_str().unwrap_or("1024x1024");
        let n = arguments["n"].as_u64().unwrap_or(1).clamp(1, 4);
        // 模型名必须显式提供（各服务商可用模型不同，无通用默认值）：剥离前导 @ 与空白后为空则报错引导。
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名（含 namespace 前缀）传入，不要自行编造")?;

        // 图生图/编辑/变体：读取参考图片并归一化为「URL 或 base64 Data URI」数组
        let image_sources: Vec<String> = arguments["image"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let mut image_refs: Vec<String> = Vec::new();
        for src in &image_sources {
            image_refs.push(normalize_image_ref(src).await?);
        }

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

        let endpoint = format!(
            "{}/images/generations",
            endpoint_base.trim_end_matches('/')
        );
        let mut body = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "n": n,
            "size": size,
        });
        if !image_refs.is_empty() {
            body["image"] = serde_json::json!(image_refs);
        }

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("图片生成请求失败: {}", e))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;

        if !status.is_success() {
            return Err(format!(
                "图片生成失败 (HTTP {}): {}",
                status.as_u16(),
                text
            ));
        }

        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {}", e))?;
        let data = json["data"]
            .as_array()
            .ok_or_else(|| format!("响应缺少 data 数组: {}", text))?;

        let mut results: Vec<String> = Vec::new();
        for (i, item) in data.iter().enumerate() {
            if let Some(url) = item["url"].as_str() {
                // 用尖括号包裹的 Markdown 图片语法返回（CommonMark 支持 `<url>` 界定 URL 边界）：
                // 前端直接内嵌显示（可点击放大）；LLM 后续提取 URL 时以尖括号为界，不会在
                // 路径中途截断（此前 `![x](url)` 裸 URL 曾被 LLM 截成根路径 + 误带标点）。
                results.push(format!("![图片{}](<{}>)", i + 1, url));
            } else if let Some(b64) = item["b64_json"].as_str() {
                let preview_len = b64.len().min(120);
                results.push(format!("{}. [base64] {}...", i + 1, &b64[..preview_len]));
            }
        }

        if results.is_empty() {
            return Err(format!("图片生成响应中没有图片: {}", text));
        }

        Ok(format!(
            "已生成 {} 张图片：\n{}",
            results.len(),
            results.join("\n")
        ))
    }
}
