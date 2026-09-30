//! 命令执行工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 4），v3.5c 起统一走
//! `tools/exec.rs::run_command`（异步管道实时读 + 活性检测判真实卡死 + 进程树终止，无秒级盲杀）。

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
         注意 cmd 语法：路径用 `\\` 分隔，**创建目录直接用 `mkdir 路径`**（cmd 的 mkdir 没有 `-p`，写 `mkdir -p` 会在当前目录多建一个名为 `-p` 的目录），\
         删除用 `del`/`rmdir`（没有 `rm -rf`），不要使用 `ls -la`、`cp -r`、`touch` 等 Unix 命令；\
         写文件用 write_file（会自动创建缺失的上级目录），一般无需本工具建目录。\
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
        run_command(cwd, command).await
    }
}
