use serde_json::Value;
use std::sync::Arc;
use crate::agent::AgentManager;
use crate::plugin::PluginHost;
use tokio::sync::Mutex as AsyncMutex;
use crate::utils::errors::AppError;
use async_trait::async_trait;
use super::registry::{
    NodeDef, NodeOutput, NodeTypeRegistration,
    NodeTypeRegistrationInfo, WorkflowNodeTypeRegistry, NodeCategory, NodeExecutorTrait,
};
use super::executors::agent_executor::AgentExecutor;
use super::executors::transform_executor::TransformExecutor;
use super::executors::interact_executor::{InteractExecutor, InteractManager};
use super::executors::api_executor::ApiExecutor;
use super::executors::plugin_executor::PluginExecuteManager;

/// 边界节点执行器（start/end）— 无操作，直接输入作为输出
pub struct NoopExecutor;

#[async_trait]
impl NodeExecutorTrait for NoopExecutor {
    async fn execute(
        &self,
        _node: &NodeDef,
        resolved_input: Value,
        _execution_id: &str,
        _emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        Ok(NodeOutput { output: resolved_input })
    }
}

/// 节点执行器分发器
pub struct NodeExecutor {
    registry: Arc<std::sync::Mutex<WorkflowNodeTypeRegistry>>,
    pub human_input_manager: Arc<InteractManager>,
    /// 插件节点执行通道管理器（前端回传结果时唤醒挂起的 oneshot）
    pub plugin_execute_manager: Arc<PluginExecuteManager>,
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
        let plugin_execute_manager = Arc::new(PluginExecuteManager::new());

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

        // 注册 start/end 边界节点（无操作，仅透传输入）
        registry.register(NodeTypeRegistration {
            type_id: "start".into(),
            name: "开始".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(NoopExecutor),
            config_schema: None,
            permissions: vec![],
        });
        registry.register(NodeTypeRegistration {
            type_id: "end".into(),
            name: "结束".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(NoopExecutor),
            config_schema: None,
            permissions: vec![],
        });

        let registry_arc = Arc::new(std::sync::Mutex::new(registry));

        // 设置插件节点类型注册回调 + 共享插件执行通道管理器
        let reg_arc = registry_arc.clone();
        let mut host_guard = plugin_host.lock().unwrap();
        host_guard.set_plugin_execute_manager(plugin_execute_manager.clone());
        host_guard.set_register_node_type(Box::new(move |registration: NodeTypeRegistration| {
            if let Ok(mut reg) = reg_arc.lock() {
                reg.register(registration);
            }
        }));
        drop(host_guard);

        Self {
            registry: registry_arc,
            human_input_manager,
            plugin_execute_manager,
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
        let plugin_host = match self.plugin_host.lock() {
            Ok(h) => h,
            Err(e) => {
                log::warn!("[NodeExecutor] 获取 PluginHost 锁失败: {}", e);
                return;
            }
        };
        let contributed = plugin_host.get_contributed_node_types();
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
                        self.plugin_execute_manager.clone(),
                    )),
                    config_schema: nt.config_schema.clone(),
                    permissions: nt.permissions.clone(),
                });
            }
        }
    }

    pub fn list_node_types(&self) -> Vec<NodeTypeRegistrationInfo> {
        self.registry.lock()
            .map(|reg| reg.get_all_registrations())
            .unwrap_or_default()
    }
}
