use serde::{Deserialize, Serialize};
use crate::utils::errors::AppError;

/// Version check result for a single component
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionCheckResult {
    pub name: String,
    pub current: Option<String>,
    pub latest: Option<String>,
    pub has_update: bool,
    pub error: Option<String>,
}

/// Version info with release time from registry
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionTimeInfo {
    pub version: String,
    pub release_time: Option<String>,
}

/// PilotDesk update check result (used in About page)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PilotdeskUpdateResponse {
    pub pilotdesk: VersionCheckResult,
    pub checked_at: String,
}

/// Compare two semver strings: returns true if `a` is older than `b`
fn is_version_older(a: &str, b: &str) -> bool {
    let a_ver = semver::Version::parse(a).unwrap_or_else(|_| semver::Version::new(0, 0, 0));
    let b_ver = semver::Version::parse(b).unwrap_or_else(|_| semver::Version::new(0, 0, 0));
    a_ver < b_ver
}

/// Build a shared HTTP client with 10s timeout.
fn http_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Network(format!("HTTP client build failed: {}", e)))
}

/// Fetch JSON from a URL with User-Agent header.
async fn http_get_json(url: &str) -> Result<serde_json::Value, AppError> {
    let client = http_client()?;
    let resp = client.get(url)
        .header("User-Agent", "PilotDesk")
        .send()
        .await
        .map_err(|e| AppError::Network(format!("HTTP request failed: {}", e)))?;
    resp.json().await.map_err(|e| AppError::Network(format!("JSON parse failed: {}", e)))
}

/// Check for PilotDesk updates only (GitHub releases).
/// Claude Code / Hermes update checks are handled in the EnvManager via check_single_npm/pypi.
#[tauri::command]
pub async fn check_pilotdesk_update() -> Result<PilotdeskUpdateResponse, AppError> {
    log::info!("[update] Checking PilotDesk update...");

    let current = env!("CARGO_PKG_VERSION").to_string();

    let latest = {
        let url = "https://api.github.com/repos/jorryn/pilotdesk/releases/latest";
        let body = http_get_json(url).await?;
        let tag = body.get("tag_name").and_then(|t| t.as_str())
            .map(|t| t.trim_start_matches('v').to_string())
            .filter(|t| !t.is_empty());
        tag
    };

    let has_update = latest.as_deref().map(|lat| is_version_older(&current, lat)).unwrap_or(false);
    let now = chrono::Local::now();
    let checked_at = now.format("%Y-%m-%d %H:%M:%S").to_string();

    log::info!("[update] PilotDesk: current={}, latest={:?}, has_update={}", current, latest, has_update);

    Ok(PilotdeskUpdateResponse {
        pilotdesk: VersionCheckResult {
            name: "PilotDesk".to_string(),
            current: Some(current),
            latest,
            has_update,
            error: None,
        },
        checked_at,
    })
}

/// Generic agent update check: reads latest_version_cmd from agents table and executes it.
/// 完全依赖数据库命令模板，代码不做任何特殊处理，保证通用性和可拓展性。
#[tauri::command]
pub async fn check_agent_update(state: tauri::State<'_, crate::DbState>, agent_type: String) -> Result<VersionTimeInfo, AppError> {
    let conn = state.get_conn()?;
    let config = crate::commands::agents::get_agent_inner(&conn, &agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型 '{}' 不存在", agent_type)))?;

    let cmd = config.latest_version_cmd;
    if cmd.is_empty() {
        return Err(AppError::Config(format!("{} 未配置版本查询命令", agent_type)));
    }

    // 统一执行命令，不区分命令类型
    let output = crate::commands::env::run_shell_cmd(&cmd)
        .map_err(|e| AppError::External(format!("版本查询失败: {}", e)))?;
    Ok(VersionTimeInfo {
        version: output,
        release_time: None,
    })
}
