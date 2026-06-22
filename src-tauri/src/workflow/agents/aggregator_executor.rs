use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use crate::utils::errors::AppError;
use super::super::registry::{NodeDef, NodeOutput, NodeExecutorTrait};

/// 聚合节点执行器
///
/// 合并多个上游节点的输出，支持三种策略：
/// - merge: 将多个 JSON Object 合并为一个
/// - concat: 将多个 JSON Array 拼接为一个
/// - pick_first: 取第一个非空输出
pub struct AggregatorExecutor;

#[async_trait]
impl NodeExecutorTrait for AggregatorExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        _execution_id: &str,
        _emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let config = node.config.as_object().ok_or_else(|| {
            AppError::Config("aggregator 配置必须是 JSON Object".into())
        })?;

        let strategy = config
            .get("strategy")
            .and_then(|v| v.as_str())
            .unwrap_or("merge");

        let input_sources = config
            .get("input_sources")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // 从 resolved_input 中提取各上游输出
        let inputs: HashMap<String, Value> = if let Some(obj) = resolved_input.as_object() {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        } else {
            HashMap::new()
        };

        let output = match strategy {
            "concat" => {
                // 拼接所有数组
                let mut result = Vec::new();
                for src in &input_sources {
                    if let Some(val) = inputs.get(src) {
                        if let Some(arr) = val.as_array() {
                            result.extend(arr.clone());
                        } else {
                            result.push(val.clone());
                        }
                    }
                }
                Value::Array(result)
            }
            "pick_first" => {
                // 取第一个非空输出
                let mut result = Value::Null;
                for src in &input_sources {
                    if let Some(val) = inputs.get(src) {
                        if !val.is_null() {
                            result = val.clone();
                            break;
                        }
                    }
                }
                result
            }
            _ => {
                // merge: 合并所有 Object
                let mut merged = serde_json::Map::new();
                for src in &input_sources {
                    if let Some(val) = inputs.get(src) {
                        if let Some(obj) = val.as_object() {
                            merged.extend(obj.clone());
                        } else {
                            merged.insert(src.clone(), val.clone());
                        }
                    }
                }
                Value::Object(merged)
            }
        };

        Ok(NodeOutput { output })
    }
}
