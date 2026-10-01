//! System Prompt 组装
//!
//! 按 Progressive Disclosure 原则组装 API Agent 的 system prompt：
//!   1. Base Agent 指令
//!   2. MEMORY.md（项目级记忆，来自工作区根目录）
//!   3. USER.md（用户级偏好，来自 PilotDesk 配置目录）
//!   4. Skill 列表（name + description，不注入完整内容）
//!
//! 技能完整内容通过 load_skill 工具按需加载（B4 实现）。

use crate::utils::process::hidden_command;

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
        let branch = hidden_command("git")
            .args(["branch", "--show-current"])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let latest_commit = hidden_command("git")
            .args(["log", "-1", "--format=%h"])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let has_uncommitted = hidden_command("git")
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
    skills: Vec<SkillEntry>,
    git_context: Option<String>,
    knowledge_bases: Vec<KnowledgeBaseBrief>,
}

/// 技能条目（仅 name + description，Progressive Disclosure）
pub struct SkillEntry {
    pub name: String,
    pub description: String,
}

/// 知识库摘要条目（只取注入需要的字段，避免把整个 view 结构拖进来）
pub struct KnowledgeBaseBrief {
    pub name: String,
    pub description: String,
    pub entry_count: usize,
    pub file_count: usize,
    /// 专属字段定义（JSON 数组串）。**必须给模型看**：`save_knowledge` 的 `meta` 要按它填，
    /// 不给的话模型写进去的条目就只有标题与正文（标签/专属属性全靠猜）。
    pub fields_json: String,
}

/// 专属字段定义 → 一行紧凑文本（`状态（key=status；单选：现行/废止）、生效日期（key=effect_date；日期）`）。
///
/// **展示名与 key 都要给**：`key` 是 meta 里真正要写的键（模型必须用它），
/// 但它常是 `status` / `fawenhao` 这类无语义的短标识，只给 key 模型猜不出这个字段要填什么；
/// `label`（「状态」「发文号」）才是语义所在。两者一起给，模型才既填得对、也填得准。
///
/// 与 `kb_llm::sanitize_meta` 同一份定义、同一套取值口径（select 只能取候选之一），
/// 这里只负责**渲染成模型读得懂的说明**。
fn fields_line(fields_json: &str) -> String {
    let Ok(serde_json::Value::Array(fields)) =
        serde_json::from_str::<serde_json::Value>(fields_json)
    else {
        return String::new();
    };
    let mut out: Vec<String> = Vec::new();
    for f in fields {
        let key = f["key"].as_str().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        // 展示名优先，缺了就回落成 key（宁可重复，也不能给模型一个没有语义的标签）
        let label = f["label"]
            .as_str()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or(key);
        let ty = match f["type"].as_str().unwrap_or("text") {
            "select" => {
                let opts: Vec<&str> = f["options"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|o| o.as_str()).collect())
                    .unwrap_or_default();
                if opts.is_empty() {
                    "单选".to_string()
                } else {
                    format!("单选：{}", opts.join("/"))
                }
            }
            "number" => "数字".to_string(),
            "bool" => "是否".to_string(),
            "date" => "日期".to_string(),
            _ => "文本".to_string(),
        };
        out.push(format!("{}（key={}；{}）", label, key, ty));
    }
    out.join("、")
}

impl SystemPromptBuilder {
    pub fn new(base_prompt: String) -> Self {
        Self {
            base_prompt,
            memory_md: None,
            user_md: None,
            skills: Vec::new(),
            git_context: None,
            knowledge_bases: Vec::new(),
        }
    }

    /// 从文件加载 MEMORY.md
    pub fn with_memory_md(mut self, workspace_root: Option<&str>) -> Self {
        if let Some(root) = workspace_root {
            self.memory_md = read_file_if_exists(root, "MEMORY.md");
        }
        self
    }

    /// 从 PilotDesk 统一配置根目录加载 USER.md（用户级偏好）。
    /// 读取根与 MEMORY.db / 技能等共用同一配置目录（见 get_pilotdesk_config_dir），
    /// 文件不存在即视为无用户偏好，不做额外目录兜底。
    pub fn with_user_md(mut self) -> Self {
        if let Some(config_dir) = get_pilotdesk_config_dir() {
            self.user_md = read_file_if_exists(&config_dir, "USER.md");
        }
        self
    }

    /// 设置技能列表（Progressive Disclosure: 仅注入 name + description）
    pub fn with_skills(mut self, skills: Vec<SkillEntry>) -> Self {
        self.skills = skills;
        self
    }

    /// 注入知识库目录摘要（库名 + 简介 + 条目数/文件数）。
    ///
    /// 为什么不是"只留工具、让模型自己去查"：模型**不知道自己有什么**，就不会去查 ——
    /// 工具描述能说"有这么个工具"，说不了"你的库里有哪些材料"。每库一行、只在非空时注入，
    /// 换来的是模型能主动检索，而不是等用户提醒"我投喂过资料"。
    pub fn with_knowledge_bases(mut self, bases: Vec<KnowledgeBaseBrief>) -> Self {
        self.knowledge_bases = bases;
        self
    }

    /// 注入 Git 仓库上下文
    pub fn with_git_context(mut self, git: &GitContext) -> Self {
        self.git_context = Some(git.format_prompt());
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
            parts.push(format!("<project_memory>\n{}\n</project_memory>", memory));
        }

        // 2.5. Git 仓库上下文
        if let Some(ref git) = self.git_context {
            parts.push(git.clone());
        }

        // 3. USER.md — 用户偏好
        if let Some(ref user) = self.user_md {
            parts.push(format!("<user_preferences>\n{}\n</user_preferences>", user));
        }

        // 4. Skill 列表 — Progressive Disclosure
        if !self.skills.is_empty() {
            let mut skill_block = String::from("<available_skills>\n");
            skill_block
                .push_str("以下是可用的技能列表。调用 load_skill 工具加载完整技能内容：\n\n");
            for skill in &self.skills {
                skill_block.push_str(&format!("- **{}**: {}\n", skill.name, skill.description));
            }
            skill_block.push_str("</available_skills>");
            parts.push(skill_block);
        }

        // 5. 知识库目录摘要 —— 让模型"知道自己有什么资料"
        if !self.knowledge_bases.is_empty() {
            let mut kb_block = String::from("<knowledge_bases>\n");
            kb_block.push_str(
                "以下是用户知识库里的资料概况。需要查资料时用 search_knowledge 检索（正文全文可查）；\
                 要写入时用 save_knowledge，并给它 **tags**（检索标签），以及按各库「专属属性」定义填 **meta** ——\
                 meta 的**键**用属性里 `key=` 后面的英文标识（不要写展示名），**值**按类型/候选项给：\n",
            );
            for b in &self.knowledge_bases {
                let desc = if b.description.trim().is_empty() {
                    String::new()
                } else {
                    format!("（{}）", b.description.trim())
                };
                kb_block.push_str(&format!(
                    "- {}{}：{} 条知识、{} 个文件\n",
                    b.name, desc, b.entry_count, b.file_count
                ));
                let fields = fields_line(&b.fields_json);
                if !fields.is_empty() {
                    kb_block.push_str(&format!("  专属属性：{}\n", fields));
                }
            }
            kb_block.push_str("</knowledge_bases>");
            parts.push(kb_block);
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

/// 获取 PilotDesk 用户配置目录 —— 即**应用根目录**（主库 / MEMORY.db / 技能 / 插件同根）。
/// Windows: `%APPDATA%\PilotDesk`；Linux/macOS: `$XDG_CONFIG_HOME/pilotdesk` 或 `~/.config/pilotdesk`。
///
/// 这里直接委托 paths::app_root_dir()，不再自己拼路径：曾经两边各拼一份，导致
/// Linux 上主库在 `~/.local/share/PilotDesk`、记忆库在 `~/.config/pilotdesk`，数据被劈成两半。
pub fn get_pilotdesk_config_dir() -> Option<String> {
    Some(
        crate::utils::paths::app_root_dir()
            .to_string_lossy()
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_build() {
        let prompt = SystemPromptBuilder::new("You are a helpful assistant.".into()).build();
        assert_eq!(prompt, "You are a helpful assistant.");
    }

    /// 知识库摘要：只在非空时注入，且要说清"怎么查" —— 模型知道自己有什么才会去查
    #[test]
    fn knowledge_bases_block_lists_briefs_and_the_way_to_search() {
        let prompt = SystemPromptBuilder::new("Base".into())
            .with_knowledge_bases(vec![KnowledgeBaseBrief {
                name: "小说创作".into(),
                description: "创作相关资料".into(),
                entry_count: 27,
                file_count: 4,
                fields_json: r#"[{"key":"status","label":"状态","type":"select","options":["现行","废止"]},{"key":"effect_date","label":"生效日期","type":"date"}]"#.into(),
            }])
            .build();
        assert!(prompt.contains("<knowledge_bases>"));
        assert!(
            prompt.contains("- 小说创作（创作相关资料）：27 条知识、4 个文件"),
            "{}",
            prompt
        );
        assert!(prompt.contains("search_knowledge"), "{}", prompt);
        // 专属属性定义必须注入，且**展示名与 key 都要有**：key 是 meta 的键（模型要写对），
        // 展示名是语义（只给 `status` 这种键，模型不知道该字段要填什么）
        assert!(
            prompt.contains(
                "专属属性：状态（key=status；单选：现行/废止）、生效日期（key=effect_date；日期）"
            ),
            "{}",
            prompt
        );
        assert!(prompt.contains("save_knowledge"), "{}", prompt);
        assert!(prompt.contains("</knowledge_bases>"));

        // 没有知识库时不留空块
        let plain = SystemPromptBuilder::new("Base".into()).build();
        assert!(!plain.contains("<knowledge_bases>"));
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
