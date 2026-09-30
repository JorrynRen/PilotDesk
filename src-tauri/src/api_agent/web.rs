//! 联网搜索共享逻辑：搜索结果结构、搜索后端分发、Bing/Tavily 解析与网页文本抽取。
//!
//! 后端选择（由 `SearchConfig.provider` 决定）：
//!   - `bing_cn`   ：Bing 中国版（cn.bing.com）免 API Key，reqwest 快速通道 + 无头浏览器兜底（默认）
//!   - `tavily`    ：Tavily Search API（需 API Key）
//!   - `bing_api`  ：Bing Web Search API（需 API Key，需在 Azure 开通）
//!
//! 本模块不负责工具注册（见 `search_web.rs` / `fetch_web.rs`）与配置持久化（见 `commands/search.rs`）。

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::tools::browser;

/// 单条搜索结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// 搜索后端
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    /// Bing 中国版（免 key，默认）
    #[default]
    BingCn,
    /// Tavily Search API
    Tavily,
    /// Bing Web Search API
    BingApi,
}

/// 搜索配置（持久化在 app_settings 的 `search_config` key 下）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchConfig {
    pub provider: SearchProvider,
    #[serde(default)]
    pub tavily_api_key: String,
    #[serde(default)]
    pub bing_api_key: String,
}

/// 网页抓取的最大输出字节数
const MAX_FETCH_OUTPUT: usize = 16_000;
/// 搜索请求超时（秒）
const HTTP_TIMEOUT_SECS: u64 = 15;
/// 搜索引擎请求使用的 User-Agent（避免部分反爬直接拦截默认 UA）
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 对查询词做 URL 编码（percent-encoding，UTF-8）
fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// 执行搜索并返回结果列表（num 限制在 1..=20）
pub async fn search_web(
    cfg: &SearchConfig,
    query: &str,
    num: usize,
) -> Result<Vec<SearchResult>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("搜索关键词不能为空".to_string());
    }
    let num = num.clamp(1, 20);

    match cfg.provider {
        SearchProvider::BingCn => search_bing_cn(query, num).await,
        SearchProvider::Tavily => search_tavily(cfg, query, num).await,
        SearchProvider::BingApi => search_bing_api(cfg, query, num).await,
    }
}

// ──────────────────────────────────────────────
//  Bing 中国版（免 key）
// ──────────────────────────────────────────────

/// 组装 Bing 中国版搜索 URL
fn bing_cn_url(query: &str, num: usize) -> String {
    format!(
        "https://cn.bing.com/search?q={}&count={}&setlang=zh-CN",
        urlencode(query),
        num
    )
}

/// Bing 中国版：先走 reqwest 快速通道，失败/空结果时降级到无头浏览器。
async fn search_bing_cn(query: &str, num: usize) -> Result<Vec<SearchResult>, String> {
    // 快速通道
    if let Ok(results) = fetch_bing_cn(query, num).await {
        if !results.is_empty() {
            return Ok(results);
        }
    }

    // 无头浏览器兜底（同步阻塞，移入专用线程）
    let query_owned = query.to_string();
    tokio::task::spawn_blocking(move || search_bing_cn_fallback(&query_owned, num))
        .await
        .map_err(|e| format!("浏览器兜底任务被中断: {}", e))?
}

/// reqwest 快速通道：GET Bing 中国版并解析 `li.b_algo`。
async fn fetch_bing_cn(query: &str, num: usize) -> Result<Vec<SearchResult>, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(bing_cn_url(query, num))
        .send()
        .await
        .map_err(|e| format!("Bing 请求失败: {}", e))?;

    let status = resp.status();
    let html = resp
        .text()
        .await
        .map_err(|e| format!("读取 Bing 响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!("Bing 返回 HTTP {}", status.as_u16()));
    }
    if is_bot_blocked(&html) {
        return Err("Bing 检测到反爬拦截".to_string());
    }

    let results = parse_bing_html(&html, num);
    if results.is_empty() {
        return Err("Bing 未解析到搜索结果".to_string());
    }
    Ok(results)
}

/// 无头浏览器兜底：用本机 Edge/Chrome 渲染 Bing 结果页并解析。
fn search_bing_cn_fallback(query: &str, num: usize) -> Result<Vec<SearchResult>, String> {
    let browser = browser::find_browser()
        .ok_or_else(|| "搜索失败：未检测到 Edge/Chrome 浏览器用于兜底".to_string())?;

    let args: Vec<String> = vec![
        "--headless".to_string(),
        "--disable-gpu".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--virtual-time-budget=8000".to_string(),
        "--dump-dom".to_string(),
        bing_cn_url(query, num),
    ];

    let (stdout, _stderr, _code) = browser::run_browser(&browser, &args)?;
    let html = crate::decode_windows_output(&stdout);
    let results = parse_bing_html(&html, num);
    if results.is_empty() {
        return Err("搜索失败：Bing 中国版未返回有效结果（可能被反爬拦截）".to_string());
    }
    Ok(results)
}

/// 检测 Bing 反爬/验证码页
fn is_bot_blocked(html: &str) -> bool {
    let lower = html.to_lowercase();
    lower.contains("b_captcha")
        || lower.contains("captcha")
        || lower.contains("验证您不是机器人")
        || lower.contains("非正常访问")
        || lower.contains("检测到异常的访问")
}

/// 用 scraper 解析 Bing 结果页的 `li.b_algo`（标题 / 链接 / 摘要）
fn parse_bing_html(html: &str, num: usize) -> Vec<SearchResult> {
    let document = scraper::Html::parse_document(html);
    let algo_sel = scraper::Selector::parse("li.b_algo").unwrap();
    let link_sel = scraper::Selector::parse("h2 a").unwrap();
    let p_sel = scraper::Selector::parse("p").unwrap();

    let mut results = Vec::new();
    for li in document.select(&algo_sel) {
        if results.len() >= num {
            break;
        }
        let link = li.select(&link_sel).next();
        let title = link
            .as_ref()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();
        let url = link
            .and_then(|e| e.value().attr("href"))
            .map(clean_bing_url)
            .unwrap_or_default();
        let snippet = li
            .select(&p_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }
    results
}

/// 还原 Bing 的 `/ck/a`、`/link` 重定向链接为真实 URL。
fn clean_bing_url(raw: &str) -> String {
    if raw.starts_with("/ck/a") || raw.starts_with("/link") {
        if let Ok(parsed) = url::Url::parse(&format!("https://www.bing.com{}", raw)) {
            for (k, v) in parsed.query_pairs() {
                if k == "u" || k == "url" {
                    return v.into_owned();
                }
            }
        }
    }
    raw.to_string()
}

// ──────────────────────────────────────────────
//  Tavily Search API
// ──────────────────────────────────────────────

async fn search_tavily(
    cfg: &SearchConfig,
    query: &str,
    num: usize,
) -> Result<Vec<SearchResult>, String> {
    if cfg.tavily_api_key.is_empty() {
        return Err("未配置 Tavily API Key（请在「设置 → 联网搜索」中填写）".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .post("https://api.tavily.com/search")
        .json(&serde_json::json!({
            "api_key": cfg.tavily_api_key,
            "query": query,
            "max_results": num,
        }))
        .send()
        .await
        .map_err(|e| format!("Tavily 请求失败: {}", e))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取 Tavily 响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!(
            "Tavily 搜索失败 (HTTP {}): {}",
            status.as_u16(),
            text
        ));
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 Tavily 响应失败: {}", e))?;
    let arr = json["results"]
        .as_array()
        .ok_or_else(|| format!("Tavily 响应缺少 results 字段: {}", text))?;

    let mut results = Vec::new();
    for item in arr {
        let title = item["title"].as_str().unwrap_or("").to_string();
        let url = item["url"].as_str().unwrap_or("").to_string();
        let snippet = item["content"].as_str().unwrap_or("").to_string();
        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }
    Ok(results)
}

// ──────────────────────────────────────────────
//  Bing Web Search API
// ──────────────────────────────────────────────

async fn search_bing_api(
    cfg: &SearchConfig,
    query: &str,
    num: usize,
) -> Result<Vec<SearchResult>, String> {
    if cfg.bing_api_key.is_empty() {
        return Err("未配置 Bing Web Search API Key（请在「设置 → 联网搜索」中填写）".to_string());
    }

    let url = format!(
        "https://api.bing.microsoft.com/v7.0/search?q={}&count={}&mkt=zh-CN",
        urlencode(query),
        num
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(&url)
        .header("Ocp-Apim-Subscription-Key", &cfg.bing_api_key)
        .send()
        .await
        .map_err(|e| format!("Bing API 请求失败: {}", e))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取 Bing API 响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!(
            "Bing API 搜索失败 (HTTP {}): {}",
            status.as_u16(),
            text
        ));
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 Bing API 响应失败: {}", e))?;
    let arr = json["webPages"]["value"]
        .as_array()
        .ok_or_else(|| format!("Bing API 响应缺少 webPages.value 字段: {}", text))?;

    let mut results = Vec::new();
    for item in arr {
        let title = item["name"].as_str().unwrap_or("").to_string();
        let url = item["url"].as_str().unwrap_or("").to_string();
        let snippet = item["snippet"].as_str().unwrap_or("").to_string();
        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }
    Ok(results)
}

// ──────────────────────────────────────────────
//  网页抓取（WebFetch 复用）
// ──────────────────────────────────────────────

/// 抓取指定 URL 的网页并提取可读文本（reqwest 快速通道，失败/空内容时走无头浏览器）。
pub async fn fetch_web_text(url: &str) -> Result<String, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("url 必须以 http:// 或 https:// 开头".to_string());
    }

    // 快速通道
    if let Ok(text) = fetch_web_text_http(url).await {
        if !text.trim().is_empty() {
            return Ok(text);
        }
    }

    // 无头浏览器兜底
    let url_owned = url.to_string();
    tokio::task::spawn_blocking(move || fetch_web_text_browser(&url_owned))
        .await
        .map_err(|e| format!("网页抓取任务被中断: {}", e))?
}

/// reqwest 快速通道：GET 网页并提取可读文本。
async fn fetch_web_text_http(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }

    // 依据 Content-Type 拒绝二进制资源（图片/音视频/压缩包等），避免把二进制当文本返回乱码
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if is_binary_content_type(&content_type) {
        return Err(format!(
            "该 URL 是二进制资源（Content-Type: {}），fetch_web 仅支持文本/HTML 页面",
            content_type
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取响应失败: {}", e))?;

    // 兜底：Content-Type 缺失/伪装时，按字节内容判定二进制
    if looks_binary(&bytes) {
        return Err("该 URL 返回的是二进制内容，fetch_web 仅支持文本/HTML 页面".to_string());
    }

    let html = String::from_utf8_lossy(&bytes).to_string();
    let text = browser::html_to_text(&html);
    if text.trim().is_empty() {
        return Err("页面内容为空或无法解析".to_string());
    }
    Ok(browser::truncate_utf8(&text, MAX_FETCH_OUTPUT).to_string())
}

/// 判断 Content-Type 是否为二进制类型（fetch_web 无法提取文本）。
fn is_binary_content_type(ct: &str) -> bool {
    let ct = ct.to_lowercase();
    ct.starts_with("image/")
        || ct.starts_with("video/")
        || ct.starts_with("audio/")
        || ct.starts_with("application/octet-stream")
        || ct.starts_with("application/pdf")
        || ct.starts_with("application/zip")
        || ct.starts_with("application/x-")
        || ct.starts_with("application/gzip")
        || ct.starts_with("application/compressed")
        || ct.contains("octet-stream")
}

/// 按字节内容判定是否二进制（含 NUL 字节，或存在非法 UTF-8 序列）。
fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return true;
    }
    let sample = &bytes[..bytes.len().min(8192)];
    let mut invalid = 0usize;
    let mut rest = sample;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(_) => break,
            Err(e) => {
                let valid = e.valid_up_to();
                let err_len = e.error_len().unwrap_or(1);
                invalid += err_len;
                rest = &rest[valid + err_len..];
            }
        }
    }
    invalid > 0
}

/// 无头浏览器兜底：渲染页面并提取可读文本。
fn fetch_web_text_browser(url: &str) -> Result<String, String> {
    let browser = browser::find_browser()
        .ok_or_else(|| "网页抓取失败：未检测到 Edge/Chrome 浏览器用于兜底".to_string())?;

    let args: Vec<String> = vec![
        "--headless".to_string(),
        "--disable-gpu".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--virtual-time-budget=8000".to_string(),
        "--dump-dom".to_string(),
        url.to_string(),
    ];

    let (stdout, _stderr, _code) = browser::run_browser(&browser, &args)?;
    let html = crate::decode_windows_output(&stdout);
    if html.trim().is_empty() {
        return Err("浏览器未返回页面内容".to_string());
    }

    let text = browser::html_to_text(&html);
    if text.trim().is_empty() {
        return Err("页面内容为空或无法解析（可能需要登录或存在反爬）".to_string());
    }
    Ok(browser::truncate_utf8(&text, MAX_FETCH_OUTPUT).to_string())
}
