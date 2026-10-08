//! 组织共享凭据（团队版）：从组织拉取共享的 provider 凭据（API Key），写入本地 API Provider 配置。
//!
//! 平台侧接口（全部 `requireAuth`，产品端用 Bearer）：
//!   GET  /api/v1/organizations/:id/credentials        掩码列表（owner/admin + 能力门控；绝不含明文，不写审计）
//!   GET  /api/v1/organizations/:id/credentials/plain  明文（在用成员 + 能力门控 + 服务端写审计；产品端消费）
//!   POST /api/v1/organizations/:id/credentials/report 上报本地凭据（owner/admin；含密钥 + 模型清单 + 备注）
//!
//! 安全边界：明文密钥只在本模块内部使用（比对 → 写入本地加密配置 / 上报平台），
//! **绝不回传前端、绝不写日志**。
//!
//! 「上报 → 别人应用」的映射约定（保证往返一致，round-trip）：
//!   label    = 本地 provider 名  → 应用方新建后本地名一致
//!   provider = 本地 api_format（协议族）→ 应用方据此确定接口格式 + 默认基址
//!   models   = 本地模型清单 [{name, note}]
//! 该约定必须与 [`org_apply_credential`] 里的匹配逻辑（`find_local_provider` + `provider_defaults`）保持一致。
//!
//! 令牌复用 [`current_access_token`]（读 `account_tokens` + 自动续期）；未登录时返回明确中文提示。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

use crate::commands::api_provider::{
    get_api_key, get_provider_format_and_models, list_api_providers, set_model_notes,
    upsert_api_provider, ApiProvider, CreateOrUpdateProvider,
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

/// 应用结果：写入的本地 provider 名称 + 是否新建 + 应用后的模型数量（供前端提示）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCredentialResult {
    pub provider_name: String,
    pub created: bool,
    /// 应用后本地 provider 的模型数量（供前端提示「密钥 + N 个模型」）
    pub models_applied: usize,
}

/// 上报入参：本地 Provider 名（按 name 精确匹配）+ 目标组织。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportCredentialInput {
    pub org_id: i64,
    pub provider_name: String,
}

/// 上报结果：是否在组织端新建（true=新凭据 / false=已更新）+ 上报的模型数量。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportCredentialResult {
    pub created: bool,
    pub provider_name: String,
    pub models_count: usize,
}

/// 上报请求体里的单条模型（`{ name, note }`）。
#[derive(Serialize)]
struct ReportModelEntry {
    name: String,
    note: String,
}

/// 上报请求体（字段名与平台契约一致：`apiKey`）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportCredentialBody {
    provider: String,
    label: String,
    api_key: String,
    models: Vec<ReportModelEntry>,
}

/// 上报响应：只关心 `created`（`credential` 由平台返回，产品端不需要）。
#[derive(Deserialize)]
struct ReportResp {
    #[serde(default)]
    created: bool,
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
    /// 平台侧模型清单（对象数组 `[{name, note}]`）；
    /// 用 `Value` 承载以便对缺失 / 非数组 / 元素不合法做容错（不因模型字段导致整条凭据解析失败）
    #[serde(default)]
    models: serde_json::Value,
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

/// 上报接口的错误映射：沿用 [`map_platform_error`] 的中文风格，
/// 但 403 额外区分「组织档位不支持」与「权限不足（非 owner/admin）」——
/// 平台若带具体文案则优先展示（如「仅组织管理员可操作」），否则回落到通用提示。
fn map_report_error(err: platform::PlatformError) -> String {
    match err.status {
        403 => {
            let detail = err
                .body
                .as_ref()
                .and_then(|b| b.get("error"))
                .and_then(|e| e.as_str())
                .unwrap_or("");
            if detail.trim().is_empty() {
                "无权上报组织凭据（需组织 owner/admin，且组织档位支持团队版）".to_string()
            } else {
                detail.to_string()
            }
        }
        401 => "请先登录平台账号".to_string(),
        _ => err.message,
    }
}

/// 解析平台返回的模型清单（对象数组 `[{name, note}]`）。
/// **容错**：缺失 / 非数组 / 元素缺 name / name 为空 → 忽略，绝不报错；
/// 结果按平台顺序返回 `(模型名, 备注)`，备注缺省为空串。
fn parse_org_models(v: &serde_json::Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|x| {
                    let name = x.get("name").and_then(|n| n.as_str())?.trim().to_string();
                    if name.is_empty() {
                        return None;
                    }
                    let note = x
                        .get("note")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some((name, note))
                })
                .collect()
        })
        .unwrap_or_default()
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
/// 1. Bearer 调 **plain** 接口取明文与模型清单，按 `provider + label` 匹配（找不到 → 友好报错）；
/// 2. 写入本地配置：本地已存在同名 provider 时，`overwrite = false` 返回明确冲突错误
///    （由前端二次确认后带 `overwrite = true` 重试），`true` 则覆盖其 **密钥、模型清单与备注**
///    （base_url / api_format 仅在为空时才补默认值）；本地不存在则按 provider 默认新建。
#[tauri::command]
pub async fn org_apply_credential(
    input: ApplyCredentialInput,
    state: State<'_, crate::DbState>,
) -> Result<ApplyCredentialResult, String> {
    let token = require_token(state.inner()).await?;

    // 1. 取明文与模型清单，并按 provider + label 精确匹配（明文只在本函数内使用）
    let path = format!("/api/v1/organizations/{}/credentials/plain", input.org_id);
    let resp: PlainCredentialsResp = platform::get_json_with_status(&path, Some(&token))
        .await
        .map_err(map_platform_error)?;
    let wanted_provider = input.provider.trim();
    let wanted_label = input.label.trim();
    let matched = resp
        .items
        .into_iter()
        .find(|c| {
            c.provider.trim() == wanted_provider
                && (wanted_label.is_empty() || c.label.trim() == wanted_label)
        })
        .ok_or_else(|| "组织中没有该凭据".to_string())?;
    if matched.secret.trim().is_empty() {
        return Err("组织中的该凭据密钥为空".to_string());
    }
    let secret = matched.secret;
    // 组织端模型清单（缺失 / 非数组 / 元素不合法 → 空数组，绝不报错）
    let org_models = parse_org_models(&matched.models);

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
                    "本地已存在同名 Provider「{}」，继续将覆盖其密钥、模型清单与备注，请确认覆盖",
                    p.name
                ));
            }
            // 覆盖：更新 api_key + 模型清单；base_url / api_format 为空时才补默认值
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
            // 模型清单：组织端非空则整体替换本地；为空则**保留本地**（避免把本地配置清空）
            let models = if org_models.is_empty() {
                p.models.clone()
            } else {
                org_models.iter().map(|(n, _)| n.clone()).collect()
            };
            (
                p.id.clone(),
                p.name.clone(),
                api_endpoint,
                api_format,
                models,
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
                org_models.iter().map(|(n, _)| n.clone()).collect(),
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

    // 组织端提供了模型清单：把其**备注**一并写入本地（upsert 只保留本地旧备注，这里用组织备注覆盖/补齐）
    if !org_models.is_empty() {
        let mut notes: HashMap<String, HashMap<String, String>> = HashMap::new();
        notes.insert(saved.id.clone(), org_models.iter().cloned().collect());
        set_model_notes(&conn, &notes).map_err(|e| e.to_string())?;
    }

    Ok(ApplyCredentialResult {
        provider_name: saved.name,
        created,
        models_applied: saved.models.len(),
    })
}

/// 把本地某个 API Provider 的凭据（密钥 + 模型清单 + 备注）上报到组织的共享凭据。
///
/// 与 [`org_apply_credential`] 的匹配约定保持一致（保证「上报 → 别人应用」往返一致）：
///   `label`    = 本地 provider 名（应用方新建后本地名一致）；
///   `provider` = 本地 `api_format`（协议族，应用方据此确定接口格式 + 默认基址）；
///   `models`   = 本地模型清单 `[{name, note}]`（note 允许空串）。
/// 返回 `{ created }`：true = 组织端新建凭据 / false = 已更新既有凭据。
#[tauri::command]
pub async fn org_report_credential(
    input: ReportCredentialInput,
    state: State<'_, crate::DbState>,
) -> Result<ReportCredentialResult, String> {
    let token = require_token(state.inner()).await?;

    // 1. 按 name 精确匹配本地 provider（找不到 → 友好中文错误）
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败：{}", e))?;
    let provider_name = input.provider_name.trim().to_string();
    if provider_name.is_empty() {
        return Err("请选择要上报的 Provider".to_string());
    }
    let local = list_api_providers(&conn).map_err(|e| e.to_string())?;
    let provider = local
        .into_iter()
        .find(|p| p.name == provider_name)
        .ok_or_else(|| format!("本地未找到名为「{}」的 Provider", provider_name))?;

    // 2. 取本地明文密钥（为空 → 明确报错）
    let secret = get_api_key(&conn, &provider.id)
        .map_err(|e| e.to_string())?
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| "该 Provider 未配置 API Key，无法上报".to_string())?;

    // 3. 组装上报内容：协议族 + 本地名 + 模型清单（含备注）
    let (api_format, model_entries) =
        get_provider_format_and_models(&conn, &provider.id).map_err(|e| e.to_string())?;
    let models: Vec<ReportModelEntry> = model_entries
        .into_iter()
        .map(|(name, note)| ReportModelEntry { name, note })
        .collect();
    let models_count = models.len();
    let body = ReportCredentialBody {
        provider: api_format,
        label: provider.name.clone(),
        api_key: secret,
        models,
    };

    // 4. 上报平台（明文仅在被序列化进请求体时使用，全程不落日志、不回传前端）
    let path = format!("/api/v1/organizations/{}/credentials/report", input.org_id);
    let resp: ReportResp = platform::post_json_with_status(&path, &body, Some(&token))
        .await
        .map_err(map_report_error)?;

    Ok(ReportCredentialResult {
        created: resp.created,
        provider_name: provider.name,
        models_count,
    })
}
