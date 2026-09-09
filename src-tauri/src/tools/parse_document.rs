//! 文档解析工具：PDF / DOCX / XLSX / PPTX 内容提取（文本、表格、图片）。
//!
//! 设计要点：
//! - **格式嗅探**用 magic bytes（PDF 魔数 / zip 包 + [Content_Types].xml 标识），不依赖扩展名；
//! - **排版保真**：blocks 模式按文档流顺序输出 `{type: paragraph|table|image}` 结构；
//! - **表格**：XLSX 用 calamine 读单元格矩阵；DOCX/PPTX 从 XML 提取表格；
//!   PDF 用 pdf-extract 文本流（按 form-feed 分页，表格降级为顺序文本）；
//! - **图片**：DOCX/PPTX 从包内 media/ 解出并落盘到 `<cwd>/attachments/<session_id>/`，返回路径；
//! - 输出受 32KB head+tail 截断保护（复用 exec.rs 策略），超大文档提示 page_range。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 输出内容上限（32KB，与命令输出一致）
const MAX_OUTPUT: usize = 32_768;

pub struct ParseDocumentTool {
    cwd: String,
    session_id: String,
}

impl ParseDocumentTool {
    pub fn new(cwd: String, session_id: String) -> Self {
        Self { cwd, session_id }
    }
}

/// 页范围解析（支持 `3`、`1-10`），返回 (start, end) 1-based 含端点；None 表示全部。
fn parse_page_range(spec: Option<&str>) -> Option<(usize, usize)> {
    let s = spec?.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split('-').collect();
    let a = parts[0].trim().parse::<usize>().ok()?;
    if parts.len() == 1 {
        Some((a, a))
    } else {
        let b = parts[1].trim().parse::<usize>().ok()?;
        Some((a, b))
    }
}

/// 截断输出（head+tail 32KB）
fn truncate_output(s: &str) -> String {
    if s.len() <= MAX_OUTPUT {
        return s.to_string();
    }
    let head_size = MAX_OUTPUT / 2;
    let tail_size = MAX_OUTPUT - head_size;
    let head_end = s
        .char_indices()
        .take_while(|&(i, _)| i < head_size)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    let tail_start = s
        .char_indices()
        .filter(|&(i, _)| i >= s.len() - tail_size)
        .next()
        .map_or(s.len(), |(i, _)| i);
    format!(
        "{}...\n[输出过长：已截断 {} 字符，总 {} 字符]\n...{}",
        &s[..head_end],
        s.len() - head_size - tail_size,
        s.len(),
        &s[tail_start..]
    )
}

fn sniff_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF-") {
        return Some("pdf");
    }
    if bytes.starts_with(b"PK\x03\x04") {
        // zip 包：按 [Content_Types].xml 判定 Office 类型
        if let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())) {
            if let Ok(mut ct) = zip.by_name("[Content_Types].xml") {
                let mut buf = String::new();
                use std::io::Read;
                let _ = ct.read_to_string(&mut buf);
                if buf.contains("wordprocessingml") {
                    return Some("docx");
                }
                if buf.contains("spreadsheetml") {
                    return Some("xlsx");
                }
                if buf.contains("presentationml") {
                    return Some("pptx");
                }
            }
        }
    }
    None
}

/// 从 XML 中提取某标签的全部文本（如 `<w:t>`），连接成一行。
fn extract_tag_texts(xml: &str, tag: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        if let Some(j) = after.find(&close) {
            out.push_str(&after[..j]);
            rest = &after[j + close.len()..];
        } else {
            break;
        }
    }
    out
}

/// 提取 DOCX 的 XML 文本节点：<w:t> 之间可能有转义，直接拼接（docx 转义实体极少，够用）。
fn extract_wt(xml: &str) -> String {
    extract_tag_texts(xml, "w:t")
}

/// 提取 PPTX 的 XML 文本节点：<a:t>。
fn extract_at(xml: &str) -> String {
    extract_tag_texts(xml, "a:t")
}

/// DOCX 段落扫描：按顶层元素（<w:p> 段落 / <w:tbl> 表格）顺序输出 blocks。
fn scan_docx(xml: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut i = 0;
    let n = xml.len();
    while i < n {
        let rest = &xml[i..];
        let p = rest.find("<w:p>").or_else(|| rest.find("<w:p "));
        let tbl = rest.find("<w:tbl>");
        match (p, tbl) {
            (None, None) => break,
            // 仅表格（无段落标签）
            (None, Some(t)) => {
                let close = rest.find("</w:tbl>");
                if let Some(c) = close {
                    blocks.push(table_block_from_xml(&rest[t..c]));
                    i += c + "</w:tbl>".len();
                } else {
                    i = n;
                }
            }
            // 表格在前：提取到 </w:tbl>
            (Some(pp), Some(tt)) if tt < pp => {
                let close = rest.find("</w:tbl>");
                if let Some(c) = close {
                    blocks.push(table_block_from_xml(&rest[tt..c]));
                    i += c + "</w:tbl>".len();
                } else {
                    i = n;
                }
            }
            // 段落在前：提取 <w:p> 内容（到 </w:p>，跳过内嵌表格的重复处理）
            (Some(pp), _) => {
                let close = rest.find("</w:p>");
                let (seg, next) = match close {
                    Some(c) => (&rest[pp..c], c + "</w:p>".len()),
                    None => (&rest[pp..], n - i),
                };
                let text = extract_wt(seg).trim().to_string();
                if !text.is_empty() {
                    blocks.push(json!({"type": "paragraph", "text": text}));
                }
                i += next;
            }
        }
    }
    blocks
}

/// 从 <w:tbl> 区域提取行/列文本矩阵。
fn table_block_from_xml(table_xml: &str) -> Value {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut rest = table_xml;
    while let Some(tr) = rest.find("<w:tr>") {
        let after = &rest[tr + "<w:tr>".len()..];
        let end = match after.find("</w:tr>") {
            Some(e) => e,
            None => break,
        };
        let tr_xml = &after[..end];
        // 单元格：<w:tc>...</w:tc>
        let mut cells = Vec::new();
        let mut cr = tr_xml;
        while let Some(tc) = cr.find("<w:tc>") {
            let cafter = &cr[tc + "<w:tc>".len()..];
            let cend = match cafter.find("</w:tc>") {
                Some(e) => e,
                None => break,
            };
            cells.push(extract_wt(&cafter[..cend]).trim().to_string());
            cr = &cafter[cend + "</w:tc>".len()..];
        }
        rows.push(cells);
        rest = &after[end + "</w:tr>".len()..];
    }
    json!({"type": "table", "rows": rows})
}

/// PPTX 单页扫描：<a:p> 段落 + <a:t> 文本。
fn scan_pptx_slide(xml: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut rest = xml;
    while let Some(p) = rest.find("<a:p>") {
        let after = &rest[p + "<a:p>".len()..];
        let end = match after.find("</a:p>") {
            Some(e) => e,
            None => break,
        };
        let text = extract_at(&after[..end]).trim().to_string();
        if !text.is_empty() {
            blocks.push(json!({"type": "paragraph", "text": text}));
        }
        rest = &after[end + "</a:p>".len()..];
    }
    blocks
}

/// 从 zip 包解出 media/ 图片并落盘，返回已写入的路径列表。
fn extract_images(
    zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
    dir: &Path,
    prefix: &str,
    limit: usize,
) -> Vec<String> {
    use std::io::Read;
    let mut paths = Vec::new();
    let mut saved = 0usize;
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| {
            let f = zip.by_index(i).ok()?;
            let n = f.name().to_string();
            (n.starts_with("word/media/") || n.starts_with("ppt/media/")).then_some(n)
        })
        .collect();
    for name in names {
        if saved >= limit {
            break;
        }
        let Ok(mut f) = zip.by_name(&name) else { continue };
        let mut bytes = Vec::new();
        let _ = f.read_to_end(&mut bytes);
        if bytes.is_empty() {
            continue;
        }
        let ext = Path::new(&name)
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "bin".to_string());
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let file = dir.join(format!("{}_{}_{}.{}", prefix, ts, saved, ext));
        if std::fs::write(&file, &bytes).is_ok() {
            paths.push(file.to_string_lossy().to_string());
            saved += 1;
        }
    }
    paths
}

#[async_trait]
impl ToolHandler for ParseDocumentTool {
    fn name(&self) -> &str {
        "parse_document"
    }

    fn description(&self) -> &str {
        "解析文档内容（PDF/Word/Excel/PPT）：提取文本、表格与图片并保持基本排版顺序。\
         传 file 指定文档绝对路径或相对工作目录路径；output=blocks 返回结构化 JSON（默认）、text 返回纯文本、\
         markdown 返回 Markdown。表格以矩阵输出，图片提取后落盘返回路径。\
         若连续 2 次调用失败，请停止自动重试，向用户确认三选一：修改参数后重试、使用相同参数再试一次、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "待解析文档路径（绝对路径或相对工作目录路径），支持 pdf/docx/xlsx/pptx"
                },
                "output": {
                    "type": "string",
                    "enum": ["blocks", "text", "markdown"],
                    "description": "输出格式：blocks=结构化 JSON（默认，含段落/表格/图片与顺序）、text=纯文本、markdown=Markdown"
                },
                "include_images": {
                    "type": "boolean",
                    "description": "是否提取并落盘文档内嵌图片（默认 true；docx/pptx 有效，pdf 不支持图片提取）"
                },
                "page_range": {
                    "type": "string",
                    "description": "可选：仅解析指定页（PDF），如 \"3\" 或 \"1-10\"；省略解析全部"
                },
                "table_format": {
                    "type": "string",
                    "enum": ["json", "csv", "markdown"],
                    "description": "表格输出格式（默认 json）"
                }
            },
            "required": ["file"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Filesystem, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let file = arguments["file"].as_str().ok_or("缺少 file 参数")?;
        let output = arguments["output"].as_str().unwrap_or("blocks").to_string();
        let include_images = arguments["include_images"].as_bool().unwrap_or(true);
        let page_range = arguments["page_range"].as_str();
        let table_format = arguments["table_format"].as_str().unwrap_or("json").to_string();

        let path = PathBuf::from(file.trim());
        let abs = if path.is_absolute() {
            path
        } else {
            PathBuf::from(&self.cwd).join(&path)
        };
        if !abs.exists() {
            return Err(format!("文档不存在: {}", abs.display()));
        }
        let bytes = std::fs::read(&abs).map_err(|e| format!("读取文档失败: {}", e))?;
        let format = sniff_format(&bytes).ok_or_else(|| {
            "无法识别的文档格式：支持 PDF（%PDF）、DOCX/XLSX/PPTX（Office 包）。旧版 doc/xls/ppt 暂不支持。".to_string()
        })?;

        let result = match format {
            "pdf" => self.parse_pdf(&bytes, page_range, &table_format).await?,
            "xlsx" => self.parse_xlsx(&abs, &table_format).await?,
            "docx" => self.parse_docx(&bytes, include_images, &table_format).await?,
            "pptx" => self.parse_pptx(&bytes, include_images, &table_format).await?,
            _ => return Err(format!("暂不支持的格式: {}", format)),
        };

        // 按 output 模式渲染
        let rendered = match output.as_str() {
            "text" => render_text(&result),
            "markdown" => render_markdown(&result, &table_format),
            _ => render_blocks(&result),
        };
        Ok(truncate_output(&rendered))
    }
}

// ── 各格式解析 ──

impl ParseDocumentTool {
    /// 图片落盘目录（按会话隔离，复用附件模型 `<cwd>/attachments/<session_id>`）。
    fn image_dir(&self) -> PathBuf {
        let base = if self.session_id.is_empty() {
            self.cwd.clone()
        } else {
            format!("{}/attachments/{}", self.cwd, self.session_id)
        };
        PathBuf::from(&base)
    }

    async fn parse_pdf(
        &self,
        bytes: &[u8],
        page_range: Option<&str>,
        _table_format: &str,
    ) -> Result<Vec<Value>, String> {
        let text = pdf_extract::extract_text_from_mem(bytes)
            .map_err(|e| format!("PDF 解析失败: {}", e))?;
        // 分页（pdf-extract 以 form-feed \x0c 分隔页）；过滤页范围
        let pages: Vec<&str> = text.split('\x0c').collect();
        let range = parse_page_range(page_range);
        let mut blocks = Vec::new();
        for (i, page) in pages.iter().enumerate() {
            let page_no = i + 1;
            if let Some((a, b)) = range {
                if page_no < a || page_no > b {
                    continue;
                }
            }
            for para in page.split('\n') {
                let t = para.trim();
                if !t.is_empty() {
                    blocks.push(json!({"type": "paragraph", "text": t, "page": page_no}));
                }
            }
        }
        if blocks.is_empty() && range.is_some() {
            return Err("指定页范围未提取到内容（可能超出总页数）".to_string());
        }
        Ok(blocks)
    }

    async fn parse_xlsx(
        &self,
        path: &Path,
        table_format: &str,
    ) -> Result<Vec<Value>, String> {
        use calamine::Reader;
        let mut wb = calamine::open_workbook_auto(path)
            .map_err(|e| format!("Excel 打开失败: {}", e))?;
        let mut blocks = Vec::new();
        for name in wb.sheet_names() {
            let range = wb
                .worksheet_range(&name)
                .map_err(|e| format!("读取工作表 {} 失败: {}", name, e))?;
            let mut rows: Vec<Vec<String>> = Vec::new();
            for row in range.rows() {
                // calamine 0.26 的 DataType 为 trait（非枚举），无法按变体 match；
                // 统一按 Debug 输出单元格内容（String/Float/Int/Bool 均可读）。
                let cells: Vec<String> = row.iter().map(|c| format!("{:?}", c)).collect();
                rows.push(cells);
            }
            let _ = table_format;
            blocks.push(json!({
                "type": "table",
                "sheet": name,
                "rows": rows
            }));
        }
        Ok(blocks)
    }

    async fn parse_docx(
        &self,
        bytes: &[u8],
        include_images: bool,
        _table_format: &str,
    ) -> Result<Vec<Value>, String> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))
            .map_err(|e| format!("DOCX 打开失败: {}", e))?;
        let mut xml = String::new();
        {
            let mut f = zip
                .by_name("word/document.xml")
                .map_err(|e| format!("读取 document.xml 失败: {}", e))?;
            use std::io::Read;
            f.read_to_string(&mut xml).map_err(|e| format!("读取 XML 失败: {}", e))?;
        }
        let mut blocks = scan_docx(&xml);
        // 图片提取
        if include_images {
            let dir = self.image_dir();
            let _ = std::fs::create_dir_all(&dir);
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))
                .map_err(|e| format!("重新打开 DOCX 失败: {}", e))?;
            let paths = extract_images(&mut zip, &dir, "docx", 8);
            for p in paths {
                blocks.push(json!({"type": "image", "path": p}));
            }
        }
        Ok(blocks)
    }

    async fn parse_pptx(
        &self,
        bytes: &[u8],
        include_images: bool,
        _table_format: &str,
    ) -> Result<Vec<Value>, String> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))
            .map_err(|e| format!("PPTX 打开失败: {}", e))?;
        // 按 slide 编号排序读取文本
        let mut slides: Vec<(u32, String)> = Vec::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| format!("读取包内文件失败: {}", e))?;
            let name = f.name().to_string();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                let num: u32 = name
                    .trim_start_matches("ppt/slides/slide")
                    .trim_end_matches(".xml")
                    .parse()
                    .unwrap_or(0);
                let mut xml = String::new();
                use std::io::Read;
                f.read_to_string(&mut xml).map_err(|e| format!("读取 slide 失败: {}", e))?;
                slides.push((num, xml));
            }
        }
        slides.sort_by_key(|(n, _)| *n);
        let mut blocks = Vec::new();
        for (num, xml) in slides {
            let mut page = scan_pptx_slide(&xml);
            for b in &mut page {
                b["slide"] = json!(num);
            }
            blocks.append(&mut page);
        }
        // 图片提取
        if include_images {
            let dir = self.image_dir();
            let _ = std::fs::create_dir_all(&dir);
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))
                .map_err(|e| format!("重新打开 PPTX 失败: {}", e))?;
            let paths = extract_images(&mut zip, &dir, "pptx", 8);
            for p in paths {
                blocks.push(json!({"type": "image", "path": p}));
            }
        }
        Ok(blocks)
    }
}

// ── 渲染 ──

fn render_blocks(blocks: &[Value]) -> String {
    serde_json::to_string_pretty(&json!({ "blocks": blocks }))
        .unwrap_or_else(|_| "序列化失败".to_string())
}

fn render_text(blocks: &[Value]) -> String {
    let mut out = String::new();
    for b in blocks {
        match b["type"].as_str() {
            Some("paragraph") => out.push_str(b["text"].as_str().unwrap_or("")),
            Some("table") => {
                if let Some(rows) = b["rows"].as_array() {
                    for row in rows {
                        let cells: Vec<&str> = row
                            .as_array()
                            .map(|r| r.iter().map(|c| c.as_str().unwrap_or("")).collect())
                            .unwrap_or_default();
                        out.push_str(&cells.join("\t"));
                        out.push('\n');
                    }
                }
            }
            Some("image") => {
                if let Some(p) = b["path"].as_str() {
                    out.push_str(&format!("[图片] {}\n", p));
                }
            }
            _ => {}
        }
        out.push('\n');
    }
    out
}

fn render_markdown(blocks: &[Value], _table_format: &str) -> String {
    let mut out = String::new();
    for b in blocks {
        match b["type"].as_str() {
            Some("paragraph") => {
                out.push_str(b["text"].as_str().unwrap_or(""));
                out.push_str("\n\n");
            }
            Some("table") => {
                if let Some(rows) = b["rows"].as_array() {
                    let header: Vec<String> = rows
                        .first()
                        .map(|r| {
                            r.as_array()
                                .map(|cells| cells.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect())
                                .unwrap_or_default()
                        })
                        .unwrap_or_default();
                    out.push('|');
                    for h in &header {
                        out.push_str(&format!(" {} |", h));
                    }
                    out.push('\n');
                    out.push('|');
                    for _ in &header {
                        out.push_str(" --- |");
                    }
                    out.push('\n');
                    for row in rows.iter().skip(1) {
                        out.push('|');
                        if let Some(cells) = row.as_array() {
                            for c in cells {
                                out.push_str(&format!(" {} |", c.as_str().unwrap_or("")));
                            }
                        }
                        out.push('\n');
                    }
                    out.push('\n');
                }
            }
            Some("image") => {
                if let Some(p) = b["path"].as_str() {
                    out.push_str(&format!("![图片]({})\n\n", p));
                }
            }
            _ => {}
        }
    }
    out
}
