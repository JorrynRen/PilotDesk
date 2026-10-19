//! 主机转移（M8）：状态机 + 线下加密迁移包导出/导入 + 旧主机销毁。
//!
//! 设计约束（设计 §8.7 / 实施计划 M8）：
//! - **`owner = host`**：仅组织 owner 可转出；接收方必须已经是组织成员。
//! - **迁移完成先于云端生效**：`verified` 必须早于 `completed`（云端 `transfer.complete`
//!   是 ownership 变更的唯一生效点，失败可干净回滚）。
//! - **数据面不经云**：迁移包只落本地介质或内网直连；线下包必须加密（口令另走控制面/线下约定）。
//! - **旧机销毁**：仅在 `completed` 之后允许；销毁前先删文件、再删业务行，最后写审计
//!   `transfer.data_destroyed`。
//!
//! 迁移包格式（二进制）：
//! ```text
//! magic(24) | version u32 | kdf_iterations u32 | salt(16) | nonce(12) | plaintext_sha256(32) | ciphertext
//! ```
//! 明文是 JSON manifest：`{ schemaVersion, orgId, dbSha256, db(base64), files[] }`，
//! 其中每个文件带 `path/sha256/size/data(base64)`。密钥 = PBKDF2-HMAC-SHA256(口令, salt)。

use std::path::{Path, PathBuf};

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use serde_json::json;

use crate::enclave::authz::{self, Role};

use super::files;
use super::org;

// ── 状态机 ──────────────────────────────────────────────────────

pub const PHASE_PENDING: &str = "pending";
pub const PHASE_ACCEPTED: &str = "accepted";
pub const PHASE_MIGRATING: &str = "migrating";
pub const PHASE_VERIFIED: &str = "verified";
/// 云端 `transfer.complete` 已生效（ownership 已变更）。
pub const PHASE_COMPLETED: &str = "completed";
pub const PHASE_REJECTED: &str = "rejected";
pub const PHASE_EXPIRED: &str = "expired";
pub const PHASE_FAILED: &str = "failed";

pub const DIRECTION_OUT: &str = "out";
pub const DIRECTION_IN: &str = "in";

/// 状态机真值表。`verified` 必须早于 `completed`（迁移未校验不得生效）。
pub fn can_advance(from: &str, to: &str) -> Result<(), String> {
    let ok = match (from, to) {
        (PHASE_PENDING, PHASE_ACCEPTED) => true,
        (PHASE_PENDING, PHASE_REJECTED) => true,
        (PHASE_PENDING, PHASE_EXPIRED) => true,
        (PHASE_ACCEPTED, PHASE_MIGRATING) => true,
        (PHASE_ACCEPTED, PHASE_REJECTED) => true,
        (PHASE_ACCEPTED, PHASE_EXPIRED) => true,
        (PHASE_ACCEPTED, PHASE_FAILED) => true,
        (PHASE_MIGRATING, PHASE_VERIFIED) => true,
        (PHASE_MIGRATING, PHASE_FAILED) => true,
        (PHASE_VERIFIED, PHASE_COMPLETED) => true,
        (PHASE_VERIFIED, PHASE_FAILED) => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "不允许的状态流转：{} → {}",
            phase_label(from),
            phase_label(to)
        ))
    }
}

pub fn phase_label(phase: &str) -> &'static str {
    match phase {
        PHASE_PENDING => "待接收方确认",
        PHASE_ACCEPTED => "接收方已接受",
        PHASE_MIGRATING => "迁移中",
        PHASE_VERIFIED => "已校验",
        PHASE_COMPLETED => "云端已生效",
        PHASE_REJECTED => "已拒绝",
        PHASE_EXPIRED => "已过期",
        PHASE_FAILED => "已失败",
        _ => "未知状态",
    }
}

// ── 视图 / 记录 ────────────────────────────────────────────────

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    pub id: String,
    pub org_id: String,
    pub direction: String,
    pub phase: String,
    pub phase_label: String,
    pub peer_device_id: String,
    pub package_hash: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcome {
    pub transfer_id: String,
    pub path: String,
    pub package_hash: String,
    pub size: u64,
    pub file_count: usize,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub transfer_id: String,
    pub org_id: String,
    pub package_hash: String,
    pub file_count: usize,
}

fn row_to_view(r: &rusqlite::Row<'_>) -> rusqlite::Result<TransferView> {
    let phase: String = r.get(3)?;
    Ok(TransferView {
        id: r.get(0)?,
        org_id: r.get(1)?,
        direction: r.get(2)?,
        phase_label: phase_label(&phase).to_string(),
        phase,
        peer_device_id: r.get(4)?,
        package_hash: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

pub fn list(conn: &Connection, org_id: &str) -> Result<Vec<TransferView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, org_id, direction, phase, peer_device_id, package_hash, created_at, updated_at
             FROM host_transfer WHERE org_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| format!("查询转移记录失败：{}", e))?;
    let rows = stmt
        .query_map(params![org_id], |r| row_to_view(r))
        .map_err(|e| format!("查询转移记录失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取转移记录失败：{}", e))
}

pub fn get(conn: &Connection, id: &str) -> Result<TransferView, String> {
    conn.query_row(
        "SELECT id, org_id, direction, phase, peer_device_id, package_hash, created_at, updated_at
         FROM host_transfer WHERE id = ?1",
        params![id],
        |r| row_to_view(r),
    )
    .optional()
    .map_err(|e| format!("读取转移记录失败：{}", e))?
    .ok_or_else(|| "转移记录不存在".to_string())
}

fn insert_record(
    conn: &Connection,
    org_id: &str,
    direction: &str,
    phase: &str,
    package_hash: &str,
) -> Result<String, String> {
    let id = crate::utils::new_id();
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO host_transfer
            (id, org_id, direction, phase, peer_device_id, package_hash, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?6)",
        params![id, org_id, direction, phase, package_hash, now],
    )
    .map_err(|e| format!("写入转移记录失败：{}", e))?;
    Ok(id)
}

/// 创建一条待接收方确认的转入记录（`pending`）。
pub fn create_incoming(conn: &Connection, org_id: &str) -> Result<TransferView, String> {
    let id = insert_record(conn, org_id, DIRECTION_IN, PHASE_PENDING, "")?;
    get(conn, &id)
}

/// 推进状态（含审计）。`verified / completed` 必须已有迁移包哈希。
pub fn advance(conn: &Connection, id: &str, to_phase: &str) -> Result<TransferView, String> {
    let cur = get(conn, id)?;
    can_advance(&cur.phase, to_phase)?;
    if matches!(to_phase, PHASE_VERIFIED | PHASE_COMPLETED) && cur.package_hash.trim().is_empty() {
        return Err("迁移包尚未校验，不能推进到该状态".to_string());
    }
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE host_transfer SET phase = ?1, updated_at = ?2 WHERE id = ?3",
        params![to_phase, now, id],
    )
    .map_err(|e| format!("更新转移状态失败：{}", e))?;

    let action = match to_phase {
        PHASE_ACCEPTED => "transfer.accept",
        PHASE_VERIFIED => "transfer.verify",
        PHASE_COMPLETED => "transfer.complete",
        PHASE_FAILED => "transfer.fail",
        _ => "transfer.advance",
    };
    let actor = authz::current_actor_id(conn).unwrap_or_default();
    let detail = json!({ "transferId": id, "from": cur.phase, "to": to_phase }).to_string();
    org::audit(conn, &cur.org_id, &actor, action, &detail)?;
    get(conn, id)
}

fn set_package_hash(conn: &Connection, id: &str, hash: &str) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE host_transfer SET package_hash = ?1, updated_at = ?2 WHERE id = ?3",
        params![hash, now, id],
    )
    .map_err(|e| format!("写入迁移包哈希失败：{}", e))?;
    Ok(())
}

// ── 迁移包：导出 ────────────────────────────────────────────────

const PKG_MAGIC: &[u8; 24] = b"PILOTDESK-ENCLAVE-PKG\0\0\0";
const PKG_VERSION: u32 = 1;
const KDF_ITERATIONS: u32 = 120_000;
/// 明文（库 + 文件）上限：超过请改用内网直连（本期为 MVP 内存实现）。
const MAX_PLAINTEXT_BYTES: usize = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct ManifestFile {
    path: String,
    sha256: String,
    size: u64,
    data: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    schema_version: i64,
    org_id: String,
    created_at: i64,
    db_sha256: String,
    db: String,
    files: Vec<ManifestFile>,
}

struct FileMeta {
    path: String,
    sha256: String,
}

/// 默认导出目录：`<应用根>/enclave/transfer`。
pub fn default_export_dir() -> PathBuf {
    crate::utils::paths::app_root_dir()
        .join("enclave")
        .join("transfer")
}

fn sanitize_component(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// 导出当前组织的加密迁移包。仅本机 owner 可发起。
pub fn export_package(
    conn: &Connection,
    org_id: &str,
    out_path: Option<&str>,
    passphrase: &str,
) -> Result<ExportOutcome, String> {
    if passphrase.chars().count() < 8 {
        return Err("迁移包口令至少 8 位".to_string());
    }
    let role = authz::role_for_org(conn, org_id)?
        .ok_or_else(|| "尚未同步该组织的治理快照，无法发起转移".to_string())?;
    if role != Role::Owner {
        return Err("仅组织所有者（owner）可发起主机转移".to_string());
    }
    let actor = authz::current_actor_id(conn)?;

    let tmp_dir = std::env::temp_dir().join(format!("pilotdesk-transfer-{}", crate::utils::new_id()));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("创建临时目录失败：{}", e))?;
    let tmp_db = tmp_dir.join("org.db");

    let result = (|| -> Result<ExportOutcome, String> {
        // 一致性快照（不阻塞当前连接上的读；WAL 模式下是事务快照）
        conn.execute("VACUUM INTO ?1", params![tmp_db.to_string_lossy().to_string()])
            .map_err(|e| format!("生成数据库快照失败：{}", e))?;

        // 只保留本组织数据：治理快照 / 本机身份 / 本机设置不随包走
        prune_snapshot_to_org(&tmp_db, org_id)?;

        let db_bytes = std::fs::read(&tmp_db).map_err(|e| format!("读取数据库快照失败：{}", e))?;
        let db_sha = hex_sha256(&db_bytes);

        // 文件资产：路径相对组织目录
        let mut metas: Vec<FileMeta> = Vec::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT file_path, file_hash FROM file_assets
                     WHERE owner_type = 'org' AND owner_id = ?1",
                )
                .map_err(|e| format!("查询文件资产失败：{}", e))?;
            let rows = stmt
                .query_map(params![org_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| format!("查询文件资产失败：{}", e))?;
            for row in rows {
                let (path, hash) = row.map_err(|e| format!("读取文件资产失败：{}", e))?;
                metas.push(FileMeta { path, sha256: hash });
            }
        }
        let root = files::resolve_team_files_root(conn)?.join(org_id);
        let mut files_out: Vec<ManifestFile> = Vec::with_capacity(metas.len());
        for m in metas {
            if !is_safe_relative_name(&m.path) {
                return Err(format!("文件资产路径不合法：{}", m.path));
            }
            let abs = root.join(&m.path);
            let bytes = std::fs::read(&abs).map_err(|e| {
                format!("读取团队文件失败（{}）：{}。请确认团队文件根可访问。", abs.display(), e)
            })?;
            let sha = hex_sha256(&bytes);
            if !m.sha256.is_empty() && m.sha256 != sha {
                return Err(format!("团队文件校验失败（{}）：记录哈希与实物不一致", m.path));
            }
            files_out.push(ManifestFile {
                path: m.path,
                sha256: sha,
                size: bytes.len() as u64,
                data: BASE64.encode(bytes),
            });
        }

        let manifest = Manifest {
            format: "pilotdesk-enclave-transfer".to_string(),
            version: PKG_VERSION,
            schema_version: conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap_or(0),
            org_id: org_id.to_string(),
            created_at: chrono::Utc::now().timestamp(),
            db_sha256: db_sha,
            db: BASE64.encode(&db_bytes),
            files: files_out,
        };
        let plaintext =
            serde_json::to_vec(&manifest).map_err(|e| format!("序列化迁移包失败：{}", e))?;
        if plaintext.len() > MAX_PLAINTEXT_BYTES {
            return Err(format!(
                "迁移包过大（{} MB），请改用内网直连迁移",
                plaintext.len() / 1024 / 1024
            ));
        }

        let envelope = encrypt_envelope(&plaintext, passphrase)?;
        let package_hash = hex_sha256(&envelope);

        let path = resolve_export_path(out_path, org_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建导出目录失败：{}", e))?;
        }
        std::fs::write(&path, &envelope).map_err(|e| format!("写入迁移包失败：{}", e))?;

        let transfer_id =
            insert_record(conn, org_id, DIRECTION_OUT, PHASE_MIGRATING, &package_hash)?;
        org::audit(
            conn,
            org_id,
            &actor,
            "transfer.export",
            &json!({ "transferId": transfer_id, "packageHash": package_hash, "fileCount": manifest.files.len() })
                .to_string(),
        )?;

        Ok(ExportOutcome {
            transfer_id,
            path: path.to_string_lossy().to_string(),
            package_hash,
            size: envelope.len() as u64,
            file_count: manifest.files.len(),
        })
    })();

    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

fn resolve_export_path(out: Option<&str>, org_id: &str) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let default_name = format!(
        "enclave-{}-{}.pdesk",
        sanitize_component(org_id),
        stamp
    );
    match out.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let p = PathBuf::from(raw);
            if p.is_dir() {
                p.join(default_name)
            } else if p.extension().is_none() {
                p.with_extension("pdesk")
            } else {
                p
            }
        }
        None => default_export_dir().join(default_name),
    }
}

/// 把快照裁剪为「单组织 + 忽略治理/本机设置」。
fn prune_snapshot_to_org(db_path: &Path, org_id: &str) -> Result<(), String> {
    let conn = Connection::open(db_path).map_err(|e| format!("打开数据库快照失败：{}", e))?;
    conn.execute_batch("PRAGMA foreign_keys=OFF;")
        .map_err(|e| format!("准备裁剪快照失败：{}", e))?;
    let stmts = [
        // 任务派生物（先删子表，按任务集合过滤）
        "DELETE FROM report_feedback WHERE report_id NOT IN (
             SELECT r.id FROM task_reports r JOIN org_tasks t ON t.id = r.task_id WHERE t.org_id = ?1
         )",
        "DELETE FROM task_comments WHERE task_id NOT IN (SELECT id FROM org_tasks WHERE org_id = ?1)",
        "DELETE FROM task_reports WHERE task_id NOT IN (SELECT id FROM org_tasks WHERE org_id = ?1)",
        "DELETE FROM task_file_links WHERE task_id NOT IN (SELECT id FROM org_tasks WHERE org_id = ?1)",
        "DELETE FROM task_files WHERE task_id NOT IN (SELECT id FROM org_tasks WHERE org_id = ?1)",
        "DELETE FROM org_tasks WHERE org_id <> ?1",
        // 文件资产
        "DELETE FROM file_assets WHERE NOT (owner_type = 'org' AND owner_id = ?1)",
        // 知识库
        "DELETE FROM knowledge_base_files WHERE knowledge_base_id NOT IN (SELECT id FROM knowledge_bases WHERE owner_type = 'org' AND owner_id = ?1)",
        "DELETE FROM knowledge_base_memories WHERE knowledge_base_id NOT IN (SELECT id FROM knowledge_bases WHERE owner_type = 'org' AND owner_id = ?1)",
        "DELETE FROM knowledge_bases WHERE NOT (owner_type = 'org' AND owner_id = ?1)",
        // 其它组织域
        "DELETE FROM notifications WHERE org_id <> ?1",
        "DELETE FROM resource_roles WHERE org_id <> ?1",
        "DELETE FROM org_policy WHERE org_id <> ?1",
        "DELETE FROM host_emergency WHERE org_id <> ?1",
        "DELETE FROM policy_audit WHERE org_id <> ?1",
    ];
    for sql in stmts {
        conn.execute(sql, params![org_id])
            .map_err(|e| format!("裁剪数据库快照失败：{}", e))?;
    }
    // 本机记录 / 治理快照不随包走
    conn.execute_batch(
        "DELETE FROM host_transfer;
         DELETE FROM org_snapshot;
         DELETE FROM member_snapshot;
         DELETE FROM local_identity;
         DELETE FROM enclave_settings;",
    )
    .map_err(|e| format!("裁剪本机数据失败：{}", e))?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| format!("整理数据库快照失败：{}", e))?;
    Ok(())
}

// ── 迁移包：导入 ────────────────────────────────────────────────

/// 导入加密迁移包：解密 → 校验 → 合并进本机 `enclave.db` → 落文件。
///
/// 成功后自动建立一条 `in` 转移记录并推进到 `verified`（迁移完成先于云端生效）。
pub fn import_package(
    conn: &Connection,
    path: &str,
    passphrase: &str,
) -> Result<ImportOutcome, String> {
    let raw = std::fs::read(path).map_err(|e| format!("读取迁移包失败：{}", e))?;
    let (plaintext, _iterations) = decrypt_envelope(&raw, passphrase)?;
    let manifest: Manifest =
        serde_json::from_slice(&plaintext).map_err(|e| format!("迁移包内容损坏：{}", e))?;
    if manifest.format != "pilotdesk-enclave-transfer" || manifest.version != PKG_VERSION {
        return Err("迁移包格式或版本不受支持".to_string());
    }
    let org_id = manifest.org_id.trim().to_string();
    if org_id.is_empty() {
        return Err("迁移包未包含组织标识".to_string());
    }
    let current_schema: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if manifest.schema_version != current_schema {
        return Err(format!(
            "迁移包数据版本（{}）与当前应用（{}）不一致，请使用相同版本应用完成转移",
            manifest.schema_version, current_schema
        ));
    }

    let actor = authz::current_actor_id(conn)?;
    let role = authz::role_for_member(conn, &org_id, &actor)?
        .ok_or_else(|| "当前账号不是该组织成员，不能接收该迁移包".to_string())?;
    if role == Role::Owner {
        let existing: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM org_tasks WHERE org_id = ?1",
                params![org_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if existing > 0 {
            return Err("本机已是该组织主机且已有业务数据，拒绝重复导入".to_string());
        }
    }

    let db_bytes = BASE64
        .decode(manifest.db.as_bytes())
        .map_err(|e| format!("迁移包数据库解码失败：{}", e))?;
    if hex_sha256(&db_bytes) != manifest.db_sha256 {
        return Err("迁移包数据库校验失败（哈希不一致）".to_string());
    }

    // 文件先校验、后落盘（任一处失败不产生半成品）
    if !is_safe_relative_name(&org_id) {
        return Err("迁移包组织标识不合法".to_string());
    }
    let org_dir = files::org_dir(conn, &org_id)?;
    for f in &manifest.files {
        if !is_safe_relative_name(&f.path) {
            return Err(format!("迁移包文件路径不合法：{}", f.path));
        }
        let bytes = BASE64
            .decode(f.data.as_bytes())
            .map_err(|e| format!("迁移包文件解码失败（{}）：{}", f.path, e))?;
        if bytes.len() as u64 != f.size || hex_sha256(&bytes) != f.sha256 {
            return Err(format!("迁移包文件校验失败（{}）", f.path));
        }
    }
    for f in &manifest.files {
        let bytes = BASE64
            .decode(f.data.as_bytes())
            .map_err(|e| format!("迁移包文件解码失败（{}）：{}", f.path, e))?;
        let target = org_dir.join(&f.path);
        if target.exists() {
            let existing = std::fs::read(&target).unwrap_or_default();
            if hex_sha256(&existing) == f.sha256 {
                continue;
            }
        }
        std::fs::write(&target, &bytes)
            .map_err(|e| format!("写入团队文件失败（{}）：{}", target.display(), e))?;
    }

    // 合并数据库：ATTACH 后在事务内 INSERT OR REPLACE（同版本 schema，列集一致）
    let tmp_db = std::env::temp_dir().join(format!("pilotdesk-import-{}.db", crate::utils::new_id()));
    std::fs::write(&tmp_db, &db_bytes).map_err(|e| format!("写入临时数据库失败：{}", e))?;
    let copy_result = (|| -> Result<(), String> {
        conn.execute("ATTACH DATABASE ?1 AS src", params![tmp_db.to_string_lossy().to_string()])
            .map_err(|e| format!("挂载迁移数据库失败：{}", e))?;
        let result = (|| -> Result<(), String> {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("开启事务失败：{}", e))?;
            let copies = [
                "INSERT OR REPLACE INTO main.org_tasks SELECT * FROM src.org_tasks WHERE org_id = ?1",
                "INSERT OR REPLACE INTO main.task_reports SELECT * FROM src.task_reports WHERE task_id IN (SELECT id FROM src.org_tasks WHERE org_id = ?1)",
                "INSERT OR REPLACE INTO main.task_comments SELECT * FROM src.task_comments WHERE task_id IN (SELECT id FROM src.org_tasks WHERE org_id = ?1)",
                "INSERT OR REPLACE INTO main.report_feedback SELECT * FROM src.report_feedback WHERE report_id IN (SELECT id FROM src.task_reports WHERE task_id IN (SELECT id FROM src.org_tasks WHERE org_id = ?1))",
                "INSERT OR REPLACE INTO main.task_files SELECT * FROM src.task_files WHERE task_id IN (SELECT id FROM src.org_tasks WHERE org_id = ?1)",
                "INSERT OR REPLACE INTO main.task_file_links SELECT * FROM src.task_file_links WHERE task_id IN (SELECT id FROM src.org_tasks WHERE org_id = ?1)",
                "INSERT OR REPLACE INTO main.file_assets SELECT * FROM src.file_assets WHERE owner_type = 'org' AND owner_id = ?1",
                "INSERT OR REPLACE INTO main.knowledge_bases SELECT * FROM src.knowledge_bases WHERE owner_type = 'org' AND owner_id = ?1",
                "INSERT OR REPLACE INTO main.knowledge_base_memories SELECT * FROM src.knowledge_base_memories WHERE knowledge_base_id IN (SELECT id FROM src.knowledge_bases WHERE owner_type = 'org' AND owner_id = ?1)",
                "INSERT OR REPLACE INTO main.knowledge_base_files SELECT * FROM src.knowledge_base_files WHERE knowledge_base_id IN (SELECT id FROM src.knowledge_bases WHERE owner_type = 'org' AND owner_id = ?1)",
                "INSERT OR REPLACE INTO main.notifications SELECT * FROM src.notifications WHERE org_id = ?1",
                "INSERT OR REPLACE INTO main.resource_roles SELECT * FROM src.resource_roles WHERE org_id = ?1",
                "INSERT OR REPLACE INTO main.org_policy SELECT * FROM src.org_policy WHERE org_id = ?1",
                "INSERT OR REPLACE INTO main.policy_audit SELECT * FROM src.policy_audit WHERE org_id = ?1",
            ];
            for sql in copies {
                tx.execute(sql, params![org_id])
                    .map_err(|e| format!("合并迁移数据失败：{}", e))?;
            }
            tx.commit().map_err(|e| format!("提交迁移数据失败：{}", e))?;
            Ok(())
        })();
        let _ = conn.execute_batch("DETACH DATABASE src");
        result
    })();
    let _ = std::fs::remove_file(&tmp_db);
    copy_result?;

    // 接收方记录：pending → accepted → migrating → verified
    let transfer_id = insert_record(conn, &org_id, DIRECTION_IN, PHASE_PENDING, "")?;
    let package_hash = hex_sha256(&raw);
    set_package_hash(conn, &transfer_id, &package_hash)?;
    advance(conn, &transfer_id, PHASE_ACCEPTED)?;
    advance(conn, &transfer_id, PHASE_MIGRATING)?;
    advance(conn, &transfer_id, PHASE_VERIFIED)?;
    org::audit(
        conn,
        &org_id,
        &actor,
        "transfer.import",
        &json!({ "transferId": transfer_id, "packageHash": package_hash, "fileCount": manifest.files.len() })
            .to_string(),
    )?;

    Ok(ImportOutcome {
        transfer_id,
        org_id,
        package_hash,
        file_count: manifest.files.len(),
    })
}

// ── 旧主机销毁 ──────────────────────────────────────────────────

/// 销毁当前组织的全部团队数据（仅 `completed` 的转出记录可调用）。
///
/// 顺序：先删团队文件目录（失败即中止）→ 再删业务行 → 写审计 `transfer.data_destroyed`。
pub fn destroy_org_data(conn: &Connection, org_id: &str, transfer_id: &str) -> Result<(), String> {
    let t = get(conn, transfer_id)?;
    if t.org_id != org_id {
        return Err("转移记录与组织不匹配".to_string());
    }
    if t.direction != DIRECTION_OUT {
        return Err("仅转出方（旧主机）可销毁数据".to_string());
    }
    if t.phase != PHASE_COMPLETED {
        return Err("云端尚未生效（completed），不能销毁旧主机数据".to_string());
    }
    let actor = authz::current_actor_id(conn)?;

    let dir = files::resolve_team_files_root(conn)?.join(org_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("删除团队文件失败（{}）：{}", dir.display(), e))?;
    }

    let stmts = [
        "DELETE FROM report_feedback WHERE report_id IN (SELECT id FROM task_reports WHERE task_id IN (SELECT id FROM org_tasks WHERE org_id = ?1))",
        "DELETE FROM task_comments WHERE task_id IN (SELECT id FROM org_tasks WHERE org_id = ?1)",
        "DELETE FROM task_reports WHERE task_id IN (SELECT id FROM org_tasks WHERE org_id = ?1)",
        "DELETE FROM task_file_links WHERE task_id IN (SELECT id FROM org_tasks WHERE org_id = ?1)",
        "DELETE FROM task_files WHERE task_id IN (SELECT id FROM org_tasks WHERE org_id = ?1)",
        "DELETE FROM org_tasks WHERE org_id = ?1",
        "DELETE FROM knowledge_base_files WHERE knowledge_base_id IN (SELECT id FROM knowledge_bases WHERE owner_type = 'org' AND owner_id = ?1)",
        "DELETE FROM knowledge_base_memories WHERE knowledge_base_id IN (SELECT id FROM knowledge_bases WHERE owner_type = 'org' AND owner_id = ?1)",
        "DELETE FROM knowledge_bases WHERE owner_type = 'org' AND owner_id = ?1",
        "DELETE FROM file_assets WHERE owner_type = 'org' AND owner_id = ?1",
        "DELETE FROM notifications WHERE org_id = ?1",
        "DELETE FROM resource_roles WHERE org_id = ?1",
        "DELETE FROM org_policy WHERE org_id = ?1",
        "DELETE FROM host_emergency WHERE org_id = ?1",
    ];
    for sql in stmts {
        conn.execute(sql, params![org_id])
            .map_err(|e| format!("销毁团队数据失败：{}", e))?;
    }
    org::audit(
        conn,
        org_id,
        &actor,
        "transfer.data_destroyed",
        &json!({ "transferId": transfer_id }).to_string(),
    )?;
    Ok(())
}

// ── 加密 / KDF ─────────────────────────────────────────────────

fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// HMAC-SHA256（PBKDF2 的内部原语；不引入额外依赖）。
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = Sha256::digest(key);
        k[..32].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    let out = outer.finalize();
    let mut result = [0u8; 32];
    result.copy_from_slice(&out);
    result
}

/// PBKDF2-HMAC-SHA256（dkLen = 32，单块）。
fn pbkdf2_sha256(passphrase: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut first = Vec::with_capacity(salt.len() + 4);
    first.extend_from_slice(salt);
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac_sha256(passphrase, &first);
    let mut out = u;
    for _ in 1..iterations.max(1) {
        u = hmac_sha256(passphrase, &u);
        for i in 0..32 {
            out[i] ^= u[i];
        }
    }
    out
}

fn encrypt_envelope(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>, String> {
    let mut salt = [0u8; 16];
    let mut nonce_bytes = [0u8; 12];
    let mut rng = OsRng;
    rng.fill_bytes(&mut salt);
    rng.fill_bytes(&mut nonce_bytes);

    let key = pbkdf2_sha256(passphrase.as_bytes(), &salt, KDF_ITERATIONS);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("创建加密器失败：{}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("加密迁移包失败：{}", e))?;

    let mut out = Vec::with_capacity(92 + ciphertext.len());
    out.extend_from_slice(PKG_MAGIC);
    out.extend_from_slice(&PKG_VERSION.to_le_bytes());
    out.extend_from_slice(&KDF_ITERATIONS.to_le_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&Sha256::digest(plaintext));
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_envelope(raw: &[u8], passphrase: &str) -> Result<(Vec<u8>, u32), String> {
    const HEADER: usize = 24 + 4 + 4 + 16 + 12 + 32;
    if raw.len() <= HEADER || raw[..24] != PKG_MAGIC[..] {
        return Err("不是有效的飞地迁移包".to_string());
    }
    let version = u32::from_le_bytes(raw[24..28].try_into().unwrap_or_default());
    if version != PKG_VERSION {
        return Err(format!("迁移包版本不受支持：{}", version));
    }
    let iterations = u32::from_le_bytes(raw[28..32].try_into().unwrap_or_default());
    let salt = &raw[32..48];
    let nonce_bytes = &raw[48..60];
    let expected = &raw[60..92];
    let ciphertext = &raw[92..];

    let key = pbkdf2_sha256(passphrase.as_bytes(), salt, iterations);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("创建解密器失败：{}", e))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "解密失败：口令不正确或迁移包已损坏".to_string())?;
    if Sha256::digest(&plaintext).as_slice() != expected {
        return Err("迁移包完整性校验失败".to_string());
    }
    Ok((plaintext, iterations))
}

fn is_safe_relative_name(name: &str) -> bool {
    if name.trim().is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    Path::new(name).components().count() == 1
}

// ── 测试 ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enclave::db::EnclaveStore;
    use std::sync::Arc;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pilotdesk-transfer-test-{}-{}", tag, crate::utils::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open_store(dir: &Path) -> Arc<EnclaveStore> {
        let store = Arc::new(EnclaveStore::open(&dir.join("enclave.db")).unwrap());
        // 测试隔离：团队文件根指向临时目录，避免写入真实应用数据
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT OR REPLACE INTO enclave_settings (key, value, updated_at)
                     VALUES (?1, ?2, 0)",
                    params![files::TEAM_FILES_ROOT_KEY, dir.to_string_lossy().to_string()],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
        store
    }

    fn seed_host(store: &EnclaveStore, org: &str, actor: &str) {
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO local_identity (user_id, email) VALUES (?1, 'host@example.com')",
                    params![actor],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id)
                     VALUES (?1, 'Org', ?2, 'owner', ?2)",
                    params![org, actor],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
    }

    fn seed_member(store: &EnclaveStore, org: &str, actor: &str) {
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO local_identity (user_id, email) VALUES (?1, 'member@example.com')",
                    params![actor],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO member_snapshot (org_id, user_id, role, display_name)
                     VALUES (?1, ?2, 'member', 'Member')",
                    params![org, actor],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
    }

    fn seed_task_and_file(store: &EnclaveStore, org: &str) {
        store
            .with_conn(|c| {
                let now = chrono::Utc::now().timestamp();
                c.execute(
                    "INSERT INTO org_tasks (id, org_id, title, creator_id, created_at, updated_at)
                     VALUES ('t1', ?1, '任务一', '1', ?2, ?2)",
                    params![org, now],
                )
                .map_err(|e| e.to_string())?;
                let dir = files::org_dir(c, org).unwrap();
                let path = dir.join("abc12345-report.md");
                std::fs::write(&path, b"hello").unwrap();
                let hash = hex_sha256(b"hello");
                c.execute(
                    "INSERT INTO file_assets (id, file_name, file_path, file_size, file_hash,
                        owner_type, owner_id, created_at, updated_at)
                     VALUES ('f1', 'report.md', 'abc12345-report.md', 5, ?1, 'org', ?2, ?3, ?3)",
                    params![hash, org, now],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn state_machine_truth_table() {
        assert!(can_advance(PHASE_PENDING, PHASE_ACCEPTED).is_ok());
        assert!(can_advance(PHASE_ACCEPTED, PHASE_MIGRATING).is_ok());
        assert!(can_advance(PHASE_MIGRATING, PHASE_VERIFIED).is_ok());
        assert!(can_advance(PHASE_VERIFIED, PHASE_COMPLETED).is_ok());
        assert!(can_advance(PHASE_PENDING, PHASE_COMPLETED).is_err());
        assert!(can_advance(PHASE_ACCEPTED, PHASE_VERIFIED).is_err());
        // 关键不变量：未校验不得云端生效
        assert!(can_advance(PHASE_MIGRATING, PHASE_COMPLETED).is_err());
        assert!(can_advance(PHASE_COMPLETED, PHASE_FAILED).is_err());
        assert!(can_advance(PHASE_FAILED, PHASE_COMPLETED).is_err());
    }

    #[test]
    fn export_import_roundtrip_and_tamper() {
        let host_dir = temp_dir("host");
        let recv_dir = temp_dir("recv");
        let host = open_store(&host_dir);
        let recv = open_store(&recv_dir);
        seed_host(&host, "org1", "1");
        seed_member(&recv, "org1", "2");
        seed_task_and_file(&host, "org1");

        let out_path = host_dir.join("pkg.pdesk");
        let exp = host
            .with_conn(|c| export_package(c, "org1", Some(out_path.to_str().unwrap()), "pass-123456"))
            .unwrap();
        assert_eq!(exp.file_count, 1);
        assert!(Path::new(&exp.path).exists());

        // 错误口令
        let err = recv
            .with_conn(|c| import_package(c, &exp.path, "wrong-pass"))
            .unwrap_err();
        assert!(err.contains("口令"));

        // 篡改
        let mut tampered = std::fs::read(&exp.path).unwrap();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        let bad = host_dir.join("bad.pdesk");
        std::fs::write(&bad, &tampered).unwrap();
        let err = recv
            .with_conn(|c| import_package(c, bad.to_str().unwrap(), "pass-123456"))
            .unwrap_err();
        assert!(err.contains("损坏") || err.contains("解密"));

        // 正常导入
        let imp = recv
            .with_conn(|c| import_package(c, &exp.path, "pass-123456"))
            .unwrap();
        assert_eq!(imp.org_id, "org1");
        recv.with_conn(|c| {
            let tasks: i64 = c
                .query_row("SELECT COUNT(*) FROM org_tasks WHERE org_id = 'org1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(tasks, 1);
            let rec = get(c, &imp.transfer_id).unwrap();
            assert_eq!(rec.phase, PHASE_VERIFIED);
            let dir = files::org_dir(c, "org1").unwrap();
            assert!(dir.join("abc12345-report.md").exists());
            Ok(())
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(&host_dir);
        let _ = std::fs::remove_dir_all(&recv_dir);
    }

    #[test]
    fn destroy_requires_completed_and_writes_audit() {
        let dir = temp_dir("destroy");
        let store = open_store(&dir);
        seed_host(&store, "org1", "1");
        seed_task_and_file(&store, "org1");

        let out_path = dir.join("pkg.pdesk");
        let exp = store
            .with_conn(|c| export_package(c, "org1", Some(out_path.to_str().unwrap()), "pass-123456"))
            .unwrap();

        let err = store
            .with_conn(|c| destroy_org_data(c, "org1", &exp.transfer_id))
            .unwrap_err();
        assert!(err.contains("completed") || err.contains("云端尚未生效"));

        store
            .with_conn(|c| {
                advance(c, &exp.transfer_id, PHASE_VERIFIED)?;
                advance(c, &exp.transfer_id, PHASE_COMPLETED)?;
                Ok(())
            })
            .unwrap();
        store
            .with_conn(|c| destroy_org_data(c, "org1", &exp.transfer_id))
            .unwrap();

        store
            .with_conn(|c| {
                let tasks: i64 = c
                    .query_row("SELECT COUNT(*) FROM org_tasks WHERE org_id = 'org1'", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(tasks, 0);
                let audited: i64 = c
                    .query_row(
                        "SELECT COUNT(*) FROM policy_audit WHERE org_id = 'org1' AND action = 'transfer.data_destroyed'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(audited, 1);
                Ok(())
            })
            .unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }
}
