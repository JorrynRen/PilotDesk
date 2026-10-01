//! 最小化 MCP (Model Context Protocol) 客户端
//!
//! 支持 stdio 传输（newline-delimited JSON-RPC 2.0）。每个 MCP 服务器
//! 通过子进程启动，握手后将其 tools 暴露为 Agent 工具。
//!
//! 自 `api_agent/mcp_client.rs` 迁入（工具架构统一 v1.0，轮 8），逻辑不变。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use crate::utils::process::{hidden_command, hidden_tokio_command};

/// MCP 服务器配置（持久化在 app_settings 的 `mcp_servers` key 下）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// 唯一名称，用于工具前缀
    pub name: String,
    /// 启动命令（如 `npx` / `node` / `uvx`）
    pub command: String,
    /// 命令参数（如 `["-y", "@modelcontextprotocol/server-filesystem", "/path"]`）
    pub args: Vec<String>,
}

/// MCP 工具描述信息（来自 tools/list）
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// MCP 客户端：管理一个 stdio 子进程，串行化 JSON-RPC 请求/响应
pub struct McpClient {
    _name: String,
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    pub async fn connect(config: &McpServerConfig) -> Result<Self, String> {
        let mut cmd = hidden_tokio_command(&config.command);
        cmd.args(&config.args);
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

        let mut client = Self {
            _name: config.name.clone(),
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        };

        client.initialize().await?;
        client.initialized().await?;
        Ok(client)
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

    /// 发送请求并等待匹配 id 的响应（跳过通知）
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write_line(&req.to_string()).await?;

        loop {
            // 内层读取超时：MCP 服务器长期无响应（卡死/断连未被察觉）时不让调用方无限阻塞。
            // 超时返回 Err（含"MCP 请求超时"），McpToolHandler::execute 的池逻辑照常 evict 并
            // 重连一次，外部可见行为不变。
            let read_result =
                tokio::time::timeout(std::time::Duration::from_secs(30), self.read_line())
                    .await
                    .map_err(|_| "MCP 请求超时（30 秒无响应）".to_string())?;
            match read_result? {
                None => return Err("MCP 服务器连接已关闭".to_string()),
                Some(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let v: Value = serde_json::from_str(line)
                        .map_err(|e| format!("解析 MCP 响应失败: {}", e))?;
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

    async fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "pilotdesk", "version": "0.1.0" }
            }),
        )
        .await?;
        Ok(())
    }

    async fn initialized(&mut self) -> Result<(), String> {
        let notif = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        self.write_line(&notif.to_string()).await
    }

    /// 列出服务器提供的工具
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, String> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        let mut infos = Vec::new();
        for t in tools {
            let name = t
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let description = t
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            let schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object"}));
            infos.push(McpToolInfo {
                name,
                description,
                schema,
            });
        }
        Ok(infos)
    }

    /// 调用指定工具，返回文本结果
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String, String> {
        let result = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;

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
            texts.push(result.to_string());
        }
        Ok(texts.join("\n"))
    }
}

impl Drop for McpClient {
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
        // 取舍说明：不在 Drop 内 block_on(child.wait()) 收尸——McpClient 通常在 tokio async
        // 上下文内被 drop（工具调用/evict 路径），此时 block_on 会 panic；spawn 时已设
        // kill_on_drop(true)，Child 句柄随其释放而关闭。Windows 无僵尸进程语义，taskkill 已
        // 同步等进程树退出，不残留；非 Windows 上 kill_on_drop 终止主进程，孙进程残留属已知
        // 平台限制（进程树 kill 需 await 收尸，Drop 无法承担）。
    }
}

/// 将某个 MCP 工具暴露为 Agent 工具（经连接池共享客户端，串行化调用 + 失败重连）。
pub struct McpToolHandler {
    pool: McpConnectionPool,
    server: McpServerConfig,
    full_name: String,
    raw_name: String,
    description: String,
    schema: Value,
}

impl McpToolHandler {
    pub fn new(pool: McpConnectionPool, server: McpServerConfig, info: McpToolInfo) -> Self {
        let full_name = format!("mcp_{}_{}", server.name, info.name);
        Self {
            pool,
            server,
            full_name: full_name.clone(),
            raw_name: info.name,
            description: format!("[MCP:{}] {}", full_name, info.description),
            schema: info.schema,
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
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Mcp]
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        // 懒连接（连接池缓存共享客户端）；连接中断时 evict 后重连一次，避免一次失败即永久失效。
        let mut attempt = 0u8;
        loop {
            let client = self.pool.get_or_connect(&self.server).await?;
            let result = {
                let mut c = client.lock().await;
                c.call_tool(&self.raw_name, arguments.clone()).await
            };
            match result {
                Ok(v) => return Ok(v),
                Err(_) if attempt == 0 => {
                    self.pool.evict(&self.server.name);
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

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
