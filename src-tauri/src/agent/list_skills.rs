#![allow(dead_code)]
//! Agent 技能扫描模块
//!
//! 从文件系统扫描 Agent 技能目录，解析 SKILL.md 文件，
//! 返回技能信息列表。从 agent/mod.rs 中提取的独立关注点。

use crate::db::models::SkillInfo;

/// 获取用户 home 目录（跨平台）
pub fn home_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

/// 解析单个 SKILL.md 文件，提取 name 和 description
pub fn parse_skill_md(path: &std::path::Path) -> Option<SkillInfo> {
    let raw = std::fs::read_to_string(path).ok()?;
    let raw = raw.trim();
    if !raw.starts_with("---") {
        return None;
    }
    // 找到第二个 "---"
    let end = raw[3..].find("---")?;
    let frontmatter = &raw[3..3 + end];

    // 用 serde_yaml 解析 frontmatter
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter).ok()?;
    let mapping = value.as_mapping()?;

    let name = mapping.get(&serde_yaml::Value::String("name".into()))?
        .as_str()?.to_string();
    let description = mapping.get(&serde_yaml::Value::String("description".into()))?
        .as_str()?.trim_matches('"').to_string();

    Some(SkillInfo::new(&name, &description, ""))
}

/// 扫描技能目录
///
/// display_mode: recursive（递归显示全部）或 collection（只显示集合名）
pub fn scan_skills_dir(skills_dir: &std::path::Path, entry_file: &str, display_mode: &str) -> Vec<SkillInfo> {
    if !skills_dir.exists() || !skills_dir.is_dir() {
        return vec![];
    }

    let mut skills = Vec::new();

    // 如果当前目录有入口文件，直接解析并返回
    let own_skill = skills_dir.join(entry_file);
    if own_skill.exists() {
        if let Some(info) = parse_skill_md(&own_skill) {
            skills.push(info);
        }
        // collection 模式：只显示集合名，不递归子目录
        if display_mode == "collection" {
            return skills;
        }
        // recursive 模式：解析入口文件后继续递归子目录
        if display_mode != "recursive" {
            return skills;
        }
    }

    // 递归遍历子目录
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            skills.extend(scan_skills_dir(&path, entry_file, display_mode));
        }
    }
    skills
}
