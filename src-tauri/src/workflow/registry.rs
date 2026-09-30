use crate::utils::errors::AppError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// 节点定义（执行时上下文）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDef {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub label: String,
    pub config: Value, // 节点类型特定配置
    pub plugin_id: Option<String>,
    pub command_id: Option<String>,
    /// 节点超时（毫秒；`None` / `0` = 不限制）。
    /// 引擎按它给节点执行加硬上限：配置了就以它为准，未配置时各 executor 用自身默认值。
    pub timeout_ms: Option<u64>,
}

/// 节点输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeOutput {
    pub output: Value,
    /// Agent 节点的会话 session_id（用于后续节点延续会话）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 节点执行最终输入（agent 节点为最终提示词，覆盖 running 时写入的输入映射）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_data: Option<String>,
    /// 节点执行工件路径（agent 节点为工作区路径）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts_path: Option<String>,
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
    Plugin(String), // 插件 ID
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
        Self {
            entries: HashMap::new(),
        }
    }

    /// 注册节点类型
    pub fn register(&mut self, registration: NodeTypeRegistration) {
        self.entries
            .insert(registration.type_id.clone(), registration);
    }

    /// 注销节点类型
    pub fn unregister(&mut self, type_id: &str) {
        self.entries.remove(type_id);
    }

    /// 按插件批量注销其贡献的全部节点类型。
    ///
    /// 必须按 `NodeCategory::Plugin(plugin_id)` 精确匹配，而不是靠 type_id 前缀猜测：
    /// type_id 由插件自由命名，前缀可能与其他插件/内置节点相似甚至重名，
    /// 用前缀猜会误删别的插件的节点类型。`Builtin` 一律保留。
    pub fn unregister_plugin(&mut self, plugin_id: &str) {
        self.entries.retain(|_, reg| match &reg.category {
            NodeCategory::Plugin(pid) => pid != plugin_id,
            NodeCategory::Builtin => true,
        });
    }

    /// 列出当前注册表中所有"贡献了节点类型"的插件 id（已去重）。
    ///
    /// 供 PluginHost 收敛"磁盘上已被手动删除、但节点类型仍留在注册表"的插件使用。
    /// 直接按 `NodeCategory::Plugin(pid)` 判定，不解析 `get_all_registrations()` 里的
    /// `"plugin:<pid>"` 字符串——插件 id 允许包含 ':'，字符串切分会误判。
    pub fn registered_plugin_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .entries
            .values()
            .filter_map(|reg| match &reg.category {
                NodeCategory::Plugin(pid) => Some(pid.clone()),
                NodeCategory::Builtin => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// 获取执行器
    pub fn get_executor(&self, type_id: &str) -> Option<Arc<dyn NodeExecutorTrait>> {
        self.entries.get(type_id).map(|r| r.executor.clone())
    }

    /// 获取所有注册类型
    pub fn get_all_registrations(&self) -> Vec<NodeTypeRegistrationInfo> {
        self.entries
            .iter()
            .map(|(id, reg)| NodeTypeRegistrationInfo {
                type_id: id.clone(),
                name: reg.name.clone(),
                category: match &reg.category {
                    NodeCategory::Builtin => "builtin".to_string(),
                    NodeCategory::Plugin(pid) => format!("plugin:{}", pid),
                },
                config_schema: reg.config_schema.clone(),
            })
            .collect()
    }
}
