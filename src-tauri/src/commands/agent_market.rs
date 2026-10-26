//! Agent 市集：把「ACP 官方目录 + 内置预置」归一化成同一份货架，并负责落到本地。
//!
//! 分工：
//! - `agent::registry`：只负责拿到官方清单（缓存/快照）；
//! - `agents-config.json`（经 `utils::market` 拉取）：只放官方目录缺失的预置条目
//!   （ACP 缺的如 hermes，以及兼容模式 CLI 预置）；
//! - 本模块：合并两份来源 → 市场条目；「获取」时落草稿（pending_install）并下载图标；
//!   「安装」时执行安装命令 → 解析 bin / 版本 → 置 ready，并把 ACP 的 conn 同步成解析后的运行时。

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::Emitter;

use crate::agent::config::{AgentConfig, CreateAgentPayload, Readiness, UpdateAgentPayload};
use crate::agent::lifecycle::{
    Artifact, DistSource, LifecycleKind, LifecyclePackage, LifecycleSpec, PackageRole, PkgState,
    RuntimeSpec, SOURCE_ACP_REGISTRY, SOURCE_PRESET,
};
use crate::agent::registry::{self, RegistryAgent};
use crate::commands::agents;
use crate::utils::errors::AppError;

// ──────────────────────────────────────────────
//  对外模型
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketPackage {
    pub role: String,
    pub package: String,
    pub args: Vec<String>,
}

/// 市场货架上的一条 Agent 配置（只含展示与是否已获取的信息）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketAgent {
    pub agent_type: String,
    pub display_name: String,
    pub description: String,
    /// 配置类型：`acp` / `cli`
    pub category: String,
    /// 接入方式：`acp` / `subprocess`
    pub transport: String,
    /// 生命周期类型：`npm` / `shell` / `uvx` / `binary`
    pub kind: String,
    pub version: String,
    /// 图标（远程 URL 或本地 `file:` 引用）
    pub icon: Option<String>,
    /// 来源：`acp-registry` / `preset`
    pub source: String,
    pub repository: Option<String>,
    pub website: Option<String>,
    pub license: Option<String>,
    pub authors: Vec<String>,
    pub packages: Vec<MarketPackage>,
    /// 完整生命周期规格：供集成配置表单在编辑历史配置（库里没有规格）时按同名条目预填，
    /// 与「本机安装」动作使用同一份数据，避免前端另造一套派生命令。
    pub lifecycle: LifecycleSpec,
    /// 当前版本是否支持直接获取（P1 只支持 npm/shell）
    pub supported: bool,
    pub unsupported_reason: Option<String>,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub pkg_status: Option<String>,
    pub update_available: bool,
}

// ──────────────────────────────────────────────
//  内部条目
// ──────────────────────────────────────────────

/// 归一化后的市场条目：展示元信息 + 落草稿所需的完整配置。
#[derive(Debug, Clone)]
struct MarketEntry {
    agent_type: String,
    display_name: String,
    description: String,
    category: String,
    transport: String,
    lifecycle: LifecycleSpec,
    version: String,
    icon: Option<String>,
    icon_url: Option<String>,
    source: String,
    registry_id: Option<String>,
    registry_version: Option<String>,
    repository: Option<String>,
    website: Option<String>,
    license: Option<String>,
    authors: Vec<String>,
    // 预置独有的兼容模式字段（官方 ACP 条目不含）
    preset: Option<PresetAgent>,
}

/// `agents-config.json` 的一条预置。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresetAgent {
    agent_type: String,
    display_name: String,
    #[serde(default)]
    description: String,
    /// `acp` / `cli`（缺省按 transport 推断）
    #[serde(default)]
    category: String,
    #[serde(default)]
    transport: String,
    /// 老版预置（v1）没有生命周期规格：缺字段不能让整份清单解析失败
    #[serde(default)]
    lifecycle: Option<LifecycleSpec>,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    sort_order: Option<i64>,
    #[serde(default)]
    version: String,
    #[serde(default)]
    run_cmd_template: Option<String>,
    #[serde(default)]
    output_parser: Option<String>,
    #[serde(default)]
    output_filter_regex: Option<String>,
    #[serde(default)]
    supports_session_continuity: Option<bool>,
    #[serde(default)]
    session_id_source: Option<String>,
    #[serde(default)]
    session_id_event_type: Option<String>,
    #[serde(default)]
    session_id_field: Option<String>,
    #[serde(default)]
    resume_arg_template: Option<String>,
    #[serde(default)]
    skills_dir: Option<String>,
    #[serde(default)]
    skill_entry_file: Option<String>,
    #[serde(default)]
    skill_display_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresetFile {
    #[serde(default)]
    agents: Vec<PresetAgent>,
}

impl MarketEntry {
    fn from_registry(a: &RegistryAgent) -> Option<Self> {
        // 分发方式优先级：npx → uvx → binary（同一 Agent 可能同时提供多种）
        // binary 分支单独处理：它没有「包名」，而是一组按平台划分的下载制品。
        let mut artifacts: Vec<Artifact> = Vec::new();
        let (kind, package, args, env) = if let Some(npx) = a.distribution.get("npx") {
            (
                LifecycleKind::Npm,
                npx.get("package").and_then(|v| v.as_str())?.to_string(),
                string_list(npx.get("args")),
                string_map(npx.get("env")),
            )
        } else if let Some(uvx) = a.distribution.get("uvx") {
            (
                LifecycleKind::Uvx,
                uvx.get("package").and_then(|v| v.as_str())?.to_string(),
                string_list(uvx.get("args")),
                string_map(uvx.get("env")),
            )
        } else if let Some(binary) = a.distribution.get("binary") {
            // 「平台键 → {archive, cmd, sha256}」：**逐平台原样收录**，装哪个平台由安装时决定。
            // 不在映射阶段随手挑一个平台（JSON 键序不定，可能取到 darwin 的 cmd）。
            artifacts = binary
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(platform, v)| {
                            let archive = v.get("archive")?.as_str()?.trim().to_string();
                            let cmd = v.get("cmd")?.as_str()?.trim().to_string();
                            if archive.is_empty() || cmd.is_empty() {
                                return None;
                            }
                            Some(Artifact {
                                platform: platform.clone(),
                                archive,
                                cmd,
                                sha256: v
                                    .get("sha256")
                                    .and_then(|s| s.as_str())
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty()),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if artifacts.is_empty() {
                // 有条目但一个平台制品都没解析出来（字段缺失/不是对象）：同样跳过，但要留痕
                log::warn!(
                    "[AgentMarket] 跳过 binary 条目：id={} 没有可用的平台制品（archive/cmd 缺失）",
                    a.id
                );
                return None;
            }
            (LifecycleKind::Binary, String::new(), Vec::new(), BTreeMap::new())
        } else {
            // ACP 协议已预留远程传输（Streamable HTTP 仍是草案），官方目录将来可能出现
            // 没有本地可执行物的条目（如新增 `http` / `remote` 分发键）。这类当前无法承载，
            // 只能丢弃；但必须留日志——否则新形态上线后只会表现为「条目凭空消失」。
            let keys = a
                .distribution
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            log::warn!(
                "[AgentMarket] 跳过无法识别的分发方式: id={}, name={}, distribution=[{}]（当前仅支持 npx/uvx/binary 本地可执行物）",
                a.id,
                a.name,
                if keys.is_empty() { "空" } else { keys.as_str() }
            );
            return None;
        };

        let packages = if package.is_empty() {
            Vec::new()
        } else {
            vec![LifecyclePackage {
                role: PackageRole::Adapter,
                package: package.clone(),
                bin: None,
                args: args.clone(),
                env,
            }]
        };

        let spec = LifecycleSpec {
            kind,
            // npm 条目不写死运行时：安装后由 bin 解析决定（解析失败退 npm exec）。
            // uvx 分发确定不做，这里只留可读的运行时形态供展示；binary 的运行时在安装后
            // 由解压出来的绝对路径直接写进 conn。
            runtime: if kind == LifecycleKind::Uvx {
                Some(RuntimeSpec {
                    command: "uvx".to_string(),
                    args: std::iter::once(package).chain(args).collect(),
                })
            } else {
                None
            },
            artifacts,
            packages,
            ..Default::default()
        };

        Some(Self {
            agent_type: a.id.clone(),
            display_name: a.name.clone(),
            description: a.description.clone(),
            category: "acp".to_string(),
            transport: "acp".to_string(),
            lifecycle: spec,
            version: a.version.clone(),
            icon: a.icon.clone(),
            icon_url: a.icon.clone(),
            source: SOURCE_ACP_REGISTRY.to_string(),
            registry_id: Some(a.id.clone()),
            registry_version: Some(a.version.clone()),
            repository: a.repository.clone(),
            website: a.website.clone(),
            license: a.license.clone(),
            authors: a.authors.clone(),
            preset: None,
        })
    }

    fn from_preset(p: PresetAgent) -> Self {
        let transport = if p.transport.trim().is_empty() {
            "subprocess".to_string()
        } else {
            p.transport.clone()
        };
        let category = if p.category.trim().is_empty() {
            if transport == "acp" {
                "acp".to_string()
            } else {
                "cli".to_string()
            }
        } else {
            p.category.clone()
        };
        Self {
            agent_type: p.agent_type.clone(),
            display_name: p.display_name.clone(),
            description: p.description.clone(),
            category,
            transport,
            // 老版预置没有生命周期规格：合成空 shell 规格，市集按「暂不支持自动安装」展示
            lifecycle: p.lifecycle.clone().unwrap_or(LifecycleSpec {
                kind: LifecycleKind::Shell,
                ..Default::default()
            }),
            version: p.version.clone(),
            icon: Some(p.icon.clone()).filter(|s| !s.trim().is_empty()),
            icon_url: p.icon_url.clone(),
            source: SOURCE_PRESET.to_string(),
            registry_id: None,
            registry_version: None,
            repository: None,
            website: None,
            license: None,
            authors: Vec::new(),
            preset: Some(p),
        }
    }

    fn to_market_agent(&self, installed: &InstalledInfo) -> MarketAgent {
        // 没有包名、没有安装命令、也没有平台制品 = 老版预置/只能手工配置，不能给「获取」入口
        let has_install_spec = !self.lifecycle.packages.is_empty()
            || self.lifecycle.install_cmd.is_some()
            || !self.lifecycle.artifacts.is_empty();
        let support = if has_install_spec {
            self.lifecycle.supported()
        } else {
            Err("该预置未提供安装规格，请手工配置".to_string())
        };
        let packages = self
            .lifecycle
            .packages
            .iter()
            .map(|p| MarketPackage {
                role: match p.role {
                    PackageRole::Adapter => "adapter".to_string(),
                    PackageRole::Host => "host".to_string(),
                },
                package: p.package.clone(),
                args: p.args.clone(),
            })
            .collect();
        MarketAgent {
            agent_type: self.agent_type.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            category: self.category.clone(),
            transport: self.transport.clone(),
            kind: self.lifecycle.kind.as_str().to_string(),
            version: self.version.clone(),
            icon: self.icon.clone(),
            source: self.source.clone(),
            repository: self.repository.clone(),
            website: self.website.clone(),
            license: self.license.clone(),
            authors: self.authors.clone(),
            packages,
            lifecycle: self.lifecycle.clone(),
            supported: support.is_ok(),
            unsupported_reason: support.err(),
            installed: installed.exists,
            installed_version: installed.installed_version.clone(),
            pkg_status: installed.pkg_status.clone(),
            // 只有来自 ACP 官方目录的条目才有「市场版本」可比：预置的 `version` 是配置版本号，
            // 拿它和 npm 里的实际包版本比较会产生假的「可更新」。
            update_available: installed.exists
                && self.registry_version.is_some()
                && !self.version.trim().is_empty()
                && installed
                    .installed_version
                    .as_deref()
                    .map(|v| v != self.version)
                    .unwrap_or(false),
        }
    }
}

fn string_list(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn string_map(v: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    v.and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

// ──────────────────────────────────────────────
//  货架组装
// ──────────────────────────────────────────────

/// 已安装信息（从 `agents` 表读）。
struct InstalledInfo {
    exists: bool,
    installed_version: Option<String>,
    pkg_status: Option<String>,
}

fn installed_map(conn: &rusqlite::Connection) -> Result<BTreeMap<String, InstalledInfo>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT agent_type, pkg_state_json, version FROM agents WHERE transport <> 'http'",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (agent_type, pkg_text, version) = row?;
        let state = pkg_text
            .as_deref()
            .and_then(|s| serde_json::from_str::<PkgState>(s).ok());
        let installed_version = state
            .as_ref()
            .and_then(|s| s.installed_version.clone())
            .or_else(|| Some(version).filter(|v| !v.trim().is_empty()));
        out.insert(
            agent_type,
            InstalledInfo {
                exists: true,
                installed_version,
                pkg_status: state.map(|s| s.status),
            },
        );
    }
    Ok(out)
}

/// 读取市场条目。`category` 指定时只加载该市场需要的数据源：
/// - `cli`：只读预置，**不触碰** ACP 官方目录（官方目录超时不会拖垮本地 CLI 市场）；
/// - `acp`：读官方目录，预置只用于补「官方未收录」的条目，预置失败不致命。
async fn load_entries(category: Option<&str>) -> Result<Vec<MarketEntry>, AppError> {
    let want_cli = category == Some("cli");
    let want_acp = !want_cli;

    let mut entries: Vec<MarketEntry> = Vec::new();

    if want_acp {
        let registry = registry::load(false)
            .await
            .map_err(AppError::Network)?;
        entries.extend(
            registry
                .agents
                .iter()
                .filter_map(MarketEntry::from_registry),
        );
    }

    match crate::utils::market::fetch_agents_config().await {
        Ok(preset_json) => {
            let preset_file = serde_json::from_value::<PresetFile>(preset_json)
                .map_err(|e| AppError::Json(format!("解析 Agent 预置失败: {}", e)))?;

            let existing: std::collections::HashSet<String> =
                entries.iter().map(|e| e.agent_type.clone()).collect();
            for p in preset_file.agents {
                // 官方目录已有的条目以官方为准（预置只补缺口）
                if existing.contains(&p.agent_type) {
                    continue;
                }
                let entry = MarketEntry::from_preset(p);
                if category.map_or(true, |c| entry.category == c) {
                    entries.push(entry);
                }
            }
        }
        Err(e) => {
            // CLI 市场完全依赖预置，读不到只能报错；ACP 市场仍可只显示官方目录。
            if want_cli {
                return Err(AppError::Network(e));
            }
            log::warn!("[agent_market] 预置读取失败，仅展示 ACP 官方目录: {}", e);
        }
    }

    entries.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()))
    });
    Ok(entries)
}

// ──────────────────────────────────────────────
//  命令
// ──────────────────────────────────────────────

/// 列出 Agent 市集（`force=true` 跳过缓存、直连官方目录刷新）。
///
/// `category` 用于把两处市场隔离：`cli` 完全跳过官方目录，`acp` 只拉官方目录
/// （预置仅补缺口），这样一侧网络超时不会连带另一侧空白。
#[tauri::command]
pub async fn agent_market_list(
    state: tauri::State<'_, crate::DbState>,
    force: Option<bool>,
    category: Option<String>,
) -> Result<Vec<MarketAgent>, AppError> {
    let cat = category.as_deref();
    if force.unwrap_or(false) && cat != Some("cli") {
        registry::load(true).await.map_err(AppError::Network)?;
    }
    let entries = load_entries(cat).await?;

    let conn = state.get_conn()?;
    let installed = installed_map(&conn)?;
    Ok(entries
        .iter()
        .map(|e| {
            e.to_market_agent(
                installed
                    .get(&e.agent_type)
                    .unwrap_or(&InstalledInfo {
                        exists: false,
                        installed_version: None,
                        pkg_status: None,
                    }),
            )
        })
        .collect())
}

/// 「获取」：落一条草稿（`pending_install`），下载图标到本地，随后由前端跳转配置页安装。
#[tauri::command]
pub async fn agent_market_acquire(
    state: tauri::State<'_, crate::DbState>,
    agent_type: String,
) -> Result<AgentConfig, AppError> {
    {
        let conn = state.get_conn()?;
        if agents::get_agent_inner(&conn, &agent_type)?.is_some() {
            return Err(AppError::InvalidInput(format!(
                "Agent「{}」已存在，请在集成配置里直接管理",
                agent_type
            )));
        }
    }

    let entries = load_entries(None).await?;
    let entry = entries
        .into_iter()
        .find(|e| e.agent_type == agent_type)
        .ok_or_else(|| AppError::NotFound(format!("市场中未找到 Agent「{}」", agent_type)))?;

    entry.lifecycle.supported().map_err(AppError::Config)?;

    let now_ts = crate::utils::now();
    let icon = match (&entry.icon_url, &entry.icon) {
        // 预置自带的本地图标直接沿用（与 CLI 的存储约定一致）
        (None, Some(local)) if local.starts_with("file:") => Some(local.clone()),
        (Some(url), _) => download_icon(&entry.agent_type, url).await.or(entry.icon.clone()),
        (None, other) => other.clone(),
    };

    // ACP 的启动命令：npm 先用 npx 形态兜底，安装完成后由 `agent_market_install` 换成解析后的 bin。
    let conn_json = if entry.transport == "acp" {
        entry
            .lifecycle
            .resolved_runtime(&BTreeMap::new())
            .map(|rt| json!({ "command": rt.command, "args": rt.args, "env": entry.lifecycle.merged_env() }))
    } else {
        None
    };

    let preset = entry.preset.clone();
    let payload = CreateAgentPayload {
        agent_type: entry.agent_type.clone(),
        display_name: entry.display_name.clone(),
        description: Some(entry.description.clone()),
        lifecycle: Some(entry.lifecycle.clone()),
        pkg_state: Some(PkgState::pending(now_ts)),
        dist_source: Some(DistSource {
            source: entry.source.clone(),
            registry_id: entry.registry_id.clone(),
            registry_version: entry.registry_version.clone(),
            icon_url: entry.icon_url.clone(),
            installed_at: now_ts,
        }),
        run_cmd_template: preset.as_ref().and_then(|p| p.run_cmd_template.clone()),
        output_parser: preset.as_ref().and_then(|p| p.output_parser.clone()),
        output_filter_regex: preset.as_ref().and_then(|p| p.output_filter_regex.clone()),
        supports_session_continuity: preset.as_ref().and_then(|p| p.supports_session_continuity),
        session_id_source: preset.as_ref().and_then(|p| p.session_id_source.clone()),
        session_id_event_type: preset.as_ref().and_then(|p| p.session_id_event_type.clone()),
        session_id_field: preset.as_ref().and_then(|p| p.session_id_field.clone()),
        resume_arg_template: preset.as_ref().and_then(|p| p.resume_arg_template.clone()),
        skills_dir: preset.as_ref().and_then(|p| p.skills_dir.clone()),
        skill_entry_file: preset.as_ref().and_then(|p| p.skill_entry_file.clone()),
        skill_display_mode: preset.as_ref().and_then(|p| p.skill_display_mode.clone()),
        color: preset.as_ref().and_then(|p| p.color.clone()),
        icon,
        sort_order: preset.as_ref().and_then(|p| p.sort_order),
        is_enabled: Some(true),
        version: Some(entry.version.clone()),
    };

    let conn = state.get_conn()?;
    let created = agents::add_agent_inner(&conn, payload)?;
    agents::set_agent_transport_inner(
        &conn,
        &entry.agent_type,
        &entry.transport,
        conn_json.as_ref().map(|v| v.to_string()).as_deref(),
    )?;
    Ok(created)
}

// ──────────────────────────────────────────────
//  二进制安装（P3 受限子集：当前平台 + zip + sha256）
// ──────────────────────────────────────────────

/// 安装包体积上限（防大文件与超长响应把内存吃满）
const ARTIFACT_MAX_BYTES: usize = 512 * 1024 * 1024;
/// 解压后总体积上限（防 zip 炸弹）
const ARTIFACT_MAX_EXTRACT_BYTES: u64 = 1024 * 1024 * 1024;

/// 下载并安装二进制制品：下载 → sha256 校验 → 解压 → 回写启动绝对路径。
///
/// 与 npm/shell 的区别：没有包管理器可代劳，落盘由应用自己完成。
/// 目录固定 `<app_root>/agents/<agent_type>/<version>/`；卸载删整棵 `<agent_type>`。
pub(crate) async fn install_binary(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, crate::DbState>,
    agent_type: &str,
    display_name: &str,
    spec: &LifecycleSpec,
    version: Option<&str>,
    allow_unverified: bool,
) -> Result<(), AppError> {
    let result =
        install_binary_inner(app, state, agent_type, display_name, spec, version, allow_unverified)
            .await;
    if let Err(err) = &result {
        let msg = format!("安装失败: {}", err);
        let _ = set_pkg_state(state, agent_type, PkgState::failed(msg.clone(), crate::utils::now()));
        let _ = app.emit(
            "install-progress",
            json!({
                "agent": agent_type,
                "message": msg,
                "progress": 100,
                "error": true
            }),
        );
    }
    result
}

async fn install_binary_inner(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, crate::DbState>,
    agent_type: &str,
    display_name: &str,
    spec: &LifecycleSpec,
    version: Option<&str>,
    allow_unverified: bool,
) -> Result<(), AppError> {
    let artifact = spec
        .current_artifact()
        .ok_or_else(|| AppError::Config(spec.binary_block_reason()))?;

    let root = crate::agent::lifecycle::binary_install_root(agent_type);
    let tag = sanitize_version_tag(version.unwrap_or(""));
    let target = root.join(&tag);

    let _ = app.emit(
        "install-progress",
        json!({
            "agent": agent_type,
            "message": format!("正在下载 {}...", display_name),
            "progress": 15
        }),
    );
    let bytes = download_bytes(&artifact.archive).await?;

    let _ = app.emit(
        "install-progress",
        json!({ "agent": agent_type, "message": "正在校验安装包...", "progress": 55 }),
    );
    // 完整性来源优先级：registry 的 sha256 → 厂商随包发布的 .sha256 / checksums.txt → 用户显式确认后不校验
    let published = artifact.sha256.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (expected, integrity) = match published {
        Some(sha) => (Some(sha.to_string()), "sha256"),
        None => match fetch_published_checksum(&artifact.archive).await {
            Some((sha, source)) => (Some(sha), source),
            None => (None, "none"),
        },
    };
    match expected.as_deref() {
        Some(sha) => verify_sha256(&bytes, sha)?,
        None if allow_unverified => {
            log::warn!(
                "[AgentMarket] 未校验安装包完整性（用户已显式确认）: {}",
                artifact.archive
            );
        }
        None => {
            return Err(AppError::Config(format!(
                "『{}』的安装包没有校验值，也没找到厂商校验文件。确认要安装，请在确认弹窗中选择「仍然安装」。",
                display_name
            )))
        }
    }

    let _ = app.emit(
        "install-progress",
        json!({ "agent": agent_type, "message": "正在解压安装包...", "progress": 75 }),
    );
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| AppError::External(format!("清理旧版本目录失败: {}", e)))?;
    }
    std::fs::create_dir_all(&target)
        .map_err(|e| AppError::External(format!("创建安装目录失败: {}", e)))?;
    extract_zip(&bytes, &target)?;

    let exe = resolve_artifact_cmd(&target, &artifact.cmd).ok_or_else(|| {
        AppError::External(format!(
            "安装包内未找到启动文件：{}（请按官方文档手工安装）",
            artifact.cmd
        ))
    })?;
    prune_other_versions(&root, &tag);

    let mut ready = PkgState::ready(crate::utils::now());
    ready.install_dir = Some(target.to_string_lossy().to_string());
    ready.integrity = Some(integrity.to_string());
    ready.installed_version = version
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    set_pkg_state(state, agent_type, ready)?;

    // ACP：启动命令换成解压出来的绝对路径（检测与门禁按文件存在性判定）
    {
        let conn = state.get_conn()?;
        let info = agents::get_agent_transport_inner(&conn, agent_type)?;
        if info.transport == "acp" {
            // 保留用户陆续加过的启动参数：更新时只换可执行文件，不重置手写配置
            let existing_args: Vec<String> = info
                .conn
                .as_deref()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                .and_then(|v| v.get("args").and_then(|a| a.as_array()).cloned())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let conn_json = json!({
                "command": exe.to_string_lossy(),
                "args": existing_args,
                "env": spec.merged_env(),
            });
            agents::set_agent_transport_inner(
                &conn,
                agent_type,
                "acp",
                Some(&conn_json.to_string()),
            )?;
        }
    }

    crate::commands::env::clear_env_cache();
    let _ = app.emit(
        "install-progress",
        json!({
            "agent": agent_type,
            "message": format!("{} 已就绪", display_name),
            "progress": 100
        }),
    );
    Ok(())
}

/// 卸载二进制：删整棵安装目录并把包状态退回「待安装」。
pub(crate) fn uninstall_binary(
    state: &tauri::State<'_, crate::DbState>,
    agent_type: &str,
) -> Result<(), AppError> {
    let root = crate::agent::lifecycle::binary_install_root(agent_type);
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .map_err(|e| AppError::External(format!("删除安装目录失败: {}", e)))?;
    }
    let conn = state.get_conn()?;
    set_pkg_state_inner(&conn, agent_type, &PkgState::pending(crate::utils::now()))?;
    crate::commands::env::clear_env_cache();
    Ok(())
}

/// 从发布源补校验值：先试同名 `<archive>.sha256`，再试同目录 `checksums.txt`。
///
/// 官方目录偶有漏录 sha256（如 VTCode 的 release 里其实带了 `.sha256`），
/// 这一步只是把厂商已经提供的信息取回来，不降低门槛。
/// 返回 (sha256, 来源标记)，来源会记进 `pkgState.integrity` 供排查。
async fn fetch_published_checksum(archive_url: &str) -> Option<(String, &'static str)> {
    let file_name = archive_url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())?
        .to_string();

    // ① 同名 .sha256（内容可能是裸哈希，也可能是 `hash  filename`）
    if let Some(text) = fetch_small_text(&format!("{}.sha256", archive_url)).await {
        if let Some(sha) = extract_sha256(&text) {
            return Some((sha, "sha256-file"));
        }
    }

    // ② 同目录 checksums.txt（按文件名匹配行）
    let dir = archive_url
        .rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or_default();
    for candidate in ["checksums.txt", "CHECKSUMS.txt", "sha256sums.txt"] {
        if let Some(text) = fetch_small_text(&format!("{}/{}", dir, candidate)).await {
            if let Some(sha) = find_sha_for_file(&text, &file_name) {
                return Some((sha, "checksums-txt"));
            }
        }
    }
    None
}

/// 拉一个小文本文件（校验文件通常 <1KB）；失败或过大一律返回 None，不影响安装主流程。
async fn fetch_small_text(url: &str) -> Option<String> {
    const MAX_CHARS: usize = 64 * 1024;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("PilotDesk-AgentMarket/1.0")
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    if resp.content_length().unwrap_or(0) > MAX_CHARS as u64 {
        return None;
    }
    let text = resp.text().await.ok()?;
    if text.len() > MAX_CHARS {
        return None;
    }
    Some(text)
}

/// 取文本里第一段 64 位十六进制（兼容裸哈希与 `sha256sum` 输出）。
fn extract_sha256(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_hexdigit())
        .find(|token| token.len() == 64)
        .map(|s| s.to_ascii_lowercase())
}

/// 在 `checksums.txt` 里按文件名找对应行的哈希。
fn find_sha_for_file(text: &str, file_name: &str) -> Option<String> {
    let needle = file_name.to_ascii_lowercase();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !trimmed.to_ascii_lowercase().contains(&needle) {
            continue;
        }
        if let Some(sha) = extract_sha256(trimmed) {
            return Some(sha);
        }
    }
    None
}

/// 流式下载并有体积上限：不要先 `bytes()` 再判大小（恶意超大会先撑爆内存）。
///
/// 与市场清单不同，这里**走系统代理**：发布包多托管在 GitHub / 厂商 CDN，
/// 国内网络常需要代理才能连通，硬编码 `no_proxy` 会让用户完全装不上。
async fn download_bytes(url: &str) -> Result<Vec<u8>, AppError> {
    use futures::StreamExt;

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(600))
        .user_agent("PilotDesk-AgentMarket/1.0")
        .build()
        .map_err(|e| AppError::External(format!("创建下载客户端失败: {}", e)))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| {
            AppError::External(format!(
                "下载安装包失败（网络或代理不可用）：{}。可改用官方文档手工安装",
                e
            ))
        })?;
    if !resp.status().is_success() {
        return Err(AppError::External(format!(
            "下载安装包失败: HTTP {}（可改用官方文档手工安装）",
            resp.status().as_u16()
        )));
    }

    let mut stream = resp.bytes_stream();
    let mut out: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AppError::External(format!("读取安装包失败: {}", e)))?;
        if out.len() + chunk.len() > ARTIFACT_MAX_BYTES {
            return Err(AppError::External(format!(
                "安装包超过 {} MB 上限，已中止",
                ARTIFACT_MAX_BYTES / 1024 / 1024
            )));
        }
        out.extend_from_slice(&chunk);
    }
    if out.is_empty() {
        return Err(AppError::External("下载到的安装包为空".to_string()));
    }
    Ok(out)
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), AppError> {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.is_empty() {
        return Err(AppError::Config(
            "安装包缺少校验值（sha256），拒绝安装".to_string(),
        ));
    }
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected {
        let short = |s: &str| s.chars().take(12).collect::<String>();
        return Err(AppError::External(format!(
            "安装包校验失败（期望 {}…，实际 {}…），已中止",
            short(&expected),
            short(&actual)
        )));
    }
    Ok(())
}

/// 解压 zip 到目标目录。只接受归档内的相对路径（zip slip 防护）。
fn extract_zip(bytes: &[u8], dest: &Path) -> Result<(), AppError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| AppError::External(format!("安装包不是有效的 zip: {}", e)))?;
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::External(format!("读取安装包失败: {}", e)))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(AppError::External(format!(
                "安装包含有非法路径：{}",
                entry.name()
            )));
        };
        if entry.is_dir() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > ARTIFACT_MAX_EXTRACT_BYTES {
            return Err(AppError::External("安装包解压后体积过大，已中止".to_string()));
        }
        let out_path = dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::External(format!("创建目录失败: {}", e)))?;
        }
        let mut out = std::fs::File::create(&out_path)
            .map_err(|e| AppError::External(format!("写入文件失败: {}", e)))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| AppError::External(format!("解压失败: {}", e)))?;
    }
    Ok(())
}

/// 归档内 `cmd` → 本机文件路径。声明路径与实际大小写/层级不符时按文件名兜底找一次。
fn resolve_artifact_cmd(dir: &Path, cmd: &str) -> Option<PathBuf> {
    let normalized = cmd.replace('\\', "/");
    let trimmed = normalized.trim_start_matches("./").trim_start_matches('/');
    let direct = dir.join(trimmed);
    if direct.is_file() {
        return Some(direct);
    }
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    find_file_recursive(dir, name, 0)
}

fn find_file_recursive(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, name, depth + 1) {
                return Some(found);
            }
        } else if path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case(name))
            .unwrap_or(false)
        {
            return Some(path);
        }
    }
    None
}

/// 版本目录名：去掉路径分隔符等不安全字符；空版本落到 `latest`。
fn sanitize_version_tag(version: &str) -> String {
    let cleaned: String = version
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "latest".to_string()
    } else {
        cleaned
    }
}

/// 更新时只保留当前版本目录，避免旧版本一直占磁盘。
fn prune_other_versions(root: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && entry.file_name().to_string_lossy() != keep {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// 安装执行体（`agent_market_install` 与 `agent_market_update` 共用）：
/// 执行安装命令 → 解析 bin / 已装版本 → 置 `ready` → 回写 ACP 的启动命令。
///
/// 失败时**落 `failed` 状态并保留错误原因**（前端据此展示重试），同时把错误返回给调用方。
async fn run_install(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, crate::DbState>,
    agent_mgr: &tauri::State<'_, crate::AsyncMutex<crate::agent::AgentManager>>,
    agent_type: &str,
    display_name: &str,
    spec: &LifecycleSpec,
    version: Option<&str>,
    allow_unverified: bool,
) -> Result<(), AppError> {
    // 二进制没有包管理器可代劳：下载/校验/解压由应用自己做，不走 shell
    if spec.kind == LifecycleKind::Binary {
        return install_binary(
            app,
            state,
            agent_type,
            display_name,
            spec,
            version,
            allow_unverified,
        )
        .await;
    }

    let install_cmd = spec
        .commands()
        .map_err(AppError::Config)?
        .install
        .ok_or_else(|| AppError::Config(format!("『{}』未配置安装命令", agent_type)))?;

    let _ = app.emit(
        "install-progress",
        json!({
            "agent": agent_type,
            "message": format!("正在安装 {}...", display_name),
            "progress": 20
        }),
    );

    let mgr = agent_mgr.lock().await;
    let result = mgr
        .execute_command_output_async(&install_cmd, "", 600, &format!("安装 {}", agent_type))
        .await;

    if let Err(e) = result {
        drop(mgr);
        let msg = format!("安装失败: {}", e);
        set_pkg_state(state, agent_type, PkgState::failed(msg.clone(), crate::utils::now()))?;
        let _ = app.emit(
            "install-progress",
            json!({
                "agent": agent_type,
                "message": msg,
                "progress": 100,
                "error": true
            }),
        );
        return Err(AppError::External(msg));
    }

    let _ = app.emit(
        "install-progress",
        json!({
            "agent": agent_type,
            "message": format!("{} 安装完成，正在校验...", display_name),
            "progress": 80
        }),
    );

    // bin 解析：npm 走 `npm view <pkg> bin --json`；解析失败不判安装失败（运行时退 npm exec）
    let bins = match spec.bin_probe_command() {
        Some(cmd) => mgr
            .execute_command_output_async(&cmd, "", 60, "解析可执行名")
            .await
            .ok()
            .map(|out| parse_bin_map(spec, &out))
            .unwrap_or_default(),
        None => BTreeMap::new(),
    };

    let installed_version = match spec.commands().map_err(AppError::Config)?.version_probe {
        Some(cmd) => mgr
            .execute_command_output_async(&cmd, "", 120, "读取已安装版本")
            .await
            .ok()
            .and_then(|out| spec.parse_installed_version(&out)),
        None => None,
    };

    drop(mgr);

    // 版本核对：包管理器对不可达 / 不存在的目标版本可能以 0 退出却什么都不改。
    // 不核对就会出现「点了安装/更新、提示成功、版本其实没变」。
    if let (Some(expected), Some(actual)) = (spec.pinned_version(), installed_version.as_deref()) {
        let norm = |v: &str| v.trim().trim_start_matches(['v', 'V']).to_string();
        if norm(&expected) != norm(actual) {
            let msg = format!(
                "安装未生效：期望 v{}，本机实际 v{}（请检查网络 / npm 镜像，或按官方文档手动安装）",
                expected, actual
            );
            set_pkg_state(
                state,
                agent_type,
                PkgState::failed(msg.clone(), crate::utils::now()),
            )?;
            let _ = app.emit(
                "install-progress",
                json!({
                    "agent": agent_type,
                    "message": msg,
                    "progress": 100,
                    "error": true
                }),
            );
            return Err(AppError::External(msg));
        }
    }

    let mut ready = PkgState::ready(crate::utils::now());
    ready.bins = bins.clone();
    ready.installed_version = installed_version.clone();
    set_pkg_state(state, agent_type, ready)?;

    // ACP：把 conn 的启动命令换成解析后的运行时（有 bin 用 bin，否则 npm exec 兜底）
    {
        let conn = state.get_conn()?;
        let info = agents::get_agent_transport_inner(&conn, agent_type)?;
        if info.transport == "acp" {
            if let Some(rt) = spec.resolved_runtime(&bins) {
                let conn_json = json!({
                    "command": rt.command,
                    "args": rt.args,
                    "env": spec.merged_env(),
                });
                agents::set_agent_transport_inner(
                    &conn,
                    agent_type,
                    "acp",
                    Some(&conn_json.to_string()),
                )?;
            }
        }
    }

    crate::commands::env::clear_env_cache();
    let _ = app.emit(
        "install-progress",
        json!({
            "agent": agent_type,
            "message": format!("{} 已就绪", display_name),
            "progress": 100
        }),
    );

    Ok(())
}

/// 历史手工创建的 Agent 没有生命周期规格：按**同名预置**接管安装/卸载命令。
///
/// 只写回「安装规格」（lifecycle / pkg_state / dist_source），不动用户配的运行命令、
/// 会话参数与展示信息 —— 把这些配置接管过来，市集与环境检测页才能对它做安装/更新/卸载。
/// 返回接管后的规格；市集里没有同名可安装预置时返回 `None`。
pub(crate) async fn adopt_preset_spec(
    state: &tauri::State<'_, crate::DbState>,
    agent_type: &str,
    transport: &str,
) -> Result<Option<LifecycleSpec>, AppError> {
    // 按本地接入方式取对应市场的预置：同名 agentType 可能同时存在 ACP 与 CLI 两条，
    // 取错就会把 ACP 适配器规格写进本地 CLI 配置（反之亦然）。
    let category = if transport == "acp" { "acp" } else { "cli" };
    let entries = load_entries(Some(category)).await?;
    let Some(entry) = entries
        .into_iter()
        .find(|e| e.agent_type == agent_type && e.source == SOURCE_PRESET)
    else {
        return Ok(None);
    };
    // 老版预置没有安装规格，无从接管
    if entry.lifecycle.packages.is_empty() && entry.lifecycle.install_cmd.is_none() {
        return Ok(None);
    }
    entry.lifecycle.supported().map_err(AppError::Config)?;

    let now_ts = crate::utils::now();
    {
        let conn = state.get_conn()?;
        agents::update_agent_inner(
            &conn,
            UpdateAgentPayload {
                agent_type: agent_type.to_string(),
                lifecycle: Some(entry.lifecycle.clone()),
                pkg_state: Some(PkgState::pending(now_ts)),
                dist_source: Some(DistSource {
                    source: entry.source.clone(),
                    registry_id: None,
                    registry_version: None,
                    icon_url: entry.icon_url.clone(),
                    installed_at: now_ts,
                }),
                ..Default::default()
            },
        )?;
    }
    Ok(Some(entry.lifecycle))
}

/// 「安装」：执行安装命令，随后解析 bin / 已安装版本并把包状态置为 `ready`。
///
/// `allow_unverified`：安装包既无 registry 校验值、又无厂商校验文件时，
/// 用户是否已显式确认「仍然安装」（不校验完整性）。
#[tauri::command]
pub async fn agent_market_install(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::DbState>,
    agent_mgr: tauri::State<'_, crate::AsyncMutex<crate::agent::AgentManager>>,
    agent_type: String,
    allow_unverified: Option<bool>,
) -> Result<AgentConfig, AppError> {
    let (config, transport) = {
        let conn = state.get_conn()?;
        let config = agents::get_agent_inner(&conn, &agent_type)?
            .ok_or_else(|| AppError::NotFound(format!("Agent 类型『{}』不存在", agent_type)))?;
        // 接管要按本地接入方式取同名预置（同名可能同时存在 ACP 与 CLI 两条）
        let transport = agents::get_agent_transport_inner(&conn, &agent_type)?.transport;
        (config, transport)
    };
    // 历史手工配置没有规格时按同名预置接管（先写回本地再安装）
    let spec = match config.lifecycle.clone() {
        Some(spec) => spec,
        None => adopt_preset_spec(&state, &agent_type, &transport).await?.ok_or_else(|| {
            AppError::Config(format!(
                "『{}』没有生命周期规格，市集中也没有同名可安装预置",
                agent_type
            ))
        })?,
    };
    spec.supported().map_err(AppError::Config)?;

    run_install(
        &app,
        &state,
        &agent_mgr,
        &agent_type,
        &config.display_name,
        &spec,
        Some(config.version.as_str()).filter(|v| !v.trim().is_empty()),
        allow_unverified.unwrap_or(false),
    )
    .await?;

    let conn = state.get_conn()?;
    agents::get_agent_inner(&conn, &agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型『{}』不存在", agent_type)))
}

/// 「更新」：用市场里的最新规格覆盖本地配置后再安装（仅 ACP 官方目录条目）。
///
/// 安装命令里的包名带钉死版本，所以更新必须先写回新版本规格，再走同一套安装/校验流程。
#[tauri::command]
pub async fn agent_market_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::DbState>,
    agent_mgr: tauri::State<'_, crate::AsyncMutex<crate::agent::AgentManager>>,
    agent_type: String,
    allow_unverified: Option<bool>,
) -> Result<AgentConfig, AppError> {
    let (config, transport) = {
        let conn = state.get_conn()?;
        let config = agents::get_agent_inner(&conn, &agent_type)?
            .ok_or_else(|| AppError::NotFound(format!("Agent 类型『{}』不存在", agent_type)))?;
        let transport = agents::get_agent_transport_inner(&conn, &agent_type)?.transport;
        (config, transport)
    };
    let entries = load_entries(None).await?;
    // 同名可能同时存在 ACP 与 CLI 两条：优先取与本地接入方式一致的条目，
    // 否则本地 CLI 配置可能被 ACP 适配器规格覆盖。
    let mut candidates: Vec<MarketEntry> = entries
        .into_iter()
        .filter(|e| e.agent_type == agent_type)
        .collect();
    let entry = match candidates.iter().position(|e| e.transport == transport) {
        Some(pos) => candidates.remove(pos),
        None => candidates.into_iter().next().ok_or_else(|| {
            AppError::NotFound(format!("市场中未找到 Agent「{}」", agent_type))
        })?,
    };
    if entry.registry_version.is_none() && entry.source != SOURCE_PRESET {
        return Err(AppError::InvalidInput(
            "该条目没有市场版本信息，无需更新".to_string(),
        ));
    }
    let is_registry = entry.registry_version.is_some();
    entry.lifecycle.supported().map_err(AppError::Config)?;

    let now_ts = crate::utils::now();
    {
        let conn = state.get_conn()?;
        agents::update_agent_inner(
            &conn,
            UpdateAgentPayload {
                agent_type: agent_type.clone(),
                display_name: None,
                description: None,
                lifecycle: Some(entry.lifecycle.clone()),
                pkg_state: Some(PkgState::pending(now_ts)),
                dist_source: Some(DistSource {
                    source: entry.source.clone(),
                    registry_id: entry.registry_id.clone(),
                    registry_version: entry.registry_version.clone(),
                    icon_url: entry.icon_url.clone(),
                    installed_at: now_ts,
                }),
                run_cmd_template: None,
                output_parser: None,
                output_filter_regex: None,
                supports_session_continuity: None,
                session_id_source: None,
                session_id_event_type: None,
                session_id_field: None,
                resume_arg_template: None,
                skills_dir: None,
                skill_entry_file: None,
                skill_display_mode: None,
                color: None,
                icon: None,
                sort_order: None,
                is_enabled: None,
                // 预置条目没有市场版本号，别用它覆盖用户本地记录的版本
                version: if is_registry {
                    Some(entry.version.clone())
                } else {
                    None
                },
            },
        )?;
    }

    run_install(
        &app,
        &state,
        &agent_mgr,
        &agent_type,
        &config.display_name,
        &entry.lifecycle,
        Some(entry.version.as_str()).filter(|v| !v.trim().is_empty()),
        allow_unverified.unwrap_or(false),
    )
    .await?;

    let conn = state.get_conn()?;
    agents::get_agent_inner(&conn, &agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型『{}』不存在", agent_type)))
}

/// 删除草稿/配置（市场页「已获取但未安装」或安装失败时可直接移除）。
#[tauri::command]
pub fn agent_market_remove(
    state: tauri::State<'_, crate::DbState>,
    agent_type: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    agents::delete_agent_inner(&conn, &agent_type)
}

fn set_pkg_state(
    state: &tauri::State<'_, crate::DbState>,
    agent_type: &str,
    pkg_state: PkgState,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    set_pkg_state_inner(&conn, agent_type, &pkg_state)
}

/// 落库实现（供环境管理页复用：卸载后需要把状态退回未安装）。
pub(crate) fn set_pkg_state_inner(
    conn: &rusqlite::Connection,
    agent_type: &str,
    pkg_state: &PkgState,
) -> Result<(), AppError> {
    let json = serde_json::to_string(pkg_state)
        .map_err(|e| AppError::Json(format!("序列化包状态失败: {}", e)))?;
    conn.execute(
        "UPDATE agents SET pkg_state_json = ?1, updated_at = ?2 WHERE agent_type = ?3",
        rusqlite::params![json, crate::utils::now(), agent_type],
    )?;
    Ok(())
}

/// 解析 `npm view <pkg> bin --json` 的输出为「包名 → 可执行名」。
///
/// 返回可能是字符串（单个 bin，命令名即包名）或对象（命令名 → 脚本路径）。
fn parse_bin_map(spec: &LifecycleSpec, output: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(pkg) = spec.primary_package() else {
        return out;
    };
    let key = crate::agent::lifecycle::package_without_version(&pkg.package);
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return out;
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::String(_)) => {
            // 单个 bin：npm 以包名（去 scope 后的最后一段）作为命令名
            if let Some(name) = key.rsplit('/').next().map(|s| s.to_string()) {
                out.insert(key, name);
            }
        }
        Ok(v) => {
            if let Some(o) = v.as_object() {
                if let Some(first) = o.keys().next() {
                    out.insert(key, first.clone());
                }
            }
        }
        Err(_) => {}
    }
    out
}

/// 下载图标到本地图标目录（与 CLI 图标同一套存储约定），返回 `file:<name>` 引用。
async fn download_icon(agent_type: &str, url: &str) -> Option<String> {
    let ext = url
        .rsplit('.')
        .next()
        .map(|s| s.to_lowercase())
        .filter(|s| matches!(s.as_str(), "svg" | "png" | "ico" | "jpg" | "jpeg" | "webp"))
        .unwrap_or_else(|| "svg".to_string());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() {
        return None;
    }

    let name = format!("{}_icon.{}", agent_type, ext);
    let dir = crate::utils::paths::agent_icons_dir();
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join(&name), &bytes).ok()?;
    Some(format!("file:{}", name))
}

/// 供会话门禁复用的就绪检查：未就绪时返回可直接展示给用户的文案。
///
/// 三道关卡，口径与环境检测一致，避免「检测显示可用、会话却起不来」：
/// 1. 包状态未就绪（草稿未装完 / 安装失败）直接拦；
/// 2. 有最新检测结果时直接按它放行/拦截——ACP 的可用性要真跑一遍协议握手
///    （copilot 未登录时 `session/new` 会回 `Authentication required`），
///    不能在每次建会话/发消息时重做；
/// 3. `deep` 时（建会话）没有缓存就先做一次完整检测并写入缓存，
///    之后同一会话内的消息只走缓存或轻量命令探测。
pub async fn readiness_gate(
    agent_mgr: &crate::agent::AgentManager,
    config: &AgentConfig,
    transport: &str,
    conn_json: Option<&str>,
    deep: bool,
) -> Option<String> {
    let hint = "请到「设置 › 运行与策略 › 环境检测」处理后再开始会话。";
    match config.readiness() {
        Readiness::Ready => {}
        Readiness::NotReady { message } => {
            return Some(format!(
                "{}（Agent：{}）。请到「设置 › Agent」完成安装后再开始会话。",
                message, config.agent_type
            ));
        }
    }

    let unavailable = |reason: Option<String>| {
        Some(format!(
            "Agent「{}」当前不可用：{}。{}",
            config.agent_type,
            reason.unwrap_or_else(|| "启动命令不可用".to_string()),
            hint
        ))
    };

    let cached = crate::commands::env::cached_agent_health(&config.agent_type);
    if let Some(health) = &cached {
        // 建会话时对 ACP 现场复核一次：登录态/运行时可能刚变（也可能刚失效），
        // 缓存只能省「发消息」的检测成本，不能当作建会话的通行证。
        if health.available && !(deep && transport == "acp") {
            return None;
        }
        // 消息路径直接按旧结论拦（便宜）
        if !health.available && !deep {
            crate::commands::env::clear_env_cache();
            return unavailable(health.reason.clone());
        }
    }

    // 需要真实判定时做一次完整检测并写缓存：
    // - 建会话（deep）；
    // - ACP 且还没有任何检测结论——命令在 PATH 上不等于能干活（未登录时握手才暴露），
    //   没探测过的 ACP 一律先探再放行，不能靠轻量命令检查放过去。
    let need_probe = if transport == "acp" {
        deep || cached.is_none()
    } else {
        deep && cached.is_some()
    };
    if need_probe {
        let probe =
            crate::commands::env::probe_agent(agent_mgr, config, transport, conn_json).await;
        crate::commands::env::remember_agent_health(&config.agent_type, &probe.health);
        if !probe.health.available {
            crate::commands::env::clear_env_cache();
            return unavailable(probe.health.reason);
        }
        return None;
    }

    let Some(cmd) = config.launch_command(transport, conn_json) else {
        return Some(format!(
            "Agent「{}」缺少可用的启动命令，无法开始会话。{}",
            config.agent_type, hint
        ));
    };

    // npm/npx 兜底命令：包没装上时 `where npm` 一样成功，不能据此放行
    let bins_empty = config
        .pkg_state
        .as_ref()
        .map(|s| s.bins.is_empty())
        .unwrap_or(true);
    if matches!(cmd.as_str(), "npm" | "npm.cmd" | "npx" | "npx.cmd") && bins_empty {
        return Some(format!(
            "Agent「{}」的适配器尚未安装完成，无法开始会话。{}",
            config.agent_type, hint
        ));
    }

    // 命令名查 PATH、路径形态查文件（下载安装的 runtime 是绝对路径，`where` 探不了）
    if crate::commands::env::probe_launch_command(agent_mgr, &cmd).await {
        None
    } else {
        Some(format!(
            "Agent「{}」的启动命令「{}」在本机不可用，无法开始会话。{}",
            config.agent_type, cmd, hint
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_map_parses_object_and_string_forms() {
        let spec = LifecycleSpec {
            kind: LifecycleKind::Npm,
            packages: vec![LifecyclePackage {
                role: PackageRole::Adapter,
                package: "@agentclientprotocol/codex-acp@2.2.2".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let obj = parse_bin_map(&spec, r#"{"codex-acp":"bin/codex-acp.js"}"#);
        assert_eq!(
            obj.get("@agentclientprotocol/codex-acp").map(|s| s.as_str()),
            Some("codex-acp")
        );
        let single = parse_bin_map(&spec, r#""./cli.js""#);
        assert_eq!(
            single
                .get("@agentclientprotocol/codex-acp")
                .map(|s| s.as_str()),
            Some("codex-acp")
        );
        assert!(parse_bin_map(&spec, "").is_empty());
    }

    #[test]
    fn registry_entry_maps_npx_to_npm_lifecycle() {
        let a: RegistryAgent = serde_json::from_value(json!({
            "id": "demo-acp",
            "name": "Demo",
            "version": "1.0.0",
            "description": "d",
            "icon": "https://x/demo.svg",
            "distribution": {
                "npx": { "package": "demo-acp@1.0.0", "args": ["--acp"], "env": { "A": "1" } }
            }
        }))
        .unwrap();
        let e = MarketEntry::from_registry(&a).expect("应能归一化");
        assert_eq!(e.category, "acp");
        assert_eq!(e.lifecycle.kind, LifecycleKind::Npm);
        assert_eq!(e.lifecycle.packages[0].package, "demo-acp@1.0.0");
        assert_eq!(e.lifecycle.packages[0].env.get("A").map(|s| s.as_str()), Some("1"));
        assert!(e.lifecycle.supported().is_ok());
        assert!(e.icon_url.is_some());
    }

    #[test]
    fn registry_entry_without_known_distribution_is_skipped() {
        let a: RegistryAgent = serde_json::from_value(json!({
            "id": "x", "name": "X", "distribution": { "mcpb": {} }
        }))
        .unwrap();
        assert!(MarketEntry::from_registry(&a).is_none());
    }
}
