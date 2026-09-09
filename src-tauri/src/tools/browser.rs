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
//! `find_browser` / `run_browser` / `html_to_text` / `truncate_utf8`。

use crate::tools::{RiskLevel, ToolHandler};
use async_trait::async_trait;
use base64::Engine;
use futures::{Sink, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::os::windows::process::CommandExt;
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
                return Err("select_option 需要 option_text / option_value / option_index 之一".to_string());
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
            .ok_or_else(|| "未检测到 Microsoft Edge 或 Google Chrome 浏览器，无法执行浏览器自动化。请安装其中之一后重试。".to_string())?;
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

        let child = std::process::Command::new(&browser)
            .args([
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
            .stderr(std::process::Stdio::null())
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| format!("启动浏览器失败: {}", e))?;

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

        let msg = Message::Text(
            json!({"id": id, "method": method, "params": params}).to_string(),
        );
        let writer = self.writer.as_mut().ok_or("浏览器连接未建立")?;
        writer
            .send(msg)
            .await
            .map_err(|e| format!("发送 CDP 消息失败: {}", e))?;
        writer.flush().await.map_err(|e| format!("刷新 CDP 消息失败: {}", e))?;

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
            return Err("页面内容为空或无法解析（可能是需要登录、存在反爬或 JS 渲染超时）".to_string());
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

    async fn fill_at(&mut self, index: u64, text: &str, press_enter: bool) -> Result<String, String> {
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
        for (event_type, params_extra) in [
            ("keyDown", meta.0.clone()),
            ("keyUp", meta.1.clone()),
        ] {
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
            _ => unreachable!(),
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
        std::fs::write(&abs, &bytes).map_err(|e| format!("保存截图失败 ({}): {}", abs.display(), e))?;
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
            return Err(format!("当前上传控件不支持多文件（收到 {} 个）", files.len()));
        }
        if !accept.trim().is_empty() {
            for f in files {
                if !accept_match(f, accept) {
                    return Err(format!("文件 {} 不符合上传控件 accept=\"{}\"", f, accept));
                }
            }
        }

        // DOM 定位 file input 并注入文件
        let doc = self
            .call("DOM.getDocument", json!({"depth": 0}))
            .await?;
        let root_id = doc["root"]["nodeId"]
            .as_i64()
            .ok_or("无法获取页面根节点")?;
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
fn key_meta(key: &str) -> (Option<(String, u32)>, Option<(String, u32)>, Option<&'static str>) {
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
    mut stream: futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>,
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
            return Err(format!("浏览器调试端口 {} 未就绪（启动失败或被占用）", port));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 校验文件路径是否符合 input accept（扩展名 / mime 前缀匹配，`*/*` 通配）
fn accept_match(path: &str, accept: &str) -> bool {
    let name = path.replace('\\', "/").rsplit('/').next().unwrap_or(path).to_lowercase();
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
                    "png" => "image/", "jpg" | "jpeg" => "image/", "gif" => "image/",
                    "webp" => "image/", "bmp" => "image/", "svg" => "image/",
                    "pdf" => "application/", "txt" => "text/", "md" => "text/",
                    "json" => "application/", "html" | "htm" => "text/",
                    "csv" => "text/", "doc" | "docx" => "application/",
                    "xls" | "xlsx" => "application/", "ppt" | "pptx" => "application/",
                    "zip" => "application/", "mp4" => "video/", "mp3" => "audio/",
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

/// 定位本机可用的 Edge / Chrome 可执行文件
pub(crate) fn find_browser() -> Option<String> {
    const CANDIDATES: &[&str] = &[
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    for c in CANDIDATES {
        if std::path::Path::new(c).is_file() {
            return Some(c.to_string());
        }
    }
    // 回退：尝试在 PATH 中查找
    for exe in ["msedge", "chrome"] {
        if let Ok(out) = std::process::Command::new("where")
            .arg(exe)
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
pub(crate) fn run_browser(browser: &str, args: &[String]) -> Result<(Vec<u8>, Vec<u8>, Option<i32>), String> {
    let mut child = std::process::Command::new(browser)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|e| format!("启动浏览器失败: {}", e))?;

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
