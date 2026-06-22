use async_trait::async_trait;
use serde_json::Value;
use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeOutput, NodeExecutorTrait};

/// 条件分支执行器
pub struct ConditionExecutor;

#[async_trait]
impl NodeExecutorTrait for ConditionExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        _execution_id: &str,
        _emitter: &tauri::AppHandle,
    ) -> Result<NodeOutput, AppError> {
        let expression = node.config.get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Config("condition 节点缺少 expression 配置".into()))?;

        let output_true = node.config.get("output_true")
            .and_then(|v| v.as_str())
            .unwrap_or("true");
        let output_false = node.config.get("output_false")
            .and_then(|v| v.as_str())
            .unwrap_or("false");

        // 简单表达式求值：支持 includes / == / != / > / < / >= / <=
        let result = evaluate_condition(expression, &resolved_input)?;

        Ok(NodeOutput {
            output: Value::String(if result {
                output_true.to_string()
            } else {
                output_false.to_string()
            }),
        })
    }
}

/// 简单条件表达式求值
fn evaluate_condition(expr: &str, input: &Value) -> Result<bool, AppError> {
    let expr = expr.trim();

    // 处理 includes 表达式: "field.includes('value')"
    if let Some(cap) = extract_includes(expr) {
        let (field, search_value) = cap;
        let field_val = resolve_field(input, &field);
        match field_val {
            Value::String(s) => Ok(s.contains(&search_value)),
            Value::Array(arr) => Ok(arr.iter().any(|v| {
                v.as_str().map_or(false, |s| s == search_value)
                    || v.as_str().map_or(false, |s| s.contains(&search_value))
            })),
            _ => Ok(false),
        }
    }
    // 处理 == 比较
    else if let Some((left, right)) = extract_binary(expr, "==") {
        let left_val = resolve_value(input, left.trim());
        let right_val = resolve_value(input, right.trim());
        Ok(left_val == right_val)
    }
    // 处理 != 比较
    else if let Some((left, right)) = extract_binary(expr, "!=") {
        let left_val = resolve_value(input, left.trim());
        let right_val = resolve_value(input, right.trim());
        Ok(left_val != right_val)
    }
    // 处理 > 比较
    else if let Some((left, right)) = extract_binary(expr, ">=") {
        let left_val = resolve_value(input, left.trim());
        let right_val = resolve_value(input, right.trim());
        Ok(compare_values(&left_val, &right_val) >= 0)
    }
    else if let Some((left, right)) = extract_binary(expr, ">") {
        let left_val = resolve_value(input, left.trim());
        let right_val = resolve_value(input, right.trim());
        Ok(compare_values(&left_val, &right_val) > 0)
    }
    else if let Some((left, right)) = extract_binary(expr, "<=") {
        let left_val = resolve_value(input, left.trim());
        let right_val = resolve_value(input, right.trim());
        Ok(compare_values(&left_val, &right_val) <= 0)
    }
    else if let Some((left, right)) = extract_binary(expr, "<") {
        let left_val = resolve_value(input, left.trim());
        let right_val = resolve_value(input, right.trim());
        Ok(compare_values(&left_val, &right_val) < 0)
    }
    else {
        // 纯布尔值
        match expr {
            "true" | "True" | "TRUE" => Ok(true),
            "false" | "False" | "FALSE" => Ok(false),
            _ => Err(AppError::Config(format!("无法解析条件表达式: {}", expr))),
        }
    }
}

fn extract_includes(expr: &str) -> Option<(String, String)> {
    let expr = expr.trim();
    if let Some(pos) = expr.find(".includes(") {
        let field = expr[..pos].trim().to_string();
        let rest = &expr[pos + 10..];
        if let Some(end) = rest.rfind(')') {
            let arg = rest[..end].trim();
            let value = arg.trim_matches('\'').trim_matches('"').to_string();
            return Some((field, value));
        }
    }
    None
}

fn extract_binary(expr: &str, op: &str) -> Option<(String, String)> {
    let expr = expr.trim();
    if let Some(pos) = expr.find(op) {
        let left = expr[..pos].trim().to_string();
        let right = expr[pos + op.len()..].trim().to_string();
        if !left.is_empty() && !right.is_empty() {
            return Some((left, right));
        }
    }
    None
}

fn resolve_field(input: &Value, field: &str) -> Value {
    let parts: Vec<&str> = field.split('.').collect();
    let mut current = input.clone();
    for part in parts {
        match current.get(part) {
            Some(v) => current = v.clone(),
            None => return Value::Null,
        }
    }
    current
}

fn resolve_value(input: &Value, raw: &str) -> Value {
    // 如果是引号包裹的字符串字面量
    if (raw.starts_with('\'') && raw.ends_with('\''))
        || (raw.starts_with('"') && raw.ends_with('"'))
    {
        return Value::String(raw[1..raw.len()-1].to_string());
    }

    // 如果是数字字面量
    if let Ok(n) = raw.parse::<i64>() {
        return Value::Number(serde_json::Number::from(n));
    }
    if let Ok(n) = raw.parse::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(n) {
            return Value::Number(num);
        }
    }

    // 如果是布尔字面量
    match raw {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }

    // 否则视为字段路径
    resolve_field(input, raw)
}

fn compare_values(a: &Value, b: &Value) -> i32 {
    match (a, b) {
        (Value::Number(na), Value::Number(nb)) => {
            let fa = na.as_f64().unwrap_or(0.0);
            let fb = nb.as_f64().unwrap_or(0.0);
            if fa > fb { 1 } else if fa < fb { -1 } else { 0 }
        }
        (Value::String(sa), Value::String(sb)) => {
            match sa.cmp(sb) {
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
            }
        }
        _ => 0,
    }
}
