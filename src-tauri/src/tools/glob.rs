//! 按 glob 模式查找文件工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 2）：两版逻辑相同（描述微差），
//! 统一复用 `crate::resolve_workspace_path` / `crate::glob_to_regex` / `crate::collect_files`。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 按 glob 模式查找文件工具
pub struct GlobTool {
    cwd: String,
}

impl GlobTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl ToolHandler for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "按 glob 模式查找文件路径。pattern 支持 `*`（不跨目录）、`**`（递归跨目录）、`?`（单个字符）。path 为搜索根目录（默认工作区）。用于快速定位项目中的文件。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "glob 模式，如 **/*.rs、src/**/*.tsx、*.py"
                },
                "path": {
                    "type": "string",
                    "description": "搜索根目录（绝对路径或相对路径，默认工作区）"
                }
            },
            "required": ["pattern"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Filesystem, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        // R3（v3.5c）：同步文件系统扫描迁出 tokio worker，恢复外层 timeout 有效性（防大目录占死 worker）。
        let cwd = self.cwd.clone();
        let res = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let pattern = arguments["pattern"].as_str().ok_or("缺少 pattern 参数")?;
            let raw_path = arguments["path"].as_str().unwrap_or(".");
            let abs_path = crate::resolve_workspace_path(raw_path, &cwd);
            let re = crate::glob_to_regex(&pattern.replace('\\', "/"));
            let files = crate::collect_files(&abs_path, 5000);
            let mut matched = Vec::new();
            for f in files {
                if re.is_match(&f) {
                    matched.push(f);
                    if matched.len() >= 200 {
                        break;
                    }
                }
            }
            if matched.is_empty() {
                Ok(format!("未找到匹配 \"{}\" 的文件", pattern))
            } else {
                Ok(format!(
                    "找到 {} 个匹配文件：\n{}",
                    matched.len(),
                    matched.join("\n")
                ))
            }
        })
        .await
        .map_err(|e| format!("glob 执行线程异常: {}", e))?;
        res
    }
}
