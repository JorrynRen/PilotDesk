//! 平台（会员中心）API 客户端：会员登录与权益查询。
//!
//! 与市场数据不同 —— 市场数据是**公开 CDN**（见 `utils/market.rs`），
//! 而账号与权益必须走**平台服务端**（`docs/membership-plan.md`）。
//! 令牌只在 Rust 侧保存与使用，前端拿不到明文（沿用云文档账号那套加密存储）。

use crate::utils::errors::AppError;

/// 平台 API 基址：默认取官网基址（`utils::site::OFFICIAL_SITE`）——
/// 登录、会员、升级等页面与官网共用同一个域名，换域名只改 `site.rs` 一行。
///
/// 不开放给用户配置：可配置（环境变量 / 设置项）会让用户分流到不同后端，
/// 客服与数据统计都无从对齐。
///
/// 局域网联调例外：允许在**构建时**用 `PILOTDESK_PLATFORM_API_BASE` 覆盖
/// （例如 `http://192.168.31.187:8000`），用于多设备、多账号测试。
/// 这是构建决策而不是用户配置：正式包不设置该变量，固定走官网。
pub const PLATFORM_API_BASE: &str = match option_env!("PILOTDESK_PLATFORM_API_BASE") {
    Some(v) => v,
    None => crate::utils::site::OFFICIAL_SITE,
};

/// OAuth 客户端标识：必须在平台 `app/src/services/entitlements/clients.ts` 里登记过，
/// 且回调地址要落在其白名单前缀内（回环地址）。
pub const CLIENT_ID: &str = "pilotdesk";

const CONNECT_TIMEOUT_SECS: u64 = 8;
const READ_TIMEOUT_SECS: u64 = 15;

/// 当前生效的平台基址（去掉结尾斜杠，便于拼接）
pub fn api_base() -> &'static str {
    PLATFORM_API_BASE
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
        .no_proxy()
        .build()
        .expect("Failed to build HTTP client")
}

/// 把平台的错误响应（`{ "error": "..." }`）转成人可读文案。
/// 平台所有接口的失败体都是 `{ error }`（见 app/src/routes/*），这里统一取出来。
async fn error_from(resp: reqwest::Response) -> AppError {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string());
    AppError::External(if message.is_empty() {
        format!("平台返回 HTTP {}", status)
    } else {
        format!("平台返回 HTTP {}：{}", status, message)
    })
}

/// 平台请求失败详情：`status` 为 HTTP 状态码（`0` 表示网络层或响应解析失败）。
///
/// 供需要**按状态码做友好映射**的调用方使用（如组织共享空间的 403 / 409）；
/// 只关心错误文案的旧调用方继续用 [`get_json`] / [`post_json`]。
#[derive(Debug)]
pub struct PlatformError {
    pub status: u16,
    pub message: String,
    /// 平台失败响应体（成功解析为 JSON 时为 `Some`）：供需要读取结构化字段的调用方使用
    /// （如 409 冲突响应里的 `existing` 摘要）。
    pub body: Option<serde_json::Value>,
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// 解析平台失败响应为带状态码的错误（结构同 [`error_from`]，但保留 status 与响应体）
async fn error_with_status(resp: reqwest::Response) -> PlatformError {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let parsed = serde_json::from_str::<serde_json::Value>(&body).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string());
    PlatformError {
        status,
        message: if message.is_empty() {
            format!("平台返回 HTTP {}", status)
        } else {
            format!("平台返回 HTTP {}：{}", status, message)
        },
        body: parsed,
    }
}

/// GET 一个 JSON 接口（可选 Bearer）；失败时保留 HTTP 状态码。
pub async fn get_json_with_status<T: serde::de::DeserializeOwned>(
    path: &str,
    token: Option<&str>,
) -> Result<T, PlatformError> {
    let mut req = http_client().get(format!("{}{}", api_base(), path));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(|e| PlatformError {
        status: 0,
        message: format!("请求平台失败：{}", e),
        body: None,
    })?;
    if !resp.status().is_success() {
        return Err(error_with_status(resp).await);
    }
    resp.json::<T>().await.map_err(|e| PlatformError {
        status: 0,
        message: format!("解析平台响应失败：{}", e),
        body: None,
    })
}

/// 瞬时故障（网络层失败、429、5xx）：多来自隧道 / 网关抖动，不是接口本身的问题。
fn is_transient_read_error(err: &PlatformError) -> bool {
    err.status == 0 || err.status == 429 || err.status >= 500
}

/// GET 一个 JSON 接口，**瞬时故障自动重试一次**（只用于幂等读取）。
///
/// 客户端经公网隧道访问平台，网关偶发 503「Service Unavailable」；
/// 一次抖动不该让用户看到「读取设备失败」，重试后仍失败才如实报错。
pub async fn get_json_retry<T: serde::de::DeserializeOwned>(
    path: &str,
    token: Option<&str>,
) -> Result<T, AppError> {
    match get_json_with_status::<T>(path, token).await {
        Ok(value) => Ok(value),
        Err(err) if is_transient_read_error(&err) => {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            get_json(path, token).await
        }
        Err(err) => Err(AppError::External(err.message)),
    }
}

/// POST 一个 JSON 接口（可选 Bearer）；失败时保留 HTTP 状态码。
pub async fn post_json_with_status<B: serde::Serialize, T: serde::de::DeserializeOwned>(
    path: &str,
    body: &B,
    token: Option<&str>,
) -> Result<T, PlatformError> {
    let mut req = http_client()
        .post(format!("{}{}", api_base(), path))
        .json(body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    send_json_with_status(req).await
}

/// PUT 一个 JSON 接口（可选 Bearer）；失败时保留 HTTP 状态码。
pub async fn put_json_with_status<B: serde::Serialize, T: serde::de::DeserializeOwned>(
    path: &str,
    body: &B,
    token: Option<&str>,
) -> Result<T, PlatformError> {
    let mut req = http_client()
        .put(format!("{}{}", api_base(), path))
        .json(body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    send_json_with_status(req).await
}

async fn send_json_with_status<T: serde::de::DeserializeOwned>(
    req: reqwest::RequestBuilder,
) -> Result<T, PlatformError> {
    let resp = req.send().await.map_err(|e| PlatformError {
        status: 0,
        message: format!("请求平台失败：{}", e),
        body: None,
    })?;
    if !resp.status().is_success() {
        return Err(error_with_status(resp).await);
    }
    resp.json::<T>().await.map_err(|e| PlatformError {
        status: 0,
        message: format!("解析平台响应失败：{}", e),
        body: None,
    })
}

/// GET 一个 JSON 接口（可选 Bearer）
pub async fn get_json<T: serde::de::DeserializeOwned>(
    path: &str,
    token: Option<&str>,
) -> Result<T, AppError> {
    let mut req = http_client().get(format!("{}{}", api_base(), path));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AppError::Network(format!("请求平台失败：{}", e)))?;
    if !resp.status().is_success() {
        return Err(error_from(resp).await);
    }
    resp.json::<T>()
        .await
        .map_err(|e| AppError::Json(format!("解析平台响应失败：{}", e)))
}

/// POST 一个 JSON 接口（可选 Bearer）
pub async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
    path: &str,
    body: &B,
    token: Option<&str>,
) -> Result<T, AppError> {
    let mut req = http_client()
        .post(format!("{}{}", api_base(), path))
        .json(body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AppError::Network(format!("请求平台失败：{}", e)))?;
    if !resp.status().is_success() {
        return Err(error_from(resp).await);
    }
    resp.json::<T>()
        .await
        .map_err(|e| AppError::Json(format!("解析平台响应失败：{}", e)))
}
