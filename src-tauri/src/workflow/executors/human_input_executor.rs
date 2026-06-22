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
    pub prompt: String,
    pub input_type: String,          // text / select / confirm / file
    pub options: Option<Vec<InputOption>>,
    pub default_value: Option<String>,
    pub timeout_minutes: Option<u64>,
    pub allow_custom: Option<bool>,
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputOption {
    pub label: String,
    pub value: String,
}

/// 等待用户响应的通道管理器
pub struct HumanInputManager {
    pending: Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<String>>>>,
}

impl HumanInputManager {
    pub fn new() -> Self {
        Self { pending: Arc::new(std::sync::Mutex::new(HashMap::new())) }
    }

    /// 注册等待通道，返回 receiver
    pub fn register_wait(&self, execution_id: &str, node_id: &str) -> Result<oneshot::Receiver<String>, AppError> {
        let key = format!("{}:{}", execution_id, node_id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().map_err(|e| AppError::Lock(e.to_string()))?.insert(key, tx);
        Ok(rx)
    }

    /// 接收用户响应，发送到通道
    pub fn resolve(&self, execution_id: &str, node_id: &str, response: String) -> Result<(), AppError> {
        let key = format!("{}:{}", execution_id, node_id);
        let tx = self.pending.lock()
            .map_err(|e| AppError::Lock(e.to_string()))?
            .remove(&key)
            .ok_or_else(|| AppError::NotFound(format!("没有等待中的输入: {}", key)))?;
        tx.send(response).map_err(|_| AppError::External("通道已关闭".into()))?;
        Ok(())
    }
}

pub struct HumanInputExecutor {
    pub manager: Arc<HumanInputManager>,
}

impl HumanInputExecutor {
    pub fn new(manager: Arc<HumanInputManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl NodeExecutorTrait for HumanInputExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let config: HumanInputConfig = serde_json::from_value(node.config.clone())
            .map_err(|e| AppError::Config(format!("human_input 配置解析失败: {}", e)))?;

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
        let rx = self.manager.register_wait(execution_id, &node.id)?;

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
            output: serde_json::json!({ "user_input": user_input }),
        })
    }
}
