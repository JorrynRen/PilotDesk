//! ACP 官方 Agent 目录（registry）读取与缓存。
//!
//! 数据源：<https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json>
//! （纯静态清单，CDN `max-age=300`；条目版本已钉死，无“latest”语义）。
//!
//! 策略：
//! - 本地缓存 24h；过期后**后台**刷新，界面永远先读缓存（离线可用）；
//! - 手动刷新（force）直接打远端，失败时回退旧缓存；
//! - 从未成功拉取过时回退**内置快照**（编译期 `include_str!`），保证首次离线也有货架。
//!
//! 这里只负责「拿到并解析清单」，与预置合并、归一化成市场条目在 `commands/agent_market.rs`。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
/// 缓存有效期：24h
pub const CACHE_TTL_SECS: i64 = 24 * 60 * 60;
const FETCH_TIMEOUT_SECS: u64 = 12;

/// 内置快照（拉取从未成功时的兜底；内容为官方清单的某个时间点副本）。
const BUILTIN_SNAPSHOT: &str = include_str!("acp_registry_snapshot.json");

/// 官方目录中的一个 Agent。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub icon: Option<String>,
    /// 分发方式：npx / uvx / binary（结构按官方定义保留原始 JSON）
    #[serde(default)]
    pub distribution: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryFile {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub agents: Vec<RegistryAgent>,
}

/// 带时间戳的缓存包装。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    fetched_at: i64,
    data: RegistryFile,
}

fn cache_path() -> PathBuf {
    crate::utils::paths::app_root_dir()
        .join("cache")
        .join("acp-registry.json")
}

fn read_cache() -> Option<CacheFile> {
    let text = std::fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str::<CacheFile>(&text).ok()
}

fn write_cache(data: &RegistryFile) -> Result<(), String> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建缓存目录失败: {}", e))?;
    }
    let payload = CacheFile {
        fetched_at: crate::utils::now(),
        data: data.clone(),
    };
    let text = serde_json::to_string(&payload).map_err(|e| format!("序列化缓存失败: {}", e))?;
    std::fs::write(&path, text).map_err(|e| format!("写入缓存失败: {}", e))
}

fn parse_builtin() -> RegistryFile {
    serde_json::from_str::<RegistryFile>(BUILTIN_SNAPSHOT).unwrap_or_default()
}

async fn fetch_remote() -> Result<RegistryFile, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .no_proxy()
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(REGISTRY_URL)
        .send()
        .await
        .map_err(|e| format!("ACP 目录请求失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("ACP 目录返回 HTTP {}", resp.status().as_u16()));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取 ACP 目录失败: {}", e))?;
    serde_json::from_str::<RegistryFile>(&text).map_err(|e| format!("解析 ACP 目录失败: {}", e))
}

/// 读取目录。
///
/// - `force=false`：缓存新鲜（< 24h）直接用；过期则尝试远端，失败回退旧缓存；
/// - `force=true`：跳过新鲜度判断直接打远端，失败回退旧缓存；
/// - 连旧缓存都没有时用内置快照。
pub async fn load(force: bool) -> Result<RegistryFile, String> {
    let cached = read_cache();
    if !force {
        if let Some(c) = &cached {
            if crate::utils::now() - c.fetched_at < CACHE_TTL_SECS {
                return Ok(c.data.clone());
            }
        }
    }

    match fetch_remote().await {
        Ok(data) => {
            if let Err(e) = write_cache(&data) {
                log::warn!("[ACP Registry] 缓存写入失败: {}", e);
            }
            Ok(data)
        }
        Err(e) => {
            log::warn!("[ACP Registry] 远端拉取失败，尝试本地缓存: {}", e);
            if let Some(c) = cached {
                return Ok(c.data);
            }
            let snapshot = parse_builtin();
            if snapshot.agents.is_empty() {
                Err(format!("ACP 目录不可用且无缓存: {}", e))
            } else {
                Ok(snapshot)
            }
        }
    }
}

/// 启动时后台刷新：缓存过期才打远端，失败静默（界面届时读旧缓存/快照）。
pub fn spawn_background_refresh() {
    tauri::async_runtime::spawn(async move {
        let fresh = read_cache()
            .map(|c| crate::utils::now() - c.fetched_at < CACHE_TTL_SECS)
            .unwrap_or(false);
        if fresh {
            return;
        }
        if let Err(e) = load(true).await {
            log::warn!("[ACP Registry] 后台刷新失败: {}", e);
        }
    });
}
