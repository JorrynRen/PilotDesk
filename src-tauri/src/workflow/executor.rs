use tauri::Manager;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex as AsyncMutex};
use std::collections::HashMap;
use crate::agent::AgentManager;
use crate::plugin::PluginHost;
use crate::workflow::executors::plugin_executor::PluginExecutor;
use crate::utils::errors::AppError;
use async_trait::async_trait;
use super::registry::{
    NodeDef, NodeOutput, NodeTypeRegistration,
    NodeTypeRegistrationInfo, WorkflowNodeTypeRegistry, NodeCategory, NodeExecutorTrait,
};
use crate::db::init::DbPool;
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
        Ok(NodeOutput { output: resolved_input, session_id: None, input_data: None, artifacts_path: None })
    }
}

/// 执行句柄（用于外部取消）
pub struct ExecutionHandle {
    pub cancelled: Arc<AtomicBool>,
}

/// 节点执行器
pub struct NodeExecutor {
    registry: Arc<std::sync::Mutex<WorkflowNodeTypeRegistry>>,
    pub human_input_manager: Arc<InteractManager>,
    /// 插件节点执行通道管理器（前端回传结果时唤醒挂起的 oneshot）
    pub plugin_execute_manager: Arc<PluginExecuteManager>,
    app_handle: tauri::AppHandle,
    #[allow(dead_code)]
    pool: DbPool,
    /// Agent 管理器（供 cancel_workflow 等命令中止子进程）
    agent_manager: Arc<AsyncMutex<AgentManager>>,
    /// Agent 管理器的 shared 副本（持有相同的 processes Arc，无需 AsyncMutex 锁）
    agent_manager_shared: AgentManager,
    /// 运行中的执行注册表（execution_id -> ExecutionHandle）
    running_executions: Arc<std::sync::Mutex<HashMap<String, ExecutionHandle>>>,
}

impl NodeExecutor {
    /// 获取 Agent 管理器的 Arc clone（供外部命令中止 Agent 子进程）
    #[allow(dead_code)]
    pub fn agent_manager(&self) -> Arc<AsyncMutex<AgentManager>> {
        self.agent_manager.clone()
    }

    /// 注册执行句柄（执行开始时调用）
    pub fn register_execution(&self, execution_id: &str) -> Arc<AtomicBool> {
        let mut map = self.running_executions.lock().unwrap();
        let handle = ExecutionHandle {
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let cancelled = handle.cancelled.clone();
        map.insert(execution_id.to_string(), handle);
        cancelled
    }

    /// 注销执行句柄（执行结束时调用）
    pub fn unregister_execution(&self, execution_id: &str) {
        let mut map = self.running_executions.lock().unwrap();
        map.remove(execution_id);
    }

    /// 取消执行（设置 cancelled 标志）
    pub fn cancel_execution(&self, execution_id: &str) -> bool {
        let map = self.running_executions.lock().unwrap();
        if let Some(handle) = map.get(execution_id) {
            handle.cancelled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
    /// 取消执行并立即终止 Agent 子进程（不获取 AgentManager 大锁）
    ///
    /// 解决 cancel_workflow 中 agent_manager.lock().await 与 AgentExecutor
    /// 竞争 AsyncMutex 锁导致的取消延迟问题。
    ///
    /// 调用顺序：
    /// 1. 先设置 cancelled AtomicBool（使 tokio::select! 能立即响应）
    /// 2. 再通过 agent_manager_shared 的 processes HashMap 直接 kill 子进程
    pub fn cancel_execution_and_kill_agents(&self, execution_id: &str, node_ids: &[String]) {
        self.cancel_execution(execution_id);

        for node_id in node_ids {
            let session_id = format!("wf_{}_{}", execution_id, node_id);
            self.agent_manager_shared.stop_generation_no_mut(&session_id);
        }
        log::info!("[NodeExecutor] cancel_and_kill: exec={}, nodes={:?}", execution_id, node_ids);
    }



    pub fn new(agent_manager: Arc<AsyncMutex<AgentManager>>, agent_manager_shared: AgentManager, app_handle: tauri::AppHandle, pool: DbPool) -> Self {
        let agent_manager_clone = agent_manager.clone();
        let mut registry = WorkflowNodeTypeRegistry::new();

        // 注册 6 种实体节点类型
        // 控制逻辑（条件/聚合/并行/延迟）由边/Gate/节点属性承载，不再需要独立执行器

        registry.register(NodeTypeRegistration {
            type_id: "agent".into(),
            name: "Agent 任务".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(AgentExecutor::new(agent_manager.clone(), pool.clone())),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_type": { "type": "string", "enum": ["claude", "hermes", "codex"] },
                    "prompt_template": { "type": "string" },
                }
            })),
            permissions: vec![],
        });


        let human_input_manager = Arc::new(InteractManager::new());
        let plugin_execute_manager = Arc::new(PluginExecuteManager::new());
        let agent_manager_field = agent_manager_clone;



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
            type_id: "subflow".into(),
            name: "子工作流".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(NoopExecutor),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "definitionId": { "type": "string", "description": "子工作流定义 ID" },
                    "input_mapping": { "type": "object", "description": "输入映射" },
                    "output_mapping": { "type": "object", "description": "输出映射" },
                }
            })),
            permissions: vec![],
        });

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

        // 统一的插件调用节点（运行时从 node.config 读取 plugin_id / command_id）
        registry.register(NodeTypeRegistration {
            type_id: "plugin".into(),
            name: "插件调用".into(),
            category: NodeCategory::Builtin,
            executor: Arc::new(PluginExecutor::new(
                String::new(),
                "plugin".into(),
                plugin_execute_manager.clone(),
            )),
            config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "plugin_id": { "type": "string", "description": "目标插件 ID" },
                    "command_id": { "type": "string", "description": "目标命令 ID" },
                },
                "required": ["plugin_id", "command_id"]
            })),
            permissions: vec![],
        });


        let registry_arc = Arc::new(std::sync::Mutex::new(registry));

        // 注意：set_plugin_execute_manager / set_register_node_type 回调
        // 在首次 sync_plugin_node_types() 中通过 AppHandle → managed state 设置（此处 PluginHost 为空，跳过）


        Self {
            registry: registry_arc,
            human_input_manager,
            plugin_execute_manager,
            app_handle,
            pool,
            agent_manager: agent_manager_field,
            running_executions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            agent_manager_shared,
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

    /// 同步插件回调到 PluginHost（不注册独立节点类型，插件的命令选择在 plugin 节点配置中完成）
    pub fn sync_plugin_node_types(&self) {
        let host = self.app_handle.state::<std::sync::Mutex<PluginHost>>();
        let _plugin_host = match host.lock() {
            Ok(mut h) => {
                // 设置回调供 JS 端 PluginAPI 使用
                let reg_arc = self.registry.clone();
                h.set_register_node_type(Box::new(move |registration: NodeTypeRegistration| {
                    if let Ok(mut reg) = reg_arc.lock() {
                        reg.register(registration);
                    }
                }));
                h.set_plugin_execute_manager(self.plugin_execute_manager.clone());
                h
            },
            Err(e) => {
                log::warn!("[NodeExecutor] 获取 PluginHost 锁失败: {}", e);
                return;
            }
        };
        // 插件命令通过 plugin 节点 + 用户选择 plugin_id/command_id 调用，
        // 此处不再注册独立节点类型
    }

    pub fn list_node_types(&self) -> Vec<NodeTypeRegistrationInfo> {
        self.registry.lock()
            .map(|reg| reg.get_all_registrations())
            .unwrap_or_default()
    }
}
