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

/// Query npm registry for latest version via PowerShell
async fn query_npm_latest(package_name: &str) -> Result<Option<String>, String> {
    let url = format!("https://registry.npmjs.org/{}/latest", package_name);
    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "(Invoke-RestMethod -Uri '{}' -TimeoutSec 10).version",
                url
            ),
        ])
        .output()
        .await
        .map_err(|e| format!("PowerShell fetch failed: {}", e))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let version = stdout.trim().to_string();
        if !version.is_empty() {
            return Ok(Some(version));
        }
    }
    Ok(None)
}

/// Query PyPI for latest version via PowerShell
async fn query_pypi_latest(package_name: &str) -> Result<Option<String>, String> {
    let url = format!("https://pypi.org/pypi/{}/json", package_name);
    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "(Invoke-RestMethod -Uri '{}' -TimeoutSec 10).info.version",
                url
            ),
        ])
        .output()
        .await
        .map_err(|e| format!("PowerShell fetch failed: {}", e))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let version = stdout.trim().to_string();
        if !version.is_empty() {
            return Ok(Some(version));
        }
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
        let output = tokio::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "try {{ (Invoke-RestMethod -Uri '{}' -TimeoutSec 10 -Headers @{{'User-Agent'='PilotDesk'}}).tag_name }} catch {{ }}",
                    url
                ),
            ])
            .output()
            .await
            .map_err(|e| AppError {
                code: "UPDATE_CHECK_FAILED".into(),
                message: format!("PowerShell fetch failed: {}", e),
                details: None,
            })?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let tag = stdout.trim().trim_start_matches('v').to_string();
            if !tag.is_empty() { Some(tag) } else { None }
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
    // Use full package endpoint to get dist-tags.latest + time[latest]
    let url = format!("https://registry.npmjs.org/{}", package_name);
    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "$r = Invoke-RestMethod -Uri '{}' -TimeoutSec 10; $latest = $r.\"dist-tags\".latest; $time = $r.time.$latest; \"$latest|$time\"",
                url
            ),
        ])
        .output()
        .await
        .map_err(|e| AppError {
            code: "NPM_QUERY_FAILED".into(),
            message: format!("查询 npm 失败: {}", e),
            details: None,
        })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().splitn(2, '|').collect();
        let version = parts.get(0).map_or("", |v| v).trim().to_string();
        let release_time = parts.get(1).filter(|s| !s.trim().is_empty()).map(|s| {
            // npm returns ISO 8601 like "2025-05-20T18:30:00.000Z", extract date only
            s.trim().split('T').next().unwrap_or(s.trim()).to_string()
        });
        return Ok(VersionTimeInfo { version, release_time });
    }
    Ok(VersionTimeInfo { version: String::new(), release_time: None })
}

/// Check latest version for a single PyPI package (used by EnvManager for per-agent checking)
/// Returns both version string and release time.
#[tauri::command]
pub async fn check_single_pypi(package_name: String) -> Result<VersionTimeInfo, AppError> {
    let url = format!("https://pypi.org/pypi/{}/json", package_name);
    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "$r = Invoke-RestMethod -Uri '{}' -TimeoutSec 10; $ver = $r.info.version; $upload = $r.releases.$ver[0].upload_time; \"$ver|$upload\"",
                url
            ),
        ])
        .output()
        .await
        .map_err(|e| AppError {
            code: "PYPI_QUERY_FAILED".into(),
            message: format!("查询 PyPI 失败: {}", e),
            details: None,
        })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().splitn(2, '|').collect();
        let version = parts.get(0).map_or("", |v| v).trim().to_string();
        let release_time = parts.get(1).map(|s| {
            // PyPI returns ISO 8601 like "2025-05-29T12:00:00", extract date only
            s.trim().split('T').next().unwrap_or(s.trim()).to_string()
        });
        return Ok(VersionTimeInfo { version, release_time });
    }
    Ok(VersionTimeInfo { version: String::new(), release_time: None })
}


