//! 工具模型清单（`tool_manifests` 表）的单行读写。
//!
//! 数据库是清单的唯一生效源：启动播种、市场刷新、恢复出厂都写这张表，
//! 工具与设置页只读这里（经由 `tools::video_manifest` 的内存投影）。
//! 这里不做任何校验——整份校验由 `tools::video_manifest` 负责，
//! 保证写入与生效使用同一套规则。

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::utils::errors::AppError;

/// 一行生效清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRow {
    pub kind: String,
    /// 清单自身的数据版本
    pub version: u64,
    /// 清单自身的格式版本
    pub schema_version: u64,
    /// 最近一次写入来源：`builtin` / `market` / `local`
    pub source: String,
    /// 整份清单原文（保留排版，供编辑器直接使用）
    pub payload: String,
    /// 最近一次发现的市场版本（仅提示用，不参与生效判断）
    pub market_version: u64,
    pub updated_at: i64,
}

/// 读取某类清单（不存在返回 None）。
pub fn load(conn: &rusqlite::Connection, kind: &str) -> Result<Option<ManifestRow>, AppError> {
    let row = conn
        .query_row(
            "SELECT kind, version, schema_version, source, payload, market_version, updated_at
             FROM tool_manifests WHERE kind = ?1",
            params![kind],
            |row| {
                Ok(ManifestRow {
                    kind: row.get(0)?,
                    version: row.get::<_, i64>(1)? as u64,
                    schema_version: row.get::<_, i64>(2)? as u64,
                    source: row.get(3)?,
                    payload: row.get(4)?,
                    market_version: row.get::<_, i64>(5)? as u64,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// 写入（或整行覆盖）生效清单。
pub fn upsert(
    conn: &rusqlite::Connection,
    kind: &str,
    version: u64,
    schema_version: u64,
    source: &str,
    payload: &str,
    market_version: u64,
) -> Result<(), AppError> {
    let now = crate::utils::now();
    conn.execute(
        "INSERT INTO tool_manifests
            (kind, version, schema_version, source, payload, market_version, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(kind) DO UPDATE SET
             version = ?2, schema_version = ?3, source = ?4, payload = ?5,
             market_version = ?6, updated_at = ?7",
        params![
            kind,
            version as i64,
            schema_version as i64,
            source,
            payload,
            market_version as i64,
            now
        ],
    )?;
    Ok(())
}

/// 只记录市场版本（本地修改存在时刷新走这里），只增不减。
pub fn touch_market_version(
    conn: &rusqlite::Connection,
    kind: &str,
    market_version: u64,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE tool_manifests SET market_version = MAX(market_version, ?2) WHERE kind = ?1",
        params![kind, market_version as i64],
    )?;
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
                version INTEGER NOT NULL DEFAULT 0,
                schema_version INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'builtin',
                payload TEXT NOT NULL,
                market_version INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL)",
        )
        .unwrap();
        conn
    }

    #[test]
    fn roundtrip_and_overwrite() {
        let conn = mem_conn();
        assert!(load(&conn, "video").unwrap().is_none());

        upsert(&conn, "video", 2, 1, "market", r#"{"version":2}"#, 2).unwrap();
        let row = load(&conn, "video").unwrap().unwrap();
        assert_eq!(row.version, 2);
        assert_eq!(row.schema_version, 1);
        assert_eq!(row.source, "market");
        assert_eq!(row.payload, r#"{"version":2}"#);
        assert_eq!(row.market_version, 2);

        // 覆盖写保持单行
        upsert(&conn, "video", 3, 1, "local", r#"{"version":3}"#, 2).unwrap();
        let row = load(&conn, "video").unwrap().unwrap();
        assert_eq!(row.version, 3);
        assert_eq!(row.source, "local");

        // 不同 kind 互不影响
        assert!(load(&conn, "image").unwrap().is_none());
    }

    #[test]
    fn market_version_only_grows() {
        let conn = mem_conn();
        upsert(&conn, "video", 1, 1, "builtin", "{}", 0).unwrap();
        touch_market_version(&conn, "video", 4).unwrap();
        assert_eq!(load(&conn, "video").unwrap().unwrap().market_version, 4);
        // CDN 回退到 v2 不会让提示消失
        touch_market_version(&conn, "video", 2).unwrap();
        assert_eq!(load(&conn, "video").unwrap().unwrap().market_version, 4);
    }
}
