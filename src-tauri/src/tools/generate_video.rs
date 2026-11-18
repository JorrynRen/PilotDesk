//! 视频生成工具：统一参数契约 + 外置模型映射清单。
//!
//! 工具只暴露固定的六个媒体槽位（首帧/尾帧/参考图/参考视频/编辑源视频/延长源视频）
//! 与六个标量（提示词/时长/画幅/分辨率/种子/负向提示词）。
//! 模型差异（端点、字段名、任务判定、结果取值、限制）全部写在
//! `resources/video-models.json` 里，由 [`video_manifest`] 解释、
//! [`video_transport`] 执行；本文件不包含任何厂商字段名。
//!
//! 发送前的本地校验：不支持的参数直接报错，绝不静默丢弃；不支持的组合直接拒绝。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tauri::Emitter;

use crate::tools::video_manifest::{
    apply_transform, snapshot, write_mapping, TransformCtx, MEDIA_SLOTS, SCALAR_SLOTS,
};
use crate::tools::video_transport::{self, Execution};
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

/// 视频生成工具
pub struct GenerateVideoTool {
    api_endpoint: String,
    api_key: String,
    /// 跨 provider 解析（providerId → (endpoint, key, api_format)）
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
    /// 前端进度事件发射（任务创建成功 / 每 30s 状态提示）；None 时静默（如目录元数据装配）
    app: Option<tauri::AppHandle>,
    session_id: String,
    /// 工作目录（binary 模式落盘用）
    cwd: String,
}

impl GenerateVideoTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
        app: Option<tauri::AppHandle>,
        session_id: String,
        cwd: String,
    ) -> Self {
        Self {
            api_endpoint,
            api_key,
            resolve,
            get_models,
            app,
            session_id,
            cwd,
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
                    model,
                    provider_id,
                    valid.join(", ")
                ));
            }
        }
        Ok(())
    }

    /// 向前端发射进度事件（`agent-tool-progress`），弥补视频生成期间的工具执行空白期。
    fn emit_progress(&self, message: &str) {
        if let Some(app) = &self.app {
            let _ = app.emit(
                "agent-tool-progress",
                serde_json::json!({
                    "sessionId": self.session_id,
                    "toolName": "generate_video",
                    "message": message,
                }),
            );
        }
        log::info!("[GenerateVideo] 进度: {}", message);
    }
}

fn text_arg(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn is_public_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

/// 回显用的值摘要：避免把 base64 载荷写进工具返回值
fn value_summary(value: &Value) -> String {
    match value {
        Value::String(text) if text.chars().count() > 60 => {
            let head: String = text.chars().take(60).collect();
            format!("{}…（{} 字符）", head, text.chars().count())
        }
        Value::String(text) => text.clone(),
        Value::Array(items) => format!("[{} 项]", items.len()),
        other => other.to_string(),
    }
}

#[async_trait]
impl ToolHandler for GenerateVideoTool {
    fn name(&self) -> &str {
        "generate_video"
    }

    fn description(&self) -> &str {
        "生成视频（文生视频 / 图生视频 / 首尾帧 / 参考图 / 参考视频）。\
         参数按统一槽位传入：firstFrame、lastFrame、referenceImages、referenceVideo、\
         sourceVideo、extendVideo，以及 prompt、duration、aspectRatio、resolution、seed、negativePrompt。\
         各模型支持的槽位不同：不支持的参数会在本地直接返回 unsupported_input，不会静默丢给上游。\
         参考视频与源视频只接受公网 http(s) URL；图像槽位可传公网 URL 或本地图片路径。\
         创建任务后自动轮询直至完成（通常 1~5 分钟）。模型需经 list_models 选择支持视频生成的模型。\
         若连续 2 次调用失败，请停止自动重试，向用户确认是继续重试、更换模型，还是跳过该工具。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "视频内容描述（英文效果最佳，尽量详细：主体、动作、镜头运动、场景氛围等；需要配音时请显式注明语言与要求）"
                },
                "model": {
                    "type": "string",
                    "description": "视频生成模型名（必填，原样复制自 list_models 清单；若模型名自身含斜杠/命名空间（如 TeleAI/xxx），必须原样保留；禁止自行缩短、编造，也禁止把 provider_id 拼进来）"
                },
                "provider": {
                    "type": "string",
                    "description": "必填：目标提供商 id，逐字取 list_models 清单里的 provider_id（标 ★ 的是当前会话提供商）；provider 与 model 必须分别传入，禁止拼成 provider_id/model"
                },
                "firstFrame": {
                    "type": "string",
                    "description": "可选：起始关键帧图像。公网 URL 或本地图片路径；仅在目标模型需要字节时才会读取本地文件"
                },
                "lastFrame": {
                    "type": "string",
                    "description": "可选：结束关键帧图像。公网 URL 或本地图片路径；必须与 firstFrame 同时传入"
                },
                "referenceImages": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "可选：内容/风格参考图，不代表时间位置。元素为公网 URL 或本地图片路径；与 firstFrame/lastFrame 互斥"
                },
                "referenceVideo": {
                    "type": "string",
                    "description": "可选：参考视频，仅接受公网 http(s) URL；与 sourceVideo 互斥"
                },
                "sourceVideo": {
                    "type": "string",
                    "description": "可选：编辑源视频（在原片上修改），仅接受公网 http(s) URL；与 referenceVideo 互斥"
                },
                "extendVideo": {
                    "type": "string",
                    "description": "可选：延长源视频（保留原片并接新段），仅接受公网 http(s) URL；与其他媒体槽位互斥"
                },
                "duration": {
                    "type": "number",
                    "description": "可选：视频时长（秒）。可用档位由目标模型决定，本地会校验，不在档位内会报 limit_exceeded"
                },
                "aspectRatio": {
                    "type": "string",
                    "enum": ["16:9", "9:16", "1:1", "21:9", "4:3", "3:4"],
                    "description": "可选：画幅比例。是否支持由目标模型决定，本地会校验"
                },
                "resolution": {
                    "type": "string",
                    "enum": ["480p", "540p", "720p", "1080p", "4k"],
                    "description": "可选：分辨率。是否支持由目标模型决定，本地会校验"
                },
                "seed": {
                    "type": "integer",
                    "description": "可选：随机种子，用于复现结果"
                },
                "negativePrompt": {
                    "type": "string",
                    "description": "可选：负向提示词，描述不希望出现的内容"
                }
            },
            "required": ["prompt", "model", "provider"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Video, ToolTag::Write, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let provider = text_arg(&arguments, "provider").ok_or_else(|| {
            "缺少 provider 参数：provider 与 model 必须分别传，禁止写成 provider_id/model。\
             请先调用 list_models，把 provider_id 与模型名分别填入 provider / model"
                .to_string()
        })?;
        let prompt = text_arg(&arguments, "prompt").ok_or("缺少 prompt 参数")?;
        let model = text_arg(&arguments, "model")
            .map(|text| text.trim_start_matches('@').to_string())
            .filter(|text| !text.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名传入，不要自行编造")?;

        // ── 收集已传参数（顺序即回显顺序）──
        let mut provided: Vec<(String, Value)> = vec![("prompt".to_string(), json!(prompt))];
        for slot in [
            "firstFrame",
            "lastFrame",
            "referenceVideo",
            "sourceVideo",
            "extendVideo",
            "duration",
            "aspectRatio",
            "resolution",
            "seed",
            "negativePrompt",
        ] {
            match slot {
                "duration" | "seed" => {
                    if let Some(value) = arguments.get(slot).filter(|value| value.is_number()) {
                        provided.push((slot.to_string(), value.clone()));
                    }
                }
                _ => {
                    if let Some(value) = text_arg(&arguments, slot) {
                        provided.push((slot.to_string(), json!(value)));
                    }
                }
            }
        }
        if let Some(items) = arguments.get("referenceImages").and_then(|value| value.as_array()) {
            let items: Vec<Value> = items
                .iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(|item| json!(item))
                .collect();
            if !items.is_empty() {
                provided.push(("referenceImages".to_string(), Value::Array(items)));
            }
        }

        let has = |name: &str| provided.iter().any(|(key, _)| key == name);

        // ── 全局互斥与依赖（写在代码里，清单不能改变）──
        if has("lastFrame") && !has("firstFrame") {
            return Err(
                "[dependency_violation] lastFrame 必须与 firstFrame 同时传入".to_string(),
            );
        }
        if has("extendVideo")
            && (has("sourceVideo")
                || has("referenceVideo")
                || has("firstFrame")
                || has("lastFrame"))
        {
            return Err(
                "[conflict] extendVideo 不能与 sourceVideo / referenceVideo / firstFrame / lastFrame 同时使用"
                    .to_string(),
            );
        }
        if has("sourceVideo") && has("referenceVideo") {
            return Err(
                "[conflict] sourceVideo 与 referenceVideo 互斥，请只保留一个".to_string(),
            );
        }
        if has("referenceImages") && (has("firstFrame") || has("lastFrame")) {
            return Err(
                "[conflict] referenceImages 与 firstFrame / lastFrame 互斥，请二选一".to_string(),
            );
        }
        for slot in ["referenceVideo", "sourceVideo", "extendVideo"] {
            if let Some(value) = provided
                .iter()
                .find(|(key, _)| key == slot)
                .and_then(|(_, value)| value.as_str())
            {
                if !is_public_url(value) {
                    return Err(format!(
                        "[unsupported_input] {} 仅接受公网 http(s) URL（视频无法内联上传）：{}",
                        slot, value
                    ));
                }
            }
        }

        // ── 解析提供商与模型 ──
        let (endpoint, api_key) = match &self.resolve {
            Some(resolve) => match resolve(&provider) {
                Some((endpoint, key, _format)) => (endpoint, key),
                None => {
                    return Err(format!(
                        "提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商",
                        provider
                    ))
                }
            },
            None => (self.api_endpoint.clone(), self.api_key.clone()),
        };
        let base = endpoint.trim_end_matches('/').to_string();
        self.validate_model(&model, &provider)?;

        // ── 选规则并做本地校验 ──
        let effective =
            snapshot().map_err(|error| format!("[manifest_invalid] {}", error))?;
        let manifest = &effective.manifest;
        let (rule, verified) = manifest
            .select(&model)
            .ok_or_else(|| format!("[manifest_invalid] 模型 '{}' 未命中任何规则", model))?;

        let mut unsupported: Vec<String> = Vec::new();
        let mut ignored: Vec<String> = Vec::new();
        for (name, _) in &provided {
            match rule.mapping.get(name) {
                Some(mapping) if mapping.ignore => ignored.push(name.clone()),
                Some(_) => {}
                None => unsupported.push(name.clone()),
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

        if let Some(count) = provided
            .iter()
            .find(|(key, _)| key == "referenceImages")
            .and_then(|(_, value)| value.as_array())
            .map(|items| items.len())
        {
            if let Some(limit) = rule.limits.reference_images {
                if count > limit {
                    return Err(format!(
                        "[limit_exceeded] referenceImages 最多 {} 张，本次传入 {} 张（模型 '{}'）",
                        limit, count, model
                    ));
                }
            }
        }
        if let Some(duration) = provided
            .iter()
            .find(|(key, _)| key == "duration")
            .and_then(|(_, value)| value.as_f64())
        {
            if !rule.limits.durations.is_empty()
                && !rule
                    .limits
                    .durations
                    .iter()
                    .any(|allowed| (allowed - duration).abs() < f64::EPSILON)
            {
                return Err(format!(
                    "[limit_exceeded] 时长 {} 秒不被模型 '{}' 支持（允许：{} 秒）",
                    duration,
                    model,
                    rule.limits
                        .durations
                        .iter()
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                        .join(" / ")
                ));
            }
        }
        for (key, allowed) in [
            ("aspectRatio", &rule.limits.aspect_ratios),
            ("resolution", &rule.limits.resolutions),
        ] {
            if let Some(value) = provided
                .iter()
                .find(|(name, _)| name == key)
                .and_then(|(_, value)| value.as_str())
            {
                if !allowed.is_empty() && !allowed.iter().any(|item| item == value) {
                    return Err(format!(
                        "[limit_exceeded] {} '{}' 不被模型 '{}' 支持（允许：{}）",
                        key,
                        value,
                        model,
                        allowed.join(" / ")
                    ));
                }
            }
        }

        // ── 构造请求体 ──
        // 路径里已嵌入模型名时（如 Veo 的 {model}），请求体不再重复写 model
        let mut body = json!({});
        if !rule.transport.create.contains("{model}") {
            body["model"] = json!(model);
        }
        let args: Map<String, Value> = provided.iter().cloned().collect();
        let ctx = TransformCtx { args: &args };
        let mut echo_fields: Vec<Value> = Vec::new();
        let mut echo_transforms = Map::new();
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
            write_mapping(&mut body, mapping, transformed.clone())
                .map_err(|error| format!("[manifest_invalid] 参数 {}: {}", name, error))?;
            echo_fields.push(json!(name));
            if let Some(transform) = &mapping.transform {
                echo_transforms.insert(
                    name.clone(),
                    json!(format!(
                        "{} => {}",
                        transform.kind(),
                        value_summary(&transformed)
                    )),
                );
            }
        }

        let absent: Vec<&str> = MEDIA_SLOTS
            .iter()
            .chain(SCALAR_SLOTS.iter())
            .copied()
            .filter(|name| !provided.iter().any(|(key, _)| key == name))
            .collect();
        let echo = json!({
            "manifestVersion": manifest.version,
            "manifestSource": effective.source.as_str(),
            "matchedRule": rule.match_pattern,
            "createPath": rule.transport.create,
            "verified": verified,
            "fields": echo_fields,
            "transforms": echo_transforms,
            "ignored": ignored,
            "absent": absent,
        });

        // ── 交给通用执行器 ──
        let output_dir = PathBuf::from(&self.cwd).join("generated-videos");
        let progress = |message: &str| self.emit_progress(message);
        let result = video_transport::execute(
            &Execution {
                base_url: &base,
                api_key: &api_key,
                model: &model,
                body: &body,
                parts: &[],
                rule,
                output_dir: &output_dir,
                label: "视频",
            },
            &progress,
        )
        .await
        .map_err(|error| format!("[provider_rejected] {}\n\nrequestEcho:\n{}", error, echo))?;

        if let Some(url) = &result.video_url {
            return Ok(format!(
                "视频已生成（任务 {}）：\n![视频](<{}>)\n\nrequestEcho:\n{}",
                result.task_id.as_deref().unwrap_or("-"),
                url,
                echo
            ));
        }
        if let Some(path) = &result.local_path {
            return Ok(format!(
                "视频已生成，已保存到本地：{}\n\nrequestEcho:\n{}",
                path, echo
            ));
        }
        Err(format!(
            "[provider_rejected] 任务已结束但没有可用的视频结果\n\nrequestEcho:\n{}",
            echo
        ))
    }
}
