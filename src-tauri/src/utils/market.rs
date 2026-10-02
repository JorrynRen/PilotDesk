//! 在线商店远程资源模块
//!
//! 统一管理在线商店的服务器源配置与远程资源获取。
//! 前端通过 Tauri Command 调用本模块获取远程资源，
//! 无需在前端维护 URL 配置（消除 TS/RS 双端重复）。
//!
//! 本地路径管理见 utils/paths.rs。

use serde_json::Value;

/// 在线商店服务器源（按优先级排列）
/// 1. jsdelivr CDN（主源，速度快，全球加速）
/// 2. GitHub Raw（降级源，直连仓库）
pub const SERVER_SOURCES: &[&str] = &[
    "https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main",
    "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main",
];

/// 在线商店路径
pub const AGENTS_CONFIG_PATH: &str = "/server/market/agents-config/agents-config.json";
pub const PLUGINS_INDEX_PATH: &str = "/server/market/plugins/plugins-index.json";
pub const INSPIRATIONS_INDEX_PATH: &str = "/server/market/inspirations/index.json";
#[allow(dead_code)]
pub const PLUGINS_DIR_PATH: &str = "/server/market/plugins";

/// 市场资源在仓库内的根路径。
/// 索引里每个插件的 `path` 字段（如 `plugins/hello-world/1.0.0`）就是相对它的相对路径。
pub const MARKET_ROOT: &str = "/server/market";

/// 根据路径构建所有服务器源的完整 URL 列表
pub fn build_urls(path: &str) -> Vec<String> {
    SERVER_SOURCES
        .iter()
        .map(|base| format!("{}{}", base.trim_end_matches('/'), path))
        .collect()
}

/// 把「某个服务器源 + 相对市场路径」拼成绝对 URL。
///
/// `relative_path` 相对 `server/market/`（例如 `plugins/hello-world/1.0.0`）。
/// 两侧斜杠都做归一化，避免拼出双斜杠。
pub fn build_market_url(source: &str, relative_path: &str) -> String {
    format!(
        "{}{}/{}",
        source.trim_end_matches('/'),
        MARKET_ROOT,
        relative_path.trim_start_matches('/')
    )
}

/// HTTP 超时配置
const CONNECT_TIMEOUT_SECS: u64 = 8;
const READ_TIMEOUT_SECS: u64 = 8;
const MAX_RETRIES: u32 = 2;
const RETRY_DELAY_MS: u64 = 1000;

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
        .no_proxy()
        .build()
        .expect("Failed to build HTTP client")
}

/// 从单个 URL 获取文本内容（含重试）
async fn fetch_url(url: &str) -> Result<String, String> {
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRIES {
        match http_client().get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    return resp
                        .text()
                        .await
                        .map_err(|e| format!("读取响应失败: {}", e));
                } else if status >= 500 && attempt < MAX_RETRIES {
                    last_err = format!("HTTP {}: {}", status, url);
                    log::warn!(
                        "[Market] 第 {} 次请求失败 ({}), {}ms 后重试...",
                        attempt,
                        last_err,
                        RETRY_DELAY_MS
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                } else {
                    return Err(format!("HTTP {}: {}", status, url));
                }
            }
            Err(e) => {
                if attempt < MAX_RETRIES && (e.is_timeout() || e.is_connect()) {
                    last_err = format!(
                        "{}: {}",
                        if e.is_timeout() {
                            "请求超时"
                        } else {
                            "连接失败"
                        },
                        url
                    );
                    log::warn!(
                        "[Market] 第 {} 次请求失败 ({}), {}ms 后重试...",
                        attempt,
                        last_err,
                        RETRY_DELAY_MS
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
                return Err(format!(
                    "{}: {}",
                    if e.is_timeout() {
                        "请求超时"
                    } else {
                        "连接失败"
                    },
                    url
                ));
            }
        }
    }
    Err(format!("重试 {} 次后仍失败: {}", MAX_RETRIES, last_err))
}

/// 按源优先级依次尝试获取远程 JSON 资源。
///
/// 返回 `(JSON, 实际命中的源)`；源即 `SERVER_SOURCES` 中的基地址，
/// 调用方可用它配合`build_market_url`把相对路径解析成同一源下的绝对地址。
pub(crate) async fn fetch_market_json_with_source(path: &str) -> Result<(Value, String), String> {
    let urls = build_urls(path);
    let mut last_err = String::new();
    for (source, url) in SERVER_SOURCES.iter().zip(urls.iter()) {
        match fetch_url(url).await {
            Ok(body) => match serde_json::from_str::<Value>(&body) {
                Ok(json) => return Ok((json, (*source).to_string())),
                Err(e) => {
                    last_err = format!("解析 JSON 失败 ({}): {}", url, e);
                }
            },
            Err(e) => {
                last_err = e;
            }
        }
    }
    Err(format!("所有服务器源均不可用: {}", last_err))
}

/// 按源优先级依次尝试获取远程 JSON 资源
pub(crate) async fn fetch_market_json(path: &str) -> Result<Value, String> {
    fetch_market_json_with_source(path)
        .await
        .map(|(json, _)| json)
}

/// [Tauri Command] 获取 Agent 市场配置
///
/// 返回 agents-config.json 的 JSON 内容，前端直接使用。
#[tauri::command]
pub async fn fetch_agents_config() -> Result<Value, String> {
    fetch_market_json(AGENTS_CONFIG_PATH).await
}

/// [Tauri Command] 获取灵感市场索引
///
/// 返回 index.json 的 JSON 内容（每条只有标题/图标/摘要等元信息，正文不在这里）。
/// 索引由 `server/scripts/generate-inspiration-index.mjs` 生成，一条灵感一个文件 ——
/// 与插件/模板市场同构：索引是清单，真数据在原文件里，正文只有一份。
#[tauri::command]
pub async fn inspiration_market_index() -> Result<Value, String> {
    fetch_market_json(INSPIRATIONS_INDEX_PATH).await
}

/// [Tauri Command] 按 id 获取单条灵感的正文
///
/// 用于详情弹窗（列表只有摘要，点开才拉正文）。
///
/// 路径**只认索引里给出的 `path`**，不接受前端传任意路径：否则这个命令就成了
/// "拿 CDN 当跳板读任意文件"的入口，而它本来只需要读市场内的灵感文件。
#[tauri::command]
pub async fn inspiration_market_fetch(id: String) -> Result<Value, String> {
    let index = fetch_market_json(INSPIRATIONS_INDEX_PATH).await?;
    let rel = index
        .get("inspirations")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        })
        .and_then(|e| e.get("path"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("灵感市场中未找到：{}", id))?;

    // 索引里的 path 相对市场根（server/market），与插件安装用的是同一套拼法
    fetch_market_json(&format!(
        "{}/{}",
        MARKET_ROOT,
        rel.trim_start_matches('/')
    ))
    .await
}
