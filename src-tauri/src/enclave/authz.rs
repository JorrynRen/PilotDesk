//! 飞地授权器：**主机侧唯一授权点**，默认拒绝。
//!
//! 两条铁律（设计 §8.3）：
//! 1. **默认拒绝**：没有明确授予的资源/动作，一律不可访问。
//! 2. **Agent 不提权**：AI Agent 只继承发起人权限，且只能读（见 [`authorize_agent`]）。
//!
//! 组织层角色以平台为唯一权威：本模块只读 `org_snapshot.my_role`（快照来自
//! `GET /api/v1/organizations/mine` 的 `role` 字段），**本地不写**。
//! 资源级角色（任务 / 知识库）在 M3 起结合 `resource_roles` 与本模块共同判定。

use rusqlite::{params, Connection, OptionalExtension};

/// 组织层角色（复用平台三档，飞地不新增角色）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Owner,
    Admin,
    Member,
}

impl Role {
    pub fn parse(raw: &str) -> Option<Role> {
        match raw {
            "owner" => Some(Role::Owner),
            "admin" => Some(Role::Admin),
            "member" => Some(Role::Member),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }
}

/// 飞地动作集合（组织级 + 资源级）。
///
/// 一期 M2 只落**组织级**动作的判定；资源级动作（任务/汇报/知识库）在 M3/M4 接入时
/// 由 [`org_allows`] 先给出「组织层是否具备该动作的基础资格」，再叠加资源级角色。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    TaskView,
    TaskCreate,
    TaskEdit,
    TaskStatus,
    TaskAssign,
    /// 硬删除终态任务及其全部记录（创建者 / 组织管理员）
    TaskDelete,
    ReportSubmit,
    ReportConfirm,
    FileUpload,
    KbView,
    KbEdit,
    KbManage,
    OrgAdmin,
    PolicyWrite,
    HostTransfer,
    EmergencySet,
    /// 修改本机团队文件根目录（仅 owner；物理落点变更，风险等级同策略）
    FileRootWrite,
}

impl Action {
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::TaskView => "task.view",
            Action::TaskCreate => "task.create",
            Action::TaskEdit => "task.edit",
            Action::TaskStatus => "task.status",
            Action::TaskAssign => "task.assign",
            Action::TaskDelete => "task.delete",
            Action::ReportSubmit => "report.submit",
            Action::ReportConfirm => "report.confirm",
            Action::FileUpload => "file.upload",
            Action::KbView => "kb.view",
            Action::KbEdit => "kb.edit",
            Action::KbManage => "kb.manage",
            Action::OrgAdmin => "org.admin",
            Action::PolicyWrite => "policy.write",
            Action::HostTransfer => "host.transfer",
            Action::EmergencySet => "emergency.set",
            Action::FileRootWrite => "files.root.write",
        }
    }

    pub fn parse(raw: &str) -> Option<Action> {
        Some(match raw {
            "task.view" => Action::TaskView,
            "task.create" => Action::TaskCreate,
            "task.edit" => Action::TaskEdit,
            "task.status" => Action::TaskStatus,
            "task.assign" => Action::TaskAssign,
            "task.delete" => Action::TaskDelete,
            "report.submit" => Action::ReportSubmit,
            "report.confirm" => Action::ReportConfirm,
            "file.upload" => Action::FileUpload,
            "kb.view" => Action::KbView,
            "kb.edit" => Action::KbEdit,
            "kb.manage" => Action::KbManage,
            "org.admin" => Action::OrgAdmin,
            "policy.write" => Action::PolicyWrite,
            "host.transfer" => Action::HostTransfer,
            "emergency.set" => Action::EmergencySet,
            "files.root.write" => Action::FileRootWrite,
            _ => return None,
        })
    }

    /// 组织级动作清单（供 [`effective_actions`] 计算与前端渲染）。
    pub fn org_actions() -> &'static [Action] {
        &[
            Action::TaskCreate,
            Action::OrgAdmin,
            Action::PolicyWrite,
            Action::HostTransfer,
            Action::EmergencySet,
            Action::FileRootWrite,
        ]
    }
}

/// 组织层角色是否具备某动作的**基础资格**（默认拒绝）。
///
/// 说明：资源级动作这里只给「组织层是否允许他做这类事」，最终是否可操作某条任务/某个库
/// 还要叠加资源级角色（M3 起）。
pub fn org_allows(role: Role, action: Action) -> bool {
    match action {
        // 组织成员默认可看团队内容（受数据出境闸门约束的是「送云端」而非「本机看」）
        Action::TaskView | Action::KbView => true,
        // 建任务：owner / admin 可直接建；member 建任务在 M3 由资源级规则决定归属
        Action::TaskCreate => matches!(role, Role::Owner | Role::Admin | Role::Member),
        Action::OrgAdmin => matches!(role, Role::Owner | Role::Admin),
        // 数据出境策略：仅 owner（设计 §8.5「仅 owner 可修改」）
        Action::PolicyWrite => role == Role::Owner,
        // 主机转移：仅 owner（owner = host，设计 §8.7）
        Action::HostTransfer => role == Role::Owner,
        // 应急开关：按实施计划 M2「仅 owner」收紧（设计 §8.3 矩阵为 owner/admin，
        // 如需放开给 admin，只需在此处加 `| Role::Admin`）
        Action::EmergencySet => role == Role::Owner,
        // 团队文件根目录：物理落点变更，仅 owner（§13）
        Action::FileRootWrite => role == Role::Owner,
        // 团队知识库条目的人工编辑 / 移除：owner / admin（成员只读）
        Action::KbEdit => matches!(role, Role::Owner | Role::Admin),
        // 其余资源级动作：单看组织层不足以放行 → 默认拒绝（M3 叠加资源级角色后再放开）
        _ => false,
    }
}

/// 某角色在组织范围内允许的动作清单（`effectiveActions` = 前端渲染按钮的唯一来源）。
pub fn effective_actions(role: Role) -> Vec<&'static str> {
    Action::org_actions()
        .iter()
        .filter(|a| org_allows(role, **a))
        .map(|a| a.as_str())
        .collect()
}

/// Agent 权限：只读白名单，**不提权**（写动作在工具层不可达）。
pub fn authorize_agent(role: Role, action: Action) -> bool {
    match action {
        Action::TaskView | Action::KbView => org_allows(role, action),
        _ => false,
    }
}

// ── 与 `enclave.db` 快照结合的判定 ──────────────────────────────

/// 当前登录用户（平台 `user.id`）——来自快照表 `local_identity`。
///
/// 未登录 / 尚未成功同步过 → 中文错误。
pub fn current_actor_id(conn: &Connection) -> Result<String, String> {
    conn.query_row("SELECT user_id FROM local_identity LIMIT 1", [], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map_err(|e| format!("读取本机登录身份失败：{}", e))?
    .filter(|s| !s.trim().is_empty())
    .ok_or_else(|| "请先登录平台账号".to_string())
}

/// 本机是否有登录身份（不区分快照绑定）。
///
/// 用来把「没登录」和「登录了但组织快照没同步」区分成两条不同的提示：
/// 前者让用户去登录，后者才让他去刷新治理快照。
pub fn has_identity(conn: &Connection) -> Result<bool, String> {
    conn.query_row("SELECT user_id FROM local_identity LIMIT 1", [], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map_err(|e| format!("读取本机登录身份失败：{}", e))
    .map(|row| row.is_some_and(|s| !s.trim().is_empty()))
}

/// 当前用户在指定组织中的角色（来自平台快照）；组织不存在 / 角色无法识别返回 `None`。
///
/// **身份绑定校验**：快照行必须由当前登录身份同步而来（`viewer_id = local_identity.user_id`）。
/// 换账号后，上一个账号留下的 `my_role` 立即失效，避免沿用旧身份越权。
pub fn role_for_org(conn: &Connection, org_id: &str) -> Result<Option<Role>, String> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT my_role, viewer_id FROM org_snapshot WHERE org_id = ?1",
            params![org_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("读取组织角色失败：{}", e))?;
    let Some((raw_role, viewer_id)) = row else {
        return Ok(None);
    };
    let actor = conn
        .query_row("SELECT user_id FROM local_identity LIMIT 1", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(|e| format!("读取本机登录身份失败：{}", e))?;
    let Some(actor) = actor.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    if viewer_id.is_empty() || viewer_id != actor {
        return Ok(None);
    }
    Ok(Role::parse(&raw_role))
}

/// 主机侧：按**指定成员**解析其在组织中的角色（用于局域网连接的身份确认）。
///
/// 与 [`role_for_org`] 的区别：后者永远以本机 `local_identity` 为准；主机在处理成员端
/// 远程请求时，actor 是连接方而不是本机登录用户，必须从只读快照里取该成员的角色。
pub fn role_for_member(
    conn: &Connection,
    org_id: &str,
    user_id: &str,
) -> Result<Option<Role>, String> {
    if user_id.trim().is_empty() {
        return Ok(None);
    }
    let owner: Option<String> = conn
        .query_row(
            "SELECT owner_user_id FROM org_snapshot WHERE org_id = ?1",
            params![org_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("读取组织所有者失败：{}", e))?;
    if owner.as_deref() == Some(user_id) {
        return Ok(Some(Role::Owner));
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT role FROM member_snapshot
             WHERE org_id = ?1 AND user_id = ?2 AND seat_status <> 'removed'",
            params![org_id, user_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("读取成员角色失败：{}", e))?;
    Ok(raw.as_deref().and_then(Role::parse))
}

/// 组织授权的最终入口：成功返回 `actor_id`（供审计），失败返回可直接展示的中文原因。
///
/// 拒绝文案区分两种情况（设计 §8.3 / §8.4）：
/// - **快照不存在或角色不可识别** → 「治理已冻结」类提示（先刷新组织快照）；
/// - **角色不足** → 「当前角色（member）无权执行该操作」。
pub fn authorize(conn: &Connection, org_id: &str, action: Action) -> Result<String, String> {
    let actor = current_actor_id(conn)?;
    let role = role_for_org(conn, org_id)?.ok_or_else(|| {
        "尚未同步该组织的治理快照，治理已冻结：请先在「组织与权限」页点击刷新".to_string()
    })?;
    if !org_allows(role, action) {
        return Err(format!(
            "当前角色（{}）无权执行该操作（需要更高的组织角色）",
            role.as_str()
        ));
    }
    Ok(actor)
}

/// 组织范围内可执行的动作清单；快照缺失时返回空（前端因此不渲染任何治理按钮）。
pub fn effective_actions_for_org(
    conn: &Connection,
    org_id: &str,
) -> Result<Vec<&'static str>, String> {
    match role_for_org(conn, org_id)? {
        Some(role) => Ok(effective_actions(role)),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认拒绝：未在 `org_allows` 明确放行的资源级动作，任何角色都不得通过。
    #[test]
    fn default_deny_for_unspecified_actions() {
        for role in [Role::Owner, Role::Admin, Role::Member] {
            for action in [
                Action::TaskEdit,
                Action::TaskStatus,
                Action::TaskAssign,
                Action::ReportSubmit,
                Action::ReportConfirm,
                Action::FileUpload,
                Action::KbManage,
            ] {
                assert!(
                    !org_allows(role, action),
                    "{} 不应在组织层直接放行 {:?}",
                    role.as_str(),
                    action
                );
            }
        }
    }

    /// 组织级治理动作矩阵：策略/转移/应急仅 owner；OrgAdmin 为 owner+admin。
    #[test]
    fn governance_actions_matrix() {
        assert!(org_allows(Role::Owner, Action::PolicyWrite));
        assert!(!org_allows(Role::Admin, Action::PolicyWrite));
        assert!(!org_allows(Role::Member, Action::PolicyWrite));

        assert!(org_allows(Role::Owner, Action::HostTransfer));
        assert!(!org_allows(Role::Admin, Action::HostTransfer));

        assert!(org_allows(Role::Owner, Action::EmergencySet));
        assert!(!org_allows(Role::Admin, Action::EmergencySet));
        assert!(!org_allows(Role::Member, Action::EmergencySet));

        assert!(org_allows(Role::Owner, Action::OrgAdmin));
        assert!(org_allows(Role::Admin, Action::OrgAdmin));
        assert!(!org_allows(Role::Member, Action::OrgAdmin));

        // 知识条目人工维护：owner / admin 可编辑，成员只读
        assert!(org_allows(Role::Owner, Action::KbEdit));
        assert!(org_allows(Role::Admin, Action::KbEdit));
        assert!(!org_allows(Role::Member, Action::KbEdit));
    }

    /// effective_actions 必须与 org_allows 一致（前端按钮的唯一来源）。
    #[test]
    fn effective_actions_consistent_with_matrix() {
        let owner = effective_actions(Role::Owner);
        assert!(owner.contains(&"policy.write"));
        assert!(owner.contains(&"host.transfer"));
        assert!(owner.contains(&"emergency.set"));

        let admin = effective_actions(Role::Admin);
        assert!(!admin.contains(&"policy.write"));
        assert!(admin.contains(&"org.admin"));

        let member = effective_actions(Role::Member);
        assert!(member.contains(&"task.create"));
        assert!(!member.contains(&"org.admin"));
    }

    /// Agent 只读白名单：写动作一律拒绝，读动作继承组织角色。
    #[test]
    fn agent_is_read_only() {
        for role in [Role::Owner, Role::Admin, Role::Member] {
            assert!(authorize_agent(role, Action::TaskView));
            assert!(authorize_agent(role, Action::KbView));
            assert!(!authorize_agent(role, Action::TaskCreate));
            assert!(!authorize_agent(role, Action::PolicyWrite));
            assert!(!authorize_agent(role, Action::EmergencySet));
        }
    }
}
