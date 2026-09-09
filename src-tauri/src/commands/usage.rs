//! 全局用量统计（`api_usage_log` → 总量 / 按 Provider / 按 Model / 按天趋势）。
//!
//! 四桶互斥口径：`prompt_tokens`=未命中输入、`cache_read_tokens`=缓存命中读取、
//! `cache_write_tokens`=缓存写入；`cached_tokens` 恒等于 read+write（兼容旧展示）。
//! 缓存命中率 = cache_read / (prompt + cache_read + cache_write)（分母 0 → 0）。

use serde::Serialize;

use rusqlite::{params, Connection};

use crate::utils::errors::AppError;

/// 一组用量合计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub call_count: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    /// 总缓存 = cache_read_tokens + cache_write_tokens（兼容旧展示口径）。
    pub cached_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// 0-100 的缓存命中率（read / (prompt + read + write)）。
    pub cache_hit_rate: f64,
}

/// 缓存命中率：read / (prompt + read + write)；分母 0 → 0。
fn cache_hit_rate(cache_read: i64, cache_write: i64, prompt: i64) -> f64 {
    let denom = prompt + cache_read + cache_write;
    if denom > 0 {
        cache_read as f64 / denom as f64 * 100.0
    } else {
        0.0
    }
}

impl UsageTotals {
    fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Self> {
        Self::from_row_at(r, 0)
    }

    /// base=0：SELECT 以 COUNT 开头；分组查询在 0 位是 name 时传 base=1。
    /// 数值列顺序固定：count, prompt, completion, total, cached, cache_read, cache_write。
    fn from_row_at(r: &rusqlite::Row, base: usize) -> rusqlite::Result<Self> {
        let prompt_tokens: i64 = r.get(base + 1)?;
        let cache_read_tokens: i64 = r.get(base + 5)?;
        let cache_write_tokens: i64 = r.get(base + 6)?;
        let rate = cache_hit_rate(cache_read_tokens, cache_write_tokens, prompt_tokens);
        Ok(Self {
            call_count: r.get(base)?,
            prompt_tokens,
            completion_tokens: r.get(base + 2)?,
            total_tokens: r.get(base + 3)?,
            cached_tokens: r.get(base + 4)?,
            cache_read_tokens,
            cache_write_tokens,
            cache_hit_rate: rate,
        })
    }
}

/// 按某维度（provider/model）分组的合计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroup {
    pub name: String,
    pub totals: UsageTotals,
}

/// 按天趋势（近 N 天内逐日）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    pub date: String,
    pub prompt_tokens: i64,
    pub cached_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_hit_rate: f64,
}

/// 全局用量汇总。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub totals: UsageTotals,
    pub by_provider: Vec<UsageGroup>,
    pub by_model: Vec<UsageGroup>,
    pub trend: Vec<UsageDay>,
}

/// 群聊房间用量汇总：房间级（director/纯补全）+ 各参与者（groupchat:{room}:{participant}）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomUsage {
    pub totals: UsageTotals,
    pub by_model: Vec<UsageGroup>,
    pub by_provider: Vec<UsageGroup>,
}

/// 房间内按某维度分组（col ∈ {provider, model}，仅本文件调用，字面量可信）。
fn room_group_by(
    conn: &Connection,
    room_id: &str,
    col: &str,
) -> Result<Vec<UsageGroup>, AppError> {
    let display = if col == "provider" {
        provider_display_names(conn)
    } else {
        std::collections::HashMap::new()
    };
    let sql = format!(
        "SELECT CASE WHEN TRIM(COALESCE({0}, '')) = '' THEN '未知' ELSE {0} END AS name,
                COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
         FROM api_usage_log
         WHERE session_id = ?1 OR session_id LIKE ?2
         GROUP BY name ORDER BY name",
        col
    );
    let exact = format!("groupchat:{}", room_id);
    let prefix = format!("groupchat:{}:%", room_id);
    let mut stmt = conn.prepare(&sql)?;
    let it = stmt.query_map(params![exact, prefix], |r| {
        Ok((r.get::<_, String>(0)?, UsageTotals::from_row_at(r, 1)?))
    })?;
    let mut rows = Vec::new();
    for row in it {
        let (name, totals) = row?;
        let name = display.get(&name).cloned().unwrap_or(name);
        rows.push(UsageGroup { name, totals });
    }
    Ok(rows)
}

/// 群聊房间用量合计（含多参与者/多模型；room 无记录时返回全 0）。
pub fn room_usage(conn: &Connection, room_id: &str) -> Result<RoomUsage, AppError> {
    let sql = "SELECT COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                      COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
               FROM api_usage_log
               WHERE session_id = ?1 OR session_id LIKE ?2";
    let exact = format!("groupchat:{}", room_id);
    let prefix = format!("groupchat:{}:%", room_id);
    let mut stmt = conn.prepare(sql)?;
    let totals = stmt.query_row(params![exact, prefix], |r| UsageTotals::from_row(r))?;
    Ok(RoomUsage {
        totals,
        by_model: room_group_by(conn, room_id, "model")?,
        by_provider: room_group_by(conn, room_id, "provider")?,
    })
}

/// provider 显示名映射：api_providers.id → api_providers.name（表不存在/无行时回落原值）。
fn provider_display_names(conn: &Connection) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'api_providers'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !has_table {
        return map;
    }
    if let Ok(mut stmt) =
        conn.prepare("SELECT id, name FROM api_providers WHERE TRIM(COALESCE(name, '')) != ''")
    {
        if let Ok(it) = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            for row in it.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

/// provider 维度分组汇总（展示名优先取 api_providers.name）。
fn group_provider(conn: &Connection, since: Option<i64>) -> Result<Vec<UsageGroup>, AppError> {
    let display = provider_display_names(conn);
    let mut rows = Vec::new();
    if let Some(s) = since {
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN TRIM(COALESCE(provider, '')) = '' THEN '未知' ELSE provider END AS name,
                    COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
             FROM api_usage_log WHERE created_at >= ?1 GROUP BY name ORDER BY name",
        )?;
        let it = stmt.query_map(params![s], |r| {
            Ok((r.get::<_, String>(0)?, UsageTotals::from_row_at(r, 1)?))
        })?;
        for row in it {
            let (name, totals) = row?;
            let name = display.get(&name).cloned().unwrap_or(name);
            rows.push(UsageGroup { name, totals });
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN TRIM(COALESCE(provider, '')) = '' THEN '未知' ELSE provider END AS name,
                    COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
             FROM api_usage_log GROUP BY name ORDER BY name",
        )?;
        let it = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, UsageTotals::from_row_at(r, 1)?))
        })?;
        for row in it {
            let (name, totals) = row?;
            let name = display.get(&name).cloned().unwrap_or(name);
            rows.push(UsageGroup { name, totals });
        }
    }
    Ok(rows)
}

/// model 维度分组汇总。
fn group_model(conn: &Connection, since: Option<i64>) -> Result<Vec<UsageGroup>, AppError> {
    let mut rows = Vec::new();
    if let Some(s) = since {
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN TRIM(COALESCE(model, '')) = '' THEN '未知' ELSE model END AS name,
                    COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
             FROM api_usage_log WHERE created_at >= ?1 GROUP BY name ORDER BY name",
        )?;
        let it = stmt.query_map(params![s], |r| {
            Ok((r.get::<_, String>(0)?, UsageTotals::from_row_at(r, 1)?))
        })?;
        for row in it {
            let (name, totals) = row?;
            rows.push(UsageGroup { name, totals });
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN TRIM(COALESCE(model, '')) = '' THEN '未知' ELSE model END AS name,
                    COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
             FROM api_usage_log GROUP BY name ORDER BY name",
        )?;
        let it = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, UsageTotals::from_row_at(r, 1)?))
        })?;
        for row in it {
            let (name, totals) = row?;
            rows.push(UsageGroup { name, totals });
        }
    }
    Ok(rows)
}

/// 总量汇总。
fn total(conn: &Connection, since: Option<i64>) -> Result<UsageTotals, AppError> {
    let t = match since {
        Some(s) => {
            let mut stmt = conn.prepare(
                "SELECT COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
                 FROM api_usage_log WHERE created_at >= ?1",
            )?;
            stmt.query_row(params![s], |r| UsageTotals::from_row(r))?
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
                 FROM api_usage_log",
            )?;
            stmt.query_row([], |r| UsageTotals::from_row(r))?
        }
    };
    Ok(t)
}

/// 按天趋势（近 N 天）。
fn trend(conn: &Connection, since: Option<i64>) -> Result<Vec<UsageDay>, AppError> {
    let mut days = Vec::new();
    if let Some(s) = since {
        let mut stmt = conn.prepare(
            "SELECT DATE(created_at, 'unixepoch') AS d,
                    COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
             FROM api_usage_log WHERE created_at >= ?1 GROUP BY d ORDER BY d ASC",
        )?;
        let it = stmt.query_map(params![s], |r| {
            let prompt_tokens: i64 = r.get(1)?;
            let cached_tokens: i64 = r.get(2)?;
            let cache_read_tokens: i64 = r.get(3)?;
            let cache_write_tokens: i64 = r.get(4)?;
            Ok(UsageDay {
                date: r.get(0)?,
                prompt_tokens,
                cached_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cache_hit_rate: cache_hit_rate(cache_read_tokens, cache_write_tokens, prompt_tokens),
            })
        })?;
        for row in it {
            days.push(row?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT DATE(created_at, 'unixepoch') AS d,
                    COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
             FROM api_usage_log GROUP BY d ORDER BY d ASC",
        )?;
        let it = stmt.query_map([], |r| {
            let prompt_tokens: i64 = r.get(1)?;
            let cached_tokens: i64 = r.get(2)?;
            let cache_read_tokens: i64 = r.get(3)?;
            let cache_write_tokens: i64 = r.get(4)?;
            Ok(UsageDay {
                date: r.get(0)?,
                prompt_tokens,
                cached_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cache_hit_rate: cache_hit_rate(cache_read_tokens, cache_write_tokens, prompt_tokens),
            })
        })?;
        for row in it {
            days.push(row?);
        }
    }
    Ok(days)
}

/// 计算用量汇总（days<=0 表示全量，默认 30 天窗口）。
pub fn usage_summary(
    conn: &Connection,
    days: Option<i64>,
) -> Result<UsageSummary, AppError> {
    let days_value = days.unwrap_or(30);
    let is_all = days_value <= 0;
    let since = if is_all {
        None
    } else {
        Some(crate::utils::now() - days_value * 24 * 3600)
    };
    Ok(UsageSummary {
        totals: total(conn, since)?,
        by_provider: group_provider(conn, since)?,
        by_model: group_model(conn, since)?,
        trend: trend(conn, since)?,
    })
}

/// 单会话用量合计（持久化历史；会话无记录时返回全 0 合计）。
pub fn session_usage(conn: &Connection, session_id: &str) -> Result<UsageTotals, AppError> {
    let mut stmt = conn.prepare(
        "SELECT COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cached_tokens), 0),
                      COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)
         FROM api_usage_log WHERE session_id = ?1",
    )?;
    let t = stmt.query_row(params![session_id], |r| UsageTotals::from_row(r))?;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE api_usage_log (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id        TEXT NOT NULL,
                provider          TEXT NOT NULL DEFAULT '',
                model             TEXT NOT NULL DEFAULT '',
                api_format        TEXT NOT NULL DEFAULT '',
                prompt_tokens     INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens      INTEGER NOT NULL DEFAULT 0,
                cached_tokens     INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                created_at        INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    fn insert(
        conn: &Connection,
        session: &str,
        provider: &str,
        model: &str,
        prompt: i64,
        completion: i64,
        cached: i64,
        created_at: i64,
    ) {
        conn.execute(
            "INSERT INTO api_usage_log
               (session_id, provider, model, api_format, prompt_tokens, completion_tokens, total_tokens,
                cached_tokens, cache_read_tokens, cache_write_tokens, created_at)
             VALUES (?1, ?2, ?3, 'openai', ?4, ?5, ?4 + ?5, ?6, ?6, 0, ?7)",
            params![session, provider, model, prompt, completion, cached, created_at],
        )
        .unwrap();
    }

    #[test]
    fn summary_totals_groups_and_trend() {
        let conn = mem_conn();
        let base = crate::utils::now() - 86400;
        insert(&conn, "s1", "deepseek", "deepseek-chat", 100, 20, 40, base - 2 * 86400);
        insert(&conn, "s2", "deepseek", "deepseek-chat", 200, 30, 80, base - 2 * 86400);
        insert(&conn, "s3", "openai", "gpt-4o", 300, 40, 0, base);
        // 无 provider 行 → 归 "未知"。
        insert(&conn, "g1", "", "claude-3-5", 50, 10, 0, base);

        let s = usage_summary(&conn, None).unwrap();
        assert_eq!(s.totals.call_count, 4);
        assert_eq!(s.totals.prompt_tokens, 650);
        assert_eq!(s.totals.completion_tokens, 100);
        assert_eq!(s.totals.total_tokens, 750);
        assert_eq!(s.totals.cached_tokens, 120);
        assert_eq!(s.totals.cache_read_tokens, 120);
        assert_eq!(s.totals.cache_write_tokens, 0);
        assert_eq!(s.totals.cache_hit_rate, (120f64 / (650f64 + 120f64)) * 100.0);

        let by_provider: std::collections::BTreeMap<&str, &UsageGroup> = s
            .by_provider
            .iter()
            .map(|g| (g.name.as_str(), g))
            .collect();
        assert_eq!(by_provider.len(), 3);
        assert_eq!(by_provider["deepseek"].totals.call_count, 2);
        assert_eq!(by_provider["openai"].totals.call_count, 1);
        assert_eq!(by_provider["未知"].totals.call_count, 1);

        let by_model: std::collections::BTreeMap<&str, &UsageGroup> = s
            .by_model
            .iter()
            .map(|g| (g.name.as_str(), g))
            .collect();
        assert_eq!(by_model["deepseek-chat"].totals.prompt_tokens, 300);

        // 近 1 天窗口只含 2 条。
        let s1 = usage_summary(&conn, Some(1)).unwrap();
        assert_eq!(s1.totals.call_count, 2);
        // 趋势按天至少 2 天。
        assert!(s.trend.len() >= 2);
    }

    #[test]
    fn cache_rate_guard_zero_denominator() {
        let conn = mem_conn();
        let now = crate::utils::now();
        insert(&conn, "s1", "p", "m", 0, 0, 0, now);
        insert(&conn, "s2", "p", "m", 100, 0, 250, now); // read 超过 prompt：分母按 prompt+read+write，命中率不会失真到 >100
        let s = usage_summary(&conn, None).unwrap();
        assert_eq!(s.totals.call_count, 2);
        assert_eq!(s.totals.cache_hit_rate, (250f64 / (100f64 + 250f64)) * 100.0);
    }

    #[test]
    fn provider_group_uses_display_name_when_known() {
        let conn = mem_conn();
        conn.execute_batch(
            "CREATE TABLE api_providers (
                id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '',
                api_endpoint TEXT NOT NULL DEFAULT '', api_key TEXT DEFAULT ''
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_providers (id, name) VALUES ('deepseek', 'DeepSeek')",
            [],
        )
        .unwrap();
        let now = crate::utils::now();
        insert(&conn, "s1", "deepseek", "deepseek-chat", 100, 20, 40, now);
        // 未在 api_providers 注册的 id → 回落显示 id。
        insert(&conn, "s2", "custom", "m", 10, 5, 0, now);

        let s = usage_summary(&conn, None).unwrap();
        let by_provider: std::collections::BTreeMap<String, &UsageGroup> = s
            .by_provider
            .iter()
            .map(|g| (g.name.clone(), g))
            .collect();
        assert!(by_provider.contains_key("DeepSeek"), "应显示 api_providers.name");
        assert!(by_provider.contains_key("custom"), "未注册 provider 回落 id");
        assert!(!by_provider.contains_key("deepseek"));
    }

    #[test]
    fn session_usage_aggregates_only_that_session() {
        let conn = mem_conn();
        let now = crate::utils::now();
        insert(&conn, "s1", "p", "m", 100, 20, 40, now);
        insert(&conn, "s1", "p", "m", 200, 30, 80, now - 1);
        insert(&conn, "s2", "p", "m", 50, 5, 0, now);
        let t1 = session_usage(&conn, "s1").unwrap();
        assert_eq!(t1.call_count, 2);
        assert_eq!(t1.prompt_tokens, 300);
        assert_eq!(t1.completion_tokens, 50);
        assert_eq!(t1.cached_tokens, 120);
        let t_none = session_usage(&conn, "sX").unwrap();
        assert_eq!(t_none.call_count, 0);
        assert_eq!(t_none.prompt_tokens, 0);
        assert_eq!(t_none.cache_hit_rate, 0.0);
    }

    #[test]
    fn room_usage_aggregates_director_and_participants() {
        let conn = mem_conn();
        let now = crate::utils::now();
        // 房间 r1：director + 两名参与者（不同模型）。
        insert(&conn, "groupchat:r1", "deepseek", "director-model", 100, 10, 50, now);
        insert(&conn, "groupchat:r1:pA", "deepseek", "participant-A", 200, 20, 100, now);
        insert(&conn, "groupchat:r1:pB", "openai", "participant-B", 300, 30, 0, now);
        // 干扰：另一房间 & 单聊（不得计入 r1）。
        insert(&conn, "groupchat:r2:pA", "openai", "participant-A", 999, 1, 0, now);
        insert(&conn, "s-chat", "deepseek", "m", 999, 1, 0, now);

        let r1 = room_usage(&conn, "r1").unwrap();
        assert_eq!(r1.totals.call_count, 3);
        assert_eq!(r1.totals.prompt_tokens, 600);
        assert_eq!(r1.totals.completion_tokens, 60);
        assert_eq!(r1.totals.cached_tokens, 150);
        let by_model: std::collections::BTreeMap<String, &UsageGroup> = r1
            .by_model
            .iter()
            .map(|g| (g.name.clone(), g))
            .collect();
        assert_eq!(by_model.len(), 3);
        assert_eq!(by_model["participant-A"].totals.prompt_tokens, 200);
        let by_provider: std::collections::BTreeMap<String, &UsageGroup> = r1
            .by_provider
            .iter()
            .map(|g| (g.name.clone(), g))
            .collect();
        assert_eq!(by_provider.len(), 2);
        let empty = room_usage(&conn, "empty").unwrap();
        assert_eq!(empty.totals.call_count, 0);
    }
}
