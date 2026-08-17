//! System Prompt 组装
//!
//! 按 Progressive Disclosure 原则组装 API Agent 的 system prompt：
//!   1. Base Agent 指令
//!   2. MEMORY.md（项目级记忆，来自工作区根目录）
//!   3. USER.md（用户级偏好，来自 PilotDesk 配置目录）
//!   4. Skill 列表（name + description，不注入完整内容）
//!
//! 技能完整内容通过 load_skill 工具按需加载（B4 实现）。

pub struct GitContext {
    pub branch: Option<String>,
    pub latest_commit: Option<String>,
    pub has_uncommitted: bool,
}

impl GitContext {
    pub fn format_prompt(&self) -> String {
        let mut lines = Vec::new();
        lines.push("## Git 仓库上下文".to_string());
        if let Some(ref branch) = self.branch {
            lines.push(format!("- 当前分支: {}", branch));
        }
        if let Some(ref commit) = self.latest_commit {
            lines.push(format!("- 最新提交: {}", commit));
        }
        if self.has_uncommitted {
            lines.push("- 注意: 工作区有未提交的变更".to_string());
        }
        lines.join("\n")
    }

    /// 从 cwd 执行 git 命令获取仓库上下文（失败时返回 None）
    pub fn from_cwd(cwd: &str) -> Option<Self> {
        let branch = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let latest_commit = std::process::Command::new("git")
            .args(["log", "-1", "--format=%h"])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let has_uncommitted = std::process::Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false);

        if branch.is_none() && latest_commit.is_none() {
            return None;
        }

        Some(Self {
            branch,
            latest_commit,
            has_uncommitted,
        })
    }
}

/// System Prompt 构建器
pub struct SystemPromptBuilder {
    base_prompt: String,
    memory_md: Option<String>,
    user_md: Option<String>,
    kv_memories: Option<String>,
    skills: Vec<SkillEntry>,
    git_context: Option<String>,
    project_context: Option<String>,
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
            git_context: None,
            project_context: None,
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

    /// 注入 Git 仓库上下文
    pub fn with_git_context(mut self, git: &GitContext) -> Self {
        self.git_context = Some(git.format_prompt());
        self
    }

    /// 注入项目级上下文文件（CLAUDE.md、README.md 等）
    pub fn with_project_context(mut self, cwd: &str) -> Self {
        self.project_context = ProjectContext::from_cwd(cwd).format_prompt();
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

        // 2.5. Git 仓库上下文
        if let Some(ref git) = self.git_context {
            parts.push(git.clone());
        }

        // 2.6. 项目上下文文件
        if let Some(ref ctx) = self.project_context {
            parts.push(ctx.clone());
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

/// 项目上下文文件扫描
pub struct ProjectContext {
    pub claude_md: Option<String>,
    pub readme: Option<String>,
}

impl ProjectContext {
    /// 扫描工作区递归查找配置文档（最多向上查找 2 层）
    pub fn from_cwd(cwd: &str) -> Self {
        let mut ctx = Self {
            claude_md: None,
            readme: None,
        };

        // 读取根目录 README.md
        ctx.readme = read_file_if_exists(cwd, "README.md");

        // 递归查找 CLAUDE.md（向上最多 2 层）
        let mut dir = std::path::PathBuf::from(cwd);
        for _ in 0..=2 {
            if let Some(parent) = dir.parent() {
                dir = parent.to_path_buf();
            } else {
                break;
            }
            if ctx.claude_md.is_none() {
                ctx.claude_md = read_file_if_exists(dir.to_string_lossy().as_ref(), "CLAUDE.md");
            }
        }

        ctx
    }

    /// 格式化为 system prompt 块
    pub fn format_prompt(&self) -> Option<String> {
        let mut lines = Vec::new();
        if let Some(ref claude) = self.claude_md {
            lines.push("## 项目配置文档 (CLAUDE.md)".to_string());
            lines.push(claude.clone());
        }
        if let Some(ref readme) = self.readme {
            lines.push("## 项目说明文档 (README.md)".to_string());
            lines.push(readme.clone());
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n\n"))
        }
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
