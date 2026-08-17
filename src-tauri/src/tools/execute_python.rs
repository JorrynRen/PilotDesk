//! Python 执行工具（会话模式与群聊参与者共用）。
//!
//! 合并自两处历史实现（工具架构统一 v1.0，轮 4）：两版逻辑相同，
//! 统一调用 `tools/exec.rs::run_python`（file/code 二选一、Python 检测缓存、30s 超时）。

use crate::tools::exec::run_python;
use crate::tools::{RiskLevel, ToolHandler, ToolTag};
use async_trait::async_trait;

/// Python 执行工具
pub struct ExecutePythonTool {
    cwd: String,
}

impl ExecutePythonTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl ToolHandler for ExecutePythonTool {
    fn name(&self) -> &str {
        "execute_python"
    }

    fn description(&self) -> &str {
        "在本地 Python 环境中执行 Python 代码或运行 .py 脚本文件。运行项目中的 Python 脚本请优先使用本工具：\
         传 file 参数指定脚本路径，或传 code 参数执行内联代码（多行、无需 shell 转义）。\
         首次调用会自动检测 Python 是否可用。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "要运行的 .py 脚本文件路径（绝对路径或相对工作目录路径）；与 code 二选一"
                },
                "code": {
                    "type": "string",
                    "description": "要执行的 Python 源代码（多行）；与 file 二选一"
                },
                "cwd": {
                    "type": "string",
                    "description": "工作目录，默认使用会话工作区"
                }
            }
        })
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn tags(&self) -> &[ToolTag] {
        &[ToolTag::Exec, ToolTag::Execute]
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String> {
        let cwd = arguments["cwd"].as_str().unwrap_or(&self.cwd);
        let code = arguments["code"].as_str();
        let file = arguments["file"].as_str();
        run_python(cwd, code, file)
    }
}
