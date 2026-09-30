//! KV 记忆工具（会话模式专属）。
//!
//! 迁移自 `lib.rs`（工具架构统一 v1.0，轮 6）：从 `builtin_tool!` 宏改写为具名 struct。

use crate::api_agent::db::{
    now_secs, MemoryEntry, MemoryStore, MemoryUpdate, MergeKind, SaveOutcome, UpdateOutcome,
};
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 保存键值对记忆（新增路径；改已有条目用 `UpdateMemoryTool`）
pub struct SaveMemoryTool {
    store: MemoryStore,
}

impl SaveMemoryTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for SaveMemoryTool {
    fn name(&self) -> &str {
        "save_memory"
    }

    fn description(&self) -> &str {
        "保存一条键值对记忆到持久化知识库（新增路径）。格式：key=名称，value=内容，category=分类（fact/preference/skill/event），important=是否重要（重要记忆不会被自动清理），tags=逗号分隔的检索标签（如 java,构建工具），便于日后按意图检索到该记忆。保存前请先用 search_memory 检查是否已有同名或等价记忆。若 key 已存在：内容等价/包含/近似时会合并进该条（标签并集、重要标记只增不减），**内容与现值无关时不会覆盖，而是报错提示改用 update_memory**；改内容、改名、改分类、删标签、取消重要标记都只能用 update_memory。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "记忆的键（唯一标识，如 project_language）"
                },
                "value": {
                    "type": "string",
                    "description": "记忆的值（要保存的内容）"
                },
                "category": {
                    "type": "string",
                    "description": "记忆分类：fact（事实）、preference（偏好）、skill（技能）、event（事件）",
                    "enum": ["fact", "preference", "skill", "event"]
                },
                "important": {
                    "type": "boolean",
                    "description": "是否为重要记忆（默认 false；true 时记忆置顶保护，自动清理永不删除）"
                },
                "tags": {
                    "type": "string",
                    "description": "可选：检索标签，多个用逗号/顿号分隔（如 db,连接配置）；存储时统一为英文逗号"
                }
            },
            "required": ["key", "value", "category"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Write]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let key = arguments["key"].as_str().ok_or("缺少 key 参数")?;
        let value = arguments["value"].as_str().ok_or("缺少 value 参数")?;
        let category = arguments["category"].as_str().ok_or("缺少 category 参数")?;
        let important = arguments["important"].as_bool().unwrap_or(false);
        let tags = arguments["tags"].as_str().unwrap_or("");

        // 写入前校验（与存储/检索预算一致），防止模型写入超长或非法内容：
        // key/value/tags 长度、category 枚举（后续按此分域检索）。
        let key = key.trim();
        let value = value.trim();
        let category = category.trim();
        if key.is_empty() || key.chars().count() > 200 {
            return Err("记忆 key 不能为空且不超过 200 字符".to_string());
        }
        if value.is_empty() || value.chars().count() > 4000 {
            return Err(format!(
                "记忆 value 不能为空且不超过 4000 字符（当前 {} 字符）；请精简后再保存",
                value.chars().count()
            ));
        }
        if !matches!(category, "fact" | "preference" | "skill" | "event") {
            return Err(format!(
                "记忆分类必须为 fact/preference/skill/event 之一（当前: {}）",
                category
            ));
        }
        if tags.chars().count() > 200 {
            return Err("记忆 tags 不能超过 200 字符".to_string());
        }

        match self.store.save_memory(key, value, category, important, tags) {
            SaveOutcome::Inserted(entry) => {
                let tags_label = if entry.tags.is_empty() { "无".to_string() } else { entry.tags.clone() };
                let pin_label = if entry.pin { "是（自动清理永不删除）" } else { "否" };
                Ok(format!(
                    "记忆已保存（新增）: [{}] {} = {}\n标签: {}；pin 保护: {}。",
                    entry.category, entry.key, entry.value, tags_label, pin_label
                ))
            }
            SaveOutcome::Merged { entry, existing_key, kind: MergeKind::Exact } => {
                let tags_label = if entry.tags.is_empty() { "无".to_string() } else { entry.tags.clone() };
                Ok(format!(
                    "未新增：key={} 已存在且内容等价（[{}]），本次已合并（分类/标签/pin 已同步，未重复写入）。\n标签: {}。",
                    existing_key, entry.category, tags_label
                ))
            }
            SaveOutcome::Merged { entry, existing_key, kind: MergeKind::Superset } => {
                let tags_label = if entry.tags.is_empty() { "无".to_string() } else { entry.tags.clone() };
                Ok(format!(
                    "未新增：本次内容与既有记忆（[{}] key={}）为包含关系，已保留更完整的一份并同步标签（信息未丢失）。\n合并后内容: {}\n标签: {}。",
                    entry.category, existing_key, entry.value, tags_label
                ))
            }
            SaveOutcome::Merged { entry, existing_key, kind: MergeKind::Similar } => {
                let tags_label = if entry.tags.is_empty() { "无".to_string() } else { entry.tags.clone() };
                Ok(format!(
                    "未新增：与既有记忆（[{}] key={}）高度相似，已整理合并（双方关键信息均保留）。\n合并后内容: {}\n标签: {}。",
                    entry.category, existing_key, entry.value, tags_label
                ))
            }
            // 覆盖会丢信息：不写入，并明确指向"明确替换"的入口（这是错误本身的处置指引，随错误一起给模型）
            SaveOutcome::Conflict { entry } => Err(format!(
                "未写入：key={} 已存在且内容与本次无关（既不包含也非近似），为避免覆盖丢信息，本次没有改动任何内容。\n当前内容: {}\n要替换它请用 update_memory(key={}, value=…)；要新增请换一个 key。",
                entry.key, entry.value, entry.key
            )),
        }
    }
}

/// 搜索键值对记忆
pub struct SearchMemoryTool {
    store: MemoryStore,
}

impl SearchMemoryTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for SearchMemoryTool {
    fn name(&self) -> &str {
        "search_memory"
    }

    fn description(&self) -> &str {
        "在记忆库中检索已保存的键值对记忆并返回内容。调用前必须先提供下列至少一个检索条件：query=话题关键词（对记忆的 key 与标签 tags 做子串模糊匹配，value 不参与匹配；多个关键词按空白/逗号/顿号/分号/竖线拆开，任一词命中即返回，命中词更多者排前），或 categories=分类数组（fact/preference/skill/event，仅在这些分类内检索）。未提供这两个参数时不会返回任何记忆内容，只会返回 key 索引并提示补充。命中的 value **原样返回、不截断**（改已有记忆前可用它读到全文）。命中按 重要置顶+热度(含30天半衰期)+命中加权 排序。默认最多返回 10 条（可用 limit 调整，上限 50），条数受总字符预算保护以防撑爆上下文；需要枚举库内条目请用 list_memory。命中条目要修改请用 update_memory、要删除请用 delete_memory（本工具只读）。如果不确定要查询的主题，不要省略参数调用，请直接询问用户想查询的偏好或事实。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "可选：检索关键词，匹配记忆的 key 与 tags（支持多个词，用空格或中英文逗号/顿号分隔）"
                },
                "categories": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["fact", "preference", "skill", "event"] },
                    "description": "可选：限定的分类数组（如 [\"preference\"]），只在这些分类内检索"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "可选：最多返回条数（默认不限制）"
                }
            },
            "required": []
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let query = arguments["query"]
            .as_str()
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let categories: Option<Vec<String>> = arguments["categories"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        v.as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                    })
                    .collect()
            })
            .filter(|c: &Vec<String>| !c.is_empty());
        // 无检索条件 = 模型漏参。不吐 value、不硬报错：仅返回 key+分类索引并引导补参，
        // 让模型能立即用正确参数重试，同时避免把无关个人记忆塞进上下文。
        if query.is_none() && categories.is_none() {
            const INDEX_CAP: usize = 30;
            let all = self.store.list_all(None, None);
            if all.is_empty() {
                return Ok(
                    "记忆库为空，暂无已保存的记忆；需要长期记住的信息可调用 save_memory 保存。"
                        .to_string(),
                );
            }
            let index: Vec<String> = all
                .iter()
                .take(INDEX_CAP)
                .map(|e| format!("- [{}] {}", e.category, e.key))
                .collect();
            let mut hint = format!(
                "search_memory 需提供 query（话题关键词）或 categories（如 [\"preference\"]）才会返回记忆内容；本次未提供，故仅列出 key 索引（不含内容）。记忆库共 {} 条：\n{}",
                self.store.quota_count(),
                index.join("\n")
            );
            if all.len() > INDEX_CAP {
                hint.push_str(&format!("\n…其余 {} 条未列出。", all.len() - INDEX_CAP));
            }
            hint.push_str("\n请补充 query 或 categories 后重新调用；要枚举条目请用 list_memory（含 value 预览）；不确定查询主题时，请先询问用户想查询的偏好或事实。");
            return Ok(hint);
        }

        // 默认最多 10 条，防止未显式传 limit 时一次返回过多撑爆上下文
        const DEFAULT_LIMIT: usize = 10;
        const MAX_LIMIT: usize = 50;
        let limit = arguments["limit"]
            .as_u64()
            .map(|n| (n as usize).min(MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);

        let results = self
            .store
            .search_memory(query, categories.as_deref(), Some(limit));
        if results.is_empty() {
            return Ok("未找到匹配的记忆。".to_string());
        }

        // 返回文本预算：按整条丢弃（不截断单条 value 内容）。value 是"改已有记忆前读到全文"
        // 的唯一来源，截断会让模型在重写时丢尾部，因此这里只限制条数、不裁内容。
        const MAX_OUTPUT_CHARS: usize = 8000;
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0usize;
        for e in &results {
            let tags = if e.tags.is_empty() {
                String::new()
            } else {
                format!(" [tags: {}]", e.tags)
            };
            let line = format!("- [{}] {}: {}{}", e.category, e.key, e.value, tags);
            used += line.chars().count();
            if used > MAX_OUTPUT_CHARS && !lines.is_empty() {
                break;
            }
            lines.push(line);
        }

        let shown = lines.len();
        let mut out = format!("找到 {} 条记忆：\n{}", results.len(), lines.join("\n"));
        if shown < results.len() {
            out.push_str(&format!(
                "\n（结果过长，仅显示前 {} 条，可缩小 limit 或细化关键词）",
                shown
            ));
        }
        Ok(out)
    }
}

/// 盘点记忆库（只读）：列出条目元数据与 value 预览，供模型判断重复/过期条目。
pub struct ListMemoryTool {
    store: MemoryStore,
}

impl ListMemoryTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}

/// 单条 value 预览长度上限：盘点是"判重"用途，短预览足以看出两条在说同一件事；
/// 要看全文用 `search_memory`（它不截断）。
const LIST_VALUE_PREVIEW_CHARS: usize = 120;
const LIST_DEFAULT_LIMIT: usize = 50;
const LIST_MAX_LIMIT: usize = 200;
/// 整块输出预算：超出即整条丢弃，绝不把 value 裁一半。
const LIST_MAX_OUTPUT_CHARS: usize = 8000;

/// 相对时长（盘点判新旧比绝对时间更直接，也免去时区格式化）。
fn format_age(updated_at: u64, now: u64) -> String {
    let secs = now.saturating_sub(updated_at);
    if secs < 60 {
        "刚刚".to_string()
    } else if secs < 3_600 {
        format!("{} 分钟前", secs / 60)
    } else if secs < 86_400 {
        format!("{} 小时前", secs / 3_600)
    } else {
        format!("{} 天前", secs / 86_400)
    }
}

#[async_trait]
impl ToolHandler for ListMemoryTool {
    fn name(&self) -> &str {
        "list_memory"
    }

    fn description(&self) -> &str {
        "盘点记忆库：按分类列出条目（key、分类、标签、重要标记、更新时间、value 预览），用于判断哪些条目重复或过期。value 只给预览，需要全文时用 search_memory（它不截断）。本工具只读，且不刷新访问热度（不会因盘点而改变记忆排序）。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "category": {
                    "type": "string",
                    "description": "可选：只列出该分类",
                    "enum": ["fact", "preference", "skill", "event"]
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": LIST_MAX_LIMIT,
                    "description": "可选：最多返回条数（默认 50，上限 200）"
                }
            },
            "required": []
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let category = arguments["category"]
            .as_str()
            .map(str::trim)
            .filter(|c| !c.is_empty());
        if let Some(c) = category {
            if !matches!(c, "fact" | "preference" | "skill" | "event") {
                return Err(format!(
                    "记忆分类必须为 fact/preference/skill/event 之一（当前: {}）",
                    c
                ));
            }
        }
        let limit = arguments["limit"]
            .as_u64()
            .map(|n| (n as usize).clamp(1, LIST_MAX_LIMIT))
            .unwrap_or(LIST_DEFAULT_LIMIT);

        let all = self.store.list_all(category, None);
        if all.is_empty() {
            return Ok(match category {
                Some(c) => format!("分类 {} 下暂无记忆。", c),
                None => "记忆库为空，暂无已保存的记忆。".to_string(),
            });
        }

        let now = now_secs();
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0usize;
        for e in all.iter().take(limit) {
            let value_chars = e.value.chars().count();
            let preview = if value_chars > LIST_VALUE_PREVIEW_CHARS {
                format!(
                    "{}…（共 {} 字符，此处为预览）",
                    e.value
                        .chars()
                        .take(LIST_VALUE_PREVIEW_CHARS)
                        .collect::<String>(),
                    value_chars
                )
            } else {
                e.value.clone()
            };
            let line = format!(
                "- [{}] {} | 标签: {} | {} | 更新: {} | {}",
                e.category,
                e.key,
                if e.tags.is_empty() {
                    "无"
                } else {
                    e.tags.as_str()
                },
                if e.pin {
                    "重要（免自动清理）"
                } else {
                    "普通"
                },
                format_age(e.updated_at, now),
                preview
            );
            used += line.chars().count();
            if used > LIST_MAX_OUTPUT_CHARS && !lines.is_empty() {
                break;
            }
            lines.push(line);
        }

        let shown = lines.len();
        let mut out = format!(
            "记忆库共 {} 条（本次显示 {} 条）：\n{}",
            self.store.quota_count(),
            shown,
            lines.join("\n")
        );
        if all.len() > shown {
            out.push_str(&format!(
                "\n（还有 {} 条未显示：受 limit 或输出预算限制，可缩小分类范围或调大 limit）",
                all.len() - shown
            ));
        }
        Ok(out)
    }
}

/// 更新一条既有记忆的语义字段（可直接改名；不新增、不合并）。
pub struct UpdateMemoryTool {
    store: MemoryStore,
}

impl UpdateMemoryTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}

/// 空标签渲染为"无"，避免对照行出现看不出变化内容的空串。
fn display_tags(tags: &str) -> &str {
    if tags.is_empty() {
        "无"
    } else {
        tags
    }
}

/// 渲染字段级改动前后对照：只列实际变化的字段。
fn render_update(before: &MemoryEntry, after: &MemoryEntry) -> String {
    let mut lines: Vec<String> = Vec::new();
    if before.key != after.key {
        lines.push(format!("- key: {} → {}", before.key, after.key));
    }
    if before.value != after.value {
        lines.push(format!("- value: {} → {}", before.value, after.value));
    }
    if before.category != after.category {
        lines.push(format!(
            "- category: {} → {}",
            before.category, after.category
        ));
    }
    if before.tags != after.tags {
        lines.push(format!(
            "- tags: {} → {}",
            display_tags(&before.tags),
            display_tags(&after.tags)
        ));
    }
    if before.pin != after.pin {
        lines.push(format!("- important: {} → {}", before.pin, after.pin));
    }
    let head = format!("记忆已更新：[{}] {}", after.category, after.key);
    if lines.is_empty() {
        return format!("{}\n提供的字段与现值相同，未发生实际变化。", head);
    }
    format!("{}\n{}", head, lines.join("\n"))
}

#[async_trait]
impl ToolHandler for UpdateMemoryTool {
    fn name(&self) -> &str {
        "update_memory"
    }

    fn description(&self) -> &str {
        "修改一条已存在的记忆（新增请用 save_memory）。只需传要改的字段，未传的字段保持原值：new_key=改名、value=改内容、category=改分类、tags=整体替换标签（传空串即清空）、important=设置或取消重要标记。key 不存在会报错且不会新建；new_key 已被占用会报错且不会覆盖另一条。本工具不做内容级去重合并——删标签、取消重要标记只能经它完成（save_memory 的标签是并集、重要标记只增不减）。改 value 前先用 search_memory 读到全文，否则会丢内容。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "要修改的记忆 key（须与库中完全一致）"
                },
                "new_key": {
                    "type": "string",
                    "description": "可选：改名后的 key（目标 key 已被占用时会报错）"
                },
                "value": {
                    "type": "string",
                    "description": "可选：新的内容（整体替换）"
                },
                "category": {
                    "type": "string",
                    "description": "可选：新的分类",
                    "enum": ["fact", "preference", "skill", "event"]
                },
                "tags": {
                    "type": "string",
                    "description": "可选：新的检索标签（整体替换，逗号分隔；传空串即清空标签）"
                },
                "important": {
                    "type": "boolean",
                    "description": "可选：设置（true）或取消（false）重要标记"
                }
            },
            "required": ["key"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Write]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let key = arguments["key"].as_str().ok_or("缺少 key 参数")?.trim();
        if key.is_empty() || key.chars().count() > 200 {
            return Err("记忆 key 不能为空且不超过 200 字符".to_string());
        }

        let new_key = arguments["new_key"]
            .as_str()
            .map(str::trim)
            .filter(|k| !k.is_empty());
        if let Some(k) = new_key {
            if k.chars().count() > 200 {
                return Err("新 key 不能超过 200 字符".to_string());
            }
        }
        let value = arguments["value"].as_str().map(str::trim);
        if let Some(v) = value {
            if v.is_empty() || v.chars().count() > 4000 {
                return Err(format!(
                    "记忆 value 不能为空且不超过 4000 字符（当前 {} 字符）",
                    v.chars().count()
                ));
            }
        }
        let category = arguments["category"].as_str().map(str::trim);
        if let Some(c) = category {
            if !matches!(c, "fact" | "preference" | "skill" | "event") {
                return Err(format!(
                    "记忆分类必须为 fact/preference/skill/event 之一（当前: {}）",
                    c
                ));
            }
        }
        let tags = arguments["tags"].as_str().map(str::trim);
        if let Some(t) = tags {
            if t.chars().count() > 200 {
                return Err("记忆 tags 不能超过 200 字符".to_string());
            }
        }
        let important = arguments["important"].as_bool();
        if new_key.is_none()
            && value.is_none()
            && category.is_none()
            && tags.is_none()
            && important.is_none()
        {
            return Err(
                "未提供任何要修改的字段：请至少传 value / category / tags / important / new_key 之一".to_string(),
            );
        }

        let patch = MemoryUpdate {
            new_key: new_key.map(String::from),
            value: value.map(String::from),
            category: category.map(String::from),
            tags: tags.map(String::from),
            pin: important,
        };
        let UpdateOutcome { before, after } = self.store.update_memory(key, &patch)?;
        Ok(render_update(&before, &after))
    }
}

/// 删除一条记忆（高风险：每次调用都需用户审批，超时默认拒绝）。
pub struct DeleteMemoryTool {
    store: MemoryStore,
}

impl DeleteMemoryTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for DeleteMemoryTool {
    fn name(&self) -> &str {
        "delete_memory"
    }

    fn description(&self) -> &str {
        "从记忆库永久删除一条记忆，不可撤销。调用前必须先用 search_memory 或 list_memory 精确确认 key——本工具按 key 精确匹配，key 不符时不会删除任何内容。标记为重要的记忆同样可被删除：重要标记只免自动清理，不阻止删除。返回值给出被删条目的完整内容，误删可用 save_memory 原样恢复。知识库条目不在本工具的删除范围（它们由所属知识库管生命周期，须在「知识库」页从库中移除），命中时会被拒绝。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "要删除的记忆 key（须与库中完全一致）"
                },
                "reason": {
                    "type": "string",
                    "description": "可选：删除原因（如「内容已过期」），会随审批请求展示给用户"
                }
            },
            "required": ["key"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::High
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Write]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let key = arguments["key"].as_str().ok_or("缺少 key 参数")?.trim();
        if key.is_empty() {
            return Err("记忆 key 不能为空".to_string());
        }
        let reason = arguments["reason"]
            .as_str()
            .map(str::trim)
            .filter(|r| !r.is_empty());

        let Some(entry) = self.store.take_session_memory(key)? else {
            return Ok(format!(
                "未找到 key 为 {} 的记忆，未删除任何内容。请先用 search_memory 或 list_memory 确认 key 后再试。",
                key
            ));
        };

        let reason_line = reason
            .map(|r| format!("\n删除原因: {}", r))
            .unwrap_or_default();
        Ok(format!(
            "已删除记忆：{}（重要标记: {}）{}\n原记录:\n- key: {}\n- category: {}\n- tags: {}\n- important: {}\n- value: {}",
            entry.key,
            if entry.pin { "是" } else { "否" },
            reason_line,
            entry.key,
            entry.category,
            display_tags(&entry.tags),
            entry.pin,
            entry.value
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 渲染对照用的条目（只覆盖被断言到的字段）。
    fn entry(key: &str, value: &str, category: &str, tags: &str, pin: bool) -> MemoryEntry {
        MemoryEntry {
            key: key.to_string(),
            value: value.to_string(),
            category: category.to_string(),
            created_at: 0,
            updated_at: 0,
            access_count: 0,
            last_accessed_at: 0,
            pin,
            tags: tags.to_string(),
        }
    }

    #[test]
    fn render_update_lists_only_changed_fields() {
        let before = entry("k", "v1", "fact", "a,b", false);
        let after = entry("k", "v1", "fact", "a", false);
        let out = render_update(&before, &after);
        assert!(out.contains("- tags: a,b → a"), "应给出标签对照: {}", out);
        assert!(!out.contains("- value:"), "未变化的字段不应出现: {}", out);
        assert!(
            !out.contains("- important:"),
            "未变化的字段不应出现: {}",
            out
        );
        assert!(!out.contains("- key:"), "未变化的字段不应出现: {}", out);
    }

    #[test]
    fn render_update_reports_no_change() {
        let e = entry("k", "v", "fact", "a", true);
        let out = render_update(&e, &e.clone());
        assert!(out.contains("未发生实际变化"), "{}", out);
    }

    #[test]
    fn render_update_renders_empty_tags_as_none() {
        let before = entry("k", "v", "fact", "a", false);
        let after = entry("k", "v", "fact", "", false);
        let out = render_update(&before, &after);
        assert!(out.contains("- tags: a → 无"), "清空标签应可读: {}", out);
    }

    #[test]
    fn format_age_buckets() {
        let now = 10_000_000u64;
        assert_eq!(format_age(now, now), "刚刚");
        assert_eq!(format_age(now - 300, now), "5 分钟前");
        assert_eq!(format_age(now - 7_200, now), "2 小时前");
        assert_eq!(format_age(now - 3 * 86_400, now), "3 天前");
    }
}
