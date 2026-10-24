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

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
use tokio::sync::mpsc;
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
    /// 设备接管（同账号换机）运行态：迁移期间冻结写请求。
    handover: Mutex<Option<HandoverRuntime>>,
    /// 已连接成员的出站通道（按组织），用于变更广播。
    subscribers: Mutex<HashMap<String, Vec<Subscriber>>>,
    /// 本机 UI 的事件出口：app 启动时注入一个通道，由 lib.rs 侧转发成 Tauri 事件。
    /// 这里刻意不持有 `AppHandle`——测试二进制不链接 GUI 运行时（无 manifest 会起不来）。
    ui_tx: Mutex<Option<mpsc::UnboundedSender<Value>>>,
    /// 订阅 id 分配器
    next_sub_id: AtomicU64,
}

/// 一条常驻成员连接：只要还连着，就接收该组织的变更事件，并计入「在线成员」。
struct Subscriber {
    id: u64,
    /// 连接握手确认过的成员 user id（在线列表按它去重）
    user_id: String,
    tx: mpsc::UnboundedSender<String>,
}

/// 进行中的设备接管：新设备从本机拉取加密包，校验后再由平台登记为新主机。
pub struct HandoverRuntime {
    pub id: String,
    pub org_id: String,
    pub user_id: String,
    /// 接管方设备 id（提交后写入本地快照，本机随即停服）
    pub device_id: String,
    /// 接管方设备名
    pub device_name: String,
    pub passphrase: String,
    pub package_hash: String,
    pub path: PathBuf,
    pub prepared_at: i64,
    pub verified: bool,
}

/// 接管迁移窗口：超时未提交即自动回滚（旧主机继续权威）。
const HANDOVER_TTL_SECS: i64 = 10 * 60;
/// 接管准备/分块下载的超时（大包可能较慢）。
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(600);

impl HostRuntime {
    fn handover_snapshot(&self) -> Option<(String, bool)> {
        let g = self.handover.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().map(|h| (h.org_id.clone(), h.verified))
    }

    /// 是否因接管迁移而冻结该组织的写请求。
    fn handover_frozen(&self, org_id: &str) -> bool {
        self.handover
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|h| h.org_id == org_id)
            .unwrap_or(false)
    }

    fn clear_handover(&self) -> Option<HandoverRuntime> {
        self.handover
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
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

    /// 注入本机 UI 的事件出口（app 启动时调用一次）。
    pub fn set_ui_sink(&self, tx: mpsc::UnboundedSender<Value>) {
        let mut g = self.ui_tx.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(tx);
    }

    fn ui_sink(&self) -> Option<mpsc::UnboundedSender<Value>> {
        self.ui_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 登记一条常驻成员连接，返回订阅 id（断开时用同一个 id 注销）。
    fn register_subscriber(
        &self,
        org_id: &str,
        user_id: &str,
        tx: mpsc::UnboundedSender<String>,
    ) -> u64 {
        let id = self.next_sub_id.fetch_add(1, Ordering::Relaxed) + 1;
        log::debug!("[enclave] 成员 {} 订阅组织 {} 的变更", user_id, org_id);
        let mut g = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(org_id.to_string())
            .or_default()
            .push(Subscriber {
                id,
                user_id: user_id.to_string(),
                tx,
            });
        id
    }

    /// 该组织当前在线的成员 user id（按连接去重；主机 owner 不在订阅列表里）。
    pub fn online_member_ids(&self, org_id: &str) -> Vec<String> {
        let g = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        let mut ids: Vec<String> = g
            .get(org_id)
            .map(|list| list.iter().map(|s| s.user_id.clone()).collect())
            .unwrap_or_default();
        ids.sort();
        ids.dedup();
        ids
    }

    fn unregister_subscriber(&self, org_id: &str, id: u64) {
        let mut g = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(list) = g.get_mut(org_id) {
            list.retain(|s| s.id != id);
            if list.is_empty() {
                g.remove(org_id);
            }
        }
    }

    /// 向该组织所有在线成员推一帧（`except` = 跳过发起本次变更的连接）；
    /// 发送失败（连接已断）的订阅顺手剔除。
    fn broadcast(&self, org_id: &str, frame: &str, except: Option<u64>) {
        let mut g = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        let Some(list) = g.get_mut(org_id) else { return };
        list.retain(|s| {
            if Some(s.id) == except {
                return true;
            }
            s.tx.send(frame.to_string()).is_ok()
        });
        if list.is_empty() {
            g.remove(org_id);
        }
    }
}

/// 写命令 → 变更主题（读命令返回 `None`，不触发广播）。
pub fn change_topic(command: &str) -> Option<&'static str> {
    if command.is_empty()
        || command.ends_with(".list")
        || command.ends_with(".get")
        || command.ends_with(".search")
        || command.ends_with(".preview")
        || command.ends_with(".capability")
    {
        return None;
    }
    // 已读状态只影响本人，不广播
    if command == "notification.mark_read" {
        return None;
    }
    // 下划线命名的历史读命令（不以 `.list` 结尾）：读不该触发任何人刷新，
    // 否则「成员收到事件 → 重拉列表 → 又广播」会互相触发。
    if matches!(
        command,
        "kb.file_list" | "kb.entry_get" | "kb.orphan_files" | "kb.file_relations"
    ) {
        return None;
    }
    if command.starts_with("kb.") {
        return Some("kb");
    }
    if command.starts_with("notification.") {
        return Some("notify");
    }
    if command.starts_with("task.")
        || command.starts_with("report.")
        || command.starts_with("progress.")
        || command.starts_with("comment.")
        || command.starts_with("file.")
    {
        return Some("task");
    }
    if command.starts_with("policy.") || command.starts_with("emergency.") {
        return Some("org");
    }
    None
}

/// 一次业务变更的提示：主题 + 可选对象 id。
///
/// 前端据此只刷新当前对象（当前任务详情 / 当前库），避免整页重拉。
#[derive(Clone)]
pub struct ChangeHint {
    pub topic: String,
    pub task_id: Option<String>,
    pub kb_id: Option<String>,
}

/// 写命令 + 参数 → 变更提示（读命令返回 `None`）。
pub fn change_hint(command: &str, payload: &Value) -> Option<ChangeHint> {
    let topic = change_topic(command)?;
    let pick = |key: &str| {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(ChangeHint {
        topic: topic.to_string(),
        task_id: pick("taskId"),
        kb_id: pick("kbId"),
    })
}

/// 广播一次业务变更：本机 UI 收 Tauri 事件，已连接的成员收到 WS `event` 帧。
///
/// 事件只带主题与对象 id，不带业务数据——成员端据此重拉，
/// 保证「不缓存团队数据」的边界不被实时通道打破。
///
/// `except` 用于跳过发起本次变更的那条连接：请求方已经拿到了响应，无需再收一遍事件。
fn emit_change(
    state: &EnclaveState,
    org_id: &str,
    hint: &ChangeHint,
    except: Option<u64>,
) {
    let at = chrono::Utc::now().timestamp();
    let payload = json!({
        "orgId": org_id,
        "topic": hint.topic,
        "taskId": hint.task_id,
        "kbId": hint.kb_id,
        "at": at,
    });
    if let Some(tx) = state.host.ui_sink() {
        let _ = tx.send(payload.clone());
    }
    let mut frame = payload;
    if let Some(obj) = frame.as_object_mut() {
        obj.insert("type".into(), json!("event"));
        // 空 id 不下发，避免前端把 null 当成「已知对象」
        obj.retain(|_, v| !v.is_null());
    }
    state.host.broadcast(org_id, &frame.to_string(), except);
}

/// 带对象 id 的变更广播（成员端请求路径用；跳过发起连接）。
pub fn notify_change_hint(
    state: &EnclaveState,
    org_id: &str,
    hint: &ChangeHint,
    except: Option<u64>,
) {
    emit_change(state, org_id, hint, except);
}

/// 主机本机写入：本机 UI + 所有在线成员都要刷新。
pub fn notify_change(state: &EnclaveState, org_id: &str, topic: &str) {
    emit_change(
        state,
        org_id,
        &ChangeHint {
            topic: topic.to_string(),
            task_id: None,
            kb_id: None,
        },
        None,
    );
}

/// 该组织当前在线的成员 user id（主机本地读取；成员端经 `org.online` RPC 取）。
pub fn online_member_ids(state: &EnclaveState, org_id: &str) -> Vec<String> {
    state.host.online_member_ids(org_id)
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
    {
        // 主机自动备份（一期前置条件：主机单点故障的本地兜底）
        let st = state.clone();
        tokio::spawn(async move { super::backup::run_loop(st).await });
    }

    let mut listener: Option<(TcpListener, tokio_rustls::TlsAcceptor)> = None;
    // 定期复核「本机是否仍是主机」：云端所有权转让后旧主机必须停服
    let mut last_role_check = std::time::Instant::now();
    loop {
        if last_role_check.elapsed() >= Duration::from_secs(30) {
            last_role_check = std::time::Instant::now();
            expire_handover(&state);
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

/// 设备接管迁移包的临时落点（与组织转让的 `transfer/` 分开）。
fn handover_dir() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join("pilotdesk-enclave-handover-test")
    }
    #[cfg(not(test))]
    {
        crate::utils::paths::app_root_dir()
            .join("enclave")
            .join("handover")
    }
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
    let file_name = Path::new(src_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment")
        .to_string();
    let begin = session
        .request("file.upload_begin", json!({ "orgId": org_id, "fileName": file_name }))
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

/// 同账号换机接管结果（前端提示用）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoverOutcome {
    pub org_id: String,
    pub package_hash: String,
    pub file_count: usize,
}

/// 从旧主机在线接管：拉取一致性快照 → 校验 → 导入 → 平台登记 → 通知旧机停服。
///
/// 只有全部成功后才登记本机；任一步失败，旧主机仍为权威源（不会出现空库主机）。
pub async fn takeover_from_host(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
) -> Result<HandoverOutcome, String> {
    let mut session = HostSession::connect(state, db, org_id).await?;
    let prepared = session
        .request_with_timeout(
            "handover.prepare",
            json!({ "orgId": org_id }),
            HANDOVER_TIMEOUT,
        )
        .await;
    let prepared = match prepared {
        Ok(v) => v,
        Err(e) => {
            session.close().await;
            return Err(e);
        }
    };
    let handover_id = prepared["handoverId"].as_str().unwrap_or("").to_string();
    let expected_hash = prepared["packageHash"].as_str().unwrap_or("").to_string();
    let passphrase = prepared["passphrase"].as_str().unwrap_or("").to_string();
    if handover_id.is_empty() || expected_hash.is_empty() || passphrase.is_empty() {
        session.close().await;
        return Err("旧主机未返回完整的接管信息".to_string());
    }

    let dest = std::env::temp_dir().join(format!("pilotdesk-handover-{}.pdesk", handover_id));
    let download = download_handover_chunks(&mut session, org_id, &handover_id, &dest).await;
    session.close().await;
    if let Err(e) = download {
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }

    let actual_hash = transfer::hex_sha256(
        &std::fs::read(&dest).map_err(|e| format!("读取接管迁移包失败：{}", e))?,
    );
    if actual_hash != expected_hash {
        let _ = std::fs::remove_file(&dest);
        return Err("接管迁移包校验失败（哈希不一致），已中止".to_string());
    }

    let imported = state
        .store
        .with_conn(|c| transfer::import_handover_package(c, dest.to_string_lossy().as_ref(), &passphrase));
    let _ = std::fs::remove_file(&dest);
    let imported = imported?;

    // 数据已到本机且校验通过，才登记为主机
    let reg = register_with_platform(db, state, org_id, true).await?;
    let host = reg
        .host
        .ok_or_else(|| "平台未返回主机登记结果".to_string())?;
    if !reg.claimed {
        return Err(format!(
            "主机登记未生效：当前主机仍为「{}」",
            host.device_name
        ));
    }

    // 通知旧主机提交（失败不影响新主机，旧主机会在下一轮快照后停服）
    if let Ok(mut s) = HostSession::connect(state, db, org_id).await {
        let _ = s
            .request(
                "handover.committed",
                json!({ "orgId": org_id, "handoverId": handover_id }),
            )
            .await;
        s.close().await;
    }

    apply_host_info(state, org_id, &host)?;
    Ok(HandoverOutcome {
        org_id: imported.org_id,
        package_hash: imported.package_hash,
        file_count: imported.file_count,
    })
}

async fn download_handover_chunks(
    session: &mut HostSession,
    org_id: &str,
    handover_id: &str,
    dest: &Path,
) -> Result<(), String> {
    let mut file = std::fs::File::create(dest).map_err(|e| format!("创建接管临时文件失败：{}", e))?;
    let mut offset = 0u64;
    loop {
        let chunk = session
            .request_with_timeout(
                "handover.chunk",
                json!({
                    "orgId": org_id,
                    "handoverId": handover_id,
                    "offset": offset,
                    "len": transfer::LAN_CHUNK_MAX,
                }),
                REQUEST_TIMEOUT,
            )
            .await?;
        let data = BASE64
            .decode(chunk["data"].as_str().unwrap_or(""))
            .map_err(|e| format!("解码接管分块失败：{}", e))?;
        file.write_all(&data)
            .map_err(|e| format!("写入接管迁移包失败：{}", e))?;
        offset += data.len() as u64;
        if chunk["eof"].as_bool() == Some(true) {
            break;
        }
        if data.is_empty() {
            return Err("旧主机返回了空分块，接管中断".to_string());
        }
    }
    Ok(())
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

/// 成员端取在线成员：`org.online` 是**主机协议方法**（读主机内存里的订阅表），
/// 不是 `enclave.call` 里的 RPC 命令。
///
/// 之前套用 [`call_host_without_token`]（它固定发 `enclave.call`）会被主机当成
/// 「未知的飞地命令：org.online」拒绝，成员端于是永远拿不到列表 —— 表现为成员设备上
/// 自己的在线状态恒为「—」，而主机侧能看到。
pub async fn online_members_via_host(
    state: &EnclaveState,
    org_id: &str,
) -> Result<Vec<String>, String> {
    let mut session = HostSession::connect_with_db(state, None, org_id).await?;
    let data = session
        .request("org.online", json!({ "orgId": org_id }))
        .await;
    session.close().await;
    let value = data?;
    serde_json::from_value(value).map_err(|e| format!("解析在线成员失败：{}", e))
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

/// 拒绝转移请求（受让人侧）：请求作废，迁移包与口令由主机清理。
pub async fn reject_lan_transfer(
    state: &EnclaveState,
    db: &crate::DbState,
    org_id: &str,
    transfer_id: &str,
) -> Result<Value, String> {
    let mut session = HostSession::connect(state, db, org_id).await?;
    let data = session
        .request(
            "transfer.reject",
            json!({ "orgId": org_id, "transferId": transfer_id }),
        )
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
pub(crate) struct HostSession {
    tx: WsTx,
    rx: WsRx,
}

impl HostSession {
    pub(crate) async fn connect(
        state: &EnclaveState,
        db: &crate::DbState,
        org_id: &str,
    ) -> Result<Self, String> {
        Self::connect_with_db(state, Some(db), org_id).await
    }

    /// 常驻监听：等待主机推送的 `event` 帧；空闲时定期 ping 保活（主机 180s 无请求会断开）。
    pub(crate) async fn wait_event(&mut self) -> Result<Value, String> {
        loop {
            match tokio::time::timeout(Duration::from_secs(45), next_json(&mut self.rx)).await {
                Ok(Ok(v)) => {
                    if v["type"].as_str() == Some("event") {
                        return Ok(v);
                    }
                    // 其他帧（如保活 ping 的响应）直接忽略
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    let id = crate::utils::new_id();
                    self.tx
                        .send(Message::Text(
                            json!({ "type": "req", "id": id, "method": "ping", "params": {} })
                                .to_string(),
                        ))
                        .await
                        .map_err(|e| format!("发送保活失败：{}", e))?;
                }
            }
        }
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
                    "deviceId": device_id(),
                    "deviceName": host_name(),
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
        self.request_with_timeout(method, params, REQUEST_TIMEOUT).await
    }

    async fn request_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
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
        let reply = tokio::time::timeout(timeout, next_json(&mut self.rx))
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
    /// 设备标识（用于区分同一账号的多台设备，以及设备接管）
    #[serde(default)]
    device_id: String,
    /// 设备名（展示用）
    #[serde(default)]
    device_name: String,
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

    // 常驻连接：写统一走通道，由独立 writer 任务发送 —— 这样广播侧也能往这条连接推事件帧。
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if send_json(&mut tx, &msg).await.is_err() {
                break;
            }
        }
    });
    let sub_id = state
        .host
        .register_subscriber(&hello.org_id, &hello.user_id, out_tx.clone());

    loop {
        let req_val = match tokio::time::timeout(IDLE_TIMEOUT, next_json(&mut rx)).await {
            Ok(Ok(v)) => v,
            _ => break,
        };
        let req: RequestMsg = match serde_json::from_value::<RequestMsg>(req_val) {
            Ok(r) if r.kind == "req" => r,
            Ok(_) => continue,
            Err(e) => {
                let _ = out_tx.send(res_err("", &format!("请求格式不正确：{}", e)));
                continue;
            }
        };
        if org_disabled(&state, &hello.org_id) && !peer.ip().is_loopback() {
            let _ = out_tx.send(res_err(&req.id, "飞地对外服务已临时关闭"));
            break;
        }
        // 设备接管迁移期间冻结写请求（只放行接管协议与只读命令）
        if state.host.handover_frozen(&hello.org_id)
            && !handover_allows(&req.method, &req.params)
        {
            let _ = out_tx.send(res_err(&req.id, "主机正在进行设备接管迁移，请稍候"));
            continue;
        }
        let reply = match req.method.as_str() {
            "ping" => res_ok(&req.id, json!({ "pong": true })),
            "handover.prepare" => match org_param(&req, &hello) {
                Ok(org) => match handle_handover_prepare(
                    &state,
                    org,
                    &hello.user_id,
                    &hello.device_id,
                    &hello.device_name,
                ) {
                    Ok(v) => res_ok(&req.id, v),
                    Err(e) => res_err(&req.id, &e),
                },
                Err(e) => res_err(&req.id, &e),
            },
            "handover.chunk" => match org_param(&req, &hello) {
                Ok(org) => {
                    let handover_id = req.params["handoverId"].as_str().unwrap_or("").to_string();
                    let offset = req.params["offset"].as_u64().unwrap_or(0);
                    let len = req.params["len"].as_u64().unwrap_or(transfer::LAN_CHUNK_MAX);
                    match handle_handover_chunk(
                        &state,
                        &handover_id,
                        org,
                        &hello.user_id,
                        &hello.device_id,
                        offset,
                        len,
                    ) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "handover.verified" => match org_param(&req, &hello) {
                Ok(org) => {
                    let handover_id = req.params["handoverId"].as_str().unwrap_or("").to_string();
                    match handle_handover_verified(
                        &state,
                        &handover_id,
                        org,
                        &hello.user_id,
                        &hello.device_id,
                    ) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "handover.committed" => match org_param(&req, &hello) {
                Ok(org) => {
                    let handover_id = req.params["handoverId"].as_str().unwrap_or("").to_string();
                    match handle_handover_committed(
                        &state,
                        &handover_id,
                        org,
                        &hello.user_id,
                        &hello.device_id,
                    ) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
            "handover.abort" => match org_param(&req, &hello) {
                Ok(org) => {
                    let handover_id = req.params["handoverId"].as_str().unwrap_or("").to_string();
                    match handle_handover_abort(
                        &state,
                        &handover_id,
                        org,
                        &hello.user_id,
                        &hello.device_id,
                    ) {
                        Ok(v) => res_ok(&req.id, v),
                        Err(e) => res_err(&req.id, &e),
                    }
                }
                Err(e) => res_err(&req.id, &e),
            },
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
            // 在线成员：只读，成员之间互相可见
            "org.online" => {
                let req_org = req.params["orgId"].as_str().unwrap_or(hello.org_id.as_str());
                if req_org != hello.org_id {
                    res_err(&req.id, "不能跨组织访问")
                } else {
                    res_ok(&req.id, json!(online_member_ids(&state, req_org)))
                }
            }
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
                    match handle_file_fetch(&state, &org, &hello.user_id, &file_id, offset, len) {
                        Ok(v) => {
                            // 下载审计：成员把原件拉到本机时记一条（只在首个分块记，避免每块刷一条）。
                            // 主机自己打开文件走本地路径、不会经过这里，所以审计天然只记"外带"。
                            if offset == 0 {
                                let _ = state.store.with_conn(|c| {
                                    let meta = files::file_meta(c, &org, &file_id)?;
                                    let detail = json!({
                                        "fileId": file_id,
                                        "fileName": meta.file_name,
                                        "fileSize": meta.file_size,
                                    });
                                    crate::enclave::org::audit(
                                        c,
                                        &org,
                                        &hello.user_id,
                                        "file.download",
                                        &detail.to_string(),
                                    )
                                });
                            }
                            res_ok(&req.id, v)
                        }
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
            "transfer.reject" => match (org_param(&req, &hello), req.params["transferId"].as_str()) {
                (Ok(org), Some(id)) => {
                    match handle_transfer_reject(&state, org, id, &hello.user_id) {
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
        // 先回响应，再广播；发起连接本身跳过（它已经拿到响应）
        let hint = request_hint(&req).filter(|_| reply_ok(&reply));
        if out_tx.send(reply).is_err() {
            break;
        }
        if let Some(hint) = hint {
            let org = req.params["orgId"].as_str().unwrap_or(hello.org_id.as_str());
            notify_change_hint(&state, org, &hint, Some(sub_id));
        }
    }

    state.host.unregister_subscriber(&hello.org_id, sub_id);
    drop(out_tx);
    let _ = writer.await;
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

// ── 设备接管（同账号换机）────────────────────────────────────────

/// 发起设备接管：校验来者是同一组织的 owner 且来自另一台设备，
/// 生成一致性加密包并冻结本组织的写请求，等待新设备下载/校验/提交。
fn handle_handover_prepare(
    state: &EnclaveState,
    org_id: &str,
    user_id: &str,
    caller_device_id: &str,
    caller_device_name: &str,
) -> Result<Value, String> {
    let role = state
        .store
        .with_conn(|c| authz::role_for_member(c, org_id, user_id))?;
    if role != Some(Role::Owner) {
        return Err("仅组织所有者可发起设备接管".to_string());
    }
    if caller_device_id.trim().is_empty() {
        return Err("接管请求缺少设备标识".to_string());
    }
    if caller_device_id == device_id() {
        return Err("本机已是该组织主机".to_string());
    }

    let now = chrono::Utc::now().timestamp();
    {
        let mut g = state.host.handover.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = g.as_ref() {
            if now.saturating_sub(h.prepared_at) < HANDOVER_TTL_SECS {
                return Err("已有进行中的设备接管，请稍候".to_string());
            }
            let old = g.take();
            if let Some(old) = old {
                let _ = std::fs::remove_file(&old.path);
            }
        }
    }

    // 一次性口令仅用于本次包加密；通过已认证通道返回给接管方
    let passphrase = format!(
        "{}{}",
        crate::utils::new_id().replace('-', ""),
        crate::utils::new_id().replace('-', "")
    );
    let built = state
        .store
        .with_conn(|c| transfer::build_package(c, org_id, &passphrase))?;
    let id = crate::utils::new_id();
    let dir = handover_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建接管目录失败：{}", e))?;
    let path = dir.join(format!("{}.pdesk", id));
    std::fs::write(&path, &built.envelope).map_err(|e| format!("写入接管迁移包失败：{}", e))?;

    *state
        .host
        .handover
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(HandoverRuntime {
        id: id.clone(),
        org_id: org_id.to_string(),
        user_id: user_id.to_string(),
        device_id: caller_device_id.to_string(),
        device_name: caller_device_name.to_string(),
        passphrase: passphrase.clone(),
        package_hash: built.package_hash.clone(),
        path,
        prepared_at: now,
        verified: false,
    });

    Ok(json!({
        "handoverId": id,
        "orgId": org_id,
        "packageHash": built.package_hash,
        "size": built.envelope.len() as u64,
        "fileCount": built.file_count,
        "passphrase": passphrase,
    }))
}

fn handover_match(
    state: &EnclaveState,
    handover_id: &str,
    org_id: &str,
    user_id: &str,
    caller_device_id: &str,
) -> Result<HandoverRuntime, String> {
    let g = state.host.handover.lock().unwrap_or_else(|e| e.into_inner());
    let h = g
        .as_ref()
        .ok_or_else(|| "没有进行中的设备接管".to_string())?;
    if h.id != handover_id || h.org_id != org_id {
        return Err("接管会话不匹配".to_string());
    }
    if h.user_id != user_id || h.device_id != caller_device_id {
        return Err("接管会话不属于当前设备".to_string());
    }
    Ok(HandoverRuntime {
        id: h.id.clone(),
        org_id: h.org_id.clone(),
        user_id: h.user_id.clone(),
        device_id: h.device_id.clone(),
        device_name: h.device_name.clone(),
        passphrase: h.passphrase.clone(),
        package_hash: h.package_hash.clone(),
        path: h.path.clone(),
        prepared_at: h.prepared_at,
        verified: h.verified,
    })
}

fn handle_handover_chunk(
    state: &EnclaveState,
    handover_id: &str,
    org_id: &str,
    user_id: &str,
    caller_device_id: &str,
    offset: u64,
    len: u64,
) -> Result<Value, String> {
    let h = handover_match(state, handover_id, org_id, user_id, caller_device_id)?;
    let total = std::fs::metadata(&h.path)
        .map_err(|e| format!("接管迁移包不可用：{}", e))?
        .len();
    let offset = offset.min(total);
    let want = len.min(transfer::LAN_CHUNK_MAX).min(total - offset);
    let mut buf = vec![0u8; want as usize];
    if want > 0 {
        let mut f = std::fs::File::open(&h.path).map_err(|e| format!("读取接管迁移包失败：{}", e))?;
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("定位接管迁移包失败：{}", e))?;
        f.read_exact(&mut buf)
            .map_err(|e| format!("读取接管迁移包失败：{}", e))?;
    }
    Ok(json!({
        "offset": offset,
        "total": total,
        "data": BASE64.encode(&buf),
        "eof": offset + want >= total,
    }))
}

fn handle_handover_verified(
    state: &EnclaveState,
    handover_id: &str,
    org_id: &str,
    user_id: &str,
    caller_device_id: &str,
) -> Result<Value, String> {
    let mut g = state.host.handover.lock().unwrap_or_else(|e| e.into_inner());
    let h = g
        .as_mut()
        .ok_or_else(|| "没有进行中的设备接管".to_string())?;
    if h.id != handover_id || h.org_id != org_id {
        return Err("接管会话不匹配".to_string());
    }
    if h.user_id != user_id || h.device_id != caller_device_id {
        return Err("接管会话不属于当前设备".to_string());
    }
    h.verified = true;
    Ok(json!({ "ok": true }))
}

fn handle_handover_committed(
    state: &EnclaveState,
    handover_id: &str,
    org_id: &str,
    user_id: &str,
    caller_device_id: &str,
) -> Result<Value, String> {
    let h = handover_match(state, handover_id, org_id, user_id, caller_device_id)?;
    // 立即把本地快照的主机登记改为接管方：本机的握手复核随即拒绝外部连接
    state.store.with_conn(|c| {
        let name = if h.device_name.trim().is_empty() {
            h.device_id.as_str()
        } else {
            h.device_name.as_str()
        };
        org::set_host_device(c, org_id, &h.device_id, name)
    })?;
    let cleared = state.host.clear_handover();
    if let Some(done) = cleared {
        let _ = std::fs::remove_file(&done.path);
    }
    state.store.with_conn(|c| {
        org::audit(
            c,
            org_id,
            user_id,
            "handover.committed",
            &json!({ "handoverId": handover_id, "toDeviceId": caller_device_id }).to_string(),
        )
    })?;
    Ok(json!({ "ok": true }))
}

fn handle_handover_abort(
    state: &EnclaveState,
    handover_id: &str,
    org_id: &str,
    user_id: &str,
    caller_device_id: &str,
) -> Result<Value, String> {
    let _ = handover_match(state, handover_id, org_id, user_id, caller_device_id)?;
    if let Some(done) = state.host.clear_handover() {
        let _ = std::fs::remove_file(&done.path);
    }
    Ok(json!({ "ok": true }))
}

/// 接管期间允许的只读命令（其余写操作一律拒绝，保证迁移包一致性）。
fn handover_allows(method: &str, params: &Value) -> bool {
    if method.starts_with("handover.") || method == "ping" {
        return true;
    }
    let command = params["command"].as_str().unwrap_or("");
    matches!(
        method,
        "task.list" | "file.stat" | "file.fetch"
    ) || (method == "enclave.call"
        && matches!(
            command,
            "task.get"
                | "task.deps_preview"
                | "task.progress_list"
                | "report.list"
                | "report.feedback_list"
                | "file.task_list"
                | "kb.list"
                | "kb.search"
                | "kb.entry_get"
                | "kb.file_list"
                | "policy.get"
        ))
}

/// 清理超时未提交的接管会话（旧主机继续权威）。
fn expire_handover(state: &EnclaveState) {
    let now = chrono::Utc::now().timestamp();
    let expired = {
        let g = state.host.handover.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref()
            .map(|h| now.saturating_sub(h.prepared_at) > HANDOVER_TTL_SECS)
            .unwrap_or(false)
    };
    if expired {
        if let Some(h) = state.host.clear_handover() {
            let _ = std::fs::remove_file(&h.path);
            log::info!("[enclave] 设备接管超时，已回滚（组织 {}）", h.org_id);
        }
    }
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

fn handle_transfer_reject(
    state: &EnclaveState,
    org_id: &str,
    transfer_id: &str,
    user_id: &str,
) -> Result<Value, String> {
    state.store.with_conn(|c| {
        let view = transfer::reject_for_member(c, transfer_id, org_id, user_id)?;
        serde_json::to_value(&view).map_err(|e| format!("序列化拒绝结果失败：{}", e))
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
    manual_addr(state, org_id)
        .or_else(|| state.discovered_host(org_id).map(|d| d.addr))
        // 广播被禁用 / 跨网段：用平台登记的地址兜底（只传地址，不传业务数据）
        .or_else(|| {
            state
                .store
                .with_conn(|c| Ok(org::host_endpoint_of(c, org_id)))
                .ok()
                .flatten()
        })
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
    /// 平台登记下发的主机地址（成员端；广播被禁时的兜底）
    pub registered_addr: Option<String>,
    /// 本机作为主机时的证书指纹（sha256 hex）
    pub host_fingerprint: Option<String>,
    /// 成员端已固定的主机证书指纹（sha256 hex）
    pub pinned_fingerprint: Option<String>,
    /// 本机设备标识
    pub device_id: String,
    /// 当前组织登记的主机设备 id（空 = 尚未登记）
    pub host_device_id: Option<String>,
    /// 当前组织登记的主机设备名
    pub host_device_name: Option<String>,
}

pub fn host_status(
    state: &EnclaveState,
    org_id: Option<&str>,
) -> Result<HostStatusView, String> {
    let my_device = device_id();
    let served = state.store.with_conn(|c| org::host_org_ids(c, &my_device))?;
    let identity = state.store.with_conn(org::local_identity)?;
    let (running, port, last_error) = state.host.view();
    let is_host_for_org = match org_id {
        Some(o) => served.iter().any(|s| s == o),
        None => !served.is_empty(),
    };
    let role = if is_host_for_org {
        "host"
    } else if identity.is_some() {
        "member"
    } else {
        "unknown"
    };
    let (host_device_id, host_device_name) = match org_id {
        Some(o) => state
            .store
            .with_conn(|c| org::host_device_of(c, o))
            .ok()
            .map(|(id, name)| (Some(id).filter(|s| !s.is_empty()), Some(name)))
            .unwrap_or((None, None)),
        None => (None, None),
    };
    let discovered = org_id.and_then(|o| state.discovered_host(o));
    let registered_addr = org_id.and_then(|o| {
        state
            .store
            .with_conn(|c| Ok(org::host_endpoint_of(c, o)))
            .ok()
            .flatten()
    });
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
        registered_addr,
        host_fingerprint,
        pinned_fingerprint,
        device_id: my_device,
        host_device_id,
        host_device_name,
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

/// 平台返回的主机登记。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub port: u16,
    /// 主机上报的局域网地址（空 = 未上报）
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRegistration {
    /// true = 本次登记生效；false = 已被其他设备登记（`host` 为当前主机）
    pub claimed: bool,
    pub host: Option<HostInfo>,
}

/// 向平台登记本机为该组织主机（仅 owner）。`takeover=true` 用于换机接管。
pub async fn register_with_platform(
    db: &crate::DbState,
    state: &EnclaveState,
    org_id: &str,
    takeover: bool,
) -> Result<HostRegistration, String> {
    let token = crate::commands::org_common::require_token(db).await?;
    let path = format!("/api/v1/organizations/{}/host", org_id);
    let body = json!({
        "deviceId": device_id(),
        "deviceName": host_name(),
        "port": configured_host_port(state),
        "address": local_lan_addr(),
        "takeover": takeover,
    });
    platform::put_json_with_status::<_, HostRegistration>(&path, &body, Some(&token))
        .await
        .map_err(|e| format!("登记主机失败：{}", e.message))
}

/// 把平台返回的主机信息写入本地快照（立即生效，无需等下一轮轮询）。
pub fn apply_host_info(state: &EnclaveState, org_id: &str, info: &HostInfo) -> Result<(), String> {
    state.store.with_conn(|c| {
        org::set_host_device(c, org_id, &info.device_id, &info.device_name)?;
        org::set_host_endpoint(c, org_id, &info.address, info.port)
    })
}

/// 本机在局域网中的出口地址（平台地址兜底发现用；只上报地址，不传业务数据）。
///
/// 用 UDP `connect` 让内核按路由表选本地地址——不实际发包，断网时也不会报错；
/// 多网卡/特殊网络可用 `PILOTDESK_ENCLAVE_HOST_ADDR` 显式覆盖，取不到时返回空串。
fn local_lan_addr() -> String {
    if let Ok(v) = std::env::var("PILOTDESK_ENCLAVE_HOST_ADDR") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_default()
}

fn host_org_ids(state: &EnclaveState) -> Vec<String> {
    state
        .store
        .with_conn(|c| org::host_org_ids(c, &device_id()))
        .unwrap_or_default()
}

/// 本机稳定设备标识：落应用目录（不随 `enclave.db` 迁移）。
///
/// 同一 owner 账号可能在多台设备登录；平台用设备 id 确定唯一主机。
static DEVICE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn device_id() -> String {
    // 测试环境固定设备 id：不读写真实应用目录，且与测试种子数据一致
    #[cfg(test)]
    {
        "test-device".to_string()
    }
    #[cfg(not(test))]
    {
        DEVICE_ID
            .get_or_init(|| {
                let path = crate::utils::paths::app_root_dir()
                    .join("enclave")
                    .join("device_id");
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    let trimmed = raw.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
                let id = format!("dev-{}", crate::utils::new_id().replace('-', ""));
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, &id);
                id
            })
            .clone()
    }
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

/// 这条请求是否可能改动业务数据；返回变更提示（读请求 / 未知方法返回 `None`）。
fn request_hint(req: &RequestMsg) -> Option<ChangeHint> {
    match req.method.as_str() {
        "enclave.call" => change_hint(
            req.params["command"].as_str().unwrap_or(""),
            &req.params.get("payload").cloned().unwrap_or_else(|| json!({})),
        ),
        // 上传收尾才真正挂到任务 / 汇报上
        "file.upload_finish" | "file.upload_keep" => Some(ChangeHint {
            topic: "task".to_string(),
            task_id: req.params["taskId"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            kb_id: None,
        }),
        _ => None,
    }
}

fn reply_ok(reply: &str) -> bool {
    serde_json::from_str::<Value>(reply)
        .ok()
        .and_then(|v| v.get("ok").and_then(|b| b.as_bool()))
        .unwrap_or(false)
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
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id, host_device_id, host_device_name)
                     VALUES ('org1', 'Org', '1', 'owner', '1', 'test-device', 'TestDevice')",
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

    /// 设备接管协议：同账号另一台设备 prepare → 冻结写 → chunk → verified → committed。
    #[tokio::test]
    async fn handover_over_ws() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-enclave-handover-ws-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(
            super::super::db::EnclaveStore::open(&dir.join("enclave.db")).unwrap(),
        );
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO local_identity (user_id, email) VALUES ('1', 'owner@example.com')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id, host_device_id, host_device_name)
                     VALUES ('org1', 'Org', '1', 'owner', '1', 'test-device', 'TestDevice')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO member_snapshot (org_id, user_id, role, display_name)
                     VALUES ('org1', '1', 'owner', 'Owner')",
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
                "type": "hello", "protocol": PROTOCOL, "orgId": "org1",
                "userId": "1", "deviceId": "new-device", "deviceName": "NewPC", "token": "",
            })
            .to_string(),
        ))
        .await
        .unwrap();
        assert_eq!(next_json(&mut rx).await.unwrap()["type"], "hello_ok");

        // prepare：生成接管包
        tx.send(Message::Text(
            json!({
                "type": "req", "id": "h1", "method": "handover.prepare",
                "params": { "orgId": "org1" },
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let prep = next_json(&mut rx).await.unwrap();
        assert_eq!(prep["ok"], true, "prepare failed: {}", prep);
        let handover_id = prep["data"]["handoverId"].as_str().unwrap().to_string();
        assert!(!handover_id.is_empty());
        assert!(prep["data"]["size"].as_u64().unwrap() > 0);

        // 迁移期间写请求被冻结
        tx.send(Message::Text(
            json!({
                "type": "req", "id": "w1", "method": "enclave.call",
                "params": { "orgId": "org1", "command": "task.create", "payload": {} },
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let frozen = next_json(&mut rx).await.unwrap();
        assert_eq!(frozen["ok"], false);
        assert!(frozen["error"].as_str().unwrap().contains("设备接管"));

        // 分块下载
        tx.send(Message::Text(
            json!({
                "type": "req", "id": "c1", "method": "handover.chunk",
                "params": { "orgId": "org1", "handoverId": handover_id, "offset": 0, "len": 1024 },
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let chunk = next_json(&mut rx).await.unwrap();
        assert_eq!(chunk["ok"], true);
        assert!(!chunk["data"]["data"].as_str().unwrap().is_empty());

        let verified = |id: &str| {
            json!({
                "type": "req", "id": id, "method": "handover.verified",
                "params": { "orgId": "org1", "handoverId": handover_id },
            })
            .to_string()
        };
        tx.send(Message::Text(verified("v1"))).await.unwrap();
        assert_eq!(next_json(&mut rx).await.unwrap()["ok"], true);

        // 提交：本地快照立即改登记，旧主机停服
        tx.send(Message::Text(
            json!({
                "type": "req", "id": "m1", "method": "handover.committed",
                "params": { "orgId": "org1", "handoverId": handover_id },
            })
            .to_string(),
        ))
        .await
        .unwrap();
        assert_eq!(next_json(&mut rx).await.unwrap()["ok"], true);

        let (device, name) = state
            .store
            .with_conn(|c| org::host_device_of(c, "org1"))
            .unwrap();
        assert_eq!(device, "new-device");
        assert_eq!(name, "NewPC");
        assert!(host_org_ids(&state).is_empty(), "旧主机不得继续对外服务");

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
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id, host_device_id, host_device_name)
                     VALUES ('org1', 'Org', '1', 'owner', '1', 'test-device', 'TestDevice')",
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
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id, host_device_id, host_device_name)
                     VALUES ('org1', 'Org', '1', 'owner', '1', 'test-device', 'TestDevice')",
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

    /// 成员写入后，同组织其他在线成员收到变更事件；发起连接自己不再收（避免重复刷新）。
    #[tokio::test]
    async fn change_event_reaches_other_members_only() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-enclave-realtime-test-{}",
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
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id, host_device_id, host_device_name)
                     VALUES ('org1', 'Org', '1', 'owner', '1', 'test-device', 'TestDevice')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                for (uid, name) in [("2", "甲"), ("3", "乙")] {
                    c.execute(
                        "INSERT INTO member_snapshot (org_id, user_id, role, display_name)
                         VALUES ('org1', ?1, 'member', ?2)",
                        params![uid, name],
                    )
                    .map_err(|e| e.to_string())?;
                }
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
                // 两条成员连接
                for _ in 0..2 {
                    if let Ok((stream, peer)) = listener.accept().await {
                        if let Ok(tls_stream) = acceptor.accept(stream).await {
                            let st = st.clone();
                            tokio::spawn(async move { handle_conn(tls_stream, peer, st).await });
                        }
                    }
                }
            }
        });

        async fn hello_as(port: u16, user: &str) -> (WsTx, WsRx) {
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
                    "orgId": "org1", "userId": user, "token": "",
                })
                .to_string(),
            ))
            .await
            .unwrap();
            assert_eq!(next_json(&mut rx).await.unwrap()["type"], "hello_ok");
            (tx, rx)
        }

        let (mut tx_a, mut rx_a) = hello_as(port, "2").await;
        let (mut tx_b, mut rx_b) = hello_as(port, "3").await;

        // 在线成员：两条握手后的连接都应出现在主机侧列表里（按 user id 去重）
        let mut online = state.host.online_member_ids("org1");
        online.sort();
        assert_eq!(online, vec!["2".to_string(), "3".to_string()]);

        // 甲建任务
        tx_a.send(Message::Text(
            json!({
                "type":"req","id":"w1","method":"enclave.call",
                "params":{
                    "orgId":"org1","command":"task.create",
                    "payload":{"input":{"title":"实时任务"}}
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let created = next_json(&mut rx_a).await.unwrap();
        assert_eq!(created["ok"], true, "create failed: {}", created);

        // 乙收到事件（只带主题，不带业务数据）
        let ev = tokio::time::timeout(Duration::from_secs(5), next_json(&mut rx_b))
            .await
            .expect("乙应在超时前收到事件")
            .unwrap();
        assert_eq!(ev["type"], "event");
        assert_eq!(ev["topic"], "task");
        assert_eq!(ev["orgId"], "org1");
        assert!(ev.get("data").is_none(), "事件不得携带业务数据");

        // 甲（发起方）不额外收事件：下一帧必须是它自己的响应
        tx_a.send(Message::Text(
            json!({
                "type":"req","id":"r1","method":"enclave.call",
                "params":{"orgId":"org1","command":"task.get","payload":{}}
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let next = tokio::time::timeout(Duration::from_secs(5), next_json(&mut rx_a))
            .await
            .expect("甲不应收到事件帧")
            .unwrap();
        assert_eq!(next["type"], "res");
        assert_eq!(next["id"], "r1");

        drop(tx_a);
        drop(rx_a);
        drop(tx_b);
        drop(rx_b);
        drop(state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 成员端取在线列表：调用方本人也必须在线。
    ///
    /// 复现路径与 `enclave_online_members` 的成员分支一致：不带平台 token 建一条临时连接，
    /// 主机应把这条连接也计入订阅列表 —— 否则成员设备上永远看不到自己的「在线」。
    #[tokio::test]
    async fn online_list_via_temp_connection_includes_caller() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-enclave-online-test-{}",
            crate::utils::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(
            super::super::db::EnclaveStore::open(&dir.join("host.db")).unwrap(),
        );
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO local_identity (user_id, email) VALUES ('1', 'host@example.com')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO org_snapshot (org_id, name, owner_user_id, my_role, viewer_id, host_device_id, host_device_name)
                     VALUES ('org1', 'Org', '1', 'owner', '1', 'test-device', 'TestDevice')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO member_snapshot (org_id, user_id, role, display_name)
                     VALUES ('org1', '2', 'member', '甲')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
        let host_state = std::sync::Arc::new(EnclaveState::new(store));

        let host_tls = tls::ensure_host_tls_in(&dir.join("tls")).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn({
            let st = host_state.clone();
            let acceptor = host_tls.acceptor.clone();
            async move {
                if let Ok((stream, peer)) = listener.accept().await {
                    if let Ok(tls_stream) = acceptor.accept(stream).await {
                        tokio::spawn(async move { handle_conn(tls_stream, peer, st).await });
                    }
                }
            }
        });

        // 成员端：只有本机身份 + 手动填写的主机地址（与真实成员设备的最小状态一致）
        let mstore = std::sync::Arc::new(
            super::super::db::EnclaveStore::open(&dir.join("member.db")).unwrap(),
        );
        mstore
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO local_identity (user_id, email) VALUES ('2', 'm@example.com')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                put_setting(
                    c,
                    &format!("{}org1", HOST_ADDR_PREFIX),
                    &format!("127.0.0.1:{}", port),
                )
            })
            .unwrap();
        let member_state = std::sync::Arc::new(EnclaveState::new(mstore));

        let ids = online_members_via_host(&member_state, "org1")
            .await
            .unwrap();
        assert!(
            ids.contains(&"2".to_string()),
            "调用方应出现在在线列表：{ids:?}"
        );

        drop(host_state);
        drop(member_state);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
