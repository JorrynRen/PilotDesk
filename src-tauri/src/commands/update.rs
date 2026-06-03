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
    let a_parts: Vec<u32> = a.split('.').filter_map(|p| p.parse::<u32>().ok()).collect();
    let b_parts: Vec<u32> = b.split('.').filter_map(|p| p.parse::<u32>().ok()).collect();
    let max_len = a_parts.len().max(b_parts.len());
    for i in 0..max_len {
        let av = a_parts.get(i).unwrap_or(&0);
        let bv = b_parts.get(i).unwrap_or(&0);
        if av < bv { return true; }
        if av > bv { return false; }
    }
    false
}

/// Query npm registry for latest version via HTTP
async fn query_npm_latest(package_name: &str) -> Result<Option<String>, String> {
    let url = format!("https://registry.npmjs.org/{}/latest", package_name);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client build failed: {}", e))?;

    let resp = client.get(&url)
        .header("User-Agent", "PilotDesk")
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.map_err(|e| format!("JSON parse failed: {}", e))?;
        let version = body.get("version").and_then(|v| v.as_str()).map(|s| s.to_string());
        return Ok(version);
    }
    Ok(None)
}

/// Query PyPI for latest version via HTTP
async fn query_pypi_latest(package_name: &str) -> Result<Option<String>, String> {
    let url = format!("https://pypi.org/pypi/{}/json", package_name);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client build failed: {}", e))?;

    let resp = client.get(&url)
        .header("User-Agent", "PilotDesk")
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.map_err(|e| format!("JSON parse failed: {}", e))?;
        let version = body.get("info").and_then(|i| i.get("version")).and_then(|v| v.as_str()).map(|s| s.to_string());
        return Ok(version);
    }
    Ok(None)
}

/// Check for PilotDesk updates only (GitHub releases).
/// Claude Code / Hermes update checks are handled in the EnvManager via check_single_npm/pypi.
#[tauri::command]
pub async fn check_pilotdesk_update() -> Result<PilotdeskUpdateResponse, AppError> {
    println!("[update] Checking PilotDesk update...");

    let current = env!("CARGO_PKG_VERSION").to_string();

    let latest = {
        let url = "https://api.github.com/repos/jorryn/pilotdesk/releases/latest";
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| AppError {
                code: "UPDATE_CHECK_FAILED".into(),
                message: format!("HTTP client build failed: {}", e),
                details: None,
            })?;

        let resp = client.get(url)
            .header("User-Agent", "PilotDesk")
            .send()
            .await
            .map_err(|e| AppError {
                code: "UPDATE_CHECK_FAILED".into(),
                message: format!("GitHub API request failed: {}", e),
                details: None,
            })?;

        if resp.status().is_success() {
            let body: serde_json::Value = resp.json().await.map_err(|e| AppError {
                code: "UPDATE_CHECK_FAILED".into(),
                message: format!("JSON parse failed: {}", e),
                details: None,
            })?;
            let tag = body.get("tag_name").and_then(|t| t.as_str())
                .map(|t| t.trim_start_matches('v').to_string())
                .filter(|t| !t.is_empty());
            tag
        } else {
            None
        }
    };

    let has_update = latest.as_deref().map(|lat| is_version_older(&current, lat)).unwrap_or(false);
    let now = chrono::Local::now();
    let checked_at = now.format("%Y-%m-%d %H:%M:%S").to_string();

    println!("[update] PilotDesk: current={}, latest={:?}, has_update={}", current, latest, has_update);

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

/// Check latest version for a single npm package (used by EnvManager for per-agent checking)
/// Returns both version string and release time.
#[tauri::command]
pub async fn check_single_npm(package_name: String) -> Result<VersionTimeInfo, AppError> {
    let url = format!("https://registry.npmjs.org/{}", package_name);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError {
            code: "NPM_QUERY_FAILED".into(),
            message: format!("查询 npm 失败: {}", e),
            details: None,
        })?;

    let resp = client.get(&url)
        .header("User-Agent", "PilotDesk")
        .send()
        .await
        .map_err(|e| AppError {
            code: "NPM_QUERY_FAILED".into(),
            message: format!("查询 npm 失败: {}", e),
            details: None,
        })?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.map_err(|e| AppError {
            code: "NPM_QUERY_FAILED".into(),
            message: format!("查询 npm 失败: {}", e),
            details: None,
        })?;

        let version = body.get("dist-tags")
            .and_then(|d| d.get("latest"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let release_time = body.get("time")
            .and_then(|t| t.get(&version))
            .and_then(|v| v.as_str())
            .map(|s| s.split('T').next().unwrap_or(s).to_string());

        return Ok(VersionTimeInfo { version, release_time });
    }

    Ok(VersionTimeInfo { version: String::new(), release_time: None })
}

/// Check latest version for a single PyPI package (used by EnvManager for per-agent checking)
/// Returns both version string and release time.
#[tauri::command]
pub async fn check_single_pypi(package_name: String) -> Result<VersionTimeInfo, AppError> {
    let url = format!("https://pypi.org/pypi/{}/json", package_name);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError {
            code: "PYPI_QUERY_FAILED".into(),
            message: format!("查询 PyPI 失败: {}", e),
            details: None,
        })?;

    let resp = client.get(&url)
        .header("User-Agent", "PilotDesk")
        .send()
        .await
        .map_err(|e| AppError {
            code: "PYPI_QUERY_FAILED".into(),
            message: format!("查询 PyPI 失败: {}", e),
            details: None,
        })?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.map_err(|e| AppError {
            code: "PYPI_QUERY_FAILED".into(),
            message: format!("查询 PyPI 失败: {}", e),
            details: None,
        })?;

        let version = body.get("info")
            .and_then(|i| i.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let release_time = body.get("releases")
            .and_then(|r| r.get(&version))
            .and_then(|arr| arr.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("upload_time"))
            .and_then(|t| t.as_str())
            .map(|s| s.split('T').next().unwrap_or(s).to_string());

        return Ok(VersionTimeInfo { version, release_time });
    }

    Ok(VersionTimeInfo { version: String::new(), release_time: None })
}
