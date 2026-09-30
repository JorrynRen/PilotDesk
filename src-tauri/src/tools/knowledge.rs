//! 知识库工具：让模型**主动**检索知识库。
//!
//! 为什么需要它：自动注入只在"本轮消息的关键词恰好命中"时发生，单条还给到 1200 字的预览；
//! 模型想"先把手里的资料翻一遍再回答"时没有任何手段。而 `search_memory` 也顶不上 ——
//! 它的匹配面是 key / tags，知识条目的正文在 value 里，等于搜不到。
//!
//! 这个工具直接走知识库自己的 FTS（trigram，中文按任意子串命中），并按 `kb_entry_links`
//! 带上**来源原文与分块序号**，让模型知道这条知识出自哪份文件的哪一块。

use async_trait::async_trait;
use serde_json::json;

use crate::api_agent::db::{meta_compact, meta_value_text};
use crate::api_agent::knowledge::{
    display_entry_title, display_file_name, EntrySlot, KnowledgeEntryView, KnowledgeStore,
};
use crate::tools::{RiskLevel, ToolHandler, ToolTag};

/// 单条正文的返回上限：知识条目常 400~1200 字，正常不会碰到；
/// 碰到就说明这条特别长，截断并**如实标注**，不假装是全文。
const BODY_LIMIT_CHARS: usize = 2000;
/// 整块输出预算：超出即整条丢弃（不把正文裁一半），保底不撑爆上下文。
const MAX_OUTPUT_CHARS: usize = 12000;
const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 20;
/// 清单模式下每个库列几个条目标题
const INDEX_TITLES_PER_BASE: usize = 12;

/// 检索 / 盘点知识库
pub struct SearchKnowledgeTool {
    /// 与装配处共用一个连接（`Arc` 而不是 clone：`KnowledgeStore` 内部已是 `Arc<Mutex<Connection>>`，
    /// 但整体没有实现 Clone，装配处希望"一个 run 只开一次库"）
    store: std::sync::Arc<KnowledgeStore>,
}

impl SearchKnowledgeTool {
    pub fn new(store: std::sync::Arc<KnowledgeStore>) -> Self {
        Self { store }
    }

    /// 按名字或 id 选库（空 = 全部）；命中不存在的名字时返回 `None` 表示"没有可用目标"。
    fn pick_bases(
        &self,
        base: Option<&str>,
    ) -> Vec<crate::api_agent::knowledge::KnowledgeBaseView> {
        let all = self.store.list_bases();
        match base {
            None => all,
            Some(want) => {
                let want = want.trim().to_lowercase();
                all.into_iter()
                    .filter(|b| {
                        b.id.to_lowercase().contains(&want) || b.name.to_lowercase().contains(&want)
                    })
                    .collect()
            }
        }
    }
}

/// 条目正文：超长则截断并标注
fn body_of(value: &str) -> String {
    let n = value.chars().count();
    if n <= BODY_LIMIT_CHARS {
        return value.trim().to_string();
    }
    let head: String = value.chars().take(BODY_LIMIT_CHARS).collect();
    format!("{}…\n（正文过长已截断，本条共 {} 字）", head, n)
}

/// 条目里带的标签（逗号分隔）拆成数组
fn tags_of(entry: &KnowledgeEntryView) -> Vec<String> {
    entry
        .tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// 读某个专属属性值（`meta_json` 是 JSON 对象串）
fn meta_value(entry: &KnowledgeEntryView, field: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(&entry.meta_json).ok()?;
    let got = v.get(field)?;
    meta_value_text(got).map(|_| got.clone())
}

/// 属性值排序：数字比大小、其余比字符串（日期写成 `2024-01-01` 时字符串比较同样成立）。
/// 跨类型回落成 JSON 文本比较 —— 目的是"总能排出个稳定序"，而不是随机。
fn compare_meta(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => {
            let sa = meta_value_text(a).unwrap_or_default();
            let sb = meta_value_text(b).unwrap_or_default();
            sa.cmp(&sb)
        }
    }
}

/// 条目上**已填**的专属属性 → `状态=现行、生效日期=2024-01-01`。
/// 给模型看：公文的"哪一版"就在这些属性里，模型引用时才有依据说清。
fn meta_line(entry: &KnowledgeEntryView) -> String {
    // 渲染口径与注入块共用 `meta_compact`，不另写一份
    meta_compact(&entry.meta_json)
}

/// 过滤条件回显（"没找到"时告诉模型这次到底限了什么）
fn meta_summary(meta: &serde_json::Map<String, serde_json::Value>) -> String {
    meta.iter()
        .filter_map(|(k, v)| meta_value_text(v).map(|d| format!("{}={}", k, d)))
        .collect::<Vec<_>>()
        .join("、")
}

#[async_trait]
impl ToolHandler for SearchKnowledgeTool {
    fn name(&self) -> &str {
        "search_knowledge"
    }

    fn description(&self) -> &str {
        "检索用户知识库里的资料（把文件/网页整理成的知识条目），返回**正文全文**与来源原文、分块序号、标签、专属属性，用于回答需要查资料的问题。\
         query=检索关键词（匹配正文、标题与标签，多个词按空白/逗号/顿号/分号拆开，任一词命中即返回；中文两字词同样有效）；\
         可选 base=限定某个知识库（按名字或 id 模糊匹配）；可选 tags=限定标签（数组，任一命中即可）；\
         可选 meta=按**专属属性**过滤（逐字段等值，如 {\"状态\":\"现行\"}，属性名以返回里「属性：」后面的写法为准）；\
         可选 order_by=按某个专属属性排序（如「生效日期」）+ order=asc/desc（默认 desc，即新的在前）——\
         公文、制度、法律类知识库常有多版本，**用 meta={\"状态\":\"现行\"} 或 order_by=生效日期 取现行版**，\
         不要凭标题猜哪一版有效。\
         **不带 query 调用时会列出有哪些知识库及各自的条目数/文件数，并给出条目标题示例** —— \
         不确定有没有相关资料时，先这样调一次，再带 query 精确检索。\
         这是用户资料库的检索入口，与 search_memory（只查用户的偏好/事实等会话记忆）用途不同：\
         查项目规范、文档、制度、教程之类的内容请用本工具。命中条目的正文原样返回（超长会标注截断）。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "检索关键词，匹配知识条目的正文、标题与标签（支持多个词，用空格或中英文逗号/顿号/分号分隔）。不传则只列出知识库清单"
                },
                "base": {
                    "type": "string",
                    "description": "可选：限定知识库（按名字或 id 模糊匹配，如「小说」「kb-制度」）"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "可选：限定标签，数组，任意一个命中即保留（如 [\"审批\",\"报销\"]）"
                },
                "meta": {
                    "type": "object",
                    "description": "可选：按专属属性过滤，逐字段等值匹配（如 {\"状态\":\"现行\"}、{\"发布单位\":\"财务部\"}）。属性名用返回里「属性：」的键"
                },
                "order_by": {
                    "type": "string",
                    "description": "可选：按某个专属属性排序（如「生效日期」）。没填该属性的条目排在最后"
                },
                "order": {
                    "type": "string",
                    "enum": ["asc", "desc"],
                    "description": "可选：排序方向，默认 desc（新的/大的在前）"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "可选：最多返回条数（默认 5，上限 20）"
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
        let base_arg = arguments["base"]
            .as_str()
            .map(str::trim)
            .filter(|b| !b.is_empty());
        let want_tags: Vec<String> = arguments["tags"]
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
            .unwrap_or_default();
        let limit = arguments["limit"]
            .as_u64()
            .map(|n| (n as usize).min(MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT)
            .max(1);
        // 专属属性过滤（逐字段等值）与排序：公文场景靠它区分「现行 / 废止」、按生效日期取最新
        let meta: serde_json::Map<String, serde_json::Value> = arguments["meta"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let order_by = arguments["order_by"]
            .as_str()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(String::from);
        let order = arguments["order"].as_str().map(str::to_lowercase);

        let bases = self.pick_bases(base_arg);
        if bases.is_empty() {
            return Ok(match base_arg {
                Some(b) => format!(
                    "没有匹配「{}」的知识库。可去掉 base 参数再试，或用不带 query 的调用先看看有哪些知识库。",
                    b
                ),
                None => "用户还没有建立任何知识库（也没有可检索的资料）。如需长期资料，可提示用户在知识库页投喂文件或网页。".to_string(),
            });
        }

        // 无 query = 清单模式：让模型先知道"有什么可用"，再决定查不查。
        // 这件事靠关键词检索做不到 —— 关键词检索只能回答"有没有命中的"，
        // 回答不了"我手里到底有哪些资料"。
        let Some(query) = query else {
            let mut out = format!("知识库共 {} 个：\n", bases.len());
            for b in &bases {
                let desc = if b.description.trim().is_empty() {
                    String::new()
                } else {
                    format!("（{}）", b.description.trim())
                };
                out.push_str(&format!(
                    "\n- {} {}{}：{} 条知识、{} 个文件\n",
                    b.name, b.id, desc, b.entry_count, b.file_count
                ));
                let titles: Vec<String> = self
                    .store
                    .list_entries(&b.id, None, None, false, None)
                    .iter()
                    .take(INDEX_TITLES_PER_BASE)
                    .map(|e| display_entry_title(&e.key))
                    .collect();
                if titles.is_empty() {
                    out.push_str("  条目为空\n");
                } else {
                    out.push_str(&format!("  条目标题示例：{}\n", titles.join("、")));
                }
            }
            out.push_str(
                "\n要检索内容请带 query 重新调用（可用 base 限定某个库、tags 限定标签）。",
            );
            return Ok(out);
        };

        // 检索模式：逐库走各自的 FTS 检索，按 key 去重（同一条知识可能属于多个库）
        let mut hits: Vec<(String, KnowledgeEntryView)> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        // 专属属性过滤交给 SQL（`list_entries` 的 meta_filter 是 json_extract 逐字段等值），
        // 不在 Rust 里重写一遍 —— 字段名转义、类型映射都在那边做对了
        let meta_filter = if meta.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(meta.clone()).to_string())
        };
        for b in &bases {
            for e in
                self.store
                    .list_entries(&b.id, Some(query), None, false, meta_filter.as_deref())
            {
                if !want_tags.is_empty() {
                    let etags = tags_of(&e);
                    if !want_tags.iter().any(|t| etags.iter().any(|x| x == t)) {
                        continue;
                    }
                }
                if seen.insert(e.key.clone()) {
                    hits.push((b.name.clone(), e));
                }
            }
        }

        // 按专属属性排序（如「生效日期 desc」= 现行版优先）。
        // 只能在 Rust 侧排：`list_entries` 的 ORDER BY 固定是 pin/updated_at，
        // 而它已经先截了 500 条 —— 排序只在已召回的候选内生效，这对"同主题多版本"的场景够用。
        if let Some(field) = order_by.as_deref().filter(|f| !f.trim().is_empty()) {
            let desc = !matches!(order.as_deref(), Some("asc"));
            hits.sort_by(|a, b| {
                let va = meta_value(&a.1, field);
                let vb = meta_value(&b.1, field);
                // 没填这个属性的排在最后：不要因为缺字段把相关条目挤到前面
                match (va, vb) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (Some(x), Some(y)) => {
                        let ord = compare_meta(&x, &y);
                        if desc {
                            ord.reverse()
                        } else {
                            ord
                        }
                    }
                }
            });
        }

        if hits.is_empty() {
            let mut scope: Vec<String> = Vec::new();
            if !want_tags.is_empty() {
                scope.push(format!("限定标签：{}", want_tags.join("、")));
            }
            if !meta.is_empty() {
                scope.push(format!("限定属性：{}", meta_summary(&meta)));
            }
            let scope = if scope.is_empty() {
                String::new()
            } else {
                format!("（{}）", scope.join("；"))
            };
            return Ok(format!(
                "知识库里没有找到与「{}」匹配的内容{}。可换关键词、去掉 base/tags/属性限制，或用不带 query 的调用看看有哪些资料。",
                query,
                scope
            ));
        }

        let total = hits.len();
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0usize;
        for (base_name, e) in hits.iter().take(limit) {
            let title = display_entry_title(&e.key);
            let place = match (e.chunk_index, e.chunk_total) {
                (Some(i), Some(n)) => format!("第 {}/{} 块", i, n),
                _ => "整条".to_string(),
            };
            let src = if e.source_ref.trim().is_empty() {
                String::new()
            } else {
                format!(" · 源自《{}》{}", display_file_name(&e.source_ref), place)
            };
            let tags = tags_of(e);
            let tags_line = if tags.is_empty() {
                String::new()
            } else {
                format!("\n  标签：{}", tags.join("、"))
            };
            // 专属属性放在正文之前：公文的"哪一版、是否现行"就在这里，
            // 模型要先看到它，才不会拿废止的版本当依据
            let attrs = meta_line(e);
            let attrs_line = if attrs.is_empty() {
                String::new()
            } else {
                format!("\n  属性：{}", attrs)
            };
            let line = format!(
                "- [知识库·{}] {}{}{}\n  {}{}\n",
                base_name,
                title,
                src,
                attrs_line,
                body_of(&e.value),
                tags_line
            );
            used += line.chars().count();
            if used > MAX_OUTPUT_CHARS && !lines.is_empty() {
                break;
            }
            lines.push(line);
        }

        let shown = lines.len();
        let mut out = format!(
            "找到 {} 条知识（关键词：{}）：\n\n{}",
            total,
            query,
            lines.join("\n")
        );
        if shown < total.min(limit) {
            out.push_str("\n（结果过长，仅显示前几条；可缩小 limit 或细化关键词）");
        } else if total > shown {
            out.push_str(&format!(
                "\n（共 {} 条命中，这里显示 {} 条；可提高 limit 或细化关键词）",
                total, shown
            ));
        }
        Ok(out)
    }
}

/// 把**提炼过的知识**直接写进知识库。
///
/// 与另外两条写入路径的分工：
///   - `kb_add_candidate`（投喂 / 工作沉淀）走「待确认」队列，因为落进去的是**用户给的原文**
///     （对话文本、粘贴的草稿），必须有人核对；
///   - 本工具是**模型自己写的一条干净知识**（无对话噪声），与 `save_memory` 对称，直接入库。
///
/// 边界：只写"知识条目"这一层（`origin='ai'`、无 `source_ref`），**不碰原文与文件分块**——
/// 后者是别人的原文，改它等于改写事实来源。
pub struct SaveKnowledgeTool {
    store: std::sync::Arc<KnowledgeStore>,
}

const SAVE_MAX_KEY: usize = 80;
const SAVE_MAX_VALUE: usize = 8000;

impl SaveKnowledgeTool {
    pub fn new(store: std::sync::Arc<KnowledgeStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for SaveKnowledgeTool {
    fn name(&self) -> &str {
        "save_knowledge"
    }

    fn description(&self) -> &str {
        "把一条**提炼过的知识**写进用户的某个知识库（直接入库，不走待确认队列）。\
         用于：用户明确要求「记住这条 / 存进知识库」，或你从讨论、资料里提炼出一条可复用的结论。\
         **base 必填**（目标库，按名字或 id；可用库见 system prompt 的 <knowledge_bases>）——\
         存进哪个库是用户的意图，不确定时先问用户，不要猜。\
         **tags 必填**：3~6 个具体检索词，日后靠它命中这条知识。\
         **meta 按目标库的「专属属性」定义填**（库定义见 <knowledge_bases> 里该库的\
         「专属属性」行）：**键用 `key=` 后面的英文标识**（如 status），别写展示名（如「状态」）——\
         展示名只是给你理解字段含义的；取值按类型/候选项（单选只能取候选之一），\
         该库定义了专属属性而你手头已有对应信息时**必须填**，别只写标题与正文；\
         库里没定义专属属性的库，meta 留空。\
         value 必须是**提炼后的正文**：只写可复用的事实与结论，不要带对话过程、\
         不要写「好的我帮你改了 / 我这就去查」这类话；原文引用请保留关键句。\
         同标题会**覆盖更新**既有条目（正则可以先用 search_knowledge 找到该改哪条）。\
         但若该标题是某份**文件的分块**（来自投喂的原文），会被拒绝——那种情况请换个标题新增，\
         或改原文后重新投喂。写用户偏好/事实等会话记忆请用 save_memory，不要用本工具。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "知识标题（简短、可检索，≤80 字）。同标题即覆盖更新"
                },
                "value": {
                    "type": "string",
                    "description": "提炼后的正文（可复用的事实与结论，不含对话过程与客套话）"
                },
                "base": {
                    "type": "string",
                    "description": "目标知识库（名字或 id，必填）。匹配到多个时会被要求写得更精确"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "检索标签（必填，3~6 个具体词，便于日后命中）"
                },
                "meta": {
                    "type": "object",
                    "description": "专属属性。**键用 <knowledge_bases> 里该库「专属属性」行中 `key=` 后的英文标识**（如 status），不要写展示名；取值按类型/候选项（如 {\"status\":\"现行\",\"effect_date\":\"2024-03-01\"}）。该库没定义专属属性就不传"
                }
            },
            "required": ["key", "value", "base", "tags"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Write]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let key = arguments["key"].as_str().map(str::trim).unwrap_or("");
        let value = arguments["value"].as_str().map(str::trim).unwrap_or("");
        let base_arg = arguments["base"].as_str().map(str::trim).unwrap_or("");
        if key.is_empty() || value.is_empty() {
            return Ok("key 与 value 都是必填（标题 + 提炼后的正文）。请补齐后重试。".to_string());
        }
        if base_arg.is_empty() {
            let names = self
                .store
                .list_bases()
                .into_iter()
                .map(|b| b.name)
                .collect::<Vec<_>>();
            return Ok(format!(
                "base（目标知识库）必填。现有知识库：{}。存进哪个库请先和用户确认。",
                if names.is_empty() {
                    "（还没有知识库）".to_string()
                } else {
                    names.join("、")
                }
            ));
        }
        let key: String = key.chars().take(SAVE_MAX_KEY).collect();
        let value: String = value.chars().take(SAVE_MAX_VALUE).collect();
        let tags: Vec<String> = arguments["tags"]
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
            .unwrap_or_default();
        let meta_json = arguments["meta"]
            .as_object()
            .filter(|m| !m.is_empty())
            .map(|m| serde_json::Value::Object(m.clone()).to_string());

        // 选库：0 个 / 多个都不猜 —— 前者让模型知道有哪些库可选，后者让模型收窄
        let matched = {
            let want = base_arg.to_lowercase();
            self.store
                .list_bases()
                .into_iter()
                .filter(|b| {
                    b.id.to_lowercase().contains(&want) || b.name.to_lowercase().contains(&want)
                })
                .collect::<Vec<_>>()
        };
        match matched.len() {
            0 => {
                let names = self
                    .store
                    .list_bases()
                    .into_iter()
                    .map(|b| b.name)
                    .collect::<Vec<_>>();
                return Ok(format!(
                    "没有匹配「{}」的知识库，本次没有写入任何内容。现有知识库：{}。",
                    base_arg,
                    if names.is_empty() {
                        "（还没有知识库）".to_string()
                    } else {
                        names.join("、")
                    }
                ));
            }
            1 => {}
            _ => {
                let names = matched
                    .iter()
                    .map(|b| format!("「{}」", b.name))
                    .collect::<Vec<_>>();
                return Ok(format!(
                    "「{}」匹配到多个知识库（{}），本次没有写入任何内容。请用更完整的名字或 id 重新调用。",
                    base_arg,
                    names.join("、")
                ));
            }
        }
        let base = &matched[0];

        // 覆盖保护：文件分块的正文是原文切片，覆盖会让它脱离原文
        let slot = self.store.entry_slot(&key);
        if let EntrySlot::ChunkOfFile {
            source_ref,
            chunk_index,
        } = &slot
        {
            let place = chunk_index
                .map(|n| format!("第 {} 块", n))
                .unwrap_or_else(|| "分块".into());
            return Ok(format!(
                "「{}」已经是《{}》的{}（来自投喂的原文，正文是原文切片），不能覆盖 —— \
                 本次没有写入任何内容。请改用另一个标题新增这条知识；如果确实要改那份原文，\
                 请在知识库页重新投喂该文件。",
                key,
                display_file_name(source_ref),
                place
            ));
        }
        let existed = matches!(slot, EntrySlot::Existing { .. });

        if let Err(e) = self.store.save_entry(
            &base.id,
            &key,
            &value,
            &tags.join(","),
            "ai",
            "",
            meta_json.as_deref(),
        ) {
            return Ok(format!("写入失败，本次没有存入任何内容：{}", e));
        }

        let attrs = meta_json
            .as_deref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
            .and_then(|v| v.as_object().cloned())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| meta_value_text(v).map(|d| format!("{}={}", k, d)))
                    .collect::<Vec<_>>()
                    .join("、")
            })
            .unwrap_or_default();

        let mut out = format!(
            "已{}到《{}》：「{}」（来源标记为「AI」，可检索、可编辑）。",
            if existed { "更新" } else { "写入" },
            base.name,
            key
        );
        if existed {
            out.push_str("注意：这是**覆盖更新**，该标题的原内容已被替换。");
        }
        if !tags.is_empty() {
            out.push_str(&format!("标签：{}。", tags.join("、")));
        }
        if !attrs.is_empty() {
            out.push_str(&format!("属性：{}。", attrs));
        }
        Ok(out)
    }
}

/// 按文件读知识库里的原文（分块内容）。
///
/// 与 `search_knowledge` 的分工：那个是"按关键词找内容"，这个是"把这份材料整篇拿来看"。
/// 两段式取用的第二段 —— 只有检索没有精读，模型拿到的是零散片段，回答"这份文件讲了什么"
/// 这类问题就只能靠拼凑。
///
/// 读的是**分块内容**而不是磁盘上的原始文件：分块已经是抽取/清洗/整理过的可读文本，
/// 而原始文件可能是 HTML（一堆标签）或二进制（docx/图片，读出来是乱码）。
pub struct ReadKnowledgeFileTool {
    store: std::sync::Arc<KnowledgeStore>,
}

/// 单次最多返回多少块 / 多少字符：整份长文档必须能**分段续读**，不能一次撑爆上下文
const READ_DEFAULT_CHUNKS: usize = 12;
const READ_MAX_CHUNKS: usize = 50;
const READ_MAX_OUTPUT_CHARS: usize = 20000;

impl ReadKnowledgeFileTool {
    pub fn new(store: std::sync::Arc<KnowledgeStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for ReadKnowledgeFileTool {
    fn name(&self) -> &str {
        "read_kb_file"
    }

    fn description(&self) -> &str {
        "读取知识库里某份**原文**的内容（按分块顺序），用于回答\"这份文件/制度/文档讲了什么\"\
         或需要通读某份材料的问题。file=文件名（或其中一段，模糊匹配）；可选 base=限定知识库。\
         **不传 file 时会列出该库里有内容的全部原文（文件名 + 块数）**。\
         长文档会被截断：返回里会说明共几块、已给到第几块，用 from 参数（从第几块开始）继续读。\
         只想按关键词找零散内容时用 search_knowledge，不要用本工具。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "文件名或其中一段（模糊匹配，如「写作模板」「制度」）。不传则列出该知识库里的全部原文"
                },
                "base": {
                    "type": "string",
                    "description": "可选：限定知识库（按名字或 id 模糊匹配）"
                },
                "from": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "可选：从第几块开始读（默认 1）。上一批末尾会告诉下一步该用哪个值"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "可选：本次最多读几块（默认 12，上限 50）"
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
        let want_file = arguments["file"]
            .as_str()
            .map(str::trim)
            .filter(|f| !f.is_empty());
        let base_arg = arguments["base"]
            .as_str()
            .map(str::trim)
            .filter(|b| !b.is_empty());
        let from = arguments["from"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(1)
            .max(1);
        let limit = arguments["limit"]
            .as_u64()
            .map(|n| (n as usize).min(READ_MAX_CHUNKS))
            .unwrap_or(READ_DEFAULT_CHUNKS)
            .max(1);

        let bases = self.store.list_bases();
        let targets: Vec<_> = match base_arg {
            None => bases,
            Some(want) => {
                let want = want.to_lowercase();
                bases
                    .into_iter()
                    .filter(|b| {
                        b.id.to_lowercase().contains(&want) || b.name.to_lowercase().contains(&want)
                    })
                    .collect()
            }
        };
        if targets.is_empty() {
            return Ok(match base_arg {
                Some(b) => format!("没有匹配「{}」的知识库。可去掉 base 参数再试。", b),
                None => "用户还没有建立任何知识库。".to_string(),
            });
        }

        // 定位：在每个库（顺序即库创建顺序）里找名字匹配的原文。
        // 带上库 id 与库名：id 用来精确取块，名字用来给模型看（也用于"来源"标注）。
        let mut found: Vec<(String, String, crate::api_agent::knowledge::SourceHit)> = Vec::new();
        for b in &targets {
            for hit in self.store.find_sources(&b.id, want_file.unwrap_or(""), 60) {
                found.push((b.id.clone(), b.name.clone(), hit));
            }
        }

        // 不传 file = 目录模式：让模型先看有哪些原文，再决定读哪一份
        let Some(want_file) = want_file else {
            if found.is_empty() {
                return Ok("这些知识库里还没有可读的原文（投喂的文件都还没抽出内容）。".to_string());
            }
            let mut out = String::from("知识库里的原文（点开某一份请带 file 参数）：\n");
            for (_, base_name, hit) in found.iter().take(40) {
                out.push_str(&format!(
                    "- [知识库·{}] {}（{} 块）\n",
                    base_name, hit.name, hit.chunk_count
                ));
            }
            if found.len() > 40 {
                out.push_str(&format!("…另有 {} 份未列出。\n", found.len() - 40));
            }
            return Ok(out);
        };

        if found.is_empty() {
            return Ok(format!(
                "知识库里没有名字含「{}」的原文。可先不带 file 调用一次看看有哪些文件，或换一段名字再试。",
                want_file
            ));
        }
        if found.len() > 1 {
            // 多个候选时**不猜**：让模型自己收窄，避免读到错的那份还当成正确答案
            let mut out = format!(
                "「{}」匹配到 {} 份原文，请用更完整的名字（或加 base）再调用：\n",
                want_file,
                found.len()
            );
            for (_, base_name, hit) in found.iter().take(20) {
                out.push_str(&format!(
                    "- [知识库·{}] {}（{} 块）\n",
                    base_name, hit.name, hit.chunk_count
                ));
            }
            return Ok(out);
        }

        let (base_id, base_name, hit) = &found[0];
        let (chunks, total) = self
            .store
            .read_file_chunks(base_id, &hit.source_ref, from, limit);
        if chunks.is_empty() {
            return Ok(format!(
                "《{}》在知识库·{}里没有可读的分块（可能原文没有抽取到正文）。",
                hit.name, base_name
            ));
        }

        // 先把块拼出来再写标题：标题里的"第 x~y 块"必须与实际给出的块一致
        // （按字符预算截断时，写死 from+limit 会对不上）
        let mut blocks: Vec<String> = Vec::new();
        let mut used = 0usize;
        for (i, c) in chunks.iter().enumerate() {
            let block = format!(
                "\n## 第 {}/{} 块 · {}\n{}\n",
                from + i,
                total,
                display_entry_title(&c.key),
                body_of(&c.value)
            );
            if used + block.chars().count() > READ_MAX_OUTPUT_CHARS && !blocks.is_empty() {
                break;
            }
            used += block.chars().count();
            blocks.push(block);
        }
        let last = from + blocks.len() - 1;
        let mut out = format!(
            "《{}》（知识库·{}，共 {} 块）—— 以下是第 {}~{} 块：\n{}",
            hit.name,
            base_name,
            total,
            from,
            last,
            blocks.join("")
        );
        if last < total {
            out.push_str(&format!(
                "\n（本文件共 {} 块，已给到第 {} 块；继续读请用 from={}）",
                total,
                last,
                last + 1
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个临时知识库 + 一条带来源的分块条目（模拟文件投喂的产物）。
    /// 返回 `Arc<KnowledgeStore>` 而不是某个工具：同一个库要喂给"检索"与"按文件读"两个工具。
    fn setup() -> (std::sync::Arc<KnowledgeStore>, String) {
        let dir = std::env::temp_dir()
            .join(format!("kb_tool_{}", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .to_string();
        std::fs::create_dir_all(&dir).unwrap();
        let store = KnowledgeStore::open(&dir).unwrap();
        store
            .create_base("kb-xiaoshuo", "小说创作", "", "[]")
            .unwrap();
        store
            .save_entry(
                "kb-xiaoshuo",
                "黄金三章-1a2b3c4d#2",
                "开篇三章要先立住主角的目标与冲突，再给第一个爽点。",
                "过签,爽点",
                "file",
                "files/1a2b3c4d-写作模板.md",
                None,
            )
            .unwrap();
        (std::sync::Arc::new(store), dir)
    }

    async fn run(tool: &SearchKnowledgeTool, args: serde_json::Value) -> String {
        tool.execute(args).await.unwrap()
    }

    /// 内部指纹不该出现在给模型的文本里：`-<sha8>#<序号>` 是去重/排序用的
    #[test]
    fn internal_fingerprints_are_stripped_from_output() {
        assert_eq!(display_entry_title("黄金三章-1a2b3c4d#2"), "黄金三章");
        assert_eq!(display_entry_title("普通条目"), "普通条目");
        // 只有"8 位十六进制 + #数字"才算指纹，别把正常标题的尾巴削掉
        assert_eq!(display_entry_title("2024-09-27#1"), "2024-09-27#1");
        assert_eq!(
            display_file_name("files/1a2b3c4d-写作模板.md"),
            "写作模板.md"
        );
        assert_eq!(display_file_name("没有指纹.md"), "没有指纹.md");
    }

    /// 不带 query 要能回答"我手里有哪些资料"——关键词检索答不了这个问题
    #[tokio::test]
    async fn empty_call_lists_bases_and_sample_titles() {
        let (store, dir) = setup();
        let tool = SearchKnowledgeTool::new(store.clone());
        let out = run(&tool, json!({})).await;
        assert!(out.contains("小说创作"), "{}", out);
        assert!(out.contains("1 条知识"), "{}", out);
        assert!(
            out.contains("黄金三章"),
            "清单里要给出条目标题示例：{}",
            out
        );
        assert!(out.contains("带 query"), "要告诉模型下一步怎么查：{}", out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn query_hits_body_and_reports_the_source_file() {
        let (store, dir) = setup();
        let tool = SearchKnowledgeTool::new(store.clone());
        // 4 个字 → 走 trigram FTS
        let out = run(&tool, json!({ "query": "开篇三章" })).await;
        assert!(out.contains("找到 1 条知识"), "{}", out);
        assert!(out.contains("开篇三章要先立住"), "正文要原样返回：{}", out);
        assert!(out.contains("写作模板.md"), "要给出源原文：{}", out);
        assert!(out.contains("过签"), "要给出标签：{}", out);
        // 标题里的内部指纹不许出现
        assert!(!out.contains("1a2b3c4d"), "{}", out);

        // 2 个字 → trigram 索引不到，靠 LIKE 退化（中文两字词极常见）
        let out = run(&tool, json!({ "query": "爽点" })).await;
        assert!(out.contains("找到 1 条知识"), "两字词不能漏：{}", out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn base_and_tags_narrow_the_scope_and_empty_results_say_so() {
        let (store, dir) = setup();
        let tool = SearchKnowledgeTool::new(store.clone());
        // 库名模糊匹配得到
        let out = run(&tool, json!({ "query": "开篇三章", "base": "小说" })).await;
        assert!(out.contains("找到 1 条知识"), "{}", out);
        // 库匹配不到：明确说没有这个库，而不是"没找到内容"
        let out = run(&tool, json!({ "base": "根本不存在的库" })).await;
        assert!(out.contains("没有匹配"), "{}", out);
        // 标签限定把那条筛掉
        let out = run(&tool, json!({ "query": "开篇三章", "tags": ["别的标签"] })).await;
        assert!(out.contains("没有找到"), "{}", out);
        assert!(out.contains("限定标签"), "{}", out);
        // 完全不相关
        let out = run(&tool, json!({ "query": "量子隧穿效应" })).await;
        assert!(out.contains("没有找到"), "{}", out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 专属属性要能参与检索：过滤（逐字段等值）、排序（生效日期 desc = 现行版优先）、并回显给模型。
    /// 公文/制度/法律场景的"该用哪一版"就靠这条。
    #[tokio::test]
    async fn meta_filters_sorts_and_is_shown_to_the_model() {
        let (store, dir) = setup();
        store
            .save_entry(
                "kb-xiaoshuo",
                "报销制度",
                "现行版：单笔 5000 元以内由直属领导审批。",
                "",
                "file",
                "files/aaaa1111-报销制度2024.md",
                Some(r#"{"状态":"现行","生效日期":"2024-03-01"}"#),
            )
            .unwrap();
        store
            .save_entry(
                "kb-xiaoshuo",
                "报销制度旧版",
                "旧版：单笔 2000 元以内由直属领导审批。",
                "",
                "file",
                "files/bbbb2222-报销制度2022.md",
                Some(r#"{"状态":"废止","生效日期":"2022-01-01"}"#),
            )
            .unwrap();
        let tool = SearchKnowledgeTool::new(store.clone());

        // 按属性过滤：只要现行的（没填该属性的条目也会被排除）
        let out = run(
            &tool,
            json!({ "query": "报销", "meta": { "状态": "现行" } }),
        )
        .await;
        assert!(out.contains("找到 1 条知识"), "{}", out);
        assert!(
            out.contains("属性：状态=现行、生效日期=2024-03-01"),
            "属性要回显给模型：{}",
            out
        );
        assert!(!out.contains("废止"), "被过滤掉的不该出现：{}", out);

        // 按生效日期降序 → 新版在前
        let out = run(&tool, json!({ "query": "报销", "order_by": "生效日期" })).await;
        assert!(
            out.find("现行版").unwrap() < out.find("旧版").unwrap(),
            "生效日期降序应让新版在前：{}",
            out
        );
        // 升序反过来
        let out = run(
            &tool,
            json!({ "query": "报销", "order_by": "生效日期", "order": "asc" }),
        )
        .await;
        assert!(
            out.find("旧版").unwrap() < out.find("现行版").unwrap(),
            "升序应让旧版在前：{}",
            out
        );

        // 过滤到没有 → 提示要说清这次限了什么，模型才知道该去掉哪个条件
        let out = run(
            &tool,
            json!({ "query": "报销", "meta": { "状态": "不存在" } }),
        )
        .await;
        assert!(out.contains("没有找到"), "{}", out);
        assert!(out.contains("限定属性"), "{}", out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn run_save(tool: &SaveKnowledgeTool, args: serde_json::Value) -> String {
        tool.execute(args).await.unwrap()
    }

    /// `save_knowledge`：写入 → 同标题覆盖更新 → 拒绝覆盖文件分块。
    /// 最后一条是硬约束：分块正文是原文切片，覆盖它会让条目脱离原文。
    #[tokio::test]
    async fn save_knowledge_writes_updates_and_refuses_to_overwrite_file_chunks() {
        let (store, dir) = setup();
        let saver = SaveKnowledgeTool::new(store.clone());

        // 新增：写入后要能**立刻被检索到**（这是"写进去有用"的端到端验证）
        let out = run_save(
            &saver,
            json!({
                "key": "报销流程要点",
                "value": "5000 元以内由直属领导审批，超过则需分管副总签字。",
                "base": "小说",
                "tags": ["报销", "审批"],
                "meta": { "状态": "现行", "生效日期": "2024-03-01" }
            }),
        )
        .await;
        assert!(out.contains("已写入到《小说创作》"), "{}", out);
        assert!(
            out.contains("属性：状态=现行、生效日期=2024-03-01"),
            "{}",
            out
        );
        let found = run(
            &SearchKnowledgeTool::new(store.clone()),
            json!({ "query": "直属领导审批" }),
        )
        .await;
        assert!(
            found.contains("报销流程要点"),
            "写进去的要能被正文检索到：{}",
            found
        );

        // 同标题再写 = 覆盖更新，条目数不增加
        let before = store
            .list_entries("kb-xiaoshuo", None, None, false, None)
            .len();
        let out = run_save(
            &saver,
            json!({ "key": "报销流程要点", "value": "改为 8000 元以内。", "base": "小说" }),
        )
        .await;
        assert!(out.contains("已更新到《小说创作》"), "{}", out);
        assert!(out.contains("覆盖更新"), "覆盖要如实说明：{}", out);
        assert_eq!(
            store
                .list_entries("kb-xiaoshuo", None, None, false, None)
                .len(),
            before,
            "覆盖更新不该新增条目"
        );

        // 碰文件分块：拒绝，且原内容一字未动
        let chunk_key = "黄金三章-1a2b3c4d#2";
        let out = run_save(
            &saver,
            json!({ "key": chunk_key, "value": "我想把它改掉。", "base": "小说" }),
        )
        .await;
        assert!(out.contains("不能覆盖"), "{}", out);
        assert!(out.contains("写作模板.md"), "要说清它来自哪份原文：{}", out);
        let untouched = store
            .list_entries("kb-xiaoshuo", None, None, false, None)
            .into_iter()
            .find(|e| e.key == chunk_key)
            .expect("分块条目还在");
        assert!(
            untouched.value.contains("开篇三章"),
            "原内容不该被改：{}",
            untouched.value
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目标库从不猜：缺失 / 匹配不到 / 匹配多个，都只提示、不写入。
    #[tokio::test]
    async fn save_knowledge_never_guesses_the_target_base() {
        let (store, dir) = setup();
        store
            .create_base("kb-guifan", "创作规范", "", "[]")
            .unwrap();
        let saver = SaveKnowledgeTool::new(store.clone());
        let count = || {
            store
                .list_entries("kb-xiaoshuo", None, None, false, None)
                .len()
        };
        let before = count();

        // 没给 base
        let out = run_save(&saver, json!({ "key": "k", "value": "v" })).await;
        assert!(out.contains("必填"), "{}", out);
        assert!(
            out.contains("小说创作"),
            "列出可选库，模型才知道该问用户什么：{}",
            out
        );

        // 匹配不到
        let out = run_save(
            &saver,
            json!({ "key": "k", "value": "v", "base": "不存在的库" }),
        )
        .await;
        assert!(out.contains("没有匹配"), "{}", out);

        // 匹配多个（「创作」同时命中「小说创作」与「创作规范」）
        let out = run_save(&saver, json!({ "key": "k", "value": "v", "base": "创作" })).await;
        assert!(out.contains("匹配到多个知识库"), "{}", out);
        assert!(
            out.contains("小说创作") && out.contains("创作规范"),
            "{}",
            out
        );

        assert_eq!(count(), before, "以上三种情况都不该写入任何内容");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `read_kb_file` 的四种收场：目录 / 读出内容 / 多候选交给模型收窄 / 找不到
    #[tokio::test]
    async fn read_file_tool_lists_reads_and_asks_to_narrow_down() {
        let (store, dir) = setup();
        store
            .save_entry(
                "kb-xiaoshuo",
                "创作流程",
                "先定题材，再写大纲。",
                "",
                "file",
                "files/9f8e7d6c-创作流程.md",
                None,
            )
            .unwrap();
        let reader = ReadKnowledgeFileTool::new(store.clone());

        // 不传 file = 目录模式：列出有哪些原文（名字 + 块数），让模型先看再点
        let out = reader.execute(json!({})).await.unwrap();
        assert!(out.contains("写作模板.md"), "{}", out);
        assert!(out.contains("创作流程.md"), "{}", out);
        assert!(out.contains("1 块"), "{}", out);

        // 唯一命中 → 读出内容：带库名、块序号、正文；且**不含内部指纹**
        let out = reader.execute(json!({ "file": "写作模板" })).await.unwrap();
        assert!(out.contains("《写作模板.md》"), "{}", out);
        assert!(out.contains("知识库·小说创作"), "{}", out);
        assert!(
            out.contains("第 1/1 块 · 黄金三章"),
            "标题要剥掉 sha 与块序号：{}",
            out
        );
        assert!(out.contains("开篇三章要先立住"), "{}", out);
        assert!(!out.contains("1a2b3c4d"), "内部指纹不该出现：{}", out);

        // 片段同时命中两份 → 不猜，列候选让模型收窄（猜错等于拿错材料当答案）
        let out = reader.execute(json!({ "file": ".md" })).await.unwrap();
        assert!(out.contains("匹配到 2 份原文"), "{}", out);
        assert!(out.contains("请用更完整的名字"), "{}", out);

        // 找不到：明确说没有，并指路（先不带 file 看清单）
        let out = reader
            .execute(json!({ "file": "根本没有这份" }))
            .await
            .unwrap();
        assert!(out.contains("没有名字含"), "{}", out);
        assert!(out.contains("不带 file"), "{}", out);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_library_tells_the_model_there_is_nothing_yet() {
        let dir = std::env::temp_dir()
            .join(format!("kb_tool_empty_{}", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .to_string();
        std::fs::create_dir_all(&dir).unwrap();
        let tool =
            SearchKnowledgeTool::new(std::sync::Arc::new(KnowledgeStore::open(&dir).unwrap()));
        let out = run(&tool, json!({})).await;
        assert!(out.contains("还没有建立任何知识库"), "{}", out);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
