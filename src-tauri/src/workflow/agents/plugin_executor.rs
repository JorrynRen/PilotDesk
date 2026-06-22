use async_trait::async_trait;
use serde_json::Value;
use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeOutput, NodeExecutorTrait};

pub struct PluginExecutor {
    pub plugin_id: String,
    pub type_id: String,
}

impl PluginExecutor {
    pub fn new(plugin_id: String, type_id: String) -> Self {
        Self { plugin_id, type_id }
    }
}

#[async_trait]
impl NodeExecutorTrait for PluginExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _resolved_input: Value,
        _execution_id: &str,
        _emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        // 插件节点执行功能尚待实现
        Err(AppError::External(format!(
            "插件节点执行尚未实现: plugin_id={}, type_id={}, node_id={}",
            self.plugin_id, self.type_id, node.id
        )))
    }
}
