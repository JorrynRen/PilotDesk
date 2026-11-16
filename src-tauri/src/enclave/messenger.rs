//! 飞地一对一协作传讯。
//!
//! 定位：跨页面的一对一协作消息，不替代通知中心 / 任务讨论，也不做群聊。
//! 持久化：按组织分月的追加式 JSONL；没有数据库表，也没有完整 IM 语义。
//! 文件：仅在线直传，默认 20 MB、10 分钟有效，只驻留内存，不进入团队文件系统。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::authz::{self, Role};

const DEFAULT_RETENTION_DAYS: i64 = 30;
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_METADATA_BYTES: usize = 16 * 1024;
pub const MESSENGER_FILE_MAX_BYTES: usize = 20 * 1024 * 1024;
const MESSENGER_FILE_TTL_SECS: i64 = 10 * 60;
const MESSAGE_LIMIT_MAX: usize = 500;
const RETENTION_KEY: &str = "im_retention_days";
const IM_ROOT_ENV: &str = "PILOTDESK_ENCLAVE_IM_ROOT";

static WRITE_LOCK: Mutex<()> = Mutex::new(());
static EPHEMERAL_FILES: OnceLock<Mutex<HashMap<String, EphemeralFile>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    pub id: String,
    pub org_id: String,
    pub from_user_id: String,
    pub to_user_id: String,
    /// text / reference / file / api-share-distribution
    pub kind: String,
    pub content: String,
    /// 引用卡 / 文件元数据等结构化扩展；引用卡只保存资源 ID。
    pub metadata: String,
    /// 明码敏感消息标记，用于提示接收方。
    pub sensitive: bool,
    pub created_at: i64,
    /// 软撤回时间；保留原消息记录，读取时按 id 合并最新状态。
    #[serde(default)]
    pub recalled_at: Option<i64>,
    #[serde(default)]
    pub recalled_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub id: String,
    pub org_id: String,
    pub from_user_id: String,
    pub to_user_id: String,
    pub kind: String,
    pub content: String,
    pub metadata: String,
    pub sensitive: bool,
    pub created_at: i64,
    pub recalled_at: Option<i64>,
    pub recalled_by: Option<String>,
    /// 服务端实时解析的引用标题；无权限或资源删除时为空。
    pub reference_title: String,
    pub reference_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageInput {
    pub peer_id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub metadata: String,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessengerMemberView {
    pub user_id: String,
    pub display_name: String,
    pub email: String,
    pub role: String,
    pub online: bool,
    pub last_message_at: i64,
    pub last_message_preview: String,
    pub last_message_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessengerSettingsView {
    pub retention_days: i64,
    pub max_file_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessengerFileView {
    pub file_name: String,
    pub mime: String,
    pub size: usize,
    pub data_base64: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
struct EphemeralFile {
    org_id: String,
    from_user_id: String,
    to_user_id: String,
    file_name: String,
    mime: String,
    bytes: Vec<u8>,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct MemberRecord {
    user_id: String,
    display_name: String,
    email: String,
    role: String,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn safe_component(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn org_dir(org_id: &str) -> PathBuf {
    im_root().join(safe_component(org_id))
}

fn im_root() -> PathBuf {
    std::env::var_os(IM_ROOT_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| {
            crate::utils::paths::app_root_dir()
                .join("enclave")
                .join("im")
        })
}

fn month_file(org_id: &str, at: i64) -> PathBuf {
    let month = chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m")
        .to_string();
    org_dir(org_id).join(format!("{}.jsonl", month))
}

fn jsonl_files(org_id: &str) -> Vec<PathBuf> {
    let dir = org_dir(org_id);
    let mut files = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|v| v.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn retention_days(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT value FROM enclave_settings WHERE key = ?1",
        params![RETENTION_KEY],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .and_then(|v| v.parse::<i64>().ok())
    .filter(|v| matches!(*v, 7 | 30 | 90))
    .unwrap_or(DEFAULT_RETENTION_DAYS)
}

pub fn settings(
    conn: &Connection,
    org_id: &str,
    actor: &str,
) -> Result<MessengerSettingsView, String> {
    ensure_member(conn, org_id, actor)?;
    Ok(MessengerSettingsView {
        retention_days: retention_days(conn),
        max_file_bytes: MESSENGER_FILE_MAX_BYTES,
    })
}

pub fn set_retention(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    days: i64,
) -> Result<MessengerSettingsView, String> {
    if !matches!(
        authz::role_for_member(conn, org_id, actor)?,
        Some(Role::Owner | Role::Admin)
    ) {
        return Err("仅组织所有者/管理员可修改传讯保留期".to_string());
    }
    if !matches!(days, 7 | 30 | 90) {
        return Err("消息保留期仅支持 7 / 30 / 90 天".to_string());
    }
    conn.execute(
        "INSERT INTO enclave_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![RETENTION_KEY, days.to_string(), now()],
    )
    .map_err(|e| format!("保存传讯保留期失败：{}", e))?;
    settings(conn, org_id, actor)
}

fn member_records(conn: &Connection, org_id: &str) -> Result<Vec<MemberRecord>, String> {
    let mut out = Vec::new();
    let owner = conn
        .query_row(
            "SELECT owner_user_id FROM org_snapshot WHERE org_id = ?1",
            params![org_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("读取组织所有者失败：{}", e))?;
    let mut stmt = conn
        .prepare(
            "SELECT user_id, display_name, email, role
               FROM member_snapshot
              WHERE org_id = ?1 AND seat_status <> 'removed'
              ORDER BY CASE role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END, display_name, email",
        )
        .map_err(|e| format!("读取组织成员失败：{}", e))?;
    let rows = stmt
        .query_map(params![org_id], |r| {
            Ok(MemberRecord {
                user_id: r.get(0)?,
                display_name: r.get(1)?,
                email: r.get(2)?,
                role: r.get(3)?,
            })
        })
        .map_err(|e| format!("读取组织成员失败：{}", e))?;
    for row in rows {
        out.push(row.map_err(|e| format!("读取组织成员失败：{}", e))?);
    }
    if let Some(owner_id) = owner.filter(|v| !v.trim().is_empty()) {
        if !out.iter().any(|m| m.user_id == owner_id) {
            out.insert(
                0,
                MemberRecord {
                    user_id: owner_id,
                    display_name: "组织所有者".to_string(),
                    email: String::new(),
                    role: "owner".to_string(),
                },
            );
        }
    }
    Ok(out)
}

fn ensure_member(conn: &Connection, org_id: &str, user_id: &str) -> Result<MemberRecord, String> {
    member_records(conn, org_id)?
        .into_iter()
        .find(|m| m.user_id == user_id)
        .ok_or_else(|| "当前账号不是该组织成员".to_string())
}

fn read_file_messages(path: &Path, cutoff: i64) -> Vec<StoredMessage> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<StoredMessage>(&line).ok())
        .filter(|m| m.created_at >= cutoff)
        .collect()
}

fn read_messages(conn: &Connection, org_id: &str) -> Vec<StoredMessage> {
    let cutoff = now() - retention_days(conn) * 86_400;
    let mut by_id = HashMap::<String, StoredMessage>::new();
    for path in jsonl_files(org_id) {
        for message in read_file_messages(&path, cutoff) {
            by_id.insert(message.id.clone(), message);
        }
    }
    let mut out = by_id.into_values().collect::<Vec<_>>();
    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

fn reference_state(conn: &Connection, org_id: &str, actor: &str, meta: &Value) -> (String, bool) {
    let ref_type = meta.get("refType").and_then(Value::as_str).unwrap_or("");
    let ref_id = meta.get("refId").and_then(Value::as_str).unwrap_or("");
    if ref_type.is_empty() || ref_id.is_empty() {
        return (String::new(), true);
    }
    match ref_type {
        "task" => match super::tasks::get_task(conn, org_id, ref_id, actor) {
            Ok(task) => (task.title, true),
            Err(_) => (String::new(), false),
        },
        "kb-entry" => {
            let kb_id = meta.get("kbId").and_then(Value::as_str).unwrap_or("");
            match super::knowledge::get_entry(conn, org_id, kb_id, ref_id) {
                Ok(Some(entry)) => (entry.title, true),
                _ => (String::new(), false),
            }
        }
        "kb-file" => match super::files::file_meta(conn, org_id, ref_id) {
            Ok(file) => (file.file_name, true),
            Err(_) => (String::new(), false),
        },
        "api-share-distribution" => api_share_distribution_state(conn, org_id, actor, ref_id),
        _ => (String::new(), false),
    }
}

fn api_share_distribution_state(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    distribution_id: &str,
) -> (String, bool) {
    type DistributionState = (
        String,
        i64,
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        i64,
        Option<i64>,
    );
    let row: Option<DistributionState> = conn
        .query_row(
            "SELECT CASE WHEN d.kind = 'route_token'
                         THEN COALESCE(t.name, '')
                         ELSE COALESCE(s.name, '')
                    END,
                    CASE WHEN d.kind = 'route_token'
                         THEN 1 ELSE COALESCE(s.enabled, 0)
                    END,
                    d.kind, d.recipient_user_id, d.created_by,
                    d.revoked_at, d.expires_at, COALESCE(t.enabled, 0), t.expires_at
               FROM api_share_distributions d
               LEFT JOIN api_share_sources s ON s.id = d.source_id AND s.org_id = d.org_id
               LEFT JOIN api_share_tokens t ON t.id = d.token_id AND t.org_id = d.org_id
              WHERE d.id = ?1 AND d.org_id = ?2",
            params![distribution_id, org_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten();
    let Some((
        source_name,
        source_enabled,
        kind,
        recipient_user_id,
        created_by,
        revoked_at,
        distribution_expires_at,
        token_enabled,
        token_expires_at,
    )) = row
    else {
        return (String::new(), false);
    };
    let at = now();
    let token_valid = kind != "route_token"
        || (token_enabled != 0 && token_expires_at.is_none_or(|expires_at| expires_at > at));
    let available = (recipient_user_id == actor || created_by == actor)
        && source_enabled != 0
        && revoked_at.is_none()
        && distribution_expires_at.is_none_or(|expires_at| expires_at > at)
        && token_valid;
    (source_name, available)
}

fn view_message(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    message: StoredMessage,
) -> MessageView {
    let recalled = message.recalled_at.is_some();
    // 撤回后接收方不再获得正文与扩展信息；发送方保留原内容用于“再次编辑”。
    let payload_visible = !recalled || message.from_user_id == actor;
    let meta = if payload_visible {
        serde_json::from_str::<Value>(&message.metadata).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let (reference_title, reference_available) = if payload_visible {
        reference_state(conn, org_id, actor, &meta)
    } else {
        (String::new(), false)
    };
    MessageView {
        id: message.id,
        org_id: message.org_id,
        from_user_id: message.from_user_id,
        to_user_id: message.to_user_id,
        kind: message.kind,
        content: if payload_visible {
            message.content
        } else {
            String::new()
        },
        metadata: if payload_visible {
            message.metadata
        } else {
            "{}".to_string()
        },
        sensitive: message.sensitive,
        created_at: message.created_at,
        recalled_at: message.recalled_at,
        recalled_by: message.recalled_by,
        reference_title,
        reference_available,
    }
}

fn preview(message: &StoredMessage) -> String {
    if message.recalled_at.is_some() {
        return "已撤回".to_string();
    }
    match message.kind.as_str() {
        "file" => {
            let meta = serde_json::from_str::<Value>(&message.metadata).unwrap_or(Value::Null);
            format!(
                "文件 · {}",
                meta.get("fileName")
                    .and_then(Value::as_str)
                    .unwrap_or("未命名文件")
            )
        }
        "reference" => "引用卡片".to_string(),
        "api-share-distribution" => "API 共享连接".to_string(),
        _ => {
            let chars = message.content.chars().take(60).collect::<String>();
            if message.content.chars().count() > 60 {
                format!("{}…", chars)
            } else {
                chars
            }
        }
    }
}

pub fn members(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    online: &[String],
) -> Result<Vec<MessengerMemberView>, String> {
    ensure_member(conn, org_id, actor)?;
    let messages = read_messages(conn, org_id);
    let online = online
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let mut out = Vec::new();
    for member in member_records(conn, org_id)?
        .into_iter()
        .filter(|m| m.user_id != actor)
    {
        let last = messages.iter().rev().find(|message| {
            (message.from_user_id == actor && message.to_user_id == member.user_id)
                || (message.from_user_id == member.user_id && message.to_user_id == actor)
        });
        out.push(MessengerMemberView {
            user_id: member.user_id.clone(),
            display_name: if member.display_name.trim().is_empty() {
                member.email.clone()
            } else {
                member.display_name
            },
            email: member.email,
            role: member.role,
            online: online.contains(member.user_id.as_str()),
            last_message_at: last.map(|m| m.created_at).unwrap_or(0),
            last_message_preview: last.map(preview).unwrap_or_default(),
            last_message_kind: last.map(|m| m.kind.clone()).unwrap_or_default(),
        });
    }
    out.sort_by(|a, b| {
        b.last_message_at
            .cmp(&a.last_message_at)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    Ok(out)
}

pub fn list_messages(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    peer_id: &str,
    limit: Option<usize>,
) -> Result<Vec<MessageView>, String> {
    ensure_member(conn, org_id, actor)?;
    ensure_member(conn, org_id, peer_id)?;
    let limit = limit.unwrap_or(100).clamp(1, MESSAGE_LIMIT_MAX);
    let mut messages = read_messages(conn, org_id)
        .into_iter()
        .filter(|m| {
            (m.from_user_id == actor && m.to_user_id == peer_id)
                || (m.from_user_id == peer_id && m.to_user_id == actor)
        })
        .collect::<Vec<_>>();
    if messages.len() > limit {
        messages = messages.split_off(messages.len() - limit);
    }
    Ok(messages
        .into_iter()
        .map(|message| view_message(conn, org_id, actor, message))
        .collect())
}

pub fn send(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    input: &SendMessageInput,
) -> Result<StoredMessage, String> {
    ensure_member(conn, org_id, actor)?;
    if input.peer_id.trim().is_empty() {
        return Err("请选择消息接收人".to_string());
    }
    if input.peer_id == actor {
        return Err("不能给自己发送传讯".to_string());
    }
    ensure_member(conn, org_id, &input.peer_id)?;
    let kind = match input.kind.trim() {
        "" => "text",
        "text" => "text",
        "reference" => "reference",
        "file" => "file",
        "api-share-distribution" => "api-share-distribution",
        _ => return Err("不支持的消息类型".to_string()),
    };
    if input.content.len() > MAX_MESSAGE_BYTES {
        return Err("消息内容超过 64 KB 上限".to_string());
    }
    if input.metadata.len() > MAX_METADATA_BYTES {
        return Err("消息扩展信息超过上限".to_string());
    }
    if kind != "file" && input.content.trim().is_empty() && input.metadata.trim().is_empty() {
        return Err("消息内容不能为空".to_string());
    }
    let metadata = if !input.metadata.trim().is_empty() {
        Some(
            serde_json::from_str::<Value>(&input.metadata)
                .map_err(|_| "消息扩展信息必须是 JSON".to_string())?,
        )
    } else {
        None
    };
    if kind == "api-share-distribution" {
        let distribution_id = metadata
            .as_ref()
            .and_then(|value| value.get("distributionId"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "API 共享卡片缺少分发标识".to_string())?;
        let valid = conn
            .query_row(
                "SELECT created_by, recipient_user_id
                   FROM api_share_distributions
                  WHERE id = ?1 AND org_id = ?2 AND revoked_at IS NULL
                    AND (expires_at IS NULL OR expires_at > ?3)",
                params![distribution_id, org_id, now()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("校验 API 共享分发失败：{}", e))?;
        if !matches!(valid, Some((created_by, recipient)) if created_by == actor && recipient == input.peer_id)
        {
            return Err("API 共享分发记录无效或不属于当前接收人".to_string());
        }
    }
    let message = StoredMessage {
        id: crate::utils::new_id(),
        org_id: org_id.to_string(),
        from_user_id: actor.to_string(),
        to_user_id: input.peer_id.clone(),
        kind: kind.to_string(),
        content: input.content.clone(),
        metadata: if input.metadata.trim().is_empty() {
            "{}".to_string()
        } else {
            input.metadata.clone()
        },
        sensitive: input.sensitive,
        created_at: now(),
        recalled_at: None,
        recalled_by: None,
    };
    append(&message)?;
    Ok(message)
}

pub fn recall(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    peer_id: &str,
    message_id: &str,
) -> Result<MessageView, String> {
    ensure_member(conn, org_id, actor)?;
    ensure_member(conn, org_id, peer_id)?;
    let original = read_messages(conn, org_id)
        .into_iter()
        .find(|message| message.id == message_id)
        .ok_or_else(|| "消息不存在或已超过保留期".to_string())?;
    if original.from_user_id != actor {
        return Err("仅发送者可以撤回消息".to_string());
    }
    if original.to_user_id != peer_id {
        return Err("消息与当前传讯对象不匹配".to_string());
    }
    if original.recalled_at.is_some() {
        return Err("消息已经撤回".to_string());
    }

    let mut recalled = original;
    recalled.recalled_at = Some(now());
    recalled.recalled_by = Some(actor.to_string());
    append(&recalled)?;
    revoke_ephemeral_file(&recalled);
    Ok(view_message(conn, org_id, actor, recalled))
}

fn append(message: &StoredMessage) -> Result<(), String> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = month_file(&message.org_id, message.created_at);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建传讯目录失败 {}: {}", parent.display(), e))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开传讯日志失败：{}", e))?;
    let line = serde_json::to_string(message).map_err(|e| format!("序列化传讯失败：{}", e))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.flush())
        .map_err(|e| format!("写入传讯日志失败：{}", e))
}

pub fn send_file(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    peer_id: &str,
    file_name: &str,
    mime: &str,
    data_base64: &str,
) -> Result<StoredMessage, String> {
    let bytes = BASE64
        .decode(data_base64)
        .map_err(|e| format!("文件内容不是有效 Base64：{}", e))?;
    if bytes.is_empty() {
        return Err("文件内容为空".to_string());
    }
    if bytes.len() > MESSENGER_FILE_MAX_BYTES {
        return Err("在线直传文件不能超过 20 MB".to_string());
    }
    ensure_member(conn, org_id, actor)?;
    ensure_member(conn, org_id, peer_id)?;
    let file_id = crate::utils::new_id();
    let expires_at = now() + MESSENGER_FILE_TTL_SECS;
    let file = EphemeralFile {
        org_id: org_id.to_string(),
        from_user_id: actor.to_string(),
        to_user_id: peer_id.to_string(),
        file_name: safe_download_name(file_name),
        mime: if mime.trim().is_empty() {
            "application/octet-stream".to_string()
        } else {
            mime.trim().to_string()
        },
        bytes,
        expires_at,
    };
    let size = file.bytes.len();
    let name = file.file_name.clone();
    let mime = file.mime.clone();
    let store = EPHEMERAL_FILES.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let mut map = store.lock().unwrap_or_else(|e| e.into_inner());
        let at = now();
        map.retain(|_, file| file.expires_at > at);
        map.insert(file_id.clone(), file);
    }
    let metadata = serde_json::json!({
        "fileId": file_id,
        "fileName": name,
        "mime": mime,
        "size": size,
        "onlineOnly": true,
        "expiresAt": expires_at,
    })
    .to_string();
    send(
        conn,
        org_id,
        actor,
        &SendMessageInput {
            peer_id: peer_id.to_string(),
            kind: "file".to_string(),
            content: String::new(),
            metadata,
            sensitive: false,
        },
    )
}

fn safe_download_name(raw: &str) -> String {
    let name = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim()
        .chars()
        .filter(|c| !matches!(c, '\0' | '\r' | '\n'))
        .take(180)
        .collect::<String>();
    if name.is_empty() {
        "file".to_string()
    } else {
        name
    }
}

fn revoke_ephemeral_file(message: &StoredMessage) {
    if message.kind != "file" {
        return;
    }
    let meta = serde_json::from_str::<Value>(&message.metadata).unwrap_or(Value::Null);
    let Some(file_id) = meta.get("fileId").and_then(Value::as_str) else {
        return;
    };
    let store = EPHEMERAL_FILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = store.lock().unwrap_or_else(|e| e.into_inner());
    map.remove(file_id);
}

pub fn get_ephemeral_file(
    org_id: &str,
    actor: &str,
    file_id: &str,
) -> Result<MessengerFileView, String> {
    let store = EPHEMERAL_FILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = store.lock().unwrap_or_else(|e| e.into_inner());
    let at = now();
    map.retain(|_, file| file.expires_at > at);
    let file = map
        .get(file_id)
        .ok_or_else(|| "文件已过期或下线；在线直传文件不会进入历史存储".to_string())?;
    if file.org_id != org_id || (file.from_user_id != actor && file.to_user_id != actor) {
        return Err("无权读取该在线文件".to_string());
    }
    Ok(MessengerFileView {
        file_name: file.file_name.clone(),
        mime: file.mime.clone(),
        size: file.bytes.len(),
        data_base64: BASE64.encode(&file.bytes),
        expires_at: file.expires_at,
    })
}

pub fn clear_thread(
    conn: &Connection,
    org_id: &str,
    actor: &str,
    peer_id: &str,
) -> Result<usize, String> {
    ensure_member(conn, org_id, actor)?;
    let cutoff = now() - retention_days(conn) * 86_400;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut removed = 0usize;
    for path in jsonl_files(org_id) {
        let messages = read_file_messages(&path, 0);
        let kept = messages
            .into_iter()
            .filter(|m| {
                let in_thread = (m.from_user_id == actor && m.to_user_id == peer_id)
                    || (m.from_user_id == peer_id && m.to_user_id == actor);
                if in_thread && m.created_at >= cutoff {
                    removed += 1;
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();
        let mut file = File::create(&path)
            .map_err(|e| format!("清理传讯日志失败 {}: {}", path.display(), e))?;
        for message in kept {
            let line =
                serde_json::to_string(&message).map_err(|e| format!("序列化传讯失败：{}", e))?;
            writeln!(file, "{}", line)
                .map_err(|e| format!("写入传讯日志失败 {}: {}", path.display(), e))?;
        }
    }
    Ok(removed)
}

pub fn prune_empty_dirs() {
    let root = im_root();
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for org in entries.flatten() {
        let path = org.path();
        if path.is_dir()
            && std::fs::read_dir(&path)
                .map(|mut it| it.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&path);
        }
    }
}

pub fn is_member(conn: &Connection, org_id: &str, user_id: &str) -> bool {
    authz::role_for_member(conn, org_id, user_id)
        .ok()
        .flatten()
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enclave::db::ENCLAVE_SCHEMA_SQL;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(ENCLAVE_SCHEMA_SQL).unwrap();
        conn.execute(
            "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id)
             VALUES ('o1', 'Team', 'u1', 'owner', 'u1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO member_snapshot (org_id, user_id, role, display_name, email, seat_status, synced_at)
             VALUES ('o1', 'u1', 'owner', '甲', '', 'active', 1),
                    ('o1', 'u2', 'member', '乙', '', 'active', 1)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn sends_and_reads_one_to_one_message() {
        let conn = conn();
        let sent = send(
            &conn,
            "o1",
            "u1",
            &SendMessageInput {
                peer_id: "u2".to_string(),
                kind: "text".to_string(),
                content: "hello".to_string(),
                metadata: "{}".to_string(),
                sensitive: false,
            },
        )
        .unwrap();
        let list = list_messages(&conn, "o1", "u2", "u1", None).unwrap();
        assert!(list.iter().any(|m| m.id == sent.id && m.content == "hello"));
    }

    #[test]
    fn recall_keeps_sender_payload_and_hides_it_from_recipient() {
        let conn = conn();
        let sent = send(
            &conn,
            "o1",
            "u1",
            &SendMessageInput {
                peer_id: "u2".to_string(),
                kind: "text".to_string(),
                content: "typo".to_string(),
                metadata: "{}".to_string(),
                sensitive: false,
            },
        )
        .unwrap();

        let recalled = recall(&conn, "o1", "u1", "u2", &sent.id).unwrap();
        assert!(recalled.recalled_at.is_some());
        assert_eq!(recalled.content, "typo");

        let sender_rows = list_messages(&conn, "o1", "u1", "u2", None).unwrap();
        let sender_row = sender_rows.iter().find(|row| row.id == sent.id).unwrap();
        assert_eq!(sender_row.content, "typo");
        assert!(sender_row.recalled_at.is_some());

        let recipient_rows = list_messages(&conn, "o1", "u2", "u1", None).unwrap();
        let recipient_row = recipient_rows.iter().find(|row| row.id == sent.id).unwrap();
        assert_eq!(recipient_row.content, "");
        assert_eq!(recipient_row.metadata, "{}");
        assert!(recipient_row.recalled_at.is_some());

        let member_rows = members(&conn, "o1", "u1", &[]).unwrap();
        let peer = member_rows.iter().find(|m| m.user_id == "u2").unwrap();
        assert_eq!(peer.last_message_preview, "已撤回");
        assert!(recall(&conn, "o1", "u2", "u1", &sent.id).is_err());
    }

    #[test]
    fn retention_accepts_only_supported_values() {
        let conn = conn();
        assert_eq!(settings(&conn, "o1", "u1").unwrap().retention_days, 30);
        set_retention(&conn, "o1", "u1", 7).unwrap();
        assert_eq!(settings(&conn, "o1", "u1").unwrap().retention_days, 7);
        assert!(set_retention(&conn, "o1", "u1", 14).is_err());
        assert!(set_retention(&conn, "o1", "u2", 30).is_err());
    }
}
