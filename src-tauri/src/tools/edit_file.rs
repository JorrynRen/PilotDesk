//! 精确字符串替换工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 3）：
//! - 会话版（原 `lib.rs`）：唯一性校验、文件历史记录 + agent-file-diff 事件
//! - 群聊版（原 `groupchat/adapter/tools.rs`）：精简替换（无历史记录）
//!
//! 统一决策：风险等级统一为 **Medium**（与 write_file 一致，修改文件即触发审批）。
//! 文件历史记录/diff 事件由 `FileHistoryService` 提供（经 ToolEnv 统一注入，会话/群聊共用）。

use crate::tools::history::FileHistoryService;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::Arc;

/// 精确字符串替换工具
pub struct EditFileTool {
    cwd: String,
    history: Option<Arc<FileHistoryService>>,
}

impl EditFileTool {
    pub fn new(cwd: String, history: Option<Arc<FileHistoryService>>) -> Self {
        Self { cwd, history }
    }
}

#[async_trait]
impl ToolHandler for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "对已有文件做精确的字符串替换。old_string 必须与文件内容完全一致且唯一出现，new_string 为替换后的内容。用于局部修改，避免整文件重写。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "文件路径（绝对路径或相对路径）"
                },
                "old_string": {
                    "type": "string",
                    "description": "要替换的原文，需与文件内容精确匹配且唯一"
                },
                "new_string": {
                    "type": "string",
                    "description": "替换后的新内容"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Medium
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Filesystem, ToolTag::Write]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let raw_path = arguments["path"].as_str().ok_or("缺少 path 参数")?;
        let old_string = arguments["old_string"].as_str().ok_or("缺少 old_string 参数")?;
        let new_string = arguments["new_string"].as_str().ok_or("缺少 new_string 参数")?;

        if old_string.is_empty() {
            return Err("old_string 不能为空".to_string());
        }

        let abs_path = crate::resolve_workspace_path(raw_path, &self.cwd);
        let content = std::fs::read_to_string(&abs_path)
            .map_err(|e| format!("读取文件失败 ({}): {}", abs_path.display(), e))?;

        let count = content.matches(old_string).count();
        if count == 0 {
            return Err(format!(
                "未在文件中找到 old_string，请确认内容与文件完全一致（含空白与换行）。\n文件: {}",
                abs_path.display()
            ));
        }
        if count > 1 {
            return Err(format!(
                "old_string 在文件中出现了 {} 次，不唯一。请提供更长的上下文使其唯一。",
                count
            ));
        }

        let new_content = content.replacen(old_string, new_string, 1);
        std::fs::write(&abs_path, &new_content)
            .map_err(|e| format!("写入文件失败 ({}): {}", abs_path.display(), e))?;

        // 文件历史 + diff 事件（经 FileHistoryService，会话/群聊共用）
        if let Some(history) = &self.history {
            history.record(&abs_path.to_string_lossy(), &content, &new_content, true);
            history.emit_diff(&abs_path.to_string_lossy(), &content, &new_content);
        }

        Ok(format!("已替换文件中的 1 处内容: {}", abs_path.display()))
    }
}
