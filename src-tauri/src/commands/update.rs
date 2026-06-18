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

/// Registry type for version queries
enum Registry {
    Npm,
    Pypi,
}

impl Registry {
    fn url(&self, package: &str) -> String {
        match self {
            Registry::Npm => format!("https://registry.npmjs.org/{}", package),
            Registry::Pypi => format!("https://pypi.org/pypi/{}/json", package),
        }
    }

    fn extract_version(&self, body: &serde_json::Value) -> String {
        match self {
            Registry::Npm => body.get("dist-tags")
                .and_then(|d| d.get("latest"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            Registry::Pypi => body.get("info")
                .and_then(|i| i.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        }
    }

    fn extract_release_time(&self, body: &serde_json::Value, version: &str) -> Option<String> {
        match self {
            Registry::Npm => body.get("time")
                .and_then(|t| t.get(version))
                .and_then(|v| v.as_str())
                .map(|s| s.split('T').next().unwrap_or(s).to_string()),
            Registry::Pypi => body.get("releases")
                .and_then(|r| r.get(version))
                .and_then(|arr| arr.as_array())
                .and_then(|arr| arr.first())
                .and_then(|r| r.get("upload_time"))
                .and_then(|t| t.as_str())
                .map(|s| s.split('T').next().unwrap_or(s).to_string()),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Registry::Npm => "npm",
            Registry::Pypi => "PyPI",
        }
    }
}

/// Query a package registry for latest version and release time.
async fn query_registry(registry: Registry, package_name: &str) -> Result<VersionTimeInfo, AppError> {
    let url = registry.url(package_name);
    let body = http_get_json(&url).await
        .map_err(|e| AppError::Network(format!("查询 {} 失败: {}", registry.label(), e)))?;

    let version = registry.extract_version(&body);
    let release_time = registry.extract_release_time(&body, &version);

    Ok(VersionTimeInfo { version, release_time })
}

/// Check latest version for a single npm package (used by EnvManager for per-agent checking)
#[tauri::command]
pub async fn check_single_npm(package_name: String) -> Result<VersionTimeInfo, AppError> {
    query_registry(Registry::Npm, &package_name).await
}

/// Check latest version for a single PyPI package (used by EnvManager for per-agent checking)
#[tauri::command]
pub async fn check_single_pypi(package_name: String) -> Result<VersionTimeInfo, AppError> {
    query_registry(Registry::Pypi, &package_name).await
}
