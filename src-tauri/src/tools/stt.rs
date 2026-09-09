//! 语音转文本工具（STT）：调用 OpenAI 兼容 `POST /audio/transcriptions`（multipart）。
//!
//! 模型选择走 list_models（LLM 决策）+ resolve_provider 跨 provider 解析；key 只进请求构造。
//! 音频输入支持本地路径 / http(s) URL / base64 data URL（复用 image_common 的来源归一化）。

use crate::tools::image_common::load_raw_bytes;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::Arc;

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
        Self { api_endpoint, api_key, resolve, get_models }
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

#[async_trait]
impl ToolHandler for SttTool {
    fn name(&self) -> &str {
        "stt"
    }

    fn description(&self) -> &str {
        "语音转文本（STT）：将音频文件转写为文字。使用 OpenAI 兼容的 /audio/transcriptions 接口。\
         调用前请先调用 list_models 查看可用模型，选择支持语音转写的模型名与提供商（model 为纯模型名，不带 @ 前缀，勿编造）。\
         识别准确率取决于所选模型与音频质量；可传 language 指定语言（ISO-639-1，如 zh）提升准确率。\
         若连续 2 次调用失败（如 HTTP 4xx/5xx、模型不支持转写），请停止自动重试，向用户确认三选一：\
         继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "音频文件：本地绝对路径、http(s) URL 或 base64 data URL；支持 mp3/wav/m4a/ogg/flac/webm 等"
                },
                "provider": {
                    "type": "string",
                    "description": "可选：目标提供商 id（先用 list_models 查看可用提供商）；省略时使用当前会话提供商"
                },
                "model": {
                    "type": "string",
                    "description": "语音转写模型名（必填，原样复制自 list_models 清单，含完整 namespace 前缀，禁止自行缩短或编造）"
                },
                "language": {
                    "type": "string",
                    "description": "可选：音频语言（ISO-639-1，如 zh/en），指定可提升准确率"
                },
                "response_format": {
                    "type": "string",
                    "enum": ["json", "text", "verbose_json"],
                    "description": "返回格式：json（默认，含文本与语言）、text（纯文本）、verbose_json（含分段与置信度）"
                },
                "temperature": {
                    "type": "number",
                    "description": "可选：采样温度（0~1），默认 0"
                }
            },
            "required": ["file", "model"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Audio, ToolTag::Read, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let file = arguments["file"].as_str().ok_or("缺少 file 参数")?;
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名（含 namespace 前缀）传入，不要自行编造")?;
        let language = arguments["language"].as_str().map(|s| s.to_string());
        let response_format = arguments["response_format"].as_str().unwrap_or("json").to_string();
        let temperature = arguments["temperature"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0);

        // 读取音频字节（本地/URL/data URL）
        let audio_bytes = load_raw_bytes(file).await?;
        if audio_bytes.is_empty() {
            return Err("音频文件为空或读取失败".to_string());
        }
        if audio_bytes.len() > 25 * 1024 * 1024 {
            return Err(format!("音频过大（{} MB），超过 25MB 限制", audio_bytes.len() / 1024 / 1024));
        }

        // 跨提供商解析
        let provider_arg = arguments["provider"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let (endpoint_base, api_key) = match (&provider_arg, &self.resolve) {
            (Some(pid), Some(resolve)) => match resolve(pid) {
                Some((ep, key, _fmt)) => (ep, key),
                None => return Err(format!("提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商", pid)),
            },
            _ => (self.api_endpoint.clone(), self.api_key.clone()),
        };

        // 校验 model 是否在合法清单内
        let provider_id = provider_arg.as_deref().unwrap_or("__default__");
        self.validate_model(&model, provider_id)?;

        let endpoint = format!("{}/audio/transcriptions", endpoint_base.trim_end_matches('/'));
        let form = reqwest::multipart::Form::new()
            .text("model", model)
            .text("response_format", response_format.clone())
            .text("temperature", temperature.to_string())
            .part(
                "file",
                reqwest::multipart::Part::bytes(audio_bytes)
                    .file_name("audio")
                    .mime_str("application/octet-stream")
                    .map_err(|e| format!("设置音频 MIME 失败: {}", e))?,
            );
        let form = if let Some(lang) = language {
            form.text("language", lang)
        } else {
            form
        };

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .bearer_auth(&api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("语音转写请求失败: {}", e))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("语音转写失败 (HTTP {}): {}", status.as_u16(), body));
        }

        if response_format == "text" {
            let text = body.trim().to_string();
            if text.is_empty() {
                return Err("转写结果为空（音频可能无有效语音）".to_string());
            }
            return Ok(text);
        }

        // json / verbose_json：解析 {text, ...}
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("解析转写结果失败: {}：{}", e, &body[..body.len().min(200)]))?;
        let text = json["text"].as_str().unwrap_or("").trim().to_string();
        if text.is_empty() {
            return Err("转写结果为空（音频可能无有效语音）".to_string());
        }
        let lang = json["language"].as_str().unwrap_or("unknown");
        let segments = json["segments"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(format!("转写文本（语言 {}，{} 段）：\n{}", lang, segments, text))
    }
}
