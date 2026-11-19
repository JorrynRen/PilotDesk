//! 文本转语音工具（TTS）。
//!
//! 模型差异（端点、鉴权、字段名、结果编码、限制）全部写在
//! `resources/tts-models.json` 里，由 [`audio_manifest`] 解释、
//! [`tool_transport`] 执行；本文件不包含厂商字段名。
//!
//! 模型选择走 list_models + resolve_provider 跨 provider 解析，并沿用图片 / 视频工具
//! 相同的模型白名单校验；key 只进请求构造，绝不进入工具返回值。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::tools::audio_manifest;
use crate::tools::tool_manifest::{apply_transform, write_mapping, TransformCtx};
use crate::tools::tool_transport::{self, Execution};
use crate::tools::{RiskLevel, ToolArtifacts, ToolFile, ToolHandler, ToolTag};

pub struct TtsTool {
    api_endpoint: String,
    api_key: String,
    /// 跨 provider 解析（providerId → (endpoint, key, api_format)）
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
    cwd: String,
    session_id: String,
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
            get_models,
            cwd,
            session_id,
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
        "文本转语音（TTS）：将文本合成为语音音频并保存到本地，返回可直接播放的音频。\
         端点、鉴权、字段与输出格式由语音合成模型清单决定，支持 OpenAI 兼容 /audio/speech、Azure、MiniMax 等。\
         调用前必须先调用 list_models 查看可用模型清单，严格按清单中的完整模型名（含 namespace 前缀）传入；\
         不确定模型能力时先调用 list_audio_capabilities（kind=tts）。\
         若连续 2 次调用失败（如 HTTP 4xx/5xx、模型不支持语音合成），请停止自动重试，向用户确认三选一：\
         继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
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
                    "description": "可选：音色（如 alloy/echo/fable/onyx/nova/shimmer 或服务商提供的音色 id）；不传由模型默认决定"
                },
                "language": {
                    "type": "string",
                    "description": "可选：语言（如 zh/en，依赖模型支持）"
                },
                "format": {
                    "type": "string",
                    "description": "可选：音频输出格式（如 mp3/wav/opus/pcm），不传由模型默认决定"
                },
                "speed": {
                    "type": "number",
                    "description": "可选：语速（0.25~4.0），不传由模型默认决定"
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
        self.run(arguments).await.map(|(text, _)| text)
    }

    async fn execute_structured(
        &self,
        arguments: serde_json::Value,
    ) -> Result<(String, ToolArtifacts), String> {
        self.run(arguments).await
    }
}

impl TtsTool {
    /// 执行并返回结构化产出（工作流 media 节点直接消费，无需解析 Markdown）。
    async fn run(&self, arguments: serde_json::Value) -> Result<(String, ToolArtifacts), String> {
        let text = arguments["text"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("缺少 text 参数")?;
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
        let effective = audio_manifest::snapshot_tts()?;
        let (rule, verified) = effective
            .manifest
            .select(&model)
            .ok_or_else(|| format!("语音合成清单缺少匹配模型 '{}' 的规则", model))?;

        // ── 收集调用方给出的参数 ──
        let mut provided: Vec<(String, Value)> = vec![("text".to_string(), json!(text))];
        if let Some(voice) = arguments["voice"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            provided.push(("voice".to_string(), json!(voice)));
        }
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
        if let Some(speed) = arguments["speed"].as_f64() {
            provided.push(("speed".to_string(), json!(speed)));
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
        if let Some(limit) = rule.limits.max_chars {
            let length = text.chars().count();
            if length > limit {
                return Err(format!(
                    "[limit_exceeded] 文本长度 {} 超过模型 '{}' 的上限 {} 字符",
                    length, model, limit
                ));
            }
        }

        // ── 构造请求 ──
        let args: Map<String, Value> = provided.iter().cloned().collect();
        let ctx = TransformCtx { args: &args };
        let mut body = json!({});
        if !rule.omit_model && !rule.transport.create.contains("{model}") {
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
            let transformed = apply_transform(mapping.transform.as_ref(), value, &ctx)
                .await
                .map_err(|error| format!("[transform_failed] 参数 {}: {}", name, error))?;
            write_mapping(&mut body, mapping, transformed)
                .map_err(|error| format!("[manifest_invalid] 参数 {}: {}", name, error))?;
            echo_fields.push(json!(name));
        }

        let absent: Vec<&str> = audio_manifest::TTS_SPEC
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
        let output_dir = if self.session_id.is_empty() {
            PathBuf::from(&self.cwd)
        } else {
            crate::utils::paths::resolve_attachments_dir(&self.cwd, &self.session_id)
        };
        let base = endpoint_base.trim_end_matches('/').to_string();
        let noop = |_: &str| {};
        let result = tool_transport::execute(
            &Execution {
                base_url: &base,
                api_key: &api_key,
                model: &model,
                body: &body,
                parts: &[],
                rule,
                output_dir: &output_dir,
                label: "语音",
                file_prefix: "tts",
            },
            &noop,
        )
        .await
        .map_err(|error| format!("[provider_rejected] {}\n\nrequestEcho:\n{}", error, echo))?;

        let path = result.local_path.ok_or_else(|| {
            format!(
                "[provider_rejected] 任务已结束但没有可用的语音结果\n\nrequestEcho:\n{}",
                echo
            )
        })?;

        Ok((
            format!(
                "语音已生成（规则 {}）：\n![音频](<{}>)",
                rule.match_pattern, path
            ),
            ToolArtifacts {
                urls: Vec::new(),
                local_paths: vec![path.clone()],
                files: vec![ToolFile {
                    file_type: "audio".to_string(),
                    path,
                }],
                manifest: Some(json!({
                    "rule": rule.match_pattern,
                    "version": effective.manifest.version,
                })),
            },
        ))
    }
}
