use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeExecutorTrait, NodeOutput};
use crate::workflow::template::TemplateEngine;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::Emitter;
use tokio::sync::oneshot;

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
        Self {
            pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
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
        self.pending
            .lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .insert(
                key,
                PendingPluginEntry {
                    tx,
                    registered_at: std::time::Instant::now(),
                    ttl_secs,
                },
            );
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
        let entry = self
            .pending
            .lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .remove(&key)
            .ok_or_else(|| AppError::NotFound(format!("没有等待中的插件执行: {}", key)))?;
        entry
            .tx
            .send(result)
            .map_err(|_| AppError::External("通道已关闭".into()))?;
        Ok(())
    }

    /// 结束等待（节点超时/取消）：立即移出登记表，使登记表严格等于"此刻真有人在等"，
    /// 避免节点已判失败、前端点了回传却找不到等待者。
    pub fn forget(&self, execution_id: &str, node_id: &str) {
        if let Ok(mut map) = self.pending.lock() {
            map.remove(&format!("{}:{}", execution_id, node_id));
        }
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
        Self {
            plugin_id,
            _type_id: type_id,
            manager,
        }
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
        let plugin_id = node
            .plugin_id
            .clone()
            .unwrap_or_else(|| self.plugin_id.clone());
        let command_id = node
            .command_id
            .clone()
            .or_else(|| {
                node.config
                    .get("commandId")
                    .and_then(|v| v.as_str().map(String::from))
            })
            .ok_or_else(|| {
                AppError::InvalidInput(format!("插件节点 \"{}\" 缺少命令 ID", node.label))
            })?;

        // 模板上下文 = 上游输入解析后的结果（各输入映射键 + __input__ 整体）
        let mut template_ctx: HashMap<String, Value> = HashMap::new();
        if !resolved_input.is_null() {
            if let Some(inp) = resolved_input.as_object() {
                for (k, v) in inp {
                    template_ctx.insert(k.clone(), v.clone());
                }
            }
            template_ctx.insert("__input__".to_string(), resolved_input.clone());
        }

        // 合并节点 config 与上游输入作为命令参数
        // config 中的 {{}} 引用输入映射的值（未命中的引用保留原文），命中时按内容推断类型
        let mut params = TemplateEngine::resolve_value(&node.config, &template_ctx);
        if !resolved_input.is_null() {
            if let (Some(obj), Some(inp)) = (params.as_object_mut(), resolved_input.as_object()) {
                for (k, v) in inp {
                    obj.entry(k.clone()).or_insert(v.clone());
                }
            } else {
                params["__input__"] = resolved_input;
            }
        }

        // 等待前端回传的上限：配置了节点超时（ms）就以它为准（下限 5 秒，避免 TTL 太短来不及回传），
        // 否则默认 30 秒。引擎层还有一层同一 timeout_ms 的硬超时，这里只是等待通道的 TTL。
        let timeout_secs = node.timeout_ms.map(|ms| (ms / 1000).max(5)).unwrap_or(30);

        // 发射事件请求前端执行插件命令
        emitter
            .emit(
                "workflow:plugin-execute",
                serde_json::json!({
                    "execution_id": execution_id,
                    "node_id": node.id,
                    "plugin_id": plugin_id,
                    "command_id": command_id,
                    "params": params,
                    "timeout_seconds": timeout_secs,
                }),
            )
            .map_err(|e| AppError::External(format!("发射插件执行事件失败: {}", e)))?;

        // 挂起等待前端回传结果
        let rx = self
            .manager
            .register_wait(execution_id, &node.id, timeout_secs)?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), rx).await;
        match result {
            Ok(Ok(res)) => {
                if res.success {
                    // 与全节点类型统一的取值语义一致：单字段对象/单元素数组解包、JSON 文本解析为结构。
                    // 插件返回 `{"result": x}` 时节点执行结果就是 `x`——下游取值本就会解包
                    // （见 TemplateEngine::normalize_value），不在这里解包会让「执行结果 / 门控合并值」
                    // 显示出一个谁都不会拿到的外壳，与其他节点类型（transform 标量、agent 文本）不一致。
                    Ok(NodeOutput {
                        output: TemplateEngine::normalize_value(&res.data),
                        session_id: None,
                        input_data: None,
                        artifacts_path: None,
                    })
                } else {
                    Err(AppError::External(
                        res.error.unwrap_or_else(|| "插件命令执行失败".to_string()),
                    ))
                }
            }
            Ok(Err(_)) => Err(AppError::External("插件命令执行通道已关闭".into())),
            Err(_) => {
                log::warn!("插件节点 \"{}\" 执行超时（{}秒）", node.label, timeout_secs);
                Err(AppError::External(format!(
                    "插件命令执行超时（{}秒）",
                    timeout_secs
                )))
            }
        }
    }
}
