//! 技能加载工具（会话模式专属）。
//!
//! 迁移自 `lib.rs`（工具架构统一 v1.0，轮 6）：从 `builtin_tool!` 宏改写为具名 struct。

use crate::api_agent::skills::SkillLoader;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 加载指定技能完整内容
pub struct LoadSkillTool {
    loader: SkillLoader,
}

impl LoadSkillTool {
    pub fn new(loader: SkillLoader) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl ToolHandler for LoadSkillTool {
    fn name(&self) -> &str {
        "load_skill"
    }

    fn description(&self) -> &str {
        "加载指定技能的完整内容。当需要详细了解某个技能的使用方法时调用此工具。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "技能名称（来自 available_skills 列表）"
                }
            },
            "required": ["name"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Agent, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let name = arguments["name"].as_str().ok_or("缺少 name 参数")?;
        let content = self
            .loader
            .load_skill(name)
            .ok_or_else(|| format!("技能不存在: {}", name))?;
        // 注入技能根目录：模型读取技能内容时即可获知脚本所在绝对路径，
        // 技能文档内无需硬编码安装路径（通用机制，所有技能受益）。
        let prefix = self
            .loader
            .skill_dir(name)
            .map(|dir| format!("> 技能根目录：{}\n\n", dir.display()))
            .unwrap_or_default();
        Ok(format!("{}{}", prefix, content))
    }
}
