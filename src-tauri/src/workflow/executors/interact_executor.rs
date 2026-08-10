use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;
use crate::utils::errors::AppError;
use tauri::Emitter;
use crate::workflow::registry::{NodeDef, NodeOutput, NodeExecutorTrait};

/// 人工介入配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanInputConfig {
    /// 提示文案
    #[serde(default = "default_prompt")]
    pub prompt: String,
    /// 输入类型：text / select / confirm / file
    #[serde(default = "default_input_type")]
    pub input_type: String,
    pub options: Option<Vec<InputOption>>,
    pub default_value: Option<String>,
    pub timeout_minutes: Option<u64>,
    pub allow_custom: Option<bool>,
    pub placeholder: Option<String>,
}

fn default_prompt() -> String { "请输入".to_string() }
fn default_input_type() -> String { "text".to_string() }

/// 兼容旧数据：解析失败时尝试从 serde_json::Value 手动映射并填入默认值。
/// 同时负责"前端参数 → 后端 HumanInputConfig"的字段转换，
/// 例如前端 `optionsJson`(JSON 字符串) 解析成 `options`，timeoutMinutes 字符串/数字统一。
pub fn parse_human_input_config(config: serde_json::Value) -> Result<HumanInputConfig, AppError> {
    // 先做一轮规范化：把前端特有字段映射成标准字段。
    // 规范化后的 Value 仍然可能缺字段，再交给下面的默认兜底解析。
    let mut obj = match config {
        Value::Object(m) => m,
        v => {
            let mut m = serde_json::Map::new();
            m.insert("__raw_value".into(), v);
            m
        }
    };

    // 规范化 timeoutMinutes：支持数字或字符串（前端 number 输入框可能返回字符串）
    if let Some(v) = obj.get("timeoutMinutes").cloned() {
        let as_u64 = v.as_u64()
            .or_else(|| v.as_i64().map(|x| x.max(0) as u64))
            .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()));
        if let Some(n) = as_u64 {
            obj.insert("timeoutMinutes".into(), Value::Number(serde_json::Number::from(n)));
        }
    }
    if let Some(v) = obj.get("timeout_minutes").cloned() {
        if let Some(n) = v.as_u64().or_else(|| v.as_i64().map(|x| x.max(0) as u64)) {
            obj.insert("timeoutMinutes".into(), Value::Number(serde_json::Number::from(n)));
        }
    }

    // 规范化 options：前端参数 key 可能是 optionsJson (JSON 字符串) 或 options (数组对象)
    let options_from_array = obj.get("options").and_then(|v| serde_json::from_value::<Vec<InputOption>>(v.clone()).ok());
    let options_from_json_str = obj.get("optionsJson").and_then(|v| {
        v.as_str().and_then(|s| serde_json::from_str::<Vec<InputOption>>(s).ok())
    }).or_else(|| {
        obj.get("optionsJson").and_then(|v| serde_json::from_value::<Vec<InputOption>>(v.clone()).ok())
    });
    if let Some(opts) = options_from_array.or(options_from_json_str) {
        obj.insert("options".into(), serde_json::to_value(&opts).unwrap_or(Value::Null));
    }
    // optionsJson 不再参与后续解析
    obj.remove("optionsJson");

    let normalized = Value::Object(obj);

    // 先按标准结构解析；失败则使用手动兜底（避免旧实例缺字段报错）
    let cfg: HumanInputConfig = match serde_json::from_value(normalized.clone()) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[InteractExecutor] 标准解析失败，尝试手动映射: {}", e);
            let obj = normalized.as_object()
                .ok_or_else(|| AppError::Config(format!("interact 配置解析失败: 不是对象（{}）", e)))?;
            HumanInputConfig {
                prompt: obj.get("prompt").and_then(|v| v.as_str().map(|s| s.to_string()))
                    .or_else(|| obj.get("Prompt").and_then(|v| v.as_str().map(|s| s.to_string())))
                    .unwrap_or_else(default_prompt),
                input_type: obj.get("inputType").and_then(|v| v.as_str().map(|s| s.to_string()))
                    .or_else(|| obj.get("input_type").and_then(|v| v.as_str().map(|s| s.to_string())))
                    .or_else(|| obj.get("InputType").and_then(|v| v.as_str().map(|s| s.to_string())))
                    .unwrap_or_else(default_input_type),
                options: obj.get("options").cloned().and_then(|v| serde_json::from_value(v).ok())
                    .or_else(|| obj.get("Options").cloned().and_then(|v| serde_json::from_value(v).ok())),
                default_value: obj.get("defaultValue").and_then(|v| v.as_str().map(|s| s.to_string()))
                    .or_else(|| obj.get("default_value").and_then(|v| v.as_str().map(|s| s.to_string()))),
                timeout_minutes: obj.get("timeoutMinutes").and_then(|v| v.as_u64())
                    .or_else(|| obj.get("timeout_minutes").and_then(|v| v.as_u64()))
                    .or_else(|| obj.get("timeoutMinutes").and_then(|v| v.as_i64().map(|x| x.max(0) as u64))),
                allow_custom: obj.get("allowCustom").and_then(|v| v.as_bool())
                    .or_else(|| obj.get("allow_custom").and_then(|v| v.as_bool())),
                placeholder: obj.get("placeholder").and_then(|v| v.as_str().map(|s| s.to_string())),
            }
        }
    };
    Ok(cfg)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputOption {
    pub label: String,
    pub value: String,
}

/// 带过期时间的待处理条目
#[allow(dead_code)]
struct PendingEntry {
    tx: oneshot::Sender<String>,
    registered_at: std::time::Instant,
    ttl_secs: u64,
}

#[allow(dead_code)]
impl PendingEntry {
    fn is_expired(&self) -> bool {
        self.registered_at.elapsed().as_secs() > self.ttl_secs
    }
}

/// 等待用户响应的通道管理器（支持 TTL 自动过期清理）
pub struct InteractManager {
    pending: Arc<std::sync::Mutex<HashMap<String, PendingEntry>>>,
}

impl InteractManager {
    pub fn new() -> Self {
        Self { pending: Arc::new(std::sync::Mutex::new(HashMap::new())) }
    }

    /// 注册等待通道，返回 receiver（同时惰性清理过期条目）
    pub fn register_wait(&self, execution_id: &str, node_id: &str, ttl_secs: u64) -> Result<oneshot::Receiver<String>, AppError> {
        // 惰性清理：每次注册时顺便清理过期条目，避免引入独立定时器
        self.cleanup_expired();

        let key = format!("{}:{}", execution_id, node_id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().map_err(|e| AppError::Lock(e.to_string()))?.insert(key, PendingEntry {
            tx,
            registered_at: std::time::Instant::now(),
            ttl_secs,
        });
        Ok(rx)
    }

    /// 接收用户响应，发送到通道
    pub fn resolve(&self, execution_id: &str, node_id: &str, response: String) -> Result<(), AppError> {
        let key = format!("{}:{}", execution_id, node_id);
        let entry = self.pending.lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .remove(&key)
            .ok_or_else(|| AppError::NotFound(format!("没有等待中的输入: {}", key)))?;
        entry.tx.send(response).map_err(|_| AppError::External("通道已关闭".into()))?;
        Ok(())
    }

    /// 清理已过期的 pending 条目（用户取消/超时后残留）
    pub fn cleanup_expired(&self) {
        let mut map = match self.pending.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        let before = map.len();
        map.retain(|key, entry| {
            if entry.is_expired() {
                log::warn!("InteractManager: 清理过期 pending 条目: {}", key);
                false
            } else {
                true
            }
        });
        let removed = before - map.len();
        if removed > 0 {
            log::info!("InteractManager: 清理了 {} 个过期条目", removed);
        }
    }
}

pub struct InteractExecutor {
    pub manager: Arc<InteractManager>,
}

impl InteractExecutor {
    pub fn new(manager: Arc<InteractManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl NodeExecutorTrait for InteractExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let config: HumanInputConfig = parse_human_input_config(node.config.clone())?;

        let timeout = config.timeout_minutes.unwrap_or(30);

        // 发射 awaiting-input 事件到前端
        emitter.emit("workflow:awaiting-input", serde_json::json!({
            "execution_id": execution_id,
            "node_id": node.id,
            "prompt": config.prompt,
            "input_type": config.input_type,
            "options": config.options,
            "allow_custom": config.allow_custom,
            "placeholder": config.placeholder,
            "timeout_minutes": timeout,
        })).ok();

        // 挂起等待用户响应
        let rx = self.manager.register_wait(execution_id, &node.id, timeout * 60)?;

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout * 60),
            rx,
        ).await;

        let user_input = match result {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return Err(AppError::External("用户取消了人工介入".into())),
            Err(_) => {
                emitter.emit("workflow:log", serde_json::json!({
                    "execution_id": execution_id,
                    "node_execution_id": node.id,
                    "level": "warn",
                    "message": format!("人工介入超时（{}分钟），使用默认值", timeout),
                })).ok();
                config.default_value.unwrap_or_default()
            }
        };

        Ok(NodeOutput {
            // 与其他节点统一：输出 { content: <用户输入值> }
            // outputMapping 配置 {{content}} 即可拿到原始值
            output: serde_json::json!({ "content": user_input }),
            session_id: None,
            input_data: None,
            artifacts_path: None,
        })
    }
}
