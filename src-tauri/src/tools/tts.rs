//! 文本转语音工具（TTS）：调用 OpenAI 兼容 `POST /audio/speech`。
//!
//! 模型选择走 list_models（LLM 决策）+ resolve_provider 跨 provider 解析；key 只进请求构造。
//! 音频落盘到 `<cwd>/attachments/<session_id>/`，返回路径供前端 convertFileSrc + <audio> 播放。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::Arc;

pub struct TtsTool {
    api_endpoint: String,
    api_key: String,
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    cwd: String,
    session_id: String,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
}

impl TtsTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
        cwd: String,
        session_id: String,
    ) -> Self {
        Self {
            api_endpoint,
            api_key,
            resolve,
            cwd,
            session_id,
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

#[async_trait]
impl ToolHandler for TtsTool {
    fn name(&self) -> &str {
        "tts"
    }

    fn description(&self) -> &str {
        "文本转语音（TTS）：将文本合成为语音音频并保存到本地。使用 OpenAI 兼容的 /audio/speech 接口。\
         调用前必须先调用 list_models 查看可用模型清单，严格按清单中的完整模型名（含 organization/ 前缀，如 TeleAI/TeleSpeechASR）传入；\
         禁止自行缩短、省略 namespace 前缀或编造清单外的模型名。\
         若连续 2 次调用失败（如 HTTP 4xx/5xx、模型不支持语音），请停止自动重试，向用户确认三选一：\
         继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "要朗读的文本内容"
                },
                "provider": {
                    "type": "string",
                    "description": "必填：目标提供商 id，逐字取 list_models 清单里的 provider_id（标 ★ 的是当前会话提供商）；provider 与 model 必须分别传入，禁止拼成 provider_id/model"
                },
                "model": {
                    "type": "string",
                    "description": "语音合成模型名（必填，原样复制自 list_models 清单；若模型名自身含斜杠/命名空间（如 TeleAI/xxx），必须原样保留；禁止自行缩短、编造，也禁止把 provider_id 拼进来）"
                },
                "voice": {
                    "type": "string",
                    "description": "可选：音色（如 alloy/echo/fable/onyx/nova/shimmer 或服务商提供的音色名）"
                },
                "language": {
                    "type": "string",
                    "description": "可选：语言（如 zh/en，依赖模型支持）"
                },
                "response_format": {
                    "type": "string",
                    "enum": ["mp3", "opus", "aac", "flac", "wav"],
                    "description": "音频格式，默认 mp3"
                },
                "speed": {
                    "type": "number",
                    "description": "语速（0.25~4.0），默认 1.0"
                }
            },
            "required": ["text", "model", "provider"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Audio, ToolTag::Write, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let text = arguments["text"].as_str().ok_or("缺少 text 参数")?;
        if text.trim().is_empty() {
            return Err("text 不能为空".to_string());
        }
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名（含 namespace 前缀）传入，不要自行编造")?;
        let voice = arguments["voice"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "alloy".to_string());
        let language = arguments["language"].as_str().map(|s| s.to_string());
        let response_format = arguments["response_format"]
            .as_str()
            .unwrap_or("mp3")
            .to_string();
        let speed = arguments["speed"].as_f64().unwrap_or(1.0).clamp(0.25, 4.0);

        // 跨提供商解析（key 仅用于请求构造，不进返回值）
        let provider_arg = arguments["provider"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // provider 与 model 必须分别传入：缺 provider 时不再静默回退会话提供商
        if provider_arg.is_none() {
            return Err("缺少 provider 参数：provider 与 model 必须分别传，禁止写成 provider_id/model。请先调用 list_models，从清单里挑出目标模型所在那一行（标 ★ 的是当前会话提供商），把 provider_id 与模型名分别填入 provider / model".to_string());
        }
        let (endpoint_base, api_key) = match (&provider_arg, &self.resolve) {
            (Some(pid), Some(resolve)) => match resolve(pid) {
                Some((ep, key, _fmt)) => (ep, key),
                None => {
                    return Err(format!(
                        "提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商",
                        pid
                    ))
                }
            },
            _ => (self.api_endpoint.clone(), self.api_key.clone()),
        };

        // 校验 model 是否在合法清单内
        let provider_id = provider_arg.as_deref().unwrap_or("");
        self.validate_model(&model, provider_id)?;

        let endpoint = format!("{}/audio/speech", endpoint_base.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": model,
            "input": text,
            "voice": voice,
            "response_format": response_format,
            "speed": speed,
        });
        let mut body = body;
        if let Some(lang) = language {
            body["language"] = serde_json::json!(lang);
        }

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("语音合成请求失败: {}", e))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("语音合成失败 (HTTP {}): {}", status.as_u16(), text));
        }
        let audio_bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("读取音频失败: {}", e))?;
        if audio_bytes.is_empty() {
            return Err("服务端返回空音频".to_string());
        }

        // 落盘（复用附件模型目录）
        let dir = if self.session_id.is_empty() {
            std::path::PathBuf::from(&self.cwd)
        } else {
            std::path::PathBuf::from(format!("{}/attachments/{}", self.cwd, self.session_id))
        };
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建音频目录失败: {}", e))?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let ext = if response_format == "mp3" {
            "mp3"
        } else {
            &response_format
        };
        let path = dir.join(format!("tts_{}.{}", ts, ext));
        std::fs::write(&path, &audio_bytes).map_err(|e| format!("写入音频文件失败: {}", e))?;

        Ok(format!(
            "语音已生成：{}（{} 字节，格式 {}）。可直接播放该文件。",
            path.display(),
            audio_bytes.len(),
            response_format
        ))
    }
}
