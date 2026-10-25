use crate::agent::config::{executable_from_template, AgentConfig};
use crate::agent::lifecycle::{package_without_version, LifecycleKind, LifecycleSpec, PackageRole};
use crate::commands::agents;
use crate::db::models::{AgentHealth, EnvInfo};
use crate::utils::errors::AppError;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tauri::Emitter;
use tokio::io::AsyncReadExt;

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
    let segments: Vec<&str> = trimmed.split(|c: char| c == ' ' || c == '(').collect();

    // Priority 1: "v" + digit (e.g. "v0.16.0")
    if let Some(ver) = segments.iter().find(|s| {
        s.starts_with('v')
            && s.len() > 1
            && s[1..].chars().next().map_or(false, |c| c.is_ascii_digit())
    }) {
        return ver.trim_start_matches('v').trim().to_string();
    }

    // Priority 2: starts with digit (e.g. "2.1.177", "0.140.0-alpha.2")
    if let Some(ver) = segments
        .iter()
        .find(|s| !s.is_empty() && s.chars().next().map_or(false, |c| c.is_ascii_digit()))
    {
        return ver.trim().to_string();
    }

    trimmed.to_string()
}

// ──────────────────────────────────────────────
//  环境检测
// ──────────────────────────────────────────────

/// 执行一条探测命令，仅看是否成功（**不启动 Agent**：部分 ACP 适配器一启动就进 serving 态）。
pub(crate) async fn probe_command_ok(
    agent_mgr: &crate::agent::AgentManager,
    command: &str,
) -> bool {
    agent_mgr
        .execute_command_output_async(command, "", 15, "启动命令探测")
        .await
        .is_ok()
}

/// `where <cmd>` / `command -v <cmd>`：启动命令是否在 PATH 上。
pub(crate) fn resolve_command_probe(command: &str) -> String {
    if cfg!(windows) {
        format!("where {}", command)
    } else {
        format!("command -v {}", command)
    }
}

/// 单个 Agent 的检测结果（版本用于列表展示，health 用于状态与门禁）。
pub(crate) struct AgentProbe {
    pub version: Option<String>,
    pub health: AgentHealth,
}

/// 评估一个 Agent 的「已安装 / 可用」状态。
///
/// **检测与环境门禁共用同一口径**，避免出现「检测显示可用、会话却起不来」。
/// - 本地 CLI：看运行模板首 token 能否在本机解析；
/// - ACP：适配器装好 ≠ Agent 可用——还要看会话真正 spawn 的启动命令（`conn.command`）
///   能否解析，以及规格里声明为 host 的宿主 Agent 包是否真的装上
///   （codex-acp / claude-agent-acp 这类适配器自身不含宿主 CLI）。
pub(crate) async fn probe_agent(
    agent_mgr: &crate::agent::AgentManager,
    agent: &AgentConfig,
    transport: &str,
    conn_json: Option<&str>,
) -> AgentProbe {
    let lifecycle = agent.lifecycle.as_ref();
    let bins = agent
        .pkg_state
        .as_ref()
        .map(|s| s.bins.clone())
        .unwrap_or_default();
    let commands = lifecycle.and_then(|l| l.commands().ok());

    // 版本探测：优先按生命周期派生。**手工配置的本地 CLI 没有规格**，
    // 退回运行模板里的可执行名 + `--version`（此前这类 Agent 会被直接跳过 → 永远「未安装」）。
    let version_probe = commands
        .as_ref()
        .and_then(|c| c.version_probe.clone())
        .or_else(|| {
            if transport == "subprocess" {
                executable_from_template(&agent.run_cmd_template)
                    .map(|exe| format!("{} --version", exe))
            } else {
                None
            }
        });

    let label = format!("{} 版本查询", agent.agent_type);
    let version = match version_probe {
        Some(cmd) => agent_mgr
            .execute_command_output_async(&cmd, "", 15, &label)
            .await
            .ok()
            .map(|v| {
                lifecycle
                    .and_then(|l| l.parse_installed_version(&v))
                    .unwrap_or_else(|| clean_version_string(&v))
            }),
        None => None,
    };

    // 会话真正 spawn 的命令：ACP 取 `conn.command`（安装完成时回写），
    // 其次退回生命周期解析出的运行时；本地 CLI 取运行模板首 token。
    let launch = agent
        .launch_command(transport, conn_json)
        .or_else(|| {
            lifecycle
                .and_then(|l| l.resolved_runtime(&bins))
                .map(|r| r.command)
        })
        .filter(|c| !c.trim().is_empty());

    // npm/npx 兜底命令不能直接当作「启动命令可用」：
    // 包没装上时 `where npm` 一样成功，会话一启动就失败。
    let via_pkg_manager = launch
        .as_deref()
        .map(|c| matches!(c, "npm" | "npm.cmd" | "npx" | "npx.cmd"))
        .unwrap_or(false);

    let launch_ok = match launch.as_deref() {
        None => false,
        Some(cmd) => {
            if via_pkg_manager && bins.is_empty() {
                false
            } else {
                probe_command_ok(agent_mgr, &resolve_command_probe(cmd)).await
            }
        }
    };

    let health = if transport == "acp" {
        // 适配器之外再查宿主 Agent：ACP 适配器通常只是协议壳，真正干活的是宿主 CLI。
        let missing_host = missing_host_package(agent_mgr, lifecycle).await;
        // 命令能找到还不够：真实跑一次 initialize + session/new，
        // 才能发现「未登录 / 对端拒绝服务」这类只有握手时才暴露的失败。
        let handshake_err = if launch_ok {
            match conn_json {
                Some(raw) => probe_acp_session(raw).await.err(),
                None => Some("缺少 ACP 连接配置（conn）".to_string()),
            }
        } else {
            None
        };
        let reason = if launch.is_none() {
            Some("连接配置缺少启动命令，请重新安装该适配器".to_string())
        } else if !launch_ok {
            if via_pkg_manager {
                Some("ACP 适配器尚未安装完成".to_string())
            } else {
                Some(format!(
                    "启动命令「{}」在本机不可用",
                    launch.as_deref().unwrap_or_default()
                ))
            }
        } else if let Some(pkg) = missing_host.as_deref() {
            Some(format!("宿主 Agent 未安装（{}）", pkg))
        } else if let Some(err) = handshake_err.as_deref() {
            Some(format!("ACP 握手失败：{}", err))
        } else {
            None
        };
        AgentHealth {
            installed: version.is_some() || launch_ok,
            available: launch_ok && missing_host.is_none() && handshake_err.is_none(),
            reason,
        }
    } else {
        let installed = version.is_some() || launch_ok;
        let reason = if !installed {
            Some("未安装".to_string())
        } else if !launch_ok {
            Some(match launch.as_deref() {
                Some(cmd) => format!("已安装，但启动命令「{}」不可用", cmd),
                None => "已安装，但启动命令不可用".to_string(),
            })
        } else {
            None
        };
        AgentHealth {
            installed,
            available: launch_ok,
            reason,
        }
    };

    AgentProbe { version, health }
}

/// 单次 ACP 握手探测的超时（spawn → initialize → session/new）。
///
/// 只覆盖协议交互，不含 npx 首次下载安装的耗时；超时即判「对端未响应」，
/// 因为这轮会话同样会因握手失败而起不来。
const ACP_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// 跑一遍真实的 ACP 握手，用来回答「这个 Agent 现在能不能真的干起活来」。
///
/// 只查「命令能不能找到」会漏掉对端拒绝服务的情况：适配器在 PATH 上并不等于可用。
/// 实测 copilot 未登录时 `initialize` 正常返回，而 `session/new` 直接回
/// `-32000 Authentication required`——会话必然失败，但 `where copilot` 完全看不出来。
async fn probe_acp_session(conn_json: &str) -> Result<(), String> {
    use crate::agent::transport::acp::{method as acp_method, ACP_PROTOCOL_VERSION};
    use crate::agent::transport::jsonrpc::JsonRpcPeer;

    let v: serde_json::Value =
        serde_json::from_str(conn_json).map_err(|e| format!("连接配置不是合法 JSON：{}", e))?;
    let command = v
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if command.is_empty() {
        return Err("连接配置缺少启动命令".to_string());
    }
    let args: Vec<String> = v
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();

    // 与会话同一套启动方式：Windows 上 npm 全局装的 CLI 往往是 .cmd/.exe 垫片
    let (program, prefix) = crate::utils::process::resolve_windows_program(&command);
    let mut cmd = crate::utils::process::hidden_tokio_command(&program);
    cmd.args(&prefix)
        .args(&args)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(env) = v.get("env").and_then(|e| e.as_object()) {
        for (k, val) in env {
            if let Some(s) = val.as_str() {
                cmd.env(k, s);
            }
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 ACP 进程失败: {}", e))?;

    // stderr 必须看：对端把真正的失败原因（依赖缺失、登录提示）写在这里
    let stderr_text = Arc::new(StdMutex::new(String::new()));
    if let Some(mut err) = child.stderr.take() {
        let buf = Arc::clone(&stderr_text);
        tokio::spawn(async move {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s).await;
            if let Ok(mut b) = buf.lock() {
                b.push_str(&s);
            }
        });
    }

    let peer = JsonRpcPeer::new(child).map_err(|e| format!("ACP 管道初始化失败: {}", e))?;

    let result = async {
        peer.request_with_timeout(
            acp_method::INITIALIZE,
            serde_json::json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false }
                }
            }),
            ACP_PROBE_TIMEOUT,
        )
        .await?;
        // 认证 / 宿主可用性通常在这一步才暴露（copilot 即如此），握手只做 initialize 会漏判
        peer.request_with_timeout(
            acp_method::SESSION_NEW,
            serde_json::json!({ "cwd": cwd.to_string_lossy(), "mcpServers": [] }),
            ACP_PROBE_TIMEOUT,
        )
        .await?;
        Ok::<(), String>(())
    }
    .await;

    // 探测完立刻结束进程：绝不留下 serving 态的孤儿适配器
    peer.kill().await;

    result.map_err(|e| {
        let extra = stderr_text
            .lock()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let friendly = if e.contains("Authentication required") || e.contains("auth required") {
            "对端要求登录授权，请先在终端完成该 Agent 的登录（如 `copilot login`）".to_string()
        } else {
            e
        };
        if extra.is_empty() {
            friendly
        } else {
            let tail: String = extra.chars().take(200).collect();
            format!("{}（stderr: {}）", friendly, tail)
        }
    })
}

/// 规格里声明为宿主（role=host）的 npm 包是否都已安装。
///
/// 只对 npm 生效：shell 形态的宿主命令（如 `hermes acp`）由启动命令解析覆盖。
async fn missing_host_package(
    agent_mgr: &crate::agent::AgentManager,
    lifecycle: Option<&LifecycleSpec>,
) -> Option<String> {
    let spec = lifecycle?;
    if spec.kind != LifecycleKind::Npm {
        return None;
    }
    let hosts: Vec<String> = spec
        .packages
        .iter()
        .filter(|p| p.role == PackageRole::Host)
        .map(|p| package_without_version(&p.package))
        .collect();
    for name in hosts {
        if name.trim().is_empty() {
            continue;
        }
        let cmd = format!("npm ls -g {} --json", name);
        let installed = agent_mgr
            .execute_command_output_async(&cmd, "", 15, "宿主 Agent 状态查询")
            .await
            .ok()
            .and_then(|out| serde_json::from_str::<serde_json::Value>(&out).ok())
            .and_then(|json| {
                json.get("dependencies")
                    .and_then(|d| d.get(&name))
                    .map(|_| ())
            })
            .is_some();
        if !installed {
            return Some(name);
        }
    }
    None
}

/// 历史/手工本地 CLI 没有生命周期规格时，按**同名预置**补最新版本查询命令。
///
/// 只读市场清单、不写本地配置：预置里 claude / codex / hermes 都带版本来源，
/// 早于市场机制创建的 Agent 因此也能查到最新版本。清单不可用就放弃，不影响检测本身。
async fn preset_latest_commands() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Ok(json) = crate::utils::market::fetch_agents_config().await else {
        return out;
    };
    let Some(list) = json.get("agents").and_then(|v| v.as_array()) else {
        return out;
    };
    for a in list {
        let (Some(agent_type), Some(spec_json)) = (
            a.get("agentType").and_then(|v| v.as_str()),
            a.get("lifecycle"),
        ) else {
            continue;
        };
        let Ok(spec) = serde_json::from_value::<LifecycleSpec>(spec_json.clone()) else {
            continue;
        };
        if let Some(cmd) = spec.commands().ok().and_then(|c| c.latest_version) {
            out.insert(agent_type.to_string(), cmd);
        }
    }
    out
}

/// 简单缓存：仅用于避免高频重复检测
static LAST_DETECT: std::sync::RwLock<Option<(Instant, EnvInfo)>> = std::sync::RwLock::new(None);
const CACHE_TTL: Duration = Duration::from_secs(120);

/// 最近一次检测得到的 Agent 健康度（按 agent_type，各自带时间戳）。
///
/// 会话门禁直接读它：ACP 的可用性判定要真跑一遍握手，不能在每次建会话/发消息时重做。
static LAST_HEALTH: std::sync::RwLock<
    Option<std::collections::HashMap<String, (Instant, AgentHealth)>>,
> = std::sync::RwLock::new(None);

/// 记录一个 Agent 的最新健康度（检测与门禁共用）。
pub(crate) fn remember_agent_health(agent_type: &str, health: &AgentHealth) {
    if let Ok(mut guard) = LAST_HEALTH.write() {
        guard
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(agent_type.to_string(), (Instant::now(), health.clone()));
    }
}

/// 读取尚未过期的健康度缓存；没有或已过期返回 `None`。
pub(crate) fn cached_agent_health(agent_type: &str) -> Option<AgentHealth> {
    let guard = LAST_HEALTH.read().ok()?;
    let (ts, health) = guard.as_ref()?.get(agent_type)?;
    if ts.elapsed() >= CACHE_TTL {
        return None;
    }
    Some(health.clone())
}


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
            if let Ok(v) = agent_mgr
                .execute_command_output_async("python --version", "", 15, "python 版本查询")
                .await
            {
                return Ok(v);
            }
            if let Ok(v) = agent_mgr
                .execute_command_output_async("python3 --version", "", 15, "python3 版本查询")
                .await
            {
                return Ok(v);
            }
            if let Ok(v) = agent_mgr
                .execute_command_output_async("py --version", "", 15, "py 版本查询")
                .await
            {
                return Ok(v);
            }
            Err("未找到 Python".to_string())
        },
    );

    let node_version = node_result.ok().map(|v| clean_version_string(&v));
    let git_version = git_result.ok().map(|v| clean_version_string(&v));
    let python_version = py_result.ok().map(|v| clean_version_string(&v));

    // emit 基础工具检测结果
    let _ = app.emit(
        "env-base-tools",
        serde_json::json!({
            "nodeVersion": node_version,
            "gitVersion": git_version,
            "pythonVersion": python_version,
        }),
    );

    // Agent 版本查询（futures::join_all 并行）
    let conn = state.get_conn()?;
    let db_agents = agents::list_agents_inner(&conn).unwrap_or_default();
    // 接入方式决定检测口径：ACP 查适配器，本地 CLI 查可执行命令
    let transport_map: std::collections::HashMap<String, String> =
        agents::list_agent_transports_inner(&conn)
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.agent_type, r.transport))
            .collect();

    let mut agent_futures = Vec::new();
    for agent in &db_agents {
        if !agent.is_enabled {
            continue;
        }
        let transport = transport_map
            .get(&agent.agent_type)
            .cloned()
            .unwrap_or_else(|| "subprocess".to_string());
        // ACP 的可用性判定要看会话真正 spawn 的命令，因此在 `conn` 里
        let conn_json = agents::get_agent_transport_inner(&conn, &agent.agent_type)
            .ok()
            .and_then(|info| info.conn);
        let agent = agent.clone();
        agent_futures.push(async move {
            let probe = probe_agent(agent_mgr, &agent, &transport, conn_json.as_deref()).await;
            (agent.agent_type, probe)
        });
    }

    let agent_results = futures::future::join_all(agent_futures).await;

    let mut agent_versions = std::collections::HashMap::new();
    let mut agent_health = std::collections::HashMap::new();
    for (agent_type, probe) in agent_results {
        let _ = app.emit(
            "env-agent-version",
            serde_json::json!({
                "agentType": agent_type,
                "version": probe.version,
                "installed": probe.health.installed,
                "available": probe.health.available,
                "reason": probe.health.reason,
            }),
        );
        agent_versions.insert(agent_type.clone(), probe.version);
        remember_agent_health(&agent_type, &probe.health);
        agent_health.insert(agent_type, probe.health);
    }

    log_env!(
        "[env] node={:?} git={:?} python={:?} agents={:?}",
        node_version,
        git_version,
        python_version,
        agent_versions
    );

    let result = EnvInfo {
        node_version,
        git_version,
        python_version,
        agent_versions,
        agent_health,
        agent_latest_versions: None,
    };

    // ── Phase 2: 更新检查（fire-and-forget 后台任务） ──
    let app_clone = app.clone();
    let state_clone: crate::DbState = crate::DbState {
        pool: state.pool.clone(),
    };
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
        let transport_map: std::collections::HashMap<String, String> =
            agents::list_agent_transports_inner(&conn)
                .unwrap_or_default()
                .into_iter()
                .map(|r| (r.agent_type, r.transport))
                .collect();

        // 先归集「查什么命令」：有生命周期规格的直接派生；
        // 没有规格的历史本地 CLI 记下来，稍后按同名预置补齐（否则这类 Agent 永远查不到最新版本）。
        let mut jobs: Vec<(String, String)> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for agent in &db_agents {
            if !agent.is_enabled {
                continue;
            }
            match agent
                .lifecycle
                .as_ref()
                .and_then(|spec| spec.commands().ok())
                .and_then(|cmds| cmds.latest_version)
            {
                Some(cmd) => jobs.push((agent.agent_type.clone(), cmd)),
                None => missing.push(agent.agent_type.clone()),
            }
        }
        if !missing.is_empty() {
            let presets = preset_latest_commands().await;
            for agent_type in missing {
                // 只对本地 CLI 回退；ACP 没有规格说明是自定义连接，不该猜版本来源
                let is_cli = transport_map
                    .get(&agent_type)
                    .map(|t| t == "subprocess")
                    .unwrap_or(true);
                if !is_cli {
                    continue;
                }
                if let Some(cmd) = presets.get(&agent_type) {
                    jobs.push((agent_type, cmd.clone()));
                }
            }
        }

        // Phase 2 并行查询所有 Agent 最新版本
        let mut latest_futures = Vec::new();
        for (agent_type, cmd) in jobs {
            let label = format!("{} 最新版本查询", agent_type);
            let mgr_clone = mgr.clone();
            latest_futures.push(async move {
                let version = mgr_clone
                    .execute_command_output_async(&cmd, "", 120, &label)
                    .await
                    .ok()
                    .map(|v| clean_version_string(&v));
                (agent_type, version)
            });
        }

        let latest_results = futures::future::join_all(latest_futures).await;
        for (agent_type, version) in latest_results {
            let _ = app_clone.emit(
                "env-agent-latest-version",
                serde_json::json!({
                    "agentType": agent_type,
                    "version": version,
                }),
            );
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
    if DETECTING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::Acquire,
            std::sync::atomic::Ordering::Relaxed,
        )
        .is_err()
    {
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

    let state_ref: crate::DbState = crate::DbState {
        pool: state.pool.clone(),
    };
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
    // 接管要按本地接入方式取同名预置（同名可能同时存在 ACP 与 CLI 两条）
    let transport = agents::get_agent_transport_inner(&conn, &agent_type)?.transport;

    // 历史手工配置没有生命周期规格：先按同名预置接管，再取安装/卸载命令
    let spec = match config.lifecycle.as_ref() {
        Some(spec) => spec.commands().map_err(AppError::Config)?,
        None => crate::commands::agent_market::adopt_preset_spec(&state, &agent_type, &transport)
            .await?
            .ok_or_else(|| AppError::Config(format!("{} 未配置生命周期规格", agent_type)))?
            .commands()
            .map_err(AppError::Config)?,
    };

    let cmd = spec
        .install
        .ok_or_else(|| AppError::Config(format!("{} 未配置安装命令", agent_type)))?;

    let _ = app.emit(
        "install-progress",
        serde_json::json!({
            "agent": agent_type,
            "message": format!("正在安装 {}...", config.display_name),
            "progress": 50
        }),
    );

    let mgr = agent_mgr.lock().await;
    mgr.execute_command_output_async(&cmd, "", 300, &format!("安装 {}", agent_type))
        .await
        .map_err(|e| AppError::External(format!("安装 {} 失败: {}", agent_type, e)))?;

    let _ = app.emit(
        "install-progress",
        serde_json::json!({
            "agent": agent_type,
            "message": format!("{} 安装完成", config.display_name),
            "progress": 100
        }),
    );

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
    // 接管要按本地接入方式取同名预置（同名可能同时存在 ACP 与 CLI 两条）
    let transport = agents::get_agent_transport_inner(&conn, &agent_type)?.transport;

    // 历史手工配置没有生命周期规格：先按同名预置接管，再取卸载命令
    let spec = match config.lifecycle.as_ref() {
        Some(spec) => spec.commands().map_err(AppError::Config)?,
        None => crate::commands::agent_market::adopt_preset_spec(&state, &agent_type, &transport)
            .await?
            .ok_or_else(|| AppError::Config(format!("{} 未配置生命周期规格", agent_type)))?
            .commands()
            .map_err(AppError::Config)?,
    };

    let cmd = spec
        .uninstall
        .ok_or_else(|| AppError::Config(format!("{} 未配置卸载命令", agent_type)))?;

    let _ = app.emit(
        "install-progress",
        serde_json::json!({
            "agent": agent_type,
            "message": format!("正在卸载 {}...", config.display_name),
            "progress": 50
        }),
    );

    let mgr = agent_mgr.lock().await;
    mgr.execute_command_output_async(&cmd, "", 300, &format!("卸载 {}", agent_type))
        .await
        .map_err(|e| AppError::External(format!("卸载 {} 失败: {}", agent_type, e)))?;

    let _ = app.emit(
        "install-progress",
        serde_json::json!({
            "agent": agent_type,
            "message": format!("{} 卸载完成", config.display_name),
            "progress": 100
        }),
    );

    // 卸载后把包状态退回「待安装」：否则就绪门禁仍放行，会话会因运行时缺失而失败。
    if config.lifecycle.is_some() {
        let pending = crate::agent::lifecycle::PkgState::pending(crate::utils::now());
        crate::commands::agent_market::set_pkg_state_inner(&conn, &agent_type, &pending)?;
    }

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
        return Err(AppError::InvalidInput("路径不能为空".into()).into());
    }

    let trimmed = path.trim();

    // Expand ~ to user home directory
    let expanded = if trimmed.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            trimmed.replacen("~", &home.to_string_lossy(), 1)
        } else {
            return Err(AppError::Config("无法解析用户主目录路径".into()).into());
        }
    } else {
        trimmed.to_string()
    };

    let p = std::path::Path::new(&expanded);

    #[cfg(target_os = "windows")]
    {
        // Reject bare drive letters like "C:" or "D:"
        if trimmed.len() <= 2 && trimmed.ends_with(':') {
            return Err(AppError::InvalidInput(
                "路径不完整，请输入完整目录路径（如 C:\\Users\\work）".into(),
            )
            .into());
        }

        // Check for invalid path characters on Windows
        let invalid_chars = ['<', '>', '"', '|', '?', '*'];
        for c in invalid_chars {
            if trimmed.contains(c) {
                return Err(AppError::InvalidInput(format!("路径包含非法字符 '{}'", c)).into());
            }
        }

        // Ensure drive letter is valid (A-Z)
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            let drive = bytes[0].to_ascii_uppercase();
            if drive < b'A' || drive > b'Z' {
                return Err(AppError::InvalidInput(
                    "无效的盘符，请输入有效的驱动器号（如 C:、D:）".into(),
                )
                .into());
            }
        }
    }

    // Check if path exists
    if p.exists() {
        if p.is_dir() {
            Ok(expanded.to_string())
        } else {
            Err(AppError::InvalidInput(format!("路径已存在但不是一个目录: {}", expanded)).into())
        }
    } else {
        // Create directory (including parent directories)
        std::fs::create_dir_all(p).map_err(|e| AppError::Io(format!("创建目录失败: {}", e)))?;
        Ok(expanded.to_string())
    }
}
