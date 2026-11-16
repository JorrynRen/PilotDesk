//! 项目管理工作归属。
//!
//! 项目不是新的工作模式：会话、圆桌、工作流仍各自运行，只通过可空 `project_id`
//! 建立归属。主目录是归属工作的默认工作根，主目录与关联目录均动态参与运行时安全路径
//! 判定；普通工作不复制项目主目录快照。ACP 宿主导入会话仅额外保存恢复所需目录。

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::utils::errors::AppError;
use crate::DbState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDirectory {
    pub id: String,
    pub project_id: String,
    pub path: String,
    pub is_primary: bool,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub archived_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_activity_at: i64,
    pub directories: Vec<ProjectDirectory>,
    pub session_count: i64,
    pub room_count: i64,
    pub workflow_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDirectoryInput {
    pub path: String,
    #[serde(default)]
    pub is_primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub directories: Vec<ProjectDirectoryInput>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorkItem {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub status: String,
    pub updated_at: i64,
}

/// 项目运行时工作区上下文。
///
/// 主目录承担工作根语义；主目录与全部关联目录统一作为 `SecurityMode` 下的
/// “安全路径”。这里不引入独立目录 ACL。
#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectWorkspaceContext {
    pub(crate) primary_root: Option<String>,
    pub(crate) linked_roots: Vec<String>,
}

/// 一次运行实际工作根的来源。
///
/// 只有三种来源；用户文档目录属于 `Global` 的内部兜底实现，不单独暴露。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceSource {
    HostBound,
    Project,
    Global,
}

/// 一次运行已经解析完成的工作区语义。
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceResolution {
    /// 本次运行的唯一 cwd / 相对路径基准。
    pub(crate) work_root: String,
    /// 项目归属的稳定主目录；宿主绑定或全局工作空间时可能为空。
    pub(crate) project_root: Option<String>,
    /// 项目关联目录，只进入安全路径，不改变 cwd。
    pub(crate) linked_roots: Vec<String>,
    /// 项目/全局记忆根。宿主绑定不会改变它。
    pub(crate) memory_root: String,
    pub(crate) source: WorkspaceSource,
    /// 当前全局工作空间是否由用户显式配置；false 表示使用用户可见兜底目录。
    pub(crate) global_workspace_configured: bool,
}

/// 可消费工作区的业务对象。
///
/// 三种对象共享同一套工作目录语义，差异只在归属元数据的读取位置与产物目录兜底名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceOwnerKind {
    Session,
    Room,
    Workflow,
}

impl WorkspaceOwnerKind {
    fn artifact_fallback(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Room => "room",
            Self::Workflow => "workflow",
        }
    }

    fn missing_error(self) -> AppError {
        match self {
            Self::Session => AppError::NotFound("会话不存在".into()),
            Self::Room => AppError::NotFound("圆桌不存在".into()),
            Self::Workflow => AppError::NotFound("工作流不存在".into()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WorkspaceOwner<'a> {
    pub(crate) kind: WorkspaceOwnerKind,
    pub(crate) id: &'a str,
}

impl<'a> WorkspaceOwner<'a> {
    pub(crate) fn session(id: &'a str) -> Self {
        Self {
            kind: WorkspaceOwnerKind::Session,
            id,
        }
    }

    pub(crate) fn room(id: &'a str) -> Self {
        Self {
            kind: WorkspaceOwnerKind::Room,
            id,
        }
    }

    pub(crate) fn workflow(id: &'a str) -> Self {
        Self {
            kind: WorkspaceOwnerKind::Workflow,
            id,
        }
    }
}

/// 业务对象及其唯一解析后的工作区上下文。
#[derive(Debug, Clone)]
pub(crate) struct ResolvedWorkspace {
    pub(crate) owner_kind: WorkspaceOwnerKind,
    pub(crate) owner_id: String,
    pub(crate) owner_title: String,
    pub(crate) project_id: Option<String>,
    pub(crate) workspace: WorkspaceResolution,
}

impl ResolvedWorkspace {
    /// 从当前工作根派生本次运行的产物目录。
    pub(crate) fn artifact_root(&self) -> String {
        self.artifact_root_for_name(&self.owner_title)
    }

    /// 圆桌允许使用主持人给出的短目录名，但仍只从统一工作根派生。
    pub(crate) fn artifact_root_for_name(&self, owner_name: &str) -> String {
        self.workspace
            .artifact_root(owner_name, self.owner_kind.artifact_fallback())
    }
}

impl WorkspaceResolution {
    /// 运行时安全根：项目主目录、关联目录、实际工作根与本次额外根。
    pub(crate) fn runtime_roots(
        &self,
        extra_roots: impl IntoIterator<Item = String>,
    ) -> Vec<String> {
        let mut roots = Vec::new();
        if let Some(project_root) = self.project_root.clone() {
            roots.push(project_root);
        }
        roots.extend(self.linked_roots.clone());
        roots.push(self.work_root.clone());
        roots.extend(extra_roots);

        let mut seen = HashSet::new();
        roots
            .into_iter()
            .filter(|root| !root.trim().is_empty())
            .filter(|root| {
                seen.insert(crate::utils::paths::path_key(std::path::Path::new(
                    root.trim(),
                )))
            })
            .collect()
    }

    /// 从本次实际工作根派生产物目录。
    ///
    /// 产物目录不是新的工作根，也不持久化；它只作为生成文件的默认落点与额外安全根。
    pub(crate) fn artifact_root(&self, owner_name: &str, fallback: &str) -> String {
        crate::utils::paths::workspace_artifacts_dir(&self.work_root, owner_name, fallback)
            .to_string_lossy()
            .to_string()
    }
}

impl ProjectWorkspaceContext {
    /// 解析不依赖宿主绑定的工作根：项目主目录 -> 全局工作空间。
    ///
    /// 这是普通会话、圆桌与工作流共用的唯一非宿主入口。调用方不能传入任意目录，
    /// 否则会重新引入已删除的“工作独立 cwd”语义。
    pub(crate) fn resolve_owned_workspace(&self, conn: &Connection) -> WorkspaceResolution {
        let global = crate::utils::paths::resolve_global_workspace(conn);
        if let Some(primary_root) = self.primary_root.as_deref() {
            let primary_root = crate::utils::paths::normalize_absolute_path(primary_root, "")
                .to_string_lossy()
                .to_string();
            return WorkspaceResolution {
                work_root: primary_root.clone(),
                project_root: Some(primary_root.clone()),
                linked_roots: self.linked_roots.clone(),
                memory_root: primary_root,
                source: WorkspaceSource::Project,
                global_workspace_configured: global.configured,
            };
        }
        let root = crate::utils::paths::normalize_absolute_path(&global.path.to_string_lossy(), "")
            .to_string_lossy()
            .to_string();
        WorkspaceResolution {
            work_root: root.clone(),
            project_root: None,
            linked_roots: self.linked_roots.clone(),
            memory_root: root,
            source: WorkspaceSource::Global,
            global_workspace_configured: global.configured,
        }
    }
}

/// 工作目录语义的唯一入口：宿主绑定目录 > 项目主目录 > 全局工作空间。
///
/// 普通会话、圆桌和工作流不保存独立 cwd；每次运行时都由归属关系重新解析。
/// 用户设备文档目录只存在于全局工作空间的内部兜底中，不属于独立来源。
fn resolve_workspace_rule(
    conn: &Connection,
    project_id: Option<&str>,
    host_resume_cwd: Option<&str>,
) -> Result<WorkspaceResolution, AppError> {
    let project_workspace = project_workspace_context(conn, project_id)?;
    let mut resolved = project_workspace.resolve_owned_workspace(conn);

    if let Some(host_root) = host_resume_cwd
        .map(str::trim)
        .filter(|root| !root.is_empty())
    {
        let host_root = crate::utils::paths::normalize_absolute_path(host_root, "");
        if !host_root.is_absolute() {
            return Err(AppError::Config(format!(
                "ACP 宿主绑定目录必须是绝对路径：{}",
                host_resume_cwd.unwrap_or_default()
            )));
        }
        resolved.work_root = host_root.to_string_lossy().to_string();
        resolved.source = WorkspaceSource::HostBound;
    }

    Ok(resolved)
}

/// 读取会话、圆桌或工作流的归属元数据并解析工作区。
///
/// 调用方只传“对象类型 + ID”，不得自行组合项目目录或选择宿主绑定优先级。
pub(crate) fn resolve_workspace_context(
    conn: &Connection,
    owner: WorkspaceOwner<'_>,
) -> Result<ResolvedWorkspace, AppError> {
    let owner_id = owner.id.trim();
    if owner_id.is_empty() {
        return Err(owner.kind.missing_error());
    }
    let (owner_title, project_id, host_resume_cwd) = match owner.kind {
        WorkspaceOwnerKind::Session => conn
            .query_row(
                "SELECT title, project_id, host_resume_cwd
                 FROM sessions WHERE id = ?1",
                params![owner_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?,
        WorkspaceOwnerKind::Room => conn
            .query_row(
                "SELECT title, project_id, NULL
                 FROM groupchat_rooms WHERE id = ?1",
                params![owner_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        None,
                    ))
                },
            )
            .optional()?,
        WorkspaceOwnerKind::Workflow => conn
            .query_row(
                "SELECT name, project_id, NULL
                 FROM workflow_definitions WHERE id = ?1",
                params![owner_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        None,
                    ))
                },
            )
            .optional()?,
    }
    .ok_or_else(|| owner.kind.missing_error())?;

    let workspace =
        resolve_workspace_rule(conn, project_id.as_deref(), host_resume_cwd.as_deref())?;
    Ok(ResolvedWorkspace {
        owner_kind: owner.kind,
        owner_id: owner_id.to_string(),
        owner_title,
        project_id,
        workspace,
    })
}

/// 无业务归属对象时仍使用同一条规则解析全局工作空间。
pub(crate) fn resolve_unowned_workspace(
    conn: &Connection,
) -> Result<WorkspaceResolution, AppError> {
    resolve_workspace_rule(conn, None, None)
}

/// 给无法自动读取项目目录的 Agent（尤其是普通 CLI 子进程）注入可发现的工作区上下文。
///
/// 安全路径只解决“能访问”，这里再解决“知道去哪儿访问”：工作根是相对路径基准，
/// 关联目录是可读写项目范围，产物目录是要求落盘的位置。
pub(crate) fn workspace_context_prompt(
    workspace: &WorkspaceResolution,
    artifact_root: Option<&str>,
) -> String {
    let work_root = workspace.work_root.trim();
    if work_root.trim().is_empty() {
        return String::new();
    }

    let mut prompt = String::from("<pilotdesk_workspace>\n");
    let source = match workspace.source {
        WorkspaceSource::HostBound => "宿主绑定目录",
        WorkspaceSource::Project => "项目主目录",
        WorkspaceSource::Global if workspace.global_workspace_configured => "全局工作空间",
        WorkspaceSource::Global => "全局工作空间（用户文档目录兜底）",
    };
    prompt.push_str(&format!("工作根来源：{}\n", source));
    prompt.push_str(&format!(
        "工作根（唯一 cwd、相对路径与命令执行基准）：{}\n",
        work_root
    ));
    if let Some(project_root) = workspace
        .project_root
        .as_deref()
        .map(str::trim)
        .filter(|root| !root.is_empty())
    {
        prompt.push_str(&format!("项目主目录：{}\n", project_root));
    } else {
        prompt.push_str("项目主目录：未归属项目\n");
    }
    if !workspace.linked_roots.is_empty() {
        prompt.push_str("关联目录（属于当前项目，与工作根使用同一套矩阵权限）：\n");
        for root in workspace
            .linked_roots
            .iter()
            .filter(|root| !root.trim().is_empty())
        {
            prompt.push_str(&format!("- {}\n", root.trim()));
        }
    } else {
        prompt.push_str("关联目录：无\n");
    }
    if let Some(artifact_root) = artifact_root.map(str::trim).filter(|root| !root.is_empty()) {
        prompt.push_str(&format!(
            "产物目录（生成文件的默认落盘位置）：{}\n",
            artifact_root
        ));
    }
    prompt.push_str(
        "路径规则：\n\
         - 以下路径均为规范化后的绝对路径；会话不持久化独立工作根快照。\n\
         - 相对路径一律以工作根解析，不按项目关联目录或产物目录解析。\n\
         - 新建文档、报告、图片、脚本等交付物默认写入产物目录；修改项目既有代码/配置时仍使用其真实绝对路径。\n\
         - 向用户报告文件时给出实际绝对路径；文件历史也按写入时的实际绝对路径记录。\n\
         - 后续修改项目归属或工作根不会改写既有文件历史，撤销时不重新解析相对路径。\n",
    );
    prompt.push_str("</pilotdesk_workspace>");
    prompt
}

#[derive(Debug)]
struct NormalizedDirectory {
    path: String,
    is_primary: bool,
    sort_order: i64,
}

const PROJECT_SELECT: &str = r#"
    SELECT
        p.id,
        p.name,
        p.description,
        p.icon,
        p.archived_at,
        p.created_at,
        p.updated_at,
        MAX(
            p.updated_at,
            COALESCE((SELECT MAX(updated_at) FROM sessions WHERE project_id = p.id), 0),
            COALESCE((SELECT MAX(updated_at) FROM groupchat_rooms WHERE project_id = p.id), 0),
            COALESCE((
                SELECT MAX(updated_at)
                FROM workflow_definitions
                WHERE project_id = p.id AND deleted_at IS NULL
            ), 0)
        ) AS last_activity_at,
        (
            SELECT COUNT(*)
            FROM sessions
            WHERE project_id = p.id
              AND COALESCE(origin, '') NOT LIKE 'workflow%'
        ) AS session_count,
        (SELECT COUNT(*) FROM groupchat_rooms WHERE project_id = p.id) AS room_count,
        (
            SELECT COUNT(*)
            FROM workflow_definitions
            WHERE project_id = p.id AND deleted_at IS NULL
        ) AS workflow_count
    FROM projects p
"#;

fn normalize_directories(
    directories: Vec<ProjectDirectoryInput>,
) -> Result<Vec<NormalizedDirectory>, AppError> {
    if directories.is_empty() {
        return Err(AppError::Config("项目至少需要一个目录".into()));
    }
    if directories.len() > 32 {
        return Err(AppError::Config("一个项目最多关联 32 个目录".into()));
    }

    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(directories.len());
    let mut primary_index = None;

    for (index, directory) in directories.into_iter().enumerate() {
        let raw_path = directory.path.trim();
        if raw_path.is_empty() {
            return Err(AppError::Config("项目目录不能为空".into()));
        }
        if raw_path.chars().count() > 2048 {
            return Err(AppError::Config("项目目录路径过长".into()));
        }
        let path = crate::utils::paths::normalize_absolute_path(raw_path, "")
            .to_string_lossy()
            .to_string();
        if !std::path::Path::new(&path).is_absolute() {
            return Err(AppError::Config(format!(
                "项目目录必须是绝对路径：{raw_path}"
            )));
        }
        let dedupe_key = crate::utils::paths::path_key(std::path::Path::new(&path));
        if !seen.insert(dedupe_key) {
            return Err(AppError::Config(format!("项目目录重复：{path}")));
        }
        if directory.is_primary {
            if primary_index.is_some() {
                return Err(AppError::Config("项目只能设置一个主目录".into()));
            }
            primary_index = Some(index);
        }
        normalized.push(NormalizedDirectory {
            path,
            is_primary: directory.is_primary,
            sort_order: index as i64,
        });
    }

    let primary_index = primary_index.unwrap_or(0);
    for (index, directory) in normalized.iter_mut().enumerate() {
        directory.is_primary = index == primary_index;
    }
    Ok(normalized)
}

fn validate_project_input(input: &ProjectInput) -> Result<(String, String, String), AppError> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::Config("项目名称不能为空".into()));
    }
    if name.chars().count() > 80 {
        return Err(AppError::Config("项目名称不能超过 80 个字符".into()));
    }
    let description = input.description.trim().to_string();
    if description.chars().count() > 2000 {
        return Err(AppError::Config("项目说明不能超过 2000 个字符".into()));
    }
    let icon = input
        .icon
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("📁")
        .to_string();
    if icon.chars().count() > 16 {
        return Err(AppError::Config("项目图标过长".into()));
    }
    Ok((name, description, icon))
}

fn project_exists(conn: &Connection, project_id: &str) -> Result<bool, AppError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM projects WHERE id = ?1",
            params![project_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// 创建/编辑工作时校验项目归属。空字符串视为未归属；已归档项目不再接收新工作，
/// 但已有工作仍保留原 `project_id`，不影响历史查看。
pub(crate) fn validate_project_membership(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Option<String>, AppError> {
    let project_id = project_id.map(str::trim).filter(|id| !id.is_empty());
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    let archived_at: Option<Option<i64>> = conn
        .query_row(
            "SELECT archived_at FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .optional()?;
    match archived_at {
        None => Err(AppError::NotFound("项目不存在".into())),
        Some(Some(_)) => Err(AppError::InvalidInput(
            "项目已归档，请先恢复项目后再创建工作".into(),
        )),
        Some(None) => Ok(Some(project_id.to_string())),
    }
}

/// 编辑已有工作时，若项目归属没有变化，则允许继续保存；只有新归入项目时才
/// 受“归档项目不接收新工作”的限制。否则归档项目里的工作流会连普通编辑都无法保存。
pub(crate) fn validate_project_membership_update(
    conn: &Connection,
    project_id: Option<&str>,
    current_project_id: Option<&str>,
) -> Result<Option<String>, AppError> {
    let next = project_id.map(str::trim).filter(|id| !id.is_empty());
    let current = current_project_id
        .map(str::trim)
        .filter(|id| !id.is_empty());
    if next == current {
        if let Some(project_id) = next {
            if !project_exists(conn, project_id)? {
                return Err(AppError::NotFound("项目不存在".into()));
            }
        }
        return Ok(next.map(str::to_string));
    }
    validate_project_membership(conn, next)
}

fn load_directories(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<HashMap<String, Vec<ProjectDirectory>>, AppError> {
    let mut grouped: HashMap<String, Vec<ProjectDirectory>> = HashMap::new();
    let mapper = |row: &rusqlite::Row<'_>| {
        Ok(ProjectDirectory {
            id: row.get(0)?,
            project_id: row.get(1)?,
            path: row.get(2)?,
            is_primary: row.get::<_, i64>(3)? != 0,
            sort_order: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    };
    match project_id {
        Some(id) => {
            let mut stmt = conn.prepare(
                "SELECT id, project_id, path, is_primary, sort_order, created_at, updated_at
                 FROM project_directories
                 WHERE project_id = ?1
                 ORDER BY sort_order ASC, created_at ASC",
            )?;
            for directory in stmt.query_map(params![id], mapper)? {
                let directory = directory?;
                grouped
                    .entry(directory.project_id.clone())
                    .or_default()
                    .push(directory);
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT id, project_id, path, is_primary, sort_order, created_at, updated_at
                 FROM project_directories
                 ORDER BY project_id ASC, sort_order ASC, created_at ASC",
            )?;
            for directory in stmt.query_map([], mapper)? {
                let directory = directory?;
                grouped
                    .entry(directory.project_id.clone())
                    .or_default()
                    .push(directory);
            }
        }
    }
    Ok(grouped)
}

/// 解析项目运行时安全根：主目录排第一，关联目录随后；按规范化路径去重。
///
/// 无项目或项目没有目录时返回空上下文，由调用方继续以全局工作空间兜底。
pub(crate) fn project_workspace_context(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<ProjectWorkspaceContext, AppError> {
    let Some(project_id) = project_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return Ok(ProjectWorkspaceContext::default());
    };

    let mut grouped = load_directories(conn, Some(project_id))?;
    let directories = grouped.remove(project_id).unwrap_or_default();
    if directories.is_empty() {
        return Ok(ProjectWorkspaceContext::default());
    }

    let primary_index = directories
        .iter()
        .position(|directory| directory.is_primary)
        .unwrap_or(0);
    let mut ordered = Vec::with_capacity(directories.len());
    ordered.push(directories[primary_index].clone());
    ordered.extend(
        directories
            .into_iter()
            .enumerate()
            .filter_map(|(index, directory)| (index != primary_index).then_some(directory)),
    );

    let mut seen = HashSet::new();
    let mut roots = Vec::with_capacity(ordered.len());
    for directory in ordered {
        let path = crate::utils::paths::normalize_absolute_path(directory.path.trim(), "")
            .to_string_lossy()
            .to_string();
        if path.is_empty() {
            continue;
        }
        let dedupe_key = crate::utils::paths::path_key(std::path::Path::new(&path));
        if seen.insert(dedupe_key) {
            roots.push(path);
        }
    }

    let primary_root = roots.first().cloned();
    let linked_roots = roots.iter().skip(1).cloned().collect();
    Ok(ProjectWorkspaceContext {
        primary_root,
        linked_roots,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWorkspaceInfo {
    pub work_root: String,
    pub project_root: Option<String>,
    pub linked_roots: Vec<String>,
    pub memory_root: String,
    pub artifact_root: String,
    pub project_id: Option<String>,
    pub source: WorkspaceSource,
    pub global_workspace_configured: bool,
    pub host_resume: bool,
}

/// 前端读取会话当前实际工作根，仅用于展示、记忆与仓库状态查询。
///
/// 这不是可编辑路径：普通会话始终按项目主目录/全局工作空间动态解析，
/// ACP 宿主导入会话才使用专用宿主恢复目录。
#[tauri::command]
pub fn session_workspace_info(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<SessionWorkspaceInfo, AppError> {
    let conn = state.get_conn()?;
    let context = resolve_workspace_context(&conn, WorkspaceOwner::session(&session_id))?;
    let artifact_root = context.artifact_root();
    let project_id = context.project_id;
    let host_resume = context.workspace.source == WorkspaceSource::HostBound;
    let workspace = context.workspace;
    Ok(SessionWorkspaceInfo {
        work_root: workspace.work_root,
        project_root: workspace.project_root,
        linked_roots: workspace.linked_roots,
        memory_root: workspace.memory_root,
        artifact_root,
        project_id,
        source: workspace.source,
        global_workspace_configured: workspace.global_workspace_configured,
        host_resume,
    })
}

fn row_to_project(
    row: &rusqlite::Row<'_>,
    directories: &mut HashMap<String, Vec<ProjectDirectory>>,
) -> rusqlite::Result<Project> {
    let id: String = row.get(0)?;
    Ok(Project {
        directories: directories.remove(&id).unwrap_or_default(),
        id,
        name: row.get(1)?,
        description: row.get(2)?,
        icon: row.get(3)?,
        archived_at: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        last_activity_at: row.get(7)?,
        session_count: row.get(8)?,
        room_count: row.get(9)?,
        workflow_count: row.get(10)?,
    })
}

fn list_projects_from_db(
    conn: &Connection,
    include_archived: bool,
) -> Result<Vec<Project>, AppError> {
    let mut directories = load_directories(conn, None)?;
    let filter = if include_archived {
        ""
    } else {
        " WHERE p.archived_at IS NULL"
    };
    let sql = format!(
        "{PROJECT_SELECT}{filter}
         ORDER BY
            CASE WHEN p.archived_at IS NULL THEN 0 ELSE 1 END,
            last_activity_at DESC,
            p.name COLLATE NOCASE ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row_to_project(row, &mut directories))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn get_project_from_db(conn: &Connection, project_id: &str) -> Result<Project, AppError> {
    let mut directories = load_directories(conn, Some(project_id))?;
    let sql = format!("{PROJECT_SELECT} WHERE p.id = ?1");
    conn.query_row(&sql, params![project_id], |row| {
        row_to_project(row, &mut directories)
    })
    .optional()?
    .ok_or_else(|| AppError::NotFound("项目不存在".into()))
}

fn insert_directories(
    tx: &rusqlite::Transaction<'_>,
    project_id: &str,
    directories: &[NormalizedDirectory],
    now: i64,
) -> Result<(), AppError> {
    for directory in directories {
        tx.execute(
            "INSERT INTO project_directories
                (id, project_id, path, is_primary, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                crate::utils::new_id(),
                project_id,
                directory.path,
                if directory.is_primary { 1 } else { 0 },
                directory.sort_order,
                now,
            ],
        )?;
    }
    Ok(())
}

#[tauri::command]
pub fn project_list(
    state: State<'_, DbState>,
    include_archived: Option<bool>,
) -> Result<Vec<Project>, AppError> {
    let conn = state.get_conn()?;
    list_projects_from_db(&conn, include_archived.unwrap_or(false))
}

#[tauri::command]
pub fn project_get(state: State<'_, DbState>, project_id: String) -> Result<Project, AppError> {
    let conn = state.get_conn()?;
    get_project_from_db(&conn, &project_id)
}

#[tauri::command]
pub fn project_create(state: State<'_, DbState>, input: ProjectInput) -> Result<Project, AppError> {
    let (name, description, icon) = validate_project_input(&input)?;
    let directories = normalize_directories(input.directories)?;
    let mut conn = state.get_conn()?;
    let tx = conn.transaction()?;
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    tx.execute(
        "INSERT INTO projects (id, name, description, icon, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, name, description, icon, now],
    )?;
    insert_directories(&tx, &id, &directories, now)?;
    tx.commit()?;
    get_project_from_db(&conn, &id)
}

#[tauri::command]
pub fn project_update(
    state: State<'_, DbState>,
    rooms: State<'_, crate::groupchat::room::RoomRegistry>,
    project_id: String,
    input: ProjectInput,
) -> Result<Project, AppError> {
    let (name, description, icon) = validate_project_input(&input)?;
    let directories = normalize_directories(input.directories)?;
    let mut conn = state.get_conn()?;
    if !project_exists(&conn, &project_id)? {
        return Err(AppError::NotFound("项目不存在".into()));
    }
    let tx = conn.transaction()?;
    let now = crate::utils::now();
    tx.execute(
        "UPDATE projects
         SET name = ?1, description = ?2, icon = ?3, updated_at = ?4
         WHERE id = ?5",
        params![name, description, icon, now, project_id],
    )?;
    tx.execute(
        "DELETE FROM project_directories WHERE project_id = ?1",
        params![project_id],
    )?;
    insert_directories(&tx, &project_id, &directories, now)?;
    tx.commit()?;
    let room_ids = {
        let mut stmt = conn.prepare("SELECT id FROM groupchat_rooms WHERE project_id = ?1")?;
        let rows = stmt.query_map(params![project_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for room_id in room_ids {
        rooms.notify_workspace_changed(&room_id);
    }
    get_project_from_db(&conn, &project_id)
}

#[tauri::command]
pub fn project_archive(state: State<'_, DbState>, project_id: String) -> Result<Project, AppError> {
    let conn = state.get_conn()?;
    let now = crate::utils::now();
    let changed = conn.execute(
        "UPDATE projects
         SET archived_at = COALESCE(archived_at, ?1), updated_at = ?1
         WHERE id = ?2",
        params![now, project_id],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound("项目不存在".into()));
    }
    get_project_from_db(&conn, &project_id)
}

#[tauri::command]
pub fn project_restore(state: State<'_, DbState>, project_id: String) -> Result<Project, AppError> {
    let conn = state.get_conn()?;
    let now = crate::utils::now();
    let changed = conn.execute(
        "UPDATE projects
         SET archived_at = NULL, updated_at = ?1
         WHERE id = ?2",
        params![now, project_id],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound("项目不存在".into()));
    }
    get_project_from_db(&conn, &project_id)
}

/// 项目详情里的统一工作清单。会话排除工作流节点自动创建的内部会话，避免同一执行过程
/// 在项目里出现多条没有独立管理意义的记录。
#[tauri::command]
pub fn project_work_items(
    state: State<'_, DbState>,
    project_id: String,
) -> Result<Vec<ProjectWorkItem>, AppError> {
    let conn = state.get_conn()?;
    if !project_exists(&conn, &project_id)? {
        return Err(AppError::NotFound("项目不存在".into()));
    }
    let mut items = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, title, COALESCE(last_message_preview, ''),
                    COALESCE(status, 'active'), updated_at
             FROM sessions
             WHERE project_id = ?1
               AND COALESCE(origin, '') NOT LIKE 'workflow%'",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(ProjectWorkItem {
                kind: "session".into(),
                id: row.get(0)?,
                title: row.get(1)?,
                subtitle: row.get(2)?,
                status: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        items.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    {
        let mut stmt = conn.prepare(
            "SELECT id, title, topic, status, updated_at
             FROM groupchat_rooms
             WHERE project_id = ?1",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(ProjectWorkItem {
                kind: "groupchat".into(),
                id: row.get(0)?,
                title: row.get(1)?,
                subtitle: row.get(2)?,
                status: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        items.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    {
        let mut stmt = conn.prepare(
            "SELECT id, name, description,
                    CASE WHEN enabled = 1 THEN 'enabled' ELSE 'disabled' END,
                    updated_at
             FROM workflow_definitions
             WHERE project_id = ?1 AND deleted_at IS NULL",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(ProjectWorkItem {
                kind: "workflow".into(),
                id: row.get(0)?,
                title: row.get(1)?,
                subtitle: row.get(2)?,
                status: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        items.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(items)
}

/// 修改已有会话/圆桌/工作流的项目归属。创建时仍走各自创建命令；本命令只处理
/// “把已有工作归入项目/移出项目”，并复用归档项目的归属校验。
#[tauri::command]
pub fn project_set_work_membership(
    state: State<'_, DbState>,
    rooms: State<'_, crate::groupchat::room::RoomRegistry>,
    work_type: String,
    work_id: String,
    project_id: Option<String>,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    let (table, current_project_id) = match work_type.as_str() {
        "session" => {
            let row = conn
                .query_row(
                    "SELECT project_id, COALESCE(origin, '')
                     FROM sessions WHERE id = ?1",
                    params![work_id],
                    |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
                .ok_or_else(|| AppError::NotFound("会话不存在".into()))?;
            if row.1.starts_with("workflow") {
                return Err(AppError::InvalidInput(
                    "工作流内部会话不支持单独调整项目归属".into(),
                ));
            }
            ("sessions", row.0)
        }
        "groupchat" => {
            let current = conn
                .query_row(
                    "SELECT project_id FROM groupchat_rooms WHERE id = ?1",
                    params![work_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .ok_or_else(|| AppError::NotFound("圆桌不存在".into()))?;
            ("groupchat_rooms", current)
        }
        "workflow" => {
            let current = conn
                .query_row(
                    "SELECT project_id FROM workflow_definitions
                     WHERE id = ?1 AND deleted_at IS NULL",
                    params![work_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .ok_or_else(|| AppError::NotFound("工作流不存在".into()))?;
            ("workflow_definitions", current)
        }
        _ => {
            return Err(AppError::InvalidInput(
                "不支持的工作类型，应为 session、groupchat 或 workflow".into(),
            ))
        }
    };
    let next_project_id = validate_project_membership_update(
        &conn,
        project_id.as_deref(),
        current_project_id.as_deref(),
    )?;
    let now = crate::utils::now();
    conn.execute(
        &format!("UPDATE {table} SET project_id = ?1, updated_at = ?2 WHERE id = ?3"),
        params![next_project_id, now, work_id],
    )?;
    if work_type == "workflow" {
        crate::commands::cloud_sync::mark_workflow_dirty(&conn, &work_id);
    }
    if work_type == "groupchat" {
        rooms.notify_workspace_changed(&work_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::init::FINAL_SCHEMA_SQL)
            .unwrap();
        conn
    }

    fn normalized(path: &str) -> String {
        crate::utils::paths::normalize_absolute_path(path, "")
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn normalizes_first_directory_as_primary_and_rejects_duplicates() {
        let directories = normalize_directories(vec![
            ProjectDirectoryInput {
                path: "D:\\Work".into(),
                is_primary: false,
            },
            ProjectDirectoryInput {
                path: "D:\\Docs".into(),
                is_primary: false,
            },
        ])
        .unwrap();
        assert!(directories[0].is_primary);
        assert!(!directories[1].is_primary);

        let error = normalize_directories(vec![
            ProjectDirectoryInput {
                path: "D:\\Work".into(),
                is_primary: false,
            },
            ProjectDirectoryInput {
                path: "d:/work".into(),
                is_primary: false,
            },
        ])
        .unwrap_err();
        assert!(error.to_string().contains("重复"));
    }

    #[test]
    fn project_workspace_context_includes_primary_and_all_linked_directories() {
        let conn = setup();
        conn.execute(
            "INSERT INTO projects (id, name, description, created_at, updated_at)
             VALUES ('p1', 'Project', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO project_directories
                (id, project_id, path, is_primary, sort_order, created_at, updated_at)
             VALUES
                ('linked', 'p1', 'D:\\Linked', 0, 0, 1, 1),
                ('primary', 'p1', 'D:\\Primary', 1, 1, 1, 1),
                ('duplicate', 'p1', 'd:/primary', 0, 2, 1, 1);",
        )
        .unwrap();

        let context = project_workspace_context(&conn, Some("p1")).unwrap();
        assert_eq!(
            context.primary_root.as_deref(),
            Some(normalized("D:\\Primary").as_str())
        );
        assert_eq!(context.linked_roots, vec![normalized("D:\\Linked")]);
        assert_eq!(
            context.resolve_owned_workspace(&conn).work_root,
            normalized("D:\\Primary")
        );
        let workspace = context.resolve_owned_workspace(&conn);
        assert_eq!(
            workspace.runtime_roots(["F:\\Artifacts".into()]),
            vec![
                normalized("D:\\Primary"),
                normalized("D:\\Linked"),
                "F:\\Artifacts".to_string(),
            ]
        );
    }

    #[test]
    fn workspace_prompt_exposes_work_root_linked_roots_and_artifacts_separately() {
        let workspace = WorkspaceResolution {
            work_root: "D:\\Primary".to_string(),
            project_root: Some("D:\\Primary".to_string()),
            linked_roots: vec!["D:\\Linked".to_string(), "E:\\Docs".to_string()],
            memory_root: "D:\\Primary".to_string(),
            source: WorkspaceSource::Project,
            global_workspace_configured: false,
        };
        let prompt = workspace_context_prompt(&workspace, Some("D:\\Primary\\outputs\\Release"));

        assert!(prompt.contains("工作根（唯一 cwd、相对路径与命令执行基准）：D:\\Primary"));
        assert!(prompt.contains("- D:\\Linked"));
        assert!(prompt.contains("- E:\\Docs"));
        assert!(
            prompt.contains("产物目录（生成文件的默认落盘位置）：D:\\Primary\\outputs\\Release")
        );
    }

    #[test]
    fn workspace_priority_is_host_then_project_then_global() {
        let conn = setup();
        conn.execute(
            "INSERT INTO projects (id, name, description, created_at, updated_at)
             VALUES ('p1', 'Project', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_directories
                (id, project_id, path, is_primary, sort_order, created_at, updated_at)
             VALUES ('d1', 'p1', 'D:\\Project', 1, 0, 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions
                (id, agent_type, title, host_resume_cwd, created_at, updated_at, project_id)
             VALUES
                ('host', 'api', 'Host', 'E:\\Host', 1, 1, 'p1'),
                ('project', 'api', 'Project', NULL, 1, 1, 'p1'),
                ('global', 'api', 'Global', NULL, 1, 1, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at)
             VALUES ('pilotdesk-workspace', 'F:\\Global', 1)",
            [],
        )
        .unwrap();

        let host = resolve_workspace_context(&conn, WorkspaceOwner::session("host"))
            .unwrap()
            .workspace;
        assert_eq!(host.work_root, normalized("E:\\Host"));
        assert_eq!(
            host.project_root.as_deref(),
            Some(normalized("D:\\Project").as_str())
        );
        assert_eq!(host.memory_root, normalized("D:\\Project"));
        assert_eq!(host.source, WorkspaceSource::HostBound);

        let project = resolve_workspace_context(&conn, WorkspaceOwner::session("project"))
            .unwrap()
            .workspace;
        assert_eq!(project.work_root, normalized("D:\\Project"));
        assert_eq!(project.source, WorkspaceSource::Project);

        let global = resolve_workspace_context(&conn, WorkspaceOwner::session("global"))
            .unwrap()
            .workspace;
        assert_eq!(global.work_root, normalized("F:\\Global"));
        assert_eq!(global.source, WorkspaceSource::Global);
        assert!(global.global_workspace_configured);
    }

    #[test]
    fn unified_workspace_context_resolves_room_and_workflow_ownership() {
        let conn = setup();
        conn.execute(
            "INSERT INTO projects (id, name, description, created_at, updated_at)
             VALUES ('p1', 'Project', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_directories
                (id, project_id, path, is_primary, sort_order, created_at, updated_at)
             VALUES
                ('primary', 'p1', 'D:\\Primary', 1, 0, 1, 1),
                ('linked', 'p1', 'D:\\Linked', 0, 1, 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO groupchat_rooms
                (id, title, topic, status, strategy, max_rounds, max_parallel,
                 created_at, updated_at, project_id)
             VALUES ('r1', 'Room Plan', '', 'idle', 'round_robin', 0, 2, 1, 1, 'p1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflow_definitions
                (id, name, project_id, created_at, updated_at)
             VALUES ('w1', 'Release Flow', 'p1', 1, 1)",
            [],
        )
        .unwrap();

        let room = resolve_workspace_context(&conn, WorkspaceOwner::room("r1")).unwrap();
        assert_eq!(room.owner_title, "Room Plan");
        assert_eq!(room.project_id.as_deref(), Some("p1"));
        assert_eq!(room.workspace.work_root, normalized("D:\\Primary"));
        assert_eq!(room.workspace.linked_roots, vec![normalized("D:\\Linked")]);
        assert_eq!(room.workspace.source, WorkspaceSource::Project);

        let workflow = resolve_workspace_context(&conn, WorkspaceOwner::workflow("w1")).unwrap();
        assert_eq!(workflow.owner_title, "Release Flow");
        assert_eq!(workflow.project_id.as_deref(), Some("p1"));
        assert_eq!(workflow.workspace.work_root, normalized("D:\\Primary"));
        assert_eq!(workflow.workspace.source, WorkspaceSource::Project);
    }

    #[test]
    fn unified_workspace_context_rejects_relative_host_binding() {
        let conn = setup();
        conn.execute(
            "INSERT INTO sessions
                (id, agent_type, title, host_resume_cwd, created_at, updated_at)
             VALUES ('s1', 'api', 'Host', 'relative\\cwd', 1, 1)",
            [],
        )
        .unwrap();

        let error = resolve_workspace_context(&conn, WorkspaceOwner::session("s1")).unwrap_err();
        assert!(error.to_string().contains("宿主绑定目录必须是绝对路径"));
    }

    #[test]
    fn unconfigured_global_workspace_stays_global_and_uses_document_fallback() {
        let conn = setup();
        let workspace = resolve_unowned_workspace(&conn).unwrap();

        assert_eq!(workspace.source, WorkspaceSource::Global);
        assert!(!workspace.global_workspace_configured);
        assert_eq!(workspace.project_root, None);
        assert_eq!(workspace.work_root, workspace.memory_root);
        assert!(std::path::Path::new(&workspace.work_root).is_absolute());
        assert_eq!(
            std::path::Path::new(&workspace.work_root).file_name(),
            Some(std::ffi::OsStr::new("PilotDesk"))
        );
    }

    #[test]
    fn project_workspace_follows_primary_directory_changes_at_runtime() {
        let conn = setup();
        conn.execute(
            "INSERT INTO projects (id, name, description, created_at, updated_at)
             VALUES ('p1', 'Project', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_directories
                (id, project_id, path, is_primary, sort_order, created_at, updated_at)
             VALUES ('d1', 'p1', 'D:\\Before', 1, 0, 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions
                (id, agent_type, title, created_at, updated_at, project_id)
             VALUES ('s1', 'api', 'Session', 1, 1, 'p1')",
            [],
        )
        .unwrap();

        assert_eq!(
            resolve_workspace_context(&conn, WorkspaceOwner::session("s1"))
                .unwrap()
                .workspace
                .work_root,
            normalized("D:\\Before")
        );
        conn.execute(
            "UPDATE project_directories SET path = 'E:\\After' WHERE id = 'd1'",
            [],
        )
        .unwrap();
        assert_eq!(
            resolve_workspace_context(&conn, WorkspaceOwner::session("s1"))
                .unwrap()
                .workspace
                .work_root,
            normalized("E:\\After")
        );
    }

    #[test]
    fn archived_project_allows_existing_work_to_be_edited_but_not_new_assignments() {
        let conn = setup();
        conn.execute(
            "INSERT INTO projects
                (id, name, description, archived_at, created_at, updated_at)
             VALUES ('archived', 'Archived', '', 10, 1, 10)",
            [],
        )
        .unwrap();

        let unchanged =
            validate_project_membership_update(&conn, Some("archived"), Some("archived")).unwrap();
        assert_eq!(unchanged.as_deref(), Some("archived"));

        let error = validate_project_membership_update(&conn, Some("archived"), None).unwrap_err();
        assert!(error.to_string().contains("已归档"));
    }

    #[test]
    fn project_session_count_excludes_workflow_internal_sessions() {
        let conn = setup();
        conn.execute(
            "INSERT INTO projects (id, name, description, created_at, updated_at)
             VALUES ('p1', 'Project', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at, project_id)
             VALUES ('user', 'User session', 1, 1, 'p1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, origin, created_at, updated_at, project_id)
             VALUES ('workflow', 'Workflow session', 'workflow:w1', 1, 1, 'p1')",
            [],
        )
        .unwrap();

        let project = get_project_from_db(&conn, "p1").unwrap();
        assert_eq!(project.session_count, 1);
    }

    #[test]
    fn project_icon_round_trips_and_blank_input_uses_default() {
        let conn = setup();
        let input = ProjectInput {
            name: "Icon Project".into(),
            description: String::new(),
            icon: Some("🚀".into()),
            directories: vec![ProjectDirectoryInput {
                path: "D:\\Icon".into(),
                is_primary: true,
            }],
        };
        let (name, description, icon) = validate_project_input(&input).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, description, icon, created_at, updated_at)
             VALUES ('p1', ?1, ?2, ?3, 1, 1)",
            params![name, description, icon],
        )
        .unwrap();

        let project = get_project_from_db(&conn, "p1").unwrap();
        assert_eq!(project.icon, "🚀");

        let blank = ProjectInput {
            icon: Some("   ".into()),
            ..input
        };
        assert_eq!(validate_project_input(&blank).unwrap().2, "📁");
    }
}
