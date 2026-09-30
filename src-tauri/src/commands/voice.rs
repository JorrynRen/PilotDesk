//! 语音输入（云端转写）：把输入框录到的音频交给**当前会话所选的提供商/模型**转成文字。
//!
//! 与 `tools::stt` 的分工：那个是 Agent 可自行调用的工具（由模型挑 provider/model）；
//! 这里是用户按麦克风触发的固定路径 —— 用当前会话（或"快捷开始"草稿）已选的 provider/model，
//! 不额外给用户挑模型的入口。两者共用同一套 OpenAI 兼容 `/audio/transcriptions` 契约。
//!
//! 音频走 base64 而不是落盘：录音是"说完即丢"的临时数据，落盘还得管清理，
//! 且无会话（草稿态）时没有可落盘的会话目录。

use std::time::Duration;

use base64::Engine;

use crate::utils::errors::AppError;

/// 音频上限：与 `tools::stt` 保持一致（超出后多数兼容实现会直接拒绝）
const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

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

    // Anthropic 原生格式没有 /audio/transcriptions（与 tools 侧注册 STT 工具的口径一致）
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
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err(AppError::InvalidInput(format!(
            "录音过大（{:.1} MB），超过 {} MB 上限",
            bytes.len() as f64 / 1_048_576.0,
            MAX_AUDIO_BYTES / 1_048_576
        ))
        .into());
    }

    let url = format!(
        "{}/audio/transcriptions",
        endpoint_base.trim_end_matches('/')
    );
    let mime = mime.unwrap_or_else(|| "audio/webm".to_string());
    // 文件名带扩展名：部分兼容实现按文件名判断容器格式
    let file_name = if mime.contains("wav") {
        "speech.wav"
    } else {
        "speech.webm"
    };
    let form = reqwest::multipart::Form::new()
        .text("model", model.clone())
        // 用 json 而不是 text：text 是各家兼容层支持度最参差的取值的那个
        .text("response_format", "json")
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes)
                .file_name(file_name)
                .mime_str(&mime)
                .map_err(|e| AppError::InvalidInput(format!("设置音频 MIME 失败: {}", e)))?,
        );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TRANSCRIBE_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::Network(format!("构造请求客户端失败: {}", e)))?;
    let resp = client
        .post(&url)
        .bearer_auth(&api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| AppError::Network(format!("语音识别请求失败: {}", e)))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail: String = body.trim().chars().take(400).collect();
        return Err(AppError::External(explain_failure(
            status.as_u16(),
            &detail,
            &provider_name,
            &model,
        ))
        .into());
    }

    // 兼容两种返回：json（{text}）与部分实现仍回纯文本
    let trimmed = body.trim();
    let text = if trimmed.starts_with('{') {
        serde_json::from_str::<serde_json::Value>(trimmed)
            .ok()
            .and_then(|v| v["text"].as_str().map(|s| s.trim().to_string()))
            .unwrap_or_default()
    } else {
        trimmed.to_string()
    };
    if text.is_empty() {
        return Err(AppError::InvalidInput(
            "没有识别到语音内容（可能是静音或说得太短）".to_string(),
        )
        .into());
    }
    Ok(text)
}

/// 把各类 HTTP 失败翻译成"用户能据此行动"的说明。
///
/// 只说明**接口层面**的原因，不对"某个模型是否具备转写能力"下结论 ——
/// 模型能力由服务端决定且会随版本变化，本地写死判断只会过期并误导人。
fn explain_failure(status: u16, detail: &str, provider: &str, model: &str) -> String {
    let hint = match status {
        401 | 403 => "鉴权失败：检查 设置 › API集成配置 里的 API Key 是否有效、是否有该模型的权限",
        402 => "该提供商账户余额/额度不足（与语音功能无关），充值或换用其它提供商即可",
        404 => "该提供商没有 /audio/transcriptions 端点，或模型名不存在；确认提供商为 OpenAI 兼容格式、模型名与清单一致",
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
