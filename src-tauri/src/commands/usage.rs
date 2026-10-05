//! 全局用量统计（`api_usage_log` → 总量 / 按 Provider / 按 Model / 按天趋势）。
//!
//! 四桶互斥口径：`prompt_tokens`=未命中输入、`cache_read_tokens`=缓存命中读取、
//! `cache_write_tokens`=缓存写入、`completion_tokens`=输出；`total_tokens`=四桶之和，
//! `cached_tokens` 恒等于 read+write（兼容旧展示）。
//! 缓存命中率 = cache_read / (prompt + cache_read + cache_write)（分母 0 → 0）。
//!
//! 协议差异已在解析层抹平（见 `api_agent/client.rs`）：OpenAI 协议的 `prompt_tokens` 含命中部分，
//! 落库前已拆出，与 Anthropic 的 `input_tokens` 同口径；**缓存写入只有 Anthropic 会上报**
//! （`cache_creation_input_tokens`），OpenAI 兼容协议响应里没有该字段，故其 read 之外的 write 恒为 0。
//!
//! 成本归因维度（[`usage_attribution`]）：会话 / 群聊 / 工作流 / 知识库。前两者与工作流全部由
//! `session_id` 命名约定与会话行 `origin` 推导，不新增列、不改写入路径；知识库一类由
//! `kb:{kbId}` 前缀识别（写入方 `commands/knowledge.rs::record_kb_usage`）。
//!
//! 条目维护：写入为纯 append（唯一入口 `record_usage_row`），**只在实体被删除时同步清理**，
//! 且三处口径一致——删会话（`commands/session.rs`）、删群聊房间（`groupchat/store.rs`）、
//! 删工作流**定义**（`workflow/mod.rs`）都会删除对应 `api_usage_log` 行。
//! 归档会话、删工作流**执行记录**均不清理：用量归因绑定的是定义本身，删实例不改变定义存在性。

use serde::{Deserialize, Serialize};

use std::collections::HashMap;

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
    /// 全 0 合计（累加起点）。
    fn empty() -> Self {
        Self {
            call_count: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cached_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cache_hit_rate: 0.0,
        }
    }

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
fn room_group_by(conn: &Connection, room_id: &str, col: &str) -> Result<Vec<UsageGroup>, AppError> {
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
    let is_provider = col == "provider";
    let mut rows = Vec::new();
    for row in it {
        let (name, totals) = row?;
        // provider 维度：历史数据可能既写 id 又写显示名，解析到同一标签后合并为一行；model 维度原样。
        let name = if is_provider {
            resolve_provider_label(&name, &display)
        } else {
            name
        };
        rows.push(UsageGroup { name, totals });
    }
    if is_provider {
        Ok(merge_groups_by_name(rows))
    } else {
        Ok(rows)
    }
}

/// 群聊房间用量合计（含多参与者/多模型；room 无记录时返回全 0）。
pub fn room_usage(conn: &Connection, room_id: &str) -> Result<RoomUsage, AppError> {
    let sql =
        "SELECT COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
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

/// 表是否存在：统计查询对可选表做渐进降级（表缺失 → 跳过关联、回落原始值）。
fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |_| Ok(()),
    )
    .is_ok()
}

/// provider 显示名映射：api_providers.id → api_providers.name（表不存在/无行时回落原值）。
fn provider_display_names(conn: &Connection) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if !table_exists(conn, "api_providers") {
        return map;
    }
    if let Ok(mut stmt) =
        conn.prepare("SELECT id, name FROM api_providers WHERE TRIM(COALESCE(name, '')) != ''")
    {
        if let Ok(it) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            for row in it.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

/// provider 原始值 → 展示标签：命中 id→name 映射用显示名；原始值本身已是某个显示名时
/// 原样保留；两者都不匹配则回落原始值（历史行既可能写 id 也可能写显示名）。
fn resolve_provider_label(
    raw: &str,
    display: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(name) = display.get(raw) {
        return name.clone();
    }
    if display.values().any(|n| n == raw) {
        return raw.to_string();
    }
    raw.to_string()
}

/// 把一次合计叠加到累计值上（命中率按叠加后的四桶重算，而非简单相加）。
fn accumulate(acc: &mut UsageTotals, add: &UsageTotals) {
    acc.call_count += add.call_count;
    acc.prompt_tokens += add.prompt_tokens;
    acc.completion_tokens += add.completion_tokens;
    acc.total_tokens += add.total_tokens;
    acc.cached_tokens += add.cached_tokens;
    acc.cache_read_tokens += add.cache_read_tokens;
    acc.cache_write_tokens += add.cache_write_tokens;
    acc.cache_hit_rate = cache_hit_rate(
        acc.cache_read_tokens,
        acc.cache_write_tokens,
        acc.prompt_tokens,
    );
}

/// 分组按合计 token 降序（体量大的在前）。
fn sort_groups_desc(groups: &mut [UsageGroup]) {
    groups.sort_by(|a, b| b.totals.total_tokens.cmp(&a.totals.total_tokens));
}

/// 按展示标签合并同名分组（sum callCount 与各 token 指标），命中率按合并后的四桶重算；
/// 合并结果按 total_tokens 降序（provider 维度专用；历史 id 与显示名两行归并为一行）。
fn merge_groups_by_name(groups: Vec<UsageGroup>) -> Vec<UsageGroup> {
    let mut merged: Vec<UsageGroup> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for g in groups {
        match index.get(&g.name) {
            Some(&i) => accumulate(&mut merged[i].totals, &g.totals),
            None => {
                index.insert(g.name.clone(), merged.len());
                merged.push(g);
            }
        }
    }
    sort_groups_desc(&mut merged);
    merged
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
            let name = resolve_provider_label(&name, &display);
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
            let name = resolve_provider_label(&name, &display);
            rows.push(UsageGroup { name, totals });
        }
    }
    Ok(merge_groups_by_name(rows))
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
                cache_hit_rate: cache_hit_rate(
                    cache_read_tokens,
                    cache_write_tokens,
                    prompt_tokens,
                ),
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
                cache_hit_rate: cache_hit_rate(
                    cache_read_tokens,
                    cache_write_tokens,
                    prompt_tokens,
                ),
            })
        })?;
        for row in it {
            days.push(row?);
        }
    }
    Ok(days)
}

/// 统计窗口起点：days<=0/None 语义与 `usage_summary` 一致（None 视作 30 天，<=0 为全量）。
fn window_since(days: Option<i64>) -> Option<i64> {
    let days_value = days.unwrap_or(30);
    if days_value <= 0 {
        None
    } else {
        Some(crate::utils::now() - days_value * 24 * 3600)
    }
}

/// 计算用量汇总（days<=0 表示全量，默认 30 天窗口）。
pub fn usage_summary(conn: &Connection, days: Option<i64>) -> Result<UsageSummary, AppError> {
    let since = window_since(days);
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

// ════════════════════════════════════════════════════════════
// 成本归因维度（会话 / 群聊 / 工作流）
// ════════════════════════════════════════════════════════════

/// 群聊用量 scope 前缀（房间级 `groupchat:{roomId}`，历史行可能带 `:{participantId}` 后缀）。
const GROUPCHAT_SCOPE_PREFIX: &str = "groupchat:";

/// 知识库用量 scope 前缀（库级 `kb:{kbId}`）。
///
/// **跨模块共用**：写入方是 `commands/knowledge.rs` 的 `record_kb_usage`（知识库的 AI 调用：
/// 投喂整理 / 文件补整理 / 片段与工作沉淀 / 对话沉淀 / AI 生成），所以这里 `pub` 出去，
/// 避免两处各写一个 "kb:" 字符串。分桶识别见 `usage_attribution`。
pub const KB_SCOPE_PREFIX: &str = "kb:";

/// 「知识库」维度里**保留的伪库 id**：自动沉淀的用量。
///
/// 自动沉淀一次调用可能写入多个库（模型按内容逐条选库），token 无法拆分到某一个库上，
/// 所以它**只并入「知识库」维度合计，不单列明细分组** —— 本质仍是知识库归因，
/// 但它不是一个库，单列一行「自动沉淀」是冗余。识别见 `usage_attribution`。
pub const KB_AUTO_ID: &str = "__auto__";

/// 工作流会话来源前缀：`workflow:{definitionId}`；历史行只有 `workflow`（无定义 id）。
const WORKFLOW_ORIGIN_PREFIX: &str = "workflow";

/// 单个归因维度：稳定 key + 该维度合计 + 明细分组。
///
/// 维度本身既是聚合口径（合计用于「按归因」总览），也是明细入口（分组供下钻展示）。
/// 约定：只有 Anthropic 会上报缓存写。`key ∈ {session, groupchat, workflow, knowledge}`。
///
/// 归因键取自 `api_usage_log.session_id`（无来源列，沿用既有命名约定；不改写入路径）：
/// - 群聊：`groupchat:{roomId}`（含历史 `groupchat:{roomId}:{participantId}`）→ 按房间聚合，
///   名称取 `groupchat_rooms.title`；
/// - 知识库：`kb:{kbId}` → 按库聚合，**名称只能填库 id**（库定义在 MEMORY.db，本查询在主库，
///   跨库不 JOIN）→ 由前端用 `kb_list_bases` 映射成名字；
/// - 工作流：会话行 `origin` 以 `workflow` 开头（`workflow:{definitionId}`）→ 按工作流定义聚合，
///   名称取 `workflow_definitions.name`；历史 `origin='workflow'` 行归入「未知工作流」；
/// - 会话：其余 → 一行一会话，名称取 `sessions.title`（会话已删则回落 session_id）。
///
/// CLI Agent（终端 / 插件 / claude、codex 等子进程）不经宿主发起 LLM 调用，不产生用量行，
/// 因此不属于任何维度——这也是各维度合计可能小于全局总量的唯一原因。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDimension {
    pub key: String,
    pub totals: UsageTotals,
    pub groups: Vec<UsageGroup>,
}

/// 成本归因维度汇总（固定顺序：会话 → 群聊 → 工作流 → 知识库）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAttribution {
    pub dimensions: Vec<UsageDimension>,
}

/// 逐 `session_id` 的用量合计（分桶输入；窗口口径同 `usage_summary`）。
fn group_by_scope(
    conn: &Connection,
    since: Option<i64>,
) -> Result<Vec<(String, UsageTotals)>, AppError> {
    let select = "SELECT session_id, COUNT(*), COALESCE(SUM(prompt_tokens), 0),
                    COALESCE(SUM(completion_tokens), 0), COALESCE(SUM(total_tokens), 0),
                    COALESCE(SUM(cached_tokens), 0), COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(cache_write_tokens), 0)
             FROM api_usage_log";
    let mut rows = Vec::new();
    if let Some(s) = since {
        let mut stmt = conn.prepare(&format!(
            "{} WHERE created_at >= ?1 GROUP BY session_id",
            select
        ))?;
        let it = stmt.query_map(params![s], |r| {
            Ok((r.get::<_, String>(0)?, UsageTotals::from_row_at(r, 1)?))
        })?;
        for row in it {
            rows.push(row?);
        }
    } else {
        let mut stmt = conn.prepare(&format!("{} GROUP BY session_id", select))?;
        let it = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, UsageTotals::from_row_at(r, 1)?))
        })?;
        for row in it {
            rows.push(row?);
        }
    }
    Ok(rows)
}

/// 会话 id → (标题, 来源)；表缺失或读取失败时为空映射（调用方回落原始 id）。
fn session_meta(conn: &Connection) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    if !table_exists(conn, "sessions") {
        return map;
    }
    if let Ok(mut stmt) =
        conn.prepare("SELECT id, COALESCE(title, ''), COALESCE(origin, '') FROM sessions")
    {
        if let Ok(it) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
            ))
        }) {
            for row in it.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

/// 群聊房间 id → 标题。
fn room_titles(conn: &Connection) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if !table_exists(conn, "groupchat_rooms") {
        return map;
    }
    if let Ok(mut stmt) = conn.prepare("SELECT id, COALESCE(title, '') FROM groupchat_rooms") {
        if let Ok(it) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            for row in it.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

/// 工作流定义 id → 名称。
fn workflow_names(conn: &Connection) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if !table_exists(conn, "workflow_definitions") {
        return map;
    }
    if let Ok(mut stmt) = conn.prepare("SELECT id, COALESCE(name, '') FROM workflow_definitions") {
        if let Ok(it) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            for row in it.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

/// 累加表 → 具名分组（key 经 `name_of` 解析为展示名；缺失时由 `name_of` 给出回落名）。
fn named_groups<F>(acc: HashMap<String, UsageTotals>, name_of: F) -> Vec<UsageGroup>
where
    F: Fn(&str) -> String,
{
    let mut groups: Vec<UsageGroup> = acc
        .into_iter()
        .map(|(key, totals)| UsageGroup {
            name: name_of(&key),
            totals,
        })
        .collect();
    sort_groups_desc(&mut groups);
    groups
}

/// 累加一次：按 key 归并合计（命中率随累加重算）。
fn merge_into(acc: &mut HashMap<String, UsageTotals>, key: &str, add: &UsageTotals) {
    match acc.get_mut(key) {
        Some(existing) => accumulate(existing, add),
        None => {
            acc.insert(key.to_string(), add.clone());
        }
    }
}

/// 一组合计的四桶累加（命中率按累加结果重算，不做简单平均）。
fn totals_of<'a, I: Iterator<Item = &'a UsageTotals>>(items: I) -> UsageTotals {
    let mut acc = UsageTotals::empty();
    for t in items {
        accumulate(&mut acc, t);
    }
    acc
}

/// 组装单个归因维度（合计由明细分组四桶累加得到）。
///
/// 例外：**知识库维度**不适用此口径 —— 自动沉淀的用量并入其合计但不单列分组，
/// 故那里直接构造 `UsageDimension`（见 `usage_attribution`）。
fn dimension(key: &str, groups: Vec<UsageGroup>) -> UsageDimension {
    UsageDimension {
        key: key.to_string(),
        totals: totals_of(groups.iter().map(|g| &g.totals)),
        groups,
    }
}

/// 计算四维成本归因（days<=0 表示全量，默认 30 天窗口）。
pub fn usage_attribution(
    conn: &Connection,
    days: Option<i64>,
) -> Result<UsageAttribution, AppError> {
    let scopes = group_by_scope(conn, window_since(days))?;
    let meta = session_meta(conn);
    let rooms = room_titles(conn);
    let workflows = workflow_names(conn);

    let mut by_session: Vec<UsageGroup> = Vec::new();
    let mut room_acc: HashMap<String, UsageTotals> = HashMap::new();
    let mut wf_acc: HashMap<String, UsageTotals> = HashMap::new();
    let mut kb_acc: HashMap<String, UsageTotals> = HashMap::new();
    // 自动沉淀（伪库 id）：只并入「知识库」维度合计，不单列分组
    let mut kb_auto: UsageTotals = UsageTotals::empty();

    for (scope, totals) in scopes {
        if let Some(rest) = scope.strip_prefix(GROUPCHAT_SCOPE_PREFIX) {
            // 房间 id 取首个 ':' 之前（兼容 `groupchat:{room}:{participant}` 历史行）
            let room_id = rest.split(':').next().unwrap_or("");
            merge_into(&mut room_acc, room_id, &totals);
            continue;
        }
        if let Some(kb_id) = scope.strip_prefix(KB_SCOPE_PREFIX) {
            if kb_id == KB_AUTO_ID {
                accumulate(&mut kb_auto, &totals);
            } else {
                merge_into(&mut kb_acc, kb_id, &totals);
            }
            continue;
        }
        let (title, origin) = meta.get(&scope).cloned().unwrap_or_default();
        if origin.starts_with(WORKFLOW_ORIGIN_PREFIX) {
            let def_id = origin.split_once(':').map(|(_, id)| id).unwrap_or("");
            merge_into(&mut wf_acc, def_id, &totals);
            continue;
        }
        by_session.push(UsageGroup {
            name: if title.trim().is_empty() {
                scope
            } else {
                title
            },
            totals,
        });
    }
    sort_groups_desc(&mut by_session);

    let by_group_chat = named_groups(room_acc, |id| {
        rooms
            .get(id)
            .filter(|t| !t.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| {
                if id.is_empty() {
                    "未知房间".to_string()
                } else {
                    format!("房间 {}", id)
                }
            })
    });
    let by_workflow = named_groups(wf_acc, |id| {
        workflows
            .get(id)
            .filter(|n| !n.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "未知工作流".to_string())
    });
    // 知识库名**解析不了**：知识库定义在 MEMORY.db，而本查询在主库上（跨库不 JOIN，
    // 也不为此把 usage 模块绑上知识库模块）。所以分组名就填库 id，由前端用
    // `kb_list_bases` 映射成名字（映射不到就显示 id —— 库被删了也能看懂是哪一条）。
    let by_knowledge = named_groups(kb_acc, |id| {
        if id.is_empty() {
            "未知知识库".to_string()
        } else {
            id.to_string()
        }
    });
    // 知识库维度是唯一「合计 ≠ 明细之和」的维度：自动沉淀的用量并入合计但不单列（见 `KB_AUTO_ID`）
    let mut kb_totals = totals_of(by_knowledge.iter().map(|g| &g.totals));
    accumulate(&mut kb_totals, &kb_auto);

    Ok(UsageAttribution {
        dimensions: vec![
            dimension("session", by_session),
            dimension("groupchat", by_group_chat),
            dimension("workflow", by_workflow),
            UsageDimension {
                key: "knowledge".to_string(),
                totals: kb_totals,
                groups: by_knowledge,
            },
        ],
    })
}

/// 导出用量报表为 CSV（UTF-8 带 BOM，便于 Excel 正确识别中文表头）。
///
/// 内容：用量总量（调用次数 / 各桶 token / 缓存命中率）+ 按模型分组明细。
/// days 口径同 `usage_summary`（None=默认 30 天，<=0=全量）。
#[tauri::command]
pub fn export_usage_report_csv(
    state: tauri::State<'_, crate::DbState>,
    days: Option<i64>,
    file_path: String,
) -> Result<(), String> {
    let conn = state
        .get_conn()
        .map_err(|e| AppError::Lock(format!("数据库连接失败: {}", e)))?;
    let summary =
        usage_summary(&conn, days).map_err(|e| AppError::Db(format!("查询用量失败: {}", e)))?;

    let mut out = String::new();
    out.push('\u{feff}'); // BOM：Excel 打开才不会把中文表头显示成乱码

    // 用量总量
    out.push_str("用量总量\n");
    out.push_str("调用次数,输入(未命中),输入(缓存读),输入(缓存写),输出,合计,缓存命中率(%)\n");
    let t = &summary.totals;
    out.push_str(&format!(
        "{},{},{},{},{},{},{:.2}\n",
        t.call_count,
        t.prompt_tokens,
        t.cache_read_tokens,
        t.cache_write_tokens,
        t.completion_tokens,
        t.total_tokens,
        t.cache_hit_rate
    ));

    // 按模型分组明细
    out.push('\n');
    out.push_str("按模型明细\n");
    out.push_str("模型,调用次数,输入(未命中),输入(缓存读),输入(缓存写),输出,合计,缓存命中率(%)\n");
    for g in &summary.by_model {
        out.push_str(&csv_field(&g.name));
        out.push_str(&format!(
            ",{},{},{},{},{},{},{:.2}\n",
            g.totals.call_count,
            g.totals.prompt_tokens,
            g.totals.cache_read_tokens,
            g.totals.cache_write_tokens,
            g.totals.completion_tokens,
            g.totals.total_tokens,
            g.totals.cache_hit_rate
        ));
    }

    std::fs::write(&file_path, out).map_err(|e| AppError::Io(format!("写入文件失败: {}", e)))?;
    Ok(())
}

/// CSV 字段转义：含逗号 / 引号 / 换行时用双引号包裹，并把内部引号翻倍。
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ════════════════════════════════════════════════════════════
// 本地用量聚合上报到平台（团队用量看板）
// ════════════════════════════════════════════════════════════
//
// 平台契约（已上线）：POST {平台基址}/api/v1/usage/report，Bearer 鉴权，body
//   { "day": "YYYY-MM-DD", "items": [{ model, calls, inputTokens, outputTokens, costFen }] }
// 数值为**当天累计值**，服务端按 (ownerType, ownerId, day, model) **覆盖写**（幂等，重复上报无害）；
// `day` 可省略（服务端取当天）。平台基址复用 `utils::platform` 里写死的常量，不新增可配置项。
//
// **只上报聚合数字**（模型名 / 调用次数 / token 数）——绝不包含任何对话内容或提示词。
//
// 本地口径（`api_usage_log`，四桶互斥）：
//   inputTokens  = prompt_tokens（未命中输入）+ cache_read_tokens + cache_write_tokens
//   outputTokens = completion_tokens
//   本表**没有金额列**（产品内无本地定价），故 costFen 恒为 0 —— 金额由平台侧按模型定价核算。

/// 上报开关设置键（默认开启）。关闭后自动上报停止，手动上报也会被拒绝并给出提示。
pub const USAGE_REPORT_ENABLED_KEY: &str = "usage_report_enabled";

/// 自动上报间隔（秒）：每 30 分钟一次。
const USAGE_REPORT_INTERVAL_SECS: u64 = 30 * 60;

/// 平台单次上报的 items 上限（契约：最多 200 条）。
const USAGE_REPORT_MAX_ITEMS: usize = 200;

/// 一条本地原始用量行（尚未按模型聚合）；一条 `api_usage_log` 记录 → `calls = 1`。
#[derive(Debug, Clone)]
pub struct UsageRow {
    pub model: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_fen: i64,
}

/// 上报条目：按 model 聚合后的当天累计值（字段名对齐平台契约，camelCase 序列化）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageItem {
    pub model: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_fen: i64,
}

/// 把原始用量行按 **model** 分组求和（calls / inputTokens / outputTokens / costFen）。
///
/// - 各数值做**非负归一**（负值按 0 计，避免脏数据把累计值拉低）；
/// - 模型名为空 → 归入「未知」；
/// - 空输入返回空 Vec；
/// - 结果按模型名升序，保证输出稳定、便于断言与比对。
pub fn aggregate_usage(rows: &[UsageRow]) -> Vec<UsageItem> {
    let mut acc: HashMap<String, UsageItem> = HashMap::new();
    for r in rows {
        let model = r.model.trim();
        let model = if model.is_empty() { "未知" } else { model }.to_string();
        let entry = acc.entry(model.clone()).or_insert_with(|| UsageItem {
            model,
            calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_fen: 0,
        });
        entry.calls += r.calls.max(0);
        entry.input_tokens += r.input_tokens.max(0);
        entry.output_tokens += r.output_tokens.max(0);
        entry.cost_fen += r.cost_fen.max(0);
    }
    let mut items: Vec<UsageItem> = acc.into_values().collect();
    items.sort_by(|a, b| a.model.cmp(&b.model));
    items
}

/// 返回 `now` 所在**本地时区**的日期（`YYYY-MM-DD`），与平台 `day` 字段口径一致。
pub fn report_day(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("%Y-%m-%d").to_string()
}

/// 本地时区「今天」0 点的时间戳（秒）。
fn today_start_ts() -> i64 {
    use chrono::TimeZone;
    let now = chrono::Local::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| chrono::Local.from_local_datetime(&dt).single())
        .unwrap_or(now)
        .timestamp()
}

/// 读取当天（本地 0 点起）的原始用量行。`api_usage_log` 无金额列 → `cost_fen` 恒 0。
fn load_today_rows(conn: &Connection) -> Result<Vec<UsageRow>, AppError> {
    let since = today_start_ts();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(model, ''), COALESCE(prompt_tokens, 0), COALESCE(completion_tokens, 0),
                COALESCE(cache_read_tokens, 0), COALESCE(cache_write_tokens, 0)
         FROM api_usage_log WHERE created_at >= ?1",
    )?;
    let it = stmt.query_map(params![since], |r| {
        let prompt: i64 = r.get(1)?;
        let completion: i64 = r.get(2)?;
        let read: i64 = r.get(3)?;
        let write: i64 = r.get(4)?;
        Ok(UsageRow {
            model: r.get(0)?,
            calls: 1,
            input_tokens: prompt + read + write,
            output_tokens: completion,
            cost_fen: 0,
        })
    })?;
    let mut rows = Vec::new();
    for row in it {
        rows.push(row?);
    }
    Ok(rows)
}

/// 组装本次将上报的 `(day, items)`（只读，不联网）。
///
/// items 超过契约上限 200 条时，按 token 体量降序保留前 200（超出部分本就无法一次上报），
/// 之后再按模型名升序还原，保证展示稳定。
pub fn build_report(conn: &Connection) -> Result<(String, Vec<UsageItem>), AppError> {
    let rows = load_today_rows(conn)?;
    let mut items = aggregate_usage(&rows);
    if items.len() > USAGE_REPORT_MAX_ITEMS {
        items.sort_by(|a, b| {
            (b.input_tokens + b.output_tokens).cmp(&(a.input_tokens + a.output_tokens))
        });
        items.truncate(USAGE_REPORT_MAX_ITEMS);
        items.sort_by(|a, b| a.model.cmp(&b.model));
    }
    Ok((report_day(chrono::Local::now()), items))
}

/// 上报守卫：聚合结果为空 → 不调用平台。
///
/// 平台对空 `items` 返回 400，且当天没有任何调用时本就没有可上报的内容；
/// 手动上报与自动上报共用此判据，避免无谓的失败请求。
fn should_skip_upload(items: &[UsageItem]) -> bool {
    items.is_empty()
}

/// 是否开启「向组织上报用量」：缺省开启；显式写 `'0'` / `'false'` 视为关闭。
pub fn usage_report_enabled(conn: &Connection) -> bool {
    match crate::commands::app_settings::get_setting(conn, USAGE_REPORT_ENABLED_KEY) {
        Ok(Some(v)) => {
            let t = v.trim().to_ascii_lowercase();
            !(t == "0" || t == "false")
        }
        _ => true,
    }
}

/// 平台上报请求体
#[derive(Serialize)]
struct ReportBody<'a> {
    day: &'a str,
    items: &'a [UsageItem],
}

/// 平台上报响应（`{ written, ownerType, ownerId }`）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportResp {
    #[serde(default)]
    written: i64,
    #[serde(default)]
    owner_type: Option<String>,
}

/// 实际 POST（命令与后台任务共用）。
async fn post_report(
    token: &str,
    day: &str,
    items: &[UsageItem],
) -> Result<ReportResp, AppError> {
    crate::utils::platform::post_json(
        "/api/v1/usage/report",
        &ReportBody { day, items },
        Some(token),
    )
    .await
}

/// 预览结果：本次将上报的当天聚合数据（供 UI 展示与确认）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReportPreview {
    pub day: String,
    pub items: Vec<UsageItem>,
}

/// 上报结果：失败不抛异常，错误放在 `error` 文案里（UI 直接展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReportResult {
    pub day: String,
    pub item_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrote: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 预览：返回本次将上报的 `{ day, items }`（只读，不联网）。
#[tauri::command]
pub fn usage_report_preview(
    state: tauri::State<'_, crate::DbState>,
) -> Result<UsageReportPreview, String> {
    let conn = state.get_conn().map_err(String::from)?;
    let (day, items) = build_report(&conn).map_err(String::from)?;
    Ok(UsageReportPreview { day, items })
}

/// 立即上报：读当天本地用量 → 按模型聚合 → 用**已登录令牌** POST 平台。
///
/// 未登录 / 已关闭 / 网络失败都**不抛异常给 UI**：返回 `error` 文案即可（离线可用性不受影响）。
#[tauri::command]
pub async fn usage_report_now(
    state: tauri::State<'_, crate::DbState>,
) -> Result<UsageReportResult, String> {
    let (day, items) = {
        let conn = state.get_conn().map_err(String::from)?;
        build_report(&conn).map_err(String::from)?
    };
    let item_count = items.len();

    // ⓪ 当天无用量：**不调用平台**（平台对空 items 返回 400），直接返回明确结果供前端提示。
    // 「今天」= 本地时区当天 0 点起（见 `today_start_ts`，秒级时间戳，与 `created_at` 单位一致）。
    if should_skip_upload(&items) {
        return Ok(UsageReportResult {
            day: day.clone(),
            item_count: 0,
            wrote: None,
            owner_type: None,
            error: None,
        });
    }

    let fail = |error: String| UsageReportResult {
        day: day.clone(),
        item_count,
        wrote: None,
        owner_type: None,
        error: Some(error),
    };

    // ① 设置开关：关闭时明确提示（不静默）
    {
        let conn = state.get_conn().map_err(String::from)?;
        if !usage_report_enabled(&conn) {
            return Ok(fail("已关闭「向组织上报用量」，未上报".to_string()));
        }
    }

    // ② 令牌：未登录平台 → 明确提示
    let token = match crate::commands::account::current_access_token(state.inner()).await {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(fail("未登录平台，无法上报".to_string())),
        Err(e) => return Ok(fail(format!("平台登录状态异常：{}", e))),
    };

    // ③ 上报：幂等覆盖写，失败只回文案
    match post_report(&token, &day, &items).await {
        Ok(resp) => Ok(UsageReportResult {
            day,
            item_count,
            wrote: Some(resp.written > 0),
            owner_type: resp.owner_type,
            error: None,
        }),
        Err(e) => Ok(fail(e.to_string())),
    }
}

/// 后台自动上报循环：启动后**先立即尝试一次**，之后每 30 分钟一次；失败静默（仅记日志），
/// 绝不影响用户使用。
///
/// 受 [`USAGE_REPORT_ENABLED_KEY`] 控制；未登录平台或当天无用量时静默跳过。
pub async fn run_auto_report_loop(state: crate::DbState) {
    loop {
        // 先立即上报一次（避免启动后要等满一个间隔才首次上报），再进入固定间隔循环。
        if let Err(e) = auto_report_once(&state).await {
            log::debug!("[UsageReport] 自动上报跳过：{}", e);
        }
        tokio::time::sleep(std::time::Duration::from_secs(USAGE_REPORT_INTERVAL_SECS)).await;
    }
}

/// 单次自动上报：开关关闭 / 未登录 / 无数据 → 静默返回；其余失败仅记日志。
async fn auto_report_once(state: &crate::DbState) -> Result<(), AppError> {
    let (day, items) = {
        let conn = state.get_conn()?;
        if !usage_report_enabled(&conn) {
            return Ok(());
        }
        build_report(&conn)?
    };
    if should_skip_upload(&items) {
        return Ok(());
    }
    let Some(token) = crate::commands::account::current_access_token(state).await? else {
        return Ok(()); // 未登录：静默
    };
    match post_report(&token, &day, &items).await {
        Ok(resp) => {
            log::info!(
                "[UsageReport] 已上报当天用量：{} 个模型，服务端写入 {} 条（ownerType={:?}）",
                items.len(),
                resp.written,
                resp.owner_type
            );
        }
        Err(e) => log::warn!("[UsageReport] 上报失败（不影响使用）：{}", e),
    }
    Ok(())
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
        insert(
            &conn,
            "s1",
            "deepseek",
            "deepseek-chat",
            100,
            20,
            40,
            base - 2 * 86400,
        );
        insert(
            &conn,
            "s2",
            "deepseek",
            "deepseek-chat",
            200,
            30,
            80,
            base - 2 * 86400,
        );
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
        assert_eq!(
            s.totals.cache_hit_rate,
            (120f64 / (650f64 + 120f64)) * 100.0
        );

        let by_provider: std::collections::BTreeMap<&str, &UsageGroup> =
            s.by_provider.iter().map(|g| (g.name.as_str(), g)).collect();
        assert_eq!(by_provider.len(), 3);
        assert_eq!(by_provider["deepseek"].totals.call_count, 2);
        assert_eq!(by_provider["openai"].totals.call_count, 1);
        assert_eq!(by_provider["未知"].totals.call_count, 1);

        let by_model: std::collections::BTreeMap<&str, &UsageGroup> =
            s.by_model.iter().map(|g| (g.name.as_str(), g)).collect();
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
        assert_eq!(
            s.totals.cache_hit_rate,
            (250f64 / (100f64 + 250f64)) * 100.0
        );
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
        let by_provider: std::collections::BTreeMap<String, &UsageGroup> =
            s.by_provider.iter().map(|g| (g.name.clone(), g)).collect();
        assert!(
            by_provider.contains_key("DeepSeek"),
            "应显示 api_providers.name"
        );
        assert!(
            by_provider.contains_key("custom"),
            "未注册 provider 回落 id"
        );
        assert!(!by_provider.contains_key("deepseek"));
    }

    #[test]
    fn provider_group_merges_id_and_display_name_rows() {
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
        // 同一 provider 的两种写法（历史 id 行 + 显示名行）应合并为一行且计数合计。
        insert(&conn, "s1", "deepseek", "deepseek-chat", 100, 20, 40, now);
        insert(&conn, "s2", "DeepSeek", "deepseek-chat", 200, 30, 60, now);

        let s = usage_summary(&conn, None).unwrap();
        assert_eq!(s.by_provider.len(), 1, "id 行与显示名行应合并为一个分组");
        let g = &s.by_provider[0];
        assert_eq!(g.name, "DeepSeek");
        assert_eq!(g.totals.call_count, 2);
        assert_eq!(g.totals.prompt_tokens, 300);
        assert_eq!(g.totals.completion_tokens, 50);
        assert_eq!(g.totals.total_tokens, 350);
        assert_eq!(g.totals.cached_tokens, 100);
        assert_eq!(g.totals.cache_read_tokens, 100);
        // 命中率按合并后四桶重算，而非简单相加。
        assert_eq!(
            g.totals.cache_hit_rate,
            (100f64 / (300f64 + 100f64)) * 100.0
        );
    }

    #[test]
    fn room_provider_group_merges_id_and_display_name_rows() {
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
        insert(&conn, "groupchat:r1:pA", "deepseek", "m", 100, 10, 0, now);
        insert(&conn, "groupchat:r1", "DeepSeek", "m", 200, 20, 0, now);

        let r1 = room_usage(&conn, "r1").unwrap();
        assert_eq!(r1.by_provider.len(), 1, "房间维度 id 行与显示名行应合并");
        let g = &r1.by_provider[0];
        assert_eq!(g.name, "DeepSeek");
        assert_eq!(g.totals.call_count, 2);
        assert_eq!(g.totals.prompt_tokens, 300);
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
        insert(
            &conn,
            "groupchat:r1",
            "deepseek",
            "director-model",
            100,
            10,
            50,
            now,
        );
        insert(
            &conn,
            "groupchat:r1:pA",
            "deepseek",
            "participant-A",
            200,
            20,
            100,
            now,
        );
        insert(
            &conn,
            "groupchat:r1:pB",
            "openai",
            "participant-B",
            300,
            30,
            0,
            now,
        );
        // 干扰：另一房间 & 单聊（不得计入 r1）。
        insert(
            &conn,
            "groupchat:r2:pA",
            "openai",
            "participant-A",
            999,
            1,
            0,
            now,
        );
        insert(&conn, "s-chat", "deepseek", "m", 999, 1, 0, now);

        let r1 = room_usage(&conn, "r1").unwrap();
        assert_eq!(r1.totals.call_count, 3);
        assert_eq!(r1.totals.prompt_tokens, 600);
        assert_eq!(r1.totals.completion_tokens, 60);
        assert_eq!(r1.totals.cached_tokens, 150);
        let by_model: std::collections::BTreeMap<String, &UsageGroup> =
            r1.by_model.iter().map(|g| (g.name.clone(), g)).collect();
        assert_eq!(by_model.len(), 3);
        assert_eq!(by_model["participant-A"].totals.prompt_tokens, 200);
        let by_provider: std::collections::BTreeMap<String, &UsageGroup> =
            r1.by_provider.iter().map(|g| (g.name.clone(), g)).collect();
        assert_eq!(by_provider.len(), 2);
        let empty = room_usage(&conn, "empty").unwrap();
        assert_eq!(empty.totals.call_count, 0);
    }

    /// 建三张归因查表：会话（含 origin）/ 群聊房间 / 工作流定义。
    fn lookup_tables(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', origin TEXT);
             CREATE TABLE groupchat_rooms (id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '');
             CREATE TABLE workflow_definitions (id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '');",
        )
        .unwrap();
    }

    fn names(groups: &[UsageGroup]) -> std::collections::BTreeMap<&str, i64> {
        groups
            .iter()
            .map(|g| (g.name.as_str(), g.totals.prompt_tokens))
            .collect()
    }

    /// 按 key 取维度（断言用）。
    fn dim<'a>(a: &'a UsageAttribution, key: &str) -> &'a UsageDimension {
        a.dimensions
            .iter()
            .find(|d| d.key == key)
            .unwrap_or_else(|| panic!("缺少维度 {}", key))
    }

    #[test]
    fn attribution_splits_session_room_workflow_and_knowledge() {
        let conn = mem_conn();
        lookup_tables(&conn);
        conn.execute(
            "INSERT INTO sessions (id, title, origin) VALUES ('s1', '写代码', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, origin) VALUES ('wf1', '工作流 · 节点A', 'workflow:d1')",
            [],
        )
        .unwrap();
        // 历史行：origin 只有 'workflow'（无定义 id），归入「未知工作流」。
        conn.execute(
            "INSERT INTO sessions (id, title, origin) VALUES ('wf0', '工作流 · 旧节点', 'workflow')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO groupchat_rooms (id, title) VALUES ('r1', '写小说')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflow_definitions (id, name) VALUES ('d1', '小说工作流')",
            [],
        )
        .unwrap();

        let now = crate::utils::now();
        insert(&conn, "s1", "p", "m", 100, 10, 0, now);
        insert(&conn, "groupchat:r1", "p", "m", 200, 20, 0, now);
        // 历史参与者级 scope 应与房间级 scope 合并为同一行。
        insert(&conn, "groupchat:r1:pA", "p", "m", 300, 30, 0, now);
        insert(&conn, "wf1", "p", "m", 400, 40, 0, now);
        insert(&conn, "wf0", "p", "m", 500, 50, 0, now);
        // 知识库：`kb:{kbId}` 前缀 → 独立维度；同名库的多次调用（如一个文件的逐块补整理）合并
        insert(&conn, "kb:kb-1", "p", "m", 60, 6, 0, now);
        insert(&conn, "kb:kb-1", "p", "m", 70, 7, 0, now);

        let a = usage_attribution(&conn, None).unwrap();
        // 维度顺序固定：会话 → 群聊 → 工作流 → 知识库
        let keys: Vec<&str> = a.dimensions.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, vec!["session", "groupchat", "workflow", "knowledge"]);

        let session = dim(&a, "session");
        assert_eq!(session.groups.len(), 1);
        assert_eq!(session.groups[0].name, "写代码");
        assert_eq!(session.groups[0].totals.prompt_tokens, 100);
        // 维度合计 = 明细之和（供「按归因」总览直接展示）
        assert_eq!(session.totals.prompt_tokens, 100);
        assert_eq!(session.totals.call_count, 1);

        let groupchat = dim(&a, "groupchat");
        assert_eq!(groupchat.groups.len(), 1);
        assert_eq!(groupchat.groups[0].name, "写小说");
        assert_eq!(groupchat.groups[0].totals.call_count, 2);
        assert_eq!(groupchat.groups[0].totals.prompt_tokens, 500);
        assert_eq!(groupchat.totals.prompt_tokens, 500);

        let workflow = dim(&a, "workflow");
        let wf = names(&workflow.groups);
        assert_eq!(wf.len(), 2);
        assert_eq!(wf["小说工作流"], 400);
        assert_eq!(wf["未知工作流"], 500);
        assert_eq!(workflow.totals.prompt_tokens, 900);

        // 知识库：分组名是**库 id**（库定义在 MEMORY.db，主库查询解析不了名字），
        // 由前端映射成库名；两次调用合并成一行、call_count=2。
        let knowledge = dim(&a, "knowledge");
        assert_eq!(knowledge.groups.len(), 1);
        assert_eq!(knowledge.groups[0].name, "kb-1");
        assert_eq!(knowledge.groups[0].totals.prompt_tokens, 130);
        assert_eq!(knowledge.groups[0].totals.call_count, 2);

        // 四维合计 = 全局总量（无用量行落在维度之外）。
        let all = usage_summary(&conn, None).unwrap().totals.prompt_tokens;
        let sum: i64 = a.dimensions.iter().map(|d| d.totals.prompt_tokens).sum();
        assert_eq!(sum, all);

        // 窗口：3 天前的行在近 1 天窗口内不可见。
        insert(&conn, "s-old", "p", "m", 999, 1, 0, now - 3 * 86400);
        let recent = usage_attribution(&conn, Some(1)).unwrap();
        assert!(names(&dim(&recent, "session").groups)
            .get("s-old")
            .is_none());
    }

    #[test]
    fn attribution_degrades_without_lookup_tables() {
        // 只有用量表：名称解析全部回落，不 panic、不因缺表报错。
        let conn = mem_conn();
        let now = crate::utils::now();
        insert(&conn, "groupchat:r9", "p", "m", 10, 1, 0, now);
        insert(&conn, "s-orphan", "p", "m", 20, 2, 0, now);

        let a = usage_attribution(&conn, None).unwrap();
        let groupchat = dim(&a, "groupchat");
        assert_eq!(groupchat.groups.len(), 1);
        assert_eq!(groupchat.groups[0].name, "房间 r9");
        let session = dim(&a, "session");
        assert_eq!(session.groups.len(), 1);
        assert_eq!(session.groups[0].name, "s-orphan");
        assert!(dim(&a, "workflow").groups.is_empty());
        assert_eq!(dim(&a, "workflow").totals.call_count, 0);
        assert!(dim(&a, "knowledge").groups.is_empty());
    }

    // ── 聚合上报单测 ──

    /// 多模型分组：不同模型各成一条；同模型多行相加。
    #[test]
    fn aggregate_usage_groups_by_model_and_sums() {
        let rows = vec![
            UsageRow {
                model: "gpt-4o".into(),
                calls: 1,
                input_tokens: 100,
                output_tokens: 50,
                cost_fen: 0,
            },
            UsageRow {
                model: "gpt-4o".into(),
                calls: 1,
                input_tokens: 200,
                output_tokens: 80,
                cost_fen: 0,
            },
            UsageRow {
                model: "claude-3-5".into(),
                calls: 1,
                input_tokens: 300,
                output_tokens: 40,
                cost_fen: 0,
            },
        ];
        let items = aggregate_usage(&rows);
        assert_eq!(items.len(), 2);
        // 按模型名升序：claude-3-5 在前
        assert_eq!(items[0].model, "claude-3-5");
        assert_eq!(items[0].calls, 1);
        assert_eq!(items[0].input_tokens, 300);
        assert_eq!(items[0].output_tokens, 40);
        // gpt-4o 两行相加
        assert_eq!(items[1].model, "gpt-4o");
        assert_eq!(items[1].calls, 2);
        assert_eq!(items[1].input_tokens, 300);
        assert_eq!(items[1].output_tokens, 130);
    }

    /// 空输入 → 空结果；负值非负归一；空模型名归入「未知」。
    #[test]
    fn aggregate_usage_empty_and_negative_normalized() {
        assert!(aggregate_usage(&[]).is_empty());

        let rows = vec![
            UsageRow {
                model: "".into(),
                calls: -3,
                input_tokens: -10,
                output_tokens: -1,
                cost_fen: -5,
            },
            UsageRow {
                model: "  ".into(),
                calls: 1,
                input_tokens: 5,
                output_tokens: 2,
                cost_fen: 1,
            },
        ];
        let items = aggregate_usage(&rows);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].model, "未知");
        assert_eq!(items[0].calls, 1);
        assert_eq!(items[0].input_tokens, 5);
        assert_eq!(items[0].output_tokens, 2);
        assert_eq!(items[0].cost_fen, 1);
    }

    /// 当天筛选：只有 `created_at >= 本地今天 0 点（秒级）` 的行进入上报；
    /// 昨天 23:59:59 的行必须被排除（这条覆盖「筛选条件能正确命中给定 created_at 的行」）。
    #[test]
    fn build_report_picks_only_today_local_rows() {
        let conn = mem_conn();
        let start = today_start_ts();
        insert(&conn, "s1", "p", "gpt-4o", 100, 10, 0, start - 1); // 昨天最后一秒 → 不含
        insert(&conn, "s1", "p", "gpt-4o", 200, 20, 0, start); // 今天 0 点整 → 含
        let (day, items) = build_report(&conn).unwrap();
        assert_eq!(day, report_day(chrono::Local::now()));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].model, "gpt-4o");
        assert_eq!(items[0].calls, 1);
        assert_eq!(items[0].input_tokens, 200); // prompt + read + write
        assert_eq!(items[0].output_tokens, 20);
        assert_eq!(items[0].cost_fen, 0); // 本表无定价 → 恒 0
    }

    /// 当天无用量 → 守卫判定「跳过上报」（不调用平台，避免空 items 的 HTTP 400）。
    #[test]
    fn usage_report_skips_when_no_today_rows() {
        let conn = mem_conn();
        let start = today_start_ts();
        insert(&conn, "s1", "p", "gpt-4o", 100, 10, 0, start - 1); // 仅昨天有数据
        let (_day, items) = build_report(&conn).unwrap();
        assert!(items.is_empty(), "昨天的用量不应落入当天上报");
        assert!(should_skip_upload(&items), "空 items 必须跳过上报");
        // 有数据时不得跳过（防止守卫误伤正常上报）
        let one = [UsageItem {
            model: "m".into(),
            calls: 1,
            input_tokens: 1,
            output_tokens: 1,
            cost_fen: 0,
        }];
        assert!(!should_skip_upload(&one));
    }

    /// `report_day` 按本地时区格式化为 `YYYY-MM-DD`。
    #[test]
    fn report_day_formats_local_date() {
        use chrono::TimeZone;
        let dt = chrono::Local
            .with_ymd_and_hms(2026, 10, 5, 23, 59, 0)
            .single()
            .expect("构造本地时间失败");
        assert_eq!(report_day(dt), "2026-10-05");

        let dt2 = chrono::Local
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("构造本地时间失败");
        assert_eq!(report_day(dt2), "2026-01-01");
    }

    /// 开关判据：缺省开启；只认 '0' / 'false' 为关闭。
    #[test]
    fn usage_report_enabled_default_and_off() {
        let conn = mem_conn();
        conn.execute_batch(
            "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL);",
        )
        .unwrap();
        assert!(usage_report_enabled(&conn), "缺省应开启");
        crate::commands::app_settings::set_setting(&conn, USAGE_REPORT_ENABLED_KEY, "1").unwrap();
        assert!(usage_report_enabled(&conn));
        crate::commands::app_settings::set_setting(&conn, USAGE_REPORT_ENABLED_KEY, "0").unwrap();
        assert!(!usage_report_enabled(&conn));
        crate::commands::app_settings::set_setting(&conn, USAGE_REPORT_ENABLED_KEY, "FALSE").unwrap();
        assert!(!usage_report_enabled(&conn));
    }
}
