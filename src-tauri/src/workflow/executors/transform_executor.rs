use crate::utils::errors::AppError;
use crate::workflow::registry::{NodeDef, NodeExecutorTrait, NodeOutput};
use async_trait::async_trait;
use serde_json::Value;
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
        let script = node
            .config
            .get("script")
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
                 1. 输入映射字段的值是否为有效数字（inputMapping 模板是否正确解析）\n\
                 2. 数值计算前确认类型（输入映射值已按内容推断类型，必要时用 Number() 显式转换）\n\
                 3. 脚本是否包含 return 语句或最后一个表达式"
                    .to_string(),
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
/// 脚本中可访问的变量 = 本节点「输入映射」里配置的字段名，直接当顶层变量使用
/// （映射配了 `input1` 就写 `input1`；多字段对象可继续下钻 `input1.result`），
/// 与其它节点类型模板里的 `{{字段名}}` 取值语义保持一致。
///
/// 字段名与 JS 保留字/内置对象同名（如 `for`、`JSON`、`Math`）时无法作为变量注入，会被跳过。
/// 没有全局上下文变量：其它节点的输出必须通过本节点的「输入映射」显式引用。
///
/// 脚本可通过 `return` 语句或最后一个表达式作为输出。
/// 输出会被 to_json 序列化为 serde_json::Value。
fn execute_js(script: &str, input: &Value) -> Result<Value, AppError> {
    let mut engine = boa_engine::Context::default();

    // 按输入映射字段名注入同名顶层变量
    let setup = build_variable_setup(input);
    if !setup.is_empty() {
        engine
            .eval(boa_engine::Source::from_bytes(&setup))
            .map_err(|e| AppError::External(format!("JS 注入变量失败: {}", e)))?;
    }

    // 预处理脚本：若不含 return 语句，将最后一行作为表达式自动加 return
    let processed_script = preprocess_script(script);

    // 包裹 IIFE 以支持 return 语句
    let wrapped = format!("(function() {{\n{}\n}})()", processed_script);
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

/// 生成「输入映射字段名 → 同名顶层变量」的注入脚本
///
/// 非法标识符、或与 JS 保留字/内置对象同名的字段会跳过注入（仅告警），避免整个脚本 SyntaxError，
/// 或把 `JSON`/`Math` 这类内置对象覆盖掉导致脚本里的内置调用失效。
fn build_variable_setup(input: &Value) -> String {
    let Some(map) = input.as_object() else {
        return String::new();
    };
    let mut setup = String::new();
    for (key, value) in map {
        if !is_valid_script_variable(key) {
            log::warn!(
                "[TransformExecutor] 输入映射字段 '{}' 不能作为脚本变量（非法标识符或与保留字/内置对象同名），已跳过注入",
                key
            );
            continue;
        }
        match serde_json::to_string(value) {
            Ok(json) => setup.push_str(&format!("var {} = {};\n", key, json)),
            Err(e) => log::warn!(
                "[TransformExecutor] 输入映射字段 '{}' 序列化失败: {}",
                key,
                e
            ),
        }
    }
    setup
}

/// 字段名能否直接作为脚本顶层变量：合法标识符且不与 JS 保留字/常用内置对象同名
fn is_valid_script_variable(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$') {
        return false;
    }
    !JS_SCRIPT_RESERVED.contains(&name)
}

/// JS 保留字、字面量与常用内置对象——与它们同名的输入映射字段不能作为脚本变量
const JS_SCRIPT_RESERVED: &[&str] = &[
    // 关键字与保留字
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    // 常用内置对象/全局函数
    "Array",
    "Boolean",
    "Date",
    "Error",
    "Infinity",
    "JSON",
    "Map",
    "Math",
    "NaN",
    "Number",
    "Object",
    "Promise",
    "RegExp",
    "Set",
    "String",
    "Symbol",
    "console",
    "globalThis",
    "parseFloat",
    "parseInt",
    "undefined",
];

/// 脚本预处理：若脚本未包含 return 语句，将最后一个非空行视为表达式自动加 return。
///
/// 这样用户可以直接写表达式（如 `age * 2`）而无需显式 return。
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
        let script = "return { label: name, value: age };";
        let result = execute_js(script, &input).unwrap();
        assert_eq!(result, json!({ "label": "alice", "value": 30 }));
    }

    #[test]
    fn test_last_expression() {
        let input = json!({ "x": 10 });
        // 没有 return，最后一行表达式作为 IIFE 返回值
        let script = "var doubled = x * 2;\ndoubled";
        let result = execute_js(script, &input).unwrap();
        assert_eq!(result, json!(20));
    }

    #[test]
    fn test_flat_variable_drills_into_object() {
        // 输入映射字段名直接当变量用；多字段对象可继续下钻
        let input = json!({ "input1": { "result": 0.98, "level": "high" } });
        assert_eq!(
            execute_js("return input1.result;", &input).unwrap(),
            json!(0.98)
        );
        assert_eq!(execute_js("input1.level", &input).unwrap(), json!("high"));
    }

    #[test]
    fn test_reserved_key_is_skipped_but_builtins_keep_working() {
        // 字段名与内置对象同名时跳过注入：既不报 SyntaxError，也不覆盖 JSON 内置对象
        let input = json!({ "JSON": { "a": 1 }, "for": 2, "1bad": 3, "ok": 4 });
        let result = execute_js(
            "return { parsed: JSON.parse('{\"a\":1}').a, ok: ok };",
            &input,
        )
        .unwrap();
        assert_eq!(result, json!({ "parsed": 1, "ok": 4 }));
    }
}
