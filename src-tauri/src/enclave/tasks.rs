//! 飞地任务域：**两层任务结构**（父任务 = 容器，子任务 = 原子交付单元）。
//!
//! 约束（设计 §5 / 最终决策）：
//! - **只有一层父子**：父任务必须是没有父任务的根任务；不做多级 WBS。
//! - **单一负责人**：`assignee_id` 一个人；协作通过 `collaborators` 与权限实现。
//! - **多前置依赖**：`blocked_by` 是任务 id 数组，**纯 AND 语义**（全部完成才解锁）；
//!   `archived` 视作已放行；写入前做 **DFS 三色环检测**。
//! - 所有写操作先过 [`crate::enclave::authz`]（组织角色 + 资源角色，默认拒绝）。

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::enclave::authz::{self, Action, Role};

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_IN_PROGRESS: &str = "in_progress";
pub const STATUS_SUBMITTED: &str = "submitted";
pub const STATUS_DONE: &str = "done";
pub const STATUS_ARCHIVED: &str = "archived";
/// 进行中可被创建者 / 负责人作废（放弃执行）；作废为终态，不再继续流转。
pub const STATUS_CANCELLED: &str = "cancelled";

/// 任务资源角色（由任务字段自动绑定，例外才走 `resource_roles`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskRole {
    /// 组织 owner/admin：具备管理视角
    Manager,
    Creator,
    Assignee,
    Collaborator,
    /// 组织内其它成员：默认只看
    Watcher,
}

/// 资源级动作矩阵（默认拒绝）。
///
/// 口径（产品确认）：
/// - 编辑（描述 / 优先级 / 截止）：仅**创建者 / 负责人**
/// - 分配负责人：仅**创建者**
/// - 状态流转：仅**创建者 / 负责人**
/// - 汇报 / 附件：创建者、负责人、参与者（collaborators）
pub fn task_allows(role: TaskRole, action: Action) -> bool {
    match action {
        Action::TaskView => true,
        Action::TaskEdit => matches!(role, TaskRole::Creator | TaskRole::Assignee),
        Action::TaskAssign => role == TaskRole::Creator,
        Action::TaskStatus => matches!(role, TaskRole::Creator | TaskRole::Assignee),
        Action::ReportSubmit | Action::FileUpload => matches!(
            role,
            TaskRole::Creator | TaskRole::Assignee | TaskRole::Collaborator
        ),
        _ => false,
    }
}

/// 参与者 = 创建者、负责人、协作者（可提交汇报 / 上传附件 / 记录进度）。
pub fn is_participant(role: TaskRole) -> bool {
    matches!(
        role,
        TaskRole::Creator | TaskRole::Assignee | TaskRole::Collaborator
    )
}

/// 任务中与「谁有资格」相关的字段快照。
pub struct TaskActors<'a> {
    pub creator_id: &'a str,
    pub assignee_id: Option<&'a str>,
    pub collaborators: &'a [String],
}

/// 按组织角色 + 任务字段解析资源角色。
///
/// **顺序很重要**：先绑定任务级身份（创建者 → 负责人 → 参与者），再退回组织管理视角。
/// 否则 owner/admin 会被 `Manager` 短路，导致「创建者」拿不到编辑/改状态权限。
pub fn resolve_task_role(org_role: Role, actor: &str, t: &TaskActors<'_>) -> TaskRole {
    if t.creator_id == actor {
        return TaskRole::Creator;
    }
    if t.assignee_id == Some(actor) {
        return TaskRole::Assignee;
    }
    if t.collaborators.iter().any(|c| c == actor) {
        return TaskRole::Collaborator;
    }
    if matches!(org_role, Role::Owner | Role::Admin) {
        return TaskRole::Manager;
    }
    TaskRole::Watcher
}

/// 状态机：返回可读错误。`archived` 只作终态（可从任意非归档态归档）。
///
/// 权限：状态归属**创建者 / 负责人**；归档与恢复仅创建者。
pub fn can_transition(from: &str, to: &str, role: TaskRole) -> Result<(), String> {
    if from == to {
        return Err(format!("任务已处于「{}」状态", status_label(to)));
    }
    let owner = matches!(role, TaskRole::Creator | TaskRole::Assignee);
    let participant = owner || role == TaskRole::Collaborator;
    let creator = role == TaskRole::Creator;
    let ok = match (from, to) {
        (STATUS_PENDING, STATUS_IN_PROGRESS) => owner,
        (STATUS_IN_PROGRESS, STATUS_PENDING) => owner,
        // 作废：进行中放弃执行（创建者 / 负责人）
        (STATUS_IN_PROGRESS, STATUS_CANCELLED) => owner,
        // 提交汇报（M4 联动 task_reports 四态）：参与者（含协作者）都可提交
        (STATUS_IN_PROGRESS, STATUS_SUBMITTED) => participant,
        // 确认 / 退回
        (STATUS_SUBMITTED, STATUS_DONE) => owner,
        (STATUS_SUBMITTED, STATUS_IN_PROGRESS) => owner,
        // 恢复汇报：已完成 / 已归档 → 进行中（补报后再提交；需要时再「放回待办」）
        (STATUS_DONE, STATUS_IN_PROGRESS) => owner,
        (STATUS_ARCHIVED, STATUS_IN_PROGRESS) => owner,
        // 归档只从「已完成」进入：收纳已收敛任务（列表默认隐藏、只读）
        (STATUS_DONE, STATUS_ARCHIVED) => creator,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "当前角色不允许把任务从「{}」变更为「{}」",
            status_label(from),
            status_label(to)
        ))
    }
}

pub fn status_label(s: &str) -> &'static str {
    match s {
        STATUS_PENDING => "待办",
        STATUS_IN_PROGRESS => "进行中",
        STATUS_SUBMITTED => "待确认",
        STATUS_DONE => "已完成",
        STATUS_ARCHIVED => "已归档",
        STATUS_CANCELLED => "已作废",
        _ => "未知",
    }
}

/// 已完成 / 已归档 / 已作废：任务收敛，内容只读。
pub fn is_closed_status(s: &str) -> bool {
    s == STATUS_DONE || s == STATUS_ARCHIVED || s == STATUS_CANCELLED
}

// ── 视图结构 ────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: String,
    pub org_id: String,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub description: String,
    pub creator_id: String,
    pub assignee_id: Option<String>,
    pub collaborators: Vec<String>,
    pub status: String,
    pub priority: String,
    pub due_date: Option<i64>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub blocked_by: Vec<String>,
    pub visibility: String,
    /// 当前是否被前置任务阻塞（archived 视作已放行）
    pub blocked: bool,
    /// 前端渲染按钮的唯一来源
    pub effective_actions: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub due_date: Option<i64>,
    #[serde(default)]
    pub blocked_by: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskInput {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub assignee_id: Option<Option<String>>,
    #[serde(default)]
    pub collaborators: Option<Vec<String>>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub due_date: Option<Option<i64>>,
}

/// 依赖变更预览：先给出影响，再由用户确认保存。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepsPreview {
    /// 命中环的任务 id 序列（空 = 无环）
    pub cycle_ids: Vec<String>,
    /// 保存后新变为阻塞的任务
    pub newly_blocked_ids: Vec<String>,
    /// 保存后新解除阻塞的任务
    pub newly_unblocked_ids: Vec<String>,
    /// 错误（自依赖 / 跨组织 / 环）——非空时 `cycle_ids` 仍可用于高亮
    pub error: Option<String>,
}

// ── 行读取 ──────────────────────────────────────────────────────

struct TaskRow {
    id: String,
    org_id: String,
    parent_task_id: Option<String>,
    title: String,
    description: String,
    creator_id: String,
    assignee_id: Option<String>,
    collaborators: Vec<String>,
    status: String,
    priority: String,
    due_date: Option<i64>,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    blocked_by: Vec<String>,
    visibility: String,
    created_at: i64,
    updated_at: i64,
}

fn parse_ids(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn ids_json(ids: &[String]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_string())
}

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    let collaborators: String = r.get(7)?;
    let blocked_by: String = r.get(13)?;
    Ok(TaskRow {
        id: r.get(0)?,
        org_id: r.get(1)?,
        parent_task_id: r.get(2)?,
        title: r.get(3)?,
        description: r.get(4)?,
        creator_id: r.get(5)?,
        assignee_id: r.get(6)?,
        collaborators: parse_ids(&collaborators),
        status: r.get(8)?,
        priority: r.get(9)?,
        due_date: r.get(10)?,
        started_at: r.get(11)?,
        completed_at: r.get(12)?,
        blocked_by: parse_ids(&blocked_by),
        visibility: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

const SELECT_COLS: &str = "id, org_id, parent_task_id, title, description, creator_id, assignee_id,
     collaborators, status, priority, due_date, started_at, completed_at, blocked_by,
     visibility, created_at, updated_at";

fn load_task(conn: &Connection, org_id: &str, task_id: &str) -> Result<TaskRow, String> {
    conn.query_row(
        &format!("SELECT {} FROM org_tasks WHERE id = ?1 AND org_id = ?2", SELECT_COLS),
        params![task_id, org_id],
        map_row,
    )
    .optional()
    .map_err(|e| format!("查询任务失败：{}", e))?
    .ok_or_else(|| "任务不存在或不属于该组织".to_string())
}

/// 全量列出某组织任务（前端按 `parentTaskId` 组树）。
pub fn list_tasks(conn: &Connection, org_id: &str, actor: &str) -> Result<Vec<TaskView>, String> {
    let org_role = require_org_role(conn, org_id)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM org_tasks WHERE org_id = ?1 ORDER BY created_at ASC",
            SELECT_COLS
        ))
        .map_err(|e| format!("查询任务失败：{}", e))?;
    let rows: Vec<TaskRow> = stmt
        .query_map(params![org_id], map_row)
        .map_err(|e| format!("查询任务失败：{}", e))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取任务失败：{}", e))?;

    let status_by_id: HashMap<String, String> = rows
        .iter()
        .map(|r| (r.id.clone(), r.status.clone()))
        .collect();

    Ok(rows
        .iter()
        .map(|r| to_view(org_role, actor, r, &status_by_id))
        .collect())
}

pub fn get_task(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    actor: &str,
) -> Result<TaskView, String> {
    let org_role = require_org_role(conn, org_id)?;
    let row = load_task(conn, org_id, task_id)?;
    let status_by_id = all_status(conn, org_id)?;
    Ok(to_view(org_role, actor, &row, &status_by_id))
}

fn all_status(conn: &Connection, org_id: &str) -> Result<HashMap<String, String>, String> {
    let mut stmt = conn
        .prepare("SELECT id, status FROM org_tasks WHERE org_id = ?1")
        .map_err(|e| format!("查询任务失败：{}", e))?;
    let map = stmt
        .query_map(params![org_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("查询任务失败：{}", e))?
        .collect::<rusqlite::Result<HashMap<_, _>>>()
        .map_err(|e| format!("读取任务失败：{}", e))?;
    Ok(map)
}

fn to_view(
    org_role: Role,
    actor: &str,
    r: &TaskRow,
    status_by_id: &HashMap<String, String>,
) -> TaskView {
    let role = resolve_task_role(
        org_role,
        actor,
        &TaskActors {
            creator_id: &r.creator_id,
            assignee_id: r.assignee_id.as_deref(),
            collaborators: &r.collaborators,
        },
    );
    let blocked = is_blocked(&r.blocked_by, status_by_id);
    let mut actions = vec![Action::TaskView.as_str().to_string()];
    for a in [
        Action::TaskEdit,
        Action::TaskStatus,
        Action::TaskAssign,
        Action::ReportSubmit,
        Action::FileUpload,
    ] {
        if task_allows(role, a) {
            actions.push(a.as_str().to_string());
        }
    }
    TaskView {
        id: r.id.clone(),
        org_id: r.org_id.clone(),
        parent_task_id: r.parent_task_id.clone(),
        title: r.title.clone(),
        description: r.description.clone(),
        creator_id: r.creator_id.clone(),
        assignee_id: r.assignee_id.clone(),
        collaborators: r.collaborators.clone(),
        status: r.status.clone(),
        priority: r.priority.clone(),
        due_date: r.due_date,
        started_at: r.started_at,
        completed_at: r.completed_at,
        blocked_by: r.blocked_by.clone(),
        visibility: r.visibility.clone(),
        blocked,
        effective_actions: actions,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

// ── 依赖判定 ────────────────────────────────────────────────────

/// 纯 AND：非空且存在未完成（done/archived 之外）的前置 → 阻塞。
pub fn is_blocked(blocked_by: &[String], status_by_id: &HashMap<String, String>) -> bool {
    blocked_by.iter().any(|id| {
        status_by_id
            .get(id)
            .map(|s| s != STATUS_DONE && s != STATUS_ARCHIVED)
            .unwrap_or(false)
    })
}

/// 校验新依赖：去重、拒自依赖、必须同组织存在；返回规范化后的列表。
pub fn validate_dep_ids(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    deps: &[String],
) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in deps {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        if id == task_id {
            return Err("任务不能依赖自己".to_string());
        }
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM org_tasks WHERE id = ?1 AND org_id = ?2",
                params![id, org_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("校验前置任务失败：{}", e))?;
        if exists.is_none() {
            return Err(format!("前置任务不存在或不属于该组织：{}", id));
        }
        if seen.insert(id.to_string()) {
            out.push(id.to_string());
        }
    }
    Ok(out)
}

/// 若把 `deps` 设为 `task_id` 的前置会成环，返回环上的任务 id 序列（含 `task_id`）。
///
/// 环检测沿 `blocked_by` 有向边做 DFS：新增 d → task 若已存在 task → … → d 的路径即成环。
pub fn detect_cycle(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    deps: &[String],
) -> Result<Option<Vec<String>>, String> {
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, blocked_by FROM org_tasks WHERE org_id = ?1")
            .map_err(|e| format!("查询依赖失败：{}", e))?;
        let rows = stmt
            .query_map(params![org_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| format!("查询依赖失败：{}", e))?;
        for row in rows {
            let (id, raw) = row.map_err(|e| format!("读取依赖失败：{}", e))?;
            edges.insert(id, parse_ids(&raw));
        }
    }
    // 用「待保存的新边」覆盖当前任务的依赖
    edges.insert(task_id.to_string(), deps.to_vec());

    for dep in deps {
        if let Some(path) = find_path(&edges, dep, task_id) {
            let mut cycle = vec![task_id.to_string()];
            cycle.extend(path);
            return Ok(Some(cycle));
        }
    }
    Ok(None)
}

/// 沿 `blocked_by` 边做迭代 DFS，返回 from → target 的路径（含两端）。
fn find_path(
    edges: &HashMap<String, Vec<String>>,
    from: &str,
    target: &str,
) -> Option<Vec<String>> {
    if from == target {
        return Some(vec![from.to_string()]);
    }
    let mut stack: Vec<(String, Vec<String>)> = vec![(from.to_string(), vec![from.to_string()])];
    let mut visited: HashSet<String> = HashSet::new();
    while let Some((node, path)) = stack.pop() {
        if !visited.insert(node.clone()) {
            continue;
        }
        for next in edges.get(&node).map(|v| v.as_slice()).unwrap_or(&[]) {
            if next == target {
                let mut p = path.clone();
                p.push(target.to_string());
                return Some(p);
            }
            if !visited.contains(next) {
                let mut p = path.clone();
                p.push(next.clone());
                stack.push((next.clone(), p));
            }
        }
    }
    None
}

/// 依赖变更预览：指出环、以及保存后「新阻塞 / 新解锁」的任务。
pub fn deps_preview(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    deps: &[String],
) -> Result<DepsPreview, String> {
    let normalized = match validate_dep_ids(conn, org_id, task_id, deps) {
        Ok(v) => v,
        Err(e) => {
            return Ok(DepsPreview {
                cycle_ids: Vec::new(),
                newly_blocked_ids: Vec::new(),
                newly_unblocked_ids: Vec::new(),
                error: Some(e),
            })
        }
    };
    let status_by_id = all_status(conn, org_id)?;
    let current = load_task(conn, org_id, task_id)?.blocked_by;
    let was = is_blocked(&current, &status_by_id);
    let now = is_blocked(&normalized, &status_by_id);
    if let Some(cycle) = detect_cycle(conn, org_id, task_id, &normalized)? {
        return Ok(DepsPreview {
            cycle_ids: cycle,
            newly_blocked_ids: Vec::new(),
            newly_unblocked_ids: Vec::new(),
            error: Some("存在循环依赖，无法保存".to_string()),
        });
    }
    Ok(DepsPreview {
        cycle_ids: Vec::new(),
        newly_blocked_ids: if now && !was { vec![task_id.to_string()] } else { Vec::new() },
        newly_unblocked_ids: if was && !now { vec![task_id.to_string()] } else { Vec::new() },
        error: None,
    })
}

// ── 写操作 ──────────────────────────────────────────────────────

fn require_org_role(conn: &Connection, org_id: &str) -> Result<Role, String> {
    authz::role_for_org(conn, org_id)?.ok_or_else(|| {
        "尚未同步该组织的治理快照，治理已冻结：请先在「组织与权限」页点击刷新".to_string()
    })
}

/// 建任务（含子任务）。父任务必须是**根任务**（两层结构）。
pub fn create_task(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    input: &CreateTaskInput,
) -> Result<TaskView, String> {
    let org_role = require_org_role(conn, org_id)?;
    let title = input.title.trim();
    if title.is_empty() {
        return Err("任务标题不能为空".to_string());
    }
    if let Some(parent) = input.parent_task_id.as_deref().filter(|s| !s.is_empty()) {
        let parent_row = load_task(conn, org_id, parent)?;
        if parent_row.parent_task_id.is_some() {
            return Err("只支持一层父子结构：父任务本身不能是子任务".to_string());
        }
    }
    let deps = input.blocked_by.clone().unwrap_or_default();
    let task_id = crate::utils::new_id();
    let deps = validate_dep_ids(conn, org_id, &task_id, &deps)?;
    if let Some(cycle) = detect_cycle(conn, org_id, &task_id, &deps)? {
        return Err(format!("存在循环依赖：{}", cycle.join(" → ")));
    }

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO org_tasks
            (id, org_id, parent_task_id, title, description, creator_id, assignee_id,
             collaborators, status, priority, due_date, started_at, completed_at,
             blocked_by, visibility, metadata, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '[]', ?8, ?9, ?10, NULL, NULL, ?11, 'org', '{}', ?12, ?12)",
        params![
            task_id,
            org_id,
            input.parent_task_id.as_deref().filter(|s| !s.is_empty()),
            title,
            input.description,
            actor,
            input.assignee_id.as_deref().filter(|s| !s.is_empty()),
            STATUS_PENDING,
            input.priority.as_deref().filter(|s| !s.is_empty()).unwrap_or("normal"),
            input.due_date,
            ids_json(&deps),
            now
        ],
    )
    .map_err(|e| format!("创建任务失败：{}", e))?;

    let _ = org_role;
    get_task(conn, org_id, &task_id, actor)
}

/// 编辑任务字段（权限：manager / creator / assignee / collaborator）。
pub fn update_task(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    input: &UpdateTaskInput,
) -> Result<TaskView, String> {
    let row = load_task(conn, org_id, task_id)?;
    if is_closed_status(&row.status) {
        return Err(format!("任务已{}，不能再修改", status_label(&row.status)));
    }
    let org_role = require_org_role(conn, org_id)?;
    let role = resolve_task_role(
        org_role,
        actor,
        &TaskActors {
            creator_id: &row.creator_id,
            assignee_id: row.assignee_id.as_deref(),
            collaborators: &row.collaborators,
        },
    );
    if !task_allows(role, Action::TaskEdit) {
        return Err("当前角色无权编辑该任务".to_string());
    }

    let title = input
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&row.title);
    let description = input.description.as_deref().unwrap_or(&row.description);
    let priority = input
        .priority
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&row.priority);

    // 负责人变更属于「分配」，需 assign 权限
    let assignee = match &input.assignee_id {
        Some(next) => {
            if !task_allows(role, Action::TaskAssign) {
                return Err("当前角色无权变更任务负责人".to_string());
            }
            next.clone()
        }
        None => row.assignee_id.clone(),
    };
    let collaborators = input
        .collaborators
        .clone()
        .unwrap_or_else(|| row.collaborators.clone());
    let due_date = match input.due_date {
        Some(next) => next,
        None => row.due_date,
    };
    if let Some(c) = &assignee {
        if c.trim().is_empty() {
            // 空串统一视为「取消负责人」
        }
    }
    let assignee = assignee.filter(|s| !s.trim().is_empty());

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE org_tasks SET title=?3, description=?4, assignee_id=?5, collaborators=?6,
                priority=?7, due_date=?8, updated_at=?9
         WHERE id=?1 AND org_id=?2",
        params![
            task_id,
            org_id,
            title,
            description,
            assignee,
            ids_json(&collaborators),
            priority,
            due_date,
            now
        ],
    )
    .map_err(|e| format!("更新任务失败：{}", e))?;
    get_task(conn, org_id, task_id, actor)
}

/// 状态流转（含 `started_at` / `completed_at` 联动；提交/确认在 M4 接汇报后补联动）。
pub fn set_status(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    to: &str,
) -> Result<TaskView, String> {
    if ![
        STATUS_PENDING,
        STATUS_IN_PROGRESS,
        STATUS_SUBMITTED,
        STATUS_DONE,
        STATUS_ARCHIVED,
        STATUS_CANCELLED,
    ]
    .contains(&to)
    {
        return Err(format!("未知任务状态：{}", to));
    }
    let row = load_task(conn, org_id, task_id)?;
    let org_role = require_org_role(conn, org_id)?;
    let role = resolve_task_role(
        org_role,
        actor,
        &TaskActors {
            creator_id: &row.creator_id,
            assignee_id: row.assignee_id.as_deref(),
            collaborators: &row.collaborators,
        },
    );
    can_transition(&row.status, to, role)?;

    // 前置依赖只在「待办 → 进行中」时检查；退回（submitted → in_progress）不该被依赖拦住，
    // 否则有前置任务的子任务一旦提交汇报就永远退不回来。
    if row.status == STATUS_PENDING && to == STATUS_IN_PROGRESS {
        let status_by_id = all_status(conn, org_id)?;
        if is_blocked(&row.blocked_by, &status_by_id) {
            let names: Vec<String> = row
                .blocked_by
                .iter()
                .filter_map(|id| status_by_id.get(id).map(|s| format!("{}（{}）", id, status_label(s))))
                .collect();
            return Err(format!(
                "存在未完成的前置任务，暂不能开始：{}",
                names.join("、")
            ));
        }
    }

    let now = chrono::Utc::now().timestamp();
    let started_at = if to == STATUS_IN_PROGRESS {
        row.started_at.or(Some(now))
    } else {
        row.started_at
    };
    let completed_at = if to == STATUS_DONE {
        Some(now)
    } else if to == STATUS_PENDING || to == STATUS_IN_PROGRESS {
        // 回到待办 / 进行中（含恢复汇报）都视为重新打开，清掉完成时间
        None
    } else {
        row.completed_at
    };

    conn.execute(
        "UPDATE org_tasks SET status=?3, started_at=?4, completed_at=?5, updated_at=?6
         WHERE id=?1 AND org_id=?2",
        params![task_id, org_id, to, started_at, completed_at, now],
    )
    .map_err(|e| format!("更新任务状态失败：{}", e))?;
    get_task(conn, org_id, task_id, actor)
}

/// 设置前置依赖（先校验 + 环检测，再落库）。
pub fn set_deps(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    deps: &[String],
) -> Result<TaskView, String> {
    let row = load_task(conn, org_id, task_id)?;
    if is_closed_status(&row.status) {
        return Err(format!("任务已{}，不能再修改依赖", status_label(&row.status)));
    }
    let org_role = require_org_role(conn, org_id)?;
    let role = resolve_task_role(
        org_role,
        actor,
        &TaskActors {
            creator_id: &row.creator_id,
            assignee_id: row.assignee_id.as_deref(),
            collaborators: &row.collaborators,
        },
    );
    if !task_allows(role, Action::TaskEdit) {
        return Err("当前角色无权修改该任务的依赖".to_string());
    }
    let normalized = validate_dep_ids(conn, org_id, task_id, deps)?;
    if let Some(cycle) = detect_cycle(conn, org_id, task_id, &normalized)? {
        return Err(format!("存在循环依赖：{}", cycle.join(" → ")));
    }
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE org_tasks SET blocked_by=?3, updated_at=?4 WHERE id=?1 AND org_id=?2",
        params![task_id, org_id, ids_json(&normalized), now],
    )
    .map_err(|e| format!("保存依赖失败：{}", e))?;
    get_task(conn, org_id, task_id, actor)
}

/// 设置任务参与者（collaborators）：仅创建者 / 负责人；自动剔除创建者与负责人。
pub fn set_collaborators(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    collaborators: &[String],
) -> Result<TaskView, String> {
    let row = load_task(conn, org_id, task_id)?;
    if is_closed_status(&row.status) {
        return Err(format!("任务已{}，不能再调整参与者", status_label(&row.status)));
    }
    let org_role = require_org_role(conn, org_id)?;
    let role = resolve_task_role(
        org_role,
        actor,
        &TaskActors {
            creator_id: &row.creator_id,
            assignee_id: row.assignee_id.as_deref(),
            collaborators: &row.collaborators,
        },
    );
    if !matches!(role, TaskRole::Creator | TaskRole::Assignee) {
        return Err("仅任务创建者或负责人可以添加参与者".to_string());
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in collaborators {
        let id = id.trim();
        if id.is_empty()
            || id == row.creator_id
            || row.assignee_id.as_deref() == Some(id)
        {
            continue;
        }
        if seen.insert(id.to_string()) {
            out.push(id.to_string());
        }
    }
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE org_tasks SET collaborators=?3, updated_at=?4 WHERE id=?1 AND org_id=?2",
        params![task_id, org_id, ids_json(&out), now],
    )
    .map_err(|e| format!("保存参与者失败：{}", e))?;
    get_task(conn, org_id, task_id, actor)
}

/// 当前 actor 在该任务上的资源角色（供汇报域复用，避免重复实现绑定规则）。
pub fn actor_task_role(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
    actor: &str,
) -> Result<TaskRole, String> {
    let row = load_task(conn, org_id, task_id)?;
    let org_role = require_org_role(conn, org_id)?;
    Ok(resolve_task_role(
        org_role,
        actor,
        &TaskActors {
            creator_id: &row.creator_id,
            assignee_id: row.assignee_id.as_deref(),
            collaborators: &row.collaborators,
        },
    ))
}

/// 读任务当前状态（不存在/跨组织即报错）。
pub fn task_status(conn: &Connection, org_id: &str, task_id: &str) -> Result<String, String> {
    Ok(load_task(conn, org_id, task_id)?.status)
}

/// 任务当前是否被前置依赖阻塞（用于提交前复核）。
pub fn blocked_now(conn: &Connection, org_id: &str, task_id: &str) -> Result<bool, String> {
    let row = load_task(conn, org_id, task_id)?;
    let status_by_id = all_status(conn, org_id)?;
    Ok(is_blocked(&row.blocked_by, &status_by_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn transition_matrix() {
        // 待办 → 进行中：创建者 / 负责人可；管理视角与非参与者不可
        assert!(can_transition(STATUS_PENDING, STATUS_IN_PROGRESS, TaskRole::Assignee).is_ok());
        assert!(can_transition(STATUS_PENDING, STATUS_IN_PROGRESS, TaskRole::Creator).is_ok());
        assert!(can_transition(STATUS_PENDING, STATUS_IN_PROGRESS, TaskRole::Manager).is_err());
        assert!(can_transition(STATUS_PENDING, STATUS_IN_PROGRESS, TaskRole::Collaborator).is_err());
        // 进行中 → 待确认：参与者（创建者 / 负责人 / 协作者）都可提交汇报
        assert!(can_transition(STATUS_IN_PROGRESS, STATUS_SUBMITTED, TaskRole::Assignee).is_ok());
        assert!(can_transition(STATUS_IN_PROGRESS, STATUS_SUBMITTED, TaskRole::Creator).is_ok());
        assert!(can_transition(STATUS_IN_PROGRESS, STATUS_SUBMITTED, TaskRole::Collaborator).is_ok());
        assert!(can_transition(STATUS_IN_PROGRESS, STATUS_SUBMITTED, TaskRole::Watcher).is_err());
        // 待确认 → 已完成 / 退回
        assert!(can_transition(STATUS_SUBMITTED, STATUS_DONE, TaskRole::Creator).is_ok());
        assert!(can_transition(STATUS_SUBMITTED, STATUS_DONE, TaskRole::Assignee).is_ok());
        assert!(can_transition(STATUS_SUBMITTED, STATUS_DONE, TaskRole::Collaborator).is_err());
        assert!(can_transition(STATUS_SUBMITTED, STATUS_IN_PROGRESS, TaskRole::Creator).is_ok());
        // 归档仅创建者
        assert!(can_transition(STATUS_DONE, STATUS_ARCHIVED, TaskRole::Creator).is_ok());
        assert!(can_transition(STATUS_DONE, STATUS_ARCHIVED, TaskRole::Assignee).is_err());
        // 作废：进行中（创建者 / 负责人）
        assert!(can_transition(STATUS_IN_PROGRESS, STATUS_CANCELLED, TaskRole::Creator).is_ok());
        assert!(can_transition(STATUS_IN_PROGRESS, STATUS_CANCELLED, TaskRole::Assignee).is_ok());
        assert!(can_transition(STATUS_IN_PROGRESS, STATUS_CANCELLED, TaskRole::Collaborator).is_err());
        // 作废为终态
        assert!(can_transition(STATUS_CANCELLED, STATUS_IN_PROGRESS, TaskRole::Creator).is_err());
        // 恢复汇报：已完成 / 已归档 → 进行中（创建者 / 负责人）
        assert!(can_transition(STATUS_DONE, STATUS_IN_PROGRESS, TaskRole::Assignee).is_ok());
        assert!(can_transition(STATUS_ARCHIVED, STATUS_IN_PROGRESS, TaskRole::Creator).is_ok());
        assert!(can_transition(STATUS_ARCHIVED, STATUS_IN_PROGRESS, TaskRole::Collaborator).is_err());
        // 同态与非法跃迁
        assert!(can_transition(STATUS_DONE, STATUS_DONE, TaskRole::Creator).is_err());
        assert!(can_transition(STATUS_PENDING, STATUS_DONE, TaskRole::Creator).is_err());
        // 恢复汇报后若需回到待办，走「进行中 → 待办」
        assert!(can_transition(STATUS_IN_PROGRESS, STATUS_PENDING, TaskRole::Assignee).is_ok());
    }

    /// 权限矩阵：描述/属性仅创建者与负责人；分配负责人仅创建者；汇报/附件含参与者。
    #[test]
    fn task_action_matrix() {
        for role in [TaskRole::Creator, TaskRole::Assignee] {
            assert!(task_allows(role, Action::TaskEdit));
            assert!(task_allows(role, Action::TaskStatus));
        }
        assert!(task_allows(TaskRole::Creator, Action::TaskAssign));
        assert!(!task_allows(TaskRole::Assignee, Action::TaskAssign));
        for role in [TaskRole::Manager, TaskRole::Collaborator, TaskRole::Watcher] {
            assert!(!task_allows(role, Action::TaskEdit), "{:?} 不应能编辑", role);
            assert!(!task_allows(role, Action::TaskStatus), "{:?} 不应能改状态", role);
        }
        for role in [TaskRole::Creator, TaskRole::Assignee, TaskRole::Collaborator] {
            assert!(task_allows(role, Action::ReportSubmit));
            assert!(task_allows(role, Action::FileUpload));
        }
        assert!(!task_allows(TaskRole::Watcher, Action::ReportSubmit));
        assert!(!task_allows(TaskRole::Watcher, Action::FileUpload));
    }

    /// 组织 owner 身为任务创建者时，必须解析为 Creator（否则会丢编辑入口）。
    #[test]
    fn creator_takes_precedence_over_org_manager() {
        let collaborators = Vec::new();
        let actors = TaskActors {
            creator_id: "1",
            assignee_id: Some("2"),
            collaborators: &collaborators,
        };
        assert_eq!(resolve_task_role(Role::Owner, "1", &actors), TaskRole::Creator);
        assert_eq!(resolve_task_role(Role::Owner, "2", &actors), TaskRole::Assignee);
        // 非任务相关人的 owner/admin 才是 Manager
        assert_eq!(resolve_task_role(Role::Owner, "9", &actors), TaskRole::Manager);
        assert_eq!(resolve_task_role(Role::Member, "9", &actors), TaskRole::Watcher);
    }

    #[test]
    fn blocked_is_and_semantics() {
        let mut status = HashMap::new();
        status.insert("a".to_string(), STATUS_DONE.to_string());
        status.insert("b".to_string(), STATUS_IN_PROGRESS.to_string());
        status.insert("c".to_string(), STATUS_ARCHIVED.to_string());
        // 空列表 → 不阻塞
        assert!(!is_blocked(&[], &status));
        // 任一未完成 → 阻塞（纯 AND）
        assert!(is_blocked(&ids(&["a", "b"]), &status));
        // done / archived 均放行
        assert!(!is_blocked(&ids(&["a", "c"]), &status));
    }

    #[test]
    fn path_detection_finds_cycle() {
        // t → a → b → t
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        edges.insert("a".to_string(), ids(&["b"]));
        edges.insert("b".to_string(), ids(&["t"]));
        let path = find_path(&edges, "a", "t").expect("应找到 a→b→t");
        assert_eq!(path, ids(&["a", "b", "t"]));
        assert!(find_path(&edges, "t", "a").is_none());
    }
}
