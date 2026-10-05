//! 组织共享凭据（团队版）：从组织拉取共享的 provider 凭据（API Key），写入本地 API Provider 配置。
//!
//! 平台侧接口（全部 `requireAuth`，产品端用 Bearer）：
//!   GET  /api/v1/organizations/:id/credentials        掩码列表（owner/admin + 能力门控；绝不含明文，不写审计）
//!   GET  /api/v1/organizations/:id/credentials/plain  明文（在用成员 + 能力门控 + 服务端写审计；产品端消费）
//!
//! 安全边界：明文密钥只在本模块内部使用（比对 → 写入本地加密配置），
//! **绝不回传前端、绝不写日志**。
//!
//! 令牌复用 [`current_access_token`]（读 `account_tokens` + 自动续期）；未登录时返回明确中文提示。

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::api_provider::{
    list_api_providers, upsert_api_provider, ApiProvider, CreateOrUpdateProvider,
};
use crate::commands::org_share::require_token;
use crate::utils::platform;

/// 组织共享凭据的展示视图（掩码，**不含明文**）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgCredentialView {
    pub id: i64,
    pub provider: String,
    pub label: String,
    /// 掩码展示串，如 `sk-••••klmn`
    pub hint: String,
    pub creator_label: String,
    pub updated_at: String,
}

/// 应用凭据入参：`overwrite` 决定本地已存在同名 provider 时的语义（前端负责二次确认）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCredentialInput {
    pub org_id: i64,
    pub provider: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub overwrite: bool,
}

/// 应用结果：写入的本地 provider 名称 + 是否新建（供前端提示）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCredentialResult {
    pub provider_name: String,
    pub created: bool,
}

// ── 平台响应结构 ────────────────────────────────────────────────

#[derive(Deserialize)]
struct CredentialsResp {
    items: Vec<CredentialRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialRow {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    creator_label: String,
    #[serde(default)]
    updated_at: String,
}

#[derive(Deserialize)]
struct PlainCredentialsResp {
    items: Vec<PlainCredentialRow>,
}

#[derive(Deserialize)]
struct PlainCredentialRow {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    label: String,
    /// 明文密钥；只在本模块内部使用
    #[serde(default)]
    secret: String,
}

// ── 辅助 ────────────────────────────────────────────────────────

/// 平台错误 → 面向用户的中文文案（对凭据接口的 403 做友好映射）。
fn map_platform_error(err: platform::PlatformError) -> String {
    match err.status {
        // 403：组织档位不含 team.provider-credentials（free / pro 组织）
        403 => "当前组织档位不支持共享凭据（需要团队版）".to_string(),
        // 401：令牌失效 / 未登录
        401 => "请先登录平台账号".to_string(),
        // 其余（含 status=0 的网络 / 解析错误）直接透传（正文已带中文前缀）
        _ => err.message,
    }
}

/// 按 provider 给出本地 API Provider 的合理默认（接口格式 + 基址）。
/// 未知 / `openai-compatible`：openai 格式、基址留空，由用户补全。
fn provider_defaults(provider: &str) -> (&'static str, &'static str) {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => ("openai", "https://api.openai.com/v1"),
        "anthropic" | "claude" => ("anthropic", "https://api.anthropic.com"),
        "deepseek" => ("openai", "https://api.deepseek.com"),
        _ => ("openai", ""),
    }
}

/// 在本地 provider 列表中查找与凭据同名的 provider：
/// 先按凭据 provider 与本地 id / name 比对，再退回按凭据 label 比对，命中即返回。
fn find_local_provider(
    list: &[ApiProvider],
    provider: &str,
    label: &str,
) -> Option<ApiProvider> {
    let provider = provider.trim();
    let label = label.trim();
    // 先按 provider 名称精确匹配，再退回按 label 匹配（新建时名称取自 label，保证重复导入可识别）
    if let Some(p) = list
        .iter()
        .find(|p| !provider.is_empty() && (p.id == provider || p.name == provider))
    {
        return Some(p.clone());
    }
    list.iter()
        .find(|p| !label.is_empty() && (p.id == label || p.name == label))
        .cloned()
}

// ── 命令 ────────────────────────────────────────────────────────

/// 某组织的共享凭据**掩码**列表（供 UI 展示，不触发明文审计）。
#[tauri::command]
pub async fn org_list_credential_providers(
    org_id: i64,
    state: State<'_, crate::DbState>,
) -> Result<Vec<OrgCredentialView>, String> {
    let token = require_token(state.inner()).await?;
    let path = format!("/api/v1/organizations/{}/credentials", org_id);
    let resp: CredentialsResp = platform::get_json_with_status(&path, Some(&token))
        .await
        .map_err(map_platform_error)?;
    Ok(resp
        .items
        .into_iter()
        .map(|r| OrgCredentialView {
            id: r.id,
            provider: r.provider,
            label: r.label,
            hint: r.hint,
            creator_label: r.creator_label,
            updated_at: r.updated_at,
        })
        .collect())
}

/// 从组织应用一条共享凭据到本地 API Provider 配置。
///
/// 1. Bearer 调 **plain** 接口取明文，按 `provider + label` 匹配（找不到 → 友好报错）；
/// 2. 写入本地配置：本地已存在同名 provider 时，`overwrite = false` 返回明确冲突错误
///    （由前端二次确认后带 `overwrite = true` 重试），`true` 则覆盖其 api_key
///    （base_url / api_format 仅在为空时才补默认值）；本地不存在则按 provider 默认新建。
#[tauri::command]
pub async fn org_apply_credential(
    input: ApplyCredentialInput,
    state: State<'_, crate::DbState>,
) -> Result<ApplyCredentialResult, String> {
    let token = require_token(state.inner()).await?;

    // 1. 取明文并按 provider + label 精确匹配（明文只在本函数内使用）
    let path = format!("/api/v1/organizations/{}/credentials/plain", input.org_id);
    let resp: PlainCredentialsResp = platform::get_json_with_status(&path, Some(&token))
        .await
        .map_err(map_platform_error)?;
    let wanted_provider = input.provider.trim();
    let wanted_label = input.label.trim();
    let secret = resp
        .items
        .into_iter()
        .find(|c| {
            c.provider.trim() == wanted_provider
                && (wanted_label.is_empty() || c.label.trim() == wanted_label)
        })
        .map(|c| c.secret)
        .ok_or_else(|| "组织中没有该凭据".to_string())?;
    if secret.trim().is_empty() {
        return Err("组织中的该凭据密钥为空".to_string());
    }

    // 2. 写入本地 API Provider 配置
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败：{}", e))?;
    let local = list_api_providers(&conn).map_err(|e| e.to_string())?;
    let existing = find_local_provider(&local, wanted_provider, wanted_label);
    let created = existing.is_none();

    let (id, name, api_endpoint, api_format, models, sort_order) = match existing {
        Some(p) => {
            if !input.overwrite {
                return Err(format!(
                    "本地已存在同名 Provider「{}」，请确认覆盖",
                    p.name
                ));
            }
            // 覆盖：只更新 api_key；base_url / api_format 为空时才补默认值
            let (def_format, def_endpoint) = provider_defaults(wanted_provider);
            let api_endpoint = if p.api_endpoint.trim().is_empty() {
                def_endpoint.to_string()
            } else {
                p.api_endpoint.clone()
            };
            let api_format = if p.api_format.trim().is_empty() {
                def_format.to_string()
            } else {
                p.api_format.clone()
            };
            (
                p.id.clone(),
                p.name.clone(),
                api_endpoint,
                api_format,
                p.models.clone(),
                Some(p.sort_order),
            )
        }
        None => {
            let (def_format, def_endpoint) = provider_defaults(wanted_provider);
            // 名称优先用凭据 label（更易辨认），缺省用 provider
            let name = if wanted_label.is_empty() {
                wanted_provider.to_string()
            } else {
                wanted_label.to_string()
            };
            (
                format!("custom_{}", crate::utils::now()),
                name,
                def_endpoint.to_string(),
                def_format.to_string(),
                Vec::new(),
                None,
            )
        }
    };

    let payload = CreateOrUpdateProvider {
        id,
        name,
        api_endpoint,
        api_key: Some(secret),
        models,
        api_format: Some(api_format),
        sort_order,
    };
    let saved = upsert_api_provider(&conn, &payload).map_err(|e| e.to_string())?;
    Ok(ApplyCredentialResult {
        provider_name: saved.name,
        created,
    })
}
