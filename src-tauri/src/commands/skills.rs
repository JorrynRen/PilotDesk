//! 技能管理命令：安装 / 卸载 / 主文件读写
//!
//! 设计取舍：前端只传「Agent + 路径」，**路径一律在 Rust 侧用该 Agent 的技能根做越界校验**
//! （`agent::list_skills::ensure_within`）。把这些能力暴露成"任意路径读写"会让前端拿到
//! 删除/改写任意文件的能力，所以命令层只做"解析技能根 + 转交模块函数"。

use std::path::{Path, PathBuf};

use crate::agent::list_skills;
use crate::db::models::SkillInfo;
use crate::utils::errors::AppError;
use crate::DbState;

/// 解析某 Agent 的技能根与入口文件名。
///
/// 未配置技能目录时直接报错（而不是猜一个路径）—— 猜错会把技能装到用户不期望的位置。
fn skills_root_for(state: &DbState, agent_type: &str) -> Result<(PathBuf, String), AppError> {
    // API Agent 的技能目录固定为 <PilotDesk 配置根>/skills，不随 Agent 配置走
    if agent_type == "api" {
        let cfg_dir = crate::api_agent::system_prompt::get_pilotdesk_config_dir()
            .ok_or_else(|| AppError::Config("无法定位 PilotDesk 配置目录".to_string()))?;
        return Ok((
            PathBuf::from(cfg_dir).join("skills"),
            list_skills::DEFAULT_ENTRY_FILE.to_string(),
        ));
    }

    let conn = state.get_conn()?;
    let agent = crate::commands::agents::get_agent_inner(&conn, agent_type)
        .map_err(|e| AppError::Config(format!("Agent「{}」不可用: {}", agent_type, e)))?
        .ok_or_else(|| AppError::NotFound(format!("Agent「{}」不存在", agent_type)))?;
    if agent.skills_dir.trim().is_empty() {
        return Err(AppError::Config(format!(
            "Agent「{}」未配置技能目录：请先到 设置 › Agent集成配置 填写「技能目录路径」",
            agent_type
        )));
    }
    let root = list_skills::resolve_skills_dir(&agent.skills_dir, agent_type).ok_or_else(|| {
        AppError::Config("技能目录路径无法解析（含 ~ 但取不到用户主目录）".to_string())
    })?;
    let entry = if agent.skill_entry_file.trim().is_empty() {
        list_skills::DEFAULT_ENTRY_FILE.to_string()
    } else {
        agent.skill_entry_file.clone()
    };
    Ok((root, entry))
}

/// 安装技能：`source_path` 为技能目录或 .zip 包；同名目录已存在时拒绝（不静默覆盖）
#[tauri::command]
pub async fn skill_install(
    state: tauri::State<'_, DbState>,
    agent_type: String,
    source_path: String,
) -> Result<SkillInfo, String> {
    let (root, entry) = skills_root_for(&state, &agent_type)?;
    list_skills::install_skill(&root, &entry, Path::new(&source_path))
}

/// 卸载技能：删除该技能目录（前端需二次确认；根目录本身与根外路径一律拒绝）
#[tauri::command]
pub fn skill_uninstall(
    state: tauri::State<'_, DbState>,
    agent_type: String,
    dir_path: String,
) -> Result<(), String> {
    let (root, _) = skills_root_for(&state, &agent_type)?;
    list_skills::uninstall_skill(&root, Path::new(&dir_path))
}

/// 读取技能主文件（编辑用）
#[tauri::command]
pub fn skill_read_entry(
    state: tauri::State<'_, DbState>,
    agent_type: String,
    entry_path: String,
) -> Result<String, String> {
    let (root, _) = skills_root_for(&state, &agent_type)?;
    list_skills::read_entry(&root, Path::new(&entry_path))
}

/// 写回技能主文件（保存前校验 frontmatter，改坏成扫不出来的技能会被拒绝）
#[tauri::command]
pub fn skill_write_entry(
    state: tauri::State<'_, DbState>,
    agent_type: String,
    entry_path: String,
    content: String,
) -> Result<(), String> {
    let (root, _) = skills_root_for(&state, &agent_type)?;
    list_skills::write_entry(&root, Path::new(&entry_path), &content)
}
