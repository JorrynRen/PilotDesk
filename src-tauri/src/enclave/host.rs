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

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_tungstenite::tungstenite::protocol::Message;

use super::authz::{self, Role};
use super::org;
use super::{tasks, EnclaveState, ENCLAVE_DEFAULT_PORT};
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

    let mut listener: Option<TcpListener> = None;
    loop {
        if listener.is_none() {
            if !host_org_ids(&state).is_empty() {
                let port = configured_host_port(&state);
                match TcpListener::bind(("0.0.0.0", port)).await {
                    Ok(l) => {
                        state.host.mark_running(port);
                        log::info!("[enclave] 主机服务已监听 0.0.0.0:{}", port);
                        listener = Some(l);
                    }
                    Err(e) => {
                        state.host.mark_stopped(Some(format!("监听端口 {} 失败：{}", port, e)));
                        log::warn!("[enclave] 主机服务监听失败（{}）：{}", port, e);
                    }
                }
            }
        }

        match listener.as_ref() {
            Some(l) => {
                let mut accept_failed = false;
                match tokio::time::timeout(Duration::from_secs(5), l.accept()).await {
                    Ok(Ok((stream, peer))) => {
                        let st = state.clone();
                        tokio::spawn(async move { handle_conn(stream, peer, st).await });
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
    let token = crate::commands::org_common::require_token(db)
        .await
        .unwrap_or_default();

    let url = format!("ws://{}", addr);
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url.as_str()))
        .await
        .map_err(|_| format!("连接主机 {} 超时", addr))?
        .map_err(|e| format!("连接主机 {} 失败：{}", addr, e))?;
    let (mut tx, mut rx) = ws.split();

    let hello = json!({
        "type": "hello",
        "protocol": PROTOCOL,
        "orgId": org_id,
        "userId": user_id,
        "email": email,
        "token": token,
    });
    tx.send(Message::Text(hello.to_string()))
        .await
        .map_err(|e| format!("发送握手失败：{}", e))?;
    let hello_reply = tokio::time::timeout(HELLO_TIMEOUT, next_json(&mut rx))
        .await
        .map_err(|_| "等待主机握手超时".to_string())??;
    match hello_reply["type"].as_str() {
        Some("hello_ok") => {}
        Some("hello_err") => {
            return Err(hello_reply["message"]
                .as_str()
                .unwrap_or("主机拒绝了连接")
                .to_string())
        }
        _ => return Err("主机返回了无法识别的握手响应".to_string()),
    }

    let id = crate::utils::new_id();
    let req = json!({
        "type": "req",
        "id": id,
        "method": "task.list",
        "params": { "orgId": org_id },
    });
    tx.send(Message::Text(req.to_string()))
        .await
        .map_err(|e| format!("发送请求失败：{}", e))?;
    let reply = tokio::time::timeout(REQUEST_TIMEOUT, next_json(&mut rx))
        .await
        .map_err(|_| "等待主机响应超时".to_string())??;

    let _ = tx.send(Message::Close(None)).await;
    if reply["ok"].as_bool() != Some(true) {
        return Err(reply["error"]
            .as_str()
            .unwrap_or("主机返回了错误")
            .to_string());
    }
    Ok(reply["data"].clone())
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

async fn handle_conn(stream: TcpStream, peer: SocketAddr, state: std::sync::Arc<EnclaveState>) {
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
            Ok(())
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
    state.store.with_conn(|c| put_setting(c, &key, raw))
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

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn({
            let st = state.clone();
            async move {
                if let Ok((stream, peer)) = listener.accept().await {
                    handle_conn(stream, peer, st).await;
                }
            }
        });

        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}", port))
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
}
