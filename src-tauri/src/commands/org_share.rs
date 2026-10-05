//! 组织共享空间（团队版）：把本地工作流共享到组织、从组织导入工作流。
//!
//! 这是「组织共享空间」在产品端的入口。平台侧接口（全部 `requireAuth`，产品端用 Bearer）：
//!   GET  /api/v1/organizations/mine                         我所属的组织
//!   GET  /api/v1/organizations/:id/resources?kind=workflow   组织的共享工作流列表
//!   POST /api/v1/organizations/:id/resources                 新建共享资源（同名 409 / 档位不含 403）
//!   GET  /api/v1/organizations/:id/resources/:resourceId     取单条（带 payload）
//!
//! 令牌复用 [`current_access_token`]（读 `account_tokens` + 自动续期）；未登录时返回明确中文提示。

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::account::current_access_token;
use crate::commands::workflow::{build_workflow_export_json, import_workflow_from_json_with_conn};
use crate::utils::platform;

/// 我所属的组织（仅保留 active 成员 + 组织在用；字段与平台 `MyOrganizationView` 对齐）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyOrg {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub plan_key: String,
    pub plan_expires_at: Option<String>,
    pub seats: i64,
    pub member_count: i64,
    pub status: String,
}

/// 共享工作流列表项（**不含 payload**，避免列表传输大对象）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWorkflow {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub creator_label: String,
    pub updated_at: String,
}

/// 共享入参：`workflowId` 优先，缺省时用 `workflowName` 定位；`name` 为共享到组织的名称。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareWorkflowInput {
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[serde(default)]
    pub workflow_name: Option<String>,
    pub org_id: i64,
    #[serde(default)]
    pub name: Option<String>,
}

/// 从组织导入入参
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportWorkflowInput {
    pub org_id: i64,
    pub resource_id: i64,
}

/// 导入结果：新本地工作流的 id 与名称（`subflow_count` 为随主流程一并导入的子工作流数量）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedWorkflow {
    pub workflow_id: String,
    pub name: String,
    /// 本次导入携带的子工作流数量（0 表示无子流）
    pub subflow_count: usize,
}

// ── 平台响应结构 ────────────────────────────────────────────────

#[derive(Deserialize)]
struct OrgsResp {
    organizations: Vec<OrgRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrgRow {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    seat_status: String,
    #[serde(default)]
    plan_key: String,
    #[serde(default)]
    plan_expires_at: Option<String>,
    #[serde(default)]
    seats: i64,
    #[serde(default)]
    member_count: i64,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
struct ResourcesResp {
    items: Vec<ResourceRow>,
}

#[derive(Deserialize)]
struct ResourceResp {
    resource: ResourceRow,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceRow {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    /// 列表接口也会返回；仅在取详情 / POST 时使用
    #[serde(default)]
    payload: String,
    #[serde(default)]
    creator_label: String,
    #[serde(default)]
    updated_at: String,
}

// ── 公共辅助 ────────────────────────────────────────────────────

/// 取一个可用的 Bearer 令牌；未登录 / 续期失败给出明确中文提示。
/// 供组织共享空间与组织共享凭据（`org_credentials`）复用。
pub(crate) async fn require_token(state: &crate::DbState) -> Result<String, String> {
    match current_access_token(state).await {
        Ok(Some(token)) => Ok(token),
        Ok(None) => Err("请先登录平台账号".to_string()),
        Err(e) => Err(format!("获取登录状态失败：{}", e)),
    }
}

/// 平台错误 → 面向用户的中文文案（对共享空间的 403 / 409 做友好映射）。
fn map_platform_error(err: platform::PlatformError) -> String {
    match err.status {
        // 403：组织档位不含 team.shared-space（free / pro 组织）
        403 => "当前组织档位不支持共享空间（需要团队版）".to_string(),
        // 409：同一组织同 kind 下名称唯一
        409 => "该组织已有同名共享工作流，请改名后重试".to_string(),
        // 401：令牌失效 / 未登录
        401 => "请先登录平台账号".to_string(),
        // 其余（含 status=0 的网络 / 解析错误）直接透传（正文已带中文前缀）
        _ => err.message,
    }
}

fn to_shared_workflow(row: ResourceRow) -> SharedWorkflow {
    SharedWorkflow {
        id: row.id,
        name: row.name,
        description: row.description,
        creator_label: row.creator_label,
        updated_at: row.updated_at,
    }
}

/// 在本地数据库里定位目标工作流 id：优先 `workflowId`，其次按 `workflowName` 精确匹配。
fn resolve_workflow_id(
    conn: &rusqlite::Connection,
    input: &ShareWorkflowInput,
) -> Result<String, String> {
    if let Some(id) = input.workflow_id.as_ref().filter(|s| !s.is_empty()) {
        return Ok(id.clone());
    }
    if let Some(name) = input.workflow_name.as_ref().filter(|s| !s.is_empty()) {
        let defs = crate::workflow::list_definitions(conn)
            .map_err(|e| format!("查询工作流失败：{}", e))?;
        return defs
            .into_iter()
            .find(|d| d.name == *name)
            .map(|d| d.id)
            .ok_or_else(|| format!("未找到名为「{}」的工作流", name));
    }
    Err("未指定要共享的工作流".to_string())
}

// ── 命令 ────────────────────────────────────────────────────────

/// 我所属的可用组织：只返回 `seatStatus === 'active'` 且 `status === 'active'` 的组织。
#[tauri::command]
pub async fn org_list_mine(state: State<'_, crate::DbState>) -> Result<Vec<MyOrg>, String> {
    let token = require_token(state.inner()).await?;
    let resp: OrgsResp = platform::get_json_with_status("/api/v1/organizations/mine", Some(&token))
        .await
        .map_err(map_platform_error)?;
    Ok(resp
        .organizations
        .into_iter()
        .filter(|o| o.seat_status == "active" && o.status == "active")
        .map(|o| MyOrg {
            id: o.id,
            name: o.name,
            role: o.role,
            plan_key: o.plan_key,
            plan_expires_at: o.plan_expires_at,
            seats: o.seats,
            member_count: o.member_count,
            status: o.status,
        })
        .collect())
}

/// 某组织的共享工作流列表（不含 payload）。
#[tauri::command]
pub async fn org_list_shared_workflows(
    org_id: i64,
    state: State<'_, crate::DbState>,
) -> Result<Vec<SharedWorkflow>, String> {
    let token = require_token(state.inner()).await?;
    let path = format!("/api/v1/organizations/{}/resources?kind=workflow", org_id);
    let resp: ResourcesResp = platform::get_json_with_status(&path, Some(&token))
        .await
        .map_err(map_platform_error)?;
    Ok(resp.items.into_iter().map(to_shared_workflow).collect())
}

/// 把本地工作流共享到组织：复用导出逻辑打包「主工作流 + 全部子工作流」（新版捆绑格式），
/// 再 POST 到平台新建共享资源。
#[tauri::command]
pub async fn org_share_workflow(
    input: ShareWorkflowInput,
    state: State<'_, crate::DbState>,
) -> Result<SharedWorkflow, String> {
    let token = require_token(state.inner()).await?;

    // 定位工作流 → 复用导出转换得到 JSON 字符串（同一个连接内完成）
    let (default_name, payload) = {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        let workflow_id = resolve_workflow_id(&conn, &input)?;
        build_workflow_export_json(&conn, &workflow_id).map_err(String::from)?
    };

    // 共享名称缺省用工作流名
    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&default_name)
        .to_string();

    let body = serde_json::json!({
        "kind": "workflow",
        "name": name,
        "payload": payload,
    });
    let path = format!("/api/v1/organizations/{}/resources", input.org_id);
    let resp: ResourceResp = platform::post_json_with_status(&path, &body, Some(&token))
        .await
        .map_err(map_platform_error)?;
    Ok(to_shared_workflow(resp.resource))
}

/// 从组织导入共享工作流：取资源详情（带 payload）→ 复用导入逻辑新建本地工作流。
#[tauri::command]
pub async fn org_import_workflow(
    input: ImportWorkflowInput,
    state: State<'_, crate::DbState>,
) -> Result<ImportedWorkflow, String> {
    let token = require_token(state.inner()).await?;
    let path = format!(
        "/api/v1/organizations/{}/resources/{}",
        input.org_id, input.resource_id
    );
    let resp: ResourceResp = platform::get_json_with_status(&path, Some(&token))
        .await
        .map_err(map_platform_error)?;

    let resource = resp.resource;
    if resource.payload.trim().is_empty() {
        return Err("共享工作流内容为空，无法导入".to_string());
    }

    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败：{}", e))?;
    let (def, subflow_count) =
        import_workflow_from_json_with_conn(&conn, &resource.payload).map_err(String::from)?;
    Ok(ImportedWorkflow {
        workflow_id: def.id,
        name: def.name,
        subflow_count,
    })
}
