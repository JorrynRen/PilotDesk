//! 工作流远程发起（专业版）。
//!
//! 云端只负责设备登记、请求状态与结果码；工作流输入、输出、日志和产物始终留在设备侧。
//! 设备令牌与账号 access token 一样加密保存在本地，前端不会拿到明文。

use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::commands::account::current_access_token;
use crate::commands::app_settings::{delete_setting, get_setting, set_setting};
use crate::commands::workflow::start_workflow_inner;
use crate::utils::errors::AppError;
use crate::utils::platform;
use crate::workflow::executor::NodeExecutor;
use crate::workflow::WorkflowInstanceStatus;

const REMOTE_DEVICE_SETTING: &str = "remote_workflow_device";
const CAP_REMOTE_RUN: &str = "workflow.remote-run";
const POLL_INTERVAL_SECS: u64 = 15;
const EXECUTION_MONITOR_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredRemoteDevice {
    device_id: i64,
    name: String,
    platform: String,
    app_version: String,
    device_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDeviceView {
    registered: bool,
    id: Option<i64>,
    name: String,
    platform: String,
    app_version: String,
    online: bool,
    last_seen_at: Option<String>,
    revoked_at: Option<String>,
    current_device: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteDeviceSummary {
    id: i64,
    name: String,
    platform: String,
    app_version: String,
    online: bool,
    last_seen_at: Option<String>,
    revoked_at: Option<String>,
}

#[derive(Deserialize)]
struct DeviceListResponse {
    devices: Vec<RemoteDeviceSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterDeviceResponse {
    device: RemoteDeviceSummary,
    device_token: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRunView {
    id: i64,
    device_id: i64,
    workflow_key: String,
    status: String,
    created_at: String,
    expires_at: String,
    claimed_at: Option<String>,
    finished_at: Option<String>,
    result_code: Option<String>,
}

#[derive(Deserialize)]
struct RunListResponse {
    requests: Vec<RemoteRunView>,
}

#[derive(Deserialize)]
struct RunCreateResponse {
    request: RemoteRunView,
}

#[derive(Deserialize)]
struct RunMutationResponse {
    request: RemoteRunView,
}

#[derive(Deserialize)]
struct ClaimResponse {
    requests: Vec<ClaimedRun>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimedRun {
    id: i64,
    workflow_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePollResult {
    claimed: usize,
    scanned: bool,
}

fn load_identity(conn: &Connection) -> Result<Option<StoredRemoteDevice>, AppError> {
    let Some(encrypted) = get_setting(conn, REMOTE_DEVICE_SETTING)? else {
        return Ok(None);
    };
    if encrypted.is_empty() {
        return Ok(None);
    }
    let json = crate::utils::crypto::decrypt(&encrypted).map_err(AppError::Config)?;
    Ok(serde_json::from_str(&json).ok())
}

fn save_identity(conn: &Connection, identity: &StoredRemoteDevice) -> Result<(), AppError> {
    let json = serde_json::to_string(identity)?;
    let encrypted = crate::utils::crypto::encrypt(&json).map_err(AppError::Config)?;
    set_setting(conn, REMOTE_DEVICE_SETTING, &encrypted)
}

async fn require_access(state: &crate::DbState) -> Result<String, String> {
    current_access_token(state)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "请先登录平台账号".to_string())
}

fn require_capability(conn: &Connection) -> Result<(), String> {
    if crate::commands::account::has_capability(conn, CAP_REMOTE_RUN) == Some(true) {
        Ok(())
    } else {
        Err("远程发起工作流为专业版功能".to_string())
    }
}

fn to_view(device: RemoteDeviceSummary, current_id: Option<i64>) -> RemoteDeviceView {
    RemoteDeviceView {
        registered: true,
        id: Some(device.id),
        name: device.name,
        platform: device.platform,
        app_version: device.app_version,
        online: device.online,
        last_seen_at: device.last_seen_at,
        revoked_at: device.revoked_at,
        current_device: current_id == Some(device.id),
    }
}

#[tauri::command]
pub async fn remote_device_register(
    state: tauri::State<'_, crate::DbState>,
    name: Option<String>,
) -> Result<RemoteDeviceView, String> {
    {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        require_capability(&conn)?;
    }
    let access = require_access(state.inner()).await?;
    let default_name = format!(
        "{} · PilotDesk",
        std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "当前设备".to_string())
    );
    let body = serde_json::json!({
        "name": name.unwrap_or(default_name),
        "platform": std::env::consts::OS,
        "appVersion": env!("CARGO_PKG_VERSION"),
    });
    let response: RegisterDeviceResponse =
        platform::post_json("/api/v1/me/remote/devices", &body, Some(&access))
            .await
            .map_err(|e| e.to_string())?;
    let identity = StoredRemoteDevice {
        device_id: response.device.id,
        name: response.device.name.clone(),
        platform: response.device.platform.clone(),
        app_version: response.device.app_version.clone(),
        device_token: response.device_token,
    };
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败：{}", e))?;
    save_identity(&conn, &identity).map_err(|e| e.to_string())?;
    Ok(to_view(response.device, Some(identity.device_id)))
}

#[tauri::command]
pub async fn remote_device_status(
    state: tauri::State<'_, crate::DbState>,
) -> Result<RemoteDeviceView, String> {
    let identity = {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        require_capability(&conn)?;
        load_identity(&conn).map_err(|e| e.to_string())?
    };
    let Some(identity) = identity else {
        return Ok(RemoteDeviceView {
            registered: false,
            id: None,
            name: String::new(),
            platform: std::env::consts::OS.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            online: false,
            last_seen_at: None,
            revoked_at: None,
            current_device: false,
        });
    };
    let access = require_access(state.inner()).await?;
    let response: DeviceListResponse =
        platform::get_json_retry("/api/v1/me/remote/devices", Some(&access))
            .await
            .map_err(|e| e.to_string())?;
    if let Some(device) = response
        .devices
        .into_iter()
        .find(|device| device.id == identity.device_id)
    {
        return Ok(to_view(device, Some(identity.device_id)));
    }
    Ok(RemoteDeviceView {
        registered: false,
        id: Some(identity.device_id),
        name: identity.name,
        platform: identity.platform,
        app_version: identity.app_version,
        online: false,
        last_seen_at: None,
        revoked_at: None,
        current_device: true,
    })
}

#[tauri::command]
pub async fn remote_devices_list(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<RemoteDeviceView>, String> {
    let identity = {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        require_capability(&conn)?;
        load_identity(&conn).map_err(|e| e.to_string())?
    };
    let access = require_access(state.inner()).await?;
    let response: DeviceListResponse =
        platform::get_json_retry("/api/v1/me/remote/devices", Some(&access))
            .await
            .map_err(|e| e.to_string())?;
    let current_id = identity.as_ref().map(|item| item.device_id);
    Ok(response
        .devices
        .into_iter()
        .map(|device| to_view(device, current_id))
        .collect())
}

#[tauri::command]
pub async fn remote_device_revoke(
    state: tauri::State<'_, crate::DbState>,
    device_id: i64,
) -> Result<(), String> {
    let access = require_access(state.inner()).await?;
    let path = format!("/api/v1/me/remote/devices/{device_id}/revoke");
    let _: serde_json::Value = platform::post_json(&path, &serde_json::json!({}), Some(&access))
        .await
        .map_err(|e| e.to_string())?;
    let conn = state
        .get_conn()
        .map_err(|e| format!("数据库连接失败：{}", e))?;
    if load_identity(&conn)
        .map_err(|e| e.to_string())?
        .is_some_and(|item| item.device_id == device_id)
    {
        delete_setting(&conn, REMOTE_DEVICE_SETTING).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn remote_run_create(
    state: tauri::State<'_, crate::DbState>,
    device_id: i64,
    workflow_key: String,
) -> Result<RemoteRunView, String> {
    let access = require_access(state.inner()).await?;
    let response: RunCreateResponse = platform::post_json(
        "/api/v1/me/remote/requests",
        &serde_json::json!({ "deviceId": device_id, "workflowKey": workflow_key }),
        Some(&access),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(response.request)
}

#[tauri::command]
pub async fn remote_run_list(
    state: tauri::State<'_, crate::DbState>,
) -> Result<Vec<RemoteRunView>, String> {
    let access = require_access(state.inner()).await?;
    let response: RunListResponse = platform::get_json_retry("/api/v1/me/remote/requests", Some(&access))
        .await
        .map_err(|e| e.to_string())?;
    Ok(response.requests)
}

#[tauri::command]
pub async fn remote_run_cancel(
    state: tauri::State<'_, crate::DbState>,
    request_id: i64,
) -> Result<RemoteRunView, String> {
    let access = require_access(state.inner()).await?;
    let path = format!("/api/v1/me/remote/requests/{request_id}/cancel");
    let response: RunMutationResponse =
        platform::post_json(&path, &serde_json::json!({}), Some(&access))
            .await
            .map_err(|e| e.to_string())?;
    Ok(response.request)
}

fn result_for_instance(
    instance: &crate::workflow::WorkflowInstance,
) -> Option<(&'static str, &'static str)> {
    match instance.status {
        WorkflowInstanceStatus::Success => Some(("succeeded", "ok")),
        WorkflowInstanceStatus::Failed => Some(("failed", "workflow_failed")),
        WorkflowInstanceStatus::Timeout => Some(("failed", "workflow_timeout")),
        WorkflowInstanceStatus::Cancelled => Some(("failed", "workflow_cancelled")),
        WorkflowInstanceStatus::Paused => Some(("failed", "paused_requires_input")),
        WorkflowInstanceStatus::Pending | WorkflowInstanceStatus::Running => None,
    }
}

async fn complete_remote_run(
    identity: &StoredRemoteDevice,
    request_id: i64,
    status: &str,
    result_code: &str,
) {
    let body = serde_json::json!({
        "deviceToken": identity.device_token,
        "requestId": request_id,
        "status": status,
        "resultCode": result_code,
    });
    if let Err(e) = platform::post_json::<_, serde_json::Value>(
        "/api/v1/me/remote/device/complete",
        &body,
        None,
    )
    .await
    {
        log::warn!("[RemoteWorkflow] 完成回执失败 #{}：{}", request_id, e);
    }
}

async fn execute_claimed(
    state: crate::DbState,
    executor: Arc<NodeExecutor>,
    app_handle: tauri::AppHandle,
    identity: StoredRemoteDevice,
    request: ClaimedRun,
) {
    let started = start_workflow_inner(
        &state,
        executor,
        app_handle,
        request.workflow_key.clone(),
        None,
        None,
        "remote",
    )
    .await;
    let instance = match started {
        Ok(instance) => instance,
        Err(e) => {
            log::warn!(
                "[RemoteWorkflow] 请求 #{} 启动失败（workflow={}）：{}",
                request.id,
                request.workflow_key,
                e
            );
            complete_remote_run(&identity, request.id, "failed", "workflow_start_failed").await;
            return;
        }
    };

    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(EXECUTION_MONITOR_SECS);
    loop {
        let terminal = state
            .get_conn()
            .ok()
            .and_then(|conn| {
                crate::workflow::events::derive_instance(&conn, &instance.id)
                    .ok()
                    .flatten()
            })
            .and_then(|current| result_for_instance(&current).map(|result| result));
        if let Some((status, code)) = terminal {
            complete_remote_run(&identity, request.id, status, code).await;
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            complete_remote_run(&identity, request.id, "failed", "monitor_timeout").await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

async fn poll_once_inner(
    state: &crate::DbState,
    executor: &Arc<NodeExecutor>,
    app_handle: &tauri::AppHandle,
) -> Result<RemotePollResult, String> {
    let identity = {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        load_identity(&conn).map_err(|e| e.to_string())?
    };
    let Some(identity) = identity else {
        return Ok(RemotePollResult {
            claimed: 0,
            scanned: false,
        });
    };
    let response: ClaimResponse = platform::post_json(
        "/api/v1/me/remote/device/claim",
        &serde_json::json!({ "deviceToken": identity.device_token, "limit": 3 }),
        None,
    )
    .await
    .map_err(|e| e.to_string())?;
    let claimed = response.requests.len();
    for request in response.requests {
        let task_state = crate::DbState {
            pool: state.pool.clone(),
        };
        let task_executor = executor.clone();
        let task_app = app_handle.clone();
        let task_identity = identity.clone();
        tauri::async_runtime::spawn(async move {
            execute_claimed(task_state, task_executor, task_app, task_identity, request).await;
        });
    }
    Ok(RemotePollResult {
        claimed,
        scanned: true,
    })
}

#[tauri::command]
pub async fn remote_poll_once(
    state: tauri::State<'_, crate::DbState>,
    executor: tauri::State<'_, Arc<NodeExecutor>>,
    app_handle: tauri::AppHandle,
) -> Result<RemotePollResult, String> {
    {
        let conn = state
            .get_conn()
            .map_err(|e| format!("数据库连接失败：{}", e))?;
        require_capability(&conn)?;
    }
    poll_once_inner(state.inner(), executor.inner(), &app_handle).await
}

/// 后台轮询：仅登记过当前设备时联网；离线或未登录时静默跳过。
pub async fn run_remote_poll_loop(
    state: crate::DbState,
    executor: Arc<NodeExecutor>,
    app_handle: tauri::AppHandle,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
        match poll_once_inner(&state, &executor, &app_handle).await {
            Ok(result) if result.claimed > 0 => {
                log::info!("[RemoteWorkflow] 已领取 {} 个远程请求", result.claimed);
            }
            Ok(_) => {}
            Err(e) => log::debug!("[RemoteWorkflow] 轮询跳过：{}", e),
        }
    }
}
