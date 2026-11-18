//! 视频模型映射清单：结构、校验、匹配、路径写入与值转换。
//!
//! 清单是「工具参数槽位 → 厂商请求字段」的唯一事实来源：端点、任务判定、
//! 映射、限制与结果取值全部以数据形式描述，本模块只做校验与解释，
//! 不含任何厂商字段名。P1 只加载随包内置的基线；远程下发在 P2 复用
//! `utils::market` 接通（整份覆盖，不做条目级合并）。

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::image_common::{normalize_http_url, MAX_IMAGE_BYTES};

/// 客户端支持的清单格式版本
pub const SUPPORTED_SCHEMA_VERSION: u64 = 1;

/// 随包内置的基线清单（离线与首次启动可用）
const BUILTIN_MANIFEST: &str = include_str!("../../resources/video-models.json");

/// 图像转换的超时与体积上限（与 image_common 保持一致）
const IMAGE_FETCH_TIMEOUT_SECS: u64 = 10;

/// 第一层媒体槽位（顺序即回显顺序）
pub const MEDIA_SLOTS: [&str; 6] = [
    "firstFrame",
    "lastFrame",
    "referenceImages",
    "referenceVideo",
    "sourceVideo",
    "extendVideo",
];

/// 第一层标量槽位
pub const SCALAR_SLOTS: [&str; 6] = [
    "prompt",
    "duration",
    "aspectRatio",
    "resolution",
    "seed",
    "negativePrompt",
];

/// 是否为第一层契约内的参数名（`mapping` 只允许这些键）
pub fn is_slot_name(name: &str) -> bool {
    MEDIA_SLOTS.contains(&name) || SCALAR_SLOTS.contains(&name)
}

// ── 清单结构 ──

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoManifest {
    pub schema_version: u64,
    pub version: u64,
    /// 排查用时间戳（仅供人工阅读）
    #[allow(dead_code)]
    #[serde(default)]
    pub updated_at: Option<String>,
    pub rules: Vec<ModelRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRule {
    /// 模型匹配模式（精确 / 通配 / 枚举）
    #[serde(rename = "match")]
    pub match_pattern: String,
    pub transport: Transport,
    pub task: TaskSpec,
    #[serde(default)]
    pub response_mode: ResponseMode,
    #[serde(default)]
    pub defaults: bool,
    /// 说明文字，仅供人工阅读
    #[allow(dead_code)]
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub mapping: BTreeMap<String, ParamMapping>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub result_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transport {
    /// 创建路径，相对 base_url
    pub create: String,
    /// 轮询路径，含 `{id}`；省略表示创建响应即最终结果
    #[serde(default)]
    pub poll: Option<String>,
    /// 二进制下载路径，含 `{id}`；binary 模式必填
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub auth: AuthKind,
    #[serde(default)]
    pub body: BodyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum AuthKind {
    #[default]
    #[serde(rename = "bearer")]
    Bearer,
    #[serde(rename = "x-goog-api-key")]
    GoogleApiKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    #[default]
    Json,
    /// P2 启用
    Multipart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResponseMode {
    #[default]
    Url,
    Binary,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskSpec {
    #[serde(default)]
    pub id_paths: Vec<String>,
    #[serde(default)]
    pub status_path: Option<String>,
    #[serde(default)]
    pub success_values: Vec<Value>,
    #[serde(default)]
    pub failure_values: Vec<Value>,
    #[serde(default)]
    pub error_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ParamMapping {
    #[serde(default)]
    pub field: Option<String>,
    #[serde(rename = "const", default)]
    pub constants: BTreeMap<String, Value>,
    #[serde(default)]
    pub transform: Option<Transform>,
    #[serde(default)]
    pub ignore: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    #[serde(default)]
    pub reference_images: Option<usize>,
    #[serde(default)]
    pub durations: Vec<f64>,
    #[serde(default)]
    pub aspect_ratios: Vec<String>,
    #[serde(default)]
    pub resolutions: Vec<String>,
}

/// 值转换：只允许枚举，不接受表达式与条件
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Transform {
    Passthrough,
    String,
    Number,
    #[serde(rename_all = "camelCase")]
    EnumMap { values: BTreeMap<String, Value> },
    #[serde(rename_all = "camelCase")]
    SecondsToFrames { fps: f64 },
    #[serde(rename_all = "camelCase")]
    AspectResolutionToSize { default_resolution: String },
    DataUriToImageObject,
    #[serde(rename_all = "camelCase")]
    DataUriToImageObjectArray {
        #[serde(default)]
        reference_type: Option<String>,
    },
    DataUriToBase64,
}

impl Transform {
    pub fn kind(&self) -> &'static str {
        match self {
            Transform::Passthrough => "passthrough",
            Transform::String => "string",
            Transform::Number => "number",
            Transform::EnumMap { .. } => "enum_map",
            Transform::SecondsToFrames { .. } => "seconds_to_frames",
            Transform::AspectResolutionToSize { .. } => "aspect_resolution_to_size",
            Transform::DataUriToImageObject => "data_uri_to_image_object",
            Transform::DataUriToImageObjectArray { .. } => "data_uri_to_image_object_array",
            Transform::DataUriToBase64 => "data_uri_to_base64",
        }
    }
}

// ── 加载与选择 ──

impl VideoManifest {
    /// 解析并校验清单（远程下发复用同一入口）
    pub fn parse(raw: &str) -> Result<Self, String> {
        let manifest: Self =
            serde_json::from_str(raw).map_err(|e| format!("清单解析失败: {}", e))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// 内置基线
    pub fn builtin() -> Result<Self, String> {
        Self::parse(BUILTIN_MANIFEST)
    }

    /// 按模型名选最具体的规则，返回规则与「是否经过清单验证」。
    ///
    /// 未命中任何具名规则时落到 `defaults: true` 的通用项，`verified = false`。
    pub fn select(&self, model: &str) -> Option<(&ModelRule, bool)> {
        let lowered = model.to_ascii_lowercase();
        let mut best: Option<(&ModelRule, MatchPattern)> = None;
        for rule in &self.rules {
            let Ok(pattern) = MatchPattern::parse(&rule.match_pattern) else {
                continue;
            };
            if !pattern.matches(&lowered) {
                continue;
            }
            let replace = match &best {
                None => true,
                Some((_, current)) => pattern.cmp_specificity(current) == Ordering::Greater,
            };
            if replace {
                best = Some((rule, pattern));
            }
        }
        if let Some((rule, _)) = best {
            return Some((rule, !rule.defaults));
        }
        self.rules.iter().find(|r| r.defaults).map(|r| (r, false))
    }

    /// 整份校验：任一条失败即整份拒绝
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(format!(
                "清单格式版本 {} 高于客户端支持的 {}",
                self.schema_version, SUPPORTED_SCHEMA_VERSION
            ));
        }
        if self.rules.is_empty() {
            return Err("清单没有任何规则".to_string());
        }
        let defaults = self.rules.iter().filter(|r| r.defaults).count();
        if defaults != 1 {
            return Err(format!(
                "清单必须恰好有一条 defaults 规则，当前为 {} 条",
                defaults
            ));
        }

        let mut patterns: Vec<(String, MatchPattern)> = Vec::with_capacity(self.rules.len());
        for rule in &self.rules {
            let pattern = MatchPattern::parse(&rule.match_pattern)
                .map_err(|e| format!("规则 '{}': {}", rule.match_pattern, e))?;

            // ── transport ──
            validate_template_path(&rule.transport.create, true)
                .map_err(|e| format!("规则 '{}'.transport.create: {}", rule.match_pattern, e))?;
            if let Some(p) = &rule.transport.poll {
                validate_template_path(p, true)
                    .map_err(|e| format!("规则 '{}'.transport.poll: {}", rule.match_pattern, e))?;
                if !p.contains("{id}") {
                    return Err(format!("规则 '{}'.transport.poll 缺少 {{id}}", rule.match_pattern));
                }
            }
            if let Some(p) = &rule.transport.content {
                validate_template_path(p, false)
                    .map_err(|e| format!("规则 '{}'.transport.content: {}", rule.match_pattern, e))?;
                if !p.contains("{id}") {
                    return Err(format!(
                        "规则 '{}'.transport.content 缺少 {{id}}",
                        rule.match_pattern
                    ));
                }
            }
            if rule.transport.body != BodyKind::Json {
                return Err(format!(
                    "规则 '{}'.transport.body 暂不支持该取值（P1 仅 json）",
                    rule.match_pattern
                ));
            }

            // ── task ──
            for path in &rule.task.id_paths {
                validate_read_path(path).map_err(|e| {
                    format!("规则 '{}'.task.idPaths: {}", rule.match_pattern, e)
                })?;
            }
            for path in rule.task.status_path.iter().chain(rule.task.error_path.iter()) {
                validate_read_path(path).map_err(|e| {
                    format!("规则 '{}'.task 路径: {}", rule.match_pattern, e)
                })?;
            }

            // ── mapping ──
            for (param, mapping) in &rule.mapping {
                if !is_slot_name(param) {
                    return Err(format!(
                        "规则 '{}'.mapping 含未知参数 '{}'",
                        rule.match_pattern, param
                    ));
                }
                if mapping.ignore && (mapping.field.is_some() || !mapping.constants.is_empty()) {
                    return Err(format!(
                        "规则 '{}'.mapping.{}: ignore 不能与 field/const 同时出现",
                        rule.match_pattern, param
                    ));
                }
                if !mapping.ignore && mapping.field.is_none() && mapping.constants.is_empty() {
                    return Err(format!(
                        "规则 '{}'.mapping.{}: 缺少 field（或显式声明 ignore）",
                        rule.match_pattern, param
                    ));
                }
                if let Some(field) = &mapping.field {
                    parse_write_path(field).map_err(|e| {
                        format!("规则 '{}'.mapping.{}.field: {}", rule.match_pattern, param, e)
                    })?;
                }
                for path in mapping.constants.keys() {
                    parse_write_path(path).map_err(|e| {
                        format!("规则 '{}'.mapping.{}.const: {}", rule.match_pattern, param, e)
                    })?;
                }
                if let Some(transform) = &mapping.transform {
                    validate_transform(transform).map_err(|e| {
                        format!("规则 '{}'.mapping.{}.transform: {}", rule.match_pattern, param, e)
                    })?;
                }
            }

            // ── limits ──
            if let Some(limit) = rule.limits.reference_images {
                if limit == 0 {
                    return Err(format!(
                        "规则 '{}'.limits.referenceImages 必须大于 0",
                        rule.match_pattern
                    ));
                }
            }
            for d in &rule.limits.durations {
                if !d.is_finite() || *d <= 0.0 {
                    return Err(format!(
                        "规则 '{}'.limits.durations 含非法时长",
                        rule.match_pattern
                    ));
                }
            }

            // ── 结果取值 ──
            match rule.response_mode {
                ResponseMode::Url => {
                    if rule.result_paths.is_empty() {
                        return Err(format!(
                            "规则 '{}'.resultPaths 在 url 模式下不能为空",
                            rule.match_pattern
                        ));
                    }
                    for path in &rule.result_paths {
                        validate_read_path(path).map_err(|e| {
                            format!("规则 '{}'.resultPaths: {}", rule.match_pattern, e)
                        })?;
                    }
                }
                ResponseMode::Binary => {
                    if rule.transport.content.is_none() {
                        return Err(format!(
                            "规则 '{}'.transport.content 在 binary 模式下必填",
                            rule.match_pattern
                        ));
                    }
                }
            }

            patterns.push((rule.match_pattern.clone(), pattern));
        }

        for i in 0..patterns.len() {
            for j in (i + 1)..patterns.len() {
                if patterns[i].0 == patterns[j].0 {
                    return Err(format!("清单存在重复的 match: '{}'", patterns[i].0));
                }
                if patterns[i].1.cmp_specificity(&patterns[j].1) == Ordering::Equal {
                    return Err(format!(
                        "清单存在优先级相同的 match: '{}' 与 '{}'",
                        patterns[i].0, patterns[j].0
                    ));
                }
            }
        }
        Ok(())
    }
}

fn validate_transform(transform: &Transform) -> Result<(), String> {
    match transform {
        Transform::EnumMap { values } => {
            if values.is_empty() {
                return Err("enum_map.values 不能为空".to_string());
            }
        }
        Transform::SecondsToFrames { fps } => {
            if !fps.is_finite() || *fps <= 0.0 {
                return Err("seconds_to_frames.fps 必须大于 0".to_string());
            }
        }
        Transform::AspectResolutionToSize {
            default_resolution,
        } => {
            aspect_size("16:9", default_resolution).map_err(|_| {
                format!(
                    "aspect_resolution_to_size.defaultResolution '{}' 不在支持列表",
                    default_resolution
                )
            })?;
        }
        _ => {}
    }
    Ok(())
}

/// 传输路径校验：相对 base_url、无协议头/域名/`..`、占位符白名单
fn validate_template_path(path: &str, allow_model: bool) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err(format!("路径必须以 / 开头: {}", path));
    }
    if path.contains("://") || path.contains("..") || path.contains("\\") {
        return Err(format!("路径不能包含协议头、域名或 .. : {}", path));
    }
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| format!("路径占位符缺少 }}: {}", path))?
            + start;
        let name = &rest[start + 1..end];
        let allowed = name == "id" || (allow_model && name == "model");
        if !allowed {
            return Err(format!("路径含未知占位符 {{{}}}: {}", name, path));
        }
        rest = &rest[end + 1..];
        if rest.contains('}') {
            return Err(format!("路径含多余的 }}: {}", path));
        }
    }
    Ok(())
}

/// 读取路径校验（响应取值、任务标识）
fn validate_read_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("读取路径为空".to_string());
    }
    for seg in path.split('.') {
        if seg.is_empty() {
            return Err(format!("读取路径 '{}' 含空段", path));
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!("读取路径 '{}' 含非法段 '{}'", path, seg));
        }
    }
    Ok(())
}

// ── 匹配语法与优先级 ──

/// 解析后的匹配模式：前缀 / 枚举 / 后缀，通配只允许出现在首尾
#[derive(Debug, Clone)]
struct MatchPattern {
    leading: bool,
    trailing: bool,
    prefix: String,
    suffix: String,
    range: Option<(i64, i64)>,
}

impl MatchPattern {
    fn parse(raw: &str) -> Result<Self, String> {
        let lowered = raw.to_ascii_lowercase();
        if lowered.is_empty() {
            return Err("match 不能为空".to_string());
        }
        let mut s = lowered.as_str();
        let leading = s.starts_with('*');
        if leading {
            s = &s[1..];
        }
        let trailing = s.ends_with('*');
        if trailing {
            s = &s[..s.len() - 1];
        }
        if s.contains('*') {
            return Err("通配符只允许出现在首尾".to_string());
        }

        let (prefix, suffix, range) = match s.find('[') {
            Some(left) => {
                let right = s
                    .find(']')
                    .ok_or_else(|| "枚举缺少 ]".to_string())?;
                if right < left || s[right + 1..].contains('[') {
                    return Err("枚举段格式非法".to_string());
                }
                let inner = &s[left + 1..right];
                let (a, b) = inner
                    .split_once(',')
                    .ok_or_else(|| "枚举格式应为 [a,b]".to_string())?;
                let a: i64 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("枚举下界不是整数: {}", a))?;
                let b: i64 = b
                    .trim()
                    .parse()
                    .map_err(|_| format!("枚举上界不是整数: {}", b))?;
                if a > b {
                    return Err(format!("枚举区间非法: [{},{}]", a, b));
                }
                (
                    s[..left].to_string(),
                    s[right + 1..].to_string(),
                    Some((a, b)),
                )
            }
            None => (s.to_string(), String::new(), None),
        };

        Ok(Self {
            leading,
            trailing,
            prefix,
            suffix,
            range,
        })
    }

    fn is_exact(&self) -> bool {
        !self.leading && !self.trailing && self.range.is_none()
    }

    fn literal_len(&self) -> usize {
        self.prefix.chars().count() + self.suffix.chars().count()
    }

    fn enum_width(&self) -> i64 {
        self.range.map(|(a, b)| b - a).unwrap_or(i64::MAX)
    }

    /// 更具体返回 Greater；四项全等返回 Equal（清单错误）
    fn cmp_specificity(&self, other: &Self) -> Ordering {
        match (self.is_exact(), other.is_exact()) {
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            _ => {}
        }
        match (self.leading, other.leading) {
            (false, true) => return Ordering::Greater,
            (true, false) => return Ordering::Less,
            _ => {}
        }
        self.literal_len()
            .cmp(&other.literal_len())
            .then_with(|| other.enum_width().cmp(&self.enum_width()))
    }

    /// `model` 必须已 ASCII 小写化
    fn matches(&self, model: &str) -> bool {
        if self.is_exact() {
            return model == self.prefix;
        }
        let starts: Vec<usize> = if self.leading {
            if self.prefix.is_empty() {
                vec![0]
            } else {
                model
                    .match_indices(self.prefix.as_str())
                    .map(|(i, _)| i)
                    .collect()
            }
        } else if model.starts_with(self.prefix.as_str()) {
            vec![0]
        } else {
            Vec::new()
        };

        for start in starts {
            let after_prefix = start + self.prefix.len();
            let Some(rest) = model.get(after_prefix..) else {
                continue;
            };
            let tail = match self.range {
                Some((low, high)) => {
                    let digits: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if digits.is_empty() {
                        continue;
                    }
                    let Ok(number) = digits.parse::<i64>() else {
                        continue;
                    };
                    if number < low || number > high {
                        continue;
                    }
                    let after_digits = &rest[digits.len()..];
                    let Some(tail) = after_digits.strip_prefix(self.suffix.as_str()) else {
                        continue;
                    };
                    tail
                }
                None => match rest.strip_prefix(self.suffix.as_str()) {
                    Some(tail) => tail,
                    None => continue,
                },
            };
            if self.trailing || tail.is_empty() {
                return true;
            }
        }
        false
    }
}

// ── 路径写入 ──

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    /// 对象键
    Key(String),
    /// 数组下标（不存在则创建）
    Index(usize),
    /// 向 `name` 数组追加一个元素
    Append(String),
}

fn parse_write_path(path: &str) -> Result<Vec<Seg>, String> {
    if path.is_empty() {
        return Err("字段路径为空".to_string());
    }
    let mut segs = Vec::new();
    for raw in path.split('.') {
        if raw.is_empty() {
            return Err(format!("字段路径 '{}' 含空段（多余的点号）", path));
        }
        if let Some(name) = raw.strip_suffix("[]") {
            validate_key_name(name, path)?;
            segs.push(Seg::Append(name.to_string()));
        } else if raw.chars().all(|c| c.is_ascii_digit()) {
            let index: usize = raw
                .parse()
                .map_err(|_| format!("字段路径 '{}' 下标非法: {}", path, raw))?;
            segs.push(Seg::Index(index));
        } else {
            validate_key_name(raw, path)?;
            segs.push(Seg::Key(raw.to_string()));
        }
    }
    Ok(segs)
}

fn validate_key_name(name: &str, path: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("字段路径 '{}' 含空键名", path));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("字段路径 '{}' 含非法键名 '{}'", path, name));
    }
    Ok(())
}

/// 把 `field` + `const` 写入请求体。
///
/// 同一映射项的多条路径共享一段公共前缀：`content[].text` 与 `content[].type`
/// 只追加一个 `content[]` 元素，不会拆成两个对象。
pub fn write_mapping(
    body: &mut Value,
    mapping: &ParamMapping,
    value: Value,
) -> Result<(), String> {
    let field = mapping
        .field
        .as_deref()
        .map(parse_write_path)
        .transpose()?;
    let constants = mapping
        .constants
        .iter()
        .map(|(path, value)| {
            let segs = parse_write_path(path)?;
            Ok((path.clone(), segs, value.clone()))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut all: Vec<&[Seg]> = Vec::new();
    if let Some(field) = &field {
        all.push(field);
    }
    for (_, segs, _) in &constants {
        all.push(segs);
    }
    if all.is_empty() {
        return Ok(());
    }

    // 公共前缀最多到最短路径的父级，保证每条路径都还有可写段
    let cap = all
        .iter()
        .map(|segs| segs.len().saturating_sub(1))
        .min()
        .unwrap_or(0);
    let prefix_len = common_prefix_len(&all).min(cap);
    let container = if prefix_len == 0 {
        &mut *body
    } else {
        resolve_container(body, &all[0][..prefix_len])?
    };

    if let Some(field) = &field {
        write_segments(container, &field[prefix_len..], value)?;
    }
    for (_, segs, value) in constants {
        write_segments(container, &segs[prefix_len..], value)?;
    }
    Ok(())
}

fn common_prefix_len(paths: &[&[Seg]]) -> usize {
    let Some(first) = paths.first() else {
        return 0;
    };
    let mut len = 0;
    'outer: while len < first.len() {
        for path in &paths[1..] {
            if path.get(len) != first.get(len) {
                break 'outer;
            }
        }
        len += 1;
    }
    len
}

fn new_container(next: Option<&Seg>) -> Value {
    match next {
        Some(Seg::Index(_)) | Some(Seg::Append(_)) => Value::Array(Vec::new()),
        _ => Value::Object(Map::new()),
    }
}

/// 逐段下钻，返回公共前缀对应的容器（不存在则创建）
fn resolve_container<'a>(body: &'a mut Value, segs: &[Seg]) -> Result<&'a mut Value, String> {
    let mut current = body;
    for (index, seg) in segs.iter().enumerate() {
        let next = segs.get(index + 1);
        current = descend(current, seg, next)?;
    }
    Ok(current)
}

fn descend<'a>(
    current: &'a mut Value,
    seg: &Seg,
    next: Option<&Seg>,
) -> Result<&'a mut Value, String> {
    match seg {
        Seg::Key(key) => {
            let object = current
                .as_object_mut()
                .ok_or_else(|| format!("字段 '{}' 的父级不是对象", key))?;
            let entry = object.entry(key.clone()).or_insert(Value::Null);
            if entry.is_null() {
                *entry = new_container(next);
            }
            Ok(entry)
        }
        Seg::Index(index) => {
            let array = current
                .as_array_mut()
                .ok_or_else(|| format!("下标 {} 的父级不是数组", index))?;
            while array.len() <= *index {
                array.push(Value::Null);
            }
            let entry = &mut array[*index];
            if entry.is_null() {
                *entry = new_container(next);
            }
            Ok(entry)
        }
        Seg::Append(key) => {
            let object = current
                .as_object_mut()
                .ok_or_else(|| format!("数组 '{}' 的父级不是对象", key))?;
            let entry = object
                .entry(key.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            let array = entry
                .as_array_mut()
                .ok_or_else(|| format!("字段 '{}' 不是数组", key))?;
            array.push(new_container(next));
            Ok(array.last_mut().expect("刚插入的元素必然存在"))
        }
    }
}

fn write_segments(current: &mut Value, segs: &[Seg], value: Value) -> Result<(), String> {
    let Some((last, parents)) = segs.split_last() else {
        return Err("写入路径为空".to_string());
    };
    let container = resolve_container(current, parents)?;
    match last {
        Seg::Key(key) => {
            let object = container
                .as_object_mut()
                .ok_or_else(|| format!("字段 '{}' 的父级不是对象", key))?;
            object.insert(key.clone(), value);
        }
        Seg::Index(index) => {
            let array = container
                .as_array_mut()
                .ok_or_else(|| format!("下标 {} 的父级不是数组", index))?;
            while array.len() <= *index {
                array.push(Value::Null);
            }
            array[*index] = value;
        }
        Seg::Append(key) => {
            let object = container
                .as_object_mut()
                .ok_or_else(|| format!("数组 '{}' 的父级不是对象", key))?;
            let entry = object
                .entry(key.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            let array = entry
                .as_array_mut()
                .ok_or_else(|| format!("字段 '{}' 不是数组", key))?;
            array.push(value);
        }
    }
    Ok(())
}

// ── 值转换 ──

/// 转换上下文：`aspect_resolution_to_size` 需要读取同批参数里的 resolution
pub struct TransformCtx<'a> {
    pub args: &'a Map<String, Value>,
}

pub async fn apply_transform(
    transform: Option<&Transform>,
    raw: &Value,
    ctx: &TransformCtx<'_>,
) -> Result<Value, String> {
    let Some(transform) = transform else {
        return Ok(raw.clone());
    };
    match transform {
        Transform::Passthrough => Ok(raw.clone()),
        Transform::String => Ok(Value::String(value_to_string(raw)?)),
        Transform::Number => Ok(Value::Number(number_of(raw)?)),
        Transform::EnumMap { values } => {
            let key = canonical_key(raw);
            values.get(&key).cloned().ok_or_else(|| {
                format!(
                    "取值 '{}' 不在允许的枚举映射中（允许：{}）",
                    key,
                    values.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })
        }
        Transform::SecondsToFrames { fps } => {
            let seconds = as_f64(raw)?;
            Ok(json!((seconds * fps).round() as i64))
        }
        Transform::AspectResolutionToSize {
            default_resolution,
        } => {
            let aspect = raw
                .as_str()
                .ok_or_else(|| "画幅参数必须是字符串".to_string())?;
            let resolution = ctx
                .args
                .get("resolution")
                .and_then(|v| v.as_str())
                .unwrap_or(default_resolution);
            Ok(Value::String(aspect_size(aspect, resolution)?))
        }
        Transform::DataUriToImageObject => {
            let source = raw
                .as_str()
                .ok_or_else(|| "图像参数必须是字符串".to_string())?;
            let (mime, bytes) = load_image_bytes(source).await?;
            Ok(json!({
                "mimeType": mime,
                "bytesBase64Encoded": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        }
        Transform::DataUriToImageObjectArray { reference_type } => {
            let sources = raw
                .as_array()
                .ok_or_else(|| "参考图参数必须是字符串数组".to_string())?;
            let mut out = Vec::with_capacity(sources.len());
            for source in sources {
                let source = source
                    .as_str()
                    .ok_or_else(|| "参考图数组元素必须是字符串".to_string())?;
                let (mime, bytes) = load_image_bytes(source).await?;
                let mut item = Map::new();
                item.insert(
                    "image".to_string(),
                    json!({
                        "mimeType": mime,
                        "bytesBase64Encoded": base64::engine::general_purpose::STANDARD.encode(bytes),
                    }),
                );
                if let Some(reference_type) = reference_type {
                    item.insert("referenceType".to_string(), json!(reference_type));
                }
                out.push(Value::Object(item));
            }
            Ok(Value::Array(out))
        }
        Transform::DataUriToBase64 => {
            let source = raw
                .as_str()
                .ok_or_else(|| "图像参数必须是字符串".to_string())?;
            Ok(Value::String(image_base64(source).await?))
        }
    }
}

fn value_to_string(value: &Value) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err("无法转为字符串".to_string()),
    }
}

fn number_of(value: &Value) -> Result<serde_json::Number, String> {
    match value {
        Value::Number(n) => Ok(n.clone()),
        Value::String(s) => {
            let parsed: f64 = s
                .trim()
                .parse()
                .map_err(|_| format!("无法把 '{}' 转为数值", s))?;
            serde_json::Number::from_f64(parsed)
                .ok_or_else(|| format!("数值 '{}' 非法", parsed))
        }
        _ => Err("期望数值".to_string()),
    }
}

fn as_f64(value: &Value) -> Result<f64, String> {
    match value {
        Value::Number(n) => n.as_f64().ok_or_else(|| "数值非法".to_string()),
        Value::String(s) => s
            .trim()
            .parse()
            .map_err(|_| format!("无法把 '{}' 转为数值", s)),
        _ => Err("期望数值".to_string()),
    }
}

/// 枚举映射的键：整数值统一去掉 `.0`
fn canonical_key(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f.abs() < 9.0e15 {
                    return (f as i64).to_string();
                }
            }
            n.to_string()
        }
        other => other.to_string(),
    }
}

/// 画幅 + 分辨率 → 像素尺寸（`720p` 按短边算）
fn aspect_size(aspect: &str, resolution: &str) -> Result<String, String> {
    let ratio = match aspect {
        "16:9" => 16.0 / 9.0,
        "9:16" => 9.0 / 16.0,
        "1:1" => 1.0,
        "21:9" => 21.0 / 9.0,
        "4:3" => 4.0 / 3.0,
        "3:4" => 3.0 / 4.0,
        other => return Err(format!("不支持的画幅 '{}'", other)),
    };
    let short_side = match resolution {
        "480p" => 480.0,
        "540p" => 540.0,
        "720p" => 720.0,
        "1080p" => 1080.0,
        "4k" => 2160.0,
        other => return Err(format!("不支持的分辨率 '{}'", other)),
    };
    let (width, height) = if ratio >= 1.0 {
        (short_side * ratio, short_side)
    } else {
        (short_side, short_side / ratio)
    };
    let even = |value: f64| ((value / 2.0).round() as i64) * 2;
    Ok(format!("{}x{}", even(width), even(height)))
}

/// 读取图片字节：Data URI 拆包、公网 URL 按需下载、本地路径读盘
async fn load_image_bytes(source: &str) -> Result<(String, Vec<u8>), String> {
    if let Some(rest) = source.strip_prefix("data:") {
        let (meta, payload) = rest
            .split_once(',')
            .ok_or_else(|| "无效的 data URI".to_string())?;
        if !meta.ends_with(";base64") {
            return Err("仅支持 base64 编码的 data URI".to_string());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|e| format!("base64 解码失败: {}", e))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!("图片为 {} 字节，超过 4MB 限制", bytes.len()));
        }
        let declared = meta.split(';').next().unwrap_or("");
        let mime = if declared.starts_with("image/") {
            declared.to_string()
        } else {
            guess_mime(&bytes).unwrap_or_else(|| "image/png".to_string())
        };
        return Ok((mime, bytes));
    }

    if source.contains("http://") || source.contains("https://") {
        let url = normalize_http_url(source).ok_or_else(|| "无法识别图片 URL".to_string())?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(IMAGE_FETCH_TIMEOUT_SECS))
            .timeout(Duration::from_secs(IMAGE_FETCH_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("下载图片失败: {}", e))?;
        if !response.status().is_success() {
            return Err(format!("下载图片失败 (HTTP {})", response.status().as_u16()));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or("").trim().to_string());
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("读取图片失败: {}", e))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!("图片为 {} 字节，超过 4MB 限制", bytes.len()));
        }
        let mime = content_type
            .filter(|value| value.starts_with("image/"))
            .or_else(|| guess_mime(&bytes))
            .unwrap_or_else(|| "image/png".to_string());
        return Ok((mime, bytes.to_vec()));
    }

    let bytes = std::fs::read(source).map_err(|e| format!("读取图片文件失败: {}", e))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!("图片为 {} 字节，超过 4MB 限制", bytes.len()));
    }
    let mime = guess_mime(&bytes).unwrap_or_else(|| "image/png".to_string());
    Ok((mime, bytes))
}

fn guess_mime(bytes: &[u8]) -> Option<String> {
    image::guess_format(bytes)
        .ok()
        .map(|format| format.to_mime_type().to_string())
}

/// 去掉 Data URI 头；URL 原样返回（不触发下载）；本地路径读盘后编码
async fn image_base64(source: &str) -> Result<String, String> {
    if let Some(rest) = source.strip_prefix("data:") {
        let (meta, payload) = rest
            .split_once(',')
            .ok_or_else(|| "无效的 data URI".to_string())?;
        if !meta.ends_with(";base64") {
            return Err("仅支持 base64 编码的 data URI".to_string());
        }
        return Ok(payload.to_string());
    }
    if source.contains("http://") || source.contains("https://") {
        return normalize_http_url(source).ok_or_else(|| "无法识别图片 URL".to_string());
    }
    let (_, bytes) = load_image_bytes(source).await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_of(manifest: &VideoManifest, model: &str) -> String {
        manifest
            .select(model)
            .map(|(rule, verified)| format!("{}|{}", rule.match_pattern, verified))
            .unwrap_or_else(|| "none".to_string())
    }

    #[test]
    fn builtin_manifest_is_valid() {
        let manifest = VideoManifest::builtin().expect("内置清单必须通过校验");
        assert_eq!(manifest.rules.len(), 6);
    }

    #[test]
    fn selects_most_specific_rule() {
        let manifest = VideoManifest::builtin().unwrap();
        assert_eq!(rule_of(&manifest, "veo-3.1"), "*veo*|true");
        assert_eq!(rule_of(&manifest, "doubao-seedance-2-0"), "*seedance-2*|true");
        assert_eq!(rule_of(&manifest, "doubao-seedance-1-0"), "*seedance*|true");
        assert_eq!(rule_of(&manifest, "Kling-v2.5"), "*kling*|true");
        assert_eq!(rule_of(&manifest, "unknown-model"), "*|false");
    }

    #[test]
    fn wildcard_and_enum_matching() {
        let p = MatchPattern::parse("*seedance-[2,10]*").unwrap();
        assert!(p.matches("doubao-seedance-2-0"));
        assert!(p.matches("seedance-10"));
        assert!(!p.matches("seedance-11"));
        assert!(!p.matches("seedance-1"));

        let exact = MatchPattern::parse("seedance-2.5").unwrap();
        assert!(exact.matches("seedance-2.5"));
        assert!(!exact.matches("seedance-2.5-pro"));

        let suffix = MatchPattern::parse("seedance-2*").unwrap();
        assert!(suffix.matches("seedance-2.5"));
        assert!(!suffix.matches("x-seedance-2.5"));
    }

    #[test]
    fn priority_prefers_exact_then_narrower() {
        let exact = MatchPattern::parse("seedance-2.5").unwrap();
        let wildcard = MatchPattern::parse("*seedance-2*").unwrap();
        assert_eq!(exact.cmp_specificity(&wildcard), Ordering::Greater);

        let enum_rule = MatchPattern::parse("seedance-[2,4]*").unwrap();
        assert_eq!(enum_rule.cmp_specificity(&wildcard), Ordering::Greater);
    }

    #[test]
    fn writes_seedance_content_array_as_one_element() {
        let mapping = ParamMapping {
            field: Some("content[].image_url.url".to_string()),
            constants: [
                ("content[].type".to_string(), json!("image_url")),
                ("content[].role".to_string(), json!("first_frame")),
            ]
            .into_iter()
            .collect(),
            transform: None,
            ignore: false,
        };
        let mut body = json!({"model": "seedance"});
        write_mapping(&mut body, &mapping, json!("https://example.com/a.png")).unwrap();
        assert_eq!(
            body["content"],
            json!([{
                "type": "image_url",
                "role": "first_frame",
                "image_url": { "url": "https://example.com/a.png" }
            }])
        );
    }

    #[test]
    fn writes_veo_instances_into_same_object() {
        let mut body = json!({});
        let prompt = ParamMapping {
            field: Some("instances.0.prompt".to_string()),
            ..Default::default()
        };
        let image = ParamMapping {
            field: Some("instances.0.image".to_string()),
            ..Default::default()
        };
        write_mapping(&mut body, &prompt, json!("a cat")).unwrap();
        write_mapping(&mut body, &image, json!({"mimeType": "image/png"})).unwrap();
        assert_eq!(
            body["instances"],
            json!([{"prompt": "a cat", "image": {"mimeType": "image/png"}}])
        );
    }

    #[test]
    fn aspect_resolution_size() {
        assert_eq!(aspect_size("16:9", "720p").unwrap(), "1280x720");
        assert_eq!(aspect_size("9:16", "1080p").unwrap(), "1080x1920");
        assert_eq!(aspect_size("1:1", "720p").unwrap(), "720x720");
        assert_eq!(aspect_size("21:9", "720p").unwrap(), "1680x720");
    }

    #[test]
    fn enum_key_normalizes_integral_numbers() {
        assert_eq!(canonical_key(&json!(5)), "5");
        assert_eq!(canonical_key(&json!(5.0)), "5");
    }
}
