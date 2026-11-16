//! 从 ACP 宿主 Agent **导入**既有会话（`session/list` + `session/load` 回放 + `session/delete`）。
//!
//! 与 `transport::acp` 的会话运行链路**互不复用状态**：这里的进程不注册进
//! `AgentManager` 进程表，函数签名不依赖 Tauri State，便于单测与命令层薄适配。
//!
//! 性能：ACP 是有状态长连协议，握手后的进程可直接服务后续请求。本模块按配置 key
//! 把**已握手**的宿主进程放入全局池复用（`HostPool`），消除"每次操作 = spawn +
//! 握手 + kill"（实测 200ms~1s，npx 适配器更慢）的延迟；空闲 10min 由后台任务回收。
//!
//! 能力判定**不按 agent 名称硬编码**：`initialize` 返回的 `agentCapabilities` 只作参考，
//! 是否支持 `session/list` 一律以实际调用结果为准（`-32601 method not found` = 不支持）。

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::io::AsyncReadExt;

use super::transport::acp::{method, peer_method, ACP_PROTOCOL_VERSION, HANDSHAKE_TIMEOUT};
use super::transport::jsonrpc::JsonRpcPeer;

/// `initialize` 握手等待上限（与任务口径一致：30s）。
///
/// 不用 `transport::acp::HANDSHAKE_TIMEOUT`（120s）：导入是用户主动触发的短操作，
/// 冷启动慢的 npx 适配器失败时由命令层提示去环境检测，不必让弹窗干等两分钟。
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);

/// `session/load` 的等待上限：整段历史要重放完才返回响应，不能沿用 30s。
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// stderr 尾部保留长度（失败提示只截尾部，避免把整段日志塞进 Toast）。
const STDERR_TAIL_CHARS: usize = 500;

/// 池内宿主进程的空闲回收时限：覆盖一次"浏览/挑选宿主会话"的操作窗口即可。
const HOST_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// 宿主会话列表里的一项（字段名与 ACP `session/list` 对齐）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSession {
    pub session_id: String,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

/// `session/list` 的结果：支持则给列表，不支持则给可直接展示的说明。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ListSessionsOutcome {
    Ok {
        sessions: Vec<HostSession>,
        /// 枚举 struct variant 的字段不受容器级 `rename_all` 约束（serde 需
        /// `rename_all_fields`，且版本敏感），故显式命名。
        #[serde(rename = "nextCursor")]
        next_cursor: Option<String>,
    },
    Unsupported {
        capability: String,
        hint: String,
    },
}

/// `session/load` 重放出来的一条消息（连续同角色分片已合并）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportedMessage {
    /// `"user"` | `"assistant"`
    pub role: String,
    pub content: String,
}

/// `session/delete` 的结果：支持则已删除，不支持则给可直接展示的说明。
///
/// 仍按「实际调用结果」判定（`-32601` = 不支持），不依赖 `initialize` 的能力位。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum DeleteSessionOutcome {
    Ok,
    Unsupported { capability: String, hint: String },
}

// ──────────────────────────────────────────────
//  一次性 ACP 子进程
// ──────────────────────────────────────────────

struct AcpHost {
    agent_id: String,
    peer: JsonRpcPeer,
    stderr: Arc<StdMutex<String>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

impl AcpHost {
    async fn spawn(
        agent_id: &str,
        command: &str,
        args: &[String],
        env: Option<&Map<String, Value>>,
        cwd: &str,
    ) -> Result<Self, String> {
        if command.trim().is_empty() {
            return Err(format!("Agent「{}」未配置 ACP 启动命令", agent_id));
        }
        let (program, prefix) = crate::utils::process::resolve_windows_program(command);
        let mut cmd = crate::utils::process::hidden_tokio_command(&program);
        cmd.args(&prefix)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // 兜底：即便后续提前返回，也不留下孤儿适配器进程
            .kill_on_drop(true);
        if let Some(env) = env {
            for (k, v) in env {
                if let Some(s) = v.as_str() {
                    cmd.env(k, s);
                }
            }
        }
        log::info!(
            "[host_sessions] 启动 ACP 进程: agent={} cmd={} args={:?} cwd={}",
            agent_id,
            program,
            args,
            cwd
        );
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("启动 ACP 进程失败: {}", e))?;

        // stderr 必须捕获：对端把真正的失败原因（未安装依赖、未登录等）写在这里
        let stderr = Arc::new(StdMutex::new(String::new()));
        let stderr_task = match child.stderr.take() {
            Some(mut err) => {
                let buf = Arc::clone(&stderr);
                Some(tokio::spawn(async move {
                    let mut s = String::new();
                    let _ = err.read_to_string(&mut s).await;
                    if let Ok(mut b) = buf.lock() {
                        b.push_str(&s);
                    }
                }))
            }
            None => None,
        };

        let peer = JsonRpcPeer::new(child)?;
        Ok(Self {
            agent_id: agent_id.to_string(),
            peer,
            stderr,
            stderr_task,
        })
    }

    /// 关进程并等 stderr 收尾（尽力而为，最多等 2s）。
    async fn shutdown(&mut self) {
        self.peer.kill().await;
        if let Some(task) = self.stderr_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
        log::info!("[host_sessions] ACP 进程已关闭: agent={}", self.agent_id);
    }

    fn stderr_tail(&self) -> String {
        let text = self
            .stderr
            .lock()
            .map(|b| b.trim().to_string())
            .unwrap_or_default();
        if text.is_empty() {
            return String::new();
        }
        let chars: Vec<char> = text.chars().collect();
        let start = chars.len().saturating_sub(STDERR_TAIL_CHARS);
        chars[start..].iter().collect()
    }

    fn with_stderr(&self, err: String) -> String {
        let detail = self.stderr_tail();
        if detail.is_empty() {
            err
        } else {
            format!("{}（对端 stderr: {}）", err, detail)
        }
    }

    async fn initialize(&self) -> Result<Value, String> {
        self.peer
            .request_with_timeout(
                method::INITIALIZE,
                json!({
                    "protocolVersion": ACP_PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": { "readTextFile": false, "writeTextFile": false }
                    }
                }),
                INITIALIZE_TIMEOUT,
            )
            .await
    }

    /// spawn + initialize 一步完成——池化复用的前提：进程取回时已是握手后状态。
    async fn spawn_initialized(
        agent_id: &str,
        command: &str,
        args: &[String],
        env: Option<&Map<String, Value>>,
        cwd: &str,
    ) -> Result<Self, String> {
        let host = Self::spawn(agent_id, command, args, env, cwd).await?;
        let init = host.initialize().await?;
        // 只作日志参考；各方法是否支持一律以实际调用结果为准
        log::debug!(
            "[host_sessions] agent={} 声明的能力: {}",
            agent_id,
            init["agentCapabilities"]
        );
        Ok(host)
    }
}

// ──────────────────────────────────────────────
//  宿主进程池（长连复用）
// ──────────────────────────────────────────────

/// 池内一个**已握手**的宿主进程。
struct IdleHost {
    host: AcpHost,
    last_used: Instant,
}

/// 按配置 key 复用已握手的 ACP 宿主进程。
///
/// ACP 是有状态长连协议：握手后的进程可直接服务后续的 `session/list` 等请求，
/// 不必每次操作重新 spawn（实测 spawn + 握手 200ms~1s，npx 适配器更慢）。
/// 空闲超过 [`HOST_IDLE_TIMEOUT`] 由后台任务回收（进程随 drop 被 `kill_on_drop` 兜底）。
struct HostPool {
    pools: StdMutex<HashMap<String, Vec<IdleHost>>>,
    /// 清理任务是否已启动（保证全局只起一个）
    cleanup_started: AtomicBool,
}

static HOST_POOL: OnceLock<Arc<HostPool>> = OnceLock::new();

impl HostPool {
    fn global() -> Arc<HostPool> {
        HOST_POOL
            .get_or_init(|| {
                Arc::new(HostPool {
                    pools: StdMutex::new(HashMap::new()),
                    cleanup_started: AtomicBool::new(false),
                })
            })
            .clone()
    }

    /// 惰性启动后台清理任务（每分钟扫一次，回收空闲超时的宿主进程）。
    ///
    /// `global()` 可能在**同步上下文**被首次调用（如 Agent 卸载时 `evict_agent`），
    /// 那里没有 tokio runtime，`tokio::spawn` 会 panic——故推迟到首个 async
    /// 调用点（take，必有 runtime）再启动。任务持有池的强引用、随进程常驻，
    /// 与 `HOST_POOL` 静态同生命周期。
    fn ensure_cleanup_task(&self) {
        if self.cleanup_started.swap(true, Ordering::SeqCst) {
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            // 同步上下文：复位标志，下次 async 调用再试
            self.cleanup_started.store(false, Ordering::SeqCst);
            return;
        }
        let pool = HostPool::global();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let mut m = pool.pools.lock().unwrap();
                for list in m.values_mut() {
                    list.retain(|h| h.last_used.elapsed() < HOST_IDLE_TIMEOUT);
                }
                m.retain(|_, list| !list.is_empty());
            }
        });
    }

    /// 取一个存活宿主进程：已退出的直接丢弃（drop 即 kill），继续试下一个。
    async fn take(&self, key: &str) -> Option<AcpHost> {
        self.ensure_cleanup_task();
        loop {
            let candidate = {
                let mut m = self.pools.lock().unwrap();
                m.get_mut(key)?.pop().map(|h| h.host)
            }?;
            if candidate.peer.is_alive().await {
                return Some(candidate);
            }
            log::info!(
                "[host_sessions] 丢弃已退出的池内进程: agent={}",
                candidate.agent_id
            );
        }
    }

    /// 操作成功后放回池（失败进程一律丢弃，不入池）。
    fn put(&self, key: String, host: AcpHost) {
        let mut m = self.pools.lock().unwrap();
        m.entry(key).or_default().push(IdleHost {
            host,
            last_used: Instant::now(),
        });
    }
}

/// 硬清某 Agent 的所有池内宿主进程（Agent 卸载/删除/配置变更时调用）。
///
/// 池化 key 以 `agent_id` 打头（见 `pool_key`），按前缀匹配移除；进程随 drop 被
/// `kill_on_drop` 兜底。可在同步上下文调用（不触碰 tokio runtime）。
pub fn evict_agent(agent_id: &str) {
    let pool = HostPool::global();
    let prefix = format!("{}\u{1e}", agent_id);
    let mut m = pool.pools.lock().unwrap();
    let keys: Vec<String> = m
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .cloned()
        .collect();
    if keys.is_empty() {
        return;
    }
    let mut n = 0usize;
    for k in &keys {
        if let Some(list) = m.remove(k) {
            n += list.len();
        }
    }
    log::info!(
        "[host_sessions] evict agent={} 共 {} 个池内进程",
        agent_id,
        n
    );
}

/// 池化 key：配置（命令/参数/环境/目录）变了就不能复用旧进程。
fn pool_key(
    agent_id: &str,
    command: &str,
    args: &[String],
    env: Option<&Map<String, Value>>,
    cwd: &str,
) -> String {
    let mut env_pairs: Vec<String> = env
        .map(|e| e.iter().map(|(k, v)| format!("{}={}", k, v)).collect())
        .unwrap_or_default();
    env_pairs.sort();
    [
        agent_id,
        command,
        &args.join("\u{1f}"),
        &env_pairs.join("\u{1f}"),
        cwd,
    ]
    .join("\u{1e}")
}

// ──────────────────────────────────────────────
//  池化执行（取池 → 池内进程失败重试一次 → 回收/丢弃）
// ──────────────────────────────────────────────

type ChunksSink = Arc<StdMutex<Vec<(String, String)>>>;

/// 一次宿主操作（池化执行器按类型分发；具体实现见下方 `*_inner`）。
enum Op<'a> {
    List {
        filter_cwd: Option<&'a str>,
        cursor: Option<&'a str>,
    },
    Load {
        cwd: &'a str,
        session_id: &'a str,
        chunks: ChunksSink,
    },
    Delete {
        session_id: &'a str,
    },
}

enum OpOutcome {
    List(ListSessionsOutcome),
    Load(Vec<ImportedMessage>),
    Delete(DeleteSessionOutcome),
}

/// 在指定宿主进程上执行一次操作（进程必须已完成 initialize）。
async fn run_op(host: &AcpHost, op: &Op<'_>) -> Result<OpOutcome, String> {
    match op {
        Op::List { filter_cwd, cursor } => list_sessions_inner(host, *filter_cwd, *cursor)
            .await
            .map(OpOutcome::List),
        Op::Load {
            cwd,
            session_id,
            chunks,
        } => {
            // 先清空（重试时上一次 attempt 的半截回放不能混入），再注册收集器——
            // `session/load` 的响应在所有重放通知之后才到，注册必须先于请求。
            // `peer.on` 按方法名覆盖注册，池化复用不会累积旧 sink。
            chunks.lock().unwrap().clear();
            let sink = Arc::clone(chunks);
            host.peer.on(
                peer_method::SESSION_UPDATE,
                Arc::new(move |_peer, _id, params| {
                    let update = &params["update"];
                    let kind = update["sessionUpdate"].as_str().unwrap_or("");
                    if kind != "user_message_chunk" && kind != "agent_message_chunk" {
                        return;
                    }
                    let text = update["content"]["text"].as_str().unwrap_or("");
                    if text.is_empty() {
                        return;
                    }
                    if let Ok(mut v) = sink.lock() {
                        v.push((kind.to_string(), text.to_string()));
                    }
                }),
            );
            load_messages_inner(host, cwd, session_id).await?;
            let collected = chunks
                .lock()
                .map(|mut v| std::mem::take(&mut *v))
                .unwrap_or_default();
            Ok(OpOutcome::Load(merge_chunks(collected)))
        }
        Op::Delete { session_id } => delete_session_inner(host, session_id)
            .await
            .map(OpOutcome::Delete),
    }
}

/// 池化执行：优先复用池内已握手进程；池内进程操作失败（多半是池化期间对端悄悄
/// 退出而探活未能覆盖）则丢弃、换新进程**重试一次**；新进程再失败才透出错误
/// （附 stderr 尾部）。操作成功的进程放回池，失败的一律 `shutdown` 丢弃。
async fn run_host_op(
    agent_id: &str,
    command: &str,
    args: &[String],
    env: Option<&Map<String, Value>>,
    cwd: &str,
    op: Op<'_>,
) -> Result<OpOutcome, String> {
    let pool = HostPool::global();
    let key = pool_key(agent_id, command, args, env, cwd);
    let pooled = pool.take(&key).await;
    let was_pooled = pooled.is_some();
    let mut host = match pooled {
        Some(h) => h,
        None => AcpHost::spawn_initialized(agent_id, command, args, env, cwd).await?,
    };
    match run_op(&host, &op).await {
        Ok(v) => {
            pool.put(key, host);
            Ok(v)
        }
        Err(e) if was_pooled => {
            log::warn!(
                "[host_sessions] 池内进程请求失败，丢弃并换新进程重试: agent={} err={}",
                agent_id,
                e
            );
            host.shutdown().await;
            let mut fresh = AcpHost::spawn_initialized(agent_id, command, args, env, cwd).await?;
            match run_op(&fresh, &op).await {
                Ok(v) => {
                    pool.put(key, fresh);
                    Ok(v)
                }
                Err(e) => {
                    fresh.shutdown().await;
                    Err(fresh.with_stderr(e))
                }
            }
        }
        Err(e) => {
            host.shutdown().await;
            Err(host.with_stderr(e))
        }
    }
}

// ──────────────────────────────────────────────
//  能力判定与解析
// ──────────────────────────────────────────────

/// JSON-RPC `-32601 method not found`。
///
/// `jsonrpc` 层把错误对象整体拼进字符串（`对端错误：{"code":-32601,...}`），
/// 故这里按错误码与消息文本双重匹配，兼容各实现对 message 大小写/措辞的差异。
fn is_method_not_found(err: &str) -> bool {
    err.contains("-32601") || err.to_ascii_lowercase().contains("method not found")
}

fn opt_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_host_sessions(resp: &Value) -> Vec<HostSession> {
    resp["sessions"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let session_id = s["sessionId"].as_str()?.to_string();
                    Some(HostSession {
                        session_id,
                        cwd: opt_string(&s["cwd"]),
                        title: opt_string(&s["title"]),
                        updated_at: opt_string(&s["updatedAt"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 把 `session/update` 分片按顺序合并：连续同角色的分片拼成一条消息。
///
/// 只认 `user_message_chunk` / `agent_message_chunk`，其余 `sessionUpdate` 类型忽略。
pub(crate) fn merge_chunks(chunks: Vec<(String, String)>) -> Vec<ImportedMessage> {
    let mut out: Vec<ImportedMessage> = Vec::new();
    for (kind, text) in chunks {
        let role = match kind.as_str() {
            "user_message_chunk" => "user",
            "agent_message_chunk" => "assistant",
            _ => continue,
        };
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == role => last.content.push_str(&text),
            _ => out.push(ImportedMessage {
                role: role.to_string(),
                content: text,
            }),
        }
    }
    out
}

// ──────────────────────────────────────────────
//  对外两个入口
// ──────────────────────────────────────────────

/// 列出宿主 Agent 的既有会话。
///
/// 无论 `initialize` 是否声明 `sessionCapabilities.list`，都实际调用一次 `session/list`：
/// 声明可能过时（协议版本差异），实际调用才能得到可依赖的结论。
pub async fn list_sessions(
    agent_id: &str,
    command: &str,
    args: &[String],
    env: Option<&Map<String, Value>>,
    cwd: &str,
    filter_cwd: Option<&str>,
    cursor: Option<&str>,
) -> Result<ListSessionsOutcome, String> {
    match run_host_op(
        agent_id,
        command,
        args,
        env,
        cwd,
        Op::List { filter_cwd, cursor },
    )
    .await?
    {
        OpOutcome::List(o) => Ok(o),
        _ => unreachable!("Op::List 只会产出 OpOutcome::List"),
    }
}

async fn list_sessions_inner(
    host: &AcpHost,
    filter_cwd: Option<&str>,
    cursor: Option<&str>,
) -> Result<ListSessionsOutcome, String> {
    let mut params = json!({});
    if let Some(dir) = filter_cwd.map(str::trim).filter(|s| !s.is_empty()) {
        params["cwd"] = json!(dir);
    }
    if let Some(c) = cursor.map(str::trim).filter(|s| !s.is_empty()) {
        params["cursor"] = json!(c);
    }
    match host
        .peer
        .request_with_timeout(method::SESSION_LIST, params, HANDSHAKE_TIMEOUT)
        .await
    {
        Ok(resp) => Ok(ListSessionsOutcome::Ok {
            sessions: parse_host_sessions(&resp),
            next_cursor: opt_string(&resp["nextCursor"]),
        }),
        Err(e) if is_method_not_found(&e) => Ok(ListSessionsOutcome::Unsupported {
            capability: "list".to_string(),
            hint: "该适配器未提供会话列表能力".to_string(),
        }),
        Err(e) => Err(e),
    }
}

/// 载入宿主会话的整段历史（`session/load` 以 `session/update` 通知重放）。
pub async fn load_messages(
    agent_id: &str,
    command: &str,
    args: &[String],
    env: Option<&Map<String, Value>>,
    cwd: &str,
    host_session_id: &str,
) -> Result<Vec<ImportedMessage>, String> {
    let chunks: ChunksSink = Arc::new(StdMutex::new(Vec::new()));
    match run_host_op(
        agent_id,
        command,
        args,
        env,
        cwd,
        Op::Load {
            cwd,
            session_id: host_session_id,
            chunks,
        },
    )
    .await?
    {
        OpOutcome::Load(msgs) => Ok(msgs),
        _ => unreachable!("Op::Load 只会产出 OpOutcome::Load"),
    }
}

async fn load_messages_inner(
    host: &AcpHost,
    cwd: &str,
    host_session_id: &str,
) -> Result<(), String> {
    host.peer
        .request_with_timeout(
            method::SESSION_LOAD,
            json!({
                "sessionId": host_session_id,
                "cwd": cwd,
                "mcpServers": [],
            }),
            LOAD_TIMEOUT,
        )
        .await
        .map(|_| ())
}

/// 删除宿主侧的一条会话（`session/delete`）。
///
/// 与列表同口径：先握手，再实际调用；`-32601` 即视为不支持（不按 agent 名硬编码）。
pub async fn delete_session(
    agent_id: &str,
    command: &str,
    args: &[String],
    env: Option<&Map<String, Value>>,
    cwd: &str,
    host_session_id: &str,
) -> Result<DeleteSessionOutcome, String> {
    match run_host_op(
        agent_id,
        command,
        args,
        env,
        cwd,
        Op::Delete {
            session_id: host_session_id,
        },
    )
    .await?
    {
        OpOutcome::Delete(o) => Ok(o),
        _ => unreachable!("Op::Delete 只会产出 OpOutcome::Delete"),
    }
}

async fn delete_session_inner(
    host: &AcpHost,
    host_session_id: &str,
) -> Result<DeleteSessionOutcome, String> {
    match host
        .peer
        .request_with_timeout(
            method::SESSION_DELETE,
            json!({ "sessionId": host_session_id }),
            HANDSHAKE_TIMEOUT,
        )
        .await
    {
        Ok(_) => Ok(DeleteSessionOutcome::Ok),
        Err(e) if is_method_not_found(&e) => Ok(DeleteSessionOutcome::Unsupported {
            capability: "delete".to_string(),
            hint: "该适配器没有提供删除会话的能力，请到宿主应用内处理".to_string(),
        }),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn merge_chunks_joins_consecutive_same_role() {
        let merged = merge_chunks(chunks(&[
            ("user_message_chunk", "你"),
            ("user_message_chunk", "好"),
            ("agent_message_chunk", "你"),
            ("agent_message_chunk", "好"),
            ("agent_message_chunk", "呀"),
        ]));
        assert_eq!(
            merged,
            vec![
                ImportedMessage {
                    role: "user".into(),
                    content: "你好".into()
                },
                ImportedMessage {
                    role: "assistant".into(),
                    content: "你好呀".into()
                },
            ]
        );
    }

    #[test]
    fn merge_chunks_ignores_other_update_types() {
        let merged = merge_chunks(chunks(&[
            ("user_message_chunk", "问"),
            ("agent_thought_chunk", "思考中"),
            ("tool_call", "ls"),
            ("agent_message_chunk", "答"),
            ("user_message_chunk", "再问"),
            ("agent_message_chunk", "再答"),
        ]));
        assert_eq!(
            merged,
            vec![
                ImportedMessage {
                    role: "user".into(),
                    content: "问".into()
                },
                ImportedMessage {
                    role: "assistant".into(),
                    content: "答".into()
                },
                ImportedMessage {
                    role: "user".into(),
                    content: "再问".into()
                },
                ImportedMessage {
                    role: "assistant".into(),
                    content: "再答".into()
                },
            ]
        );
    }

    #[test]
    fn merge_chunks_skips_empty_text() {
        let merged = merge_chunks(chunks(&[
            ("user_message_chunk", ""),
            ("user_message_chunk", "有内容"),
            ("agent_message_chunk", ""),
        ]));
        assert_eq!(
            merged,
            vec![ImportedMessage {
                role: "user".into(),
                content: "有内容".into()
            }]
        );
    }

    #[test]
    fn outcome_serializes_with_camel_case_status_and_cursor() {
        let ok = ListSessionsOutcome::Ok {
            sessions: vec![],
            next_cursor: Some("c1".into()),
        };
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["nextCursor"], "c1");

        let unsupported = ListSessionsOutcome::Unsupported {
            capability: "list".into(),
            hint: "不支持".into(),
        };
        let v = serde_json::to_value(unsupported).unwrap();
        assert_eq!(v["status"], "unsupported");
        assert_eq!(v["hint"], "不支持");
    }

    #[test]
    fn method_not_found_is_recognized() {
        assert!(is_method_not_found(
            r#"对端错误：{"code":-32601,"message":"Method not found"}"#
        ));
        assert!(is_method_not_found("Method Not Found"));
        assert!(!is_method_not_found(
            r#"对端错误：{"code":-32602,"message":"Invalid params"}"#
        ));
    }

    #[test]
    fn parse_host_sessions_reads_snake_and_camel_fields() {
        let resp = json!({
            "sessions": [
                {"sessionId": "s1", "cwd": "C:\\work", "title": "标题", "updatedAt": "2026-10-26T10:00:00Z"},
                {"sessionId": "s2"},
                {"title": "没有 id 的条目会被忽略"}
            ],
            "nextCursor": "c2"
        });
        let list = parse_host_sessions(&resp);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].session_id, "s1");
        assert_eq!(list[0].cwd.as_deref(), Some("C:\\work"));
        assert_eq!(list[1].title, None);
        assert_eq!(opt_string(&resp["nextCursor"]).as_deref(), Some("c2"));
    }
}
