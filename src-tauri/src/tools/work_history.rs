//! 工作检索工具：跨会话、圆桌、工作流定义与执行实例的只读查询。
//!
//! 读取约束：
//! - 列表只返回轻量元数据，默认最近 30 天、最多 20 条；
//! - 详情按实体分别设置 `LIMIT`，不复用会加载完整消息/事件流的列表函数；
//! - 用量单独聚合，金额在未配置价格时明确返回未知，不生成估算值；
//! - 不读取 Provider 密钥、共享令牌、附件正文或其他敏感表。

use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::db::init::DbPool;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

pub const SEARCH_WORK_ITEMS: &str = "search_work_items";
pub const READ_WORK_ITEM: &str = "read_work_item";
pub const QUERY_WORK_USAGE: &str = "query_work_usage";

const DEFAULT_SEARCH_DAYS: i64 = 30;
const DEFAULT_SEARCH_LIMIT: usize = 10;
const MAX_SEARCH_LIMIT: usize = 20;
const DEFAULT_USAGE_DAYS: i64 = 30;
const READ_OUTPUT_BUDGET_CHARS: usize = 10_000;
const MAX_DETAIL_STRING_CHARS: usize = 3_000;
const RECENT_SESSION_MESSAGES: i64 = 8;
const RECENT_ROOM_MESSAGES: i64 = 12;

fn arg_string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
            .or_else(|| value.as_str().and_then(|v| v.parse::<i64>().ok()))
    })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn value_text(value: &Value, max_chars: usize) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    truncate_chars(&text, max_chars)
}

fn parse_cursor(cursor: Option<String>) -> usize {
    cursor
        .as_deref()
        .and_then(|value| value.strip_prefix("offset:"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

fn resolve_window(args: &Value, default_days: i64) -> (Option<i64>, Option<i64>, Option<i64>) {
    let explicit_since = arg_i64(args, "since");
    let explicit_until = arg_i64(args, "until");
    let requested_days = arg_i64(args, "days");
    let days = if explicit_since.is_some() || explicit_until.is_some() {
        None
    } else {
        Some(requested_days.unwrap_or(default_days))
    };
    let since = explicit_since.or_else(|| {
        days.filter(|value| *value > 0)
            .map(|value| crate::utils::now() - value * 24 * 3600)
    });
    (since, explicit_until, days)
}

fn project_value(conn: &Connection, project_id: Option<&str>) -> Result<Option<Value>, String> {
    let Some(project_id) = project_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let project = conn
        .query_row(
            "SELECT id, name, icon FROM projects WHERE id = ?1",
            params![project_id],
            |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "name": row.get::<_, String>(1)?,
                    "icon": row.get::<_, String>(2)?,
                }))
            },
        )
        .optional()
        .map_err(|error| format!("读取项目信息失败: {}", error))?;
    Ok(project.or_else(|| {
        Some(json!({
            "id": project_id,
            "name": "项目不可用",
            "icon": "📁",
        }))
    }))
}

fn enabled_status(enabled: i64) -> &'static str {
    if enabled == 1 {
        "enabled"
    } else {
        "disabled"
    }
}

#[derive(Debug, Clone)]
struct WorkHit {
    kind: String,
    id: String,
    title: String,
    subtitle: String,
    status: String,
    updated_at: i64,
    project_id: Option<String>,
    definition_id: Option<String>,
}

impl WorkHit {
    fn sort_key(&self) -> (&str, &str) {
        (&self.kind, &self.id)
    }
}

fn type_requested(args: &Value, kind: &str) -> bool {
    let requested = arg_string(args, "kind").unwrap_or_else(|| "all".to_string());
    requested == "all" || requested == kind || (kind == "workflow_run" && requested == "run")
}

fn push_time_filters(
    clauses: &mut Vec<String>,
    params: &mut Vec<SqlValue>,
    column: &str,
    since: Option<i64>,
    until: Option<i64>,
) {
    if let Some(since) = since {
        clauses.push(format!("{} >= ?", column));
        params.push(SqlValue::Integer(since));
    }
    if let Some(until) = until {
        clauses.push(format!("{} < ?", column));
        params.push(SqlValue::Integer(until));
    }
}

fn push_project_filter(
    clauses: &mut Vec<String>,
    params: &mut Vec<SqlValue>,
    column: &str,
    project_id: Option<&str>,
) {
    if let Some(project_id) = project_id {
        clauses.push(format!("{} = ?", column));
        params.push(SqlValue::Text(project_id.to_string()));
    }
}

fn push_keyword_filter(
    clauses: &mut Vec<String>,
    params: &mut Vec<SqlValue>,
    columns: &[&str],
    keyword: Option<&str>,
) {
    let Some(keyword) = keyword else {
        return;
    };
    let needle = format!("%{}%", keyword);
    let expressions = columns
        .iter()
        .map(|column| format!("COALESCE({}, '') LIKE ?", column))
        .collect::<Vec<_>>();
    clauses.push(format!("({})", expressions.join(" OR ")));
    for _ in columns {
        params.push(SqlValue::Text(needle.clone()));
    }
}

fn search_sessions(conn: &Connection, args: &Value, limit: usize) -> Result<Vec<WorkHit>, String> {
    let mut clauses = vec![
        "status = 'active'".to_string(),
        "COALESCE(origin, '') NOT LIKE 'workflow%'".to_string(),
    ];
    let mut query_params: Vec<SqlValue> = Vec::new();
    push_time_filters(
        &mut clauses,
        &mut query_params,
        "updated_at",
        arg_i64(args, "since"),
        arg_i64(args, "until"),
    );
    push_project_filter(
        &mut clauses,
        &mut query_params,
        "project_id",
        arg_string(args, "project_id").as_deref(),
    );
    push_keyword_filter(
        &mut clauses,
        &mut query_params,
        &["title", "last_message_preview"],
        arg_string(args, "query").as_deref(),
    );
    if let Some(status) = arg_string(args, "status") {
        clauses.push("status = ?".to_string());
        query_params.push(SqlValue::Text(status));
    }
    let sql = format!(
        "SELECT id, title, COALESCE(last_message_preview, ''), status, updated_at, project_id,
                NULL
         FROM sessions
         WHERE {}
         ORDER BY updated_at DESC
         LIMIT ?",
        clauses.join(" AND ")
    );
    query_params.push(SqlValue::Integer(limit as i64));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("准备会话检索失败: {}", error))?;
    let rows = stmt
        .query_map(params_from_iter(query_params), |row| {
            Ok(WorkHit {
                kind: "session".to_string(),
                id: row.get(0)?,
                title: row.get(1)?,
                subtitle: row.get(2)?,
                status: row.get(3)?,
                updated_at: row.get(4)?,
                project_id: row.get(5)?,
                definition_id: row.get(6)?,
            })
        })
        .map_err(|error| format!("检索会话失败: {}", error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取会话检索结果失败: {}", error))
}

fn search_rooms(conn: &Connection, args: &Value, limit: usize) -> Result<Vec<WorkHit>, String> {
    let mut clauses: Vec<String> = Vec::new();
    let mut query_params: Vec<SqlValue> = Vec::new();
    push_time_filters(
        &mut clauses,
        &mut query_params,
        "updated_at",
        arg_i64(args, "since"),
        arg_i64(args, "until"),
    );
    push_project_filter(
        &mut clauses,
        &mut query_params,
        "project_id",
        arg_string(args, "project_id").as_deref(),
    );
    push_keyword_filter(
        &mut clauses,
        &mut query_params,
        &["title", "topic"],
        arg_string(args, "query").as_deref(),
    );
    if let Some(status) = arg_string(args, "status") {
        clauses.push("status = ?".to_string());
        query_params.push(SqlValue::Text(status));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT id, title, topic, status, updated_at, project_id, NULL
         FROM groupchat_rooms
         {}
         ORDER BY updated_at DESC
         LIMIT ?",
        where_sql
    );
    query_params.push(SqlValue::Integer(limit as i64));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("准备圆桌检索失败: {}", error))?;
    let rows = stmt
        .query_map(params_from_iter(query_params), |row| {
            Ok(WorkHit {
                kind: "groupchat".to_string(),
                id: row.get(0)?,
                title: row.get(1)?,
                subtitle: row.get(2)?,
                status: row.get(3)?,
                updated_at: row.get(4)?,
                project_id: row.get(5)?,
                definition_id: row.get(6)?,
            })
        })
        .map_err(|error| format!("检索圆桌失败: {}", error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取圆桌检索结果失败: {}", error))
}

fn search_workflow_definitions(
    conn: &Connection,
    args: &Value,
    limit: usize,
) -> Result<Vec<WorkHit>, String> {
    let mut clauses = vec!["deleted_at IS NULL".to_string()];
    let mut query_params: Vec<SqlValue> = Vec::new();
    push_time_filters(
        &mut clauses,
        &mut query_params,
        "updated_at",
        arg_i64(args, "since"),
        arg_i64(args, "until"),
    );
    push_project_filter(
        &mut clauses,
        &mut query_params,
        "project_id",
        arg_string(args, "project_id").as_deref(),
    );
    push_keyword_filter(
        &mut clauses,
        &mut query_params,
        &["name", "description"],
        arg_string(args, "query").as_deref(),
    );
    if let Some(status) = arg_string(args, "status") {
        clauses.push("CASE WHEN enabled = 1 THEN 'enabled' ELSE 'disabled' END = ?".to_string());
        query_params.push(SqlValue::Text(status));
    }
    let sql = format!(
        "SELECT id, name, description,
                CASE WHEN enabled = 1 THEN 'enabled' ELSE 'disabled' END,
                updated_at, project_id, NULL
         FROM workflow_definitions
         WHERE {}
         ORDER BY updated_at DESC
         LIMIT ?",
        clauses.join(" AND ")
    );
    query_params.push(SqlValue::Integer(limit as i64));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("准备工作流定义检索失败: {}", error))?;
    let rows = stmt
        .query_map(params_from_iter(query_params), |row| {
            Ok(WorkHit {
                kind: "workflow".to_string(),
                id: row.get(0)?,
                title: row.get(1)?,
                subtitle: row.get(2)?,
                status: row.get(3)?,
                updated_at: row.get(4)?,
                project_id: row.get(5)?,
                definition_id: row.get(6)?,
            })
        })
        .map_err(|error| format!("检索工作流定义失败: {}", error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取工作流定义检索结果失败: {}", error))
}

fn search_workflow_runs(
    conn: &Connection,
    args: &Value,
    limit: usize,
) -> Result<Vec<WorkHit>, String> {
    let mut clauses = vec![
        "e.kind = 'execution/created'".to_string(),
        "d.deleted_at IS NULL".to_string(),
    ];
    let mut query_params: Vec<SqlValue> = Vec::new();
    push_time_filters(
        &mut clauses,
        &mut query_params,
        "e.created_at",
        arg_i64(args, "since"),
        arg_i64(args, "until"),
    );
    push_project_filter(
        &mut clauses,
        &mut query_params,
        "d.project_id",
        arg_string(args, "project_id").as_deref(),
    );
    push_keyword_filter(
        &mut clauses,
        &mut query_params,
        &["d.name", "d.description", "e.execution_id"],
        arg_string(args, "query").as_deref(),
    );
    if let Some(status) = arg_string(args, "status") {
        clauses.push(
            "COALESCE(
                (SELECT json_extract(s.payload, '$.status')
                   FROM workflow_events s
                  WHERE s.execution_id = e.execution_id AND s.kind = 'execution/status'
                  ORDER BY s.seq DESC LIMIT 1),
                json_extract(e.payload, '$.status'),
                ''
             ) = ?"
                .to_string(),
        );
        query_params.push(SqlValue::Text(status));
    }
    let sql = format!(
        "SELECT e.execution_id, d.name, e.payload, d.project_id, d.id,
                COALESCE(
                    (SELECT json_extract(s.payload, '$.status')
                       FROM workflow_events s
                      WHERE s.execution_id = e.execution_id AND s.kind = 'execution/status'
                      ORDER BY s.seq DESC LIMIT 1),
                    json_extract(e.payload, '$.status'),
                    ''
                ),
                COALESCE(
                    (SELECT MAX(s.created_at)
                       FROM workflow_events s
                      WHERE s.execution_id = e.execution_id
                        AND s.kind IN ('execution/status', 'execution/progress', 'execution/reset')),
                    e.created_at
                )
         FROM workflow_events e
         JOIN workflow_definitions d
           ON d.id = json_extract(e.payload, '$.definitionId')
         WHERE {}
         ORDER BY e.created_at DESC
         LIMIT ?",
        clauses.join(" AND ")
    );
    query_params.push(SqlValue::Integer(limit as i64));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("准备工作流执行检索失败: {}", error))?;
    let rows = stmt
        .query_map(params_from_iter(query_params), |row| {
            let execution_id: String = row.get(0)?;
            Ok(WorkHit {
                kind: "workflow_run".to_string(),
                id: execution_id.clone(),
                title: row.get(1)?,
                subtitle: format!("执行实例 {}", execution_id),
                status: row.get(5)?,
                updated_at: row.get(6)?,
                project_id: row.get(3)?,
                definition_id: row.get(4)?,
            })
        })
        .map_err(|error| format!("检索工作流执行失败: {}", error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取工作流执行检索结果失败: {}", error))
}

fn search_work_items(conn: &Connection, args: &Value) -> Result<String, String> {
    let limit = arg_i64(args, "limit")
        .unwrap_or(DEFAULT_SEARCH_LIMIT as i64)
        .clamp(1, MAX_SEARCH_LIMIT as i64) as usize;
    let offset = parse_cursor(arg_string(args, "cursor"));
    let fetch_limit = offset.saturating_add(limit).saturating_add(1);
    let (_window_since, _window_until, days) = resolve_window(args, DEFAULT_SEARCH_DAYS);
    let mut effective_args = args.clone();
    if let Some(days) = days {
        effective_args["days"] = json!(days);
    }
    let (since, until, _) = resolve_window(&effective_args, DEFAULT_SEARCH_DAYS);
    effective_args["since"] = since.map(Value::from).unwrap_or(Value::Null);
    effective_args["until"] = until.map(Value::from).unwrap_or(Value::Null);

    let mut hits = Vec::new();
    if type_requested(&effective_args, "session") {
        hits.extend(search_sessions(conn, &effective_args, fetch_limit)?);
    }
    if type_requested(&effective_args, "groupchat") {
        hits.extend(search_rooms(conn, &effective_args, fetch_limit)?);
    }
    if type_requested(&effective_args, "workflow") {
        hits.extend(search_workflow_definitions(
            conn,
            &effective_args,
            fetch_limit,
        )?);
    }
    if type_requested(&effective_args, "workflow_run") {
        hits.extend(search_workflow_runs(conn, &effective_args, fetch_limit)?);
    }

    hits.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.sort_key().cmp(&right.sort_key()))
    });
    hits.dedup_by(|left, right| left.kind == right.kind && left.id == right.id);
    let has_more = hits.len() > offset.saturating_add(limit);
    let page = hits
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();

    let mut items = Vec::with_capacity(page.len());
    for hit in &page {
        items.push(json!({
            "kind": hit.kind,
            "id": hit.id,
            "title": hit.title,
            "subtitle": truncate_chars(&hit.subtitle, 500),
            "status": hit.status,
            "updatedAt": hit.updated_at,
            "project": project_value(conn, hit.project_id.as_deref())?,
            "definitionId": hit.definition_id,
        }));
    }
    let next_cursor = has_more.then(|| format!("offset:{}", offset + limit));
    serde_json::to_string_pretty(&json!({
        "items": items,
        "count": page.len(),
        "nextCursor": next_cursor,
        "hasMore": has_more,
        "window": {
            "since": since,
            "until": until,
            "days": days,
        },
        "note": "仅返回轻量元数据；读取详情请调用 read_work_item。",
    }))
    .map_err(|error| format!("序列化工作检索结果失败: {}", error))
}

fn usage_cost_value() -> Value {
    json!({
        "estimatedCost": null,
        "currency": null,
        "status": "pricing_not_configured",
        "note": "未配置模型价格，不提供金额估算。",
    })
}

fn session_titles(conn: &Connection) -> HashMap<String, (String, Option<String>, String)> {
    let mut map = HashMap::new();
    if let Ok(mut stmt) = conn
        .prepare("SELECT id, COALESCE(title, ''), project_id, COALESCE(origin, '') FROM sessions")
    {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ),
            ))
        }) {
            for row in rows.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

fn room_titles(conn: &Connection) -> HashMap<String, (String, Option<String>)> {
    let mut map = HashMap::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT id, COALESCE(title, ''), project_id FROM groupchat_rooms")
    {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?),
            ))
        }) {
            for row in rows.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

fn workflow_titles(conn: &Connection) -> HashMap<String, (String, Option<String>)> {
    let mut map = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT id, COALESCE(name, ''), project_id
         FROM workflow_definitions WHERE deleted_at IS NULL",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?),
            ))
        }) {
            for row in rows.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

fn participant_names(conn: &Connection) -> HashMap<(String, String), String> {
    let mut map = HashMap::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT room_id, id, COALESCE(display_name, '') FROM groupchat_participants")
    {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                row.get::<_, String>(2)?,
            ))
        }) {
            for row in rows.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

#[derive(Debug, Clone, Default)]
struct UsageTotals {
    call_count: i64,
    prompt_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
    cached_tokens: i64,
}

impl UsageTotals {
    fn add(&mut self, row: &UsageRow) {
        self.call_count = self.call_count.saturating_add(row.call_count);
        self.prompt_tokens = self.prompt_tokens.saturating_add(row.prompt_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(row.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(row.cache_write_tokens);
        self.completion_tokens = self.completion_tokens.saturating_add(row.completion_tokens);
        self.total_tokens = self.total_tokens.saturating_add(row.total_tokens);
        self.cached_tokens = self
            .cached_tokens
            .saturating_add(row.cache_read_tokens + row.cache_write_tokens);
    }

    fn cache_hit_rate(&self) -> f64 {
        let denominator = self.prompt_tokens + self.cache_read_tokens + self.cache_write_tokens;
        if denominator > 0 {
            self.cache_read_tokens as f64 / denominator as f64 * 100.0
        } else {
            0.0
        }
    }

    fn as_value(&self) -> Value {
        json!({
            "callCount": self.call_count,
            "promptTokens": self.prompt_tokens,
            "cacheReadTokens": self.cache_read_tokens,
            "cacheWriteTokens": self.cache_write_tokens,
            "completionTokens": self.completion_tokens,
            "totalTokens": self.total_tokens,
            "cachedTokens": self.cached_tokens,
            "cacheHitRate": self.cache_hit_rate(),
        })
    }
}

#[derive(Debug, Clone)]
struct UsageRow {
    scope: String,
    provider: String,
    model: String,
    channel: String,
    date: String,
    call_count: i64,
    prompt_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
}

#[derive(Debug, Clone)]
struct AttributedWork {
    kind: String,
    id: String,
    name: String,
    project_id: Option<String>,
    participant: String,
}

#[derive(Debug, Clone)]
struct AttributedUsage {
    work: AttributedWork,
    provider: String,
    model: String,
    channel: String,
    date: String,
    call_count: i64,
    prompt_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
}

fn load_usage_rows(
    conn: &Connection,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<Vec<UsageRow>, String> {
    let mut clauses: Vec<String> = Vec::new();
    let mut query_params: Vec<SqlValue> = Vec::new();
    push_time_filters(&mut clauses, &mut query_params, "created_at", since, until);
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT session_id, COALESCE(provider, ''), COALESCE(model, ''),
                COALESCE(channel, 'api'),
                DATE(created_at, 'unixepoch', 'localtime'),
                COUNT(*),
                COALESCE(SUM(prompt_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COALESCE(SUM(cache_write_tokens), 0),
                COALESCE(SUM(completion_tokens), 0),
                COALESCE(SUM(total_tokens), 0)
         FROM api_usage_log
         {}
         GROUP BY session_id, provider, model, channel,
                  DATE(created_at, 'unixepoch', 'localtime')",
        where_sql
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("准备用量查询失败: {}", error))?;
    let rows = stmt
        .query_map(params_from_iter(query_params), |row| {
            Ok(UsageRow {
                scope: row.get(0)?,
                provider: row.get(1)?,
                model: row.get(2)?,
                channel: row.get(3)?,
                date: row.get(4)?,
                call_count: row.get(5)?,
                prompt_tokens: row.get(6)?,
                cache_read_tokens: row.get(7)?,
                cache_write_tokens: row.get(8)?,
                completion_tokens: row.get(9)?,
                total_tokens: row.get(10)?,
            })
        })
        .map_err(|error| format!("执行用量查询失败: {}", error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取用量结果失败: {}", error))
}

fn attribute_usage(
    rows: Vec<UsageRow>,
    sessions: &HashMap<String, (String, Option<String>, String)>,
    rooms: &HashMap<String, (String, Option<String>)>,
    workflows: &HashMap<String, (String, Option<String>)>,
    participants: &HashMap<(String, String), String>,
) -> Vec<AttributedUsage> {
    let mut attributed = Vec::new();
    for row in rows {
        let work = if let Some(rest) = row.scope.strip_prefix("groupchat:") {
            let mut parts = rest.splitn(2, ':');
            let room_id = parts.next().unwrap_or("");
            let participant_id = parts.next().unwrap_or("");
            let Some((title, project_id)) = rooms.get(room_id) else {
                continue;
            };
            let participant = if participant_id.is_empty() {
                "主持人".to_string()
            } else {
                participants
                    .get(&(room_id.to_string(), participant_id.to_string()))
                    .filter(|name| !name.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| participant_id.to_string())
            };
            AttributedWork {
                kind: "groupchat".to_string(),
                id: room_id.to_string(),
                name: if title.trim().is_empty() {
                    room_id.to_string()
                } else {
                    title.clone()
                },
                project_id: project_id.clone(),
                participant,
            }
        } else if row.scope.starts_with("kb:") {
            // 工作用量只覆盖会话/圆桌/工作流；知识库用量由知识库自身统计。
            continue;
        } else if let Some((title, project_id, origin)) = sessions.get(&row.scope) {
            if let Some(definition_id) = origin.strip_prefix("workflow:") {
                let Some((workflow_name, workflow_project_id)) = workflows.get(definition_id)
                else {
                    continue;
                };
                AttributedWork {
                    kind: "workflow".to_string(),
                    id: definition_id.to_string(),
                    name: if workflow_name.trim().is_empty() {
                        definition_id.to_string()
                    } else {
                        workflow_name.clone()
                    },
                    project_id: workflow_project_id.clone(),
                    participant: "工作流节点".to_string(),
                }
            } else if origin.starts_with("workflow") {
                continue;
            } else {
                AttributedWork {
                    kind: "session".to_string(),
                    id: row.scope.clone(),
                    name: if title.trim().is_empty() {
                        row.scope.clone()
                    } else {
                        title.clone()
                    },
                    project_id: project_id.clone(),
                    participant: "会话直连".to_string(),
                }
            }
        } else {
            continue;
        };
        attributed.push(AttributedUsage {
            work,
            provider: if row.provider.trim().is_empty() {
                "未知".to_string()
            } else {
                row.provider
            },
            model: if row.model.trim().is_empty() {
                "未知".to_string()
            } else {
                row.model
            },
            channel: row.channel,
            date: row.date,
            call_count: row.call_count,
            prompt_tokens: row.prompt_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_write_tokens,
            completion_tokens: row.completion_tokens,
            total_tokens: row.total_tokens,
        });
    }
    attributed
}

#[derive(Debug, Clone)]
struct UsageQuery {
    work_type: String,
    work_id: Option<String>,
    project_id: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    participant: Option<String>,
    group_by: String,
}

impl UsageQuery {
    fn from_args(args: &Value) -> Self {
        Self {
            work_type: arg_string(args, "work_type").unwrap_or_else(|| "all".to_string()),
            work_id: arg_string(args, "work_id"),
            project_id: arg_string(args, "project_id"),
            model: arg_string(args, "model"),
            provider: arg_string(args, "provider"),
            participant: arg_string(args, "participant"),
            group_by: arg_string(args, "group_by").unwrap_or_else(|| "work_item".to_string()),
        }
    }

    fn matches(&self, row: &AttributedUsage) -> bool {
        if self.work_type != "all"
            && self.work_type != row.work.kind
            && !(self.work_type == "run" && row.work.kind == "workflow")
        {
            return false;
        }
        if let Some(work_id) = &self.work_id {
            if work_id != &row.work.id {
                return false;
            }
        }
        if let Some(project_id) = &self.project_id {
            if row.work.project_id.as_deref() != Some(project_id.as_str()) {
                return false;
            }
        }
        if let Some(model) = &self.model {
            if model != &row.model {
                return false;
            }
        }
        if let Some(provider) = &self.provider {
            if provider != &row.provider {
                return false;
            }
        }
        if let Some(participant) = &self.participant {
            let needle = participant.to_lowercase();
            if !row.work.participant.to_lowercase().contains(&needle) {
                return false;
            }
        }
        true
    }

    fn group_name(&self, row: &AttributedUsage) -> (String, String) {
        match self.group_by.as_str() {
            "model" => (row.model.clone(), row.model.clone()),
            "provider" => (row.provider.clone(), row.provider.clone()),
            "participant" => (
                format!("{}:{}", row.work.kind, row.work.participant),
                row.work.participant.clone(),
            ),
            "date" => (row.date.clone(), row.date.clone()),
            _ => (
                format!("{}:{}", row.work.kind, row.work.id),
                row.work.name.clone(),
            ),
        }
    }
}

fn row_totals(row: &AttributedUsage) -> UsageRow {
    UsageRow {
        scope: row.work.id.clone(),
        provider: row.provider.clone(),
        model: row.model.clone(),
        channel: row.channel.clone(),
        date: row.date.clone(),
        call_count: row.call_count,
        prompt_tokens: row.prompt_tokens,
        cache_read_tokens: row.cache_read_tokens,
        cache_write_tokens: row.cache_write_tokens,
        completion_tokens: row.completion_tokens,
        total_tokens: row.total_tokens,
    }
}

fn aggregate_usage(conn: &Connection, args: &Value, query: &UsageQuery) -> Result<Value, String> {
    let (since, until, days) = resolve_window(args, DEFAULT_USAGE_DAYS);
    let rows = load_usage_rows(conn, since, until)?;
    let sessions = session_titles(conn);
    let rooms = room_titles(conn);
    let workflows = workflow_titles(conn);
    let participants = participant_names(conn);
    let attributed = attribute_usage(rows, &sessions, &rooms, &workflows, &participants);

    let mut totals = UsageTotals::default();
    let mut grouped: BTreeMap<String, (String, UsageTotals)> = BTreeMap::new();
    let mut matched = 0usize;
    for row in attributed.iter().filter(|row| query.matches(row)) {
        matched += 1;
        let single = row_totals(row);
        totals.add(&single);
        let (key, name) = query.group_name(row);
        grouped
            .entry(key)
            .or_insert_with(|| (name, UsageTotals::default()))
            .1
            .add(&single);
    }
    let mut groups = grouped
        .into_values()
        .map(|(name, totals)| {
            json!({
                "name": name,
                "totals": totals.as_value(),
            })
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        let left_total = left["totals"]["totalTokens"].as_i64().unwrap_or(0);
        let right_total = right["totals"]["totalTokens"].as_i64().unwrap_or(0);
        right_total
            .cmp(&left_total)
            .then_with(|| left["name"].as_str().cmp(&right["name"].as_str()))
    });
    if query.group_by == "date" {
        groups.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    }
    let scope_note = if query.work_type == "workflow" || query.work_type == "run" {
        "工作流用量按工作流定义归因，不能准确拆分到单次执行。"
    } else {
        "工作流节点用量归入其工作流定义；普通会话与圆桌按各自工作项归因。"
    };
    Ok(json!({
        "window": {
            "since": since,
            "until": until,
            "days": days,
        },
        "filters": {
            "workType": query.work_type,
            "workId": query.work_id,
            "projectId": query.project_id,
            "model": query.model,
            "provider": query.provider,
            "participant": query.participant,
        },
        "groupBy": query.group_by,
        "totals": totals.as_value(),
        "groups": groups,
        "matchedBuckets": matched,
        "cost": usage_cost_value(),
        "attributionNote": scope_note,
    }))
}

fn query_work_usage(conn: &Connection, args: &Value) -> Result<String, String> {
    let query = UsageQuery::from_args(args);
    let value = aggregate_usage(conn, args, &query)?;
    serde_json::to_string_pretty(&value).map_err(|error| format!("序列化用量统计失败: {}", error))
}

fn details_root(
    conn: &Connection,
    kind: &str,
    id: &str,
    title: &str,
    status: &str,
    updated_at: i64,
    project_id: Option<&str>,
    details: Value,
) -> Result<Value, String> {
    Ok(json!({
        "kind": kind,
        "id": id,
        "title": title,
        "status": status,
        "updatedAt": updated_at,
        "project": project_value(conn, project_id)?,
        "details": details,
    }))
}

fn read_session(conn: &Connection, id: &str) -> Result<Value, String> {
    let row = conn
        .query_row(
            "SELECT id, title, COALESCE(status, 'active'), updated_at, created_at,
                    COALESCE(agent_type, ''), COALESCE(api_provider, ''),
                    COALESCE(api_model, ''), COALESCE(last_message_preview, ''),
                    COALESCE(message_count, 0), project_id, COALESCE(origin, '')
             FROM sessions WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("读取会话失败: {}", error))?
        .ok_or_else(|| format!("会话不存在: {}", id))?;

    let summary = crate::eventlog::latest_session_summary(conn, id)
        .map_err(|error| format!("读取会话摘要失败: {}", error))?;
    let mut recent_messages = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT seq, kind, payload, created_at
                 FROM session_events
                 WHERE session_id = ?1 AND kind NOT IN ('summary/result', 'todo/state')
                 ORDER BY seq DESC LIMIT ?2",
            )
            .map_err(|error| format!("准备会话消息读取失败: {}", error))?;
        let rows = stmt
            .query_map(params![id, RECENT_SESSION_MESSAGES], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|error| format!("执行会话消息读取失败: {}", error))?;
        for row in rows.flatten() {
            let payload = serde_json::from_str::<Value>(&row.2).unwrap_or(Value::Null);
            recent_messages.push(json!({
                "seq": row.0,
                "eventKind": row.1,
                "role": payload.get("role").and_then(Value::as_str).unwrap_or(""),
                "content": truncate_chars(
                    payload.get("content").and_then(Value::as_str).unwrap_or(""),
                    1_200
                ),
                "createdAt": row.3,
            }));
        }
        recent_messages.reverse();
    }
    let details = json!({
        "source": {
            "agentType": row.4,
            "provider": row.5,
            "model": row.6,
            "origin": row.10,
        },
        "createdAt": row.3,
        "messageCount": row.8,
        "lastMessagePreview": truncate_chars(&row.7, 1_000),
        "summary": summary.as_deref().map(|value| truncate_chars(value, 3_000)),
        "recentMessages": recent_messages,
    });
    details_root(
        conn,
        "session",
        id,
        &row.0,
        &row.1,
        row.2,
        row.9.as_deref(),
        details,
    )
}

fn read_groupchat(conn: &Connection, id: &str) -> Result<Value, String> {
    let room = crate::groupchat::store::get_room(conn, id)
        .map_err(|error| format!("读取圆桌失败: {}", error))?
        .ok_or_else(|| format!("圆桌不存在: {}", id))?;
    let participants = crate::groupchat::store::list_participants(conn, id)
        .map_err(|error| format!("读取圆桌参与者失败: {}", error))?
        .into_iter()
        .take(24)
        .map(|participant| {
            json!({
                "id": participant.id,
                "type": participant.participant_type,
                "displayName": participant.display_name,
                "role": truncate_chars(&participant.system_role, 500),
                "status": participant.status,
            })
        })
        .collect::<Vec<_>>();
    let tasks = crate::groupchat::store::list_tasks(conn, id)
        .map_err(|error| format!("读取圆桌任务失败: {}", error))?
        .into_iter()
        .take(24)
        .map(|task| {
            json!({
                "id": task.id,
                "taskNo": task.task_no,
                "description": truncate_chars(&task.description, 800),
                "assignee": task.assignee,
                "status": task.status,
                "resultSummary": task.result_summary.as_deref().map(|v| truncate_chars(v, 1_000)),
                "error": task.error.as_deref().map(|v| truncate_chars(v, 1_000)),
            })
        })
        .collect::<Vec<_>>();

    let mut messages = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT payload FROM room_events
                 WHERE room_id = ?1 AND kind = 'message'
                 ORDER BY seq DESC LIMIT ?2",
            )
            .map_err(|error| format!("准备圆桌消息读取失败: {}", error))?;
        let rows = stmt
            .query_map(params![id, RECENT_ROOM_MESSAGES], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| format!("执行圆桌消息读取失败: {}", error))?;
        for payload in rows.flatten() {
            if let Ok(message) =
                serde_json::from_str::<crate::groupchat::models::MessageRow>(&payload)
            {
                messages.push(json!({
                    "seq": message.seq,
                    "round": message.round,
                    "sender": message.sender,
                    "kind": message.kind,
                    "content": truncate_chars(&message.content, 1_000),
                    "timestamp": message.timestamp,
                }));
            }
        }
        messages.reverse();
    }
    let details = json!({
        "topic": truncate_chars(&room.topic, 2_000),
        "strategy": room.strategy,
        "maxRounds": room.max_rounds,
        "maxParallel": room.max_parallel,
        "allowAutoCli": room.allow_auto_cli == 1,
        "createdAt": room.created_at,
        "participants": participants,
        "tasks": tasks,
        "recentMessages": messages,
    });
    details_root(
        conn,
        "groupchat",
        id,
        &room.title,
        &room.status,
        room.updated_at,
        room.project_id.as_deref(),
        details,
    )
}

fn recent_workflow_runs(
    conn: &Connection,
    definition_id: &str,
    limit: i64,
) -> Result<Vec<Value>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT e.execution_id,
                    COALESCE(
                        (SELECT json_extract(s.payload, '$.status')
                           FROM workflow_events s
                          WHERE s.execution_id = e.execution_id AND s.kind = 'execution/status'
                          ORDER BY s.seq DESC LIMIT 1),
                        json_extract(e.payload, '$.status'),
                        ''
                    ),
                    COALESCE(
                        (SELECT json_extract(s.payload, '$.completionRate')
                           FROM workflow_events s
                          WHERE s.execution_id = e.execution_id
                            AND s.kind IN ('execution/status', 'execution/progress')
                          ORDER BY s.seq DESC LIMIT 1),
                        0
                    ),
                    COALESCE(
                        (SELECT MAX(s.created_at)
                           FROM workflow_events s
                          WHERE s.execution_id = e.execution_id),
                        e.created_at
                    ),
                    COALESCE(
                        (SELECT json_extract(s.payload, '$.errorMessage')
                           FROM workflow_events s
                          WHERE s.execution_id = e.execution_id AND s.kind = 'execution/status'
                          ORDER BY s.seq DESC LIMIT 1),
                        ''
                    )
             FROM workflow_events e
             WHERE e.kind = 'execution/created'
               AND json_extract(e.payload, '$.definitionId') = ?1
             ORDER BY e.created_at DESC
             LIMIT ?2",
        )
        .map_err(|error| format!("准备最近执行读取失败: {}", error))?;
    let rows = stmt
        .query_map(params![definition_id, limit], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "status": row.get::<_, String>(1)?,
                "completionRate": row.get::<_, f64>(2)?,
                "updatedAt": row.get::<_, i64>(3)?,
                "error": truncate_chars(&row.get::<_, String>(4)?, 1_000),
            }))
        })
        .map_err(|error| format!("执行最近执行读取失败: {}", error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取最近执行结果失败: {}", error))
}

fn workflow_definition_details(conn: &Connection, id: &str) -> Result<Value, String> {
    let definition = crate::workflow::get_definition(conn, id)
        .map_err(|error| format!("读取工作流定义失败: {}", error))?
        .ok_or_else(|| format!("工作流定义不存在: {}", id))?;
    let mut node_count = 0usize;
    let mut node_type_counts: BTreeMap<String, usize> = BTreeMap::new();
    let stages = definition
        .stages
        .iter()
        .map(|stage| {
            node_count += stage.nodes.len();
            for node in &stage.nodes {
                let node_type = serde_json::to_value(&node.node_type)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_else(|| "unknown".to_string());
                *node_type_counts.entry(node_type).or_default() += 1;
            }
            json!({
                "id": stage.id,
                "name": stage.name,
                "order": stage.order,
                "nodeCount": stage.nodes.len(),
                "gateStrategy": format!("{:?}", stage.gate.strategy),
            })
        })
        .collect::<Vec<_>>();
    let trigger = serde_json::to_value(&definition.trigger).unwrap_or(Value::Null);
    let recent_runs = recent_workflow_runs(conn, id, 5)?;
    let details = json!({
        "version": definition.version,
        "description": truncate_chars(&definition.description, 2_000),
        "trigger": trigger,
        "stageCount": definition.stages.len(),
        "nodeCount": node_count,
        "nodeTypes": node_type_counts,
        "stages": stages,
        "recentRuns": recent_runs,
    });
    details_root(
        conn,
        "workflow",
        id,
        &definition.name,
        enabled_status(i64::from(definition.enabled)),
        definition.updated_at,
        definition.project_id.as_deref(),
        details,
    )
}

fn read_workflow_run(conn: &Connection, id: &str) -> Result<Value, String> {
    let instance = crate::workflow::events::derive_instance(conn, id)
        .map_err(|error| format!("读取工作流执行失败: {}", error))?
        .ok_or_else(|| format!("工作流执行实例不存在: {}", id))?;
    let statuses = crate::workflow::events::derive_node_statuses(conn, id)
        .map_err(|error| format!("读取工作流节点状态失败: {}", error))?;
    let definition = crate::workflow::get_definition(conn, &instance.definition_id)
        .ok()
        .flatten();
    let labels = definition
        .as_ref()
        .map(|definition| {
            definition
                .stages
                .iter()
                .flat_map(|stage| stage.nodes.iter())
                .map(|node| (node.id.clone(), node.label.clone()))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let node_states = statuses
        .into_iter()
        .take(40)
        .map(|(node_id, status)| {
            json!({
                "nodeId": node_id,
                "label": labels.get(&node_id),
                "status": status,
            })
        })
        .collect::<Vec<_>>();
    let context_keys = instance
        .context
        .as_object()
        .map(|object| object.keys().take(30).cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let output_preview = instance
        .output
        .as_ref()
        .map(|value| value_text(value, 4_000));
    let project_id = definition
        .as_ref()
        .and_then(|definition| definition.project_id.as_deref());
    let details = json!({
        "definitionId": instance.definition_id,
        "definitionName": instance.definition_name,
        "trigger": instance.trigger,
        "triggerDetail": instance.trigger_detail,
        "startedAt": instance.started_at,
        "completedAt": instance.completed_at,
        "completionRate": instance.completion_rate,
        "skippedCount": instance.skipped_count,
        "error": instance.error.as_deref().map(|value| truncate_chars(value, 2_000)),
        "contextKeys": context_keys,
        "outputSource": instance.output_source,
        "outputNodeLabel": instance.output_node_label,
        "outputPreview": output_preview,
        "nodeStates": node_states,
    });
    details_root(
        conn,
        "workflow_run",
        id,
        &instance.definition_name,
        crate::workflow::events::status_code(&instance.status),
        instance.created_at,
        project_id,
        details,
    )
}

fn usage_scope_for(kind: &str, id: &str) -> (String, String) {
    match kind {
        "session" => ("session".to_string(), id.to_string()),
        "groupchat" => ("groupchat".to_string(), id.to_string()),
        "workflow" => ("workflow".to_string(), id.to_string()),
        "workflow_run" => ("workflow".to_string(), id.to_string()),
        _ => (kind.to_string(), id.to_string()),
    }
}

fn attach_usage(conn: &Connection, mut root: Value, kind: &str, id: &str) -> Result<Value, String> {
    let definition_id = if kind == "workflow_run" {
        root.get("details")
            .and_then(|details| details.get("definitionId"))
            .and_then(Value::as_str)
            .unwrap_or(id)
    } else {
        id
    };
    let (work_type, work_id) = usage_scope_for(kind, definition_id);
    let query = UsageQuery {
        work_type,
        work_id: Some(work_id),
        project_id: None,
        model: None,
        provider: None,
        participant: None,
        group_by: "work_item".to_string(),
    };
    let aggregate = aggregate_usage(conn, &json!({"days": 0}), &query)?;
    root["usage"] = json!({
        "scope": if kind == "workflow_run" { "workflow_definition" } else { kind },
        "totals": aggregate.get("totals").cloned().unwrap_or(Value::Null),
        "cost": aggregate.get("cost").cloned().unwrap_or_else(usage_cost_value),
        "note": if kind == "workflow_run" {
            "工作流用量按定义汇总，不能准确拆分到单次执行。"
        } else {
            "统计范围为全部历史记录。"
        },
    });
    Ok(root)
}

fn shrink_json_strings(value: &mut Value) {
    match value {
        Value::String(text) if text.chars().count() > MAX_DETAIL_STRING_CHARS => {
            *text = truncate_chars(text, MAX_DETAIL_STRING_CHARS);
        }
        Value::Array(items) => {
            for item in items {
                shrink_json_strings(item);
            }
        }
        Value::Object(object) => {
            for item in object.values_mut() {
                shrink_json_strings(item);
            }
        }
        _ => {}
    }
}

fn serialize_detail_with_budget(mut value: Value) -> Result<String, String> {
    shrink_json_strings(&mut value);
    let mut body = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("序列化工作详情失败: {}", error))?;
    if body.chars().count() <= READ_OUTPUT_BUDGET_CHARS {
        return Ok(body);
    }

    let meta = json!({
        "kind": value.get("kind").cloned().unwrap_or(Value::Null),
        "id": value.get("id").cloned().unwrap_or(Value::Null),
        "title": value.get("title").cloned().unwrap_or(Value::Null),
        "status": value.get("status").cloned().unwrap_or(Value::Null),
        "updatedAt": value.get("updatedAt").cloned().unwrap_or(Value::Null),
        "project": value.get("project").cloned().unwrap_or(Value::Null),
        "budgetTruncated": true,
        "preview": truncate_chars(&body, READ_OUTPUT_BUDGET_CHARS.saturating_sub(1_200)),
    });
    body = serde_json::to_string_pretty(&meta)
        .map_err(|error| format!("序列化工作详情预览失败: {}", error))?;
    Ok(truncate_chars(&body, READ_OUTPUT_BUDGET_CHARS))
}

fn read_work_item(conn: &Connection, args: &Value) -> Result<String, String> {
    let requested_kind = arg_string(args, "kind")
        .ok_or_else(|| "缺少参数 kind（session/groupchat/workflow/workflow_run）".to_string())?;
    let kind = match requested_kind.as_str() {
        "session" => "session",
        "groupchat" | "room" => "groupchat",
        "workflow" => "workflow",
        "workflow_run" | "workflow_instance" | "run" => "workflow_run",
        _ => {
            return Err(format!(
                "不支持的工作类型: {}；可用 session/groupchat/workflow/workflow_run",
                requested_kind
            ))
        }
    };
    let id = arg_string(args, "id").ok_or_else(|| "缺少参数 id".to_string())?;
    let mut value = match kind {
        "session" => read_session(conn, &id)?,
        "groupchat" => read_groupchat(conn, &id)?,
        "workflow" => workflow_definition_details(conn, &id)?,
        "workflow_run" => read_workflow_run(conn, &id)?,
        _ => unreachable!(),
    };
    if args
        .get("include_usage")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        value = attach_usage(conn, value, kind, &id)?;
    }
    serialize_detail_with_budget(value)
}

pub struct SearchWorkItemsTool {
    pool: DbPool,
}

impl SearchWorkItemsTool {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ToolHandler for SearchWorkItemsTool {
    fn name(&self) -> &str {
        SEARCH_WORK_ITEMS
    }

    fn description(&self) -> &str {
        "只读检索最近的会话、圆桌、工作流定义和执行实例。支持关键词、项目、类型、状态、时间范围和游标分页；默认最近 30 天、10 条，最多 20 条，只返回轻量元数据，不返回全文。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "标题、主题、描述或执行 ID 的模糊关键词"},
                "kind": {
                    "type": "string",
                    "enum": ["all", "session", "groupchat", "workflow", "workflow_run"],
                    "description": "工作类型；默认 all"
                },
                "status": {"type": "string", "description": "可选状态过滤，如 active/running/success/failed/enabled"},
                "project_id": {"type": "string", "description": "可选项目 ID"},
                "days": {"type": "integer", "minimum": 0, "description": "最近天数，默认 30；0 表示不限"},
                "since": {"type": "integer", "description": "可选 Unix 秒时间下界（含）"},
                "until": {"type": "integer", "description": "可选 Unix 秒时间上界（不含）"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 20, "description": "返回条数，默认 10"},
                "cursor": {"type": "string", "description": "上次结果的 nextCursor"}
            },
            "additionalProperties": false
        })
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Read]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let conn = self
            .pool
            .get()
            .map_err(|error| format!("获取数据库连接失败: {}", error))?;
        search_work_items(&conn, &arguments)
    }
}

pub struct ReadWorkItemTool {
    pool: DbPool,
}

impl ReadWorkItemTool {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ToolHandler for ReadWorkItemTool {
    fn name(&self) -> &str {
        READ_WORK_ITEM
    }

    fn description(&self) -> &str {
        "只读读取单个会话、圆桌、工作流定义或执行实例的紧凑详情。会话优先返回摘要并补最近消息；圆桌返回任务状态和最近讨论；工作流返回最近执行、错误和产出预览。输出上限约 10000 字符。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["session", "groupchat", "workflow", "workflow_run"],
                    "description": "工作类型"
                },
                "id": {"type": "string", "description": "工作项 ID"},
                "include_usage": {
                    "type": "boolean",
                    "description": "是否附带该工作的历史 Token 用量快照；默认 false"
                }
            },
            "required": ["kind", "id"],
            "additionalProperties": false
        })
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Read, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let conn = self
            .pool
            .get()
            .map_err(|error| format!("获取数据库连接失败: {}", error))?;
        read_work_item(&conn, &arguments)
    }
}

pub struct QueryWorkUsageTool {
    pool: DbPool,
}

impl QueryWorkUsageTool {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ToolHandler for QueryWorkUsageTool {
    fn name(&self) -> &str {
        QUERY_WORK_USAGE
    }

    fn description(&self) -> &str {
        "只读聚合会话、圆桌和工作流的 Token 用量。支持按工作项、模型、Provider、参与者或日期分组，以及项目、工作、模型、Provider、参与者和时间过滤。未配置模型价格时金额明确返回未知；工作流只能按定义聚合，不能准确拆分到单次执行。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "days": {"type": "integer", "minimum": 0, "description": "最近天数，默认 30；0 表示全部历史"},
                "since": {"type": "integer", "description": "可选 Unix 秒时间下界（含）"},
                "until": {"type": "integer", "description": "可选 Unix 秒时间上界（不含）"},
                "work_type": {
                    "type": "string",
                    "enum": ["all", "session", "groupchat", "workflow"],
                    "description": "工作类型；默认 all"
                },
                "work_id": {"type": "string", "description": "可选工作项 ID；工作流使用定义 ID"},
                "project_id": {"type": "string", "description": "可选项目 ID"},
                "model": {"type": "string", "description": "可选模型名"},
                "provider": {"type": "string", "description": "可选 Provider 名或 ID"},
                "participant": {"type": "string", "description": "可选圆桌参与者显示名或 ID"},
                "group_by": {
                    "type": "string",
                    "enum": ["work_item", "model", "provider", "participant", "date"],
                    "description": "分组维度，默认 work_item"
                }
            },
            "additionalProperties": false
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Read, ToolTag::HighCost]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let conn = self
            .pool
            .get()
            .map_err(|error| format!("获取数据库连接失败: {}", error))?;
        query_work_usage(&conn, &arguments)
    }
}

/// 构造工作检索工具目录；运行期与工具管理目录共用，避免两份元数据漂移。
pub fn tools(pool: DbPool) -> Vec<std::sync::Arc<dyn ToolHandler>> {
    vec![
        std::sync::Arc::new(SearchWorkItemsTool::new(pool.clone())),
        std::sync::Arc::new(ReadWorkItemTool::new(pool.clone())),
        std::sync::Arc::new(QueryWorkUsageTool::new(pool)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{is_default_disabled, ToolProfile};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, icon TEXT NOT NULL DEFAULT '📁',
                archived_at INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
             );
             CREATE TABLE sessions (
                id TEXT PRIMARY KEY, agent_type TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                last_message_preview TEXT DEFAULT '', message_count INTEGER DEFAULT 0,
                status TEXT DEFAULT 'active', api_provider TEXT, api_model TEXT,
                agent_session_id TEXT, origin TEXT, project_id TEXT
             );
             CREATE TABLE groupchat_rooms (
                id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', topic TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'idle', strategy TEXT NOT NULL DEFAULT 'round_robin',
                max_rounds INTEGER NOT NULL DEFAULT 5, max_parallel INTEGER NOT NULL DEFAULT 2,
                director_id TEXT, current_task_id TEXT, created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL, goal_notes TEXT NOT NULL DEFAULT '[]',
                allow_auto_cli INTEGER NOT NULL DEFAULT 1, project_id TEXT
             );
             CREATE TABLE groupchat_participants (
                id TEXT NOT NULL, room_id TEXT NOT NULL, participant_type TEXT NOT NULL,
                agent_config TEXT NOT NULL DEFAULT '{}', display_name TEXT NOT NULL DEFAULT '',
                system_role TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'active',
                PRIMARY KEY (room_id, id)
             );
             CREATE TABLE workflow_definitions (
                id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '', version TEXT NOT NULL DEFAULT '1.0.0',
                description TEXT NOT NULL DEFAULT '', trigger TEXT NOT NULL DEFAULT '{}',
                stages TEXT NOT NULL DEFAULT '[]', input_schema TEXT, output_schema TEXT, icon TEXT,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1, deleted_at INTEGER, project_id TEXT
             );
             CREATE TABLE workflow_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT, execution_id TEXT NOT NULL,
                kind TEXT NOT NULL, payload TEXT NOT NULL DEFAULT '{}',
                model_visible INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL
             );
             CREATE TABLE session_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
                kind TEXT NOT NULL, payload TEXT NOT NULL DEFAULT '{}',
                model_visible INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL
             );
             CREATE TABLE room_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT, room_id TEXT NOT NULL,
                kind TEXT NOT NULL, payload TEXT NOT NULL DEFAULT '{}',
                model_visible INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL
             );
             CREATE TABLE api_usage_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT '', model TEXT NOT NULL DEFAULT '',
                api_format TEXT NOT NULL DEFAULT '', prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0, total_tokens INTEGER NOT NULL DEFAULT 0,
                cached_tokens INTEGER NOT NULL DEFAULT 0, cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0, channel TEXT NOT NULL DEFAULT 'api',
                created_at INTEGER NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn search_filters_project_keyword_and_returns_cursor() {
        let conn = test_conn();
        let now = crate::utils::now();
        conn.execute(
            "INSERT INTO projects VALUES ('p1', 'Alpha', '📁', NULL, ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions
             (id, agent_type, title, created_at, updated_at, last_message_preview, message_count,
              status, origin, project_id)
             VALUES ('s1', 'api', 'Alpha 方案整理', ?1, ?2, '已完成接口设计', 3, 'active', NULL, 'p1')",
            params![now - 10, now - 1],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions
             (id, agent_type, title, created_at, updated_at, last_message_preview, message_count,
              status, origin, project_id)
             VALUES ('s2', 'api', 'Beta 无关工作', ?1, ?2, '另一项目', 1, 'active', NULL, NULL)",
            params![now - 10, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions
             (id, agent_type, title, created_at, updated_at, last_message_preview, message_count,
              status, origin, project_id)
             VALUES ('s3', 'api', 'Alpha 接口复核', ?1, ?2, '等待复核', 2, 'active', NULL, 'p1')",
            params![now - 20, now - 2],
        )
        .unwrap();

        let result = search_work_items(
            &conn,
            &json!({"query": "接口", "project_id": "p1", "limit": 1}),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["count"], 1);
        assert_eq!(value["items"][0]["id"], "s1");
        assert!(value["nextCursor"].as_str().is_some());

        let next = search_work_items(
            &conn,
            &json!({
                "query": "接口",
                "project_id": "p1",
                "limit": 1,
                "cursor": value["nextCursor"].as_str().unwrap()
            }),
        )
        .unwrap();
        let next_value: Value = serde_json::from_str(&next).unwrap();
        assert_eq!(next_value["count"], 1);
        assert_eq!(next_value["items"][0]["id"], "s3");
        assert_eq!(next_value["hasMore"], false);
    }

    #[test]
    fn search_includes_workflow_run_and_latest_status() {
        let conn = test_conn();
        let now = crate::utils::now();
        conn.execute(
            "INSERT INTO workflow_definitions
             (id, name, description, created_at, updated_at) VALUES ('w1', '日报生成', '', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflow_events (execution_id, kind, payload, created_at)
             VALUES ('run1', 'execution/created', ?1, ?2)",
            params![
                json!({"definitionId": "w1", "status": "running"}).to_string(),
                now - 5
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflow_events (execution_id, kind, payload, created_at)
             VALUES ('run1', 'execution/status', ?1, ?2)",
            params![
                json!({"status": "success", "completionRate": 1.0}).to_string(),
                now
            ],
        )
        .unwrap();

        let result =
            search_work_items(&conn, &json!({"kind": "workflow_run", "status": "success"}))
                .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["count"], 1);
        assert_eq!(value["items"][0]["kind"], "workflow_run");
        assert_eq!(value["items"][0]["status"], "success");
    }

    #[test]
    fn usage_returns_exact_tokens_and_unknown_cost() {
        let conn = test_conn();
        let now = crate::utils::now();
        conn.execute(
            "INSERT INTO sessions
             (id, title, created_at, updated_at, status, origin, project_id)
             VALUES ('s1', '成本会话', ?1, ?1, 'active', NULL, NULL)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_usage_log
             (session_id, provider, model, prompt_tokens, completion_tokens, total_tokens,
              cached_tokens, cache_read_tokens, cache_write_tokens, channel, created_at)
             VALUES ('s1', 'deepseek', 'deepseek-chat', 100, 50, 175, 25, 20, 5, 'api', ?1)",
            params![now],
        )
        .unwrap();

        let result = query_work_usage(&conn, &json!({"days": 30})).unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["totals"]["totalTokens"], 175);
        assert_eq!(value["totals"]["cacheReadTokens"], 20);
        assert_eq!(value["totals"]["cacheWriteTokens"], 5);
        assert_eq!(value["cost"]["estimatedCost"], Value::Null);
        assert_eq!(value["cost"]["status"], "pricing_not_configured");
    }

    #[test]
    fn session_detail_respects_output_budget() {
        let conn = test_conn();
        let now = crate::utils::now();
        conn.execute(
            "INSERT INTO sessions
             (id, title, created_at, updated_at, status, origin)
             VALUES ('s1', '长会话', ?1, ?1, 'active', NULL)",
            params![now],
        )
        .unwrap();
        for index in 0..8 {
            conn.execute(
                "INSERT INTO session_events (session_id, kind, payload, created_at)
                 VALUES ('s1', 'assistant/message', ?1, ?2)",
                params![
                    json!({"role": "assistant", "content": "长内容".repeat(5_000)}).to_string(),
                    now + index
                ],
            )
            .unwrap();
        }
        let body = read_work_item(&conn, &json!({"kind": "session", "id": "s1"})).unwrap();
        assert!(body.chars().count() <= READ_OUTPUT_BUDGET_CHARS);
    }

    #[test]
    fn groupchat_profile_disables_work_history_tools() {
        let profile = ToolProfile::groupchat();
        for name in [SEARCH_WORK_ITEMS, READ_WORK_ITEM, QUERY_WORK_USAGE] {
            assert!(
                is_default_disabled(&profile, name),
                "{name} should be disabled"
            );
        }
    }
}
