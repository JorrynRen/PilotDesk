//! 工具模型清单的本地编辑层（`tool_manifests` 表）。
//!
//! 只有用户改过的整份清单才落库；市场缓存仍是可删除、可重建的文件派生数据。
//! 这里不做任何校验——清单的整份校验由 `tools::video_manifest` 负责，
//! 保证写入与生效使用同一套规则。

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::utils::errors::AppError;

/// 一行本地编辑记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalManifest {
    /// 编辑时所基于的基线版本
    pub base_version: u64,
    /// 基线来源：`builtin` / `remote`
    pub base_source: String,
    /// 用户编辑后的整份清单原文
    pub payload: String,
    pub updated_at: i64,
}

/// 读取某类清单的本地编辑（不存在返回 None）。
pub fn load(conn: &rusqlite::Connection, kind: &str) -> Result<Option<LocalManifest>, AppError> {
    let row = conn
        .query_row(
            "SELECT base_version, base_source, payload, updated_at FROM tool_manifests WHERE kind = ?1",
            params![kind],
            |row| {
                Ok(LocalManifest {
                    base_version: row.get::<_, i64>(0)? as u64,
                    base_source: row.get(1)?,
                    payload: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// 写入（或整行覆盖）本地编辑。
pub fn save(
    conn: &rusqlite::Connection,
    kind: &str,
    base_version: u64,
    base_source: &str,
    payload: &str,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO tool_manifests (kind, base_version, base_source, payload, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(kind) DO UPDATE SET
             base_version = ?2, base_source = ?3, payload = ?4, updated_at = ?5",
        params![kind, base_version as i64, base_source, payload, now],
    )?;
    Ok(())
}

/// 删除本地编辑（放弃修改）。
pub fn delete(conn: &rusqlite::Connection, kind: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM tool_manifests WHERE kind = ?1", params![kind])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tool_manifests (
                kind TEXT PRIMARY KEY,
                base_version INTEGER NOT NULL DEFAULT 0,
                base_source TEXT NOT NULL DEFAULT 'builtin',
                payload TEXT NOT NULL,
                updated_at INTEGER NOT NULL)",
        )
        .unwrap();
        conn
    }

    #[test]
    fn roundtrip_and_delete() {
        let conn = mem_conn();
        assert!(load(&conn, "video").unwrap().is_none());
        save(&conn, "video", 3, "remote", r#"{"version":4}"#).unwrap();
        let row = load(&conn, "video").unwrap().unwrap();
        assert_eq!(row.base_version, 3);
        assert_eq!(row.base_source, "remote");
        assert_eq!(row.payload, r#"{"version":4}"#);

        // 覆盖写保持单行
        save(&conn, "video", 4, "remote", r#"{"version":5}"#).unwrap();
        assert_eq!(load(&conn, "video").unwrap().unwrap().base_version, 4);

        // 不同 kind 互不影响
        assert!(load(&conn, "image").unwrap().is_none());

        delete(&conn, "video").unwrap();
        assert!(load(&conn, "video").unwrap().is_none());
    }
}
