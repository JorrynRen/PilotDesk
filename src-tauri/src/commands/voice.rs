//! 语音输入（云端转写）：把输入框录到的音频交给**当前会话所选的提供商/模型**转成文字。
//!
//! 与 `tools::stt` 的分工：那个是 Agent 可自行调用的工具（由模型挑 provider/model）；
//! 这里是用户按麦克风触发的固定路径 —— 用当前会话（或"快捷开始"草稿）已选的 provider/model，
//! 不额外给用户挑模型的入口。两者共用同一份语音转写清单与执行器。
//!
//! 音频走 base64 而不是落盘：录音是"说完即丢"的临时数据，落盘还得管清理，
//! 且无会话（草稿态）时没有可落盘的会话目录。

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use crate::tools::audio_manifest;
use crate::tools::stt::audio_file_meta;
use crate::tools::tool_manifest::{apply_transform, BodyKind, TransformCtx};
use crate::tools::tool_transport::{self, Execution, FormPart};
use crate::utils::errors::AppError;

/// 转写请求超时：一段语音的转写通常在数秒内返回，给足余量但别无限等
const TRANSCRIBE_TIMEOUT_SECS: u64 = 120;

#[tauri::command]
pub async fn transcribe_audio(
    state: tauri::State<'_, crate::DbState>,
    provider_id: String,
    model: String,
    audio_base64: String,
    mime: Option<String>,
) -> Result<String, String> {
    // 取 endpoint / key / 格式（key 只在请求构造里用，不回传前端）
    let (endpoint_base, api_key, api_format, provider_name) = {
        let conn = state.get_conn()?;
        let provider = crate::commands::api_provider::get_api_provider(&conn, &provider_id)?
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "提供商「{}」不存在，请先在 设置 › API集成配置 里添加",
                    provider_id
                ))
            })?;
        let key =
            crate::commands::api_provider::get_api_key(&conn, &provider_id)?.unwrap_or_default();
        if key.trim().is_empty() {
            return Err(
                AppError::Config(format!("提供商「{}」未配置 API Key", provider.name)).into(),
            );
        }
        (
            provider.api_endpoint,
            key,
            provider.api_format,
            provider.name,
        )
    };

    if model.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "当前没有可用的模型：请先在输入框选择会话方式/模型".to_string(),
        )
        .into());
    }

    // Anthropic 原生格式没有语音转写端点（与 tools 侧注册 STT 工具的口径一致）
    if api_format.eq_ignore_ascii_case("anthropic") {
        return Err(AppError::InvalidInput(format!(
            "提供商「{}」是 Anthropic 原生格式，不支持语音转写；请在输入框切到 OpenAI 兼容的提供商/模型",
            provider_name
        ))
        .into());
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_base64.as_bytes())
        .map_err(|e| AppError::InvalidInput(format!("音频数据解析失败: {}", e)))?;
    if bytes.is_empty() {
        return Err(AppError::InvalidInput("录音内容为空".to_string()).into());
    }

    // ── 选规则（与 Agent 侧 stt 工具同一份清单）──
    let effective = audio_manifest::snapshot_stt()?;
    let (rule, _) = effective
        .manifest
        .select(&model)
        .ok_or_else(|| format!("语音转写清单缺少匹配模型 '{}' 的规则", model))?;

    let max_bytes = rule.limits.max_bytes.unwrap_or(25 * 1024 * 1024);
    if bytes.len() > max_bytes {
        return Err(AppError::InvalidInput(format!(
            "录音过大（{:.1} MB），超过模型 '{}' 的 {} MB 上限",
            bytes.len() as f64 / 1_048_576.0,
            model,
            max_bytes / 1_048_576
        ))
        .into());
    }

    // ── 收集参数：音频 + 固定 json 返回（规则未声明则不传，交给服务端默认）──
    let mime = mime.unwrap_or_else(|| "audio/webm".to_string());
    let data_url = format!("data:{};base64,{}", mime, audio_base64);
    let mut provided: Vec<(String, Value)> = vec![("file".to_string(), json!(data_url))];
    if rule.mapping.contains_key("format") {
        provided.push(("format".to_string(), json!("json")));
    }

    // ── 构造请求 ──
    let args: Map<String, Value> = provided.iter().cloned().collect();
    let ctx = TransformCtx { args: &args };
    let multipart = rule.transport.body == BodyKind::Multipart;
    let mut body = json!({});
    let mut parts: Vec<FormPart> = Vec::new();

    if multipart {
        if !rule.transport.create.contains("{model}") {
            parts.push(FormPart::Text {
                name: "model".to_string(),
                value: model.clone(),
            });
        }
    } else if !rule.transport.create.contains("{model}") {
        body["model"] = json!(model);
    }

    for (name, value) in &provided {
        let Some(mapping) = rule.mapping.get(name) else {
            continue;
        };
        if mapping.ignore || mapping.consume {
            continue;
        }
        if multipart {
            let field = mapping
                .field
                .clone()
                .ok_or_else(|| format!("语音转写清单参数 {} 缺少 field", name))?;
            if mapping
                .transform
                .as_ref()
                .is_some_and(|transform| transform.is_file_part())
            {
                let (file_name, file_mime) = audio_file_meta(&data_url, &bytes);
                parts.push(FormPart::Binary {
                    name: field,
                    bytes: bytes.clone(),
                    file_name,
                    mime: file_mime,
                });
            } else {
                let transformed = apply_transform(mapping.transform.as_ref(), value, &ctx)
                    .await
                    .map_err(|error| format!("语音转写参数 {} 转换失败: {}", name, error))?;
                parts.push(FormPart::Text {
                    name: field,
                    value: text_of(&transformed),
                });
            }
        } else {
            let transformed = apply_transform(mapping.transform.as_ref(), value, &ctx)
                .await
                .map_err(|error| format!("语音转写参数 {} 转换失败: {}", name, error))?;
            crate::tools::tool_manifest::write_mapping(&mut body, mapping, transformed)
                .map_err(|error| format!("语音转写清单参数 {} 非法: {}", name, error))?;
        }
    }

    // ── 交给通用执行器（端点、鉴权、字段全部来自清单）──
    let output_dir = std::path::PathBuf::from(".");
    let base = endpoint_base.trim_end_matches('/').to_string();
    let noop = |_: &str| {};
    let result = tokio::time::timeout(
        Duration::from_secs(TRANSCRIBE_TIMEOUT_SECS),
        tool_transport::execute(
            &Execution {
                base_url: &base,
                api_key: &api_key,
                model: &model,
                body: &body,
                parts: &parts,
                rule,
                output_dir: &output_dir,
                label: "语音转写",
                file_prefix: "transcript",
            },
            &noop,
        ),
    )
    .await
    .map_err(|_| {
        AppError::Network(format!(
            "语音识别超时（超过 {} 秒），请缩短录音后重试",
            TRANSCRIBE_TIMEOUT_SECS
        ))
    })?
    .map_err(|error| failure_with_hint(&error, &provider_name, &model))?;

    let text = result
        .text
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput("没有识别到语音内容（可能是静音或说得太短）".to_string())
        })?;
    Ok(text)
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 从执行器错误里取出 HTTP 状态码，补上可行动的说明。
fn failure_with_hint(error: &str, provider: &str, model: &str) -> String {
    let status = error
        .split("(HTTP ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|code| code.trim().parse::<u16>().ok());
    match status {
        Some(status) => {
            let detail: String = error.chars().take(400).collect();
            explain_failure(status, &detail, provider, model)
        }
        None => format!(
            "语音识别失败：{}\n提供商「{}」· 模型「{}」",
            error, provider, model
        ),
    }
}

/// 把各类 HTTP 失败翻译成"用户能据此行动"的说明。
///
/// 只说明**接口层面**的原因，不对"某个模型是否具备转写能力"下结论 ——
/// 模型能力由服务端决定且会随版本变化，本地写死判断只会过期并误导人。
fn explain_failure(status: u16, detail: &str, provider: &str, model: &str) -> String {
    let hint = match status {
        401 | 403 => "鉴权失败：检查 设置 › API集成配置 里的 API Key 是否有效、是否有该模型的权限",
        402 => "该提供商账户余额/额度不足（与语音功能无关），充值或换用其它提供商即可",
        404 => "该提供商没有语音转写端点，或模型名不存在；确认提供商格式、模型名与语音模型清单一致",
        413 => "音频体积超出该提供商限制，请缩短录音",
        429 => "请求过于频繁或超出配额，稍后重试",
        500..=599 => "提供商服务端出错（请求已到达服务端）：可能是该接口对模型/参数的支持问题，也可能是一次瞬时故障，可稍后重试",
        _ => "请对照下方服务端返回内容排查",
    };
    format!(
        "语音识别失败 (HTTP {})：{}\n提供商「{}」· 模型「{}」\n{}",
        status, detail, provider, model, hint
    )
}
