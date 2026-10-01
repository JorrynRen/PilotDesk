//! 浏览器自动化工具（CDP 交互式升级版）。
//!
//! 自 v2 起从「命令行无头」升级为「CDP 真浏览器会话」：
//! - `navigate` / `fetch`：打开页面（含 JS 渲染），提取文本
//! - `snapshot`：带索引的交互元素快照（对齐 browser 技能快照格式，模型无需懂 CSS）
//! - `click` / `fill` / `select_option`：按索引交互
//! - `execute_script`：执行任意 JS
//! - `screenshot`：截图（支持整页）
//! - `upload_files`：文件上传（accept/multiple 校验）
//!
//! 实现：启动本机 Edge/Chrome（独立 `--user-data-dir` + `--remote-debugging-port`），
//! 经 tokio-tungstenite 直连 CDP WebSocket。单次操作 45s 超时；浏览器进程
//! 退出后自动重启并重试一次。
//!
//! 反爬边界：真浏览器内核可过基础反爬（UA/JS 校验），但无法绕过登录、验证码、
//! Cloudflare 挑战等高级反爬——遇此类页面应停止并提示用户，不承诺绕过。
//!
//! 保留公共原语（fetch_web 渲染 fallback 复用）：
//! `find_browser` / `run_browser` / `truncate_utf8`。
//!
//! HTML→文本有两个口径，别混用：
//! - `html_to_text`：**扁平纯文本**（删标签 + 块级换行 + 丢空行）。适合"喂给模型看一眼"的场景，
//!   例如 browser 工具的页面文本输出。
//! - `html_to_markdown`：**保结构的 markdown**（标题/列表/表格/围栏/链接 + 块间空行）。
//!   知识库的"提取正文"必须用这个 —— 它的分块器按 markdown 结构分段，
//!   喂扁平文本会把整页当成一个段落（详见 `html_to_markdown` 的说明）。

use crate::tools::{RiskLevel, ToolHandler};
use async_trait::async_trait;
use base64::Engine;
use futures::{Sink, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Child;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::tungstenite::Error as WsError;

/// 单次浏览器操作超时（秒）
const BROWSER_TIMEOUT_SECS: u64 = 45;
/// 抓取正文的最大输出字节数（超过则截断）
const MAX_TEXT_OUTPUT: usize = 16_000;
/// 快照内嵌的页面文本预览上限（字符）
const SNAPSHOT_TEXT_PREVIEW: usize = 3_000;

/// Windows 上给子进程加 CREATE_NO_WINDOW（避免拉起浏览器时弹黑框）；其他平台是空操作。
#[cfg(windows)]
fn no_console_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn no_console_window(_cmd: &mut std::process::Command) {}

/// 浏览器自动化工具
pub struct BrowserTool {
    cwd: String,
    session: tokio::sync::Mutex<Option<BrowserSession>>,
}

impl BrowserTool {
    pub fn new(cwd: String) -> Self {
        Self {
            cwd,
            session: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl ToolHandler for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "使用本机 Edge/Chrome 无头浏览器访问网页，真实渲染 JS 动态页。支持：\
         'navigate'/'fetch' 打开并读取页面内容（fetch 返回全文）、\
         'snapshot' 获取带索引的交互元素列表（按钮/输入框/链接等）、\
         'click' 按索引点击、'fill' 按索引填写输入框、'select_option' 下拉选择、\
         'execute_script' 执行 JS、'screenshot' 保存截图、'upload_files' 按索引上传文件。\
         适用于渲染后的动态页面、表单交互与截图存档。\
         反爬边界：可过基础校验，但无法绕过登录、验证码、Cloudflare 挑战等，\
         遇到此类页面请停止并告知用户，不要反复重试。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "fetch", "snapshot", "click", "fill", "select_option", "execute_script", "screenshot", "upload_files"],
                    "description": "操作类型：navigate=打开页面并返回快照；fetch=打开并返回全文；snapshot=获取当前页交互元素快照；click/fill/select_option/upload_files=按索引交互；execute_script=执行 JS；screenshot=保存截图"
                },
                "url": {
                    "type": "string",
                    "description": "要访问的 URL（navigate/fetch 需要，必须以 http:// 或 https:// 开头）"
                },
                "index": {
                    "type": "integer",
                    "description": "交互元素索引（来自 snapshot 的 [n] 编号；click/fill/select_option/upload_files 需要）"
                },
                "text": {
                    "type": "string",
                    "description": "要填入输入框的文本（fill 需要）"
                },
                "press_enter": {
                    "type": "boolean",
                    "description": "fill 后是否回车提交（可选，默认 false）"
                },
                "option_text": {
                    "type": "string",
                    "description": "下拉选项按可见文本匹配（select_option 三选一）"
                },
                "option_value": {
                    "type": "string",
                    "description": "下拉选项按 value 属性匹配（select_option 三选一）"
                },
                "option_index": {
                    "type": "integer",
                    "description": "下拉选项按位置匹配（select_option 三选一，0 起）"
                },
                "script": {
                    "type": "string",
                    "description": "要执行的 JavaScript 表达式（execute_script 需要）"
                },
                "path": {
                    "type": "string",
                    "description": "截图保存路径（仅 screenshot 需要；绝对或相对工作区路径，默认 pilotdesk_screenshot.png）"
                },
                "full_page": {
                    "type": "boolean",
                    "description": "截图是否整页（screenshot 可选，默认 false）"
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "上传的文件绝对路径列表（upload_files 需要）"
                }
            },
            "required": ["action"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Medium
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let action = arguments["action"]
            .as_str()
            .ok_or("缺少 action 参数")?
            .to_string();
        let cwd = self.cwd.clone();
        let mut guard = self.session.lock().await;
        // 首次调用：启动浏览器会话
        if guard.is_none() {
            *guard = Some(BrowserSession::launch().await?);
        }

        let first = run_action(guard.as_mut(), &action, &arguments, &cwd).await;
        match first {
            Ok(r) => Ok(r),
            Err(e) if connection_lost(&e) => {
                log::warn!("[browser] 连接失效，重启浏览器后重试: {}", e);
                let mut ns = BrowserSession::launch().await?;
                let retry = run_action(Some(&mut ns), &action, &arguments, &cwd).await;
                *guard = Some(ns);
                retry
            }
            Err(e) => Err(e),
        }
    }
}

/// 判断错误是否为连接失效（触发重启重试）
fn connection_lost(e: &str) -> bool {
    e.contains("CDP")
        || e.contains("WebSocket")
        || e.contains("已关闭")
        || e.contains("发送")
        || e.contains("连接")
}

/// 动作分发（共享给首次执行与重启重试）
async fn run_action(
    session: Option<&mut BrowserSession>,
    action: &str,
    args: &Value,
    cwd: &str,
) -> Result<String, String> {
    let s = session.ok_or_else(|| "浏览器会话未启动".to_string())?;
    match action {
        "navigate" => {
            let url = require_url(args)?;
            s.navigate_to(&url).await
        }
        "fetch" => {
            let url = require_url(args)?;
            s.navigate_to(&url).await?;
            s.body_text().await
        }
        "snapshot" => s.snapshot_text().await,
        "click" => {
            let i = args["index"].as_u64().ok_or("缺少 index 参数")?;
            s.click_at(i).await
        }
        "fill" => {
            let i = args["index"].as_u64().ok_or("缺少 index 参数")?;
            let text = args["text"].as_str().ok_or("缺少 text 参数")?;
            let enter = args["press_enter"].as_bool().unwrap_or(false);
            s.fill_at(i, text, enter).await
        }
        "select_option" => {
            let i = args["index"].as_u64().ok_or("缺少 index 参数")?;
            let ot = args["option_text"].as_str();
            let ov = args["option_value"].as_str();
            let oi = args["option_index"].as_i64();
            if ot.is_none() && ov.is_none() && oi.is_none() {
                return Err(
                    "select_option 需要 option_text / option_value / option_index 之一".to_string(),
                );
            }
            s.select_at(i, ot, ov, oi).await
        }
        "execute_script" => {
            let script = args["script"].as_str().ok_or("缺少 script 参数")?;
            s.run_script(script).await
        }
        "screenshot" => {
            let path = args["path"].as_str().unwrap_or("pilotdesk_screenshot.png");
            let full = args["full_page"].as_bool().unwrap_or(false);
            s.shot(cwd, path, full).await
        }
        "upload_files" => {
            let i = args["index"].as_u64().ok_or("缺少 index 参数")?;
            let files = args["files"]
                .as_array()
                .ok_or("缺少 files 参数（文件路径列表）")?
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>();
            if files.is_empty() {
                return Err("files 至少需要一个文件路径".to_string());
            }
            s.upload(i, &files).await
        }
        other => Err(format!("未知的浏览器操作: {}", other)),
    }
}

fn require_url(args: &Value) -> Result<String, String> {
    let url = args["url"].as_str().ok_or("缺少 url 参数")?.to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("url 必须以 http:// 或 https:// 开头".to_string());
    }
    Ok(url)
}

// ════════════════════════════════════════════════════════════════
// CDP 会话
// ════════════════════════════════════════════════════════════════

/// 浏览器进程 + CDP WebSocket 会话（延迟启动、进程退出自动重启）
struct BrowserSession {
    child: Option<Child>,
    user_data_dir: PathBuf,
    next_id: u64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    writer: Option<Pin<Box<dyn Sink<Message, Error = WsError> + Send>>>,
}

impl BrowserSession {
    /// 启动 Edge/Chrome（独立配置目录 + 随机调试端口），连接页面级 CDP WebSocket。
    async fn launch() -> Result<Self, String> {
        let browser = find_browser()
            .ok_or_else(|| "未检测到可用的 Chromium 系浏览器（Edge / Chrome / Chromium / Brave），无法执行浏览器自动化。请安装其中之一后重试。".to_string())?;
        let port = pick_free_port()?;
        let user_data_dir = std::env::temp_dir().join(format!(
            "pilotdesk_browser_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&user_data_dir)
            .map_err(|e| format!("创建浏览器配置目录失败: {}", e))?;

        let mut cmd = std::process::Command::new(&browser);
        cmd.args([
            "--headless",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-sync",
            "--disable-blink-features=AutomationControlled",
            &format!("--remote-debugging-port={}", port),
            &format!("--user-data-dir={}", user_data_dir.display()),
            "about:blank",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        no_console_window(&mut cmd);

        let child = cmd.spawn().map_err(|e| format!("启动浏览器失败: {}", e))?;

        let page_ws = wait_for_page_ws(port).await?;
        let (stream, _resp) = tokio_tungstenite::connect_async(&page_ws)
            .await
            .map_err(|e| format!("连接浏览器调试接口失败: {}", e))?;
        let (writer, reader) = stream.split();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        spawn_reader(reader, pending.clone());

        Ok(Self {
            child: Some(child),
            user_data_dir,
            next_id: 1,
            pending,
            writer: Some(Box::pin(writer)),
        })
    }

    /// 发送 CDP 命令并等待响应（45s 超时）
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let msg = Message::Text(json!({"id": id, "method": method, "params": params}).to_string());
        let writer = self.writer.as_mut().ok_or("浏览器连接未建立")?;
        writer
            .send(msg)
            .await
            .map_err(|e| format!("发送 CDP 消息失败: {}", e))?;
        writer
            .flush()
            .await
            .map_err(|e| format!("刷新 CDP 消息失败: {}", e))?;

        let resp = tokio::time::timeout(Duration::from_secs(BROWSER_TIMEOUT_SECS), rx)
            .await
            .map_err(|_| format!("CDP 调用超时: {}", method))?
            .map_err(|_| "CDP 通道已关闭".to_string())??;

        if let Some(err) = resp.get("error") {
            let msg = err["message"].as_str().unwrap_or("未知错误");
            return Err(format!("{} 失败: {}", method, msg));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    /// 执行 JS 表达式，返回结果值（异常转错误）
    async fn evaluate(&mut self, expression: &str) -> Result<Value, String> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "userGesture": true
                }),
            )
            .await?;
        if let Some(ex) = r.get("exceptionDetails") {
            let desc = ex["exception"]["description"]
                .as_str()
                .or_else(|| ex["text"].as_str())
                .unwrap_or("未知 JS 异常");
            return Err(format!("JS 执行失败: {}", desc));
        }
        Ok(r.get("result")
            .and_then(|v| v.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// 注入交互元素索引引导（幂等；navigate 后自动重建）
    async fn ensure_boot(&mut self) -> Result<(), String> {
        self.evaluate(BOOT_JS).await.map(|_| ())
    }

    /// 等待页面加载完成（最长 12s，SPA 卡在 interactive 时超时继续）
    async fn wait_load(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let state = self
                .evaluate("document.readyState")
                .await
                .unwrap_or_else(|_| Value::String("".into()));
            if state.as_str() == Some("complete") {
                break;
            }
            if Instant::now() > deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        Ok(())
    }

    /// 等正文"长稳"：`readyState=complete` 之后 SPA 往往还在拉数据渲染，直接取 DOM 只能拿到骨架。
    /// 连续两次读到同样的正文长度就当稳定（最长 `max_ms`）；正文一直为空也会在超时后返回。
    async fn wait_content_stable(&mut self, max_ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(max_ms);
        let mut last = usize::MAX;
        loop {
            let n = self
                .evaluate("document.body ? document.body.innerText.length : 0")
                .await
                .ok()
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            if n > 0 && n == last {
                break;
            }
            if Instant::now() > deadline {
                break;
            }
            last = n;
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    async fn page_title_url(&mut self) -> (String, String) {
        let title = self
            .evaluate("document.title")
            .await
            .and_then(|v| Ok(v.as_str().unwrap_or("").to_string()))
            .unwrap_or_default();
        let url = self
            .evaluate("location.href")
            .await
            .and_then(|v| Ok(v.as_str().unwrap_or("").to_string()))
            .unwrap_or_default();
        (title, url)
    }

    // ── 动作实现 ──

    async fn navigate_to(&mut self, url: &str) -> Result<String, String> {
        self.call("Page.navigate", json!({"url": url})).await?;
        self.wait_load().await?;
        self.snapshot_text().await
    }

    async fn body_text(&mut self) -> Result<String, String> {
        let v = self
            .evaluate("document.body ? document.body.innerText : ''")
            .await?;
        let text = v.as_str().unwrap_or("").to_string();
        if text.trim().is_empty() {
            return Err(
                "页面内容为空或无法解析（可能是需要登录、存在反爬或 JS 渲染超时）".to_string(),
            );
        }
        Ok(truncate_utf8(&text, MAX_TEXT_OUTPUT).to_string())
    }

    async fn snapshot_text(&mut self) -> Result<String, String> {
        self.ensure_boot().await?;
        let v = self.evaluate(SNAPSHOT_JS).await?;
        let (title, url) = self.page_title_url().await;
        let items = v["items"].as_array().cloned().unwrap_or_default();
        let mut lines = Vec::new();
        lines.push(format!("标题: {}", title));
        lines.push(format!("URL: {}", url));
        lines.push(format!("交互元素（{} 个）：", items.len()));
        for (idx, it) in items.iter().enumerate() {
            lines.push(format_element(idx, it));
        }
        // 页面文本预览（供快速判断内容，全文用 fetch）
        let body = self
            .evaluate("document.body ? document.body.innerText : ''")
            .await
            .and_then(|v| Ok(v.as_str().unwrap_or("").to_string()))
            .unwrap_or_default();
        if !body.trim().is_empty() {
            lines.push("---".to_string());
            lines.push("页面文本（预览）:".to_string());
            lines.push(truncate_utf8(&body, SNAPSHOT_TEXT_PREVIEW).to_string());
        }
        Ok(lines.join("\n"))
    }

    async fn click_at(&mut self, index: u64) -> Result<String, String> {
        self.ensure_boot().await?;
        self.evaluate(&format!(
            "(function(){{ var el = window.__PD_BROWSER__.get({}); el.scrollIntoView({{block:'center',inline:'nearest'}}); el.click(); return 'ok'; }})()",
            index
        ))
        .await?;
        self.snapshot_text().await
    }

    async fn fill_at(
        &mut self,
        index: u64,
        text: &str,
        press_enter: bool,
    ) -> Result<String, String> {
        self.ensure_boot().await?;
        let escaped = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
        self.evaluate(&format!(
            "(function(){{ var el = window.__PD_BROWSER__.get({}); el.focus(); el.value = {}; el.dispatchEvent(new Event('input',{{bubbles:true}})); el.dispatchEvent(new Event('change',{{bubbles:true}})); return 'ok'; }})()",
            index, escaped
        ))
        .await?;
        if press_enter {
            self.press_key("Enter").await?;
        }
        self.snapshot_text().await
    }

    /// 派发 Enter 键（补全虚拟键码以触发默认动作，如表单提交）
    async fn press_key(&mut self, key: &str) -> Result<(), String> {
        let meta = key_meta(key);
        for (event_type, params_extra) in [("keyDown", meta.0.clone()), ("keyUp", meta.1.clone())] {
            let mut params = json!({"type": event_type, "key": key});
            if let Some(m) = params_extra {
                params["code"] = json!(m.0);
                params["windowsVirtualKeyCode"] = json!(m.1);
                params["nativeVirtualKeyCode"] = json!(m.1);
            }
            if event_type == "keyDown" {
                if let Some(t) = meta.2 {
                    params["text"] = json!(t);
                }
            }
            self.call("Input.dispatchKeyEvent", params).await?;
        }
        Ok(())
    }

    async fn select_at(
        &mut self,
        index: u64,
        opt_text: Option<&str>,
        opt_value: Option<&str>,
        opt_index: Option<i64>,
    ) -> Result<String, String> {
        self.ensure_boot().await?;
        let t = opt_text.map(|s| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()));
        let v = opt_value.map(|s| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()));
        let i = opt_index.map(|n| n.to_string());
        let script = match (&t, &v, i) {
            (Some(tt), _, _) => format!(
                "(function(){{ var el = window.__PD_BROWSER__.get({}); if (el.tagName !== 'SELECT') throw new Error('元素不是下拉框'); var os = Array.from(el.options); var k = os.findIndex(function(o){{ return o.text.trim() === {}; }}); if (k < 0) throw new Error('未找到选项文本'); el.selectedIndex = k; el.dispatchEvent(new Event('change',{{bubbles:true}})); return 'ok'; }})()",
                index, tt
            ),
            (_, Some(vv), _) => format!(
                "(function(){{ var el = window.__PD_BROWSER__.get({}); if (el.tagName !== 'SELECT') throw new Error('元素不是下拉框'); var os = Array.from(el.options); var k = os.findIndex(function(o){{ return o.value === {}; }}); if (k < 0) throw new Error('未找到选项 value'); el.selectedIndex = k; el.dispatchEvent(new Event('change',{{bubbles:true}})); return 'ok'; }})()",
                index, vv
            ),
            (_, _, Some(ii)) => format!(
                "(function(){{ var el = window.__PD_BROWSER__.get({}); if (el.tagName !== 'SELECT') throw new Error('元素不是下拉框'); if ({} < 0 || {} >= el.options.length) throw new Error('选项索引超出范围'); el.selectedIndex = {}; el.dispatchEvent(new Event('change',{{bubbles:true}})); return 'ok'; }})()",
                index, ii, ii, ii
            ),
            // 调用方保证三者至少给了一个；真全空时给出明确错误，而不是 panic
            _ => return Err("选择下拉框需要提供文本、value 或索引之一".to_string()),
        };
        self.evaluate(&script).await?;
        self.snapshot_text().await
    }

    async fn run_script(&mut self, script: &str) -> Result<String, String> {
        let v = self.evaluate(script).await?;
        match v {
            Value::String(s) => Ok(s),
            Value::Null => Ok("(null)".to_string()),
            other => Ok(serde_json::to_string_pretty(&other).unwrap_or_default()),
        }
    }

    async fn shot(&mut self, cwd: &str, path: &str, full_page: bool) -> Result<String, String> {
        let abs = crate::resolve_workspace_path(path, cwd);
        if let Some(parent) = abs.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建截图目录失败 ({}): {}", parent.display(), e))?;
            }
        }
        let r = self
            .call(
                "Page.captureScreenshot",
                json!({"format": "png", "captureBeyondViewport": full_page}),
            )
            .await?;
        let b64 = r["data"].as_str().ok_or("截图响应缺少 data")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("截图数据解码失败: {}", e))?;
        std::fs::write(&abs, &bytes)
            .map_err(|e| format!("保存截图失败 ({}): {}", abs.display(), e))?;
        Ok(format!("截图已保存: {}", abs.display()))
    }

    async fn upload(&mut self, index: u64, files: &[String]) -> Result<String, String> {
        self.ensure_boot().await?;
        // 解析索引元素 → file input 序号 + 元信息（multiple/accept）
        let meta = self
            .evaluate(&format!(
                "(function(i){{ var el = window.__PD_BROWSER__.get(i); var input = null; \
                 var isFile = el instanceof HTMLInputElement && (el.type||'').toLowerCase() === 'file'; \
                 if (isFile) {{ input = el; }} \
                 else if (el.tagName === 'LABEL' && el.control instanceof HTMLInputElement && (el.control.type||'').toLowerCase() === 'file') {{ input = el.control; }} \
                 else if (el.closest) {{ var l = el.closest('label'); if (l && l.control instanceof HTMLInputElement && (l.control.type||'').toLowerCase() === 'file') {{ input = l.control; }} }} \
                 if (!input) {{ var ds = el.querySelectorAll ? Array.from(el.querySelectorAll('input[type=\"file\"]')) : []; if (ds.length === 1) input = ds[0]; }} \
                 if (!input) throw new Error('element_index=' + i + ' 不是文件上传控件'); \
                 var all = Array.from(document.querySelectorAll('input[type=\"file\"]')); \
                 return {{ k: all.indexOf(input), multiple: Boolean(input.multiple), accept: input.getAttribute('accept') || '' }}; \
                 }})({})",
                index
            ))
            .await?;
        let k = meta["k"].as_i64().ok_or("无法解析上传控件")?;
        let multiple = meta["multiple"].as_bool().unwrap_or(false);
        let accept = meta["accept"].as_str().unwrap_or("");

        // 校验：数量 + accept 扩展名/mime
        if files.len() > 1 && !multiple {
            return Err(format!(
                "当前上传控件不支持多文件（收到 {} 个）",
                files.len()
            ));
        }
        if !accept.trim().is_empty() {
            for f in files {
                if !accept_match(f, accept) {
                    return Err(format!("文件 {} 不符合上传控件 accept=\"{}\"", f, accept));
                }
            }
        }

        // DOM 定位 file input 并注入文件
        let doc = self.call("DOM.getDocument", json!({"depth": 0})).await?;
        let root_id = doc["root"]["nodeId"].as_i64().ok_or("无法获取页面根节点")?;
        let qr = self
            .call(
                "DOM.querySelectorAll",
                json!({"nodeId": root_id, "selector": "input[type=\"file\"]"}),
            )
            .await?;
        let ids = qr["nodeIds"].as_array().ok_or("查询上传控件失败")?;
        let node_id = ids
            .get(k as usize)
            .and_then(|v| v.as_i64())
            .ok_or("上传控件索引超出范围")?;
        self.call(
            "DOM.setFileInputFiles",
            json!({"nodeId": node_id, "files": files}),
        )
        .await?;
        Ok(format!(
            "已将 {} 个文件放入页面上传控件（未点击提交按钮）",
            files.len()
        ))
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.user_data_dir);
    }
}

/// 交互元素索引引导（幂等注入；click/fill/select/upload 复用同一选择器与过滤，保证索引一致）
const BOOT_JS: &str = r#"
(function () {
  if (window.__PD_BROWSER__) return;
  var SEL = 'a, button, input, select, textarea, [role="button"], [role="link"], [contenteditable="true"], [contenteditable=""]';
  function visible(el) {
    if (el.disabled) return false;
    var r = el.getBoundingClientRect();
    if (r.width <= 0 && r.height <= 0) return false;
    var cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden') return false;
    return true;
  }
  function collect() {
    return Array.from(document.querySelectorAll(SEL)).filter(visible);
  }
  function get(i) {
    var els = collect();
    if (i < 0 || i >= els.length) {
      throw new Error('element_index=' + i + ' 超出范围（当前 ' + els.length + ' 个），请先调用 snapshot 刷新');
    }
    return els[i];
  }
  window.__PD_BROWSER__ = { get: get, collect: collect };
})();
"#;

/// 生成交互元素快照（依赖 BOOT_JS 已注入）
const SNAPSHOT_JS: &str = r#"
(function () {
  var els = window.__PD_BROWSER__.collect();
  return {
    count: els.length,
    items: els.map(function (el) {
      var tag = el.tagName.toLowerCase();
      return {
        tag: tag,
        type: (el.getAttribute('type') || '').toLowerCase(),
        name: el.name || '',
        id: el.id || '',
        placeholder: el.placeholder || '',
        href: (tag === 'a' ? (el.getAttribute('href') || '') : ''),
        value: (el.value !== undefined ? String(el.value).slice(0, 50) : ''),
        text: (el.innerText || el.value || el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 100),
        role: el.getAttribute('role') || ''
      };
    })
  };
})()
"#;

/// 格式化单个交互元素为 "[i] tag ... | 文本" 行
fn format_element(idx: usize, it: &Value) -> String {
    let tag = it["tag"].as_str().unwrap_or("");
    let mut s = format!("[{}] {}", idx, tag);
    if let Some(t) = it["type"].as_str() {
        if !t.is_empty() {
            s.push_str(&format!(" type=\"{}\"", t));
        }
    }
    if let Some(n) = it["name"].as_str() {
        if !n.is_empty() {
            s.push_str(&format!(" name=\"{}\"", n));
        }
    }
    if let Some(p) = it["placeholder"].as_str() {
        if !p.is_empty() {
            s.push_str(&format!(" placeholder=\"{}\"", p));
        }
    }
    if let Some(h) = it["href"].as_str() {
        if !h.is_empty() {
            s.push_str(&format!(" href=\"{}\"", h));
        }
    }
    if let Some(r) = it["role"].as_str() {
        if !r.is_empty() {
            s.push_str(&format!(" role=\"{}\"", r));
        }
    }
    let txt = it["text"].as_str().unwrap_or("");
    if !txt.is_empty() {
        s.push_str(&format!(" | {}", txt));
    }
    s
}

/// Enter 键元信息：(keyDown 补充, keyUp 补充, keyDown text)
fn key_meta(
    key: &str,
) -> (
    Option<(String, u32)>,
    Option<(String, u32)>,
    Option<&'static str>,
) {
    let meta: Option<(&str, u32, &str)> = match key {
        "Enter" => Some(("Enter", 13, "\r")),
        "Tab" => Some(("Tab", 9, "\t")),
        "Backspace" => Some(("Backspace", 8, "")),
        "Delete" => Some(("Delete", 46, "")),
        "Escape" => Some(("Escape", 27, "")),
        "ArrowUp" => Some(("ArrowUp", 38, "")),
        "ArrowDown" => Some(("ArrowDown", 40, "")),
        _ => None,
    };
    match meta {
        Some((code, vk, text)) => {
            let extra = (code.to_string(), vk);
            let text_opt = if text.is_empty() { None } else { Some(text) };
            (Some(extra.clone()), Some(extra), text_opt)
        }
        None => (None, None, None),
    }
}

/// 启动 reader task：分发响应到 pending，连接关闭时清空 pending
fn spawn_reader(
    mut stream: futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
) {
    tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                let _ = tx.send(Ok(v));
                            }
                        }
                    }
                }
                Ok(Message::Binary(bin)) => {
                    if let Ok(text) = String::from_utf8(bin.to_vec()) {
                        if let Ok(v) = serde_json::from_str::<Value>(&text) {
                            if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                                if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                    let _ = tx.send(Ok(v));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::debug!("[browser] CDP 流错误: {}", e);
                    break;
                }
                _ => {}
            }
        }
        let mut map = pending.lock().unwrap();
        for (_, tx) in map.drain() {
            let _ = tx.send(Err("CDP 连接已关闭".to_string()));
        }
    });
}

/// 占用一个空闲 TCP 端口作为调试端口（Chrome 随后绑定，竞态概率极低）
fn pick_free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("分配调试端口失败: {}", e))?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

/// 轮询浏览器调试接口（最长 15s）获取页面级 WebSocket URL
async fn wait_for_page_ws(port: u16) -> Result<String, String> {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(resp) = client
            .get(format!("http://127.0.0.1:{}/json/list", port))
            .send()
            .await
        {
            if let Ok(list) = resp.json::<Value>().await {
                if let Some(arr) = list.as_array() {
                    for t in arr {
                        if t["type"].as_str() == Some("page") {
                            if let Some(u) = t["webSocketDebuggerUrl"].as_str() {
                                return Ok(u.to_string());
                            }
                        }
                    }
                }
            }
        }
        if Instant::now() > deadline {
            return Err(format!(
                "浏览器调试端口 {} 未就绪（启动失败或被占用）",
                port
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 校验文件路径是否符合 input accept（扩展名 / mime 前缀匹配，`*/*` 通配）
fn accept_match(path: &str, accept: &str) -> bool {
    let name = path
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_lowercase();
    for raw in accept.split(',') {
        let token = raw.trim().to_lowercase();
        if token.is_empty() {
            continue;
        }
        if token.starts_with('.') && name.ends_with(&token) {
            return true;
        }
        if token == "*/*" {
            return true;
        }
        // mime 前缀匹配（如 image/*）
        if token.ends_with("/*") {
            let prefix = &token[..token.len() - 1];
            if let Some(ext) = name.rsplit('.').next() {
                let mime = match ext {
                    "png" => "image/",
                    "jpg" | "jpeg" => "image/",
                    "gif" => "image/",
                    "webp" => "image/",
                    "bmp" => "image/",
                    "svg" => "image/",
                    "pdf" => "application/",
                    "txt" => "text/",
                    "md" => "text/",
                    "json" => "application/",
                    "html" | "htm" => "text/",
                    "csv" => "text/",
                    "doc" | "docx" => "application/",
                    "xls" | "xlsx" => "application/",
                    "ppt" | "pptx" => "application/",
                    "zip" => "application/",
                    "mp4" => "video/",
                    "mp3" => "audio/",
                    _ => "",
                };
                if mime.starts_with(prefix) {
                    return true;
                }
            }
        }
    }
    false
}

// ════════════════════════════════════════════════════════════════
// 公共原语（fetch_web 渲染 fallback 复用，保持签名不变）
// ════════════════════════════════════════════════════════════════

/// 用应用内无头浏览器**渲染**页面并取回 DOM（`documentElement.outerHTML`）。
///
/// 给知识库的"提取正文"用：普通 HTTP 只能拿到 JS 空壳时改走这条。
/// **一次性会话**（启动 → 导航 → 取值 → 结束，进程与临时配置目录由 `Drop` 清掉），
/// 与 `BrowserTool` 的长会话无关 —— 那个是给模型交互式操作用的，不该被一次抓取占住。
///
/// 能力边界与 `BrowserTool` 一致：真内核能过基础的 UA/JS 校验，
/// 但**过不了滑块 / 验证码这类需要人机交互的挑战**（实测百度百家号就是如此，旧版
/// `--headless` 与 `--headless=new` 都同样吃滑块页）—— 调用方要识别并明确报错，不要反复重试。
///
/// 用 `Page.navigate` + `wait_load` 而不复用 `navigate_to`：后者会顺带产出一份交互元素快照
/// （页面上千个链接时要逐个格式化），对"只要 DOM"的场合是纯浪费。
pub(crate) async fn fetch_rendered_html(url: &str, max_bytes: usize) -> Result<String, String> {
    let mut s = BrowserSession::launch().await?;
    s.call("Page.navigate", json!({ "url": url })).await?;
    s.wait_load().await?;
    s.wait_content_stable(2000).await;
    let v = s
        .evaluate("document.documentElement ? document.documentElement.outerHTML : ''")
        .await?;
    let html = v.as_str().unwrap_or("").to_string();
    if html.trim().is_empty() {
        return Err("渲染后仍未取到页面内容".to_string());
    }
    if html.len() > max_bytes {
        return Err(format!(
            "渲染后的页面超过 {} MB，请改用片段投喂",
            max_bytes / 1024 / 1024
        ));
    }
    Ok(html)
}

/// 定位本机可用的 Chromium 系浏览器（Edge / Chrome / Chromium / Brave）
///
/// 三平台的安装位置差异很大，所以分平台给候选：Windows 是固定安装目录，
/// macOS 都在 `/Applications/*.app/Contents/MacOS/` 下；Linux 各发行版太散
/// （`/usr/bin`、`/snap/bin`、`/opt/...` 都有），不放绝对路径，统一交给 PATH。
pub(crate) fn find_browser() -> Option<String> {
    #[cfg(windows)]
    const ABSOLUTE_CANDIDATES: &[&str] = &[
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];

    #[cfg(target_os = "macos")]
    const ABSOLUTE_CANDIDATES: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ];

    #[cfg(not(any(windows, target_os = "macos")))]
    const ABSOLUTE_CANDIDATES: &[&str] = &[];

    for c in ABSOLUTE_CANDIDATES {
        if std::path::Path::new(c).is_file() {
            return Some(c.to_string());
        }
    }

    #[cfg(windows)]
    const NAMES: &[&str] = &["msedge", "chrome"];

    #[cfg(target_os = "macos")]
    const NAMES: &[&str] = &[
        "google-chrome",
        "chromium",
        "microsoft-edge",
        "brave-browser",
    ];

    #[cfg(not(any(windows, target_os = "macos")))]
    const NAMES: &[&str] = &[
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "microsoft-edge-stable",
        "brave-browser",
    ];

    // 回退：在 PATH 中查找。Windows 用 `where`，Unix 用 `which`，两者都是逐行输出路径。
    #[cfg(windows)]
    const WHICH: &str = "where";
    #[cfg(not(windows))]
    const WHICH: &str = "which";

    for name in NAMES {
        if let Ok(out) = std::process::Command::new(WHICH)
            .arg(name)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                let p = line.trim();
                if !p.is_empty() && std::path::Path::new(p).is_file() {
                    return Some(p.to_string());
                }
            }
        }
    }
    None
}

/// 运行浏览器子进程，返回 (stdout, stderr, exit_code)，带超时兜底
pub(crate) fn run_browser(
    browser: &str,
    args: &[String],
) -> Result<(Vec<u8>, Vec<u8>, Option<i32>), String> {
    let mut cmd = std::process::Command::new(browser);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    no_console_window(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| format!("启动浏览器失败: {}", e))?;

    // 将 stdout/stderr 移到读取线程，避免大输出（如 dump-dom）填满管道缓冲区导致子进程阻塞
    let stdout = child.stdout.take().expect("stdout 已 piped");
    let stderr = child.stderr.take().expect("stderr 已 piped");
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::BufReader::new(stdout).read_to_end(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::BufReader::new(stderr).read_to_end(&mut buf);
        buf
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(BROWSER_TIMEOUT_SECS);
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "浏览器操作超时（{} 秒），已强制终止",
                        BROWSER_TIMEOUT_SECS
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("浏览器进程异常: {}", e));
            }
        }
    };

    let stdout_buf = stdout_handle.join().unwrap_or_default();
    let stderr_buf = stderr_handle.join().unwrap_or_default();
    Ok((stdout_buf, stderr_buf, exit_code))
}

/// 将 HTML 转换为可读纯文本（去除脚本/样式/标签，解码常见实体，折叠空白）
pub(crate) fn html_to_text(html: &str) -> String {
    // 去除 script/style/noscript 及其内容（Rust regex 不支持反向引用，逐类匹配）
    let mut html = html.to_string();
    for tag in ["script", "style", "noscript"] {
        let re = regex::Regex::new(&format!(r"(?is)<{}[^>]*>.*?</{}>", tag, tag)).unwrap();
        html = re.replace_all(&html, " ").to_string();
    }
    // 去除注释
    let re_comment = regex::Regex::new(r"(?s)<!--.*?-->").unwrap();
    let html = re_comment.replace_all(&html, "");
    // 块级元素换行
    let re_br = regex::Regex::new(
        r"(?i)<(br\s*/?>|/p>|/div>|/li>|/tr>|/h[1-6]>|/section>|/article>|/blockquote>)",
    )
    .unwrap();
    let html = re_br.replace_all(&html, "\n");
    // 去除所有剩余标签
    let re_tag = regex::Regex::new(r"<[^>]+>").unwrap();
    let text = re_tag.replace_all(&html, "");

    // 解码常见 HTML 实体
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ");

    // 逐行折叠空白并丢弃空行
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !trimmed.is_empty() {
            out.push_str(&trimmed);
            out.push('\n');
        }
    }
    out
}

/// 去掉 `<script>` / `<style>` / `<noscript>` / `<svg>` / `<template>` / 注释块。
///
/// 用字面扫描而不是 DOM 变更 API：对大小写做 ASCII 小写映射（**字节长度不变，下标可直接复用**），
/// 这样只需要把整段小写化一次，后续都在这份映射上找位置 —— 每次查找都重新小写化的话，
/// 一个几 MB 的页面遇上几十个 `<script>` 就是几十倍的无谓开销。
///
/// 放在这里（而不是调用方）是因为它是**提取层的内部步骤**：`html_to_markdown` 自己就会调它，
/// 调用方不需要知道"要先剔脚本"这件事 —— 忘了调就等于把 JS 灌进知识库。
pub(crate) fn strip_script_style(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < html.len() {
        // 找下一个"要丢的块"起点
        let mut best: Option<(usize, usize)> = None; // (起始, 标签名长度)
        for tag in ["script", "style", "noscript", "svg", "template"] {
            let pat = format!("<{}", tag);
            if let Some(p) = lower[i..].find(&pat) {
                let at = i + p;
                // 必须是真正的标签（`<script>` / `<script ` / `<script/`），不是 `<scriptfoo`
                let next = lower
                    .as_bytes()
                    .get(at + pat.len())
                    .copied()
                    .unwrap_or(b'>');
                if matches!(next, b'>' | b' ' | b'\n' | b'\t' | b'\r' | b'/') {
                    if best.map(|(b, _)| at < b).unwrap_or(true) {
                        best = Some((at, pat.len()));
                    }
                }
            }
        }
        // 注释块
        if let Some(p) = lower[i..].find("<!--") {
            let at = i + p;
            if best.map(|(b, _)| at < b).unwrap_or(true) {
                out.push_str(&html[i..at]);
                i = match lower[at..].find("-->") {
                    Some(c) => at + c + 3,
                    None => html.len(),
                };
                continue;
            }
        }
        match best {
            Some((at, pat_len)) => {
                out.push_str(&html[i..at]);
                let close = format!("</{}>", &lower[at + 1..at + pat_len]);
                i = match lower[at..].find(&close) {
                    Some(c) => at + c + close.len(),
                    // 没有闭合标签：从这里到结尾都不要（避免把剩下的正文也留下）。
                    // 浏览器对畸形的 `<script>` 也是这个行为（一直吃到文件结束）
                    None => html.len(),
                };
            }
            None => {
                out.push_str(&html[i..]);
                break;
            }
        }
    }
    out
}

/// 把 HTML 转成 **markdown**（保结构），供知识库的"提取正文"使用。
///
/// 与 `html_to_text` 的关键区别是**不把结构丢掉**。这不是为了好看：
///   1. 知识库的分块器 `split_blocks_ranged` 是**按 markdown 结构**写的 —— 它认 `#` 标题
///      （还会把标题粘到后继块）、围栏代码、markdown 表格、列表项，并按**空行**分段。
///      喂给它"丢掉空行的扁平文本"，整页会被当成**一个段落**，于是 AI 整理的按字符分批
///      永远只有一批，页面一大就必然超限、必然降级回原始噪声文本；
///   2. 模型拿到带结构的输入，去噪与分节的准确度明显更高（能区分正文段落与导航残留）；
///   3. 落盘的 `.md` 才算名副其实 —— 用户点「打开原文」看到的是能读的东西。
///
/// 只做"搬运"不做"判断"：脚本 / 样式 / `<head>` 这些**显然不是正文**的部分剔掉，
/// 至于"哪段是广告、哪段是相关阅读"交给 AI 整理那一层 —— 提取层替它做判断只会把正文一起误删。
///
/// 已知取舍：`<blockquote>` 不转成 `> ` 引用、`<img>` 直接丢弃、不做 markdown 转义
/// （原文里的 `*`/`[` 可能与生成的标记互相干扰，但网页正文里极少见）；
/// 列表项内部的多个段落会被并成一行（宁可少一个分段，也不能把列表结构拆坏）。
pub(crate) fn html_to_markdown(html: &str) -> String {
    let html = strip_script_style(html);
    let mut s = Md::default();
    let mut i = 0usize;
    while i < html.len() {
        // 文本段：一路吃到下一个 '<'
        let next = html[i..].find('<').map(|p| i + p).unwrap_or(html.len());
        if next > i {
            s.text(&decode_entities(&html[i..next]));
            i = next;
            continue;
        }
        // 注释（`strip_script_style` 已去掉，这里只是兜底：注释里可能含 '>'，不能当普通标签扫）
        if html[i..].starts_with("<!--") {
            i = html[i..]
                .find("-->")
                .map(|p| i + p + 3)
                .unwrap_or(html.len());
            continue;
        }
        let Some(gt) = find_tag_end(&html, i) else {
            break;
        };
        s.tag(&html[i + 1..gt]);
        i = gt + 1;
    }
    normalize_md(&s.out)
}

/// 标签结束位置：属性值里可能出现 `>`（如 `title="a>b"`），所以要带引号状态扫描
fn find_tag_end(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = start + 1;
    while i < s.len() {
        let b = bytes[i];
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => {
                if b == b'"' || b == b'\'' {
                    quote = Some(b);
                } else if b == b'>' {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// HTML→Markdown 的转换状态机
#[derive(Default)]
struct Md {
    out: String,
    /// 正在 `<head>` 里：title/meta 不是正文，全部忽略
    into_head: bool,
    /// `<pre>` 内：内容原样保留，不折叠空白、不解释内联标签
    in_pre: bool,
    /// 列表项嵌套深度：> 0 时禁止块级空行（否则一个列表项会被拆成两段）
    in_li: usize,
    /// 列表栈：None = `<ul>`，Some(n) = `<ol>` 的当前序号
    list_stack: Vec<Option<u32>>,
    /// 当前 `<a>` 的 href（`None` = 这个链接不该转成 markdown，例如纯锚点）
    link: Option<String>,
    /// 表格缓冲（渲染时要先知道列数、以及首行是不是表头）
    table: Option<MdTable>,
    /// 当前单元格内容（Some = 正在某个单元格里，文本要写到这里而不是 out）
    cell: Option<String>,
}

#[derive(Default)]
struct MdTable {
    rows: Vec<(bool, Vec<String>)>,
    cur: Vec<String>,
    /// 当前行出现过 `<th>`
    cur_th: bool,
}

impl Md {
    fn sink_push(&mut self, s: &str) {
        match &mut self.cell {
            Some(c) => c.push_str(s),
            None => self.out.push_str(s),
        }
    }

    /// 块级分隔：留一个空行。列表项内、表格内都不允许（那会把一个项/一个单元格拆成两段）
    fn block_break(&mut self) {
        if self.in_li > 0 || self.table.is_some() || self.out.is_empty() {
            return;
        }
        if self.out.ends_with("\n\n") {
            return;
        }
        if self.out.ends_with('\n') {
            self.out.push('\n');
        } else {
            self.out.push_str("\n\n");
        }
    }

    /// 文本片段。`<pre>` 内原样保留，其余折叠空白
    fn text(&mut self, raw: &str) {
        if self.into_head || raw.is_empty() {
            return;
        }
        if self.in_pre {
            self.sink_push(raw);
            return;
        }
        // 表格标记之间的换行/缩进不是内容（单元格内的文本走 cell，不会到这里）
        if self.table.is_some() && self.cell.is_none() {
            return;
        }
        let collapsed = collapse_ws(raw);
        if collapsed.is_empty() {
            return;
        }
        // 片段首尾的空格要和已写出的内容合并，否则会积出多余空格
        let ends_open = self.out.is_empty() || self.out.ends_with(' ') || self.out.ends_with('\n');
        let s = if ends_open {
            collapsed.trim_start()
        } else {
            collapsed.as_str()
        };
        if s.is_empty() {
            return;
        }
        self.sink_push(s);
    }

    fn render_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        self.cell = None;
        if t.rows.is_empty() {
            return;
        }
        let cols = t.rows.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        self.block_break();
        let line = |cells: &[String], out: &mut String| {
            out.push('|');
            for i in 0..cols {
                let c = cells.get(i).map(String::as_str).unwrap_or("");
                out.push(' ');
                out.push_str(c);
                out.push_str(" |");
            }
            out.push('\n');
        };
        // HTML 表格常常只用 `<td>`、没有 `<th>`；那就补一行空表头，
        // 而不是把第一行数据当成表头（那会凭空改变内容含义）。
        // 注意顺序：**表头 → 分隔行 → 数据行**，写反了整张表就不是表格了
        let body_from = if t.rows[0].0 {
            line(&t.rows[0].1, &mut self.out);
            1
        } else {
            line(&vec![String::new(); cols], &mut self.out);
            0
        };
        line(&vec!["---".to_string(); cols], &mut self.out);
        for (_, cells) in &t.rows[body_from..] {
            line(cells, &mut self.out);
        }
    }

    fn tag(&mut self, inner: &str) {
        let inner = inner.trim();
        let closing = inner.starts_with('/');
        let body = if closing {
            inner[1..].trim_start()
        } else {
            inner
        };
        let name_len = body
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(body.len());
        if name_len == 0 {
            return; // `<!DOCTYPE html>` / `<?xml …?>` / `<![CDATA[…` 之类
        }
        let name = body[..name_len].to_ascii_lowercase();

        // `<head>` 里的 title/meta 不是正文；用标志位而不是"跳到 `</head>`"，
        // 因为缺失闭合标签时跳转型会直接把整页吃掉。`<body>` 也作为退出信号
        if name == "head" || name == "body" {
            self.into_head = name == "head" && !closing;
            return;
        }
        if self.into_head {
            return;
        }

        if self.in_pre {
            if name == "br" {
                self.sink_push("\n");
            } else if name == "pre" && closing {
                self.in_pre = false;
                self.sink_push("\n```");
                self.block_break();
            }
            // `<pre>` 内的其它标签（含 <code>）一律忽略
            return;
        }

        match (name.as_str(), closing) {
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                self.block_break();
                let level = name[1..].parse::<usize>().unwrap_or(1);
                self.sink_push(&format!("{} ", "#".repeat(level)));
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => self.block_break(),

            (
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside"
                | "nav" | "blockquote" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "form"
                | "fieldset" | "caption" | "address",
                false,
            ) => self.block_break(),
            (
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside"
                | "nav" | "blockquote" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "form"
                | "fieldset" | "caption" | "address",
                true,
            ) => self.block_break(),

            ("br", false) => {
                // markdown 的硬换行：行尾反斜杠（用两个空格的话会被收尾清理吃掉）。
                // 单元格里不能换行 —— markdown 表格的单元格必须是单行
                if self.cell.is_none() {
                    self.sink_push("\\\n");
                }
            }
            ("hr", false) => {
                self.block_break();
                self.sink_push("---");
                self.block_break();
            }
            ("pre", false) => {
                self.block_break();
                self.in_pre = true;
                self.sink_push("```\n");
            }

            ("ul" | "ol", false) => {
                self.block_break();
                self.list_stack
                    .push(if name == "ol" { Some(0) } else { None });
            }
            ("ul" | "ol", true) => {
                self.list_stack.pop();
                self.block_break();
            }
            ("li", false) => {
                self.in_li += 1;
                if !self.out.is_empty() && !self.out.ends_with('\n') {
                    self.out.push('\n');
                }
                let depth = self.list_stack.len().saturating_sub(1);
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", n)
                    }
                    _ => "- ".to_string(),
                };
                self.out.push_str(&"  ".repeat(depth));
                self.out.push_str(&marker);
            }
            // `</li>` 不写空行：紧挨着的下一个 `<li>` 会补一个单换行，
            // 这样得到的是紧凑列表（项之间有空行会变成 loose list，也更占地方）
            ("li", true) => self.in_li = self.in_li.saturating_sub(1),

            ("table", false) => {
                self.block_break();
                self.table = Some(MdTable::default());
            }
            ("table", true) => self.render_table(),
            ("tr", false) => {
                if let Some(t) = &mut self.table {
                    t.cur.clear();
                    t.cur_th = false;
                }
            }
            ("tr", true) => {
                if let Some(t) = &mut self.table {
                    let row = (t.cur_th, std::mem::take(&mut t.cur));
                    t.rows.push(row);
                }
            }
            ("td" | "th", false) => {
                // 前一个单元格没闭合时先收掉，别把两格并成一格
                self.flush_cell();
                if name == "th" {
                    if let Some(t) = &mut self.table {
                        t.cur_th = true;
                    }
                }
                self.cell = Some(String::new());
            }
            ("td" | "th", true) => self.flush_cell(),

            ("a", false) => {
                let href = attr_value(inner, "href").unwrap_or_default();
                if href.is_empty() || href.starts_with('#') {
                    self.link = None;
                } else {
                    self.link = Some(href);
                    self.sink_push("[");
                }
            }
            ("a", true) => {
                if let Some(href) = self.link.take() {
                    self.sink_push(&format!("]({})", href.trim()));
                }
            }
            ("strong" | "b", false) => self.sink_push("**"),
            ("strong" | "b", true) => self.sink_push("**"),
            ("em" | "i", false) => self.sink_push("*"),
            ("em" | "i", true) => self.sink_push("*"),
            ("code", false) => self.sink_push("`"),
            ("code", true) => self.sink_push("`"),
            _ => {}
        }
    }

    fn flush_cell(&mut self) {
        let Some(c) = self.cell.take() else { return };
        // 单元格必须是单行、且不能含裸 '|'（那会被解析成列分隔符）
        let one_line = collapse_ws(&c.replace('\n', " "))
            .trim()
            .replace('|', "\\|");
        if let Some(t) = &mut self.table {
            t.cur.push(one_line);
        }
    }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(ch);
    }
    // 片段开头的空白要不要保留由调用方决定（跨片段合并时用），这里保留一个标记性空格
    if space {
        out.push(' ');
    }
    out
}

/// 取标签属性值（`href="…"` / `href='…'` / `href=…`）
fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(p) = lower[from..].find(attr) {
        let at = from + p;
        from = at + attr.len();
        // 必须是独立的属性名（前面不是字母数字，后面跟 '='）
        let before_ok = at == 0 || !tag.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        if !before_ok {
            continue;
        }
        let bytes = rest.as_bytes();
        if bytes.is_empty() {
            return None;
        }
        return match bytes[0] {
            q @ (b'"' | b'\'') => {
                let end = rest[1..]
                    .find(q as char)
                    .map(|e| e + 1)
                    .unwrap_or(rest.len());
                Some(rest[1..end].to_string())
            }
            _ => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                Some(rest[..end].trim_end_matches('/').to_string())
            }
        };
    }
    None
}

/// 解码 HTML 实体：命名实体走小表，数字实体（含十六进制）一律支持。
///
/// 数字实体必须支持 —— 页面里的中文标点常被写成 `&#12289;`（、）、`&#x3002;`（。）这类，
/// 不解码就是正文里一串 "&#12289;"，既难读也会污染检索与标签。
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        if s.as_bytes()[i] == b'&' {
            // 只在前 12 个字符内找 ';'：实体都很短，避免把一整段文本当成实体扫描
            let semi = s[i..]
                .char_indices()
                .take(12)
                .find(|(_, c)| *c == ';')
                .map(|(o, _)| i + o);
            if let Some(semi) = semi {
                if let Some(ch) = entity_char(&s[i + 1..semi]) {
                    out.push(ch);
                    i = semi + 1;
                    continue;
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn entity_char(name: &str) -> Option<char> {
    if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    if let Some(dec) = name.strip_prefix('#') {
        return dec.parse::<u32>().ok().and_then(char::from_u32);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" | "ensp" | "emsp" | "thinsp" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "middot" => '·',
        "ldquo" => '“',
        "rdquo" => '”',
        "lsquo" => '‘',
        "rsquo" => '’',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "sect" => '§',
        "para" => '¶',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "le" => '≤',
        "ge" => '≥',
        "ne" => '≠',
        "sup2" => '²',
        "sup3" => '³',
        "frac12" => '½',
        "yen" => '¥',
        "euro" => '€',
        "pound" => '£',
        _ => return None,
    })
}

/// 收尾：每行去尾空格、连续空行压到一个、去掉首尾空行。
///
/// 转换过程中是"宁可多写分隔符"（简单且不会漏），统一在这里收敛 ——
/// 中途各处都做精确控制的话，十个出口就有十处要维护。
fn normalize_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blanks = 0usize;
    for line in s.lines() {
        let l = line.trim_end();
        if l.is_empty() {
            blanks += 1;
            if blanks > 1 || out.is_empty() {
                continue;
            }
            out.push('\n');
        } else {
            blanks = 0;
            out.push_str(l);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// 安全截断 UTF-8 字符串至指定字节数（边界对齐到字符）
pub(crate) fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if i + c.len_utf8() > max_bytes {
            break;
        }
        end = i + c.len_utf8();
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 脚本 / 样式 / `<head>` 都不是正文，必须一个都不留
    #[test]
    fn markdown_drops_scripts_styles_and_head() {
        let html = "<html><head><title>某某网 - 首页</title><style>body{color:red}</style>\
                    <meta name=\"x\" content=\"y\"></head><body>\
                    <script>var a = 1 < 2;</script><h1>管理办法</h1><p>正文第一段。</p></body></html>";
        let md = html_to_markdown(html);
        assert!(!md.contains("某某网"), "网页标题不该进正文：{md}");
        assert!(!md.contains("color:red"));
        assert!(!md.contains("var a = 1"));
        assert!(md.contains("# 管理办法"), "{md}");
        assert!(md.contains("正文第一段。"));
    }

    /// **块级之间必须有空行** —— 这是这条链路的核心：知识库的分块器按空行分段，
    /// 没有空行整页就是"一个段落"，AI 整理的按字符分批永远不会触发
    #[test]
    fn markdown_separates_blocks_with_blank_lines() {
        let md = html_to_markdown("<p>第一段。</p><p>第二段。</p><div>第三段。</div>");
        assert_eq!(md, "第一段。\n\n第二段。\n\n第三段。");
    }

    #[test]
    fn markdown_keeps_lists_tables_code_and_links() {
        let html = "<ul><li>甲</li><li>乙</li></ul>\
                    <table><tr><th>名称</th><th>数量</th></tr><tr><td>A</td><td>1</td></tr></table>\
                    <pre><code>let  a  =  1;\n</code></pre>\
                    <p>见 <a href=\"https://example.com/x\">说明</a>，另见 <a href=\"#top\">顶部</a>。</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("- 甲\n- 乙"), "紧凑列表：{md}");
        // 表头 → 分隔行 → 数据行（顺序反了就不是表格）
        let header = md.find("| 名称 | 数量 |").expect("缺表头");
        let sep = md.find("| --- | --- |").expect("缺分隔行");
        let row = md.find("| A | 1 |").expect("缺数据行");
        assert!(header < sep && sep < row, "表格顺序不对：{md}");
        assert!(md.contains("let  a  =  1;"), "围栏内不折叠空格：{md}");
        assert!(md.contains("[说明](https://example.com/x)"), "{md}");
        // 纯锚点链接不转成 markdown（否则会得到一堆 [顶部](#top)）
        assert!(!md.contains("](#top)"), "{md}");
    }

    /// 数字实体必须解码：页面里的中文标点常写成 `&#12289;` / `&#x3002;` 这类
    #[test]
    fn markdown_decodes_numeric_entities() {
        let md = html_to_markdown("<p>甲&#12289;乙&#x3002;丙&nbsp;丁&amp;戊&未知;</p>");
        assert_eq!(md, "甲、乙。丙 丁&戊&未知;");
    }

    /// 没有 `<th>` 的表格补齐空表头，而不是把第一行数据当成表头（那会改变内容含义）
    #[test]
    fn markdown_pads_header_for_headerless_table() {
        let md = html_to_markdown("<table><tr><td>A</td><td>1</td></tr></table>");
        assert_eq!(md, "|  |  |\n| --- | --- |\n| A | 1 |");
    }
}
