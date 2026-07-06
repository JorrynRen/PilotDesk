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
pub const PLUGINS_INDEX_PATH: &str = "/server/market/plugins/index.json";
#[allow(dead_code)]
pub const PLUGINS_DIR_PATH: &str = "/server/market/plugins";

/// 根据路径构建所有服务器源的完整 URL 列表
pub fn build_urls(path: &str) -> Vec<String> {
    SERVER_SOURCES
        .iter()
        .map(|base| format!("{}{}", base.trim_end_matches('/'), path))
        .collect()
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
                    return resp.text()
                        .await
                        .map_err(|e| format!("读取响应失败: {}", e));
                } else if status >= 500 && attempt < MAX_RETRIES {
                    last_err = format!("HTTP {}: {}", status, url);
                    log::warn!("[Market] 第 {} 次请求失败 ({}), {}ms 后重试...", attempt, last_err, RETRY_DELAY_MS);
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
                        if e.is_timeout() { "请求超时" } else { "连接失败" },
                        url
                    );
                    log::warn!("[Market] 第 {} 次请求失败 ({}), {}ms 后重试...", attempt, last_err, RETRY_DELAY_MS);
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
                return Err(format!(
                    "{}: {}",
                    if e.is_timeout() { "请求超时" } else { "连接失败" },
                    url
                ));
            }
        }
    }
    Err(format!("重试 {} 次后仍失败: {}", MAX_RETRIES, last_err))
}

/// 按源优先级依次尝试获取远程 JSON 资源
pub(crate) async fn fetch_market_json(path: &str) -> Result<Value, String> {
    let urls = build_urls(path);
    let mut last_err = String::new();
    for url in &urls {
        match fetch_url(url).await {
            Ok(body) => match serde_json::from_str::<Value>(&body) {
                Ok(json) => return Ok(json),
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

/// [Tauri Command] 获取 Agent 市场配置
///
/// 返回 agents-config.json 的 JSON 内容，前端直接使用。
#[tauri::command]
pub async fn fetch_agents_config() -> Result<Value, String> {
    fetch_market_json(AGENTS_CONFIG_PATH).await
}
