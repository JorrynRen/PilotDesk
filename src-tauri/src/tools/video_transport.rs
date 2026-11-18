//! 通用生成任务执行器：按清单里的传输描述拼路径、发请求、判状态、取结果。
//!
//! 本模块不含任何厂商字段名或端点，所有取值来自 [`ModelRule`]：
//! - `transport`：创建 / 轮询 / 内容路径、鉴权方式、请求体编码（json / multipart）
//! - `task`：任务标识候选路径、状态路径、成功与失败取值、错误路径
//! - `responseMode`：`url` 直接取地址，`binary` 拉取内容落盘
//! - `output`：`file` 落盘（视频默认），`markdown_url` 回传图片地址

use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::Value;

use super::video_manifest::{AuthKind, BodyKind, ModelRule, OutputKind, ResponseMode, TaskSpec};

/// 轮询间隔与整体超时
const POLL_INTERVAL_SECS: u64 = 3;
const POLL_TIMEOUT_SECS: u64 = 900;
const PROGRESS_INTERVAL_SECS: u64 = 30;

/// multipart 表单的一项
pub enum FormPart {
    Text { name: String, value: String },
    /// 已按 PNG 归一化的文件 part
    Png { name: String, bytes: Vec<u8> },
}

/// 执行入参
pub struct Execution<'a> {
    pub base_url: &'a str,
    pub api_key: &'a str,
    pub model: &'a str,
    /// JSON 请求体（`body: multipart` 时忽略）
    pub body: &'a Value,
    /// multipart 表单（`body: json` 时忽略）
    pub parts: &'a [FormPart],
    pub rule: &'a ModelRule,
    /// `binary` 模式与 `b64_json` 落盘目录
    pub output_dir: &'a Path,
    /// 错误与进度文案里的名词（"视频" / "图片"）
    pub label: &'a str,
}

/// 执行结果
pub struct ExecutionResult {
    pub task_id: Option<String>,
    pub video_url: Option<String>,
    pub local_path: Option<String>,
    /// `url` 模式下的全部结果（图片可能多张）
    pub urls: Vec<String>,
}

/// 任务状态判定
pub enum TaskState {
    Pending,
    Success,
    Failed(String),
}

/// 执行一次完整任务：创建 → 轮询 → 取结果。
pub async fn execute(
    execution: &Execution<'_>,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<ExecutionResult, String> {
    let label = execution.label;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    // ── 创建任务 ──
    let create_url = build_url(
        execution.base_url,
        &execution.rule.transport.create,
        execution.model,
        None,
    )?;
    let mut request = client.post(&create_url);
    if execution.rule.transport.body == BodyKind::Multipart {
        let mut form = reqwest::multipart::Form::new();
        for part in execution.parts {
            match part {
                FormPart::Text { name, value } => {
                    form = form.text(name.clone(), value.clone());
                }
                FormPart::Png { name, bytes } => {
                    let file = reqwest::multipart::Part::bytes(bytes.clone())
                        .file_name(format!("{}.png", name))
                        .mime_str("image/png")
                        .map_err(|e| format!("设置图片 MIME 失败: {}", e))?;
                    form = form.part(name.clone(), file);
                }
            }
        }
        request = request.multipart(form);
    } else {
        request = request.json(execution.body);
    }
    let request = with_auth(request, execution.rule.transport.auth, execution.api_key);
    let response = request
        .send()
        .await
        .map_err(|e| format!("{}任务创建请求失败: {}", label, e))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("读取创建响应失败: {}", e))?;
    if !status.is_success() {
        return Err(format!(
            "{}任务创建失败 (HTTP {}): {}",
            label,
            status.as_u16(),
            text
        ));
    }
    let created: Value =
        serde_json::from_str(&text).map_err(|e| format!("解析创建响应失败: {}", e))?;

    // 创建响应即最终结果（同步接口或响应已带地址）
    if execution.rule.output == OutputKind::MarkdownUrl {
        let urls = collect_outputs(&created, execution)?;
        if !urls.is_empty() {
            return Ok(ExecutionResult {
                task_id: None,
                video_url: urls.first().cloned(),
                local_path: None,
                urls,
            });
        }
    } else if let Some(url) = result_url(&created, execution.rule) {
        return Ok(ExecutionResult {
            task_id: None,
            video_url: Some(url.clone()),
            local_path: None,
            urls: vec![url],
        });
    }
    if execution.rule.transport.poll.is_none() {
        if execution.rule.response_mode == ResponseMode::Binary {
            let path = download_content(&client, execution, None).await?;
            return Ok(ExecutionResult {
                task_id: None,
                video_url: None,
                local_path: Some(path),
                urls: Vec::new(),
            });
        }
        return Err(format!("响应缺少{}地址: {}", label, text));
    }

    // ── 轮询任务 ──
    let task_id = task_id(&created, &execution.rule.task)
        .ok_or_else(|| format!("响应缺少任务标识: {}", text))?;
    progress(&format!(
        "{}生成任务已创建（{}），正在生成中，请耐心等待…",
        label, task_id
    ));

    let poll_template = execution
        .rule
        .transport
        .poll
        .as_deref()
        .expect("已确认存在轮询路径");
    let deadline = Instant::now() + Duration::from_secs(POLL_TIMEOUT_SECS);
    let started = Instant::now();
    let mut last_report = Instant::now();

    loop {
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        if Instant::now() >= deadline {
            return Err(format!(
                "{}生成超时（超过 15 分钟），任务 {} 可能仍在处理中",
                label, task_id
            ));
        }
        if last_report.elapsed() >= Duration::from_secs(PROGRESS_INTERVAL_SECS) {
            progress(&format!(
                "{}生成还在进行，请耐心等待…（已等待约 {} 秒）",
                label,
                started.elapsed().as_secs()
            ));
            last_report = Instant::now();
        }

        let poll_url = build_url(
            execution.base_url,
            poll_template,
            execution.model,
            Some(&task_id),
        )?;
        let request = with_auth(
            client.get(&poll_url),
            execution.rule.transport.auth,
            execution.api_key,
        );
        let response = request
            .send()
            .await
            .map_err(|e| format!("查询任务状态失败: {}", e))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| format!("读取任务状态失败: {}", e))?;
        if !status.is_success() {
            return Err(format!(
                "查询任务状态失败 (HTTP {}): {}",
                status.as_u16(),
                text
            ));
        }
        let current: Value =
            serde_json::from_str(&text).map_err(|e| format!("解析任务状态失败: {}", e))?;

        match evaluate(&current, &execution.rule.task) {
            TaskState::Failed(message) => {
                return Err(format!("{}生成失败（任务 {}）: {}", label, task_id, message))
            }
            TaskState::Success => {
                if execution.rule.output == OutputKind::MarkdownUrl {
                    let urls = collect_outputs(&current, execution)?;
                    if !urls.is_empty() {
                        return Ok(ExecutionResult {
                            task_id: Some(task_id),
                            video_url: urls.first().cloned(),
                            local_path: None,
                            urls,
                        });
                    }
                } else if let Some(url) = result_url(&current, execution.rule) {
                    return Ok(ExecutionResult {
                        task_id: Some(task_id),
                        video_url: Some(url.clone()),
                        local_path: None,
                        urls: vec![url],
                    });
                }
                if execution.rule.response_mode == ResponseMode::Binary {
                    let path = download_content(&client, execution, Some(&task_id)).await?;
                    return Ok(ExecutionResult {
                        task_id: Some(task_id),
                        video_url: None,
                        local_path: Some(path),
                        urls: Vec::new(),
                    });
                }
                return Err(format!("任务已完成但响应缺少{}地址: {}", label, text));
            }
            TaskState::Pending => {}
        }
    }
}

/// 拼接 base_url 与清单路径，替换 `{model}` / `{id}` 占位符
pub fn build_url(
    base_url: &str,
    template: &str,
    model: &str,
    id: Option<&str>,
) -> Result<String, String> {
    if !template.starts_with('/') {
        return Err(format!("清单路径必须以 / 开头: {}", template));
    }
    let mut path = template.replace("{model}", model);
    if let Some(id) = id {
        path = path.replace("{id}", id);
    }
    if path.contains("{model}") || path.contains("{id}") {
        return Err(format!("路径占位符未解析: {}", path));
    }
    Ok(format!("{}{}", base_url.trim_end_matches('/'), path))
}

fn with_auth(
    request: reqwest::RequestBuilder,
    auth: AuthKind,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match auth {
        AuthKind::Bearer => request.bearer_auth(api_key),
        AuthKind::GoogleApiKey => request.header("x-goog-api-key", api_key),
    }
}

/// 按点号读取字段（纯数字段按数组下标处理）
pub fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = match segment.parse::<usize>() {
            Ok(index) => current.as_array()?.get(index)?,
            Err(_) => current.get(segment)?,
        };
    }
    Some(current)
}

/// 按点号读取字段，`*` 段展开为数组全部元素
fn get_paths<'a>(value: &'a Value, path: &str) -> Vec<&'a Value> {
    let mut current = vec![value];
    for segment in path.split('.') {
        if segment.is_empty() {
            return Vec::new();
        }
        let mut next = Vec::new();
        for item in current {
            if segment == "*" {
                if let Some(array) = item.as_array() {
                    next.extend(array.iter());
                }
            } else if let Ok(index) = segment.parse::<usize>() {
                if let Some(found) = item.as_array().and_then(|array| array.get(index)) {
                    next.push(found);
                }
            } else if let Some(found) = item.get(segment) {
                next.push(found);
            }
        }
        current = next;
        if current.is_empty() {
            return Vec::new();
        }
    }
    current
}

/// 创建响应中的任务标识：按 `idPaths` 顺序尝试
pub fn task_id(response: &Value, task: &TaskSpec) -> Option<String> {
    task.id_paths.iter().find_map(|path| {
        let value = get_path(response, path)?;
        if let Some(text) = value.as_str() {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
            return None;
        }
        if value.is_number() {
            return Some(value.to_string());
        }
        None
    })
}

/// 任务状态判定：错误路径优先，其次成功 / 失败取值，都不命中则继续轮询
pub fn evaluate(response: &Value, task: &TaskSpec) -> TaskState {
    if let Some(path) = &task.error_path {
        if let Some(value) = get_path(response, path) {
            if !value.is_null() {
                let message = message_of(value);
                if !message.trim().is_empty() {
                    return TaskState::Failed(message);
                }
            }
        }
    }
    if let Some(path) = &task.status_path {
        let Some(value) = get_path(response, path) else {
            return TaskState::Pending;
        };
        if task.failure_values.iter().any(|item| value_eq(value, item)) {
            return TaskState::Failed(message_of(value));
        }
        if task.success_values.iter().any(|item| value_eq(value, item)) {
            return TaskState::Success;
        }
        return TaskState::Pending;
    }
    TaskState::Success
}

/// 按 `resultPaths` 顺序提取首个结果地址
pub fn result_url(response: &Value, rule: &ModelRule) -> Option<String> {
    rule.result_paths.iter().find_map(|path| {
        get_path(response, path)
            .and_then(|value| value.as_str())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    })
}

/// 收集 `resultPaths` 的全部结果：地址原样返回，`b64_json` 落盘后返回本地路径。
fn collect_outputs(
    response: &Value,
    execution: &Execution<'_>,
) -> Result<Vec<String>, String> {
    let mut outputs: Vec<String> = Vec::new();
    for path in &execution.rule.result_paths {
        for value in get_paths(response, path) {
            let Some(text) = value.as_str().map(str::trim).filter(|t| !t.is_empty()) else {
                continue;
            };
            if is_remote_url(text) {
                if !outputs.iter().any(|item| item == text) {
                    outputs.push(text.to_string());
                }
            } else if execution.rule.output == OutputKind::MarkdownUrl {
                let path = save_base64_image(text, execution.output_dir, outputs.len())?;
                outputs.push(path);
            } else if !outputs.iter().any(|item| item == text) {
                outputs.push(text.to_string());
            }
        }
    }
    Ok(outputs)
}

fn is_remote_url(text: &str) -> bool {
    text.starts_with("http://") || text.starts_with("https://")
}

fn save_base64_image(text: &str, dir: &Path, index: usize) -> Result<String, String> {
    let payload = text.split_once(',').map(|(_, rest)| rest).unwrap_or(text);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|e| format!("解码图片 base64 失败: {}", e))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建图片目录失败: {}", e))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("image-{}-{}.png", stamp, index + 1));
    std::fs::write(&path, bytes).map_err(|e| format!("保存图片失败: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

fn value_eq(left: &Value, right: &Value) -> bool {
    match (left.as_str(), right.as_str()) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => left == right,
    }
}

fn message_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `binary` 模式：按 `content` 路径拉取内容并落盘
async fn download_content(
    client: &reqwest::Client,
    execution: &Execution<'_>,
    id: Option<&str>,
) -> Result<String, String> {
    let label = execution.label;
    let template = execution
        .rule
        .transport
        .content
        .as_deref()
        .ok_or_else(|| "binary 模式缺少 content 路径".to_string())?;
    let url = build_url(execution.base_url, template, execution.model, id)?;
    let request = with_auth(
        client.get(&url),
        execution.rule.transport.auth,
        execution.api_key,
    );
    let response = request
        .send()
        .await
        .map_err(|e| format!("下载{}失败: {}", label, e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("下载{}失败 (HTTP {})", label, status.as_u16()));
    }
    let extension = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase())
        .map(|value| {
            if value.contains("webm") {
                "webm"
            } else {
                "mp4"
            }
        })
        .unwrap_or("mp4");
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("读取{}失败: {}", label, e))?;

    std::fs::create_dir_all(execution.output_dir)
        .map_err(|e| format!("创建{}目录失败: {}", label, e))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = execution
        .output_dir
        .join(format!("video-{}.{}", stamp, extension));
    std::fs::write(&path, &bytes).map_err(|e| format!("保存{}失败: {}", label, e))?;
    log::info!(
        "[Generate:{}] binary 落盘: {} ({} 字节)",
        label,
        path.display(),
        bytes.len()
    );
    Ok(path.to_string_lossy().to_string())
}
