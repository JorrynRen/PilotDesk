//! 图片生成工具：统一文生图、图生图与图像编辑。
//!
//! 模型差异（端点、编码方式、字段名、结果取值、限制）全部写在
//! `resources/image-models.json` 里，由 [`image_manifest`] 解释、
//! [`video_transport`] 执行；本文件不包含任何厂商字段名。
//!
//! 发送前的本地校验：不支持的参数直接报错，绝不静默丢弃；不支持的组合直接拒绝。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::tools::image_common::resolve_image_bytes;
use crate::tools::image_manifest::{self, SPEC};
use crate::tools::tool_manifest::{apply_transform, write_mapping, TransformCtx};
use crate::tools::video_transport::{self, Execution, FormPart};
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

/// 图片生成工具
pub struct GenerateImageTool {
    api_endpoint: String,
    api_key: String,
    /// 跨 provider 解析（providerId → (endpoint, key, api_format)）；None 表示仅支持当前会话提供商。
    /// key 只用于请求构造，绝不进入工具返回值/LLM 上下文。
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    /// 当前会话提供商 id（与 self.api_endpoint/api_key 对应；None 表示未知）。
    #[allow(dead_code)]
    current_provider: Option<String>,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
    /// 工作目录（`b64_json` 结果落盘用）
    cwd: String,
}

impl GenerateImageTool {
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        current_provider: Option<String>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
        cwd: String,
    ) -> Self {
        Self {
            api_endpoint,
            api_key,
            resolve,
            current_provider,
            get_models,
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
                    model, provider_id, valid.join(", ")
                ));
            }
        }
        Ok(())
    }
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[async_trait]
impl ToolHandler for GenerateImageTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        "生成或编辑图片。纯文生图时只传 prompt；传入 referenceImages 时进行图生图/图像编辑/变体，mask 仅编辑类模型支持。端点与字段由图片模型清单决定（默认 /images/generations，编辑类模型走 /images/edits）。模型需经 list_models 选择支持图片生成的模型；不确定能力时先调用 list_image_capabilities。若连续 2 次调用失败（如 HTTP 4xx/5xx、当前模型不支持图片生成），请停止自动重试，向用户确认三选一：继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "图片描述（英文效果最佳，尽可能详细）"
                },
                "provider": {
                    "type": "string",
                    "description": "必填：目标提供商 id，逐字取 list_models 清单里的 provider_id（标 ★ 的是当前会话提供商）；provider 与 model 必须分别传入，禁止拼成 provider_id/model"
                },
                "size": {
                    "type": "string",
                    "description": "输出尺寸，如 1024x1024、1792x1024、1K、2K 等，默认 1024x1024（具体支持取决于模型）"
                },
                "model": {
                    "type": "string",
                    "description": "图片生成模型名（必填，原样复制自 list_models 清单；若模型名自身含斜杠/命名空间（如 TeleAI/xxx），必须原样保留；禁止自行缩短、编造，也禁止把 provider_id 拼进来）"
                },
                "n": {
                    "type": "integer",
                    "description": "生成数量，默认 1，上限由模型清单决定"
                },
                "referenceImages": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "可选：图生图/图像编辑/图像变体时传入的参考图片数组。每项可为公网 URL、base64 Data URI 或本地绝对路径。省略则为纯文生图。"
                },
                "image": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "referenceImages 的兼容别名，二者择一即可。"
                },
                "mask": {
                    "type": "string",
                    "description": "可选：遮罩图片（URL、Data URI 或本地路径），仅编辑类模型支持，且必须与 referenceImages 同时传入。"
                }
            },
            "required": ["prompt", "model", "provider"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Image, ToolTag::Write, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let prompt = arguments["prompt"].as_str().ok_or("缺少 prompt 参数")?;
        // 模型名必须显式提供（各服务商可用模型不同，无通用默认值）：剥离前导 @ 与空白后为空则报错引导。
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名（含 namespace 前缀）传入，不要自行编造")?;

        // 跨提供商解析：指定 provider 时运行时查库取 endpoint/key（key 仅用于请求构造，不进返回值）。
        let provider_arg = arguments["provider"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // provider 与 model 必须分别传入：缺 provider 时不再静默回退会话提供商，避免把别家的模型打到会话端点上
        if provider_arg.is_none() {
            return Err("缺少 provider 参数：provider 与 model 必须分别传，禁止写成 provider_id/model。请先调用 list_models，从清单里挑出目标模型所在那一行（标 ★ 的是当前会话提供商），把 provider_id 与模型名分别填入 provider / model".to_string());
        }
        let provider = provider_arg.unwrap();
        let (endpoint_base, api_key) = match &self.resolve {
            Some(resolve) => match resolve(&provider) {
                Some((ep, key, _fmt)) => (ep, key),
                None => {
                    return Err(format!(
                        "提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商",
                        provider
                    ))
                }
            },
            None => (self.api_endpoint.clone(), self.api_key.clone()),
        };
        self.validate_model(&model, &provider)?;

        // ── 选规则 ──
        let effective =
            image_manifest::snapshot().map_err(|error| format!("[manifest_invalid] {}", error))?;
        let manifest = &effective.manifest;
        let (rule, verified) = manifest
            .select(&model)
            .ok_or_else(|| format!("[manifest_invalid] 模型 '{}' 未命中任何规则", model))?;

        // ── 收集参数 ──
        let mut provided: Vec<(String, Value)> = vec![("prompt".to_string(), json!(prompt))];
        let size = arguments["size"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("1024x1024");
        provided.push(("size".to_string(), json!(size)));
        let max_n = rule.limits.max_n.unwrap_or(4).max(1);
        let n = arguments["n"].as_u64().unwrap_or(1).clamp(1, max_n as u64);
        provided.push(("n".to_string(), json!(n)));

        let mut references = string_list(&arguments["referenceImages"]);
        if references.is_empty() {
            references = string_list(&arguments["image"]);
        }
        if !references.is_empty() {
            provided.push((
                "referenceImages".to_string(),
                json!(references.clone()),
            ));
        }
        if let Some(mask) = arguments["mask"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            provided.push(("mask".to_string(), json!(mask)));
        }

        let has = |name: &str| provided.iter().any(|(key, _)| key == name);

        // ── 依赖与限制（写在代码里，清单不能改变）──
        if has("mask") && !has("referenceImages") {
            return Err(
                "[dependency_violation] mask 必须与 referenceImages 同时传入".to_string(),
            );
        }
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
        if let Some(limit) = rule.limits.reference_images {
            if references.len() > limit {
                return Err(format!(
                    "[limit_exceeded] referenceImages 最多 {} 张，本次传入 {} 张（模型 '{}'）",
                    limit,
                    references.len(),
                    model
                ));
            }
        }
        if !rule.limits.sizes.is_empty() {
            if let Some(size) = provided
                .iter()
                .find(|(key, _)| key == "size")
                .and_then(|(_, value)| value.as_str())
            {
                if !rule.limits.sizes.iter().any(|item| item == size) {
                    return Err(format!(
                        "[limit_exceeded] 尺寸 '{}' 不被模型 '{}' 支持（允许：{}）",
                        size,
                        model,
                        rule.limits.sizes.join(" / ")
                    ));
                }
            }
        }

        // ── 构造请求 ──
        let args: Map<String, Value> = provided.iter().cloned().collect();
        let ctx = TransformCtx { args: &args };
        let multipart = rule.transport.body == image_manifest::BodyKind::Multipart;

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
                    let sources = if value.is_array() {
                        string_list(value)
                    } else {
                        value.as_str().map(|s| vec![s.to_string()]).unwrap_or_default()
                    };
                    for (index, source) in sources.iter().enumerate() {
                        let bytes = resolve_image_bytes(source)
                            .await
                            .map_err(|error| format!("[transform_failed] 参数 {}: {}", name, error))?;
                        let name = if sources.len() > 1 {
                            format!("{}[{}]", field, index)
                        } else {
                            field.clone()
                        };
                        parts.push(FormPart::Png { name, bytes });
                    }
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

        let absent: Vec<&str> = SPEC
            .slots
            .iter()
            .copied()
            .filter(|name| !provided.iter().any(|(key, _)| key == name))
            .collect();
        let echo = json!({
            "manifestVersion": manifest.version,
            "manifestSource": effective.source.as_str(),
            "matchedRule": rule.match_pattern,
            "createPath": rule.transport.create,
            "body": rule.transport.body,
            "output": rule.output,
            "verified": verified,
            "fields": echo_fields,
            "absent": absent,
        });

        // ── 交给通用执行器 ──
        let output_dir = PathBuf::from(&self.cwd).join("generated-images");
        let base = endpoint_base.trim_end_matches('/').to_string();
        let noop = |_: &str| {};
        let result = video_transport::execute(
            &Execution {
                base_url: &base,
                api_key: &api_key,
                model: &model,
                body: &body,
                parts: &parts,
                rule,
                output_dir: &output_dir,
                label: "图片",
            },
            &noop,
        )
        .await
        .map_err(|error| format!("[provider_rejected] {}\n\nrequestEcho:\n{}", error, echo))?;

        if result.urls.is_empty() {
            return Err(format!(
                "[provider_rejected] 任务已结束但没有可用的图片结果\n\nrequestEcho:\n{}",
                echo
            ));
        }
        let images: Vec<String> = result
            .urls
            .iter()
            .enumerate()
            .map(|(index, url)| format!("![图片{}](<{}>)", index + 1, url))
            .collect();
        Ok(format!(
            "已生成 {} 张图片：\n{}\n\nrequestEcho:\n{}",
            images.len(),
            images.join("\n"),
            echo
        ))
    }
}
