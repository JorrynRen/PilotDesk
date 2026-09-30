//! [遗留] JSON 文件版 KV 记忆库（已弃用）
//!
//! 曾用于 API Agent 跨会话持久化记忆（~/.pilotdesk/memories.json）。
//! 生产路径已被 [`crate::api_agent::db::MemoryStore`]（SQLite `MEMORY.db`）取代。
//! 本模块仅保留自测与历史对照使用，不作为生产存储；新功能请基于 `api_agent::db`。

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::Mutex;

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

/// KV 记忆库
///
/// 线程安全的键值对记忆存储，支持：
///   - save_memory: 保存或更新一条记忆
///   - search_memory: 按关键词搜索记忆
///   - get_top: 获取访问频率最高的 N 条记忆（用于 system prompt 注入）
pub struct MemoryStore {
    memories: Arc<Mutex<Vec<MemoryEntry>>>,
    file_path: String,
}

#[allow(dead_code)]
impl MemoryStore {
    /// 创建或加载记忆库
    pub fn new(file_path: String) -> Self {
        let memories = Self::load_from_file(&file_path);
        Self {
            memories: Arc::new(Mutex::new(memories)),
            file_path,
        }
    }

    /// 保存或更新一条记忆
    pub fn save_memory(&self, key: &str, value: &str, category: &str) -> MemoryEntry {
        let now = Self::now_secs();
        let mut mems = self.memories.lock().unwrap();

        if let Some(existing) = mems.iter_mut().find(|m| m.key == key) {
            existing.value = value.to_string();
            existing.category = category.to_string();
            existing.updated_at = now;
            existing.access_count += 1;
            let entry = existing.clone();
            self.persist(&mems);
            entry
        } else {
            let entry = MemoryEntry {
                key: key.to_string(),
                value: value.to_string(),
                category: category.to_string(),
                created_at: now,
                updated_at: now,
                access_count: 1,
            };
            mems.push(entry.clone());
            self.persist(&mems);
            entry
        }
    }

    /// 按关键词搜索记忆
    ///
    /// 在 key 和 value 中进行子串匹配，返回匹配项。
    /// 按访问频率降序排列。
    pub fn search_memory(&self, query: &str) -> Vec<MemoryEntry> {
        let query_lower = query.to_lowercase();
        let mut mems = self.memories.lock().unwrap();

        // 先收集匹配项（避免同时持有不可变和可变借用）
        let matched_keys: Vec<String> = mems
            .iter()
            .filter(|m| {
                m.key.to_lowercase().contains(&query_lower)
                    || m.value.to_lowercase().contains(&query_lower)
            })
            .map(|m| m.key.clone())
            .collect();

        // 更新访问计数
        let now = Self::now_secs();
        for key in &matched_keys {
            if let Some(original) = mems.iter_mut().find(|m| &m.key == key) {
                original.access_count += 1;
                original.updated_at = now;
            }
        }

        // 收集结果
        let mut results: Vec<MemoryEntry> = mems
            .iter()
            .filter(|m| matched_keys.contains(&m.key))
            .cloned()
            .collect();

        if !results.is_empty() {
            self.persist(&mems);
        }

        // 按访问频率降序
        results.sort_by(|a, b| b.access_count.cmp(&a.access_count));
        results
    }

    /// 获取访问频率最高的 N 条记忆（用于 system prompt 注入）
    pub fn get_top(&self, limit: usize) -> Vec<MemoryEntry> {
        let mems = self.memories.lock().unwrap();
        let mut sorted: Vec<MemoryEntry> = mems.clone();
        sorted.sort_by(|a, b| b.access_count.cmp(&a.access_count));
        sorted.truncate(limit);
        sorted
    }

    /// 获取记忆总数
    pub fn count(&self) -> usize {
        self.memories.lock().unwrap().len()
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

    fn load_from_file(file_path: &str) -> Vec<MemoryEntry> {
        match std::fs::read_to_string(file_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    fn persist(&self, mems: &[MemoryEntry]) {
        if let Ok(json) = serde_json::to_string_pretty(mems) {
            // 确保父目录存在
            if let Some(parent) = std::path::Path::new(&self.file_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.file_path, json);
        }
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

impl Clone for MemoryStore {
    fn clone(&self) -> Self {
        Self {
            memories: self.memories.clone(),
            file_path: self.file_path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> MemoryStore {
        let tmp = std::env::temp_dir().join(format!(
            "pilotdesk_test_memories_{}.json",
            std::process::id()
        ));
        let path = tmp.to_string_lossy().to_string();
        let store = MemoryStore::new(path.clone());
        // Clean up after test
        std::fs::remove_file(&path).ok();
        store
    }

    #[test]
    fn test_save_and_search() {
        let store = temp_store();

        store.save_memory("project_language", "TypeScript", "fact");
        store.save_memory("db_path", "./data.db", "fact");
        store.save_memory("user_style", "concise", "preference");

        let results = store.search_memory("language");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "project_language");

        let results = store.search_memory("data");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "./data.db");
    }

    #[test]
    fn test_get_top() {
        let store = temp_store();

        store.save_memory("a", "1", "fact");
        store.save_memory("b", "2", "fact");
        store.save_memory("c", "3", "preference");

        // Access "a" multiple times
        store.search_memory("1");
        store.search_memory("1");

        let top = store.get_top(2);
        assert_eq!(top.len(), 2);
        // "a" should be first due to higher access count
        assert_eq!(top[0].key, "a");
    }

    #[test]
    fn test_format_for_prompt() {
        let store = temp_store();
        store.save_memory("lang", "Rust", "fact");

        let formatted = store.format_for_prompt(5).unwrap();
        assert!(formatted.contains("<key_memories>"));
        assert!(formatted.contains("lang"));
        assert!(formatted.contains("Rust"));
        assert!(formatted.contains("</key_memories>"));
    }
}
