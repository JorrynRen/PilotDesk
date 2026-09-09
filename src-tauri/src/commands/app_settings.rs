use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use crate::utils::errors::AppError;

/// Get a setting value by key
pub fn get_setting(conn: &rusqlite::Connection, key: &str) -> Result<Option<String>, AppError> {
    let value: Option<String> = conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?",
        params![key],
        |row| row.get("value"),
    ).optional()?;
    Ok(value)
}

/// Set a setting value (insert or update)
pub fn set_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = ?3",
        params![key, value, now],
    )?;
    Ok(())
}

/// Delete a setting by key
#[allow(dead_code)]
pub fn delete_setting(conn: &rusqlite::Connection, key: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM app_settings WHERE key = ?", params![key])?;
    Ok(())
}

/// 流式 chunk 空闲超时配置键（秒）。会话与群聊共用该值；0 = 禁用空闲检测（仅调试用）。
pub const STREAM_IDLE_SECS_KEY: &str = "api_stream_idle_secs";
const STREAM_IDLE_SECS_DEFAULT: u64 = 90;

/// 读取流式 chunk 空闲超时（秒）：合法值 0（禁用）或 10..=600；缺省/非法回退 90。
pub fn load_stream_idle_secs(conn: &rusqlite::Connection) -> u64 {
    match get_setting(conn, STREAM_IDLE_SECS_KEY) {
        Ok(Some(v)) => v
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|s| *s == 0 || (10..=600).contains(s))
            .unwrap_or(STREAM_IDLE_SECS_DEFAULT),
        _ => STREAM_IDLE_SECS_DEFAULT,
    }
}

/// 「按作用域禁用技能」配置键（值为此文件的 `SkillScopeDisabled` JSON）。
///
/// 语义（P2-2b 阶段一）：该禁用集只决定技能目录**是否向模型披露**
/// （是否注入 `<available_skills>`），`load_skill` 仍全局可点名加载被隐藏技能——
/// 隐藏 ≠ 禁用，因此不需要改动任何工具注册表。会话与群聊两个作用域独立。
pub const SKILL_SCOPE_DISABLED_KEY: &str = "skill_scope_disabled";

/// 技能按作用域禁用集：`session` = 单 Agent 会话注入面，`groupchat` = 群聊注入面。
/// 字段为技能名列表；JSON 键与字段同名（小写），供后续前端管理 UI 直接读写。
#[derive(Default, Serialize, Deserialize, Clone, Debug)]
pub struct SkillScopeDisabled {
    pub session: Vec<String>,
    pub groupchat: Vec<String>,
}

/// 读取按作用域禁用集：键缺失或 JSON 解析失败（含字段类型不符）时返回空默认；
/// 对合法 JSON 做规范化——技能名 trim 后剔除空串（空名无匹配意义）。
pub fn load_skill_scope_disabled(conn: &rusqlite::Connection) -> SkillScopeDisabled {
    let mut v: SkillScopeDisabled = match get_setting(conn, SKILL_SCOPE_DISABLED_KEY) {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
        _ => SkillScopeDisabled::default(),
    };
    for list in [&mut v.session, &mut v.groupchat] {
        for name in list.iter_mut() {
            *name = name.trim().to_string();
        }
        list.retain(|n| !n.is_empty());
    }
    v
}

/// 持久化按作用域禁用集（整量覆盖写）。供后续前端管理 UI 的 Tauri 命令包装调用。
#[allow(dead_code)]
pub fn save_skill_scope_disabled(conn: &rusqlite::Connection, v: &SkillScopeDisabled) -> Result<(), AppError> {
    let json = serde_json::to_string(v)?;
    set_setting(conn, SKILL_SCOPE_DISABLED_KEY, &json)
}

// ── 设置「两平面」地基：代码默认平面 vs 用户显式设置平面 ──
// 目标：读设置能区分"当前值来自代码默认"还是"用户显式改过"，供设置页展示 默认/自定义。
// 不新增表/版本号：判定只需「代码默认注册表 + 与 DB 当前值比较」。存量种子行若等于默认，
// 会被判为"默认"，无需迁移；用户真正改过（值 != 默认）即视为显式自定义。
const SETTING_DEFAULTS: &[(&str, &str)] = &[
    ("mode_prompt_native", ""),
    (
        "mode_prompt_fast",
        "快速简洁回答，直接给出结论，无需详细解释推理过程",
    ),
    (
        "mode_prompt_think",
        "逐步分析推理，详细解释你的思路和过程，给出完整的推理链",
    ),
    (
        "mode_prompt_expert",
        "以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案",
    ),
    (
        "mode_prompt_plan",
        "先分析需求并制定清晰的分步执行计划，先向用户呈现完整计划等待确认，未经确认不要执行任何操作。",
    ),
    ("workflow_max_concurrency", "10"),
    (STREAM_IDLE_SECS_KEY, "90"),
];

/// 返回某设置的代码默认值（未登记键返回 None）。
#[allow(dead_code)] // 供前端命令/设置页使用；测试覆盖。
pub fn setting_default(key: &str) -> Option<&'static str> {
    SETTING_DEFAULTS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
}

/// 读取"生效值"：DB 中已存则取 DB（用户当前状态），否则回退代码默认（未登记默认回退空串）。
/// 兼容既有 `get_setting` 语义，新增"有默认兜底"读取面。
#[allow(dead_code)] // 同上，供前端命令/设置页使用。
pub fn effective_setting(conn: &rusqlite::Connection, key: &str) -> String {
    match get_setting(conn, key) {
        Ok(Some(v)) => v,
        _ => setting_default(key).unwrap_or("").to_string(),
    }
}

/// 判断某设置当前是否为"用户显式自定义"：DB 有值且与代码默认不同。
/// DB 无值或等于默认 → 视为默认（未显式改过或改回默认）。
#[allow(dead_code)] // 同上，供前端命令/设置页使用。
pub fn is_custom_setting(conn: &rusqlite::Connection, key: &str) -> bool {
    match get_setting(conn, key) {
        Ok(Some(v)) => match setting_default(key) {
            Some(d) => v != d,
            None => true, // 未登记默认的键一旦落库即视为自定义
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小内存 app_settings 表（与 db/init.rs 中 FINAL_SCHEMA_SQL 的定义一致）。
    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL)",
        )
        .unwrap();
        conn
    }

    /// 键缺失 / 坏 JSON / 字段类型不符：一律回退空默认，不 panic。
    #[test]
    fn skill_scope_disabled_default_when_missing_or_bad_json() {
        let conn = mem_conn();
        // 键缺失 → 空默认。
        let v = load_skill_scope_disabled(&conn);
        assert!(v.session.is_empty() && v.groupchat.is_empty());
        // 坏 JSON → 空默认。
        set_setting(&conn, SKILL_SCOPE_DISABLED_KEY, "not-json{").unwrap();
        let v = load_skill_scope_disabled(&conn);
        assert!(v.session.is_empty() && v.groupchat.is_empty());
        // 结构非法（字段类型不符）→ 空默认。
        set_setting(&conn, SKILL_SCOPE_DISABLED_KEY, r#"{"session":123}"#).unwrap();
        let v = load_skill_scope_disabled(&conn);
        assert!(v.session.is_empty() && v.groupchat.is_empty());
    }

    /// 保存/读取往返：load 侧规范化（trim + 剔空项），session 与 groupchat 互不干扰。
    #[test]
    fn skill_scope_disabled_roundtrip_and_sanitize() {
        let conn = mem_conn();
        let v = SkillScopeDisabled {
            session: vec!["web_search".into(), "  ".into(), " read_file ".into()],
            groupchat: vec!["write_file".into(), "".into(), "edit_file".into()],
        };
        save_skill_scope_disabled(&conn, &v).unwrap();
        let loaded = load_skill_scope_disabled(&conn);
        assert_eq!(loaded.session, vec!["web_search", "read_file"]);
        assert_eq!(loaded.groupchat, vec!["write_file", "edit_file"]);
        // 删除键后再次读取 → 回退空默认。
        delete_setting(&conn, SKILL_SCOPE_DISABLED_KEY).unwrap();
        let loaded = load_skill_scope_disabled(&conn);
        assert!(loaded.session.is_empty() && loaded.groupchat.is_empty());
    }

    /// 两平面判定：DB 无值/等于默认 → 默认；用户改值 → 自定义；未登记默认的键落库即自定义。
    #[test]
    fn two_plane_default_vs_custom() {
        let conn = mem_conn();
        assert_eq!(
            effective_setting(&conn, "mode_prompt_fast"),
            "快速简洁回答，直接给出结论，无需详细解释推理过程"
        );
        assert!(!is_custom_setting(&conn, "mode_prompt_fast"));

        set_setting(&conn, "mode_prompt_fast", "自定义快速回答").unwrap();
        assert_eq!(effective_setting(&conn, "mode_prompt_fast"), "自定义快速回答");
        assert!(is_custom_setting(&conn, "mode_prompt_fast"));

        // 改回与默认一致 → 视为默认。
        set_setting(&conn, "mode_prompt_fast", "快速简洁回答，直接给出结论，无需详细解释推理过程")
            .unwrap();
        assert!(!is_custom_setting(&conn, "mode_prompt_fast"));

        // 未登记默认的键：落库即自定义；从未设置的键回退空串。
        set_setting(&conn, "some_unknown_key", "1").unwrap();
        assert!(is_custom_setting(&conn, "some_unknown_key"));
        assert_eq!(effective_setting(&conn, "never_set_key"), "");
    }
}
