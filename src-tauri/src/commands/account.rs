//! 会员账号（产品端登录）：PKCE 授权 + 令牌加密存储 + 会员权益。
//!
//! 流程（docs/membership-plan.md §6）：
//!   ① `account_login_begin` —— 起本地回环端口 + 生成 PKCE，返回平台授权页地址；
//!   ② 前端用**系统浏览器**打开该地址（`tauri-plugin-shell` 的 open，已授权）；
//!   ③ 用户在平台页登录并同意 → 平台 302 回环地址并带上一次性 code；
//!   ④ `account_login_complete` —— 换令牌、加密落库、拉权益返回给前端。
//!
//! 令牌（access / refresh）与云文档账号同一套做法：整块 JSON 加密后存一个
//! `app_settings` 键，**前端永远拿不到明文**；access 过期时用 refresh 自动续。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::commands::app_settings::{delete_setting, get_setting, set_setting};
use crate::utils::errors::AppError;
use crate::utils::platform;

/// 令牌整块密文存放的键
const TOKENS_SETTING: &str = "account_tokens";
/// 权益缓存（明文 JSON，无秘密）：平台不可达时按它放行
const CACHE_SETTING: &str = "account_entitlements_cache";
/// 离线宽限：平台不可达时，缓存在此天数内仍按「上次已知权益」放行。
/// 本期的差异化授权是**体验分层不是安全边界**，所以离线宽限是可接受的。
const OFFLINE_GRACE_DAYS: i64 = 7;
/// 等待回环回调的超时（用户要在浏览器里登录 / 同意）
const CALLBACK_TIMEOUT_SECS: u64 = 300;
/// access 提前多少秒视为过期（避免边界上正好用到刚过期的令牌）
const ACCESS_SKEW_SECS: i64 = 30;

// ── 落盘的令牌 ──────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct StoredTokens {
    access_token: String,
    refresh_token: String,
    /// RFC3339（UTC）
    access_expires_at: String,
    refresh_expires_at: String,
}

fn load_tokens(conn: &rusqlite::Connection) -> Result<Option<StoredTokens>, AppError> {
    let Some(encrypted) = get_setting(conn, TOKENS_SETTING)? else {
        return Ok(None);
    };
    if encrypted.is_empty() {
        return Ok(None);
    }
    match crate::utils::crypto::decrypt(&encrypted) {
        Ok(json) => Ok(serde_json::from_str(&json).ok()),
        // 换过机器 / 密钥文件被删 → 当作未登录（用户下一步本来就是重新登录）
        Err(e) => {
            log::warn!("[Account] 令牌无法解密（换机或密钥丢失）：{}", e);
            Ok(None)
        }
    }
}

fn save_tokens(conn: &rusqlite::Connection, tokens: &StoredTokens) -> Result<(), AppError> {
    let json = serde_json::to_string(tokens)?;
    let encrypted = crate::utils::crypto::encrypt(&json).map_err(AppError::Config)?;
    set_setting(conn, TOKENS_SETTING, &encrypted)
}

fn clear_tokens(conn: &rusqlite::Connection) -> Result<(), AppError> {
    delete_setting(conn, TOKENS_SETTING)
}

// ── 平台响应结构（字段与 app/src/routes 的 JSON 对齐）──────────────

#[derive(Deserialize)]
struct MeResp {
    user: MeUser,
}

#[derive(Deserialize)]
struct MeUser {
    email: String,
    nickname: String,
    /// 内置头像键（null = 未选择）；老平台不返回该字段时按 null 处理
    #[serde(rename = "avatarId", default)]
    avatar_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountOrganization {
    id: i64,
    name: String,
    role: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntitlementsResp {
    plan: PlanResp,
    capabilities: Vec<String>,
    expired: bool,
    #[allow(dead_code)]
    expired_plan_key: Option<String>,
    /// 平台下发的配额（键为平台登记项；-1 = 不限；缺省键 = 未配置）
    #[serde(default)]
    quotas: HashMap<String, i64>,
    /// 权益来源：personal（个人订阅）/ organization（组织）/ free（免费档）
    #[serde(default)]
    source: Option<String>,
    /// 来源组织（source = organization 时非空）
    #[serde(default)]
    organization: Option<AccountOrganization>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanResp {
    key: String,
    name: String,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResp {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    refresh_expires_in: i64,
}

#[derive(Serialize)]
struct TokenReq<'a> {
    grant_type: &'a str,
    code: &'a str,
    code_verifier: &'a str,
    client_id: &'a str,
    redirect_uri: &'a str,
}

#[derive(Serialize)]
struct RefreshReq<'a> {
    grant_type: &'a str,
    refresh_token: &'a str,
    client_id: &'a str,
}

#[derive(Serialize)]
struct RevokeReq<'a> {
    refresh_token: &'a str,
}

// ── 回给前端的视图（**不含令牌**）────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub email: String,
    pub nickname: String,
    /// 内置头像键（null = 未选择，前端回退首字母头像）
    #[serde(default)]
    pub avatar_id: Option<String>,
    /// 等级键（free / pro / team）；已过期时平台按 free 返回
    pub plan_key: String,
    pub plan_name: String,
    /// 到期时间（ISO；free 为空）
    pub expires_at: Option<String>,
    /// 已解锁的受限能力键 —— 前端据此显示 / 隐藏入口
    pub capabilities: Vec<String>,
    /// 平台下发的配额（键为平台登记项；-1 = 不限）
    #[serde(default)]
    pub quotas: HashMap<String, i64>,
    /// 权益来源：personal（个人订阅）/ organization（组织）/ free（免费档）
    #[serde(default)]
    pub source: String,
    /// 来源组织（source=organization 时非空）
    #[serde(default)]
    pub organization: Option<AccountOrganization>,
    /// 是否已过期（平台按 free 返回，但仍告知原档位）
    pub expired: bool,
    /// 是否来自离线缓存（平台不可达、仍在宽限期内）——前端给"离线"轻提示
    #[serde(default)]
    pub stale: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginBegin {
    /// 供前端用系统浏览器打开的授权页地址
    pub authorize_url: String,
}

// ── 待完成的授权会话（begin 与 complete 之间）────────────────────

struct PendingLogin {
    code_verifier: String,
    redirect_uri: String,
    rx: tokio::sync::oneshot::Receiver<Result<String, String>>,
}

/// 挂到 Tauri 全局状态上的登录会话槽（同一时刻只允许一次登录）
#[derive(Default)]
pub struct AccountLoginState {
    pending: Mutex<Option<PendingLogin>>,
}

// ── PKCE 与回环 ────────────────────────────────────────────────

/// 随机串（PKCE verifier / state）：两个 UUIDv4 拼成 64 位十六进制
fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// `code_challenge = BASE64URL( SHA256(code_verifier) )`（S256，不带 padding）
fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn rfc3339_after(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
}

/// 解析 `a=1&b=2` 形式的查询串（已做百分号解码）
fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let decode = |s: &str| {
            percent_encoding::percent_decode_str(&s.replace('+', " "))
                .decode_utf8_lossy()
                .to_string()
        };
        map.insert(decode(k), decode(v));
    }
    map
}

/// 等待回调：只处理 `GET /callback`，其余连接（浏览器预连接、favicon、代理 CONNECT）回 204 后继续等。
///
/// 浏览器一次导航可能并行开多条连接，只 accept 第一条会把真正的回调挤掉：
/// 真正的回调请求落到已关闭的监听上，页面显示「无法访问此网站」，
/// 用户就永远看不到「授权成功」。因此这里必须循环到真正的 `/callback` 为止。
async fn accept_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const WAIT_TOTAL: std::time::Duration = std::time::Duration::from_secs(180);
    const WAIT_READ: std::time::Duration = std::time::Duration::from_secs(10);
    let deadline = tokio::time::Instant::now() + WAIT_TOTAL;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("等待授权回调超时，请回到 PilotDesk 重新登录。".to_string());
        }

        let (mut socket, _) = tokio::time::timeout(remaining, listener.accept())
            .await
            .map_err(|_| "等待授权回调超时，请回到 PilotDesk 重新登录。".to_string())?
            .map_err(|e| format!("等待回调失败：{}", e))?;

        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(WAIT_READ, socket.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            // 只连不发（预连接）或读超时：礼貌关闭，继续等真正的回调
            _ => {
                let _ = socket
                    .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                    .await;
                continue;
            }
        };
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("");

        // favicon、预连接、代理 CONNECT 等：不是回调，忽略并继续等
        if !target.starts_with("/callback") {
            let _ = socket
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }

        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        let params = parse_query(query);

        let (message, ok) = if let Some(err) = params.get("error") {
            (format!("授权未完成：{}", err), false)
        } else if params.get("state").map(String::as_str) != Some(expected_state) {
            (
                "state 校验失败，请回到 PilotDesk 重新登录。".to_string(),
                false,
            )
        } else {
            (
                "授权成功，已可关闭此页面并回到 PilotDesk。".to_string(),
                true,
            )
        };

        // 成功时给出「即将自动关闭」的提示并真的关掉：用户不用自己找关闭按钮，
        // 也不会因为停留在一个 127.0.0.1 页面上误以为网络出了问题。
        let hint = if ok {
            "<br><span style=\"font-size:13px;color:#9aa5c4\">本页将在 3 秒后自动关闭。</span>"
        } else {
            ""
        };
        let script = if ok {
            "<script>setTimeout(function(){window.close()},3000)</script>"
        } else {
            ""
        };
        let html = format!(
            "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>PilotDesk 授权</title></head>\
<body style=\"margin:0;height:100vh;display:flex;align-items:center;justify-content:center;\
font-family:-apple-system,BlinkMacSystemFont,'Segoe UI','PingFang SC','Microsoft YaHei',sans-serif;\
background:#0b1020;color:#e8ecf8\"><p style=\"font-size:15px;text-align:center\">{}{}</p>{}</body></html>",
            message, hint, script
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            html.as_bytes().len(),
            html
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.flush().await;

        if !ok {
            return Err(message);
        }
        return params
            .get("code")
            .cloned()
            .filter(|c| !c.is_empty())
            .ok_or_else(|| "回调里没有授权码".to_string());
    }
}

// ── 令牌续期 ────────────────────────────────────────────────────

/// 保证拿到一个可用的 access token：快过期就先刷新
async fn ensure_access(state: &crate::DbState, tokens: &StoredTokens) -> Result<String, AppError> {
    let fresh = chrono::DateTime::parse_from_rfc3339(&tokens.access_expires_at)
        .map(|exp| {
            exp.with_timezone(&chrono::Utc)
                > chrono::Utc::now() + chrono::Duration::seconds(ACCESS_SKEW_SECS)
        })
        .unwrap_or(false);
    if fresh {
        return Ok(tokens.access_token.clone());
    }

    let resp: TokenResp = match platform::post_json_with_status(
        "/api/v1/oauth/refresh",
        &RefreshReq {
            grant_type: "refresh_token",
            refresh_token: &tokens.refresh_token,
            client_id: platform::CLIENT_ID,
        },
        None,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            // 网络层失败 / 5xx / 429 = 平台临时不可用：**不能清令牌**。
            // 否则平台维护或抖动一次，就把用户变成「未登录」，
            // 连带 7 天离线宽限失效（下一次 account_status 读不到令牌会直接返回 None）。
            if is_transient_status(e.status) {
                return Err(AppError::Network(e.message));
            }
            // 真正的鉴权失败（refresh 被吊销 / 过期 / 客户端被拒）→ 清掉本地令牌，回到未登录
            if let Ok(conn) = state.get_conn() {
                let _ = clear_tokens(&conn);
            }
            return Err(AppError::External(e.message));
        }
    };

    let next = StoredTokens {
        access_token: resp.access_token,
        refresh_token: resp.refresh_token,
        access_expires_at: rfc3339_after(resp.expires_in),
        refresh_expires_at: rfc3339_after(resp.refresh_expires_in),
    };
    if let Ok(conn) = state.get_conn() {
        let _ = save_tokens(&conn, &next);
    }
    Ok(next.access_token)
}

/// 取当前可用的平台 access token（未登录返回 `None`；临近过期会自动刷新）。
///
/// 供产品内其它需要 Bearer 调用平台接口的功能复用（如用量上报）。
/// 令牌只在 Rust 侧使用，**绝不**回传前端明文。
pub(crate) async fn current_access_token(
    state: &crate::DbState,
) -> Result<Option<String>, AppError> {
    let tokens = {
        let conn = state.get_conn()?;
        load_tokens(&conn)?
    };
    match tokens {
        Some(t) => ensure_access(state, &t).await.map(Some),
        None => Ok(None),
    }
}

/// 平台「临时不可用」：网络层失败 / 429 / 5xx。
///
/// 这类失败**不代表登录失效**：令牌要保留，按离线宽限处理；
/// 只有 4xx（refresh 被吊销、过期等）才清令牌回到未登录。
fn is_transient_status(status: u16) -> bool {
    status == 0 || status == 429 || status >= 500
}

/// 调平台接口并保留状态码语义：临时不可用映射成 `Network`（走离线缓存），
/// 其余 4xx 才映射成 `External`（鉴权失败）。
async fn get_json_typed<T: serde::de::DeserializeOwned>(
    path: &str,
    token: &str,
) -> Result<T, AppError> {
    platform::get_json_with_status(path, Some(token))
        .await
        .map_err(|e| {
            if is_transient_status(e.status) {
                AppError::Network(e.message)
            } else {
                AppError::External(e.message)
            }
        })
}

/// 最近一次「平台不可达」告警时间：多窗口各自 focus 刷新时避免刷屏。
static LAST_OFFLINE_WARN_AT: AtomicI64 = AtomicI64::new(0);

/// 平台不可达日志节流：60 秒内只 WARN 一次，其余降为 DEBUG。
fn log_offline(msg: &str) {
    let now = chrono::Utc::now().timestamp();
    let last = LAST_OFFLINE_WARN_AT.load(Ordering::Relaxed);
    if now - last >= 60 {
        LAST_OFFLINE_WARN_AT.store(now, Ordering::Relaxed);
        log::warn!("[Account] 平台不可达，按离线缓存处理：{}", msg);
    } else {
        log::debug!("[Account] 平台仍不可达：{}", msg);
    }
}

/// 拉账号资料 + 会员权益，组装成前端视图
async fn fetch_view(
    state: &crate::DbState,
    tokens: &StoredTokens,
) -> Result<AccountView, AppError> {
    let access = ensure_access(state, tokens).await?;
    let me: MeResp = get_json_typed("/api/v1/me", &access).await?;
    let ent: EntitlementsResp = get_json_typed("/api/v1/me/entitlements", &access).await?;
    Ok(AccountView {
        email: me.user.email,
        nickname: me.user.nickname,
        avatar_id: me.user.avatar_id,
        plan_key: ent.plan.key,
        plan_name: ent.plan.name,
        expires_at: ent.plan.expires_at,
        capabilities: ent.capabilities,
        quotas: ent.quotas,
        source: ent.source.unwrap_or_else(|| "free".to_string()),
        organization: ent.organization,
        expired: ent.expired,
        stale: false,
    })
}

// ── 离线缓存（宽限期内按上次已知权益放行）────────────────────────

#[derive(Serialize, Deserialize)]
struct CachedEntitlements {
    view: AccountView,
    /// 缓存时间（RFC3339）
    cached_at: String,
}

fn save_cache(conn: &rusqlite::Connection, view: &AccountView) -> Result<(), AppError> {
    let cached = CachedEntitlements {
        view: AccountView {
            stale: false,
            ..view.clone()
        },
        cached_at: chrono::Utc::now().to_rfc3339(),
    };
    set_setting(conn, CACHE_SETTING, &serde_json::to_string(&cached)?)
}

/// 读缓存；仅在宽限期内有效（超期当作没有，避免拿很久以前的权益忽悠用户）
fn load_cache_within_grace(conn: &rusqlite::Connection) -> Option<AccountView> {
    let raw = get_setting(conn, CACHE_SETTING).ok().flatten()?;
    if raw.is_empty() {
        return None;
    }
    let cached: CachedEntitlements = serde_json::from_str(&raw).ok()?;
    let at = chrono::DateTime::parse_from_rfc3339(&cached.cached_at).ok()?;
    let age = chrono::Utc::now() - at.with_timezone(&chrono::Utc);
    if age > chrono::Duration::days(OFFLINE_GRACE_DAYS) {
        return None;
    }
    Some(AccountView {
        stale: true,
        ..cached.view
    })
}

/// 某受限能力是否已解锁（读权益离线缓存）：
/// - 无缓存 / 缓存坏 → `None`（未知，调用方可交由服务端裁决，如云同步的 403）；
/// - 有缓存 → `Some(true/false)`（按 `capabilities` 判定，与前端 `hasCapability` 同一份数据）。
///
/// 说明：能力只是「体验分层」，不构成安全边界（docs/membership-plan.md §1），真正的准入由服务端负责。
pub fn has_capability(conn: &rusqlite::Connection, key: &str) -> Option<bool> {
    let raw = match get_setting(conn, CACHE_SETTING) {
        Ok(Some(v)) if !v.is_empty() => v,
        _ => return None,
    };
    let cached: CachedEntitlements = match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(_) => return None,
    };
    Some(cached.view.capabilities.iter().any(|k| k == key))
}

// ── Tauri 命令 ─────────────────────────────────────────────────

/// ① 开始登录：起回环端口 + 生成 PKCE，返回平台授权页地址（由前端用系统浏览器打开）
#[tauri::command]
pub async fn account_login_begin(
    login: tauri::State<'_, AccountLoginState>,
) -> Result<LoginBegin, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("无法启动本地回调端口：{}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读取回调端口失败：{}", e))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{}/callback", port);
    let state = random_token();
    let verifier = random_token();
    let challenge = code_challenge(&verifier);
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();

    let expected = state.clone();
    tauri::async_runtime::spawn(async move {
        let result = accept_callback(listener, &expected).await;
        let _ = tx.send(result);
    });

    let authorize_url = format!(
        "{}/api/v1/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
        platform::api_base(),
        platform::CLIENT_ID,
        percent_encoding::utf8_percent_encode(&redirect_uri, percent_encoding::NON_ALPHANUMERIC),
        challenge,
        state,
    );

    *login
        .pending
        .lock()
        .map_err(|_| "内部状态锁定失败".to_string())? = Some(PendingLogin {
        code_verifier: verifier,
        redirect_uri,
        rx,
    });

    Ok(LoginBegin { authorize_url })
}

/// ④ 完成登录：等回环回调 → 换令牌 → 加密落库 → 返回账号与权益
#[tauri::command]
pub async fn account_login_complete(
    state: tauri::State<'_, crate::DbState>,
    login: tauri::State<'_, AccountLoginState>,
) -> Result<AccountView, String> {
    let pending = login
        .pending
        .lock()
        .map_err(|_| "内部状态锁定失败".to_string())?
        .take()
        .ok_or_else(|| "没有进行中的登录，请先点「登录」".to_string())?;

    let code = tokio::time::timeout(
        std::time::Duration::from_secs(CALLBACK_TIMEOUT_SECS),
        pending.rx,
    )
    .await
    .map_err(|_| "等待授权超时，请重试".to_string())?
    .map_err(|_| "登录已取消".to_string())??;

    let tokens: TokenResp = platform::post_json(
        "/api/v1/oauth/token",
        &TokenReq {
            grant_type: "authorization_code",
            code: &code,
            code_verifier: &pending.code_verifier,
            client_id: platform::CLIENT_ID,
            redirect_uri: &pending.redirect_uri,
        },
        None,
    )
    .await
    .map_err(String::from)?;

    let stored = StoredTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        access_expires_at: rfc3339_after(tokens.expires_in),
        refresh_expires_at: rfc3339_after(tokens.refresh_expires_in),
    };
    {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        save_tokens(&conn, &stored).map_err(String::from)?;
    }

    fetch_view(state.inner(), &stored)
        .await
        .map_err(String::from)
}

/// 当前登录状态与权益：未登录返回 `None`；**平台不可达时按离线缓存放行**（宽限期内）
#[tauri::command]
pub async fn account_status(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Option<AccountView>, String> {
    let tokens = {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        load_tokens(&conn).map_err(String::from)?
    };
    let Some(tokens) = tokens else {
        return Ok(None);
    };
    match fetch_view(state.inner(), &tokens).await {
        Ok(view) => {
            // 每次成功都刷新缓存，供离线宽限使用
            if let Ok(conn) = state.get_conn() {
                let _ = save_cache(&conn, &view);
            }
            Ok(Some(view))
        }
        // 平台临时不可用（离线 / 5xx / 429）：宽限期内按「上次已知权益」放行，
        // 避免断网或平台维护时直接掉登录态；日志做节流，避免多窗口 focus 反复刷屏
        Err(AppError::Network(msg)) => {
            log_offline(&msg);
            Ok(state
                .get_conn()
                .ok()
                .and_then(|c| load_cache_within_grace(&c)))
        }
        // 鉴权类失败（refresh 已失效并清掉本地令牌）：按未登录处理，不用缓存糊弄
        Err(e) => {
            log::warn!("[Account] 拉取会员权益失败：{}", e);
            Ok(None)
        }
    }
}

/// 本地是否存在登录会话（只读本地库，不发任何网络请求）。
///
/// 供前端在窗口 focus 等高频时机先判断登录态：未登录就不调平台接口，
/// 避免多窗口无条件刷新会员权益，放大请求量与错误日志。
#[tauri::command]
pub fn account_has_session(state: tauri::State<'_, crate::DbState>) -> Result<bool, String> {
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败：{}", e))?;
    Ok(load_tokens(&conn).map_err(String::from)?.is_some())
}

/// 平台基址，供前端拼「升级页 / 会员中心」等地址（基址只在 Rust 一处，避免前后端各写一份漂移）。
///
/// 这些页面**用系统浏览器打开**（与登录同一机制）：登录是在浏览器里完成的，会话 Cookie
/// 自然就在浏览器里，点开即用；而应用内 WebView 是另一套 Cookie jar，打开会要求重新登录。
#[tauri::command]
pub fn account_platform_base() -> String {
    platform::api_base().to_string()
}

/// 官网基址，供「关于」页外链等前端展示使用。
///
/// 唯一来源是 `utils::site::OFFICIAL_SITE`（市场数据的官网源也从它派生），
/// 换域名只改那一行，前端不再各自硬编码。
#[tauri::command]
pub fn site_official_url() -> String {
    crate::utils::site::official_site().to_string()
}

/// 退出登录：先尽力吊销服务端令牌，再清本地（含飞地本机身份）。
#[tauri::command]
pub async fn account_logout(
    state: tauri::State<'_, crate::DbState>,
    enclave: tauri::State<'_, std::sync::Arc<crate::enclave::EnclaveState>>,
) -> Result<(), String> {
    let tokens = {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        load_tokens(&conn).map_err(String::from)?
    };

    if let Some(tokens) = tokens {
        // 吊销失败（网络不通 / 已失效）不阻断退出：本地清了就算退出
        let _ = platform::post_json::<_, serde_json::Value>(
            "/api/v1/oauth/revoke",
            &RevokeReq {
                refresh_token: &tokens.refresh_token,
            },
            None,
        )
        .await;
    }

    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败：{}", e))?;
    clear_tokens(&conn).map_err(String::from)?;
    drop(conn);

    // 飞地：清掉本机登录身份。组织/成员快照按 viewer 绑定失效，但身份必须显式清空，
    // 否则退出后本地命令仍会以上一个账号的 actor 执行。
    enclave.store.with_conn(|c| {
        c.execute("DELETE FROM local_identity", [])
            .map(|_| ())
            .map_err(|e| format!("清理飞地登录身份失败：{}", e))
    })?;
    Ok(())
}
