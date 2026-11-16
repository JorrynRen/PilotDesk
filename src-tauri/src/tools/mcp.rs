//! MCP (Model Context Protocol) 客户端
//!
//! 支持两种传输：
//! - **stdio**：newline-delimited JSON-RPC 2.0，每个服务器通过子进程启动；
//! - **Streamable HTTP**：POST + 可选 SSE 响应流，携带 `Mcp-Session-Id` 会话头，
//!   关闭时按规范发送 DELETE 结束会话（Last-Event-ID 断线续传未实现，见 HttpTransport 注释）。
//!
//! 握手后按服务端声明的 capabilities 暴露能力：tools 直接注册为 Agent 工具；
//! resources / prompts 以合成工具（`mcp_<服务器>__list_resources` 等）形式暴露。
//! 其后按 MCP 规范补齐：协议版本协商与握手校验、列表分页、`isError` 透传、
//! Windows 命令垫片解析、env/cwd/超时配置、工具注解风险分级、
//! `tools/list_changed` 通知感知与工具失效安全重连。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::tools::{LocalPathArgument, RiskLevel, ToolHandler, ToolTag};
use crate::utils::process::{hidden_command, hidden_tokio_command, resolve_windows_program};

/// 客户端支持的 **Legacy** 协议版本（按新→旧排列，服务端降级时按此协商）。
/// Legacy 走 initialize/initialized 握手；modern 走 server/discover + 逐请求 `_meta`，
/// 两者由 McpClient::connect 自动探测 era 后分流。
const SUPPORTED_LEGACY_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// 客户端支持的 **Modern** 协议版本（当前最新 revision）。
/// Modern 无握手、无状态、每请求带 `_meta`；服务端以 `server/discover` 广告能力。
const SUPPORTED_MODERN_VERSIONS: &[&str] = &["2026-07-28"];

/// 逐请求 `_meta` 中携带的 clientId（modern 协议必填）
const MCP_CLIENT_ID: &str = "pilotdesk";

/// 单次 JSON-RPC 请求的默认超时（秒）
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// 列表分页上限（防御服务端返回异常 cursor 造成死循环）
const MAX_PAGES: usize = 100;

// ─────────────────────────────────────────────────────────────────────────────
// 配置
// ─────────────────────────────────────────────────────────────────────────────

/// MCP 服务器配置（持久化在 app_settings 的 `mcp_servers` key 下）。
/// 新增字段全部带 `#[serde(default)]`，与旧版纯 stdio 配置向后兼容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// 唯一名称，用于工具前缀
    pub name: String,
    /// 启动命令（如 `npx` / `node` / `uvx`）；HTTP 传输时缺省为空
    #[serde(default)]
    pub command: String,
    /// 命令参数（如 `["-y", "@modelcontextprotocol/server-filesystem", "/path"]`）
    #[serde(default)]
    pub args: Vec<String>,
    /// 追加的环境变量（与父进程环境合并，同名覆盖）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// 子进程工作目录。stdio 显式配置时必须为绝对路径；缺省使用当前会话工作根。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 单次请求超时（秒，缺省 30）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 传输类型："stdio" / "http"；缺省按 `url` 有无推断
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// HTTP 传输的服务地址（Streamable HTTP endpoint）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP 自定义请求头（如 Authorization）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

impl McpServerConfig {
    fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1))
    }

    /// 是否使用 HTTP 传输：显式 `transport` 优先，否则按 `url` 有无推断
    pub fn is_http(&self) -> bool {
        if let Some(t) = self
            .transport
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return t.eq_ignore_ascii_case("http");
        }
        self.url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .is_some()
    }

    pub(crate) fn normalized_cwd(&self) -> Result<Option<String>, String> {
        let Some(cwd) = self
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        let expanded = crate::utils::paths::resolve_tilde(cwd);
        if !std::path::Path::new(&expanded).is_absolute() {
            return Err(format!(
                "MCP 服务器 {} 的 cwd 必须是绝对路径: {}",
                self.name, cwd
            ));
        }
        let normalized = crate::utils::paths::normalize_absolute_path(&expanded, "");
        if !normalized.is_dir() {
            return Err(format!(
                "MCP 服务器 {} 的 cwd 不存在或不是目录: {}",
                self.name,
                normalized.display()
            ));
        }
        Ok(Some(normalized.to_string_lossy().to_string()))
    }

    /// 连接前校验：按传输类型检查必填字段，把配置错误挡在进程/网络启动之前
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("MCP 服务器名称不能为空".to_string());
        }
        if self.is_http() {
            let url = self.url.as_deref().map(str::trim).unwrap_or("");
            if url.is_empty() {
                return Err(format!(
                    "MCP 服务器 {} 使用 HTTP 传输但未配置 url",
                    self.name
                ));
            }
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!(
                    "MCP 服务器 {} 的 url 必须以 http:// 或 https:// 开头",
                    self.name
                ));
            }
        } else if self.command.trim().is_empty() {
            return Err(format!(
                "MCP 服务器 {} 使用 stdio 传输但未配置 command",
                self.name
            ));
        } else {
            self.normalized_cwd()?;
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 工具注解与能力描述
// ─────────────────────────────────────────────────────────────────────────────

/// 工具注解（`tools/list` 中的 `annotations`，缺省值按 MCP 规范）
#[derive(Debug, Clone)]
pub struct ToolAnnotations {
    /// 只读（规范缺省 false）
    pub read_only: bool,
    /// 破坏性（规范缺省 true）
    pub destructive: bool,
    /// 幂等（规范缺省 false）
    pub idempotent: bool,
}

impl Default for ToolAnnotations {
    fn default() -> Self {
        // 规范约定：未声明注解时按"可能有破坏性"处理，宁严勿松。
        Self {
            read_only: false,
            destructive: true,
            idempotent: false,
        }
    }
}

impl ToolAnnotations {
    fn from_value(v: Option<&Value>) -> Self {
        let get = |key: &str, default: bool| {
            v.and_then(|a| a.get(key))
                .and_then(Value::as_bool)
                .unwrap_or(default)
        };
        Self {
            read_only: get("readOnlyHint", false),
            destructive: get("destructiveHint", true),
            idempotent: get("idempotentHint", false),
        }
    }

    /// 失败后是否可安全重试：只读或幂等的工具重复调用无副作用。
    fn retryable(&self) -> bool {
        self.read_only || self.idempotent
    }
}

/// 服务端声明的能力（initialize 返回的 capabilities）
#[derive(Debug, Clone, Copy, Default)]
pub struct McpCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

/// MCP 工具描述信息（来自 tools/list）
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub schema: Value,
    pub annotations: ToolAnnotations,
}

/// MCP 资源描述信息（来自 resources/list）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceInfo {
    pub uri: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// MCP 提示词描述信息（来自 prompts/list）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    /// 参数列表（原样透传：name/description/required）
    pub arguments: Vec<Value>,
}

// ─────────────────────────────────────────────────────────────────────────────
// 传输抽象
// ─────────────────────────────────────────────────────────────────────────────

/// JSON-RPC 传输层：Legacy（initialize 握手）与 Modern（server/discover + 逐请求 `_meta`）
/// 两种 era 共用的统一接口。request/notify 内部自行处理超时与报文匹配；服务端推送的通知
/// 缓存在传输内，由 `take_notifications` 排空（客户端据此感知 tools/list_changed 等）。
#[async_trait::async_trait]
trait Transport: Send {
    /// 发送请求并等待匹配 `id` 的响应（跳过通知；服务端反向请求按未实现回绝）。
    /// 注意：modern transport 会自动在 JSON-RPC 对象里注入 `_meta`（protocolVersion + clientId），
    /// 调用方不需要自己加；legacy transport 不加。
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String>;
    /// 发送通知（无响应）。modern transport 同样自动注入 `_meta`。
    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String>;
    /// 排空缓存的服务端通知
    fn take_notifications(&mut self) -> Vec<Value> {
        Vec::new()
    }
    /// Legacy：initialize 成功后回调（HttpTransport 据此在后续请求携带 MCP-Protocol-Version 头）；
    /// Modern 不使用，留空。
    fn on_initialized(&mut self, _version: &str) {}
    /// Modern：server/discover 成功后回调，缓存协商后的协议版本（逐请求 `_meta` 用）；
    /// Legacy 不使用，留空。
    fn on_discovered(&mut self, _version: &str) {}
    /// 当前协商后的协议版本（Legacy 来自 initialize 响应；Modern 来自 server/discover 或初始值）。
    /// 目前版本经 on_initialized/on_discovered 回传给 McpClient，本访问器预留供后续查询用。
    #[allow(dead_code)]
    fn protocol_version(&self) -> &str {
        SUPPORTED_LEGACY_VERSIONS[0]
    }
    /// 客户端 id（modern 协议逐请求 `_meta` 必填；legacy 不需要，留空）。
    /// 同上：modern transport 内部自行注入 `_meta`，本访问器预留。
    #[allow(dead_code)]
    fn client_id(&self) -> &str {
        ""
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// stdio 传输
// ─────────────────────────────────────────────────────────────────────────────

/// stdio 传输：管理一个子进程，串行化 newline-delimited JSON-RPC 请求/响应
struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    timeout: Duration,
    /// 服务端推送的通知（tools/list_changed、progress 等），等待调用方排空
    notifications: Vec<Value>,
}

impl StdioTransport {
    async fn spawn(config: &McpServerConfig, timeout: Duration) -> Result<Self, String> {
        // Windows 的 CreateProcess 只自动补 .exe，不按 PATHEXT 查找，
        // 因此 npx / uvx / 各类 npm 全局 CLI 必须解析到真实可执行目标，
        // 否则必然 "program not found"。
        let (program, prefix_args) = resolve_windows_program(&config.command);
        let mut cmd = hidden_tokio_command(&program);
        cmd.args(&prefix_args);
        cmd.args(&config.args);
        if let Some(dir) = config.normalized_cwd()? {
            cmd.current_dir(dir);
        }
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());
        // 主进程随 Child 释放即终止（与 Drop 的进程树终止互补，覆盖 Drop 前被提前丢弃的场景）
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("启动 MCP 服务器 {} 失败: {}", config.name, e))?;
        let stdin = child.stdin.take().ok_or("无法获取 MCP stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取 MCP stdout")?;

        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            timeout,
            notifications: Vec::new(),
        })
    }

    async fn write_line(&mut self, line: &str) -> Result<(), String> {
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("写入 MCP 失败: {}", e))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("写入 MCP 失败: {}", e))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("写入 MCP 失败: {}", e))?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<Option<String>, String> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("读取 MCP 失败: {}", e))?;
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(line))
    }
}

#[async_trait::async_trait]
impl Transport for StdioTransport {
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write_line(&req.to_string()).await?;

        let timeout = self.timeout;
        loop {
            // 读超时：MCP 服务器长期无响应（卡死/断连未被察觉）时不让调用方无限阻塞。
            let read_result = match tokio::time::timeout(timeout, self.read_line()).await {
                Ok(r) => r?,
                Err(_) => {
                    // 按规范尽力通知服务端取消，避免其为已放弃的请求继续占用工作线程。
                    let _ = self
                        .notify(
                            "notifications/cancelled",
                            Some(json!({ "requestId": id, "reason": "timeout" })),
                        )
                        .await;
                    return Err(format!("MCP 请求超时（{} 秒无响应）", timeout.as_secs()));
                }
            };

            match read_result {
                None => return Err("MCP 服务器连接已关闭".to_string()),
                Some(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let v: Value = serde_json::from_str(line)
                        .map_err(|e| format!("解析 MCP 响应失败: {}", e))?;

                    if v.get("method").is_some() {
                        if let Some(server_id) = v.get("id").cloned() {
                            // 服务端发来的请求（有 method + id）：客户端未声明相关能力，统一回
                            // "方法未实现"，避免服务端挂在等待响应的状态。
                            let resp = json!({
                                "jsonrpc": "2.0",
                                "id": server_id,
                                "error": { "code": -32601, "message": "client 未实现该方法" }
                            });
                            let _ = self.write_line(&resp.to_string()).await;
                        } else {
                            // 服务端通知（无 id）：缓冲，供 list_changed 感知与诊断
                            self.notifications.push(v);
                        }
                        continue;
                    }

                    if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                        if let Some(err) = v.get("error") {
                            return Err(format!("MCP 错误: {}", err));
                        }
                        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                    }
                }
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut notif = json!({"jsonrpc": "2.0", "method": method});
        if let Some(p) = params {
            notif["params"] = p;
        }
        self.write_line(&notif.to_string()).await
    }

    fn take_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Windows 进程树终止：多数 MCP server 由 npx/uvx/node 拉起父子进程链，只杀主进程会
        // 残留孙进程（继续占用端口/资源）。taskkill /T /F 按父子关系递归强杀整棵树，与
        // tools/exec.rs 的 kill_tree 同思路；此处为同步调用，Drop 中可直接执行，且同步等待
        // 进程树终止完毕后才返回，即"收尸到 quiescence"。
        #[cfg(windows)]
        if let Some(pid) = self.child.id() {
            let _ = hidden_command("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        // 主进程补一发 kill（taskkill 失败或非 Windows 时的兜底；进程已被杀时无害）。
        let _ = self.child.start_kill();
        // 取舍说明：不在 Drop 内 block_on(child.wait()) 收尸——传输通常在 tokio async
        // 上下文内被 drop（工具调用/evict 路径），此时 block_on 会 panic；spawn 时已设
        // kill_on_drop(true)，Child 句柄随其释放而关闭。Windows 无僵尸进程语义，taskkill 已
        // 同步等进程树退出，不残留；非 Windows 上 kill_on_drop 终止主进程，孙进程残留属已知
        // 平台限制（进程树 kill 需 await 收尸，Drop 无法承担）。
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Streamable HTTP 传输
// ─────────────────────────────────────────────────────────────────────────────

/// SSE 帧解析器：增量喂字节，按空行边界产出完整 `data` 载荷。
/// `event:`/`id:`/`retry:` 行忽略——不做 Last-Event-ID 断线续传（见 HttpTransport 注释）。
#[derive(Default)]
struct SseParser {
    buf: Vec<u8>,
    data_lines: Vec<String>,
}

impl SseParser {
    /// 喂入字节块，返回本块内完整解析出的事件 data 列表
    fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut events = Vec::new();
        self.buf.extend_from_slice(chunk);
        let mut line_start = 0usize;
        while let Some(pos) = self.buf[line_start..].iter().position(|&b| b == b'\n') {
            let end = line_start + pos;
            let line = String::from_utf8_lossy(&self.buf[line_start..end])
                .trim_end_matches('\r')
                .to_string();
            line_start = end + 1;
            if line.is_empty() {
                // 事件边界：多条 data: 按规范以 \n 拼接
                if !self.data_lines.is_empty() {
                    events.push(self.data_lines.join("\n"));
                    self.data_lines.clear();
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                // 规范允许 data: 后带一个可选空格
                self.data_lines
                    .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
        }
        if line_start > 0 {
            self.buf.drain(..line_start);
        }
        events
    }
}

/// Streamable HTTP 传输（MCP 2025-03-26 起）：
/// - 每个 JSON-RPC 消息一次 POST；`Accept: application/json, text/event-stream`；
/// - 服务端可回单 JSON 或 SSE 流（流内可夹带通知，缓冲供排空）；
/// - initialize 响应头携带 `Mcp-Session-Id` 时，后续请求回传；
/// - 握手成功后后续请求携带 `MCP-Protocol-Version` 头；
/// - 关闭时发送 DELETE 显式结束会话（Drop 内 spawn 后台任务）。
///
/// 已知限制：不实现 GET  standalone SSE 流与 Last-Event-ID 断线续传——
/// 服务端推送的通知只在 POST 响应流内送达，这对请求/响应式工具调用已足够。
struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: HashMap<String, String>,
    session_id: Option<String>,
    protocol_version: Option<String>,
    notifications: Vec<Value>,
    timeout: Duration,
}

impl HttpTransport {
    fn new(config: &McpServerConfig, timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: config.url.clone().unwrap_or_default(),
            headers: config.headers.clone(),
            session_id: None,
            protocol_version: None,
            notifications: Vec::new(),
            timeout,
        }
    }

    /// 构造一次 POST（统一头：内容类型、双 Accept、自定义头、会话/协议版本头）
    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut rb = self
            .client
            .post(&self.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            );
        for (k, v) in &self.headers {
            rb = rb.header(k, v);
        }
        if let Some(sid) = &self.session_id {
            rb = rb.header("Mcp-Session-Id", sid);
        }
        if let Some(pv) = &self.protocol_version {
            rb = rb.header("MCP-Protocol-Version", pv);
        }
        rb.json(body)
    }

    /// 从单条 JSON-RPC 消息中提取结果或错误
    fn match_response(id: u64, v: Value) -> Result<Value, String> {
        if let Some(err) = v.get("error") {
            return Err(format!("MCP 错误: {}", err));
        }
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(format!("MCP 响应 id 不匹配: {}", v))
        }
    }

    /// 处理一条 SSE 事件消息：命中请求 id 则返回，通知入缓冲，其余忽略
    fn handle_sse_message(&mut self, id: u64, data: &str) -> Option<Result<Value, String>> {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return None, // 非 JSON 帧（如心跳）忽略
        };
        if v.get("method").is_some() {
            if v.get("id").is_some() {
                // 服务端经 SSE 发起的反向请求：客户端未声明能力，不回响应（仅记录）
                log::warn!("[MCP] 忽略服务端经 SSE 的反向请求（客户端未声明能力）");
            } else {
                self.notifications.push(v);
            }
            return None;
        }
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            return Some(Self::match_response(id, v));
        }
        None
    }

    async fn do_request(&mut self, id: u64, method: &str, body: &Value) -> Result<Value, String> {
        let resp = self
            .post(body)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP 请求失败: {}", e))?;

        // initialize 响应可能携带会话 id（后续所有请求必须回传）
        if method == "initialize" {
            if let Some(v) = resp
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
            {
                self.session_id = Some(v.to_string());
            }
        }

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("MCP HTTP 错误 {}: {}", status, text));
        }

        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        if ctype.starts_with("text/event-stream") {
            // SSE 响应流：增量解析，命中匹配 id 的消息即返回
            use futures::StreamExt;
            let mut parser = SseParser::default();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("读取 MCP SSE 流失败: {}", e))?;
                for data in parser.feed(&chunk) {
                    if let Some(result) = self.handle_sse_message(id, &data) {
                        return result;
                    }
                }
            }
            Err("MCP SSE 流结束但未收到匹配响应".to_string())
        } else {
            // 单 JSON 响应（部分服务端不标 Content-Type，缺省按 JSON 解析）
            let text = resp
                .text()
                .await
                .map_err(|e| format!("读取 MCP 响应失败: {}", e))?;
            let v: Value =
                serde_json::from_str(&text).map_err(|e| format!("解析 MCP 响应失败: {}", e))?;
            Self::match_response(id, v)
        }
    }
}

#[async_trait::async_trait]
impl Transport for HttpTransport {
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match tokio::time::timeout(self.timeout, self.do_request(id, method, &body)).await {
            Ok(r) => r,
            Err(_) => {
                // 与 stdio 路径一致：超时尽力通知服务端取消
                let _ = self
                    .notify(
                        "notifications/cancelled",
                        Some(json!({ "requestId": id, "reason": "timeout" })),
                    )
                    .await;
                Err(format!(
                    "MCP 请求超时（{} 秒无响应）",
                    self.timeout.as_secs()
                ))
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut body = json!({"jsonrpc": "2.0", "method": method});
        if let Some(p) = params {
            body["params"] = p;
        }
        let resp = self
            .post(&body)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP 通知失败: {}", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("MCP HTTP 通知错误 {}", status));
        }
        // 通知响应体（202 无体 / JSON / SSE）必须读尽，否则连接无法归还连接池；
        // SSE 体中的消息按通知入缓冲（通知响应本不该携带响应消息）。
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ctype.starts_with("text/event-stream") {
            use futures::StreamExt;
            let mut parser = SseParser::default();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("读取 MCP SSE 流失败: {}", e))?;
                for data in parser.feed(&chunk) {
                    if let Ok(v) = serde_json::from_str::<Value>(&data) {
                        if v.get("method").is_some() && v.get("id").is_none() {
                            self.notifications.push(v);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn take_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
    }

    fn on_initialized(&mut self, version: &str) {
        // 规范要求握手成功后后续请求携带协议版本头
        self.protocol_version = Some(version.to_string());
    }
}

impl Drop for HttpTransport {
    fn drop(&mut self) {
        // 按规范以 DELETE 显式结束会话；Drop 不能 await，交给后台任务尽力发送。
        // 仅在持有 tokio 运行时句柄时可发送（正常路径都满足；否则静默跳过）。
        if let Some(sid) = self.session_id.clone() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let client = self.client.clone();
                let url = self.url.clone();
                let headers = self.headers.clone();
                let pv = self.protocol_version.clone();
                handle.spawn(async move {
                    let mut rb = client.delete(&url).header("Mcp-Session-Id", sid);
                    for (k, v) in &headers {
                        rb = rb.header(k, v);
                    }
                    if let Some(pv) = pv {
                        rb = rb.header("MCP-Protocol-Version", pv);
                    }
                    let _ = rb.send().await;
                });
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Modern (2026-07-28) 传输
//
// 与 Legacy 的核心差异：
// - 无 initialize / initialized 握手；能力从 `server/discover` 一次性取得。
// - 每请求携带 `_meta` 信封（protocolVersion + clientId）。
// - HTTP 无 Mcp-Session-Id，Drop 时不发 DELETE（无状态）。
// - 不再处理服务端反向请求（modern 协议不支持 server→client 主动调用）。
// ─────────────────────────────────────────────────────────────────────────────

/// 带 `_meta` 的 JSON-RPC 对象构造器（modern 每请求复用）
fn modern_wrap(method: &str, id: Option<u64>, params: Option<Value>, meta: &Value) -> Value {
    let mut obj = json!({
        "jsonrpc": "2.0",
        "_meta": meta,
        "method": method,
    });
    if let Some(i) = id {
        obj["id"] = json!(i);
    }
    if let Some(p) = params {
        obj["params"] = p;
    }
    obj
}

fn modern_meta(version: &str) -> Value {
    json!({
        "protocolVersion": version,
        "clientId": MCP_CLIENT_ID,
    })
}

// ── modern stdio ──

/// Modern NDJSON stdio：读写格式与 Legacy 完全相同（NDJSON），
/// 但请求体里多了 `_meta`，且无握手。
struct ModernStdioTransport {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    timeout: Duration,
    protocol_version: String,
    notifications: Vec<Value>,
}

impl ModernStdioTransport {
    async fn spawn(config: &McpServerConfig, timeout: Duration) -> Result<Self, String> {
        let (program, prefix_args) = resolve_windows_program(&config.command);
        let mut cmd = hidden_tokio_command(&program);
        cmd.args(&prefix_args);
        cmd.args(&config.args);
        if let Some(dir) = config.normalized_cwd()? {
            cmd.current_dir(dir);
        }
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("启动 MCP 服务器 {} 失败: {}", config.name, e))?;
        let stdin = child.stdin.take().ok_or("无法获取 MCP stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取 MCP stdout")?;

        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            timeout,
            protocol_version: SUPPORTED_MODERN_VERSIONS[0].to_string(),
            notifications: Vec::new(),
        })
    }

    async fn write_value(&mut self, v: &Value) -> Result<(), String> {
        let line = serde_json::to_string(v).map_err(|e| format!("序列化 MCP 消息失败: {}", e))?;
        use tokio::io::AsyncWriteExt;
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("写入 MCP 失败: {}", e))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("写入 MCP 失败: {}", e))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("写入 MCP 失败: {}", e))
    }

    async fn read_value(&mut self) -> Result<Option<Value>, String> {
        let mut line = String::new();
        use tokio::io::AsyncBufReadExt;
        let n = self
            .reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("读取 MCP 失败: {}", e))?;
        if n == 0 {
            return Ok(None);
        }
        let line = line.trim();
        if line.is_empty() {
            return Ok(Some(Value::Null)); // 让上层 continue
        }
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("解析 MCP 响应失败: {}", e))?;
        Ok(Some(v))
    }
}

#[async_trait::async_trait]
impl Transport for ModernStdioTransport {
    fn protocol_version(&self) -> &str {
        &self.protocol_version
    }
    fn client_id(&self) -> &str {
        MCP_CLIENT_ID
    }
    fn on_discovered(&mut self, version: &str) {
        self.protocol_version = version.to_string();
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let meta = modern_meta(&self.protocol_version.clone());
        let params_for_wrap = params.clone();
        let req = modern_wrap(method, Some(id), Some(params_for_wrap), &meta);
        self.write_value(&req).await?;

        let timeout = self.timeout;
        loop {
            let read_result = match tokio::time::timeout(timeout, self.read_value()).await {
                Ok(r) => r?,
                Err(_) => {
                    // modern 协议没有 notifications/cancelled（是 legacy 私有的），
                    // 超时直接返回错误即可。
                    return Err(format!("MCP 请求超时（{} 秒无响应）", timeout.as_secs()));
                }
            };
            match read_result {
                None => return Err("MCP 服务器连接已关闭".to_string()),
                Some(Value::Null) => continue,
                Some(v) => {
                    // modern 协议不再支持 server→client 主动请求
                    if v.get("method").is_some() && v.get("id").is_none() {
                        self.notifications.push(v);
                        continue;
                    }
                    if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                        // -32022 UnsupportedProtocolVersionError：服务端列出支持版本，
                        // 从中选一个重试（只重试一次，避免无限循环）。
                        if let Some(err) = v.get("error") {
                            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                            if code == -32022 {
                                let supported = err
                                    .get("data")
                                    .and_then(|d| d.get("supported"))
                                    .and_then(|s| s.as_array())
                                    .ok_or_else(|| format!("UnsupportedProtocolVersionError 缺少 supported 列表: {}", v))?;
                                if let Some(new_ver) = supported
                                    .iter()
                                    .find_map(|x| x.as_str())
                                    .filter(|s| SUPPORTED_MODERN_VERSIONS.contains(s))
                                {
                                    self.protocol_version = new_ver.to_string();
                                    // 递归重试（版本已更新，后续请求都会用新版）
                                    return self.request(id, method, params.clone()).await;
                                }
                                return Err(format!(
                                    "服务端不支持任何客户端 modern 版本（服务端支持: {}）",
                                    serde_json::Value::Array(supported.clone()).to_string()
                                ));
                            }
                            return Err(format!("MCP 错误: {}", err));
                        }
                        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // 不匹配的响应（没有 id 的通知）：缓冲
                    if v.get("method").is_some() {
                        self.notifications.push(v);
                    }
                }
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let meta = modern_meta(&self.protocol_version.clone());
        let notif = modern_wrap(method, None, params, &meta);
        self.write_value(&notif).await
    }

    fn take_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
    }
}

impl Drop for ModernStdioTransport {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(pid) = self.child.id() {
            let _ = hidden_command("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        let _ = self.child.start_kill();
    }
}

// ── modern HTTP ──

/// Modern Streamable HTTP：无状态（无 Mcp-Session-Id、Drop 不发 DELETE），
/// 每请求携带 `_meta` 信封与 MCP-Protocol-Version 头。
struct ModernHttpTransport {
    client: reqwest::Client,
    url: String,
    headers: HashMap<String, String>,
    protocol_version: String,
    timeout: Duration,
    notifications: Vec<Value>,
}

impl ModernHttpTransport {
    fn new(config: &McpServerConfig, timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: config.url.clone().unwrap_or_default(),
            headers: config.headers.clone(),
            protocol_version: SUPPORTED_MODERN_VERSIONS[0].to_string(),
            timeout,
            notifications: Vec::new(),
        }
    }

    fn build_request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut rb = self
            .client
            .post(&self.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header("MCP-Protocol-Version", &self.protocol_version);
        for (k, v) in &self.headers {
            rb = rb.header(k, v);
        }
        rb.json(body)
    }

    /// 处理一条 SSE 事件：命中请求 id 即返回，通知缓冲，-32022 触发版本重试
    fn handle_event(&mut self, id: u64, data: &str) -> Option<Result<Value, String>> {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return None,
        };
        if v.get("method").is_some() && v.get("id").is_none() {
            self.notifications.push(v);
            return None;
        }
        if let Some(err) = v.get("error") {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
            if code == -32022 {
                let supported = err
                    .get("data")
                    .and_then(|d| d.get("supported"))
                    .and_then(|s| s.as_array());
                if let Some(arr) = supported {
                    if let Some(new_ver) = arr
                        .iter()
                        .find_map(|x| x.as_str())
                        .filter(|s| SUPPORTED_MODERN_VERSIONS.contains(s))
                    {
                        self.protocol_version = new_ver.to_string();
                    } else {
                        return Some(Err(format!(
                            "服务端不支持任何客户端 modern 版本: {}",
                            serde_json::Value::Array(arr.clone()).to_string()
                        )));
                    }
                }
                return None; // 让外层重试
            }
            if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                return Some(Err(format!("MCP 错误: {}", err)));
            }
        }
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            return Some(Ok(v.get("result").cloned().unwrap_or(Value::Null)));
        }
        None
    }

    async fn do_request(&mut self, id: u64, method: &str, body: &Value) -> Result<Value, String> {
        let resp = self
            .build_request(body)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP 请求失败: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            // modern 规范：版本不支持返回 4xx + -32022 JSON-RPC error body
            let text = resp.text().await.unwrap_or_default();
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(err) = v.get("error") {
                    let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                    if code == -32022 {
                        let supported = err
                            .get("data")
                            .and_then(|d| d.get("supported"))
                            .and_then(|s| s.as_array());
                        if let Some(arr) = supported {
                            if let Some(new_ver) = arr
                                .iter()
                                .find_map(|x| x.as_str())
                                .filter(|s| SUPPORTED_MODERN_VERSIONS.contains(s))
                            {
                                self.protocol_version = new_ver.to_string();
                                // 版本已更新，重试一次
                                let meta = modern_meta(&self.protocol_version.clone());
                                let body2 = modern_wrap(
                                    method,
                                    Some(id),
                                    body.get("params").cloned(),
                                    &meta,
                                );
                                return Box::pin(self.do_request(id, method, &body2)).await;
                            }
                        }
                        return Err(format!("服务端不支持任何客户端 modern 版本: {}", text));
                    }
                }
            }
            return Err(format!("MCP HTTP 错误 {}: {}", status, text));
        }

        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        if ctype.starts_with("text/event-stream") {
            use futures::StreamExt;
            let mut parser = SseParser::default();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("读取 MCP SSE 流失败: {}", e))?;
                for data in parser.feed(&chunk) {
                    match self.handle_event(id, &data) {
                        Some(Ok(r)) => return Ok(r),
                        Some(Err(e)) => return Err(e),
                        None => continue,
                    }
                }
            }
            Err("MCP SSE 流结束但未收到匹配响应".to_string())
        } else {
            let text = resp
                .text()
                .await
                .map_err(|e| format!("读取 MCP 响应失败: {}", e))?;
            let v: Value =
                serde_json::from_str(&text).map_err(|e| format!("解析 MCP 响应失败: {}", e))?;
            // -32022 同样处理
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                if code == -32022 {
                    let supported = err
                        .get("data")
                        .and_then(|d| d.get("supported"))
                        .and_then(|s| s.as_array());
                    if let Some(arr) = supported {
                        if let Some(new_ver) = arr
                            .iter()
                            .find_map(|x| x.as_str())
                            .filter(|s| SUPPORTED_MODERN_VERSIONS.contains(s))
                        {
                            self.protocol_version = new_ver.to_string();
                            let meta = modern_meta(&self.protocol_version.clone());
                            let body2 =
                                modern_wrap(method, Some(id), body.get("params").cloned(), &meta);
                            return Box::pin(self.do_request(id, method, &body2)).await;
                        }
                        return Err(format!(
                            "服务端不支持任何客户端 modern 版本: {}",
                            serde_json::Value::Array(arr.clone()).to_string()
                        ));
                    }
                }
                return Err(format!("MCP 错误: {}", err));
            }
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        }
    }
}

#[async_trait::async_trait]
impl Transport for ModernHttpTransport {
    fn protocol_version(&self) -> &str {
        &self.protocol_version
    }
    fn client_id(&self) -> &str {
        MCP_CLIENT_ID
    }
    fn on_discovered(&mut self, version: &str) {
        self.protocol_version = version.to_string();
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let meta = modern_meta(&self.protocol_version.clone());
        let body = modern_wrap(method, Some(id), Some(params), &meta);
        match tokio::time::timeout(self.timeout, self.do_request(id, method, &body)).await {
            Ok(r) => r,
            Err(_) => Err(format!(
                "MCP 请求超时（{} 秒无响应）",
                self.timeout.as_secs()
            )),
        }
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let meta = modern_meta(&self.protocol_version.clone());
        let body = modern_wrap(method, None, params, &meta);
        let resp = self
            .build_request(&body)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP 通知失败: {}", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("MCP HTTP 通知错误 {}", status));
        }
        // 通知响应体必须读尽（连接池归还）；SSE 通知缓冲
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ctype.starts_with("text/event-stream") {
            use futures::StreamExt;
            let mut parser = SseParser::default();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("读取 MCP SSE 流失败: {}", e))?;
                for data in parser.feed(&chunk) {
                    if let Ok(v) = serde_json::from_str::<Value>(&data) {
                        if v.get("method").is_some() && v.get("id").is_none() {
                            self.notifications.push(v);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn take_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Era 探测 & modern server/discover
// ─────────────────────────────────────────────────────────────────────────────

/// era 探测返回：识别为 legacy（可降级）或 fatal（不可恢复）
enum DiscoverError {
    LegacyServer,
    Fatal(String),
}

/// 从 capabilities JSON 提取 tools/resources/prompts 标志（legacy/modern 共用）
fn caps_from_value(capabilities: &Value) -> McpCapabilities {
    McpCapabilities {
        tools: capabilities.get("tools").is_some(),
        resources: capabilities.get("resources").is_some(),
        prompts: capabilities.get("prompts").is_some(),
    }
}

/// modern server/discover 请求（stdio）：
/// - 成功 → Ok((capabilities, version))
/// - 服务端是 legacy（unknown method / timeout / 非 -32022 错误）→ LegacyServer
/// - 其他不可恢复错误 → Fatal
async fn modern_discover_stdio(
    transport: &mut ModernStdioTransport,
) -> Result<(McpCapabilities, String), DiscoverError> {
    // 在 transport 外直接发一次性 server/discover：复用现代 transport 的读写逻辑
    use tokio::io::AsyncBufReadExt;

    let meta = modern_meta(&transport.protocol_version.clone());
    let req = modern_wrap("server/discover", Some(1), Some(json!({})), &meta);
    transport
        .write_value(&req)
        .await
        .map_err(|e| DiscoverError::Fatal(format!("{}", e)))?;

    loop {
        let mut line = String::new();
        let n = transport
            .reader
            .read_line(&mut line)
            .await
            .map_err(|e| DiscoverError::Fatal(format!("{}", e)))?;
        if n == 0 {
            // stdio 进程直接退出：legacy server 可能因为不认识 _meta 直接挂掉
            return Err(DiscoverError::LegacyServer);
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value =
            serde_json::from_str(line).map_err(|e| DiscoverError::Fatal(format!("{}", e)))?;

        if v.get("method").is_some() {
            // 服务端反向请求或通知（探测阶段不该有，但忽略即可）
            continue;
        }

        // 匹配 id=1 的响应
        if v.get("id").and_then(|x| x.as_u64()) == Some(1) {
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                if code == -32601 // Method Not Found → legacy server 不认识 server/discover
                    || msg.contains("unknown method")
                    || msg.contains("not found")
                    || msg.contains("unrecognized")
                {
                    return Err(DiscoverError::LegacyServer);
                }
                if code == -32022 {
                    // modern server 拒绝当前版本 → 选 supported 里的一个重试
                    // 但 stdio 路径这里重试意义不大（版本不匹配说明客户端太新/太旧），
                    // 直接 Fatal 让上层报告。
                    return Err(DiscoverError::Fatal(format!(
                        "服务端不支持任何客户端 modern 版本: {}",
                        err
                    )));
                }
                // 其他 JSON-RPC 错误：也按 legacy 处理（服务端可能对带 _meta 的请求有其他兼容问题）
                return Err(DiscoverError::LegacyServer);
            }

            // 成功：提取 capabilities + version
            let result = v.get("result").cloned().unwrap_or(Value::Null);
            let capabilities = result
                .get("capabilities")
                .cloned()
                .unwrap_or_else(|| json!({}));
            // modern 版本协商：server/discover 响应里可能带 protocolVersion
            let version = result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(SUPPORTED_MODERN_VERSIONS[0])
                .to_string();
            if !SUPPORTED_MODERN_VERSIONS.contains(&version.as_str()) {
                return Err(DiscoverError::Fatal(format!(
                    "服务端返回不受支持的 modern 版本: {}",
                    version
                )));
            }
            return Ok((caps_from_value(&capabilities), version));
        }
    }
}

/// modern server/discover 请求（HTTP）：
/// modern server 成功返回 JSON；legacy server 可能返回 4xx 或其他非 modern 错误
async fn modern_discover_http(
    transport: &mut ModernHttpTransport,
) -> Result<(McpCapabilities, String), DiscoverError> {
    let meta = modern_meta(&transport.protocol_version.clone());
    let body = modern_wrap("server/discover", Some(1), Some(json!({})), &meta);

    let resp = transport
        .build_request(&body)
        .send()
        .await
        .map_err(|e| DiscoverError::Fatal(format!("MCP HTTP 探测失败: {}", e)))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        // 400 + -32022 → modern server 拒绝版本（但仍在 modern era）
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                if code == -32022 {
                    return Err(DiscoverError::Fatal(format!(
                        "服务端不支持任何客户端 modern 版本: {}",
                        text
                    )));
                }
            }
        }
        // 4xx/5xx 非 modern 错误 → legacy server
        return Err(DiscoverError::LegacyServer);
    }

    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    let text = if ctype.starts_with("text/event-stream") {
        // 探测不该是 SSE 流，但以防万一
        let mut buf = String::new();
        use futures::StreamExt;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| DiscoverError::Fatal(format!("读 SSE 失败: {}", e)))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
        }
        buf
    } else {
        resp.text()
            .await
            .map_err(|e| DiscoverError::Fatal(format!("读响应失败: {}", e)))?
    };

    let v: Value = serde_json::from_str(&text).map_err(|_e| {
        // legacy server 可能返回非 JSON（HTML 错误页等）→ legacy
        DiscoverError::LegacyServer
    })?;

    // modern server 响应必须带 id=1 或 result
    if let Some(err) = v.get("error") {
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        if code == -32601
            || msg.contains("unknown method")
            || msg.contains("not found")
            || msg.contains("unrecognized")
        {
            return Err(DiscoverError::LegacyServer);
        }
        if code == -32022 {
            return Err(DiscoverError::Fatal(format!(
                "服务端不支持任何客户端 modern 版本: {}",
                err
            )));
        }
        // 其他错误：按 legacy 处理
        return Err(DiscoverError::LegacyServer);
    }

    let result = v.get("result").cloned().unwrap_or(Value::Null);
    let capabilities = result
        .get("capabilities")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(SUPPORTED_MODERN_VERSIONS[0])
        .to_string();
    if !SUPPORTED_MODERN_VERSIONS.contains(&version.as_str()) {
        return Err(DiscoverError::Fatal(format!(
            "服务端返回不受支持的 modern 版本: {}",
            version
        )));
    }
    Ok((caps_from_value(&capabilities), version))
}

// ─────────────────────────────────────────────────────────────────────────────
// 客户端（dual-era 自动探测）
// ─────────────────────────────────────────────────────────────────────────────

/// MCP 客户端：dual-era 自动探测后持有对应传输层，完成能力发现并暴露 tools/resources/prompts。
/// Legacy 走 initialize/initialized 握手；Modern 走 server/discover + 逐请求 `_meta`。
pub struct McpClient {
    name: String,
    transport: Box<dyn Transport>,
    next_id: u64,
    capabilities: McpCapabilities,
    /// 当前协商后的协议版本（诊断日志用）
    protocol_version: String,
}

impl McpClient {
    pub async fn connect(config: &McpServerConfig) -> Result<Self, String> {
        config.validate()?;
        let timeout = config.request_timeout();

        // 先尝试 modern：spawn/connect modern transport，发 server/discover。
        // 成功 → modern；返回 unknown method 或其他 legacy 风格错误 → drop 重建 legacy。
        // HTTP 同理（modern 先发，4xx body 非 -32022 → 走 legacy initialize）。
        let transport: Box<dyn Transport> = if config.is_http() {
            // HTTP：先 modern（带 _meta + MCP-Protocol-Version）
            let mut modern = ModernHttpTransport::new(config, timeout);
            match tokio::time::timeout(timeout, modern_discover_http(&mut modern)).await {
                Ok(Ok((caps, version))) => {
                    modern.on_discovered(&version);
                    return Ok(Self {
                        name: config.name.clone(),
                        transport: Box::new(modern),
                        next_id: 1,
                        capabilities: caps,
                        protocol_version: version,
                    }
                    .log_connected(&config.name));
                }
                Ok(Err(DiscoverError::LegacyServer)) => {
                    // 服务端是 legacy：modern transport 里没保留状态，直接丢弃
                    Box::new(HttpTransport::new(config, timeout))
                }
                Ok(Err(DiscoverError::Fatal(e))) => return Err(e),
                Err(_) => {
                    return Err(format!(
                        "探测 MCP 服务端 era 超时（{} 秒）",
                        timeout.as_secs()
                    ))
                }
            }
        } else {
            // stdio：先 modern（server/discover）
            let mut modern = ModernStdioTransport::spawn(config, timeout).await?;
            match tokio::time::timeout(timeout, modern_discover_stdio(&mut modern)).await {
                Ok(Ok((caps, version))) => {
                    modern.on_discovered(&version);
                    return Ok(Self {
                        name: config.name.clone(),
                        transport: Box::new(modern),
                        next_id: 1,
                        capabilities: caps,
                        protocol_version: version,
                    }
                    .log_connected(&config.name));
                }
                Ok(Err(DiscoverError::LegacyServer)) => {
                    // drop modern（进程被杀），重建 legacy
                    Box::new(StdioTransport::spawn(config, timeout).await?)
                }
                Ok(Err(DiscoverError::Fatal(e))) => return Err(e),
                Err(_) => {
                    return Err(format!(
                        "探测 MCP 服务端 era 超时（{} 秒）",
                        timeout.as_secs()
                    ))
                }
            }
        };

        // legacy 路径
        let mut client = Self {
            name: config.name.clone(),
            transport,
            next_id: 1,
            capabilities: McpCapabilities::default(),
            protocol_version: String::new(),
        };
        client.initialize_legacy().await?;
        client.initialized_legacy().await?;
        Ok(client.log_connected(&config.name))
    }

    fn log_connected(self, server_name: &str) -> Self {
        let caps = self.capabilities;
        log::info!(
            "[MCP] 已连接 {}：protocol={} tools={} resources={} prompts={}",
            server_name,
            self.protocol_version,
            caps.tools,
            caps.resources,
            caps.prompts
        );
        self
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.transport.request(id, method, params).await
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        self.transport.notify(method, params).await
    }

    async fn initialize_legacy(&mut self) -> Result<(), String> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": SUPPORTED_LEGACY_VERSIONS[0],
                    "capabilities": {},
                    "clientInfo": {
                        "name": "pilotdesk",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;

        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if version.is_empty() {
            return Err(format!("MCP 服务器 {} 未返回 protocolVersion", self.name));
        }
        if !SUPPORTED_LEGACY_VERSIONS.contains(&version.as_str()) {
            return Err(format!(
                "MCP 服务器 {} 返回不受支持的 Legacy 协议版本 {}（客户端支持: {}）",
                self.name,
                version,
                SUPPORTED_LEGACY_VERSIONS.join(" / ")
            ));
        }

        let capabilities = result
            .get("capabilities")
            .cloned()
            .unwrap_or_else(|| json!({}));
        self.capabilities = caps_from_value(&capabilities);
        self.protocol_version = version.clone();
        self.transport.on_initialized(&version);
        Ok(())
    }

    async fn initialized_legacy(&mut self) -> Result<(), String> {
        self.notify("notifications/initialized", None).await
    }

    /// 服务端声明的能力（装配层据此决定是否注册 resources/prompts 合成工具）
    pub fn capabilities(&self) -> McpCapabilities {
        self.capabilities
    }

    /// 排空服务端推送的通知（工具调用间隙顺带取用）
    pub fn take_notifications(&mut self) -> Vec<Value> {
        self.transport.take_notifications()
    }

    /// 通用分页拉取：`method` 响应中 `key` 数组按 `nextCursor` 翻页合并
    async fn list_paginated(&mut self, method: &str, key: &str) -> Result<Vec<Value>, String> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;

        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let result = self.request(method, params).await?;
            if let Some(arr) = result.get(key).and_then(|v| v.as_array()) {
                items.extend(arr.iter().cloned());
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }

        Ok(items)
    }

    /// 列出服务器提供的工具（按 name 去重）
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, String> {
        // 未声明 tools 能力时不应调用 tools/list。
        if !self.capabilities.tools {
            return Ok(Vec::new());
        }

        let raw = self.list_paginated("tools/list", "tools").await?;
        let mut infos = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for t in raw {
            let name = t
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() || !seen.insert(name.clone()) {
                continue;
            }
            let title = t.get("title").and_then(|x| x.as_str()).map(str::to_string);
            let description = t
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            let schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object"}));
            let annotations = ToolAnnotations::from_value(t.get("annotations"));
            infos.push(McpToolInfo {
                name,
                title,
                description,
                schema,
                annotations,
            });
        }
        Ok(infos)
    }

    /// 调用指定工具，返回文本结果
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String, String> {
        let result = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;

        // 工具级失败通过 isError 表达（JSON-RPC 层仍是成功响应），必须转成 Err，
        // 否则模型会把"执行失败"当成正常输出继续推理。
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();

        let mut texts: Vec<String> = Vec::new();
        for c in content {
            if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(txt) = c.get("text").and_then(|t| t.as_str()) {
                    texts.push(txt.to_string());
                }
            } else {
                texts.push(c.to_string());
            }
        }

        if texts.is_empty() {
            // 无 content 时优先取结构化结果（outputSchema 对应的机器可读载荷）。
            match result.get("structuredContent") {
                Some(sc) => {
                    texts.push(serde_json::to_string_pretty(sc).unwrap_or_else(|_| sc.to_string()))
                }
                None => texts.push(result.to_string()),
            }
        }

        let output = texts.join("\n");
        if is_error {
            return Err(format!("MCP 工具 {} 执行失败: {}", name, output));
        }
        Ok(output)
    }

    /// 列出服务器资源（按 uri 去重）
    pub async fn list_resources(&mut self) -> Result<Vec<McpResourceInfo>, String> {
        if !self.capabilities.resources {
            return Ok(Vec::new());
        }
        let raw = self.list_paginated("resources/list", "resources").await?;
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for r in raw {
            let uri = r
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if uri.is_empty() || !seen.insert(uri.clone()) {
                continue;
            }
            out.push(McpResourceInfo {
                uri,
                name: r
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                description: r
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                mime_type: r
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
        Ok(out)
    }

    /// 读取资源内容：text 直接拼接；blob（base64 二进制）以占位标注，避免污染上下文
    pub async fn read_resource(&mut self, uri: &str) -> Result<String, String> {
        let result = self
            .request("resources/read", json!({ "uri": uri }))
            .await?;
        let contents = result
            .get("contents")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        for c in contents {
            if let Some(t) = c.get("text").and_then(Value::as_str) {
                parts.push(t.to_string());
            } else if let Some(b) = c.get("blob").and_then(Value::as_str) {
                // base64 长度 ≈ 原始字节数 * 4/3
                parts.push(format!("[binary {} bytes (base64)]", b.len() * 3 / 4));
            }
        }
        if parts.is_empty() {
            parts.push(result.to_string());
        }
        Ok(parts.join("\n"))
    }

    /// 列出服务器提示词（按 name 去重）
    pub async fn list_prompts(&mut self) -> Result<Vec<McpPromptInfo>, String> {
        if !self.capabilities.prompts {
            return Ok(Vec::new());
        }
        let raw = self.list_paginated("prompts/list", "prompts").await?;
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for p in raw {
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if name.is_empty() || !seen.insert(name.clone()) {
                continue;
            }
            out.push(McpPromptInfo {
                name,
                title: p.get("title").and_then(Value::as_str).map(str::to_string),
                description: p
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                arguments: p
                    .get("arguments")
                    .and_then(|a| a.as_array())
                    .cloned()
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// 获取提示词：messages 中 text 内容按角色前缀拼接
    pub async fn get_prompt(&mut self, name: &str, arguments: Value) -> Result<String, String> {
        let result = self
            .request(
                "prompts/get",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        let messages = result
            .get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        for m in messages {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
            let content = m.get("content").cloned().unwrap_or(Value::Null);
            // content 可能是单对象或数组
            let items = match content.as_array() {
                Some(arr) => arr.clone(),
                None => vec![content],
            };
            for c in items {
                if c.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = c.get("text").and_then(Value::as_str) {
                        parts.push(format!("[{}] {}", role, t));
                    }
                } else {
                    parts.push(format!("[{}] {}", role, c));
                }
            }
        }
        if parts.is_empty() {
            parts.push(result.to_string());
        }
        Ok(parts.join("\n\n"))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 工具装配（MCP 工具 → Agent 工具）
// ─────────────────────────────────────────────────────────────────────────────

/// 将某个 MCP 工具暴露为 Agent 工具（经连接池共享客户端，串行化调用 + 失败重连）。
pub struct McpToolHandler {
    pool: McpConnectionPool,
    server: McpServerConfig,
    full_name: String,
    raw_name: String,
    description: String,
    schema: Value,
    annotations: ToolAnnotations,
}

impl McpToolHandler {
    pub fn new(pool: McpConnectionPool, server: McpServerConfig, info: McpToolInfo) -> Self {
        let full_name = format!("mcp_{}_{}", server.name, info.name);
        let description = if info.description.trim().is_empty() {
            format!(
                "[MCP:{}] {}",
                full_name,
                info.title.as_deref().unwrap_or(&info.name)
            )
        } else {
            format!("[MCP:{}] {}", full_name, info.description)
        };
        Self {
            pool,
            server,
            full_name,
            raw_name: info.name,
            description,
            schema: info.schema,
            annotations: info.annotations,
        }
    }
}

fn is_remote_path_source(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("data:")
        || value.starts_with("blob:")
}

fn normalized_path_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_path_like_key(key: &str) -> bool {
    let key = normalized_path_key(key);
    matches!(
        key.as_str(),
        "path"
            | "paths"
            | "file"
            | "files"
            | "filename"
            | "filenames"
            | "filepath"
            | "filepaths"
            | "dir"
            | "dirs"
            | "directory"
            | "directories"
            | "root"
            | "roots"
            | "cwd"
            | "folder"
            | "folders"
            | "workspace"
            | "workspaceroot"
            | "workspaceroots"
    ) || key.ends_with("path")
        || key.ends_with("paths")
        || key.ends_with("dir")
        || key.ends_with("dirs")
        || key.ends_with("directory")
        || key.ends_with("directories")
        || key.ends_with("root")
        || key.ends_with("roots")
        || key.ends_with("cwd")
        || key.ends_with("folder")
        || key.ends_with("folders")
        || matches!(
            key.as_str(),
            "sourcefile"
                | "targetfile"
                | "inputfile"
                | "outputfile"
                | "uploadfile"
                | "downloadfile"
                | "sourcefiles"
                | "targetfiles"
                | "inputfiles"
                | "outputfiles"
        )
}

fn collect_string_values(value: &Value, paths: &mut Vec<LocalPathArgument>) {
    match value {
        Value::String(path) => {
            let path = path.trim();
            if !path.is_empty() && !is_remote_path_source(path) {
                paths.push(LocalPathArgument {
                    path: path.to_string(),
                    base: None,
                });
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_string_values(item, paths);
            }
        }
        Value::Object(object) => {
            for item in object.values() {
                collect_string_values(item, paths);
            }
        }
        _ => {}
    }
}

/// 从 MCP JSON Schema 和实际参数中提取本地路径。
///
/// 路径字段名可能由任意服务端定义，因此同时检查 schema 属性名与实际参数名。
/// 一旦参数值看起来包含路径、却无法解析成字符串或字符串数组，则返回 Err 让调用方
/// 失败关闭，避免服务端用嵌套对象绕过矩阵。
fn mcp_local_path_arguments(
    schema: &Value,
    arguments: &Value,
) -> Result<Vec<LocalPathArgument>, String> {
    let mut paths = Vec::new();

    fn walk_schema(
        schema: &Value,
        arguments: &Value,
        paths: &mut Vec<LocalPathArgument>,
    ) -> Result<(), String> {
        let Some(schema_object) = schema.as_object() else {
            return Ok(());
        };
        if let Some(properties) = schema_object.get("properties").and_then(Value::as_object) {
            for (key, property_schema) in properties {
                let Some(argument) = arguments.get(key) else {
                    continue;
                };
                if is_path_like_key(key) {
                    if !matches!(argument, Value::String(_) | Value::Array(_) | Value::Null) {
                        return Err(format!(
                            "MCP 参数 {} 声明为路径字段，但值不是字符串或字符串数组",
                            key
                        ));
                    }
                    collect_string_values(argument, paths);
                } else if argument.is_object() || argument.is_array() {
                    walk_schema(property_schema, argument, paths)?;
                }
            }
        }
        for keyword in ["items", "anyOf", "oneOf", "allOf"] {
            if let Some(nested) = schema_object.get(keyword) {
                if let Some(items) = nested.as_array() {
                    for item in items {
                        walk_schema(item, arguments, paths)?;
                    }
                } else {
                    walk_schema(nested, arguments, paths)?;
                }
            }
        }
        Ok(())
    }

    walk_schema(schema, arguments, &mut paths)?;

    // schema 可能用 additionalProperties 或没有声明完整属性；实际参数中的路径键同样纳入。
    fn walk_arguments(value: &Value, paths: &mut Vec<LocalPathArgument>) -> Result<(), String> {
        match value {
            Value::Object(object) => {
                for (key, item) in object {
                    if is_path_like_key(key) {
                        if !matches!(item, Value::String(_) | Value::Array(_) | Value::Null) {
                            return Err(format!(
                                "MCP 参数 {} 看起来是本地路径，但值不是字符串或字符串数组",
                                key
                            ));
                        }
                        collect_string_values(item, paths);
                    } else if item.is_object() || item.is_array() {
                        walk_arguments(item, paths)?;
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk_arguments(item, paths)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk_arguments(arguments, &mut paths)?;

    let mut seen = HashSet::new();
    paths.retain(|argument| {
        let key = argument.path.replace('\\', "/").to_lowercase();
        seen.insert(key)
    });
    paths.sort_by_cached_key(|argument| {
        (
            argument.path.replace('\\', "/").to_lowercase(),
            argument
                .base
                .as_deref()
                .unwrap_or_default()
                .replace('\\', "/")
                .to_lowercase(),
        )
    });
    Ok(paths)
}

/// 排空客户端通知并记录 list_changed 日志（各 handler execute 前的统一动作）
fn drain_notifications(client: &mut McpClient, server_name: &str) {
    for n in client.take_notifications() {
        match n.get("method").and_then(Value::as_str) {
            Some("notifications/tools/list_changed") => log::info!(
                "[MCP] 服务器 {} 推送 tools/list_changed（工具清单将在下次会话重新枚举）",
                server_name
            ),
            Some("notifications/resources/list_changed") => {
                log::info!("[MCP] 服务器 {} 推送 resources/list_changed", server_name)
            }
            Some("notifications/prompts/list_changed") => {
                log::info!("[MCP] 服务器 {} 推送 prompts/list_changed", server_name)
            }
            _ => log::debug!("[MCP] 服务器 {} 通知: {}", server_name, n),
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for McpToolHandler {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.schema.clone()
    }

    fn risk_level(&self) -> RiskLevel {
        // 按 MCP 注解分档：只读 → Low（静默）；显式非破坏 → Medium（确认）；
        // 未声明或声明破坏性 → High（严格确认）。
        if self.annotations.read_only {
            RiskLevel::Low
        } else if self.annotations.destructive {
            RiskLevel::High
        } else {
            RiskLevel::Medium
        }
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Mcp]
    }

    fn local_path_arguments(&self, arguments: &Value) -> Result<Vec<LocalPathArgument>, String> {
        mcp_local_path_arguments(&self.schema, arguments)
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        // 懒连接（连接池缓存共享客户端）；连接中断时 evict 后重连一次，避免一次失败即永久失效。
        // 仅在只读/幂等工具上重试——否则可能重复执行有副作用的写操作。
        let retryable = self.annotations.retryable();
        let mut attempt = 0u8;
        let mut retried_not_found = false;
        loop {
            let client = self.pool.get_or_connect(&self.server).await?;
            let result = {
                let mut c = client.lock().await;
                drain_notifications(&mut c, &self.server.name);
                c.call_tool(&self.raw_name, arguments.clone()).await
            };
            match result {
                Ok(v) => return Ok(v),
                Err(e) => {
                    // 工具被服务端移除（list_changed 后缓存清单过期）：重连重试一次。
                    // 此时调用未真正执行，重试安全，不受注解 retryable 限制。
                    if !retried_not_found && e.to_ascii_lowercase().contains("not found") {
                        self.pool.evict(&self.server.name);
                        retried_not_found = true;
                        continue;
                    }
                    if attempt == 0 && retryable {
                        self.pool.evict(&self.server.name);
                        attempt += 1;
                        continue;
                    }
                    if retried_not_found {
                        return Err(format!(
                            "{}（服务端工具列表可能已变更，将在下次会话重新枚举）",
                            e
                        ));
                    }
                    return Err(e);
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 合成工具（resources / prompts 能力 → Agent 工具）
//
// 命名约定：双下划线 `mcp_<服务器>__<操作>`，与服务端原生工具
// （单下划线 `mcp_<服务器>_<工具名>`）区分，避免同名冲突。
// ─────────────────────────────────────────────────────────────────────────────

/// 资源合成工具操作类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceKind {
    List,
    Read,
}

/// 将 MCP 服务器的 resources 能力暴露为合成 Agent 工具
pub struct McpResourceHandler {
    pool: McpConnectionPool,
    server: McpServerConfig,
    full_name: String,
    description: String,
    schema: Value,
    kind: ResourceKind,
}

impl McpResourceHandler {
    fn build(pool: McpConnectionPool, server: McpServerConfig, kind: ResourceKind) -> Self {
        let (suffix, description, schema) = match kind {
            ResourceKind::List => (
                "list_resources",
                format!(
                    "List resources available on MCP server \"{}\" (uri / name / description / mimeType).",
                    server.name
                ),
                json!({"type": "object", "properties": {}}),
            ),
            ResourceKind::Read => (
                "read_resource",
                format!(
                    "Read a resource from MCP server \"{}\" by uri (from list_resources).",
                    server.name
                ),
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "资源 URI" }
                    },
                    "required": ["uri"]
                }),
            ),
        };
        Self {
            pool,
            full_name: format!("mcp_{}__{}", server.name, suffix),
            server,
            description: format!("[MCP] {}", description),
            schema,
            kind,
        }
    }

    pub fn list(pool: McpConnectionPool, server: McpServerConfig) -> Self {
        Self::build(pool, server, ResourceKind::List)
    }

    pub fn read(pool: McpConnectionPool, server: McpServerConfig) -> Self {
        Self::build(pool, server, ResourceKind::Read)
    }
}

#[async_trait::async_trait]
impl ToolHandler for McpResourceHandler {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.schema.clone()
    }

    fn risk_level(&self) -> RiskLevel {
        // 列举与读取均为只读操作
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Mcp]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let client = self.pool.get_or_connect(&self.server).await?;
        let mut c = client.lock().await;
        drain_notifications(&mut c, &self.server.name);
        match self.kind {
            ResourceKind::List => {
                let list = c.list_resources().await?;
                serde_json::to_string_pretty(&list)
                    .map_err(|e| format!("序列化资源列表失败: {}", e))
            }
            ResourceKind::Read => {
                let uri = arguments
                    .get("uri")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| "缺少必需参数 uri".to_string())?;
                c.read_resource(uri).await
            }
        }
    }
}

/// 提示词合成工具操作类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    List,
    Get,
}

/// 将 MCP 服务器的 prompts 能力暴露为合成 Agent 工具
pub struct McpPromptHandler {
    pool: McpConnectionPool,
    server: McpServerConfig,
    full_name: String,
    description: String,
    schema: Value,
    kind: PromptKind,
}

impl McpPromptHandler {
    fn build(pool: McpConnectionPool, server: McpServerConfig, kind: PromptKind) -> Self {
        let (suffix, description, schema) = match kind {
            PromptKind::List => (
                "list_prompts",
                format!(
                    "List prompt templates available on MCP server \"{}\" (name / description / arguments).",
                    server.name
                ),
                json!({"type": "object", "properties": {}}),
            ),
            PromptKind::Get => (
                "get_prompt",
                format!(
                    "Get a rendered prompt from MCP server \"{}\" by name (from list_prompts).",
                    server.name
                ),
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "提示词名称" },
                        "arguments": {
                            "type": "object",
                            "description": "提示词参数（键值对，取自 list_prompts 的 arguments 清单）"
                        }
                    },
                    "required": ["name"]
                }),
            ),
        };
        Self {
            pool,
            full_name: format!("mcp_{}__{}", server.name, suffix),
            server,
            description: format!("[MCP] {}", description),
            schema,
            kind,
        }
    }

    pub fn list(pool: McpConnectionPool, server: McpServerConfig) -> Self {
        Self::build(pool, server, PromptKind::List)
    }

    pub fn get(pool: McpConnectionPool, server: McpServerConfig) -> Self {
        Self::build(pool, server, PromptKind::Get)
    }
}

#[async_trait::async_trait]
impl ToolHandler for McpPromptHandler {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.schema.clone()
    }

    fn risk_level(&self) -> RiskLevel {
        match self.kind {
            PromptKind::List => RiskLevel::Low,
            // get 可能携带用户参数触发服务端模板渲染，按可写网络请求对待
            PromptKind::Get => RiskLevel::Medium,
        }
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Mcp]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let client = self.pool.get_or_connect(&self.server).await?;
        let mut c = client.lock().await;
        drain_notifications(&mut c, &self.server.name);
        match self.kind {
            PromptKind::List => {
                let list = c.list_prompts().await?;
                serde_json::to_string_pretty(&list)
                    .map_err(|e| format!("序列化提示词列表失败: {}", e))
            }
            PromptKind::Get => {
                let name = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| "缺少必需参数 name".to_string())?;
                let args = arguments
                    .get("arguments")
                    .cloned()
                    .filter(|a| a.is_object())
                    .unwrap_or_else(|| json!({}));
                c.get_prompt(name, args).await
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 连接池
// ─────────────────────────────────────────────────────────────────────────────

/// MCP 连接池：按服务器名称懒连接并缓存共享客户端。
/// `std::sync::Mutex` 仅保护 map，跨 await 只传递 owned `Arc`（不持有锁跨 await）。
#[derive(Clone, Default)]
pub struct McpConnectionPool {
    connections: Arc<std::sync::Mutex<HashMap<String, Arc<Mutex<McpClient>>>>>,
}

impl McpConnectionPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取（必要时懒连接）某服务器的共享客户端；连接失败返回 Err（不缓存失败）。
    pub async fn get_or_connect(
        &self,
        server: &McpServerConfig,
    ) -> Result<Arc<Mutex<McpClient>>, String> {
        if let Some(c) = self
            .connections
            .lock()
            .map_err(|_| "MCP 连接池锁失败".to_string())?
            .get(&server.name)
        {
            return Ok(c.clone());
        }
        let client = McpClient::connect(server).await?;
        let shared = Arc::new(Mutex::new(client));
        self.connections
            .lock()
            .map_err(|_| "MCP 连接池锁失败".to_string())?
            .insert(server.name.clone(), shared.clone());
        Ok(shared)
    }

    /// 断开并移除某服务器连接（卸载 / 失败重连用）。
    pub fn evict(&self, server_name: &str) {
        if let Ok(mut m) = self.connections.lock() {
            m.remove(server_name);
        }
    }

    /// 当前已连接服务器数（诊断用）。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.connections.lock().map(|m| m.len()).unwrap_or(0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 测试
//
// 全部 in-process：MockTransport 脚本化协议单测 + tokio TcpListener 迷你
// Streamable HTTP 服务器集成测试，不依赖外部进程或网络。
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tokio::io::AsyncReadExt;

    /// 脚本化传输：每次 request 弹出队首，校验 method 顺序后返回预定结果
    struct MockTransport {
        script: VecDeque<(String, Result<Value, String>)>,
        /// 已收到的请求 (method, params)，断言用
        log: Arc<std::sync::Mutex<Vec<(String, Value)>>>,
    }

    #[async_trait::async_trait]
    impl Transport for MockTransport {
        async fn request(
            &mut self,
            _id: u64,
            method: &str,
            params: Value,
        ) -> Result<Value, String> {
            self.log.lock().unwrap().push((method.to_string(), params));
            let (expected, result) = self.script.pop_front().expect("脚本队列已空");
            assert_eq!(expected, method, "请求方法顺序与脚本不符");
            result
        }
        async fn notify(&mut self, _method: &str, _params: Option<Value>) -> Result<(), String> {
            Ok(())
        }
    }

    type RequestLog = Arc<std::sync::Mutex<Vec<(String, Value)>>>;

    /// 构造携带脚本化传输的客户端（绕过 connect，直接驱动协议方法）
    fn client_with(script: Vec<(&str, Result<Value, String>)>) -> (McpClient, RequestLog) {
        let log: RequestLog = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mock = MockTransport {
            script: script
                .into_iter()
                .map(|(m, r)| (m.to_string(), r))
                .collect(),
            log: log.clone(),
        };
        (
            McpClient {
                name: "mock".to_string(),
                transport: Box::new(mock),
                next_id: 1,
                capabilities: McpCapabilities::default(),
                protocol_version: SUPPORTED_LEGACY_VERSIONS[0].to_string(),
            },
            log,
        )
    }

    fn init_result(version: &str, caps: Value) -> Value {
        json!({
            "protocolVersion": version,
            "capabilities": caps,
            "serverInfo": {"name": "mock", "version": "0"}
        })
    }

    fn stdio_config() -> McpServerConfig {
        McpServerConfig {
            name: "s".to_string(),
            command: "x".to_string(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            timeout_secs: None,
            transport: None,
            url: None,
            headers: HashMap::new(),
        }
    }

    // ── 配置 ──

    #[test]
    fn config_validate_and_transport_inference() {
        // stdio 缺 command → 报错
        let mut cfg = stdio_config();
        cfg.command = "  ".into();
        assert!(cfg.validate().unwrap_err().contains("command"));

        // url 存在 → 自动按 http 推断
        let mut cfg = stdio_config();
        cfg.url = Some("https://x.example/mcp".into());
        assert!(cfg.is_http());
        assert!(cfg.validate().is_ok());

        // 非法 url scheme
        cfg.url = Some("ftp://x".into());
        assert!(cfg.validate().unwrap_err().contains("http"));

        // 显式 stdio 优先于 url 推断
        cfg.transport = Some("stdio".into());
        assert!(!cfg.is_http());
        assert!(cfg.validate().is_ok());

        // http 但 url 为空 → 报错
        let mut cfg = stdio_config();
        cfg.transport = Some("http".into());
        assert!(cfg.validate().unwrap_err().contains("url"));

        // 空名称
        let mut cfg = stdio_config();
        cfg.name = " ".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn local_path_extraction_covers_schema_and_nested_arguments() {
        let schema = json!({
            "type": "object",
            "properties": {
                "sourcePath": {"type": "string"},
                "profile": {"type": "object"},
                "options": {
                    "type": "object",
                    "properties": {
                        "outputDir": {"type": "string"},
                        "remoteUrl": {"type": "string"}
                    }
                }
            }
        });
        let paths = mcp_local_path_arguments(
            &schema,
            &json!({
                "sourcePath": "src/lib.rs",
                "profile": {"displayName": "developer"},
                "options": {
                    "outputDir": "target",
                    "remoteUrl": "https://example.com/file.txt"
                }
            }),
        )
        .unwrap();
        assert_eq!(
            paths,
            vec![
                LocalPathArgument {
                    path: "src/lib.rs".into(),
                    base: None
                },
                LocalPathArgument {
                    path: "target".into(),
                    base: None
                }
            ]
        );
    }

    #[test]
    fn local_path_extraction_fails_closed_for_path_like_objects() {
        let err = mcp_local_path_arguments(
            &json!({"type": "object", "properties": {"filePath": {"type": "object"}}}),
            &json!({"filePath": {"unexpected": "C:\\secret.txt"}}),
        )
        .unwrap_err();
        assert!(err.contains("filePath"));
    }

    // ── 握手 ──

    #[tokio::test]
    async fn initialize_rejects_unknown_version() {
        let (mut client, _) = client_with(vec![(
            "initialize",
            Ok(init_result("2099-01-01", json!({}))),
        )]);
        let err = client.initialize_legacy().await.unwrap_err();
        assert!(err.contains("不受支持的 Legacy 协议版本"));
    }

    #[tokio::test]
    async fn initialize_rejects_missing_version() {
        let (mut client, _) = client_with(vec![("initialize", Ok(json!({"capabilities": {}})))]);
        let err = client.initialize_legacy().await.unwrap_err();
        assert!(err.contains("protocolVersion"));
    }

    #[tokio::test]
    async fn initialize_captures_capabilities() {
        let (mut client, _) = client_with(vec![(
            "initialize",
            Ok(init_result(
                "2025-06-18",
                json!({"tools": {}, "resources": {}, "prompts": {}}),
            )),
        )]);
        client.initialize_legacy().await.unwrap();
        let caps = client.capabilities();
        assert!(caps.tools && caps.resources && caps.prompts);
    }

    // ── tools ──

    #[tokio::test]
    async fn list_tools_skipped_without_capability() {
        let (mut client, log) = client_with(vec![]);
        let tools = client.list_tools().await.unwrap();
        assert!(tools.is_empty());
        assert!(
            log.lock().unwrap().is_empty(),
            "未声明 tools 能力时不应发请求"
        );
    }

    #[tokio::test]
    async fn list_tools_paginates_and_dedups() {
        let (mut client, log) = client_with(vec![
            (
                "tools/list",
                Ok(json!({
                    "tools": [
                        {"name": "a"},
                        {"name": "b", "description": "d", "inputSchema": {"type": "object"},
                         "annotations": {"readOnlyHint": true}}
                    ],
                    "nextCursor": "p2"
                })),
            ),
            (
                "tools/list",
                Ok(json!({"tools": [{"name": "a"}, {"name": "c"}]})),
            ),
        ]);
        client.capabilities.tools = true;
        let tools = client.list_tools().await.unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"], "跨页合并并按 name 去重");
        assert!(tools[1].annotations.read_only);
        // 第二次请求必须携带 cursor
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].1, json!({}));
        assert_eq!(log[1].1, json!({"cursor": "p2"}));
    }

    #[tokio::test]
    async fn call_tool_surfaces_is_error() {
        let (mut client, _) = client_with(vec![(
            "tools/call",
            Ok(json!({"isError": true, "content": [{"type": "text", "text": "boom"}]})),
        )]);
        let err = client.call_tool("x", json!({})).await.unwrap_err();
        assert!(err.contains("boom"), "isError 必须转成 Err: {}", err);
    }

    #[tokio::test]
    async fn call_tool_falls_back_to_structured_content() {
        let (mut client, _) = client_with(vec![(
            "tools/call",
            Ok(json!({"structuredContent": {"a": 1}})),
        )]);
        let out = client.call_tool("x", json!({})).await.unwrap();
        assert!(out.contains("\"a\": 1"));
    }

    // ── resources / prompts ──

    #[tokio::test]
    async fn list_resources_skipped_without_capability() {
        let (mut client, log) = client_with(vec![]);
        assert!(client.list_resources().await.unwrap().is_empty());
        assert!(client.list_prompts().await.unwrap().is_empty());
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_resources_dedups_by_uri() {
        let (mut client, _) = client_with(vec![(
            "resources/list",
            Ok(json!({"resources": [
                {"uri": "u1", "name": "n1", "mimeType": "text/plain"},
                {"uri": "u1", "name": "dup"},
                {"uri": "u2", "description": "d2"}
            ]})),
        )]);
        client.capabilities.resources = true;
        let list = client.list_resources().await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].uri, "u1");
        assert_eq!(list[0].mime_type.as_deref(), Some("text/plain"));
    }

    #[tokio::test]
    async fn read_resource_joins_text_and_marks_blob() {
        let (mut client, _) = client_with(vec![(
            "resources/read",
            Ok(json!({"contents": [
                {"uri": "u", "text": "hello"},
                {"uri": "u", "blob": "QUJD"},
                {"uri": "u", "text": "world"}
            ]})),
        )]);
        let out = client.read_resource("u").await.unwrap();
        assert_eq!(out, "hello\n[binary 3 bytes (base64)]\nworld");
    }

    #[tokio::test]
    async fn get_prompt_formats_messages() {
        let (mut client, _) = client_with(vec![(
            "prompts/get",
            Ok(json!({"messages": [
                {"role": "user", "content": {"type": "text", "text": "hi"}},
                {"role": "assistant", "content": [{"type": "text", "text": "yo"}]}
            ]})),
        )]);
        let out = client.get_prompt("p", json!({})).await.unwrap();
        assert_eq!(out, "[user] hi\n\n[assistant] yo");
    }

    // ── 工具装配 ──

    #[test]
    fn annotations_map_to_risk_level() {
        let cfg = stdio_config();
        let mk = |ann: ToolAnnotations| {
            McpToolHandler::new(
                McpConnectionPool::new(),
                cfg.clone(),
                McpToolInfo {
                    name: "t".to_string(),
                    title: None,
                    description: String::new(),
                    schema: json!({}),
                    annotations: ann,
                },
            )
            .risk_level()
        };
        let ann = |r, d, i| ToolAnnotations {
            read_only: r,
            destructive: d,
            idempotent: i,
        };
        assert_eq!(mk(ann(true, false, false)), RiskLevel::Low);
        assert_eq!(mk(ann(false, false, false)), RiskLevel::Medium);
        assert_eq!(mk(ann(false, true, true)), RiskLevel::High);
        assert_eq!(mk(ToolAnnotations::default()), RiskLevel::High);
    }

    #[test]
    fn synthetic_tool_shapes() {
        let cfg = stdio_config();
        let pool = McpConnectionPool::new();

        let rl = McpResourceHandler::list(pool.clone(), cfg.clone());
        assert_eq!(rl.name(), "mcp_s__list_resources");
        assert_eq!(rl.risk_level(), RiskLevel::Low);

        let rr = McpResourceHandler::read(pool.clone(), cfg.clone());
        assert_eq!(rr.name(), "mcp_s__read_resource");
        assert_eq!(rr.parameters()["required"][0], "uri");

        let pl = McpPromptHandler::list(pool.clone(), cfg.clone());
        assert_eq!(pl.name(), "mcp_s__list_prompts");
        assert_eq!(pl.risk_level(), RiskLevel::Low);

        let pg = McpPromptHandler::get(pool.clone(), cfg.clone());
        assert_eq!(pg.name(), "mcp_s__get_prompt");
        assert_eq!(pg.risk_level(), RiskLevel::Medium);
        assert_eq!(pg.parameters()["required"][0], "name");
    }

    // ── Streamable HTTP 集成 ──

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// 迷你 Streamable HTTP 服务器：记录 (rpc方法/DELETE, 会话头)，按 method 分发响应
    async fn spawn_http_server() -> (String, Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_server = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(x) => x,
                    Err(_) => break,
                };
                let seen = seen_server.clone();
                tokio::spawn(async move {
                    // 读请求头（\r\n\r\n 为止）
                    let mut buf: Vec<u8> = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let header_end = loop {
                        let n = socket.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                            break pos + 4;
                        }
                    };
                    let header_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let content_length: usize = header_text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    let session = header_text.lines().find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("mcp-session-id:")
                            .map(|v| v.trim().to_string())
                    });
                    let request_line = header_text.lines().next().unwrap_or("").to_string();
                    // 读请求体
                    let mut body = buf[header_end..].to_vec();
                    while body.len() < content_length {
                        let n = socket.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&tmp[..n]);
                    }
                    body.truncate(content_length);

                    // DELETE：会话关闭
                    if request_line.starts_with("DELETE") {
                        seen.lock().unwrap().push(("DELETE".to_string(), session));
                        let _ = socket
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                            .await;
                        return;
                    }

                    let body_json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let rpc_method = body_json
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let id = body_json.get("id").cloned().unwrap_or(Value::Null);
                    seen.lock().unwrap().push((rpc_method.clone(), session));

                    let (status, ctype, extra, payload) = match rpc_method.as_str() {
                        "initialize" => (
                            "200 OK",
                            "application/json".to_string(),
                            vec![("Mcp-Session-Id".to_string(), "sess-123".to_string())],
                            json!({"jsonrpc": "2.0", "id": id, "result": {
                                "protocolVersion": "2025-06-18",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "mini", "version": "0"}
                            }})
                            .to_string(),
                        ),
                        "notifications/initialized" | "notifications/cancelled" => (
                            "202 Accepted",
                            "text/plain".to_string(),
                            vec![],
                            String::new(),
                        ),
                        "tools/list" => {
                            // 走 SSE 响应流，验证客户端流式解析
                            let msg = json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [
                                {"name": "echo", "description": "echo tool",
                                 "inputSchema": {"type": "object"},
                                 "annotations": {"readOnlyHint": true}}
                            ]}})
                            .to_string();
                            (
                                "200 OK",
                                "text/event-stream".to_string(),
                                vec![],
                                format!("event: message\ndata: {}\n\n", msg),
                            )
                        }
                        "tools/call" => (
                            "200 OK",
                            "application/json".to_string(),
                            vec![],
                            json!({"jsonrpc": "2.0", "id": id, "result": {
                                "content": [{"type": "text", "text": "pong"}]
                            }})
                            .to_string(),
                        ),
                        _ => (
                            "200 OK",
                            "application/json".to_string(),
                            vec![],
                            json!({"jsonrpc": "2.0", "id": id,
                                   "error": {"code": -32601, "message": "no such method"}})
                            .to_string(),
                        ),
                    };

                    let mut resp = format!(
                        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n",
                        status,
                        ctype,
                        payload.len()
                    );
                    for (k, v) in &extra {
                        resp.push_str(&format!("{}: {}\r\n", k, v));
                    }
                    resp.push_str("\r\n");
                    resp.push_str(&payload);
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });
        (format!("http://{}/mcp", addr), seen)
    }

    #[tokio::test]
    async fn http_transport_end_to_end() {
        let (url, seen) = spawn_http_server().await;
        let config = McpServerConfig {
            name: "web".to_string(),
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            timeout_secs: Some(5),
            transport: Some("http".to_string()),
            url: Some(url),
            headers: HashMap::new(),
        };

        let mut client = McpClient::connect(&config).await.unwrap();
        assert!(client.capabilities().tools);

        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert!(tools[0].annotations.read_only);

        let out = client.call_tool("echo", json!({})).await.unwrap();
        assert_eq!(out, "pong");

        // Drop 后应按规范后台发送 DELETE 结束会话（轮询等待异步任务）
        drop(client);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            {
                let s = seen.lock().unwrap();
                let methods: Vec<&str> = s.iter().map(|(m, _)| m.as_str()).collect();
                if methods.contains(&"DELETE") {
                    // dual-era：connect 先发 server/discover 探测，legacy 服务端回 -32601
                    // 后自动降级走 initialize 握手，故探测请求出现在序列最前
                    assert_eq!(
                        methods,
                        [
                            "server/discover",
                            "initialize",
                            "notifications/initialized",
                            "tools/list",
                            "tools/call",
                            "DELETE"
                        ],
                        "请求顺序与方法不符"
                    );
                    // initialize 之后的每个请求都必须回传会话 id
                    // （server/discover 探测发生在会话建立前，天然无 session id）
                    for (m, sid) in s.iter() {
                        if m != "initialize" && m != "server/discover" {
                            assert_eq!(sid.as_deref(), Some("sess-123"), "{} 未回传会话 id", m);
                        }
                    }
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "未收到 DELETE 会话关闭请求"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn http_transport_rejects_unknown_version() {
        // Legacy 服务端（server/discover 回 -32601）在 initialize 回传不受支持版本时，
        // connect 必须在握手阶段报错。dual-era 下请求分两条连接：
        // 连接 1 = modern 探测（server/discover），连接 2 = legacy initialize。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    let n = match socket.read(&mut buf).await {
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let payload = if req.contains("server/discover") {
                        json!({"jsonrpc": "2.0", "id": 1,
                               "error": {"code": -32601, "method": "no such method"}})
                        .to_string()
                    } else {
                        json!({"jsonrpc": "2.0", "id": 1, "result": {
                            "protocolVersion": "2099-01-01",
                            "capabilities": {},
                            "serverInfo": {"name": "mini", "version": "0"}
                        }})
                        .to_string()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        payload.len(),
                        payload
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });
        let config = McpServerConfig {
            name: "web".to_string(),
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            timeout_secs: Some(5),
            transport: Some("http".to_string()),
            url: Some(format!("http://{}/mcp", addr)),
            headers: HashMap::new(),
        };
        // McpClient 未实现 Debug，不能用 unwrap_err
        let err = match McpClient::connect(&config).await {
            Ok(_) => panic!("应拒绝不受支持的协议版本"),
            Err(e) => e,
        };
        assert!(
            err.contains("不受支持的 Legacy 协议版本"),
            "实际错误: {}",
            err
        );
    }
}
