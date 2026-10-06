//! 在线商店远程资源模块
//!
//! 统一管理在线商店的服务器源配置与远程资源获取。
//! 前端通过 Tauri Command 调用本模块获取远程资源，
//! 无需在前端维护 URL 配置（消除 TS/RS 双端重复）。
//!
//! 本地路径管理见 utils/paths.rs。

use serde_json::Value;

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// URL 路径段编码集：保留 RFC 3986 的 unreserved 字符（`- . _ ~`），其余全部百分号编码。
///
/// 与索引生成器侧 `encodeURIComponent` 的集合一致（它同样不编码这几个字符），
/// 所以 Rust 侧自己拼出的路径与索引里已编码的 `path` 落在同一套规则上。
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn encode_path_segment(s: &str) -> String {
    utf8_percent_encode(s, PATH_SEGMENT).to_string()
}

/// 在线商店服务器源（按优先级排列）
/// 1. jsdelivr CDN（主源，速度快，全球加速）
/// 2. GitHub Raw（降级源，直连仓库）
pub const SERVER_SOURCES: &[&str] = &[
    "https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main",
    "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main",
];

/// 在线商店路径
pub const AGENTS_CONFIG_PATH: &str = "/server/market/agents-config/agents-config.json";
pub const PLUGINS_INDEX_PATH: &str = "/server/market/plugins/plugins-index.json";
pub const INSPIRATIONS_INDEX_PATH: &str = "/server/market/inspirations/index.json";
pub const WORKFLOW_INDEX_PATH: &str = "/server/market/workflow/workflow-index.json";
#[allow(dead_code)]
pub const PLUGINS_DIR_PATH: &str = "/server/market/plugins";

/// 市场资源在仓库内的根路径。
/// 索引里每个插件的 `path` 字段（如 `plugins/hello-world/1.0.0`）就是相对它的相对路径。
pub const MARKET_ROOT: &str = "/server/market";

/// 根据路径构建所有服务器源的完整 URL 列表
pub fn build_urls(path: &str) -> Vec<String> {
    SERVER_SOURCES
        .iter()
        .map(|base| format!("{}{}", base.trim_end_matches('/'), path))
        .collect()
}

/// 把「某个服务器源 + 相对市场路径」拼成绝对 URL。
///
/// `relative_path` 相对 `server/market/`（例如 `plugins/hello-world/1.0.0`）。
/// 两侧斜杠都做归一化，避免拼出双斜杠。
pub fn build_market_url(source: &str, relative_path: &str) -> String {
    format!(
        "{}{}/{}",
        source.trim_end_matches('/'),
        MARKET_ROOT,
        relative_path.trim_start_matches('/')
    )
}

/// HTTP 超时配置
const CONNECT_TIMEOUT_SECS: u64 = 8;
const READ_TIMEOUT_SECS: u64 = 8;
const MAX_RETRIES: u32 = 2;
const RETRY_DELAY_MS: u64 = 1000;

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
        .no_proxy()
        .build()
        .expect("Failed to build HTTP client")
}

/// 从单个 URL 获取文本内容（含重试）
async fn fetch_url(url: &str) -> Result<String, String> {
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRIES {
        match http_client().get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    return resp
                        .text()
                        .await
                        .map_err(|e| format!("读取响应失败: {}", e));
                } else if status >= 500 && attempt < MAX_RETRIES {
                    last_err = format!("HTTP {}: {}", status, url);
                    log::warn!(
                        "[Market] 第 {} 次请求失败 ({}), {}ms 后重试...",
                        attempt,
                        last_err,
                        RETRY_DELAY_MS
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                } else {
                    return Err(format!("HTTP {}: {}", status, url));
                }
            }
            Err(e) => {
                if attempt < MAX_RETRIES && (e.is_timeout() || e.is_connect()) {
                    last_err = format!(
                        "{}: {}",
                        if e.is_timeout() {
                            "请求超时"
                        } else {
                            "连接失败"
                        },
                        url
                    );
                    log::warn!(
                        "[Market] 第 {} 次请求失败 ({}), {}ms 后重试...",
                        attempt,
                        last_err,
                        RETRY_DELAY_MS
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
                return Err(format!(
                    "{}: {}",
                    if e.is_timeout() {
                        "请求超时"
                    } else {
                        "连接失败"
                    },
                    url
                ));
            }
        }
    }
    Err(format!("重试 {} 次后仍失败: {}", MAX_RETRIES, last_err))
}

/// 按源优先级依次尝试获取远程 JSON 资源。
///
/// 返回 `(JSON, 实际命中的源)`；源即 `SERVER_SOURCES` 中的基地址，
/// 调用方可用它配合`build_market_url`把相对路径解析成同一源下的绝对地址。
pub(crate) async fn fetch_market_json_with_source(path: &str) -> Result<(Value, String), String> {
    let urls = build_urls(path);
    let mut last_err = String::new();
    for (source, url) in SERVER_SOURCES.iter().zip(urls.iter()) {
        match fetch_url(url).await {
            Ok(body) => match serde_json::from_str::<Value>(&body) {
                Ok(json) => return Ok((json, (*source).to_string())),
                Err(e) => {
                    last_err = format!("解析 JSON 失败 ({}): {}", url, e);
                }
            },
            Err(e) => {
                last_err = e;
            }
        }
    }
    Err(format!("所有服务器源均不可用: {}", last_err))
}

/// 按源优先级依次尝试获取远程 JSON 资源
pub(crate) async fn fetch_market_json(path: &str) -> Result<Value, String> {
    fetch_market_json_with_source(path)
        .await
        .map(|(json, _)| json)
}

/// 按源优先级依次尝试获取远程**文本**资源（原样返回，不做 JSON 解析）
///
/// 模板安装要把文件原文落到磁盘，经由 `serde_json::Value` 转一手会丢掉原始排版
/// （导入逻辑虽然只认结构，但落到用户机器上的文件保持原样更好排查）。
async fn fetch_market_text(path: &str) -> Result<String, String> {
    let urls = build_urls(path);
    let mut last_err = String::new();
    for url in urls.iter() {
        match fetch_url(url).await {
            Ok(body) => return Ok(body),
            Err(e) => last_err = e,
        }
    }
    Err(format!("所有服务器源均不可用: {}", last_err))
}

/// [Tauri Command] 获取 Agent 市场配置
///
/// 返回 agents-config.json 的 JSON 内容，前端直接使用。
#[tauri::command]
pub async fn fetch_agents_config() -> Result<Value, String> {
    fetch_market_json(AGENTS_CONFIG_PATH).await
}

/// [Tauri Command] 获取灵感市场索引
///
/// 返回 index.json 的 JSON 内容（每条只有标题/图标/摘要等元信息，正文不在这里）。
/// 索引由 `server/scripts/generate-inspiration-index.mjs` 生成，一条灵感一个文件 ——
/// 与插件/模板市场同构：索引是清单，真数据在原文件里，正文只有一份。
#[tauri::command]
pub async fn inspiration_market_index() -> Result<Value, String> {
    fetch_market_json(INSPIRATIONS_INDEX_PATH).await
}

/// [Tauri Command] 按 id 获取单条灵感的正文
///
/// 用于详情弹窗（列表只有摘要，点开才拉正文）。
///
/// 路径**只认索引里给出的 `path`**，不接受前端传任意路径：否则这个命令就成了
/// "拿 CDN 当跳板读任意文件"的入口，而它本来只需要读市场内的灵感文件。
#[tauri::command]
pub async fn inspiration_market_fetch(id: String) -> Result<Value, String> {
    let index = fetch_market_json(INSPIRATIONS_INDEX_PATH).await?;
    let rel = index
        .get("inspirations")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        })
        .and_then(|e| e.get("path"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("灵感市场中未找到：{}", id))?;

    // 索引里的 path 相对市场根（server/market），与插件安装用的是同一套拼法
    fetch_market_json(&format!("{}/{}", MARKET_ROOT, rel.trim_start_matches('/'))).await
}

// ── 工作流模板市场 ──
//
// 索引由 `server/scripts/generate-workflow-index.mjs` 生成：模板派生字段（名称/版本/阶段数/
// 节点数/触发方式/节点类型）来自模板 JSON 本身，分类/标签/作者等人工字段来自 catalog.json。
// 前端只拉索引即可渲染列表与详情（详情零请求），安装动作走下面的命令。

/// 本地安装记录在 app_settings 表里的键（值是 JSON map：模板 id → 记录）
const WORKFLOW_INSTALLS_KEY: &str = "workflow_market_installs";

/// [Tauri Command] 获取工作流模板市场索引
#[tauri::command]
pub async fn workflow_market_index() -> Result<Value, String> {
    fetch_market_json(WORKFLOW_INDEX_PATH).await
}

/// 读全部安装记录（map）。记录是本地派生物：不存在 / 内容损坏都按"没装过"处理，
/// 不能让一份坏记录把整个市场页卡死（最坏后果只是显示未安装，重装即可覆盖）。
fn load_install_records(conn: &rusqlite::Connection) -> Value {
    match crate::commands::app_settings::get_setting(conn, WORKFLOW_INSTALLS_KEY) {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            log::warn!("[Market] 安装记录不是合法 JSON，按空记录处理: {}", e);
            serde_json::json!({})
        }),
        Ok(None) => serde_json::json!({}),
        Err(e) => {
            log::warn!("[Market] 读取安装记录失败，按空记录处理: {}", e);
            serde_json::json!({})
        }
    }
}

fn save_install_records(conn: &rusqlite::Connection, records: &Value) -> Result<(), String> {
    crate::commands::app_settings::set_setting(conn, WORKFLOW_INSTALLS_KEY, &records.to_string())
        .map_err(|e| format!("保存安装记录失败: {}", e))
}

/// [Tauri Command] 读取本地安装记录
///
/// 返回 map（模板 id → `{version, mainId, subIds, installedAt}`）。
/// 前端用它派生「已安装 / 可更新」：记录里的版本与索引版本不一致即"可更新"。
#[tauri::command]
pub fn workflow_market_installs(state: tauri::State<'_, crate::DbState>) -> Result<Value, String> {
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    Ok(load_install_records(&conn))
}

/// 把模板目录的文件下载到 `tmp_dir`（**保持原文件名**）。
///
/// 文件名必须保留：老包靠 `[子]xxx(ref_N).json` 命名收集子工作流，新包靠 `manifest.json`
/// 声明成员文件——改名都会让子工作流对不上号。新包的清单在索引里是独立的 `manifestFile`
/// 字段（不计入 `subFiles` 的「子工作流」语义），这里单独下载：导入时要靠同目录的清单识别新包，
/// 缺了它会退回老路径、丢掉子工作流。子文件的路径在 Rust 侧统一重编码 ——
/// 目录名带中文与全角括号，不编码直接拼 URL 会得到非法请求。
async fn download_template_files(
    tmp_dir: &std::path::Path,
    main_rel: &str,
    main_file: &str,
    manifest_file: Option<&str>,
    dir: &str,
    sub_files: &[String],
) -> Result<(), String> {
    std::fs::create_dir_all(tmp_dir).map_err(|e| format!("创建临时目录失败: {}", e))?;
    let base = format!("{}/", MARKET_ROOT);

    // 主文件路径直接用索引里已编码好的 path（生成器侧一次编好，避免两处编码规则漂移）
    let main_text =
        fetch_market_text(&format!("{}{}", base, main_rel.trim_start_matches('/'))).await?;
    std::fs::write(tmp_dir.join(main_file), main_text)
        .map_err(|e| format!("写入主文件失败: {}", e))?;

    // 新文件包的清单（老包无此字段 → None，行为不变）
    if let Some(manifest) = manifest_file {
        let rel = format!(
            "workflow/{}/{}",
            encode_path_segment(dir),
            encode_path_segment(manifest)
        );
        let text = fetch_market_text(&format!("{}{}", base, rel)).await?;
        std::fs::write(tmp_dir.join(manifest), text)
            .map_err(|e| format!("写入工作流清单失败: {}", e))?;
    }

    for sub in sub_files {
        let rel = format!(
            "workflow/{}/{}",
            encode_path_segment(dir),
            encode_path_segment(sub)
        );
        let text = fetch_market_text(&format!("{}{}", base, rel)).await?;
        std::fs::write(tmp_dir.join(sub), text)
            .map_err(|e| format!("写入子工作流文件失败: {}", e))?;
    }
    Ok(())
}

/// [Tauri Command] 安装工作流模板（下载 → 导入 → 记安装记录）
///
/// 安装 = 按索引把模板目录里的主/子文件下载到临时目录，再交给
/// `import_workflow_from_file_with_conn`（新包按 `manifest.json` 清单导入，老包收集同目录
/// `[子]` 文件并重建子工作流引用）。
/// 重复安装（更新）时先用本地记录删掉上一次的 主+子 定义，避免「工作流定义」里堆出同名副本。
///
/// 路径只认索引给出的 dir/mainFile/subFiles，不接受前端传任意路径 ——
/// 否则这个命令就成了"拿 CDN 当跳板下载任意文件"的入口（同 [`inspiration_market_fetch`]）。
#[tauri::command]
pub async fn workflow_market_install(
    state: tauri::State<'_, crate::DbState>,
    id: String,
) -> Result<Value, String> {
    // 1. 索引里定位模板
    let index = fetch_market_json(WORKFLOW_INDEX_PATH).await?;
    let entry = index
        .get("templates")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        })
        .ok_or_else(|| format!("模板市场中未找到：{}", id))?;

    let main_rel = entry
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "索引条目缺少 path".to_string())?;
    let main_file = entry
        .get("mainFile")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "索引条目缺少 mainFile".to_string())?;
    let dir = entry.get("dir").and_then(|v| v.as_str()).unwrap_or("");
    let version = entry
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sub_files: Vec<String> = entry
        .get("subFiles")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // 新文件包的清单文件名（老索引无此字段 → None，行为不变）
    let manifest_file = entry.get("manifestFile").and_then(|v| v.as_str());

    // 2. 下载到临时目录（失败要清理，避免在系统临时目录里留垃圾）
    let tmp_dir =
        std::env::temp_dir().join(format!("pilotdesk-market-wf-{}", crate::utils::new_id()));
    if let Err(e) =
        download_template_files(&tmp_dir, main_rel, main_file, manifest_file, dir, &sub_files).await
    {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    // 3. 导入：先清掉上一次装的 主+子 定义，再导入新的一份
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败: {}", e))?;
    let mut records = load_install_records(&conn);
    if let Some(old) = records.get(id.as_str()) {
        let mut old_ids: Vec<String> = Vec::new();
        if let Some(m) = old.get("mainId").and_then(|v| v.as_str()) {
            old_ids.push(m.to_string());
        }
        if let Some(subs) = old.get("subIds").and_then(|v| v.as_array()) {
            old_ids.extend(subs.iter().filter_map(|v| v.as_str().map(str::to_string)));
        }
        for old_id in old_ids {
            // 内部替换：市场重装前真删上一次装的旧定义（不进回收站）；
            // 旧定义可能已被用户手动删掉/改过，删不到只记日志，不打断这次安装
            if let Err(e) = crate::workflow::hard_delete_definition(&conn, &old_id) {
                log::warn!("[Market] 清理旧定义失败 {}: {}", old_id, e);
            }
        }
    }

    let main_path = tmp_dir.join(main_file);
    let imported = match crate::commands::workflow::import_workflow_from_file_with_conn(
        &conn,
        &main_path.to_string_lossy(),
    ) {
        Ok(outcome) => outcome.definition,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(e);
        }
    };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // 4. 记安装记录：子工作流 id 从导入后的主定义里提取（Subflow 节点的 definitionId 指向
    //    新生成的子定义），这样"下次更新先删旧的"能连子工作流一起清掉
    let sub_ids: Vec<String> = imported
        .stages
        .iter()
        .flat_map(|s| s.nodes.iter())
        .filter(|n| n.node_type == crate::workflow::WorkflowNodeType::Subflow)
        .filter_map(|n| {
            n.params
                .as_ref()?
                .get("definitionId")?
                .as_str()
                .map(str::to_string)
        })
        .collect();

    let record = serde_json::json!({
        "version": version,
        "mainId": imported.id,
        "subIds": sub_ids,
        "installedAt": crate::utils::now(),
    });
    if let Some(map) = records.as_object_mut() {
        map.insert(id.clone(), record.clone());
    }
    save_install_records(&conn, &records)?;
    Ok(record)
}
