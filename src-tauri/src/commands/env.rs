use tauri::Emitter;
use std::time::{Duration, Instant};
use crate::db::models::EnvInfo;
use crate::utils::errors::AppError;
use crate::commands::agents;

/// Debug logging for env detection
macro_rules! log_env {
    ($($arg:tt)*) => {{
        log::debug!($($arg)*);
    }};
}

// ──────────────────────────────────────────────
//  工具函数
// ──────────────────────────────────────────────


/// Clean version string: keep only the semver part
fn clean_version_string(raw: &str) -> String {
    let trimmed = raw.trim();
    let segments: Vec<&str> = trimmed
        .split(|c: char| c == ' ' || c == '(')
        .collect();

    // Priority 1: "v" + digit (e.g. "v0.16.0")
    if let Some(ver) = segments.iter().find(|s| {
        s.starts_with('v') && s.len() > 1
            && s[1..].chars().next().map_or(false, |c| c.is_ascii_digit())
    }) {
        return ver.trim_start_matches('v').trim().to_string();
    }

    // Priority 2: starts with digit (e.g. "2.1.177", "0.140.0-alpha.2")
    if let Some(ver) = segments.iter().find(|s| {
        !s.is_empty() && s.chars().next().map_or(false, |c| c.is_ascii_digit())
    }) {
        return ver.trim().to_string();
    }

    trimmed.to_string()
}

// ──────────────────────────────────────────────
//  环境检测
// ──────────────────────────────────────────────

/// 简单缓存：仅用于避免高频重复检测
static LAST_DETECT: std::sync::RwLock<Option<(Instant, EnvInfo)>> = std::sync::RwLock::new(None);
const CACHE_TTL: Duration = Duration::from_secs(120);

/// 通过 AgentManager 异步路径执行环境检测
async fn detect_env_with_console(
    app: &tauri::AppHandle,
    state: &crate::DbState,
    agent_mgr: &crate::agent::AgentManager,
) -> Result<EnvInfo, AppError> {
    log_env!("[env] Detecting environment via async path...");

    // ── Phase 1: 基础工具（tokio::join! 真正并行） ──
    let (node_result, git_result, py_result) = tokio::join!(
        agent_mgr.execute_command_output_async("node --version", "", 15, "node 版本查询"),
        agent_mgr.execute_command_output_async("git --version", "", 15, "git 版本查询"),
        async {
            // python fallback: python → python3 → py
            if let Ok(v) = agent_mgr.execute_command_output_async("python --version", "", 15, "python 版本查询").await {
                return Ok(v);
            }
            if let Ok(v) = agent_mgr.execute_command_output_async("python3 --version", "", 15, "python3 版本查询").await {
                return Ok(v);
            }
            if let Ok(v) = agent_mgr.execute_command_output_async("py --version", "", 15, "py 版本查询").await {
                return Ok(v);
            }
            Err("未找到 Python".to_string())
        },
    );

    let node_version = node_result.ok().map(|v| clean_version_string(&v));
    let git_version = git_result.ok().map(|v| clean_version_string(&v));
    let python_version = py_result.ok().map(|v| clean_version_string(&v));

    // emit 基础工具检测结果
    let _ = app.emit("env-base-tools", serde_json::json!({
        "nodeVersion": node_version,
        "gitVersion": git_version,
        "pythonVersion": python_version,
    }));

    // Agent 版本查询（futures::join_all 并行）
    let conn = state.get_conn()?;
    let db_agents = agents::list_agents_inner(&conn).unwrap_or_default();

    let mut agent_futures = Vec::new();
    for agent in &db_agents {
        if !agent.is_enabled || agent.version_cmd.is_empty() {
            continue;
        }
        let cmd = agent.version_cmd.clone();
        let label = format!("{} 版本查询", agent.agent_type);
        let agent_type = agent.agent_type.clone();
        agent_futures.push(async move {
            let version = agent_mgr.execute_command_output_async(
                &cmd, "", 15, &label,
            ).await.ok().map(|v| clean_version_string(&v));
            (agent_type, version)
        });
    }

    let agent_results = futures::future::join_all(agent_futures).await;

    let mut agent_versions = std::collections::HashMap::new();
    for (agent_type, version) in agent_results {
        let _ = app.emit("env-agent-version", serde_json::json!({
            "agentType": agent_type,
            "version": version,
        }));
        agent_versions.insert(agent_type, version);
    }

    log_env!("[env] node={:?} git={:?} python={:?} agents={:?}",
        node_version, git_version, python_version, agent_versions);

    let result = EnvInfo {
        node_version,
        git_version,
        python_version,
        agent_versions,
        agent_latest_versions: None,
    };

    // ── Phase 2: 更新检查（fire-and-forget 后台任务） ──
    let app_clone = app.clone();
    let state_clone: crate::DbState = crate::DbState { pool: state.pool.clone() };
    let mgr = std::sync::Arc::new(agent_mgr.shared());

    tokio::spawn(async move {
        let conn = match state_clone.get_conn() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[env/Phase2] get conn failed: {:?}", e);
                return;
            }
        };
        let db_agents = agents::list_agents_inner(&conn).unwrap_or_default();

        // Phase 2 并行查询所有 Agent 最新版本
        // 构建 async future 列表（避免 map 闭包 move mgr）
        let mut latest_futures = Vec::new();
        for agent in &db_agents {
            if !agent.is_enabled || agent.latest_version_cmd.is_empty() {
                continue;
            }
            let cmd = agent.latest_version_cmd.clone();
            let label = format!("{} 最新版本查询", agent.agent_type);
            let agent_type = agent.agent_type.clone();
            let mgr_clone = mgr.clone();
            latest_futures.push(async move {
                let version = mgr_clone.execute_command_output_async(
                    &cmd, "", 120, &label,
                ).await.ok().map(|v| clean_version_string(&v));
                (agent_type, version)
            });
        }

        let latest_results = futures::future::join_all(latest_futures).await;
        for (agent_type, version) in latest_results {
            let _ = app_clone.emit("env-agent-latest-version", serde_json::json!({
                "agentType": agent_type,
                "version": version,
            }));
            log_env!("[env/Phase2] {} latest={:?}", agent_type, version);
        }
    });

    Ok(result)
}

#[tauri::command]
pub async fn detect_env(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::DbState>,
    agent_mgr: tauri::State<'_, crate::AsyncMutex<crate::agent::AgentManager>>,
) -> Result<EnvInfo, AppError> {
    {
        let cache = LAST_DETECT.read().unwrap();
        if let Some((fetched, ref info)) = *cache {
            if fetched.elapsed() < CACHE_TTL {
                return Ok(info.clone());
            }
        }
    }

    // In-progress guard: prevent concurrent detect from executing duplicate work
    static DETECTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DETECTING.compare_exchange(false, true, std::sync::atomic::Ordering::Acquire, std::sync::atomic::Ordering::Relaxed).is_err() {
        let cache = LAST_DETECT.read().unwrap();
        if let Some((fetched, ref info)) = *cache {
            if fetched.elapsed() < CACHE_TTL {
                return Ok(info.clone());
            }
        }
        // Wait for the in-flight detection to complete
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if !DETECTING.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
        }
        let cache = LAST_DETECT.read().unwrap();
        if let Some((fetched, ref info)) = *cache {
            if fetched.elapsed() < CACHE_TTL {
                return Ok(info.clone());
            }
        }
    }

    let state_ref: crate::DbState = crate::DbState { pool: state.pool.clone() };
    let mgr = agent_mgr.lock().await;
    let result = detect_env_with_console(&app, &state_ref, &mgr).await;

    DETECTING.store(false, std::sync::atomic::Ordering::Release);

    if let Ok(ref info) = result {
        let mut cache = LAST_DETECT.write().unwrap();
        *cache = Some((Instant::now(), info.clone()));
    }

    result
}

/// 清除环境检测缓存（安装/更新后调用）
pub fn clear_env_cache() {
    if let Ok(mut cache) = LAST_DETECT.write() {
        *cache = None;
    }
}

#[tauri::command]
pub async fn clear_env_detect_cache() -> Result<(), AppError> {
    clear_env_cache();
    Ok(())
}

// ──────────────────────────────────────────────
//  安装/卸载/更新 — 从 DB 读取命令并执行
// ──────────────────────────────────────────────

/// 根据 agent_type 从 DB 读取 install_cmd 并执行
#[tauri::command]
pub async fn install_agent(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::DbState>,
    agent_mgr: tauri::State<'_, crate::AsyncMutex<crate::agent::AgentManager>>,
    agent_type: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    let config = agents::get_agent_inner(&conn, &agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型 '{}' 不存在", agent_type)))?;

    let cmd = config.install_cmd;
    if cmd.is_empty() {
        return Err(AppError::Config(format!("{} 未配置安装命令", agent_type)));
    }

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent_type,
        "message": format!("正在安装 {}...", config.display_name),
        "progress": 50
    }));

    let mgr = agent_mgr.lock().await;
    mgr.execute_command_output_async(&cmd, "", 300, &format!("安装 {}", agent_type))
        .await
        .map_err(|e| AppError::External(format!("安装 {} 失败: {}", agent_type, e)))?;

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent_type,
        "message": format!("{} 安装完成", config.display_name),
        "progress": 100
    }));

    clear_env_cache();
    Ok(())
}

/// 根据 agent_type 从 DB 读取 uninstall_cmd 并执行
#[tauri::command]
pub async fn uninstall_agent(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::DbState>,
    agent_mgr: tauri::State<'_, crate::AsyncMutex<crate::agent::AgentManager>>,
    agent_type: String,
) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    let config = agents::get_agent_inner(&conn, &agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型 '{}' 不存在", agent_type)))?;

    let cmd = config.uninstall_cmd;
    if cmd.is_empty() {
        return Err(AppError::Config(format!("{} 未配置卸载命令", agent_type)));
    }

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent_type,
        "message": format!("正在卸载 {}...", config.display_name),
        "progress": 50
    }));

    let mgr = agent_mgr.lock().await;
    mgr.execute_command_output_async(&cmd, "", 300, &format!("卸载 {}", agent_type))
        .await
        .map_err(|e| AppError::External(format!("卸载 {} 失败: {}", agent_type, e)))?;

    let _ = app.emit("install-progress", serde_json::json!({
        "agent": agent_type,
        "message": format!("{} 卸载完成", config.display_name),
        "progress": 100
    }));

    clear_env_cache();
    Ok(())
}

// ──────────────────────────────────────────────
//  目录校验
// ──────────────────────────────────────────────

/// Validate a directory path and create it if it doesn't exist.
/// Returns the normalized path on success, or an error message on failure.
#[tauri::command]
pub fn ensure_dir(path: String) -> Result<String, String> {
    if path.trim().is_empty() {
        return Err("路径不能为空".into());
    }

    let trimmed = path.trim();

    // Expand ~ to user home directory
    let expanded = if trimmed.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            trimmed.replacen("~", &home.to_string_lossy(), 1)
        } else {
            return Err("无法解析用户主目录路径".into());
        }
    } else {
        trimmed.to_string()
    };

    let p = std::path::Path::new(&expanded);

    #[cfg(target_os = "windows")]
    {
        // Reject bare drive letters like "C:" or "D:"
        if trimmed.len() <= 2 && trimmed.ends_with(':') {
            return Err("路径不完整，请输入完整目录路径（如 C:\\Users\\work）".into());
        }

        // Check for invalid path characters on Windows
        let invalid_chars = ['<', '>', '"', '|', '?', '*'];
        for c in invalid_chars {
            if trimmed.contains(c) {
                return Err(format!("路径包含非法字符 '{}'", c));
            }
        }

        // Ensure drive letter is valid (A-Z)
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            let drive = bytes[0].to_ascii_uppercase();
            if drive < b'A' || drive > b'Z' {
                return Err("无效的盘符，请输入有效的驱动器号（如 C:、D:）".into());
            }
        }
    }

    // Check if path exists
    if p.exists() {
        if p.is_dir() {
            Ok(expanded.to_string())
        } else {
            Err(format!("路径已存在但不是一个目录: {}", expanded))
        }
    } else {
        // Create directory (including parent directories)
        std::fs::create_dir_all(p)
            .map_err(|e| format!("创建目录失败: {}", e))?;
        Ok(expanded.to_string())
    }
}
