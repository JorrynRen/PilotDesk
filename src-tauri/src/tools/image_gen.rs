// 自 api_agent/image_gen.rs 迁入（工具架构统一 v1.0，轮 7），逻辑不变。

use crate::tools::{RiskLevel, ToolHandler};
use async_trait::async_trait;
use base64::Engine;

/// 图片生成工具：调用 OpenAI 兼容的 `/images/generations` 接口。
/// 复用当前 API Provider 的 endpoint 与 API Key，无需额外配置。
pub struct ImageGenTool {
    api_endpoint: String,
    api_key: String,
}

impl ImageGenTool {
    pub fn new(api_endpoint: String, api_key: String) -> Self {
        Self { api_endpoint, api_key }
    }
}

/// 将图片来源归一化为「公网 URL 或 base64 Data URI」字符串，供 JSON body 的 image 数组使用。
/// 本地文件路径会被读取并转换为 base64 Data URI；URL / Data URI 直接透传。
async fn normalize_image_ref(source: &str) -> Result<String, String> {
    if source.starts_with("data:")
        || source.starts_with("http://")
        || source.starts_with("https://")
    {
        return Ok(source.to_string());
    }
    let bytes = std::fs::read(source).map_err(|e| format!("读取图片文件失败: {}", e))?;
    let format =
        image::guess_format(&bytes).map_err(|e| format!("无法识别图片格式: {}", e))?;
    let mime = format.to_mime_type();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", mime, b64))
}

#[async_trait]
impl ToolHandler for ImageGenTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        "生成或编辑图片。纯文生图时只传 prompt；传入 image 数组时进行图生图/图像编辑/变体（需当前模型支持）。使用 OpenAI 兼容的 /images/generations 接口。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "图片描述（英文效果最佳，尽可能详细）"
                },
                "size": {
                    "type": "string",
                    "description": "输出尺寸，如 1024x1024、1792x1024、1K、2K 等，默认 1024x1024（具体支持取决于模型）"
                },
                "model": {
                    "type": "string",
                    "description": "图片生成模型名，默认 dall-e-3；如提供商使用其他模型可在此指定"
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

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let prompt = arguments["prompt"]
            .as_str()
            .ok_or("缺少 prompt 参数")?;
        let size = arguments["size"].as_str().unwrap_or("1024x1024");
        let n = arguments["n"].as_u64().unwrap_or(1).clamp(1, 4);
        let model = arguments["model"].as_str().unwrap_or("dall-e-3");

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

        let endpoint = format!(
            "{}/images/generations",
            self.api_endpoint.trim_end_matches('/')
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
            .bearer_auth(&self.api_key)
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
                results.push(format!("{}. {}", i + 1, url));
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
