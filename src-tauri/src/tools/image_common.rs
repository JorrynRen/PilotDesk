//! 图生图工具共享原语（edit_image 复用，generate_image 的 image 数组路径独立）。
//!
//! 自 `api_agent/image_to_image.rs` 拆出（工具架构统一 v1.0，命名规约对齐）：
//! 公共的图片来源解析、PNG 归一化与 multipart 上传逻辑。
//!
//! 复用当前 API Provider 的 endpoint 与 API Key，仅 OpenAI 兼容提供商可用。
//! 输入图片来源支持本地路径 / URL / base64 data URL；仅做最小必要处理：
//! 非 PNG 图片转码为 PNG（保留原始尺寸与宽高比），并校验体积，不做强制缩放/裁剪，
//! 以适配任意 OpenAI 兼容的图生图模型。

use base64::Engine;
use std::io::Cursor;

/// 上传图片的体积上限（4MB，多数图片接口的通用限制）。
pub(crate) const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// 从 LLM 传入的图片引用中提取规范 http(s) URL：
/// LLM 常把 Markdown 语法残留/正文标点一并带入（如 `![x](https://..)`、反引号包裹、尾部逗号）。
/// 采用「白名单保留」策略：只剥**绝不可能是 URL 字符**的尾部（空白、反引号、全角标点、
/// Markdown 尖括号闭合符、正文英文逗号/叹号/问号），**保留** `.` `;` `:` `'` `(` `)` 等
/// URL 合法字符——避免截断真实 URL（签名值/文件名/路径参数以这些字符结尾时不能被误删）。
pub(crate) fn normalize_http_url(raw: &str) -> Option<String> {
    let s = raw
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim();
    let idx = s.find("http://").or_else(|| s.find("https://"))?;
    let url = &s[idx..];
    // 尾部剥除集：空白、反引号、全角标点、Markdown 尖括号闭合 `>` 与链接闭合 `)`（`![x](<url>)` 整段传入时）、
    // 正文英文逗号/叹号/问号。**保留** `.` `;` `:` `'` `(` 等 URL 合法字符，避免截断真实 URL。
    let trimmed =
        url.trim_end_matches(|c: char| c.is_whitespace() || "`>，。；：！？、,!?)".contains(c));
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 解析图片来源（本地绝对路径 / http(s) URL / base64 data URL）→ 原始字节。
pub(crate) async fn load_raw_bytes(source: &str) -> Result<Vec<u8>, String> {
    // base64 data URL：data:image/png;base64,xxxx
    if source.starts_with("data:") {
        let b64 = source.split(',').nth(1).ok_or("无效的 data URL")?;
        return base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("base64 解码失败: {}", e));
    }

    // 网络图片：先规范化 URL（剥离序号/标点等污染），避免带 `1.` 前缀或尾部逗号导致下载/上游 404。
    if source.contains("http://") || source.contains("https://") {
        let url = normalize_http_url(source).ok_or("无法识别图片 URL")?;
        return reqwest::get(&url)
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
pub(crate) async fn resolve_image_bytes(source: &str) -> Result<Vec<u8>, String> {
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
pub(crate) async fn upload_to_images_endpoint(
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

    let endpoint = format!("{}/images/{}", api_endpoint.trim_end_matches('/'), path);

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
        let code = status.as_u16();
        log::warn!(
            "[image_common] /images/{} 上传失败: endpoint={}, status={}",
            path,
            endpoint,
            code
        );
        let mut msg = format!("图片生成失败 (HTTP {}): {}", code, text);
        // 404/405/501：服务商/模型很可能未实现 /images/{path} 端点（聚合渠道常见：只开放
        // generations 生成、不开放 edits 编辑）。此时提示改用 generate_image 图生图降级，
        // 而非让 LLM 盲目重试同一个必然失败的端点。
        if code == 404 || code == 405 || code == 501 {
            msg.push_str(&format!(
                "。该服务商/模型可能不支持 /images/{}（图像编辑）端点，请改用 generate_image 并传入 image 数组进行图生图/图像编辑，或更换支持编辑的模型。",
                path
            ));
        }
        return Err(msg);
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
