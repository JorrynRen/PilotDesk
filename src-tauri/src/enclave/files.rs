//! 飞地文件资产：落盘、去重、与任务关联。
//!
//! **存储布局（§13 架构决策）**：`<本机团队文件根目录>/<orgId>/`，与个人知识库完全独立。
//! 本机团队文件根目录默认 `app_root/enclave/files`，可由 owner 改为本地固定盘绝对路径
//! （存 `enclave_settings.team_files_root`，随库迁移）。
//!
//! 文件按 **sha256** 命名（`<sha8>-<原文件名>`）实现组织内去重；`file_assets.file_path`
//! 存的是**相对本组织目录**的文件名，因此根目录迁移/主机转移都只需搬目录。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `enclave_settings` 中保存团队文件**基础目录**的键。
///
/// 语义：用户选的是基础目录，实际文件根 = `<基础目录>/enclave/files`（与默认布局一致）。
pub const TEAM_FILES_ROOT_KEY: &str = "team_files_root";

/// 默认基础目录（应用根目录）：其下同样是 `enclave/files`。
pub fn default_base() -> PathBuf {
    crate::utils::paths::app_root_dir()
}

/// 去掉 Windows 扩展长度前缀（`\\?\C:\...` / `\\?\UNC\srv\share`），并去除首尾引号。
fn normalize_path(raw: &str) -> String {
    let s = raw.trim().trim_matches('"');
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else {
        s.to_string()
    };
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// 读团队文件**基础目录**；未设置则用应用根目录。
pub fn resolve_team_base(conn: &Connection) -> Result<PathBuf, String> {
    let custom: Option<String> = conn
        .query_row(
            "SELECT value FROM enclave_settings WHERE key = ?1",
            params![TEAM_FILES_ROOT_KEY],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("读取本机团队文件根失败：{}", e))?;
    Ok(custom
        .map(|s| normalize_path(&s))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .map(strip_effective_suffix)
        .unwrap_or_else(default_base))
}

/// 兼容早期版本把「实际文件根」直接存进该键的情况：若以 `enclave/files` 结尾，回退到其父目录。
fn strip_effective_suffix(p: PathBuf) -> PathBuf {
    if p.ends_with(Path::new("enclave").join("files")) {
        p.parent()
            .and_then(|x| x.parent())
            .map(|x| x.to_path_buf())
            .unwrap_or(p)
    } else {
        p
    }
}

/// 团队文件**实际根目录**：`<基础目录>/enclave/files`（与默认布局一致）。
pub fn resolve_team_files_root(conn: &Connection) -> Result<PathBuf, String> {
    Ok(resolve_team_base(conn)?.join("enclave").join("files"))
}

/// 本组织的文件目录（不存在则创建）。
///
/// **盘不可用时返回错误**，绝不静默回退到默认目录（避免文件分叉）。
pub fn org_dir(conn: &Connection, org_id: &str) -> Result<PathBuf, String> {
    let root = resolve_team_files_root(conn)?;
    let dir = root.join(org_id);
    std::fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "本机团队文件根不可用（{}）：{}。请检查磁盘是否在线或调整本机团队文件根目录。",
            dir.display(),
            e
        )
    })?;
    Ok(dir)
}

/// 判断两路径是否重叠（相等 / 互为父子）。
fn paths_overlap(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// `std::fs::canonicalize` 在 Windows 上会返回 `\\?\C:\...`（扩展长度前缀），
/// 这里统一归一化后再用，避免把前缀写进设置 / 审计、也避免路径比较误判。
fn canonical_normalized(p: &Path) -> PathBuf {
    match std::fs::canonicalize(p) {
        Ok(c) => {
            let raw = c.to_string_lossy();
            let clean = normalize_path(&raw);
            PathBuf::from(clean)
        }
        Err(_) => p.to_path_buf(),
    }
}

/// 校验并创建新的团队文件**基础目录**（实际文件根 = `<基础目录>/enclave/files`）。
///
/// 规则（§13）：绝对路径、拒绝 UNC/网络盘、不得与个人知识库根重叠、可创建。
/// 兼容系统目录选择器返回的 `\\?\C:\...` 形式（自动去前缀）。
pub fn validate_base(conn: &Connection, raw: &str) -> Result<PathBuf, String> {
    let normalized = normalize_path(raw);
    if normalized.trim().is_empty() {
        return Err("请填写本机团队文件根目录".to_string());
    }
    if normalized.starts_with("\\\\") || normalized.starts_with("//") {
        return Err("不支持网络路径（UNC）：团队数据必须落在本机固定盘".to_string());
    }
    let p = PathBuf::from(normalized.trim());
    if !p.is_absolute() {
        return Err("请填写绝对路径".to_string());
    }
    std::fs::create_dir_all(&p)
        .map_err(|e| format!("目录不可用（{}）：{}", p.display(), e))?;
    let canonical = canonical_normalized(&p);

    // 与个人知识库根比对：基础目录本身、以及其下的 enclave/files 都不能与个人库重叠
    let kb_root = crate::utils::paths::resolve_knowledge_root(conn);
    let kb_canonical = canonical_normalized(&kb_root);
    let effective = canonical.join("enclave").join("files");
    if paths_overlap(&canonical, &kb_canonical) || paths_overlap(&effective, &kb_canonical) {
        return Err(format!(
            "不能与个人知识库目录重叠（{}）",
            kb_canonical.display()
        ));
    }
    Ok(canonical)
}

/// 设置团队文件**基础目录**并搬迁既有文件（复制 → 逐文件 sha256 校验 → 切换 → 删旧）。
pub fn set_base(conn: &Connection, new_base: &Path) -> Result<(), String> {
    let old_root = resolve_team_files_root(conn)?;
    let new_root = new_base.join("enclave").join("files");
    let old_canonical = canonical_normalized(&old_root);
    let new_canonical = canonical_normalized(&new_root);
    if old_canonical == new_canonical {
        save_base(conn, new_base)?;
        return Ok(());
    }

    // 收集需要搬迁的 (org_id, file_path, file_hash)
    let mut stmt = conn
        .prepare(
            "SELECT owner_id, file_path, file_hash FROM file_assets
             WHERE owner_type = 'org' AND owner_id <> ''",
        )
        .map_err(|e| format!("读取文件资产失败：{}", e))?;
    let assets: Vec<(String, String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("读取文件资产失败：{}", e))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取文件资产失败：{}", e))?;

    let mut copied: Vec<PathBuf> = Vec::new();
    let mut touched_orgs: Vec<String> = Vec::new();
    for (org_id, rel, hash) in &assets {
        let src = old_root.join(org_id).join(rel);
        let dst = new_root.join(org_id).join(rel);
        if !src.exists() {
            // 原文缺失（历史上手动删过）：跳过，不阻塞搬迁
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败（{}）：{}", parent.display(), e))?;
        }
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("复制文件失败（{}）：{}", src.display(), e))?;
        let actual = hash_file(&dst)?;
        if !hash.is_empty() && actual != *hash {
            // 校验失败：回滚已复制的文件，保持原设置
            let _ = std::fs::remove_file(&dst);
            for f in &copied {
                let _ = std::fs::remove_file(f);
            }
            return Err(format!("文件校验失败，已回滚：{}", src.display()));
        }
        copied.push(dst);
        if !touched_orgs.contains(org_id) {
            touched_orgs.push(org_id.clone());
        }
    }

    save_base(conn, new_base)?;

    // 切换成功后才删旧目录（按组织整目录删，避免误伤）
    for org_id in touched_orgs {
        let old_dir = old_root.join(&org_id);
        if old_dir.exists() {
            let _ = std::fs::remove_dir_all(&old_dir);
        }
    }
    Ok(())
}

fn save_base(conn: &Connection, base: &Path) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO enclave_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![TEAM_FILES_ROOT_KEY, base.to_string_lossy(), now],
    )
    .map_err(|e| format!("保存本机团队文件根失败：{}", e))?;
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("读取文件失败（{}）：{}", path.display(), e))?;
    Ok(hex_sha256(&bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// 只保留文件名本身，剔除路径分隔符与首尾空白。
fn safe_file_name(raw: &str) -> String {
    let name = Path::new(raw)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let name = name.trim().replace(['/', '\\'], "_");
    if name.is_empty() {
        "file".to_string()
    } else {
        name
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAssetView {
    pub id: String,
    pub file_name: String,
    pub file_type: String,
    pub file_size: i64,
    pub file_hash: String,
    pub uploaded_by: String,
    pub uploaded_at: i64,
    /// 绝对路径（供「打开/另存」直接使用）
    pub abs_path: String,
    /// 挂载到完成汇报（`report_id`）时非空
    pub report_id: Option<String>,
    /// 挂载到过程记录（`progress_id`）时非空
    pub progress_id: Option<String>,
}

/// 把本机文件挂到任务或完成汇报上（内容按 sha256 去重，同一内容在组织内只落一份）。
///
/// `report_id`：关联到完成汇报（仅草稿/退回状态可挂）。
/// 过程记录附件**只能随记录一次性提交**，不走这里（见 [`attach_progress_file`]）。
pub fn attach_file(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    src_path: &str,
    report_id: Option<&str>,
) -> Result<FileAssetView, String> {
    attach_file_inner(conn, org_id, actor, task_id, src_path, report_id, None, false)
}

/// 过程记录创建时随记录写入附件（内部入口，提交后不可再调用）。
pub(crate) fn attach_progress_file(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    src_path: &str,
    progress_id: &str,
) -> Result<FileAssetView, String> {
    attach_file_inner(
        conn,
        org_id,
        actor,
        task_id,
        src_path,
        None,
        Some(progress_id),
        true,
    )
}

fn attach_file_inner(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    src_path: &str,
    report_id: Option<&str>,
    progress_id: Option<&str>,
    allow_progress: bool,
) -> Result<FileAssetView, String> {
    // 任务必须存在且属于本组织；只有参与者（创建者/负责人/协作者）可上传
    let role = crate::enclave::tasks::actor_task_role(conn, org_id, task_id, actor)?;
    if !crate::enclave::tasks::is_participant(role) {
        return Err("只有任务创建者、负责人或参与者可以上传附件".to_string());
    }
    if crate::enclave::tasks::is_closed_status(&crate::enclave::tasks::task_status(
        conn, org_id, task_id,
    )?) {
        return Err("任务已完成、归档或作废，不能再调整附件".to_string());
    }
    if progress_id.is_some() && !allow_progress {
        return Err("过程记录提交后不可再补充附件，请在新建记录时一并提交".to_string());
    }
    if report_id.is_some() && progress_id.is_some() {
        return Err("附件只能挂载到一处（任务资料 / 过程记录 / 完成汇报）".to_string());
    }
    if let Some(pid) = progress_id {
        let ok: Option<String> = conn
            .query_row(
                "SELECT id FROM task_comments
                 WHERE id = ?1 AND task_id = ?2 AND content_type = 'progress' AND deleted_at IS NULL",
                params![pid, task_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("查询过程记录失败：{}", e))?;
        if ok.is_none() {
            return Err("过程记录不存在或不属于该任务".to_string());
        }
    }
    if let Some(rid) = report_id {
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM task_reports WHERE id = ?1 AND task_id = ?2",
                params![rid, task_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("查询完成汇报失败：{}", e))?;
        match status.as_deref() {
            Some("draft") => {}
            Some("returned") => {
                return Err("汇报已退回：请先「继续修改」转为草稿后再调整交付物".to_string())
            }
            Some(_) => return Err("汇报已提交，不能再增删交付物".to_string()),
            None => return Err("完成汇报不存在或不属于该任务".to_string()),
        }
    }

    let src = PathBuf::from(src_path);
    let bytes = std::fs::read(&src)
        .map_err(|e| format!("读取待上传文件失败（{}）：{}", src.display(), e))?;
    let hash = hex_sha256(&bytes);
    let dir = org_dir(conn, org_id)?;
    let orig_name = safe_file_name(&src.to_string_lossy());
    let file_type = src
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let stored = format!("{}-{}", &hash[..8.min(hash.len())], orig_name);

    // 组织内已有同样内容 → 复用已有资产行
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT id, file_path FROM file_assets
             WHERE owner_type='org' AND owner_id=?1 AND file_hash=?2 LIMIT 1",
            params![org_id, hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("查询文件资产失败：{}", e))?;

    let (file_id, rel_path) = match existing {
        Some((id, rel)) if dir.join(&rel).exists() => (id, rel),
        _ => {
            let rel = stored;
            let abs = dir.join(&rel);
            std::fs::write(&abs, &bytes)
                .map_err(|e| format!("写入团队文件失败（{}）：{}", abs.display(), e))?;
            let id = crate::utils::new_id();
            let now = chrono::Utc::now().timestamp();
            conn.execute(
                "INSERT INTO file_assets
                    (id, file_name, file_path, file_type, file_size, file_hash,
                     storage_type, owner_type, owner_id, metadata, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'local', 'org', ?7, '{}', ?8, ?8)",
                params![
                    id,
                    orig_name,
                    rel,
                    file_type,
                    bytes.len() as i64,
                    hash,
                    org_id,
                    now
                ],
            )
            .map_err(|e| format!("登记文件资产失败：{}", e))?;
            (id, rel)
        }
    };

    let now = chrono::Utc::now().timestamp();
    let scope = match (report_id, progress_id) {
        (None, None) => "task".to_string(),
        (Some(rid), None) => format!("report:{}", rid),
        (None, Some(pid)) => format!("progress:{}", pid),
        _ => unreachable!(), // 已在上面拒绝双重挂载
    };
    conn.execute(
        "INSERT INTO task_file_links
            (id, task_id, file_id, scope, report_id, progress_id, uploaded_by, uploaded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(task_id, file_id, scope) DO UPDATE SET
            uploaded_by = excluded.uploaded_by,
            uploaded_at = excluded.uploaded_at",
        params![
            crate::utils::new_id(),
            task_id,
            file_id,
            scope,
            report_id,
            progress_id,
            actor,
            now
        ],
    )
    .map_err(|e| format!("关联任务文件失败：{}", e))?;

    // 过程记录：把文件 id 追加到该记录的 attachments（便于按记录展示）
    if let Some(pid) = progress_id {
        let raw: String = conn
            .query_row(
                "SELECT attachments FROM task_comments WHERE id = ?1",
                params![pid],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "[]".to_string());
        let mut ids: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
        if !ids.iter().any(|x| x == &file_id) {
            ids.push(file_id.clone());
        }
        conn.execute(
            "UPDATE task_comments SET attachments = ?2, updated_at = ?3 WHERE id = ?1",
            params![pid, serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into()), now],
        )
        .map_err(|e| format!("更新过程记录附件失败：{}", e))?;
    }

    Ok(FileAssetView {
        id: file_id,
        file_name: orig_name,
        file_type,
        file_size: bytes.len() as i64,
        file_hash: hash,
        uploaded_by: actor.to_string(),
        uploaded_at: now,
        abs_path: dir.join(rel_path).to_string_lossy().to_string(),
        report_id: report_id.map(|s| s.to_string()),
        progress_id: progress_id.map(|s| s.to_string()),
    })
}

/// 某任务的附件列表。
pub fn list_task_files(
    conn: &Connection,
    org_id: &str,
    task_id: &str,
) -> Result<Vec<FileAssetView>, String> {
    let dir = org_dir(conn, org_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT f.id, f.file_name, f.file_type, f.file_size, f.file_hash, f.file_path,
                    l.uploaded_by, l.uploaded_at, l.report_id, l.progress_id
             FROM task_file_links l JOIN file_assets f ON f.id = l.file_id
             WHERE l.task_id = ?1 AND f.owner_type='org' AND f.owner_id = ?2
             ORDER BY l.uploaded_at DESC",
        )
        .map_err(|e| format!("查询任务文件失败：{}", e))?;
    let rows = stmt
        .query_map(params![task_id, org_id], |r| {
            let rel: String = r.get(5)?;
            Ok(FileAssetView {
                id: r.get(0)?,
                file_name: r.get(1)?,
                file_type: r.get(2)?,
                file_size: r.get(3)?,
                file_hash: r.get(4)?,
                uploaded_by: r.get(6)?,
                uploaded_at: r.get(7)?,
                abs_path: dir.join(rel).to_string_lossy().to_string(),
                report_id: r.get(8)?,
                progress_id: r.get(9)?,
            })
        })
        .map_err(|e| format!("查询任务文件失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取任务文件失败：{}", e))
}

/// 解除一条文件挂载（原件在「已无任何挂载」时才删除）。
///
/// 权限口径：
/// - 过程记录附件：**不可解除**（过程留痕不可变）；
/// - 汇报交付物：汇报须处于 draft / returned，且当前用户是任务参与者；
/// - 任务资料：上传者本人，或任务创建者 / 负责人。
pub fn detach_file(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    file_id: &str,
    report_id: Option<&str>,
    progress_id: Option<&str>,
) -> Result<(), String> {
    let role = crate::enclave::tasks::actor_task_role(conn, org_id, task_id, actor)?;
    if crate::enclave::tasks::is_closed_status(&crate::enclave::tasks::task_status(
        conn, org_id, task_id,
    )?) {
        return Err("任务已完成、归档或作废，不能再调整附件".to_string());
    }

    if progress_id.is_some() {
        return Err("过程记录附件不可删除（过程留痕不可变）".to_string());
    }

    let scope = if let Some(rid) = report_id {
        if !crate::enclave::tasks::is_participant(role) {
            return Err("只有任务创建者、负责人或参与者可以删除交付物".to_string());
        }
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM task_reports WHERE id = ?1 AND task_id = ?2",
                params![rid, task_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("查询完成汇报失败：{}", e))?;
        match status.as_deref() {
            Some("draft") => {}
            Some("returned") => {
                return Err("汇报已退回：请先「继续修改」转为草稿后再调整交付物".to_string())
            }
            Some(_) => return Err("汇报已提交，不能删除交付物".to_string()),
            None => return Err("完成汇报不存在或不属于该任务".to_string()),
        }
        format!("report:{}", rid)
    } else {
        // 任务资料：上传者本人或任务创建者 / 负责人
        let uploaded_by: Option<String> = conn
            .query_row(
                "SELECT uploaded_by FROM task_file_links
                 WHERE task_id = ?1 AND file_id = ?2 AND scope = 'task'",
                params![task_id, file_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("查询文件挂载失败：{}", e))?;
        let can = matches!(role, crate::enclave::tasks::TaskRole::Creator | crate::enclave::tasks::TaskRole::Assignee)
            || uploaded_by.as_deref() == Some(actor);
        if !can {
            return Err("只有上传者、创建者或负责人可以删除任务资料".to_string());
        }
        "task".to_string()
    };

    let removed = conn
        .execute(
            "DELETE FROM task_file_links WHERE task_id = ?1 AND file_id = ?2 AND scope = ?3",
            params![task_id, file_id, scope],
        )
        .map_err(|e| format!("解除文件挂载失败：{}", e))?;
    if removed == 0 {
        return Err("附件不存在或已解除".to_string());
    }

    // 全库已无挂载 → 删除资产行与物理文件（尽力而为，失败不阻塞解除）
    //
    // 「无挂载」必须同时看任务挂载和知识库引用：同一份原件可能既是交付物、
    // 又被知识条目引用，只看 task_file_links 会把知识库正在用的原文删掉。
    let task_refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_file_links WHERE file_id = ?1",
            params![file_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("查询文件挂载失败：{}", e))?;
    let kb_refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM knowledge_base_files WHERE file_id = ?1",
            params![file_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("查询知识库引用失败：{}", e))?;
    if task_refs == 0 && kb_refs == 0 {
        let rel: Option<String> = conn
            .query_row(
                "SELECT file_path FROM file_assets
                 WHERE id = ?1 AND owner_type = 'org' AND owner_id = ?2",
                params![file_id, org_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("查询文件资产失败：{}", e))?;
        conn.execute(
            "DELETE FROM file_assets WHERE id = ?1 AND owner_type = 'org' AND owner_id = ?2",
            params![file_id, org_id],
        )
        .map_err(|e| format!("删除文件资产失败：{}", e))?;
        if let Some(rel) = rel {
            if let Ok(dir) = org_dir(conn, org_id) {
                let _ = std::fs::remove_file(dir.join(rel));
            }
        }
    }
    Ok(())
}

/// 知识库文件页的「无引用文件」：既没挂任务、也没被任何知识条目引用。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanFileView {
    pub file_id: String,
    pub file_name: String,
    pub file_size: i64,
    pub created_at: i64,
}

fn orphan_rows(conn: &Connection, org_id: &str) -> Result<Vec<OrphanFileView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT fa.id, fa.file_name, fa.file_size, fa.created_at
               FROM file_assets fa
              WHERE fa.owner_type = 'org' AND fa.owner_id = ?1
                AND NOT EXISTS (SELECT 1 FROM task_file_links l WHERE l.file_id = fa.id)
                AND NOT EXISTS (SELECT 1 FROM knowledge_base_files k WHERE k.file_id = fa.id)
              ORDER BY fa.created_at DESC",
        )
        .map_err(|e| format!("查询无引用文件失败：{}", e))?;
    let rows = stmt
        .query_map(params![org_id], |r| {
            Ok(OrphanFileView {
                file_id: r.get(0)?,
                file_name: r.get(1)?,
                file_size: r.get(2)?,
                created_at: r.get(3)?,
            })
        })
        .map_err(|e| format!("查询无引用文件失败：{}", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取无引用文件失败：{}", e))
}

/// 列出无引用文件（删除条目后留下的孤儿）。
pub fn list_orphan_files(conn: &Connection, org_id: &str) -> Result<Vec<OrphanFileView>, String> {
    orphan_rows(conn, org_id)
}

/// 删除一份团队文件原件。
///
/// **前置条件**：没有任何任务挂载、也没有任何知识条目引用。引用还在时拒绝删除，
/// 由调用方引导用户先去条目里解除关联 —— 原件是内容寻址、按 sha256 去重的，
/// 一份文件可能被多个条目/任务共用，不能从某一个入口静默删掉。
pub fn delete_asset(conn: &Connection, org_id: &str, file_id: &str) -> Result<(), String> {
    let rel: Option<String> = conn
        .query_row(
            "SELECT file_path FROM file_assets
              WHERE id = ?1 AND owner_type = 'org' AND owner_id = ?2",
            params![file_id, org_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("查询文件资产失败：{}", e))?;
    let Some(rel) = rel else {
        return Err("文件不存在或已删除".to_string());
    };
    let task_refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_file_links WHERE file_id = ?1",
            params![file_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("查询任务引用失败：{}", e))?;
    let kb_refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM knowledge_base_files WHERE file_id = ?1",
            params![file_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("查询知识库引用失败：{}", e))?;
    if task_refs > 0 || kb_refs > 0 {
        return Err(format!(
            "仍有引用：{} 个任务 / {} 个知识条目，请先在条目里解除关联",
            task_refs, kb_refs
        ));
    }
    conn.execute(
        "DELETE FROM file_assets WHERE id = ?1 AND owner_type = 'org' AND owner_id = ?2",
        params![file_id, org_id],
    )
    .map_err(|e| format!("删除文件资产失败：{}", e))?;
    if let Ok(dir) = org_dir(conn, org_id) {
        let _ = std::fs::remove_file(dir.join(rel));
    }
    Ok(())
}

/// 批量清理无引用文件（磁盘回收）。返回删除数量。
pub fn cleanup_orphan_files(conn: &Connection, org_id: &str) -> Result<usize, String> {
    let rows = orphan_rows(conn, org_id)?;
    let mut removed = 0usize;
    for f in rows {
        if delete_asset(conn, org_id, &f.file_id).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// 知识库投喂：把文件写入团队文件根并登记为组织文件资产（**不挂任务**）。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedFile {
    pub file_id: String,
    pub file_name: String,
    pub file_type: String,
    pub rel_path: String,
    pub abs_path: String,
}

/// 用内存字节登记一个知识库投喂文件（按 sha256 在组织内去重）。
pub fn import_bytes(
    conn: &Connection,
    org_id: &str,
    file_name: &str,
    bytes: &[u8],
) -> Result<ImportedFile, String> {
    if bytes.is_empty() {
        return Err("文件内容为空".to_string());
    }
    let dir = org_dir(conn, org_id)?;
    let hash = hex_sha256(bytes);
    let orig_name = safe_file_name(file_name);
    let file_type = Path::new(file_name)
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let stored = format!("{}-{}", &hash[..8.min(hash.len())], orig_name);

    let existing: Option<(String, String, String)> = conn
        .query_row(
            "SELECT id, file_path, file_type FROM file_assets
             WHERE owner_type='org' AND owner_id=?1 AND file_hash=?2 LIMIT 1",
            params![org_id, hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("查询文件资产失败：{}", e))?;

    let (file_id, rel_path, file_type) = match existing {
        Some((id, rel, ft)) if dir.join(&rel).exists() => (id, rel, ft),
        _ => {
            let rel = stored;
            let abs = dir.join(&rel);
            std::fs::write(&abs, bytes)
                .map_err(|e| format!("写入团队文件失败（{}）：{}", abs.display(), e))?;
            let id = crate::utils::new_id();
            let now = chrono::Utc::now().timestamp();
            conn.execute(
                "INSERT INTO file_assets
                    (id, file_name, file_path, file_type, file_size, file_hash,
                     storage_type, owner_type, owner_id, metadata, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'local', 'org', ?7, '{}', ?8, ?8)",
                params![
                    id,
                    orig_name,
                    rel,
                    file_type,
                    bytes.len() as i64,
                    hash,
                    org_id,
                    now
                ],
            )
            .map_err(|e| format!("登记文件资产失败：{}", e))?;
            (id, rel, file_type)
        }
    };

    Ok(ImportedFile {
        file_id,
        file_name: orig_name,
        file_type,
        abs_path: dir.join(&rel_path).to_string_lossy().to_string(),
        rel_path,
    })
}

/// 从本机路径导入（主机本机投喂用）。
pub fn import_local_file(
    conn: &Connection,
    org_id: &str,
    src_path: &str,
) -> Result<ImportedFile, String> {
    let src = PathBuf::from(src_path);
    let bytes = std::fs::read(&src)
        .map_err(|e| format!("读取待投喂文件失败（{}）：{}", src.display(), e))?;
    let name = safe_file_name(&src.to_string_lossy());
    import_bytes(conn, org_id, &name, &bytes)
}

/// 文件元信息（成员端 `file.stat` 用）。
pub struct FileMetaView {
    pub file_name: String,
    pub file_hash: String,
    pub file_size: i64,
    pub abs_path: String,
}

/// 读取文件资产元信息（含主机本地绝对路径，仅主机内部使用）。
pub fn file_meta(
    conn: &Connection,
    org_id: &str,
    file_id: &str,
) -> Result<FileMetaView, String> {
    let row: Option<(String, String, i64, String)> = conn
        .query_row(
            "SELECT file_name, file_hash, file_size, file_path FROM file_assets
             WHERE id=?1 AND owner_type='org' AND owner_id=?2",
            params![file_id, org_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| format!("查询文件失败：{}", e))?;
    let (file_name, file_hash, file_size, rel) =
        row.ok_or_else(|| "文件不存在或不属于该组织".to_string())?;
    Ok(FileMetaView {
        file_name,
        file_hash,
        file_size,
        abs_path: org_dir(conn, org_id)?.join(rel).to_string_lossy().to_string(),
    })
}

/// 用内存字节挂载文件（成员经局域网 WS 上传时使用）。
///
/// 复用 [`attach_file_inner`] 的全部校验与去重逻辑：先落一个临时文件（文件名保持原名，
/// 以便保留扩展名），挂载完成后立即清理。
pub fn attach_bytes(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    task_id: &str,
    file_name: &str,
    bytes: &[u8],
    report_id: Option<&str>,
) -> Result<FileAssetView, String> {
    let dir = std::env::temp_dir().join(format!("pilotdesk-upload-{}", crate::utils::new_id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建上传临时目录失败：{}", e))?;
    let safe = safe_file_name(file_name);
    let path = dir.join(&safe);
    std::fs::write(&path, bytes).map_err(|e| format!("写入上传临时文件失败：{}", e))?;
    let result = attach_file_inner(
        conn,
        org_id,
        actor,
        task_id,
        &path.to_string_lossy(),
        report_id,
        None,
        false,
    );
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// 文件的绝对路径（供「打开」用）。
pub fn file_abs_path(
    conn: &Connection,
    org_id: &str,
    file_id: &str,
) -> Result<String, String> {
    let rel: Option<String> = conn
        .query_row(
            "SELECT file_path FROM file_assets
             WHERE id=?1 AND owner_type='org' AND owner_id=?2",
            params![file_id, org_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("查询文件失败：{}", e))?;
    let rel = rel.ok_or_else(|| "文件不存在或不属于该组织".to_string())?;
    Ok(org_dir(conn, org_id)?.join(rel).to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_stable_hex() {
        // "abc" 的 sha256 前 8 位
        assert!(hex_sha256(b"abc").starts_with("ba7816bf"));
        assert_eq!(hex_sha256(b"abc").len(), 64);
    }

    #[test]
    fn safe_name_strips_paths() {
        assert_eq!(safe_file_name("/tmp/a/b/报告.pdf"), "报告.pdf");
        assert_eq!(safe_file_name("C:\\x\\y.docx"), "y.docx");
        assert_eq!(safe_file_name(""), "file");
    }

    /// 系统目录选择器返回的 `\\?\` 前缀必须去掉，否则会被当作 UNC 拒绝。
    #[test]
    fn normalize_strips_extended_prefix() {
        assert_eq!(normalize_path(r"\\?\C:\Users\x"), r"C:\Users\x");
        assert_eq!(normalize_path(r"\\?\UNC\srv\share"), r"\\srv\share");
        assert_eq!(normalize_path("  \"C:\\a b\"  "), "C:\\a b");
    }

    /// 自定义基础目录下必须再补 `enclave/files`，不能直接用用户选的目录。
    #[test]
    fn custom_base_appends_enclave_files() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE enclave_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        // 未设置 → 应用根目录下的 enclave/files
        let default_root = resolve_team_files_root(&conn).unwrap();
        assert!(default_root.ends_with("enclave\\files") || default_root.ends_with("enclave/files"));

        conn.execute(
            "INSERT INTO enclave_settings (key, value, updated_at) VALUES (?1, ?2, 0)",
            params![TEAM_FILES_ROOT_KEY, r"C:\TeamData"],
        )
        .unwrap();
        let base = resolve_team_base(&conn).unwrap();
        assert_eq!(base, PathBuf::from(r"C:\TeamData"));
        let root = resolve_team_files_root(&conn).unwrap();
        assert_eq!(root, PathBuf::from(r"C:\TeamData\enclave\files"));
    }
}
