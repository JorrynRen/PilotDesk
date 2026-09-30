//! 知识库的 LLM 操作：模型解析 + 按主题生成知识草稿
//!
//! 模型口径（已定）：设置里 `kb_model_config`（`{providerId, model}`）优先；
//! 未配置时回落**第一个可用提供商**（有 endpoint、有 Key、配了模型）。
//! 与语音识别的覆盖项、记忆意图模型同一套思路：特定用途的模型覆盖 + 回落，不强制用户先配。
//!
//! 生成结果一律落成**待确认候选**（origin=ai），用户核对/修改后才入库 —— 模型产出不等于事实。

use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};

use crate::api_agent::client::ApiClient;
use crate::api_agent::types::{ApiFormat, ChatMessage, ChatRequest, UsageRecord};
use crate::commands::api_provider::get_api_key;
use crate::utils::paths;

/// 设置键：`{ "providerId": "...", "model": "..." }`
pub const KB_MODEL_SETTING: &str = "kb_model_config";

/// 生成/整理用的单次输出上限（token）
const MAX_TOKENS: u32 = 2000;

/// 一次知识库操作（一次命令）的模型用量累加器。
///
/// **为什么挂在 `KbModel` 上**：`KbModel` 本来就在每次命令里由 `resolve_kb_model` 现造一份，
/// 生命周期恰好等于"这一次整理 / 生成"；这样不必给 kb_llm 的每个公开函数再加一个回调参数
/// （`summarize` 那种 `on_usage` 要一路透传五层）。
///
/// **为什么逐条留痕而不是只累加一个总数**：`api_usage_log` 的 `call_count` 就是行数 ——
/// 压成一行会把"整理一个 500 块的文件"记成 1 次调用；分批整理、json 模式重试都各算一次。
#[derive(Debug, Clone, Default)]
pub struct KbUsage(Arc<Mutex<Vec<UsageRecord>>>);

impl KbUsage {
    fn push(&self, u: UsageRecord) {
        if let Ok(mut v) = self.0.lock() {
            v.push(u);
        }
    }

    /// 取走本次命令累积的全部用量（取后清空）。落库由命令层做（它才有 conn 与归因键）。
    pub fn take(&self) -> Vec<UsageRecord> {
        self.0
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct KbModel {
    /// 提供商 id：落用量记录要的是 id（与 `agent_loop` 的 provider 维度同一口径），
    /// `provider_name` 只用于给人看。
    pub provider_id: String,
    pub provider_name: String,
    pub endpoint: String,
    pub api_key: String,
    pub api_format: String,
    pub model: String,
    /// true = 设置里没指定（或指定不可用），走的是"第一个可用提供商"回落
    pub from_fallback: bool,
    /// 本次操作的用量累加器（见 `KbUsage`）
    pub usage: KbUsage,
}

/// 模型产出的知识草稿
#[derive(Debug, Clone, Default)]
pub struct KbDraft {
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    /// 专属属性值（按目标库的字段定义填）
    pub meta: Map<String, Value>,
}

/// 解析知识库模型：显式配置 → 第一个可用提供商
pub fn resolve_kb_model(conn: &Connection) -> Result<KbModel, String> {
    if let Some(raw) = paths::get_app_setting_opt(conn, KB_MODEL_SETTING) {
        // 配置存在但解析不出（手工改坏等）按未配置处理：知识库不该因此彻底不可用
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            let pid = v["providerId"].as_str().unwrap_or("").trim().to_string();
            let model = v["model"].as_str().unwrap_or("").trim().to_string();
            if !pid.is_empty() && !model.is_empty() {
                let p = provider_row(conn, &pid)?
                    .ok_or_else(|| format!("知识库模型指向的提供商已不存在：{}", pid))?;
                let key = get_api_key(conn, &pid)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("提供商「{}」没有配置 API Key", p.name))?;
                return Ok(KbModel {
                    provider_id: pid,
                    provider_name: p.name,
                    endpoint: p.endpoint,
                    api_key: key,
                    api_format: p.api_format,
                    model,
                    from_fallback: false,
                    usage: KbUsage::default(),
                });
            }
        }
    }

    // 回落：第一个"能用"的提供商（按 sort_order，跳过 anthropic 原生格式 —— 走不了 /chat/completions）
    let mut stmt = conn
        .prepare(
            "SELECT id, name, api_endpoint, api_format, models FROM api_providers ORDER BY sort_order",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?
                    .unwrap_or_else(|| "openai".into()),
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut saw_any = false;
    for row in rows.filter_map(|r| r.ok()) {
        let (id, name, endpoint, api_format, models_json) = row;
        if endpoint.trim().is_empty() || models_from_json(&models_json).is_empty() {
            continue;
        }
        saw_any = true;
        if api_format.to_lowercase().contains("anthropic") {
            continue;
        }
        let Some(key) = get_api_key(conn, &id).map_err(|e| e.to_string())? else {
            continue;
        };
        return Ok(KbModel {
            provider_id: id,
            provider_name: name,
            endpoint,
            api_key: key,
            api_format,
            model: models_from_json(&models_json)[0].clone(),
            from_fallback: true,
            usage: KbUsage::default(),
        });
    }

    if saw_any {
        Err("知识库的模型调用目前只支持 OpenAI 兼容格式；请在「设置 › 知识库」里指定一个 OpenAI 兼容的模型".into())
    } else {
        Err("还没有可用的 API 提供商：请先在「设置 › API 提供商」里配置，或在「设置 › 知识库」里指定模型".into())
    }
}

struct ProviderRow {
    name: String,
    endpoint: String,
    api_format: String,
}

fn provider_row(conn: &Connection, id: &str) -> Result<Option<ProviderRow>, String> {
    use rusqlite::{params, OptionalExtension};
    conn.query_row(
        "SELECT name, api_endpoint, api_format FROM api_providers WHERE id = ?1",
        params![id],
        |r| {
            Ok(ProviderRow {
                name: r.get(0)?,
                endpoint: r.get(1)?,
                api_format: r
                    .get::<_, Option<String>>(2)?
                    .unwrap_or_else(|| "openai".into()),
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// models 列是 `[{name, note}]`（也兼容纯字符串数组）
fn models_from_json(raw: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(items)) => items
            .iter()
            .filter_map(|it| match it {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o.get("name").and_then(|n| n.as_str()).map(String::from),
                _ => None,
            })
            .filter(|s| !s.trim().is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// 一条知识的整理结果：标题 / 标签 / 专属属性（**不含正文**）
#[derive(Debug, Clone, Default)]
pub struct KbEnrich {
    pub title: String,
    pub tags: Vec<String>,
    pub meta: Map<String, Value>,
}

/// 整理一段**已有的**正文：产出标题 / 标签 / 专属属性。
///
/// **绝不改写正文**：投喂进来的原文是用户的事实来源，让模型重写等于让它改数据。
/// 这里只做"打标 + 填属性"，把原文变成可检索、可筛选的知识 —— 这正是此前缺的一步
/// （缺了它，专属属性永远为空、检索只能靠原文关键词）。
pub async fn enrich_text(
    model: &KbModel,
    text: &str,
    base_name: &str,
    fields_json: &str,
    hint: &str,
) -> Result<KbEnrich, String> {
    ensure_openai_compatible(model)?;
    let system = "你是知识库助手，任务是为一段**已有的正文**打标签、填属性。\n\
        规则：\n\
        1. 只输出一个 JSON 对象，不要解释、不要 markdown 代码块；\n\
        2. 字段固定为 title / tags / meta，**不要输出或改写正文**；\n\
        3. title ≤ 30 字，概括这段正文讲的是什么（不要照抄第一句话）；\n\
        4. tags 给 3~6 个检索标签：优先写能直接命中这段内容的**具体**词\n\
           （函数名、类名、文件路径、命令、配置键、业务概念、专有名词）；\n\
           不要用「知识」「笔记」「文档」「内容」这类任何文本都能套的空词；\n\
        5. meta 按给定的专属属性字段定义填值：\n\
           - 单选字段只能取候选项之一；数字/日期/是否按类型给值；\n\
           - **能从正文判断出来的就要填**（例如正文说了模块名、章节、语言、来源类型），判断不了才省略该字段；\n\
           - 不要编造正文里没有依据的值，不要自创字段名；\n\
        6. 只依据正文内容判断，不要向外补充事实。";
    let trimmed: String = text.chars().take(4000).collect();
    let hint_line = if hint.trim().is_empty() {
        String::new()
    } else {
        format!("线索：{}\n", hint.trim())
    };
    let user = format!(
        "目标知识库：{}\n专属属性字段定义（JSON 数组）：{}\n{}\n正文：\n{}",
        base_name,
        if fields_json.trim().is_empty() {
            "[]"
        } else {
            fields_json
        },
        hint_line,
        trimmed
    );
    let raw = complete_json(model, system, &user).await?;
    let mut out = parse_enrich(&raw)?;
    out.meta = sanitize_meta(fields_json, &out.meta);
    Ok(out)
}

/// 解析整理输出（title 可缺省，tags/meta 可空 —— 整理是"增益"，不该因为模型少给字段就整条失败）
fn parse_enrich(raw: &str) -> Result<KbEnrich, String> {
    let text = extract_json(raw)
        .ok_or_else(|| "模型没有输出 JSON 对象（可换一个更擅长结构化输出的模型）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("解析模型输出失败: {}", e))?;
    Ok(KbEnrich {
        title: v["title"].as_str().unwrap_or("").trim().to_string(),
        tags: normalize_tags_json(&v["tags"]),
        meta: v["meta"].as_object().cloned().unwrap_or_default(),
    })
}

/// 片段投喂的 AI 预处理结果。
///
/// 与文件/网页的关键区别：**这里的正文允许被改写** —— 片段是用户自己粘的草稿，
/// 他期望的是"整理成规范知识"（分节、加标题、去口水话），而不是原样存下来再打标。
#[derive(Debug, Clone, Default)]
pub struct SnippetDraft {
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub meta: Map<String, Value>,
}

/// 把用户粘的原始片段整理成一条规范知识（**允许改写正文**）。
///
/// 边界：可以重排、分节、去重、补齐标点；**不得增删事实**，数字/命令/路径/专有名词必须原样保留。
/// 原始片段由调用方保留在候选说明里，用户采纳前可以逐字对比。
pub async fn process_snippet(
    model: &KbModel,
    raw: &str,
    base_name: &str,
    fields_json: &str,
    hint: &str,
) -> Result<SnippetDraft, String> {
    ensure_openai_compatible(model)?;
    let system = "你是知识库助手，任务是把用户粘来的一段**草稿**整理成一条规范、可检索的知识。\n\
        规则：\n\
        1. 只输出一个 JSON 对象，不要解释、不要 markdown 代码块；\n\
        2. 字段固定为 title / content / tags / meta；\n\
        3. content 是**整理后的正文**：分节与分段、补全标点、去掉口水话与重复表达，可用 markdown\n\
           小标题与列表；**不得增删或改动任何事实** —— 数字、单位、命令、路径、配置键、专有名词、\n\
           引用必须原样保留；拿不准的内容宁可原样留着，也不要「顺手改对」；\n\
        4. 原文零散、没有结构的，按内容归类成小节；原文本来就清楚的，不要为改而改；\n\
        5. **title 必填**（≤ 30 字）：概括这条知识讲什么，不能省略、不要输出空串（正文首行往往是\
           半句话，直接截取会得到很差的标题）；不要照抄第一句；\n\
        6. tags 给 3~6 个检索标签：优先写能直接命中内容的**具体**词\n\
           （函数名、类名、文件路径、命令、配置键、业务概念、专有名词）；\n\
           不要用「知识」「笔记」「文档」「内容」这类任何文本都能套的空词；\n\
        7. meta 按给定的专属属性字段定义填值：单选只能取候选项之一；数字/日期/是否按类型给值；\n\
           能从内容判断出来的就要填，判断不了才省略；不要编造、不要自创字段名；\n\
        8. 只依据用户给的内容整理，不要向外补充资料。";
    let hint_line = if hint.trim().is_empty() {
        String::new()
    } else {
        format!("线索：{}\n", hint.trim())
    };
    let user = format!(
        "目标知识库：{}\n专属属性字段定义（JSON 数组）：{}\n{}\n原始片段：\n{}",
        base_name,
        if fields_json.trim().is_empty() {
            "[]"
        } else {
            fields_json
        },
        hint_line,
        raw
    );
    let resp = complete_json_capped(model, system, &user, MAX_TOKENS).await?;
    let text = extract_json(&resp)
        .ok_or_else(|| "模型没有输出 JSON 对象（可换一个更擅长结构化输出的模型）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("解析模型输出失败: {}", e))?;
    let content = v["content"].as_str().unwrap_or("").trim().to_string();
    if content.is_empty() {
        return Err("模型没有给出整理后的正文".into());
    }
    Ok(SnippetDraft {
        title: v["title"]
            .as_str()
            .unwrap_or("")
            .trim()
            .chars()
            .take(40)
            .collect(),
        content,
        tags: normalize_tags_json(&v["tags"]),
        meta: sanitize_meta(
            fields_json,
            &v["meta"].as_object().cloned().unwrap_or_default(),
        ),
    })
}

/// AI 整理网页正文的产物：一条规范知识（**正文允许改写**）。
#[derive(Debug, Clone, PartialEq)]
pub struct PageSection {
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub meta: Map<String, Value>,
}

/// 一次送给模型的正文上限（字符）。
///
/// 与文件的 `plan_chunks` 不同：那条链路模型**只输出块序号**，所以能一次给它 60 块；
/// 这条链路模型**要输出正文本身**，输入与输出同量级 —— 还按 60 块给，输出会被撑爆。
///
/// 取值受"输出必须装得下输入"约束：中文约 1~1.5 token/字，4000 字正文的整理结果
/// 最多约 6000 token，配 `PAGE_MAX_TOKENS = 8000` 还剩 2000 给 JSON 外壳与标题标签。
/// **宁可分多批也不要把单批撑到临界**：一批输出被截断 = 整页降级回原始噪声文本，
/// 而多批只是多几次调用。单块本身就超预算时也要自成一批，否则永远攒不满。
const PAGE_CHARS_PER_CALL: usize = 4000;
/// 整理输出的 token 上限（见 `PAGE_CHARS_PER_CALL` 的换算）
const PAGE_MAX_TOKENS: u32 = 8000;

/// 网页投喂：让 AI 把抓到的正文**整理成若干条规范知识**（允许改写正文、允许丢弃噪声），
/// 同时给**整篇内容起一个能长期使用的文件名**。
///
/// 与 `plan_chunks`（文件投喂）的关键区别：文件是用户的原文，模型只定边界、正文一字不改；
/// 网页是**参考源** —— 本身非标准化，且抓下来混着大量噪声（导航/页脚/广告/相关阅读/评论区），
/// 只让模型划边界的话，出来的仍是一堆没条理的原始文本。
///
/// 底线与片段投喂一致：可以重排、分节、去噪、补标点，**不得增删或改动任何事实**
/// （数字、单位、日期、命令、路径、专有名词、条款编号原样保留）。
///
/// 返回 `(整篇的文件名, 各条知识)`。文件名只在**首批**里取（首批是页面开头，最接近 H1/导语）；
/// 用户自己填了名字时调用方会忽略它。失败一律由调用方降级为规则分块 —— AI 只是增益。
pub async fn organize_page(
    model: &KbModel,
    blocks: &[String],
    page_title: &str,
    base_name: &str,
    fields_json: &str,
    hint: &str,
) -> Result<(String, Vec<PageSection>), String> {
    ensure_openai_compatible(model)?;
    if blocks.is_empty() {
        return Err("没有可整理的正文".into());
    }
    let mut doc_title = String::new();
    let mut out: Vec<PageSection> = Vec::new();
    let mut batch: Vec<&String> = Vec::new();
    let mut chars = 0usize;
    for b in blocks {
        let n = b.chars().count();
        // 攒够预算就先发一批；单块本身就超预算时也要自成一批，否则永远攒不满
        if !batch.is_empty() && chars + n > PAGE_CHARS_PER_CALL {
            let (t, s) =
                organize_batch(model, &batch, page_title, base_name, fields_json, hint).await?;
            if doc_title.is_empty() {
                doc_title = t;
            }
            out.extend(s);
            batch.clear();
            chars = 0;
        }
        batch.push(b);
        chars += n;
    }
    if !batch.is_empty() {
        let (t, s) =
            organize_batch(model, &batch, page_title, base_name, fields_json, hint).await?;
        if doc_title.is_empty() {
            doc_title = t;
        }
        out.extend(s);
    }
    if out.is_empty() {
        return Err("模型没有给出任何可用的知识条目".into());
    }
    Ok((doc_title, out))
}

async fn organize_batch(
    model: &KbModel,
    batch: &[&String],
    page_title: &str,
    base_name: &str,
    fields_json: &str,
    hint: &str,
) -> Result<(String, Vec<PageSection>), String> {
    let system = "你是知识库助手，负责把**网页抓取到的正文**整理成若干条规范、可检索的知识，\
        并给整篇内容起一个文件名。\n\
        网页原文通常是非标准化的：既有真正的正文，也混着大量噪声。你的任务是把噪声剔掉、\
        把正文整理成条理清晰的知识。\n\
        规则：\n\
        1. 只输出一个 JSON 对象，不要解释、不要 markdown 代码块；\n\
        2. 形态：{\"docTitle\":\"整篇的文件名\",\"sections\":[{\"title\":\"…\",\"content\":\"…\",\
           \"tags\":[\"…\"],\"meta\":{}}]}；\n\
        3. **必须丢弃的噪声**（不要出现在输出里）：站内导航、面包屑、页脚、版权与备案信息、\n\
           广告与推广、订阅/登录/下载提示、「相关阅读/推荐」列表、评论区及其回复、标签云、\n\
           分享按钮文案、纯装饰性的图片说明、反复出现的站点名与栏目名；\n\
        4. content 是**整理后的正文**：按主题分节分段、补全标点、去掉重复与口水话，可用 markdown\n\
           小标题与列表让它更好读；**不得增删或改动任何事实** —— 数字、单位、日期、命令、路径、\n\
           配置键、专有名词、引用、条款编号必须原样保留；拿不准的内容宁可原样留着，\n\
           也不要「顺手改对」；\n\
        5. 一条只讲一个主题；原文本身就是清楚的一整篇，就整理成 1~3 条，不要为拆而拆；\n\
           孤立的小标题、一句提示、表格残片这类碎块，并入相邻同类，不要单独成条；\n\
        6. **docTitle 必填**（≤ 24 字）：给**整篇内容**起一个以后能长期使用的文件名 ——\n\
           用「主题 + 类型」的写法（如「政府采购管理办法」「React 19 迁移指南」「降压药用药说明」）。\n\
           不要用站点名、栏目名、「首页」「下载」「详情」「某某网」这类空话，也不要带网站后缀；\n\
           给到的网页原标题如果已经是个好名字，可以沿用它（删掉站点后缀即可）；\n\
        7. **title 必填**（≤ 30 字）：每条概括自己讲什么，不能省略、不要输出空串；\n\
           不要照抄第一句、不要带 # 号；\n\
        8. tags 给 3~6 个检索标签：优先写能直接命中内容的**具体**词（函数名、类名、文件路径、\n\
           命令、配置键、业务概念、专有名词）；不要用「知识」「笔记」「文档」「内容」「首页」\n\
           这类任何文本都能套的空词；\n\
        9. meta 按给定的专属属性字段定义填值：单选只能取候选项之一；数字/日期/是否按类型给值；\n\
           能从内容判断出来的就要填，判断不了才省略；不要编造、不要自创字段名；\n\
        10. **只依据给到的内容整理**，不要向外补充资料；**不要遗漏正文里的实质信息** ——\n\
           去噪是删掉噪声，不是删掉内容。";
    let hint_line = if hint.trim().is_empty() {
        String::new()
    } else {
        format!("线索：{}\n", hint.trim())
    };
    let title_line = if page_title.trim().is_empty() {
        String::new()
    } else {
        format!("网页原标题（未必可用，仅供参考）：{}\n", page_title.trim())
    };
    let user = format!(
        "目标知识库：{}\n专属属性字段定义（JSON 数组）：{}\n{}{}\n网页正文（原始抓取，含噪声）：\n\n{}",
        base_name,
        if fields_json.trim().is_empty() { "[]" } else { fields_json },
        hint_line,
        title_line,
        batch.iter().map(|b| b.as_str()).collect::<Vec<_>>().join("\n\n")
    );
    let raw = complete_json_capped(model, &system, &user, PAGE_MAX_TOKENS).await?;
    match parse_sections(&raw, fields_json) {
        Ok(v) => Ok(v),
        Err(e) => {
            // 与 `plan_batch` 同一条理由：格式跑偏或被 max_tokens 截断时，多要一次远比
            // 让整篇"整理"失败划算（用户勾了「存为 markdown」，失败就是这次投喂白做）
            log::warn!("[KB] 整理输出不可用，重试一次：{}", e);
            let retry_user = format!(
                "{}\n\n【上一次的输出不可用：{}】\n请重新回答：只输出一个 JSON 对象，\
                 不要解释、不要 markdown 代码块；若上次是条数太多写不完，请把同类内容合并成更少的条。",
                user, e
            );
            let raw = complete_json_capped(model, &system, &retry_user, PAGE_MAX_TOKENS).await?;
            parse_sections(&raw, fields_json)
        }
    }
}

/// 解析模型的整理结果，返回 `(docTitle, 各条知识)`。
///
/// 与 `parse_plans` 不同，这里**没有区间修复**可言 —— 正文是模型自己写出来的，
/// 所以只做三件事：容忍 `{sections:[…]}` 与"根就是数组"两种形态、丢掉没有正文的条目、
/// 把 meta 按字段定义洗一遍。
fn parse_sections(raw: &str, fields_json: &str) -> Result<(String, Vec<PageSection>), String> {
    let text = extract_json(raw)
        .ok_or_else(|| "模型没有输出 JSON 对象（可换一个更擅长结构化输出的模型）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("解析模型输出失败: {}", e))?;
    // 根是数组时没有 docTitle 可取，调用方回落到网页标题
    let doc_title = v["docTitle"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(40)
        .collect();
    let items = match v["sections"].as_array() {
        Some(a) if !a.is_empty() => a.clone(),
        _ => v.as_array().cloned().unwrap_or_default(),
    };
    let mut out: Vec<PageSection> = Vec::new();
    for it in items {
        let content = it["content"].as_str().unwrap_or("").trim().to_string();
        // 没有正文的条目一律丢掉：标题和标签单独存在没有意义，落库只会污染检索
        if content.is_empty() {
            continue;
        }
        out.push(PageSection {
            title: it["title"]
                .as_str()
                .unwrap_or("")
                .trim()
                .chars()
                .take(40)
                .collect(),
            content,
            tags: normalize_tags_json(&it["tags"]),
            meta: sanitize_meta(
                fields_json,
                &it["meta"].as_object().cloned().unwrap_or_default(),
            ),
        });
    }
    if out.is_empty() {
        return Err("模型没有给出任何可用的知识条目".into());
    }
    Ok((doc_title, out))
}

/// 一段对话里提炼出的一条候选知识（**可为 0 条** —— 闲聊本就没有可沉淀的东西）。
#[derive(Debug, Clone, PartialEq)]
pub struct DigestSection {
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub meta: Map<String, Value>,
    /// 这条知识出自对话里的哪几句（**原样摘录**，用户核对时能对回原文）
    pub source_quote: String,
}

/// 交给模型的一个知识库定义（多库自动沉淀时用）。
///
/// 为什么要把**全部库**一次性给模型：自动沉淀没有用户预先指定的目标库，
/// 而"专属属性"与库强相关（每库 `fields` 不同），必须在同一次调用里既选库又填属性，
/// 否则要么再调一次模型补属性、要么让用户手工填。库数量通常是个位数，
/// 目录（id / 名称 / 说明 / 字段定义）的 token 量相对对话内容可忽略。
#[derive(Debug, Clone)]
pub struct KbBaseDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub fields_json: String,
}

/// 一次送给模型的对话上限（字符）。
///
/// 对话沉淀要模型**输出**提炼结果，输入与输出同量级 —— 不设上限的话，勾上几十条长发言
/// （群聊里尤其常见，一次发言几百上千字）就是一次几十万字符的请求：撑爆上下文、白烧额度。
/// 超长时按前 N 字整理，并在正文里**如实告诉模型**已截断（否则它会当成完整对话来下结论）；
/// 调用方同时把"被截断了"回报给用户。
pub const DIGEST_MAX_CHARS: usize = 24_000;

/// 对话是否超过单次整理上限。调用方（命令层）据此回报用户"只整理了前一部分"；
/// `digest_conversation` 内部用的是同一个判据 —— 两处必须一致，否则会出现
/// "模型按截断后的内容整理、界面却说没截断"。
pub fn conversation_is_truncated(conversation: &str) -> bool {
    conversation.chars().count() > DIGEST_MAX_CHARS
}

/// 把**一段带角色的对话**整理成若干条候选知识。
///
/// 与 `process_snippet`（用户粘的一段草稿 → 一条知识）的区别：对话里常含**多个主题**，
/// 而且混着过程性内容（寒暄、试错、"我再查查"）。所以这里产出的是 **0..N 条**，
/// 由模型判断哪几句真的沉淀出了结论。
///
/// 三条硬要求（写进提示词，也写在返回值里）：
///   ① **带角色**：输入标明谁说的（用户 / 助手；群聊里则是各参与者名），模型才不会把
///      "别人提的建议"当成"用户的事实"；
///   ② **只提炼不发明**：只能写对话里已经出现的事实与结论，不得补充外部知识、不得推测；
///   ③ **可溯源**：每条都要 `sourceQuote` —— 从对话里原样摘一句原话，用户采纳前能对回原文。
///
/// 因此正文**允许改写**（去掉过程性对话、分节、补标点），但**不得增删或改动事实** ——
/// 与片段投喂同一条底线：原始对话整段保留在候选说明里，可逐字比对。
pub async fn digest_conversation(
    model: &KbModel,
    conversation: &str,
    base_name: &str,
    fields_json: &str,
) -> Result<Vec<DigestSection>, String> {
    ensure_openai_compatible(model)?;
    if conversation.trim().is_empty() {
        return Err("没有可整理的对话内容".into());
    }
    let system = "你是知识库助手，任务是从**一段对话记录**里提炼出可长期复用的知识候选。\n\
        规则：\n\
        1. 只输出一个 JSON 对象，不要解释、不要 markdown 代码块；\n\
        2. 形态：{\"notes\":[{\"title\":\"…\",\"content\":\"…\",\"tags\":[\"…\"],\"meta\":{},\
           \"sourceQuote\":\"…\"}]}；\n\
        3. **只提炼，不发明**：只能写对话里已经明确出现的事实、结论、决定、方法、参数、\
           命令、路径、数字。**不得补充对话之外的知识，不得推测**；多人对话里尤其要分清\
           「某位参与者主张的」≠「已经确认的」：**不得把任何人的建议/推测写成已确认的事实**\
           —— 拿不准就原样保留语气（如「建议……」）；\n\
        4. 一段对话常有**多个主题**，按主题拆成多条；同一主题的讨论只出一条；\n\
        5. **没有可沉淀的内容就返回空数组**（`{\"notes\":[]}`）：寒暄、闲聊、纯过程性追问、\
           尚无结论的讨论都不要硬凑成知识。宁可 0 条，不要凑数；\n\
        6. content 是**提炼后的正文**：去掉过程性对话（「好的」「我试试」「稍等」）与重复，\
           分节分段、补全标点，可用 markdown 小标题与列表；要能**脱离这段对话独立读懂**；\n\
        7. **title 必填**（≤ 30 字）：概括这条知识讲什么，不要照抄第一句、不要带 # 号；\n\
        8. tags 给 3~6 个检索标签：优先写能直接命中内容的**具体**词（函数名、类名、文件路径、\n\
           命令、配置键、业务概念、专有名词）；不要用「知识」「笔记」「文档」「内容」这类空词；\n\
        9. meta 按给定的专属属性字段定义填值：单选只能取候选项之一；数字/日期/是否按类型给值；\n\
           能从对话判断出来的就要填，判断不了才省略；不要编造、不要自创字段名；\n\
        10. **sourceQuote 必填**（≤ 80 字）：从对话里**原样摘**一句最能代表这条知识的原话\
           （优先摘用户说的、含具体事实的那句），用于溯源。不要改写、不要拼凑。";
    // 超长对话按前 N 字整理，并把"已截断"写进正文 —— 模型自己不知道边界在哪，
    // 不说清它会把前半段当成整段对话来下结论（调用方另把同样的判断回报给用户）
    let truncated = conversation_is_truncated(conversation);
    let shown: String = if truncated {
        conversation.chars().take(DIGEST_MAX_CHARS).collect()
    } else {
        conversation.to_string()
    };
    let cutoff = if truncated {
        format!(
            "\n\n【注意】对话过长，以上仅为其**前 {} 字**（并非完整对话）；请只依据已给到的内容提炼。",
            DIGEST_MAX_CHARS
        )
    } else {
        String::new()
    };
    let user = format!(
        "目标知识库：{}\n专属属性字段定义（JSON 数组）：{}\n\n对话记录（含角色）：\n\n{}{}",
        base_name,
        if fields_json.trim().is_empty() {
            "[]"
        } else {
            fields_json
        },
        shown,
        cutoff
    );
    let raw = complete_json_capped(model, &system, &user, PAGE_MAX_TOKENS).await?;
    match parse_digest(&raw, fields_json) {
        Ok(v) => Ok(v),
        Err(e) => {
            // 与 `organize_batch` 同一条理由：JSON 被包住或被 max_tokens 截断，多要一次
            // 远比让这次沉淀整体失败划算
            log::warn!("[KB] 对话沉淀输出不可用，重试一次：{}", e);
            let retry_user = format!(
                "{}\n\n【上一次的输出不可用：{}】\n请重新回答：只输出一个 JSON 对象，\
                 不要解释、不要 markdown 代码块；若上次是条数太多写不完，请把同类内容合并成更少的条。",
                user, e
            );
            let raw = complete_json_capped(model, &system, &retry_user, PAGE_MAX_TOKENS).await?;
            parse_digest(&raw, fields_json)
        }
    }
}

/// 多库版对话沉淀：把**全部知识库**的定义一次性交给模型，由模型为每条产出**选定目标库**，
/// 并按该库的专属属性字段填 `meta` —— 一次调用同时完成"选库"与"填属性"。
///
/// 与单库版 `digest_conversation` 的分工：
///   - 单库版：**用户已明确指定库**的显式入口（多选沉淀 / 片段投喂 / 工作流产出等）；
///   - 多库版：**自动沉淀**（用户没有预先指定库，由模型按内容归置）。
///
/// 返回 `Vec<(base_id, DigestSection)>`；`base_id` 一定是 `bases` 里真实存在的 id
/// （模型给了无法匹配的库时，该条被丢弃并记日志 —— 候选必须有真实 kb_id）。
pub async fn digest_conversation_multi(
    model: &KbModel,
    conversation: &str,
    bases: &[KbBaseDef],
) -> Result<Vec<(String, DigestSection)>, String> {
    ensure_openai_compatible(model)?;
    if conversation.trim().is_empty() {
        return Err("没有可整理的对话内容".into());
    }
    if bases.is_empty() {
        return Err("没有可用的知识库".into());
    }

    // 目录：每个库的 id / 名称 / 说明 / 专属字段定义（含候选项、类型）
    let catalog: Vec<Value> = bases
        .iter()
        .map(|b| {
            let fields: Value = serde_json::from_str(if b.fields_json.trim().is_empty() {
                "[]"
            } else {
                b.fields_json.as_str()
            })
            .unwrap_or_else(|_| json!([]));
            json!({ "id": b.id, "name": b.name, "description": b.description, "fields": fields })
        })
        .collect();
    let catalog_str = serde_json::to_string_pretty(&catalog).unwrap_or_else(|_| "[]".into());

    let system = "你是知识库助手，任务是从**一段对话记录**里提炼出可长期复用的知识候选，\
        并为每条候选**选择它最该归属的那一个知识库**。\n\
        规则：\n\
        1. 只输出一个 JSON 对象，不要解释、不要 markdown 代码块；\n\
        2. 形态：{\"notes\":[{\"base\":\"<知识库 id>\",\"title\":\"…\",\"content\":\"…\",\
           \"tags\":[\"…\"],\"meta\":{},\"sourceQuote\":\"…\"}]}；\n\
        3. **base 必填**：只能填「知识库目录」里某个库的 **id**（原样照抄，不要用名称、不要自创）；\
           一条候选只归一个库；\n\
        4. **只提炼，不发明**：只能写对话里已经明确出现的事实、结论、决定、方法、参数、\
           命令、路径、数字。**不得补充对话之外的知识，不得推测**；多人对话里尤其要分清\
           「某位参与者主张的」≠「已经确认的」：**不得把任何人的建议/推测写成已确认的事实**\
           —— 拿不准就原样保留语气（如「建议……」）；\n\
        5. 一段对话常有**多个主题**，按主题拆成多条；同一主题的讨论只出一条；\n\
        6. **没有可沉淀的内容就返回空数组**（`{\"notes\":[]}`）：寒暄、闲聊、纯过程性追问、\
           尚无结论的讨论都不要硬凑成知识。宁可 0 条，不要凑数；\n\
        7. content 是**提炼后的正文**：去掉过程性对话（「好的」「我试试」「稍等」）与重复，\
           分节分段、补全标点，可用 markdown 小标题与列表；要能**脱离这段对话独立读懂**；\n\
        8. **title 必填**（≤ 30 字）：概括这条知识讲什么，不要照抄第一句、不要带 # 号；\n\
        9. tags 给 3~6 个检索标签：优先写能直接命中内容的**具体**词（函数名、类名、文件路径、\
           命令、配置键、业务概念、专有名词）；不要用「知识」「笔记」「文档」「内容」这类空词；\n\
        10. meta 按**所选知识库**的 `fields` 定义填值：单选只能取候选项之一；数字/日期/是否\
           按类型给值；能从对话判断出来的就要填，判断不了才省略；不要编造、不要自创字段名；\n\
        11. **sourceQuote 必填**（≤ 80 字）：从对话里**原样摘**一句最能代表这条知识的原话\
           （优先摘用户说的、含具体事实的那句），用于溯源。不要改写、不要拼凑。";
    let truncated = conversation_is_truncated(conversation);
    let shown: String = if truncated {
        conversation.chars().take(DIGEST_MAX_CHARS).collect()
    } else {
        conversation.to_string()
    };
    let cutoff = if truncated {
        format!(
            "\n\n【注意】对话过长，以上仅为其**前 {} 字**（并非完整对话）；请只依据已给到的内容提炼。",
            DIGEST_MAX_CHARS
        )
    } else {
        String::new()
    };
    let user = format!(
        "知识库目录（JSON 数组；base 只能填其中的 id）：\n{}\n\n对话记录（含角色）：\n\n{}{}",
        catalog_str, shown, cutoff
    );
    let raw = complete_json_capped(model, &system, &user, PAGE_MAX_TOKENS).await?;
    match parse_digest_multi(&raw, bases) {
        Ok(v) => Ok(v),
        Err(e) => {
            log::warn!("[KB] 多库对话沉淀输出不可用，重试一次：{}", e);
            let retry_user = format!(
                "{}\n\n【上一次的输出不可用：{}】\n请重新回答：只输出一个 JSON 对象，\
                 不要解释、不要 markdown 代码块；若上次是条数太多写不完，请把同类内容合并成更少的条。",
                user, e
            );
            let raw = complete_json_capped(model, &system, &retry_user, PAGE_MAX_TOKENS).await?;
            parse_digest_multi(&raw, bases)
        }
    }
}

/// 解析多库版对话沉淀的输出：每条解析出目标库（按 id，其次名称），并按**该库**的字段定义净化 meta。
///
/// 目标库匹配不上时（模型编了不存在的库）：若只有一个候选库则回落到它，否则**丢弃该条**并记日志 ——
/// `kb_candidates.kb_id` 必须指向真实存在的库。
fn parse_digest_multi(
    raw: &str,
    bases: &[KbBaseDef],
) -> Result<Vec<(String, DigestSection)>, String> {
    let text = extract_json(raw)
        .ok_or_else(|| "模型没有输出 JSON 对象（可换一个更擅长结构化输出的模型）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("解析模型输出失败: {}", e))?;
    let items = match v["notes"].as_array() {
        Some(a) => a.clone(),
        None => v.as_array().cloned().unwrap_or_default(),
    };
    let mut out: Vec<(String, DigestSection)> = Vec::new();
    for it in items {
        let content = it["content"].as_str().unwrap_or("").trim().to_string();
        if content.is_empty() {
            continue;
        }
        let raw_base = it["base"].as_str().unwrap_or("").trim();
        let base = bases
            .iter()
            .find(|b| b.id == raw_base || b.name == raw_base)
            .or_else(|| {
                if bases.len() == 1 {
                    bases.first()
                } else {
                    None
                }
            });
        let Some(base) = base else {
            log::warn!(
                "[KB] 自动沉淀丢弃一条：模型给的知识库无法匹配（base={:?}）",
                raw_base
            );
            continue;
        };
        out.push((
            base.id.clone(),
            DigestSection {
                title: it["title"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(40)
                    .collect(),
                content,
                tags: normalize_tags_json(&it["tags"]),
                meta: sanitize_meta(
                    &base.fields_json,
                    &it["meta"].as_object().cloned().unwrap_or_default(),
                ),
                source_quote: it["sourceQuote"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(200)
                    .collect(),
            },
        ));
    }
    Ok(out)
}

/// 解析对话沉淀的输出。
///
/// 与 `parse_sections` 的关键差别：**空结果是合法的**（闲聊没有可沉淀的知识），
/// 所以这里不因 `notes` 为空报错 —— 调用方据此返回"0 条候选"，而不是让整次操作失败。
/// 仍然丢掉没有正文的条目（标题单独存在没有意义）。
fn parse_digest(raw: &str, fields_json: &str) -> Result<Vec<DigestSection>, String> {
    let text = extract_json(raw)
        .ok_or_else(|| "模型没有输出 JSON 对象（可换一个更擅长结构化输出的模型）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("解析模型输出失败: {}", e))?;
    let items = match v["notes"].as_array() {
        Some(a) => a.clone(),
        // 也容忍模型直接把数组当根
        None => v.as_array().cloned().unwrap_or_default(),
    };
    let mut out: Vec<DigestSection> = Vec::new();
    for it in items {
        let content = it["content"].as_str().unwrap_or("").trim().to_string();
        if content.is_empty() {
            continue;
        }
        out.push(DigestSection {
            title: it["title"]
                .as_str()
                .unwrap_or("")
                .trim()
                .chars()
                .take(40)
                .collect(),
            content,
            tags: normalize_tags_json(&it["tags"]),
            meta: sanitize_meta(
                fields_json,
                &it["meta"].as_object().cloned().unwrap_or_default(),
            ),
            source_quote: it["sourceQuote"]
                .as_str()
                .unwrap_or("")
                .trim()
                .chars()
                .take(200)
                .collect(),
        });
    }
    Ok(out)
}

/// 把整理结果拼成**落盘的那份文档**：`# 文件名` + 各条正文，条间留一个空行。
///
/// 为什么要拼：库里这份知识的样子是「一份文档 + 它的 N 条知识」，那落盘的 `.md` 就该是这份文档本身，
/// 而不是抓下来那坨带噪声、没结构的原始文本 —— 用户点「打开原文」看到什么，
/// 才决定这份文件知识以后还能不能用上。
///
/// 条间留空行不只是排版：**每条正文都能在文件里找到一段连续文本**，文件与条目才对得上
/// （不是"文件是一坨、条目是另一坨"）。注意 `split_blocks_ranged` 会把 `#` 标题粘到后继块，
/// 所以再次分块时第一个块会带上标题 —— 那是分块器的既定行为，不影响这里的对应关系。
pub fn compose_page_doc(doc_title: &str, sections: &[PageSection]) -> String {
    let mut out = String::new();
    if !doc_title.trim().is_empty() {
        out.push_str("# ");
        out.push_str(doc_title.trim());
        out.push_str("\n\n");
    }
    for s in sections {
        let body = s.content.trim();
        if body.is_empty() {
            continue;
        }
        out.push_str(body);
        out.push_str("\n\n");
    }
    if out.is_empty() {
        return out;
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// AI 给出的**分组计划**：把本地粗切的第 `from..=to` 块合成一条知识。
///
/// 这是「AI 参与分块」的契约：模型只输出**块序号 + 标题 + 标签 + 属性**；
/// 正文既不进模型、也不由模型输出 —— 调用方按序号从原文区间切片，正文一字不改。
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkPlan {
    /// 1-based，含
    pub from: usize,
    /// 1-based，含
    pub to: usize,
    pub title: String,
    pub tags: Vec<String>,
    pub meta: Map<String, Value>,
}

/// 每次送模型的块数：预览截断后输入约 60×200 字符，输出计划也能一次装下
const PLAN_BLOCKS_PER_CALL: usize = 60;
/// 每块给模型的预览长度（只用于判断主题与边界；正文不进模型）
const PLAN_PREVIEW_CHARS: usize = 200;
/// 计划输出的 token 上限（每条约 60~100 token）
const PLAN_MAX_TOKENS: u32 = 4000;

/// 让 AI 决定"哪些块合成一条、丢弃哪些块"，并同时给出每条的标题/标签/属性。
///
/// 与旧做法（先机械分块、再逐块打标）的区别：**分块本身由模型决定**，块长与主题由模型把握，
/// 因此不会出现"块内断开、段内断开"；而正文仍由调用方按原文区间切片，模型改不了事实来源。
/// 块数超过一批时分批调用（批边界处不再跨批合并，属可接受的取舍）。
pub async fn plan_chunks(
    model: &KbModel,
    blocks: &[String],
    base_name: &str,
    fields_json: &str,
    hint: &str,
    // `want_title`：是否顺便让模型给**整篇**取个名字（界面上的「允许重命名」）。
    // 之所以"顺便"：这次调用本来就在读这些块，多要一个字段不增加调用次数与延迟。
    want_title: bool,
) -> Result<(String, Vec<ChunkPlan>), String> {
    ensure_openai_compatible(model)?;
    if blocks.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    let total = blocks.len();
    let mut out: Vec<ChunkPlan> = Vec::new();
    let mut doc_title = String::new();
    let mut start = 0usize;
    while start < total {
        let end = (start + PLAN_BLOCKS_PER_CALL).min(total);
        // 只问首批：整篇的名字在首批（文档开头）就能判断，后续批次再问一遍是白花输出 token
        let (title, plans) = plan_batch(
            model,
            &blocks[start..end],
            start + 1,
            total,
            base_name,
            fields_json,
            hint,
            want_title && start == 0,
        )
        .await?;
        if !title.is_empty() {
            doc_title = title;
        }
        // 批内序号 → 全文序号
        out.extend(plans.into_iter().map(|mut p| {
            p.from += start;
            p.to += start;
            p
        }));
        start = end;
    }
    Ok((doc_title, out))
}

#[allow(clippy::too_many_arguments)]
async fn plan_batch(
    model: &KbModel,
    batch: &[String],
    global_start: usize,
    global_total: usize,
    base_name: &str,
    fields_json: &str,
    hint: &str,
    want_title: bool,
) -> Result<(String, Vec<ChunkPlan>), String> {
    let n = batch.len();
    let mut listing = String::new();
    for (i, b) in batch.iter().enumerate() {
        let chars = b.chars().count();
        let preview: String = b.chars().take(PLAN_PREVIEW_CHARS).collect();
        listing.push_str(&format!(
            "{}. ({}字) {}\n",
            i + 1,
            chars,
            preview.replace('\n', " ")
        ));
    }
    // 「允许重命名」时顺带要一个整篇的名字（拼进规则 2，避免打乱后面的编号）
    let title_req = if want_title {
        "；本批是全文开头，另请在 docTitle 字段给出**整篇文档**的名字：用「主题 + 类型」的写法\
         （如「政府采购管理办法」「XX 系统运维手册」），不超过 30 字，不要带扩展名、\
         不要照抄文件名、不要空话（「文档」「未命名」「首页」）"
    } else {
        ""
    };
    let system = format!(
        "你是知识库助手，负责把一篇长文档的**段落块**分组成若干条知识。\n\
        规则：\n\
        1. 只输出一个 JSON 对象，不要解释、不要 markdown 代码块；\n\
        2. 形态：{{\"chunks\":[{{\"from\":1,\"to\":3,\"title\":\"…\",\"tags\":[\"…\"],\"meta\":{{}}}}]}}{title_req}；\n\
        3. from/to 是**本次列表里的块序号**（含两端），必须按序、互不重叠；**不要复述或改写块的正文**；\n\
        4. 分组要让每条自成一个小主题：相邻且同主题的块合成一条；出现新章节标题处应断开；\n\
        5. 每条目标长度约 400~1200 字（列表里给了每块字数，据此估算）；过短的段落可与相邻同类合并；\n\
        6. **title 必填**：每条都要给标题（≤ 30 字），概括这一组讲的是什么；标题缺失会导致这条知识\
        只能用文件名当标题，所以**不能省略、不要输出空串**；不要照抄第一句、不要带 # 号；\n\
        7. tags 给 3~6 个检索标签：优先写能直接命中内容的**具体**词（函数名、类名、文件路径、命令、\
        配置键、业务概念、专有名词）；不要用「知识」「笔记」「文档」「内容」这类任何文本都能套的空词；\n\
        8. meta 按给定的专属属性字段定义填值：单选只能取候选项之一；数字/日期/是否按类型给值；\
        **能从内容判断出来的就要填**，判断不了才省略该字段；不要编造、不要自创字段名；\n\
        9. 只依据给定内容判断，不要向外补充事实；\n\
        10. **不要丢弃任何块**：所有块都必须被某个组的 from/to 覆盖 —— 这是用户的原文，\
        丢内容等于丢数据。"
    );
    let hint_line = if hint.trim().is_empty() {
        String::new()
    } else {
        format!("线索：{}\n", hint.trim())
    };
    let user = format!(
        "目标知识库：{}\n专属属性字段定义（JSON 数组）：{}\n{}\n\
         本次共 {} 个块（编号 1~{}；它们是全文的第 {}~{} 块，全文共 {} 块）。\
         每块给出「序号、字数、开头预览」：\n{}",
        base_name,
        if fields_json.trim().is_empty() {
            "[]"
        } else {
            fields_json
        },
        hint_line,
        n,
        n,
        global_start,
        global_start + n - 1,
        global_total,
        listing
    );
    let raw = complete_json_capped(model, &system, &user, PLAN_MAX_TOKENS).await?;
    match parse_plans(&raw, n, fields_json) {
        Ok(v) => Ok(v),
        Err(e) => {
            // 输出不可用大多是两种情况：JSON 被解释文字包住/格式跑偏；分组过多导致输出被
            // max_tokens 截断。两种都值得再要一次 —— 一次调用换掉"整篇降级为规则分块"，
            // 后面那条路的代价是用户看到一堆半句话截出来的标题，还以为是功能没生效。
            log::warn!("[KB] 分组输出不可用，重试一次：{}", e);
            let retry_user = format!(
                "{}\n\n【上一次的输出不可用：{}】\n请重新回答：只输出一个 JSON 对象，\
                 不要解释、不要 markdown 代码块；若上次是因为分组太多写不完，\
                 请把相邻的同类块合并成更少的组再输出。",
                user, e
            );
            let raw = complete_json_capped(model, &system, &retry_user, PLAN_MAX_TOKENS).await?;
            parse_plans(&raw, n, fields_json)
        }
    }
}

/// 解析并**修复**模型的分组计划：
/// - 越界/倒序的区间丢弃；与已接受区间重叠的也丢弃（先到先得，保证不重叠）；
/// - **没被任何组覆盖的块并入前一组**（模型漏块是常见失误，不能让用户内容凭空消失）。
///
/// 这里没有任何"丢弃块"的余地 —— 走这条链路的只有文件投喂，那是用户的原文，
/// 丢内容等于丢数据；网页投喂允许去噪与改写，走的是 `organize_page`。
///
/// 返回 `(整篇标题, 分组)`：标题是「允许重命名」要用的，模型没给就是空串（调用方保留原名）。
fn parse_plans(
    raw: &str,
    batch_len: usize,
    fields_json: &str,
) -> Result<(String, Vec<ChunkPlan>), String> {
    let text = extract_json(raw)
        .ok_or_else(|| "模型没有输出 JSON 对象（可换一个更擅长结构化输出的模型）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("解析模型输出失败: {}", e))?;
    let doc_title = v["docTitle"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(40)
        .collect::<String>();
    let items = match v["chunks"].as_array() {
        Some(a) if !a.is_empty() => a.clone(),
        // 也容忍模型直接把数组当根对象
        _ => v.as_array().cloned().unwrap_or_default(),
    };

    let mut ranges: Vec<ChunkPlan> = Vec::new();
    for it in items {
        let from = it["from"].as_u64().unwrap_or(0) as usize;
        let to = it["to"].as_u64().unwrap_or(0) as usize;
        if from == 0 || to == 0 || from > to || to > batch_len {
            continue;
        }
        if ranges.iter().any(|r| !(to < r.from || from > r.to)) {
            continue; // 与已接受区间重叠
        }
        ranges.push(ChunkPlan {
            from,
            to,
            title: it["title"]
                .as_str()
                .unwrap_or("")
                .trim()
                .chars()
                .take(40)
                .collect(),
            tags: normalize_tags_json(&it["tags"]),
            meta: it["meta"].as_object().cloned().unwrap_or_default(),
        });
    }
    if ranges.is_empty() {
        return Err("模型没有给出可用的分组（from/to 全部非法）".into());
    }
    ranges.sort_by_key(|r| r.from);

    // 覆盖率修复：落在空隙里的块并进前一组（第一组之前则并进后一组）
    let mut covered: Vec<bool> = vec![false; batch_len + 1];
    for r in &ranges {
        for i in r.from..=r.to {
            covered[i] = true;
        }
    }
    for i in 1..=batch_len {
        if covered[i] {
            continue;
        }
        if let Some(prev) = ranges.iter_mut().rev().find(|r| r.to < i) {
            prev.to = i; // 空隙并入前一组（中间都是空隙，连续扩到 i 是安全的）
        } else if let Some(first) = ranges.iter_mut().find(|r| r.from > i) {
            first.from = i;
        }
    }

    // meta 按字段定义校验（丢弃未知字段、纠正类型、单选命中候选项）
    for r in ranges.iter_mut() {
        r.meta = sanitize_meta(fields_json, &r.meta);
    }
    Ok((doc_title, ranges))
}

fn normalize_tags_json(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(arr) = v.as_array() {
        for t in arr {
            let s = match t {
                Value::String(s) => s.trim().to_string(),
                // 有些模型会把标签给成 {name: "..."} 或数字
                Value::Object(o) => o
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            if s.is_empty() || s.chars().count() > 24 {
                continue;
            }
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out.truncate(8);
    out
}

/// 按专属字段定义校验模型给的 meta：**丢弃未知字段、按类型纠正、单选必须命中候选项**。
///
/// 不校验的话，模型编的字段名与越界取值会直接进库，筛选与图谱连边就全乱了。
fn sanitize_meta(fields_json: &str, raw: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    let Ok(Value::Array(fields)) = serde_json::from_str::<Value>(fields_json) else {
        return out;
    };
    for f in fields {
        let key = f["key"].as_str().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        let Some(Value::String(_)) = raw.get(key) else {
            // 也接受数字/布尔直接给值
            let Some(v) = raw.get(key) else { continue };
            if let Some(n) = v.as_f64() {
                out.insert(key.to_string(), json!(n));
                continue;
            }
            if let Some(b) = v.as_bool() {
                out.insert(key.to_string(), json!(b));
                continue;
            }
            continue;
        };
        let ty = f["type"].as_str().unwrap_or("text");
        let value_str = raw[key].as_str().unwrap_or("").trim();
        if value_str.is_empty() {
            continue;
        }
        match ty {
            "select" => {
                let options: Vec<String> = f["options"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|o| o.as_str())
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                // 命中候选项才收（忽略大小写/空白差异）
                if let Some(hit) = options
                    .iter()
                    .find(|o| o.trim().eq_ignore_ascii_case(value_str))
                {
                    out.insert(key.to_string(), json!(hit));
                }
            }
            "number" => {
                if let Ok(n) = value_str.parse::<f64>() {
                    out.insert(key.to_string(), json!(n));
                }
            }
            "bool" => match value_str.to_lowercase().as_str() {
                "true" | "是" | "yes" | "y" | "1" => {
                    out.insert(key.to_string(), json!(true));
                }
                "false" | "否" | "no" | "n" | "0" => {
                    out.insert(key.to_string(), json!(false));
                }
                _ => {}
            },
            "date" => {
                // 只收 YYYY-MM-DD（模型常给 "2024年3月" 之类，宁缺勿滥）
                let bytes = value_str.as_bytes();
                let ok = value_str.len() == 10
                    && bytes[4] == b'-'
                    && bytes[7] == b'-'
                    && value_str.chars().enumerate().all(|(i, c)| {
                        if i == 4 || i == 7 {
                            c == '-'
                        } else {
                            c.is_ascii_digit()
                        }
                    });
                if ok {
                    out.insert(key.to_string(), json!(value_str));
                }
            }
            _ => {
                let v: String = value_str.chars().take(200).collect();
                out.insert(key.to_string(), json!(v));
            }
        }
    }
    out
}

/// 按主题生成一条知识草稿（标题 / 正文 / 标签 / 专属属性一次产出）
pub async fn generate_draft(
    model: &KbModel,
    topic: &str,
    base_name: &str,
    fields_json: &str,
) -> Result<KbDraft, String> {
    ensure_openai_compatible(model)?;
    let system = "你是知识库助手，负责把用户给的主题/要求写成一条可长期复用的知识条目。\n\
        规则：\n\
        1. 只输出一个 JSON 对象，不要解释、不要 markdown 代码块；\n\
        2. 字段固定为 title / content / tags / meta；\n\
        3. title ≤ 40 字，一句话说清这条知识是什么；\n\
        4. content 300~800 字，分段或分点写，要能脱离上下文独立读懂；\n\
        5. tags 给 3~5 个检索标签（字符串数组）；\n\
        6. meta 按给定的专属属性字段定义填值：单选字段只能取候选项之一，类型要匹配，判断不了的字段直接省略（不要编）；\n\
        7. 不要编造具体事实。涉及你不确定的信息，就在 content 里写明「待验证」或不确定之处，不要假装确定。";
    let user = format!(
        "目标知识库：{}\n专属属性字段定义（JSON 数组）：{}\n主题/要求：{}\n\n请输出 JSON。",
        base_name,
        if fields_json.trim().is_empty() {
            "[]"
        } else {
            fields_json
        },
        topic
    );
    let raw = complete_json(model, system, &user).await?;
    parse_draft(&raw)
}

fn ensure_openai_compatible(model: &KbModel) -> Result<(), String> {
    if model.api_format.to_lowercase().contains("anthropic") {
        return Err(format!(
            "提供商「{}」是 Anthropic 原生格式，没有 /chat/completions 端点；请在「设置 › 知识库」里指定 OpenAI 兼容的模型",
            model.provider_name
        ));
    }
    Ok(())
}

/// 一次非流式对话，要求 JSON 输出；provider 不支持 `response_format` 时降级重试一次
async fn complete_json(model: &KbModel, system: &str, user: &str) -> Result<String, String> {
    complete_json_capped(model, system, user, MAX_TOKENS).await
}

/// 同上，但可指定输出上限（分组计划要输出几十条，2000 token 不够）
async fn complete_json_capped(
    model: &KbModel,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let client = ApiClient::new(
        model.endpoint.trim().to_string(),
        model.api_key.clone(),
        ApiFormat::OpenAI,
    );
    let build = |json_mode: bool| ChatRequest {
        model: model.model.clone(),
        messages: vec![ChatMessage::system(system), ChatMessage::user(user)],
        tools: None,
        tool_choice: None,
        stream: false,
        temperature: Some(0.3),
        max_tokens: Some(max_tokens),
        response_format: if json_mode {
            Some(json!({ "type": "json_object" }))
        } else {
            None
        },
    };
    match client.chat(&build(true)).await {
        Ok(resp) => {
            // 非流式响应里 `usage` 是有值的（流式那条路才走回调），别丢 —— 知识库的账全靠这里记
            if let Some(u) = resp.usage {
                model.usage.push(u);
            }
            Ok(resp.content)
        }
        Err(e) if looks_like_json_mode_unsupported(&e) => {
            log::warn!(
                "[KB] provider 可能不支持 response_format，降级重试一次: {}",
                e
            );
            match client.chat(&build(false)).await {
                Ok(resp) => {
                    if let Some(u) = resp.usage {
                        model.usage.push(u);
                    }
                    Ok(resp.content)
                }
                Err(e) => Err(e.to_string()),
            }
        }
        Err(e) => Err(e),
    }
}

/// 请求形态问题通常是 400/422；各家文案不一，只做粗判并只降级重试一次
fn looks_like_json_mode_unsupported(err: &str) -> bool {
    err.contains("400") || err.contains("422") || err.to_lowercase().contains("response_format")
}

/// 解析模型输出：容忍 ```json 包裹与前后多余文字（只取第一个 JSON 对象）
fn parse_draft(raw: &str) -> Result<KbDraft, String> {
    let text = extract_json(raw)
        .ok_or_else(|| "模型没有输出 JSON 对象（可换一个更擅长结构化输出的模型）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("解析模型输出失败: {}", e))?;
    let title = v["title"].as_str().unwrap_or("").trim().to_string();
    let content = v["content"].as_str().unwrap_or("").trim().to_string();
    if title.is_empty() || content.is_empty() {
        return Err("模型输出缺少 title 或 content".into());
    }
    Ok(KbDraft {
        title,
        content,
        tags: v["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        meta: v["meta"].as_object().cloned().unwrap_or_default(),
    })
}

/// 从模型的输出里抠出 JSON 文本（容忍 markdown 代码块围栏与前后的废话）。
///
/// 定位方式是**第一个结构性括号**：`{` 与 `[` 谁先出现就按谁配对。
/// 只认 `{` 的话，`[{…}]`（模型把数组当根）会被截成内层对象 —— 于是 `parse_plans` /
/// `parse_sections` 里"也容忍数组当根"那两处分支永远走不到（这个 bug 由单元测试抓出来）。
fn extract_json(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let start = bytes.iter().position(|b| *b == b'{' || *b == b'[')?;
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if *b == b'\\' {
                escaped = true;
            } else if *b == b'"' {
                in_str = false;
            }
            continue;
        }
        match *b {
            b'"' => in_str = true,
            // 只数与自己同型的那对括号：对象里的数组、数组里的对象都由各自的配对负责，
            // 不会互相干扰（字符串里的括号已被上面的 in_str 挡掉）
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(raw[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落盘文档的拼法：`# 文件名` + 各条正文，条间空行
    #[test]
    fn compose_page_doc_puts_title_first_and_keeps_every_section() {
        let sections = vec![
            PageSection {
                title: "适用范围".into(),
                content: "本制度适用于全体员工。".into(),
                tags: vec![],
                meta: Default::default(),
            },
            PageSection {
                title: "生效日期".into(),
                content: "自 2026 年 1 月 1 日起施行。".into(),
                tags: vec![],
                meta: Default::default(),
            },
        ];
        let doc = compose_page_doc("政府采购管理办法", &sections);
        assert_eq!(
            doc,
            "# 政府采购管理办法\n\n本制度适用于全体员工。\n\n自 2026 年 1 月 1 日起施行。\n"
        );
        // 每条正文都要能在文件里找到（否则"文件"和"条目"就是两套内容）
        for s in &sections {
            assert!(
                doc.contains(s.content.as_str()),
                "文件里找不到：{}",
                s.content
            );
        }
        // 没有文件名时不留一行空标题
        assert_eq!(
            compose_page_doc("  ", &sections[..1]),
            "本制度适用于全体员工。\n"
        );
        // 异常：有名字但一条正文都没有（`organize_page` 会先报错，这里只保证不崩）
        assert_eq!(compose_page_doc("标题", &[]), "# 标题\n");
    }

    /// 分组计划的解析与修复：模型漏块 / 重叠 / 越界都不能让用户内容凭空消失
    #[test]
    fn parse_plans_repairs_gaps_and_overlaps() {
        // 漏块：只给了 1-2 与 5-6 → 3、4 应并入前一组
        let raw = r#"{"chunks":[{"from":1,"to":2,"title":"A"},{"from":5,"to":6,"title":"B"}]}"#;
        let (title, plans) = parse_plans(raw, 6, "[]").unwrap();
        assert!(title.is_empty(), "没要整篇名字时不该凭空造一个");
        let spans: Vec<(usize, usize)> = plans.iter().map(|p| (p.from, p.to)).collect();
        assert_eq!(spans, vec![(1, 4), (5, 6)], "空隙应并入前一组");

        // 重叠：先到先得（(2,5) 被丢），剩余空隙补进前一组
        let raw = r#"{"chunks":[{"from":1,"to":3,"title":"A"},{"from":2,"to":5,"title":"B"},{"from":6,"to":6,"title":"C"}]}"#;
        let (_t, plans) = parse_plans(raw, 6, "[]").unwrap();
        let spans: Vec<(usize, usize)> = plans.iter().map(|p| (p.from, p.to)).collect();
        assert_eq!(spans, vec![(1, 5), (6, 6)]);

        // 全部非法（越界 + 倒序）→ 报错，调用方据此降级为规则分块
        assert!(parse_plans(r#"{"chunks":[{"from":9,"to":3}]}"#, 6, "[]").is_err());

        // 模型多写的 drop 要被**无视**：文件投喂是用户的原文，一个块都不能少
        let raw = r#"{"chunks":[{"from":1,"to":4,"title":"A"}],"drop":[2]}"#;
        let (_t, plans) = parse_plans(raw, 4, "[]").unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            (plans[0].from, plans[0].to),
            (1, 4),
            "drop 必须被忽略，不能把正文切掉"
        );

        // 「允许重命名」时顺带要的整篇名字：和分组共处同一个 JSON
        let raw = r#"{"docTitle":"政府采购管理办法","chunks":[{"from":1,"to":2,"title":"A"}]}"#;
        let (title, plans) = parse_plans(raw, 2, "[]").unwrap();
        assert_eq!(title, "政府采购管理办法");
        assert_eq!(plans.len(), 1);
    }

    /// 网页整理结果的解析：容忍两种形态；没有正文的条目一律丢掉（标题单独存在没有意义）
    #[test]
    fn parse_sections_extracts_doc_title_and_drops_empty_content() {
        let raw = r#"{"docTitle":"政府采购管理办法","sections":[
            {"title":"适用范围","content":"本制度适用于全体员工。","tags":["制度","适用范围"],"meta":{}},
            {"title":"只有标题"},
            {"title":"","content":"   "}
        ]}"#;
        let (doc_title, out) = parse_sections(raw, "[]").unwrap();
        assert_eq!(doc_title, "政府采购管理办法");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "适用范围");
        assert_eq!(out[0].content, "本制度适用于全体员工。");
        assert_eq!(out[0].tags, vec!["制度", "适用范围"]);

        // 也容忍模型把数组当根：此时没有 docTitle 可取，调用方回落到网页标题
        let raw = r#"[{"title":"A","content":"内容"}]"#;
        let (doc_title, out) = parse_sections(raw, "[]").unwrap();
        assert_eq!(doc_title, "");
        assert_eq!(out.len(), 1);

        // 一条可用条目都没有 → 报错，调用方降级为规则分块（投喂不能因此失败）
        assert!(parse_sections(r#"{"sections":[{"title":"只有标题"}]}"#, "[]").is_err());
    }

    #[test]
    fn models_json_accepts_both_shapes() {
        assert_eq!(
            models_from_json(r#"[{"name":"gpt-4o","note":"a"}]"#),
            vec!["gpt-4o"]
        );
        assert_eq!(models_from_json(r#"["a","b"]"#), vec!["a", "b"]);
        assert!(models_from_json("not json").is_empty());
    }

    #[test]
    fn draft_parsing_tolerates_fences_and_prose() {
        let raw = "好的，这是结果：\n```json\n{\"title\":\"标题\",\"content\":\"正文内容\",\"tags\":[\"a\",\"\"],\"meta\":{\"module\":\"后端\"}}\n```\n希望有帮助";
        let d = parse_draft(raw).unwrap();
        assert_eq!(d.title, "标题");
        assert_eq!(d.content, "正文内容");
        assert_eq!(d.tags, vec!["a"]);
        assert_eq!(d.meta.get("module").and_then(|v| v.as_str()), Some("后端"));

        assert!(parse_draft("没有 JSON").is_err());
        assert!(parse_draft(r#"{"title":"只有标题"}"#).is_err());
    }

    #[test]
    fn json_object_extraction_handles_nested_braces_in_strings() {
        let raw = r#"prefix {"a":"含 } 和 { 的字符串","b":{"c":1}} suffix"#;
        let got = extract_json(raw).unwrap();
        assert_eq!(got, r#"{"a":"含 } 和 { 的字符串","b":{"c":1}}"#);
    }

    #[test]
    fn enrich_parses_tags_and_sanitizes_meta() {
        let fields = r#"[
            {"key":"module","type":"select","options":["前端","后端"]},
            {"key":"lang","type":"text"},
            {"key":"stars","type":"number"},
            {"key":"verified","type":"bool"},
            {"key":"reviewed","type":"date"}
        ]"#;
        // 标签：重复项去重、`{name:...}` 形态兼容、超长丢弃
        let raw = r#"```json
        {"title":"  知识标题  ",
         "tags":["auth","auth",{"name":"PilotDesk"},"这个标签实在是太长了已经超过二十四个字符的限制了吧"],
         "meta":{"module":"后端","lang":"Rust","stars":"42","verified":"是","reviewed":"2024年3月","unknown":"x"}}
        ```"#;
        let e = parse_enrich(raw).unwrap();
        assert_eq!(e.title, "知识标题", "标题应去掉首尾空白");
        assert_eq!(
            e.tags,
            vec!["auth", "PilotDesk"],
            "重复去重、对象形态兼容、超长丢弃"
        );

        let meta = sanitize_meta(fields, &e.meta);
        assert_eq!(
            meta.get("module").and_then(|v| v.as_str()),
            Some("后端"),
            "命中候选项保留"
        );
        assert_eq!(meta.get("lang").and_then(|v| v.as_str()), Some("Rust"));
        assert_eq!(
            meta.get("stars").and_then(|v| v.as_f64()),
            Some(42.0),
            "字符串数字转数字"
        );
        assert_eq!(
            meta.get("verified").and_then(|v| v.as_bool()),
            Some(true),
            "「是」转 true"
        );
        assert!(
            meta.get("reviewed").is_none(),
            "非 YYYY-MM-DD 的日期宁缺勿滥"
        );
        assert!(meta.get("unknown").is_none(), "自创字段名必须丢弃");

        // 单选越界值、空值、超长文本
        let bad = serde_json::json!({"module":"运维","lang":"","reviewed":"2024-03-05"});
        let meta2 = sanitize_meta(fields, bad.as_object().unwrap());
        assert!(
            meta2.get("module").is_none(),
            "单选越界值应当丢弃而不是入库"
        );
        assert!(meta2.get("lang").is_none(), "空字符串不写");
        assert_eq!(
            meta2.get("reviewed").and_then(|v| v.as_str()),
            Some("2024-03-05")
        );
    }

    #[test]
    fn enrich_parse_requires_json_object() {
        assert!(parse_enrich("模型什么都没给").is_err());
        // 只有 tags 没有 title 也算成功（整理是增益，不该整条失败）
        let ok = parse_enrich(r#"{"tags":["a"]}"#).unwrap();
        assert!(ok.title.is_empty());
        assert_eq!(ok.tags, vec!["a"]);
    }

    /// 对话沉淀：一段对话 → 多条候选；**空结果是合法的**（闲聊没有可沉淀的知识）；
    /// 每条都要带原样摘录的 sourceQuote（可溯源），并丢掉没有正文的条目。
    #[test]
    fn parse_digest_yields_many_and_allows_empty() {
        let raw = r#"```json
        {"notes":[
          {"title":"报销额度","content":"单笔 5000 元以内由直属领导审批。","tags":["报销","审批"],
           "meta":{"module":"后端"},"sourceQuote":"我们公司 5000 元以内直属领导批"},
          {"title":"只有标题"},
          {"title":"","content":"   "}
        ]}
        ```"#;
        let fields = r#"[{"key":"module","type":"select","options":["前端","后端"]}]"#;
        let out = parse_digest(raw, fields).unwrap();
        assert_eq!(out.len(), 1, "没有正文的条目要丢掉");
        assert_eq!(out[0].title, "报销额度");
        assert_eq!(out[0].tags, vec!["报销", "审批"]);
        assert_eq!(out[0].source_quote, "我们公司 5000 元以内直属领导批");
        assert_eq!(
            out[0].meta.get("module").and_then(|v| v.as_str()),
            Some("后端")
        );

        // 闲聊 → 0 条是**成功**，不是失败：调用方据此提示"没有可沉淀的知识"
        assert!(parse_digest(r#"{"notes":[]}"#, "[]").unwrap().is_empty());
        // 模型把数组当根也容忍
        assert_eq!(
            parse_digest(r#"[{"title":"A","content":"内容"}]"#, "[]")
                .unwrap()
                .len(),
            1
        );
        // 连 JSON 都没有才是失败（调用方重试一次）
        assert!(parse_digest("模型什么都没给", "[]").is_err());
    }

    /// 超长对话的截断判据：**恰好等于上限不算截断**（边界不能提前一格），
    /// 而且按**字符**而不是字节算（中文一个字 3 字节，按字节算会误判成超长）。
    #[test]
    fn conversation_truncation_uses_chars_and_includes_the_limit() {
        let ok = "中".repeat(DIGEST_MAX_CHARS);
        assert!(!conversation_is_truncated(&ok), "恰好到上限不该算截断");
        let over = "中".repeat(DIGEST_MAX_CHARS + 1);
        assert!(conversation_is_truncated(&over));
        assert!(!conversation_is_truncated(""));
    }
}
