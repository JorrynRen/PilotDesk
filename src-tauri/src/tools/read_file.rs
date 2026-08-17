//! 读取文件工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 1）：
//! - 会话版（原 `lib.rs`）：支持 `offset`/`limit` 分段读取、head+tail 智能截断（128KB）、
//!   系统关键文件防护（system32 config/drivers）
//! - 群聊版（原 `groupchat/adapter/tools.rs`）：`~/`/`%USERPROFILE%` 路径展开
//! 两版均含二进制检测（`crate::is_binary_bytes`），统一保留。
//! 行为合并规则：二进制检测 → 系统关键文件防护 → 路径展开 → 分段/截断读取。

use crate::tools::deps::expand_user_path;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 读取文件工具
pub struct ReadFileTool {
    cwd: String,
}

impl ReadFileTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl ToolHandler for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "读取指定路径的文件内容。path 可以是绝对路径或相对于工作目录的相对路径。\
用于查看代码文件、配置文件、日志等。大文件可通过 offset（起始字节偏移）和 \
limit（读取字节数）分段读取。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "文件路径（绝对路径或相对路径）"
                },
                "offset": {
                    "type": "integer",
                    "description": "起始字节偏移（从 0 开始，默认 0）"
                },
                "limit": {
                    "type": "integer",
                    "description": "本次最多读取的字节数（缺省时自动截断大文件）"
                }
            },
            "required": ["path"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Filesystem, ToolTag::Read]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let raw_path = arguments["path"].as_str().ok_or("缺少 path 参数")?;
        let offset = arguments["offset"].as_u64().unwrap_or(0) as usize;
        let limit = arguments["limit"].as_u64().map(|v| v as usize);

        // 安全检查：拒绝读取系统关键文件
        let path_lower = raw_path.to_lowercase();
        if path_lower.contains("\\windows\\system32\\config")
            || path_lower.contains("\\windows\\system32\\drivers")
        {
            return Err("安全限制：不允许读取系统关键文件".to_string());
        }

        // 路径预处理：展开 ~/ 与 %USERPROFILE%
        let expanded = expand_user_path(raw_path);

        let file_path = std::path::Path::new(&expanded);
        let abs_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            std::path::Path::new(&self.cwd).join(&expanded)
        };

        let bytes = std::fs::read(&abs_path)
            .map_err(|e| format!("读取文件失败 ({}): {}", abs_path.display(), e))?;

        // 二进制文件（图片/压缩包等）按文本读取只会产生乱码，明确拒绝
        if crate::is_binary_bytes(&bytes) {
            return Ok(format!(
                "该文件是二进制内容（总大小 {} KB），read_file 仅支持文本文件，无法按文本读取。",
                bytes.len() / 1024
            ));
        }

        let total = bytes.len();

        // 指定 offset/limit 时按字节范围分段读取
        if limit.is_some() || offset > 0 {
            let start = offset.min(total);
            let end = match limit {
                Some(l) => (offset + l).min(total),
                None => total,
            };
            let slice = if start < end { &bytes[start..end] } else { &[] };
            let text = String::from_utf8_lossy(slice).into_owned();
            return Ok(format!(
                "文件字节范围 [{}, {})，总大小 {} 字节:\n{}",
                start, end, total, text
            ));
        }

        let content = String::from_utf8_lossy(&bytes).into_owned();
        // ── 智能截断：head + tail 策略 ──
        const MAX_READ: usize = 131_072; // 128KB
        if content.len() > MAX_READ {
            // 安全地找到字符边界，避免在 UTF-8 多字节字符中间截断
            let head_size = MAX_READ / 2;
            let tail_size = MAX_READ - head_size;
            let head_end = content
                .char_indices()
                .take_while(|&(i, _)| i < head_size)
                .last()
                .map_or(0, |(i, c)| i + c.len_utf8());
            let tail_start = content
                .char_indices()
                .filter(|&(i, _)| i >= content.len() - tail_size)
                .next()
                .map_or(content.len(), |(i, _)| i);
            let head = &content[..head_end];
            let tail = &content[tail_start..];
            Ok(format!(
                "文件内容（前{}KB + 后{}KB，完整大小{}KB）:\n{}\n\n... (中间 {} 字节已省略，可用 offset/limit 分段读取) ...\n\n{}",
                head_end / 1024,
                (content.len() - tail_start) / 1024,
                content.len() / 1024,
                head,
                content.len() - head_end - (content.len() - tail_start),
                tail
            ))
        } else {
            Ok(content)
        }
    }
}
