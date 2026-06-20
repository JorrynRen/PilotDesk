use serde::{Deserialize, Serialize};
use rusqlite::{params, Connection};
use crate::utils::errors::AppError;
use crate::utils::now;
use base64::Engine;

// ──────────────────────────────────────────────
//  数据模型
// ──────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// Agent 类型标识（claude / hermes / codex 等）
    pub agent_type: String,
    /// 显示名称（如 "Claude Code"）
    pub display_name: String,
    /// 描述
    pub description: String,
    /// CLI 命令名
    pub cli_command: String,
    /// npm 包名（None 表示非 npm 安装）
    pub npm_package: Option<String>,
    /// pip 包名（None 表示非 pip 安装）
    pub pip_package: Option<String>,
    /// 安装命令
    pub install_cmd: String,
    /// 卸载命令
    pub uninstall_cmd: String,
    /// 更新命令
    pub update_cmd: String,
    /// 版本检测命令
    pub version_cmd: String,
    /// 最新版本查询命令
    pub latest_version_cmd: String,
    /// 启动命令模板，{message} 占位符替换
    pub run_cmd_template: String,
    /// 输出解析器类型
    pub output_parser: String,
    /// 噪声行过滤正则
    pub output_filter_regex: String,
    /// 版本号提取正则
    pub version_pattern: String,
    /// 是否支持会话延续
    pub supports_session_continuity: bool,
    /// session_id 来源
    pub session_id_source: String,
    /// JSON 事件类型
    pub session_id_event_type: String,
    /// JSON 字段名
    pub session_id_field: String,
    /// 恢复参数模板
    pub resume_arg_template: String,
    /// 技能目录路径（支持 {agent_type} 占位符），空字符串表示使用智能目录 ~/.{agent_type}/skills/
    pub skills_dir: String,
    /// 技能入口文件名（默认 SKILL.md）
    pub skill_entry_file: String,
    /// 技能显示模式：recursive（递归显示全部）或 collection（只显示集合名）
    pub skill_display_mode: String,
    /// UI 主题色
    pub color: String,
    /// UI 图标
    pub icon: String,
    /// 排序序号
    pub sort_order: i64,
    /// 是否启用
    pub is_enabled: bool,
    /// 是否为预置 Agent
    pub is_builtin: bool,
    /// 版本号（用于 Agent 市场更新检测）
    pub version: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentPayload {
    pub agent_type: String,
    pub display_name: String,
    pub description: Option<String>,
    pub cli_command: String,
    pub npm_package: Option<String>,
    pub pip_package: Option<String>,
    pub install_cmd: Option<String>,
    pub uninstall_cmd: Option<String>,
    pub update_cmd: Option<String>,
    pub version_cmd: Option<String>,
    pub latest_version_cmd: Option<String>,
    pub run_cmd_template: Option<String>,
    pub output_parser: Option<String>,
    pub output_filter_regex: Option<String>,
    pub version_pattern: Option<String>,
    pub supports_session_continuity: Option<bool>,
    pub session_id_source: Option<String>,
    pub session_id_event_type: Option<String>,
    pub session_id_field: Option<String>,
    pub resume_arg_template: Option<String>,
    pub skills_dir: Option<String>,
    pub skill_entry_file: Option<String>,
    pub skill_display_mode: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub sort_order: Option<i64>,
    pub is_enabled: Option<bool>,
    /// 版本号
    pub version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentPayload {
    pub agent_type: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub cli_command: Option<String>,
    pub npm_package: Option<String>,
    pub pip_package: Option<String>,
    pub install_cmd: Option<String>,
    pub uninstall_cmd: Option<String>,
    pub update_cmd: Option<String>,
    pub version_cmd: Option<String>,
    pub latest_version_cmd: Option<String>,
    pub run_cmd_template: Option<String>,
    pub output_parser: Option<String>,
    pub output_filter_regex: Option<String>,
    pub version_pattern: Option<String>,
    pub supports_session_continuity: Option<bool>,
    pub session_id_source: Option<String>,
    pub session_id_event_type: Option<String>,
    pub session_id_field: Option<String>,
    pub resume_arg_template: Option<String>,
    pub skills_dir: Option<String>,
    pub skill_entry_file: Option<String>,
    pub skill_display_mode: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub sort_order: Option<i64>,
    pub is_enabled: Option<bool>,
    /// 版本号
    pub version: Option<String>,
}

// ──────────────────────────────────────────────
//  CRUD 命令
// ──────────────────────────────────────────────

#[tauri::command]
pub fn list_agents(state: tauri::State<'_, crate::DbState>) -> Result<Vec<AgentConfig>, AppError> {
    let conn = state.get_conn()?;
    list_agents_inner(&conn)
}

#[tauri::command]
pub fn get_agent(state: tauri::State<'_, crate::DbState>, agent_type: String) -> Result<Option<AgentConfig>, AppError> {
    let conn = state.get_conn()?;
    get_agent_inner(&conn, &agent_type)
}

#[tauri::command]
pub fn add_agent(state: tauri::State<'_, crate::DbState>, payload: CreateAgentPayload) -> Result<AgentConfig, AppError> {
    let conn = state.get_conn()?;
    add_agent_inner(&conn, payload)
}

#[tauri::command]
pub fn update_agent(state: tauri::State<'_, crate::DbState>, payload: UpdateAgentPayload) -> Result<AgentConfig, AppError> {
    let conn = state.get_conn()?;
    update_agent_inner(&conn, payload)
}

#[tauri::command]
pub fn delete_agent(state: tauri::State<'_, crate::DbState>, agent_type: String) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    delete_agent_inner(&conn, &agent_type)
}

// ──────────────────────────────────────────────
//  导入/导出 Agent 配置（JSON）
// ──────────────────────────────────────────────

#[tauri::command]
pub fn export_agents_json(state: tauri::State<'_, crate::DbState>, file_path: String) -> Result<(), AppError> {
    let conn = state.get_conn()?;
    let agents = list_agents_inner(&conn)?;
    // 使用与 Agent 市场一致的格式：{ version: 1, agents: [...] }
    let output = serde_json::json!({
        "version": 1,
        "agents": agents
    });
    let json = serde_json::to_string_pretty(&output)
        .map_err(|e| AppError::External(format!("JSON 序列化失败: {}", e)))?;
    std::fs::write(&file_path, json)
        .map_err(|e| AppError::External(format!("写入文件失败: {}", e)))?;
    Ok(())
}

#[tauri::command]
pub fn import_agents_json(state: tauri::State<'_, crate::DbState>, file_path: String) -> Result<ImportResult, AppError> {
    let conn = state.get_conn()?;
    let json = std::fs::read_to_string(&file_path)
        .map_err(|e| AppError::External(format!("读取文件失败: {}", e)))?;

    // 兼容两种格式：{ version: 1, agents: [...] } 或 [...]（旧格式）
    let imported: Vec<AgentConfig> = {
        // 先尝试解析为包装格式
        if let Ok(wrapped) = serde_json::from_str::<serde_json::Value>(&json) {
            if let Some(agents) = wrapped.get("agents").and_then(|a| a.as_array()) {
                serde_json::from_value(serde_json::Value::Array(agents.clone()))
                    .map_err(|e| AppError::InvalidInput(format!("JSON 解析失败: {}", e)))?
            } else {
                serde_json::from_str(&json)
                    .map_err(|e| AppError::InvalidInput(format!("JSON 解析失败: {}", e)))?
            }
        } else {
            serde_json::from_str(&json)
                .map_err(|e| AppError::InvalidInput(format!("JSON 解析失败: {}", e)))?
        }
    };

    let mut success = 0u32;
    let mut errors = Vec::new();

    for agent in &imported {
        if agent.agent_type.is_empty() || agent.display_name.is_empty() || agent.cli_command.is_empty() {
            errors.push(format!("跳过无效配置: {}", agent.display_name));
            continue;
        }
        // Try add first, if exists then update
        match add_agent_inner(&conn, CreateAgentPayload {
            agent_type: agent.agent_type.clone(),
            display_name: agent.display_name.clone(),
            description: Some(agent.description.clone()),
            cli_command: agent.cli_command.clone(),
            npm_package: agent.npm_package.clone(),
            pip_package: agent.pip_package.clone(),
            install_cmd: Some(agent.install_cmd.clone()),
            uninstall_cmd: Some(agent.uninstall_cmd.clone()),
            update_cmd: Some(agent.update_cmd.clone()),
            version_cmd: Some(agent.version_cmd.clone()),
            latest_version_cmd: Some(agent.latest_version_cmd.clone()),
            run_cmd_template: Some(agent.run_cmd_template.clone()),
            output_parser: Some(agent.output_parser.clone()),
            output_filter_regex: Some(agent.output_filter_regex.clone()),
            version_pattern: Some(agent.version_pattern.clone()),
            supports_session_continuity: Some(agent.supports_session_continuity),
            session_id_source: Some(agent.session_id_source.clone()),
            session_id_event_type: Some(agent.session_id_event_type.clone()),
            session_id_field: Some(agent.session_id_field.clone()),
            resume_arg_template: Some(agent.resume_arg_template.clone()),
            skills_dir: Some(agent.skills_dir.clone()),
            skill_entry_file: Some(agent.skill_entry_file.clone()),
            skill_display_mode: Some(agent.skill_display_mode.clone()),
            color: Some(agent.color.clone()),
            icon: Some(agent.icon.clone()),
            sort_order: Some(agent.sort_order),
            is_enabled: Some(agent.is_enabled),
            version: Some(agent.version.clone()),
        }) {
            Ok(_) => success += 1,
            Err(_) => {
                // Agent exists, try update
                match update_agent_inner(&conn, UpdateAgentPayload {
                    agent_type: agent.agent_type.clone(),
                    display_name: Some(agent.display_name.clone()),
                    description: Some(agent.description.clone()),
                    cli_command: Some(agent.cli_command.clone()),
                    npm_package: agent.npm_package.clone(),
                    pip_package: agent.pip_package.clone(),
                    install_cmd: Some(agent.install_cmd.clone()),
                    uninstall_cmd: Some(agent.uninstall_cmd.clone()),
                    update_cmd: Some(agent.update_cmd.clone()),
                    version_cmd: Some(agent.version_cmd.clone()),
                    latest_version_cmd: Some(agent.latest_version_cmd.clone()),
                    run_cmd_template: Some(agent.run_cmd_template.clone()),
                    output_parser: Some(agent.output_parser.clone()),
                    output_filter_regex: Some(agent.output_filter_regex.clone()),
                    version_pattern: Some(agent.version_pattern.clone()),
                    supports_session_continuity: Some(agent.supports_session_continuity),
                    session_id_source: Some(agent.session_id_source.clone()),
                    session_id_event_type: Some(agent.session_id_event_type.clone()),
                    session_id_field: Some(agent.session_id_field.clone()),
                    resume_arg_template: Some(agent.resume_arg_template.clone()),
                    skills_dir: Some(agent.skills_dir.clone()),
                    skill_entry_file: Some(agent.skill_entry_file.clone()),
                    skill_display_mode: Some(agent.skill_display_mode.clone()),
                    color: Some(agent.color.clone()),
                    icon: Some(agent.icon.clone()),
                    sort_order: Some(agent.sort_order),
                    is_enabled: Some(agent.is_enabled),
                    version: Some(agent.version.clone()),
                }) {
                    Ok(_) => success += 1,
                    Err(e) => errors.push(format!("{}: {}", agent.agent_type, e)),
                }
            }
        }
    }

    Ok(ImportResult { success, errors })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImportResult {
    pub success: u32,
    pub errors: Vec<String>,
}

/// 上传用户选择的图片作为 Agent 图标
/// 将图片复制到用户资源目录的 icons/ 下（%APPDATA%/com.pilotdesk.app/resources/icons/），命名为 {agentType}_icon.{ext}
#[tauri::command]
pub fn upload_agent_icon(
    agent_type: String,
    source_path: String,
    resources: tauri::State<'_, crate::ResourcePaths>,
) -> Result<String, AppError> {
    let source = std::path::Path::new(&source_path);
    if !source.exists() {
        return Err(AppError::NotFound(format!("源文件不存在: {}", source_path)));
    }

    // 获取文件扩展名
    let ext = source.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();

    // 验证扩展名
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "ico" | "svg" | "webp" => {},
        _ => return Err(AppError::InvalidInput(format!("不支持的图片格式: .{} (支持: png/jpg/gif/ico/svg/webp)", ext))),
    }

    // 目标文件名: {agentType}_icon.{ext}
    let file_name = format!("{}_icon.{}", agent_type, ext);
    let dest = resources.user.join("icons").join(&file_name);

    // 确保目标目录存在
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Io(format!("创建目录失败: {}", e)))?;
    }

    // 复制文件
    std::fs::copy(&source, &dest)
        .map_err(|e| AppError::Io(format!("复制图标文件失败: {}", e)))?;

    // 返回 file: 协议路径
    Ok(format!("file:{}", file_name))
}

#[tauri::command]
pub fn read_agent_icon(icon_name: String, resources: tauri::State<'_, crate::ResourcePaths>) -> Result<String, AppError> {
    // icon_name: "claude_icon.ico" (不含 file: 前缀)
    // 路径规则（按优先级）：
    //   1. 用户资源目录: user/icons/xxx.ico（用户上传的自定义图标）
    //   2. 内置资源目录: builtin/resources/icons/xxx.ico（生产模式）
    //   3. Dev 模式:     exe/../../resources/icons/xxx.ico
    let icon_path = {
        // 1. 用户资源目录
        let user_path = resources.user.join("icons").join(&icon_name);
        if user_path.exists() {
            user_path
        } else {
            // 2. 内置资源目录（生产模式）
            let builtin_path = resources.builtin.join("resources").join("icons").join(&icon_name);
            if builtin_path.exists() {
                builtin_path
            } else {
                // 3. Dev 模式 fallback
                let mut dev_path = std::env::current_exe()
                    .map_err(|e| AppError::Io(format!("获取 exe 路径失败: {}", e)))?;
                dev_path.pop();
                dev_path.pop();
                dev_path.pop();
                dev_path.push("resources");
                dev_path.push("icons");
                dev_path.push(&icon_name);
                if dev_path.exists() {
                    dev_path
                } else {
                    return Err(AppError::NotFound(format!(
                        "图标文件不存在 (已尝试 user/builtin/dev 路径): {}",
                        icon_name
                    )));
                }
            }
        }
    };
    let data = std::fs::read(&icon_path)
        .map_err(|e| AppError::Io(format!("读取图标文件失败: {}", e)))?;
    // 检测文件扩展名确定 MIME 类型
    let mime = match icon_path.extension().and_then(|e| e.to_str()) {
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok(format!("data:{};base64,{}", mime, b64))
}

// ──────────────────────────────────────────────
//  内部实现（可被其他模块调用）
// ──────────────────────────────────────────────

const SELECT_COLS: &str = "agent_type, display_name, description, cli_command, npm_package, pip_package,
    install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
    output_parser, output_filter_regex, version_pattern, supports_session_continuity,
    session_id_source, session_id_event_type, session_id_field, resume_arg_template,
    skills_dir, skill_entry_file, skill_display_mode,
    color, icon, sort_order, is_enabled, is_builtin, version, created_at, updated_at";

fn row_to_agent(row: &rusqlite::Row) -> rusqlite::Result<AgentConfig> {
    Ok(AgentConfig {
        agent_type: row.get(0)?,
        display_name: row.get(1)?,
        description: row.get(2)?,
        cli_command: row.get(3)?,
        npm_package: row.get(4)?,
        pip_package: row.get(5)?,
        install_cmd: row.get(6)?,
        uninstall_cmd: row.get(7)?,
        update_cmd: row.get(8)?,
        version_cmd: row.get(9)?,
        latest_version_cmd: row.get(10)?,
        run_cmd_template: row.get(11)?,
        output_parser: row.get(12)?,
        output_filter_regex: row.get(13)?,
        version_pattern: row.get(14)?,
        supports_session_continuity: row.get::<_, i64>(15)? != 0,
        session_id_source: row.get(16)?,
        session_id_event_type: row.get(17)?,
        session_id_field: row.get(18)?,
        resume_arg_template: row.get(19)?,
        skills_dir: row.get(20)?,
        skill_entry_file: row.get(21)?,
        skill_display_mode: row.get(22)?,
        color: row.get(23)?,
        icon: row.get(24)?,
        sort_order: row.get(25)?,
        is_enabled: row.get::<_, i64>(26)? != 0,
        is_builtin: row.get::<_, i64>(27)? != 0,
        version: row.get(28)?,
        created_at: row.get(29)?,
        updated_at: row.get(30)?,
    })
}

pub fn list_agents_inner(conn: &Connection) -> Result<Vec<AgentConfig>, AppError> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM agents ORDER BY sort_order ASC, agent_type ASC", SELECT_COLS)
    )?;

    let agents = stmt.query_map([], |row| row_to_agent(row))?;
    let mut result = Vec::new();
    for agent in agents {
        result.push(agent?);
    }
    Ok(result)
}

pub fn get_agent_inner(conn: &Connection, agent_type: &str) -> Result<Option<AgentConfig>, AppError> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM agents WHERE agent_type = ?1", SELECT_COLS)
    )?;

    let mut rows = stmt.query_map(params![agent_type], |row| row_to_agent(row))?;
    match rows.next() {
        Some(Ok(agent)) => Ok(Some(agent)),
        Some(Err(e)) => Err(AppError::Db(e.to_string())),
        None => Ok(None),
    }
}

pub fn add_agent_inner(conn: &Connection, payload: CreateAgentPayload) -> Result<AgentConfig, AppError> {
    let now_ts = now();
    let description = payload.description.unwrap_or_default();
    let install_cmd = payload.install_cmd.unwrap_or_default();
    let uninstall_cmd = payload.uninstall_cmd.unwrap_or_default();
    let update_cmd = payload.update_cmd.unwrap_or_default();
    let version_cmd = payload.version_cmd.unwrap_or_else(|| format!("{} --version", payload.cli_command));
    let latest_version_cmd = payload.latest_version_cmd.unwrap_or_default();
    let run_cmd_template = payload.run_cmd_template.unwrap_or_else(|| format!("{} {{message}}", payload.cli_command));
    let output_parser = payload.output_parser.unwrap_or_else(|| "raw-text".to_string());
    let output_filter_regex = payload.output_filter_regex.unwrap_or_default();
    let version_pattern = payload.version_pattern.unwrap_or_else(|| r"v?(\d+\.\d+\.\d+[\w.-]*)".to_string());
    let supports_session_continuity = if payload.supports_session_continuity.unwrap_or(false) { 1 } else { 0 };
    let session_id_source = payload.session_id_source.unwrap_or_else(|| "none".to_string());
    let session_id_event_type = payload.session_id_event_type.unwrap_or_default();
    let session_id_field = payload.session_id_field.unwrap_or_default();
    let resume_arg_template = payload.resume_arg_template.unwrap_or_default();
    let skills_dir = payload.skills_dir.unwrap_or_default();
    let skill_entry_file = payload.skill_entry_file.unwrap_or_else(|| "SKILL.md".to_string());
    let skill_display_mode = payload.skill_display_mode.unwrap_or_else(|| "recursive".to_string());
    let color = payload.color.unwrap_or_else(|| "#6366F1".to_string());
    let icon = payload.icon.unwrap_or_default();  // 空字符串表示使用首字母
    let sort_order = payload.sort_order.unwrap_or(0);
    let version = payload.version.unwrap_or_default();
    let is_enabled = if payload.is_enabled.unwrap_or(true) { 1 } else { 0 };

    conn.execute(
        "INSERT INTO agents (agent_type, display_name, description, cli_command, npm_package, pip_package,
         install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
         output_parser, output_filter_regex, version_pattern, supports_session_continuity,
         session_id_source, session_id_event_type, session_id_field, resume_arg_template,
         skills_dir, skill_entry_file, skill_display_mode,
         color, icon, sort_order, is_enabled, is_builtin, version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, 0, ?28, ?29, ?29)",
        params![
            payload.agent_type, payload.display_name, description, payload.cli_command,
            payload.npm_package, payload.pip_package,
            install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd, run_cmd_template,
            output_parser, output_filter_regex, version_pattern, supports_session_continuity,
            session_id_source, session_id_event_type, session_id_field, resume_arg_template,
            skills_dir, skill_entry_file, skill_display_mode,
            color, icon, sort_order, is_enabled, version, now_ts
        ],
    )?;

    Ok(AgentConfig {
        agent_type: payload.agent_type,
        display_name: payload.display_name,
        description,
        cli_command: payload.cli_command,
        npm_package: payload.npm_package,
        pip_package: payload.pip_package,
        install_cmd,
        uninstall_cmd,
        update_cmd,
        version_cmd,
        latest_version_cmd,
        run_cmd_template,
        output_parser,
        output_filter_regex,
        version_pattern,
        supports_session_continuity: supports_session_continuity != 0,
        session_id_source,
        session_id_event_type,
        session_id_field,
        resume_arg_template,
        skills_dir,
        skill_entry_file,
        skill_display_mode,
        color,
        icon,
        sort_order,
        is_enabled: is_enabled != 0,
        is_builtin: false,
        version,
        created_at: now_ts,
        updated_at: now_ts,
    })
}

pub fn update_agent_inner(conn: &Connection, payload: UpdateAgentPayload) -> Result<AgentConfig, AppError> {
    let existing = get_agent_inner(conn, &payload.agent_type)?
        .ok_or_else(|| AppError::NotFound(format!("Agent 类型 '{}' 不存在", payload.agent_type)))?;

    let now_ts = now();
    let display_name = payload.display_name.unwrap_or(existing.display_name);
    let description = payload.description.unwrap_or(existing.description);
    let cli_command = payload.cli_command.unwrap_or(existing.cli_command);
    let npm_package = payload.npm_package.or(existing.npm_package);
    let pip_package = payload.pip_package.or(existing.pip_package);
    let install_cmd = payload.install_cmd.unwrap_or(existing.install_cmd);
    let uninstall_cmd = payload.uninstall_cmd.unwrap_or(existing.uninstall_cmd);
    let update_cmd = payload.update_cmd.unwrap_or(existing.update_cmd);
    let version_cmd = payload.version_cmd.unwrap_or(existing.version_cmd);
    let latest_version_cmd = payload.latest_version_cmd.unwrap_or(existing.latest_version_cmd);
    let run_cmd_template = payload.run_cmd_template.unwrap_or(existing.run_cmd_template);
    let output_parser = payload.output_parser.unwrap_or(existing.output_parser);
    let output_filter_regex = payload.output_filter_regex.unwrap_or(existing.output_filter_regex);
    let version_pattern = payload.version_pattern.unwrap_or(existing.version_pattern);
    let supports_session_continuity = if let Some(v) = payload.supports_session_continuity { if v { 1 } else { 0 } } else { if existing.supports_session_continuity { 1 } else { 0 } };
    let session_id_source = payload.session_id_source.unwrap_or(existing.session_id_source);
    let session_id_event_type = payload.session_id_event_type.unwrap_or(existing.session_id_event_type);
    let session_id_field = payload.session_id_field.unwrap_or(existing.session_id_field);
    let resume_arg_template = payload.resume_arg_template.unwrap_or(existing.resume_arg_template);
    let skills_dir = payload.skills_dir.unwrap_or(existing.skills_dir);
    let skill_entry_file = payload.skill_entry_file.unwrap_or(existing.skill_entry_file);
    let skill_display_mode = payload.skill_display_mode.unwrap_or(existing.skill_display_mode);
    let color = payload.color.unwrap_or(existing.color);
    let icon = payload.icon.unwrap_or(existing.icon);  // None=保留旧值, Some("")=清空
    let sort_order = payload.sort_order.unwrap_or(existing.sort_order);
    let version = payload.version.unwrap_or(existing.version);
    let is_enabled = if let Some(v) = payload.is_enabled { if v { 1 } else { 0 } } else { if existing.is_enabled { 1 } else { 0 } };

    conn.execute(
        "UPDATE agents SET display_name=?1, description=?2, cli_command=?3, npm_package=?4,
         pip_package=?5, install_cmd=?6, uninstall_cmd=?7, update_cmd=?8,
         version_cmd=?9, latest_version_cmd=?10, run_cmd_template=?11, output_parser=?12,
         output_filter_regex=?13, version_pattern=?14, supports_session_continuity=?15,
         session_id_source=?16, session_id_event_type=?17, session_id_field=?18,
         resume_arg_template=?19, skills_dir=?20, skill_entry_file=?21, skill_display_mode=?22,
         color=?23, icon=?24, sort_order=?25, is_enabled=?26, version=?27,
         updated_at=?28 WHERE agent_type=?29",
        params![display_name, description, cli_command, npm_package, pip_package,
            install_cmd, uninstall_cmd, update_cmd, version_cmd,
            latest_version_cmd, run_cmd_template, output_parser, output_filter_regex,
            version_pattern, supports_session_continuity, session_id_source,
            session_id_event_type, session_id_field, resume_arg_template,
            skills_dir, skill_entry_file, skill_display_mode,
            color, icon, sort_order, is_enabled, version, now_ts, payload.agent_type],
    )?;

    Ok(AgentConfig {
        agent_type: payload.agent_type,
        display_name,
        description,
        cli_command,
        npm_package,
        pip_package,
        install_cmd,
        uninstall_cmd,
        update_cmd,
        version_cmd,
        latest_version_cmd,
        run_cmd_template,
        output_parser,
        output_filter_regex,
        version_pattern,
        supports_session_continuity: supports_session_continuity != 0,
        session_id_source,
        session_id_event_type,
        session_id_field,
        resume_arg_template,
        skills_dir,
        skill_entry_file,
        skill_display_mode,
        color,
        icon,
        sort_order,
        is_enabled: is_enabled != 0,
        is_builtin: existing.is_builtin,
        version,
        created_at: existing.created_at,
        updated_at: now_ts,
    })
}

pub fn delete_agent_inner(conn: &Connection, agent_type: &str) -> Result<(), AppError> {
    // Prevent deletion of builtin agents
    let existing = get_agent_inner(conn, agent_type)?;
    if let Some(agent) = &existing {
        if agent.is_builtin {
            return Err(AppError::InvalidInput(format!("预置 Agent '{}' 不可删除", agent_type)));
        }
    }
    let affected = conn.execute("DELETE FROM agents WHERE agent_type = ?1", params![agent_type])?;
    if affected == 0 {
        return Err(AppError::NotFound(format!("Agent 类型 '{}' 不存在", agent_type)));
    }
    Ok(())
}
