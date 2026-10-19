//! 局域网主机服务（M7）：明文 WebSocket 控制通道 + UDP 发现 + 应急联动。
//!
//! 设计约束：
//! - **不缓存**：成员端每次请求实时发往主机；主机是团队业务数据的唯一权威源。
//! - **身份在主机侧确认**：成员端自报 `userId` 不可信，主机用平台 token 在线校验，
//!   公网不可用时退回 `member_snapshot` 只读快照（治理冻结、业务照常）。
//! - **应急开关**：`host_emergency` 为真时拒绝非回环连接；本机（主机自己的 UI）不受影响。
//! - **明文**：一期只做应用层身份确认，TLS 在 M9 替换 `accept` 层，帧协议不变。
//!
//! 协议（JSON 文本帧）：
//! ```text
//! → {"type":"hello","protocol":1,"orgId":"…","userId":"…","email":"…","token":"…"}
//! ← {"type":"hello_ok","protocol":1,"orgId":"…","authMode":"token|snapshot"}
//! ← {"type":"hello_err","message":"…"}
//! → {"type":"req","id":"…","method":"task.list","params":{"orgId":"…"}}
//! ← {"type":"res","id":"…","ok":true,"data":…}
//! ← {"type":"res","id":"…","ok":false,"error":"…"}
//! ```

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_tungstenite::tungstenite::protocol::Message;

use super::authz::{self, Role};
use super::org;
use super::{files, rpc, tasks, tls, transfer, EnclaveState, ENCLAVE_DEFAULT_PORT};
use crate::utils::platform;

/// 平台 `/api/v1/me` 的最小响应（只取身份 id 做一致性校验）。
#[derive(Deserialize)]
struct CloudMe {
    user: CloudMeUser,
}

#[derive(Deserialize)]
struct CloudMeUser {
    id: i64,
}

/// 发现报文魔数（避免与其他 UDP 广播混淆）。
const MAGIC: &str = "PILOTDESK_ENCLAVE";
/// 协议版本：帧结构变化时递增。
const PROTOCOL: u32 = 1;
/// UDP 发现端口（主机广播、成员监听）。
pub const DISCOVERY_PORT: u16 = 47821;
/// 发现报文的保鲜期（秒）：超过视为主机已离线。
pub const DISCOVERY_TTL_SECS: i64 = 15;
/// 发现广播间隔。
const BEACON_INTERVAL: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);

const HOST_PORT_KEY: &str = "enclave_host_port";
const HOST_ADDR_PREFIX: &str = "enclave_host_addr:";
/// 成员端固定的主机证书指纹（TOFU；重置后下次连接重新记录）。
const HOST_FP_PREFIX: &str = "enclave_host_fp:";
/// 成员上传单个文件的大小上限（MVP：整文件读入内存后分块发送）。
const UPLOAD_MAX_BYTES: u64 = 512 * 1024 * 1024;

// ── 运行态 ──────────────────────────────────────────────────────

/// 主机服务的运行态（只放内存，不落库）。
#[derive(Default)]
pub struct HostRuntime {
    inner: Mutex<HostInner>,
}

#[derive(Default)]
struct HostInner {
    running: bool,
    port: u16,
    last_error: Option<String>,
}

impl HostRuntime {
    fn mark_running(&self, port: u16) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.running = true;
        g.port = port;
        g.last_error = None;
    }

    fn mark_stopped(&self, error: Option<String>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.running = false;
        g.port = 0;
        g.last_error = error;
    }

    /// `(running, port, lastError)`
    pub fn view(&self) -> (bool, u16, Option<String>) {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (g.running, g.port, g.last_error.clone())
    }
}

/// 发现到的主机（成员端内存缓存，不落库）。
#[derive(Clone)]
pub struct DiscoveredHost {
    pub addr: String,
    pub host_name: String,
    pub seen_at: i64,
}

// ── 主机服务主循环 ──────────────────────────────────────────────

/// 启动主机服务。所有机器都会跑：非主机时只做发现监听；成为 owner 后自动开始监听。
pub async fn run_host_loop(state: std::sync::Arc<EnclaveState>) {
    {
        let st = state.clone();
        tokio::spawn(async move { discovery_listen(st).await });
    }
    {
        let st = state.clone();
        tokio::spawn(async move { beacon_loop(st).await });
    }

    let mut listener: Option<(TcpListener, tokio_rustls::TlsAcceptor)> = None;
    // 定期复核「本机是否仍是主机」：云端所有权转让后旧主机必须停服
    let mut last_role_check = std::time::Instant::now();
    loop {
        if last_role_check.elapsed() >= Duration::from_secs(30) {
            last_role_check = std::time::Instant::now();
            if listener.is_some() && host_org_ids(&state).is_empty() {
                listener = None;
                state.host.mark_stopped(None);
                log::info!("[enclave] 本机已不是任何组织的主机，停止对外服务");
            }
        }
        if listener.is_none() {
            if !host_org_ids(&state).is_empty() {
                let port = configured_host_port(&state);
                match TcpListener::bind(("0.0.0.0", port)).await {
                    Ok(l) => match tls::ensure_host_tls() {
                        Ok(host_tls) => {
                            state.host.mark_running(port);
                            log::info!(
                                "[enclave] 主机服务已监听 wss://0.0.0.0:{}（指纹 {}…）",
                                port,
                                host_tls.fingerprint.chars().take(16).collect::<String>()
                            );
                            listener = Some((l, host_tls.acceptor));
                        }
                        Err(e) => {
                            state.host.mark_stopped(Some(e.clone()));
                            log::warn!("[enclave] 主机 TLS 初始化失败：{}", e);
                        }
                    },
                    Err(e) => {
                        state.host.mark_stopped(Some(format!("监听端口 {} 失败：{}", port, e)));
                        log::warn!("[enclave] 主机服务监听失败（{}）：{}", port, e);
                    }
                }
            }
        }

        match listener.as_ref() {
            Some((l, acceptor)) => {
                let mut accept_failed = false;
                match tokio::time::timeout(Duration::from_secs(5), l.accept()).await {
                    Ok(Ok((stream, peer))) => {
                        let st = state.clone();
                        let acceptor = acceptor.clone();
                        tokio::spawn(async move {
                            match acceptor.accept(stream).await {
                                Ok(tls_stream) => handle_conn(tls_stream, peer, st).await,
                                Err(e) => log::warn!(
                                    "[enclave] 与 {} 的 TLS 握手失败：{}",
                                    peer,
                                    e
                                ),
                            }
                        });
                    }
                    Ok(Err(e)) => {
                        state
                            .host
                            .mark_stopped(Some(format!("接受连接失败：{}", e)));
                        accept_failed = true;
                    }
                    Err(_) => {}
                }
                if accept_failed {
                    listener = None;
                }
            }
            None => tokio::time::sleep(Duration::from_secs(10)).await,
        }
    }
}

/// 成员端拉取任务列表（同步命令层通过它拿到主机的 `data`）。
pub async fn fetch_task_list(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
) -> Result<Value, String> {
    call_host(state, db, org_id, "task.list", json!({})).await
}

/// 把成员端命令转发到主机执行（主机侧统一按成员身份授权）。
pub async fn call_host(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    command: &str,
    payload: Value,
) -> Result<Value, String> {
    // task.list 由主机侧专用方法处理；其余走通用分发器
    let method = if command == "task.list" {
        "task.list".to_string()
    } else {
        "enclave.call".to_string()
    };
    let mut session = HostSession::connect(state, db, org_id).await?;
    let data = if method == "task.list" {
        session.request("task.list", json!({ "orgId": org_id })).await
    } else {
        session
            .request(
                "enclave.call",
                json!({ "orgId": org_id, "command": command, "payload": payload }),
            )
            .await
    };
    session.close().await;
    data
}

/// 只保留安全字符（用于上传临时目录 / 缓存文件名）。
fn safe_component(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.trim().is_empty() {
        "file".to_string()
    } else {
        s
    }
}

fn upload_path(org_id: &str, user_id: &str, upload_id: &str) -> PathBuf {
    crate::utils::paths::app_root_dir()
        .join("enclave")
        .join("uploads")
        .join(safe_component(org_id))
        .join(safe_component(user_id))
        .join(format!("{}.part", safe_component(upload_id)))
}

/// 把上传令牌解析为本机临时文件（仅接受本组织 + 本成员名下的会话）。
pub(crate) fn resolve_upload_token(
    org_id: &str,
    user_id: &str,
    token: &str,
) -> Result<PathBuf, String> {
    if token.trim().is_empty()
        || token.contains('/')
        || token.contains('\\')
        || token.contains("..")
    {
        return Err("附件上传令牌不合法".to_string());
    }
    let path = upload_path(org_id, user_id, token);
    if !path.exists() {
        return Err("附件上传会话不存在或已过期".to_string());
    }
    Ok(path)
}

fn file_cache_dir(org_id: &str) -> PathBuf {
    crate::utils::paths::app_root_dir()
        .join("enclave")
        .join("cache")
        .join(safe_component(org_id))
}

fn cache_root() -> PathBuf {
    crate::utils::paths::app_root_dir().join("enclave").join("cache")
}

fn uploads_root() -> PathBuf {
    crate::utils::paths::app_root_dir().join("enclave").join("uploads")
}

/// 成员文件缓存默认上限（全机共享，按最近访问时间淘汰）。
const CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 缓存上限设置键（本机全局，不随组织区分）。
const CACHE_MAX_KEY: &str = "enclave_cache_max_bytes";
/// 自定义上限的可用区间：256 MB ~ 100 GB。
const CACHE_LIMIT_MIN: u64 = 256 * 1024 * 1024;
const CACHE_LIMIT_MAX: u64 = 100 * 1024 * 1024 * 1024;
/// 淘汰目标：降到上限的 90%，避免频繁触发。
const CACHE_TARGET_RATIO: f64 = 0.9;
/// 上传临时文件保鲜期（秒）：异常中断的上传不长期占用磁盘。
const UPLOAD_STALE_SECS: u64 = 24 * 3600;

/// 流式计算文件 SHA-256（分块读取，不整文件入内存）。
fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("读取文件失败：{}", e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("读取文件失败：{}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

/// 当前生效的缓存上限（未设置时用默认值）。
fn cache_limit(conn: &Connection) -> u64 {
    get_setting(conn, CACHE_MAX_KEY)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(CACHE_MAX_BYTES)
}

/// 本机文件缓存的占用与上限（供「组织与权限」页展示 / 调整）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheInfoView {
    pub dir: String,
    pub used_bytes: u64,
    pub max_bytes: u64,
    pub file_count: usize,
}

/// 统计缓存占用；目录不存在时返回 0。
pub fn cache_info(conn: &Connection) -> CacheInfoView {
    let root = cache_root();
    let mut files = Vec::new();
    collect_files(&root, &mut files);
    CacheInfoView {
        dir: root.to_string_lossy().to_string(),
        used_bytes: files.iter().map(|(_, size, _)| *size).sum(),
        max_bytes: cache_limit(conn),
        file_count: files.len(),
    }
}

/// 调整缓存上限（按区间夹取），并立即执行一次淘汰。
pub fn set_cache_limit(conn: &Connection, max_bytes: u64) -> Result<CacheInfoView, String> {
    let clamped = max_bytes.clamp(CACHE_LIMIT_MIN, CACHE_LIMIT_MAX);
    put_setting(conn, CACHE_MAX_KEY, &clamped.to_string())?;
    enforce_cache_limit(&cache_root(), clamped)?;
    Ok(cache_info(conn))
}

/// 清空本机文件缓存（不影响主机数据）。
pub fn clear_cache(conn: &Connection) -> Result<CacheInfoView, String> {
    let root = cache_root();
    if root.exists() {
        std::fs::remove_dir_all(&root).map_err(|e| format!("清空文件缓存失败：{}", e))?;
    }
    Ok(cache_info(conn))
}

fn touch_cache_entry(path: &Path) {
    // 用 mtime 记录「最近访问」，作为 LRU 依据（Windows 默认不更新 atime）
    if let Ok(file) = OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn collect_files(dir: &Path, out: &mut Vec<(PathBuf, u64, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect_files(&path, out),
            Ok(t) if t.is_file() => {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push((path.clone(), size, mtime_secs(&path)));
            }
            _ => {}
        }
    }
}

/// 把缓存总量压到上限以内：按 mtime 从旧到新删除，直到降到 90%。
fn enforce_cache_limit(root: &Path, max_bytes: u64) -> Result<(), String> {
    let mut files = Vec::new();
    collect_files(root, &mut files);
    let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum();
    if total <= max_bytes {
        return Ok(());
    }
    files.sort_by_key(|(_, _, mtime)| *mtime);
    let target = (max_bytes as f64 * CACHE_TARGET_RATIO) as u64;
    for (path, size, _) in files {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}

/// 清理异常中断留下的上传临时文件（超过 24h）。
fn cleanup_stale_uploads() {
    let mut files = Vec::new();
    collect_files(&uploads_root(), &mut files);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for (path, _, mtime) in files {
        if path.extension().and_then(|e| e.to_str()) == Some("part")
            && now.saturating_sub(mtime) > UPLOAD_STALE_SECS
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 流式发送本地文件：边读边算 SHA-256，内存占用不超过一个分块。
///
/// 返回 `(sha256, 实际发送字节数)`；读盘与网络发送交错进行，不会整文件入内存。
async fn send_file_stream(
    session: &mut HostSession,
    org_id: &str,
    upload_id: &str,
    path: &Path,
) -> Result<(String, u64), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("读取待上传文件失败：{}", e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; transfer::LAN_CHUNK_MAX as usize];
    let mut offset = 0u64;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("读取待上传文件失败：{}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        session
            .request(
                "file.upload_chunk",
                json!({
                    "orgId": org_id,
                    "uploadId": upload_id,
                    "offset": offset,
                    "data": BASE64.encode(&buf[..n]),
                }),
            )
            .await?;
        offset += n as u64;
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    Ok((sha256, offset))
}

/// 成员端上传本地文件到主机（流式分块；返回主机侧的 `FileAssetView`）。
pub async fn upload_file(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    task_id: &str,
    src_path: &str,
    report_id: Option<&str>,
) -> Result<Value, String> {
    let meta = std::fs::metadata(src_path).map_err(|e| format!("读取待上传文件失败：{}", e))?;
    if meta.len() > UPLOAD_MAX_BYTES {
        return Err(format!(
            "文件过大（{} MB），单文件上限 {} MB",
            meta.len() / 1024 / 1024,
            UPLOAD_MAX_BYTES / 1024 / 1024
        ));
    }
    let file_name = Path::new(src_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();

    let mut session = HostSession::connect(state, db, org_id).await?;
    let begin = session
        .request(
            "file.upload_begin",
            json!({ "orgId": org_id, "fileName": file_name }),
        )
        .await?;
    let upload_id = begin["uploadId"].as_str().unwrap_or("").to_string();
    if upload_id.is_empty() {
        session.close().await;
        return Err("主机未返回上传会话".to_string());
    }

    let sent = send_file_stream(&mut session, org_id, &upload_id, Path::new(src_path)).await;
    let result = match sent {
        Ok((sha256, _size)) => {
            session
                .request(
                    "file.upload_finish",
                    json!({
                        "orgId": org_id,
                        "uploadId": upload_id,
                        "taskId": task_id,
                        "reportId": report_id,
                        "fileName": file_name,
                        "sha256": sha256,
                    }),
                )
                .await
        }
        Err(e) => Err(e),
    };
    session.close().await;
    result
}

/// 成员端上传一个「临时附件」（过程记录用）：成功返回上传令牌，不落文件资产。
///
/// 过程记录附件必须随记录一次性写入，因此先上传到主机临时区，再由
/// `task.progress_add` 用令牌解析成本机路径。
pub async fn upload_temp_file(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    src_path: &str,
) -> Result<String, String> {
    let meta = std::fs::metadata(src_path).map_err(|e| format!("读取待上传文件失败：{}", e))?;
    if meta.len() > UPLOAD_MAX_BYTES {
        return Err("文件超过单文件大小上限".to_string());
    }

    let mut session = HostSession::connect(state, db, org_id).await?;
    let begin = session
        .request("file.upload_begin", json!({ "orgId": org_id, "fileName": "attachment" }))
        .await?;
    let upload_id = begin["uploadId"].as_str().unwrap_or("").to_string();
    if upload_id.is_empty() {
        session.close().await;
        return Err("主机未返回上传会话".to_string());
    }
    let sent = send_file_stream(&mut session, org_id, &upload_id, Path::new(src_path)).await;
    let kept = match sent {
        Ok((sha256, _size)) => {
            session
                .request(
                    "file.upload_keep",
                    json!({ "orgId": org_id, "uploadId": upload_id, "sha256": sha256 }),
                )
                .await
        }
        Err(e) => Err(e),
    };
    session.close().await;
    let kept = kept?;
    Ok(kept["token"].as_str().unwrap_or("").to_string())
}

/// 成员端打开文件：下载到内容寻址缓存（`<sha8>-<名>`）并返回本地路径。
pub async fn fetch_file_to_cache(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    file_id: &str,
) -> Result<String, String> {
    let mut session = HostSession::connect(state, db, org_id).await?;
    let stat = session
        .request("file.stat", json!({ "orgId": org_id, "fileId": file_id }))
        .await?;
    let file_name = stat["fileName"].as_str().unwrap_or("file").to_string();
    let file_hash = stat["fileHash"].as_str().unwrap_or("").to_string();
    let file_size = stat["fileSize"].as_u64().unwrap_or(0);

    let dir = file_cache_dir(org_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建文件缓存目录失败：{}", e))?;
    let safe_name = safe_component(&file_name);
    let target = dir.join(format!(
        "{}-{}",
        &file_hash[..8.min(file_hash.len())],
        safe_name
    ));

    // 命中缓存：大小一致且哈希一致才复用（哈希流式计算，避免大文件整读入内存）
    if target.exists() {
        let size_ok = std::fs::metadata(&target)
            .map(|m| m.len() == file_size)
            .unwrap_or(false);
        if size_ok && file_sha256(&target).map(|h| h == file_hash).unwrap_or(false) {
            touch_cache_entry(&target);
            session.close().await;
            return Ok(target.to_string_lossy().to_string());
        }
    }

    let part = target.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| format!("创建缓存文件失败：{}", e))?;
    let mut offset = 0u64;
    loop {
        let chunk = session
            .request(
                "file.fetch",
                json!({
                    "orgId": org_id,
                    "fileId": file_id,
                    "offset": offset,
                    "len": transfer::LAN_CHUNK_MAX,
                }),
            )
            .await?;
        let data = BASE64
            .decode(chunk["data"].as_str().unwrap_or(""))
            .map_err(|e| format!("解码文件分块失败：{}", e))?;
        use std::io::Write;
        file.write_all(&data)
            .map_err(|e| format!("写入文件缓存失败：{}", e))?;
        offset += data.len() as u64;
        if chunk["eof"].as_bool() == Some(true) {
            break;
        }
        if data.is_empty() {
            let _ = std::fs::remove_file(&part);
            session.close().await;
            return Err("主机返回了空分块，下载中断".to_string());
        }
    }
    drop(file);
    session.close().await;

    let downloaded_hash = file_sha256(&part)?;
    if downloaded_hash != file_hash {
        let _ = std::fs::remove_file(&part);
        return Err("文件校验失败（哈希不一致），已丢弃".to_string());
    }
    if target.exists() {
        let _ = std::fs::remove_file(&target);
    }
    std::fs::rename(&part, &target).map_err(|e| format!("保存文件缓存失败：{}", e))?;
    touch_cache_entry(&target);
    let limit = state
        .store
        .with_conn(|c| Ok(cache_limit(c)))
        .unwrap_or(CACHE_MAX_BYTES);
    let _ = enforce_cache_limit(&cache_root(), limit);
    Ok(target.to_string_lossy().to_string())
}

/// 主机知识库工具（成员端）转发：不带平台 token，主机退回成员快照校验。
pub async fn call_host_without_token(
    state: &EnclaveState,
    org_id: &str,
    command: &str,
    payload: Value,
) -> Result<Value, String> {
    let mut session = HostSession::connect_with_db(state, None, org_id).await?;
    let data = session
        .request(
            "enclave.call",
            json!({ "orgId": org_id, "command": command, "payload": payload }),
        )
        .await;
    session.close().await;
    data
}

/// 拉取发给当前账号的转移请求（收到后由本地决定是否接受）。
pub async fn fetch_pending_transfers(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
) -> Result<Value, String> {
    let mut session = HostSession::connect(state, db, org_id).await?;
    let data = session
        .request("transfer.pending", json!({ "orgId": org_id }))
        .await;
    session.close().await;
    data
}

/// 接受转移请求（返回一次性迁移口令与包哈希）。
pub async fn accept_lan_transfer(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    transfer_id: &str,
) -> Result<Value, String> {
    let mut session = HostSession::connect(state, db, org_id).await?;
    let data = session
        .request(
            "transfer.accept",
            json!({ "orgId": org_id, "transferId": transfer_id }),
        )
        .await;
    session.close().await;
    data
}

/// 分块下载主机迁移包到 `dest`，返回文件 sha256。
pub async fn download_lan_package(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    transfer_id: &str,
    dest: &Path,
) -> Result<String, String> {
    let mut session = HostSession::connect(state, db, org_id).await?;
    let mut file = std::fs::File::create(dest).map_err(|e| format!("创建临时迁移包失败：{}", e))?;
    let mut offset = 0u64;
    loop {
        let chunk = session
            .request(
                "transfer.package",
                json!({
                    "orgId": org_id,
                    "transferId": transfer_id,
                    "offset": offset,
                    "len": transfer::LAN_CHUNK_MAX,
                }),
            )
            .await?;
        let data = BASE64
            .decode(chunk["data"].as_str().unwrap_or(""))
            .map_err(|e| format!("解码迁移分块失败：{}", e))?;
        use std::io::Write;
        file.write_all(&data)
            .map_err(|e| format!("写入迁移包失败：{}", e))?;
        offset += data.len() as u64;
        if chunk["eof"].as_bool() == Some(true) {
            break;
        }
        if data.is_empty() {
            return Err("主机返回了空分块，迁移中断".to_string());
        }
    }
    drop(file);
    session.close().await;
    let bytes = std::fs::read(dest).map_err(|e| format!("读取迁移包失败：{}", e))?;
    Ok(transfer::hex_sha256(&bytes))
}

/// 告知主机：受让人已完成校验（`migrating → verified`）。
pub async fn confirm_lan_verified(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    transfer_id: &str,
) -> Result<(), String> {
    let mut session = HostSession::connect(state, db, org_id).await?;
    let result = session
        .request(
            "transfer.verified",
            json!({ "orgId": org_id, "transferId": transfer_id }),
        )
        .await;
    session.close().await;
    result.map(|_| ())
}

// ── 成员端主机会话 ─────────────────────────────────────────────

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
type WsTx = futures::stream::SplitSink<WsStream, Message>;
type WsRx = futures::stream::SplitStream<WsStream>;

/// 已完成 `hello` 握手的主机连接：后续 `request` 复用同一连接（一次迁移多次分块）。
struct HostSession {
    tx: WsTx,
    rx: WsRx,
}

impl HostSession {
    async fn connect(
        state: &EnclaveState,
        db: &crate::DbState,
        org_id: &str,
    ) -> Result<Self, String> {
        Self::connect_with_db(state, Some(db), org_id).await
    }

    async fn connect_with_db(
        state: &EnclaveState,
        db: Option<&crate::DbState>,
        org_id: &str,
    ) -> Result<Self, String> {
        let addr = resolve_host(state, org_id).ok_or_else(|| {
            "未发现主机：请确认主机在线且与本机在同一局域网；仍失败时可在「组织与权限」填写主机地址"
                .to_string()
        })?;
        let (user_id, email) = state.store.with_conn(|c| {
            let actor = authz::current_actor_id(c)?;
            let email = c
                .query_row("SELECT email FROM local_identity LIMIT 1", [], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
                .map_err(|e| format!("读取本机身份失败：{}", e))?
                .unwrap_or_default();
            Ok((actor, email))
        })?;
        let token = match db {
            Some(db) => crate::commands::org_common::require_token(db)
                .await
                .unwrap_or_default(),
            None => String::new(),
        };

        let url = format!("wss://{}", addr);
        // 首次连接 TOFU 记录指纹；之后必须一致（防冒充 / 防主机悄悄换证书）
        let pin_key = format!("{}{}", HOST_FP_PREFIX, org_id);
        let expected = state
            .store
            .with_conn(|c| Ok(get_setting(c, &pin_key)))?;
        let (ws, fingerprint) =
            tls::connect_pinned(&url, expected.as_deref(), CONNECT_TIMEOUT).await?;
        if expected
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
        {
            let _ = state
                .store
                .with_conn(|c| put_setting(c, &pin_key, &fingerprint));
            log::info!(
                "[enclave] 已固定主机 {} 的证书指纹：{}…",
                addr,
                fingerprint.chars().take(16).collect::<String>()
            );
        }
        let (tx, rx) = ws.split();
        let mut session = Self { tx, rx };

        session
            .tx
            .send(Message::Text(
                json!({
                    "type": "hello",
                    "protocol": PROTOCOL,
                    "orgId": org_id,
                    "userId": user_id,
                    "email": email,
                    "token": token,
                })
                .to_string(),
            ))
            .await
            .map_err(|e| format!("发送握手失败：{}", e))?;
        let hello_reply = tokio::time::timeout(HELLO_TIMEOUT, next_json(&mut session.rx))
            .await
            .map_err(|_| "等待主机握手超时".to_string())??;
        match hello_reply["type"].as_str() {
            Some("hello_ok") => Ok(session),
            Some("hello_err") => Err(hello_reply["message"]
                .as_str()
                .unwrap_or("主机拒绝了连接")
                .to_string()),
            _ => Err("主机返回了无法识别的握手响应".to_string()),
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = crate::utils::new_id();
        let req = json!({
            "type": "req",
            "id": id,
            "method": method,
            "params": params,
        });
        self.tx
            .send(Message::Text(req.to_string()))
            .await
            .map_err(|e| format!("发送请求失败：{}", e))?;
        let reply = tokio::time::timeout(REQUEST_TIMEOUT, next_json(&mut self.rx))
            .await
            .map_err(|_| "等待主机响应超时".to_string())??;
        if reply["ok"].as_bool() != Some(true) {
            return Err(reply["error"]
                .as_str()
                .unwrap_or("主机返回了错误")
                .to_string());
        }
        Ok(reply["data"].clone())
    }

    async fn close(mut self) {
        let _ = self.tx.send(Message::Close(None)).await;
    }
}

// ── 连接处理 ────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelloMsg {
    #[serde(rename = "type")]
    kind: String,
    protocol: u32,
    org_id: String,
    user_id: String,
    #[serde(default)]
    token: String,
}

#[derive(Deserialize)]
struct RequestMsg {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: String,
    method: String,
    #[serde(default)]
    params: Value,
}

async fn handle_conn(
    stream: tokio_rustls::server::TlsStream<TcpStream>,
    peer: SocketAddr,
    state: std::sync::Arc<EnclaveState>,
) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut tx, mut rx) = ws.split();

    let hello_val = match tokio::time::timeout(HELLO_TIMEOUT, next_json(&mut rx)).await {
        Ok(Ok(v)) => v,
        _ => return,
    };
    let hello: HelloMsg = match serde_json::from_value::<HelloMsg>(hello_val) {
        Ok(h) if h.kind == "hello" => h,
        _ => {
            let _ = send_json(&mut tx, &hello_err("握手格式不正确")).await;
            return;
        }
    };
    if hello.protocol != PROTOCOL {
        let _ = send_json(
            &mut tx,
            &hello_err(&format!(
                "协议版本不匹配（主机 {}，客户端 {}）",
                PROTOCOL, hello.protocol
            )),
        )
        .await;
        return;
    }
    // 主机身份复核：云端所有权已转让的旧主机必须拒绝继续提供服务
    if !host_org_ids(&state).iter().any(|o| o == &hello.org_id) {
        let _ = send_json(&mut tx, &hello_err("本机已不是该组织的主机")).await;
        return;
    }
    if org_disabled(&state, &hello.org_id) && !peer.ip().is_loopback() {
        let _ = send_json(&mut tx, &hello_err("飞地对外服务已临时关闭")).await;
        return;
    }
    let auth_mode = match verify_member(&state, &hello).await {
        Ok(mode) => mode,
        Err(msg) => {
            let _ = send_json(&mut tx, &hello_err(&msg)).await;
            return;
        }
    };
    let ok = json!({
        "type": "hello_ok",
        "protocol": PROTOCOL,
        "orgId": hello.org_id,
        "authMode": auth_mode,
    });
    if send_json(&mut tx, &ok.to_string()).await.is_err() {
        return;
    }

    loop {
        let req_val = match tokio::time::timeout(IDLE_TIMEOUT, next_json(&mut rx)).await {
            Ok(Ok(v)) => v,
            _ => break,
        };
        let req: RequestMsg = match serde_json::from_value::<RequestMsg>(req_val) {
            Ok(r) if r.kind == "req" => r,
            Ok(_) => continue,
            Err(e) => {
                let _ = send_json(&mut tx, &res_err("", &format!("请求格式不正确：{}", e))).await;
                continue;
            }
        };
        if org_disabled(&state, &hello.org_id) && !peer.ip().is_loopback() {
            let _ = send_json(&mut tx, &res_err(&req.id, "飞地对外服务已临时关闭")).await;
            break;
        }
        let reply = match req.method.as_str() {
            "ping" => res_ok(&req.id, json!({ "pong": true })),
            "task.list" => {
                let req_org = req.params["orgId"].as_str().unwrap_or(hello.org_id.as_str());
                if req_org != hello.org_id {
                    res_err(&req.id, "不能跨组织访问")
                } else {
                    match handle_task_list(&state, req_org, &hello.user_id) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
            }
            "enclave.call" => match org_param(&req, &hello) {
                Ok(org) => {
                    let command = req.params["command"].as_str().unwrap_or("").to_string();
                    let payload = req.params.get("payload").cloned().unwrap_or(json!({}));
                    match handle_enclave_call(&state, org, &hello.user_id, &command, &payload) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "file.upload_begin" => match org_param(&req, &hello) {
                Ok(org) => match handle_upload_begin(&state, org, &hello.user_id) {
                    Ok(v) => res_ok(&req.id, v),
                    Err(e) => res_err(&req.id, &e),
                },
                Err(e) => res_err(&req.id, &e),
            },
            "file.upload_chunk" => match org_param(&req, &hello) {
                Ok(org) => {
                    let upload_id = req.params["uploadId"].as_str().unwrap_or("").to_string();
                    let offset = req.params["offset"].as_u64().unwrap_or(0);
                    let data = req.params["data"].as_str().unwrap_or("").to_string();
                    match handle_upload_chunk(org, &hello.user_id, &upload_id, offset, &data) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "file.upload_finish" => match org_param(&req, &hello) {
                Ok(org) => {
                    let upload_id = req.params["uploadId"].as_str().unwrap_or("").to_string();
                    let task_id = req.params["taskId"].as_str().unwrap_or("").to_string();
                    let report_id = req.params["reportId"].as_str().map(str::to_string);
                    let file_name = req.params["fileName"].as_str().unwrap_or("file").to_string();
                    let sha256 = req.params["sha256"].as_str().unwrap_or("").to_string();
                    match handle_upload_finish(
                        &state,
                        org,
                        &hello.user_id,
                        &upload_id,
                        &task_id,
                        report_id.as_deref(),
                        &file_name,
                        &sha256,
                    ) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "file.upload_keep" => match org_param(&req, &hello) {
                Ok(org) => {
                    let upload_id = req.params["uploadId"].as_str().unwrap_or("").to_string();
                    let sha256 = req.params["sha256"].as_str().unwrap_or("").to_string();
                    match handle_upload_keep(org, &hello.user_id, &upload_id, &sha256) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "file.stat" => match org_param(&req, &hello) {
                Ok(org) => {
                    let file_id = req.params["fileId"].as_str().unwrap_or("").to_string();
                    match handle_file_stat(&state, org, &hello.user_id, &file_id) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "file.fetch" => match org_param(&req, &hello) {
                Ok(org) => {
                    let file_id = req.params["fileId"].as_str().unwrap_or("").to_string();
                    let offset = req.params["offset"].as_u64().unwrap_or(0);
                    let len = req.params["len"].as_u64().unwrap_or(transfer::LAN_CHUNK_MAX);
                    match handle_file_fetch(&state, org, &hello.user_id, &file_id, offset, len) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "transfer.pending" => match org_param(&req, &hello) {
                Ok(org) => match handle_transfer_pending(&state, org, &hello.user_id) {
                    Ok(v) => res_ok(&req.id, v),
                    Err(e) => res_err(&req.id, &e),
                },
                Err(e) => res_err(&req.id, &e),
            },
            "transfer.accept" => match (org_param(&req, &hello), req.params["transferId"].as_str()) {
                (Ok(org), Some(id)) => {
                    match handle_transfer_accept(&state, org, id, &hello.user_id) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                _ => res_err(&req.id, "参数不完整"),
            },
            "transfer.package" => match (org_param(&req, &hello), req.params["transferId"].as_str()) {
                (Ok(org), Some(id)) => {
                    let offset = req.params["offset"].as_u64().unwrap_or(0);
                    let len = req.params["len"].as_u64().unwrap_or(transfer::LAN_CHUNK_MAX);
                    match handle_transfer_package(&state, org, id, &hello.user_id, offset, len) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                _ => res_err(&req.id, "参数不完整"),
            },
            "transfer.verified" => match (org_param(&req, &hello), req.params["transferId"].as_str()) {
                (Ok(org), Some(id)) => {
                    match handle_transfer_verified(&state, org, id, &hello.user_id) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                _ => res_err(&req.id, "参数不完整"),
            },
            other => res_err(&req.id, &format!("未知方法：{}", other)),
        };
        if send_json(&mut tx, &reply).await.is_err() {
            break;
        }
    }
}

async fn verify_member(state: &EnclaveState, hello: &HelloMsg) -> Result<&'static str, String> {
    let orgs = host_org_ids(state);
    if !orgs.iter().any(|o| o == &hello.org_id) {
        return Err("本机不是该组织的主机".to_string());
    }
    let role = state
        .store
        .with_conn(|c| authz::role_for_member(c, &hello.org_id, &hello.user_id))?;
    if role.is_none() {
        return Err("当前账号不是该组织成员".to_string());
    }
    // 在线：用平台 token 校验自报身份；公网不可用时退回只读快照（设计 §7.3）。
    if !hello.token.trim().is_empty() {
        if let Ok(me) = platform::get_json_with_status::<CloudMe>(
            "/api/v1/me",
            Some(&hello.token),
        )
        .await
        {
            if me.user.id.to_string() != hello.user_id {
                return Err("登录身份与连接身份不一致".to_string());
            }
            return Ok("token");
        }
    }
    Ok("snapshot")
}

fn handle_task_list(state: &EnclaveState, org_id: &str, user_id: &str) -> Result<Value, String> {
    state.store.with_conn(|c| {
        let role: Role = authz::role_for_member(c, org_id, user_id)?
            .ok_or_else(|| "当前账号不是该组织成员".to_string())?;
        let list = tasks::list_tasks_with_role(c, org_id, user_id, role)?;
        serde_json::to_value(&list).map_err(|e| format!("序列化任务失败：{}", e))
    })
}

fn handle_enclave_call(
    state: &EnclaveState,
    org_id: &str,
    user_id: &str,
    command: &str,
    payload: &Value,
) -> Result<Value, String> {
    state.store.with_conn(|c| {
        // 每次转发都重新确认成员身份（连接期间可能被移出组织）
        authz::role_for_member(c, org_id, user_id)?
            .ok_or_else(|| "当前账号不是该组织成员".to_string())?;
        rpc::dispatch(c, org_id, user_id, command, payload)
    })
}

fn ensure_member(state: &EnclaveState, org_id: &str, user_id: &str) -> Result<(), String> {
    state.store.with_conn(|c| {
        authz::role_for_member(c, org_id, user_id)?
            .map(|_| ())
            .ok_or_else(|| "当前账号不是该组织成员".to_string())
    })
}

fn handle_upload_begin(
    state: &EnclaveState,
    org_id: &str,
    user_id: &str,
) -> Result<Value, String> {
    ensure_member(state, org_id, user_id)?;
    cleanup_stale_uploads();
    let upload_id = crate::utils::new_id();
    let path = upload_path(org_id, user_id, &upload_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建上传目录失败：{}", e))?;
    }
    std::fs::File::create(&path).map_err(|e| format!("创建上传临时文件失败：{}", e))?;
    Ok(json!({ "uploadId": upload_id }))
}

fn handle_upload_chunk(
    org_id: &str,
    user_id: &str,
    upload_id: &str,
    offset: u64,
    data: &str,
) -> Result<Value, String> {
    if upload_id.is_empty() {
        return Err("缺少上传会话".to_string());
    }
    let bytes = BASE64
        .decode(data)
        .map_err(|e| format!("解码上传分块失败：{}", e))?;
    let path = upload_path(org_id, user_id, upload_id);
    let current = std::fs::metadata(&path)
        .map_err(|_| "上传会话不存在或已过期".to_string())?
        .len();
    if offset != current {
        return Err(format!(
            "上传分块偏移不一致（期望 {}，收到 {}）",
            current, offset
        ));
    }
    if current + bytes.len() as u64 > UPLOAD_MAX_BYTES {
        let _ = std::fs::remove_file(&path);
        return Err("文件超过单文件大小上限".to_string());
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开上传临时文件失败：{}", e))?;
    file.write_all(&bytes)
        .map_err(|e| format!("写入上传分块失败：{}", e))?;
    Ok(json!({ "size": current + bytes.len() as u64 }))
}

#[allow(clippy::too_many_arguments)]
fn handle_upload_finish(
    state: &EnclaveState,
    org_id: &str,
    user_id: &str,
    upload_id: &str,
    task_id: &str,
    report_id: Option<&str>,
    file_name: &str,
    sha256: &str,
) -> Result<Value, String> {
    ensure_member(state, org_id, user_id)?;
    let path = upload_path(org_id, user_id, upload_id);
    let bytes = std::fs::read(&path).map_err(|_| "上传会话不存在或已过期".to_string())?;
    if bytes.len() as u64 > UPLOAD_MAX_BYTES {
        let _ = std::fs::remove_file(&path);
        return Err("文件超过单文件大小上限".to_string());
    }
    if !sha256.is_empty() && transfer::hex_sha256(&bytes) != sha256 {
        let _ = std::fs::remove_file(&path);
        return Err("上传文件校验失败（哈希不一致）".to_string());
    }
    let result = state.store.with_conn(|c| {
        files::attach_bytes(c, org_id, user_id, task_id, file_name, &bytes, report_id)
    });
    let _ = std::fs::remove_file(&path);
    let view = result?;
    serde_json::to_value(&view).map_err(|e| format!("序列化文件信息失败：{}", e))
}

fn handle_upload_keep(
    org_id: &str,
    user_id: &str,
    upload_id: &str,
    sha256: &str,
) -> Result<Value, String> {
    let path = resolve_upload_token(org_id, user_id, upload_id)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("读取上传文件失败：{}", e))?;
    if !sha256.is_empty() && transfer::hex_sha256(&bytes) != sha256 {
        let _ = std::fs::remove_file(&path);
        return Err("上传文件校验失败（哈希不一致）".to_string());
    }
    Ok(json!({ "token": upload_id }))
}

fn handle_file_stat(
    state: &EnclaveState,
    org_id: &str,
    user_id: &str,
    file_id: &str,
) -> Result<Value, String> {
    ensure_member(state, org_id, user_id)?;
    state.store.with_conn(|c| {
        let meta = files::file_meta(c, org_id, file_id)?;
        Ok(json!({
            "fileName": meta.file_name,
            "fileHash": meta.file_hash,
            "fileSize": meta.file_size,
        }))
    })
}

fn handle_file_fetch(
    state: &EnclaveState,
    org_id: &str,
    user_id: &str,
    file_id: &str,
    offset: u64,
    len: u64,
) -> Result<Value, String> {
    ensure_member(state, org_id, user_id)?;
    state.store.with_conn(|c| {
        let meta = files::file_meta(c, org_id, file_id)?;
        let path = PathBuf::from(&meta.abs_path);
        let total = std::fs::metadata(&path).map_err(|e| format!("文件不可用：{}", e))?.len();
        let offset = offset.min(total);
        let want = len.min(transfer::LAN_CHUNK_MAX).min(total - offset);
        let mut buf = vec![0u8; want as usize];
        if want > 0 {
            let mut f = std::fs::File::open(&path).map_err(|e| format!("读取文件失败：{}", e))?;
            f.seek(SeekFrom::Start(offset))
                .map_err(|e| format!("定位文件失败：{}", e))?;
            f.read_exact(&mut buf)
                .map_err(|e| format!("读取文件失败：{}", e))?;
        }
        Ok(json!({
            "offset": offset,
            "total": total,
            "data": BASE64.encode(&buf),
            "eof": offset + want >= total,
        }))
    })
}

fn org_param<'a>(req: &'a RequestMsg, hello: &'a HelloMsg) -> Result<&'a str, String> {
    let org = req.params["orgId"]
        .as_str()
        .unwrap_or(hello.org_id.as_str());
    if org != hello.org_id {
        return Err("不能跨组织访问".to_string());
    }
    Ok(org)
}

fn handle_transfer_pending(
    state: &EnclaveState,
    org_id: &str,
    user_id: &str,
) -> Result<Value, String> {
    state.store.with_conn(|c| {
        let list = transfer::pending_for(c, org_id, user_id)?;
        serde_json::to_value(&list).map_err(|e| format!("序列化转移请求失败：{}", e))
    })
}

fn handle_transfer_accept(
    state: &EnclaveState,
    org_id: &str,
    transfer_id: &str,
    user_id: &str,
) -> Result<Value, String> {
    state.store.with_conn(|c| {
        let info = transfer::accept_for_member(c, transfer_id, org_id, user_id)?;
        serde_json::to_value(&info).map_err(|e| format!("序列化接受结果失败：{}", e))
    })
}

fn handle_transfer_package(
    state: &EnclaveState,
    org_id: &str,
    transfer_id: &str,
    user_id: &str,
    offset: u64,
    len: u64,
) -> Result<Value, String> {
    state.store.with_conn(|c| {
        let chunk = transfer::read_package_chunk(c, transfer_id, org_id, user_id, offset, len)?;
        serde_json::to_value(&chunk).map_err(|e| format!("序列化迁移分块失败：{}", e))
    })
}

fn handle_transfer_verified(
    state: &EnclaveState,
    org_id: &str,
    transfer_id: &str,
    user_id: &str,
) -> Result<Value, String> {
    state.store.with_conn(|c| {
        let view = transfer::mark_member_verified(c, transfer_id, org_id, user_id)?;
        serde_json::to_value(&view).map_err(|e| format!("序列化转移状态失败：{}", e))
    })
}

// ── 发现：UDP 广播 + 监听 ──────────────────────────────────────

async fn beacon_loop(state: std::sync::Arc<EnclaveState>) {
    let sock = match UdpSocket::bind(("0.0.0.0", 0)).await {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[enclave] 发现广播套接字创建失败：{}", e);
            return;
        }
    };
    let _ = sock.set_broadcast(true);
    loop {
        let orgs: Vec<String> = host_org_ids(&state)
            .into_iter()
            .filter(|o| !org_disabled(&state, o))
            .collect();
        if !orgs.is_empty() {
            let (running, port, _) = state.host.view();
            let port = if running && port != 0 {
                port
            } else {
                configured_host_port(&state)
            };
            let payload = json!({
                "magic": MAGIC,
                "protocol": PROTOCOL,
                "orgIds": orgs,
                "port": port,
                "hostName": host_name(),
                "tls": true,
            })
            .to_string();
            let target = ("255.255.255.255", configured_discovery_port());
            let _ = sock.send_to(payload.as_bytes(), target).await;
            // 同机回环：方便本机/测试环境发现
            let _ = sock
                .send_to(payload.as_bytes(), ("127.0.0.1", configured_discovery_port()))
                .await;
        }
        tokio::time::sleep(BEACON_INTERVAL).await;
    }
}

async fn discovery_listen(state: std::sync::Arc<EnclaveState>) {
    let sock = match UdpSocket::bind(("0.0.0.0", configured_discovery_port())).await {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[enclave] 发现监听失败（{}）：{}", configured_discovery_port(), e);
            return;
        }
    };
    let mut buf = [0u8; 4096];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let v: Value = match serde_json::from_slice(&buf[..n]) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["magic"].as_str() != Some(MAGIC) || v["protocol"].as_u64() != Some(PROTOCOL as u64) {
            continue;
        }
        let port = v["port"].as_u64().unwrap_or(ENCLAVE_DEFAULT_PORT as u64) as u16;
        let host_name = v["hostName"].as_str().unwrap_or("").to_string();
        let addr = format!("{}:{}", src.ip(), port);
        let now = chrono::Utc::now().timestamp();
        if let Some(orgs) = v["orgIds"].as_array() {
            let mut map = state
                .discovered
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for org_id in orgs.iter().filter_map(|o| o.as_str()) {
                map.insert(
                    org_id.to_string(),
                    DiscoveredHost {
                        addr: addr.clone(),
                        host_name: host_name.clone(),
                        seen_at: now,
                    },
                );
            }
        }
    }
}

// ── 设置 / 状态 ────────────────────────────────────────────────

/// 清除成员端固定的主机证书指纹（主机重装 / 更换主机后使用）。
pub fn reset_fingerprint(state: &EnclaveState, org_id: &str) -> Result<(), String> {
    state
        .store
        .with_conn(|c| clear_fingerprint(c, org_id))
}

fn clear_fingerprint(conn: &Connection, org_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM enclave_settings WHERE key = ?1",
        params![format!("{}{}", HOST_FP_PREFIX, org_id)],
    )
    .map_err(|e| format!("清除主机指纹失败：{}", e))?;
    Ok(())
}

/// 成员端手动填写主机地址（留空表示清除，回到自动发现）。
pub fn set_manual_addr(state: &EnclaveState, org_id: &str, addr: &str) -> Result<(), String> {
    let key = format!("{}{}", HOST_ADDR_PREFIX, org_id);
    let raw = addr
        .trim()
        .trim_start_matches("ws://")
        .trim_start_matches("wss://")
        .trim_end_matches('/');
    if raw.is_empty() {
        return state.store.with_conn(|c| {
            c.execute("DELETE FROM enclave_settings WHERE key = ?1", params![key])
                .map_err(|e| format!("清除主机地址失败：{}", e))?;
            // 地址清除后，旧指纹也不再适用（重新发现时重新 TOFU）
            clear_fingerprint(c, org_id)
        });
    }
    let (host, port) = raw
        .rsplit_once(':')
        .ok_or_else(|| "主机地址格式应为 主机:端口".to_string())?;
    if host.trim().is_empty() {
        return Err("主机地址不能为空".to_string());
    }
    port.parse::<u16>()
        .map_err(|_| "主机端口不是有效数字".to_string())?;
    state.store.with_conn(|c| {
        put_setting(c, &key, raw)?;
        // 换了地址可能换了主机：清掉旧指纹，下次连接重新 TOFU
        clear_fingerprint(c, org_id)
    })
}

pub fn manual_addr(state: &EnclaveState, org_id: &str) -> Option<String> {
    state
        .store
        .with_conn(|c| Ok(get_setting(c, &format!("{}{}", HOST_ADDR_PREFIX, org_id))))
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
}

fn resolve_host(state: &EnclaveState, org_id: &str) -> Option<String> {
    manual_addr(state, org_id).or_else(|| state.discovered_host(org_id).map(|d| d.addr))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatusView {
    /// host / member / unknown
    pub role: String,
    pub running: bool,
    pub port: u16,
    pub host_name: String,
    pub served_org_ids: Vec<String>,
    pub disabled: bool,
    pub last_error: Option<String>,
    /// 当前组织手动填写的主机地址（成员端）
    pub manual_addr: Option<String>,
    /// 当前组织发现到的主机（成员端）
    pub discovered_addr: Option<String>,
    pub discovered_host_name: Option<String>,
    pub discovered_seen_at: Option<i64>,
    /// 本机作为主机时的证书指纹（sha256 hex）
    pub host_fingerprint: Option<String>,
    /// 成员端已固定的主机证书指纹（sha256 hex）
    pub pinned_fingerprint: Option<String>,
}

pub fn host_status(
    state: &EnclaveState,
    org_id: Option<&str>,
) -> Result<HostStatusView, String> {
    let served = state.store.with_conn(org::host_org_ids)?;
    let identity = state.store.with_conn(org::local_identity)?;
    let (running, port, last_error) = state.host.view();
    let role = if !served.is_empty() {
        "host"
    } else if identity.is_some() {
        "member"
    } else {
        "unknown"
    };
    let discovered = org_id.and_then(|o| state.discovered_host(o));
    let pinned_fingerprint = org_id.and_then(|o| {
        state
            .store
            .with_conn(|c| Ok(get_setting(c, &format!("{}{}", HOST_FP_PREFIX, o))))
            .ok()
            .flatten()
            .filter(|s| !s.trim().is_empty())
    });
    let host_fingerprint = if !served.is_empty() {
        tls::host_fingerprint().ok()
    } else {
        None
    };
    Ok(HostStatusView {
        role: role.to_string(),
        running,
        port,
        host_name: host_name(),
        served_org_ids: served,
        disabled: state.is_disabled(),
        last_error,
        manual_addr: org_id.and_then(|o| manual_addr(state, o)),
        discovered_addr: discovered.as_ref().map(|d| d.addr.clone()),
        discovered_host_name: discovered.as_ref().map(|d| d.host_name.clone()),
        discovered_seen_at: discovered.map(|d| d.seen_at),
        host_fingerprint,
        pinned_fingerprint,
    })
}

fn configured_host_port(state: &EnclaveState) -> u16 {
    std::env::var("PILOTDESK_ENCLAVE_HOST_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            state
                .store
                .with_conn(|c| Ok(get_setting(c, HOST_PORT_KEY)))
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(ENCLAVE_DEFAULT_PORT)
}

fn configured_discovery_port() -> u16 {
    std::env::var("PILOTDESK_ENCLAVE_DISCOVERY_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DISCOVERY_PORT)
}

fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "PilotDesk-Host".to_string())
}

fn host_org_ids(state: &EnclaveState) -> Vec<String> {
    state
        .store
        .with_conn(org::host_org_ids)
        .unwrap_or_default()
}

fn org_disabled(state: &EnclaveState, org_id: &str) -> bool {
    state
        .store
        .with_conn(|c| org::get_emergency(c, org_id))
        .ok()
        .flatten()
        .unwrap_or(false)
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
    .map_err(|e| format!("保存设置失败：{}", e))?;
    Ok(())
}

// ── 帧工具 ──────────────────────────────────────────────────────

async fn next_json<S>(rx: &mut S) -> Result<Value, String>
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match rx.next().await {
            Some(Ok(Message::Text(t))) => {
                return serde_json::from_str(&t).map_err(|e| format!("解析帧失败：{}", e))
            }
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
            Some(Ok(Message::Close(_))) | None => return Err("连接已关闭".to_string()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(format!("连接错误：{}", e)),
        }
    }
}

async fn send_json<S>(tx: &mut S, text: &str) -> Result<(), String>
where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    tx.send(Message::Text(text.to_string()))
        .await
        .map_err(|e| format!("发送帧失败：{}", e))
}

fn hello_err(message: &str) -> String {
    json!({ "type": "hello_err", "message": message }).to_string()
}

fn res_ok(id: &str, data: Value) -> String {
    json!({ "type": "res", "id": id, "ok": true, "data": data }).to_string()
}

fn res_err(id: &str, message: &str) -> String {
    json!({ "type": "res", "id": id, "ok": false, "error": message }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_addr_validation() {
        assert_eq!(
            "ws://192.168.1.9:47820/".trim_start_matches("ws://").trim_end_matches('/'),
            "192.168.1.9:47820"
        );
        assert!("192.168.1.9".rsplit_once(':').is_none());
    }

    #[test]
    fn discovery_ttl_is_positive() {
        assert!(DISCOVERY_TTL_SECS > 0);
    }

    /// 缓存上限：按最近访问时间淘汰，淘汰后总量不高于目标值。
    #[test]
    fn cache_eviction_evicts_oldest() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-enclave-cache-test-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        let make = |name: &str, age_secs: u64| {
            let path = dir.join(name);
            std::fs::write(&path, [0u8; 4]).unwrap();
            let file = OpenOptions::new().write(true).open(&path).unwrap();
            file.set_modified(now - Duration::from_secs(age_secs)).unwrap();
            path
        };
        let a = make("a.bin", 3 * 3600);
        let b = make("b.bin", 2 * 3600);
        let c = make("c.bin", 3600);

        // 总量 12 字节，上限 8 → 目标 7：淘汰 a、b，保留 c
        enforce_cache_limit(&dir, 8).unwrap();
        assert!(!a.exists(), "最旧的 a 应被淘汰");
        assert!(!b.exists(), "次旧的 b 应被淘汰");
        assert!(c.exists(), "最新的 c 应保留");

        let remaining: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        assert!(remaining <= 7);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 协议级联调：成员端握手（快照模式）→ `task.list` 往返。
    #[tokio::test]
    async fn handshake_and_task_list_over_ws() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-enclave-host-test-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(
            super::super::db::EnclaveStore::open(&dir.join("enclave.db")).unwrap(),
        );
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO local_identity (user_id, email) VALUES ('1', 'host@example.com')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id)
                     VALUES ('org1', 'Org', '1', 'owner', '1')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO member_snapshot (org_id, user_id, role, display_name)
                     VALUES ('org1', '2', 'member', 'Member')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
        let state = std::sync::Arc::new(EnclaveState::new(store));

        let host_tls = tls::ensure_host_tls_in(&dir.join("tls")).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn({
            let st = state.clone();
            let acceptor = host_tls.acceptor.clone();
            async move {
                if let Ok((stream, peer)) = listener.accept().await {
                    if let Ok(tls_stream) = acceptor.accept(stream).await {
                        handle_conn(tls_stream, peer, st).await;
                    }
                }
            }
        });

        let (ws, _) = tls::connect_pinned(
            &format!("wss://127.0.0.1:{}", port),
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let (mut tx, mut rx) = ws.split();
        tx.send(Message::Text(
            json!({
                "type": "hello", "protocol": PROTOCOL,
                "orgId": "org1", "userId": "2", "token": "",
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let hello = next_json(&mut rx).await.unwrap();
        assert_eq!(hello["type"], "hello_ok");
        assert_eq!(hello["authMode"], "snapshot");

        tx.send(Message::Text(
            json!({
                "type": "req", "id": "r1", "method": "task.list",
                "params": { "orgId": "org1" },
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let res = next_json(&mut rx).await.unwrap();
        assert_eq!(res["ok"], true);
        assert!(res["data"].as_array().unwrap().is_empty());

        drop(tx);
        drop(rx);
        drop(state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 局域网转移协议：pending → accept → package 分块 → verified。
    #[tokio::test]
    async fn lan_transfer_over_ws() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-enclave-lan-test-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(
            super::super::db::EnclaveStore::open(&dir.join("enclave.db")).unwrap(),
        );
        store
            .with_conn(|c| {
                // 测试隔离：团队文件根指向临时目录
                c.execute(
                    "INSERT OR REPLACE INTO enclave_settings (key, value, updated_at)
                     VALUES ('team_files_root', ?1, 0)",
                    params![dir.to_string_lossy().to_string()],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO local_identity (user_id, email) VALUES ('1', 'host@example.com')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id)
                     VALUES ('org1', 'Org', '1', 'owner', '1')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO member_snapshot (org_id, user_id, role, display_name)
                     VALUES ('org1', '2', 'member', 'Member')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
        let state = std::sync::Arc::new(EnclaveState::new(store));
        let offer = state
            .store
            .with_conn(|c| transfer::create_lan_offer(c, "org1", "2"))
            .unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let host_tls = tls::ensure_host_tls_in(&dir.join("tls")).unwrap();
        tokio::spawn({
            let st = state.clone();
            let acceptor = host_tls.acceptor.clone();
            async move {
                if let Ok((stream, peer)) = listener.accept().await {
                    if let Ok(tls_stream) = acceptor.accept(stream).await {
                        handle_conn(tls_stream, peer, st).await;
                    }
                }
            }
        });

        let (ws, _) = tls::connect_pinned(
            &format!("wss://127.0.0.1:{}", port),
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let (mut tx, mut rx) = ws.split();
        tx.send(Message::Text(
            json!({
                "type": "hello", "protocol": PROTOCOL,
                "orgId": "org1", "userId": "2", "token": "",
            })
            .to_string(),
        ))
        .await
        .unwrap();
        assert_eq!(next_json(&mut rx).await.unwrap()["type"], "hello_ok");

        // pending：只能看到发给自己的请求
        tx.send(Message::Text(
            json!({"type":"req","id":"p1","method":"transfer.pending","params":{"orgId":"org1"}})
                .to_string(),
        ))
        .await
        .unwrap();
        let pending = next_json(&mut rx).await.unwrap();
        assert_eq!(pending["ok"], true);
        assert_eq!(pending["data"].as_array().unwrap().len(), 1);

        // accept：返回一次性口令
        tx.send(Message::Text(
            json!({
                "type":"req","id":"p2","method":"transfer.accept",
                "params":{"orgId":"org1","transferId":offer.transfer_id}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let accepted = next_json(&mut rx).await.unwrap();
        assert_eq!(accepted["ok"], true);
        assert!(!accepted["data"]["passphrase"].as_str().unwrap().is_empty());

        // package：按块读取，首块把状态推进到 migrating
        tx.send(Message::Text(
            json!({
                "type":"req","id":"p3","method":"transfer.package",
                "params":{"orgId":"org1","transferId":offer.transfer_id,"offset":0,"len":1024}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let chunk = next_json(&mut rx).await.unwrap();
        assert_eq!(chunk["ok"], true);
        assert!(!chunk["data"]["data"].as_str().unwrap().is_empty());

        // verified：主机侧记录推进到 verified
        tx.send(Message::Text(
            json!({
                "type":"req","id":"p4","method":"transfer.verified",
                "params":{"orgId":"org1","transferId":offer.transfer_id}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let verified = next_json(&mut rx).await.unwrap();
        assert_eq!(verified["ok"], true);
        assert_eq!(verified["data"]["phase"], "verified");

        let _ = std::fs::remove_file(transfer::lan_package_path(&offer.transfer_id));
        drop(tx);
        drop(rx);
        drop(state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 通用命令转发：成员经 `enclave.call` 建任务 / 查详情 / 改状态，actor 归属成员。
    #[tokio::test]
    async fn member_rpc_over_ws() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-enclave-rpc-test-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(
            super::super::db::EnclaveStore::open(&dir.join("enclave.db")).unwrap(),
        );
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT OR REPLACE INTO enclave_settings (key, value, updated_at)
                     VALUES ('team_files_root', ?1, 0)",
                    params![dir.to_string_lossy().to_string()],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO local_identity (user_id, email) VALUES ('1', 'host@example.com')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id)
                     VALUES ('org1', 'Org', '1', 'owner', '1')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO member_snapshot (org_id, user_id, role, display_name)
                     VALUES ('org1', '2', 'member', 'Member')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
        let state = std::sync::Arc::new(EnclaveState::new(store));

        let host_tls = tls::ensure_host_tls_in(&dir.join("tls")).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn({
            let st = state.clone();
            let acceptor = host_tls.acceptor.clone();
            async move {
                if let Ok((stream, peer)) = listener.accept().await {
                    if let Ok(tls_stream) = acceptor.accept(stream).await {
                        handle_conn(tls_stream, peer, st).await;
                    }
                }
            }
        });

        let (ws, _) = tls::connect_pinned(
            &format!("wss://127.0.0.1:{}", port),
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let (mut tx, mut rx) = ws.split();
        tx.send(Message::Text(
            json!({
                "type": "hello", "protocol": PROTOCOL,
                "orgId": "org1", "userId": "2", "token": "",
            })
            .to_string(),
        ))
        .await
        .unwrap();
        assert_eq!(next_json(&mut rx).await.unwrap()["type"], "hello_ok");

        // 预置一条团队知识（供成员端 Agent 工具经主机检索）
        state
            .store
            .with_conn(|c| {
                let now = chrono::Utc::now().timestamp();
                c.execute(
                    "INSERT INTO knowledge_bases
                        (id, name, scope, owner_type, owner_id, is_implicit, created_at, updated_at)
                     VALUES ('kb-org-org1', '团队库', 'org', 'org', 'org1', 1, ?1, ?1)",
                    params![now],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO knowledge_base_memories
                        (knowledge_base_id, memory_id, title, content, tags, origin,
                         source_ref, added_by, added_at, updated_at)
                     VALUES ('kb-org-org1', 'task:t1', '任务产出', '团队知识内容', 'tag',
                             'task_output', 't1', '1', ?1, ?1)",
                    params![now],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();

        // 成员建任务：creatorId 必须是成员 2，而不是主机身份 1
        tx.send(Message::Text(
            json!({
                "type":"req","id":"c1","method":"enclave.call",
                "params":{
                    "orgId":"org1","command":"task.create",
                    "payload":{"input":{"title":"成员创建的任务"}}
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let created = next_json(&mut rx).await.unwrap();
        assert_eq!(created["ok"], true, "create failed: {}", created);
        let task_id = created["data"]["id"].as_str().unwrap().to_string();
        assert_eq!(created["data"]["creatorId"], "2");

        // 查详情
        tx.send(Message::Text(
            json!({
                "type":"req","id":"c2","method":"enclave.call",
                "params":{"orgId":"org1","command":"task.get","payload":{"taskId":task_id}}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let got = next_json(&mut rx).await.unwrap();
        assert_eq!(got["ok"], true);
        assert_eq!(got["data"]["title"], "成员创建的任务");

        // 改状态：待办 → 进行中
        tx.send(Message::Text(
            json!({
                "type":"req","id":"c3","method":"enclave.call",
                "params":{
                    "orgId":"org1","command":"task.set_status",
                    "payload":{"taskId":task_id,"status":"in_progress"}
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let moved = next_json(&mut rx).await.unwrap();
        assert_eq!(moved["ok"], true, "set_status failed: {}", moved);
        assert_eq!(moved["data"]["status"], "in_progress");

        // 成员上传文件：begin → chunk → finish
        let content = b"hello world".to_vec();
        tx.send(Message::Text(
            json!({
                "type":"req","id":"u1","method":"file.upload_begin",
                "params":{"orgId":"org1","fileName":"note.txt"}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let begin = next_json(&mut rx).await.unwrap();
        assert_eq!(begin["ok"], true);
        let upload_id = begin["data"]["uploadId"].as_str().unwrap().to_string();

        tx.send(Message::Text(
            json!({
                "type":"req","id":"u2","method":"file.upload_chunk",
                "params":{
                    "orgId":"org1","uploadId":upload_id,"offset":0,
                    "data":BASE64.encode(&content)
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        assert_eq!(next_json(&mut rx).await.unwrap()["ok"], true);

        tx.send(Message::Text(
            json!({
                "type":"req","id":"u3","method":"file.upload_finish",
                "params":{
                    "orgId":"org1","uploadId":upload_id,"taskId":task_id,
                    "fileName":"note.txt","sha256":transfer::hex_sha256(&content)
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let finished = next_json(&mut rx).await.unwrap();
        assert_eq!(finished["ok"], true, "upload failed: {}", finished);
        let file_id = finished["data"]["id"].as_str().unwrap().to_string();

        // 任务附件列表：成员拿不到主机本地路径
        tx.send(Message::Text(
            json!({
                "type":"req","id":"u4","method":"enclave.call",
                "params":{"orgId":"org1","command":"file.task_list","payload":{"taskId":task_id}}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let listed = next_json(&mut rx).await.unwrap();
        assert_eq!(listed["ok"], true);
        assert_eq!(listed["data"].as_array().unwrap().len(), 1);
        assert_eq!(listed["data"][0]["absPath"], "");

        // 元信息 + 下载
        tx.send(Message::Text(
            json!({
                "type":"req","id":"u5","method":"file.stat",
                "params":{"orgId":"org1","fileId":file_id}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let stat = next_json(&mut rx).await.unwrap();
        assert_eq!(stat["ok"], true);
        assert_eq!(stat["data"]["fileSize"], content.len());

        tx.send(Message::Text(
            json!({
                "type":"req","id":"u6","method":"file.fetch",
                "params":{"orgId":"org1","fileId":file_id,"offset":0,"len":64}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let fetched = next_json(&mut rx).await.unwrap();
        assert_eq!(fetched["ok"], true);
        assert_eq!(
            BASE64.decode(fetched["data"]["data"].as_str().unwrap()).unwrap(),
            content
        );
        assert_eq!(fetched["data"]["eof"], true);

        // 过程记录附件：先上传临时令牌，再随记录提交
        let progress_content = b"progress note".to_vec();
        tx.send(Message::Text(
            json!({
                "type":"req","id":"p1","method":"file.upload_begin",
                "params":{"orgId":"org1","fileName":"attachment"}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let pbegin = next_json(&mut rx).await.unwrap();
        let pupload_id = pbegin["data"]["uploadId"].as_str().unwrap().to_string();
        tx.send(Message::Text(
            json!({
                "type":"req","id":"p2","method":"file.upload_chunk",
                "params":{
                    "orgId":"org1","uploadId":pupload_id,"offset":0,
                    "data":BASE64.encode(&progress_content)
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        assert_eq!(next_json(&mut rx).await.unwrap()["ok"], true);
        tx.send(Message::Text(
            json!({
                "type":"req","id":"p3","method":"file.upload_keep",
                "params":{
                    "orgId":"org1","uploadId":pupload_id,
                    "sha256":transfer::hex_sha256(&progress_content)
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let kept = next_json(&mut rx).await.unwrap();
        assert_eq!(kept["ok"], true);
        let token = kept["data"]["token"].as_str().unwrap().to_string();

        tx.send(Message::Text(
            json!({
                "type":"req","id":"p4","method":"enclave.call",
                "params":{
                    "orgId":"org1","command":"task.progress_add",
                    "payload":{"taskId":task_id,"content":"进度说明","attachments":[token]}
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let progress = next_json(&mut rx).await.unwrap();
        assert_eq!(progress["ok"], true, "progress failed: {}", progress);
        assert_eq!(progress["data"]["attachments"].as_array().unwrap().len(), 1);

        // 安全：客户端不能直接指定主机路径当附件
        tx.send(Message::Text(
            json!({
                "type":"req","id":"p5","method":"enclave.call",
                "params":{
                    "orgId":"org1","command":"task.progress_add",
                    "payload":{
                        "taskId":task_id,"content":"恶意路径",
                        "attachments":["C:\\Windows\\win.ini"]
                    }
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let rejected = next_json(&mut rx).await.unwrap();
        assert_eq!(rejected["ok"], false, "raw path must be rejected: {}", rejected);

        // 治理只读：成员端 Agent 工具向主机复核策略
        tx.send(Message::Text(
            json!({
                "type":"req","id":"k0","method":"enclave.call",
                "params":{"orgId":"org1","command":"policy.get","payload":{}}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let policy = next_json(&mut rx).await.unwrap();
        assert_eq!(policy["ok"], true);
        assert_eq!(policy["data"]["explicit"], false);
        assert_eq!(policy["data"]["allowCloudModel"], false);

        // 知识库检索（成员端 Agent 工具同款 RPC）
        tx.send(Message::Text(
            json!({
                "type":"req","id":"k1","method":"enclave.call",
                "params":{
                    "orgId":"org1","command":"kb.search",
                    "payload":{"query":"团队知识","limit":10}
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let found = next_json(&mut rx).await.unwrap();
        assert_eq!(found["ok"], true, "kb.search failed: {}", found);
        assert_eq!(found["data"].as_array().unwrap().len(), 1);
        assert_eq!(found["data"][0]["memoryId"], "task:t1");

        drop(tx);
        drop(rx);
        drop(state);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
