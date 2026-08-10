//! System Prompt 组装
//!
//! 按 Progressive Disclosure 原则组装 API Agent 的 system prompt：
//!   1. Base Agent 指令
//!   2. MEMORY.md（项目级记忆，来自工作区根目录）
//!   3. USER.md（用户级偏好，来自 PilotDesk 配置目录）
//!   4. Skill 列表（name + description，不注入完整内容）
//!
//! 技能完整内容通过 load_skill 工具按需加载（B4 实现）。

/// System Prompt 构建器
pub struct SystemPromptBuilder {
    base_prompt: String,
    memory_md: Option<String>,
    user_md: Option<String>,
    kv_memories: Option<String>,
    skills: Vec<SkillEntry>,
}

/// 技能条目（仅 name + description，Progressive Disclosure）
pub struct SkillEntry {
    pub name: String,
    pub description: String,
}

impl SystemPromptBuilder {
    pub fn new(base_prompt: String) -> Self {
        Self {
            base_prompt,
            memory_md: None,
            user_md: None,
            kv_memories: None,
            skills: Vec::new(),
        }
    }

    /// 从文件加载 MEMORY.md
    pub fn with_memory_md(mut self, workspace_root: Option<&str>) -> Self {
        if let Some(root) = workspace_root {
            self.memory_md = read_file_if_exists(root, "MEMORY.md");
        }
        self
    }

    /// 从 PilotDesk 配置目录加载 USER.md
    pub fn with_user_md(mut self) -> Self {
        if let Some(config_dir) = get_pilotdesk_config_dir() {
            self.user_md = read_file_if_exists(&config_dir, "USER.md");
        }
        self
    }

    /// 设置 KV 记忆块（从 MemoryStore.format_for_prompt 获取）
    pub fn with_kv_memories(mut self, kv_block: Option<String>) -> Self {
        self.kv_memories = kv_block;
        self
    }

    /// 设置技能列表（Progressive Disclosure: 仅注入 name + description）
    pub fn with_skills(mut self, skills: Vec<SkillEntry>) -> Self {
        self.skills = skills;
        self
    }

    /// 组装最终 system prompt
    pub fn build(self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // 1. Base prompt
        if !self.base_prompt.is_empty() {
            parts.push(self.base_prompt);
        }

        // 2. MEMORY.md — 项目级记忆
        if let Some(ref memory) = self.memory_md {
            parts.push(format!(
                "<project_memory>\n{}\n</project_memory>",
                memory
            ));
        }

        // 3. USER.md — 用户偏好
        if let Some(ref user) = self.user_md {
            parts.push(format!(
                "<user_preferences>\n{}\n</user_preferences>",
                user
            ));
        }

        // 3.5. KV 记忆 — 跨会话持久化知识
        if let Some(ref kv) = self.kv_memories {
            parts.push(kv.clone());
        }

        // 4. Skill 列表 — Progressive Disclosure
        if !self.skills.is_empty() {
            let mut skill_block = String::from("<available_skills>\n");
            skill_block.push_str("以下是可用的技能列表。调用 load_skill 工具加载完整技能内容：\n\n");
            for skill in &self.skills {
                skill_block.push_str(&format!(
                    "- **{}**: {}\n",
                    skill.name,
                    skill.description
                ));
            }
            skill_block.push_str("</available_skills>");
            parts.push(skill_block);
        }

        parts.join("\n\n")
    }
}

/// 读取文件内容（如果存在）
fn read_file_if_exists(dir: &str, filename: &str) -> Option<String> {
    let path = std::path::Path::new(dir).join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => Some(content.trim().to_string()),
        _ => None,
    }
}

/// 获取 PilotDesk 用户配置目录
/// Windows: %APPDATA%/PilotDesk
/// Linux/macOS: ~/.config/pilotdesk
pub fn get_pilotdesk_config_dir() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(|d| format!("{}\\PilotDesk", d))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME")
            .or_else(|_| std::env::var("XDG_CONFIG_HOME"))
            .ok()
            .map(|d| format!("{}/.config/pilotdesk", d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_build() {
        let prompt = SystemPromptBuilder::new("You are a helpful assistant.".into())
            .build();
        assert_eq!(prompt, "You are a helpful assistant.");
    }

    #[test]
    fn test_with_skills() {
        let prompt = SystemPromptBuilder::new("Base prompt".into())
            .with_skills(vec![
                SkillEntry {
                    name: "web_search".into(),
                    description: "Search the web".into(),
                },
                SkillEntry {
                    name: "read_file".into(),
                    description: "Read a file".into(),
                },
            ])
            .build();
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("web_search"));
        assert!(prompt.contains("read_file"));
        assert!(prompt.contains("</available_skills>"));
    }

    #[test]
    fn test_pilotdesk_config_dir_exists() {
        // 仅验证函数不 panic
        let _ = get_pilotdesk_config_dir();
    }
}
