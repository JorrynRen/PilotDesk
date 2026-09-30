//! 技能加载器
//!
//! 扫描技能目录，解析 SKILL.md 文件的 YAML frontmatter，
//! 支持 Progressive Disclosure 模式（先注入 name+description，按需加载完整内容）。

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// SKILL.md frontmatter 结构
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
}

/// 技能完整信息
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub name: String,
    pub description: String,
    pub full_content: String,
}

/// 技能加载器
#[derive(Clone)]
pub struct SkillLoader {
    skills_dir: Option<String>,
    skills: Vec<LoadedSkill>,
    /// 技能名 → 技能所在目录（用于 load_skill 返回时注入技能根目录，供脚本绝对路径定位）
    dirs: HashMap<String, PathBuf>,
}

impl SkillLoader {
    /// 创建技能加载器并扫描目录
    pub fn new(skills_dir: Option<String>) -> Self {
        let mut loader = Self {
            skills_dir,
            skills: Vec::new(),
            dirs: HashMap::new(),
        };
        loader.scan();
        loader
    }

    /// 获取技能名+描述列表（用于 System Prompt Progressive Disclosure）
    pub fn list_skills(&self) -> Vec<crate::api_agent::system_prompt::SkillEntry> {
        self.skills
            .iter()
            .map(|s| crate::api_agent::system_prompt::SkillEntry {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect()
    }

    /// 按名称加载完整技能内容
    pub fn load_skill(&self, name: &str) -> Option<String> {
        // 精确匹配
        if let Some(skill) = self.skills.iter().find(|s| s.name == name) {
            return Some(skill.full_content.clone());
        }
        // 模糊匹配（忽略大小写）
        let name_lower = name.to_lowercase();
        self.skills
            .iter()
            .find(|s| s.name.to_lowercase() == name_lower)
            .map(|s| s.full_content.clone())
    }

    /// 获取技能数量
    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.skills.len()
    }

    /// 获取技能所在目录的绝对路径（精确匹配，回退忽略大小写）。
    /// 供 `load_skill` 返回内容时注入技能根目录，模型据此以绝对路径调用技能脚本。
    pub fn skill_dir(&self, name: &str) -> Option<PathBuf> {
        self.dirs.get(name).cloned().or_else(|| {
            self.dirs
                .iter()
                .find(|(k, _)| k.to_lowercase() == name.to_lowercase())
                .map(|(_, v)| v.clone())
        })
    }

    /// 扫描技能目录
    fn scan(&mut self) {
        let dir = match &self.skills_dir {
            Some(d) if !d.is_empty() => d.clone(),
            _ => return,
        };

        let skills_path = std::path::Path::new(&dir);
        if !skills_path.exists() || !skills_path.is_dir() {
            return;
        }

        // 递归扫描所有 SKILL.md 文件
        self.scan_dir(skills_path);
    }

    fn scan_dir(&mut self, dir: &std::path::Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.scan_dir(&path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
                self.try_load_skill_file(&path);
            }
        }
    }

    fn try_load_skill_file(&mut self, path: &std::path::Path) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };

        // 解析 YAML frontmatter（--- ... ---）
        let (name, description) = parse_frontmatter(&content);

        // 记录技能所在目录（SKILL.md 的父目录）
        if let Some(dir) = path.parent() {
            self.dirs.insert(name.clone(), dir.to_path_buf());
        }

        self.skills.push(LoadedSkill {
            name,
            description,
            full_content: content,
        });
    }
}

/// 解析 SKILL.md 的 YAML frontmatter
///
/// 格式：
/// ```markdown
/// ---
/// name: my_skill
/// description: does something useful
/// ---
///
/// # Skill content...
/// ```
fn parse_frontmatter(content: &str) -> (String, String) {
    let trimmed = content.trim();

    // 检查是否以 --- 开头
    if !trimmed.starts_with("---") {
        // 没有 frontmatter，使用文件名作为 name，第一行作为 description
        let first_line = trimmed.lines().next().unwrap_or("Unknown Skill").trim();
        let name = first_line
            .trim_start_matches('#')
            .trim()
            .to_lowercase()
            .replace(' ', "_");
        let desc = first_line.trim_start_matches('#').trim().to_string();
        return (name, desc);
    }

    // 查找闭合的 ---
    let rest = &trimmed[3..]; // 跳过开头的 ---
    let end_idx = match rest.find("\n---") {
        Some(i) => i,
        None => {
            // 没有闭合的 ---，回退
            let first_line = trimmed.lines().nth(1).unwrap_or("Unknown Skill").trim();
            let name = first_line
                .trim_start_matches('#')
                .trim()
                .to_lowercase()
                .replace(' ', "_");
            let desc = first_line.trim_start_matches('#').trim().to_string();
            return (name, desc);
        }
    };

    let yaml_str = &rest[..end_idx].trim();

    // 解析 YAML
    match serde_yaml::from_str::<SkillFrontmatter>(yaml_str) {
        Ok(fm) => (fm.name, fm.description),
        Err(_) => {
            // YAML 解析失败，尝试简单解析
            let name = yaml_str
                .lines()
                .find(|l| l.starts_with("name:"))
                .map(|l| {
                    l.trim_start_matches("name:")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string()
                })
                .unwrap_or_else(|| "unknown_skill".to_string());

            let desc = yaml_str
                .lines()
                .find(|l| l.starts_with("description:"))
                .map(|l| {
                    l.trim_start_matches("description:")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string()
                })
                .unwrap_or_else(|| "No description".to_string());

            (name, desc)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_valid() {
        let content = r#"---
name: web_search
description: Search the web for information
---

# Web Search Skill

Use this skill to search the web."#;

        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, "web_search");
        assert_eq!(desc, "Search the web for information");
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "# My Awesome Skill\n\nDoes things.";
        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, "my_awesome_skill");
        assert_eq!(desc, "My Awesome Skill");
    }

    #[test]
    fn test_parse_frontmatter_quoted() {
        let content = r#"---
name: "code review"
description: 'Review code for bugs'
---

Do the review."#;

        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, "code review");
        assert_eq!(desc, "Review code for bugs");
    }
}
