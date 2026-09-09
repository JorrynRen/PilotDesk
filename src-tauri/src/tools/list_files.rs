//! 列出目录内容工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 2）：
//! - 会话版（原 `lib.rs`）：系统目录防护（c:\windows）、`~/`/`%USERPROFILE%` 展开
//! - 群聊版（原 `groupchat/adapter/tools.rs`）：同逻辑（无系统目录防护）
//! 统一保留：系统目录防护 + 路径展开 + 200 条目上限。

use crate::tools::deps::expand_user_path;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 列出目录内容工具
pub struct ListFilesTool {
    cwd: String,
}

impl ListFilesTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl ToolHandler for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "列出指定目录下的文件和子目录（包含名称和类型）。path 可以是绝对路径或相对于工作目录的相对路径。用于浏览项目结构、查看目录内容等。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "目录路径（绝对路径或相对路径，留空表示工作区目录）"
                }
            },
            "required": []
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Filesystem, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        // R3（v3.5c）：同步目录枚举迁出 tokio worker，恢复外层 timeout 有效性（防大目录占死 worker）。
        let cwd = self.cwd.clone();
        let res = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let raw_path = arguments["path"].as_str().unwrap_or(".");

            // 安全检查：拒绝列出系统关键目录
            let path_lower = raw_path.to_lowercase();
            if path_lower.starts_with("c:\\windows")
                || path_lower.starts_with("c:\\windows\\system32")
            {
                return Err("安全限制：不允许列出系统目录".to_string());
            }

            // 路径预处理：展开 ~/ 与 %USERPROFILE%
            let expanded = expand_user_path(raw_path);

            let dir_path = std::path::Path::new(&expanded);
            let abs_path = if dir_path.is_absolute() {
                dir_path.to_path_buf()
            } else {
                std::path::Path::new(&cwd).join(&expanded)
            };

            let entries = std::fs::read_dir(&abs_path)
                .map_err(|e| format!("列出目录失败 ({}): {}", abs_path.display(), e))?;
            let mut result = String::new();
            let mut count = 0usize;
            const MAX_ENTRIES: usize = 200;
            for entry in entries.flatten() {
                if count >= MAX_ENTRIES {
                    result.push_str(&format!("\n... 还有更多条目（仅显示前{}条）\n", MAX_ENTRIES));
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let kind = match entry.file_type() {
                    Ok(ft) if ft.is_dir() => "<DIR>",
                    Ok(_) => "<FILE>",
                    Err(_) => "<?>",
                };
                result.push_str(&format!("{} {}\n", name, kind));
                count += 1;
            }
            if count == 0 {
                result.push_str("(目录为空)\n");
            }
            Ok(result)
        })
        .await
        .map_err(|e| format!("list_files 执行线程异常: {}", e))?;
        res
    }
}
