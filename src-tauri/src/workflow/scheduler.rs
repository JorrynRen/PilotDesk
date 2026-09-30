use super::executor::NodeExecutor;
use crate::utils::errors::AppError;
use crate::utils::now;
use crate::DbPool;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

/// 解析 Cron 表达式并返回严格晚于 `from_ts` 的下一次触发时间戳（秒）。
///
/// **按本机时区解释**：用户写「`0 0 9 * * *`」期待的是本地 9 点。此前按 UTC 解析，
/// 在东八区会整体后移 8 小时（"每天 15:10" 被排到本地 23:10），这是必须避免的语义错位。
///
/// 5 字段写法（`分 时 日 月 周`，用户从系统 crontab 抄来的常见形态）按「秒=0」补齐后再解析；
/// 解析失败返回 `None` —— 调用方必须据此拒绝创建 / 停用调度，否则该调度会永远"到期"而反复触发。
pub fn next_run_after(cron_expr: &str, from_ts: i64) -> Option<i64> {
    let trimmed = cron_expr.trim();
    let normalized = if trimmed.split_whitespace().count() == 5 {
        format!("0 {}", trimmed)
    } else {
        trimmed.to_string()
    };
    let schedule = cron::Schedule::from_str(&normalized).ok()?;
    let from = local_datetime(from_ts)?;
    schedule.after(&from).next().map(|t| t.timestamp())
}

/// 时间戳（秒）→ 本地时区时间：Cron 的"几点几分"是墙上时钟，必须落在本地时区上
fn local_datetime(ts: i64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
        .map(chrono::DateTime::<chrono::Local>::from)
}

/// 校验 Cron 表达式是否可用（创建/保存前调用）
pub fn validate_cron(cron_expr: &str) -> Result<(), String> {
    match next_run_after(cron_expr, now()) {
        Some(_) => Ok(()),
        None => Err(format!(
            "Cron 表达式无法解析: \"{}\"（需 6 字段: 秒 分 时 日 月 星期，或 5 字段: 分 时 日 月 星期）",
            cron_expr.trim()
        )),
    }
}

/// 工作流调度记录

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSchedule {
    pub id: String,
    pub workflow_id: String,
    pub cron_expression: String,
    pub enabled: bool,
    pub input_data: String,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 定时调度器

pub struct WorkflowScheduler {
    pool: DbPool,
    running: Arc<AtomicBool>,
    handle: Arc<AsyncMutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl WorkflowScheduler {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            running: Arc::new(AtomicBool::new(false)),
            handle: Arc::new(AsyncMutex::new(None)),
        }
    }

    /// 启动调度器后台任务

    pub async fn start(&self, executor: Arc<NodeExecutor>, app_handle: tauri::AppHandle) {
        if self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(true, Ordering::SeqCst);

        // 启动校正：`next_run_at` 是表达式派生出来的缓存，口径一变（例如改为按本机时区解析）
        // 或应用长时间未开，库里的值就可能与实际排期不符，这里按当前口径重算一次
        match resync_pending_next_runs(&self.pool) {
            Ok(0) => {}
            Ok(n) => log::info!("[WorkflowScheduler] 启动校正定时排期: {} 条", n),
            Err(e) => log::warn!("[WorkflowScheduler] 启动校正定时排期失败: {}", e),
        }

        let running = self.running.clone();
        let pool = self.pool.clone();
        let handle = tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                // 每分钟检查一次

                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                // 查询到期的调度

                let now_ts = now();
                let schedules = match get_due_schedules(&pool, now_ts) {
                    Ok(s) => s,
                    Err(e) => {
                        log::error!("查询到期调度失败: {}", e);
                        continue;
                    }
                };
                for schedule in schedules {
                    let exec = executor.clone();
                    let handle = app_handle.clone();
                    let wf_id = schedule.workflow_id.clone();
                    let input: serde_json::Value = serde_json::from_str(&schedule.input_data)
                        .unwrap_or(serde_json::Value::Null);
                    let sched_id = schedule.id.clone();
                    let cron_expr = schedule.cron_expression.clone();
                    let trigger_ts = now_ts; // 复用外层时间戳，避免闭包内重新获取

                    // ① 先推进「下一次执行时间」：表达式非法 / 定义缺失 / 已停用都不能让
                    //    next_run_at 停在过去——否则该调度每分钟都会重新到期、反复触发。
                    let Some(next_ts) = next_run_after(&cron_expr, now_ts) else {
                        log::error!(
                            "[WorkflowScheduler] Cron 表达式非法，已自动停用调度: id={}, expr={}",
                            sched_id,
                            cron_expr
                        );
                        if let Err(e) = set_schedule_enabled(&pool, &sched_id, false) {
                            log::error!("[WorkflowScheduler] 停用非法调度失败: {}", e);
                        }
                        continue;
                    };
                    if let Err(e) = mark_schedule_ran(&pool, &sched_id, trigger_ts, next_ts) {
                        log::error!("[WorkflowScheduler] 更新调度时间失败: {}", e);
                    }

                    // ② 取定义（同步段完成；r2d2 连接不跨 await 持有）
                    let def = {
                        let conn = match pool.get() {
                            Ok(c) => c,
                            Err(e) => {
                                log::error!("获取数据库连接失败: {}", e);
                                continue;
                            }
                        };
                        match super::get_definition(&conn, &wf_id) {
                            Ok(Some(d)) => d,
                            Ok(None) => {
                                log::warn!("调度的工作流不存在: {}", wf_id);
                                continue;
                            }
                            Err(e) => {
                                log::error!("查询工作流失败: {}", e);
                                continue;
                            }
                        }
                    };

                    // ③ 工作流被停用（enabled=false）时不执行：定时是"无人值守"入口，停用就该彻底安静
                    if !def.enabled {
                        log::info!(
                            "[WorkflowScheduler] 工作流已停用，跳过本次定时执行: {} ({})",
                            def.name,
                            wf_id
                        );
                        continue;
                    }

                    // ④ 建实例（同步）
                    let execution_id = crate::utils::new_id();
                    let instance = super::WorkflowInstance {
                        id: execution_id.clone(),
                        definition_id: def.id.clone(),
                        definition_name: def.name.clone(),
                        status: super::WorkflowInstanceStatus::Running,
                        context: serde_json::json!({}),
                        trigger: "cron".to_string(),
                        trigger_detail: Some(cron_expr.clone()),
                        started_at: Some(trigger_ts),
                        completed_at: None,
                        completion_rate: 0.0,
                        skipped_count: 0,
                        output: None,
                        output_source: None,
                        output_node_label: None,
                        error: None,
                        created_at: trigger_ts,
                    };
                    {
                        let conn = match pool.get() {
                            Ok(c) => c,
                            Err(e) => {
                                log::error!("获取数据库连接失败: {}", e);
                                continue;
                            }
                        };
                        if let Err(e) = super::create_instance(&conn, &instance) {
                            log::error!("创建调度执行实例失败: {}", e);
                            continue;
                        }
                    }

                    // ⑤ 执行（异步）；终态后派发 workflow.completed / workflow.failed 事件
                    tokio::spawn(async move {
                        if let Err(e) = super::triggers::run_definition_top_level(
                            &exec,
                            &def,
                            &execution_id,
                            input,
                            &handle,
                            5,
                            super::triggers::TRIGGER_CRON,
                        )
                        .await
                        {
                            log::error!("调度工作流执行失败: {}", e);
                            emit_terminal_failure(&handle, &def, &execution_id, &e);
                        }
                    });
                }
            }
        });
        *self.handle.lock().await = Some(handle);
    }

    /// 停止调度器

    #[allow(dead_code)]
    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.lock().await.take() {
            handle.abort();
        }
    }
}

/// 定时执行失败时补发一条终态进度事件。
///
/// 手动执行由 `commands/workflow.rs` 发出成功/失败帧，定时执行没有这一步——不补的话用户既收不到
/// 失败通知，前端也没有刷新时机（卡片上"上次/下次执行"就是靠这类事件触发的刷新）。
fn emit_terminal_failure(
    app: &tauri::AppHandle,
    def: &super::WorkflowDefinition,
    execution_id: &str,
    err: &AppError,
) {
    let text = err.to_string();
    let cancelled = text.contains("取消") || text.to_lowercase().contains("cancel");
    super::triggers::emit_execution_frame(
        app,
        def,
        execution_id,
        if cancelled { "cancelled" } else { "failed" },
        super::triggers::TRIGGER_CRON,
        Some(&text),
    );
}

/// 启动时校正未到期调度的 `next_run_at`（返回被修正的条数）。
///
/// `next_run_at` 是表达式派生的缓存值；早期版本按 UTC 解析表达式，在东八区会整体偏移 8 小时
/// （"每天 15:10" 被排到本地 23:10）。这里对**尚未到期**的调度按当前口径重算，避免它在错误的
/// 钟点跑一次；**已到期的保持原样**——那是一次正常的迟到补跑，重算会把它抹掉。
fn resync_pending_next_runs(pool: &DbPool) -> Result<usize, AppError> {
    let conn = pool.get().map_err(|e| AppError::Lock(e.to_string()))?;
    let now_ts = now();

    let rows: Vec<(String, String, Option<i64>)> =
        {
            let mut stmt = conn.prepare(
            "SELECT id, cron_expression, next_run_at FROM workflow_schedules WHERE enabled = 1",
        ).map_err(|e| AppError::Db(e.to_string()))?;
            // 先落到局部变量再返回：直接作为块尾表达式会让 MappedRows 临时值活过 `stmt` 的析构
            let collected = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|e| AppError::Db(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AppError::Db(e.to_string()))?;
            collected
        };

    let mut fixed = 0usize;
    for (id, expr, stored) in rows {
        let pending = stored.map_or(true, |ts| ts > now_ts);
        if !pending {
            continue;
        }
        let Some(next_ts) = next_run_after(&expr, now_ts) else {
            continue;
        };
        if stored == Some(next_ts) {
            continue;
        }
        conn.execute(
            "UPDATE workflow_schedules SET next_run_at = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![next_ts, now_ts, id],
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
        fixed += 1;
    }
    Ok(fixed)
}

/// 查询到期的调度

fn get_due_schedules(pool: &DbPool, now_ts: i64) -> Result<Vec<WorkflowSchedule>, AppError> {
    let conn = pool.get().map_err(|e| AppError::Lock(e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, workflow_id, cron_expression, enabled, input_data,
                last_run_at, next_run_at, created_at, updated_at

         FROM workflow_schedules

         WHERE enabled = 1 AND next_run_at <= ?1

         ORDER BY next_run_at ASC",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    let schedules = stmt
        .query_map([now_ts], |row| {
            Ok(WorkflowSchedule {
                id: row.get(0)?,
                workflow_id: row.get(1)?,
                cron_expression: row.get(2)?,
                enabled: row.get::<_, i32>(3)? != 0,
                input_data: row.get(4)?,
                last_run_at: row.get(5)?,
                next_run_at: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(schedules)
}

// ── CRUD ──

pub fn create_schedule(
    conn: &rusqlite::Connection,
    sched: &WorkflowSchedule,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO workflow_schedules (id, workflow_id, cron_expression, enabled, input_data,
         last_run_at, next_run_at, created_at, updated_at)

         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            sched.id,
            sched.workflow_id,
            sched.cron_expression,
            sched.enabled as i32,
            sched.input_data,
            sched.last_run_at,
            sched.next_run_at,
            sched.created_at,
            sched.updated_at,
        ],
    )?;
    Ok(())
}

pub fn list_schedules(conn: &rusqlite::Connection) -> Result<Vec<WorkflowSchedule>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, cron_expression, enabled, input_data,
                last_run_at, next_run_at, created_at, updated_at

         FROM workflow_schedules ORDER BY next_run_at ASC",
    )?;
    let schedules = stmt
        .query_map([], |row| {
            Ok(WorkflowSchedule {
                id: row.get(0)?,
                workflow_id: row.get(1)?,
                cron_expression: row.get(2)?,
                enabled: row.get::<_, i32>(3)? != 0,
                input_data: row.get(4)?,
                last_run_at: row.get(5)?,
                next_run_at: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(schedules)
}

pub fn delete_schedule(conn: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM workflow_schedules WHERE id = ?1",
        rusqlite::params![id],
    )?;
    Ok(())
}

/// 启停一条调度（非法表达式由调度器自动停用；UI 也可手动启停）
pub fn set_schedule_enabled(pool: &DbPool, id: &str, enabled: bool) -> Result<(), AppError> {
    let conn = pool.get().map_err(|e| AppError::Lock(e.to_string()))?;
    conn.execute(
        "UPDATE workflow_schedules SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![enabled as i32, now(), id],
    )?;
    Ok(())
}

/// 记录本次执行时间并推进下一次执行时间
pub fn mark_schedule_ran(
    pool: &DbPool,
    id: &str,
    last_run_at: i64,
    next_run_at: i64,
) -> Result<(), AppError> {
    let conn = pool.get().map_err(|e| AppError::Lock(e.to_string()))?;
    conn.execute(
        "UPDATE workflow_schedules SET last_run_at = ?1, next_run_at = ?2, updated_at = ?3 WHERE id = ?4",
        rusqlite::params![last_run_at, next_run_at, now(), id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{next_run_after, validate_cron};
    use chrono::Timelike;

    /// 时间戳 → 本地墙上时钟（时, 分, 秒）
    fn local_hms(ts: i64) -> (u32, u32, u32) {
        let local = super::local_datetime(ts).expect("时间戳应可转换");
        (local.hour(), local.minute(), local.second())
    }

    /// 回归：非法表达式必须被识别出来。旧实现把它直接落库，调度器每次轮询都判定"已到期"，
    /// 结果这条调度每分钟重复触发一次（next_run_at 永远停在过去）。
    #[test]
    fn invalid_cron_is_rejected() {
        assert!(validate_cron("这不是表达式").is_err());
        assert!(validate_cron("bogus").is_err());
        assert!(next_run_after("bogus", 1_700_000_000).is_none());
    }

    /// 首次执行时间按表达式真实计算，不再是 now+60；且"几点"按**本地时区**解释
    /// （回归：按 UTC 解析会让东八区的"每天 15:10"排到本地 23:10）。
    #[test]
    fn first_run_follows_expression_in_local_time() {
        let from = 1_700_000_000;
        let next = next_run_after("0 10 15 * * *", from).expect("表达式应可解析");
        assert!(next > from, "下一次执行必须严格晚于当前时间");
        assert_eq!(local_hms(next), (15, 10, 0), "应落在本地 15:10:00");
        assert!(next - from > 60, "不应退化成 now+60 的兜底排期");
    }

    /// 5 字段写法（系统 crontab 形态）按「秒=0」补齐，避免用户抄来的表达式被一律拒绝
    #[test]
    fn five_field_expression_is_accepted() {
        let from = 1_700_000_000;
        let next = next_run_after("0 9 * * *", from).expect("5 字段表达式应可解析");
        assert_eq!(local_hms(next), (9, 0, 0));
    }
}
