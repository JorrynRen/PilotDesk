use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex as AsyncMutex;
use crate::utils::errors::AppError;
use crate::utils::now;
use crate::DbPool;
use super::executor::NodeExecutor;
use super::engine::WorkflowEngine;

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
    pub async fn start(
        &self,
        executor: Arc<NodeExecutor>,
        app_handle: tauri::AppHandle,
    ) {
        if self.running.load(Ordering::SeqCst) {
            return;
        }
        self.running.store(true, Ordering::SeqCst);

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
                    let pool = pool.clone();
                    let wf_id = schedule.workflow_id.clone();
                    let input: serde_json::Value = serde_json::from_str(&schedule.input_data)
                        .unwrap_or(serde_json::Value::Null);

                    let sched_id = schedule.id.clone();
                    let cron_expr = schedule.cron_expression.clone();
                    tokio::spawn(async move {
                        // 获取工作流定义
                        let conn = match pool.get() {
                            Ok(c) => c,
                            Err(e) => {
                                log::error!("获取数据库连接失败: {}", e);
                                return;
                            }
                        };

                        let def = match super::get_definition(&conn, &wf_id) {
                            Ok(Some(d)) => d,
                            Ok(None) => {
                                log::warn!("调度的工作流不存在: {}", wf_id);
                                return;
                            }
                            Err(e) => {
                                log::error!("查询工作流失败: {}", e);
                                return;
                            }
                        };

                        let instance_id = crate::utils::new_id();
                        let now_ts = now();
                        let instance = super::WorkflowInstance {
                            id: instance_id.clone(),
                            definition_id: def.id.clone(),
                            definition_name: def.name.clone(),
                            status: super::WorkflowInstanceStatus::Running,
                            context: serde_json::json!({}),
                            steps: serde_json::json!({}),
                            current_node_id: None,
                            trigger: "cron".to_string(),
                            trigger_detail: Some(schedule.cron_expression.clone()),
                            started_at: Some(now_ts),
                            completed_at: None,
                            estimated_remaining: None,
                            error: None,
                            created_at: now_ts,
                        };

                        if let Err(e) = super::create_instance(&conn, &instance) {
                            log::error!("创建调度执行实例失败: {}", e);
                            return;
                        }

                        // 计算下次执行时间
                        if let Ok(schedule) = <cron::Schedule as std::str::FromStr>::from_str(&cron_expr) {
                            use chrono::DateTime;
                            let now_ts = now();
                            let now_dt: chrono::DateTime<chrono::Utc> = DateTime::from_timestamp(now_ts, 0).unwrap_or_default();
                            if let Some(next) = schedule.after::<chrono::Utc>(&now_dt).next() {
                                let next_ts = next.timestamp();
                                let _ = conn.execute(
                                    "UPDATE workflow_schedules SET last_run_at = ?1, next_run_at = ?2, updated_at = ?3 WHERE id = ?4",
                                    rusqlite::params![now_ts, next_ts, now_ts, sched_id],
                                );
                            }
                        }

                        if let Err(e) = WorkflowEngine::execute_with_concurrency(
                            &exec,
                            &def,
                            &instance_id,
                            input,
                            &handle,
                            5,
                        ).await {
                            log::error!("调度工作流执行失败: {}", e);
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

/// 查询到期的调度
fn get_due_schedules(pool: &DbPool, now_ts: i64) -> Result<Vec<WorkflowSchedule>, AppError> {
    let conn = pool.get().map_err(|e| AppError::Lock(e.to_string()))?;
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, cron_expression, enabled, input_data,
                last_run_at, next_run_at, created_at, updated_at
         FROM workflow_schedules
         WHERE enabled = 1 AND next_run_at <= ?1
         ORDER BY next_run_at ASC"
    ).map_err(|e| AppError::Db(e.to_string()))?;

    let schedules = stmt.query_map([now_ts], |row| {
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
    }).map_err(|e| AppError::Db(e.to_string()))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| AppError::Db(e.to_string()))?;

    Ok(schedules)
}

// ── CRUD ──

pub fn create_schedule(conn: &rusqlite::Connection, sched: &WorkflowSchedule) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO workflow_schedules (id, workflow_id, cron_expression, enabled, input_data,
         last_run_at, next_run_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            sched.id, sched.workflow_id, sched.cron_expression, sched.enabled as i32,
            sched.input_data, sched.last_run_at, sched.next_run_at,
            sched.created_at, sched.updated_at,
        ],
    )?;
    Ok(())
}

pub fn list_schedules(conn: &rusqlite::Connection) -> Result<Vec<WorkflowSchedule>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, cron_expression, enabled, input_data,
                last_run_at, next_run_at, created_at, updated_at
         FROM workflow_schedules ORDER BY next_run_at ASC"
    )?;

    let schedules = stmt.query_map([], |row| {
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
    })?.collect::<Result<Vec<_>, _>>()?;

    Ok(schedules)
}

pub fn delete_schedule(conn: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM workflow_schedules WHERE id = ?1", rusqlite::params![id])?;
    Ok(())
}

#[allow(dead_code)]
pub fn update_schedule_next_run(conn: &rusqlite::Connection, id: &str, next_run_at: i64) -> Result<(), AppError> {
    let now_ts = now();
    conn.execute(
        "UPDATE workflow_schedules SET last_run_at = ?1, next_run_at = ?2, updated_at = ?3 WHERE id = ?4",
        rusqlite::params![now_ts, next_run_at, now_ts, id],
    )?;
    Ok(())
}
