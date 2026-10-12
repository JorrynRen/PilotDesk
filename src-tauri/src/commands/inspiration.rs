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

    // Trigger FTS5 reindex
    conn.execute(
        "INSERT INTO inspirations_fts(inspirations_fts) VALUES('rebuild')",
        [],
    )?;

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

    // Reindex FTS5
    conn.execute(
        "INSERT INTO inspirations_fts(inspirations_fts) VALUES('rebuild')",
        [],
    )?;

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

/// Full-text search using FTS5
pub fn search_inspirations(
    conn: &Connection,
    query: String,
    limit: u32,
) -> Result<Vec<Inspiration>, AppError> {
    let limit = limit.max(1).min(100);
    let sql = format!(
        "SELECT i.id, i.icon, i.title, i.content, i.source_agent, i.is_favorite, i.created_at, i.updated_at, i.market_id, \
         COALESCE(json_group_array(it.tag) FILTER (WHERE it.tag IS NOT NULL), '[]') as tags \
         FROM inspirations i \
         JOIN inspirations_fts fts ON i.rowid = fts.rowid \
         LEFT JOIN inspiration_tags it ON i.id = it.inspiration_id \
         WHERE inspirations_fts MATCH ?1 \
         GROUP BY i.id \
         ORDER BY rank \
         LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Inspiration> = stmt
        .query_map(params![query, limit], row_to_inspiration)?
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
}
