use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;
use crate::utils::errors::AppError;

/// 缓存模板变量正则表达式，避免每次 resolve 调用重新编译
static TEMPLATE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{\{(.+?)\}\}").expect("模板正则编译失败")
});

/// 模板引擎 — 解析 {{variable}} 和 JSONPath 表达式
pub struct TemplateEngine;

impl TemplateEngine {
    /// 解析模板字符串，替换所有 {{variable}} 占位符
    /// 预处理模板表达式，统一为 context 可查找的格式
    /// 新架构格式：{{key.节点ID.阶段ID}} → context 中以 nodeId 为 key 存储，解析为 nodeId -> key
    /// 新架构格式：{{key.节点ID.阶段ID}} → {{节点ID.key}}
    fn expand_short_format(template: &str) -> String {
        // 新格式：{{参数名.节点ID.阶段ID}} → {{节点ID.参数名}}
        // 三段式：key.nodeId.stageId → nodeId.key（忽略 stageId，context 按 nodeId 索引）
        let re_new = Regex::new(r"\{\{([a-zA-Z_]\w*)\.([a-zA-Z0-9_-]+)\.([a-zA-Z0-9_-]+)\}\}").unwrap();
        let template = re_new.replace_all(template, "{{$2.$1}}").to_string();



        template.to_string()
    }

    pub fn resolve(
        template: &str,
        context: &HashMap<String, Value>,
    ) -> Result<String, AppError> {
        let template = Self::expand_short_format(template);
        let re = &*TEMPLATE_REGEX;
        let mut result = template.to_string();

        for cap in re.captures_iter(&template) {
            let expression = cap.get(1).unwrap().as_str().trim();
            let resolved = Self::resolve_expression(expression, context)?;
            result = result.replace(&cap[0], &resolved);
        }

        Ok(result)
    }

    #[allow(dead_code)]
    /// 解析对象中的所有字符串字段（递归）
    pub fn resolve_value(
        value: &Value,
        context: &HashMap<String, Value>,
    ) -> Result<Value, AppError> {
        match value {
            Value::String(s) => {
                let resolved = Self::resolve(s, context)?;
                Ok(Value::String(resolved))
            }
            Value::Object(map) => {
                let mut new_map = serde_json::Map::new();
                for (k, v) in map {
                    new_map.insert(k.clone(), Self::resolve_value(v, context)?);
                }
                Ok(Value::Object(new_map))
            }
            Value::Array(arr) => {
                let mut new_arr = Vec::new();
                for v in arr {
                    new_arr.push(Self::resolve_value(v, context)?);
                }
                Ok(Value::Array(new_arr))
            }
            other => Ok(other.clone()),
        }
    }

    /// 解析单个表达式，支持 JSONPath
    fn resolve_expression(expr: &str, context: &HashMap<String, Value>) -> Result<String, AppError> {
        // 支持特殊变量（__trigger__/__input__ 是引擎内部 key，也支持无前缀别名）
        if expr == "trigger.output" || expr == "input" || expr == "__input__" {
            if let Some(val) = context.get("__trigger__").or_else(|| context.get("__input__")) {
                return Ok(Self::value_to_string(val));
            }
        }

        // 优先完整匹配（支持 gate_output.stageId 等含点号的 context key）
        if let Some(val) = context.get(expr) {
            return Ok(Self::value_to_string(val));
        }

        let parts: Vec<&str> = expr.splitn(2, '.').collect();
        if parts.len() < 2 {
            // 简单变量名（无点号），直接查 context
            if let Some(val) = context.get(expr) {
                return Ok(Self::value_to_string(val));
            }
            return Err(AppError::InvalidInput(format!("无效的模板变量: {}", expr)));
        }

        let first = parts[0];
        let rest = parts[1];

        let value = context.get(first)
            .ok_or_else(|| AppError::NotFound(format!("变量 {} 的输出不存在", first)))?;

        Self::jsonpath_extract(value, rest)
    }

    fn jsonpath_extract(value: &Value, path: &str) -> Result<String, AppError> {
        let mut current = value.clone();

        for segment in path.split('.') {
            if segment.is_empty() { continue; }

            if let Some(idx_start) = segment.find('[') {
                let field = &segment[..idx_start];
                let idx_str = &segment[idx_start+1..segment.len()-1];

                if !field.is_empty() {
                    current = current.get(field)
                        .ok_or_else(|| AppError::NotFound(format!("字段 {} 不存在", field)))?
                        .clone();
                }

                let idx: usize = idx_str.parse()
                    .map_err(|_| AppError::InvalidInput(format!("无效的数组索引: {}", idx_str)))?;
                current = current.get(idx)
                    .ok_or_else(|| AppError::NotFound(format!("数组索引 {} 越界", idx)))?
                    .clone();
            } else {
                current = current.get(segment)
                    .ok_or_else(|| AppError::NotFound(format!("字段 {} 不存在", segment)))?
                    .clone();
            }
        }

        Ok(Self::value_to_string(&current))
    }

    fn value_to_string(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            other => serde_json::to_string(other).unwrap_or_default(),
        }
    }
}
