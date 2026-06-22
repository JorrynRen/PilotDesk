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
use super::agents::agent_executor::AgentExecutor;
use super::agents::transform_executor::TransformExecutor;
use super::agents::human_input_executor::{HumanInputExecutor, HumanInputManager};
use super::agents::api_executor::ApiExecutor;
use super::agents::condition_executor::ConditionExecutor;
use super::agents::aggregator_executor::AggregatorExecutor;
use super::agents::approval_executor::ApprovalExecutor;

/// 节点执行器分发器
pub struct NodeExecutor {
    registry: Arc<std::sync::Mutex<WorkflowNodeTypeRegistry>>,
    pub human_input_manager: Arc<HumanInputManager>,
    plugin_host: Arc<std::sync::Mutex<PluginHost>>,
}

impl NodeExecutor {
    pub fn new(agent_manager: Arc<AsyncMutex<AgentManager>>, plugin_host: Arc<std::sync::Mutex<PluginHost>>) -> Self {
        let mut registry = WorkflowNodeTypeRegistry::new();

        // 注册内置节点类型
        registry.register(NodeTypeRegistration {
            type_id: "agent_task".into(),
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
            type_id: "api_call".into(),
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

        registry.register(NodeTypeRegistration {
            type_id: "condition".into(),
            name: "条件分支".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(ConditionExecutor),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "expression": { "type": "string", "description": "条件表达式" },
                    "output_true": { "type": "string", "description": "条件满足时的输出" },
                    "output_false": { "type": "string", "description": "条件不满足时的输出" },
                }
            })),
            permissions: vec![],
        });

        let human_input_manager = Arc::new(HumanInputManager::new());

        registry.register(NodeTypeRegistration {
            type_id: "human_input".into(),
            name: "人工介入".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(HumanInputExecutor::new(human_input_manager.clone())),
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
            type_id: "aggregator".into(),
            name: "聚合节点".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(AggregatorExecutor),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "strategy": { "type": "string", "enum": ["merge", "concat", "pick_first"], "default": "merge" },
                    "input_sources": { "type": "array", "items": { "type": "string" } },
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
    /// 插件动态注册节点类型
    pub fn register_plugin_node_type(&self, registration: NodeTypeRegistration) {
        if let Ok(mut reg) = self.registry.lock() {
            reg.register(registration);
        }
    }

    #[allow(dead_code)]
    /// 插件卸载时注销节点类型
    pub fn unregister_plugin_node_type(&self, type_id: &str) {
        if let Ok(mut reg) = self.registry.lock() {
            reg.unregister(type_id);
        }
    }

    /// 从 PluginHost 同步所有插件贡献的节点类型到注册表
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
                        executor: Arc::new(crate::workflow::agents::plugin_executor::PluginExecutor::new(
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

    /// 获取所有注册类型
    pub fn list_node_types(&self) -> Vec<NodeTypeRegistrationInfo> {
        self.registry.lock()
            .map(|reg| reg.get_all_registrations())
            .unwrap_or_default()
    }
}
