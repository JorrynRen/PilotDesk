pub use crate::db::models::Inspiration;
use crate::utils::errors::AppError;
use rusqlite::{params, Connection};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInspirationPayload {
    pub icon: Option<String>,
    pub title: String,
    pub content: String,
    pub source_agent: Option<String>,
    pub tags: Option<Vec<String>>,
    /// 来自「资源市集 › 灵感」时带上市场那条的 id，用于标记来源与去重；用户自建不传。
    pub market_id: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInspirationPayload {
    pub id: String,
    pub icon: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub source_agent: Option<String>,
    pub is_favorite: Option<bool>,
    pub tags: Option<Vec<String>>,
}

fn row_to_inspiration(row: &rusqlite::Row<'_>) -> Result<Inspiration, rusqlite::Error> {
    let tags_json: String = row.get("tags")?;
    let tags: Vec<String> = if tags_json.is_empty() || tags_json == "[]" || tags_json == "[null]" {
        vec![]
    } else {
        serde_json::from_str(&tags_json).unwrap_or_default()
    };
    Ok(Inspiration {
        id: row.get("id")?,
        icon: row.get("icon")?,
        title: row.get("title")?,
        content: row.get("content")?,
        source_agent: row.get("source_agent")?,
        is_favorite: row.get::<_, i32>("is_favorite")? != 0,
        tags,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        market_id: row.get("market_id")?,
    })
}

/// List all inspirations with optional tag filter
pub fn list_inspirations(
    conn: &Connection,
    tag: Option<String>,
    favorite_only: bool,
) -> Result<Vec<Inspiration>, AppError> {
    let mut sql = String::from(
        "SELECT i.id, i.icon, i.title, i.content, i.source_agent, i.is_favorite, i.created_at, i.updated_at, i.market_id,          COALESCE(json_group_array(it.tag) FILTER (WHERE it.tag IS NOT NULL), '[]') as tags          FROM inspirations i          LEFT JOIN inspiration_tags it ON i.id = it.inspiration_id"
    );
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut conditions: Vec<String> = Vec::new();

    if favorite_only {
        conditions.push("i.is_favorite = 1".to_string());
    }

    if let Some(ref t) = tag {
        if !t.is_empty() {
            conditions.push(format!(
                "i.id IN (SELECT inspiration_id FROM inspiration_tags WHERE tag = ?{})",
                params_vec.len() + 1
            ));
            params_vec.push(Box::new(t.clone()));
        }
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    sql.push_str(" GROUP BY i.id ORDER BY i.updated_at DESC");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();
    let rows: Vec<Inspiration> = stmt
        .query_map(param_refs.as_slice(), row_to_inspiration)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Get a single inspiration by ID
pub fn get_inspiration(conn: &Connection, id: String) -> Result<Inspiration, AppError> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.icon, i.title, i.content, i.source_agent, i.is_favorite, i.created_at, i.updated_at, i.market_id,          COALESCE(json_group_array(it.tag) FILTER (WHERE it.tag IS NOT NULL), '[]') as tags          FROM inspirations i          LEFT JOIN inspiration_tags it ON i.id = it.inspiration_id          WHERE i.id = ?1          GROUP BY i.id"
    )?;
    let mut rows = stmt.query_map(params![id], row_to_inspiration)?;
    rows.next()
        .ok_or(AppError::NotFound("灵感不存在".to_string()))?
        .map_err(|e| AppError::Db(e.to_string()))
}

/// Create a new inspiration
pub fn create_inspiration(
    conn: &Connection,
    payload: CreateInspirationPayload,
) -> Result<Inspiration, AppError> {
    let id = crate::utils::new_id();
    let now = crate::utils::now();
    let icon = payload.icon.unwrap_or_else(|| "💡".to_string());
    let source_agent = payload.source_agent.unwrap_or_else(|| "manual".to_string());

    conn.execute(
        "INSERT INTO inspirations (id, icon, title, content, source_agent, is_favorite, created_at, updated_at, market_id) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6, ?7)",
        params![id, icon, payload.title, payload.content, source_agent, now, payload.market_id],
    )?;

    // Insert tags
    if let Some(tags) = payload.tags {
        let mut tag_stmt = conn.prepare(
            "INSERT OR IGNORE INTO inspiration_tags (inspiration_id, tag) VALUES (?1, ?2)",
        )?;
        for tag in tags {
            tag_stmt.execute(params![id, tag])?;
        }
    }

    // 云同步：新灵感视为本地已改动，下轮会作为上传候选推送到云端（best-effort）
    crate::commands::cloud_sync::mark_inspiration_dirty(conn, &id);

    get_inspiration(conn, id)
}

/// Update an existing inspiration
pub fn update_inspiration(
    conn: &Connection,
    payload: UpdateInspirationPayload,
) -> Result<Inspiration, AppError> {
    let now = crate::utils::now();

    // Check existence
    let existing = get_inspiration(conn, payload.id.clone())?;

    // Update fields
    let icon = payload.icon.unwrap_or(existing.icon);
    let title = payload.title.unwrap_or(existing.title);
    let content = payload.content.unwrap_or(existing.content);
    let source_agent = payload.source_agent.unwrap_or(existing.source_agent);
    let is_favorite = payload.is_favorite.unwrap_or(existing.is_favorite) as i32;

    conn.execute(
        "UPDATE inspirations SET icon = ?1, title = ?2, content = ?3, source_agent = ?4, is_favorite = ?5, updated_at = ?6 WHERE id = ?7",
        params![icon, title, content, source_agent, is_favorite, now, payload.id],
    )?;

    // Update tags if provided
    if let Some(tags) = payload.tags {
        // Delete existing tags
        conn.execute(
            "DELETE FROM inspiration_tags WHERE inspiration_id = ?1",
            params![payload.id],
        )?;
        // Insert new tags
        let mut tag_stmt = conn.prepare(
            "INSERT OR IGNORE INTO inspiration_tags (inspiration_id, tag) VALUES (?1, ?2)",
        )?;
        for tag in tags {
            tag_stmt.execute(params![payload.id, tag])?;
        }
    }

    // 云同步：内容 / 标签变更视为本地已改动，下轮会作为上传候选推送（best-effort）
    crate::commands::cloud_sync::mark_inspiration_dirty(conn, &payload.id);

    get_inspiration(conn, payload.id)
}

/// Delete an inspiration
pub fn delete_inspiration(conn: &Connection, id: String) -> Result<(), AppError> {
    // 显式删标签：外键级联依赖 PRAGMA foreign_keys=ON，显式删在两种配置下都稳。
    conn.execute(
        "DELETE FROM inspiration_tags WHERE inspiration_id = ?1",
        params![id],
    )?;
    let rows = conn.execute("DELETE FROM inspirations WHERE id = ?1", params![id])?;
    if rows == 0 {
        return Err(AppError::NotFound("灵感不存在".to_string()));
    }
    // 云同步：删除即硬删（灵感无回收站）→ 直接写墓碑，靠墓碑补送通道把删除意图送达云端（best-effort，
    // 不影响删除本身）。
    crate::commands::cloud_sync::mark_inspiration_deleted(conn, &id);
    Ok(())
}

/// 转义 LIKE 通配符（`%` / `_`）与转义符自身，配合 SQL 里的 `ESCAPE '\'` 使用。
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Search inspirations by title / content / tag.
///
/// 走 LIKE 子串匹配，**不用 FTS5**：FTS5 默认的 unicode61 分词器会把一整串汉字当成
/// 一个 token，中文等于搜不到（知识库为此专门换成 trigram 分词器）；而且 `MATCH` 遇到
/// `-` / 引号等字符会直接语法报错。灵感库是个人级小数据集，全表 LIKE 足够且没有这些坑，
/// 也不需要 tokenizer 迁移。多个词按空白/常见分隔符切分，词间 OR（任一命中即返回），
/// 与知识库搜索同一口径。
pub fn search_inspirations(
    conn: &Connection,
    query: String,
    limit: u32,
) -> Result<Vec<Inspiration>, AppError> {
    let limit = limit.max(1).min(100);
    let terms: Vec<String> = query
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '、' | ';' | '；' | '|'))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(escape_like)
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    // 每个词匹配「标题 / 正文 / 标签」任一字段；词间 OR。列名用 ?1、?2… 显式编号，
    // 因为这里要把同一批词重复绑定 3 次（三个字段各一次），位置参数容易数错。
    let mut idx = 0usize;
    let mut clauses: Vec<String> = Vec::new();
    for _ in &terms {
        let (a, b, c) = (idx + 1, idx + 2, idx + 3);
        idx += 3;
        clauses.push(format!(
            "(i.title LIKE ?{a} ESCAPE '\\' OR i.content LIKE ?{b} ESCAPE '\\' \
             OR EXISTS (SELECT 1 FROM inspiration_tags it2 WHERE it2.inspiration_id = i.id AND it2.tag LIKE ?{c} ESCAPE '\\'))"
        ));
    }
    let sql = format!(
        "SELECT i.id, i.icon, i.title, i.content, i.source_agent, i.is_favorite, i.created_at, i.updated_at, i.market_id, \
         COALESCE(json_group_array(it.tag) FILTER (WHERE it.tag IS NOT NULL), '[]') as tags \
         FROM inspirations i \
         LEFT JOIN inspiration_tags it ON i.id = it.inspiration_id \
         WHERE {} \
         GROUP BY i.id \
         ORDER BY i.updated_at DESC \
         LIMIT ?{}",
        clauses.join(" OR "),
        idx + 1
    );
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    for t in &terms {
        let pattern = format!("%{}%", t);
        for _ in 0..3 {
            params_vec.push(Box::new(pattern.clone()));
        }
    }
    params_vec.push(Box::new(limit as i64));
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Inspiration> = stmt
        .query_map(param_refs.as_slice(), row_to_inspiration)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Get all unique tags
pub fn list_tags(conn: &Connection) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare("SELECT DISTINCT tag FROM inspiration_tags ORDER BY tag")?;
    let tags: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tags)
}

// ── 灵感导出（投稿 JSON / 全量备份）──────────────────────────────────────
//
// 目标：让用户在应用里编辑好的内容一键导出为**符合灵感市场格式**的 JSON（投稿用），
// 或全量打包成 zip 备份。市场单条格式（见 `server/market/inspirations/<id>/<id>.json`
// 与索引生成器 `server/scripts/generate-inspiration-index.mjs`）：
//   { "id": "<slug>", "title": "...", "icon": "📝", "tags": ["..."], "content": "..." }
// 其中 `id/title/content` 必填，`icon/tags` 可选。本模块**只写这几个字段**，
// **不写** `source_agent / is_favorite / 时间戳 / market_id` —— 那些是本地的组织与同步元数据，
// 不属于市场条目。id 规则必须与索引生成器同源，不得漂移。

/// 市场 id 长度上限：与 `generate-inspiration-index.mjs` 的 `ID_MAX_LEN` 保持一致
const MARKET_ID_MAX_LEN: usize = 64;

/// 校验市场 id 是否合规。
///
/// 规则与索引生成器**同源**（不得漂移）：`^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`、长度 ≤ 64。
/// 首尾都必须是字母/数字（短横线只能出现在中间），避免 `foo-` 这类尾部连字符带来的路径/文案歧义。
/// id 即市场里的目录名与文件名，一旦发布不可更改（见生成器文件头），因此发布前必须卡死。
pub fn validate_market_inspiration_id(id: &str) -> Result<(), String> {
    const HINT: &str = "仅允许小写字母 / 数字 / 短横线（-），且必须以字母或数字开头与结尾";
    if id.is_empty() {
        return Err(format!("市场 id 不能为空：{}", HINT));
    }
    if id.chars().count() > MARKET_ID_MAX_LEN {
        return Err(format!(
            "市场 id「{}」长度 {} 超过上限 {}，请缩短",
            id,
            id.chars().count(),
            MARKET_ID_MAX_LEN
        ));
    }
    let mut chars = id.chars();
    // 已确认非空，`next` 必有值
    let first = chars.next().unwrap_or('\0');
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!("市场 id「{}」不合规：{}", id, HINT));
    }
    if id.ends_with('-') {
        return Err(format!("市场 id「{}」不合规：{}", id, HINT));
    }
    if chars.any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')) {
        return Err(format!("市场 id「{}」不合规：{}", id, HINT));
    }
    Ok(())
}

/// 从标题派生一个**合规的默认市场 id**（供投稿弹窗预填，用户可再改）。
///
/// 步骤：小写化 → 把非 `[a-z0-9]` 折叠为单个 `-` → 去首尾 `-` → 按上限截断。
/// 纯中文/纯符号标题会得到空串 → 回退 `insp-<8 位随机小写十六进制>`。
/// **不做拼音转换**（那会引入新依赖，且转换结果同样不保证唯一）。
fn derive_market_inspiration_id(title: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in title.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_lowercase() || lc.is_ascii_digit() {
            out.push(lc);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            // 非 [a-z0-9] 一律折叠成单个 -（开头的符号直接丢弃，避免首字符是 -）
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.chars().count() > MARKET_ID_MAX_LEN {
        out = out.chars().take(MARKET_ID_MAX_LEN).collect();
        // 截断点可能正好落在 - 上，再收一次尾
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        // uuid 去掉连字符后取前 8 位：天然是十六进制，长度固定，无需额外依赖
        let hex = crate::utils::new_id().replace('-', "");
        out = format!("insp-{}", &hex[..8.min(hex.len())]);
    }
    out
}

/// [Tauri Command] 由标题派生一个合规的默认市场 id（投稿弹窗预填用）。
#[tauri::command]
pub fn suggest_market_inspiration_id(title: String) -> String {
    derive_market_inspiration_id(&title)
}

/// 市场单条灵感 JSON 的字段与顺序（与市场源文件逐字对齐：id → title → icon → tags → content）。
#[derive(serde::Serialize)]
struct MarketInspirationJson<'a> {
    id: &'a str,
    title: &'a str,
    icon: &'a str,
    tags: &'a [String],
    content: &'a str,
}

/// 把本地一条灵感组装成市场格式 JSON 文本（UTF-8、`to_string_pretty`、末尾换行）。
///
/// `market_id` 用调用方确认后的值（单条导出前必须已通过 [`validate_market_inspiration_id`]）。
/// `icon/tags` 一并写出（本地一定有值：icon 默认 💡、tags 可能为空数组），与市场源文件一致。
fn market_json_string(insp: &Inspiration, market_id: &str) -> String {
    let payload = MarketInspirationJson {
        id: market_id,
        title: &insp.title,
        icon: &insp.icon,
        tags: &insp.tags,
        content: &insp.content,
    };
    // 这些字段都是字符串 / 字符串数组，序列化不会失败；兜底为空串而非 panic
    let body = serde_json::to_string_pretty(&payload).unwrap_or_default();
    format!("{}\n", body)
}

/// [Tauri Command] 导出单条灵感为市场格式 JSON（投稿用）。
///
/// 先校验用户确认后的 `market_id`（不合法直接返回错误），再按它组装 JSON 写到 `dest_path`。
#[tauri::command]
pub fn export_inspiration_json(
    state: tauri::State<'_, crate::DbState>,
    id: String,
    market_id: String,
    dest_path: String,
) -> Result<(), String> {
    validate_market_inspiration_id(&market_id)?;
    let dest = dest_path.trim();
    if dest.is_empty() {
        return Err("请选择导出位置".to_string());
    }
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    let insp = get_inspiration(&conn, id).map_err(|e| e.to_string())?;
    std::fs::write(dest, market_json_string(&insp, &market_id))
        .map_err(|e| format!("写入文件失败: {}", e))
}

/// [Tauri Command] 把全部灵感导出为 zip（全量备份），目录布局与市场同构：
/// `inspirations/<id>/<id>.json`，`<id>` 优先取该条的 `market_id`，为空则回退它的 UUID。
/// 返回导出条数。**不生成 index.json**（索引是市场 CI 流水线的产物，避免两套实现漂移）。
#[tauri::command]
pub fn export_inspirations_zip(
    state: tauri::State<'_, crate::DbState>,
    dest_path: String,
) -> Result<usize, String> {
    use std::io::Write;

    let dest = dest_path.trim();
    if dest.is_empty() {
        return Err("请选择导出位置".to_string());
    }
    let conn = state.get_conn().map_err(|e| e.to_string())?;
    let list = list_inspirations(&conn, None, false).map_err(|e| e.to_string())?;

    let file = std::fs::File::create(dest).map_err(|e| format!("创建压缩包失败: {}", e))?;
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut count = 0usize;
    for insp in &list {
        // 有市场 id 就用它（保证投稿包与市场目录一致），否则用本地 UUID 兜底
        let id = match insp.market_id.as_deref() {
            Some(m) if !m.trim().is_empty() => m.trim().to_string(),
            _ => insp.id.clone(),
        };
        let name = format!("inspirations/{id}/{id}.json");
        zip.start_file(name, options)
            .map_err(|e| format!("写入压缩包失败: {}", e))?;
        zip.write_all(market_json_string(insp, &id).as_bytes())
            .map_err(|e| format!("写入压缩包失败: {}", e))?;
        count += 1;
    }
    zip.finish().map_err(|e| format!("收尾压缩包失败: {}", e))?;
    Ok(count)
}

// ── 灵感导入（投稿 JSON / 全量备份 zip）──────────────────────────────────
//
// 与导出互逆：把符合市场格式的单条 JSON 或导出产出的 zip 重新灌回本地库。落库口径与
// 「资源市集 › 灵感」导入**同源**（见 `src/components/market/InspirationMarketTab.tsx`）：
// 以 `market_id`（= 文件里的 `id`）判重，本地已有同 market_id 的那条就覆盖更新，否则新建；
// 新建的 `source_agent` 标记为 `market`，`market_id` 记下导入的市场 id（不发明新的来源标记）。

/// 导入结果统计。`errors` 为被跳过/失败的文件或条目 + 原因（供 UI 展示前几条）。
#[derive(serde::Serialize, Default)]
pub struct InspirationImportResult {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// 导入文件的字段（市场格式：id/title/content 必填，icon/tags 可选）。
#[derive(serde::Deserialize)]
struct MarketInspirationImport {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    content: Option<String>,
}

/// 已通过校验的一条导入条目（id 合法、title/content 非空）。
struct ValidEntry {
    id: String,
    title: String,
    icon: String,
    tags: Vec<String>,
    content: String,
}

/// 单条失败原因分类：
/// - `Parse`：JSON 无法解析（只计入 errors，不占 skipped）；
/// - `Invalid`：可解析但不满足必填/合规要求（计入 skipped + errors）。
enum EntryError {
    Parse(String),
    Invalid(String),
}

/// 解析并校验一份市场格式 JSON（`id/title/content` 必填，`icon/tags` 可选）。
fn parse_and_validate_market_entry(text: &str) -> Result<ValidEntry, EntryError> {
    let raw: MarketInspirationImport =
        serde_json::from_str(text).map_err(|e| EntryError::Parse(format!("解析失败（{}）", e)))?;
    if let Err(reason) = validate_market_inspiration_id(&raw.id) {
        return Err(EntryError::Invalid(format!("id 不合规（{}）", reason)));
    }
    let title = raw.title.unwrap_or_default();
    if title.trim().is_empty() {
        return Err(EntryError::Invalid("title 不能为空".to_string()));
    }
    let content = raw.content.unwrap_or_default();
    if content.trim().is_empty() {
        return Err(EntryError::Invalid("content 不能为空".to_string()));
    }
    // icon 缺省与本地新建同源（💡）；tags 缺省为空数组
    let icon = raw
        .icon
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "💡".to_string());
    Ok(ValidEntry {
        id: raw.id,
        title,
        icon,
        tags: raw.tags.unwrap_or_default(),
        content,
    })
}

/// 同批内按 id 去重：后出现的覆盖先出现的，整体只保留一条（不重复计数）。
fn dedup_entries_by_id(entries: Vec<ValidEntry>) -> Vec<ValidEntry> {
    let mut ordered: Vec<ValidEntry> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for e in entries {
        match index.get(&e.id) {
            Some(&i) => ordered[i] = e,
            None => {
                index.insert(e.id.clone(), ordered.len());
                ordered.push(e);
            }
        }
    }
    ordered
}

/// 在一个事务里落库：按 `market_id` 判重，命中则覆盖更新（仅 title/icon/content/tags，不动创建时间），
/// 否则新建（来源标记 `market`）。返回 `(created, updated)`。
fn apply_import_entries(
    conn: &mut Connection,
    entries: &[ValidEntry],
) -> Result<(usize, usize), AppError> {
    use rusqlite::OptionalExtension;

    let tx = conn.transaction()?;
    let mut created = 0usize;
    let mut updated = 0usize;

    for e in entries {
        let now = crate::utils::now();
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM inspirations WHERE market_id = ?1 LIMIT 1",
                params![e.id],
                |row| row.get(0),
            )
            .optional()?;

        let local_id = match existing {
            Some(local_id) => {
                tx.execute(
                    "UPDATE inspirations SET icon = ?1, title = ?2, content = ?3, updated_at = ?4 WHERE id = ?5",
                    params![e.icon, e.title, e.content, now, local_id],
                )?;
                updated += 1;
                local_id
            }
            None => {
                let local_id = crate::utils::new_id();
                tx.execute(
                    "INSERT INTO inspirations (id, icon, title, content, source_agent, is_favorite, created_at, updated_at, market_id) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6, ?7)",
                    params![local_id, e.icon, e.title, e.content, "market", now, e.id],
                )?;
                created += 1;
                local_id
            }
        };

        // 标签整体替换为导入值（与市场导入/更新语义一致）
        tx.execute(
            "DELETE FROM inspiration_tags WHERE inspiration_id = ?1",
            params![local_id],
        )?;
        {
            let mut tag_stmt = tx.prepare(
                "INSERT OR IGNORE INTO inspiration_tags (inspiration_id, tag) VALUES (?1, ?2)",
            )?;
            for tag in &e.tags {
                tag_stmt.execute(params![local_id, tag])?;
            }
        }

        // 云同步：导入即本地已改动，下轮作为上传候选推送（best-effort）
        crate::commands::cloud_sync::mark_inspiration_dirty(&tx, &local_id);
    }

    tx.commit()?;
    Ok((created, updated))
}

/// 去重 + 落库，并把计数累加回结果（供两个导入命令共用）。
fn finalize_import(
    conn: &mut Connection,
    entries: Vec<ValidEntry>,
    result: &mut InspirationImportResult,
) -> Result<(), String> {
    let deduped = dedup_entries_by_id(entries);
    let (created, updated) = apply_import_entries(conn, &deduped).map_err(|e| e.to_string())?;
    result.created += created;
    result.updated += updated;
    Ok(())
}

/// 从 zip 归档收集导入条目（与 `export_inspirations_zip` 的布局同构：`inspirations/<id>/<id>.json`）。
/// 非 `.json` 或不符合布局的条目忽略并记入 `errors`；归档损坏 → `Err`。
fn read_zip_entries<R: std::io::Read + std::io::Seek>(
    reader: R,
    result: &mut InspirationImportResult,
) -> Result<Vec<ValidEntry>, String> {
    use std::io::Read;

    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("读取压缩包失败：{}", e))?;
    let mut valid: Vec<ValidEntry> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取压缩包条目失败：{}", e))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if !name.ends_with(".json") {
            result.errors.push(format!("{}：忽略非 JSON 文件", name));
            continue;
        }
        // 布局校验：`inspirations/<id>/<id>.json`
        let parts: Vec<&str> = name.split('/').collect();
        let layout_ok = parts.len() == 3
            && parts[0] == "inspirations"
            && !parts[1].is_empty()
            && parts[1] == parts[2].trim_end_matches(".json");
        if !layout_ok {
            result.errors.push(format!(
                "{}：不符合导入布局（应为 inspirations/<id>/<id>.json）",
                name
            ));
            continue;
        }
        let mut text = String::new();
        if let Err(e) = entry.read_to_string(&mut text) {
            result.errors.push(format!("{}：读取失败（{}）", name, e));
            continue;
        }
        match parse_and_validate_market_entry(&text) {
            Ok(parsed) => valid.push(parsed),
            Err(EntryError::Parse(msg)) => result.errors.push(format!("{}：{}", name, msg)),
            Err(EntryError::Invalid(msg)) => {
                result.skipped += 1;
                result.errors.push(format!("{}：{}", name, msg));
            }
        }
    }
    Ok(valid)
}

/// [Tauri Command] 从若干 JSON 文件导入灵感（投稿格式；支持一次选多个文件）。
///
/// 每个文件独立处理：读取/解析失败只记入 `errors`，不影响其它文件；校验失败计入 `skipped`。
/// 全部成功条目的落库在一个事务里完成；单文件失败不回滚已成功的条目。
#[tauri::command]
pub async fn import_inspirations_from_files(
    state: tauri::State<'_, crate::DbState>,
    paths: Vec<String>,
) -> Result<InspirationImportResult, String> {
    let mut result = InspirationImportResult::default();
    let mut valid: Vec<ValidEntry> = Vec::new();

    for path in &paths {
        let label = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| path.clone());
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                result.errors.push(format!("{}：读取失败（{}）", label, e));
                continue;
            }
        };
        match parse_and_validate_market_entry(&text) {
            Ok(entry) => valid.push(entry),
            Err(EntryError::Parse(msg)) => result.errors.push(format!("{}：{}", label, msg)),
            Err(EntryError::Invalid(msg)) => {
                result.skipped += 1;
                result.errors.push(format!("{}：{}", label, msg));
            }
        }
    }

    let mut conn = state.get_conn().map_err(|e| e.to_string())?;
    finalize_import(&mut conn, valid, &mut result)?;
    Ok(result)
}

/// [Tauri Command] 从 zip 导入灵感（布局与 `export_inspirations_zip` 的产出同构）。
///
/// zip 不存在/损坏 → `Err`；归档内不符合布局的条目忽略并记入 `errors`。
#[tauri::command]
pub async fn import_inspirations_zip(
    state: tauri::State<'_, crate::DbState>,
    zip_path: String,
) -> Result<InspirationImportResult, String> {
    let path = zip_path.trim();
    if path.is_empty() {
        return Err("请选择要导入的压缩包".to_string());
    }
    let file = std::fs::File::open(path).map_err(|e| format!("打开压缩包失败：{}", e))?;
    let mut result = InspirationImportResult::default();
    let valid = read_zip_entries(file, &mut result)?;

    let mut conn = state.get_conn().map_err(|e| e.to_string())?;
    finalize_import(&mut conn, valid, &mut result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_inspiration() -> Inspiration {
        Inspiration {
            id: "11111111-1111-4111-8111-111111111111".to_string(),
            icon: "🔍".to_string(),
            title: "代码评审：先给结论再给依据".to_string(),
            content: "第一行\n第二行".to_string(),
            source_agent: "manual".to_string(),
            is_favorite: true,
            tags: vec!["编程".to_string(), "评审".to_string()],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_100,
            market_id: Some("code-review-checklist".to_string()),
        }
    }

    #[test]
    fn validate_accepts_legal_ids() {
        assert!(validate_market_inspiration_id("code-review-checklist").is_ok());
        assert!(validate_market_inspiration_id("a").is_ok());
        assert!(validate_market_inspiration_id("0abc").is_ok());
        assert!(validate_market_inspiration_id("a--b-2").is_ok());
        // 长度恰在上限（64）
        assert!(validate_market_inspiration_id(&"a".repeat(MARKET_ID_MAX_LEN)).is_ok());
    }

    #[test]
    fn validate_rejects_illegal_ids() {
        // 空
        assert!(validate_market_inspiration_id("").is_err());
        // 首字符非法（- 或大写）
        assert!(validate_market_inspiration_id("-abc").is_err());
        assert!(validate_market_inspiration_id("Abc").is_err());
        // 非法字符
        assert!(validate_market_inspiration_id("a_b").is_err());
        assert!(validate_market_inspiration_id("a b").is_err());
        assert!(validate_market_inspiration_id("中文").is_err());
        // 超长（65）
        assert!(validate_market_inspiration_id(&"a".repeat(MARKET_ID_MAX_LEN + 1)).is_err());
        // 错误信息可读：说明允许的字符与长度
        let err = validate_market_inspiration_id("中文").unwrap_err();
        assert!(err.contains("小写字母"), "err = {}", err);
        let err = validate_market_inspiration_id(&"a".repeat(MARKET_ID_MAX_LEN + 1)).unwrap_err();
        assert!(err.contains("超过上限"), "err = {}", err);
    }

    #[test]
    fn suggest_from_english_title() {
        assert_eq!(derive_market_inspiration_id("Hello World"), "hello-world");
        // 连续的非字母数字折叠成单个 -
        assert_eq!(derive_market_inspiration_id("Hello   World!!!"), "hello-world");
        // 去首尾 -（中间的仍旧保留）
        assert_eq!(derive_market_inspiration_id("  ===Hello==="), "hello");
        assert_eq!(derive_market_inspiration_id("  Hello---World  "), "hello-world");
    }

    #[test]
    fn suggest_from_mixed_title_strips_non_latin() {
        assert_eq!(derive_market_inspiration_id("中文Hello啊World"), "hello-world");
    }

    #[test]
    fn suggest_truncates_to_max_len() {
        let id = derive_market_inspiration_id(&"a".repeat(100));
        assert_eq!(id.chars().count(), MARKET_ID_MAX_LEN);
        assert!(validate_market_inspiration_id(&id).is_ok());
    }

    #[test]
    fn suggest_falls_back_to_random_for_non_latin_or_symbol() {
        for title in ["全中文标题", "!!!???", "   ", ""] {
            let id = derive_market_inspiration_id(title);
            assert!(id.starts_with("insp-"), "title={} id={}", title, id);
            // insp- + 8 位十六进制 = 13
            assert_eq!(id.chars().count(), 13);
            assert!(validate_market_inspiration_id(&id).is_ok(), "id={}", id);
        }
    }

    #[test]
    fn suggest_always_yields_valid_id() {
        for title in ["Hello World", "中文Hello啊World", "a-1_2.3", &"x".repeat(200)] {
            let id = derive_market_inspiration_id(title);
            assert!(validate_market_inspiration_id(&id).is_ok(), "title={} id={}", title, id);
        }
    }

    #[test]
    fn market_json_matches_format_and_drops_local_fields() {
        let insp = sample_inspiration();
        let text = market_json_string(&insp, "code-review-checklist");
        assert!(text.ends_with("\n"), "末尾应有换行");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        // 字段内容等价（顺序无关）
        assert_eq!(
            v,
            serde_json::json!({
                "id": "code-review-checklist",
                "title": "代码评审：先给结论再给依据",
                "icon": "🔍",
                "tags": ["编程", "评审"],
                "content": "第一行\n第二行",
            })
        );
        // id 用传入的 market_id，而非本地 UUID
        assert_eq!(v["id"], "code-review-checklist");
        // 只有 5 个字段，且不含任何本地元数据
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 5);
        for k in ["source_agent", "is_favorite", "created_at", "updated_at", "market_id"] {
            assert!(!obj.contains_key(k), "不应写入本地字段 {}", k);
        }
    }

    #[test]
    fn market_json_keeps_empty_tags_as_array() {
        let mut insp = sample_inspiration();
        insp.tags = vec![];
        let v: serde_json::Value =
            serde_json::from_str(&market_json_string(&insp, "no-tag")).unwrap();
        assert_eq!(v["tags"], serde_json::json!([]));
    }

    // ── 导入单测 ──────────────────────────────────────────────────────────

    /// 内存库 + 终态 schema（导入用例只依赖 inspirations / inspiration_tags 两张表）。
    fn import_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::init::FINAL_SCHEMA_SQL).unwrap();
        conn
    }

    fn valid_entry(id: &str, title: &str, content: &str, tags: &[&str]) -> ValidEntry {
        ValidEntry {
            id: id.to_string(),
            title: title.to_string(),
            icon: "📝".to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            content: content.to_string(),
        }
    }

    fn market_json(id: &str, title: &str, content: &str, tags: &[&str]) -> String {
        serde_json::json!({
            "id": id,
            "title": title,
            "icon": "📝",
            "tags": tags,
            "content": content,
        })
        .to_string()
    }

    /// 取某 market_id 对应的标签（按 tag 排序，便于断言）。
    fn tags_of(conn: &Connection, market_id: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT it.tag FROM inspiration_tags it \
                 JOIN inspirations i ON i.id = it.inspiration_id \
                 WHERE i.market_id = ?1 ORDER BY it.tag",
            )
            .unwrap();
        stmt.query_map(params![market_id], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    #[test]
    fn import_creates_new_marked_as_market() {
        let mut conn = import_conn();
        let mut result = InspirationImportResult::default();
        finalize_import(
            &mut conn,
            vec![valid_entry("new-tip", "标题", "正文", &["b", "a"])],
            &mut result,
        )
        .unwrap();

        assert_eq!((result.created, result.updated, result.skipped), (1, 0, 0));
        assert!(result.errors.is_empty());
        let (market_id, source, is_fav): (Option<String>, String, i64) = conn
            .query_row(
                "SELECT market_id, source_agent, is_favorite FROM inspirations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(market_id.as_deref(), Some("new-tip"));
        // 来源标记与市场导入一致（InspirationMarketTab: sourceAgent: 'market'）
        assert_eq!(source, "market");
        assert_eq!(is_fav, 0);
        assert_eq!(tags_of(&conn, "new-tip"), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn import_same_market_id_updates_in_place_and_replaces_tags() {
        let mut conn = import_conn();
        let mut r1 = InspirationImportResult::default();
        finalize_import(
            &mut conn,
            vec![valid_entry("tip", "旧标题", "旧正文", &["旧"])],
            &mut r1,
        )
        .unwrap();
        assert_eq!((r1.created, r1.updated), (1, 0));
        let (first_local_id, created_at): (String, i64) = conn
            .query_row("SELECT id, created_at FROM inspirations", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();

        let mut r2 = InspirationImportResult::default();
        finalize_import(
            &mut conn,
            vec![valid_entry("tip", "新标题", "新正文", &["新1", "新2"])],
            &mut r2,
        )
        .unwrap();
        assert_eq!((r2.created, r2.updated), (0, 1));

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM inspirations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "同 market_id 不应产生重复行");

        let (local_id, title, content, cat): (String, String, String, i64) = conn
            .query_row(
                "SELECT id, title, content, created_at FROM inspirations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(local_id, first_local_id, "应更新同一条本地行");
        assert_eq!(title, "新标题");
        assert_eq!(content, "新正文");
        assert_eq!(cat, created_at, "创建时间不应被改动");
        assert_eq!(tags_of(&conn, "tip"), vec!["新1".to_string(), "新2".to_string()]);
    }

    #[test]
    fn invalid_id_is_rejected_with_reason() {
        match parse_and_validate_market_entry(&market_json("Bad_ID", "t", "c", &[])) {
            Err(EntryError::Invalid(msg)) => {
                assert!(msg.contains("id 不合规"), "msg = {}", msg)
            }
            Err(EntryError::Parse(m)) => panic!("应为校验失败而非解析失败：{}", m),
            Ok(_) => panic!("非法 id 不应通过校验"),
        }
    }

    #[test]
    fn empty_title_or_content_is_rejected() {
        match parse_and_validate_market_entry(&market_json("ok-id", "  ", "c", &[])) {
            Err(EntryError::Invalid(m)) => assert!(m.contains("title"), "m = {}", m),
            _ => panic!("空 title 应被拒绝"),
        }
        match parse_and_validate_market_entry(&market_json("ok-id", "t", "", &[])) {
            Err(EntryError::Invalid(m)) => assert!(m.contains("content"), "m = {}", m),
            _ => panic!("空 content 应被拒绝"),
        }
        // 缺字段（title/content 缺失）同样按空处理被拒
        let missing: String = serde_json::json!({ "id": "ok-id" }).to_string();
        match parse_and_validate_market_entry(&missing) {
            Err(EntryError::Invalid(_)) => {}
            _ => panic!("缺失 title/content 应被拒绝"),
        }
    }

    #[test]
    fn duplicate_ids_in_one_batch_count_once_and_last_wins() {
        let deduped = dedup_entries_by_id(vec![
            valid_entry("dup", "第一次", "c1", &["a"]),
            valid_entry("dup", "第二次", "c2", &["b"]),
        ]);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].title, "第二次");

        let mut conn = import_conn();
        let mut result = InspirationImportResult::default();
        finalize_import(
            &mut conn,
            vec![
                valid_entry("dup", "第一次", "c1", &["a"]),
                valid_entry("dup", "第二次", "c2", &["b"]),
            ],
            &mut result,
        )
        .unwrap();
        assert_eq!(
            (result.created, result.updated),
            (1, 0),
            "同批重复 id 只应计一次"
        );
        let (count, title): (i64, String) = conn
            .query_row("SELECT COUNT(*), MAX(title) FROM inspirations", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(title, "第二次");
    }

    #[test]
    fn zip_import_reads_layout_and_tolerates_extras() {
        use std::io::Write;

        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("inspirations/first-tip/first-tip.json", opts)
                .unwrap();
            zip.write_all(market_json("first-tip", "标题一", "正文一", &["x"]).as_bytes())
                .unwrap();
            zip.start_file("inspirations/second-tip/second-tip.json", opts)
                .unwrap();
            zip.write_all(market_json("second-tip", "标题二", "正文二", &[]).as_bytes())
                .unwrap();
            // 混入：非 json 文件
            zip.start_file("README.md", opts).unwrap();
            zip.write_all(b"hello").unwrap();
            // 混入：布局不符（文件名与目录名不一致）
            zip.start_file("inspirations/broken/other-name.json", opts)
                .unwrap();
            zip.write_all(market_json("broken", "t", "c", &[]).as_bytes())
                .unwrap();
            // 混入：布局对但 id 非法 → 计入 skipped
            zip.start_file("inspirations/xyz/xyz.json", opts).unwrap();
            zip.write_all(market_json("Bad_ID", "t", "c", &[]).as_bytes())
                .unwrap();
            zip.finish().unwrap();
        }

        let mut result = InspirationImportResult::default();
        let valid = read_zip_entries(buf, &mut result).unwrap();
        assert_eq!(valid.len(), 2, "仅两条合布局且合规的条目进入待导入");
        assert_eq!(result.skipped, 1, "非法 id 条目计入 skipped");
        assert_eq!(result.errors.len(), 3, "非 json / 布局不符 / id 非法各一条");

        let mut conn = import_conn();
        finalize_import(&mut conn, valid, &mut result).unwrap();
        assert_eq!((result.created, result.updated, result.skipped), (2, 0, 1));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM inspirations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(tags_of(&conn, "first-tip"), vec!["x".to_string()]);
    }

    #[test]
    fn corrupted_zip_returns_error() {
        let mut result = InspirationImportResult::default();
        let err = read_zip_entries(std::io::Cursor::new(b"definitely not a zip".to_vec()), &mut result);
        assert!(err.is_err(), "损坏的压缩包应返回 Err");
    }

    // ── 搜索单测 ──────────────────────────────────────────────────────────

    /// 直接插一行灵感（+ 标签），供搜索用例使用。
    fn insert_inspiration(conn: &Connection, id: &str, title: &str, content: &str, tags: &[&str]) {
        conn.execute(
            "INSERT INTO inspirations (id, icon, title, content, source_agent, is_favorite, created_at, updated_at, market_id)
             VALUES (?1, '💡', ?2, ?3, 'manual', 0, 1, 1, NULL)",
            rusqlite::params![id, title, content],
        )
        .unwrap();
        for t in tags {
            conn.execute(
                "INSERT OR IGNORE INTO inspiration_tags (inspiration_id, tag) VALUES (?1, ?2)",
                rusqlite::params![id, t],
            )
            .unwrap();
        }
    }

    #[test]
    fn search_matches_chinese_substring_in_title_content_and_tag() {
        let conn = import_conn();
        insert_inspiration(&conn, "a", "写作助手：先给结论", "把要点放在最前面", &["写作"]);
        insert_inspiration(&conn, "b", "代码评审清单", "先给结论再给依据", &["编程"]);

        // 中文两字子串（FTS5 默认 unicode61 分词器会把整串汉字当一个 token，这种查询必定搜不到）
        let hit = search_inspirations(&conn, "结论".to_string(), 50).unwrap();
        assert_eq!(hit.len(), 2, "标题/正文中的「结论」都应命中");

        // 标签命中
        let by_tag = search_inspirations(&conn, "写作".to_string(), 50).unwrap();
        assert_eq!(by_tag.len(), 1);
        assert_eq!(by_tag[0].id, "a");

        // 多个词：词间 OR（任一命中即返回）
        let any = search_inspirations(&conn, "写作 评审".to_string(), 50).unwrap();
        assert_eq!(any.len(), 2);

        // 无命中
        assert!(search_inspirations(&conn, "查无此词".to_string(), 50)
            .unwrap()
            .is_empty());

        // 通配符被转义：`%` 只当普通字符，不能退化成"匹配全部"
        assert!(search_inspirations(&conn, "%".to_string(), 50)
            .unwrap()
            .is_empty());
    }
}
