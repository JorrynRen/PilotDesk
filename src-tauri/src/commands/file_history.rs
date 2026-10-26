//! Agent 文件修改历史与撤销
//!
//! write_file / edit_file 在覆盖文件前会记录旧内容快照，供用户撤销到上一个版本。

use rusqlite::types::ToSql;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::db::init::DbPool;
use crate::utils::errors::AppError;
use crate::utils::process::hidden_command;

/// 超过该字节数不备份（避免数据库膨胀）
const MAX_BACKUP_BYTES: usize = 2_000_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryEntry {
    pub id: i64,
    pub session_id: String,
    pub file_path: String,
    pub file_existed: bool,
    pub created_at: i64,
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<FileHistoryEntry> {
    Ok(FileHistoryEntry {
        id: row.get(0)?,
        session_id: row.get(1)?,
        file_path: row.get(2)?,
        file_existed: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
    })
}

/// 记录一次文件修改（同步，供 write_file / edit_file 工具内部调用）。
/// 内容超过阈值时跳过备份。
pub fn record_file_change(
    pool: &DbPool,
    session_id: &str,
    file_path: &str,
    backup_content: &str,
    file_existed: bool,
) {
    if backup_content.len() > MAX_BACKUP_BYTES {
        return;
    }
    if let Ok(conn) = pool.get() {
        let now = crate::utils::now();
        let _ = conn.execute(
            "INSERT INTO file_history (session_id, file_path, backup_content, file_existed, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, file_path, backup_content, if file_existed { 1 } else { 0 }, now],
        );
    }
}

/// 历史分页结果（总数 + 当前页条目）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryPage {
    pub total: i64,
    pub items: Vec<FileHistoryEntry>,
}

/// 构造查询条件（session_id / path_keyword），返回 (WHERE 子句, 动态参数)。
fn build_filters(
    session_id: &Option<String>,
    path_keyword: &Option<String>,
) -> (String, Vec<Box<dyn ToSql>>) {
    let mut conds: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(sid) = session_id.as_deref() {
        if !sid.is_empty() {
            conds.push("session_id = ?".to_string());
            args.push(Box::new(sid.to_string()));
        }
    }
    if let Some(kw) = path_keyword.as_deref() {
        let kw = kw.trim();
        if !kw.is_empty() {
            conds.push("file_path LIKE ?".to_string());
            args.push(Box::new(format!("%{}%", kw)));
        }
    }
    let where_sql = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    (where_sql, args)
}

#[tauri::command]
pub fn list_file_history(
    state: State<'_, crate::DbState>,
    session_id: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    path_keyword: Option<String>,
) -> Result<FileHistoryPage, AppError> {
    let conn = state.get_conn()?;
    let limit = limit.unwrap_or(100).clamp(1, 500);
    let offset = offset.unwrap_or(0).max(0);

    let (where_sql, filter_args) = build_filters(&session_id, &path_keyword);
    let params_ref: Vec<&dyn ToSql> = filter_args.iter().map(|b| b.as_ref()).collect();

    // 总数（供前端展示/触底判断）
    let total = {
        let sql = format!("SELECT COUNT(*) FROM file_history{}", where_sql);
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_row(params_ref.as_slice(), |r| r.get::<_, i64>(0))?
    };

    // 当前页条目
    let items = {
        let mut item_args = filter_args;
        item_args.push(Box::new(limit));
        item_args.push(Box::new(offset));
        let item_params: Vec<&dyn ToSql> = item_args.iter().map(|b| b.as_ref()).collect();
        let sql = format!(
            "SELECT id, session_id, file_path, file_existed, created_at
             FROM file_history{} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
            where_sql
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(item_params.as_slice(), row_to_entry)?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    Ok(FileHistoryPage { total, items })
}

/// 清理文件修改历史：scope = "one"（按 history_id）/"session"（按 session_id）/"all"（清空全部）。
/// 返回删除的记录数。
#[tauri::command]
pub fn delete_file_history(
    state: State<'_, crate::DbState>,
    scope: String,
    history_id: Option<i64>,
    session_id: Option<String>,
) -> Result<i64, AppError> {
    let conn = state.get_conn()?;
    let deleted = match scope.as_str() {
        "one" => {
            let id =
                history_id.ok_or_else(|| AppError::InvalidInput("缺少 history_id".to_string()))?;
            conn.execute("DELETE FROM file_history WHERE id = ?1", params![id])?
        }
        "session" => {
            let sid =
                session_id.ok_or_else(|| AppError::InvalidInput("缺少 session_id".to_string()))?;
            conn.execute(
                "DELETE FROM file_history WHERE session_id = ?1",
                params![sid],
            )?
        }
        "all" => conn.execute("DELETE FROM file_history", [])?,
        other => return Err(AppError::InvalidInput(format!("未知清理范围: {}", other))),
    };
    Ok(deleted as i64)
}

/// 历史会话项（区分会话/群聊房间，供前端筛选下拉展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistorySession {
    pub id: String,
    /// "session"（正式会话）/"room"（群聊房间）。
    pub kind: String,
}

/// 列出所有出现过文件修改历史的会话/房间 id（按最近修改时间倒序，供前端筛选下拉）。
/// 通过 `sessions` 表区分：存在记录为会话，否则为群聊房间（room_id 不落 sessions 表）。
#[tauri::command]
pub fn list_file_history_sessions(
    state: State<'_, crate::DbState>,
) -> Result<Vec<FileHistorySession>, AppError> {
    let conn = state.get_conn()?;
    let mut stmt = conn.prepare(
        "SELECT fh.session_id, EXISTS(SELECT 1 FROM sessions s WHERE s.id = fh.session_id) AS is_session
         FROM file_history fh GROUP BY fh.session_id ORDER BY MAX(fh.created_at) DESC",
    )?;
    let sessions = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let is_session: bool = row.get(1)?;
            Ok(FileHistorySession {
                id,
                kind: if is_session {
                    "session".to_string()
                } else {
                    "room".to_string()
                },
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(sessions)
}

#[tauri::command]
pub fn undo_file_history(
    state: State<'_, crate::DbState>,
    history_id: i64,
) -> Result<String, AppError> {
    let conn = state.get_conn()?;

    let (file_path, backup_content, file_existed) = conn
        .query_row(
            "SELECT file_path, backup_content, file_existed FROM file_history WHERE id = ?1",
            params![history_id],
            |row| {
                let path: String = row.get(0)?;
                let backup: Option<String> = row.get(1)?;
                let existed: i64 = row.get(2)?;
                Ok((path, backup, existed != 0))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound("未找到该历史记录".to_string()))?;

    if file_existed {
        std::fs::write(&file_path, backup_content.unwrap_or_default())
            .map_err(|e| AppError::Io(format!("恢复文件失败 ({}): {}", file_path, e)))?;
    } else {
        // 该文件在修改前不存在，撤销即删除
        match std::fs::remove_file(&file_path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::Io(format!("删除文件失败 ({}): {}", file_path, e))),
        }
    }

    // 撤销后删除该条历史记录
    conn.execute(
        "DELETE FROM file_history WHERE id = ?1",
        params![history_id],
    )?;

    Ok(format!("已撤销对文件的修改: {}", file_path))
}

// ─────────────────────────────────────────────────────────────
// 仓库状态（会话 / 圆桌右侧「文件历史」面板顶部的状态条）
//
// 与上面的 `file_history` 快照互补而非同类：
// - 快照：本会话 Agent 通过 write_file / edit_file 写过什么，可逐条撤销；
// - 仓库状态：该目录在 Git 下的真实状态（含用户手改、未跟踪文件），只读。
//
// 不是 Git 仓库、或本机没有 Git 时返回 `is_repo = false`，前端整条隐藏（Git 属推荐依赖）。
// ─────────────────────────────────────────────────────────────

/// 未提交明细最多返回多少条（前端展开查看用；总数另行统计）
const MAX_REPO_STATUS_ENTRIES: usize = 50;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusEntry {
    /// 相对仓库根的路径（重命名取新路径）
    pub path: String,
    /// 精简状态码：M / A / D / R / C / U / ?（`?` = 未跟踪）
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    pub is_repo: bool,
    /// 仓库根（绝对路径）：会话目录是仓库子目录时，用它说明"属于哪个仓库"
    pub root: Option<String>,
    /// 分支名；detached HEAD 时为空
    pub branch: Option<String>,
    /// 最新提交短 hash；空仓库（无提交）时为空
    pub head: Option<String>,
    /// 已跟踪文件的改动条数
    pub changed_count: i64,
    /// 未跟踪条目数（未跟踪目录按一条计，避免 node_modules 之类撑爆）
    pub untracked_count: i64,
    /// 前 N 条明细
    pub entries: Vec<GitStatusEntry>,
}

impl RepoStatus {
    fn not_a_repo() -> Self {
        Self {
            is_repo: false,
            root: None,
            branch: None,
            head: None,
            changed_count: 0,
            untracked_count: 0,
            entries: Vec::new(),
        }
    }
}

/// 在 `dir` 下执行只读 git 命令；失败（含未装 Git）返回 `None`。
fn run_git(dir: &str, args: &[&str]) -> Option<String> {
    let out = hidden_command("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// 解析 `git status --porcelain=v1` 的一行：`XY <path>` / `XY <old> -> <new>`。
fn parse_status_line(line: &str) -> Option<GitStatusEntry> {
    if line.len() < 4 {
        return None;
    }
    let code = line[..2].trim();
    let raw = line[3..].trim();
    // 重命名/复制取新路径；带引号的路径（含特殊字符）去掉包裹的引号
    let path = raw
        .rsplit(" -> ")
        .next()
        .unwrap_or(raw)
        .trim_matches('"')
        .to_string();
    if path.is_empty() {
        return None;
    }
    let status = if code.is_empty() {
        "?".to_string()
    } else if code == "??" {
        "?".to_string()
    } else {
        code.chars().next().unwrap_or('?').to_string()
    };
    Some(GitStatusEntry { path, status })
}

fn collect_repo_status(dir: &str) -> RepoStatus {
    if dir.trim().is_empty() {
        return RepoStatus::not_a_repo();
    }
    let Some(root) = run_git(dir, &["rev-parse", "--show-toplevel"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return RepoStatus::not_a_repo();
    };

    // 分支名在 detached HEAD 时为空；HEAD 在空仓库（无提交）时取不到
    let branch = run_git(&root, &["branch", "--show-current"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let head = run_git(&root, &["rev-parse", "--short", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // 统一在仓库根执行：路径相对根，前端可与 root 拼成绝对路径打开文件
    let raw = run_git(&root, &["status", "--porcelain=v1", "-unormal"]).unwrap_or_default();
    let mut changed_count = 0i64;
    let mut untracked_count = 0i64;
    let mut entries = Vec::new();
    for line in raw.lines() {
        let Some(entry) = parse_status_line(line) else {
            continue;
        };
        if entry.status == "?" {
            untracked_count += 1;
        } else {
            changed_count += 1;
        }
        if entries.len() < MAX_REPO_STATUS_ENTRIES {
            entries.push(entry);
        }
    }

    RepoStatus {
        is_repo: true,
        root: Some(root),
        branch,
        head,
        changed_count,
        untracked_count,
        entries,
    }
}

/// 查询目录所属仓库的状态（文件历史面板顶部状态条）。
/// `cwd` 为空时回退到全局工作区（与 Agent / 房间未指定项目根时的默认目录一致）。
#[tauri::command]
pub async fn git_repo_status(
    state: State<'_, crate::DbState>,
    cwd: Option<String>,
) -> Result<RepoStatus, AppError> {
    let dir = match cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => v.to_string(),
        None => {
            let conn = state.get_conn()?;
            crate::utils::paths::resolve_workspace_path(None, "", &conn)
                .to_string_lossy()
                .to_string()
        }
    };
    // git 命令是阻塞调用（大仓库 status 可能耗时），放到阻塞线程池，避免占住异步执行器
    tauri::async_runtime::spawn_blocking(move || collect_repo_status(&dir))
        .await
        .map_err(|e| AppError::External(format!("仓库状态查询失败: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::parse_status_line;

    #[test]
    fn parses_modified_and_untracked_lines() {
        let modified = parse_status_line(" M src/lib.rs").expect("应解析出改动行");
        assert_eq!(modified.status, "M");
        assert_eq!(modified.path, "src/lib.rs");

        let untracked = parse_status_line("?? outputs/").expect("应解析出未跟踪行");
        assert_eq!(untracked.status, "?");
        assert_eq!(untracked.path, "outputs/");
    }

    #[test]
    fn parses_rename_takes_new_path() {
        let renamed = parse_status_line("R  old/name.rs -> new/name.rs").expect("应解析出重命名行");
        assert_eq!(renamed.status, "R");
        assert_eq!(renamed.path, "new/name.rs");
    }

    #[test]
    fn ignores_short_lines() {
        assert!(parse_status_line("").is_none());
        assert!(parse_status_line("M").is_none());
    }
}
