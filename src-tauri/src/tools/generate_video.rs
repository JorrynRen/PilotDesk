//! 视频生成工具：调用 OpenAI 兼容的异步视频任务 API。
//!
//! 工作流（异步任务，服务商通用）：
//! 1. `POST {base}/v1/videos` 创建任务 → 返回任务标识（`task_id`/`video_id`/`id`）
//! 2. 轮询 `GET {base}/v1/videos/{task_id}` 直至 `completed`
//! 3. 从响应取视频 URL（兼容 `video_url` / `remixed_from_video_id` 等常见字段），返回 Markdown 视频链接
//!
//! 复用当前 API Provider 的 endpoint 与 API Key，无需额外配置；
//! 跨提供商经 `resolve_provider` 解析（key 只用于请求构造，不进返回值/LLM 上下文）。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use base64::Engine;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Emitter;

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
}

impl GenerateVideoTool {
    pub fn new(
        api_endpoint: String,
        api_key: String,
        resolve: Option<Arc<dyn Fn(&str) -> Option<(String, String, String)> + Send + Sync>>,
        get_models: Option<Arc<dyn Fn(String) -> Vec<String> + Send + Sync>>,
        app: Option<tauri::AppHandle>,
        session_id: String,
    ) -> Self {
        Self { api_endpoint, api_key, resolve, get_models, app, session_id }
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
            let _ = app.emit("agent-tool-progress", serde_json::json!({
                "sessionId": self.session_id,
                "toolName": "generate_video",
                "message": message,
            }));
        }
        log::info!("[GenerateVideo] 进度: {}", message);
    }
}

/// 图片来源归一化：公网 URL（清洗）或本地路径 → base64 Data URI。
/// 图生视频要求公网可访问 URL；本地文件转 Data URI 由服务端决定是否支持（不支持时返回上游错误）。
async fn normalize_image_source(source: &str) -> Result<String, String> {
    if source.starts_with("data:") {
        return Ok(source.to_string());
    }
    if source.contains("http://") || source.contains("https://") {
        return crate::tools::image_common::normalize_http_url(source)
            .ok_or_else(|| "无法识别图片 URL".to_string());
    }
    let bytes = std::fs::read(source).map_err(|e| format!("读取图片文件失败: {}", e))?;
    let format = image::guess_format(&bytes).map_err(|e| format!("无法识别图片格式: {}", e))?;
    let mime = format.to_mime_type();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", mime, b64))
}

#[async_trait]
impl ToolHandler for GenerateVideoTool {
    fn name(&self) -> &str {
        "generate_video"
    }

    fn description(&self) -> &str {
        "生成视频（文生视频 / 图生视频）。通过 OpenAI 兼容的异步视频任务 API：创建任务后自动轮询直至完成（视频生成通常需 1~5 分钟）。模型需经 list_models 选择支持视频生成的模型。若连续 2 次调用失败（如 HTTP 4xx/5xx、当前模型不支持视频生成），请停止自动重试，向用户确认三选一：继续使用当前模型重试、提供新的 model 名称后重试、或跳过此工具调用。"
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
                    "description": "视频生成模型名（必填，原样复制自 list_models 清单，含完整 namespace 前缀，禁止自行缩短或编造）"
                },
                "provider": {
                    "type": "string",
                    "description": "可选：目标提供商 id（先用 list_models 查看可用提供商）；省略时使用当前会话提供商。跨提供商时需与 model 配合指定。"
                },
                "image": {
                    "type": "string",
                    "description": "可选：图生视频的参考图片。公网 URL 或本地绝对路径；建议使用公网可访问的 URL。"
                },
                "width": {
                    "type": "integer",
                    "description": "视频宽度，默认由服务商决定（如 1152；具体支持范围取决于模型）"
                },
                "height": {
                    "type": "integer",
                    "description": "视频高度，默认由服务商决定（如 768；具体支持范围取决于模型）"
                },
                "num_frames": {
                    "type": "integer",
                    "description": "总帧数（≤441，需满足 8n+1，默认 241）。时长 = num_frames / frame_rate，如 121 帧@24fps≈5 秒、241 帧@24fps≈10 秒"
                },
                "frame_rate": {
                    "type": "integer",
                    "description": "帧率 1~60，默认 24"
                }
            },
            "required": ["prompt", "model"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Video, ToolTag::Write, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let prompt = arguments["prompt"].as_str().ok_or("缺少 prompt 参数")?;
        let model = arguments["model"]
            .as_str()
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty())
            .ok_or("缺少 model 参数：请先调用 list_models 查看可用模型，从清单中复制完整的模型名（含 namespace 前缀）传入，不要自行编造")?;

        // 图生视频/关键帧：归一化参考图（URL 清洗 / 本地转 Data URI）
        let image_ref = match arguments["image"]
            .as_str()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            Some(src) => Some(normalize_image_source(src).await?),
            None => None,
        };

        let width = arguments["width"].as_u64().map(|v| v as i64);
        let height = arguments["height"].as_u64().map(|v| v as i64);
        let num_frames = arguments["num_frames"].as_u64().map(|v| v as i64);
        let frame_rate = arguments["frame_rate"].as_u64().map(|v| v as i64);

        // 跨提供商解析（key 仅用于请求构造，不进返回值）
        let provider_arg = arguments["provider"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let (endpoint_base, api_key) = match (&provider_arg, &self.resolve) {
            (Some(pid), Some(resolve)) => match resolve(pid) {
                Some((ep, key, _fmt)) => (ep, key),
                None => return Err(format!("提供商 [@{}] 不存在或未配置，请先用 list_models 查看可用提供商", pid)),
            },
            _ => (self.api_endpoint.clone(), self.api_key.clone()),
        };
        let base = endpoint_base.trim_end_matches('/').to_string();

        // 校验 model 是否在合法清单内
        let provider_id = provider_arg.as_deref().unwrap_or("__default__");
        self.validate_model(&model, provider_id)?;

        // 1. 创建视频任务（endpoint_base 已含 /v1，与图片/音频工具一致）
        let create_url = format!("{}/videos", base);
        let mut body = serde_json::json!({
            "model": model,
            "prompt": prompt,
        });
        if let Some(img) = image_ref {
            body["image"] = serde_json::json!(img);
        }
        if let Some(w) = width {
            body["width"] = serde_json::json!(w);
        }
        if let Some(h) = height {
            body["height"] = serde_json::json!(h);
        }
        if let Some(nf) = num_frames {
            body["num_frames"] = serde_json::json!(nf);
        }
        if let Some(fr) = frame_rate {
            body["frame_rate"] = serde_json::json!(fr);
        }

        let client = reqwest::Client::new();
        let resp = client
            .post(&create_url)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("视频任务创建请求失败: {}", e))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("读取响应失败: {}", e))?;
        if !status.is_success() {
            return Err(format!("视频任务创建失败 (HTTP {}): {}", status.as_u16(), text));
        }
        let task_json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {}", e))?;
        // 任务标识：优先 task_id，其次 video_id / id
        let task_id = task_json["task_id"]
            .as_str()
            .or_else(|| task_json["video_id"].as_str())
            .or_else(|| task_json["id"].as_str())
            .ok_or_else(|| format!("响应缺少任务标识 (task_id/video_id): {}", text))?
            .to_string();
        log::info!("[GenerateVideo] 任务已创建: task_id={}, prompt_len={}", task_id, prompt.len());
        // 立即告知用户任务已创建（弥补生成期间空白期）
        self.emit_progress("视频生成任务已创建，正在生成中，请耐心等待…");

        // 2. 轮询任务结果（endpoint_base 已含 /v1）
        //    完成/失败以服务端响应为准：响应已含视频 URL 即视为完成（不依赖 status 字段命名），
        //    失败状态及时返回；处理中/排队继续轮询；本地超时仅作兜底。
        let poll_url = format!("{}/videos/{}", base, task_id);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(900);
        let mut last_report = Instant::now();
        let started = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if tokio::time::Instant::now() > deadline {
                return Err("视频生成超时（超过 15 分钟），任务可能仍在处理中。".to_string());
            }
            // 每 30 秒向前端输出一次状态提示
            if last_report.elapsed() >= Duration::from_secs(30) {
                self.emit_progress(&format!(
                    "视频生成还在进行，请耐心等待…（已等待约 {} 秒）",
                    started.elapsed().as_secs()
                ));
                last_report = Instant::now();
            }
            let resp = client
                .get(&poll_url)
                .bearer_auth(&api_key)
                .send()
                .await
                .map_err(|e| format!("查询任务状态失败: {}", e))?;
            let status_code = resp.status();
            let body_text = resp
                .text()
                .await
                .map_err(|e| format!("读取任务状态失败: {}", e))?;
            if !status_code.is_success() {
                return Err(format!("查询任务状态失败 (HTTP {}): {}", status_code.as_u16(), body_text));
            }
            let j: serde_json::Value = serde_json::from_str(&body_text)
                .map_err(|e| format!("解析任务状态失败: {}", e))?;

            // 1) 响应已含视频 URL（兼容 video_url / remixed_from_video_id / result / data 嵌套）→ 完成
            if let Some(url) = extract_video_url(&j) {
                log::info!("[GenerateVideo] 任务 {} 已完成", task_id);
                // 尖括号界定 URL 边界（与 generate_image 一致，防 LLM 提取截断）
                return Ok(format!(
                    "视频已生成（任务 {}）：\n![视频](<{}>)",
                    task_id, url
                ));
            }
            // 2) 任务状态（官方枚举：queued / in_progress / completed / failed；兼容其他服务商变体）
            let st = extract_status(&j).unwrap_or_default();
            log::info!("[GenerateVideo] 任务 {} 状态: {}", task_id, st);
            match st.as_str() {
                // 已完成但未提取到 URL（上方 extract_video_url 已优先处理）→ 明确报错，避免死等超时
                "completed" | "succeeded" | "success" | "done" | "finished" | "complete" => {
                    return Err(format!(
                        "任务已完成（状态 {}）但响应缺少视频 URL: {}",
                        st, body_text
                    ));
                }
                "failed" | "error" | "cancelled" | "canceled" | "rejected" | "failure" => {
                    return Err(format!("视频生成失败（状态 {}）: {}", st, body_text));
                }
                // queued（排队等待）/ in_progress（生成中）/ 状态缺失 → 继续轮询
                "queued" | "in_progress" | "processing" | "pending" | "running" | "" => {}
                // 其他未知状态：记录日志后继续轮询（本地超时兜底，不误判）
                other => {
                    log::warn!("[GenerateVideo] 未知任务状态: {}", other);
                }
            }
        }
    }
}

/// 从任务响应中提取视频 URL（兼容字段与嵌套层级）。
fn extract_video_url(j: &serde_json::Value) -> Option<&str> {
    ["video_url", "remixed_from_video_id", "output", "url"]
        .iter()
        .find_map(|k| j[k].as_str())
        .or_else(|| j["result"]["video_url"].as_str())
        .or_else(|| j["data"]["video_url"].as_str())
        .or_else(|| j["task"]["video_url"].as_str())
}

/// 从任务响应中提取任务状态（小写；兼容顶层 / result / data / task 嵌套）。
fn extract_status(j: &serde_json::Value) -> Option<String> {
    ["status", "task_status"]
        .iter()
        .find_map(|k| j[k].as_str())
        .or_else(|| j["result"]["status"].as_str())
        .or_else(|| j["data"]["status"].as_str())
        .or_else(|| j["task"]["status"].as_str())
        .map(|s| s.to_lowercase())
}
