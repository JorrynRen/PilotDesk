//! 语音转文本工具（STT）。
//!
//! 模型差异（端点、鉴权、表单字段、结果取值、限制）全部写在
//! `resources/stt-models.json` 里，由 [`audio_manifest`] 解释、
//! [`tool_transport`] 执行；本文件不包含厂商字段名。
//!
//! 音频输入支持本地路径 / http(s) URL / base64 data URL（复用 image_common 的来源归一化）。
//! 模型选择走 list_models + resolve_provider，并沿用图片 / 视频工具相同的模型白名单校验。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::tools::audio_manifest;
use crate::tools::image_common::load_raw_bytes;
use crate::tools::tool_manifest::{apply_transform, write_mapping, TransformCtx};
use crate::tools::tool_transport::{self, Execution, FormPart};
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

pub struct SttTool {
    api_endpoint: String,
    api_key: String,
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
}

impl SttTool {
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
    ) -> Self {
        Self {
            api_endpoint,
            api_key,
            resolve,
            get_models,
        }
    }

    /// 校验 model 是否在当前 provider 的合法模型列表中，不在则返回引导错误。
    fn validate_model(&self, model: &str, provider_id: &str) -> Result<(), String> {
        if let Some(get_models) = &self.get_models {
            let valid: Vec<String> = get_models(provider_id.to_string());
            if !valid.is_empty() && !valid.iter().any(|m| m == model) {
                return Err(format!(
                    "模型 '{}' 不是提供商 '@{}' 的有效模型。\
                     有效模型：{}\n请重新调用 list_models，从返回结果中完整复制模型名（含 namespace 前缀如 TeleAI/xxx）后重试。",
                    model, provider_id, valid.join(", ")
                ));
            }
        }
        Ok(())
    }
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 按来源扩展名或字节头推断文件名与 MIME（multipart 文件 part 用）
pub(crate) fn audio_file_meta(source: &str, bytes: &[u8]) -> (String, String) {
    let sniffed = if bytes.starts_with(b"ID3")
        || (bytes.len() > 1 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0)
    {
        Some(("mp3", "audio/mpeg"))
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Some(("wav", "audio/wav"))
    } else if bytes.starts_with(b"OggS") {
        Some(("ogg", "audio/ogg"))
    } else if bytes.starts_with(b"fLaC") {
        Some(("flac", "audio/flac"))
    } else if bytes.get(4..8) == Some(b"ftyp") {
        Some(("m4a", "audio/mp4"))
    } else if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some(("webm", "audio/webm"))
    } else {
        None
    };
    if let Some((extension, mime)) = sniffed {
        return (format!("audio.{}", extension), mime.to_string());
    }

    let extension = source
        .split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit('.').next())
        .map(|ext| ext.to_ascii_lowercase())
        .filter(|ext| {
            matches!(
                ext.as_str(),
                "mp3" | "wav" | "m4a" | "ogg" | "flac" | "webm" | "aac" | "opus" | "amr"
            )
        })
        .unwrap_or_else(|| "mp3".to_string());
    let mime = match extension.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "webm" => "audio/webm",
        "aac" => "audio/aac",
        "opus" => "audio/opus",
        "amr" => "audio/amr",
        _ => "application/octet-stream",
    };
    (format!("audio.{}", extension), mime.to_string())
}

#[async_trait]
impl ToolHandler for SttTool {
    fn name(&self) -> &str {
        "stt"
    }

    fn description(&self) -> &str {
        "语音转文本（STT）：将音频文件转写为文字。\
         端点、鉴权、表单字段与返回格式由语音转写模型清单决定，支持 OpenAI 兼容 /audio/transcriptions、Azure 等。\
         调用前请先调用 list_models 查看可用模型，选择支持语音转写的模型名与提供商；\
         不确定模型能力（是否支持 temperature / verbose_json 等）时先调用 list_audio_capabilities（kind=stt）。\
         识别准确率取决于所选模型与音频质量；可传 language 指定语言（ISO-639-1，如 zh）提升准确率。\
         若连续 2 次调用失败（如 HTTP 4xx/5xx、模型不支持转写），请停止自动重试，向用户确认三选一：\
         继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "音频文件：本地绝对路径、http(s) URL 或 base64 data URL；支持 mp3/wav/m4a/ogg/flac/webm 等"
                },
                "provider": {
                    "type": "string",
                    "description": "必填：目标提供商 id，逐字取 list_models 清单里的 provider_id（标 ★ 的是当前会话提供商）；provider 与 model 必须分别传入，禁止拼成 provider_id/model"
                },
                "model": {
                    "type": "string",
                    "description": "语音转写模型名（必填，原样复制自 list_models 清单；若模型名自身含斜杠/命名空间（如 TeleAI/xxx），必须原样保留；禁止自行缩短、编造，也禁止把 provider_id 拼进来）"
                },
                "language": {
                    "type": "string",
                    "description": "可选：音频语言（ISO-639-1，如 zh/en），指定可提升准确率"
                },
                "format": {
                    "type": "string",
                    "description": "可选：返回格式（json / text / verbose_json），不传由模型默认决定"
                },
                "temperature": {
                    "type": "number",
                    "description": "可选：采样温度（0~1），部分模型不支持"
                }
            },
            "required": ["file", "model", "provider"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Audio, ToolTag::Read, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let file = arguments["file"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("缺少 file 参数")?;
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名（含 namespace 前缀）传入，不要自行编造")?;

        // 跨提供商解析（key 仅用于请求构造，不进返回值）
        let provider_id = arguments["provider"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 provider 参数：provider 与 model 必须分别传，禁止写成 provider_id/model。请先调用 list_models，从清单里挑出目标模型所在那一行（标 ★ 的是当前会话提供商），把 provider_id 与模型名分别填入 provider / model")?;
        let (endpoint_base, api_key) = match &self.resolve {
            Some(resolve) => match resolve(&provider_id) {
                Some((ep, key, _fmt)) => (ep, key),
                None => {
                    return Err(format!(
                        "提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商",
                        provider_id
                    ))
                }
            },
            None => (self.api_endpoint.clone(), self.api_key.clone()),
        };

        // 校验 model 是否在合法清单内（与图片 / 视频工具一致）
        self.validate_model(&model, &provider_id)?;

        // ── 选规则 ──
        let effective = audio_manifest::snapshot_stt()?;
        let (rule, verified) = effective
            .manifest
            .select(&model)
            .ok_or_else(|| format!("语音转写清单缺少匹配模型 '{}' 的规则", model))?;

        // ── 读取音频（本地 / URL / data URL）──
        let audio_bytes = load_raw_bytes(file).await?;
        if audio_bytes.is_empty() {
            return Err("音频文件为空或读取失败".to_string());
        }
        let max_bytes = rule.limits.max_bytes.unwrap_or(25 * 1024 * 1024);
        if audio_bytes.len() > max_bytes {
            return Err(format!(
                "[limit_exceeded] 音频过大（{} MB），超过模型 '{}' 的 {} MB 限制",
                audio_bytes.len() / 1024 / 1024,
                model,
                max_bytes / 1024 / 1024
            ));
        }

        // ── 收集调用方给出的参数 ──
        let mut provided: Vec<(String, Value)> = vec![("file".to_string(), json!(file))];
        if let Some(language) = arguments["language"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            provided.push(("language".to_string(), json!(language)));
        }
        let format = arguments["format"]
            .as_str()
            .or_else(|| arguments["response_format"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(format) = format {
            provided.push(("format".to_string(), json!(format)));
        }
        if let Some(temperature) = arguments["temperature"].as_f64() {
            provided.push(("temperature".to_string(), json!(temperature)));
        }

        // ── 不支持即报错，绝不静默丢弃 ──
        let mut unsupported: Vec<String> = Vec::new();
        for (name, _) in &provided {
            match rule.mapping.get(name) {
                Some(mapping) if !mapping.ignore => {}
                _ => unsupported.push(name.clone()),
            }
        }
        if !unsupported.is_empty() {
            let supported: Vec<String> = rule
                .mapping
                .iter()
                .filter(|(_, mapping)| !mapping.ignore)
                .map(|(name, _)| name.clone())
                .collect();
            return Err(format!(
                "[unsupported_input] 模型 '{}' 不支持参数：{}（命中规则 {}）。该规则支持：{}",
                model,
                unsupported.join("、"),
                rule.match_pattern,
                supported.join("、")
            ));
        }

        // ── 构造请求 ──
        let args: Map<String, Value> = provided.iter().cloned().collect();
        let ctx = TransformCtx { args: &args };
        let multipart = rule.transport.body == crate::tools::tool_manifest::BodyKind::Multipart;
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

        let mut echo_fields: Vec<Value> = Vec::new();
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
                    .ok_or_else(|| format!("[manifest_invalid] 参数 {} 缺少 field", name))?;
                if mapping
                    .transform
                    .as_ref()
                    .is_some_and(|transform| transform.is_file_part())
                {
                    let (file_name, mime) = audio_file_meta(file, &audio_bytes);
                    parts.push(FormPart::Binary {
                        name: field,
                        bytes: audio_bytes.clone(),
                        file_name,
                        mime,
                    });
                } else {
                    let transformed = apply_transform(mapping.transform.as_ref(), value, &ctx)
                        .await
                        .map_err(|error| format!("[transform_failed] 参数 {}: {}", name, error))?;
                    parts.push(FormPart::Text {
                        name: field,
                        value: text_of(&transformed),
                    });
                }
            } else {
                let transformed = apply_transform(mapping.transform.as_ref(), value, &ctx)
                    .await
                    .map_err(|error| format!("[transform_failed] 参数 {}: {}", name, error))?;
                write_mapping(&mut body, mapping, transformed)
                    .map_err(|error| format!("[manifest_invalid] 参数 {}: {}", name, error))?;
            }
            echo_fields.push(json!(name));
        }

        let absent: Vec<&str> = audio_manifest::STT_SPEC
            .slots
            .iter()
            .copied()
            .filter(|name| !provided.iter().any(|(key, _)| key == name))
            .collect();
        let echo = json!({
            "manifestVersion": effective.manifest.version,
            "manifestSource": effective.source.as_str(),
            "matchedRule": rule.match_pattern,
            "createPath": rule.transport.create,
            "verified": verified,
            "fields": echo_fields,
            "absent": absent,
        });

        // ── 交给通用执行器 ──
        let output_dir = std::path::PathBuf::from(".");
        let base = endpoint_base.trim_end_matches('/').to_string();
        let noop = |_: &str| {};
        let result = tool_transport::execute(
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
        )
        .await
        .map_err(|error| format!("[provider_rejected] {}\n\nrequestEcho:\n{}", error, echo))?;

        let text = result
            .text
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "转写结果为空（音频可能无有效语音）\n\nrequestEcho:\n{}",
                    echo
                )
            })?;

        Ok(format!(
            "转写文本（规则 {}）：\n{}\n\nrequestEcho:\n{}",
            rule.match_pattern, text, echo
        ))
    }
}
