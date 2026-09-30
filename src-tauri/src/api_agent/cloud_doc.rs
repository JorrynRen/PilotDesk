//! 云文档源（第一期：语雀）
//!
//! 定位：**只负责"取内容"** —— 列知识库、列文档、取正文（markdown）。
//! 落库一律交给 `commands::knowledge` 里已有的投喂链路
//! （`prepare_cloud` → AI 整理 → `ingest_prepared`），
//! 这样"云文档"与"文件 / 网页"共用同一套 sha 去重、分块、AI 整理、图谱与检索逻辑。
//!
//! 凭证只在后端存在：命令只收 `source`（平台名），Token 由命令层从 app_settings 解密后传进来，
//! **前端永远拿不到明文**。
//!
//! 加飞书时：在 `CloudSource` 里加一项，补 `id/label/api_base`，
//! 再实现对应的 `list_repos / list_docs / fetch_doc`（或按平台分支）即可。

use serde::Serialize;
use std::time::Duration;

/// 语雀公有云接口基址（企业版是 `https://{company}.yuque.com/api/v2`，本期不支持）
const YUQUE_API_BASE: &str = "https://www.yuque.com/api/v2";
/// 列表接口的分页：每页条数与最多翻几页（防"知识库特别大"时把请求打成海量）
const PAGE_SIZE: usize = 100;
const MAX_LIST_PAGES: usize = 5;
const TIMEOUT_SECS: u64 = 30;

/// 支持的云文档平台。第一期只有语雀。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudSource {
    Yuque,
}

impl CloudSource {
    /// `""` 也按语雀处理：只有一个平台时，前端不传来源也应当能用
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "yuque" | "" => Ok(Self::Yuque),
            other => Err(format!("暂不支持该云文档平台：{}", other)),
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::Yuque => "yuque",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Yuque => "语雀",
        }
    }

    fn api_base(&self) -> &'static str {
        match self {
            Self::Yuque => YUQUE_API_BASE,
        }
    }
}

/// 校验凭证的结果：`login` 用于后续接口寻址，`display` 给界面显示"配的是谁"
pub struct CloudAccount {
    pub login: String,
    pub display: String,
}

/// 云文档平台上的一个知识库（= 一个可含多篇文档的集合）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRepo {
    /// 命名空间（`login/slug`）：列文档、取正文都用它定位
    pub namespace: String,
    pub name: String,
    pub description: String,
    /// 所属团队（个人知识库为空）—— 用来在长列表里区分同名知识库
    pub group_name: String,
    pub doc_count: i64,
    pub updated_at: String,
}

/// 知识库里的一篇文档（只有元信息，正文要另取）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudDoc {
    /// 文档 slug（在本知识库内唯一）
    pub id: String,
    pub title: String,
    pub word_count: i64,
    pub updated_at: String,
}

/// 取回的一篇文档正文
pub struct CloudDocContent {
    pub title: String,
    /// markdown 正文
    pub body: String,
    /// 在线地址：落进 `kb_files.url`，供"打开原文"与来源回溯
    pub url: String,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("创建请求客户端失败: {}", e))
}

/// GET 一个返回 `{ "data": ... }` 信封的接口，取出 `data`。
///
/// 错误信息里必须带上平台名与原始 message：Token 无效、没有该文档权限、路径写错
/// 在语雀都是 4xx，只回一句"请求失败"用户没法自查。
async fn get_data(
    client: &reqwest::Client,
    source: CloudSource,
    token: &str,
    path: &str,
) -> Result<serde_json::Value, String> {
    let url = format!("{}{}", source.api_base(), path);
    let resp = client
        .get(&url)
        .header("X-Auth-Token", token)
        // 部分平台对默认 UA 直接拒绝，显式给一个
        .header("User-Agent", "PilotDesk/1.0 (knowledge-cloud-ingest)")
        .send()
        .await
        .map_err(|e| format!("请求{}失败: {}", source.label(), e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        let code = status.as_u16();
        if code == 401 || code == 403 {
            return Err(format!(
                "{} 凭证无效或没有该内容的权限：{}",
                source.label(),
                msg
            ));
        }
        return Err(format!(
            "{} 接口返回 HTTP {}：{}",
            source.label(),
            code,
            msg
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("解析{}返回失败: {}", source.label(), e))?;
    // 语雀成功响应统一是 `{ "data": ... }`；没有 data 时当作空值，由调用方判断
    Ok(value
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

/// 翻页取一个列表接口的全部条目（带页数上限）
async fn get_array_paged(
    client: &reqwest::Client,
    source: CloudSource,
    token: &str,
    path: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let mut out = Vec::new();
    for page in 0..MAX_LIST_PAGES {
        let sep = if path.contains('?') { '&' } else { '?' };
        let data = get_data(
            client,
            source,
            token,
            &format!(
                "{}{}limit={}&offset={}",
                path,
                sep,
                PAGE_SIZE,
                page * PAGE_SIZE
            ),
        )
        .await?;
        let arr = data
            .as_array()
            .ok_or_else(|| format!("{} 返回的不是列表", source.label()))?;
        let got = arr.len();
        out.extend(arr.iter().cloned());
        if got < PAGE_SIZE {
            break;
        }
    }
    Ok(out)
}

fn str_of(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn int_of(v: &serde_json::Value, key: &str) -> i64 {
    v.get(key).and_then(|x| x.as_i64()).unwrap_or(0)
}

/// 校验 Token。返回账号信息 —— 顺便把"这个 Token 是谁的"确认下来，
/// 免得用户把别人的 Token 配进来却以为配好了。
pub async fn verify(source: CloudSource, token: &str) -> Result<CloudAccount, String> {
    let client = client()?;
    let me = get_data(&client, source, token, "/user").await?;
    let login = str_of(&me, "login");
    if login.is_empty() {
        return Err(format!(
            "{} 没有返回用户信息，请检查 Token 是否有效",
            source.label()
        ));
    }
    let name = str_of(&me, "name");
    let display = if name.is_empty() {
        login.clone()
    } else {
        format!("{}（{}）", name, login)
    };
    Ok(CloudAccount { login, display })
}

/// 列出该账号可见的全部知识库：个人 + 各团队下的。
///
/// 团队那一段用 `if let Ok` 吞掉失败：个人版语雀账号调团队接口会 403，
/// 那不是"拉取失败"，只是"没有团队" —— 不该让整次拉取报错。
pub async fn list_repos(
    source: CloudSource,
    token: &str,
    login: &str,
) -> Result<Vec<CloudRepo>, String> {
    let client = client()?;
    let mut repos: Vec<CloudRepo> = Vec::new();

    for item in get_array_paged(&client, source, token, &format!("/users/{}/repos", login)).await? {
        if let Some(r) = parse_repo(&item, "") {
            repos.push(r);
        }
    }

    if let Ok(groups) =
        get_array_paged(&client, source, token, &format!("/users/{}/groups", login)).await
    {
        for g in groups {
            let glogin = str_of(&g, "login");
            if glogin.is_empty() {
                continue;
            }
            let gname = {
                let n = str_of(&g, "name");
                if n.is_empty() {
                    glogin.clone()
                } else {
                    n
                }
            };
            if let Ok(items) =
                get_array_paged(&client, source, token, &format!("/groups/{}/repos", glogin)).await
            {
                for item in items {
                    if let Some(r) = parse_repo(&item, &gname) {
                        repos.push(r);
                    }
                }
            }
        }
    }

    Ok(repos)
}

/// `namespace` 是空的知识库条目一律丢掉：没有它就无法定位文档，列出来只会点不动
fn parse_repo(v: &serde_json::Value, group_name: &str) -> Option<CloudRepo> {
    let namespace = str_of(v, "namespace");
    if namespace.trim().is_empty() {
        return None;
    }
    Some(CloudRepo {
        namespace,
        name: str_of(v, "name"),
        description: str_of(v, "description"),
        group_name: group_name.to_string(),
        doc_count: int_of(v, "items_count"),
        updated_at: str_of(v, "updated_at"),
    })
}

/// 列出某个知识库下的文档
pub async fn list_docs(
    source: CloudSource,
    token: &str,
    namespace: &str,
) -> Result<Vec<CloudDoc>, String> {
    let ns = namespace.trim();
    if ns.is_empty() {
        return Err("请先选择知识库".into());
    }
    let client = client()?;
    let items = get_array_paged(&client, source, token, &format!("/repos/{}/docs", ns)).await?;
    Ok(items
        .iter()
        .filter_map(|v| {
            let slug = str_of(v, "slug");
            if slug.trim().is_empty() {
                return None;
            }
            Some(CloudDoc {
                id: slug,
                title: str_of(v, "title"),
                word_count: int_of(v, "word_count"),
                updated_at: str_of(v, "updated_at"),
            })
        })
        .collect())
}

/// 取一篇文档的正文。
///
/// 正文优先用 `body`（语雀给的就是 markdown）；它为空时退回 `body_html` 本地转 markdown
/// —— 图片 / 附件 / 画板不在开放接口的返回里，这篇只能是纯文本正文。
pub async fn fetch_doc(
    source: CloudSource,
    token: &str,
    namespace: &str,
    doc_id: &str,
) -> Result<CloudDocContent, String> {
    let ns = namespace.trim();
    if ns.is_empty() {
        return Err("请先选择知识库".into());
    }
    let client = client()?;
    let data = get_data(
        &client,
        source,
        token,
        &format!("/repos/{}/docs/{}", ns, doc_id.trim()),
    )
    .await?;

    let title = {
        let t = str_of(&data, "title");
        if t.is_empty() {
            doc_id.trim().to_string()
        } else {
            t
        }
    };
    let mut body = str_of(&data, "body");
    if body.trim().is_empty() {
        let html = str_of(&data, "body_html");
        if !html.trim().is_empty() {
            body = crate::tools::browser::html_to_markdown(&html);
        }
    }
    if body.trim().is_empty() {
        return Err(format!(
            "文档「{}」没有可投喂的正文（空文档，或正文全在图片 / 附件 / 画板里）",
            title
        ));
    }
    Ok(CloudDocContent {
        title,
        body,
        url: format!("https://www.yuque.com/{}/{}", ns, doc_id.trim()),
    })
}
