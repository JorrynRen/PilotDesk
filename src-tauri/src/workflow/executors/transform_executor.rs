use async_trait::async_trait;
use serde_json::Value;
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

        if script.trim().is_empty() {
            return Err(AppError::Config("transform 节点 script 不能为空".into()));
        }

        let result = execute_js(script, &resolved_input)?;

        // 如果脚本返回 null/undefined（通常是 NaN 或执行异常），
        // 返回明确的错误信息，帮助用户定位问题（而非静默返回空对象）
        if result.is_null() {
            return Err(AppError::External(
                "转换脚本返回了 null/undefined（可能是 NaN 或缺少 return）\n\
                 请检查：\n\
                 1. 脚本中 input.xxx 的值是否为有效数字（inputMapping 模板是否正确解析）\n\
                 2. 数值计算前是否需要用 Number() 显式转换（所有 inputMapping 值均为字符串类型）\n\
                 3. 脚本是否包含 return 语句或最后一个表达式".to_string()
            ));
        }

        Ok(NodeOutput {
            output: result,
            session_id: None,
            input_data: None,
            artifacts_path: None,
        })
    }
}

/// 使用 boa_engine 执行 JavaScript 转换脚本
///
/// 脚本中可访问的变量：
///   - `input`：上游 inputMapping 解析后的对象（如 { key1: "v1", key2: "v2" }）
///   - `inputs`：`input` 的别名（向后兼容）
///   - `context`：`input` 的别名（与配置面板 placeholder 描述一致）
///
/// 脚本可通过 `return` 语句或最后一个表达式作为输出。
/// 输出会被 to_json 序列化为 serde_json::Value。
fn execute_js(script: &str, input: &Value) -> Result<Value, AppError> {
    let mut engine = boa_engine::Context::default();

    // 注入 input / inputs / context 三个变量
    let input_json = serde_json::to_string(input)
        .map_err(|e| AppError::Json(e.to_string()))?;
    let setup = format!(
        "var input = {}; var inputs = input; var context = input;",
        input_json
    );
    engine
        .eval(boa_engine::Source::from_bytes(&setup))
        .map_err(|e| AppError::External(format!("JS 注入变量失败: {}", e)))?;

    // 预处理脚本：若不含 return 语句，将最后一行作为表达式自动加 return
    let processed_script = preprocess_script(script);

    // 包裹 IIFE 以支持 return 语句
    let wrapped = format!(
        "(function() {{\n{}\n}})()",
        processed_script
    );

    let result = engine
        .eval(boa_engine::Source::from_bytes(&wrapped))
        .map_err(|e| {
            // 提取脚本前 200 字符作为上下文，便于调试
            let snippet: String = script.chars().take(200).collect();
            AppError::External(format!(
                "JS 执行错误: {}\n--- 脚本片段(前200字符) ---\n{}",
                e, snippet
            ))
        })?;

    // undefined → null → 空对象
    let result_json = if result.is_undefined() {
        Value::Null
    } else {
        result
            .to_json(&mut engine)
            .map_err(|e| AppError::External(format!("JS 序列化错误: {}", e)))?
    };

    Ok(result_json)
}

/// 脚本预处理：若脚本未包含 return 语句，将最后一个非空行视为表达式自动加 return。
///
/// 这样用户可以写 `input.x * 2` 而无需显式 return。
/// 已包含 return 的脚本不做处理。
fn preprocess_script(script: &str) -> String {
    if script.contains("return ") || script.contains("return\t") || script.contains("return\n") {
        return script.to_string();
    }

    let lines: Vec<&str> = script.lines().collect();
    // 找到最后一个非空非注释行
    let mut last_idx = None;
    for (i, line) in lines.iter().enumerate().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") {
            continue;
        }
        last_idx = Some(i);
        break;
    }

    let Some(last_idx) = last_idx else {
        return script.to_string();
    };

    // 不转换的情况：最后一行已经是语句（以 ; 结尾 或 块声明）
    let last_line = lines[last_idx].trim_end();
    if last_line.ends_with(';')
        || last_line.ends_with('}')
        || last_line.ends_with('{')
        || last_line.ends_with(':')
    {
        return script.to_string();
    }

    // 在最后一行前加 return
    let mut new_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    let original = &new_lines[last_idx];
    let leading_ws: String = original.chars().take_while(|c| c.is_whitespace()).collect();
    new_lines[last_idx] = format!("{}return {}", leading_ws, original.trim_start());
    new_lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_return_statement() {
        let input = json!({ "name": "alice", "age": 30 });
        let script = "return { label: input.name, value: input.age };";
        let result = execute_js(script, &input).unwrap();
        assert_eq!(result, json!({ "label": "alice", "value": 30 }));
    }

    #[test]
    fn test_last_expression() {
        let input = json!({ "x": 10 });
        // 没有 return，最后一行表达式作为 IIFE 返回值
        let script = "var doubled = input.x * 2;\ndoubled";
        let result = execute_js(script, &input).unwrap();
        assert_eq!(result, json!(20));
    }

    #[test]
    fn test_inputs_alias() {
        let input = json!({ "key": "v" });
        let script = "return inputs.key;";
        let result = execute_js(script, &input).unwrap();
        assert_eq!(result, json!("v"));
    }

    #[test]
    fn test_context_alias() {
        let input = json!({ "key": "v" });
        let script = "return context.key;";
        let result = execute_js(script, &input).unwrap();
        assert_eq!(result, json!("v"));
    }
}
