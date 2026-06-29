use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::Emitter;
use tokio::sync::oneshot;
use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeOutput, NodeExecutorTrait};

/// 插件命令执行结果（前端回传）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginExecuteResult {
    pub success: bool,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub error: Option<String>,
}

/// 带过期时间的待处理插件执行条目
struct PendingPluginEntry {
    tx: oneshot::Sender<PluginExecuteResult>,
    registered_at: std::time::Instant,
    ttl_secs: u64,
}

impl PendingPluginEntry {
    fn is_expired(&self) -> bool {
        self.registered_at.elapsed().as_secs() > self.ttl_secs
    }
}

/// 插件命令执行通道管理器（支持 TTL 自动过期清理）
///
/// 插件命令 handler 注册在前端 JS 运行时，后端无法直接调用。
/// 后端通过 emit `workflow:plugin-execute` 事件请求前端执行，
/// 前端执行完成后通过 `respond_plugin_execute` 命令回传结果，
/// 唤醒此处挂起的 oneshot 通道。
pub struct PluginExecuteManager {
    pending: Arc<std::sync::Mutex<HashMap<String, PendingPluginEntry>>>,
}

impl PluginExecuteManager {
    pub fn new() -> Self {
        Self { pending: Arc::new(std::sync::Mutex::new(HashMap::new())) }
    }

    /// 注册等待通道，返回 receiver（惰性清理过期条目）
    pub fn register_wait(
        &self,
        execution_id: &str,
        node_id: &str,
        ttl_secs: u64,
    ) -> Result<oneshot::Receiver<PluginExecuteResult>, AppError> {
        self.cleanup_expired();
        let key = format!("{}:{}", execution_id, node_id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().map_err(|e| AppError::Lock(e.to_string()))?.insert(key, PendingPluginEntry {
            tx,
            registered_at: std::time::Instant::now(),
            ttl_secs,
        });
        Ok(rx)
    }

    /// 接收前端回传的执行结果
    pub fn resolve(
        &self,
        execution_id: &str,
        node_id: &str,
        result: PluginExecuteResult,
    ) -> Result<(), AppError> {
        let key = format!("{}:{}", execution_id, node_id);
        let entry = self.pending.lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .remove(&key)
            .ok_or_else(|| AppError::NotFound(format!("没有等待中的插件执行: {}", key)))?;
        entry.tx.send(result).map_err(|_| AppError::External("通道已关闭".into()))?;
        Ok(())
    }

    /// 清理已过期的 pending 条目（前端未响应/取消后残留）
    pub fn cleanup_expired(&self) {
        let mut map = match self.pending.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        let before = map.len();
        map.retain(|key, entry| {
            if entry.is_expired() {
                log::warn!("PluginExecuteManager: 清理过期 pending 条目: {}", key);
                false
            } else {
                true
            }
        });
        let removed = before - map.len();
        if removed > 0 {
            log::info!("PluginExecuteManager: 清理了 {} 个过期条目", removed);
        }
    }
}

pub struct PluginExecutor {
    pub plugin_id: String,
    pub _type_id: String,
    pub manager: Arc<PluginExecuteManager>,
}

impl PluginExecutor {
    pub fn new(plugin_id: String, type_id: String, manager: Arc<PluginExecuteManager>) -> Self {
        Self { plugin_id, _type_id: type_id, manager }
    }
}

#[async_trait]
impl NodeExecutorTrait for PluginExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        // 优先使用节点上的 plugin_id / command_id，回退到 executor 绑定的 plugin_id
        let plugin_id = node.plugin_id.clone().unwrap_or_else(|| self.plugin_id.clone());
        let command_id = node.command_id.clone()
            .or_else(|| node.config.get("commandId").and_then(|v| v.as_str().map(String::from)))
            .ok_or_else(|| AppError::InvalidInput(format!(
                "插件节点 \"{}\" 缺少命令 ID", node.label
            )))?;

        // 合并节点 config 与上游输入作为命令参数
        let mut params = node.config.clone();
        if !resolved_input.is_null() {
            if let (Some(obj), Some(inp)) = (params.as_object_mut(), resolved_input.as_object()) {
                for (k, v) in inp {
                    obj.entry(k.clone()).or_insert(v.clone());
                }
            } else {
                params["__input__"] = resolved_input;
            }
        }

        // 默认超时 30 秒，可通过节点 timeout_seconds 配置（下限 5 秒）
        let timeout_secs = node.timeout_seconds.unwrap_or(30).max(5);

        // 发射事件请求前端执行插件命令
        emitter.emit("workflow:plugin-execute", serde_json::json!({
            "execution_id": execution_id,
            "node_id": node.id,
            "plugin_id": plugin_id,
            "command_id": command_id,
            "params": params,
            "timeout_seconds": timeout_secs,
        })).map_err(|e| AppError::External(format!("发射插件执行事件失败: {}", e)))?;

        // 挂起等待前端回传结果
        let rx = self.manager.register_wait(execution_id, &node.id, timeout_secs)?;

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            rx,
        ).await;

        match result {
            Ok(Ok(res)) => {
                if res.success {
                    Ok(NodeOutput { output: res.data, session_id: None })
                } else {
                    Err(AppError::External(
                        res.error.unwrap_or_else(|| "插件命令执行失败".to_string()),
                    ))
                }
            }
            Ok(Err(_)) => Err(AppError::External("插件命令执行通道已关闭".into())),
            Err(_) => {
                log::warn!("插件节点 \"{}\" 执行超时（{}秒）", node.label, timeout_secs);
                Err(AppError::External(format!("插件命令执行超时（{}秒）", timeout_secs)))
            }
        }
    }
}
