//! 图生图工具：图像编辑（/images/edits）与图像变体（/images/variations）。
//!
//! 复用当前 API Provider 的 endpoint 与 API Key，仅 OpenAI 兼容提供商可用。
//! 输入图片来源支持本地路径 / URL / base64 data URL；本模块仅做最小必要处理：
//! 非 PNG 图片转码为 PNG（保留原始尺寸与宽高比），并校验体积，不做强制缩放/裁剪，
//! 以适配任意 OpenAI 兼容的图生图模型。
// 自 api_agent/image_to_image.rs 迁入（工具架构统一 v1.0，轮 7），逻辑不变。

use crate::tools::{RiskLevel, ToolHandler};
use async_trait::async_trait;
use base64::Engine;
use std::io::Cursor;

/// 上传图片的体积上限（4MB，多数图片接口的通用限制）。
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// 解析图片来源（本地绝对路径 / http(s) URL / base64 data URL）→ 原始字节。
async fn load_raw_bytes(source: &str) -> Result<Vec<u8>, String> {
    // base64 data URL：data:image/png;base64,xxxx
    if source.starts_with("data:") {
        let b64 = source.split(',').nth(1).ok_or("无效的 data URL")?;
        return base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("base64 解码失败: {}", e));
    }

    // 网络图片
    if source.starts_with("http://") || source.starts_with("https://") {
        return reqwest::get(source)
            .await
            .map_err(|e| format!("下载图片失败: {}", e))?
            .bytes()
            .await
            .map_err(|e| format!("读取图片失败: {}", e))
            .map(|b| b.to_vec());
    }

    // 本地文件
    std::fs::read(source).map_err(|e| format!("读取图片文件失败: {}", e))
}

/// 归一化：将任意来源图片转为 PNG（仅当原图非 PNG 时转码）。
/// 保留原始尺寸与宽高比，不做缩放/裁剪，避免破坏图片内容或与目标输出尺寸冲突。
async fn resolve_image_bytes(source: &str) -> Result<Vec<u8>, String> {
    let raw = load_raw_bytes(source).await?;

    // 已经是 PNG，直接透传原始字节，避免重新编码。
    if raw.starts_with(b"\x89PNG\r\n\x1a\n") {
        if raw.len() > MAX_IMAGE_BYTES {
            return Err(format!("图片为 {} 字节，超过 4MB 限制", raw.len()));
        }
        return Ok(raw);
    }

    // 非 PNG：解码后转 PNG，不缩放、不裁剪。
    let img = image::load_from_memory(&raw)
        .map_err(|e| format!("无法解析图片（可能不是有效的图片格式）: {}", e))?;

    let mut png_bytes: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败: {}", e))?;

    if png_bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "图片转换后为 {} 字节，超过 4MB 限制",
            png_bytes.len()
        ));
    }

    Ok(png_bytes)
}

/// 上传 multipart 到 `/images/{path}`（edits / variations），解析响应返回结果文本。
#[allow(clippy::too_many_arguments)]
async fn upload_to_images_endpoint(
    path: &str,
    api_endpoint: &str,
    api_key: &str,
    model: &str,
    image_png: Vec<u8>,
    mask_png: Option<Vec<u8>>,
    prompt: Option<&str>,
    size: &str,
    n: u64,
) -> Result<String, String> {
    use reqwest::multipart::{Form, Part};

    let endpoint = format!(
        "{}/images/{}",
        api_endpoint.trim_end_matches('/'),
        path
    );

    let image_part = Part::bytes(image_png)
        .file_name("image.png")
        .mime_str("image/png")
        .map_err(|e| format!("设置图片 MIME 失败: {}", e))?;

    let mut form = Form::new()
        .text("model", model.to_string())
        .text("size", size.to_string())
        .text("n", n.to_string());
    if let Some(p) = prompt {
        form = form.text("prompt", p.to_string());
    }
    form = form.part("image", image_part);
    if let Some(mask) = mask_png {
        let mask_part = Part::bytes(mask)
            .file_name("mask.png")
            .mime_str("image/png")
            .map_err(|e| format!("设置遮罩 MIME 失败: {}", e))?;
        form = form.part("mask", mask_part);
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(&endpoint)
        .bearer_auth(api_key)
        .multipart(form)
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

/// 图像编辑工具：基于已有图片做可控修改。
pub struct ImageEditTool {
    api_endpoint: String,
    api_key: String,
}

impl ImageEditTool {
    pub fn new(api_endpoint: String, api_key: String) -> Self {
        Self { api_endpoint, api_key }
    }
}

#[async_trait]
impl ToolHandler for ImageEditTool {
    fn name(&self) -> &str {
        "edit_image"
    }

    fn description(&self) -> &str {
        "编辑/局部修改已有图片（图生图）。使用 OpenAI 兼容的 /images/edits 接口，仅支持 dall-e-2。"
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
                "model": {
                    "type": "string",
                    "description": "图像编辑模型名，默认 dall-e-2；如提供商使用其他模型可在此指定"
                }
            },
            "required": ["image", "prompt"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let image = arguments["image"].as_str().ok_or("缺少 image 参数")?;
        let prompt = arguments["prompt"].as_str().ok_or("缺少 prompt 参数")?;
        let size = arguments["size"].as_str().unwrap_or("1024x1024");
        let n = arguments["n"].as_u64().unwrap_or(1).clamp(1, 10);
        let model = arguments["model"].as_str().unwrap_or("dall-e-2");

        let image_png = resolve_image_bytes(image).await?;
        let mask_png = match arguments["mask"].as_str() {
            Some(mask) => Some(resolve_image_bytes(mask).await?),
            None => None,
        };

        upload_to_images_endpoint(
            "edits",
            &self.api_endpoint,
            &self.api_key,
            model,
            image_png,
            mask_png,
            Some(prompt),
            size,
            n,
        )
        .await
    }
}

/// 图像变体工具：基于已有图片生成风格一致的变体。
pub struct ImageVariationTool {
    api_endpoint: String,
    api_key: String,
}

impl ImageVariationTool {
    pub fn new(api_endpoint: String, api_key: String) -> Self {
        Self { api_endpoint, api_key }
    }
}

#[async_trait]
impl ToolHandler for ImageVariationTool {
    fn name(&self) -> &str {
        "image_variation"
    }

    fn description(&self) -> &str {
        "基于已有图片生成风格一致的变体（图生图）。使用 OpenAI 兼容的 /images/variations 接口，仅支持 dall-e-2。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "原图来源：本地绝对路径、http(s) URL 或 base64 data URL"
                },
                "size": {
                    "type": "string",
                    "description": "输出尺寸，如 1024x1024、1792x1024、1024x1792 等，默认 1024x1024（具体支持取决于模型）"
                },
                "n": {
                    "type": "integer",
                    "description": "生成数量，默认 1，最大 10"
                },
                "model": {
                    "type": "string",
                    "description": "图像变体模型名，默认 dall-e-2；如提供商使用其他模型可在此指定"
                }
            },
            "required": ["image"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let image = arguments["image"].as_str().ok_or("缺少 image 参数")?;
        let size = arguments["size"].as_str().unwrap_or("1024x1024");
        let n = arguments["n"].as_u64().unwrap_or(1).clamp(1, 10);
        let model = arguments["model"].as_str().unwrap_or("dall-e-2");

        let image_png = resolve_image_bytes(image).await?;

        upload_to_images_endpoint(
            "variations",
            &self.api_endpoint,
            &self.api_key,
            model,
            image_png,
            None,
            None,
            size,
            n,
        )
        .await
    }
}
