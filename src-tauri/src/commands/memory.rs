//! 项目级记忆（MEMORY.md）管理命令。
//!
//! 记忆根 = 会话实际使用的工作目录：`session.cwd` 非空取它，为空取兜底工作区
//! （`resolve_session_cwd` 与运行时一致）。只读/写入 `<记忆根>/MEMORY.md`，
//! 供设置页项目记忆区与右侧面板轻量预览使用；模型注入读取该文件在 lib.rs 完成。

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::api_agent::db::{
    MemoryEntry, MemoryStore, MEMORY_IDLE_SECS, MEMORY_MAX_ENTRIES, MEMORY_MIN_ACCESS,
};
use crate::utils::errors::AppError;
use crate::DbState;

/// MEMORY.md 文件名（固定于记忆根目录）。
const MEMORY_FILE: &str = "MEMORY.md";
/// 单文件内容上限（字符）：超出拒绝写入，避免挤占上下文。
const MAX_CHARS: usize = 12_000;

/// 项目记忆读取结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemory {
    pub root: String,
    pub exists: bool,
    pub content: String,
    pub char_count: usize,
}

/// 路径归一化键（用于授权比较）：反斜杠统一、去尾部分隔符、小写。
fn norm_key(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// 记忆根解析：`cwd` 非空用其解析路径，空串回退兜底工作区（与运行时一致）。
pub(crate) fn resolve_memory_root(conn: &Connection, cwd: Option<&str>) -> String {
    let c = cwd.map(str::trim).filter(|s| !s.is_empty());
    crate::utils::paths::resolve_session_cwd(conn, c)
}

/// 可授权的记忆根集合 = 会话显式 cwd（解析后）∪ 兜底工作区。
fn authorized_roots(conn: &Connection) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let push =
        |roots: &mut Vec<String>, seen: &mut std::collections::HashSet<String>, root: String| {
            let key = norm_key(&root);
            if !key.is_empty() && seen.insert(key) {
                roots.push(root);
            }
        };

    let mut stmt = match conn.prepare(
        "SELECT DISTINCT cwd FROM sessions WHERE cwd IS NOT NULL AND cwd <> '' ORDER BY cwd",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
        for row in rows.flatten() {
            let root = crate::utils::paths::resolve_session_cwd(conn, Some(&row));
            push(&mut roots, &mut seen, root);
        }
    }
    let fallback = crate::utils::paths::resolve_session_cwd(conn, None);
    push(&mut roots, &mut seen, fallback);
    roots
}

fn is_authorized_root(conn: &Connection, root: &str) -> bool {
    let key = norm_key(root);
    authorized_roots(conn).iter().any(|r| norm_key(r) == key)
}

fn read_named_file(root: &str, filename: &str) -> ProjectMemory {
    let path = format!("{}/{}", root.trim_end_matches(['/', '\\']), filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => ProjectMemory {
            root: root.to_string(),
            exists: true,
            char_count: content.chars().count(),
            content,
        },
        Err(_) => ProjectMemory {
            root: root.to_string(),
            exists: false,
            content: String::new(),
            char_count: 0,
        },
    }
}

fn read_memory_file(root: &str) -> ProjectMemory {
    read_named_file(root, MEMORY_FILE)
}

/// 原子写（同目录临时文件 + 覆盖目标；Windows 先删后更名）。
fn write_named_file(root: &str, filename: &str, content: &str) -> Result<(), AppError> {
    let dir = root.trim_end_matches(['/', '\\']);
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::Io(format!("无法创建目录 {}: {}", dir, e)))?;
    let target = format!("{}/{}", dir, filename);
    let tmp = format!("{}.{}.tmp", target, std::process::id());
    std::fs::write(&tmp, content)
        .map_err(|e| AppError::Io(format!("写入 {} 失败: {}", filename, e)))?;
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&tmp, &target)
        .map_err(|e| AppError::Io(format!("替换 {} 失败: {}", filename, e)))?;
    Ok(())
}

fn write_memory_file(root: &str, content: &str) -> Result<(), AppError> {
    write_named_file(root, MEMORY_FILE, content)
}

/// 已使用项目根列表（会话 cwd 去重 + 兜底工作区），供项目选择器。
#[tauri::command]
pub fn list_project_roots(state: State<'_, DbState>) -> Result<Vec<String>, String> {
    let conn = state.pool.get().map_err(AppError::from)?;
    Ok(authorized_roots(&conn))
}

/// 读取指定记忆根的 MEMORY.md（cwd 为空视为兜底记忆根）。
#[tauri::command]
pub fn get_project_memory(
    state: State<'_, DbState>,
    cwd: Option<String>,
) -> Result<ProjectMemory, String> {
    let conn = state.pool.get().map_err(AppError::from)?;
    let root = resolve_memory_root(&conn, cwd.as_deref());
    if !is_authorized_root(&conn, &root) {
        return Err(
            AppError::InvalidInput(format!("目录不在项目记忆范围内，拒绝读取: {}", root)).into(),
        );
    }
    Ok(read_memory_file(&root))
}

/// 写入指定记忆根的 MEMORY.md（空 content 视为清空该文件，保留空文件）。
#[tauri::command]
pub fn update_project_memory(
    state: State<'_, DbState>,
    cwd: Option<String>,
    content: String,
) -> Result<ProjectMemory, String> {
    let char_count = content.chars().count();
    if char_count > MAX_CHARS {
        return Err(AppError::InvalidInput(format!(
            "MEMORY.md 内容超长（{} 字，上限 {} 字），请精简后保存。",
            char_count, MAX_CHARS
        ))
        .into());
    }
    let conn = state.pool.get().map_err(AppError::from)?;
    let root = resolve_memory_root(&conn, cwd.as_deref());
    if !is_authorized_root(&conn, &root) {
        return Err(
            AppError::InvalidInput(format!("目录不在项目记忆范围内，拒绝写入: {}", root)).into(),
        );
    }
    write_memory_file(&root, &content)?;
    Ok(ProjectMemory {
        root,
        exists: true,
        char_count,
        content,
    })
}

/// MEMORY.md 新建模板。
#[tauri::command]
pub fn project_memory_template() -> Result<String, String> {
    Ok(format!(
        "# 项目记忆（MEMORY.md）\n\n\
         本文件为当前项目的长期记忆，会随每次对话注入模型上下文。请用简洁、可复用的事实与规则填写，不要记录过程性对话。建议分节维护：\n\n\
         ## 项目概述\n\n\
         ## 技术栈与代码约定\n\n\
         ## 常用路径\n\n\
         ## 用户偏好与决策记录\n\n\
         ## 注意事项\n"
    ))
}

// ── 用户偏好（USER.md，位于统一配置根；随会话随 USER.md 注入 model 上下文）──

/// USER.md 文件名（固定于统一配置根）。
const USER_FILE: &str = "USER.md";

/// 统一配置根（Windows: %APPDATA%/PilotDesk 等），与 MEMORY.db/技能共用。
fn config_root() -> Result<String, AppError> {
    crate::api_agent::system_prompt::get_pilotdesk_config_dir()
        .ok_or_else(|| AppError::Config("无法获取配置目录".to_string()))
}

/// 读取用户偏好 USER.md（文件不存在返回 exists=false）。
#[tauri::command]
pub fn get_user_preferences() -> Result<ProjectMemory, String> {
    let root = config_root()?;
    Ok(read_named_file(&root, USER_FILE))
}

/// 写入用户偏好 USER.md（空 content 视为清空，保留空文件）。
#[tauri::command]
pub fn update_user_preferences(content: String) -> Result<ProjectMemory, String> {
    let char_count = content.chars().count();
    if char_count > MAX_CHARS {
        return Err(AppError::InvalidInput(format!(
            "USER.md 内容超长（{} 字，上限 {} 字），请精简后保存。",
            char_count, MAX_CHARS
        ))
        .into());
    }
    let root = config_root()?;
    write_named_file(&root, USER_FILE, &content)?;
    Ok(ProjectMemory {
        root,
        exists: true,
        char_count,
        content,
    })
}

/// USER.md 新建模板。
#[tauri::command]
pub fn user_preferences_template() -> Result<String, String> {
    Ok(format!(
        "# 用户偏好（USER.md）\n\n\
         本文件为全局用户级偏好，会随每次对话注入模型上下文。请用简洁、可复用的事实与规则填写（如语言/框架偏好、常用命令、命名习惯等）：\n\n\
         ## 沟通风格\n\n\
         ## 技术偏好\n\n\
         ## 常用命令与工具\n\n\
         ## 通用约定\n"
    ))
}

// ── 全局 KV 记忆（MEMORY.db key_memories，与 save/search_memory 工具同库）──

/// 打开全局 KV 记忆库（基于配置目录下的 MEMORY.db）。
fn open_memory_store() -> Result<MemoryStore, AppError> {
    let dir = crate::api_agent::system_prompt::get_pilotdesk_config_dir()
        .ok_or_else(|| AppError::Config("无法获取配置目录".to_string()))?;
    MemoryStore::new(&dir).map_err(AppError::Db)
}

/// KV 条目视图（前端展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntryView {
    pub key: String,
    pub value: String,
    pub category: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub access_count: u64,
    pub last_accessed_at: u64,
    pub pin: bool,
    pub tags: String,
}

impl From<&MemoryEntry> for MemoryEntryView {
    fn from(e: &MemoryEntry) -> Self {
        Self {
            key: e.key.clone(),
            value: e.value.clone(),
            category: e.category.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
            access_count: e.access_count,
            last_accessed_at: e.last_accessed_at,
            pin: e.pin,
            tags: e.tags.clone(),
        }
    }
}

/// KV 记忆自动维护策略（与后端常量同源，供前端展示阈值）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPolicy {
    /// 总条数上限
    pub max_entries: usize,
    /// 冷记忆冷却期（天）
    pub idle_days: u64,
    /// 冷记忆访问次数上限（≤ 该值且超冷却期才可自动清理）
    pub min_access: u64,
}

impl MemoryPolicy {
    fn current() -> Self {
        Self {
            max_entries: MEMORY_MAX_ENTRIES,
            idle_days: MEMORY_IDLE_SECS / 86_400,
            min_access: MEMORY_MIN_ACCESS,
        }
    }
}

/// 全局 KV 记忆统计（总数/pin 数/注入 top-5/可清理候选数 + 策略）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    pub total: usize,
    pub pinned: usize,
    pub candidates: usize,
    pub injected: Vec<MemoryEntryView>,
    pub policy: MemoryPolicy,
}

/// 全量列表（可选分类/关键词过滤）。
#[tauri::command]
pub fn list_memory_entries(
    category: Option<String>,
    query: Option<String>,
) -> Result<Vec<MemoryEntryView>, String> {
    let store = open_memory_store()?;
    Ok(store
        .list_all(category.as_deref(), query.as_deref())
        .iter()
        .map(MemoryEntryView::from)
        .collect())
}

/// 新增/更新一条全局 KV 记忆（按 key 覆盖/新增；不做内容级去重，设置页手动编辑语义；
/// important=true 置 pin 保护，免自动清理；tags 为逗号分隔检索标签，可选）。
#[tauri::command]
pub fn save_memory_entry(
    key: String,
    value: String,
    category: String,
    important: Option<bool>,
    tags: Option<String>,
) -> Result<MemoryEntryView, String> {
    let key = key.trim().to_string();
    let value = value.trim().to_string();
    let category = category.trim().to_string();
    let tags = tags
        .map(|t| t.trim().trim_matches(',').to_string())
        .unwrap_or_default();
    if key.is_empty() || key.chars().count() > 200 {
        return Err(
            AppError::InvalidInput("记忆 key 不能为空且不超过 200 字符".to_string()).into(),
        );
    }
    if value.is_empty() || value.chars().count() > 4000 {
        return Err(
            AppError::InvalidInput("记忆 value 不能为空且不超过 4000 字符".to_string()).into(),
        );
    }
    if category.is_empty() || category.chars().count() > 32 {
        return Err(AppError::InvalidInput("记忆分类不能为空且不超过 32 字符".to_string()).into());
    }
    if tags.chars().count() > 200 {
        return Err(AppError::InvalidInput("记忆 tags 不能超过 200 字符".to_string()).into());
    }
    let store = open_memory_store()?;
    Ok(MemoryEntryView::from(&store.upsert_memory(
        &key,
        &value,
        &category,
        important.unwrap_or(false),
        &tags,
    )))
}

/// 删除一条全局 KV 记忆；返回是否命中。
/// 知识库条目会被拒绝（它们归「知识库」页管理），见 `MemoryStore::take_session_memory`。
#[tauri::command]
pub fn delete_memory_entry(key: String) -> Result<bool, String> {
    let store = open_memory_store()?;
    Ok(store.take_session_memory(key.trim())?.is_some())
}

/// 置/取消某条 KV 记忆的 pin（重要）标记。
#[tauri::command]
pub fn set_memory_pin(key: String, pin: bool) -> Result<(), String> {
    let store = open_memory_store()?;
    if store.set_pin(key.trim(), pin) {
        Ok(())
    } else {
        Err(AppError::NotFound(format!("未找到该记忆: {}", key.trim())).into())
    }
}

/// 预览自动维护将清理的候选条目（不删除），供设置页二次确认。
#[tauri::command]
pub fn preview_memory_maintenance() -> Result<Vec<MemoryEntryView>, String> {
    let store = open_memory_store()?;
    Ok(store
        .maintenance_candidates()
        .iter()
        .map(MemoryEntryView::from)
        .collect())
}

/// 执行一次自动维护（僵尸清理 + 配额驱逐），返回被删除的 key 列表。
#[tauri::command]
pub fn run_memory_maintenance() -> Result<Vec<String>, String> {
    let store = open_memory_store()?;
    let keys: Vec<String> = store
        .maintenance_candidates()
        .iter()
        .map(|e| e.key.clone())
        .collect();
    let mut removed = Vec::new();
    for key in keys {
        if store.delete_memory(&key) {
            removed.push(key);
        }
    }
    if !removed.is_empty() {
        log::info!("[Agent DB] 手动维护: 清理 {} 条冷记忆", removed.len());
    }
    Ok(removed)
}

/// 全局 KV 统计（总数/pin 数/注入 top-5/候选清理数 + 策略阈值）。
#[tauri::command]
pub fn get_memory_stats() -> Result<MemoryStats, String> {
    let store = open_memory_store()?;
    let candidates = store.maintenance_candidates().len();
    Ok(MemoryStats {
        // 与列表同一口径：只算会话记忆，知识库条目（不占 600 配额）不计入
        total: store.quota_count(),
        pinned: store.pinned_count(),
        candidates,
        // 与真实 system prompt 注入同一来源（统一评分排序），保证预览即所见
        injected: store
            .ranked_top(5)
            .iter()
            .map(MemoryEntryView::from)
            .collect(),
        policy: MemoryPolicy::current(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> String {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir()
            .join(format!(
                "pilotdesk_memory_md_test_{}_{}",
                std::process::id(),
                n
            ))
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn test_read_missing_and_write_read() {
        let dir = temp_root();
        let _ = std::fs::remove_dir_all(&dir);

        let none = read_memory_file(&dir);
        assert!(!none.exists);
        assert!(none.content.is_empty());

        write_memory_file(&dir, "# 项目记忆\nhi").unwrap();
        let some = read_memory_file(&dir);
        assert!(some.exists);
        assert!(some.content.contains("hi"));

        // 清空：写空串保留空文件
        write_memory_file(&dir, "").unwrap();
        let cleared = read_memory_file(&dir);
        assert!(cleared.exists);
        assert!(cleared.content.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_norm_key() {
        assert_eq!(norm_key("C:\\Project\\"), "c:/project");
        assert_eq!(norm_key("c:/project"), "c:/project");
    }
}
