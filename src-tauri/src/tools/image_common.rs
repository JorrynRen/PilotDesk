//! 图片生成工具共享原语：图片来源解析与 PNG 归一化。
//!
//! 自 `api_agent/image_to_image.rs` 拆出（工具架构统一 v1.0，命名规约对齐）：
//! 输入图片来源支持本地路径 / URL / base64 data URL；仅做最小必要处理：
//! 非 PNG 图片转码为 PNG（保留原始尺寸与宽高比），并校验体积，不做强制缩放/裁剪，
//! 以适配任意 OpenAI 兼容的图生图模型。multipart 请求由通用执行器组装。

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
