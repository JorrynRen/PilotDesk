//! 文件内容正则搜索工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 2）：两版逻辑相同，
//! 统一复用 `crate::resolve_workspace_path` / `crate::glob_to_regex` / `crate::collect_files`。

use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 文件内容搜索工具
pub struct GrepTool {
    cwd: String,
}

impl GrepTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl ToolHandler for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "在指定目录的文件内容中按正则表达式搜索。path 为搜索根目录（默认工作区），include 可选按 glob 过滤文件类型。用于快速定位代码符号、字符串或模式。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "要搜索的正则表达式，如 fn\\s+main、TODO、import.*from"
                },
                "path": {
                    "type": "string",
                    "description": "搜索根目录（绝对路径或相对路径，默认工作区）"
                },
                "include": {
                    "type": "string",
                    "description": "可选：按 glob 过滤文件名，如 *.rs、**/*.ts"
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "是否区分大小写，默认 false"
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
        let pattern = arguments["pattern"].as_str().ok_or("缺少 pattern 参数")?;
        let raw_path = arguments["path"].as_str().unwrap_or(".");
        let include = arguments["include"].as_str().map(|s| s.replace('\\', "/"));
        let case_sensitive = arguments["case_sensitive"].as_bool().unwrap_or(false);

        let re = regex::RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|e| format!("正则表达式无效: {}", e))?;

        let abs_path = crate::resolve_workspace_path(raw_path, &self.cwd);
        let inc_re = include.as_ref().map(|s| crate::glob_to_regex(s));

        let files = crate::collect_files(&abs_path, 5000);
        let mut out = String::new();
        let mut match_count = 0usize;
        const MAX_MATCHES: usize = 200;

        'outer: for rel in files {
            if let Some(re2) = &inc_re {
                if !re2.is_match(&rel) {
                    continue;
                }
            }
            let full = abs_path.join(&rel);
            let content = match std::fs::read(&full) {
                Ok(bytes) => {
                    if bytes.contains(&0) {
                        continue;
                    }
                    String::from_utf8_lossy(&bytes).into_owned()
                }
                Err(_) => continue,
            };
            for (ln, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    out.push_str(&format!("{}:{}: {}\n", rel, ln + 1, line.trim()));
                    match_count += 1;
                    if match_count >= MAX_MATCHES {
                        break 'outer;
                    }
                }
            }
        }

        if out.is_empty() {
            Ok(format!("未找到匹配 \"{}\" 的内容", pattern))
        } else {
            Ok(format!("找到 {} 处匹配：\n{}", match_count, out))
        }
    }
}
