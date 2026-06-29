use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeOutput, NodeExecutorTrait};

pub struct TransformExecutor;

#[async_trait]
impl NodeExecutorTrait for TransformExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        _execution_id: &str,
        _emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let script = node.config.get("script")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Config("transform 节点缺少 script 配置".into()))?;

        let mut inputs = HashMap::new();
        inputs.insert("input".to_string(), resolved_input);

        let result = execute_js(script, &inputs)?;
        Ok(NodeOutput { output: result, session_id: None })
    }
}

/// 使用 boa_engine 执行 JavaScript 转换脚本
fn execute_js(script: &str, inputs: &HashMap<String, Value>) -> Result<Value, AppError> {
    let mut engine = boa_engine::Context::default();

    let inputs_json = serde_json::to_string(inputs)
        .map_err(|e| AppError::Json(e.to_string()))?;
    engine.eval(
        boa_engine::Source::from_bytes(&format!("const inputs = {};", inputs_json))
    ).map_err(|e| AppError::External(format!("JS 执行错误: {}", e)))?;

    let result = engine.eval(boa_engine::Source::from_bytes(script))
        .map_err(|e| AppError::External(format!("JS 执行错误: {}", e)))?;

    let result_json = result.to_json(&mut engine)
        .map_err(|e| AppError::External(format!("JS 序列化错误: {}", e)))?;

    Ok(result_json)
}
