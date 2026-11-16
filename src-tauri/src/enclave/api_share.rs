//! 飞地 API 共享：共享源、令牌、统一分发、用量与 HTTPS 路由网关。
//!
//! 边界：
//! - Provider 原始 Key 只存在主库 `agent_secrets`，不进入 `enclave.db` 或传讯消息；
//! - 共享源只定义可共享 Provider、模型范围与启停状态；
//! - 路由令牌只能由组织所有者/管理员签发，并通过分发记录主动分享；
//! - 限流、并发、每日请求和 Token 额度只在令牌层；
//! - 路由令牌与 Provider 配置共用 `api_share_distributions`；
//! - 共享 API 对消费方一律按云端 Provider 处理。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{Datelike, Days, NaiveDate};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_rustls::TlsAcceptor;

use super::authz::{self, Role};
use super::{host, tls, EnclaveState};

pub const API_SHARE_DEFAULT_PORT: u16 = 47822;
pub const API_SHARE_USAGE_RETENTION_DAYS: i64 = 365;
const API_SHARE_PORT_KEY: &str = "enclave_api_share_port";
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_USAGE_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const DISTRIBUTION_KIND_ROUTE_TOKEN: &str = "route_token";
const DISTRIBUTION_KIND_PROVIDER_CONFIG: &str = "provider_config";

const ALLOWED_ENDPOINTS: &[&str] = &[
    "/v1/models",
    "/v1/chat/completions",
    "/v1/responses",
    "/v1/embeddings",
    "/v1/messages",
];

static TOKEN_BUCKETS: OnceLock<Mutex<HashMap<String, TokenBucket>>> = OnceLock::new();
static TOKEN_SEMAPHORES: OnceLock<Mutex<HashMap<String, Arc<Semaphore>>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareSourceView {
    pub id: String,
    pub org_id: String,
    pub provider_id: String,
    pub name: String,
    pub models: Vec<String>,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub token_count: i64,
    pub distribution_count: i64,
    pub request_count: i64,
    pub total_tokens: i64,
    pub can_manage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiShareSourceInput {
    pub provider_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateApiShareSourceInput {
    pub name: Option<String>,
    pub models: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareTokenSourceView {
    pub source_id: String,
    pub source_name: String,
    pub source_enabled: bool,
    /// 该来源下当前对外暴露的模型别名，例如 `[生产]deepseek-chat`。
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareTokenView {
    pub id: String,
    pub org_id: String,
    pub owner_user_id: String,
    pub owner_name: String,
    pub created_by: String,
    pub name: String,
    pub token_prefix: String,
    pub api_format: String,
    pub sources: Vec<ApiShareTokenSourceView>,
    pub allowed_models: Vec<String>,
    pub enabled: bool,
    pub revoked_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub rpm_limit: i64,
    pub burst_limit: i64,
    pub max_concurrency: i64,
    pub daily_request_limit: i64,
    pub daily_token_limit: i64,
    pub copied_at: Option<i64>,
    pub first_used_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub request_count: i64,
    pub has_usage: bool,
    pub owner_blocked: bool,
    pub today_requests: i64,
    pub today_tokens: i64,
    pub today_rate_limited: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedApiShareToken {
    pub token: String,
    pub base_url: String,
    pub view: ApiShareTokenView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareTokenGrantInput {
    pub source_id: String,
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiShareTokenInput {
    #[serde(default)]
    pub grants: Vec<ApiShareTokenGrantInput>,
    #[serde(default)]
    pub owner_user_id: String,
    #[serde(default)]
    pub name: String,
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub rpm_limit: i64,
    #[serde(default)]
    pub burst_limit: i64,
    #[serde(default)]
    pub max_concurrency: i64,
    #[serde(default)]
    pub daily_request_limit: i64,
    #[serde(default)]
    pub daily_token_limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateApiShareTokenInput {
    pub name: Option<String>,
    pub grants: Option<Vec<ApiShareTokenGrantInput>>,
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub clear_expiry: bool,
    pub rpm_limit: Option<i64>,
    pub burst_limit: Option<i64>,
    pub max_concurrency: Option<i64>,
    pub daily_request_limit: Option<i64>,
    pub daily_token_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareTokenSecret {
    pub token: String,
    pub base_url: String,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareDistributionView {
    pub id: String,
    pub org_id: String,
    pub kind: String,
    pub source_id: Option<String>,
    pub source_name: String,
    pub token_id: Option<String>,
    pub token_name: String,
    pub batch_id: String,
    pub created_by: String,
    pub creator_name: String,
    pub recipient_user_id: String,
    pub recipient_name: String,
    pub models: Vec<String>,
    pub message_id: Option<String>,
    pub sent_at: Option<i64>,
    pub opened_at: Option<i64>,
    pub applied_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub source_enabled: bool,
    pub token_enabled: bool,
    pub token_expires_at: Option<i64>,
    pub recipient_blocked: bool,
    pub token_owner_blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiShareDistributionInput {
    pub kind: String,
    pub source_id: Option<String>,
    pub token_id: Option<String>,
    pub recipient_user_ids: Vec<String>,
    #[serde(default)]
    pub models: Vec<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedApiShareDistributions {
    pub batch_id: String,
    pub items: Vec<ApiShareDistributionView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedApiShareConnection {
    pub distribution_id: String,
    pub kind: String,
    pub source_id: Option<String>,
    pub source_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_format: String,
    pub api_key: String,
    pub models: Vec<String>,
    /// 仅随领取结果返回，用于把共享源中的模型备注恢复到本机 Provider。
    #[serde(default)]
    pub model_notes: HashMap<String, String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareUsageView {
    pub source_id: String,
    pub granularity: String,
    pub requests: i64,
    pub success_requests: i64,
    pub rate_limited_requests: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub trend: Vec<ApiShareUsagePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareUsagePoint {
    pub start_day: String,
    pub end_day: String,
    pub label: String,
    pub requests: i64,
    pub success_requests: i64,
    pub rate_limited_requests: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareProviderView {
    pub id: String,
    pub name: String,
    pub api_format: String,
    pub models: Vec<String>,
    pub api_key_set: bool,
    pub trust_domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareAccessInfo {
    pub gateway_url: String,
    pub port: u16,
    pub certificate_fingerprint: String,
    pub enabled: bool,
    pub disabled_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiShareMemberAccessView {
    pub user_id: String,
    pub display_name: String,
    pub email: String,
    pub role: String,
    pub blocked: bool,
    pub reason: String,
    pub blocked_by: String,
    pub blocked_at: Option<i64>,
}

#[derive(Debug, Clone)]
struct SourceRecord {
    id: String,
    org_id: String,
    provider_id: String,
    name: String,
    models: Vec<String>,
    enabled: bool,
    created_by: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone)]
struct TokenRecord {
    id: String,
    org_id: String,
    owner_user_id: String,
    created_by: String,
    name: String,
    token_prefix: String,
    secret_ciphertext: String,
    api_format: String,
    enabled: bool,
    revoked_at: Option<i64>,
    archived_at: Option<i64>,
    expires_at: Option<i64>,
    rpm_limit: i64,
    burst_limit: i64,
    max_concurrency: i64,
    daily_request_limit: i64,
    daily_token_limit: i64,
    created_at: i64,
}

#[derive(Debug, Clone)]
struct TokenGrantRecord {
    source_id: String,
    source_name: String,
    source_enabled: bool,
    source_model: String,
    exposed_model: String,
}

#[derive(Debug, Clone)]
struct TokenListRecord {
    id: String,
    org_id: String,
    owner_user_id: String,
    owner_name: String,
    created_by: String,
    name: String,
    token_prefix: String,
    api_format: String,
    enabled: bool,
    expires_at: Option<i64>,
    rpm_limit: i64,
    burst_limit: i64,
    max_concurrency: i64,
    daily_request_limit: i64,
    daily_token_limit: i64,
    copied_at: Option<i64>,
    first_used_at: Option<i64>,
    last_used_at: Option<i64>,
    request_count: i64,
    today_requests: i64,
    today_tokens: i64,
    today_rate_limited: i64,
    created_at: i64,
    revoked_at: Option<i64>,
    archived_at: Option<i64>,
    has_usage: bool,
    owner_blocked: bool,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn next_utc_midnight() -> i64 {
    let now = chrono::Utc::now();
    (now.date_naive() + chrono::Days::new(1))
        .and_hms_opt(0, 0, 0)
        .map(|value| value.and_utc().timestamp())
        .unwrap_or_else(|| now.timestamp() + 86_400)
}

pub fn cleanup_usage(conn: &Connection) -> Result<usize, String> {
    let cutoff = (chrono::Utc::now().date_naive()
        - chrono::Days::new(API_SHARE_USAGE_RETENTION_DAYS as u64))
    .format("%Y-%m-%d")
    .to_string();
    cleanup_usage_before(conn, &cutoff)
}

fn cleanup_usage_before(conn: &Connection, cutoff: &str) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM api_share_usage_daily
          WHERE day < ?1
             OR NOT EXISTS (
                  SELECT 1
                    FROM api_share_sources s
                   WHERE s.id = api_share_usage_daily.source_id
                     AND s.org_id = api_share_usage_daily.org_id
                )",
        params![cutoff],
    )
    .map_err(|e| format!("清理 API 共享用量失败：{}", e))
}

pub async fn run_maintenance_loop(state: Arc<EnclaveState>) {
    loop {
        match state.store.with_conn(cleanup_usage) {
            Ok(removed) if removed > 0 => {
                log::info!(
                    "[enclave] API 共享用量清理完成：移除 {} 条超过 {} 天的聚合记录",
                    removed,
                    API_SHARE_USAGE_RETENTION_DAYS
                );
            }
            Ok(_) => {}
            Err(error) => log::warn!("[enclave] API 共享用量清理失败：{}", error),
        }
        let delay = next_utc_midnight().saturating_sub(now()).max(60) as u64;
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

fn hash_secret(raw: &str) -> String {
    Sha256::digest(raw.as_bytes())
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

fn raw_token() -> String {
    format!(
        "pdsk_{}",
        crate::utils::new_id().replace('-', "").to_ascii_lowercase()
    )
}

fn ensure_distribution_credential(
    conn: &Connection,
    org_id: &str,
    distribution_id: &str,
    parent_token_id: &str,
) -> Result<String, String> {
    let existing = conn
        .query_row(
            "SELECT secret_ciphertext, enabled, revoked_at
               FROM api_share_distribution_credentials
              WHERE org_id = ?1 AND distribution_id = ?2",
            params![org_id, distribution_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("读取分发凭据失败：{}", e))?;
    if let Some((ciphertext, enabled, revoked_at)) = existing {
        if !enabled || revoked_at.is_some() {
            return Err("分发凭据已撤销".to_string());
        }
        return crate::utils::crypto::decrypt(&ciphertext)
            .map_err(|e| format!("解密分发凭据失败：{}", e));
    }
    let raw = raw_token();
    let encrypted =
        crate::utils::crypto::encrypt(&raw).map_err(|e| format!("加密分发凭据失败：{}", e))?;
    conn.execute(
        "INSERT INTO api_share_distribution_credentials
            (id, org_id, distribution_id, parent_token_id, token_prefix, token_hash,
             secret_ciphertext, enabled, revoked_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, NULL, ?8)",
        params![
            crate::utils::new_id(),
            org_id,
            distribution_id,
            parent_token_id,
            token_prefix(&raw),
            hash_secret(&raw),
            encrypted,
            now()
        ],
    )
    .map_err(|e| format!("创建分发凭据失败：{}", e))?;
    Ok(raw)
}

fn token_prefix(raw: &str) -> String {
    raw.chars().take(12).collect()
}

fn models_json(models: &[String]) -> String {
    serde_json::to_string(models).unwrap_or_else(|_| "[]".to_string())
}

fn parse_models(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn can_manage(role: Option<Role>) -> bool {
    matches!(role, Some(Role::Owner) | Some(Role::Admin))
}

fn member_is_blocked(conn: &Connection, org_id: &str, user_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM api_share_member_blocks
             WHERE org_id = ?1 AND user_id = ?2
         )",
        params![org_id, user_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|e| format!("读取 API 共享成员黑名单失败：{}", e))
}

fn member_access_view(
    conn: &Connection,
    org_id: &str,
    user_id: &str,
) -> Result<ApiShareMemberAccessView, String> {
    conn.query_row(
        "SELECT m.user_id,
                COALESCE(NULLIF(m.display_name, ''), NULLIF(m.email, ''), m.user_id),
                m.email, m.role,
                CASE WHEN b.user_id IS NULL THEN 0 ELSE 1 END,
                COALESCE(b.reason, ''), COALESCE(b.created_by, ''), b.created_at
           FROM member_snapshot m
           LEFT JOIN api_share_member_blocks b
             ON b.org_id = m.org_id AND b.user_id = m.user_id
          WHERE m.org_id = ?1 AND m.user_id = ?2",
        params![org_id, user_id],
        |row| {
            Ok(ApiShareMemberAccessView {
                user_id: row.get(0)?,
                display_name: row.get(1)?,
                email: row.get(2)?,
                role: row.get(3)?,
                blocked: row.get::<_, i64>(4)? != 0,
                reason: row.get(5)?,
                blocked_by: row.get(6)?,
                blocked_at: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("读取 API 共享成员权限失败：{}", e))?
    .ok_or_else(|| "成员不属于当前组织".to_string())
}

pub fn list_member_access(
    conn: &Connection,
    org_id: &str,
    actor: &str,
) -> Result<Vec<ApiShareMemberAccessView>, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可管理 API 共享成员黑名单".to_string());
    }
    let mut stmt = conn
        .prepare(
            "SELECT m.user_id,
                    COALESCE(NULLIF(m.display_name, ''), NULLIF(m.email, ''), m.user_id),
                    m.email, m.role,
                    CASE WHEN b.user_id IS NULL THEN 0 ELSE 1 END,
                    COALESCE(b.reason, ''), COALESCE(b.created_by, ''), b.created_at
               FROM member_snapshot m
               LEFT JOIN api_share_member_blocks b
                 ON b.org_id = m.org_id AND b.user_id = m.user_id
              WHERE m.org_id = ?1 AND m.seat_status = 'active'
              ORDER BY
                CASE m.role
                  WHEN 'owner' THEN 0
                  WHEN 'admin' THEN 1
                  ELSE 2
                END,
                COALESCE(NULLIF(m.display_name, ''), NULLIF(m.email, ''), m.user_id)",
        )
        .map_err(|e| format!("读取 API 共享成员权限失败：{}", e))?;
    let rows = stmt
        .query_map(params![org_id], |row| {
            Ok(ApiShareMemberAccessView {
                user_id: row.get(0)?,
                display_name: row.get(1)?,
                email: row.get(2)?,
                role: row.get(3)?,
                blocked: row.get::<_, i64>(4)? != 0,
                reason: row.get(5)?,
                blocked_by: row.get(6)?,
                blocked_at: row.get(7)?,
            })
        })
        .map_err(|e| format!("读取 API 共享成员权限失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取 API 共享成员权限失败：{}", e))
}

pub fn block_member(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    user_id: &str,
    reason: &str,
) -> Result<ApiShareMemberAccessView, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可修改 API 共享成员黑名单".to_string());
    }
    if user_id == actor {
        return Err("不能将当前账号加入 API 共享黑名单".to_string());
    }
    let target = member_access_view(conn, org_id, user_id)?;
    if target.role == "owner" {
        return Err("不能将组织所有者加入 API 共享黑名单".to_string());
    }
    let reason = reason.trim();
    let reason = if reason.is_empty() {
        "管理员限制其使用组织共享 API"
    } else {
        reason
    };
    conn.execute(
        "INSERT INTO api_share_member_blocks
            (org_id, user_id, reason, created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(org_id, user_id) DO UPDATE SET
            reason = excluded.reason,
            created_by = excluded.created_by,
            created_at = excluded.created_at",
        params![org_id, user_id, reason, actor, now()],
    )
    .map_err(|e| format!("加入 API 共享成员黑名单失败：{}", e))?;
    member_access_view(conn, org_id, user_id)
}

pub fn unblock_member(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    user_id: &str,
) -> Result<ApiShareMemberAccessView, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可修改 API 共享成员黑名单".to_string());
    }
    member_access_view(conn, org_id, user_id)?;
    conn.execute(
        "DELETE FROM api_share_member_blocks WHERE org_id = ?1 AND user_id = ?2",
        params![org_id, user_id],
    )
    .map_err(|e| format!("移出 API 共享成员黑名单失败：{}", e))?;
    member_access_view(conn, org_id, user_id)
}

fn validate_limits(
    rpm_limit: i64,
    burst_limit: i64,
    max_concurrency: i64,
    daily_request_limit: i64,
    daily_token_limit: i64,
) -> Result<(), String> {
    if rpm_limit < 0
        || burst_limit < 0
        || max_concurrency < 0
        || daily_request_limit < 0
        || daily_token_limit < 0
    {
        return Err("限流与额度不能为负数".to_string());
    }
    if rpm_limit == 0 && burst_limit > 0 {
        return Err("设置 Burst 前请先设置 RPM".to_string());
    }
    if max_concurrency > 1_000 {
        return Err("并发上限不能超过 1000".to_string());
    }
    Ok(())
}

fn validate_source_models(models: &[String]) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let normalized = models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .filter(|model| seen.insert((*model).to_string()))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        return Err("共享源至少需要选择一种模型".to_string());
    }
    Ok(normalized)
}

fn validate_model_scope(
    scope: &[String],
    requested: &[String],
    scope_label: &str,
) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let requested = requested
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .filter(|model| seen.insert((*model).to_string()))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    let allowed = scope.iter().map(String::as_str).collect::<HashSet<_>>();
    if let Some(model) = requested
        .iter()
        .find(|model| !allowed.contains(model.as_str()))
    {
        return Err(format!("模型 {} 不在{scope_label}允许列表中", model));
    }
    Ok(requested.to_vec())
}

fn validate_models(source: &SourceRecord, requested: &[String]) -> Result<Vec<String>, String> {
    validate_model_scope(&source.models, requested, "共享源")
}

fn name_key(value: &str) -> String {
    value.trim().to_lowercase()
}

fn exposed_model(source_name: &str, source_model: &str) -> String {
    format!("[{}]{}", source_name.trim(), source_model.trim())
}

fn ensure_source_name_available(
    conn: &Connection,
    org_id: &str,
    name: &str,
    excluding_id: Option<&str>,
) -> Result<(), String> {
    let key = name_key(name);
    if key.is_empty() {
        return Err("共享名称不能为空".to_string());
    }
    let exists: i64 = conn
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM api_share_sources
                WHERE org_id = ?1 AND name_key = ?2 AND (?3 IS NULL OR id <> ?3)
             )",
            params![org_id, key, excluding_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("检查共享名称失败：{}", e))?;
    if exists != 0 {
        return Err("共享名称在组织内已存在，请使用其他名称".to_string());
    }
    Ok(())
}

fn ensure_token_name_available(
    conn: &Connection,
    org_id: &str,
    name: &str,
    excluding_id: Option<&str>,
) -> Result<(), String> {
    let key = name_key(name);
    if key.is_empty() {
        return Err("令牌名称不能为空".to_string());
    }
    let exists: i64 = conn
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM api_share_tokens
                WHERE org_id = ?1 AND name_key = ?2 AND (?3 IS NULL OR id <> ?3)
             )",
            params![org_id, key, excluding_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("检查令牌名称失败：{}", e))?;
    if exists != 0 {
        return Err("令牌名称在组织内已存在，请使用其他名称".to_string());
    }
    Ok(())
}

fn validate_token_grants(
    state: &EnclaveState,
    conn: &Connection,
    org_id: &str,
    grants: &[ApiShareTokenGrantInput],
) -> Result<(Vec<TokenGrantRecord>, String), String> {
    if grants.is_empty() {
        return Err("路由令牌至少需要授权一个共享源".to_string());
    }
    let pool = state
        .main_db()
        .ok_or_else(|| "主库连接不可用，无法读取 Provider 协议".to_string())?;
    let main_conn = pool
        .get()
        .map_err(|e| format!("读取 Provider 配置失败：{}", e))?;
    let mut merged = HashMap::<String, (SourceRecord, Vec<String>)>::new();
    for grant in grants {
        let source_id = grant.source_id.trim();
        if source_id.is_empty() {
            return Err("共享源标识不能为空".to_string());
        }
        let entry = match merged.entry(source_id.to_string()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let source = get_source_record(conn, org_id, source_id)?;
                if !source.enabled {
                    return Err(format!("共享源 {} 未启用", source.name));
                }
                entry.insert((source, Vec::new()))
            }
        };
        let requested = if grant.models.is_empty() {
            entry.0.models.clone()
        } else {
            validate_models(&entry.0, &grant.models)?
        };
        for model in requested {
            if !entry.1.contains(&model) {
                entry.1.push(model);
            }
        }
    }

    let mut provider_formats = HashMap::<String, String>::new();
    let mut token_api_format: Option<String> = None;
    let mut out = Vec::new();
    let mut exposed_models = HashSet::new();
    let mut sources = merged.into_values().collect::<Vec<_>>();
    sources.sort_by(|left, right| left.0.name.cmp(&right.0.name));
    for (source, models) in sources {
        if models.is_empty() {
            return Err(format!("共享源 {} 没有可授权模型", source.name));
        }
        let api_format = match provider_formats.get(&source.provider_id) {
            Some(value) => value.clone(),
            None => {
                let provider = crate::commands::api_provider::get_api_provider(
                    &main_conn,
                    &source.provider_id,
                )
                .map_err(|e| format!("读取 Provider 配置失败：{}", e))?
                .ok_or_else(|| format!("共享源 {} 对应的 Provider 已删除", source.name))?;
                let value = provider_api_format(&provider.api_format);
                provider_formats.insert(source.provider_id.clone(), value.clone());
                value
            }
        };
        if let Some(expected) = token_api_format.as_ref() {
            if !expected.eq_ignore_ascii_case(&api_format) {
                return Err(format!(
                    "同一令牌只能组合相同 API Format 的共享源（{} 与 {} 不一致）",
                    expected, api_format
                ));
            }
        } else {
            token_api_format = Some(api_format);
        }
        for source_model in models {
            let alias = exposed_model(&source.name, &source_model);
            if !exposed_models.insert(alias.clone()) {
                return Err(format!("对外模型标识重复：{}", alias));
            }
            out.push(TokenGrantRecord {
                source_id: source.id.clone(),
                source_name: source.name.clone(),
                source_enabled: source.enabled,
                source_model,
                exposed_model: alias,
            });
        }
    }
    Ok((
        out,
        token_api_format.unwrap_or_else(|| "openai".to_string()),
    ))
}

fn provider_api_format(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "openai".to_string()
    } else {
        value.to_string()
    }
}

pub fn provider_catalog(
    state: &EnclaveState,
    org_id: &str,
    actor: &str,
) -> Result<Vec<ApiShareProviderView>, String> {
    state
        .store
        .with_conn(|conn| provider_catalog_with_conn(state, conn, org_id, actor))
}

pub fn provider_catalog_with_conn(
    state: &EnclaveState,
    conn: &Connection,
    org_id: &str,
    actor: &str,
) -> Result<Vec<ApiShareProviderView>, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可查看可共享 Provider".to_string());
    }
    let pool = state
        .main_db()
        .ok_or_else(|| "主库连接不可用".to_string())?;
    let main_conn = pool
        .get()
        .map_err(|e| format!("读取 Provider 配置失败：{}", e))?;
    let providers = crate::commands::api_provider::list_api_providers(&main_conn)
        .map_err(|e| format!("读取 Provider 配置失败：{}", e))?;
    Ok(providers
        .into_iter()
        .map(|provider| ApiShareProviderView {
            trust_domain: crate::commands::api_provider::provider_trust_domain(
                &main_conn,
                &provider.id,
            )
            .as_str()
            .to_string(),
            id: provider.id,
            name: provider.name,
            api_format: provider.api_format,
            models: provider.models,
            api_key_set: provider.api_key_set,
        })
        .collect())
}

fn source_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRecord> {
    Ok(SourceRecord {
        id: row.get(0)?,
        org_id: row.get(1)?,
        provider_id: row.get(2)?,
        name: row.get(3)?,
        models: parse_models(&row.get::<_, String>(4)?),
        enabled: row.get::<_, i64>(5)? != 0,
        created_by: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn get_source_record(
    conn: &Connection,
    org_id: &str,
    source_id: &str,
) -> Result<SourceRecord, String> {
    conn.query_row(
        "SELECT id, org_id, provider_id, name, models, enabled,
                created_by, created_at, updated_at
           FROM api_share_sources
          WHERE id = ?1 AND org_id = ?2",
        params![source_id, org_id],
        source_from_row,
    )
    .optional()
    .map_err(|e| format!("读取 API 共享源失败：{}", e))?
    .ok_or_else(|| "API 共享源不存在或不属于当前组织".to_string())
}

fn source_view(
    conn: &Connection,
    record: SourceRecord,
    role: Option<Role>,
) -> Result<ApiShareSourceView, String> {
    let token_count = conn
        .query_row(
            "SELECT COUNT(DISTINCT g.token_id)
               FROM api_share_token_grants g
               JOIN api_share_tokens t ON t.id = g.token_id
              WHERE g.source_id = ?1 AND t.org_id = ?2 AND t.enabled = 1
                AND t.archived_at IS NULL
                AND (t.expires_at IS NULL OR t.expires_at > ?3)",
            params![record.id, record.org_id, now()],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    let distribution_count = conn
        .query_row(
            "SELECT COUNT(*) FROM api_share_distributions
              WHERE source_id = ?1 AND org_id = ?2 AND revoked_at IS NULL
                AND archived_at IS NULL
                AND applied_at IS NULL AND (expires_at IS NULL OR expires_at > ?3)",
            params![record.id, record.org_id, now()],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    let (request_count, total_tokens) = conn
        .query_row(
            "SELECT COALESCE(SUM(requests), 0), COALESCE(SUM(total_tokens), 0)
               FROM api_share_usage_daily
              WHERE source_id = ?1 AND org_id = ?2",
            params![record.id, record.org_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, 0));
    Ok(ApiShareSourceView {
        id: record.id,
        org_id: record.org_id,
        provider_id: record.provider_id,
        name: record.name,
        models: record.models,
        enabled: record.enabled,
        created_by: record.created_by,
        created_at: record.created_at,
        updated_at: record.updated_at,
        token_count,
        distribution_count,
        request_count,
        total_tokens,
        can_manage: can_manage(role),
    })
}

pub fn list_sources(
    conn: &Connection,
    org_id: &str,
    actor: &str,
) -> Result<Vec<ApiShareSourceView>, String> {
    let role = authz::role_for_member(conn, org_id, actor)?;
    if role.is_none() {
        return Err("当前账号不是该组织成员".to_string());
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, org_id, provider_id, name, models, enabled,
                    created_by, created_at, updated_at
               FROM api_share_sources
              WHERE org_id = ?1 AND (?2 = 1 OR enabled = 1)
              ORDER BY updated_at DESC",
        )
        .map_err(|e| format!("读取 API 共享列表失败：{}", e))?;
    let rows = stmt
        .query_map(
            params![org_id, if can_manage(role) { 1 } else { 0 }],
            source_from_row,
        )
        .map_err(|e| format!("读取 API 共享列表失败：{}", e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(source_view(
            conn,
            row.map_err(|e| format!("读取 API 共享源失败：{}", e))?,
            role,
        )?);
    }
    Ok(out)
}

pub fn create_source(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    input: &CreateApiShareSourceInput,
) -> Result<ApiShareSourceView, String> {
    let role = authz::role_for_member(conn, org_id, actor)?;
    if !can_manage(role) {
        return Err("仅组织所有者/管理员可发布 API 共享".to_string());
    }
    if input.provider_id.trim().is_empty() {
        return Err("请选择 API Provider".to_string());
    }
    let models = validate_source_models(&input.models)?;
    let id = crate::utils::new_id();
    let at = now();
    let name = if input.name.trim().is_empty() {
        input.provider_id.trim()
    } else {
        input.name.trim()
    };
    ensure_source_name_available(conn, org_id, name, None)?;
    conn.execute(
        "INSERT INTO api_share_sources
            (id, org_id, provider_id, name, name_key, models, enabled,
             created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![
            id,
            org_id,
            input.provider_id.trim(),
            name,
            name_key(name),
            models_json(&models),
            if input.enabled { 1 } else { 0 },
            actor,
            at
        ],
    )
    .map_err(|e| format!("创建 API 共享源失败：{}", e))?;
    source_view(conn, get_source_record(conn, org_id, &id)?, role)
}

pub fn update_source(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    source_id: &str,
    input: &UpdateApiShareSourceInput,
) -> Result<ApiShareSourceView, String> {
    let role = authz::role_for_member(conn, org_id, actor)?;
    if !can_manage(role) {
        return Err("仅组织所有者/管理员可修改 API 共享".to_string());
    }
    let current = get_source_record(conn, org_id, source_id)?;
    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&current.name)
        .to_string();
    if name != current.name {
        let referenced: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_share_token_grants WHERE source_id = ?1",
                params![source_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("检查共享源引用失败：{}", e))?;
        if referenced > 0 {
            return Err("该共享源已被令牌引用，不能修改名称；请先解除相关模型授权".to_string());
        }
        ensure_source_name_available(conn, org_id, &name, Some(source_id))?;
    }
    let models = match input.models.as_ref() {
        Some(models) => validate_source_models(models)?,
        None => current.models.clone(),
    };
    let enabled = input.enabled.unwrap_or(current.enabled);
    conn.execute(
        "UPDATE api_share_sources
            SET name = ?1, name_key = ?2, models = ?3, enabled = ?4, updated_at = ?5
          WHERE id = ?6 AND org_id = ?7",
        params![
            name,
            name_key(&name),
            models_json(&models),
            if enabled { 1 } else { 0 },
            now(),
            source_id,
            org_id
        ],
    )
    .map_err(|e| format!("更新 API 共享源失败：{}", e))?;
    let removed_models = current
        .models
        .iter()
        .filter(|model| !models.contains(model))
        .cloned()
        .collect::<Vec<_>>();
    for model in removed_models {
        conn.execute(
            "DELETE FROM api_share_token_grants
              WHERE source_id = ?1 AND source_model = ?2",
            params![source_id, model],
        )
        .map_err(|e| format!("同步移除失效模型授权失败：{}", e))?;
    }
    source_view(conn, get_source_record(conn, org_id, source_id)?, role)
}

pub fn delete_source(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    source_id: &str,
) -> Result<(), String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可删除 API 共享".to_string());
    }
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启共享源删除事务失败：{}", e))?;
    tx.execute(
        "DELETE FROM api_share_usage_daily WHERE org_id = ?1 AND source_id = ?2",
        params![org_id, source_id],
    )
    .map_err(|e| format!("删除共享源用量失败：{}", e))?;
    let changed = tx
        .execute(
            "DELETE FROM api_share_sources WHERE id = ?1 AND org_id = ?2",
            params![source_id, org_id],
        )
        .map_err(|e| format!("删除 API 共享源失败：{}", e))?;
    if changed == 0 {
        return Err("API 共享源不存在".to_string());
    }
    tx.commit()
        .map_err(|e| format!("提交共享源删除事务失败：{}", e))?;
    Ok(())
}

fn token_record(conn: &Connection, org_id: &str, token_id: &str) -> Result<TokenRecord, String> {
    conn.query_row(
        "SELECT id, org_id, owner_user_id, created_by, name, token_prefix,
                secret_ciphertext, api_format, enabled, revoked_at, archived_at,
                expires_at, rpm_limit, burst_limit, max_concurrency,
                daily_request_limit, daily_token_limit, created_at
           FROM api_share_tokens
          WHERE id = ?1 AND org_id = ?2",
        params![token_id, org_id],
        |row| {
            Ok(TokenRecord {
                id: row.get(0)?,
                org_id: row.get(1)?,
                owner_user_id: row.get(2)?,
                created_by: row.get(3)?,
                name: row.get(4)?,
                token_prefix: row.get(5)?,
                secret_ciphertext: row.get(6)?,
                api_format: row.get(7)?,
                enabled: row.get::<_, i64>(8)? != 0,
                revoked_at: row.get(9)?,
                archived_at: row.get(10)?,
                expires_at: row.get(11)?,
                rpm_limit: row.get(12)?,
                burst_limit: row.get(13)?,
                max_concurrency: row.get(14)?,
                daily_request_limit: row.get(15)?,
                daily_token_limit: row.get(16)?,
                created_at: row.get(17)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("读取共享令牌失败：{}", e))?
    .ok_or_else(|| "共享令牌不存在".to_string())
}

fn token_grants(
    conn: &Connection,
    org_id: &str,
    token_id: &str,
) -> Result<Vec<TokenGrantRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT g.source_id, s.name, s.enabled, g.source_model, g.exposed_model
               FROM api_share_token_grants g
               JOIN api_share_sources s
                 ON s.id = g.source_id AND s.org_id = ?2
              WHERE g.token_id = ?1
              ORDER BY s.name, g.source_model",
        )
        .map_err(|e| format!("读取令牌模型授权失败：{}", e))?;
    let rows = stmt
        .query_map(params![token_id, org_id], |row| {
            Ok(TokenGrantRecord {
                source_id: row.get(0)?,
                source_name: row.get(1)?,
                source_enabled: row.get::<_, i64>(2)? != 0,
                source_model: row.get(3)?,
                exposed_model: row.get(4)?,
            })
        })
        .map_err(|e| format!("读取令牌模型授权失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取令牌模型授权失败：{}", e))
}

fn token_view(conn: &Connection, record: TokenListRecord) -> Result<ApiShareTokenView, String> {
    let grants = token_grants(conn, &record.org_id, &record.id)?;
    let mut sources = Vec::<ApiShareTokenSourceView>::new();
    let mut source_indexes = HashMap::<String, usize>::new();
    let mut allowed_models = Vec::new();
    for grant in grants {
        let index = match source_indexes.get(&grant.source_id) {
            Some(index) => *index,
            None => {
                let index = sources.len();
                source_indexes.insert(grant.source_id.clone(), index);
                sources.push(ApiShareTokenSourceView {
                    source_id: grant.source_id,
                    source_name: grant.source_name,
                    source_enabled: grant.source_enabled,
                    models: Vec::new(),
                });
                index
            }
        };
        if !sources[index].models.contains(&grant.exposed_model) {
            sources[index].models.push(grant.exposed_model.clone());
        }
        if !allowed_models.contains(&grant.exposed_model) {
            allowed_models.push(grant.exposed_model);
        }
    }
    Ok(ApiShareTokenView {
        id: record.id,
        org_id: record.org_id,
        owner_user_id: record.owner_user_id,
        owner_name: record.owner_name,
        created_by: record.created_by,
        name: record.name,
        token_prefix: record.token_prefix,
        api_format: record.api_format,
        sources,
        allowed_models,
        enabled: record.enabled,
        revoked_at: record.revoked_at,
        archived_at: record.archived_at,
        expires_at: record.expires_at,
        rpm_limit: record.rpm_limit,
        burst_limit: record.burst_limit,
        max_concurrency: record.max_concurrency,
        daily_request_limit: record.daily_request_limit,
        daily_token_limit: record.daily_token_limit,
        copied_at: record.copied_at,
        first_used_at: record.first_used_at,
        last_used_at: record.last_used_at,
        request_count: record.request_count,
        has_usage: record.has_usage,
        owner_blocked: record.owner_blocked,
        today_requests: record.today_requests,
        today_tokens: record.today_tokens,
        today_rate_limited: record.today_rate_limited,
        created_at: record.created_at,
    })
}

fn token_list_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TokenListRecord> {
    Ok(TokenListRecord {
        id: row.get(0)?,
        org_id: row.get(1)?,
        owner_user_id: row.get(2)?,
        owner_name: row.get(3)?,
        created_by: row.get(4)?,
        name: row.get(5)?,
        token_prefix: row.get(6)?,
        api_format: row.get(7)?,
        enabled: row.get::<_, i64>(8)? != 0,
        expires_at: row.get(9)?,
        rpm_limit: row.get(10)?,
        burst_limit: row.get(11)?,
        max_concurrency: row.get(12)?,
        daily_request_limit: row.get(13)?,
        daily_token_limit: row.get(14)?,
        copied_at: row.get(15)?,
        first_used_at: row.get(16)?,
        last_used_at: row.get(17)?,
        request_count: row.get(18)?,
        today_requests: row.get(19)?,
        today_tokens: row.get(20)?,
        today_rate_limited: row.get(21)?,
        created_at: row.get(22)?,
        revoked_at: row.get(23)?,
        archived_at: row.get(24)?,
        has_usage: row.get::<_, i64>(25)? != 0,
        owner_blocked: row.get::<_, i64>(26)? != 0,
    })
}

const TOKEN_VIEW_COLUMNS: &str = "SELECT t.id, t.org_id, t.owner_user_id,
            COALESCE(NULLIF(m.display_name, ''), NULLIF(m.email, ''), t.owner_user_id),
            t.created_by, t.name, t.token_prefix, t.api_format, t.enabled,
            t.expires_at, t.rpm_limit, t.burst_limit, t.max_concurrency,
            t.daily_request_limit, t.daily_token_limit, t.copied_at, t.first_used_at,
            t.last_used_at, t.request_count,
            COALESCE(u.requests, 0), COALESCE(u.total_tokens, 0),
            COALESCE(u.rate_limited_requests, 0), t.created_at,
            t.revoked_at, t.archived_at,
            (
              t.first_used_at IS NOT NULL
              OR t.last_used_at IS NOT NULL
              OR t.request_count > 0
              OR EXISTS (
                SELECT 1 FROM api_share_usage_daily h
                 WHERE h.org_id = t.org_id AND h.token_id = t.id
              )
            ),
            EXISTS (
              SELECT 1 FROM api_share_member_blocks b
               WHERE b.org_id = t.org_id AND b.user_id = t.owner_user_id
            )
       FROM api_share_tokens t
       LEFT JOIN member_snapshot m
         ON m.org_id = t.org_id AND m.user_id = t.owner_user_id
       LEFT JOIN (
         SELECT org_id, token_id,
                SUM(requests) AS requests,
                SUM(total_tokens) AS total_tokens,
                SUM(rate_limited_requests) AS rate_limited_requests
           FROM api_share_usage_daily
          WHERE day = ?2
          GROUP BY org_id, token_id
       ) u ON u.org_id = t.org_id AND u.token_id = t.id";

fn get_token_view(
    conn: &Connection,
    org_id: &str,
    token_id: &str,
) -> Result<ApiShareTokenView, String> {
    conn.query_row(
        &format!("{TOKEN_VIEW_COLUMNS} WHERE t.org_id = ?1 AND t.id = ?3"),
        params![org_id, today(), token_id],
        token_list_record_from_row,
    )
    .optional()
    .map_err(|e| format!("读取共享令牌失败：{}", e))?
    .ok_or_else(|| "共享令牌不存在".to_string())
    .and_then(|record| token_view(conn, record))
}

pub fn list_tokens(
    conn: &Connection,
    org_id: &str,
    actor: &str,
) -> Result<Vec<ApiShareTokenView>, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可管理路由令牌".to_string());
    }
    let mut stmt = conn
        .prepare(&format!(
            "{TOKEN_VIEW_COLUMNS}
             WHERE t.org_id = ?1
             ORDER BY t.created_at DESC"
        ))
        .map_err(|e| format!("读取共享令牌失败：{}", e))?;
    let rows = stmt
        .query_map(params![org_id, today()], token_list_record_from_row)
        .map_err(|e| format!("读取共享令牌失败：{}", e))?;
    let records = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取共享令牌失败：{}", e))?;
    records
        .into_iter()
        .map(|record| token_view(conn, record))
        .collect()
}

pub fn create_token(
    state: &EnclaveState,
    conn: &Connection,
    org_id: &str,
    actor: &str,
    input: &CreateApiShareTokenInput,
) -> Result<IssuedApiShareToken, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可签发路由令牌".to_string());
    }
    let owner = if input.owner_user_id.trim().is_empty() {
        actor
    } else {
        input.owner_user_id.trim()
    };
    authz::role_for_member(conn, org_id, owner)?
        .ok_or_else(|| "令牌所有者不属于当前组织".to_string())?;
    if member_is_blocked(conn, org_id, owner)? {
        return Err("该成员已被 API 共享黑名单限制，无法创建或持有新令牌".to_string());
    }
    if let Some(expires_at) = input.expires_at {
        if expires_at <= now() {
            return Err("令牌过期时间必须晚于当前时间".to_string());
        }
    }
    validate_limits(
        input.rpm_limit,
        input.burst_limit,
        input.max_concurrency,
        input.daily_request_limit,
        input.daily_token_limit,
    )?;
    let (grants, api_format) = validate_token_grants(state, conn, org_id, &input.grants)?;
    let token = raw_token();
    let encrypted =
        crate::utils::crypto::encrypt(&token).map_err(|e| format!("加密共享令牌失败：{}", e))?;
    let id = crate::utils::new_id();
    let at = now();
    let name = if input.name.trim().is_empty() {
        format!("{} 的令牌", owner)
    } else {
        input.name.trim().to_string()
    };
    ensure_token_name_available(conn, org_id, &name, None)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启共享令牌创建事务失败：{}", e))?;
    tx.execute(
        "INSERT INTO api_share_tokens
            (id, org_id, owner_user_id, created_by, name, name_key, api_format,
             token_prefix, token_hash, secret_ciphertext, secret_version, enabled,
             expires_at, rpm_limit, burst_limit, max_concurrency, daily_request_limit,
             daily_token_limit, copied_at, first_used_at, last_used_at, request_count,
             created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, 1, ?11, ?12, ?13,
                 ?14, ?15, ?16, NULL, NULL, NULL, 0, ?17)",
        params![
            id,
            org_id,
            owner,
            actor,
            name,
            name_key(&name),
            api_format,
            token_prefix(&token),
            hash_secret(&token),
            encrypted,
            input.expires_at,
            input.rpm_limit,
            input.burst_limit,
            input.max_concurrency,
            input.daily_request_limit,
            input.daily_token_limit,
            at
        ],
    )
    .map_err(|e| format!("创建共享令牌失败：{}", e))?;
    for grant in grants {
        tx.execute(
            "INSERT INTO api_share_token_grants
                (token_id, source_id, source_model, exposed_model, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                grant.source_id,
                grant.source_model,
                grant.exposed_model,
                at
            ],
        )
        .map_err(|e| format!("写入令牌模型授权失败：{}", e))?;
    }
    tx.commit()
        .map_err(|e| format!("提交共享令牌创建事务失败：{}", e))?;
    Ok(IssuedApiShareToken {
        token,
        base_url: gateway_base_url(),
        view: get_token_view(conn, org_id, &id)?,
    })
}

pub fn update_token(
    state: &EnclaveState,
    conn: &Connection,
    org_id: &str,
    actor: &str,
    token_id: &str,
    input: &UpdateApiShareTokenInput,
) -> Result<ApiShareTokenView, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可修改路由令牌".to_string());
    }
    let current = token_record(conn, org_id, token_id)?;
    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&current.name)
        .to_string();
    if name != current.name {
        let distributed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_share_distributions
                  WHERE org_id = ?1 AND token_id = ?2",
                params![org_id, token_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("检查令牌分发记录失败：{}", e))?;
        if distributed > 0 {
            return Err("该令牌已产生分发记录，不能修改名称".to_string());
        }
        ensure_token_name_available(conn, org_id, &name, Some(token_id))?;
    }
    let validated = input
        .grants
        .as_ref()
        .map(|grants| validate_token_grants(state, conn, org_id, grants))
        .transpose()?;
    let api_format = validated
        .as_ref()
        .map(|(_, api_format)| api_format.clone())
        .unwrap_or_else(|| current.api_format.clone());
    let expires_at = if input.clear_expiry {
        None
    } else {
        input.expires_at.or(current.expires_at)
    };
    if let Some(value) = expires_at {
        if value <= now() {
            return Err("令牌过期时间必须晚于当前时间".to_string());
        }
    }
    let rpm = input.rpm_limit.unwrap_or(current.rpm_limit);
    let burst = input.burst_limit.unwrap_or(current.burst_limit);
    let concurrency = input.max_concurrency.unwrap_or(current.max_concurrency);
    let daily_requests = input
        .daily_request_limit
        .unwrap_or(current.daily_request_limit);
    let daily_tokens = input.daily_token_limit.unwrap_or(current.daily_token_limit);
    validate_limits(rpm, burst, concurrency, daily_requests, daily_tokens)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启共享令牌更新事务失败：{}", e))?;
    tx.execute(
        "UPDATE api_share_tokens
            SET name = ?1, name_key = ?2, api_format = ?3, expires_at = ?4,
                rpm_limit = ?5, burst_limit = ?6, max_concurrency = ?7,
                daily_request_limit = ?8, daily_token_limit = ?9
          WHERE id = ?10 AND org_id = ?11",
        params![
            name,
            name_key(&name),
            api_format,
            expires_at,
            rpm,
            burst,
            concurrency,
            daily_requests,
            daily_tokens,
            token_id,
            org_id
        ],
    )
    .map_err(|e| format!("更新共享令牌额度失败：{}", e))?;
    if let Some((grants, _)) = validated {
        tx.execute(
            "DELETE FROM api_share_token_grants WHERE token_id = ?1",
            params![token_id],
        )
        .map_err(|e| format!("清理旧令牌模型授权失败：{}", e))?;
        let at = now();
        for grant in grants {
            tx.execute(
                "INSERT INTO api_share_token_grants
                    (token_id, source_id, source_model, exposed_model, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    token_id,
                    grant.source_id,
                    grant.source_model,
                    grant.exposed_model,
                    at
                ],
            )
            .map_err(|e| format!("更新令牌模型授权失败：{}", e))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("提交共享令牌更新事务失败：{}", e))?;
    get_token_view(conn, org_id, token_id)
}

pub fn revoke_token(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    token_id: &str,
) -> Result<(), String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可撤销路由令牌".to_string());
    }
    token_record(conn, org_id, token_id)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启令牌撤销事务失败：{}", e))?;
    tx.execute(
        "UPDATE api_share_tokens
            SET enabled = 0, revoked_at = COALESCE(revoked_at, ?1)
          WHERE id = ?2 AND org_id = ?3",
        params![now(), token_id, org_id],
    )
    .map_err(|e| format!("撤销共享令牌失败：{}", e))?;
    tx.execute(
        "UPDATE api_share_distribution_credentials
            SET enabled = 0, revoked_at = COALESCE(revoked_at, ?1)
          WHERE org_id = ?2 AND parent_token_id = ?3",
        params![now(), org_id, token_id],
    )
    .map_err(|e| format!("撤销令牌分发凭据失败：{}", e))?;
    tx.commit()
        .map_err(|e| format!("提交令牌撤销失败：{}", e))?;
    Ok(())
}

fn token_has_usage(conn: &Connection, org_id: &str, token_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
              FROM api_share_tokens t
             WHERE t.id = ?1 AND t.org_id = ?2
               AND (
                 t.first_used_at IS NOT NULL
                 OR t.last_used_at IS NOT NULL
                 OR t.request_count > 0
                 OR EXISTS (
                   SELECT 1 FROM api_share_usage_daily u
                    WHERE u.org_id = t.org_id AND u.token_id = t.id
                 )
               )
         )",
        params![token_id, org_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|e| format!("检查共享令牌使用记录失败：{}", e))
}

pub fn archive_token(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    token_id: &str,
) -> Result<ApiShareTokenView, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可归档路由令牌".to_string());
    }
    let current = token_record(conn, org_id, token_id)?;
    if current.enabled {
        return Err("请先撤销令牌后再归档".to_string());
    }
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启令牌归档事务失败：{}", e))?;
    let at = now();
    tx.execute(
        "UPDATE api_share_tokens
            SET archived_at = COALESCE(archived_at, ?1)
          WHERE id = ?2 AND org_id = ?3",
        params![at, token_id, org_id],
    )
    .map_err(|e| format!("归档共享令牌失败：{}", e))?;
    tx.execute(
        "UPDATE api_share_distributions
            SET archived_at = COALESCE(archived_at, ?1)
          WHERE org_id = ?2 AND token_id = ?3",
        params![at, org_id, token_id],
    )
    .map_err(|e| format!("归档令牌分发记录失败：{}", e))?;
    tx.commit()
        .map_err(|e| format!("提交令牌归档失败：{}", e))?;
    get_token_view(conn, org_id, token_id)
}

pub fn delete_token(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    token_id: &str,
) -> Result<(), String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可删除路由令牌".to_string());
    }
    let current = token_record(conn, org_id, token_id)?;
    if current.enabled {
        return Err("请先撤销令牌后再删除".to_string());
    }
    if token_has_usage(conn, org_id, token_id)? {
        return Err("该令牌已有调用数据，只能归档，不能删除".to_string());
    }
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启令牌删除事务失败：{}", e))?;
    tx.execute(
        "DELETE FROM api_share_distributions
          WHERE org_id = ?1 AND token_id = ?2",
        params![org_id, token_id],
    )
    .map_err(|e| format!("删除令牌分发记录失败：{}", e))?;
    let changed = tx
        .execute(
            "DELETE FROM api_share_tokens WHERE id = ?1 AND org_id = ?2",
            params![token_id, org_id],
        )
        .map_err(|e| format!("删除共享令牌失败：{}", e))?;
    if changed == 0 {
        return Err("共享令牌不存在".to_string());
    }
    tx.commit()
        .map_err(|e| format!("提交令牌删除失败：{}", e))?;
    Ok(())
}

pub fn token_secret(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    token_id: &str,
) -> Result<ApiShareTokenSecret, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可复制路由令牌".to_string());
    }
    let token = token_record(conn, org_id, token_id)?;
    if member_is_blocked(conn, org_id, &token.owner_user_id)? {
        return Err("令牌所有者已被 API 共享黑名单限制".to_string());
    }
    if !token.enabled {
        return Err("共享令牌已撤销".to_string());
    }
    if token
        .expires_at
        .is_some_and(|expires_at| expires_at <= now())
    {
        return Err("共享令牌已过期".to_string());
    }
    if token_grants(conn, org_id, token_id)?
        .iter()
        .all(|grant| !grant.source_enabled)
    {
        return Err("令牌关联的共享源均已停用".to_string());
    }
    let raw = crate::utils::crypto::decrypt(&token.secret_ciphertext)
        .map_err(|e| format!("解密共享令牌失败：{}", e))?;
    conn.execute(
        "UPDATE api_share_tokens SET copied_at = COALESCE(copied_at, ?1)
          WHERE id = ?2 AND org_id = ?3",
        params![now(), token_id, org_id],
    )
    .map_err(|e| format!("更新令牌复制状态失败：{}", e))?;
    Ok(ApiShareTokenSecret {
        token: raw,
        base_url: gateway_base_url(),
        expires_at: token.expires_at,
    })
}

fn distribution_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApiShareDistributionView> {
    Ok(ApiShareDistributionView {
        id: row.get(0)?,
        org_id: row.get(1)?,
        kind: row.get(2)?,
        source_id: row.get(3)?,
        source_name: row.get(4)?,
        token_id: row.get(5)?,
        token_name: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        batch_id: row.get(7)?,
        created_by: row.get(8)?,
        creator_name: row.get(9)?,
        recipient_user_id: row.get(10)?,
        recipient_name: row.get(11)?,
        models: parse_models(&row.get::<_, String>(12)?),
        message_id: row.get(13)?,
        sent_at: row.get(14)?,
        opened_at: row.get(15)?,
        applied_at: row.get(16)?,
        revoked_at: row.get(17)?,
        expires_at: row.get(18)?,
        created_at: row.get(19)?,
        source_enabled: row.get::<_, i64>(20)? != 0,
        token_enabled: row.get::<_, i64>(21)? != 0,
        token_expires_at: row.get(22)?,
        archived_at: row.get(23)?,
        recipient_blocked: row.get::<_, i64>(24)? != 0,
        token_owner_blocked: row.get::<_, i64>(25)? != 0,
    })
}

const DISTRIBUTION_VIEW_COLUMNS: &str = "SELECT d.id, d.org_id, d.kind, d.source_id,
            CASE WHEN d.kind = 'route_token'
                 THEN COALESCE(t.name, '')
                 ELSE COALESCE(s.name, '')
            END,
            d.token_id, t.name,
            d.batch_id, d.created_by,
            COALESCE(NULLIF(cm.display_name, ''), NULLIF(cm.email, ''), d.created_by),
            d.recipient_user_id,
            COALESCE(NULLIF(rm.display_name, ''), NULLIF(rm.email, ''), d.recipient_user_id),
            d.models, d.message_id, d.sent_at, d.opened_at, d.applied_at, d.revoked_at,
            d.expires_at, d.created_at,
            CASE WHEN d.kind = 'route_token'
                 THEN EXISTS (
                   SELECT 1
                     FROM api_share_token_grants g
                     JOIN api_share_sources gs
                       ON gs.id = g.source_id AND gs.org_id = d.org_id
                    WHERE g.token_id = d.token_id AND gs.enabled = 1
                 )
                 ELSE COALESCE(s.enabled, 0)
            END,
            COALESCE(t.enabled, 0), t.expires_at,
            d.archived_at,
            EXISTS (
              SELECT 1 FROM api_share_member_blocks b
               WHERE b.org_id = d.org_id AND b.user_id = d.recipient_user_id
            ),
            EXISTS (
              SELECT 1 FROM api_share_member_blocks b
               WHERE b.org_id = d.org_id AND b.user_id = t.owner_user_id
            )
       FROM api_share_distributions d
       LEFT JOIN api_share_sources s ON s.id = d.source_id AND s.org_id = d.org_id
       LEFT JOIN api_share_tokens t
         ON t.id = d.token_id AND t.org_id = d.org_id
       LEFT JOIN member_snapshot cm
         ON cm.org_id = d.org_id AND cm.user_id = d.created_by
       LEFT JOIN member_snapshot rm
         ON rm.org_id = d.org_id AND rm.user_id = d.recipient_user_id";

fn get_distribution_view(
    conn: &Connection,
    org_id: &str,
    distribution_id: &str,
) -> Result<ApiShareDistributionView, String> {
    conn.query_row(
        &format!("{DISTRIBUTION_VIEW_COLUMNS} WHERE d.org_id = ?1 AND d.id = ?2"),
        params![org_id, distribution_id],
        distribution_from_row,
    )
    .optional()
    .map_err(|e| format!("读取 API 共享分发记录失败：{}", e))?
    .ok_or_else(|| "API 共享分发记录不存在".to_string())
}

pub fn list_distributions(
    conn: &Connection,
    org_id: &str,
    actor: &str,
) -> Result<Vec<ApiShareDistributionView>, String> {
    let role = authz::role_for_member(conn, org_id, actor)?
        .ok_or_else(|| "当前账号不是该组织成员".to_string())?;
    let manager = can_manage(Some(role));
    let mut stmt = conn
        .prepare(&format!(
            "{DISTRIBUTION_VIEW_COLUMNS}
             WHERE d.org_id = ?1
               AND (?3 = 1 OR d.recipient_user_id = ?4 OR d.created_by = ?4)
             ORDER BY d.created_at DESC"
        ))
        .map_err(|e| format!("读取 API 共享分发记录失败：{}", e))?;
    let rows = stmt
        .query_map(
            params![org_id, "", if manager { 1 } else { 0 }, actor],
            distribution_from_row,
        )
        .map_err(|e| format!("读取 API 共享分发记录失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取 API 共享分发记录失败：{}", e))
}

pub fn create_distributions(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    input: &CreateApiShareDistributionInput,
) -> Result<CreatedApiShareDistributions, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可分发连接".to_string());
    }
    let kind = input.kind.trim();
    if !matches!(
        kind,
        DISTRIBUTION_KIND_ROUTE_TOKEN | DISTRIBUTION_KIND_PROVIDER_CONFIG
    ) {
        return Err("不支持的分发类型".to_string());
    }
    let recipients = input
        .recipient_user_ids
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if recipients.is_empty() {
        return Err("请选择接收成员".to_string());
    }
    let mut unique = HashSet::new();
    for recipient in &recipients {
        if recipient == actor {
            return Err("不能分发连接给自己".to_string());
        }
        authz::role_for_member(conn, org_id, recipient)?
            .ok_or_else(|| "接收成员不属于当前组织".to_string())?;
        if member_is_blocked(conn, org_id, recipient)? {
            return Err("接收成员已被 API 共享黑名单限制".to_string());
        }
        unique.insert(recipient.clone());
    }
    let (token, source, models) = if kind == DISTRIBUTION_KIND_ROUTE_TOKEN {
        let token_id = input
            .token_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "路由令牌分发必须选择令牌".to_string())?;
        let token = token_record(conn, org_id, token_id)?;
        if member_is_blocked(conn, org_id, &token.owner_user_id)? {
            return Err("令牌所有者已被 API 共享黑名单限制".to_string());
        }
        if !token.enabled {
            return Err("共享令牌已撤销".to_string());
        }
        if token
            .expires_at
            .is_some_and(|expires_at| expires_at <= now())
        {
            return Err("共享令牌已过期".to_string());
        }
        let token_models = token_grants(conn, org_id, token_id)?
            .into_iter()
            .filter(|grant| grant.source_enabled)
            .map(|grant| grant.exposed_model)
            .collect::<Vec<_>>();
        if token_models.is_empty() {
            return Err("令牌当前没有可用的共享模型".to_string());
        }
        let models = if input.models.is_empty() {
            token_models
        } else {
            validate_model_scope(&token_models, &input.models, "路由令牌")?
        };
        (Some(token), None, models)
    } else {
        let source_id = input
            .source_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Provider 配置分发必须选择共享源".to_string())?;
        let source = get_source_record(conn, org_id, source_id)?;
        if !source.enabled {
            return Err("共享源未启用".to_string());
        }
        let models = if input.models.is_empty() {
            source.models.clone()
        } else {
            validate_models(&source, &input.models)?
        };
        if models.is_empty() {
            return Err("共享源尚未选择模型，无法分发 Provider 配置".to_string());
        }
        (None, Some(source), models)
    };
    let expires_at = input.expires_at;
    if expires_at.is_some_and(|value| value <= now()) {
        return Err("分发有效期必须晚于当前时间".to_string());
    }
    let batch_id = crate::utils::new_id();
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启分发事务失败：{}", e))?;
    let mut ids = Vec::new();
    for recipient in unique {
        let id = crate::utils::new_id();
        tx.execute(
            "INSERT INTO api_share_distributions
                (id, org_id, kind, source_id, token_id, batch_id, created_by,
                 recipient_user_id, models, message_id, sent_at, opened_at, applied_at,
                 revoked_at, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, NULL, NULL,
                     NULL, ?10, ?11)",
            params![
                id,
                org_id,
                kind,
                source.as_ref().map(|value| value.id.as_str()),
                token.as_ref().map(|value| value.id.as_str()),
                batch_id,
                actor,
                recipient,
                models_json(&models),
                expires_at,
                now()
            ],
        )
        .map_err(|e| format!("创建分发记录失败：{}", e))?;
        if let Some(token) = token.as_ref() {
            ensure_distribution_credential(&tx, org_id, &id, &token.id)?;
        }
        ids.push(id);
    }
    tx.commit()
        .map_err(|e| format!("提交分发记录失败：{}", e))?;
    let items = ids
        .iter()
        .map(|id| get_distribution_view(conn, org_id, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CreatedApiShareDistributions { batch_id, items })
}

pub fn mark_distribution_sent(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    distribution_id: &str,
    message_id: &str,
) -> Result<ApiShareDistributionView, String> {
    let role = authz::role_for_member(conn, org_id, actor)?
        .ok_or_else(|| "当前账号不是该组织成员".to_string())?;
    let distribution = get_distribution_view(conn, org_id, distribution_id)?;
    if distribution.created_by != actor && !can_manage(Some(role)) {
        return Err("只能更新自己创建的分发记录".to_string());
    }
    if message_id.trim().is_empty() {
        return Err("缺少传讯消息标识".to_string());
    }
    conn.execute(
        "UPDATE api_share_distributions
            SET message_id = ?1, sent_at = COALESCE(sent_at, ?2)
          WHERE id = ?3 AND org_id = ?4 AND revoked_at IS NULL",
        params![message_id.trim(), now(), distribution_id, org_id],
    )
    .map_err(|e| format!("更新分发发送状态失败：{}", e))?;
    get_distribution_view(conn, org_id, distribution_id)
}

pub fn revoke_distribution(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    distribution_id: &str,
) -> Result<(), String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可撤销分发".to_string());
    }
    let at = now();
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启分发撤销事务失败：{}", e))?;
    let changed = tx
        .execute(
            "UPDATE api_share_distributions
                SET revoked_at = COALESCE(revoked_at, ?1)
              WHERE id = ?2 AND org_id = ?3",
            params![at, distribution_id, org_id],
        )
        .map_err(|e| format!("撤销分发失败：{}", e))?;
    if changed == 0 {
        return Err("分发记录不存在".to_string());
    }
    tx.execute(
        "UPDATE api_share_distribution_credentials
            SET enabled = 0, revoked_at = COALESCE(revoked_at, ?1)
          WHERE org_id = ?2 AND distribution_id = ?3",
        params![at, org_id, distribution_id],
    )
    .map_err(|e| format!("撤销分发凭据失败：{}", e))?;
    tx.commit()
        .map_err(|e| format!("提交分发撤销失败：{}", e))?;
    Ok(())
}

pub fn archive_distribution(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    distribution_id: &str,
) -> Result<ApiShareDistributionView, String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可归档分发".to_string());
    }
    let changed = conn
        .execute(
            "UPDATE api_share_distributions
                SET archived_at = COALESCE(archived_at, ?1)
              WHERE id = ?2 AND org_id = ?3",
            params![now(), distribution_id, org_id],
        )
        .map_err(|e| format!("归档分发失败：{}", e))?;
    if changed == 0 {
        return Err("分发记录不存在".to_string());
    }
    get_distribution_view(conn, org_id, distribution_id)
}

pub fn delete_distribution(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    distribution_id: &str,
) -> Result<(), String> {
    if !can_manage(authz::role_for_member(conn, org_id, actor)?) {
        return Err("仅组织所有者/管理员可删除分发".to_string());
    }
    let distribution = get_distribution_view(conn, org_id, distribution_id)?;
    if distribution.kind == DISTRIBUTION_KIND_ROUTE_TOKEN {
        let token_id = distribution
            .token_id
            .as_deref()
            .ok_or_else(|| "分发记录缺少路由令牌".to_string())?;
        if token_has_usage(conn, org_id, token_id)? {
            return Err("该令牌已有调用数据，相关分发只能归档，不能删除".to_string());
        }
    }
    let changed = conn
        .execute(
            "DELETE FROM api_share_distributions WHERE id = ?1 AND org_id = ?2",
            params![distribution_id, org_id],
        )
        .map_err(|e| format!("删除分发失败：{}", e))?;
    if changed == 0 {
        return Err("分发记录不存在".to_string());
    }
    Ok(())
}

fn distributed_model_notes(
    main_conn: &Connection,
    provider_id: &str,
    models: &[String],
) -> Result<HashMap<String, String>, String> {
    let (_, entries) =
        crate::commands::api_provider::get_provider_format_and_models(main_conn, provider_id)
            .map_err(|e| format!("读取 Provider 模型备注失败：{}", e))?;
    let distributed_models: HashSet<&str> = models.iter().map(String::as_str).collect();
    Ok(entries
        .into_iter()
        .filter(|(name, _)| distributed_models.contains(name.as_str()))
        .collect())
}

fn route_distributed_model_notes(
    state: &EnclaveState,
    conn: &Connection,
    org_id: &str,
    token_id: &str,
    exposed_models: &[String],
) -> Result<HashMap<String, String>, String> {
    let selected = exposed_models
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let grants = token_grants(conn, org_id, token_id)?;
    let pool = state
        .main_db()
        .ok_or_else(|| "主库连接不可用".to_string())?;
    let main_conn = pool
        .get()
        .map_err(|e| format!("读取 Provider 配置失败：{}", e))?;
    let mut notes = HashMap::new();
    for grant in grants {
        if !selected.contains(grant.exposed_model.as_str()) {
            continue;
        }
        let source = get_source_record(conn, org_id, &grant.source_id)?;
        let (_, entries) = crate::commands::api_provider::get_provider_format_and_models(
            &main_conn,
            &source.provider_id,
        )
        .map_err(|e| format!("读取 Provider 模型备注失败：{}", e))?;
        if let Some((_, note)) = entries
            .into_iter()
            .find(|(name, _)| name == &grant.source_model)
        {
            if !note.trim().is_empty() {
                notes.insert(grant.exposed_model, note);
            }
        }
    }
    Ok(notes)
}

pub fn open_distribution(
    state: &EnclaveState,
    conn: &Connection,
    org_id: &str,
    actor: &str,
    distribution_id: &str,
) -> Result<OpenedApiShareConnection, String> {
    authz::role_for_member(conn, org_id, actor)?
        .ok_or_else(|| "当前账号不是该组织成员".to_string())?;
    let distribution = get_distribution_view(conn, org_id, distribution_id)?;
    if distribution.recipient_user_id != actor {
        return Err("该连接未分发给当前账号".to_string());
    }
    if member_is_blocked(conn, org_id, actor)? {
        return Err("当前成员已被 API 共享黑名单限制，暂时无法领取连接".to_string());
    }
    if distribution.revoked_at.is_some() {
        return Err("该连接分发已撤销".to_string());
    }
    if distribution
        .expires_at
        .is_some_and(|expires_at| expires_at <= now())
    {
        return Err("该连接分发已过期".to_string());
    }
    if !distribution.source_enabled {
        return Err("当前没有可用的共享来源，暂时无法领取连接".to_string());
    }
    let (provider_id, provider_name, base_url, api_format, api_key, model_notes) =
        if distribution.kind == DISTRIBUTION_KIND_ROUTE_TOKEN {
            let token_id = distribution
                .token_id
                .as_deref()
                .ok_or_else(|| "分发记录缺少路由令牌".to_string())?;
            let token = token_record(conn, org_id, token_id)?;
            if member_is_blocked(conn, org_id, &token.owner_user_id)? {
                return Err("路由令牌所有者已被 API 共享黑名单限制".to_string());
            }
            if !token.enabled {
                return Err("路由令牌已撤销".to_string());
            }
            if token
                .expires_at
                .is_some_and(|expires_at| expires_at <= now())
            {
                return Err("路由令牌已过期".to_string());
            }
            let raw = ensure_distribution_credential(conn, org_id, distribution_id, token_id)?;
            let model_notes =
                route_distributed_model_notes(state, conn, org_id, token_id, &distribution.models)?;
            (
                format!("shared_token_{}", token.id),
                token.name,
                gateway_base_url(),
                token.api_format,
                raw,
                model_notes,
            )
        } else {
            let source_id = distribution
                .source_id
                .as_deref()
                .ok_or_else(|| "Provider 配置分发缺少共享源".to_string())?;
            let source = get_source_record(conn, org_id, source_id)?;
            let pool = state
                .main_db()
                .ok_or_else(|| "主库连接不可用".to_string())?;
            let main_conn = pool
                .get()
                .map_err(|e| format!("读取 Provider 配置失败：{}", e))?;
            let provider =
                crate::commands::api_provider::get_api_provider(&main_conn, &source.provider_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "Provider 已删除，请联系共享管理员".to_string())?;
            let key = crate::commands::api_provider::get_api_key(&main_conn, &source.provider_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Provider 尚未配置 API Key".to_string())?;
            let model_notes =
                distributed_model_notes(&main_conn, &source.provider_id, &distribution.models)?;
            (
                format!("shared_source_{}", source.id),
                provider.name,
                provider.api_endpoint,
                provider_api_format(&provider.api_format),
                key,
                model_notes,
            )
        };
    conn.execute(
        "UPDATE api_share_distributions SET opened_at = COALESCE(opened_at, ?1)
          WHERE id = ?2 AND org_id = ?3",
        params![now(), distribution_id, org_id],
    )
    .map_err(|e| format!("更新连接领取状态失败：{}", e))?;
    Ok(OpenedApiShareConnection {
        distribution_id: distribution.id,
        kind: distribution.kind,
        source_id: distribution.source_id,
        source_name: distribution.source_name,
        provider_id,
        provider_name,
        base_url,
        api_format,
        api_key,
        models: distribution.models,
        model_notes,
        expires_at: distribution.expires_at,
    })
}

pub fn mark_distribution_applied(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    distribution_id: &str,
) -> Result<ApiShareDistributionView, String> {
    authz::role_for_member(conn, org_id, actor)?
        .ok_or_else(|| "当前账号不是该组织成员".to_string())?;
    let distribution = get_distribution_view(conn, org_id, distribution_id)?;
    if distribution.recipient_user_id != actor {
        return Err("只能确认分发给自己的连接".to_string());
    }
    if member_is_blocked(conn, org_id, actor)? {
        return Err("当前成员已被 API 共享黑名单限制".to_string());
    }
    if distribution.revoked_at.is_some() {
        return Err("该连接分发已撤销".to_string());
    }
    if distribution.opened_at.is_none() {
        return Err("请先领取连接配置".to_string());
    }
    conn.execute(
        "UPDATE api_share_distributions
            SET applied_at = COALESCE(applied_at, ?1)
          WHERE id = ?2 AND org_id = ?3",
        params![now(), distribution_id, org_id],
    )
    .map_err(|e| format!("更新连接导入状态失败：{}", e))?;
    get_distribution_view(conn, org_id, distribution_id)
}

#[derive(Debug, Clone, Default)]
struct UsagePointTotals {
    requests: i64,
    success_requests: i64,
    rate_limited_requests: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
}

fn month_start(month: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(&format!("{}-01", month), "%Y-%m-%d")
        .map_err(|_| format!("用量月份格式不正确：{}", month))
}

fn next_month_start(date: NaiveDate) -> NaiveDate {
    if date.month() == 12 {
        NaiveDate::from_ymd_opt(date.year() + 1, 1, 1).expect("valid next year")
    } else {
        NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1).expect("valid next month")
    }
}

fn usage_point(
    start: NaiveDate,
    end: NaiveDate,
    label: String,
    totals: UsagePointTotals,
) -> ApiShareUsagePoint {
    ApiShareUsagePoint {
        start_day: start.format("%Y-%m-%d").to_string(),
        end_day: end.format("%Y-%m-%d").to_string(),
        label,
        requests: totals.requests,
        success_requests: totals.success_requests,
        rate_limited_requests: totals.rate_limited_requests,
        prompt_tokens: totals.prompt_tokens,
        completion_tokens: totals.completion_tokens,
        total_tokens: totals.total_tokens,
    }
}

pub fn usage(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    source_id: Option<&str>,
    token_id: Option<&str>,
    from_day: Option<&str>,
    to_day: Option<&str>,
) -> Result<ApiShareUsageView, String> {
    if authz::role_for_member(conn, org_id, actor)?.is_none() {
        return Err("当前账号不是该组织成员".to_string());
    }
    let parsed_from = from_day
        .map(|day| {
            NaiveDate::parse_from_str(day, "%Y-%m-%d")
                .map_err(|_| "开始日期格式应为 YYYY-MM-DD".to_string())
        })
        .transpose()?;
    let parsed_to = to_day
        .map(|day| {
            NaiveDate::parse_from_str(day, "%Y-%m-%d")
                .map_err(|_| "结束日期格式应为 YYYY-MM-DD".to_string())
        })
        .transpose()?;
    if matches!((parsed_from, parsed_to), (Some(from), Some(to)) if from > to) {
        return Err("开始日期不能晚于结束日期".to_string());
    }

    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(SUM(requests), 0), COALESCE(SUM(success_requests), 0),
                    COALESCE(SUM(rate_limited_requests), 0),
                    COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0)
               FROM api_share_usage_daily
              WHERE org_id = ?1
                AND (?2 IS NULL OR source_id = ?2)
                AND (?3 IS NULL OR token_id = ?3)
                AND (?4 IS NULL OR day >= ?4)
                AND (?5 IS NULL OR day <= ?5)",
        )
        .map_err(|e| format!("读取共享用量失败：{}", e))?;
    let row = stmt
        .query_row(
            params![org_id, source_id, token_id, from_day, to_day],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .map_err(|e| format!("读取共享用量失败：{}", e))?;

    // 全部时间按自然月聚合；明确指定区间时按 UTC 日补零，保证趋势连续。
    let granularity = if from_day.is_none() && to_day.is_none() {
        "month"
    } else {
        "day"
    };
    let bucket_expr = if granularity == "month" {
        "substr(day, 1, 7)"
    } else {
        "day"
    };
    let trend_sql = format!(
        "SELECT {bucket_expr} AS bucket,
                COALESCE(SUM(requests), 0), COALESCE(SUM(success_requests), 0),
                COALESCE(SUM(rate_limited_requests), 0),
                COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                COALESCE(SUM(total_tokens), 0)
           FROM api_share_usage_daily
          WHERE org_id = ?1
            AND (?2 IS NULL OR source_id = ?2)
            AND (?3 IS NULL OR token_id = ?3)
            AND (?4 IS NULL OR day >= ?4)
            AND (?5 IS NULL OR day <= ?5)
          GROUP BY bucket
          ORDER BY bucket"
    );
    let mut trend_stmt = conn
        .prepare(&trend_sql)
        .map_err(|e| format!("读取共享用量趋势失败：{}", e))?;
    let rows = trend_stmt
        .query_map(
            params![org_id, source_id, token_id, from_day, to_day],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    UsagePointTotals {
                        requests: row.get(1)?,
                        success_requests: row.get(2)?,
                        rate_limited_requests: row.get(3)?,
                        prompt_tokens: row.get(4)?,
                        completion_tokens: row.get(5)?,
                        total_tokens: row.get(6)?,
                    },
                ))
            },
        )
        .map_err(|e| format!("读取共享用量趋势失败：{}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取共享用量趋势失败：{}", e))?;
    let mut buckets: HashMap<String, UsagePointTotals> = rows.into_iter().collect();

    let trend = if granularity == "month" {
        let mut periods = buckets.keys().cloned().collect::<Vec<_>>();
        periods.sort();
        let Some(first_period) = periods.first() else {
            return Ok(ApiShareUsageView {
                source_id: source_id.unwrap_or_default().to_string(),
                granularity: granularity.to_string(),
                requests: row.0,
                success_requests: row.1,
                rate_limited_requests: row.2,
                prompt_tokens: row.3,
                completion_tokens: row.4,
                total_tokens: row.5,
                trend: Vec::new(),
            });
        };
        let last_period = periods.last().expect("non-empty periods");
        let mut cursor = month_start(first_period)?;
        let end = month_start(last_period)?;
        let mut points = Vec::new();
        while cursor <= end {
            let period = cursor.format("%Y-%m").to_string();
            let next = next_month_start(cursor);
            let end_of_month = next.pred_opt().expect("valid previous day");
            points.push(usage_point(
                cursor,
                end_of_month,
                period.clone(),
                buckets.remove(&period).unwrap_or_default(),
            ));
            cursor = next;
        }
        points
    } else {
        let first_available = buckets
            .keys()
            .min()
            .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
        let last_available = buckets
            .keys()
            .max()
            .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
        let start = parsed_from.or(first_available);
        let end = parsed_to.or(last_available);
        match (start, end) {
            (Some(start), Some(end)) => {
                let mut cursor = start;
                let mut points = Vec::new();
                while cursor <= end {
                    let day = cursor.format("%Y-%m-%d").to_string();
                    points.push(usage_point(
                        cursor,
                        cursor,
                        cursor.format("%m-%d").to_string(),
                        buckets.remove(&day).unwrap_or_default(),
                    ));
                    cursor = cursor + Days::new(1);
                }
                points
            }
            _ => Vec::new(),
        }
    };

    Ok(ApiShareUsageView {
        source_id: source_id.unwrap_or_default().to_string(),
        granularity: granularity.to_string(),
        requests: row.0,
        success_requests: row.1,
        rate_limited_requests: row.2,
        prompt_tokens: row.3,
        completion_tokens: row.4,
        total_tokens: row.5,
        trend,
    })
}

pub fn gateway_port() -> u16 {
    std::env::var(API_SHARE_PORT_KEY)
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(API_SHARE_DEFAULT_PORT)
}

pub fn gateway_base_url() -> String {
    let host = host::local_lan_addr();
    let host = if host.is_empty() { "127.0.0.1" } else { &host };
    format!("https://{}:{}/v1", host, gateway_port())
}

pub fn access_info(state: &EnclaveState, org_id: &str) -> ApiShareAccessInfo {
    state
        .store
        .with_conn(|conn| Ok(access_info_with_conn(state, conn, org_id)))
        .unwrap_or_else(|_| access_info_for_org(state, false))
}

pub fn access_info_with_conn(
    state: &EnclaveState,
    conn: &Connection,
    org_id: &str,
) -> ApiShareAccessInfo {
    let is_host_org = super::org::host_org_ids(conn, &host::device_id())
        .unwrap_or_default()
        .iter()
        .any(|id| id == org_id);
    access_info_for_org(state, is_host_org)
}

fn access_info_for_org(state: &EnclaveState, is_host_org: bool) -> ApiShareAccessInfo {
    let enabled = !state.is_disabled() && is_host_org;
    ApiShareAccessInfo {
        gateway_url: gateway_base_url(),
        port: gateway_port(),
        certificate_fingerprint: tls::host_fingerprint().unwrap_or_default(),
        enabled,
        disabled_reason: if state.is_disabled() {
            "对外服务已临时关闭，API 共享网关暂停服务".to_string()
        } else if !is_host_org {
            "本机不是该组织的团队主机".to_string()
        } else {
            String::new()
        },
    }
}

// ── HTTPS 网关 ──────────────────────────────────────────────────

struct HttpRequest {
    method: String,
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Debug, Clone)]
struct SharedTokenRow {
    token_id: String,
    credential_id: Option<String>,
    org_id: String,
    owner_user_id: String,
    principal_user_id: String,
    blocked: bool,
    api_format: String,
    rpm_limit: i64,
    burst_limit: i64,
    max_concurrency: i64,
    daily_request_limit: i64,
    daily_token_limit: i64,
}

#[derive(Debug, Clone)]
struct SharedTokenGrantRow {
    source_id: String,
    source_model: String,
    exposed_model: String,
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    rpm_limit: i64,
    burst_limit: i64,
}

struct GatewayPermit {
    _concurrency: Option<OwnedSemaphorePermit>,
}

struct GatewayLimitError {
    message: String,
    retry_after: i64,
    limit_requests: Option<i64>,
    remaining_requests: Option<i64>,
    limit_tokens: Option<i64>,
    remaining_tokens: Option<i64>,
    already_recorded: bool,
}

enum GatewayAcquireError {
    Limit(GatewayLimitError),
    Internal(String),
}

impl GatewayLimitError {
    fn rpm(retry_after: i64) -> Self {
        Self {
            message: "请求频率超过令牌限额".to_string(),
            retry_after: retry_after.max(1),
            limit_requests: None,
            remaining_requests: None,
            limit_tokens: None,
            remaining_tokens: None,
            already_recorded: false,
        }
    }
}

pub async fn run_gateway_loop(state: Arc<EnclaveState>) {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            log::warn!("[enclave] API 共享网关 HTTP 客户端创建失败：{}", error);
            return;
        }
    };
    let mut listener: Option<(TcpListener, TlsAcceptor)> = None;
    loop {
        let should_run = !state.is_disabled() && !host::host_org_ids(&state).is_empty();
        if !should_run {
            listener = None;
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        if listener.is_none() {
            let port = gateway_port();
            match TcpListener::bind(("0.0.0.0", port)).await {
                Ok(value) => match tls::ensure_host_tls() {
                    Ok(host_tls) => {
                        log::info!("[enclave] API 共享网关已监听 https://0.0.0.0:{}", port);
                        listener = Some((value, host_tls.acceptor));
                    }
                    Err(error) => {
                        log::warn!("[enclave] API 共享网关 TLS 初始化失败：{}", error)
                    }
                },
                Err(error) => {
                    log::warn!("[enclave] API 共享网关监听 {} 失败：{}", port, error)
                }
            }
        }
        let Some((listener_ref, acceptor)) = listener.as_ref() else {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };
        match tokio::time::timeout(Duration::from_secs(2), listener_ref.accept()).await {
            Ok(Ok((stream, peer))) => {
                let acceptor = acceptor.clone();
                let state = state.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    match acceptor.accept(stream).await {
                        Ok(mut tls_stream) => {
                            if let Err(error) =
                                handle_gateway_connection(&mut tls_stream, peer, &state, &client)
                                    .await
                            {
                                log::debug!("[enclave] API 共享请求结束（{}）：{}", peer, error);
                            }
                        }
                        Err(error) => {
                            log::debug!("[enclave] API 共享 TLS 握手失败（{}）：{}", peer, error)
                        }
                    }
                });
            }
            Ok(Err(error)) => {
                log::warn!("[enclave] API 共享网关连接失败：{}", error);
                listener = None;
            }
            Err(_) => {}
        }
    }
}

async fn handle_gateway_connection(
    stream: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    _peer: std::net::SocketAddr,
    state: &EnclaveState,
    client: &reqwest::Client,
) -> Result<(), String> {
    let request = read_http_request(stream).await?;
    if request.path.contains('?') {
        let query = request
            .path
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or("");
        if query.contains("token=") || query.contains("api_key=") || query.contains("key=") {
            return write_json_response(
                stream,
                400,
                &json!({ "error": { "message": "令牌不能通过 URL 查询参数传递" } }),
            )
            .await;
        }
    }
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    if !ALLOWED_ENDPOINTS.contains(&path) {
        return write_json_response(
            stream,
            404,
            &json!({ "error": { "message": "不支持的 API 路径" } }),
        )
        .await;
    }
    if state.is_disabled() {
        return write_json_response(
            stream,
            503,
            &json!({ "error": { "message": "对外服务已临时关闭，API 共享不可用" } }),
        )
        .await;
    }
    let raw_token = match bearer_token(&request.headers) {
        Some(token) => token,
        None => {
            return write_json_response(
                stream,
                401,
                &json!({ "error": { "message": "缺少 Authorization: Bearer 或 x-api-key" } }),
            )
            .await
        }
    };
    let token = match lookup_token(state, &raw_token) {
        Ok(token) => token,
        Err(_) => {
            return write_json_response(
                stream,
                401,
                &json!({ "error": { "message": "共享令牌无效、已撤销或已过期" } }),
            )
            .await
        }
    };
    if token.blocked {
        return write_json_response(
            stream,
            403,
            &json!({ "error": { "message": "该成员已被组织 API 共享黑名单限制" } }),
        )
        .await;
    }
    if !host::host_org_ids(state)
        .iter()
        .any(|org| org == &token.org_id)
    {
        return write_json_response(
            stream,
            503,
            &json!({ "error": { "message": "本机已不是该组织的主机" } }),
        )
        .await;
    }
    let grant = if path == "/v1/models" {
        None
    } else {
        let model = request_model(&request.body);
        if model.is_empty() {
            return write_json_response(
                stream,
                400,
                &json!({ "error": { "message": "请求缺少模型标识" } }),
            )
            .await;
        }
        match match_token_grant(state, &token, &model) {
            Ok(grant) => Some(grant),
            Err(_) => return write_json_response(
                stream,
                403,
                &json!({ "error": { "message": format!("模型 {} 未被该共享令牌授权", model) } }),
            )
            .await,
        }
    };
    let permit = match acquire_gateway_limit(
        state,
        &token,
        path,
        grant.as_ref().map(|value| value.source_id.as_str()),
    ) {
        Ok(permit) => permit,
        Err(GatewayAcquireError::Limit(error)) => {
            if !error.already_recorded {
                if let Some(grant) = grant.as_ref() {
                    record_rate_limited(state, &token, &grant.source_id);
                }
            }
            return write_rate_limit_response(stream, &error).await;
        }
        Err(GatewayAcquireError::Internal(error)) => {
            log::warn!("[enclave] API 共享额度检查失败：{}", error);
            return write_json_response(
                stream,
                500,
                &json!({ "error": { "message": "共享额度检查暂时不可用" } }),
            )
            .await;
        }
    };
    if path == "/v1/models" {
        touch_token(state, &token.token_id);
        let grants = match available_token_grants(state, &token) {
            Ok(grants) => grants,
            Err(error) => {
                log::warn!("[enclave] API 共享模型列表读取失败：{}", error);
                return write_json_response(
                    stream,
                    500,
                    &json!({ "error": { "message": "共享模型列表暂时不可用" } }),
                )
                .await;
            }
        };
        let data = grants
            .iter()
            .map(|grant| {
                json!({
                    "id": grant.exposed_model,
                    "object": "model",
                    "owned_by": "pilotdesk-enclave",
                })
            })
            .collect::<Vec<_>>();
        drop(permit);
        return write_json_response(stream, 200, &json!({ "object": "list", "data": data })).await;
    }
    let grant = grant.expect("non-models requests always resolve a grant");
    touch_token(state, &token.token_id);
    let result = proxy_request(stream, &request, path, &token, &grant, state, client).await;
    drop(permit);
    result
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    if let Some(raw) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    {
        if let Some(token) = raw
            .trim()
            .strip_prefix("Bearer ")
            .or_else(|| raw.trim().strip_prefix("bearer "))
        {
            if !token.trim().is_empty() {
                return Some(token.trim().to_string());
            }
        }
    }
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn request_model(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn shared_token_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SharedTokenRow> {
    Ok(SharedTokenRow {
        credential_id: row.get(0)?,
        token_id: row.get(1)?,
        org_id: row.get(2)?,
        owner_user_id: row.get(3)?,
        principal_user_id: row.get(4)?,
        blocked: row.get::<_, i64>(5)? != 0,
        api_format: row.get(6)?,
        rpm_limit: row.get(7)?,
        burst_limit: row.get(8)?,
        max_concurrency: row.get(9)?,
        daily_request_limit: row.get(10)?,
        daily_token_limit: row.get(11)?,
    })
}

fn lookup_token(state: &EnclaveState, raw: &str) -> Result<SharedTokenRow, String> {
    let at = now();
    state.store.with_conn(|conn| {
        let token_hash = hash_secret(raw);
        let credential = conn
            .query_row(
                "SELECT c.id, t.id, t.org_id, t.owner_user_id,
                        d.recipient_user_id,
                        EXISTS (
                          SELECT 1 FROM api_share_member_blocks b
                           WHERE b.org_id = t.org_id AND b.user_id = d.recipient_user_id
                        ) OR EXISTS (
                          SELECT 1 FROM api_share_member_blocks b
                           WHERE b.org_id = t.org_id AND b.user_id = t.owner_user_id
                        ),
                        t.api_format,
                        t.rpm_limit, t.burst_limit, t.max_concurrency,
                        t.daily_request_limit, t.daily_token_limit
                   FROM api_share_distribution_credentials c
                   JOIN api_share_distributions d
                     ON d.id = c.distribution_id AND d.org_id = c.org_id
                   JOIN api_share_tokens t
                     ON t.id = c.parent_token_id AND t.org_id = c.org_id
                  WHERE c.token_hash = ?1
                    AND c.enabled = 1 AND c.revoked_at IS NULL
                    AND d.revoked_at IS NULL
                    AND (d.expires_at IS NULL OR d.expires_at > ?2)
                    AND t.enabled = 1
                    AND (t.expires_at IS NULL OR t.expires_at > ?2)",
                params![token_hash, at],
                shared_token_from_row,
            )
            .optional()
            .map_err(|e| format!("读取分发凭据失败：{}", e))?;
        if let Some(credential) = credential {
            return Ok(credential);
        }
        conn.query_row(
            "SELECT NULL, t.id, t.org_id, t.owner_user_id, t.owner_user_id,
                    EXISTS (
                      SELECT 1 FROM api_share_member_blocks b
                       WHERE b.org_id = t.org_id AND b.user_id = t.owner_user_id
                    ),
                    t.api_format,
                    t.rpm_limit, t.burst_limit, t.max_concurrency,
                    t.daily_request_limit, t.daily_token_limit
               FROM api_share_tokens t
              WHERE t.token_hash = ?1 AND t.enabled = 1
                AND (t.expires_at IS NULL OR t.expires_at > ?2)",
            params![token_hash, at],
            shared_token_from_row,
        )
        .optional()
        .map_err(|e| format!("读取共享令牌失败：{}", e))?
        .ok_or_else(|| "无效令牌".to_string())
    })
}

fn available_token_grants(
    state: &EnclaveState,
    token: &SharedTokenRow,
) -> Result<Vec<SharedTokenGrantRow>, String> {
    state.store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT g.source_id, g.source_model, g.exposed_model
                   FROM api_share_token_grants g
                   JOIN api_share_sources s
                     ON s.id = g.source_id AND s.org_id = ?2
                  WHERE g.token_id = ?1 AND s.enabled = 1
                  ORDER BY g.exposed_model",
            )
            .map_err(|e| format!("读取共享令牌模型授权失败：{}", e))?;
        let rows = stmt
            .query_map(params![token.token_id, token.org_id], |row| {
                Ok(SharedTokenGrantRow {
                    source_id: row.get(0)?,
                    source_model: row.get(1)?,
                    exposed_model: row.get(2)?,
                })
            })
            .map_err(|e| format!("读取共享令牌模型授权失败：{}", e))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| format!("读取共享令牌模型授权失败：{}", e))
    })
}

fn match_token_grant(
    state: &EnclaveState,
    token: &SharedTokenRow,
    exposed_model: &str,
) -> Result<SharedTokenGrantRow, String> {
    available_token_grants(state, token)?
        .into_iter()
        .find(|grant| grant.exposed_model == exposed_model)
        .ok_or_else(|| "模型未被该共享令牌授权".to_string())
}

fn acquire_gateway_limit(
    state: &EnclaveState,
    token: &SharedTokenRow,
    path: &str,
    source_id: Option<&str>,
) -> Result<GatewayPermit, GatewayAcquireError> {
    if token.rpm_limit > 0 {
        let capacity = if token.burst_limit > 0 {
            token.burst_limit
        } else {
            token.rpm_limit
        };
        let rate_per_second = token.rpm_limit as f64 / 60.0;
        let buckets = TOKEN_BUCKETS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut buckets = buckets.lock().unwrap_or_else(|error| error.into_inner());
        let bucket = buckets
            .entry(token.token_id.clone())
            .or_insert_with(|| TokenBucket {
                tokens: capacity as f64,
                last_refill: Instant::now(),
                rpm_limit: token.rpm_limit,
                burst_limit: token.burst_limit,
            });
        if bucket.rpm_limit != token.rpm_limit || bucket.burst_limit != token.burst_limit {
            bucket.tokens = capacity as f64;
            bucket.last_refill = Instant::now();
            bucket.rpm_limit = token.rpm_limit;
            bucket.burst_limit = token.burst_limit;
        }
        let elapsed = bucket.last_refill.elapsed().as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rate_per_second).min(capacity as f64);
        bucket.last_refill = Instant::now();
        if bucket.tokens < 1.0 {
            let retry_after = ((1.0 - bucket.tokens) / rate_per_second).ceil() as i64;
            return Err(GatewayAcquireError::Limit(GatewayLimitError::rpm(
                retry_after,
            )));
        }
        bucket.tokens -= 1.0;
    }

    let concurrency = if token.max_concurrency > 0 {
        let semaphores = TOKEN_SEMAPHORES.get_or_init(|| Mutex::new(HashMap::new()));
        let semaphore_key = format!("{}:{}", token.token_id, token.max_concurrency);
        let semaphore = {
            let mut map = semaphores.lock().unwrap_or_else(|error| error.into_inner());
            map.entry(semaphore_key)
                .or_insert_with(|| Arc::new(Semaphore::new(token.max_concurrency as usize)))
                .clone()
        };
        match semaphore.try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                return Err(GatewayAcquireError::Limit(GatewayLimitError {
                    message: "共享令牌并发请求数已达上限".to_string(),
                    retry_after: 1,
                    limit_requests: None,
                    remaining_requests: None,
                    limit_tokens: None,
                    remaining_tokens: None,
                    already_recorded: false,
                }))
            }
        }
    } else {
        None
    };

    // 非模型列表请求统一登记一次，既用于日额度检查，也作为用量统计的请求基数。
    if path != "/v1/models" {
        let source_id = source_id
            .ok_or_else(|| GatewayAcquireError::Internal("缺少请求命中的共享源".to_string()))?;
        reserve_daily_request(state, token, source_id)?;
    }
    Ok(GatewayPermit {
        _concurrency: concurrency,
    })
}

fn reserve_daily_request(
    state: &EnclaveState,
    token: &SharedTokenRow,
    source_id: &str,
) -> Result<(), GatewayAcquireError> {
    let outcome = state
        .store
        .with_conn(|conn| {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("开始日额度检查失败：{}", e))?;
            let (requests, total_tokens) = tx
                .query_row(
                    "SELECT COALESCE(SUM(requests), 0), COALESCE(SUM(total_tokens), 0)
                       FROM api_share_usage_daily
                      WHERE org_id = ?1 AND token_id = ?2 AND day = ?3",
                    params![token.org_id, token.token_id, today()],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(|e| format!("读取日额度失败：{}", e))?;
            let rate_limited = |message: String,
                                retry_after: i64,
                                limit_requests,
                                remaining_requests,
                                limit_tokens,
                                remaining_tokens| {
                GatewayLimitError {
                    message,
                    retry_after,
                    limit_requests,
                    remaining_requests,
                    limit_tokens,
                    remaining_tokens,
                    already_recorded: true,
                }
            };
            if token.daily_request_limit > 0 && requests >= token.daily_request_limit {
                let error = rate_limited(
                    "共享令牌今日请求额度已用完".to_string(),
                    next_utc_midnight().saturating_sub(now()).max(1),
                    Some(token.daily_request_limit),
                    Some(0),
                    (token.daily_token_limit > 0).then_some(token.daily_token_limit),
                    (token.daily_token_limit > 0)
                        .then_some((token.daily_token_limit - total_tokens).max(0)),
                );
                increment_rate_limited_conn(&tx, token, source_id)?;
                tx.commit()
                    .map_err(|e| format!("提交限流记录失败：{}", e))?;
                return Ok(Err(error));
            }
            if token.daily_token_limit > 0 && total_tokens >= token.daily_token_limit {
                let error = rate_limited(
                    "共享令牌今日 Token 额度已用完".to_string(),
                    next_utc_midnight().saturating_sub(now()).max(1),
                    (token.daily_request_limit > 0).then_some(token.daily_request_limit),
                    (token.daily_request_limit > 0)
                        .then_some((token.daily_request_limit - requests).max(0)),
                    Some(token.daily_token_limit),
                    Some(0),
                );
                increment_rate_limited_conn(&tx, token, source_id)?;
                tx.commit()
                    .map_err(|e| format!("提交限流记录失败：{}", e))?;
                return Ok(Err(error));
            }
            tx.execute(
                "INSERT INTO api_share_usage_daily
                (org_id, source_id, token_id, day, requests, success_requests,
                 rate_limited_requests, prompt_tokens, completion_tokens, total_tokens,
                 updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, 0, 0, 0, 0, 0, ?5)
             ON CONFLICT(org_id, source_id, token_id, day) DO UPDATE SET
                 requests = requests + 1,
                 updated_at = excluded.updated_at",
                params![token.org_id, source_id, token.token_id, today(), now()],
            )
            .map_err(|e| format!("登记日请求额度失败：{}", e))?;
            tx.commit()
                .map_err(|e| format!("提交日请求额度失败：{}", e))?;
            Ok(Ok(()))
        })
        .map_err(GatewayAcquireError::Internal)?;
    outcome.map_err(GatewayAcquireError::Limit)
}

fn mark_first_use(conn: &Connection, token_id: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE api_share_tokens
            SET first_used_at = COALESCE(first_used_at, ?1),
                last_used_at = ?1,
                request_count = request_count + 1
          WHERE id = ?2",
        params![now(), token_id],
    )
    .map_err(|e| format!("更新令牌使用状态失败：{}", e))?;
    Ok(())
}

fn touch_token(state: &EnclaveState, token_id: &str) {
    let _ = state.store.with_conn(|conn| mark_first_use(conn, token_id));
}

fn increment_rate_limited_conn(
    conn: &Connection,
    token: &SharedTokenRow,
    source_id: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO api_share_usage_daily
            (org_id, source_id, token_id, day, requests, success_requests,
             rate_limited_requests, prompt_tokens, completion_tokens, total_tokens,
             updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, 0, 1, 0, 0, 0, ?5)
         ON CONFLICT(org_id, source_id, token_id, day) DO UPDATE SET
             rate_limited_requests = rate_limited_requests + 1,
             updated_at = excluded.updated_at",
        params![token.org_id, source_id, token.token_id, today(), now()],
    )
    .map_err(|e| format!("记录限流请求失败：{}", e))?;
    Ok(())
}

fn record_rate_limited(state: &EnclaveState, token: &SharedTokenRow, source_id: &str) {
    let _ = state
        .store
        .with_conn(|conn| increment_rate_limited_conn(conn, token, source_id));
}

async fn proxy_request(
    stream: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    request: &HttpRequest,
    path: &str,
    token: &SharedTokenRow,
    grant: &SharedTokenGrantRow,
    state: &EnclaveState,
    client: &reqwest::Client,
) -> Result<(), String> {
    let source = state
        .store
        .with_conn(|conn| get_source_record(conn, &token.org_id, &grant.source_id))?;
    let pool = state
        .main_db()
        .ok_or_else(|| "主库连接不可用".to_string())?;
    let (base_url, api_key, api_format) = {
        let conn = pool
            .get()
            .map_err(|e| format!("读取 Provider 配置失败：{}", e))?;
        let provider = crate::commands::api_provider::get_api_provider(&conn, &source.provider_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "共享 Provider 已删除".to_string())?;
        let key = crate::commands::api_provider::get_api_key(&conn, &source.provider_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "共享 Provider 尚未配置 API Key".to_string())?;
        (
            provider.api_endpoint,
            key,
            provider_api_format(&provider.api_format),
        )
    };
    let mut upstream_body: Value =
        serde_json::from_slice(&request.body).map_err(|_| "请求体不是有效 JSON".to_string())?;
    let object = upstream_body
        .as_object_mut()
        .ok_or_else(|| "请求体必须是 JSON 对象".to_string())?;
    object.insert(
        "model".to_string(),
        Value::String(grant.source_model.clone()),
    );
    let upstream_body =
        serde_json::to_vec(&upstream_body).map_err(|e| format!("重写共享模型标识失败：{}", e))?;
    let target = target_url(&base_url, path);
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|e| format!("无效请求方法：{}", e))?;
    let mut headers = HeaderMap::new();
    for name in [
        "content-type",
        "accept",
        "anthropic-version",
        "anthropic-beta",
        "openai-beta",
        "user-agent",
    ] {
        if let Some(value) = request.headers.get(name) {
            if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) {
                headers.insert(header_name, value.clone());
            }
        }
    }
    if api_format.eq_ignore_ascii_case("anthropic") {
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key).map_err(|e| format!("API Key 无效：{}", e))?,
        );
        if !headers.contains_key("anthropic-version") {
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        }
    } else {
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", api_key))
                .map_err(|e| format!("API Key 无效：{}", e))?,
        );
    }
    let started = Instant::now();
    let response = client
        .request(method, &target)
        .headers(headers)
        .body(upstream_body)
        .send()
        .await
        .map_err(|e| format!("转发 Provider 请求失败：{}", e))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nCache-Control: no-store\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        content_type
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|e| format!("写入共享响应失败：{}", e))?;
    let mut body_stream = response.bytes_stream();
    let mut captured = Vec::new();
    while let Some(chunk) = body_stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取 Provider 响应失败：{}", e))?;
        if captured.len() < MAX_USAGE_CAPTURE_BYTES {
            let left = MAX_USAGE_CAPTURE_BYTES - captured.len();
            captured.extend_from_slice(&chunk[..chunk.len().min(left)]);
        }
        let frame = format!("{:x}\r\n", chunk.len());
        stream
            .write_all(frame.as_bytes())
            .await
            .map_err(|e| format!("写入共享响应失败：{}", e))?;
        stream
            .write_all(&chunk)
            .await
            .map_err(|e| format!("写入共享响应失败：{}", e))?;
        stream
            .write_all(b"\r\n")
            .await
            .map_err(|e| format!("写入共享响应失败：{}", e))?;
    }
    stream
        .write_all(b"0\r\n\r\n")
        .await
        .map_err(|e| format!("写入共享响应失败：{}", e))?;
    let (prompt, completion, total) = extract_usage(&captured);
    let success = i64::from(status.is_success());
    let _ = record_upstream_usage(
        state,
        token,
        &grant.source_id,
        success,
        prompt,
        completion,
        total,
    );
    let _ = started;
    Ok(())
}

fn record_upstream_usage(
    state: &EnclaveState,
    token: &SharedTokenRow,
    source_id: &str,
    success: i64,
    prompt: i64,
    completion: i64,
    total: i64,
) -> Result<(), String> {
    state.store.with_conn(|conn| {
        conn.execute(
            "INSERT INTO api_share_usage_daily
                (org_id, source_id, token_id, day, requests, success_requests,
                 rate_limited_requests, prompt_tokens, completion_tokens, total_tokens,
                 updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, 0, ?6, ?7, ?8, ?9)
             ON CONFLICT(org_id, source_id, token_id, day) DO UPDATE SET
                 success_requests = success_requests + excluded.success_requests,
                 prompt_tokens = prompt_tokens + excluded.prompt_tokens,
                 completion_tokens = completion_tokens + excluded.completion_tokens,
                 total_tokens = total_tokens + excluded.total_tokens,
                 updated_at = excluded.updated_at",
            params![
                token.org_id,
                source_id,
                token.token_id,
                today(),
                success,
                prompt,
                completion,
                total,
                now()
            ],
        )
        .map_err(|e| format!("记录共享用量失败：{}", e))?;
        Ok(())
    })
}

fn target_url(base: &str, path: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let suffix = if base.ends_with("/v1") && path.starts_with("/v1/") {
        &path[3..]
    } else {
        path
    };
    format!("{}{}", base, suffix)
}

fn extract_usage(bytes: &[u8]) -> (i64, i64, i64) {
    fn from_value(value: &Value) -> Option<(i64, i64, i64)> {
        let usage = value.get("usage")?;
        let prompt = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let completion = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let total = usage
            .get("total_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(prompt + completion);
        Some((prompt, completion, total))
    }
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        if let Some(usage) = from_value(&value) {
            return usage;
        }
    }
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines().rev() {
        let payload = line.trim().strip_prefix("data:").unwrap_or(line.trim());
        if payload == "[DONE]" || payload.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            if let Some(usage) = from_value(&value) {
                return usage;
            }
        }
    }
    (0, 0, 0)
}

async fn read_http_request(
    stream: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> Result<HttpRequest, String> {
    let mut buffer = Vec::with_capacity(8192);
    let mut temp = [0u8; 8192];
    let header_end = loop {
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        if buffer.len() >= MAX_HTTP_HEADER_BYTES {
            return Err("HTTP 请求头过大".to_string());
        }
        let count = stream
            .read(&mut temp)
            .await
            .map_err(|e| format!("读取 HTTP 请求失败：{}", e))?;
        if count == 0 {
            return Err("连接在请求头完成前关闭".to_string());
        }
        buffer.extend_from_slice(&temp[..count]);
    };
    let header_text = std::str::from_utf8(&buffer[..header_end])
        .map_err(|_| "HTTP 请求头不是有效 UTF-8".to_string())?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().ok_or_else(|| "缺少 HTTP 请求行".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
        return Err("HTTP 请求行格式不正确".to_string());
    }
    let mut headers = HeaderMap::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            let value = value.trim();
            if let (Ok(header_name), Ok(header_value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse::<usize>().unwrap_or(0);
                }
                headers.insert(header_name, header_value);
            }
        }
    }
    if content_length > MAX_HTTP_BODY_BYTES {
        return Err("HTTP 请求体超过 32 MB 上限".to_string());
    }
    let body_start = header_end + 4;
    let mut body = buffer[body_start..].to_vec();
    while body.len() < content_length {
        let count = stream
            .read(&mut temp)
            .await
            .map_err(|e| format!("读取 HTTP 请求体失败：{}", e))?;
        if count == 0 {
            return Err("连接在请求体完成前关闭".to_string());
        }
        body.extend_from_slice(&temp[..count]);
    }
    body.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

async fn write_rate_limit_response(
    stream: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    error: &GatewayLimitError,
) -> Result<(), String> {
    let mut headers = vec![
        ("Retry-After".to_string(), error.retry_after.to_string()),
        (
            "X-RateLimit-Reset".to_string(),
            next_utc_midnight().to_string(),
        ),
    ];
    if let Some(value) = error.limit_requests {
        headers.push(("X-RateLimit-Limit-Requests".to_string(), value.to_string()));
    }
    if let Some(value) = error.remaining_requests {
        headers.push((
            "X-RateLimit-Remaining-Requests".to_string(),
            value.to_string(),
        ));
    }
    if let Some(value) = error.limit_tokens {
        headers.push(("X-RateLimit-Limit-Tokens".to_string(), value.to_string()));
    }
    if let Some(value) = error.remaining_tokens {
        headers.push((
            "X-RateLimit-Remaining-Tokens".to_string(),
            value.to_string(),
        ));
    }
    write_json_response_with_headers(
        stream,
        429,
        &json!({ "error": { "message": error.message } }),
        &headers,
    )
    .await
}

async fn write_json_response(
    stream: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    status: u16,
    body: &Value,
) -> Result<(), String> {
    write_json_response_with_headers(stream, status, body, &[]).await
}

async fn write_json_response_with_headers(
    stream: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    status: u16,
    body: &Value,
    extra_headers: &[(String, String)],
) -> Result<(), String> {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let extra = extra_headers
        .iter()
        .map(|(name, value)| format!("{}: {}\r\n", name, value))
        .collect::<String>();
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{}Connection: close\r\n\r\n",
        status,
        reason,
        bytes.len(),
        extra
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|e| format!("写入 HTTP 响应失败：{}", e))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|e| format!("写入 HTTP 响应失败：{}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_db(dir: &std::path::Path) -> crate::DbPool {
        let manager = r2d2_sqlite::SqliteConnectionManager::file(dir.join("pilotdesk.db"));
        let pool = r2d2::Pool::builder().max_size(1).build(manager).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE agents (
                agent_type TEXT PRIMARY KEY,
                display_name TEXT NOT NULL DEFAULT '',
                transport TEXT NOT NULL DEFAULT '',
                conn TEXT NOT NULL DEFAULT '',
                sort_order INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                secret_ref TEXT
             );
             CREATE TABLE agent_secrets (
                id TEXT PRIMARY KEY,
                secret TEXT NOT NULL DEFAULT ''
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agents
                (agent_type, display_name, transport, conn, sort_order, created_at, updated_at)
             VALUES ('api:p1', 'Provider', 'http', ?1, 0, 1, 1)",
            params![
                r#"{"baseUrl":"https://api.example.com/v1","apiFormat":"openai","models":[{"name":"m1","note":"适合圆桌自动选择"},{"name":"m2","note":"不属于本次分发"}]}"#
            ],
        )
        .unwrap();
        pool
    }

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "PRAGMA foreign_keys=ON;\n{}",
            crate::enclave::db::ENCLAVE_SCHEMA_SQL
        ))
        .unwrap();
        conn.execute(
            "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id)
             VALUES ('o1', 'Team', 'owner', 'owner', 'owner')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO member_snapshot
                (org_id, user_id, role, display_name, email, seat_status, synced_at)
             VALUES ('o1', 'owner', 'owner', 'Owner', '', 'active', 1),
                    ('o1', 'admin', 'admin', 'Admin', '', 'active', 1),
                    ('o1', 'member', 'member', 'Member', '', 'active', 1)",
            [],
        )
        .unwrap();
        conn
    }

    fn source(conn: &Connection) -> ApiShareSourceView {
        source_with(conn, "p1", "共享模型", vec!["m1".to_string()])
    }

    fn source_with(
        conn: &Connection,
        provider_id: &str,
        name: &str,
        models: Vec<String>,
    ) -> ApiShareSourceView {
        create_source(
            conn,
            "o1",
            "owner",
            &CreateApiShareSourceInput {
                provider_id: provider_id.to_string(),
                name: name.to_string(),
                models,
                enabled: true,
            },
        )
        .unwrap()
    }

    fn token_input(source_id: String, owner: &str) -> CreateApiShareTokenInput {
        CreateApiShareTokenInput {
            grants: vec![ApiShareTokenGrantInput {
                source_id,
                models: vec!["m1".to_string()],
            }],
            owner_user_id: owner.to_string(),
            name: "test".to_string(),
            expires_at: None,
            rpm_limit: 60,
            burst_limit: 10,
            max_concurrency: 2,
            daily_request_limit: 100,
            daily_token_limit: 10_000,
        }
    }

    fn main_db_with_providers(providers: &[(&str, &str)]) -> crate::DbPool {
        let manager = r2d2_sqlite::SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(manager).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE agents (
                agent_type TEXT PRIMARY KEY,
                display_name TEXT NOT NULL DEFAULT '',
                transport TEXT NOT NULL DEFAULT '',
                conn TEXT NOT NULL DEFAULT '',
                sort_order INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                secret_ref TEXT
             );
             CREATE TABLE agent_secrets (
                id TEXT PRIMARY KEY,
                secret TEXT NOT NULL DEFAULT ''
             );",
        )
        .unwrap();
        for (provider_id, api_format) in providers {
            let conn_json = json!({
                "baseUrl": "https://api.example.com/v1",
                "apiFormat": api_format,
                "models": [
                    {"name": "m1", "note": "适合圆桌自动选择"},
                    {"name": "m2", "note": "不属于本次分发"}
                ]
            });
            conn.execute(
                "INSERT INTO agents
                    (agent_type, display_name, transport, conn, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, 'http', ?3, 0, 1, 1)",
                params![
                    format!("api:{provider_id}"),
                    format!("Provider {provider_id}"),
                    conn_json.to_string()
                ],
            )
            .unwrap();
        }
        pool
    }

    fn test_state_with_providers(providers: &[(&str, &str)]) -> EnclaveState {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-api-share-test-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store =
            Arc::new(crate::enclave::db::EnclaveStore::open(&dir.join("enclave.db")).unwrap());
        let state = EnclaveState::new(store);
        state.attach_main_db(main_db_with_providers(providers));
        state
    }

    fn test_state() -> EnclaveState {
        test_state_with_providers(&[("p1", "openai")])
    }

    fn create_test_token(
        conn: &Connection,
        actor: &str,
        input: &CreateApiShareTokenInput,
    ) -> Result<IssuedApiShareToken, String> {
        let state = test_state();
        create_token(&state, conn, "o1", actor, input)
    }

    #[test]
    fn token_secret_is_encrypted_and_recoverable() {
        let conn = conn();
        let source = source(&conn);
        let issued = create_test_token(&conn, "owner", &token_input(source.id, "owner")).unwrap();
        let (hash, ciphertext): (String, String) = conn
            .query_row(
                "SELECT token_hash, secret_ciphertext FROM api_share_tokens WHERE id = ?1",
                params![issued.view.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(hash, hash_secret(&issued.token));
        assert_ne!(ciphertext, issued.token);
        let recovered = token_secret(&conn, "o1", "owner", &issued.view.id).unwrap();
        assert_eq!(recovered.token, issued.token);
        let copied_at: Option<i64> = conn
            .query_row(
                "SELECT copied_at FROM api_share_tokens WHERE id = ?1",
                params![issued.view.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(copied_at.is_some());
    }

    #[test]
    fn source_requires_a_stable_model_scope() {
        let conn = conn();
        let error = create_source(
            &conn,
            "o1",
            "owner",
            &CreateApiShareSourceInput {
                provider_id: "provider".to_string(),
                name: "Empty".to_string(),
                models: Vec::new(),
                enabled: true,
            },
        )
        .unwrap_err();
        assert!(error.contains("至少需要选择一种模型"));
    }

    #[test]
    fn members_cannot_create_tokens() {
        let conn = conn();
        let source = source(&conn);
        let error =
            create_test_token(&conn, "member", &token_input(source.id, "member")).unwrap_err();
        assert!(error.contains("仅组织所有者/管理员"));
    }

    #[test]
    fn members_cannot_manage_issued_tokens() {
        let conn = conn();
        let source = source(&conn);
        let issued = create_test_token(&conn, "owner", &token_input(source.id, "member")).unwrap();
        let token_id = issued.view.id;
        let update = UpdateApiShareTokenInput {
            name: Some("member rename".to_string()),
            grants: None,
            expires_at: None,
            clear_expiry: false,
            rpm_limit: None,
            burst_limit: None,
            max_concurrency: None,
            daily_request_limit: None,
            daily_token_limit: None,
        };

        for error in [
            list_tokens(&conn, "o1", "member").unwrap_err(),
            token_secret(&conn, "o1", "member", &token_id).unwrap_err(),
            update_token(&test_state(), &conn, "o1", "member", &token_id, &update).unwrap_err(),
            revoke_token(&conn, "o1", "member", &token_id).unwrap_err(),
            archive_token(&conn, "o1", "member", &token_id).unwrap_err(),
            delete_token(&conn, "o1", "member", &token_id).unwrap_err(),
        ] {
            assert!(error.contains("仅组织所有者/管理员"));
        }
    }

    #[test]
    fn token_view_reflects_source_pause() {
        let conn = conn();
        let source = source(&conn);
        create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();
        update_source(
            &conn,
            "o1",
            "owner",
            &source.id,
            &UpdateApiShareSourceInput {
                name: None,
                models: None,
                enabled: Some(false),
            },
        )
        .unwrap();

        let tokens = list_tokens(&conn, "o1", "owner").unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].enabled);
        assert!(!tokens[0].sources[0].source_enabled);
    }

    #[test]
    fn limits_are_scoped_to_each_token() {
        let conn = conn();
        let source = source(&conn);
        let first =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "member")).unwrap();
        let mut input = token_input(source.id, "admin");
        input.name = "second".to_string();
        input.rpm_limit = 5;
        input.burst_limit = 5;
        input.daily_request_limit = 7;
        let second = create_test_token(&conn, "owner", &input).unwrap();
        assert_ne!(first.view.id, second.view.id);
        assert_eq!(first.view.rpm_limit, 60);
        assert_eq!(second.view.rpm_limit, 5);
        assert_eq!(second.view.daily_request_limit, 7);
    }

    #[test]
    fn token_can_combine_sources_and_route_distribution_uses_token_identity() {
        let conn = conn();
        let primary = source_with(&conn, "p1", "Primary", vec!["m1".to_string()]);
        let secondary = source_with(&conn, "p1", "Secondary", vec!["m2".to_string()]);
        let mut input = token_input(primary.id.clone(), "member");
        input.name = "Cross-source token".to_string();
        input.grants = vec![
            ApiShareTokenGrantInput {
                source_id: primary.id.clone(),
                models: vec!["m1".to_string()],
            },
            ApiShareTokenGrantInput {
                source_id: secondary.id.clone(),
                models: vec!["m2".to_string()],
            },
        ];
        let issued = create_test_token(&conn, "owner", &input).unwrap();

        assert_eq!(issued.view.sources.len(), 2);
        assert_eq!(
            issued.view.allowed_models,
            vec!["[Primary]m1".to_string(), "[Secondary]m2".to_string()]
        );
        for invalid_model in ["m1", "[Unknown]m2"] {
            let error = create_distributions(
                &conn,
                "o1",
                "owner",
                &CreateApiShareDistributionInput {
                    kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                    source_id: None,
                    token_id: Some(issued.view.id.clone()),
                    recipient_user_ids: vec!["admin".to_string()],
                    models: vec![invalid_model.to_string()],
                    expires_at: Some(now() + 3_600),
                },
            )
            .unwrap_err();
            assert!(error.contains("不在路由令牌允许列表中"));
        }

        let created = create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id.clone()),
                recipient_user_ids: vec!["admin".to_string()],
                models: issued.view.allowed_models.clone(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap();
        let distribution = &created.items[0];
        assert_eq!(distribution.source_id, None);
        assert_eq!(distribution.source_name, input.name);
        assert_eq!(
            distribution.token_id.as_deref(),
            Some(issued.view.id.as_str())
        );
        assert_eq!(distribution.models, issued.view.allowed_models);
    }

    #[test]
    fn deleting_one_source_keeps_cross_source_token_and_other_grants() {
        let conn = conn();
        let primary = source_with(&conn, "p1", "Primary", vec!["m1".to_string()]);
        let secondary = source_with(&conn, "p1", "Secondary", vec!["m2".to_string()]);
        let mut input = token_input(primary.id.clone(), "member");
        input.name = "Surviving token".to_string();
        input.grants = vec![
            ApiShareTokenGrantInput {
                source_id: primary.id.clone(),
                models: vec!["m1".to_string()],
            },
            ApiShareTokenGrantInput {
                source_id: secondary.id.clone(),
                models: vec!["m2".to_string()],
            },
        ];
        let issued = create_test_token(&conn, "owner", &input).unwrap();

        delete_source(&conn, "o1", "owner", &primary.id).unwrap();

        let token = get_token_view(&conn, "o1", &issued.view.id).unwrap();
        assert_eq!(token.sources.len(), 1);
        assert_eq!(token.sources[0].source_id, secondary.id);
        assert_eq!(token.allowed_models, vec!["[Secondary]m2".to_string()]);
        let grant_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_share_token_grants WHERE token_id = ?1",
                params![issued.view.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(grant_count, 1);
    }

    #[test]
    fn daily_request_limit_sums_all_sources_for_one_token() {
        let state = test_state();
        state
            .store
            .with_conn(|conn| {
                for (source_id, requests, total_tokens) in
                    [("s1", 1_i64, 10_i64), ("s2", 1_i64, 20_i64)]
                {
                    conn.execute(
                        "INSERT INTO api_share_usage_daily
                            (org_id, source_id, token_id, day, requests, success_requests,
                             rate_limited_requests, prompt_tokens, completion_tokens,
                             total_tokens, updated_at)
                         VALUES ('o1', ?1, 't1', ?2, ?3, ?3, 0, ?4, 0, ?5, 1)",
                        params![source_id, today(), requests, total_tokens, total_tokens],
                    )
                    .map_err(|e| format!("写入跨来源测试用量失败：{}", e))?;
                }
                Ok(())
            })
            .unwrap();
        let token = SharedTokenRow {
            token_id: "t1".to_string(),
            credential_id: None,
            org_id: "o1".to_string(),
            owner_user_id: "owner".to_string(),
            principal_user_id: "owner".to_string(),
            blocked: false,
            api_format: "openai".to_string(),
            rpm_limit: 0,
            burst_limit: 0,
            max_concurrency: 0,
            daily_request_limit: 2,
            daily_token_limit: 0,
        };

        let error = match reserve_daily_request(&state, &token, "s3") {
            Err(GatewayAcquireError::Limit(error)) => error,
            _ => panic!("跨来源请求数应合并计算并触发日额度"),
        };
        assert!(error.message.contains("今日请求额度已用完"));
    }

    #[test]
    fn token_rejects_sources_with_different_api_formats() {
        let state = test_state_with_providers(&[("p1", "openai"), ("p2", "anthropic")]);
        let conn = conn();
        let openai = source_with(&conn, "p1", "OpenAI source", vec!["m1".to_string()]);
        let anthropic = source_with(&conn, "p2", "Anthropic source", vec!["m2".to_string()]);
        let mut input = token_input(openai.id.clone(), "member");
        input.grants = vec![
            ApiShareTokenGrantInput {
                source_id: openai.id,
                models: vec!["m1".to_string()],
            },
            ApiShareTokenGrantInput {
                source_id: anthropic.id,
                models: vec!["m2".to_string()],
            },
        ];

        let error = create_token(&state, &conn, "o1", "owner", &input).unwrap_err();
        assert!(error.contains("相同 API Format"));
    }

    #[test]
    fn usage_filters_by_token_and_utc_day_range() {
        let conn = conn();
        let source = source(&conn);
        for (token_id, day, requests) in [
            ("t1", "2026-11-10", 2),
            ("t1", "2026-11-11", 3),
            ("t2", "2026-11-11", 4),
        ] {
            conn.execute(
                "INSERT INTO api_share_usage_daily
                    (org_id, source_id, token_id, day, requests, success_requests,
                     rate_limited_requests, prompt_tokens, completion_tokens, total_tokens, updated_at)
                 VALUES ('o1', ?1, ?2, ?3, ?4, ?4, 0, ?4, ?4, ?5, 1)",
                params![source.id, token_id, day, requests, requests * 2],
            )
            .unwrap();
        }

        let filtered = usage(
            &conn,
            "o1",
            "owner",
            Some(&source.id),
            Some("t1"),
            Some("2026-11-10"),
            Some("2026-11-11"),
        )
        .unwrap();
        assert_eq!(filtered.requests, 5);
        assert_eq!(filtered.success_requests, 5);
        assert_eq!(filtered.total_tokens, 10);
        assert_eq!(filtered.granularity, "day");
        assert_eq!(filtered.trend.len(), 2);
        assert_eq!(filtered.trend[0].label, "11-10");
        assert_eq!(filtered.trend[0].requests, 2);
        assert_eq!(filtered.trend[1].requests, 3);

        let all = usage(&conn, "o1", "owner", None, None, None, None).unwrap();
        assert_eq!(all.requests, 9);
        assert_eq!(all.granularity, "month");
        assert_eq!(all.trend.len(), 1);
        assert_eq!(all.trend[0].label, "2026-11");
        assert!(usage(
            &conn,
            "o1",
            "owner",
            None,
            None,
            Some("2026-11-12"),
            Some("2026-11-11"),
        )
        .is_err());
    }

    #[test]
    fn usage_all_time_groups_by_month_and_fills_missing_months() {
        let conn = conn();
        let source = source(&conn);
        for (day, requests, success_requests, prompt_tokens, completion_tokens) in
            [("2026-10-31", 2, 1, 10, 5), ("2026-12-01", 3, 3, 30, 15)]
        {
            conn.execute(
                "INSERT INTO api_share_usage_daily
                    (org_id, source_id, token_id, day, requests, success_requests,
                     rate_limited_requests, prompt_tokens, completion_tokens, total_tokens, updated_at)
                 VALUES ('o1', ?1, 't1', ?2, ?3, ?4, 0, ?5, ?6, ?7, 1)",
                params![
                    source.id,
                    day,
                    requests,
                    success_requests,
                    prompt_tokens,
                    completion_tokens,
                    prompt_tokens + completion_tokens
                ],
            )
            .unwrap();
        }

        let all = usage(&conn, "o1", "owner", None, None, None, None).unwrap();
        assert_eq!(all.granularity, "month");
        assert_eq!(all.requests, 5);
        assert_eq!(all.trend.len(), 3);
        assert_eq!(
            all.trend
                .iter()
                .map(|point| point.label.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-10", "2026-11", "2026-12"]
        );
        assert_eq!(all.trend[0].requests, 2);
        assert_eq!(all.trend[0].start_day, "2026-10-01");
        assert_eq!(all.trend[0].end_day, "2026-10-31");
        assert_eq!(all.trend[1].requests, 0);
        assert_eq!(all.trend[1].start_day, "2026-11-01");
        assert_eq!(all.trend[1].end_day, "2026-11-30");
        assert_eq!(all.trend[2].success_requests, 3);
        assert_eq!(all.trend[2].total_tokens, 45);
    }

    #[test]
    fn gateway_usage_counts_requests_when_daily_limits_are_disabled() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-api-share-usage-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store =
            Arc::new(crate::enclave::db::EnclaveStore::open(&dir.join("enclave.db")).unwrap());
        let state = EnclaveState::new(store.clone());
        let token = SharedTokenRow {
            token_id: "t1".to_string(),
            credential_id: None,
            org_id: "o1".to_string(),
            owner_user_id: "owner".to_string(),
            principal_user_id: "owner".to_string(),
            blocked: false,
            api_format: "openai".to_string(),
            rpm_limit: 0,
            burst_limit: 0,
            max_concurrency: 0,
            daily_request_limit: 0,
            daily_token_limit: 0,
        };

        assert!(matches!(
            reserve_daily_request(&state, &token, "s1"),
            Ok(())
        ));
        record_upstream_usage(&state, &token, "s1", 1, 10, 20, 30).unwrap();
        let (requests, success_requests, total_tokens) = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT requests, success_requests, total_tokens
                       FROM api_share_usage_daily
                      WHERE org_id = ?1 AND source_id = ?2 AND token_id = ?3 AND day = ?4",
                    params!["o1", "s1", "t1", today()],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .map_err(|e| format!("读取测试用量失败：{}", e))
            })
            .unwrap();
        assert_eq!((requests, success_requests, total_tokens), (1, 1, 30));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_source_models_prunes_token_grants() {
        let conn = conn();
        let source = source(&conn);
        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();
        update_source(
            &conn,
            "o1",
            "owner",
            &source.id,
            &UpdateApiShareSourceInput {
                name: None,
                models: Some(vec!["m2".to_string()]),
                enabled: None,
            },
        )
        .unwrap();
        let token = get_token_view(&conn, "o1", &issued.view.id).unwrap();
        assert!(token.sources.is_empty());
        let error = create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id),
                recipient_user_ids: vec!["member".to_string()],
                models: Vec::new(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap_err();
        assert!(error.contains("没有可用"));
    }

    #[test]
    fn member_blacklist_is_independent_of_distribution_records() {
        let conn = conn();
        let source = source(&conn);
        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "member")).unwrap();
        let created = create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id.clone()),
                recipient_user_ids: vec!["admin".to_string()],
                models: Vec::new(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap();

        block_member(&conn, "o1", "owner", "member", "限制令牌所有者").unwrap();
        let token = get_token_view(&conn, "o1", &issued.view.id).unwrap();
        assert!(token.owner_blocked);
        let distribution = get_distribution_view(&conn, "o1", &created.items[0].id).unwrap();
        assert!(!distribution.recipient_blocked);
        assert!(distribution.token_owner_blocked);
        let block_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_share_member_blocks
                  WHERE org_id = 'o1' AND user_id = 'member'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(block_count, 1);

        unblock_member(&conn, "o1", "owner", "member").unwrap();
        assert!(
            !get_token_view(&conn, "o1", &issued.view.id)
                .unwrap()
                .owner_blocked
        );
        assert!(
            !get_distribution_view(&conn, "o1", &created.items[0].id)
                .unwrap()
                .token_owner_blocked
        );

        block_member(&conn, "o1", "owner", "admin", "限制凭据接收成员").unwrap();
        assert!(
            get_distribution_view(&conn, "o1", &created.items[0].id)
                .unwrap()
                .recipient_blocked
        );
        assert!(
            !get_token_view(&conn, "o1", &issued.view.id)
                .unwrap()
                .owner_blocked
        );
    }

    #[test]
    fn blocked_member_cannot_receive_tokens_or_distributions() {
        let conn = conn();
        let source = source(&conn);
        block_member(&conn, "o1", "owner", "member", "").unwrap();

        let member_error =
            create_test_token(&conn, "member", &token_input(source.id.clone(), "member"))
                .unwrap_err();
        assert!(member_error.contains("仅组织所有者/管理员"));

        let token_error =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "member"))
                .unwrap_err();
        assert!(token_error.contains("黑名单"));

        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();
        let distribution_error = create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id),
                recipient_user_ids: vec!["member".to_string()],
                models: Vec::new(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap_err();
        assert!(distribution_error.contains("黑名单"));
    }

    #[test]
    fn revoking_route_distribution_disables_only_its_credential() {
        let conn = conn();
        let source = source(&conn);
        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();
        let created = create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id.clone()),
                recipient_user_ids: vec!["member".to_string()],
                models: Vec::new(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap();
        let distribution_id = &created.items[0].id;
        let (credential_id, raw): (String, String) = conn
            .query_row(
                "SELECT id, secret_ciphertext FROM api_share_distribution_credentials
                  WHERE org_id = 'o1' AND distribution_id = ?1",
                params![distribution_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let raw = crate::utils::crypto::decrypt(&raw).unwrap();

        revoke_distribution(&conn, "o1", "owner", distribution_id).unwrap();
        let (enabled, revoked_at): (i64, Option<i64>) = conn
            .query_row(
                "SELECT enabled, revoked_at FROM api_share_distribution_credentials
                  WHERE id = ?1",
                params![credential_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(enabled, 0);
        assert!(revoked_at.is_some());
        let parent: (i64, Option<i64>) = conn
            .query_row(
                "SELECT enabled, revoked_at FROM api_share_tokens WHERE id = ?1",
                params![issued.view.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(parent, (1, None));
        assert!(raw.starts_with("pdsk_"));
        assert_ne!(raw, issued.token);
    }

    #[test]
    fn route_distribution_opening_returns_gateway_token_without_provider_key() {
        let conn = conn();
        let source = source(&conn);
        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();
        let created = create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id),
                recipient_user_ids: vec!["member".to_string()],
                models: Vec::new(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap();
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-api-share-route-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store =
            Arc::new(crate::enclave::db::EnclaveStore::open(&dir.join("enclave.db")).unwrap());
        let state = EnclaveState::new(store.clone());
        state.attach_main_db(main_db(&dir));
        let distribution_id = created.items[0].id.clone();
        let second_distribution_id = format!("{distribution_id}-admin");
        let (opened, second_credential) = store
            .with_conn(|target| {
                target
                    .execute(
                        "INSERT INTO org_snapshot
                        (org_id, name, owner_user_id, my_role, viewer_id)
                     VALUES ('o1', 'Team', 'owner', 'owner', 'owner')",
                        [],
                    )
                    .map_err(|e| format!("写入测试组织失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO member_snapshot
                        (org_id, user_id, role, display_name, email, seat_status, synced_at)
                     VALUES ('o1', 'owner', 'owner', 'Owner', '', 'active', 1)",
                        [],
                    )
                    .map_err(|e| format!("写入测试成员失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO member_snapshot
                        (org_id, user_id, role, display_name, email, seat_status, synced_at)
                     VALUES ('o1', 'member', 'member', 'Member', '', 'active', 1)",
                        [],
                    )
                    .map_err(|e| format!("写入测试成员失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO member_snapshot
                        (org_id, user_id, role, display_name, email, seat_status, synced_at)
                     VALUES ('o1', 'admin', 'admin', 'Admin', '', 'active', 1)",
                        [],
                    )
                    .map_err(|e| format!("写入测试成员失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO member_snapshot
                        (org_id, user_id, role, display_name, email, seat_status, synced_at)
                     VALUES ('o1', 'tokenowner', 'member', 'Token Owner', '', 'active', 1)",
                        [],
                    )
                    .map_err(|e| format!("写入测试成员失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO api_share_sources
                        (id, org_id, provider_id, name, models,
                         enabled, created_by, created_at, updated_at)
                     VALUES ('s1', 'o1', 'p1', 'Shared', '[\"m1\"]', 1, 'owner', 1, 1)",
                        [],
                    )
                    .map_err(|e| format!("写入测试共享源失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO api_share_tokens
                        (id, org_id, owner_user_id, created_by, name, name_key,
                         api_format, token_prefix, token_hash, secret_ciphertext,
                         enabled, rpm_limit, burst_limit, max_concurrency,
                         daily_request_limit, daily_token_limit, created_at)
                     VALUES ('t1', 'o1', 'tokenowner', 'owner', 'Token', 'token',
                             'openai', 'pdsk_test', ?1, ?2, 1, 0, 0, 0, 0, 0, 1)",
                        params![
                            hash_secret("pdsk_secret"),
                            crate::utils::crypto::encrypt("pdsk_secret").unwrap()
                        ],
                    )
                    .map_err(|e| format!("写入测试令牌失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO api_share_token_grants
                        (token_id, source_id, source_model, exposed_model, created_at)
                     VALUES ('t1', 's1', 'm1', '[Shared]m1', 1)",
                        [],
                    )
                    .map_err(|e| format!("写入测试令牌模型授权失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO api_share_distributions
                        (id, org_id, kind, source_id, token_id, batch_id, created_by,
                         recipient_user_id, models, created_at)
                     VALUES (?1, 'o1', 'route_token', NULL, 't1', 'b1', 'owner',
                             'member', '[\"[Shared]m1\"]', 1)",
                        params![&distribution_id],
                    )
                    .map_err(|e| format!("写入测试分发失败：{}", e))?;
                target
                    .execute(
                        "INSERT INTO api_share_distributions
                        (id, org_id, kind, source_id, token_id, batch_id, created_by,
                         recipient_user_id, models, created_at)
                     VALUES (?1, 'o1', 'route_token', NULL, 't1', 'b1', 'owner',
                             'admin', '[\"[Shared]m1\"]', 1)",
                        params![&second_distribution_id],
                    )
                    .map_err(|e| format!("写入测试分发失败：{}", e))?;
                let second_credential =
                    ensure_distribution_credential(target, "o1", &second_distribution_id, "t1")?;
                let opened = open_distribution(&state, target, "o1", "member", &distribution_id)?;
                Ok((opened, second_credential))
            })
            .unwrap();
        assert_eq!(opened.kind, DISTRIBUTION_KIND_ROUTE_TOKEN);
        assert!(opened.api_key.starts_with("pdsk_"));
        assert_ne!(opened.api_key, "pdsk_secret");
        assert_eq!(opened.api_format, "openai");
        assert!(opened.base_url.starts_with("https://"));
        assert_eq!(
            opened.model_notes.get("[Shared]m1").map(String::as_str),
            Some("适合圆桌自动选择")
        );
        assert!(
            !opened.model_notes.contains_key("m2"),
            "只应返回本次分发范围内的模型备注"
        );
        assert_eq!(opened.provider_id, "shared_token_t1");
        assert_eq!(opened.provider_name, "Token");
        let credential_hash: String = store
            .with_conn(|target| {
                target
                    .query_row(
                        "SELECT token_hash FROM api_share_distribution_credentials
                          WHERE org_id = 'o1' AND distribution_id = ?1",
                        params![distribution_id],
                        |row| row.get(0),
                    )
                    .map_err(|e| format!("读取测试分发凭据失败：{}", e))
            })
            .unwrap();
        assert_eq!(credential_hash, hash_secret(&opened.api_key));
        let credential_identity = lookup_token(&state, &opened.api_key).unwrap();
        assert!(credential_identity.credential_id.is_some());
        assert_eq!(credential_identity.principal_user_id, "member");
        assert!(!credential_identity.blocked);
        let second_identity = lookup_token(&state, &second_credential).unwrap();
        assert_eq!(second_identity.principal_user_id, "admin");
        assert!(!second_identity.blocked);
        let parent_identity = lookup_token(&state, "pdsk_secret").unwrap();
        assert!(parent_identity.credential_id.is_none());
        assert_eq!(parent_identity.principal_user_id, "tokenowner");
        assert!(!parent_identity.blocked);

        store
            .with_conn(|target| {
                block_member(target, "o1", "owner", "member", "测试成员级黑名单")?;
                Ok(())
            })
            .unwrap();
        assert!(lookup_token(&state, &opened.api_key).unwrap().blocked);
        assert!(!lookup_token(&state, &second_credential).unwrap().blocked);
        assert!(!lookup_token(&state, "pdsk_secret").unwrap().blocked);

        store
            .with_conn(|target| unblock_member(target, "o1", "owner", "member"))
            .unwrap();
        store
            .with_conn(|target| {
                block_member(target, "o1", "owner", "tokenowner", "测试令牌所有者黑名单")
            })
            .unwrap();
        assert!(lookup_token(&state, &opened.api_key).unwrap().blocked);
        assert!(lookup_token(&state, &second_credential).unwrap().blocked);
        assert!(lookup_token(&state, "pdsk_secret").unwrap().blocked);

        store
            .with_conn(|target| unblock_member(target, "o1", "owner", "tokenowner"))
            .unwrap();
        assert!(!lookup_token(&state, &opened.api_key).unwrap().blocked);
        assert!(!lookup_token(&state, &second_credential).unwrap().blocked);
        assert!(!lookup_token(&state, "pdsk_secret").unwrap().blocked);

        store
            .with_conn(|target| revoke_token(target, "o1", "owner", "t1"))
            .unwrap();
        assert!(lookup_token(&state, &opened.api_key).is_err());
        assert!(lookup_token(&state, &second_credential).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_deletion_requires_revocation() {
        let conn = conn();
        let source = source(&conn);
        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();

        let error = delete_token(&conn, "o1", "owner", &issued.view.id).unwrap_err();
        assert!(error.contains("先撤销"));

        revoke_token(&conn, "o1", "owner", &issued.view.id).unwrap();
        delete_token(&conn, "o1", "owner", &issued.view.id).unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_share_tokens WHERE id = ?1",
                params![issued.view.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn deleting_revoked_unused_token_cascades_route_distributions() {
        let conn = conn();
        let source = source(&conn);
        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();
        create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id.clone()),
                recipient_user_ids: vec!["member".to_string()],
                models: Vec::new(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap();

        revoke_token(&conn, "o1", "owner", &issued.view.id).unwrap();
        delete_token(&conn, "o1", "owner", &issued.view.id).unwrap();

        let distributions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_share_distributions WHERE token_id = ?1",
                params![issued.view.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(distributions, 0);
    }

    #[test]
    fn used_token_can_only_be_archived_and_archives_its_distributions() {
        let conn = conn();
        let source = source(&conn);
        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();
        create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id.clone()),
                recipient_user_ids: vec!["member".to_string()],
                models: Vec::new(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_share_usage_daily
                (org_id, source_id, token_id, day, requests, success_requests,
                 rate_limited_requests, prompt_tokens, completion_tokens, total_tokens, updated_at)
             VALUES ('o1', ?1, ?2, ?3, 1, 1, 0, 1, 1, 2, 1)",
            params![&source.id, &issued.view.id, today()],
        )
        .unwrap();

        revoke_token(&conn, "o1", "owner", &issued.view.id).unwrap();
        let error = delete_token(&conn, "o1", "owner", &issued.view.id).unwrap_err();
        assert!(error.contains("只能归档"));

        let archived = archive_token(&conn, "o1", "owner", &issued.view.id).unwrap();
        assert!(archived.archived_at.is_some());
        assert!(archived.has_usage);
        let distribution_archived_at: Option<i64> = conn
            .query_row(
                "SELECT archived_at FROM api_share_distributions WHERE token_id = ?1",
                params![issued.view.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(distribution_archived_at.is_some());
    }

    #[test]
    fn used_route_distribution_can_only_be_archived() {
        let conn = conn();
        let source = source(&conn);
        let issued =
            create_test_token(&conn, "owner", &token_input(source.id.clone(), "owner")).unwrap();
        let created = create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_ROUTE_TOKEN.to_string(),
                source_id: None,
                token_id: Some(issued.view.id.clone()),
                recipient_user_ids: vec!["member".to_string()],
                models: Vec::new(),
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_share_usage_daily
                (org_id, source_id, token_id, day, requests, success_requests,
                 rate_limited_requests, prompt_tokens, completion_tokens, total_tokens, updated_at)
             VALUES ('o1', ?1, ?2, ?3, 1, 1, 0, 1, 1, 2, 1)",
            params![&source.id, &issued.view.id, today()],
        )
        .unwrap();
        let distribution_id = &created.items[0].id;

        let error = delete_distribution(&conn, "o1", "owner", distribution_id).unwrap_err();
        assert!(error.contains("只能归档"));

        let archived = archive_distribution(&conn, "o1", "owner", distribution_id).unwrap();
        assert!(archived.archived_at.is_some());
    }

    #[test]
    fn provider_distribution_can_be_deleted_without_usage_check() {
        let conn = conn();
        let source = source(&conn);
        let created = create_distributions(
            &conn,
            "o1",
            "owner",
            &CreateApiShareDistributionInput {
                kind: DISTRIBUTION_KIND_PROVIDER_CONFIG.to_string(),
                source_id: Some(source.id),
                token_id: None,
                recipient_user_ids: vec!["member".to_string()],
                models: vec!["m1".to_string()],
                expires_at: Some(now() + 3_600),
            },
        )
        .unwrap();
        let distribution_id = &created.items[0].id;

        delete_distribution(&conn, "o1", "owner", distribution_id).unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_share_distributions WHERE id = ?1",
                params![distribution_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn target_url_avoids_duplicate_v1() {
        assert_eq!(
            target_url("https://api.example.com/v1", "/v1/chat/completions"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            target_url("https://api.example.com", "/v1/messages"),
            "https://api.example.com/v1/messages"
        );
    }
}
