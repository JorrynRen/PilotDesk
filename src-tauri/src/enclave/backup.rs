//! 主机自动备份（设计 §13 一期前置条件「主机转移与备份」）。
//!
//! 不是云同步、也不改变权威：备份是**主机本机**的灾难兜底——把 `enclave.db`
//! 的一致性快照与本机承载组织的团队文件打包到本机备份目录，按保留份数滚动清理。
//!
//! - 成员端无任何备份（不缓存团队数据）；
//! - 备份包为普通 zip（不含口令）：落在用户自己的磁盘上，用于误删 / 库损坏后的恢复；
//! - 主机转移仍走 `transfer.rs` 的加密迁移包，二者互不替代。

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{files, host, org};
use crate::utils::paths;

const KEY_ENABLED: &str = "enclave_backup_enabled";
const KEY_INTERVAL_HOURS: &str = "enclave_backup_interval_hours";
const KEY_KEEP: &str = "enclave_backup_keep";
const KEY_LAST_AT: &str = "enclave_backup_last_at";
const KEY_LAST_PATH: &str = "enclave_backup_last_path";
const KEY_LAST_SIZE: &str = "enclave_backup_last_size";
const KEY_LAST_ERROR: &str = "enclave_backup_last_error";

const DEFAULT_INTERVAL_HOURS: i64 = 24;
/// 默认只保留 2 份：团队文件较多时备份包体积大，份数过多会占用过多磁盘
const DEFAULT_KEEP: i64 = 2;

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatusView {
    pub enabled: bool,
    pub interval_hours: i64,
    pub keep: i64,
    /// 最近一次成功备份时间（unix 秒，0 = 从未备份）
    pub last_at: i64,
    pub last_path: String,
    pub last_size: i64,
    pub last_error: String,
    /// 备份目录（本机路径）
    pub dir: String,
}

fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM enclave_settings WHERE key = ?1",
        params![key],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

fn put_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO enclave_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, value, now],
    )
    .map_err(|e| format!("保存备份设置失败：{}", e))?;
    Ok(())
}

fn setting_i64(conn: &Connection, key: &str, default: i64) -> i64 {
    get_setting(conn, key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// 备份目录：`<应用目录>/enclave/backups`。
pub fn backup_dir() -> PathBuf {
    paths::app_root_dir().join("enclave").join("backups")
}

pub fn status(conn: &Connection) -> Result<BackupStatusView, String> {
    Ok(BackupStatusView {
        enabled: get_setting(conn, KEY_ENABLED)
            .map(|v| v != "0")
            .unwrap_or(false),
        interval_hours: setting_i64(conn, KEY_INTERVAL_HOURS, DEFAULT_INTERVAL_HOURS)
            .clamp(1, 168),
        keep: setting_i64(conn, KEY_KEEP, DEFAULT_KEEP).clamp(1, 30),
        last_at: setting_i64(conn, KEY_LAST_AT, 0),
        last_path: get_setting(conn, KEY_LAST_PATH).unwrap_or_default(),
        last_size: setting_i64(conn, KEY_LAST_SIZE, 0),
        last_error: get_setting(conn, KEY_LAST_ERROR).unwrap_or_default(),
        dir: backup_dir().to_string_lossy().to_string(),
    })
}

pub fn set_config(
    conn: &Connection,
    enabled: bool,
    interval_hours: i64,
    keep: i64,
) -> Result<BackupStatusView, String> {
    if !(1..=168).contains(&interval_hours) {
        return Err("备份间隔需在 1 ~ 168 小时之间".to_string());
    }
    if !(1..=30).contains(&keep) {
        return Err("保留份数需在 1 ~ 30 之间".to_string());
    }
    put_setting(conn, KEY_ENABLED, if enabled { "1" } else { "0" })?;
    put_setting(conn, KEY_INTERVAL_HOURS, &interval_hours.to_string())?;
    put_setting(conn, KEY_KEEP, &keep.to_string())?;
    status(conn)
}

/// 是否到了下一次自动备份时间（且本机确实承载了至少一个组织）。
pub fn due(conn: &Connection) -> bool {
    let st = match status(conn) {
        Ok(s) => s,
        Err(_) => return false,
    };
    if !st.enabled {
        return false;
    }
    let hosts = org::host_org_ids(conn, &host::device_id()).unwrap_or_default();
    if hosts.is_empty() {
        return false;
    }
    let now = chrono::Utc::now().timestamp();
    now.saturating_sub(st.last_at) >= st.interval_hours.saturating_mul(3600)
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub path: String,
    pub size: i64,
    pub file_count: usize,
    pub org_count: usize,
    pub created_at: i64,
}

/// 立即执行一次备份（同步、CPU/IO 密集；命令层用 `spawn_blocking` 包一层）。
pub fn run_backup(conn: &Connection) -> Result<BackupInfo, String> {
    let now = chrono::Utc::now().timestamp();
    let org_ids = org::host_org_ids(conn, &host::device_id())?;
    if org_ids.is_empty() {
        return Err("本机不是任何组织的主机，未生成备份".to_string());
    }
    let dir = backup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败：{}", e))?;

    let tmp_db = std::env::temp_dir().join(format!("pilotdesk-backup-{}.db", crate::utils::new_id()));
    let result = (|| -> Result<BackupInfo, String> {
        conn.execute("VACUUM INTO ?1", params![tmp_db.to_string_lossy().to_string()])
            .map_err(|e| format!("生成数据库快照失败：{}", e))?;

        let file_name = format!("enclave-backup-{}.zip", now);
        let out_path = dir.join(&file_name);
        let file = File::create(&out_path).map_err(|e| format!("创建备份包失败：{}", e))?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // 1) 数据库快照
        zip.start_file("enclave.db", opts)
            .map_err(|e| format!("写入数据库快照失败：{}", e))?;
        let mut db_file = File::open(&tmp_db).map_err(|e| format!("读取数据库快照失败：{}", e))?;
        std::io::copy(&mut db_file, &mut zip)
            .map_err(|e| format!("写入数据库快照失败：{}", e))?;

        // 2) 团队文件（按组织目录；只备份本机作为主机的组织）
        let root = files::resolve_team_files_root(conn)?;
        let mut file_count = 0usize;
        for org_id in &org_ids {
            let org_dir = root.join(org_id);
            if !org_dir.is_dir() {
                continue;
            }
            let mut queue: VecDeque<PathBuf> = VecDeque::new();
            queue.push_back(org_dir.clone());
            while let Some(current) = queue.pop_front() {
                let entries = match std::fs::read_dir(&current) {
                    Ok(e) => e,
                    Err(e) => {
                        return Err(format!("读取团队文件目录失败（{}）：{}", current.display(), e))
                    }
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        queue.push_back(path);
                        continue;
                    }
                    let rel = match path.strip_prefix(&root) {
                        Ok(r) => r.to_string_lossy().replace('\\', "/"),
                        Err(_) => continue,
                    };
                    let name = format!("files/{}", rel);
                    zip.start_file(name, opts)
                        .map_err(|e| format!("写入备份条目失败：{}", e))?;
                    let mut f = File::open(&path)
                        .map_err(|e| format!("读取团队文件失败（{}）：{}", path.display(), e))?;
                    std::io::copy(&mut f, &mut zip)
                        .map_err(|e| format!("写入备份条目失败：{}", e))?;
                    file_count += 1;
                }
            }
        }

        // 3) 清单
        let manifest = serde_json::json!({
            "format": "pilotdesk-enclave-backup",
            "version": 1,
            "createdAt": now,
            "schemaVersion": conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap_or(0),
            "orgIds": &org_ids,
            "fileCount": file_count,
        });
        zip.start_file("manifest.json", opts)
            .map_err(|e| format!("写入备份清单失败：{}", e))?;
        zip.write_all(manifest.to_string().as_bytes())
            .map_err(|e| format!("写入备份清单失败：{}", e))?;
        zip.finish().map_err(|e| format!("完成备份包失败：{}", e))?;

        let size = std::fs::metadata(&out_path).map(|m| m.len() as i64).unwrap_or(0);
        put_setting(conn, KEY_LAST_AT, &now.to_string())?;
        put_setting(conn, KEY_LAST_PATH, &out_path.to_string_lossy())?;
        put_setting(conn, KEY_LAST_SIZE, &size.to_string())?;
        put_setting(conn, KEY_LAST_ERROR, "")?;

        prune(&dir, status(conn)?.keep)?;

        Ok(BackupInfo {
            path: out_path.to_string_lossy().to_string(),
            size,
            file_count,
            org_count: org_ids.len(),
            created_at: now,
        })
    })();

    let _ = std::fs::remove_file(&tmp_db);
    if let Err(e) = &result {
        let _ = put_setting(conn, KEY_LAST_ERROR, e);
    }
    result
}

/// 保留最近 `keep` 份，其余按文件名（内含 unix 秒）从旧到新删除。
fn prune(dir: &Path, keep: i64) -> Result<(), String> {
    let keep = keep.max(1) as usize;
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("读取备份目录失败：{}", e))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("enclave-backup-") && name.ends_with(".zip") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    if names.len() <= keep {
        return Ok(());
    }
    for name in names.iter().take(names.len() - keep) {
        let _ = std::fs::remove_file(dir.join(name));
    }
    Ok(())
}

/// 后台自动备份：每 10 分钟检查一次是否到期；备份本身在阻塞线程池执行。
pub async fn run_loop(state: Arc<super::EnclaveState>) {
    // 启动后先让出资源，避免与启动期其它初始化争抢 IO
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        let is_due = state.store.with_conn(|c| Ok(due(c))).unwrap_or(false);
        if is_due {
            let st = state.clone();
            match tokio::task::spawn_blocking(move || st.store.with_conn(|c| run_backup(c))).await {
                Ok(Ok(info)) => log::info!(
                    "[enclave] 自动备份完成：{}（{} 个文件，{} 字节）",
                    info.path,
                    info.file_count,
                    info.size
                ),
                Ok(Err(e)) => log::warn!("[enclave] 自动备份失败：{}", e),
                Err(e) => log::warn!("[enclave] 自动备份线程异常：{}", e),
            }
        }
        tokio::time::sleep(Duration::from_secs(600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enclave::db::EnclaveStore;

    fn store() -> EnclaveStore {
        let dir = std::env::temp_dir().join(format!("pd-backup-{}", crate::utils::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        EnclaveStore::open(&dir.join("enclave.db")).unwrap()
    }

    #[test]
    fn config_is_clamped_and_validated() {
        let store = store();
        store
            .with_conn(|c| {
                assert!(set_config(c, true, 0, 7).is_err());
                assert!(set_config(c, true, 24, 0).is_err());
                let st = set_config(c, false, 12, 3)?;
                assert!(!st.enabled);
                assert_eq!(st.interval_hours, 12);
                assert_eq!(st.keep, 3);
                assert!(!due(c));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn backup_requires_host_org() {
        let store = store();
        store
            .with_conn(|c| {
                // 测试种子没有 host 组织 → 明确报错而不是生成空包
                assert!(run_backup(c).is_err());
                Ok(())
            })
            .unwrap();
    }
}
