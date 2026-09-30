//! 图片读取工具：把图片交给视觉模型读取，返回其文本结果与图片的绝对路径/URL。
//!
//! 设计要点（模型/端点解析沿用 generate_image 的做法）：
//! - 复用会话 provider 的 endpoint/key 发起一次**独立的非流式多模态调用**，图片字节
//!   不进入主会话上下文；只有文本结果与图片绝对路径/URL 回到主循环。
//! - 模型选择：显式 `model` > 会话模型；跨 provider 由 `provider` 参数经 `resolve` 解析。
//! - 视觉能力不做装配期预判：直接实测。若会话模型不接受图片输入，**工具内自动回退一次**
//!   ——按用户在模型说明中显式声明的图片输入支持挑一个候选模型再试一次——不依赖主模型
//!   自己换模型重试。两次都失败才返回引导文案。
//! - 全部失败被工具结果隔离：AgentLoop 的 Err 分支只追加一条 tool 消息，不中断整轮对话。

use crate::api_agent::client::ApiClient;
use crate::api_agent::types::{ApiFormat, ChatMessage, ChatRequest};
use crate::tools::deps::expand_user_path;
use crate::tools::image_common;
use crate::tools::{ProviderModelInfo, RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use base64::Engine;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 送入视觉模型前的最长边上限（像素）。超过则等比缩小，避免超大截图被服务商拒绝。
const MAX_EDGE: u32 = 1568;

/// 未提供 `question` 时的默认读取指令。
const DEFAULT_QUESTION: &str = "请阅读这张图片并完整描述其内容；若图片中包含文字，请逐字转述。";

/// 模型说明中声明「支持图片输入」的关键短语（小写匹配）。
///
/// 只收录**输入能力**措辞，刻意不含 `image`/`图片` 这类裸词：图片生成模型（dall-e、flux 等）
/// 的备注通常就是「图片生成」，裸词会把它们误选为回退候选。
const IMAGE_INPUT_TOKENS: [&str; 10] = [
    "vision",
    "视觉",
    "读图",
    "图片输入",
    "图像输入",
    "图片理解",
    "图像理解",
    "多模态",
    "multimodal",
    "image input",
];

/// 判定模型说明是否显式声明「支持图片输入」。
///
/// 反向描述（「不支持图片」等）一律不命中，避免把禁用说明误读为能力声明。
fn note_declares_image_input(note: &str) -> bool {
    let n = note.to_lowercase();
    if n.contains("不支持") || n.contains("not support") || n.contains("unsupported") {
        return false;
    }
    IMAGE_INPUT_TOKENS.iter().any(|t| n.contains(t))
}

/// 视觉调用失败的分类结果。
struct VisionError {
    /// 是否属于「当前模型不接受图片输入」。仅这类失败值得换模型重试——网络/鉴权/限流
    /// 换模型不会更好，原样返回。
    image_unsupported: bool,
    message: String,
}

/// 自动回退候选（来自 list_models 的 provider/模型清单；不含 key）。
struct FallbackCandidate {
    provider_id: String,
    model: String,
    endpoint: String,
}

/// 图片读取工具
pub struct ReadImageTool {
    /// 会话 provider 的 API 地址（视觉调用复用）
    api_endpoint: String,
    api_key: String,
    /// 会话 provider 的协议格式（决定走 chat / chat_anthropic）
    api_format: ApiFormat,
    /// 会话模型名（`model` 参数缺省时使用）
    session_model: String,
    /// 跨 provider 解析（providerId → (endpoint, key, api_format)）；None 表示仅当前会话提供商。
    /// key 只用于请求构造，绝不进入工具返回值/LLM 上下文。
    resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
    /// 模型白名单校验：provider_id → Vec<合法模型名>。None 时跳过校验。
    get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
    /// 回退候选数据源（provider/模型/说明；不含 key）。
    list_providers: Option<Arc<dyn Fn() -> Vec<ProviderModelInfo> + Send + Sync>>,
    /// 工作目录（相对路径基准）
    cwd: String,
}

impl ReadImageTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        api_endpoint: String,
        api_key: String,
        api_format: ApiFormat,
        session_model: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
        list_providers: Option<Arc<dyn Fn() -> Vec<ProviderModelInfo> + Send + Sync>>,
        cwd: String,
    ) -> Self {
        Self {
            api_endpoint,
            api_key,
            api_format,
            session_model,
            resolve,
            get_models,
            list_providers,
            cwd,
        }
    }

    /// 校验 model 是否在指定 provider 的合法模型列表中，不在则返回引导错误。
    fn validate_model(&self, model: &str, provider_id: &str) -> Result<(), String> {
        if let Some(get_models) = &self.get_models {
            let valid = get_models(provider_id.to_string());
            if !valid.is_empty() && !valid.iter().any(|m| m == model) {
                return Err(format!(
                    "模型 '{}' 不是提供商 '@{}' 的有效模型。有效模型：{}\n\
                     请重新调用 list_models，从返回结果中完整复制模型名后重试。",
                    model,
                    provider_id,
                    valid.join(", ")
                ));
            }
        }
        Ok(())
    }

    /// 发起一次视觉调用。成功返回模型对图片的文本读取结果。
    async fn attempt_vision(
        &self,
        endpoint: &str,
        api_key: &str,
        format: &ApiFormat,
        model: &str,
        question: &str,
        data_url: &str,
    ) -> Result<String, VisionError> {
        let client = ApiClient::new(endpoint.to_string(), api_key.to_string(), format.clone());
        let request = ChatRequest {
            model: model.to_string(),
            messages: vec![ChatMessage::user_with_images(
                question,
                vec![data_url.to_string()],
            )],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            max_tokens: None,
            response_format: None,
        };
        let response = match format {
            ApiFormat::Anthropic => client.chat_anthropic(&request).await,
            ApiFormat::OpenAI => client.chat(&request).await,
        }
        .map_err(|e| classify_vision_error(&e, model))?;

        let text = response.content.trim().to_string();
        if text.is_empty() {
            return Err(VisionError {
                image_unsupported: true,
                message: format!("模型 '{}' 未返回图片内容（可能不支持图片输入）。", model),
            });
        }
        Ok(text)
    }

    /// 挑选自动回退候选：仅取模型说明中显式声明支持图片输入的模型，
    /// 与会话 provider 同 endpoint 的候选优先，其次才是其他 provider。
    fn pick_fallback_candidate(&self, exclude_model: &str) -> Option<FallbackCandidate> {
        let list = self.list_providers.as_ref()?;
        let mut same_endpoint: Option<FallbackCandidate> = None;
        let mut other_endpoint: Option<FallbackCandidate> = None;

        for p in list() {
            for m in &p.models {
                if m.name == exclude_model {
                    continue;
                }
                if !m
                    .description
                    .as_deref()
                    .map_or(false, note_declares_image_input)
                {
                    continue;
                }
                let cand = FallbackCandidate {
                    provider_id: p.provider_id.clone(),
                    model: m.name.clone(),
                    endpoint: p.endpoint.clone(),
                };
                let same =
                    p.endpoint.trim_end_matches('/') == self.api_endpoint.trim_end_matches('/');
                if same {
                    if same_endpoint.is_none() {
                        same_endpoint = Some(cand);
                    }
                } else if other_endpoint.is_none() {
                    other_endpoint = Some(cand);
                }
            }
        }
        same_endpoint.or(other_endpoint)
    }

    /// 解析候选 provider 的调用凭据。同 endpoint 的候选直接复用会话凭据，
    /// 其余经 `resolve` 运行时查库（key 只进请求构造）。
    fn credentials_for(&self, cand: &FallbackCandidate) -> Option<(String, String, ApiFormat)> {
        if cand.endpoint.trim_end_matches('/') == self.api_endpoint.trim_end_matches('/') {
            return Some((
                self.api_endpoint.clone(),
                self.api_key.clone(),
                self.api_format.clone(),
            ));
        }
        let resolve = self.resolve.as_ref()?;
        resolve(&cand.provider_id)
            .map(|(ep, key, fmt)| (ep, key, fmt.parse::<ApiFormat>().unwrap_or_default()))
    }
}

/// 相对路径按工作目录解析；`~/`、`%USERPROFILE%` 前缀先展开。
fn resolve_local_path(cwd: &str, raw: &str) -> PathBuf {
    let expanded = expand_user_path(raw);
    let p = Path::new(&expanded);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(cwd).join(&expanded)
    }
}

/// 计算等比缩小后的尺寸；未超过上限时返回 None（表示无需缩放）。
fn fit_dimensions(w: u32, h: u32, max_edge: u32) -> Option<(u32, u32)> {
    let longest = w.max(h);
    if longest == 0 || longest <= max_edge {
        return None;
    }
    let scale = max_edge as f64 / longest as f64;
    Some((
        ((w as f64) * scale).round().max(1.0) as u32,
        ((h as f64) * scale).round().max(1.0) as u32,
    ))
}

/// 解码任意格式图片 → 需要时等比缩小 → 编码为 PNG，并校验体积上限。
fn normalize_to_png(raw: Vec<u8>) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(&raw)
        .map_err(|e| format!("无法解析图片（可能不是有效的图片格式）: {}", e))?;
    let img = match fit_dimensions(img.width(), img.height(), MAX_EDGE) {
        Some((w, h)) => img.resize_exact(w, h, image::imageops::FilterType::Lanczos3),
        None => img,
    };
    let mut png: Vec<u8> = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败: {}", e))?;
    if png.len() > image_common::MAX_IMAGE_BYTES {
        return Err(format!(
            "图片为 {} 字节，超过 {} MB 上限，请先缩小图片后重试",
            png.len(),
            image_common::MAX_IMAGE_BYTES / 1024 / 1024
        ));
    }
    Ok(png)
}

/// 视觉调用失败文案：区分「模型不支持图片输入」与「模型不可用」，其余原样透出。
///
/// 判定依据是服务商返回的错误文本（外部边界，只能按文案识别）。
fn classify_vision_error(err: &str, model: &str) -> VisionError {
    let lower = err.to_lowercase();
    let mentions_image = lower.contains("image_url")
        || lower.contains("image input")
        || lower.contains("image content")
        || lower.contains("图片")
        || lower.contains("图像");
    let unsupported = mentions_image
        && (lower.contains("not support")
            || lower.contains("unsupported")
            || lower.contains("不支持")
            || lower.contains("invalid")
            || lower.contains("cannot")
            || lower.contains("无法"));
    if unsupported {
        return VisionError {
            image_unsupported: true,
            message: format!(
                "模型 '{}' 不支持图片输入。请先调用 list_models 查看模型清单，\
                 选择一个支持图片输入的模型，用 model 参数（或先切换会话模型）后重试。\
                 原始错误：{}",
                model, err
            ),
        };
    }
    let model_missing = lower.contains("model")
        && (lower.contains("not found")
            || lower.contains("not exist")
            || lower.contains("不存在")
            || lower.contains("no such"));
    if model_missing {
        return VisionError {
            image_unsupported: false,
            message: format!(
                "模型 '{}' 不可用：{}。请先调用 list_models 确认可用的模型名。",
                model, err
            ),
        };
    }
    VisionError {
        image_unsupported: false,
        message: format!("读取图片失败：{}", err),
    }
}

/// 成功结果：回显图片绝对路径/URL（供后续图生图复用），并附视觉模型的读取文本。
fn render_success(display_ref: &str, model: &str, text: &str) -> String {
    format!(
        "已读取图片：{}\n（如需图生图/图像编辑，可把该引用作为 generate_image 的 image 参数传入）\n\n\
         <image_content model=\"{}\">\n{}\n</image_content>",
        display_ref, model, text
    )
}

#[async_trait]
impl ToolHandler for ReadImageTool {
    fn name(&self) -> &str {
        "read_image"
    }

    fn description(&self) -> &str {
        "读取一张图片的内容。支持本地路径（绝对/相对）、http(s) URL 与 base64 data URL。\
图片会交给视觉模型识别，返回文本结果与图片的绝对路径/URL（该路径可作为 generate_image 的 \
image 参数用于图生图/图像编辑）。默认使用当前会话模型；会话模型不支持图片输入时会自动改用其他\
声明支持图片的模型重试一次，若仍失败则返回提示。注意：返回的是模型对图片的文本理解，需要像素级\
操作时请配合 generate_image。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "图片路径（绝对/相对）或 http(s) URL 或 base64 data URL"
                },
                "question": {
                    "type": "string",
                    "description": "可选：希望模型针对这张图片回答的问题；省略则请求完整描述并转述图中文字"
                },
                "model": {
                    "type": "string",
                    "description": "可选：执行读取的视觉模型名（原样复制自 list_models；自身含斜杠/命名空间则原样保留）；省略时使用当前会话模型；禁止把 provider_id 拼进模型名"
                },
                "provider": {
                    "type": "string",
                    "description": "可选：目标提供商 id（先用 list_models 查看）；省略时使用当前会话提供商。若要指定别的提供商，请把 provider 与 model 都填上，禁止拼成 provider_id/model"
                }
            },
            "required": ["path"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Filesystem, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let raw_path = arguments["path"]
            .as_str()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 path 参数：请提供图片的绝对/相对路径或 http(s) URL")?;
        let question = arguments["question"]
            .as_str()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_QUESTION);

        // ── 1. 归类来源 + 计算回显引用（绝对路径 / URL / 内联标记）──
        let is_data = raw_path.starts_with("data:");
        let is_url = raw_path.contains("http://") || raw_path.contains("https://");
        let (source, display_ref) = if is_data {
            (raw_path.to_string(), "[内联 base64 图片]".to_string())
        } else if is_url {
            let url = image_common::normalize_http_url(raw_path).ok_or("无法识别图片 URL")?;
            (url.clone(), url)
        } else {
            let abs = resolve_local_path(&self.cwd, raw_path);
            if !abs.is_file() {
                return Err(format!("图片文件不存在: {}", abs.display()));
            }
            let display = abs.canonicalize().unwrap_or_else(|_| abs.clone());
            (
                abs.to_string_lossy().into_owned(),
                display.to_string_lossy().into_owned(),
            )
        };

        // ── 2. 读取并归一化（等比缩小 → PNG → 体积上限）──
        let raw = image_common::load_raw_bytes(&source).await?;
        let png = normalize_to_png(raw)?;
        let data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );

        // ── 3. 解析端点与目标模型（显式 provider/model > 会话 provider/model）──
        let provider_arg = arguments["provider"]
            .as_str()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let (endpoint_base, api_key, api_format) = match (provider_arg, &self.resolve) {
            (Some(pid), Some(resolve)) => match resolve(pid) {
                Some((ep, key, fmt)) => (ep, key, fmt.parse::<ApiFormat>().unwrap_or_default()),
                None => {
                    return Err(format!(
                        "提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商",
                        pid
                    ))
                }
            },
            _ => (
                self.api_endpoint.clone(),
                self.api_key.clone(),
                self.api_format.clone(),
            ),
        };
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.session_model.clone());
        if model.trim().is_empty() {
            return Err(
                "缺少 model 参数：请先调用 list_models 选择一个支持图片输入的模型".to_string(),
            );
        }
        self.validate_model(&model, provider_arg.unwrap_or(""))?;

        // ── 4. 视觉调用（非流式；失败由 AgentLoop 的 Err 分支隔离，不中断整轮）──
        match self
            .attempt_vision(
                &endpoint_base,
                &api_key,
                &api_format,
                &model,
                question,
                &data_url,
            )
            .await
        {
            Ok(text) => Ok(render_success(&display_ref, &model, &text)),
            Err(primary) if primary.image_unsupported => {
                let Some(cand) = self.pick_fallback_candidate(&model) else {
                    return Err(primary.message);
                };
                let Some((ep, key, fmt)) = self.credentials_for(&cand) else {
                    return Err(primary.message);
                };
                log::info!(
                    "[read_image] 会话模型 '{}' 不支持图片输入，自动回退模型 '{}'（provider=@{}）",
                    model,
                    cand.model,
                    cand.provider_id
                );
                match self
                    .attempt_vision(&ep, &key, &fmt, &cand.model, question, &data_url)
                    .await
                {
                    Ok(text) => Ok(render_success(&display_ref, &cand.model, &text)),
                    Err(retry) => Err(format!(
                        "{}（已自动尝试模型 '{}' 仍失败：{}）",
                        primary.message, cand.model, retry.message
                    )),
                }
            }
            Err(other) => Err(other.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_dimensions_scales_down_and_keeps_aspect() {
        assert_eq!(fit_dimensions(3136, 1568, 1568), Some((1568, 784)));
        assert_eq!(fit_dimensions(800, 600, 1568), None);
        assert_eq!(fit_dimensions(1568, 1568, 1568), None);
        assert_eq!(fit_dimensions(1, 4000, 1568), Some((1, 1568)));
    }

    #[test]
    fn fit_dimensions_handles_degenerate_sizes() {
        assert_eq!(fit_dimensions(0, 0, 1568), None);
    }

    #[test]
    fn unsupported_image_error_is_marked_for_fallback() {
        let err = classify_vision_error(
            "HTTP 400: invalid content type image_url is not supported by this model",
            "text-only-model",
        );
        assert!(err.image_unsupported);
        assert!(err.message.contains("不支持图片输入"));
        assert!(err.message.contains("list_models"));
        assert!(err.message.contains("text-only-model"));
    }

    #[test]
    fn missing_model_error_does_not_trigger_fallback() {
        let err = classify_vision_error("HTTP 404: model not found", "ghost-model");
        assert!(!err.image_unsupported);
        assert!(err.message.contains("不可用"));
        assert!(err.message.contains("ghost-model"));
    }

    #[test]
    fn unrelated_error_passes_through_without_fallback() {
        let err = classify_vision_error("HTTP 500: internal error", "m");
        assert!(!err.image_unsupported);
        assert!(err.message.starts_with("读取图片失败"));
    }

    #[test]
    fn note_declares_image_input_matches_explicit_capability_phrases() {
        assert!(note_declares_image_input("支持视觉输入，可读图"));
        assert!(note_declares_image_input("多模态模型 (multimodal)"));
        assert!(note_declares_image_input("supports vision"));
        // 图片生成类备注（裸词 image/图片）不得命中，否则会误选为回退候选。
        assert!(!note_declares_image_input("图片生成模型"));
        assert!(!note_declares_image_input("dall-e 文生图"));
        assert!(!note_declares_image_input(""));
    }

    #[test]
    fn note_declares_image_input_rejects_negated_capability() {
        assert!(!note_declares_image_input("不支持图片输入"));
        assert!(!note_declares_image_input("not support image input"));
        assert!(!note_declares_image_input("unsupported vision"));
    }

    fn model(name: &str, note: &str) -> crate::tools::ModelSpec {
        crate::tools::ModelSpec {
            name: name.to_string(),
            description: Some(note.to_string()),
        }
    }

    fn provider(
        id: &str,
        endpoint: &str,
        models: Vec<crate::tools::ModelSpec>,
    ) -> ProviderModelInfo {
        ProviderModelInfo {
            provider_id: id.to_string(),
            provider_name: id.to_string(),
            endpoint: endpoint.to_string(),
            api_format: "openai".to_string(),
            is_session: false,
            models,
        }
    }

    fn tool_at(endpoint: &str, providers: Vec<ProviderModelInfo>) -> ReadImageTool {
        ReadImageTool::new(
            endpoint.to_string(),
            "session-key".to_string(),
            ApiFormat::OpenAI,
            "session-model".to_string(),
            None,
            None,
            Some(Arc::new(move || providers.clone())),
            ".".to_string(),
        )
    }

    #[test]
    fn fallback_prefers_same_endpoint_and_ignores_non_vision_notes() {
        let tool = tool_at(
            "https://a.example/v1",
            vec![
                provider(
                    "p1",
                    "https://a.example/v1",
                    vec![
                        model("text-only", "不支持图片输入"),
                        model("dall-e", "图片生成"),
                        model("vl-a", "支持视觉输入"),
                    ],
                ),
                provider("p2", "https://b.example/v1", vec![model("vl-b", "多模态")]),
            ],
        );
        let cand = tool
            .pick_fallback_candidate("session-model")
            .expect("同 endpoint 存在声明支持图片的模型");
        assert_eq!(cand.model, "vl-a");
        assert_eq!(cand.provider_id, "p1");
    }

    #[test]
    fn fallback_uses_other_provider_when_same_endpoint_has_no_candidate() {
        let tool = tool_at(
            "https://a.example/v1",
            vec![
                provider(
                    "p1",
                    "https://a.example/v1",
                    vec![model("dall-e", "图片生成")],
                ),
                provider(
                    "p2",
                    "https://b.example/v1",
                    vec![model("vl-b", "支持图片理解")],
                ),
            ],
        );
        let cand = tool
            .pick_fallback_candidate("session-model")
            .expect("应回退到其他 provider 的候选");
        assert_eq!(cand.model, "vl-b");
        assert_eq!(cand.provider_id, "p2");
    }

    #[test]
    fn fallback_skips_the_model_that_already_failed() {
        let tool = tool_at(
            "https://a.example/v1",
            vec![provider(
                "p1",
                "https://a.example/v1",
                vec![model("vl-a", "视觉")],
            )],
        );
        assert!(tool.pick_fallback_candidate("vl-a").is_none());
    }

    #[test]
    fn credentials_reuse_session_key_for_same_endpoint() {
        let tool = tool_at("https://a.example/v1", vec![]);
        let cand = FallbackCandidate {
            provider_id: "p1".to_string(),
            model: "vl-a".to_string(),
            // 尾斜杠差异不应影响「同一 provider」的判定
            endpoint: "https://a.example/v1/".to_string(),
        };
        let (endpoint, key, _) = tool
            .credentials_for(&cand)
            .expect("同 endpoint 应复用会话凭据");
        assert_eq!(endpoint, "https://a.example/v1");
        assert_eq!(key, "session-key");
    }

    #[test]
    fn credentials_absent_for_other_provider_without_resolver() {
        let tool = tool_at("https://a.example/v1", vec![]);
        let cand = FallbackCandidate {
            provider_id: "p2".to_string(),
            model: "vl-b".to_string(),
            endpoint: "https://b.example/v1".to_string(),
        };
        assert!(tool.credentials_for(&cand).is_none());
    }

    #[test]
    fn normalize_to_png_downscales_oversized_image() {
        let mut raw: Vec<u8> = Vec::new();
        image::DynamicImage::new_rgb8(4000, 1000)
            .write_to(&mut std::io::Cursor::new(&mut raw), image::ImageFormat::Png)
            .expect("构造测试图片");
        let out = normalize_to_png(raw).expect("应成功归一化");
        let decoded = image::load_from_memory(&out).expect("输出应是有效图片");
        assert_eq!(decoded.width(), MAX_EDGE);
        assert_eq!(decoded.height(), 392);
    }
}
