//! 最小化 MCP (Model Context Protocol) 客户端
//!
//! 支持 stdio 传输（newline-delimited JSON-RPC 2.0）。每个 MCP 服务器
//! 通过子进程启动，握手后将其 tools 暴露为 Agent 工具。
//!
//! 自 `api_agent/mcp_client.rs` 迁入（工具架构统一 v1.0，轮 8），逻辑不变。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::tools::{RiskLevel, ToolHandler, ToolTag};

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
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());

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
            match self.read_line().await? {
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
            .request(
                "tools/call",
                json!({"name": name, "arguments": arguments}),
            )
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
        let _ = self.child.start_kill();
    }
}

/// 将某个 MCP 工具暴露为 Agent 工具（共享一个 McpClient，串行化调用）
pub struct McpToolHandler {
    client: Arc<Mutex<McpClient>>,
    full_name: String,
    raw_name: String,
    description: String,
    schema: Value,
}

impl McpToolHandler {
    pub fn new(
        client: Arc<Mutex<McpClient>>,
        server_name: &str,
        info: McpToolInfo,
    ) -> Self {
        Self {
            client,
            full_name: format!("mcp_{}_{}", server_name, info.name),
            raw_name: info.name,
            description: format!("[MCP:{}] {}", server_name, info.description),
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
        let mut client = self.client.lock().await;
        client.call_tool(&self.raw_name, arguments).await
    }
}
