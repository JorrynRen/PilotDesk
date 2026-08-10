//! 独立数据库 — pilotdesk_agent.db
//!
//! API Agent 专属的轻量级 SQLite 数据库，与主项目 pilotdesk.db 分离。
//! 包含 key_memories 表，替代原有的 JSON 文件持久化方案。
//!
//! 设计原则：
//!   - 线程安全（Arc<Mutex<Connection>>）
//!   - 自动建表（首次打开时创建）
//!   - 自动迁移：首次启动时从 memories.json 导入已有数据
//!   - 不替代 CLI Agent 内部记忆

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// 单条记忆记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
    pub category: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub access_count: u64,
}

/// SQLite 版 KV 记忆库
///
/// 数据库路径：{config_dir}/pilotdesk_agent.db
/// 表结构：key_memories (key, value, category, created_at, updated_at, access_count)
pub struct MemoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl MemoryStore {
    /// 打开或创建 Agent 数据库
    ///
    /// `db_dir` 为 PilotDesk 配置目录的路径（通常为 %APPDATA%/PilotDesk）。
    /// 首次打开时自动建表，并尝试从 memories.json 迁移已有数据。
    pub fn new(db_dir: &str) -> Result<Self, String> {
        let db_path = format!("{}/pilotdesk_agent.db", db_dir.trim_end_matches(['/', '\\']));

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("无法打开 Agent 数据库 {}: {}", db_path, e))?;

        // 启用 WAL 模式，提升并发读取性能
        conn.execute_batch("PRAGMA journal_mode=WAL;").ok();

        // 建表
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS key_memories (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT 'fact',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_memories_category ON key_memories(category);
            CREATE INDEX IF NOT EXISTS idx_memories_access ON key_memories(access_count DESC);",
        )
        .map_err(|e| format!("创建记忆表失败: {}", e))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        // 自动迁移：从 JSON → SQLite（仅首次导入，幂等）
        store.migrate_from_json(db_dir);

        log::info!(
            "[Agent DB] 数据库已就绪: {}, 当前记忆数: {}",
            db_path,
            store.count()
        );
        Ok(store)
    }

    /// 保存或更新一条记忆
    pub fn save_memory(&self, key: &str, value: &str, category: &str) -> MemoryEntry {
        let now = now_secs();
        let conn = self.conn.lock().unwrap();

        // 尝试更新已存在的 key
        let affected = conn
            .execute(
                "UPDATE key_memories SET value = ?1, category = ?2, updated_at = ?3, access_count = access_count + 1
                 WHERE key = ?4",
                params![value, category, now, key],
            )
            .unwrap_or(0);

        if affected == 0 {
            // 插入新记录
            conn.execute(
                "INSERT INTO key_memories (key, value, category, created_at, updated_at, access_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                params![key, value, category, now, now],
            )
            .unwrap();
        }

        // 返回完整记录
        conn.query_row(
            "SELECT key, value, category, created_at, updated_at, access_count
             FROM key_memories WHERE key = ?1",
            params![key],
            row_to_entry,
        )
        .unwrap()
    }

    /// 按关键词搜索记忆（key + value 子串匹配，按访问频率降序）
    pub fn search_memory(&self, query: &str) -> Vec<MemoryEntry> {
        let like = format!("%{}%", query);
        let conn = self.conn.lock().unwrap();

        // 更新匹配记录的访问计数
        conn.execute(
            "UPDATE key_memories SET access_count = access_count + 1, updated_at = ?1
             WHERE key LIKE ?2 OR value LIKE ?2",
            params![now_secs(), like],
        )
        .ok();

        let mut stmt = conn
            .prepare(
                "SELECT key, value, category, created_at, updated_at, access_count
                 FROM key_memories
                 WHERE key LIKE ?1 OR value LIKE ?1
                 ORDER BY access_count DESC",
            )
            .unwrap();

        stmt.query_map(params![like], row_to_entry)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// 获取访问频率最高的 N 条记忆（用于 system prompt 注入）
    pub fn get_top(&self, limit: usize) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT key, value, category, created_at, updated_at, access_count
                 FROM key_memories
                 ORDER BY access_count DESC
                 LIMIT ?1",
            )
            .unwrap();

        stmt.query_map(params![limit as i64], row_to_entry)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// 获取记忆总数
    pub fn count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM key_memories", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// 格式化记忆列表为 system prompt 中的块
    pub fn format_for_prompt(&self, limit: usize) -> Option<String> {
        let top = self.get_top(limit);
        if top.is_empty() {
            return None;
        }

        let mut block = String::from("<key_memories>\n");
        for entry in &top {
            block.push_str(&format!(
                "  - [{}] {}: {}\n",
                entry.category, entry.key, entry.value
            ));
        }
        block.push_str("</key_memories>");
        Some(block)
    }

    // ── 内部方法 ──

    /// 从 memories.json 迁移数据到 SQLite（仅首次）
    fn migrate_from_json(&self, db_dir: &str) {
        let json_path = format!("{}/memories.json", db_dir.trim_end_matches(['/', '\\']));

        let content = match std::fs::read_to_string(&json_path) {
            Ok(c) => c,
            Err(_) => {
                log::debug!("[Agent DB] 无 JSON 文件需要迁移: {}", json_path);
                return;
            }
        };

        let entries: Vec<MemoryEntry> = match serde_json::from_str(&content) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("[Agent DB] JSON 解析失败: {}", e);
                return;
            }
        };

        if entries.is_empty() {
            return;
        }

        let conn = self.conn.lock().unwrap();
        let mut inserted = 0u32;

        for entry in &entries {
            // INSERT OR IGNORE 保证幂等性
            let affected = conn
                .execute(
                    "INSERT OR IGNORE INTO key_memories (key, value, category, created_at, updated_at, access_count)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        entry.key,
                        entry.value,
                        entry.category,
                        entry.created_at,
                        entry.updated_at,
                        entry.access_count,
                    ],
                )
                .unwrap_or(0);

            if affected > 0 {
                inserted += 1;
            }
        }

        if inserted > 0 {
            log::info!(
                "[Agent DB] 已从 {} 迁移 {} 条记忆到 pilotdesk_agent.db",
                json_path,
                inserted
            );
            // 迁移成功后备份原文件
            let backup_path = format!("{}.bak", json_path);
            let _ = std::fs::rename(&json_path, &backup_path);
        }
    }
}

impl Clone for MemoryStore {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
        }
    }
}

// ── 辅助函数 ──

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<MemoryEntry> {
    Ok(MemoryEntry {
        key: row.get(0)?,
        value: row.get(1)?,
        category: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        access_count: row.get(5)?,
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn temp_db_dir() -> String {
        std::env::temp_dir()
            .join(format!(
                "pilotdesk_agent_test_{}_{}",
                std::process::id(),
                now_secs()
            ))
            .to_string_lossy()
            .to_string()
    }

    fn temp_store() -> (MemoryStore, String) {
        INIT.call_once(|| {
            let _ = env_logger::builder().is_test(true).try_init();
        });

        let dir = temp_db_dir();
        std::fs::create_dir_all(&dir).ok();
        let store = MemoryStore::new(&dir).unwrap();
        (store, dir)
    }

    #[test]
    fn test_save_and_search() {
        let (store, dir) = temp_store();

        store.save_memory("project_language", "TypeScript", "fact");
        store.save_memory("db_path", "./data.db", "fact");
        store.save_memory("user_style", "concise", "preference");

        let results = store.search_memory("language");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "project_language");

        let results = store.search_memory("data");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "./data.db");

        // 清理
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_existing() {
        let (store, dir) = temp_store();

        store.save_memory("a", "v1", "fact");
        let results = store.search_memory("a");
        assert_eq!(results[0].value, "v1");
        assert_eq!(results[0].access_count, 2); // save + search

        store.save_memory("a", "v2", "fact");
        let results = store.search_memory("a");
        assert_eq!(results[0].value, "v2");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_get_top() {
        let (store, dir) = temp_store();

        store.save_memory("a", "1", "fact");
        store.save_memory("b", "2", "fact");
        store.save_memory("c", "3", "preference");

        // 多次访问 "a"
        store.search_memory("1");
        store.search_memory("1");

        let top = store.get_top(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].key, "a"); // 访问次数最高

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_format_for_prompt() {
        let (store, dir) = temp_store();

        store.save_memory("lang", "Rust", "fact");

        let formatted = store.format_for_prompt(5).unwrap();
        assert!(formatted.contains("<key_memories>"));
        assert!(formatted.contains("lang"));
        assert!(formatted.contains("Rust"));
        assert!(formatted.contains("</key_memories>"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_json_migration() {
        let dir = temp_db_dir();
        std::fs::create_dir_all(&dir).ok();

        // 先创建 JSON 文件
        let entries = vec![
            MemoryEntry {
                key: "json_key1".into(),
                value: "json_val1".into(),
                category: "fact".into(),
                created_at: 1000,
                updated_at: 1000,
                access_count: 1,
            },
            MemoryEntry {
                key: "json_key2".into(),
                value: "json_val2".into(),
                category: "preference".into(),
                created_at: 2000,
                updated_at: 2000,
                access_count: 2,
            },
        ];

        let json_path = format!("{}/memories.json", dir);
        std::fs::write(&json_path, serde_json::to_string(&entries).unwrap()).unwrap();

        // 打开数据库（应触发迁移）
        let store = MemoryStore::new(&dir).unwrap();

        let results = store.search_memory("json_key");
        assert_eq!(results.len(), 2);
        assert_eq!(store.count(), 2);

        // JSON 文件应被备份为 .bak
        assert!(!std::path::Path::new(&json_path).exists());
        assert!(std::path::Path::new(&format!("{}.bak", json_path)).exists());

        // 幂等性：再次打开不应重复导入
        let store2 = MemoryStore::new(&dir).unwrap();
        assert_eq!(store2.count(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
