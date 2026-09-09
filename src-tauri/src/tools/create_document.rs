//! 文档创建工具：生成 DOCX / XLSX / PDF 文件（结构化内容 → Office/PDF 二进制）。
//!
//! 与 write_file 的边界：write_file 写纯文本（代码/配置/JSON）；本工具写结构化
//! Office/PDF 文件（报告、表格、演示等）。content 支持两类输入：
//! - JSON 数组 blocks：`[{type: heading|paragraph|table|list, ...}]`
//! - 纯文本：按行写入段落
//!
//! DOCX/XLSX/PPTX 采用手写标准 OOXML（zip + XML，零额外依赖，Office/WPS 可直接打开）；
//! PDF 用 printpdf，中文依赖系统字体（Windows 优先 simhei.ttf / simfang.ttf），
//! 缺失时回退内置 Helvetica（中文显示为占位，描述中已引导）。
//! PPTX 骨架含 presentation/slide/layout/master/theme 完整引用链（PowerPoint 严格校验，
//! 兼容性需真机验证）；内容按行自动分页，表格独立成页。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;

pub struct CreateDocumentTool {
    cwd: String,
}

impl CreateDocumentTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

/// XML 文本转义（docx/xlsx 文本节点）
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 从 block 提取样式：(color_hex6, align, bold, italic, size_pt)。缺省为 None/false。
/// 颜色接受 `#RRGGBB` 或 `RRGGBB`，归一化为 6 位大写十六进制。
fn block_style(b: &Value) -> (Option<String>, Option<String>, bool, bool, Option<f64>) {
    let color = b["color"]
        .as_str()
        .map(|s| s.trim().trim_start_matches('#').to_uppercase())
        .filter(|s| s.len() == 6);
    let align = b["align"].as_str().map(|s| s.to_string());
    let bold = b["bold"].as_bool().unwrap_or(false);
    let italic = b["italic"].as_bool().unwrap_or(false);
    let size = b["size"].as_f64().filter(|s| *s > 0.0);
    (color, align, bold, italic, size)
}

/// 读取本地图片像素尺寸（px）。
fn image_dimensions(path: &str) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .into_dimensions()
        .ok()
}

/// `#RRGGBB` → (r, g, b) 0-255（printpdf 0.6 通道为 f32）。
fn hex_to_rgb(hex: &str) -> Option<(f32, f32, f32)> {
    let h = u32::from_str_radix(hex, 16).ok()?;
    Some((
        ((h >> 16) & 0xFF) as f32,
        ((h >> 8) & 0xFF) as f32,
        (h & 0xFF) as f32,
    ))
}

/// 图片扩展名（小写，无点）。
fn image_ext(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "png".to_string())
}

/// 解析 content：JSON blocks 数组优先，否则按纯文本单段落。
fn parse_blocks(content: &str) -> Vec<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(content) {
        if let Some(arr) = v.as_array() {
            return arr.clone();
        }
    }
    content
        .lines()
        .map(|l| serde_json::json!({"type": "paragraph", "text": l.trim_end()}))
        .collect()
}

/// slide 内支持的元素类型（PPT 技能兼容子集）。
fn is_slide_element(t: &str) -> bool {
    matches!(
        t,
        "title" | "subtitle" | "section_title"
            | "paragraph" | "list"
            | "info" | "soft_skills"
            | "divider" | "decorative_line"
            | "card" | "timeline_item" | "skill_group"
            | "decorative_circle" | "highlight_text"
    )
}

/// 渲染前校验 block 类型：未知类型明确报错，避免"静默空白文档"。
fn validate_blocks(blocks: &[Value], format: &str) -> Result<(), String> {
    let allowed = ["heading", "paragraph", "table", "list", "image"];
    for b in blocks {
        let t = b["type"].as_str().unwrap_or("");
        if t.is_empty() {
            return Err("block 缺少 type 字段".to_string());
        }
        if t == "slide" {
            if format != "pptx" {
                return Err("slide 结构（逐页 PPT）仅支持 format=pptx；docx/xlsx/pdf 请使用扁平 blocks（heading/paragraph/table/list/image）".to_string());
            }
            if let Some(elems) = b["elements"].as_array() {
                for e in elems {
                    let et = e["type"].as_str().unwrap_or("");
                    if !is_slide_element(et) {
                        return Err(format!("slide 元素类型不支持: {}（支持 title/subtitle/section_title/paragraph/list/info/soft_skills/divider/decorative_line/card/timeline_item/skill_group/decorative_circle/highlight_text）", et));
                    }
                }
            }
            continue;
        }
        if !allowed.contains(&t) {
            return Err(format!(
                "block 类型不支持: {}（支持 {}；PPT 可用 slide 逐页结构）",
                t,
                allowed.join("/")
            ));
        }
    }
    Ok(())
}

/// 输出路径解析（绝对或相对 cwd）
fn resolve_out(cwd: &str, file: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(file.trim());
    let abs = if p.is_absolute() { p } else { PathBuf::from(cwd).join(&p) };
    if let Some(parent) = abs.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建输出目录失败: {}", e))?;
        }
    }
    Ok(abs)
}

/// 简单文本截断（避免异常长内容撑爆）
fn truncate_line(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{}...", cut)
    }
}

#[async_trait]
impl ToolHandler for CreateDocumentTool {
    fn name(&self) -> &str {
        "create_document"
    }

    fn description(&self) -> &str {
        "创建 Office/PDF 文档（docx/xlsx/pptx/pdf）并保存到本地路径。传 format 指定格式、file 指定输出路径（绝对或相对工作目录）、\
         content 提供内容：JSON blocks 数组（[{type:heading,level,text},{type:paragraph,text},{type:table,header,rows,headerStyle},{type:list,items},{type:image,path}]）\
         或纯文本（按行成段）。文本类 block 支持样式字段：align(left/center/right)、color(#RRGGBB)、bold、italic、size(字号pt)；\
         表格默认首行加粗+浅蓝底（headerStyle:false 可关闭）；image 支持本地图片（docx/pptx 插入，自动等比缩放）。\
         PPT(pptx) 额外支持逐页 slide 结构：[{\"type\":\"slide\",\"layout\":\"title/content/end\",\"background_color\":\"#1F4E79\",\"elements\":[...]}]——\
         每 slide 一页，elements 支持 title/subtitle/section_title/paragraph/list/info/soft_skills/divider/decorative_line/card/timeline_item/skill_group/decorative_circle，\
         样式字段兼容 font_size/font_color。\
         PDF 中文需系统存在中文字体（Windows 的 simhei.ttf/simfang.ttf 自动探测）。\
         若连续 2 次调用失败，请停止自动重试，向用户确认三选一：修改参数后重试、使用相同参数再试一次、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "format": {
                    "type": "string",
                    "enum": ["docx", "xlsx", "pdf", "pptx"],
                    "description": "输出格式：docx（Word）、xlsx（Excel）、pptx（PowerPoint）、pdf（PDF）"
                },
                "file": {
                    "type": "string",
                    "description": "输出文件路径（绝对路径或相对工作目录路径），如 C:\\Users\\xxx\\Desktop\\report.docx"
                },
                "content": {
                    "type": "string",
                    "description": "文档内容：JSON blocks 数组或纯文本。扁平块：heading/paragraph/table(header,rows,headerStyle)/list(items)/image(path)，可选样式 align/color(#RRGGBB)/bold/italic/size(pt)。PPT 逐页 slide：{\"type\":\"slide\",\"background_color\":\"#1F4E79\",\"elements\":[{\"type\":\"title|subtitle|section_title|paragraph|list|info|card|timeline_item|skill_group\",...}]}，元素样式字段 font_size/font_color。示例：扁平 [{\"type\":\"heading\",\"level\":1,\"text\":\"报告\",\"color\":\"#4472C4\",\"align\":\"center\"},{\"type\":\"paragraph\",\"text\":\"正文\",\"bold\":true},{\"type\":\"table\",\"rows\":[[\"列A\",\"列B\"],[1,2]]}]；PPT [{\"type\":\"slide\",\"background_color\":\"#1F4E79\",\"elements\":[{\"type\":\"title\",\"text\":\"标题\",\"font_size\":60,\"font_color\":\"#FFFFFF\"}]}]"
                }
            },
            "required": ["format", "file", "content"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Filesystem, ToolTag::Write]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let format = arguments["format"].as_str().ok_or("缺少 format 参数（docx/xlsx/pptx/pdf）")?;
        let file = arguments["file"].as_str().ok_or("缺少 file 参数（输出路径）")?;
        let content = arguments["content"].as_str().ok_or("缺少 content 参数")?;
        let abs = resolve_out(&self.cwd, file)?;
        let blocks = parse_blocks(content);
        validate_blocks(&blocks, format)?;

        match format {
            "docx" => write_docx(&abs, &blocks),
            "xlsx" => write_xlsx(&abs, &blocks),
            "pptx" => write_pptx(&abs, &blocks),
            "pdf" => write_pdf(&abs, &blocks),
            other => Err(format!("不支持的格式: {}（支持 docx/xlsx/pptx/pdf）", other)),
        }?;

        Ok(format!("已创建文档：{}", abs.display()))
    }
}

// ── DOCX（手写标准 OOXML：zip 打包 word/document.xml）──

/// 渲染一个 docx 段落（含样式：颜色/对齐/加粗/斜体/字号）。
/// 返回 (段落 XML, 是否非空文本)。
fn docx_paragraph_xml(text: &str, color: &Option<String>, align: &Option<String>, bold: bool, italic: bool, size: Option<f64>) -> String {
    let t = xml_escape(text);
    let mut rpr = String::new();
    if bold {
        rpr.push_str("<w:b/>");
    }
    if italic {
        rpr.push_str("<w:i/>");
    }
    if let Some(c) = color {
        rpr.push_str(&format!("<w:color w:val=\"{}\"/>", c));
    }
    if let Some(sz) = size {
        rpr.push_str(&format!("<w:sz w:val=\"{}\"/>", (sz * 2.0) as i64));
    }
    let jc = match align.as_deref() {
        Some("center") => "center",
        Some("right") => "right",
        _ => "left",
    };
    if rpr.is_empty() {
        format!(
            "<w:p><w:pPr><w:jc w:val=\"{}\"/></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>",
            jc, t
        )
    } else {
        format!(
            "<w:p><w:pPr><w:jc w:val=\"{}\"/><w:rPr>{}</w:rPr></w:pPr><w:r><w:rPr>{}</w:rPr><w:t>{}</w:t></w:r></w:p>",
            jc, rpr, rpr, t
        )
    }
}

/// 将 blocks 渲染为 docx 的 document.xml body 内容，并收集需打包的图片。
/// 返回 (body, 图片列表)。图片元组：media 文件名, 字节, 关系 id, 宽 EMU, 高 EMU。
fn docx_body_xml(blocks: &[Value]) -> (String, Vec<(String, Vec<u8>, usize, i64, i64)>) {
    let mut body = String::new();
    let mut images: Vec<(String, Vec<u8>, usize, i64, i64)> = Vec::new();
    let mut pic_id = 2usize;
    for b in blocks {
        let (color, align, bold, italic, size) = block_style(b);
        match b["type"].as_str() {
            Some("heading") => {
                let level = b["level"].as_u64().unwrap_or(1);
                let hsize = if level <= 1 { 20.0 } else { 15.0 };
                let text = b["text"].as_str().unwrap_or("");
                body.push_str(&docx_paragraph_xml(text, &color, &align, true, italic, size.or(Some(hsize))));
            }
            Some("image") => {
                if let Some(path) = b["path"].as_str() {
                    if let (Ok(bytes), Some((w, h))) = (std::fs::read(path), image_dimensions(path)) {
                        // 宽度上限 6 英寸（5486400 EMU），等比缩放
                        let scale = if w > 600 { 600.0 / w as f64 } else { 1.0 };
                        let cx = ((w as f64) * 9525.0 * scale) as i64;
                        let cy = ((h as f64) * 9525.0 * scale) as i64;
                        let ext = image_ext(path);
                        let media_name = format!("image{}.{}", images.len() + 1, ext);
                        let rel_id = images.len() + 1;
                        images.push((media_name, bytes, rel_id, cx, cy));
                        body.push_str(&format!(
                            "<w:p><w:r><w:drawing>\
                            <wp:inline xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
                            <wp:extent cx=\"{}\" cy=\"{}\"/>\
                            <wp:docPr id=\"{}\" name=\"Picture {}\"/>\
                            <a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
                            <a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
                            <pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
                            <pic:nvPicPr><pic:cNvPr id=\"{}\" name=\"Picture {}\"/><pic:cNvPicPr/></pic:nvPicPr>\
                            <pic:blipFill><a:blip r:embed=\"rId{}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
                            <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>\
                            <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr>\
                            </pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>",
                            cx, cy, pic_id, pic_id, pic_id, pic_id, rel_id, cx, cy
                        ));
                        pic_id += 1;
                    }
                }
            }
            Some("table") => {
                if let Some(rs) = b["rows"].as_array() {
                    body.push_str(
                        "<w:tbl><w:tblPr><w:tblBorders>\
                         <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
                         <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
                         <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
                         <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
                         <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
                         <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
                         </w:tblBorders></w:tblPr>",
                    );
                    let header_style = b["headerStyle"].as_bool().unwrap_or(true);
                    for (ri, r) in rs.iter().enumerate() {
                        body.push_str("<w:tr>");
                        if let Some(cells) = r.as_array() {
                            for c in cells {
                                let cell = xml_escape(c.as_str().unwrap_or(""));
                                let is_header = header_style && ri == 0;
                                if is_header {
                                    body.push_str(&format!(
                                        "<w:tc><w:tcPr><w:tcW w:w=\"2400\" w:type=\"dxa\"/>\
                                        <w:shd w:val=\"clear\" w:fill=\"D9E2F3\"/></w:tcPr>\
                                        <w:p><w:pPr><w:rPr><w:b/></w:rPr></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>{}</w:t></w:r></w:p></w:tc>",
                                        cell
                                    ));
                                } else {
                                    body.push_str(&format!(
                                        "<w:tc><w:tcPr><w:tcW w:w=\"2400\" w:type=\"dxa\"/></w:tcPr>\
                                        <w:p><w:r><w:t>{}</w:t></w:r></w:p></w:tc>",
                                        cell
                                    ));
                                }
                            }
                        }
                        body.push_str("</w:tr>");
                    }
                    body.push_str("</w:tbl>");
                }
            }
            Some("list") => {
                if let Some(items) = b["items"].as_array() {
                    for it in items {
                        let text = format!("- {}", it.as_str().unwrap_or(""));
                        body.push_str(&docx_paragraph_xml(&text, &color, &align, bold, italic, size));
                    }
                }
            }
            _ => {
                for line in b["text"].as_str().unwrap_or("").lines() {
                    body.push_str(&docx_paragraph_xml(line, &color, &align, bold, italic, size));
                }
            }
        }
    }
    (body, images)
}

fn write_docx(abs: &PathBuf, blocks: &[Value]) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    let (body, images) = docx_body_xml(blocks);
    let document_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <w:body>{}</w:body></w:document>",
        body
    );
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Default Extension=\"png\" ContentType=\"image/png\"/>\
        <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
        <Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
        <Default Extension=\"gif\" ContentType=\"image/gif\"/>\
        <Default Extension=\"webp\" ContentType=\"image/webp\"/>\
        <Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
        <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
        </Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
        </Relationships>";

    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default();
    let add = |writer: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>, name: &str, data: &[u8]| -> Result<(), String> {
        writer.start_file(name, options).map_err(|e| format!("打包失败（{}）: {}", name, e))?;
        writer.write_all(data).map_err(|e| format!("写入失败: {}", e))?;
        Ok(())
    };
    add(&mut writer, "[Content_Types].xml", content_types.as_bytes())?;
    add(&mut writer, "_rels/.rels", rels.as_bytes())?;
    add(&mut writer, "word/document.xml", document_xml.as_bytes())?;
    if !images.is_empty() {
        // word/_rels/document.xml.rels + media 图片
        let mut doc_rels = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
            <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for (i, (media_name, _, _, _, _)) in images.iter().enumerate() {
            let rid = i + 1;
            doc_rels.push_str(&format!(
                "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{}\"/>",
                rid, media_name
            ));
        }
        doc_rels.push_str("</Relationships>");
        add(&mut writer, "word/_rels/document.xml.rels", doc_rels.as_bytes())?;
        for (media_name, bytes, _, _, _) in &images {
            add(&mut writer, &format!("word/media/{}", media_name), bytes)?;
        }
    }
    let cursor = writer.finish().map_err(|e| format!("DOCX 打包失败: {}", e))?;
    std::fs::write(abs, cursor.into_inner()).map_err(|e| format!("写入文件失败: {}", e))
}

// ── XLSX（手写标准 OOXML：zip 打包 sheet/workbook）──

/// 列引用（0-based → A/B/.../AA）
fn xlsx_col_name(i: usize) -> String {
    let mut n = i;
    let mut s = String::new();
    loop {
        let rem = (n % 26) as u8;
        s.insert(0, (b'A' + rem) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s
}

/// xlsx 单元格样式索引：
/// 0=默认 1=加粗 2=表头(加粗+底色) 3=居中 4=加粗居中
fn xlsx_style_idx(is_header: bool, bold: bool, align: Option<&str>) -> u32 {
    if is_header {
        return 2;
    }
    let centered = matches!(align, Some("center") | Some("right"));
    match (bold, centered) {
        (true, true) => 4,
        (false, true) => 3,
        (true, false) => 1,
        _ => 0,
    }
}

fn write_xlsx(abs: &PathBuf, blocks: &[Value]) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    // 文本行样式表（行索引 → (align, bold)）
    let mut text_row_styles: Vec<(usize, Option<String>, bool)> = Vec::new();
    let mut sheet_rows2: Vec<(Vec<String>, bool)> = Vec::new();
    for b in blocks {
        let (_, align, bold, _, _) = block_style(b);
        match b["type"].as_str() {
            Some("table") => {
                if let Some(rs) = b["rows"].as_array() {
                    let header_style = b["headerStyle"].as_bool().unwrap_or(true);
                    for (ri, r) in rs.iter().enumerate() {
                        let row: Vec<String> = r
                            .as_array()
                            .map(|cells| {
                                cells
                                    .iter()
                                    .map(|c| truncate_line(c.as_str().unwrap_or(""), 1024))
                                    .collect()
                            })
                            .unwrap_or_default();
                        sheet_rows2.push((row, header_style && ri == 0));
                    }
                }
            }
            _ => {
                for line in b["text"].as_str().unwrap_or("").lines() {
                    let row = vec![truncate_line(line, 4096)];
                    text_row_styles.push((sheet_rows2.len(), align.clone(), bold));
                    sheet_rows2.push((row, false));
                }
            }
        }
    }
    let sheet_rows = sheet_rows2;

    let mut sheet = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    for (ri, (row, is_header)) in sheet_rows.iter().enumerate() {
        sheet.push_str(&format!("<row r=\"{}\">", ri + 1));
        for (ci, cell) in row.iter().enumerate() {
            let refname = format!("{}{}", xlsx_col_name(ci), ri + 1);
            // 样式：表头行固定 s=2；文本行按块级样式（align/bold）
            let s = if *is_header {
                2
            } else {
                let (_, t_align, t_bold) = text_row_styles
                    .iter()
                    .find(|(r, _, _)| *r == ri)
                    .cloned()
                    .unwrap_or((ri, None, false));
                xlsx_style_idx(false, t_bold, t_align.as_deref())
            };
            let s_attr = if s > 0 { format!(" s=\"{}\"", s) } else { String::new() };
            if let Ok(num) = cell.trim().parse::<f64>() {
                sheet.push_str(&format!("<c r=\"{}\"{}><v>{}</v></c>", refname, s_attr, num));
            } else {
                sheet.push_str(&format!(
                    "<c r=\"{}\"{} t=\"inlineStr\"><is><t>{}</t></is></c>",
                    refname,
                    s_attr,
                    xml_escape(cell)
                ));
            }
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");

    // 样式表：0=默认 1=加粗 2=表头(加粗+浅蓝底) 3=居中 4=加粗居中
    let styles = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
        <fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font>\
        <font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
        <fills count=\"3\"><fill><patternFill patternType=\"none\"/></fill>\
        <fill><patternFill patternType=\"gray125\"/></fill>\
        <fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFD9E2F3\"/><bgColor indexed=\"64\"/></patternFill></fill></fills>\
        <borders count=\"1\"><border/></borders>\
        <cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
        <cellXfs count=\"5\">\
        <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
        <xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/>\
        <xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"0\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\"/>\
        <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyAlignment=\"1\"><alignment horizontal=\"center\"/></xf>\
        <xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\" applyAlignment=\"1\"><alignment horizontal=\"center\"/></xf>\
        </cellXfs></styleSheet>";

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
        <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
        <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
        </Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
        </Relationships>";
    let workbook = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
        <sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>";
    let workbook_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
        <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
        </Relationships>";

    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default();
    for (name, data) in [
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", workbook_rels.as_bytes()),
        ("xl/styles.xml", styles.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
    ] {
        writer.start_file(name, options).map_err(|e| format!("打包失败（{}）: {}", name, e))?;
        writer.write_all(data).map_err(|e| format!("写入失败: {}", e))?;
    }
    let cursor = writer.finish().map_err(|e| format!("XLSX 打包失败: {}", e))?;
    std::fs::write(abs, cursor.into_inner()).map_err(|e| format!("写入文件失败: {}", e))
}

// ── PPTX（手写标准 OOXML：presentation + slides + layout + master + theme）──

/// PPTX 页面尺寸（16:9，EMU；1pt=12700 EMU）
const PPTX_SLIDE_W: i64 = 12_192_000;
const PPTX_SLIDE_H: i64 = 6_858_000;
const PPTX_MARGIN: i64 = 457_200;

/// 最小主题（PowerPoint 打开所需的 theme1.xml，Office 经典色板）
const PPTX_THEME_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="PilotDesk Theme"><a:themeElements>
<a:clrScheme name="PilotDesk"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="PilotDesk"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea/><a:cs/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea/><a:cs/></a:minorFont></a:fontScheme>
<a:fmtScheme name="PilotDesk"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
<a:lnStyleLst><a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="12700" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="19050" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln></a:lnStyleLst>
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme>
</a:themeElements></a:theme>"#;

/// PPTX 渲染项（文本行或图片）
enum PptxItem {
    Text { size: f64, text: String, bold: bool, italic: bool, color: Option<String>, align: Option<String> },
    Image { path: String, w: u32, h: u32 },
}

/// PPTX 页内形状（文本框或图片）
enum PptxShape {
    Text { x: i64, y: i64, w: i64, h: i64, size: f64, text: String, bold: bool, italic: bool, color: Option<String>, align: Option<String> },
    Pic { x: i64, y: i64, cx: i64, cy: i64, id: i64, rel_id: u32 },
}

/// blocks 平铺为渲染项（含样式与图片）。
fn pptx_items(blocks: &[Value]) -> Vec<PptxItem> {
    let mut items = Vec::new();
    for b in blocks {
        let (color, align, bold, italic, size) = block_style(b);
        match b["type"].as_str() {
            Some("heading") => {
                let level = b["level"].as_u64().unwrap_or(1);
                let hsize = size.unwrap_or(if level <= 1 { 32.0 } else { 24.0 });
                items.push(PptxItem::Text {
                    size: hsize,
                    text: b["text"].as_str().unwrap_or("").to_string(),
                    bold: true,
                    italic,
                    color: color.clone(),
                    align: align.clone(),
                });
            }
            Some("image") => {
                if let Some(path) = b["path"].as_str() {
                    if let Some((w, h)) = image_dimensions(path) {
                        items.push(PptxItem::Image { path: path.to_string(), w, h });
                    }
                }
            }
            Some("table") => {
                if let Some(rs) = b["rows"].as_array() {
                    for r in rs {
                        let row: Vec<String> = r
                            .as_array()
                            .map(|cells| cells.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect())
                            .unwrap_or_default();
                        items.push(PptxItem::Text {
                            size: 16.0,
                            text: row.join("  |  "),
                            bold: false,
                            italic: false,
                            color: color.clone(),
                            align: align.clone(),
                        });
                    }
                }
            }
            Some("list") => {
                if let Some(items_arr) = b["items"].as_array() {
                    for it in items_arr {
                        items.push(PptxItem::Text {
                            size: size.unwrap_or(18.0),
                            text: format!("- {}", it.as_str().unwrap_or("")),
                            bold,
                            italic,
                            color: color.clone(),
                            align: align.clone(),
                        });
                    }
                }
            }
            _ => {
                for line in b["text"].as_str().unwrap_or("").lines() {
                    items.push(PptxItem::Text {
                        size: size.unwrap_or(18.0),
                        text: line.to_string(),
                        bold,
                        italic,
                        color: color.clone(),
                        align: align.clone(),
                    });
                }
            }
        }
    }
    items
}

/// 一页 PPT 内容（顶层 slide 结构时每 slide 一页，携带背景色）。
struct PptxSlide {
    background: Option<String>,
    items: Vec<PptxItem>,
}

/// 颜色归一化：#RRGGBB / RRGGBB → 6 位大写。
fn normalize_color(s: &str) -> Option<String> {
    let c = s.trim().trim_start_matches('#').to_uppercase();
    (c.len() == 6).then_some(c)
}

/// slide 元素 → PptxItem（兼容技能 schema：font_size/font_color 等字段别名）。
fn slide_element_items(e: &Value) -> Vec<PptxItem> {
    let t = e["type"].as_str().unwrap_or("");
    let color = e["font_color"].as_str().and_then(normalize_color).or_else(|| e["color"].as_str().and_then(normalize_color));
    let size = e["font_size"].as_f64().or_else(|| e["size"].as_f64()).filter(|s| *s > 0.0);
    let bold = e["bold"].as_bool().unwrap_or(false);
    let align = e["align"].as_str().map(|s| s.to_string());
    let text = |v: &Value| v["text"].as_str().unwrap_or("").to_string();
    let text_item = |s: f64, txt: String, b: bool| PptxItem::Text {
        size: s,
        text: txt,
        bold: b,
        italic: false,
        color: color.clone(),
        align: align.clone(),
    };

    match t {
        "title" => vec![text_item(size.unwrap_or(32.0), text(e), true)],
        "subtitle" => vec![text_item(size.unwrap_or(22.0), text(e), false)],
        "section_title" => vec![text_item(size.unwrap_or(24.0), text(e), true)],
        "paragraph" => text(e).lines().map(|l| text_item(size.unwrap_or(18.0), l.to_string(), bold)).collect(),
        "list" => e["items"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|it| text_item(size.unwrap_or(18.0), format!("- {}", it.as_str().unwrap_or("")), bold))
                    .collect()
            })
            .unwrap_or_default(),
        "info" | "soft_skills" => vec![text_item(size.unwrap_or(16.0), text(e), bold)],
        "divider" | "decorative_line" => vec![text_item(12.0, String::new(), false)], // 空行分隔
        "card" => {
            let mut lines = Vec::new();
            if let Some(t2) = e["title"].as_str() {
                lines.push(format!("{}", t2));
            }
            if let Some(s) = e["subtitle"].as_str() {
                lines.push(s.to_string());
            }
            if let Some(ds) = e["details"].as_array() {
                for d in ds {
                    lines.push(format!("  · {}", d.as_str().unwrap_or("")));
                }
            }
            lines.into_iter().map(|l| text_item(size.unwrap_or(16.0), l, false)).collect()
        }
        "timeline_item" => {
            let mut lines = Vec::new();
            let mut head = String::new();
            if let Some(p) = e["period"].as_str() {
                head.push_str(p);
            }
            if let Some(c) = e["company"].as_str() {
                if !head.is_empty() {
                    head.push_str("  |  ");
                }
                head.push_str(c);
            }
            if let Some(pos) = e["position"].as_str() {
                if !head.is_empty() {
                    head.push_str("  |  ");
                }
                head.push_str(pos);
            }
            if !head.is_empty() {
                lines.push(head);
            }
            if let Some(rs) = e["responsibilities"].as_array() {
                for r in rs {
                    lines.push(format!("  · {}", r.as_str().unwrap_or("")));
                }
            }
            lines.into_iter().map(|l| text_item(size.unwrap_or(16.0), l, false)).collect()
        }
        "skill_group" => {
            let category = e["category"].as_str().unwrap_or("");
            let tags = e["tags"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("、"))
                .unwrap_or_default();
            vec![text_item(size.unwrap_or(16.0), format!("{}：{}", category, tags), bold)]
        }
        // 装饰元素忽略
        _ => Vec::new(),
    }
}

/// 顶层 blocks → 分页结构：含 slide 时每 slide 一页（携带背景色），否则扁平块单页。
fn pptx_slides(blocks: &[Value]) -> Vec<PptxSlide> {
    let has_slide = blocks.iter().any(|b| b["type"].as_str() == Some("slide"));
    if !has_slide {
        return vec![PptxSlide { background: None, items: pptx_items(blocks) }];
    }
    let mut out = Vec::new();
    for b in blocks {
        if b["type"].as_str() == Some("slide") {
            let bg = b["background_color"].as_str().and_then(normalize_color);
            let mut items = Vec::new();
            if let Some(elems) = b["elements"].as_array() {
                for e in elems {
                    items.extend(slide_element_items(e));
                }
            }
            out.push(PptxSlide { background: bg, items });
        } else {
            out.push(PptxSlide {
                background: None,
                items: pptx_items(std::slice::from_ref(b)),
            });
        }
    }
    out
}

/// 单页 slide XML：渲染文本形状与图片，id 从 first_id 起；可携带背景色。
fn pptx_slide_xml(shapes: &[PptxShape], first_id: i64, background: &Option<String>) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
        xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
        <p:cSld>",
    );
    if let Some(bg) = background {
        out.push_str(&format!(
            "<p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>",
            bg
        ));
    }
    out.push_str(
        "<p:spTree>\
        <p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
        <p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>",
    );
    let mut id = first_id;
    for shape in shapes {
        match shape {
            PptxShape::Text { x, y, w, h, size, text, bold, italic, color, align } => {
                let t = xml_escape(text);
                let sz = (size * 100.0) as i64;
                let mut rpr = format!("lang=\"zh-CN\" sz=\"{}\"", sz);
                if *bold {
                    rpr.push_str(" b=\"1\"");
                }
                if *italic {
                    rpr.push_str(" i=\"1\"");
                }
                let mut rpr_body = String::new();
                if let Some(c) = color {
                    rpr_body.push_str(&format!("<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>", c));
                }
                let algn = match align.as_deref() {
                    Some("center") => "ctr",
                    Some("right") => "r",
                    _ => "l",
                };
                out.push_str(&format!(
                    "<p:sp><p:nvSpPr><p:cNvPr id=\"{}\" name=\"TextBox {}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>\
                    <p:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>\
                    <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>\
                    <p:txBody><a:bodyPr wrap=\"none\"/><a:lstStyle/>\
                    <a:p><a:pPr algn=\"{}\" marL=\"0\" marR=\"0\"/><a:r><a:rPr {}>{}</a:rPr><a:t>{}</a:t></a:r></a:p>\
                    </p:txBody></p:sp>",
                    id, id, x, y, w, h, algn, rpr, rpr_body, t
                ));
            }
            PptxShape::Pic { x, y, cx, cy, id: pid, rel_id } => {
                out.push_str(&format!(
                    "<p:pic><p:nvPicPr><p:cNvPr id=\"{}\" name=\"Picture {}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>\
                    <p:blipFill><a:blip r:embed=\"rId{}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>\
                    <p:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>\
                    <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
                    pid, pid, rel_id, x, y, cx, cy
                ));
            }
        }
        id += 1;
    }
    out.push_str("</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>");
    out
}

/// 单页 slide 的关系文件：rId1=slideLayout，rId2..=media 图片。
fn pptx_slide_rels(media: &[(String, Vec<u8>)]) -> String {
    let mut rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>",
    );
    for (i, (name, _)) in media.iter().enumerate() {
        rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/{}\"/>",
            i + 2,
            name
        ));
    }
    rels.push_str("</Relationships>");
    rels
}

fn write_pptx(abs: &PathBuf, blocks: &[Value]) -> Result<(), String> {
    use zip::write::SimpleFileOptions;

    // ── 分页：slides（顶层 slide 结构每 slide 一页；扁平块自动拆页）──
    let slides_in = pptx_slides(blocks);
    let mut slides: Vec<String> = Vec::new();
    let mut slide_rels_list: Vec<String> = Vec::new();
    let mut slide_media: Vec<Vec<(String, Vec<u8>)>> = Vec::new();
    let mut shapes: Vec<PptxShape> = Vec::new();
    let mut media: Vec<(String, Vec<u8>)> = Vec::new();
    let mut rel_count = 1usize; // 每页 rId1=layout 预留
    let mut global_media_idx = 0usize; // media 文件名全局唯一（跨页不重名）
    let mut y = PPTX_MARGIN;
    let mut background: Option<String> = None;

    for slide_in in slides_in {
        // 每个顶层 slide 强制换页（首个 slide 用初始 y，后续在此重置）
        if !shapes.is_empty() {
            slides.push(pptx_slide_xml(&shapes, 2, &background));
            slide_rels_list.push(pptx_slide_rels(&media));
            slide_media.push(std::mem::take(&mut media));
            shapes.clear();
            rel_count = 1;
            y = PPTX_MARGIN;
        }
        background = slide_in.background;

        for item in slide_in.items {
            let need_h: i64 = match &item {
                PptxItem::Text { size, .. } => (size * 25_000.0) as i64 + 100_000,
                PptxItem::Image { w, h, .. } => {
                    let max_w = PPTX_SLIDE_W - 2 * PPTX_MARGIN;
                    let scale = if *w as i64 > max_w { max_w as f64 / *w as f64 } else { 1.0 };
                    (*h as f64 * 9525.0 * scale) as i64
                }
            };
            if y + need_h > PPTX_SLIDE_H - PPTX_MARGIN && !shapes.is_empty() {
                slides.push(pptx_slide_xml(&shapes, 2, &background));
                slide_rels_list.push(pptx_slide_rels(&media));
                slide_media.push(std::mem::take(&mut media));
                shapes.clear();
                rel_count = 1;
                y = PPTX_MARGIN;
            }
            match item {
                PptxItem::Text { size, text, bold, italic, color, align } => {
                    shapes.push(PptxShape::Text {
                        x: PPTX_MARGIN,
                        y,
                        w: PPTX_SLIDE_W - 2 * PPTX_MARGIN,
                        h: need_h,
                        size,
                        text: truncate_line(&text, 300),
                        bold,
                        italic,
                        color,
                        align,
                    });
                }
                PptxItem::Image { path, w, h } => {
                    let max_w = PPTX_SLIDE_W - 2 * PPTX_MARGIN;
                    let scale = if w as i64 > max_w { max_w as f64 / w as f64 } else { 1.0 };
                    let cx = (w as f64 * 9525.0 * scale) as i64;
                    let cy = (h as f64 * 9525.0 * scale) as i64;
                    rel_count += 1;
                    if let Ok(bytes) = std::fs::read(&path) {
                        global_media_idx += 1;
                        let ext = image_ext(&path);
                        media.push((format!("image{}.{}", global_media_idx, ext), bytes));
                        shapes.push(PptxShape::Pic {
                            x: PPTX_MARGIN,
                            y,
                            cx,
                            cy,
                            id: 2 + shapes.len() as i64,
                            rel_id: rel_count as u32,
                        });
                    }
                }
            }
            y += need_h;
        }
    }
    if !shapes.is_empty() {
        slides.push(pptx_slide_xml(&shapes, 2, &background));
        slide_rels_list.push(pptx_slide_rels(&media));
        slide_media.push(std::mem::take(&mut media));
    }
    if slides.is_empty() {
        slides.push(pptx_slide_xml(&[], 2, &background));
        slide_rels_list.push(pptx_slide_rels(&[]));
        slide_media.push(Vec::new());
    }

    // ── 动态关系/覆盖（随 slide 数量）──
    let mut sld_ids = String::new();
    let mut pres_rels = String::new();
    let mut content_overrides = String::new();
    for (i, _) in slides.iter().enumerate() {
        let n = i + 1;
        sld_ids.push_str(&format!("<p:sldId id=\"{}\" r:id=\"rId{}\"/>", 255 + n, n));
        pres_rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{}.xml\"/>",
            n, n
        ));
        content_overrides.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
            n
        ));
    }

    let content_types = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Default Extension=\"png\" ContentType=\"image/png\"/>\
        <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
        <Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
        <Default Extension=\"gif\" ContentType=\"image/gif\"/>\
        <Default Extension=\"webp\" ContentType=\"image/webp\"/>\
        <Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
        <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
        <Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>\
        <Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>\
        <Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/>\
        <Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/>\
        <Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>\
        {}</Types>",
        content_overrides
    );
    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/>\
        <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
        <Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/>\
        </Relationships>";
    let presentation = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <p:presentation xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
        xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
        <p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rIdMaster\"/></p:sldMasterIdLst>\
        <p:sldIdLst>{}</p:sldIdLst>\
        <p:sldSz cx=\"{}\" cy=\"{}\"/>\
        <p:notesSz cx=\"6858000\" cy=\"9144000\"/>\
        </p:presentation>",
        sld_ids, PPTX_SLIDE_W, PPTX_SLIDE_H
    );
    let presentation_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rIdMaster\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"slideMasters/slideMaster1.xml\"/>\
        {}</Relationships>",
        pres_rels
    );
    let master = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <p:sldMaster xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
        xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
        <p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val=\"bg1\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>\
        <p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
        <p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld>\
        <p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>\
        <p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rIdLayout1\"/></p:sldLayoutIdLst>\
        <p:txStyles><p:titleStyle><a:lvl1pPr/></p:titleStyle><p:bodyStyle><a:lvl1pPr/></p:bodyStyle><p:otherStyle/></p:txStyles>\
        </p:sldMaster>";
    let master_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rIdLayout1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>\
        <Relationship Id=\"rIdTheme1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"../theme/theme1.xml\"/>\
        </Relationships>";
    let layout = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <p:sldLayout xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
        xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" type=\"blank\">\
        <p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
        <p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld>\
        <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>\
        </p:sldLayout>";
    let layout_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"../slideMasters/slideMaster1.xml\"/>\
        </Relationships>";
    let core_props = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
        xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
        xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
        <dc:title>PilotDesk Presentation</dc:title><cp:revision>1</cp:revision></cp:coreProperties>";
    let app_props = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" \
        xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">\
        <Application>PilotDesk</Application><Slides>{}</Slides></Properties>",
        slides.len()
    );

    // ── 打包 ──
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default();
    let add = |writer: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>, name: &str, data: &[u8]| -> Result<(), String> {
        writer.start_file(name, options).map_err(|e| format!("打包失败（{}）: {}", name, e))?;
        writer.write_all(data).map_err(|e| format!("写入失败: {}", e))?;
        Ok(())
    };
    add(&mut writer, "[Content_Types].xml", content_types.as_bytes())?;
    add(&mut writer, "_rels/.rels", root_rels.as_bytes())?;
    add(&mut writer, "docProps/core.xml", core_props.as_bytes())?;
    add(&mut writer, "docProps/app.xml", app_props.as_bytes())?;
    add(&mut writer, "ppt/presentation.xml", presentation.as_bytes())?;
    add(&mut writer, "ppt/_rels/presentation.xml.rels", presentation_rels.as_bytes())?;
    add(&mut writer, "ppt/slideMasters/slideMaster1.xml", master.as_bytes())?;
    add(&mut writer, "ppt/slideMasters/_rels/slideMaster1.xml.rels", master_rels.as_bytes())?;
    add(&mut writer, "ppt/slideLayouts/slideLayout1.xml", layout.as_bytes())?;
    add(&mut writer, "ppt/slideLayouts/_rels/slideLayout1.xml.rels", layout_rels.as_bytes())?;
    add(&mut writer, "ppt/theme/theme1.xml", PPTX_THEME_XML.as_bytes())?;
    for (i, slide_xml) in slides.iter().enumerate() {
        let n = i + 1;
        add(&mut writer, &format!("ppt/slides/slide{}.xml", n), slide_xml.as_bytes())?;
        add(
            &mut writer,
            &format!("ppt/slides/_rels/slide{}.xml.rels", n),
            slide_rels_list[i].as_bytes(),
        )?;
    }
    for media in &slide_media {
        for (name, bytes) in media {
            add(&mut writer, &format!("ppt/media/{}", name), bytes)?;
        }
    }
    let cursor = writer.finish().map_err(|e| format!("PPTX 打包失败: {}", e))?;
    std::fs::write(abs, cursor.into_inner()).map_err(|e| format!("写入文件失败: {}", e))
}

// ── PDF（printpdf：内置/系统中文字体 + 基础分页排版）──

#[cfg(windows)]
fn system_font_candidates() -> Vec<String> {
    [
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simfang.ttf",
        "C:\\Windows\\Fonts\\simkai.ttf",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[cfg(not(windows))]
fn system_font_candidates() -> Vec<String> {
    ["/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn write_pdf(abs: &PathBuf, blocks: &[Value]) -> Result<(), String> {
    use printpdf::{BuiltinFont, Color, Mm, PdfDocument, Rgb};
    let (doc, page1, layer1) = PdfDocument::new(
        "PilotDesk Document",
        Mm(210.0),
        Mm(297.0),
        "Layer 1",
    );

    // 中文字体：优先系统 TTF（ttc 集合不被 rusttype 支持，故仅单字体 ttf 文件）
    let mut font = None;
    for cand in system_font_candidates() {
        if let Ok(data) = std::fs::read(&cand) {
            if let Ok(f) = doc.add_external_font(data.as_slice()) {
                font = Some(f);
                break;
            }
        }
    }
    let font_ref = match font {
        Some(f) => f,
        None => doc
            .add_builtin_font(BuiltinFont::Helvetica)
            .map_err(|e| format!("PDF 字体初始化失败: {}", e))?,
    };
    // 粗体：内置 Helvetica-Bold；中文字体（黑体/仿宋）本身厚重，正文粗体近似够用
    let bold_font = doc
        .add_builtin_font(BuiltinFont::HelveticaBold)
        .map_err(|e| format!("PDF 粗体字体初始化失败: {}", e))?;

    let margin = 20.0f64;
    let page_h = 297.0f64;
    let mut y = page_h - margin;
    let line_h = 6.5f64;
    let max_chars = 52usize;

    let mut page = doc.get_page(page1);
    let mut layer = page.get_layer(layer1);

    macro_rules! ensure_line {
        () => {
            if y < margin + line_h {
                let (p, l) = doc.add_page(Mm(210.0), Mm(297.0), "Layer 1");
                page = doc.get_page(p);
                layer = page.get_layer(l);
                y = page_h - margin;
            }
        };
    }

    for b in blocks {
        let (color, _align, bold, _italic, size) = block_style(b);
        let (fsize, text): (f64, String) = match b["type"].as_str() {
            Some("heading") => {
                let level = b["level"].as_u64().unwrap_or(1);
                let hsize = size.unwrap_or(if level <= 1 { 20.0 } else { 16.0 });
                (hsize, b["text"].as_str().unwrap_or("").to_string())
            }
            Some("table") => {
                let mut rows = Vec::new();
                if let Some(rs) = b["rows"].as_array() {
                    for r in rs {
                        if let Some(cells) = r.as_array() {
                            rows.push(
                                cells
                                    .iter()
                                    .map(|c| c.as_str().unwrap_or(""))
                                    .collect::<Vec<_>>()
                                    .join("  |  "),
                            );
                        }
                    }
                }
                (size.unwrap_or(10.0), rows.join("\n"))
            }
            Some("list") => {
                let items: Vec<String> = b["items"]
                    .as_array()
                    .map(|arr| arr.iter().map(|it| format!("- {}", it.as_str().unwrap_or(""))).collect())
                    .unwrap_or_default();
                (size.unwrap_or(12.0), items.join("\n"))
            }
            _ => (size.unwrap_or(12.0), b["text"].as_str().unwrap_or("").to_string()),
        };

        // 文字颜色（printpdf 通道 0-255）
        if let Some(hex) = &color {
            if let Some((r, g, bl)) = hex_to_rgb(hex) {
                layer.set_fill_color(Color::Rgb(Rgb::new(r, g, bl, None)));
            }
        } else {
            layer.set_fill_color(Color::Rgb(Rgb::new(0.0, 0.0, 0.0, None)));
        }
        let font = if bold { &bold_font } else { &font_ref };

        for line in text.lines() {
            if line.trim().is_empty() {
                y -= line_h * 0.5;
                continue;
            }
            ensure_line!();
            let shown = truncate_line(line, max_chars);
            layer.use_text(&shown, fsize as f32, Mm(margin as f32), Mm(y as f32), font);
            y -= if fsize >= 16.0 { line_h + 1.0 } else { line_h };
        }
        y -= line_h * 0.6;
    }

    let bytes = doc
        .save_to_bytes()
        .map_err(|e| format!("PDF 生成失败: {}", e))?;
    std::fs::write(abs, bytes).map_err(|e| format!("写入文件失败: {}", e))
}
