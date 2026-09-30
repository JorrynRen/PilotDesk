//! 写入文件工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 3）：
//! - 会话版（原 `lib.rs`）：系统目录防护、危险命令检查（限定 bat/cmd/ps1 扩展名）、
//!   文件历史记录 + agent-file-diff 事件、权限友好提示
//! - 群聊版（原 `groupchat/adapter/tools.rs`）：精简写文件（无历史记录）
//!
//! 统一决策：
//! - 风险等级统一为 **Medium**（修改文件是有副作用操作，双端一致触发审批）
//! - 危险命令检查按扩展名限定（bat/cmd/ps1），避免普通代码/文档误伤（有审批兜底）
//! - 文件历史记录/diff 事件由 `FileHistoryService` 提供（经 ToolEnv 统一注入，会话/群聊共用）

use crate::tools::deps::expand_user_path;
use crate::tools::history::FileHistoryService;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;
use std::sync::Arc;

/// 写入文件工具
pub struct WriteFileTool {
    cwd: String,
    history: Option<Arc<FileHistoryService>>,
    /// 权限规则（deny_paths 系统目录 + deny 命令模式内容检查；经 ToolEnv 注入，会话/群聊共用）
    rules: Option<Arc<crate::api_agent::agent_loop::PermissionRules>>,
}

impl WriteFileTool {
    pub fn new(
        cwd: String,
        history: Option<Arc<FileHistoryService>>,
        rules: Option<Arc<crate::api_agent::agent_loop::PermissionRules>>,
    ) -> Self {
        Self {
            cwd,
            history,
            rules,
        }
    }
}

#[async_trait]
impl ToolHandler for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "将内容写入指定路径的文件。path 可以是绝对路径或相对于工作目录的相对路径。如果文件已存在则覆盖。用于生成代码文件、脚本、配置文件等。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "文件路径（绝对路径如 C:\\Users\\xxx\\Desktop\\tool.bat，或相对路径如 output\\script.bat）"
                },
                "content": {
                    "type": "string",
                    "description": "要写入文件的完整内容"
                }
            },
            "required": ["path", "content"]
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
        let content = arguments["content"].as_str().ok_or("缺少 content 参数")?;

        // 路径预处理：展开 ~ 和 %USERPROFILE%
        let expanded = expand_user_path(raw_path);

        let file_path = std::path::Path::new(&expanded);
        let abs_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            std::path::Path::new(&self.cwd).join(&expanded)
        };

        // ── 权限规则安全检查：风险路径拦截（路径子策略决定 deny 是否生效，无限制模式放行）──
        if let Some(rules) = &self.rules {
            if rules.decide_path(&abs_path.to_string_lossy(), false)
                == crate::api_agent::agent_loop::Action::Deny
            {
                return Err("安全限制：路径命中风险路径，拒绝写入。".to_string());
            }
        }

        // ── 脚本文件内容安全检查（限定可执行脚本扩展名，避免普通代码/文档误伤）──
        // deny 命令模式命中即拒绝（risky/allow 不适用于文件内容检查）
        let ext = abs_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ext == "bat" || ext == "cmd" || ext == "ps1" {
            if let Some(rules) = &self.rules {
                if let Some(pat) = rules.deny_hit(content) {
                    return Err(format!(
                        "安全限制：脚本文件中包含危险操作，拒绝写入。\n被拦截的关键词: {}\n请移除相关命令后重试。",
                        pat
                    ));
                }
            }
        }

        // 确保父目录存在
        if let Some(parent) = abs_path.parent() {
            if !parent.exists() {
                match std::fs::create_dir_all(parent) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        // 跨用户目录权限问题（如 LLM 猜测的用户名与实际不符）
                        // 尝试直接写入——如果目录结构恰好存在，写入会成功
                        log::warn!(
                            "[write_file] create_dir_all 权限不足（可能跨用户目录），尝试直接写入: {:?}",
                            parent
                        );
                    }
                    Err(e) => {
                        return Err(format!("创建目录失败 ({}): {}", parent.display(), e));
                    }
                }
            }
        }

        let existed_before = abs_path.exists();
        let old_content = if existed_before {
            std::fs::read_to_string(&abs_path).unwrap_or_default()
        } else {
            String::new()
        };

        match std::fs::write(&abs_path, content) {
            Ok(()) => {
                // 文件历史 + diff 事件（经 FileHistoryService，会话/群聊共用）
                if let Some(history) = &self.history {
                    history.record(
                        &abs_path.to_string_lossy(),
                        &old_content,
                        content,
                        existed_before,
                    );
                    history.emit_diff(&abs_path.to_string_lossy(), &old_content, content);
                }
                Ok(format!("文件已成功写入: {}", abs_path.display()))
            }
            Err(e) => {
                let hint = match e.kind() {
                    std::io::ErrorKind::PermissionDenied => {
                        if let Some(home) = dirs::home_dir() {
                            let home_str = home.to_string_lossy().to_lowercase();
                            let abs_str = abs_path.to_string_lossy().to_lowercase();
                            if abs_str.starts_with("c:\\users\\") && !abs_str.starts_with(&home_str)
                            {
                                "（用户名不匹配——请使用当前用户路径而非猜测的用户名）"
                            } else {
                                "（提示：可能被安全软件拦截，或目标目录需要管理员权限）"
                            }
                        } else {
                            "（提示：可能被安全软件拦截，或目标目录需要管理员权限）"
                        }
                    }
                    std::io::ErrorKind::NotFound => "（提示：路径不存在，请检查目录是否正确）",
                    std::io::ErrorKind::InvalidInput => "（提示：路径包含非法字符）",
                    _ => "",
                };
                Err(format!("写入文件失败: {} {}", e, hint))
            }
        }
    }
}
