//! 意见反馈：把用户提交的反馈转发到平台服务端（`POST /api/v1/me/feedback`）。
//!
//! 为什么不前端直连：
//! 1. 令牌只在 Rust 侧保存与使用（见 `utils/platform.rs`），前端拿不到明文；
//! 2. `project` / 客户端版本 / 渠道等标识由后端注入，避免前端篡改或遗漏；
//! 3. 平台接口都在 `/api/v1/me/*` 命名空间下（`requireAuth`），与用户中心同源，
//!    用户中心才看得到自己提交的反馈。
//!
//! 未登录一律拒绝：没有账号归属的反馈在用户中心不可见，也无法邮件回访。

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use crate::commands::org_common::{map_platform_error, require_token};
use crate::utils::platform;

/// 平台侧项目标识（多项目共用反馈接口时用于路由与字段校验）。
/// **编译期常量、不接受前端传入**，否则前端可伪造成其它项目。
const PROJECT: &str = "pilotdesk";

/// 平台反馈接口路径（同属 `/api/v1/me/*`，用户中心可见）
const FEEDBACK_PATH: &str = "/api/v1/me/feedback";

/// 一次提交最多附带多少条日志片段
const MAX_LOG_LINES: usize = 8;
/// 单条日志片段的最大字符数（超出截断）
const MAX_LOG_CHARS: usize = 500;
const MAX_TITLE_CHARS: usize = 80;
const MAX_CONTENT_CHARS: usize = 2000;

/// 一条日志片段（由 `read_recent_logs` 返回，前端勾选后回传）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FeedbackLogLine {
    pub ts: i64,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// 提交入参（前端只填用户可编辑的部分，其余标识由后端补）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitFeedbackInput {
    /// `bug` | `suggestion` | `other`
    pub category: String,
    pub title: String,
    pub content: String,
    /// 联系方式：留空则用账号邮箱（平台侧按账号回访）
    #[serde(default)]
    pub contact_email: Option<String>,
    /// 幂等键：前端生成，网络失败重试时复用同一个值
    pub client_request_id: String,
    #[serde(default)]
    pub logs: Vec<FeedbackLogLine>,
    /// 系统标识（缺省取编译目标）
    #[serde(default)]
    pub os: Option<String>,
    /// 界面语言（缺省 `zh-CN`）
    #[serde(default)]
    pub locale: Option<String>,
}

/// 提交结果：平台回执编号
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitFeedbackResult {
    pub id: String,
    pub received_at: i64,
    pub status: String,
}

/// 平台成功响应
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlatformFeedbackResp {
    id: String,
    #[serde(default)]
    received_at: i64,
    #[serde(default)]
    status: String,
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// 平台错误 → 面向用户的中文文案。
///
/// 429 单独处理：把 `retryAfter`（秒）换算成分钟，用户才知道要等多久；
/// 其余沿用 `org_common` 的统一映射（401/403 等）。
fn map_feedback_error(err: platform::PlatformError) -> String {
    if err.status == 429 {
        let minutes = err
            .body
            .as_ref()
            .and_then(|v| v.get("retryAfter").and_then(|x| x.as_i64()))
            .map(|secs| (secs + 59) / 60)
            .unwrap_or(60);
        return format!("提交过于频繁，请约 {} 分钟后重试", minutes);
    }
    map_platform_error(err)
}

/// 提交意见反馈。
///
/// 前置：必须已登录平台账号（未登录返回明确中文提示，前端据此引导登录后重试）。
/// 校验在前端与后端各做一次：前端给即时反馈，后端防止绕过。
#[tauri::command]
pub async fn submit_feedback(
    input: SubmitFeedbackInput,
    state: State<'_, crate::DbState>,
) -> Result<SubmitFeedbackResult, String> {
    let token = require_token(state.inner()).await?;

    let category = input.category.trim().to_string();
    if !matches!(category.as_str(), "bug" | "suggestion" | "other") {
        return Err("反馈分类不合法".to_string());
    }

    let title = input.title.trim().to_string();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("标题需为 1–{} 字", MAX_TITLE_CHARS));
    }

    let content = input.content.trim().to_string();
    if content.is_empty() || content.chars().count() > MAX_CONTENT_CHARS {
        return Err(format!("描述需为 1–{} 字", MAX_CONTENT_CHARS));
    }

    let client_request_id = input.client_request_id.trim().to_string();
    if client_request_id.is_empty() {
        return Err("缺少提交标识，请重新打开反馈弹窗后再试".to_string());
    }

    let logs: Vec<FeedbackLogLine> = input
        .logs
        .into_iter()
        .take(MAX_LOG_LINES)
        .map(|l| FeedbackLogLine {
            ts: l.ts,
            level: l.level,
            target: l.target,
            message: truncate_chars(&l.message, MAX_LOG_CHARS),
        })
        .collect();

    let contact_email = input
        .contact_email
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let body = json!({
        "project": PROJECT,
        "category": category,
        "title": title,
        "content": content,
        "contactEmail": contact_email,
        "clientRequestId": client_request_id,
        "logs": logs,
        "client": {
            "appVersion": env!("CARGO_PKG_VERSION"),
            // 渠道：debug 构建视为 dev，其余按正式包
            "channel": if cfg!(debug_assertions) { "dev" } else { "stable" },
            "os": input.os.unwrap_or_else(|| std::env::consts::OS.to_string()),
            "locale": input.locale.unwrap_or_else(|| "zh-CN".to_string()),
        },
    });

    let resp: PlatformFeedbackResp =
        platform::post_json_with_status(FEEDBACK_PATH, &body, Some(&token))
            .await
            .map_err(map_feedback_error)?;

    Ok(SubmitFeedbackResult {
        id: resp.id,
        received_at: resp.received_at,
        status: if resp.status.trim().is_empty() {
            "pending".to_string()
        } else {
            resp.status
        },
    })
}
