#![allow(dead_code)]
//! 统一路径管理模块
//!
//! 职责范围：本地文件系统路径的统一管理与派生
//! 配置、数据库与用户工作空间分开管理。
//!
//! 工作区路径解析规则：调用方已持有的运行根 → 全局工作空间（app_settings）
//! → 全局工作空间的用户可见兜底目录（`<文档>/PilotDesk` → `<用户主目录>/PilotDesk`）。
//! 文档目录本身不是独立工作根，只是“全局工作空间未配置”时的实现兜底。
//!
//! 不包含：远程在线商店 URL 配置
//!   → Rust 端：见 market.rs（SERVER_SOURCES / build_urls / PLUGINS_INDEX_PATH 等）
//!   → 前端：  见 src/utils/market.ts（与 market.rs 保持镜像同步）
//!   注意：两端的 SERVER_SOURCES 和路径常量必须手动保持一致
//!
//! 全局只有三类路径（其余都从它们派生）：
//!   1. 配置目录      app_root_dir()：主库 / MEMORY.db / plugins / resources / skills —— 用户基本不用维护
//!   2. 附件目录      `<工作根>/attachments/<owner_id>` —— 用户一般不用维护
//!   3. 产物目录      `<工作根>/outputs/<名字>` —— 用户可见
//! 「全局工作空间」是未归属项目时的默认工作根（见 resolve_workspace_path）。

use std::path::PathBuf;

/// 将 Windows 扩展长度路径转换为用户可读形式。
///
/// `std::fs::canonicalize` 在 Windows 上会返回 `\\?\C:\...`；UNC 路径则是
/// `\\?\UNC\server\share`。这些前缀只属于 Win32 文件 API，不应写入工具输出、
/// 消息历史或界面。非扩展长度路径保持原样。
pub fn strip_windows_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{}", rest);
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            return rest.to_string();
        }
    }
    path.to_string()
}

/// 用户可读的路径字符串。实际文件操作仍应优先使用原始 `Path`。
pub fn path_display(path: &std::path::Path) -> String {
    strip_windows_verbatim_prefix(&path.to_string_lossy())
}

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
    // Keep normal installs on the platform default, but allow development and
    // automated acceptance runs to use a throwaway data root.
    if let Some(root) = std::env::var_os("PILOTDESK_APP_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
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

/// 全局工作空间未配置时的用户可见兜底目录。
///
/// 优先用户文档目录，无法取得时回退用户主目录。应用配置目录属于内部数据，
/// 不能再被当作会话/圆桌/工作流的运行工作根。
fn global_workspace_fallback_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("PilotDesk")
}

/// 一次全局工作空间解析结果。
///
/// `configured=false` 表示使用用户可见兜底目录，但语义上仍属于「全局工作空间」，
/// 不是第四种工作根来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalWorkspace {
    pub path: PathBuf,
    pub configured: bool,
}

/// 数据库文件路径
pub fn db_path() -> PathBuf {
    app_root_dir().join("pilotdesk.db")
}

/// 飞地（Enclave）团队数据库文件路径。
///
/// 与 `pilotdesk.db` 同根（应用配置目录）但**物理独立**：团队业务数据不混入个人库，
/// 也不会被个人侧的同步/备份带走（见设计 §6.1）。
pub fn enclave_db_path() -> PathBuf {
    app_root_dir().join("enclave.db")
}

/// 飞地文件资产目录：`<应用根>/enclave/files/`（任务附件与汇报成果的原文档）。
pub fn enclave_files_dir() -> PathBuf {
    app_root_dir().join("enclave").join("files")
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

/// 技能安装目录（API Agent 的固定技能根：`<配置根>/skills`，与 Agent 自定义 skills_dir 无关）
pub fn skills_dir() -> PathBuf {
    app_root_dir().join("skills")
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

/// 解析全局工作空间：用户配置 -> 用户可见兜底目录。
///
/// v14 之前新库可能把应用配置目录写成默认值；该值不代表用户选择，按未配置处理。
pub fn resolve_global_workspace(conn: &rusqlite::Connection) -> GlobalWorkspace {
    match get_global_workspace(conn) {
        Some(global) => {
            let resolved = normalize_absolute_path(&global, "");
            if !resolved.is_absolute() || path_key(&resolved) == path_key(&app_root_dir()) {
                GlobalWorkspace {
                    path: global_workspace_fallback_dir(),
                    configured: false,
                }
            } else {
                GlobalWorkspace {
                    path: resolved,
                    configured: true,
                }
            }
        }
        None => GlobalWorkspace {
            path: global_workspace_fallback_dir(),
            configured: false,
        },
    }
}

/// 在调用方已持有运行根时派生其子路径；否则从全局工作空间派生。
///
/// 这里不接受“用户为普通工作另行指定的工作根”。运行根只能由
/// `commands::projects` 按宿主绑定、项目归属、全局工作空间三级规则解析。
pub fn resolve_workspace_path(
    caller_workspace: Option<&str>,
    sub_path: &str,
    conn: &rusqlite::Connection,
) -> PathBuf {
    match caller_workspace {
        Some(dir) if !dir.trim().is_empty() => normalize_absolute_path(sub_path, dir),
        _ => normalize_absolute_path(
            sub_path,
            &resolve_global_workspace(conn).path.to_string_lossy(),
        ),
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

/// Lexically normalize `.` / `..` without requiring the path to exist.
fn normalize_path_components(path: &std::path::Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !path.has_root() {
                    normalized.push("..");
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// Resolve a user-supplied path to a normalized absolute path.
///
/// Existing files/roots are canonicalized. For a new file, the nearest existing
/// parent is canonicalized before appending the filename so `.` / `..` and
/// symlinks do not leak into persisted file-history paths.
pub fn normalize_absolute_path(raw_path: &str, cwd: &str) -> PathBuf {
    let expanded = strip_windows_verbatim_prefix(&resolve_tilde(raw_path));
    let candidate = if std::path::Path::new(&expanded).is_absolute() {
        PathBuf::from(expanded)
    } else {
        PathBuf::from(strip_windows_verbatim_prefix(cwd)).join(expanded)
    };
    let lexical = normalize_path_components(&candidate);
    if let Ok(canonical) = std::fs::canonicalize(&lexical) {
        return PathBuf::from(strip_windows_verbatim_prefix(&canonical.to_string_lossy()));
    }
    if let (Some(parent), Some(file_name)) = (lexical.parent(), lexical.file_name()) {
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            let resolved = canonical_parent.join(file_name);
            return PathBuf::from(strip_windows_verbatim_prefix(&resolved.to_string_lossy()));
        }
    }
    lexical
}

fn is_remote_context_path(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("base64:")
}

fn normalize_context_path(value: &str, cwd: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || is_remote_context_path(value) {
        return None;
    }
    let normalized = normalize_absolute_path(value, cwd);
    normalized
        .is_absolute()
        .then(|| normalized.to_string_lossy().to_string())
}

fn normalize_json_path_field(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    cwd: &str,
) {
    let Some(serde_json::Value::String(raw)) = object.get_mut(key) else {
        return;
    };
    if let Some(normalized) = normalize_context_path(raw, cwd) {
        *raw = normalized;
    }
}

fn normalize_json_path_array(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    cwd: &str,
) {
    let Some(serde_json::Value::Array(items)) = object.get_mut(key) else {
        return;
    };
    for item in items {
        let serde_json::Value::String(raw) = item else {
            continue;
        };
        if let Some(normalized) = normalize_context_path(raw, cwd) {
            *raw = normalized;
        }
    }
}

/// Normalize persisted tool-context paths before they enter message history.
///
/// Only built-in local-file tools are rewritten. Remote URLs/data URIs and dynamic
/// MCP arguments keep their original shape, because their transport semantics may
/// require a virtual or remote path rather than a local absolute path.
pub fn normalize_tool_call_context_paths(tool_calls: &str, cwd: &str) -> String {
    if cwd.trim().is_empty() {
        return tool_calls.to_string();
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(tool_calls) else {
        return tool_calls.to_string();
    };
    let Some(steps) = value.as_array_mut() else {
        return tool_calls.to_string();
    };

    for step in steps {
        let Some(object) = step.as_object_mut() else {
            continue;
        };
        normalize_json_path_field(object, "filePath", cwd);
        let tool_name = object
            .get("toolName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some(mut args_value) = object.get("args").cloned() else {
            continue;
        };
        let mut args = match &args_value {
            serde_json::Value::String(raw) => match serde_json::from_str(raw) {
                Ok(value) => value,
                Err(_) => continue,
            },
            serde_json::Value::Object(_) => args_value.clone(),
            _ => continue,
        };
        let Some(args_object) = args.as_object_mut() else {
            continue;
        };

        match tool_name.as_str() {
            "write_file" | "edit_file" | "read_file" | "read_image" | "list_files" | "glob"
            | "grep" | "browser" => normalize_json_path_field(args_object, "path", cwd),
            "parse_document" | "create_document" => {
                normalize_json_path_field(args_object, "file", cwd)
            }
            "edit_image" => {
                normalize_json_path_field(args_object, "image", cwd);
                normalize_json_path_field(args_object, "mask", cwd);
            }
            "generate_image" => {
                normalize_json_path_array(args_object, "referenceImages", cwd);
                normalize_json_path_array(args_object, "image", cwd);
                normalize_json_path_field(args_object, "mask", cwd);
            }
            "generate_video" => {
                normalize_json_path_field(args_object, "firstFrame", cwd);
                normalize_json_path_field(args_object, "lastFrame", cwd);
                normalize_json_path_array(args_object, "referenceImages", cwd);
            }
            "stt" => {
                normalize_json_path_field(args_object, "image", cwd);
                normalize_json_path_field(args_object, "file", cwd);
            }
            "execute_command" => normalize_json_path_field(args_object, "cwd", cwd),
            "execute_python" => {
                let python_cwd = args_object
                    .get("cwd")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|raw| normalize_context_path(raw, cwd));
                normalize_json_path_field(args_object, "cwd", cwd);
                if let Some(python_cwd) = python_cwd {
                    normalize_json_path_field(args_object, "file", &python_cwd);
                } else {
                    normalize_json_path_field(args_object, "file", cwd);
                }
            }
            _ => {}
        }
        normalize_json_path_array(args_object, "files", cwd);

        let primary_path = matches!(
            tool_name.as_str(),
            "write_file" | "edit_file" | "create_document" | "edit_image"
        )
        .then(|| {
            args_object
                .get("path")
                .or_else(|| args_object.get("file"))
                .or_else(|| args_object.get("image"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .flatten();
        if object.get("filePath").is_none() {
            if let Some(primary_path) = primary_path {
                object.insert(
                    "filePath".to_string(),
                    serde_json::Value::String(primary_path),
                );
            }
        }
        args_value = match args_value {
            serde_json::Value::String(_) => {
                serde_json::Value::String(serde_json::to_string(&args).unwrap_or_default())
            }
            _ => args,
        };
        object.insert("args".to_string(), args_value);
    }

    serde_json::to_string(&value).unwrap_or_else(|_| tool_calls.to_string())
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

/// 从工作根派生一次运行使用的产物目录：`<工作根>/outputs/<名字>`。
///
/// `owner_name` 只能传工作名称（会话/圆桌/工作流标题），不能传调用方持有的绝对路径。
/// 返回值属于运行时派生值，不持久化；工作归属变化后，下一次运行自然落到新工作根下。
pub fn workspace_artifacts_dir(work_root: &str, owner_name: &str, fallback: &str) -> PathBuf {
    let base = normalize_absolute_path(work_root, "");
    let dir_name = sanitize_dir_name(owner_name.trim(), fallback);
    let dir = base.join("outputs").join(dir_name);
    let _ = std::fs::create_dir_all(&dir);
    normalize_absolute_path(&dir.to_string_lossy(), "")
}

/// 工作流产物目录：`<工作根>/outputs/<工作流名>`；无项目时工作根即全局工作区。
///
/// 与群聊的 `<全局工作区>/outputs/<房间名>` 同构：按名字归类、不按执行实例或时间分层、
/// 不做清理。返回值只作为产物落点与额外安全根（工作区路径权限仍由矩阵统一判定）；
/// Agent 的 cwd 始终是项目主目录或全局工作区，不随产物目录变化。目录不存在时顺带创建。
pub fn workflow_artifacts_dir(
    conn: &rusqlite::Connection,
    workflow_name: &str,
    work_root: Option<&str>,
) -> PathBuf {
    let base = work_root
        .filter(|path| !path.trim().is_empty())
        .map(|path| normalize_absolute_path(path, ""))
        .unwrap_or_else(|| resolve_workspace_path(None, "", conn));
    workspace_artifacts_dir(&base.to_string_lossy(), workflow_name, "workflow")
}

/// 会话/圆桌附件落盘目录：`<动态工作根>/attachments/<owner_id>`
pub fn resolve_attachments_dir(work_root: &str, owner_id: &str) -> PathBuf {
    normalize_absolute_path(work_root, "")
        .join("attachments")
        .join(owner_id)
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
    fn workflow_artifacts_live_under_outputs_not_at_the_work_root() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let work_root = tmp_dir("workflow-root");
        let artifacts =
            workflow_artifacts_dir(&conn, "Release Notes", Some(&work_root.to_string_lossy()));

        assert_eq!(
            path_key(&artifacts),
            path_key(&work_root.join("outputs").join("Release Notes"))
        );
        assert_ne!(artifacts, work_root);
        assert!(artifacts.exists(), "产物目录应在执行前准备好");
    }

    #[test]
    fn absolute_paths_are_normalized_for_new_files() {
        let root = tmp_dir("normalize-path");
        let normalized =
            normalize_absolute_path("../report.md", &root.join("nested").to_string_lossy());
        let canonical_root = PathBuf::from(strip_windows_verbatim_prefix(
            &std::fs::canonicalize(&root).unwrap().to_string_lossy(),
        ));

        assert_eq!(normalized, canonical_root.join("report.md"));
        assert!(normalized.is_absolute());
        assert!(!normalized.to_string_lossy().starts_with(r"\\?\"));
    }

    #[test]
    fn windows_verbatim_paths_are_user_readable() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\E:\tmp\outputs\report.md"),
            r"E:\tmp\outputs\report.md"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\server\share\report.md"),
            r"\\server\share\report.md"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"E:\tmp\outputs\report.md"),
            r"E:\tmp\outputs\report.md"
        );
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
