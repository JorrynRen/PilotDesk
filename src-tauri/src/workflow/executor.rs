use serde_json::Value;
use std::sync::Arc;
use crate::agent::AgentManager;
use crate::plugin::PluginHost;
use tokio::sync::Mutex as AsyncMutex;
use crate::utils::errors::AppError;
use super::registry::{
    NodeDef, NodeOutput, NodeTypeRegistration,
    NodeTypeRegistrationInfo, WorkflowNodeTypeRegistry, NodeCategory,
};
use super::executors::agent_executor::AgentExecutor;
use super::executors::transform_executor::TransformExecutor;
use super::executors::interact_executor::{InteractExecutor, InteractManager};
use super::executors::api_executor::ApiExecutor;

/// 节点执行器分发器
pub struct NodeExecutor {
    registry: Arc<std::sync::Mutex<WorkflowNodeTypeRegistry>>,
    pub human_input_manager: Arc<InteractManager>,
    plugin_host: Arc<std::sync::Mutex<PluginHost>>,
}

impl NodeExecutor {
    pub fn new(agent_manager: Arc<AsyncMutex<AgentManager>>, plugin_host: Arc<std::sync::Mutex<PluginHost>>) -> Self {
        let mut registry = WorkflowNodeTypeRegistry::new();

        // 注册 6 种实体节点类型
        // 控制逻辑（条件/聚合/并行/延迟）由边/Gate/节点属性承载，不再需要独立执行器

        registry.register(NodeTypeRegistration {
            type_id: "agent".into(),
            name: "Agent 任务".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(AgentExecutor::new(agent_manager.clone())),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_type": { "type": "string", "enum": ["claude", "hermes", "codex"] },
                    "prompt_template": { "type": "string" },
                }
            })),
            permissions: vec![],
        });

        registry.register(NodeTypeRegistration {
            type_id: "transform".into(),
            name: "代码转换".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(TransformExecutor),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": "JavaScript 转换脚本" },
                }
            })),
            permissions: vec![],
        });

        registry.register(NodeTypeRegistration {
            type_id: "api".into(),
            name: "API 调用".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(ApiExecutor::new()),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "请求 URL" },
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "DELETE"] },
                    "body_template": { "type": "string", "description": "请求体模板" },
                    "timeout_seconds": { "type": "integer", "default": 60 },
                }
            })),
            permissions: vec!["network:http".to_string()],
        });

        let human_input_manager = Arc::new(InteractManager::new());

        registry.register(NodeTypeRegistration {
            type_id: "interact".into(),
            name: "人工交互".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(InteractExecutor::new(human_input_manager.clone())),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "input_type": { "type": "string", "enum": ["text", "select", "confirm", "file"] },
                }
            })),
            permissions: vec![],
        });

        registry.register(NodeTypeRegistration {
            type_id: "approval".into(),
            name: "人工审批".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(ApprovalExecutor),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "审批提示文案" },
                    "options": { "type": "array", "items": { "type": "object" } },
                    "timeout_minutes": { "type": "integer", "default": 1440 },
                    "default_value": { "type": "string", "default": "approve" },
                }
            })),
            permissions: vec![],
        });

        let registry_arc = Arc::new(std::sync::Mutex::new(registry));

        // 设置插件节点类型注册回调
        let reg_arc = registry_arc.clone();
        plugin_host.lock().unwrap().set_register_node_type(Box::new(move |registration: NodeTypeRegistration| {
            if let Ok(mut reg) = reg_arc.lock() {
                reg.register(registration);
            }
        }));

        Self {
            registry: registry_arc,
            human_input_manager,
            plugin_host,
        }
    }

    /// 从注册表查找执行器并执行
    pub async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let executor = {
            let reg = self.registry.lock().map_err(|e| AppError::Lock(e.to_string()))?;
            reg.get_executor(&node.node_type)
                .ok_or_else(|| AppError::InvalidInput(format!("未知节点类型: {}", node.node_type)))?
        };

        executor.execute(node, resolved_input, execution_id, emitter).await
    }

    #[allow(dead_code)]
    pub fn register_plugin_node_type(&self, registration: NodeTypeRegistration) {
        if let Ok(mut reg) = self.registry.lock() {
            reg.register(registration);
        }
    }

    #[allow(dead_code)]
    pub fn unregister_plugin_node_type(&self, type_id: &str) {
        if let Ok(mut reg) = self.registry.lock() {
            reg.unregister(type_id);
        }
    }

    pub fn sync_plugin_node_types(&self) {
        let plugin_host = self.plugin_host.lock().map_err(|e| e.to_string());
        if let Ok(host) = plugin_host {
            let contributed = host.get_contributed_node_types();
            if let Ok(mut reg) = self.registry.lock() {
                for (plugin_id, nt) in contributed {
                    use crate::workflow::registry::NodeCategory;
                    reg.register(crate::workflow::registry::NodeTypeRegistration {
                        type_id: nt.type_id.clone(),
                        name: nt.name.clone(),
                        category: NodeCategory::Plugin(plugin_id.clone()),
                        executor: Arc::new(crate::workflow::executors::plugin_executor::PluginExecutor::new(
                            plugin_id.clone(),
                            nt.type_id.clone(),
                        )),
                        config_schema: nt.config_schema.clone(),
                        permissions: nt.permissions.clone(),
                    });
                }
            }
        }
    }

    pub fn list_node_types(&self) -> Vec<NodeTypeRegistrationInfo> {
        self.registry.lock()
            .map(|reg| reg.get_all_registrations())
            .unwrap_or_default()
    }
}
