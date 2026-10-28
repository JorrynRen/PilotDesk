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

use crate::agent::AgentManager;
use crate::commands::agents;
use crate::commands::env::{cached_agent_health, cached_agent_versions, clean_version_string};
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

/// 依赖工具版本探测的超时上限（秒）。
///
/// 只是 `--version`，正常在几百毫秒内返回；给 5s 是为了不把提交卡在
/// 「环境检测」上——超时就按未知（`null`）上报，不阻塞反馈本身。
const PROBE_TIMEOUT_SECS: u64 = 5;
/// 环境摘要里最多带上多少个已启用 Agent（防止配置很多时撑大请求体）
const MAX_ENV_AGENTS: usize = 12;

/// 一个已启用 Agent 的环境摘要。
///
/// 只放定位问题必需的三件事：接入方式、版本、是否安装/可用。
/// **不带**启动参数与连接配置——那里面有密钥和本机路径。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackEnvAgent {
    agent_type: String,
    display_name: String,
    /// `subprocess`（本地 CLI）/ `acp`
    transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    available: Option<bool>,
}

/// 运行环境摘要（随反馈提交，帮助平台侧判断是环境问题还是产品缺陷）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackEnv {
    os: String,
    arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    npm_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    python_version: Option<String>,
    agents: Vec<FeedbackEnvAgent>,
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

/// 依赖工具版本（探测结果，拿不到就是 `None`）。
struct ToolVersions {
    node: Option<String>,
    npm: Option<String>,
    git: Option<String>,
    python: Option<String>,
}

/// 现场探测依赖工具版本。
///
/// Node / npm / Git / Python 的版本几乎决定「Agent 装不上 / 起不来」类问题的归因，
/// 且各自只是一次 `--version`，并行跑、5s 上限，拿不到就按未知上报，不阻塞提交。
///
/// **只拿 `&AgentManager`，不碰数据库连接**：`rusqlite::Connection` 不是 `Sync`，
/// 带着它跨 await 会让命令 future 不满足 Tauri 要求的 `Send`。
async fn probe_tools(agent_mgr: &AgentManager) -> ToolVersions {
    let (node, npm, git, py) = tokio::join!(
        agent_mgr.execute_command_output_async(
            "node --version",
            "",
            PROBE_TIMEOUT_SECS,
            "反馈环境摘要 node"
        ),
        agent_mgr.execute_command_output_async(
            "npm --version",
            "",
            PROBE_TIMEOUT_SECS,
            "反馈环境摘要 npm"
        ),
        agent_mgr.execute_command_output_async(
            "git --version",
            "",
            PROBE_TIMEOUT_SECS,
            "反馈环境摘要 git"
        ),
        async {
            for cmd in ["python --version", "python3 --version", "py --version"] {
                if let Ok(v) = agent_mgr
                    .execute_command_output_async(
                        cmd,
                        "",
                        PROBE_TIMEOUT_SECS,
                        "反馈环境摘要 python",
                    )
                    .await
                {
                    return Some(clean_version_string(&v));
                }
            }
            None
        },
    );

    ToolVersions {
        node: node.ok().map(|v| clean_version_string(&v)),
        npm: npm.ok().map(|v| clean_version_string(&v)),
        git: git.ok().map(|v| clean_version_string(&v)),
        python: py,
    }
}

/// 已启用 Agent 的版本与安装/可用状态。
///
/// 版本与状态取环境检测的既有缓存（120s 内有效），没有缓存就留空——
/// **不在这里触发批量探测**，否则提交反馈会先卡十几秒。
fn collect_agents(conn: &rusqlite::Connection) -> Vec<FeedbackEnvAgent> {
    let transports: std::collections::HashMap<String, String> =
        agents::list_agent_transports_inner(conn)
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.agent_type, r.transport))
            .collect();
    let cached_versions = cached_agent_versions();

    let mut rows = Vec::new();
    for agent in agents::list_agents_inner(conn).unwrap_or_default() {
        if !agent.is_enabled {
            continue;
        }
        let health = cached_agent_health(&agent.agent_type);
        rows.push(FeedbackEnvAgent {
            transport: transports
                .get(&agent.agent_type)
                .cloned()
                .unwrap_or_else(|| "subprocess".to_string()),
            version: cached_versions.get(&agent.agent_type).cloned().or_else(|| {
                agent
                    .pkg_state
                    .as_ref()
                    .and_then(|s| s.installed_version.clone())
            }),
            installed: health.as_ref().map(|h| h.installed),
            available: health.as_ref().map(|h| h.available),
            display_name: agent.display_name,
            agent_type: agent.agent_type,
        });
        if rows.len() >= MAX_ENV_AGENTS {
            break;
        }
    }
    rows
}

/// 提交意见反馈。
///
/// 前置：必须已登录平台账号（未登录返回明确中文提示，前端据此引导登录后重试）。
/// 校验在前端与后端各做一次：前端给即时反馈，后端防止绕过。
#[tauri::command]
pub async fn submit_feedback(
    input: SubmitFeedbackInput,
    state: State<'_, crate::DbState>,
    agent_mgr: State<'_, crate::AsyncMutex<AgentManager>>,
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

    // 环境摘要：工具版本现场探测；Agent 版本/状态读环境检测缓存。
    // 探测先做（需锁 AgentManager），随后同步读库，避免连接跨 await。
    let tools = {
        let mgr = agent_mgr.lock().await;
        probe_tools(&mgr).await
    };
    let agent_rows = {
        let conn = state.get_conn()?;
        collect_agents(&conn)
    };
    let env = FeedbackEnv {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        node_version: tools.node,
        npm_version: tools.npm,
        git_version: tools.git,
        python_version: tools.python,
        agents: agent_rows,
    };

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
        "env": env,
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
