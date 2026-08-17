//! 命令执行工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 4）：
//! - 会话版（原 `lib.rs`）：30s 超时轮询 + 输出解码 + 32KB head/tail 截断 + Python 引导描述
//! - 群聊版（原 `groupchat/adapter/tools.rs`）：精简执行（.output() 同步阻塞，无超时）
//! 统一采用会话版能力（`tools/exec.rs::run_command`），描述保留"Python 优先 execute_python"引导。

use crate::tools::exec::run_command;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// 命令执行工具
pub struct ExecuteCommandTool {
    cwd: String,
}

impl ExecuteCommandTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl ToolHandler for ExecuteCommandTool {
    fn name(&self) -> &str {
        "execute_command"
    }

    fn description(&self) -> &str {
        "在用户的工作区目录执行 Windows Shell 命令（cmd.exe）。用于获取系统信息、查看文件列表、git 操作、构建任务等。\
         运行 Python 代码或 .py 脚本时请优先使用 execute_python 工具；本工具保留兜底执行能力（如确有特殊需求仍可通过 python 命令执行）。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的命令（cmd.exe 格式，如 echo %USERPROFILE%\\Desktop / dir / git status）"
                },
                "cwd": {
                    "type": "string",
                    "description": "工作目录，默认使用会话工作区"
                }
            },
            "required": ["command"]
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::High
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Exec, ToolTag::Execute]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let command = arguments["command"].as_str().ok_or("缺少 command 参数")?;
        let cwd = arguments["cwd"].as_str().unwrap_or(&self.cwd);
        run_command(cwd, command)
    }
}
