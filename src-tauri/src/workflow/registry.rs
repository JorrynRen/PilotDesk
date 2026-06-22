use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use crate::utils::errors::AppError;

/// 节点定义（执行时上下文）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDef {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub label: String,
    pub config: Value,          // 节点类型特定配置
    pub plugin_id: Option<String>,
    pub command_id: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub retry_count: Option<u32>,
    pub retry_interval_ms: Option<u64>,
}

/// 节点输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeOutput {
    pub output: Value,
}

/// 节点执行器 trait — 所有节点类型实现此接口
#[async_trait]
pub trait NodeExecutorTrait: Send + Sync {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError>;
}

/// 节点类型分类
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum NodeCategory {
    Builtin,
    Plugin(String),  // 插件 ID
}

/// 节点类型注册信息
pub struct NodeTypeRegistration {
    pub type_id: String,
    pub name: String,
    pub category: NodeCategory,
    pub executor: Arc<dyn NodeExecutorTrait>,
    pub config_schema: Option<Value>,
    #[allow(dead_code)]
    pub permissions: Vec<String>,
}

/// 节点类型注册信息（序列化版本，用于前端同步）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTypeRegistrationInfo {
    pub type_id: String,
    pub name: String,
    pub category: String,
    pub config_schema: Option<Value>,
}

/// 节点类型注册表
pub struct WorkflowNodeTypeRegistry {
    entries: HashMap<String, NodeTypeRegistration>,
}

impl WorkflowNodeTypeRegistry {
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// 注册节点类型
    pub fn register(&mut self, registration: NodeTypeRegistration) {
        self.entries.insert(registration.type_id.clone(), registration);
    }

    /// 注销节点类型
    pub fn unregister(&mut self, type_id: &str) {
        self.entries.remove(type_id);
    }

    /// 获取执行器
    pub fn get_executor(&self, type_id: &str) -> Option<Arc<dyn NodeExecutorTrait>> {
        self.entries.get(type_id).map(|r| r.executor.clone())
    }

    /// 获取所有注册类型
    pub fn get_all_registrations(&self) -> Vec<NodeTypeRegistrationInfo> {
        self.entries.iter().map(|(id, reg)| NodeTypeRegistrationInfo {
            type_id: id.clone(),
            name: reg.name.clone(),
            category: match &reg.category {
                NodeCategory::Builtin => "builtin".to_string(),
                NodeCategory::Plugin(pid) => format!("plugin:{}", pid),
            },
            config_schema: reg.config_schema.clone(),
        }).collect()
    }
}
