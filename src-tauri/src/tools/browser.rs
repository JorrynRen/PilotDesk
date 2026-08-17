//! 浏览器自动化工具：使用本机 Edge/Chrome 无头模式抓取网页内容与截图。
//!
//! 这是「浏览器自动化」能力的轻量实现——不引入 Playwright/Puppeteer 等重依赖，
//! 直接复用 Windows 上几乎必定存在的 Microsoft Edge（或 Google Chrome）无头模式：
//!   - `fetch`      ：渲染页面并抓取可读文本（等价于 `--headless --dump-dom` + 去标签）
//!   - `screenshot` ：渲染页面并保存整页截图到本地文件
//!
//! 更复杂的点击/填表等交互自动化，可复用本项目的 MCP 客户端接入 Playwright MCP 等服务器。
// 自 api_agent/browser.rs 迁入（工具架构统一 v1.0，轮 7），逻辑不变。

use crate::tools::{RiskLevel, ToolHandler};
use async_trait::async_trait;
use std::os::windows::process::CommandExt;

/// 单次浏览器操作超时（秒）
const BROWSER_TIMEOUT_SECS: u64 = 45;
/// 抓取正文的最大输出字节数（超过则截断）
const MAX_TEXT_OUTPUT: usize = 16_000;

/// 浏览器自动化工具
pub struct BrowserTool {
    cwd: String,
}

impl BrowserTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl ToolHandler for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "使用本机 Edge/Chrome 无头浏览器访问网页。支持两种操作：\
'fetch' 渲染并抓取页面可读文本（适合阅读/提取网页内容），\
'screenshot' 渲染并保存整页截图为 PNG 文件（适合截图存档）。\
适用于访问普通公开网页、查看渲染后的页面内容；无法绕过登录或验证码。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["fetch", "screenshot"],
                    "description": "操作类型：fetch=抓取页面文本；screenshot=保存截图"
                },
                "url": {
                    "type": "string",
                    "description": "要访问的 URL，必须以 http:// 或 https:// 开头"
                },
                "path": {
                    "type": "string",
                    "description": "截图保存路径（仅 screenshot 需要；绝对路径或相对工作区路径，默认 pilotdesk_screenshot.png）"
                },
                "width": {
                    "type": "integer",
                    "description": "视口宽度（像素），默认 1280"
                },
                "height": {
                    "type": "integer",
                    "description": "视口高度（像素），默认 900"
                }
            },
            "required": ["action", "url"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Medium
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let action = arguments["action"]
            .as_str()
            .ok_or("缺少 action 参数")?
            .to_string();
        let url = arguments["url"]
            .as_str()
            .ok_or("缺少 url 参数")?
            .to_string();

        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("url 必须以 http:// 或 https:// 开头".to_string());
        }

        let path = arguments["path"].as_str().map(|s| s.to_string());
        let width = arguments["width"].as_u64().unwrap_or(1280).clamp(320, 3840) as u32;
        let height = arguments["height"].as_u64().unwrap_or(900).clamp(240, 2160) as u32;
        let cwd = self.cwd.clone();

        tokio::task::spawn_blocking(move || {
            let browser = find_browser().ok_or_else(|| {
                "未检测到 Microsoft Edge 或 Google Chrome 浏览器，无法执行浏览器自动化。\
请安装其中之一后重试。"
                    .to_string()
            })?;

            match action.as_str() {
                "fetch" => browser_fetch(&browser, &url),
                "screenshot" => browser_screenshot(
                    &browser,
                    &url,
                    &cwd,
                    path.as_deref().unwrap_or("pilotdesk_screenshot.png"),
                    width,
                    height,
                ),
                other => Err(format!("未知的浏览器操作: {}", other)),
            }
        })
        .await
        .map_err(|e| format!("浏览器任务被中断: {}", e))?
    }
}

/// 定位本机可用的 Edge / Chrome 可执行文件
pub(crate) fn find_browser() -> Option<String> {
    const CANDIDATES: &[&str] = &[
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    for c in CANDIDATES {
        if std::path::Path::new(c).is_file() {
            return Some(c.to_string());
        }
    }
    // 回退：尝试在 PATH 中查找
    for exe in ["msedge", "chrome"] {
        if let Ok(out) = std::process::Command::new("where")
            .arg(exe)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                let p = line.trim();
                if !p.is_empty() && std::path::Path::new(p).is_file() {
                    return Some(p.to_string());
                }
            }
        }
    }
    None
}

/// 运行浏览器子进程，返回 (stdout, stderr, exit_code)，带超时兜底
pub(crate) fn run_browser(browser: &str, args: &[String]) -> Result<(Vec<u8>, Vec<u8>, Option<i32>), String> {
    let mut child = std::process::Command::new(browser)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|e| format!("启动浏览器失败: {}", e))?;

    // 将 stdout/stderr 移到读取线程，避免大输出（如 dump-dom）填满管道缓冲区导致子进程阻塞
    let stdout = child.stdout.take().expect("stdout 已 piped");
    let stderr = child.stderr.take().expect("stderr 已 piped");
    let stdout_handle = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = std::io::BufReader::new(stdout).read_to_end(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = std::io::BufReader::new(stderr).read_to_end(&mut buf);
        buf
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(BROWSER_TIMEOUT_SECS);
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "浏览器操作超时（{} 秒），已强制终止",
                        BROWSER_TIMEOUT_SECS
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("浏览器进程异常: {}", e));
            }
        }
    };

    let stdout_buf = stdout_handle.join().unwrap_or_default();
    let stderr_buf = stderr_handle.join().unwrap_or_default();
    Ok((stdout_buf, stderr_buf, exit_code))
}

/// 抓取页面并提取可读文本
fn browser_fetch(browser: &str, url: &str) -> Result<String, String> {
    let args: Vec<String> = vec![
        "--headless".to_string(),
        "--disable-gpu".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--hide-scrollbars".to_string(),
        "--mute-audio".to_string(),
        "--virtual-time-budget=8000".to_string(),
        "--window-size=1280,900".to_string(),
        "--dump-dom".to_string(),
        url.to_string(),
    ];

    let (stdout, stderr, _code) = run_browser(browser, &args)?;
    let html = crate::decode_windows_output(&stdout);
    if html.trim().is_empty() {
        return Err(format!(
            "浏览器未返回页面内容。stderr: {}",
            crate::decode_windows_output(&stderr).trim()
        ));
    }

    let text = html_to_text(&html);
    if text.trim().is_empty() {
        return Err("页面内容为空或无法解析（可能是需要登录、存在反爬或 JS 渲染超时）".to_string());
    }

    Ok(truncate_utf8(&text, MAX_TEXT_OUTPUT).to_string())
}

/// 渲染页面并保存整页截图
fn browser_screenshot(
    browser: &str,
    url: &str,
    cwd: &str,
    path: &str,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let abs = crate::resolve_workspace_path(path, cwd);
    if let Some(parent) = abs.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建截图目录失败 ({}): {}", parent.display(), e))?;
        }
    }

    let args: Vec<String> = vec![
        "--headless".to_string(),
        "--disable-gpu".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--hide-scrollbars".to_string(),
        "--mute-audio".to_string(),
        format!("--window-size={},{}", width, height),
        format!("--screenshot={}", abs.display()),
        url.to_string(),
    ];

    let (_stdout, stderr, _code) = run_browser(browser, &args)?;
    if !abs.is_file() {
        return Err(format!(
            "截图失败，未生成文件。stderr: {}",
            crate::decode_windows_output(&stderr).trim()
        ));
    }

    Ok(format!("截图已保存: {}", abs.display()))
}

/// 将 HTML 转换为可读纯文本（去除脚本/样式/标签，解码常见实体，折叠空白）
pub(crate) fn html_to_text(html: &str) -> String {
    // 去除 script/style/noscript 及其内容（Rust regex 不支持反向引用，逐类匹配）
    let mut html = html.to_string();
    for tag in ["script", "style", "noscript"] {
        let re = regex::Regex::new(&format!(r"(?is)<{}[^>]*>.*?</{}>", tag, tag)).unwrap();
        html = re.replace_all(&html, " ").to_string();
    }
    // 去除注释
    let re_comment = regex::Regex::new(r"(?s)<!--.*?-->").unwrap();
    let html = re_comment.replace_all(&html, "");
    // 块级元素换行
    let re_br = regex::Regex::new(
        r"(?i)<(br\s*/?>|/p>|/div>|/li>|/tr>|/h[1-6]>|/section>|/article>|/blockquote>)",
    )
    .unwrap();
    let html = re_br.replace_all(&html, "\n");
    // 去除所有剩余标签
    let re_tag = regex::Regex::new(r"<[^>]+>").unwrap();
    let text = re_tag.replace_all(&html, "");

    // 解码常见 HTML 实体
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ");

    // 逐行折叠空白并丢弃空行
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !trimmed.is_empty() {
            out.push_str(&trimmed);
            out.push('\n');
        }
    }
    out
}

/// 安全截断 UTF-8 字符串至指定字节数（边界对齐到字符）
pub(crate) fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if i + c.len_utf8() > max_bytes {
            break;
        }
        end = i + c.len_utf8();
    }
    &s[..end]
}
