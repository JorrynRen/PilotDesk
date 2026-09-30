use crate::utils::errors::AppError;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

/// 缓存模板变量正则表达式，避免每次 resolve 调用重新编译
static TEMPLATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{(.+?)\}\}").expect("模板正则编译失败"));

/// 模板引擎 — 解析 {{variable}} 和 JSONPath 表达式
pub struct TemplateEngine;
impl TemplateEngine {
    /// 解析模板字符串，替换所有 {{variable}} 占位符
    /// 预处理模板表达式，统一为 context 可查找的格式
    /// 新架构格式：{{key.节点ID.阶段ID}} → context 中以 nodeId 为 key 存储，解析为 nodeId -> key
    /// 新架构格式：{{key.节点ID.阶段ID}} → {{节点ID.key}}
    /// 展开输入映射简写：
    ///   `参数名.节点ID.阶段ID`        → `节点ID.参数名`
    ///   `参数名.子路径.节点ID.阶段ID`  → `节点ID.参数名.子路径`
    ///
    /// 以 `gate_output.` / `__` 开头的是引擎内建变量（如 `gate_output.<阶段ID>`），
    /// **不参与简写改写**——否则 `{{gate_output.<阶段ID>.<字段>}}` 会被误改写成
    /// `{{<阶段ID>.gate_output}}` 导致解析失败。
    fn expand_shorthand_expr(expr: &str) -> String {
        if expr.starts_with("gate_output.") || expr.starts_with("__") {
            return expr.to_string();
        }

        // 标识符：字母/下划线开头，仅含字母数字下划线
        fn is_ident(s: &str) -> bool {
            let mut chars = s.chars();
            match chars.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
                _ => return false,
            }
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }

        // 节点/阶段 ID：字母数字下划线连字符，且不以连字符开头
        fn is_id_like(s: &str) -> bool {
            !s.is_empty()
                && !s.starts_with('-')
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }

        // 子路径：`a`、`a[0]`、`a.b[1].c` 形式
        fn is_path_like(s: &str) -> bool {
            s.split('.').all(|seg| {
                let (name, indices) = match seg.split_once('[') {
                    Some((name, rest)) => (name, Some(rest)),
                    None => (seg, None),
                };
                if !is_ident(name) {
                    return false;
                }
                match indices {
                    Some(rest) => {
                        rest.ends_with(']')
                            && rest[..rest.len() - 1].chars().all(|c| c.is_ascii_digit())
                    }
                    None => true,
                }
            })
        }

        let parts: Vec<&str> = expr.split('.').collect();
        match parts.as_slice() {
            [key, node, stage] if is_ident(key) && is_id_like(node) && is_id_like(stage) => {
                format!("{}.{}", node, key)
            }
            [key, sub, node, stage]
                if is_ident(key) && is_path_like(sub) && is_id_like(node) && is_id_like(stage) =>
            {
                format!("{}.{}.{}", node, key, sub)
            }
            _ => expr.to_string(),
        }
    }

    pub fn resolve(template: &str, context: &HashMap<String, Value>) -> Result<String, AppError> {
        let mut result = String::new();
        let mut cursor = 0usize;
        for cap in TEMPLATE_REGEX.captures_iter(template) {
            let whole = cap.get(0).unwrap();
            result.push_str(&template[cursor..whole.start()]);
            let expression = cap.get(1).unwrap().as_str().trim();
            let expanded = Self::expand_shorthand_expr(expression);
            result.push_str(&Self::resolve_expression(&expanded, context)?);
            cursor = whole.end();
        }

        result.push_str(&template[cursor..]);
        Ok(result)
    }

    /// 模板中是否含 `{{...}}` 占位符（用于区分"字面量"与"引用"）
    pub fn has_placeholder(template: &str) -> bool {
        TEMPLATE_REGEX.is_match(template)
    }

    /// 测试用：暴露简写展开逻辑（跨模块单测校验 `gate_output.` 前缀不被误改写）
    #[cfg(test)]
    pub(crate) fn expand_shorthand_for_test(expr: &str) -> String {
        Self::expand_shorthand_expr(expr)
    }

    /// 宽松解析：未解析的占位符替换为**空串**，并返回未解析的表达式列表。
    ///
    /// 用于"值不存在也应产出无值、而不是把 `{{...}}` 原文当值"的场景（输入映射、提示词、请求体）。
    pub fn resolve_lossy(
        template: &str,
        context: &HashMap<String, Value>,
    ) -> (String, Vec<String>) {
        let mut result = String::new();
        let mut unresolved: Vec<String> = Vec::new();
        let mut cursor = 0usize;
        for cap in TEMPLATE_REGEX.captures_iter(template) {
            let whole = cap.get(0).unwrap();
            result.push_str(&template[cursor..whole.start()]);
            let expression = cap.get(1).unwrap().as_str().trim();
            let expanded = Self::expand_shorthand_expr(expression);
            match Self::resolve_expression(&expanded, context) {
                Ok(value) => result.push_str(&value),
                Err(_) => unresolved.push(expression.to_string()),
            }

            cursor = whole.end();
        }

        result.push_str(&template[cursor..]);
        (result, unresolved)
    }

    /// 解析对象/数组中的所有字符串字段（递归），命中模板的按内容推断类型
    ///
    /// 与 `resolve` 的区别是不把未命中当作错误：未含模板的字符串原样保留（保持用户
    /// 在参数框里输入的类型），含模板但未解析的占位符按**空**处理（整串为纯占位符时归一为 `Null`），
    /// 避免把 `{{...}}` 原文当成真实值传给下游。
    pub fn resolve_value(value: &Value, context: &HashMap<String, Value>) -> Value {
        match value {
            Value::String(s) => Self::resolve_string(s, context),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Self::resolve_value(v, context)))
                    .collect(),
            ),
            Value::Array(arr) => Value::Array(
                arr.iter()
                    .map(|v| Self::resolve_value(v, context))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// 解析单个字符串：未含模板原样返回；含模板时未解析的占位符按空处理
    /// （整串就是一个未解析占位符时，空串会被 `infer_typed_value` 归一成 `Null`，
    /// 即"值不存在就是无值"，不再把 `{{...}}` 原文当值往下传）。
    fn resolve_string(s: &str, context: &HashMap<String, Value>) -> Value {
        if !TEMPLATE_REGEX.is_match(s) {
            return Value::String(s.to_string());
        }
        let (resolved, unresolved) = Self::resolve_lossy(s, context);
        if !unresolved.is_empty() {
            log::warn!(
                "[TemplateEngine] 模板未解析，对应位置按空处理: {} | 未解析: {:?}",
                s,
                unresolved
            );
        }
        infer_typed_value(&resolved)
    }

    /// 解析单个表达式，支持 JSONPath
    fn resolve_expression(
        expr: &str,
        context: &HashMap<String, Value>,
    ) -> Result<String, AppError> {
        // 精确匹配优先，含点号的扁平 key（如 gate_output.stageId）也走这里。
        //
        // 必须先于下面的保留别名：输入映射里叫 input / trigger 的键是**普通变量**，
        // 若让别名先命中，{{input}} 会取到整个输入对象（序列化成 JSON 字符串）
        // 而不是映射值，节点会拿到 `{"input":3,"input2":4}` 这种值。
        if let Some(val) = context.get(expr) {
            return Ok(Self::value_to_string(val));
        }

        // `gate_output.<阶段ID>.<路径>`：阶段合并值在 context 里是**整键** `gate_output.<阶段ID>`
        // （键名含点号），表达式多出下钻路径时不能按通用的「首段 = 顶层键」拆解，否则取不到值。
        if let Some(rest) = expr.strip_prefix("gate_output.") {
            let (stage_id, path) = match rest.split_once('.') {
                Some((stage_id, path)) => (stage_id, path),
                None => (rest, ""),
            };
            let key = format!("gate_output.{}", stage_id);
            let value = context
                .get(&key)
                .ok_or_else(|| AppError::NotFound(format!("变量 {} 的输出不存在", key)))?;
            return if path.is_empty() {
                Ok(Self::value_to_string(value))
            } else {
                Self::jsonpath_extract(value, path)
            };
        }

        // 保留别名（__trigger__/__input__ 是引擎内部 key，也支持无前缀别名）
        if expr == "trigger.output" || expr == "input" || expr == "__input__" {
            if let Some(val) = context
                .get("__trigger__")
                .or_else(|| context.get("__input__"))
            {
                return Ok(Self::value_to_string(val));
            }
        }

        let parts: Vec<&str> = expr.splitn(2, '.').collect();
        if parts.len() < 2 {
            // 简单变量名（无点号）：回退到 __input__ 查找（子工作流场景：{{title}} 自动匹配 __input__.title）
            if let Some(input_val) = context.get("__input__") {
                if let Some(val) = input_val.get(expr) {
                    return Ok(Self::value_to_string(val));
                }
            }
            return Err(AppError::InvalidInput(format!("无效的模板变量: {}", expr)));
        }

        let first = parts[0];
        let rest = parts[1];
        let value = context
            .get(first)
            .ok_or_else(|| AppError::NotFound(format!("变量 {} 的输出不存在", first)))?;
        Self::jsonpath_extract(value, rest)
    }

    /// 解析对象/数组路径，返回**原始值**（不做字符串化）
    ///
    /// 路径语法：`a.b`、`a[0].b`。当当前值是 JSON 文本（对象/数组）时，会先解析再继续下钻——
    /// 上游把 JSON 作为字符串输出（如 Agent 返回的结构化结果）时，无需再加转换节点取值。
    pub fn extract_value(value: &Value, path: &str) -> Result<Value, AppError> {
        let mut current = value.clone();
        for segment in path.split('.') {
            if segment.is_empty() {
                continue;
            }
            current = Self::extract_segment(&current, segment)?;
        }
        Ok(current)
    }

    /// 单段下钻（`field` 或 `field[idx]`）；当前值是 JSON 文本时先解析成结构
    fn extract_segment(current: &Value, segment: &str) -> Result<Value, AppError> {
        let base = Self::as_structural(current);
        let Some(idx_start) = segment.find('[') else {
            return base
                .get(segment)
                .ok_or_else(|| AppError::NotFound(format!("字段 {} 不存在", segment)))
                .cloned();
        };
        if !segment.ends_with(']') {
            return Err(AppError::InvalidInput(format!("无效的路径段: {}", segment)));
        }

        let field = &segment[..idx_start];
        let idx_str = &segment[idx_start + 1..segment.len() - 1];
        let container = if field.is_empty() {
            base
        } else {
            base.get(field)
                .ok_or_else(|| AppError::NotFound(format!("字段 {} 不存在", field)))?
                .clone()
        };
        // 字段值本身可能是 JSON 文本（如 "[{...}]"）：先解析再做下标
        let container = Self::as_structural(&container);
        let idx: usize = idx_str
            .parse()
            .map_err(|_| AppError::InvalidInput(format!("无效的数组索引: {}", idx_str)))?;
        container
            .get(idx)
            .ok_or_else(|| AppError::NotFound(format!("数组索引 {} 越界", idx)))
            .cloned()
    }

    /// 取值归一化（全节点类型统一的取值语义）
    ///
    /// 1. 字符串且是 JSON 对象/数组文本 → 解析为结构值（模板插值、脚本、条件都拿到结构）；
    /// 2. 「单字段对象 / 单元素数组」→ 解包取唯一值，单值结果无需用户指名字段
    ///    （已是结构值同样解包：插件返回 `{"result": x}` 时取值就是 `x`）；
    /// 3. 其它情况（多字段结构、解析失败、普通文本）原样返回。
    pub fn normalize_value(value: &Value) -> Value {
        let structural = Self::parse_json_text(value).unwrap_or_else(|| value.clone());
        Self::unwrap_single(structural)
    }

    /// 单字段对象 / 单元素数组 → 取唯一值；其它原样
    fn unwrap_single(value: Value) -> Value {
        match value {
            Value::Object(map) if map.len() == 1 => map
                .into_iter()
                .next()
                .map(|(_, v)| v)
                .unwrap_or(Value::Null),
            Value::Array(arr) if arr.len() == 1 => arr.into_iter().next().unwrap_or(Value::Null),
            other => other,
        }
    }

    /// 结构视图：JSON 文本（对象/数组）解析为结构值，其它值原样
    pub(crate) fn as_structural(value: &Value) -> Value {
        Self::parse_json_text(value).unwrap_or_else(|| value.clone())
    }

    /// 字符串形态的 JSON 文本 → 结构值；普通文本返回 None（避免把 "123"/"true" 误当结构）
    fn parse_json_text(value: &Value) -> Option<Value> {
        let trimmed = value.as_str()?.trim();
        if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
            return None;
        }
        serde_json::from_str::<Value>(trimmed).ok()
    }

    fn jsonpath_extract(value: &Value, path: &str) -> Result<String, AppError> {
        Ok(Self::value_to_string(&Self::extract_value(value, path)?))
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

/// 根据字符串内容智能推断原始类型（数字/布尔/null/字符串）
///
/// 模板解析的结果总是字符串，但上游值可能是数字或布尔；不还原类型会让下游
/// 拿到 "6" 而非 6。策略：
/// - 空字符串 → Value::Null（避免 JS 中 "" * 3 = 0 的反直觉行为）
/// - "true" / "false" → Value::Bool
/// - 纯数字（整数/浮点/正负号）→ Value::Number
/// - 其他情况保持 Value::String
pub(crate) fn infer_typed_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::Null;
    }

    if trimmed == "true" {
        return Value::Bool(true);
    }
    if trimmed == "false" {
        return Value::Bool(false);
    }

    // 尝试解析为数字（严格匹配，不接受前后空格——前面 trim 过了）
    // 先尝试整数（i64 / u64 避免精度丢失）
    if let Ok(n) = trimmed.parse::<i64>() {
        return Value::Number(serde_json::Number::from(n));
    }
    if let Ok(n) = trimmed.parse::<u64>() {
        return Value::Number(serde_json::Number::from(n));
    }
    // 再尝试 f64（注意 NaN/Inf 无法序列化为 JSON Number，需排除）
    if let Ok(f) = trimmed.parse::<f64>() {
        if f.is_finite() {
            if let Some(num) = serde_json::Number::from_f64(f) {
                return Value::Number(num);
            }
        }
    }

    Value::String(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn ctx() -> HashMap<String, Value> {
        let mut ctx = HashMap::new();
        ctx.insert("amount".to_string(), json!(6));
        ctx.insert("label".to_string(), json!("苹果"));
        ctx.insert(
            "__input__".to_string(),
            json!({ "amount": 6, "label": "苹果" }),
        );
        ctx
    }

    #[test]
    fn resolve_value_infers_number_from_template() {
        let resolved = TemplateEngine::resolve_value(&json!({ "value": "{{amount}}" }), &ctx());
        assert_eq!(resolved, json!({ "value": 6 }));
    }

    #[test]
    fn resolve_value_prefers_mapping_key_over_reserved_alias() {
        // 输入映射里存在名为 input 的键时，{{input}} 必须取映射值，
        // 而不是被保留别名 input（= 整个输入对象）遮蔽。
        let mut context = HashMap::new();
        context.insert("input".to_string(), json!(3));
        context.insert("input2".to_string(), json!(4));
        context.insert("__input__".to_string(), json!({ "input": 3, "input2": 4 }));
        let resolved = TemplateEngine::resolve_value(
            &json!({ "a": "{{input}}", "b": "{{input2}}" }),
            &context,
        );
        assert_eq!(resolved, json!({ "a": 3, "b": 4 }));
    }

    #[test]
    fn resolve_value_keeps_reserved_alias_without_same_named_key() {
        // 没有同名映射键时，别名语义不变：{{input}} / {{__input__}} 仍取整个输入对象。
        let mut context = HashMap::new();
        context.insert("__input__".to_string(), json!({ "amount": 6 }));
        let resolved = TemplateEngine::resolve_value(&json!({ "value": "{{input}}" }), &context);
        assert_eq!(resolved, json!({ "value": "{\"amount\":6}" }));
    }

    #[test]
    fn resolve_value_resolves_from_input_alias() {
        let resolved =
            TemplateEngine::resolve_value(&json!({ "value": "{{__input__.amount}}" }), &ctx());
        assert_eq!(resolved, json!({ "value": 6 }));
    }

    #[test]
    fn resolve_value_keeps_plain_string_as_string() {
        // 参数框里手写的 "6" 不是模板，保持字符串，不参与类型推断
        let resolved = TemplateEngine::resolve_value(&json!({ "value": "6" }), &ctx());
        assert_eq!(resolved, json!({ "value": "6" }));
    }

    #[test]
    fn resolve_value_keeps_text_when_variable_missing() {
        // 未解析的占位符按空处理：整串就是纯占位符 → 空串 → 归一为 null（无值）
        let resolved = TemplateEngine::resolve_value(&json!({ "value": "{{missing}}" }), &ctx());
        assert_eq!(resolved, json!({ "value": null }));
    }

    #[test]
    fn resolve_value_drops_unresolved_placeholder_inside_text() {
        // 文本插值场景：未解析的占位符占位为空串，其余文本保留
        let resolved =
            TemplateEngine::resolve_value(&json!({ "value": "值：{{missing}}；" }), &ctx());
        assert_eq!(resolved, json!({ "value": "值：；" }));
    }

    #[test]
    fn resolve_expands_gate_output_prefix_before_generic_path_split() {
        // `gate_output.<阶段ID>` 是含点号的整键，多带下钻路径时仍要按整键取到合并值再下钻
        let mut context = HashMap::new();
        context.insert(
            "gate_output.stage1".to_string(),
            json!({ "n1": { "result": 0.98 } }),
        );
        assert_eq!(
            TemplateEngine::resolve("{{gate_output.stage1}}", &context).unwrap(),
            "{\"n1\":{\"result\":0.98}}"
        );
        assert_eq!(
            TemplateEngine::resolve("{{gate_output.stage1.n1.result}}", &context).unwrap(),
            "0.98"
        );
        // 阶段合并值不存在 → 解析失败（调用方按无值处理），不回退原文
        assert!(TemplateEngine::resolve("{{gate_output.stage9}}", &context).is_err());
    }

    #[test]
    fn shorthand_expansion_keeps_builtin_prefixes() {
        // {{gate_output.阶段.字段}} 是引擎内建变量，若被按输入映射简写改写会变成不可解析的三段式
        assert_eq!(
            TemplateEngine::expand_shorthand_expr("gate_output.stage1.result"),
            "gate_output.stage1.result"
        );
        assert_eq!(
            TemplateEngine::expand_shorthand_expr("__input__.amount"),
            "__input__.amount"
        );
        // 输入映射简写仍正常展开
        assert_eq!(
            TemplateEngine::expand_shorthand_expr("out.node1.stage1"),
            "node1.out"
        );
        assert_eq!(
            TemplateEngine::expand_shorthand_expr("out.result.node1.stage1"),
            "node1.out.result"
        );
    }

    #[test]
    fn resolve_value_resolves_nested_object_and_array() {
        let resolved = TemplateEngine::resolve_value(
            &json!({
                "list": ["{{amount}}", { "text": "共 {{amount}} 个" }],
                "untouched": 3
            }),
            &ctx(),
        );
        assert_eq!(
            resolved,
            json!({
                "list": [6, { "text": "共 6 个" }],
                "untouched": 3
            })
        );
    }

    #[test]
    fn resolve_value_drills_into_json_text() {
        // 上游把 JSON 作为文本输出（Agent 结构化结果的常见形态）：无需转换节点即可取值
        let mut context = HashMap::new();
        context.insert("n1".to_string(), json!({ "result": "{\"score\": 60}" }));

        // 四段式（带子路径）：{{参数名.子路径.节点ID.阶段ID}} → {{节点ID.参数名.子路径}}
        assert_eq!(
            TemplateEngine::resolve_value(&json!({ "v": "{{result.score.n1.stage1}}" }), &context),
            json!({ "v": 60 })
        );

        // 普通文本不会被当成结构解析：下钻失败 → 无值（null），不回退原文
        let mut plain = HashMap::new();
        plain.insert("n3".to_string(), json!({ "result": "60" }));
        assert_eq!(
            TemplateEngine::resolve_value(&json!({ "v": "{{result.score.n3.stage1}}" }), &plain),
            json!({ "v": null })
        );
    }

    #[test]
    fn resolve_value_supports_array_index_into_json_text() {
        // 段内含 [N] 时不会被三段式规则改写，可直接下钻 JSON 文本
        let mut context = HashMap::new();
        context.insert(
            "n1".to_string(),
            json!({ "list": "[{\"name\":\"alice\"},{\"name\":\"bob\"}]" }),
        );
        assert_eq!(
            TemplateEngine::resolve_value(&json!({ "v": "{{n1.list[1].name}}" }), &context),
            json!({ "v": "bob" })
        );
    }

    #[test]
    fn normalize_value_parses_json_text_and_unwraps_single_value() {
        // 单字段对象 / 单元素数组 → 解包取唯一值（单值结果无需用户指名字段）
        assert_eq!(
            TemplateEngine::normalize_value(&json!("{\"result\": 0.98}")),
            json!(0.98)
        );
        assert_eq!(
            TemplateEngine::normalize_value(&json!("[0.98]")),
            json!(0.98)
        );
        // 插件常见的 `{"result": x}` 外壳（已是结构值，非 JSON 文本）同样解包
        assert_eq!(
            TemplateEngine::normalize_value(&json!({ "result": -0.9899924966004454 })),
            json!(-0.9899924966004454)
        );
        // 多字段对象 → 解析为结构（供 input1.result 这类下钻使用）
        assert_eq!(
            TemplateEngine::normalize_value(&json!("{\"a\": 1, \"b\": 2}")),
            json!({ "a": 1, "b": 2 })
        );
        // 普通文本 / 非法 JSON → 原样
        assert_eq!(
            TemplateEngine::normalize_value(&json!("hello")),
            json!("hello")
        );
        assert_eq!(
            TemplateEngine::normalize_value(&json!("{oops}")),
            json!("{oops}")
        );
    }
}
