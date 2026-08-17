use std::borrow::Cow;
use std::path::{Path, PathBuf};
use tauri::http::header::CONTENT_TYPE;
use tauri::http::{Request, Response, StatusCode};
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};

/// 判断路径是否为目录（供前端自定义标签决定 iframe 使用目录索引协议还是 asset 文件协议）
#[tauri::command]
pub fn path_is_directory(path: String) -> bool {
    std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false)
}

/// HTML 转义（防止路径/文件名中的特殊字符破坏页面结构）
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 对路径做 URL query 编码（与前端 encodeURIComponent 语义一致）
fn encode_query(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// 从 query 中解析 path 参数（URL 解码）
fn parse_path_param(query: Option<&str>) -> Option<String> {
    let q = query?;
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            if k == "path" {
                return percent_decode_str(v).decode_utf8().ok().map(|s| s.into_owned());
            }
        }
    }
    None
}

/// 目录索引页样式（跟随系统深浅色）
const INDEX_CSS: &str = r#"
  :root { --bg:#fff; --fg:#1f2328; --muted:#6e7781; --border:#d0d7de; --accent:#0969da; }
  @media (prefers-color-scheme: dark) {
    :root { --bg:#0d1117; --fg:#e6edf3; --muted:#8b949e; --border:#30363d; --accent:#58a6ff; }
  }
  * { box-sizing: border-box; }
  body { margin:0; padding:16px 20px; background:var(--bg); color:var(--fg); font:13px/1.6 system-ui,-apple-system,"Segoe UI","Microsoft YaHei",sans-serif; }
  h1 { font-size:15px; font-weight:600; margin:0 0 8px; word-break:break-all; }
  .parent { margin:0 0 4px; }
  .parent a { color:var(--accent); text-decoration:none; }
  ul { list-style:none; margin:8px 0 0; padding:0; }
  li { border-bottom:1px solid var(--border); }
  a { display:block; padding:6px 8px; border-radius:6px; color:var(--fg); text-decoration:none; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
  a:hover { background:color-mix(in srgb, var(--accent) 10%, transparent); }
"#;

/// 渲染目录索引主体（子目录 + 文件列表）
fn render_dir_index(dir: &Path) -> String {
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                dirs.push(name);
            } else {
                files.push(name);
            }
        }
    }
    let cmp_ci = |a: &String, b: &String| a.to_lowercase().cmp(&b.to_lowercase());
    dirs.sort_by(cmp_ci);
    files.sort_by(cmp_ci);

    let mut body = String::new();
    body.push_str(&format!("<h1>{}</h1>", html_escape(&dir.display().to_string())));
    if let Some(parent) = dir.parent() {
        body.push_str(&format!(
            "<p class=\"parent\"><a href=\"?path={}\">↑ 上级目录</a></p>",
            encode_query(&parent.display().to_string())
        ));
    }
    body.push_str("<ul>");
    for d in &dirs {
        let child = dir.join(d);
        body.push_str(&format!(
            "<li class=\"dir\"><a href=\"?path={}\" title=\"{}\">📁 {}</a></li>",
            encode_query(&child.display().to_string()),
            html_escape(&child.display().to_string()),
            html_escape(d)
        ));
    }
    for f in &files {
        let child = dir.join(f);
        body.push_str(&format!(
            "<li class=\"file\"><a href=\"?path={}\" title=\"{}\">📄 {}</a></li>",
            encode_query(&child.display().to_string()),
            html_escape(&child.display().to_string()),
            html_escape(f)
        ));
    }
    body.push_str("</ul>");
    body
}

/// 按扩展名猜测文件 Content-Type
fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("txt" | "md" | "log" | "csv") => "text/plain; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// 错误页（友好的 HTML 响应）
fn error_page(message: &str, status: StatusCode) -> Response<Cow<'static, [u8]>> {
    let html = format!(
        "<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>错误</title></head>\
         <body style=\"font:13px system-ui,sans-serif;padding:24px;color:#1f2328\">\
         <p>⚠️ {}</p></body></html>",
        html_escape(message)
    );
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Cow::Owned(html.into_bytes()))
        .unwrap()
}

/// 单文件读取大小上限（避免把超大文件整个读入内存）
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// dirindex 自定义协议 handler：
/// 访问 `http://dirindex.localhost/?path=<urlencoded 路径>`。
/// - 目录：返回文件列表索引页（目录/文件链接均为 ?path= 形式，可继续导航）
/// - 文件：直接返回文件内容（按扩展名给 Content-Type）
pub fn handle_dirindex(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let Some(path_str) = parse_path_param(request.uri().query()) else {
        return error_page("缺少 path 参数", StatusCode::BAD_REQUEST);
    };
    if path_str.is_empty() {
        return error_page("path 为空", StatusCode::BAD_REQUEST);
    }

    let p = PathBuf::from(&path_str);
    let meta = match std::fs::metadata(&p) {
        Ok(m) => m,
        Err(e) => {
            return error_page(
                &format!("无法访问 {}：{}", path_str, e),
                StatusCode::NOT_FOUND,
            )
        }
    };

    if meta.is_dir() {
        let html = format!(
            "<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>{}</title>\
             <style>{}</style></head><body>{}</body></html>",
            html_escape(&p.display().to_string()),
            INDEX_CSS,
            render_dir_index(&p)
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Cow::Owned(html.into_bytes()))
            .unwrap();
    }

    // 文件
    if meta.len() > MAX_FILE_BYTES {
        return error_page(
            &format!(
                "文件过大（{}），超过 {} MB 上限",
                path_str,
                MAX_FILE_BYTES / 1024 / 1024
            ),
            StatusCode::PAYLOAD_TOO_LARGE,
        );
    }
    match std::fs::read(&p) {
        Ok(bytes) => {
            let ct = content_type_for(&p);
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, ct)
                .body(Cow::Owned(bytes))
                .unwrap()
        }
        Err(e) => error_page(
            &format!("无法读取 {}：{}", path_str, e),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}
