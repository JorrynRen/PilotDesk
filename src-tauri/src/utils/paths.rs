#![allow(dead_code)]
//! 统一路径管理模块
//!
//! 职责范围：本地文件系统路径的统一管理与派生
//! 所有路径派生自**应用根目录**（app_root_dir，见下）
//!
//! 工作区路径解析规则：调用方自有配置 → 全局工作区（app_settings）→ 应用根目录
//!
//! 不包含：远程在线商店 URL 配置
//!   → Rust 端：见 market.rs（SERVER_SOURCES / build_urls / PLUGINS_INDEX_PATH 等）
//!   → 前端：  见 src/utils/market.ts（与 market.rs 保持镜像同步）
//!   注意：两端的 SERVER_SOURCES 和路径常量必须手动保持一致
//!
//! 全局只有三类路径（其余都从它们派生）：
//!   1. 配置目录      app_root_dir()：主库 / MEMORY.db / plugins / resources / skills —— 用户基本不用维护
//!   2. 附件目录      `<会话工作目录>/attachments/<session_id>` —— 用户一般不用维护
//!   3. 产物目录      `<全局工作区>/outputs/<名字>` —— 用户可见
//! 「全局工作区」本身就是"用户未选用其它位置时"的兜底目录（见 resolve_workspace_path）。

use std::path::PathBuf;

/// 应用根目录（配置与数据同根）。
///
/// - Windows: `%APPDATA%\PilotDesk`
/// - Linux/macOS: `$XDG_CONFIG_HOME/pilotdesk`（未设则 `~/.config/pilotdesk`）
///
/// 为什么非 Windows 也用 config 根而不是 `dirs::data_dir()`：主库、`MEMORY.db`、技能目录
/// 必须同根，否则会出现"两个根、两种大小写"（曾出现：主库在 `~/.local/share/PilotDesk`，
/// 记忆库在 `~/.config/pilotdesk`，同一台机器上数据被劈成两半）。
///
/// 注意：知识库正文这类**大体积用户数据**不放在这里 —— 它们放「全局工作区/Knowledge」，
/// 那个位置用户可见、可换盘、可整体备份。
pub fn app_root_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir()
            .expect("Cannot determine app data directory")
            .join("PilotDesk")
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
            .expect("Cannot determine app config directory")
            .join("pilotdesk")
    }
}

/// 数据库文件路径
pub fn db_path() -> PathBuf {
    app_root_dir().join("pilotdesk.db")
}

/// 加密密钥文件路径
pub fn encryption_key_path() -> PathBuf {
    app_root_dir().join(".key")
}

/// 用户资源目录
pub fn user_resources_dir() -> PathBuf {
    app_root_dir().join("resources")
}

/// Agent 图标目录
pub fn agent_icons_dir() -> PathBuf {
    user_resources_dir().join("icons")
}

/// Agent 自定义配置目录
pub fn agent_configs_dir() -> PathBuf {
    user_resources_dir().join("agents")
}

/// 其他资源目录
pub fn assets_dir() -> PathBuf {
    user_resources_dir().join("assets")
}

/// 插件安装目录
pub fn plugins_dir() -> PathBuf {
    app_root_dir().join("plugins")
}

/// 工作流模板目录（预留）
pub fn templates_dir() -> PathBuf {
    app_root_dir().join("templates")
}

/// 用户系统资源目录（预留）
pub fn users_dir() -> PathBuf {
    app_root_dir().join("users")
}

/// 从 app_settings 表读取配置（空串视为未设置）
pub fn get_app_setting_opt(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    use rusqlite::OptionalExtension;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = ?",
            rusqlite::params![key],
            |row| row.get("value"),
        )
        .optional()
        .ok()?;
    value.and_then(|v| if v.is_empty() { None } else { Some(v) })
}

/// 从 app_settings 表读取全局工作区路径
/// 键名: pilotdesk-workspace
pub fn get_global_workspace(conn: &rusqlite::Connection) -> Option<String> {
    get_app_setting_opt(conn, "pilotdesk-workspace")
}

/// 解析工作区路径：调用方自有配置 -> 全局工作区 -> 应用根目录
pub fn resolve_workspace_path(
    caller_workspace: Option<&str>,
    sub_path: &str,
    conn: &rusqlite::Connection,
) -> PathBuf {
    match caller_workspace {
        Some(dir) if !dir.is_empty() => {
            let resolved = resolve_tilde(dir);
            PathBuf::from(resolved).join(sub_path)
        }
        _ => match get_global_workspace(conn) {
            Some(global) => {
                let resolved = resolve_tilde(&global);
                PathBuf::from(resolved).join(sub_path)
            }
            None => app_root_dir().join(sub_path),
        },
    }
}

/// 知识库目录名：与产物目录 `outputs/` 同级放在全局工作区下，用户看得见、可换盘、可整体备份
pub const KNOWLEDGE_DIR_NAME: &str = "Knowledge";

/// 知识库根目录里的"勿手工清理"标记文件（资产 vs 产物的边界提示）
const KNOWLEDGE_MARKER: &str = ".pilotdesk-knowledge";

/// 知识库文件根目录：app_settings `knowledge-root` → 全局工作区/Knowledge → 应用根目录/Knowledge。
///
/// 设置里改的就是这个根（支持 `~`）；DB 里只存相对路径，所以换根/换盘不会让已入库的原文失效。
/// 注意 `MEMORY.db` **不跟着走**（始终在本地配置目录）—— SQLite 放同步盘会损坏。
pub fn resolve_knowledge_root(conn: &rusqlite::Connection) -> PathBuf {
    match get_app_setting_opt(conn, "knowledge-root") {
        Some(custom) => PathBuf::from(resolve_tilde(&custom)),
        None => resolve_workspace_path(None, KNOWLEDGE_DIR_NAME, conn),
    }
}

/// 确保知识库根与 `files/` 就绪，并写一个标记文件。
///
/// 标记文件的用处：根目录默认落在全局工作区里，与可随时清理的 `outputs/` 同一棵树下 ——
/// 用户手工清理工作区时，至少要看到"这里由 PilotDesk 管理、不要删"这句话。
pub fn ensure_knowledge_dirs(root: &std::path::Path) -> Result<(), String> {
    let files = root.join("files");
    std::fs::create_dir_all(&files)
        .map_err(|e| format!("创建知识库目录失败 {}: {}", files.display(), e))?;
    let marker = root.join(KNOWLEDGE_MARKER);
    if !marker.exists() {
        let _ = std::fs::write(
            &marker,
            "本目录由 PilotDesk 管理：存放知识库原文（files/）。\n\
             这是知识资产，不是可清理的产物（产物在 ../outputs/）——请勿手工删除。\n",
        );
    }
    Ok(())
}

/// 知识库原文目录：`<知识库根>/files`
pub fn knowledge_files_dir(root: &std::path::Path) -> PathBuf {
    root.join("files")
}

/// 统计知识库原文：文件数与总字节数（files/ 是扁平的，一层 read_dir 就够）
pub fn count_knowledge_files(root: &std::path::Path) -> (usize, u64) {
    let dir = knowledge_files_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (0, 0);
    };
    let mut count = 0usize;
    let mut bytes = 0u64;
    for e in entries.flatten() {
        if let Ok(meta) = e.metadata() {
            if meta.is_file() {
                count += 1;
                bytes += meta.len();
            }
        }
    }
    (count, bytes)
}

/// 迁移结果：搬走的 / 跳过的（目标已存在且同名，同名即同 sha ⇒ 同内容）/ 失败的清单
#[derive(Debug, Default)]
pub struct MigrateOutcome {
    pub moved: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
}

/// 把知识库原文从旧根搬到新根（**幂等**：目标已有同名文件就跳过，重试必然收敛）。
///
/// 同盘走 rename（瞬时、不额外占空间）；跨盘 rename 失败时退化为复制 + 删源，
/// 复制失败则保留源文件并记入 failed —— 绝不在没拿到目标副本前删源。
pub fn migrate_knowledge_files(
    old_root: &std::path::Path,
    new_root: &std::path::Path,
) -> Result<MigrateOutcome, String> {
    let from = knowledge_files_dir(old_root);
    if !from.exists() {
        return Ok(MigrateOutcome::default());
    }
    let to = knowledge_files_dir(new_root);
    std::fs::create_dir_all(&to)
        .map_err(|e| format!("创建目标目录失败 {}: {}", to.display(), e))?;

    let mut out = MigrateOutcome::default();
    let entries = std::fs::read_dir(&from).map_err(|e| format!("读取旧目录失败: {}", e))?;
    for entry in entries.flatten() {
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        let Some(name) = src.file_name() else {
            continue;
        };
        let dst = to.join(name);
        if dst.exists() {
            // 目标已存在：文件名含内容指纹（<sha8>-<原名>）⇒ 同名即同内容，这是重复副本。
            // 必须把源删掉，否则旧目录永远清不干净、重试也无法收敛。
            // 体积不同属"指纹撞车/文件被改"的反常情况：两边都留着并报出来，由人处理。
            let src_len = std::fs::metadata(&src).map(|m| m.len()).ok();
            let dst_len = std::fs::metadata(&dst).map(|m| m.len()).ok();
            if src_len.is_some() && src_len == dst_len {
                let _ = std::fs::remove_file(&src);
                out.skipped += 1;
            } else {
                out.failed.push(format!(
                    "{}: 目标已存在且大小不同，未处理",
                    name.to_string_lossy()
                ));
            }
            continue;
        }
        match std::fs::rename(&src, &dst) {
            Ok(()) => out.moved += 1,
            Err(_) => match std::fs::copy(&src, &dst) {
                Ok(_) => {
                    // 复制成功才删源；删不掉也不算失败（目标已在，下次会跳过）
                    let _ = std::fs::remove_file(&src);
                    out.moved += 1;
                }
                Err(e) => out
                    .failed
                    .push(format!("{}: {}", name.to_string_lossy(), e)),
            },
        }
    }
    Ok(out)
}

/// 迁移成功后清理旧根的空壳。
///
/// 只清**我们自己创建的东西**：标记文件 `.pilotdesk-knowledge` 与空的 `files/`；
/// 旧根里还有别的内容（用户的 outputs/、其它文件）就整目录保留 —— 我们不是这个目录的主人。
pub fn cleanup_old_knowledge_root(old_root: &std::path::Path) {
    let marker = old_root.join(KNOWLEDGE_MARKER);
    if !marker.exists() {
        return; // 没标记 = 不是我们建的根，不碰
    }
    let files = knowledge_files_dir(old_root);
    let files_empty = std::fs::read_dir(&files)
        .map(|it| it.flatten().next().is_none())
        .unwrap_or(false);
    if files_empty {
        let _ = std::fs::remove_dir(&files);
    }
    // files/ 已清掉且目录里没有别的东西 → 连标记与目录一起收掉
    let dir_empty = std::fs::read_dir(old_root)
        .map(|it| {
            it.flatten()
                .all(|e| e.file_name() == std::ffi::OsStr::new(KNOWLEDGE_MARKER))
        })
        .unwrap_or(false);
    if files_empty && dir_empty {
        let _ = std::fs::remove_file(&marker);
        let _ = std::fs::remove_dir(old_root);
    }
}

/// 路径归一化键（比较用）：canonicalize 后统一分隔符、去尾部分隔符、小写（Windows 大小写不敏感）
pub fn path_key(p: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    resolved
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// 解析会话工作目录：`session.cwd` 优先，为空则回退到全局工作区（含 app_root_dir 兜底）
pub fn resolve_session_cwd(conn: &rusqlite::Connection, session_cwd: Option<&str>) -> String {
    match session_cwd {
        Some(dir) if !dir.is_empty() => resolve_tilde(dir),
        _ => resolve_workspace_path(None, "", conn)
            .to_string_lossy()
            .to_string(),
    }
}

/// 目录名清洗：过滤 Windows 非法字符、剥离结尾点/空格（Windows 会静默剥离导致路径漂移）、
/// 截断 64 字符、保留设备名（CON/PRN/...）兜底；全空时用 `fallback`。
///
/// 群聊产物目录与工作流产物目录共用这一份规则，避免两边各写一套后行为漂移。
pub fn sanitize_dir_name(title: &str, fallback: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    let truncated: String = trimmed.chars().take(64).collect();
    if truncated.is_empty() {
        return fallback.to_string();
    }
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let upper = truncated.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or(&upper);
    if RESERVED.contains(&base) {
        format!("{}_dir", truncated)
    } else {
        truncated
    }
}

/// 工作流产物目录：`<全局工作区>/outputs/<工作流名>`。
///
/// 与群聊的 `<全局工作区>/outputs/<房间名>` 同构：按名字归类、不按执行实例或时间分层、
/// 不做清理。返回值同时被用作节点的 cwd 与授权边界（工作区内路径免审批），
/// 目录不存在时顺带创建。
pub fn workflow_artifacts_dir(conn: &rusqlite::Connection, workflow_name: &str) -> PathBuf {
    let base = resolve_workspace_path(None, "", conn)
        .to_string_lossy()
        .to_string();
    let dir_name = sanitize_dir_name(workflow_name, "workflow");
    let dir = PathBuf::from(format!(
        "{}\\outputs\\{}",
        base.trim_end_matches(['\\', '/']),
        dir_name
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 会话附件落盘目录：`<工作目录>/attachments/<session_id>`
pub fn resolve_attachments_dir(
    conn: &rusqlite::Connection,
    session_cwd: Option<&str>,
    session_id: &str,
) -> PathBuf {
    PathBuf::from(resolve_session_cwd(conn, session_cwd))
        .join("attachments")
        .join(session_id)
}

/// 解析路径中的 `~` 与 `%USERPROFILE%` 为用户 home 目录
/// （`~` 同时支持 `~/` 与 `~\` 两种分隔符）
/// `pub(crate)`：命令层处理用户手输的目录（如知识库根）也要走同一套展开
pub(crate) fn resolve_tilde(path: &str) -> String {
    if path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            let rest = &path[2..];
            return home.join(rest).to_string_lossy().to_string();
        }
    } else if path.to_lowercase().starts_with("%userprofile%") {
        if let Some(home) = dirs::home_dir() {
            let rest = &path[14..]; // len("%USERPROFILE%") == 14
            return home
                .join(rest.trim_start_matches('\\').trim_start_matches('/'))
                .to_string_lossy()
                .to_string();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// 独立临时目录（带纳秒：只用 pid + 序号会因系统复用 pid 而撞上上一轮遗留的文件）
    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk_paths_{}_{}_{}_{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn migrate_moves_files_and_is_idempotent() {
        let old = tmp_dir("old");
        let new = tmp_dir("new");
        ensure_knowledge_dirs(&old).unwrap();
        ensure_knowledge_dirs(&new).unwrap();
        std::fs::write(knowledge_files_dir(&old).join("a-1.md"), b"AAA").unwrap();
        std::fs::write(knowledge_files_dir(&old).join("b-2.md"), b"BBB").unwrap();
        // 目标已存在同名文件：同名即同内容（文件名含内容指纹）→ 应当跳过
        std::fs::write(knowledge_files_dir(&new).join("b-2.md"), b"BBB").unwrap();

        let out = migrate_knowledge_files(&old, &new).unwrap();
        assert_eq!(out.moved, 1);
        assert_eq!(out.skipped, 1);
        assert!(out.failed.is_empty());
        assert!(knowledge_files_dir(&new).join("a-1.md").exists());
        assert!(!knowledge_files_dir(&old).join("a-1.md").exists());

        // 再跑一次：源已空 → 全是跳过，不报错（重试收敛）
        let again = migrate_knowledge_files(&old, &new).unwrap();
        assert_eq!(again.moved, 0);
        assert_eq!(again.skipped, 0, "源目录已空，没有可搬的了");
    }

    #[test]
    fn cleanup_only_touches_our_own_root() {
        // 1) 我们建的根 + files 空 → 连目录一起收掉
        let mine = tmp_dir("mine");
        ensure_knowledge_dirs(&mine).unwrap();
        cleanup_old_knowledge_root(&mine);
        assert!(!mine.exists(), "自己的空壳应当被清理");

        // 2) 没有标记文件 → 一律不碰
        let foreign = tmp_dir("foreign");
        std::fs::create_dir_all(knowledge_files_dir(&foreign)).unwrap();
        cleanup_old_knowledge_root(&foreign);
        assert!(foreign.exists());
        assert!(knowledge_files_dir(&foreign).exists());

        // 3) 我们的根，但目录里还有别的东西（如 outputs/）→ 保留目录，只清空的 files/
        let shared = tmp_dir("shared");
        ensure_knowledge_dirs(&shared).unwrap();
        std::fs::create_dir_all(shared.join("outputs")).unwrap();
        cleanup_old_knowledge_root(&shared);
        assert!(shared.exists(), "还有别人的东西就不该删目录");
        assert!(shared.join("outputs").exists());
        assert!(
            !knowledge_files_dir(&shared).exists(),
            "空的 files/ 应当被收掉"
        );

        // 4) files/ 里还有文件 → 什么都不删
        let busy = tmp_dir("busy");
        ensure_knowledge_dirs(&busy).unwrap();
        std::fs::write(knowledge_files_dir(&busy).join("x.md"), b"x").unwrap();
        cleanup_old_knowledge_root(&busy);
        assert!(knowledge_files_dir(&busy).exists());
        assert!(knowledge_files_dir(&busy).join("x.md").exists());
    }

    #[test]
    fn count_reports_files_and_bytes() {
        let root = tmp_dir("count");
        ensure_knowledge_dirs(&root).unwrap();
        std::fs::write(knowledge_files_dir(&root).join("a.md"), b"12345").unwrap();
        std::fs::write(knowledge_files_dir(&root).join("b.md"), b"12").unwrap();
        let (n, bytes) = count_knowledge_files(&root);
        assert_eq!(n, 2);
        assert_eq!(bytes, 7);
    }
}

/// 获取内置资源目录（开发模式）
#[allow(dead_code)]
pub fn builtin_resources_dir() -> PathBuf {
    let mut dir = std::env::current_exe().expect("Cannot determine executable path");
    dir.pop();
    dir.join("resources")
}
