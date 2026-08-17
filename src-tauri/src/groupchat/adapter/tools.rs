//! 群聊参与者的工具集（复用 AgentLoop 的 `ToolHandler`）。
//!
//! 提供文件/命令等核心工具，供 `PilotDeskLlmClient` 在群聊场景下复用
//! `run_agent_turn` 时注入。风险分级：
//! - Low（读）：read_file / list_files / glob / grep / web_search / web_fetch —— 自动放行
//! - Medium（写）：write_file / edit_file —— 触发 ApprovalHandler（Director 裁决）
//! - High（执行）：execute_command —— 触发 ApprovalHandler（Director 裁决）

use crate::api_agent::agent_loop::ToolRegistry;
use crate::tools::web_fetch::WebFetchTool;
use crate::tools::web_search::WebSearchTool;
use std::sync::Arc;

/// 构建群聊参与者工具集。`cwd` 为工作区目录（用于相对路径解析与越界保护）。
pub fn build_groupchat_tool_registry(conn: &rusqlite::Connection, cwd: &str) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();

    // ── read_file（Low，统一实现见 tools/read_file.rs）──
    registry.register(Arc::new(
        crate::tools::read_file::ReadFileTool::new(cwd.to_string()),
    ));

    // ── list_files（Low，统一实现见 tools/list_files.rs）──
    registry.register(Arc::new(
        crate::tools::list_files::ListFilesTool::new(cwd.to_string()),
    ));

    // ── glob（Low，统一实现见 tools/glob.rs）──
    registry.register(Arc::new(
        crate::tools::glob::GlobTool::new(cwd.to_string()),
    ));

    // ── grep（Low，统一实现见 tools/grep.rs）──
    registry.register(Arc::new(
        crate::tools::grep::GrepTool::new(cwd.to_string()),
    ));

    // ── write_file（Medium，统一实现见 tools/write_file.rs）──
    registry.register(Arc::new(
        crate::tools::write_file::WriteFileTool::new(cwd.to_string()),
    ));

    // ── edit_file（Medium，统一实现见 tools/edit_file.rs）──
    registry.register(Arc::new(
        crate::tools::edit_file::EditFileTool::new(cwd.to_string()),
    ));

    // ── execute_command（High，统一实现见 tools/execute_command.rs）──
    registry.register(Arc::new(
        crate::tools::execute_command::ExecuteCommandTool::new(cwd.to_string()),
    ));

    // ── execute_python（Low，统一实现见 tools/execute_python.rs）──
    registry.register(Arc::new(
        crate::tools::execute_python::ExecutePythonTool::new(cwd.to_string()),
    ));

    // ── web_search / web_fetch（Low）──
    let search_config = crate::commands::search::load_search_config(conn);
    registry.register(Arc::new(WebSearchTool::new(search_config)));
    registry.register(Arc::new(WebFetchTool::new()));

    Arc::new(registry)
}

