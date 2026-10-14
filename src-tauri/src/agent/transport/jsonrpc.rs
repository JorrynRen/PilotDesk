//! 通用 JSON-RPC 2.0 Peer（newline-delimited JSON，over 子进程 stdio）
//!
//! 由 `tools/mcp.rs` 的单向客户端抽取而来，补齐**双向**能力：
//! - 出站：`request`（带 id、等响应）与 `notify`（通知）
//! - 入站：对端发来的**请求**（有 id，可回写响应）与**通知**（无 id）分发到注册的 handler
//!
//! ACP（P2）需要双向——agent → client 的 `session/update`（通知）、
//! `session/request_permission` / `fs/*` / `terminal/*`（请求）；
//! MCP 只需出站。两者共用本实现，避免两套 JSON-RPC 读写。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{oneshot, Mutex};

/// 入站请求/通知的处理回调。
///
/// - `peer`：回写响应（`JsonRpcPeer::respond`）；需要异步处理时由 handler 自行 `tokio::spawn`
/// - `id`：`Some` = 请求（**必须**回写，否则对端会挂起）；`None` = 通知
pub type PeerHandler = Arc<dyn Fn(JsonRpcPeer, Option<Value>, Value) + Send + Sync>;

/// 默认请求超时（与 `tools/mcp.rs` 的 30s 口径一致）。
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 双向 JSON-RPC Peer（`Clone` 后共享同一连接）。
#[derive(Clone)]
pub struct JsonRpcPeer {
    inner: Arc<Inner>,
}

struct Inner {
    next_id: AtomicI64,
    stdin: Mutex<ChildStdin>,
    /// 出站请求 id → 等待响应的发送端
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>,
    /// method → 入站处理器
    handlers: StdMutex<HashMap<String, PeerHandler>>,
    child: Mutex<Child>,
}

impl JsonRpcPeer {
    /// 接管一个已 `spawn` 的子进程，并启动后台读循环。
    pub fn new(mut child: Child) -> Result<Self, String> {
        let stdin = child.stdin.take().ok_or("无法获取子进程 stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取子进程 stdout")?;

        let inner = Arc::new(Inner {
            next_id: AtomicI64::new(1),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            handlers: StdMutex::new(HashMap::new()),
            child: Mutex::new(child),
        });
        let peer = Self {
            inner: Arc::clone(&inner),
        };

        // 后台读循环：解析每一行并分发（响应 → pending；请求/通知 → handler）
        let reader = peer.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<Value>(&line) {
                            Ok(v) => reader.dispatch(v).await,
                            Err(e) => log::warn!("[jsonrpc] 丢弃不可解析的行: {}", e),
                        }
                    }
                    Ok(None) => {
                        // 读循环退出必须在日志里可见：ACP 的回合结束响应走同一条 stdout，
                        // 若循环先死，后续响应会被永久吞掉（表现为"任务已完成却一直等待"）。
                        log::warn!("[jsonrpc] 读循环退出：对端 stdout 已关闭");
                        break;
                    }
                    Err(e) => {
                        log::warn!("[jsonrpc] 读循环退出：读取失败 err={}", e);
                        break;
                    }
                }
            }
            // 读循环结束 = 对端已断开。必须把在途请求全部唤醒为错误：
            // 否则它们会一直干等到各自超时（ACP 回合表现为"对端已死却仍显示思考中"）。
            let mut pending = reader.inner.pending.lock().await;
            for (_id, tx) in pending.drain() {
                let _ = tx.send(Err("对端连接已断开（进程退出或 stdout 关闭）".to_string()));
            }
        });

        Ok(peer)
    }

    /// 注册某 method 的入站处理器（请求与通知共用，按 `id` 是否为 `None` 区分）。
    pub fn on(&self, method: &str, handler: PeerHandler) {
        if let Ok(mut m) = self.inner.handlers.lock() {
            m.insert(method.to_string(), handler);
        }
    }

    async fn write(&self, v: &Value) -> Result<(), String> {
        let mut guard = self.inner.stdin.lock().await;
        guard
            .write_all(v.to_string().as_bytes())
            .await
            .map_err(|e| format!("写入失败: {}", e))?;
        guard
            .write_all(b"\n")
            .await
            .map_err(|e| format!("写入失败: {}", e))?;
        guard.flush().await.map_err(|e| format!("写入失败: {}", e))
    }

    /// 发送请求并等待匹配 id 的响应（默认 30s 超时）。
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_with_timeout(method, params, DEFAULT_REQUEST_TIMEOUT)
            .await
    }

    /// 发送请求并等待响应（自定义超时）。
    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(e) = self.write(&req).await {
            self.inner.pending.lock().await.remove(&id);
            return Err(e);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(format!("请求 {} 的响应通道已关闭", method)),
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                let msg = format!("JSON-RPC 请求超时（{}s）：{}", timeout.as_secs(), method);
                // 诊断打点：回合级请求被超时判死时必须在日志里可见（否则表现为"对端无响应"）
                log::warn!("[jsonrpc] {}", msg);
                Err(msg)
            }
        }
    }

    /// 发送通知（无 id、不等响应）。
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    /// 回写某条入站请求的响应。
    pub async fn respond(&self, id: Value, result: Result<Value, String>) {
        let msg = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(m) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": m}})
            }
        };
        let _ = self.write(&msg).await;
    }

    /// 入站报文分发。
    async fn dispatch(&self, v: Value) {
        let method = v
            .get("method")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string());
        let id = v.get("id").cloned();

        match (method, id) {
            // 响应：有 id、无 method
            (None, Some(id)) => {
                // 兼容对端把 id 回成字符串的实现（ACP 真机联调中曾见）
                let key = id
                    .as_i64()
                    .or_else(|| id.as_str().and_then(|s| s.parse::<i64>().ok()));
                match key {
                    Some(k) => {
                        if let Some(tx) = self.inner.pending.lock().await.remove(&k) {
                            let res = if let Some(err) = v.get("error") {
                                Err(format!("对端错误：{}", err))
                            } else {
                                Ok(v.get("result").cloned().unwrap_or(Value::Null))
                            };
                            let _ = tx.send(res);
                        } else {
                            // 无主响应：要么是超时/取消后迟到的响应，要么是对端 id 不匹配。
                            // 必须打日志——否则"回合结束响应被丢弃"完全看不见。
                            log::warn!("[jsonrpc] 收到无主响应（无对应在途请求，已丢弃）：id={}", k);
                        }
                    }
                    None => log::warn!(
                        "[jsonrpc] 响应 id 既非整数也非数字字符串，无法匹配（已丢弃）：id={}",
                        id
                    ),
                }
            }
            // 入站请求 / 通知
            (Some(method), id) => {
                let handler = self
                    .inner
                    .handlers
                    .lock()
                    .ok()
                    .and_then(|m| m.get(&method).cloned());
                match handler {
                    Some(h) => {
                        let params = v.get("params").cloned().unwrap_or(Value::Null);
                        h(self.clone(), id, params);
                    }
                    // 未注册的请求：按约定回错，避免对端无限等待（通知则静默丢弃）
                    None => {
                        if let Some(id) = id {
                            self.respond(id, Err(format!("未实现的请求：{}", method)))
                                .await;
                        }
                    }
                }
            }
            (None, None) => {}
        }
    }

    /// 终止子进程。
    pub async fn kill(&self) {
        let mut child = self.inner.child.lock().await;
        let _ = child.kill().await;
    }
}
