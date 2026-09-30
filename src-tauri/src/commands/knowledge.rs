//! 知识库命令（前端 /knowledge 页面的数据入口）
//!
//! 所有命令都现场打开 MEMORY.db（与 commands/memory.rs 同一套做法），
//! 领域逻辑在 api_agent::knowledge 里，这里只做"定位配置目录 + 转发"。

use crate::api_agent::cloud_doc;
use crate::api_agent::knowledge::{
    assemble_plans, cap_chunks, derive_title, display_entry_title, display_file_name,
    merge_small_blocks, prepare_cloud, prepare_file, prepare_web, web_file_name, DeleteBaseOutcome,
    IngestOutcome, IngestPrep, KnowledgeBaseView, KnowledgeCandidateView, KnowledgeEntryView,
    KnowledgeFileAttrView, KnowledgeFileRelationView, KnowledgeFileView, KnowledgeStore,
    PlannedChunk, RemoveFilesOutcome, UnlinkEntriesOutcome, MIN_MODEL_BLOCK,
};
use crate::api_agent::system_prompt::get_pilotdesk_config_dir;
use crate::utils::errors::AppError;
use crate::utils::paths;
use crate::DbState;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::State;

/// 网页抓取上限（字节）：超过就不抓了，避免一个页面把内存和库都撑满
const MAX_FETCH_BYTES: usize = 5 * 1024 * 1024;

fn store() -> Result<KnowledgeStore, AppError> {
    let dir = get_pilotdesk_config_dir()
        .ok_or_else(|| AppError::Config("无法确定配置目录".to_string()))?;
    KnowledgeStore::open(&dir).map_err(AppError::Db)
}

/// 知识库文件根目录：由主库的 app_settings 决定（`knowledge-root` → 全局工作区/Knowledge → 应用根目录）
fn knowledge_root(state: &State<'_, DbState>) -> Result<PathBuf, AppError> {
    let conn = state.pool.get().map_err(AppError::from)?;
    Ok(paths::resolve_knowledge_root(&conn))
}

/// 投喂前的目录准备（根 + files/ + 标记文件）
fn prepared_root(state: &State<'_, DbState>) -> Result<PathBuf, AppError> {
    let root = knowledge_root(state)?;
    paths::ensure_knowledge_dirs(&root).map_err(AppError::Io)?;
    Ok(root)
}

/* ── 库定义 ── */

#[tauri::command]
pub fn kb_list_bases() -> Result<Vec<KnowledgeBaseView>, String> {
    Ok(store()?.list_bases())
}

#[tauri::command]
pub fn kb_create_base(
    id: String,
    name: String,
    description: String,
    field_schema: String,
) -> Result<KnowledgeBaseView, String> {
    store()?.create_base(&id, &name, &description, &field_schema)
}

#[tauri::command]
pub fn kb_update_base(
    id: String,
    name: String,
    description: String,
    field_schema: String,
) -> Result<(), String> {
    store()?.update_base(&id, &name, &description, &field_schema)
}

#[tauri::command]
pub fn kb_delete_base(state: State<'_, DbState>, id: String) -> Result<DeleteBaseOutcome, String> {
    // 需要 root：孤儿文件的磁盘原文要跟着登记行一起删
    let root = knowledge_root(&state)?;
    store()?.delete_base(&id, &root)
}

/* ── 条目 ── */

#[tauri::command]
pub fn kb_list_entries(
    kb_id: String,
    query: Option<String>,
    origin: Option<String>,
    pin_only: Option<bool>,
    meta_filter: Option<String>,
) -> Result<Vec<KnowledgeEntryView>, String> {
    Ok(store()?.list_entries(
        &kb_id,
        query.as_deref(),
        origin.as_deref(),
        pin_only.unwrap_or(false),
        meta_filter.as_deref(),
    ))
}

/// 图谱「展开某个文件」用：取该原文的分块。
///
/// 不走 `kb_list_entries` 的原因：那个有整库 500 条的闸门，一个上千块的文件拿不全。
/// **默认不设上限**：要不要展开一个文件，由前端按整库规模自己决定（小库默认全展开），
/// 服务端不做截断 —— 截断会让人以为"这个文件就这么多块"。
#[tauri::command]
pub fn kb_list_file_chunks(
    kb_id: String,
    source_ref: String,
    limit: Option<usize>,
) -> Result<Vec<KnowledgeEntryView>, String> {
    Ok(store()?.list_file_chunks(&kb_id, &source_ref, limit))
}

/// 文件级专属属性（由该文件所有分块的 `kb_meta` 聚合而来）。
///
/// 文件视图「按属性分目录」用：`kb_files` 上没有属性列，属性只在分块关联行上，
/// 所以"这个文件属于哪个目录"只能由分块聚合得到（一致才有值，不一致进 `conflicts`）。
#[tauri::command]
pub fn kb_list_file_attrs(kb_id: String) -> Result<Vec<KnowledgeFileAttrView>, String> {
    Ok(store()?.file_attr_map(&kb_id))
}

#[tauri::command]
pub fn kb_save_entry(
    kb_id: String,
    key: String,
    value: String,
    tags: Option<String>,
    origin: Option<String>,
    source_ref: Option<String>,
    meta_json: Option<String>,
) -> Result<(), String> {
    store()?.save_entry(
        &kb_id,
        &key,
        &value,
        tags.as_deref().unwrap_or(""),
        origin.as_deref().unwrap_or(""),
        source_ref.as_deref().unwrap_or(""),
        meta_json.as_deref(),
    )
}

/// 解除关联；返回是否连带删除了条目本体（不再属于任何库时）
#[tauri::command]
pub fn kb_unlink_entry(kb_id: String, key: String) -> Result<bool, String> {
    store()?.unlink_entry(&kb_id, &key)
}

/// 批量解除关联（列表多选后一次处理）；语义与单条完全一致，只是放在一个事务里
#[tauri::command]
pub fn kb_unlink_entries(kb_id: String, keys: Vec<String>) -> Result<UnlinkEntriesOutcome, String> {
    store()?.unlink_entries(&kb_id, &keys)
}

/* ── 文件知识 ── */

#[tauri::command]
pub fn kb_list_files(
    state: State<'_, DbState>,
    kb_id: String,
) -> Result<Vec<KnowledgeFileView>, String> {
    let root = knowledge_root(&state)?;
    Ok(store()?.list_files(&root, &kb_id))
}

/// 本库文件之间的关系（正文—附件 / 新旧版本）
#[tauri::command]
pub fn kb_list_file_relations(kb_id: String) -> Result<Vec<KnowledgeFileRelationView>, String> {
    Ok(store()?.list_file_relations(&kb_id))
}

/// 建立文件关系：`kind` = attachment（from=正文, to=附件）/ supersedes（from=新版, to=旧版）
#[tauri::command]
pub fn kb_link_files(kind: String, from: String, to: String) -> Result<(), String> {
    store()?.link_files(&kind, &from, &to)
}

/// 解除文件关系；返回是否命中
#[tauri::command]
pub fn kb_unlink_files(kind: String, from: String, to: String) -> Result<bool, String> {
    store()?.unlink_files(&kind, &from, &to)
}

/// 从库中移除文件：**只解关联，不碰库定义**（想删文件就别去点「删除知识库」）
#[tauri::command]
pub fn kb_remove_files(
    state: State<'_, DbState>,
    kb_id: String,
    file_ids: Vec<String>,
) -> Result<RemoveFilesOutcome, String> {
    let root = knowledge_root(&state)?;
    store()?.remove_files(&kb_id, &file_ids, &root)
}

/// 投喂本地文件：**本地准备 → AI 分组（不持锁）→ 落库**
///
/// 三段分开是必须的：AI 调用要 await，而连接池 guard / 存储锁都不能跨 await；
/// AI 失败（无模型/超时/输出不合规）时走规则分块 —— 投喂不会失败。
///
/// 两个可选开关（界面上是两个勾选，**默认都不勾**）：
///   - `as_markdown`「存为 markdown 格式」：内容由 AI 整理、落盘为 `.md`，**不再与源文件逐字一致**；
///   - `rename`「允许重命名」：允许改变文件名 —— 行内填了名字就用它，留空则由 AI 取。
///     关掉时**一律保留原文件名**（内容与格式也都不动），用于"公文/制度的内容一个字不能改、
///     但文件名错了（`s(1).htm`）"这种场景。
#[tauri::command]
pub async fn kb_ingest_file(
    state: State<'_, DbState>,
    kb_id: String,
    path: String,
    as_markdown: Option<bool>,
    rename: Option<bool>,
    name: Option<String>,
) -> Result<IngestOutcome, String> {
    let as_markdown = as_markdown.unwrap_or(false);
    let rename = rename.unwrap_or(false);
    let source = Path::new(&path);
    let original = source
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let root = prepared_root(&state)?;
    let mut prep = prepare_file(source, as_markdown)?;
    // 用户填的名字（关掉开关时一律忽略 —— 界面上输入框此时是禁用的，不会出现"填了却没生效"）
    let typed = if rename {
        name.as_deref().map(str::trim).filter(|s| !s.is_empty())
    } else {
        None
    };
    // AI 只在"允许重命名、且用户自己没填"时才取名：用户意图优先
    let want_ai_name = rename && typed.is_none();
    let (plans, truncated, note) = if as_markdown {
        match organize_document(&state, &kb_id, &prep, file_stem(&original), "").await {
            Ok(w) => {
                // 整理成功 → 产物是 markdown，名字统一用 `.md`
                let base = typed.unwrap_or(w.doc_title.trim());
                if !base.is_empty() {
                    prep.name = web_file_name(strip_doc_ext(base));
                }
                prep.mime = "text/markdown".into();
                prep.write_bytes = w.doc.into_bytes();
                (Some(w.chunks), w.truncated, String::new())
            }
            // 「存为 markdown」是用户勾出来的**明确加工意图**，做不到就不能悄悄存原样：
            // 他要的是整理过的 .md，给他一份原始 HTML 属于"开关说谎"，而且库里还多一份噪声文件。
            // 直接失败并说明退路，比"看着像成功"有用 —— 这条踩过一次（用户反馈"勾了没生效"）。
            Err(e) => {
                return Err(AppError::External(format!(
                    "「存为 markdown 格式」未生效，本次没有入库：{}。可稍后重试，或取消勾选后按原文件投喂。",
                    e
                ))
                .into());
            }
        }
    } else {
        match plan_ingest(&state, &kb_id, &prep, &original, want_ai_name).await {
            Ok((title, chunks, truncated)) => {
                // 内容与格式都没变 → **保留原扩展名**（`制度.htm` 里装的仍是 HTML，
                // 命名成 `.md` 只是让文件名说谎）
                let base = typed.unwrap_or(title.trim());
                let renamed = !base.is_empty();
                if renamed {
                    prep.name = with_original_ext(base, &original);
                }
                // 勾了「允许重命名」又留空名字 = 交给 AI 取名。AI 没给名字时名字不变，
                // 这种事必须说出来：否则又是一次"勾了没生效"的静默失败。
                let note = if want_ai_name && !renamed {
                    "AI 没能给出文件名，已保留原文件名".to_string()
                } else {
                    String::new()
                };
                (Some(chunks), truncated, note)
            }
            Err(e) => {
                if let Some(t) = typed {
                    prep.name = with_original_ext(t, &original);
                }
                (None, false, e.to_string())
            }
        }
    };
    store()?.ingest_prepared(&root, &kb_id, prep, "", "file", plans, truncated, &note)
}

/// 文件名去扩展名（给模型当"这是篇什么"的线索；不用于落盘）
fn file_stem(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// 只改名字、**保留原扩展名**。
///
/// 内容与格式都没变时，扩展名跟着改会让文件名说谎（`制度.htm` 里装的还是 HTML）。
/// 用户自己写了扩展名也一律以原扩展名为准 —— 输入框右侧会把后缀固定显示出来，
/// 所以这不是"吃掉用户输入"，而是界面上说好的口径；只在它与原扩展名相同时才不重复追加。
fn with_original_ext(name: &str, original: &str) -> String {
    let stem = paths::sanitize_dir_name(name, "file");
    match original
        .rsplit_once('.')
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty())
    {
        Some(ext)
            if !stem
                .to_lowercase()
                .ends_with(&format!(".{}", ext.to_lowercase())) =>
        {
            format!("{}.{}", stem, ext)
        }
        _ => stem,
    }
}

/// AI 分组（文件投喂）：模型只定边界，正文按原文区间切片、一字不改。
/// `Err` 是"未生效的原因"，由调用方降级为规则分块并如实回传。
///
/// 模型与字段定义必须在 await **之前**取好；拿不到模型、调用失败、输出不合规都算未生效 ——
/// 用户的原文永远要能落库，AI 只是增益。
async fn plan_ingest(
    state: &State<'_, DbState>,
    kb_id: &str,
    prep: &IngestPrep,
    hint: &str,
    want_title: bool,
) -> Result<(String, Vec<PlannedChunk>, bool), AppError> {
    if prep.blocks.is_empty() {
        return Ok((String::new(), Vec::new(), false)); // 没抽出正文（格式不支持等）：无需 AI，也不是失败
    }
    let (model, base_name, fields_json) = enrich_context(state, kb_id)?;
    // 喂模型的是**合并过碎块**的那一份；切正文也按它算序号，两边必须是同一份（见 assemble_plans）
    let blocks = merge_small_blocks(&prep.text, &prep.blocks, MIN_MODEL_BLOCK);
    let block_texts: Vec<String> = blocks.iter().map(|b| b.text.clone()).collect();
    let planned = crate::api_agent::kb_llm::plan_chunks(
        &model,
        &block_texts,
        &base_name,
        &fields_json,
        hint,
        want_title,
    )
    .await
    .map_err(AppError::External);
    // 成败都记账（失败的调用也可能已计费）
    record_kb_usage(state, kb_id, &model);
    match planned {
        Ok((doc_title, plans)) => {
            let (chunks, truncated) = assemble_plans(prep, &blocks, &plans);
            log::info!(
                "[KB] AI 分组完成：{} 个块（合并前 {}）→ {} 条知识（{}）",
                blocks.len(),
                prep.blocks.len(),
                chunks.len(),
                prep.name
            );
            Ok((doc_title, chunks, truncated))
        }
        Err(e) => {
            log::warn!("[KB] AI 分组未生效，降级为规则分块（{}）：{}", prep.name, e);
            Err(e)
        }
    }
}

/// 投喂网页：抓取 → 本地粗清洗 → **AI 整理成规范知识** → 落库。
///
/// 与文件投喂的关键区别是这里**允许 AI 改写正文**。理由有两条：
///   ① 网页内容本身就是非标准化的，它只是**参考源**，不是用户的原文；
///   ② 抓下来的文本混着大量噪声（导航 / 页脚 / 广告 / 相关阅读 / 评论区），本就应该被清洗。
/// 只让模型划边界的话，出来的仍是一堆没条理的原始文本。
/// 底线与片段投喂一致：可以重排、分节、去噪、补标点，**不得增删或改动任何事实**。
///
/// `name` 是用户给这份原文起的名字；**留空则交给 AI 取名**（网页标题常常是站点名或
/// 「下载」「首页」这类空话，拿它当文件名，后期根本认不出这份文件知识）。
/// 只有 AI 也没给出名字时才回落到网页 `<title>`。
///
/// 抓取与 AI 都是网络等待，所以整体 async（期间不持有任何存储锁）。
#[tauri::command]
pub async fn kb_ingest_url(
    state: State<'_, DbState>,
    kb_id: String,
    url: String,
    name: Option<String>,
) -> Result<IngestOutcome, String> {
    let root = prepared_root(&state)?;
    let target = url.trim().to_string();
    if !(target.starts_with("http://") || target.starts_with("https://")) {
        return Err(AppError::InvalidInput("只支持 http/https 地址".into()).into());
    }
    // 先普通请求；只拿到 JS 空壳/待验证页时自动改用内置浏览器渲染一次（见 `load_page`）
    let html = load_page(&target).await?;
    let page_title = extract_title(&html, &target);
    // 用户填了就用用户的（用户意图优先）；没填先挂网页标题占位，AI 给出名字后再换掉
    let user_name = name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| strip_doc_ext(s).to_string());
    // 本地把 HTML 转成 markdown：既剔掉脚本/样式/head 这些显然不是正文的部分，
    // 又把标题/列表/表格/链接保下来 —— 后续的分块与 AI 整理都依赖这些结构（见 html_to_markdown 的说明）
    let text = crate::tools::browser::html_to_markdown(&html);
    if text.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "页面没有可提取的正文（可能是纯 JS 渲染的站点）".into(),
        )
        .into());
    }
    // 落盘内容 = **AI 整理后的版本**（见下方 Ok 分支）：库里这份知识是「一份文档 + 它的 N 条知识」，
    // 产物就该是这份文档本身，而不是抓下来那坨带噪声的原始文本。
    // 注意 `prep.text` / `prep.blocks` 仍是**本地提取出的原始 markdown**（AI 的输入），
    // 与 `write_bytes` 刻意不同 —— 它们只用于整理这一阶段，落库阶段不读 `text`
    let mut prep = prepare_web(user_name.as_deref().unwrap_or(&page_title), &text)?;
    let (plans, truncated, note) =
        match organize_document(&state, &kb_id, &prep, &page_title, &target).await {
            Ok(w) => {
                // 用户没指定名字 → 用 AI 给整篇取的名字（`prep.sha` 取自原文，不受改名影响）
                if user_name.is_none() && !w.doc_title.trim().is_empty() {
                    prep.name = web_file_name(&w.doc_title);
                }
                prep.write_bytes = w.doc.into_bytes();
                (Some(w.chunks), w.truncated, String::new())
            }
            // AI 未生效：落盘退回本地提取的 markdown（干净但未整理），条目走规则分块
            Err(e) => (None, false, e.to_string()),
        };
    store()?.ingest_prepared(&root, &kb_id, prep, &target, "url", plans, truncated, &note)
}

/* ── 云文档源（第一期：语雀） ──
 *
 * 交互链路：设置页添加账号 → 投喂页「云文档」模式选账号 / 知识库 → 勾选文档 → 投喂。
 * 职责划分：**取内容**在 `api_agent::cloud_doc`，**落库**复用上面那条投喂链路
 * （`prepare_cloud` → AI 整理 → `ingest_prepared`），所以去重、分块、图谱、检索全都不用重写。
 *
 * **账号是复数**：可以同时配多个 —— 同一平台的两个语雀账号（个人 / 团队），或将来的多个平台，
 * 各是一条账号记录。所以下面的命令一律按 `account_id` 定位，不存在"当前那一个"。
 *
 * 凭证只在后端：账号列表与 Token 一起以密文落盘（`utils::crypto`，与 API 提供商同一套
 * AES-256-GCM + DPAPI 保护），前端只拿得到"有哪几个账号"，永远拿不到明文。
 */

/// 云文档账号整表（含 Token）以密文存在这一个键下
const CLOUD_ACCOUNTS_SETTING: &str = "kb_cloud_accounts";

/// 单账号时代的旧键：只在迁移时读一次（见 `migrate_legacy_account`），迁完即清空
const LEGACY_CLOUD_SOURCE_SETTING: &str = "kb_cloud_source";
const LEGACY_CLOUD_TOKEN_SETTING: &str = "kb_cloud_token";
const LEGACY_CLOUD_ACCOUNT_SETTING: &str = "kb_cloud_account";

/// 落盘的账号记录（**含明文 Token** —— 整个数组一起加密，不逐字段加密）
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccountStored {
    pub id: String,
    /// 平台标识（`cloud_doc::CloudSource::id`）
    pub source: String,
    /// 账号名：用户填的，或用平台账号名兜底，所以落盘必非空
    pub label: String,
    pub token: String,
}

/// 回给前端的账号视图：**不含 Token**
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccountView {
    pub id: String,
    pub source: String,
    pub label: String,
}

impl CloudAccountStored {
    fn view(&self) -> CloudAccountView {
        CloudAccountView {
            id: self.id.clone(),
            source: self.source.clone(),
            label: self.label.clone(),
        }
    }
}

fn app_setting_set(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<(), AppError> {
    crate::commands::app_settings::set_setting(conn, key, value)
}

/// 读出全部账号。
///
/// 密文解不开（换过机器、密钥文件被删）时按"没有账号"处理，而不是让整个设置页报错 ——
/// 用户此时的下一步动作本来就是"重新添加一遍"。
fn cloud_accounts_load(conn: &rusqlite::Connection) -> Result<Vec<CloudAccountStored>, AppError> {
    let Some(encrypted) = paths::get_app_setting_opt(conn, CLOUD_ACCOUNTS_SETTING) else {
        return migrate_legacy_account(conn);
    };
    let json = crate::utils::crypto::decrypt(&encrypted)
        .map_err(|e| AppError::Config(format!("读取云文档账号失败: {}", e)))?;
    Ok(serde_json::from_str(&json).unwrap_or_default())
}

fn cloud_accounts_save(
    conn: &rusqlite::Connection,
    accounts: &[CloudAccountStored],
) -> Result<(), AppError> {
    let json = serde_json::to_string(accounts)
        .map_err(|e| AppError::Json(format!("序列化云文档账号失败: {}", e)))?;
    app_setting_set(
        conn,
        CLOUD_ACCOUNTS_SETTING,
        &crate::utils::crypto::encrypt(&json).map_err(AppError::Config)?,
    )
}

fn cloud_account_find(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<CloudAccountStored, AppError> {
    cloud_accounts_load(conn)?
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| {
            AppError::NotFound(
                "这个云文档账号已不存在：请到「设置 › 知识库 › 云文档源」重新选择".to_string(),
            )
        })
}

/// 把单账号时代的旧配置转成一条账号记录。
///
/// 这个功能上线前只存过"一个平台 + 一个 Token"，老配置不该因为改成多账号就消失。
/// 迁完清空旧键：不清的话，用户删光账号后旧配置会自己"复活"。
fn migrate_legacy_account(
    conn: &rusqlite::Connection,
) -> Result<Vec<CloudAccountStored>, AppError> {
    let Some(encrypted) = paths::get_app_setting_opt(conn, LEGACY_CLOUD_TOKEN_SETTING) else {
        return Ok(Vec::new());
    };
    let source = paths::get_app_setting_opt(conn, LEGACY_CLOUD_SOURCE_SETTING)
        .unwrap_or_else(|| "yuque".to_string());
    let label = paths::get_app_setting_opt(conn, LEGACY_CLOUD_ACCOUNT_SETTING).unwrap_or_default();
    let token = crate::utils::crypto::decrypt(&encrypted)
        .map_err(|e| AppError::Config(format!("读取旧版云文档凭证失败: {}", e)))?;
    let accounts = vec![CloudAccountStored {
        id: uuid::Uuid::new_v4().to_string(),
        source,
        label: if label.trim().is_empty() {
            "已配置账号".to_string()
        } else {
            label
        },
        token,
    }];
    cloud_accounts_save(conn, &accounts)?;
    for key in [
        LEGACY_CLOUD_SOURCE_SETTING,
        LEGACY_CLOUD_TOKEN_SETTING,
        LEGACY_CLOUD_ACCOUNT_SETTING,
    ] {
        app_setting_set(conn, key, "")?;
    }
    log::info!("[KB] 旧版云文档源配置已迁移为账号记录");
    Ok(accounts)
}

/// 已配置的云文档账号列表（设置页与投喂页共用；**不含 Token**）
#[tauri::command]
pub fn kb_cloud_accounts_list(state: State<'_, DbState>) -> Result<Vec<CloudAccountView>, String> {
    let conn = state.pool.get().map_err(AppError::from)?;
    Ok(cloud_accounts_load(&conn)?
        .iter()
        .map(|a| a.view())
        .collect())
}

/// 添加一个云文档账号：**先验证再落盘**。
///
/// 验证不过就不写入 —— 否则列表里会出现一条"看着配好了、一拉就报错"的账号，
/// 而用户无法判断是 Token 填错了还是别的问题。
#[tauri::command]
pub async fn kb_cloud_account_add(
    state: State<'_, DbState>,
    source: String,
    label: Option<String>,
    token: String,
) -> Result<CloudAccountView, String> {
    let src = cloud_doc::CloudSource::parse(&source)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::InvalidInput("Token 不能为空".into()).into());
    }
    // 网络校验在 await 里做，**不能持有连接池 guard**
    let account = cloud_doc::verify(src, &token).await?;
    let label = label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or(account.display);

    let conn = state.pool.get().map_err(AppError::from)?;
    let mut accounts = cloud_accounts_load(&conn)?;
    // 同一个 Token 加两遍必然是误操作；同一个账号换不同 Token（不同权限范围）是有意为之，放行
    if accounts
        .iter()
        .any(|a| a.source == src.id() && a.token == token)
    {
        return Err(
            AppError::InvalidInput(format!("这个 {} 账号已经添加过了", src.label())).into(),
        );
    }
    let record = CloudAccountStored {
        id: uuid::Uuid::new_v4().to_string(),
        source: src.id().to_string(),
        label,
        token,
    };
    let view = record.view();
    accounts.push(record);
    cloud_accounts_save(&conn, &accounts)?;
    Ok(view)
}

/// 删除一个云文档账号
#[tauri::command]
pub fn kb_cloud_account_remove(state: State<'_, DbState>, id: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(AppError::from)?;
    let mut accounts = cloud_accounts_load(&conn)?;
    let before = accounts.len();
    accounts.retain(|a| a.id != id);
    if accounts.len() == before {
        return Err(AppError::NotFound("要删除的账号不存在（可能已经删掉了）".into()).into());
    }
    Ok(cloud_accounts_save(&conn, &accounts)?)
}

/// 取出账号的平台与 Token。连接池 guard 在函数内释放，**不跨 await**。
fn cloud_account_credentials(
    state: &State<'_, DbState>,
    account_id: &str,
) -> Result<(cloud_doc::CloudSource, String), AppError> {
    let conn = state.pool.get().map_err(AppError::from)?;
    let account = cloud_account_find(&conn, account_id)?;
    let source = cloud_doc::CloudSource::parse(&account.source).map_err(AppError::InvalidInput)?;
    Ok((source, account.token))
}

/// 列出某个账号可见的云文档知识库（个人 + 团队）
#[tauri::command]
pub async fn kb_cloud_list_repos(
    state: State<'_, DbState>,
    account_id: String,
) -> Result<Vec<cloud_doc::CloudRepo>, String> {
    let (source, token) = cloud_account_credentials(&state, &account_id)?;
    // login 现取、不落盘：旧版迁移来的账号没存 login；顺带也确认了这个 Token 现在仍然有效
    let account = cloud_doc::verify(source, &token).await?;
    cloud_doc::list_repos(source, &token, &account.login).await
}

/// 列出某个知识库下的文档（只返回元信息，正文在投喂时逐篇取）
#[tauri::command]
pub async fn kb_cloud_list_docs(
    state: State<'_, DbState>,
    account_id: String,
    namespace: String,
) -> Result<Vec<cloud_doc::CloudDoc>, String> {
    let (source, token) = cloud_account_credentials(&state, &account_id)?;
    cloud_doc::list_docs(source, &token, &namespace).await
}

/// 投喂一篇云文档：取正文 → AI 整理（失败则规则分块）→ 落库。
///
/// 与 `kb_ingest_url` 同一条下半程，差别只有"取内容"这一步（那里是抓网页，这里是调 API）。
/// 来源记 `cloud`：列表与图谱按来源筛选时，"云文档"要能与"网页投喂"分开。
#[tauri::command]
pub async fn kb_cloud_ingest_doc(
    state: State<'_, DbState>,
    kb_id: String,
    account_id: String,
    namespace: String,
    doc_id: String,
    name: Option<String>,
) -> Result<IngestOutcome, String> {
    let (source, token) = cloud_account_credentials(&state, &account_id)?;
    // 目录准备放在网络请求之前：失败早退时不会留下半成品目录
    let root = prepared_root(&state)?;
    let content = cloud_doc::fetch_doc(source, &token, &namespace, &doc_id).await?;
    // 用户填了名字就用用户的（与网页投喂同一口径：用户意图优先）
    let user_name = name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| strip_doc_ext(s).to_string());
    let title = user_name.clone().unwrap_or_else(|| content.title.clone());
    let mut prep = prepare_cloud(&title, &content.body)?;
    let (plans, truncated, note) =
        match organize_document(&state, &kb_id, &prep, &content.title, &content.url).await {
            Ok(w) => {
                if user_name.is_none() && !w.doc_title.trim().is_empty() {
                    prep.name = web_file_name(&w.doc_title);
                }
                prep.write_bytes = w.doc.into_bytes();
                (Some(w.chunks), w.truncated, String::new())
            }
            // AI 未生效：落盘退回原文正文（未整理），条目走规则分块
            Err(e) => (None, false, e.to_string()),
        };
    store()?.ingest_prepared(
        &root,
        &kb_id,
        prep,
        &content.url,
        "cloud",
        plans,
        truncated,
        &note,
    )
}

/// 去掉用户顺手打上的文档后缀（`制度.md` → `制度`）—— 落盘名由代码补后缀，
/// 不剥的话会变成 `制度.md.md`。
///
/// 用**显式清单**而不是"最后一段像扩展名就剥"：`报告 2024.01` 这种名字很常见，
/// 按"像扩展名"的规则会把 `.01` 当成后缀剥掉，等于改坏了用户给的名字。
fn strip_doc_ext(s: &str) -> &str {
    let lower = s.to_ascii_lowercase();
    for ext in [
        ".markdown",
        ".md",
        ".txt",
        ".html",
        ".htm",
        ".csv",
        ".json",
        ".log",
        ".pdf",
        ".doc",
        ".docx",
        ".xls",
        ".xlsx",
        ".ppt",
        ".pptx",
    ] {
        if lower.ends_with(ext) {
            return s[..s.len() - ext.len()].trim_end();
        }
    }
    s
}

/// 「整理成一份文档」的产物（网页投喂与文件的「存为 markdown 格式」共用）。
///
/// 用结构体而不是四元组：四个字段各有含义，返回 `(Vec, bool, String, String)` 那种签名
/// 在调用点看谁是谁全靠位置，改一次就得同时改几处。
struct OrganizedDoc {
    /// 要落库的各条知识（已被分块上限截断）
    chunks: Vec<PlannedChunk>,
    truncated: bool,
    /// 整篇的文件名（AI 取的）
    doc_title: String,
    /// **要落盘的那份文档** = AI 整理后的版本（见 `compose_page_doc`）
    doc: String,
}

/// 「整理成一份文档」：**允许 AI 改写正文**。网页投喂与文件的「存为 markdown 格式」共用这一条。
///
/// 为什么允许改写（与文件的默认路径相反）：
///   ① 网页内容本身就是非标准化的，它只是**参考源**，不是用户的原文；
///   ② 抓下来/另存下来的文本混着大量噪声（导航 / 页脚 / 广告 / 相关阅读 / 评论区），本就应该清洗；
///   ③ 用户勾了「存为 markdown 格式」，就是明确要"加工"，而不是原样保存。
/// 底线与片段投喂一致：可以重排、分节、去噪、补标点，**不得增删或改动任何事实**。
///
/// `page_title` 是给模型判断整篇讲什么的**线索**（网页标题 / 原文件名），不是最终名字。
/// `Err` 是"未生效的原因"，由调用方降级为规则分块并如实回传。
async fn organize_document(
    state: &State<'_, DbState>,
    kb_id: &str,
    prep: &IngestPrep,
    page_title: &str,
    hint: &str,
) -> Result<OrganizedDoc, AppError> {
    if prep.blocks.is_empty() {
        return Err(AppError::InvalidInput("没有可整理的正文".into()));
    }
    let (model, base_name, fields_json) = enrich_context(state, kb_id)?;
    // 与文件投喂同一条口径：碎块先合并再喂模型，否则块数一多就整篇降级
    let blocks = merge_small_blocks(&prep.text, &prep.blocks, MIN_MODEL_BLOCK);
    let block_texts: Vec<String> = blocks.iter().map(|b| b.text.clone()).collect();
    let organized = crate::api_agent::kb_llm::organize_page(
        &model,
        &block_texts,
        page_title,
        &base_name,
        &fields_json,
        hint,
    )
    .await
    .map_err(AppError::External);
    // 成败都记账（失败的调用也可能已计费）
    record_kb_usage(state, kb_id, &model);
    let (doc_title, sections) = organized?;
    log::info!(
        "[KB] AI 整理完成：{} 个块 → {} 条知识（文件名为「{}」，线索「{}」）",
        blocks.len(),
        sections.len(),
        doc_title,
        page_title
    );
    // 落盘文档按**全部** section 拼（在截断之前）：截断是"条目太多画不下"的取舍，
    // 不该把原文也一起砍掉 —— 文件比条目多出一些内容，总好过内容凭空消失
    let doc = crate::api_agent::kb_llm::compose_page_doc(&doc_title, &sections);
    let chunks: Vec<PlannedChunk> = sections
        .into_iter()
        .map(|s| PlannedChunk {
            text: s.content,
            title: s.title,
            tags: s.tags.join(","),
            meta_json: serde_json::to_string(&s.meta).unwrap_or_else(|_| "{}".into()),
        })
        .collect();
    let (chunks, truncated) = cap_chunks(chunks);
    Ok(OrganizedDoc {
        chunks,
        truncated,
        doc_title,
        doc,
    })
}

/// 正文短到不像一篇文章的阈值：验证页、JS 空壳页都落在这一档（正常文章远不止 300 字）
const MIN_ARTICLE_CHARS: usize = 300;

/// **需要人机交互**的验证页特征：渲染也过不去（滑块/验证码得人来点），
/// 命中就直接报错，不再白启动一次浏览器。
///
/// 为什么必须识别这类页面：它的状态码是 **200**、体积也不小，去标签后还能剩下几行文字
/// —— 实测百度百家号那页剩下的是
/// `百度安全验证 / 网络不给力，请稍后重试 / 返回首页 / 问题反馈`。
/// 于是它会一路通过"没有正文"的检查，最后在库里留下一个名为「百度安全验证」的文件、
/// 条目写着「网络不给力，请稍后重试」。**不识别，这个失败就是静默的**：
/// 用户看到投喂成功，只是内容完全没用。
const HARD_BLOCK_MARKERS: &[&str] = &[
    "安全验证",
    "人机验证",
    "验证码",
    "滑块",
    "拖动滑块",
    "请完成验证",
    "环境异常",
    "访问过于频繁",
    "访问频繁",
    "请求过于频繁",
    "access denied",
    "403 forbidden",
];

/// **可能靠渲染自动通过**的特征（Cloudflare 的 "Just a moment…" 就会自己过），
/// 所以值得用浏览器再试一次
const SOFT_BLOCK_MARKERS: &[&str] = &[
    "just a moment",
    "checking your browser",
    "verifying you are human",
    "正在验证",
    "请开启javascript",
    "enable javascript",
];

/// 抓回来的页面能不能用
enum PageVerdict {
    Ok,
    /// 人机验证/明确拦截（带命中的特征词）
    Blocked(&'static str),
    /// 可能在等待验证脚本自动放行
    Soft,
    /// 正文短得不像文章，也没有任何拦截特征 —— 典型的 JS 空壳
    Shell,
}

/// 判定页面可用性。特征词**只对短页面生效**：一篇讲"验证码"的文章也会命中这些词，
/// 但它不会是 300 字以下 —— 不加这条门槛就会误伤正常内容。
fn judge_page(title: &str, markdown: &str) -> PageVerdict {
    if markdown.chars().count() >= MIN_ARTICLE_CHARS {
        return PageVerdict::Ok;
    }
    let probe = format!(
        "{}\n{}",
        title,
        markdown.chars().take(600).collect::<String>()
    )
    .to_lowercase();
    if let Some(m) = HARD_BLOCK_MARKERS
        .iter()
        .copied()
        .find(|m| probe.contains(*m))
    {
        return PageVerdict::Blocked(m);
    }
    if SOFT_BLOCK_MARKERS.iter().any(|m| probe.contains(*m)) {
        return PageVerdict::Soft;
    }
    PageVerdict::Shell
}

/// 被人机验证挡住时的用户可见文案：**必须给出可执行的路**。
/// 用户在浏览器里明明看得到这篇内容，只回一句"抓取失败"没有意义。
fn block_message(marker: &str) -> String {
    format!(
        "该站点要求人机验证（命中「{}」），程序抓不到内容。请改用这两条路之一：\
         ① 在浏览器里打开该页 → 另存为 .html → 用「文件」投喂（能保留原文与文件名）；\
         ② 直接复制正文 → 用「片段」投喂（同样会由 AI 整理成规范知识）。",
        marker
    )
}

/// 抓页面：先普通 HTTP，**结果不可用时自动改用内置浏览器渲染一次**。
///
/// 调度规则（各自的理由写在上面两个常量的注释里）：
///   - 普通结果 `Ok`            → 直接用（绝大多数站点，零额外开销）
///   - 普通结果**硬拦截**        → 直接报错：渲染也过不去，省下一次浏览器启动
///   - 普通结果软拦截 / 空壳，或普通请求本身失败 → 用浏览器渲染重试
///   - 渲染后仍被硬拦            → 报明确错误（并指路）
async fn load_page(url: &str) -> Result<String, AppError> {
    let plain = fetch_page(url).await;
    match &plain {
        Ok((html, ctype)) => {
            if !ctype.contains("html") && !ctype.is_empty() {
                return Err(AppError::External(format!(
                    "该地址返回的是 {}，不是网页；请先下载文件再用「文件」投喂",
                    ctype
                )));
            }
            let md = crate::tools::browser::html_to_markdown(html);
            match judge_page(&extract_title(html, url), &md) {
                PageVerdict::Ok => return Ok(html.clone()),
                PageVerdict::Blocked(m) => {
                    log::warn!("[KB] 命中人机验证特征「{m}」，不再尝试渲染：{url}");
                    return Err(AppError::External(block_message(m)));
                }
                PageVerdict::Soft | PageVerdict::Shell => {
                    log::info!("[KB] 普通抓取只拿到空壳/待验证页，改用内置浏览器渲染：{url}");
                }
            }
        }
        Err(e) => log::info!("[KB] 普通抓取失败（{e}），改用内置浏览器渲染：{url}"),
    }

    match crate::tools::browser::fetch_rendered_html(url, MAX_FETCH_BYTES).await {
        Ok(html) => {
            let md = crate::tools::browser::html_to_markdown(&html);
            match judge_page(&extract_title(&html, url), &md) {
                PageVerdict::Blocked(m) => Err(AppError::External(block_message(m))),
                // Soft（可能仍在验证中）/ Shell（页面确实很短）都往下走：
                // 拿不到更多东西了，但总比直接报错强，交给 AI 与后续流程判断
                _ => Ok(html),
            }
        }
        Err(e) => {
            let head = match &plain {
                Err(pe) => format!("普通抓取：{pe}；"),
                Ok(_) => String::new(),
            };
            Err(AppError::External(format!(
                "{head}内置浏览器渲染也失败：{e}"
            )))
        }
    }
}

/// 抓取网页：带超时、大小上限与 UA（部分站点对默认 UA 直接 403）
async fn fetch_page(url: &str) -> Result<(String, String), AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Network(format!("创建请求客户端失败: {}", e)))?;
    let resp = client
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
        )
        .send()
        .await
        .map_err(|e| AppError::Network(format!("抓取失败: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Network(format!(
            "抓取失败: HTTP {}",
            resp.status().as_u16()
        )));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Network(format!("读取响应失败: {}", e)))?;
    if bytes.len() > MAX_FETCH_BYTES {
        return Err(AppError::InvalidInput(format!(
            "页面超过 {} MB，请改用片段投喂",
            MAX_FETCH_BYTES / 1024 / 1024
        )));
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), content_type))
}

/// 标题：优先 `<title>`，取不到就退回主机名（分块 key 与文件名都要用）
fn extract_title(html: &str, url: &str) -> String {
    if let Ok(sel) = scraper::Selector::parse("title") {
        let doc = scraper::Html::parse_document(html);
        if let Some(el) = doc.select(&sel).next() {
            let t = el.text().collect::<String>().trim().to_string();
            if !t.is_empty() {
                return t.chars().take(80).collect();
            }
        }
    }
    url.split("//")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .unwrap_or("webpage")
        .to_string()
}

/* ── 知识库模型（设置页展示与调试用） ── */

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbModelInfo {
    pub provider_name: String,
    pub model: String,
    /// true = 设置里没指定（或指定的不可用），当前用的是"第一个可用提供商"回落
    pub from_fallback: bool,
}

/// 当前实际生效的知识库模型（不含 api_key —— key 绝不出后端）。
/// 解析不出来返回 None（设置页只做展示，不当错误处理）。
#[tauri::command]
pub fn kb_effective_model(state: State<'_, DbState>) -> Result<Option<KbModelInfo>, String> {
    let conn = state.pool.get().map_err(AppError::from)?;
    Ok(crate::api_agent::kb_llm::resolve_kb_model(&conn)
        .ok()
        .map(|m| KbModelInfo {
            provider_name: m.provider_name,
            model: m.model,
            from_fallback: m.from_fallback,
        }))
}

/* ── 知识库根目录（设置页） ── */

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbRootInfo {
    pub root: String,
    /// true = 用户在设置里指定过；false = 走默认位置（全局工作区/Knowledge）
    pub is_custom: bool,
    pub file_count: usize,
    pub bytes: u64,
}

#[tauri::command]
pub fn kb_root_info(state: State<'_, DbState>) -> Result<KbRootInfo, String> {
    let conn = state.pool.get().map_err(AppError::from)?;
    let root = paths::resolve_knowledge_root(&conn);
    let is_custom = paths::get_app_setting_opt(&conn, "knowledge-root").is_some();
    drop(conn);
    let (file_count, bytes) = paths::count_knowledge_files(&root);
    Ok(KbRootInfo {
        root: root.to_string_lossy().into_owned(),
        is_custom,
        file_count,
        bytes,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbRootChangeOutcome {
    pub root: String,
    pub moved: usize,
    pub skipped: usize,
    /// 搬运失败的文件（非空时**设置不会改动**，旧根仍是完整可用的一套）
    pub failed: Vec<String>,
    /// 旧目录空壳是否已清理（仅清我们自己建的标记与空的 files/）
    pub cleaned: bool,
}

/// 更改知识库根目录。
///
/// - `root`：None / 空串 = 恢复默认（全局工作区/Knowledge）
/// - `migrate`：true = 先把原文搬过去，**全部成功才写设置**；有失败则保持旧根不动并返回失败清单（可重试，已搬的会因同名同 sha 被跳过）
///
/// DB 不需要任何改动：库里存的是相对路径，换根只是"同一套相对结构换个前缀"。
#[tauri::command]
pub fn kb_set_root(
    state: State<'_, DbState>,
    root: Option<String>,
    migrate: bool,
) -> Result<KbRootChangeOutcome, String> {
    let conn = state.pool.get().map_err(AppError::from)?;
    let old_root = paths::resolve_knowledge_root(&conn);
    let custom = root
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .map(|r| paths::resolve_tilde(&r));

    let new_root = match &custom {
        Some(r) => PathBuf::from(r),
        None => paths::resolve_workspace_path(None, paths::KNOWLEDGE_DIR_NAME, &conn),
    };

    let old_key = paths::path_key(&old_root);
    let new_key = paths::path_key(&new_root);
    let same = old_key == new_key;

    // 父子关系会让两边互相吞并（新根在旧根里 → 迁移时把目标算进源里），直接拒绝
    if !same
        && (new_key.starts_with(&format!("{}/", old_key))
            || old_key.starts_with(&format!("{}/", new_key)))
    {
        return Err(AppError::InvalidInput(
            "新目录不能是当前知识库目录的子目录或父目录，请另选一个位置".into(),
        )
        .into());
    }

    let write_setting = |value: &str| -> Result<(), String> {
        crate::commands::app_settings::set_setting(&conn, "knowledge-root", value)
            .map_err(|e| e.to_string())
    };
    // 空串 = 未指定（走默认回落位置）
    let setting_value = custom.clone().unwrap_or_default();

    // 同目录：只写设置（幂等）
    if same {
        write_setting(&setting_value)?;
        return Ok(KbRootChangeOutcome {
            root: new_root.to_string_lossy().into_owned(),
            moved: 0,
            skipped: 0,
            failed: Vec::new(),
            cleaned: false,
        });
    }

    if migrate {
        paths::ensure_knowledge_dirs(&new_root)?;
        let out = paths::migrate_knowledge_files(&old_root, &new_root)?;
        if !out.failed.is_empty() {
            // 有失败就不改设置：旧的一套仍然完整可用，用户修好权限/空间后再来一次即可
            return Ok(KbRootChangeOutcome {
                root: old_root.to_string_lossy().into_owned(),
                moved: out.moved,
                skipped: out.skipped,
                failed: out.failed,
                cleaned: false,
            });
        }
        paths::cleanup_old_knowledge_root(&old_root);
        write_setting(&setting_value)?;
        Ok(KbRootChangeOutcome {
            root: new_root.to_string_lossy().into_owned(),
            moved: out.moved,
            skipped: out.skipped,
            failed: Vec::new(),
            cleaned: true,
        })
    } else {
        // 仅切换：不动文件（旧原文留在原处，其"打开原文"会失效，但条目与检索不受影响）
        write_setting(&setting_value)?;
        paths::ensure_knowledge_dirs(&new_root)?;
        Ok(KbRootChangeOutcome {
            root: new_root.to_string_lossy().into_owned(),
            moved: 0,
            skipped: 0,
            failed: Vec::new(),
            cleaned: false,
        })
    }
}

/* ── 批量导出（分享 / 资产拷贝） ── */

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbExportOutcome {
    pub dest: String,
    pub exported: usize,
    /// 因重名而改了文件名（加 -1 / -2 后缀，不覆盖已有文件）
    pub renamed: usize,
    /// 库里登记了、但磁盘上找不到原文
    pub missing: usize,
    pub failed: Vec<String>,
}

/// 批量导出知识库原文：`as_zip` = 打包成一个 .zip，否则复制进目标目录。
///
/// `file_ids` 为空 = 导出该库全部文件。文件名保持原样，重名加后缀**不覆盖**已有文件 ——
/// 导出是"拿出去用"的动作，静默覆盖用户目录里的同名文件是不可接受的。
#[tauri::command]
pub fn kb_export_files(
    state: State<'_, DbState>,
    kb_id: String,
    file_ids: Vec<String>,
    dest: String,
    as_zip: bool,
) -> Result<KbExportOutcome, String> {
    let root = knowledge_root(&state)?;
    let all = store()?.list_files(&root, &kb_id);
    let picked: Vec<_> = if file_ids.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|f| file_ids.contains(&f.id))
            .collect()
    };
    if picked.is_empty() {
        return Err(AppError::InvalidInput("没有可导出的文件".into()).into());
    }
    let dest_path = PathBuf::from(dest.trim());
    if dest_path.as_os_str().is_empty() {
        return Err(AppError::InvalidInput("请选择导出位置".into()).into());
    }

    let mut out = KbExportOutcome {
        dest: dest_path.to_string_lossy().into_owned(),
        exported: 0,
        renamed: 0,
        missing: 0,
        failed: Vec::new(),
    };
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();

    if as_zip {
        let file = std::fs::File::create(&dest_path)
            .map_err(|e| AppError::Io(format!("创建压缩包失败: {}", e)))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for f in &picked {
            let src = PathBuf::from(&f.abs_path);
            if !src.exists() {
                out.missing += 1;
                continue;
            }
            let name = unique_export_name(&f.name, &mut used);
            if name != f.name {
                out.renamed += 1;
            }
            let result = std::fs::File::open(&src)
                .map_err(|e| e.to_string())
                .and_then(|mut input| {
                    zip.start_file(name.clone(), options)
                        .map_err(|e| e.to_string())
                        .and_then(|_| {
                            std::io::copy(&mut input, &mut zip)
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        })
                });
            match result {
                Ok(()) => out.exported += 1,
                Err(e) => out.failed.push(format!("{}: {}", name, e)),
            }
        }
        zip.finish()
            .map_err(|e| AppError::Io(format!("收尾压缩包失败: {}", e)))?;
    } else {
        std::fs::create_dir_all(&dest_path)
            .map_err(|e| AppError::Io(format!("创建目标目录失败: {}", e)))?;
        // 目标目录里已有的名字先占位，导出不覆盖它们
        if let Ok(entries) = std::fs::read_dir(&dest_path) {
            for e in entries.flatten() {
                used.insert(e.file_name().to_string_lossy().to_string());
            }
        }
        for f in &picked {
            let src = PathBuf::from(&f.abs_path);
            if !src.exists() {
                out.missing += 1;
                continue;
            }
            let name = unique_export_name(&f.name, &mut used);
            if name != f.name {
                out.renamed += 1;
            }
            match std::fs::copy(&src, dest_path.join(&name)) {
                Ok(_) => out.exported += 1,
                Err(e) => out.failed.push(format!("{}: {}", name, e)),
            }
        }
    }
    Ok(out)
}

/// 导出文件名去重：`a.md` → `a-1.md`（用同一个 used 集合，目录模式与 zip 模式共用）
fn unique_export_name(name: &str, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{}", e)),
        None => (name.to_string(), String::new()),
    };
    for i in 1..10_000 {
        let candidate = format!("{}-{}{}", stem, i, ext);
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    format!("{}-{}", stem, uuid::Uuid::new_v4())
}

/* ── 条目级导出（片段 / AI 生成 / 工作沉淀唯一的交付出口） ── */

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbEntryExportOutcome {
    pub dest: String,
    pub exported: usize,
    /// 库里找不到的 key（条目可能已被删除 / 解除关联）
    pub missing: Vec<String>,
}

/// 把选中的知识条目导出成**一个 markdown 文件**（`dest` = 完整文件路径，已由前端 `saveDialog` 选好）。
///
/// 为什么需要这条出口：片段投喂 / AI 生成 / 工作沉淀这三类知识**没有原文文件**，
/// 「文件」页的批量导出（`kb_export_files`）够不到它们 —— 少了这条，它们就只能在库里看，
/// 拿不出去用（贴进文档、发给同事）。
///
/// 顺序按调用方给的 `keys` 走（= 界面上的顺序），不从库里重排：导出是"把眼前这批拿走"，
/// 顺序漂移会让人对不上。文件名与标题一律过 `display_entry_title`（剥掉内部指纹）。
#[tauri::command]
pub fn kb_export_entries(
    kb_id: String,
    keys: Vec<String>,
    dest: String,
) -> Result<KbEntryExportOutcome, String> {
    let dest = dest.trim().to_string();
    if dest.is_empty() {
        return Err(AppError::InvalidInput("请选择导出位置".into()).into());
    }
    let store = store()?;
    let base_name = store
        .list_bases()
        .into_iter()
        .find(|b| b.id == kb_id)
        .map(|b| b.name)
        .unwrap_or_else(|| "知识库".to_string());

    let all = store.list_entries(&kb_id, None, None, false, None);
    let mut by_key: std::collections::HashMap<&str, &KnowledgeEntryView> =
        all.iter().map(|e| (e.key.as_str(), e)).collect();

    let mut missing: Vec<String> = Vec::new();
    let mut body: Vec<String> = Vec::new();
    for key in &keys {
        match by_key.remove(key.as_str()) {
            Some(e) => body.push(entry_markdown(e)),
            None => missing.push(key.clone()),
        }
    }
    if body.is_empty() {
        return Err(
            AppError::InvalidInput("没有可导出的条目（选中的条目可能已被删除）".into()).into(),
        );
    }

    let doc = format!(
        "# {} · 知识导出\n\n> 共 {} 条\n\n{}",
        base_name,
        body.len(),
        body.join("\n\n---\n\n")
    );
    std::fs::write(&dest, doc).map_err(|e| AppError::Io(format!("写入失败: {}", e)))?;
    Ok(KbEntryExportOutcome {
        dest,
        exported: body.len(),
        missing,
    })
}

/// 一条知识 → markdown 段（标题 + 正文 + 专属属性 + 标签 + 来源），与前端「复制」同一口径。
fn entry_markdown(e: &KnowledgeEntryView) -> String {
    let mut parts = vec![
        format!("## {}", display_entry_title(&e.key)),
        String::new(),
        e.value.trim().to_string(),
    ];
    // 专属属性是判断"哪一版、是否现行"的依据，导出时不能丢（公文/制度场景）
    let attrs = crate::api_agent::db::meta_compact(&e.meta_json);
    if !attrs.is_empty() {
        parts.push(String::new());
        parts.push(format!("- 属性：{}", attrs));
    }
    let tags: Vec<&str> = e
        .tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if !tags.is_empty() {
        parts.push(String::new());
        parts.push(
            tags.iter()
                .map(|t| format!("#{}", t))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if !e.source_ref.trim().is_empty() {
        let place = match (e.chunk_index, e.chunk_total) {
            (Some(i), Some(n)) => format!(" · 第 {}/{} 块", i, n),
            _ => String::new(),
        };
        parts.push(String::new());
        parts.push(format!(
            "> 来源：{}{}",
            display_file_name(&e.source_ref),
            place
        ));
    }
    parts.join("\n")
}

/* ── 待确认队列 ── */

#[tauri::command]
pub fn kb_list_candidates(kb_id: String) -> Result<Vec<KnowledgeCandidateView>, String> {
    Ok(store()?.list_candidates(&kb_id))
}

/* ── 对话沉淀（一段带角色的对话 → 0..N 条候选） ── */

/// 对话沉淀的输入消息（前端选中的消息区间，**带角色**）
#[derive(Debug, Clone, serde::Deserialize)]
pub struct KbDigestMessage {
    /// "user" | "assistant"（其余原样保留为角色名）
    pub role: String,
    pub content: String,
}

/// 把选中消息拼成给模型的对话文本。**必须带角色**：模型要能分清"用户说的"与"助手建议的"，
/// 否则会把助手的建议当成用户确认的事实。
fn format_conversation(messages: &[KbDigestMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        let who = match m.role.as_str() {
            "user" => "用户",
            "assistant" | "ai" => "助手",
            "system" => "系统",
            other => other,
        };
        let text = m.content.trim();
        if text.is_empty() {
            continue;
        }
        out.push_str(&format!("{}：{}\n\n", who, text));
    }
    out.trim_end().to_string()
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbDigestOutcome {
    /// 产出的候选条数（**0 是合法结果**：这段对话没有可沉淀的知识）
    pub count: usize,
    pub ids: Vec<i64>,
    /// 对话过长被截断（只整理了前 `DIGEST_MAX_CHARS` 字）—— UI 要说出来，
    /// 否则用户以为整段都整理了（群聊一次勾很多条长发言时会发生）
    pub truncated: bool,
}

/// 把一段**带角色的对话**整理成若干条待确认候选（origin=work）。
///
/// 与「存为知识」（`kb_add_candidate` 直接落原文）的区别：那边存的是**对话原文**，噪声大；
/// 这里是**把对话当素材交给 AI 结构化整理** —— 一段对话常含多个主题，产出 0..N 条干净候选，
/// 每条带标题/正文/标签/专属属性与**原文摘录**（可溯源），用户逐条核对后采纳。
///
/// `kb_id` 是**单库**：专属属性字段定义随库而异，一次整理只能对着一套字段填；
/// 要存进别的库就再选一次（与 `save_knowledge` 的"目标库必填、不猜"一致）。
/// 模型不可用/输出不合规 → 报错，**不静默降级为落原始对话** —— 那正是「存为知识」已经能做的事，
/// 降级只会把一次结构化沉淀变成一次噪声入库。
#[tauri::command]
pub async fn kb_digest_conversation(
    state: State<'_, DbState>,
    kb_id: String,
    messages: Vec<KbDigestMessage>,
) -> Result<KbDigestOutcome, String> {
    let conversation = format_conversation(&messages);
    if conversation.is_empty() {
        return Err(AppError::InvalidInput("选中的消息没有可用内容".into()).into());
    }
    // 与 `kb_llm::digest_conversation` 内部同一判据（同一个函数）：用来回报用户"只整理了前一部分"
    let truncated = crate::api_agent::kb_llm::conversation_is_truncated(&conversation);
    let (model, base_name, fields_json) = enrich_context(&state, &kb_id)?;
    let digested = crate::api_agent::kb_llm::digest_conversation(
        &model,
        &conversation,
        &base_name,
        &fields_json,
    )
    .await;
    // 成败都记账（失败的调用也可能已计费）
    record_kb_usage(&state, &kb_id, &model);
    let notes = digested?;
    let store = store()?;
    let mut ids: Vec<i64> = Vec::new();
    for n in notes {
        let key = if n.title.trim().is_empty() {
            derive_title(&n.content)
        } else {
            n.title.clone()
        };
        if key.trim().is_empty() {
            continue;
        }
        let meta_json = serde_json::to_string(&n.meta).unwrap_or_else(|_| "{}".into());
        let reason = if n.source_quote.is_empty() {
            "由一段对话整理（模型未给出原文摘录）".to_string()
        } else {
            format!("由一段对话整理。原文摘录：{}", n.source_quote)
        };
        ids.push(store.add_candidate(
            &kb_id,
            &key,
            &n.content,
            &meta_json,
            &n.tags.join(","),
            "work",
            "",
            &reason,
        )?);
    }
    Ok(KbDigestOutcome {
        count: ids.len(),
        ids,
        truncated,
    })
}

/// 取「整理」所需的模型与目标库字段定义。
/// 连接池 guard 不能跨 await，所以在进 await 之前一次性取出（模型含 api_key，只在后端使用）。
///
/// 模型**唯一来源**是 `kb_llm::resolve_kb_model`（=「设置 › 知识库」的 `kb_model_config`，
/// 未指定时回落第一个可用提供商）；设置页的「当前生效」调的也是同一个函数，两边不会漂移。
/// 这里把解析结果记一行日志：整理用了哪个模型必须能在运行记录里查到（`from_fallback` 尤其要标出来 ——
/// 那是"我明明指定了却没生效"的唯一线索）。
fn enrich_context(
    state: &State<'_, DbState>,
    kb_id: &str,
) -> Result<(crate::api_agent::kb_llm::KbModel, String, String), AppError> {
    let conn = state.pool.get().map_err(AppError::from)?;
    let model = crate::api_agent::kb_llm::resolve_kb_model(&conn).map_err(AppError::Config)?;
    let base = store()?
        .list_bases()
        .into_iter()
        .find(|b| b.id == kb_id)
        .ok_or_else(|| AppError::NotFound(format!("知识库 {} 不存在", kb_id)))?;
    log::info!(
        "[KB] 整理使用模型：{} · {}（{}）",
        model.provider_name,
        model.model,
        if model.from_fallback {
            "自动回落"
        } else {
            "设置›知识库 指定"
        }
    );
    Ok((model, base.name, base.fields_json))
}

/// 把本次知识库操作产生的模型用量落进 `api_usage_log`（归因键 `kb:{kb_id}`）。
///
/// 与记忆意图路由 / 滚动摘要同一范式：**模块内只负责把用量交出来，落库由持有 conn 与归因键的上层做**
/// （`kb_llm` 里没有 DbState，也不该有）。
///
/// **逐条落**而不是合并成一行：`api_usage_log` 的 `call_count` 就是行数，压成一行会把
/// "整理一个 500 块的文件"记成 1 次调用。**失败也照样记** —— 调用已经发生（可能已计费），
/// 用量表不该因为业务侧报错就漏账。
///
/// 归因粒度只到"知识库"这一层，**记不到"哪次会话触发的沉淀"**：`kb_*` 命令的入参里没有
/// 会话/房间上下文（要更细就得让前端把 sessionId 一起传下来）。
fn record_kb_usage(
    state: &State<'_, DbState>,
    kb_id: &str,
    model: &crate::api_agent::kb_llm::KbModel,
) {
    let Ok(conn) = state.pool.get() else {
        log::warn!("[KB] 取不到连接，本次用量未记账");
        return;
    };
    record_kb_usage_conn(&conn, kb_id, model);
}

/// 落库本体（吃 `&Connection`：轮末自动沉淀那边只有 `DbPool`，没有 `State`）。
fn record_kb_usage_conn(
    conn: &rusqlite::Connection,
    kb_id: &str,
    model: &crate::api_agent::kb_llm::KbModel,
) {
    let records = model.usage.take();
    if records.is_empty() {
        return;
    }
    let api_format = model
        .api_format
        .parse::<crate::api_agent::types::ApiFormat>()
        .unwrap_or_default();
    let scope = format!("{}{}", crate::commands::usage::KB_SCOPE_PREFIX, kb_id);
    for u in records {
        let _ = crate::api_agent::agent_loop::record_usage_row(
            conn,
            &scope,
            &model.provider_id,
            &model.model,
            &api_format,
            u.prompt,
            u.completion,
            u.total,
            u.cache_read,
            u.cache_write,
        );
    }
}

/* ── 沉淀会话增量（显式命令 / 自动触发共用） ── */

/// 「设置 › 知识库」里的**自动沉淀开关**（`app_settings`）。缺省 / 非 "0" = **开启**。
///
/// 为什么不再需要"先指定目标库"：产出由**知识库模型**按内容自行选库，并在同一次调用里
/// 按所选库的专属字段填属性（全部库的定义一次性交给模型；库数量通常是个位数，
/// 目录 token 相对对话内容可忽略）。
pub const KB_AUTO_ENABLED_SETTING: &str = "kb_auto_enabled";

/// 自动沉淀开关是否开启（缺省视为开启：`kb_auto_enabled` 为 "0"/"false" 才算关闭）。
fn kb_auto_enabled(conn: &rusqlite::Connection) -> bool {
    paths::get_app_setting_opt(conn, KB_AUTO_ENABLED_SETTING)
        .map(|v| {
            let t = v.trim();
            !(t == "0" || t.eq_ignore_ascii_case("false"))
        })
        .unwrap_or(true)
}

/// 每个会话的**沉淀水位**（已处理到第几条消息，存 `app_settings` 的 KV）。
///
/// 放 KV 而不是给 `sessions` 加列：不动 schema、不影响既有 SELECT 的列序；代价是删会话要顺手清
/// （见 `commands/session.rs` 的清理处）。水位的作用是**增量**：只把"上次之后的新消息"交给模型，
/// 既不重复产出候选，输入也更小。
pub fn kb_sink_cursor_key(session_id: &str) -> String {
    format!("kb_sink_cursor:{}", session_id)
}

/// 沉淀结果的对外形状（显式命令 / 自动触发共用）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SedimentOutcome {
    /// 产出的候选条数（0 是合法结果：这段内容没有可沉淀的知识）
    pub count: usize,
    /// 对话过长被截断（只整理了前 `DIGEST_MAX_CHARS` 字）—— UI 要说出来，别让用户以为整段都整理了
    pub truncated: bool,
    /// `count == 0` 时的原因：`busy`（同会话在途）/ `no_new`（没有新增）/ `no_base`（没有知识库）/ `off`（自动开关关闭）/ `too_soon`（距上次太近）/ `error`
    pub skipped: String,
}

impl SedimentOutcome {
    fn skip(reason: &str) -> Self {
        Self {
            count: 0,
            truncated: false,
            skipped: reason.to_string(),
        }
    }
}

/// 值得整理的**最小新增**：条数与字符**同时**低于阈值才跳过
/// （长回答条数少、短问答条数多，两种都能过；避免"一问一答"就触发）。
const SEDIMENT_MIN_NEW_MESSAGES: usize = 4;
const SEDIMENT_MIN_NEW_CHARS: usize = 1200;

/// 同一会话两次**自动**沉淀的最小间隔（秒，进程内）。显式命令不受此限。
const SEDIMENT_MIN_INTERVAL_SECS: i64 = 600;

/// 每会话的**在途标记** —— 真正的互斥：防"空闲触发 + 显式命令"并发整理同一会话
/// （并发双方都会读到同一个旧水位 → 各调一次模型 → 重复候选 + 重复计费）。
/// 与"最小间隔"是**两件事**：前者按**在途状态**，后者按**时间窗**，都要有。
fn sediment_inflight() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static SET: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 在途守卫：Drop 即释放（含失败 / 提前返回的所有路径）。
struct InFlightGuard(String);
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = sediment_inflight().lock() {
            set.remove(&self.0);
        }
    }
}
fn try_acquire_inflight(session_id: &str) -> Option<InFlightGuard> {
    let mut set = sediment_inflight().lock().ok()?;
    if !set.insert(session_id.to_string()) {
        return None; // 已有一次在跑
    }
    Some(InFlightGuard(session_id.to_string()))
}

/// 自动触发的"上次运行时间"（进程内）。重启后重新计时，最坏也只是多沉淀一次（仍然只进候选队列）。
fn last_run_map() -> &'static std::sync::Mutex<std::collections::HashMap<String, i64>> {
    static MAP: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
fn within_min_interval(session_id: &str) -> bool {
    let Ok(map) = last_run_map().lock() else {
        return false;
    };
    match map.get(session_id) {
        Some(prev) => crate::utils::now().saturating_sub(*prev) < SEDIMENT_MIN_INTERVAL_SECS,
        None => false,
    }
}
fn mark_run(session_id: &str) {
    if let Ok(mut map) = last_run_map().lock() {
        map.insert(session_id.to_string(), crate::utils::now());
    }
}

/// 整理某会话**自水位以来的新增**，产出候选（`origin='work'`）。显式命令与自动触发共用。
///
/// 与旧版（轮末版）的三点不同：
///   ① **从 DB 重建对话**（`load_session_messages`），不再依赖内存 messages ——
///      消息事件**只增不删**（滚动摘要只是**追加** `summary/result` 事件，且该事件已被消息投影排除）
///      → 列表**单调增长**，于是"消息条数"当水位才是安全的。
///      （旧的轮末路径吃的是内存 `output.messages`，压缩会把早期消息替换成摘要 → 列表变短/错位 →
///      下标水位可能重复或漏 —— 这正是弃用轮末的原因之一。）
///   ② **不含开关 / 限频闸门**（那是自动触发的事，见 `auto_sediment_session`）：显式命令不受开关限制。
///   ③ 返回 `Result`：硬失败（模型不可用 / 输出不合规）**如实报错**；软跳过用 `skipped` 说明。
///
/// 目标库不预设：把**全部知识库**的定义交给模型，由它逐条选库并填该库专属属性
/// （见 `kb_llm::digest_conversation_multi`）。三段分开是硬约束：**连接池 guard 不能跨 await**。
pub async fn sediment_session(
    state: &DbState,
    session_id: &str,
) -> Result<SedimentOutcome, AppError> {
    // 在途互斥：同一会话同一时刻只允许一次整理在跑（显式命令与自动触发之间也互斥）
    let Some(_inflight) = try_acquire_inflight(session_id) else {
        return Ok(SedimentOutcome::skip("busy"));
    };

    // ① 同步段：重建对话 + 读水位 → 拼增量 → 解析模型与全部库定义（guard 用完即还）
    let (bases, conversation, cursor_next, model, truncated) = {
        let conn = state.pool.get().map_err(AppError::from)?;
        let messages = crate::commands::session::load_session_messages(&conn, session_id)?;
        let cursor: usize = paths::get_app_setting_opt(&conn, &kb_sink_cursor_key(session_id))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        // 水位比列表还长时夹回（删过消息 / 异常数据时防"跳过一切"）
        let cursor = cursor.min(messages.len());
        // 只取水位之后、有正文的用户/助手消息（工具调用与系统提示不进沉淀）
        let fresh: Vec<KbDigestMessage> = messages
            .iter()
            .skip(cursor)
            .filter_map(|m| {
                if m.role != "user" && m.role != "assistant" {
                    return None;
                }
                let text = m.content.trim();
                if text.is_empty() {
                    return None;
                }
                Some(KbDigestMessage {
                    role: m.role.clone(),
                    content: text.to_string(),
                })
            })
            .collect();
        let chars: usize = fresh.iter().map(|m| m.content.chars().count()).sum();
        if fresh.len() < SEDIMENT_MIN_NEW_MESSAGES && chars < SEDIMENT_MIN_NEW_CHARS {
            return Ok(SedimentOutcome::skip("no_new"));
        }
        let conversation = format_conversation(&fresh);
        if conversation.is_empty() {
            return Ok(SedimentOutcome::skip("no_new"));
        }
        // 模型不可用 → 硬错误（显式命令如实报错；自动触发由 `auto_sediment_session` 吞掉、只记日志）
        let model = crate::api_agent::kb_llm::resolve_kb_model(&conn).map_err(AppError::Config)?;
        let kb_store = store()?;
        let bases: Vec<crate::api_agent::kb_llm::KbBaseDef> = kb_store
            .list_bases()
            .into_iter()
            .map(|b| crate::api_agent::kb_llm::KbBaseDef {
                id: b.id,
                name: b.name,
                description: b.description,
                fields_json: b.fields_json,
            })
            .collect();
        if bases.is_empty() {
            return Ok(SedimentOutcome::skip("no_base"));
        }
        let truncated = crate::api_agent::kb_llm::conversation_is_truncated(&conversation);
        (bases, conversation, messages.len(), model, truncated)
    };

    // ② 网络段：整理（不持任何 guard）。成败都要记用量（失败的调用也可能已计费）
    let digested =
        crate::api_agent::kb_llm::digest_conversation_multi(&model, &conversation, &bases).await;

    // ③ 落库段：记用量 + 按各条自选的库写候选 + 推进水位
    let conn = state.pool.get().map_err(AppError::from)?;
    // 一次调用可能写入多个库 → 用量记到保留伪库 id：只并入「知识库」维度合计，不单列分组
    record_kb_usage_conn(&conn, crate::commands::usage::KB_AUTO_ID, &model);
    let notes = digested.map_err(AppError::External)?;
    let kb_store = store()?;
    let mut count = 0usize;
    for (kb_id, n) in notes {
        let key = if n.title.trim().is_empty() {
            derive_title(&n.content)
        } else {
            n.title.clone()
        };
        if key.trim().is_empty() {
            continue;
        }
        let meta_json = serde_json::to_string(&n.meta).unwrap_or_else(|_| "{}".into());
        let reason = if n.source_quote.is_empty() {
            "整理新增对话".to_string()
        } else {
            format!("整理新增对话。原文摘录：{}", n.source_quote)
        };
        if kb_store
            .add_candidate(
                &kb_id,
                &key,
                &n.content,
                &meta_json,
                &n.tags.join(","),
                "work",
                "",
                &reason,
            )
            .is_ok()
        {
            count += 1;
        }
    }
    // 无论产出几条都推进水位：否则同一段对话会被反复整理（"0 条"也是有效结论）
    if let Err(e) = crate::commands::app_settings::set_setting(
        &conn,
        &kb_sink_cursor_key(session_id),
        &cursor_next.to_string(),
    ) {
        log::warn!("[KB] 沉淀水位写入失败（下次会重复整理这一段）：{}", e);
    }
    if count > 0 {
        log::info!(
            "[KB] 沉淀会话新增：{} 条候选进「待确认」（水位 → {}）",
            count,
            cursor_next
        );
    }
    Ok(SedimentOutcome {
        count,
        truncated,
        skipped: String::new(),
    })
}

/// 自动触发入口（前端「App 空闲」调用）：先过"开关 + 最小间隔"两道**自动专属**闸门，再走核心。
///
/// 失败只记日志 —— 自动沉淀是增益，绝不能影响用户正在做的事。这也是它与显式命令的差别：
/// 显式命令（`kb_sediment_session`）直接调核心、如实报错。
pub async fn auto_sediment_session(state: &DbState, session_id: &str) -> SedimentOutcome {
    // 闸门 ①：开关（默认开）
    match state.pool.get() {
        Ok(conn) => {
            if !kb_auto_enabled(&conn) {
                return SedimentOutcome::skip("off");
            }
        }
        Err(e) => {
            log::warn!("[KB] 自动沉淀跳过：取连接失败（{}）", e);
            return SedimentOutcome::skip("error");
        }
    }
    // 闸门 ③：最小间隔（进程内限频；在途互斥由核心负责）
    if within_min_interval(session_id) {
        return SedimentOutcome::skip("too_soon");
    }
    mark_run(session_id); // 无论成败都记，避免"一直失败就反复烧钱"
    match sediment_session(state, session_id).await {
        Ok(o) => o,
        Err(e) => {
            log::warn!("[KB] 自动沉淀未生效：{}", e);
            SedimentOutcome::skip("error")
        }
    }
}

/// 显式命令：整理本会话自上次以来的新增。**不受自动开关 / 限频约束**（只受在途互斥与"没有新增"）。
#[tauri::command]
pub async fn kb_sediment_session(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<SedimentOutcome, String> {
    Ok(sediment_session(&state, &session_id).await?)
}

/// 自动触发命令（前端「App 空闲」调用）：受开关与最小间隔约束，失败只记日志。
#[tauri::command]
pub async fn kb_auto_sediment_session(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<SedimentOutcome, String> {
    Ok(auto_sediment_session(&state, &session_id).await)
}

/// 用户给的标签 + 模型给的标签合并（用户在前、去重、最多 8 个）
fn merge_tags(user: &str, model: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    for t in user.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    for t in model {
        if !out.contains(t) {
            out.push(t.clone());
        }
    }
    out.truncate(8);
    out.join(",")
}

/// 入队一条待确认候选。**投喂即整理**：默认先让模型给出标题/标签/专属属性，再落队列，
/// 你在队列里核对修改后才入库；整理失败不阻断入库，只把原因写在候选的说明里。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn kb_add_candidate(
    state: State<'_, DbState>,
    kb_id: String,
    key: String,
    value: String,
    meta_json: Option<String>,
    tags: Option<String>,
    origin: Option<String>,
    source_ref: Option<String>,
    reason: Option<String>,
    // 是否先做 AI 整理（默认开）
    enrich: Option<bool>,
) -> Result<i64, String> {
    let user_meta = meta_json.as_deref().unwrap_or("{}").to_string();
    let mut key = key.trim().to_string();
    let mut value = value;
    let mut tags_str = tags.unwrap_or_default();
    let mut reason_str = reason.unwrap_or_default();
    let mut meta_str = user_meta.clone();
    // 片段投喂是"用户自己粘的草稿"，允许 AI 结构化改写正文；其余来源（工作沉淀）只打标不改写
    let is_snippet = origin.as_deref() == Some("snippet");
    let raw_snippet = value.clone();

    if enrich.unwrap_or(true) {
        let ctx = enrich_context(&state, &kb_id);
        match ctx {
            Ok((model, base_name, fields_json)) => {
                if is_snippet {
                    match crate::api_agent::kb_llm::process_snippet(
                        &model,
                        &value,
                        &base_name,
                        &fields_json,
                        &key,
                    )
                    .await
                    {
                        Ok(d) => {
                            if key.is_empty() && !d.title.is_empty() {
                                key = d.title;
                            }
                            value = d.content; // 片段路径**允许**用整理后的正文
                            tags_str = merge_tags(&tags_str, &d.tags);
                            let mut merged = d.meta;
                            if let Ok(serde_json::Value::Object(user)) =
                                serde_json::from_str::<serde_json::Value>(&user_meta)
                            {
                                for (k, v) in user {
                                    merged.insert(k, v);
                                }
                            }
                            meta_str =
                                serde_json::to_string(&merged).unwrap_or_else(|_| "{}".into());
                            reason_str = format!(
                                "已由 AI 整理成规范知识（正文可能被重排/分节，事实未增删）。\
                                 采纳前可对照原始片段：\n{}",
                                raw_snippet.chars().take(1500).collect::<String>()
                            );
                        }
                        Err(err) => {
                            log::warn!("[KB] 片段整理未生效: {}", err);
                            reason_str = format!("AI 整理未生效（已按原文入库）：{}", err);
                        }
                    }
                } else {
                    match crate::api_agent::kb_llm::enrich_text(
                        &model,
                        &value,
                        &base_name,
                        &fields_json,
                        &key,
                    )
                    .await
                    {
                        Ok(e) => {
                            if key.is_empty() && !e.title.is_empty() {
                                key = e.title;
                            }
                            tags_str = merge_tags(&tags_str, &e.tags);
                            // 用户显式填的专属属性优先于模型推断
                            let mut merged = e.meta;
                            if let Ok(serde_json::Value::Object(user)) =
                                serde_json::from_str::<serde_json::Value>(&user_meta)
                            {
                                for (k, v) in user {
                                    merged.insert(k, v);
                                }
                            }
                            meta_str =
                                serde_json::to_string(&merged).unwrap_or_else(|_| "{}".into());
                            if reason_str.trim().is_empty() {
                                reason_str = "已由 AI 整理：标题、标签、专属属性可改后采纳".into();
                            }
                        }
                        Err(err) => {
                            log::warn!("[KB] 整理候选失败: {}", err);
                            reason_str = format!("AI 整理未生效：{}", err);
                        }
                    }
                }
                // 成败都记账（失败的调用也可能已计费）
                record_kb_usage(&state, &kb_id, &model);
            }
            Err(err) => {
                log::warn!("[KB] 整理不可用: {}", err);
                reason_str = format!("AI 整理未生效：{}", err);
            }
        }
    }

    // 标题兜底：模型没给标题、调用方也没给 → 从正文推导一个能看的标题（句末标点优先，
    // 不像以前那样直接截前 30 个字把半句话当标题）
    if key.trim().is_empty() {
        key = derive_title(&value);
    }
    if key.trim().is_empty() {
        key = "未命名知识".to_string();
    }

    store()?.add_candidate(
        &kb_id,
        &key,
        &value,
        &meta_str,
        &tags_str,
        origin.as_deref().unwrap_or(""),
        source_ref.as_deref().unwrap_or(""),
        &reason_str,
    )
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbEnrichOutcome {
    /// 成功整理的分块数
    pub chunks: usize,
    /// 该原文的分块总数
    pub total: usize,
    pub failed: usize,
    pub error: String,
    /// 本次**补写过的专属属性字段**（去重、保序）：让"AI 动了哪些字段"可见，
    /// 而不是只汇报"整理了 N 条"（AI 主导维护之下，这是用户唯一能看到的痕迹）
    pub fields: Vec<String>,
}

/// AI 整理某个原文的全部分块：逐个打标 + 填专属属性后写回（**不改写正文**）。
///
/// 一个文件几十个分块就是几十次调用，所以按**文件**粒度暴露：前端逐个文件调用、每完成一个就刷新列表，
/// 用户能看到进度；失败的文件保留原文（只是少了标签与属性），行上可直接重试。
#[tauri::command]
pub async fn kb_enrich_file(
    state: State<'_, DbState>,
    kb_id: String,
    file_id: String,
) -> Result<KbEnrichOutcome, String> {
    let root = knowledge_root(&state)?;
    let (model, base_name, fields_json) = enrich_context(&state, &kb_id)?;
    let store = store()?;
    let file = store
        .list_files(&root, &kb_id)
        .into_iter()
        .find(|f| f.id == file_id)
        .ok_or_else(|| AppError::NotFound(format!("文件 {} 不在该知识库中", file_id)))?;
    let chunks = store.file_chunks(&kb_id, &file.rel_path);
    let total = chunks.len();
    if total == 0 {
        store.mark_file_enriched(&file_id, true, "")?;
        return Ok(KbEnrichOutcome {
            chunks: 0,
            total: 0,
            failed: 0,
            error: String::new(),
            fields: Vec::new(),
        });
    }

    let mut done = 0usize;
    let mut failed = 0usize;
    let mut last_err = String::new();
    let mut fields: Vec<String> = Vec::new();
    for (key, value) in chunks {
        match crate::api_agent::kb_llm::enrich_text(
            &model,
            &value,
            &base_name,
            &fields_json,
            &file.name,
        )
        .await
        {
            Ok(e) => {
                let meta_json = serde_json::to_string(&e.meta).unwrap_or_else(|_| "{}".into());
                // 记下**本次补写过哪些属性字段**（不是"改动了什么"——那要逐条 diff，
                // 而这里的语义是"AI 刚写入了这些字段"，如实说这一句就够了）
                if let Ok(serde_json::Value::Object(map)) =
                    serde_json::from_str::<serde_json::Value>(&meta_json)
                {
                    for k in map.keys() {
                        if !fields.iter().any(|f| f == k) {
                            fields.push(k.clone());
                        }
                    }
                }
                match store.update_entry_enrich(&kb_id, &key, &meta_json, &e.tags.join(",")) {
                    Ok(()) => done += 1,
                    Err(err) => {
                        failed += 1;
                        last_err = err;
                    }
                }
            }
            Err(err) => {
                failed += 1;
                last_err = err;
                // 连续失败多半是模型/额度/网络问题，接着跑只会白烧额度
                if failed >= 3 {
                    break;
                }
            }
        }
    }
    // 逐块调用，所以这里可能攒了 N 条用量 —— 一次性落库（每条一行，call_count 才是真实调用次数）
    record_kb_usage(&state, &kb_id, &model);
    let ok = failed == 0;
    store.mark_file_enriched(&file_id, ok, if ok { "" } else { &last_err })?;
    Ok(KbEnrichOutcome {
        chunks: done,
        total,
        failed,
        error: last_err,
        fields,
    })
}

/// AI 生成：按主题产出一条知识草稿，落成待确认候选（origin=ai），返回候选 id。
///
/// 模型来自「设置 › 知识库」的 `kb_model_config`，未配置时回落第一个可用提供商；
/// 两条降级：没有任何可用提供商 → 报错说明去哪配；provider 不支持 json 输出 → 自动重试一次。
#[tauri::command]
pub async fn kb_generate(
    state: State<'_, DbState>,
    kb_id: String,
    topic: String,
) -> Result<i64, String> {
    let topic = topic.trim().to_string();
    if topic.is_empty() {
        return Err(AppError::InvalidInput("请先写一个主题或要求".into()).into());
    }
    // 与其余整理路径共用 `enrich_context`：模型解析只有一处，设置页「当前生效」显示的也是它
    let (model, base_name, fields_json) = enrich_context(&state, &kb_id)?;
    let drafted =
        crate::api_agent::kb_llm::generate_draft(&model, &topic, &base_name, &fields_json).await;
    // 成败都记账（失败的调用也可能已计费）
    record_kb_usage(&state, &kb_id, &model);
    let draft = drafted?;
    let meta_json = serde_json::to_string(&draft.meta).unwrap_or_else(|_| "{}".to_string());
    // 模型偶尔不按格式给标题：优先用用户给的主题（最像标题），再退到正文推导
    let title = if draft.title.trim().is_empty() {
        let topic_title: String = topic.trim().chars().take(40).collect();
        if topic_title.is_empty() {
            derive_title(&draft.content)
        } else {
            topic_title
        }
    } else {
        draft.title
    };
    store()?.add_candidate(
        &kb_id,
        &title,
        &draft.content,
        &meta_json,
        &draft.tags.join(","),
        "ai",
        "",
        &format!("AI 生成 · 模型 {} · 主题：{}", model.model, topic),
    )
}

/// 采纳候选：可带用户改过的标题/正文/专属属性
#[tauri::command]
pub fn kb_adopt_candidate(
    id: i64,
    key: Option<String>,
    value: Option<String>,
    meta_json: Option<String>,
) -> Result<(), String> {
    store()?.adopt_candidate(id, key.as_deref(), value.as_deref(), meta_json.as_deref())
}

#[tauri::command]
pub fn kb_reject_candidate(id: i64) -> Result<(), String> {
    store()?.reject_candidate(id)
}

#[cfg(test)]
mod tests {
    use super::unique_export_name;
    use std::collections::HashSet;

    /// 对话沉淀必须**带角色**：模型要能分清"用户说的"与"助手建议的"
    #[test]
    fn conversation_is_formatted_with_roles() {
        use super::{format_conversation, KbDigestMessage};
        let msgs = vec![
            KbDigestMessage {
                role: "user".into(),
                content: " 5000 以内谁批？ ".into(),
            },
            KbDigestMessage {
                role: "assistant".into(),
                content: "建议直属领导批。".into(),
            },
            KbDigestMessage {
                role: "user".into(),
                content: "   ".into(),
            }, // 空消息跳过
        ];
        let got = format_conversation(&msgs);
        assert_eq!(got, "用户：5000 以内谁批？\n\n助手：建议直属领导批。");
        assert!(format_conversation(&[]).is_empty());
    }

    /// 导出绝不覆盖：重名一律加后缀（这是"拿出去用"的动作，静默覆盖用户文件不可接受）
    #[test]
    fn export_names_never_overwrite() {
        let mut used: HashSet<String> = HashSet::new();
        // 目标目录里已经有 report.md
        used.insert("report.md".to_string());

        assert_eq!(unique_export_name("report.md", &mut used), "report-1.md");
        assert_eq!(unique_export_name("report.md", &mut used), "report-2.md");
        // 无扩展名 / 多点扩展名 / 首次出现
        assert_eq!(unique_export_name("no-ext", &mut used), "no-ext");
        assert_eq!(unique_export_name("no-ext", &mut used), "no-ext-1");
        assert_eq!(unique_export_name("a.tar.gz", &mut used), "a.tar.gz");
        assert_eq!(unique_export_name("a.tar.gz", &mut used), "a.tar-1.gz");
    }

    use super::{judge_page, PageVerdict};

    /// 真实数据：这是百度百家号被拦时**实际**返回的页面（去标签后的全部正文）。
    /// 它 HTTP 200、非空，所以能溜过"有没有正文"的检查 —— 必须靠特征词拦下来，
    /// 否则库里会多出一个名为「百度安全验证」的文件。
    #[test]
    fn real_block_page_is_detected() {
        let text = "百度安全验证\n\n网络不给力，请稍后重试\n\n返回首页\n\n问题反馈";
        assert!(matches!(
            judge_page("百度安全验证", text),
            PageVerdict::Blocked(_)
        ));
        // 换一种挂羊头卖狗肉的标题也拦得住（正文里还有特征词）
        assert!(matches!(
            judge_page("某某新闻", text),
            PageVerdict::Blocked(_)
        ));
    }

    /// 长文里出现"验证码"不能误伤：一篇讲验证码的文章不是 300 字以下
    #[test]
    fn long_article_mentioning_captcha_is_not_blocked() {
        let text = format!(
            "验证码安全设计\n\n{}",
            "本文讨论验证码的演进与失效场景。".repeat(40)
        );
        assert!(text.chars().count() >= 300);
        assert!(matches!(
            judge_page("验证码安全设计", &text),
            PageVerdict::Ok
        ));
    }

    /// 短页面的三种去向：硬拦（报错，不渲染）/ 软拦（值得渲染一次）/ 空壳（值得渲染一次）
    #[test]
    fn short_pages_are_classified_for_rendering() {
        assert!(matches!(
            judge_page("安全验证", "请拖动滑块完成验证"),
            PageVerdict::Blocked(_)
        ));
        assert!(matches!(
            judge_page("Just a moment...", "checking your browser"),
            PageVerdict::Soft
        ));
        assert!(matches!(
            judge_page("某某公司", "首页 产品 关于我们"),
            PageVerdict::Shell
        ));
    }

    /// 文案必须给出可执行的路，而不是只说"失败"（用户在浏览器里明明看得到）
    #[test]
    fn block_message_points_to_workarounds() {
        let msg = super::block_message("安全验证");
        assert!(msg.contains("人机验证"), "{msg}");
        assert!(msg.contains(".html"), "应指路「文件」投喂：{msg}");
        assert!(msg.contains("片段"), "应指路「片段」投喂：{msg}");
    }

    /// 「允许重命名」只改名字、**保留原扩展名**：内容与格式都没变，扩展名跟着改会让文件名说谎
    #[test]
    fn rename_keeps_the_original_extension() {
        use super::with_original_ext;
        assert_eq!(
            with_original_ext("2024年度采购管理办法", "s(1).htm"),
            "2024年度采购管理办法.htm"
        );
        // 用户把原后缀也打进来了：不重复追加
        assert_eq!(with_original_ext("制度.htm", "s(1).htm"), "制度.htm");
        // 用户写了个别的后缀：以原格式为准（界面把后缀固定显示，这不是"吃掉输入"）
        assert_eq!(with_original_ext("制度.md", "s(1).htm"), "制度.md.htm");
        // 原文件没有后缀：就只是改名
        assert_eq!(with_original_ext("新名字", "README"), "新名字");
    }

    /// 去掉用户顺手打上的后缀，避免 `制度.md.md`
    #[test]
    fn doc_ext_is_stripped_only_for_known_formats() {
        use super::strip_doc_ext;
        assert_eq!(strip_doc_ext("制度.md"), "制度");
        assert_eq!(strip_doc_ext("制度.HTML"), "制度");
        // `报告 2024.01` 不是扩展名，不能被剥掉（那等于改坏用户给的名字）
        assert_eq!(strip_doc_ext("报告 2024.01"), "报告 2024.01");
    }
}
