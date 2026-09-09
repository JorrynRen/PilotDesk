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
        // R3（v3.5c）：文件系统同步 IO 迁出 tokio worker（spawn_blocking）。同步 std::fs 在 async
        // 内会令 AgentLoop 外层 tokio timeout 失效（同步阻塞不推进时钟）；迁出后超时可正常取消等待，
        // 大目录/大文件扫描不再占死 worker 线程。
        let cwd = self.cwd.clone();
        let res = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let pattern = arguments["pattern"].as_str().ok_or("缺少 pattern 参数")?;
            let raw_path = arguments["path"].as_str().unwrap_or(".");
            let include = arguments["include"].as_str().map(|s| s.replace('\\', "/"));
            let case_sensitive = arguments["case_sensitive"].as_bool().unwrap_or(false);

            let re = regex::RegexBuilder::new(pattern)
                .case_insensitive(!case_sensitive)
                .build()
                .map_err(|e| format!("正则表达式无效: {}", e))?;

            let abs_path = crate::resolve_workspace_path(raw_path, &cwd);
            if !abs_path.exists() {
                return Err(format!("搜索目录不存在: {}", abs_path.display()));
            }
            let inc_re = include.as_ref().map(|s| crate::glob_to_regex(s));

            let files = crate::collect_files(&abs_path, 5000);
            let mut out = String::new();
            let mut match_count = 0usize;
            let mut skipped_big = 0usize;
            const MAX_MATCHES: usize = 200;
            // 单文件内容搜索上限：超过则跳过（防巨型文件全量读入内存/逐行匹配卡慢），并计数提示。
            const MAX_GREP_FILE_BYTES: u64 = 4 * 1024 * 1024;

            'outer: for rel in files {
                if let Some(re2) = &inc_re {
                    if !re2.is_match(&rel) {
                        continue;
                    }
                }
                let full = abs_path.join(&rel);
                let Ok(meta) = std::fs::metadata(&full) else {
                    continue;
                };
                if meta.len() > MAX_GREP_FILE_BYTES {
                    skipped_big += 1;
                    continue;
                }
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

            let big_note = if skipped_big > 0 {
                format!("\n（另有 {} 个超大文件被跳过，如需在其中搜索请改用 execute_python 定向处理）", skipped_big)
            } else {
                String::new()
            };
            if out.is_empty() {
                Ok(format!("未找到匹配 \"{}\" 的内容{}", pattern, big_note))
            } else {
                Ok(format!("找到 {} 处匹配：\n{}{}", match_count, out, big_note))
            }
        })
        .await
        .map_err(|e| format!("grep 执行线程异常: {}", e))?;
        res
    }
}
