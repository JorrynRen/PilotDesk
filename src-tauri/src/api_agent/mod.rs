//! API Agent 模块
//!
//! 为 API 直连模式提供完整的 Agent 能力：
//! - HTTP + SSE 流式客户端
//! - Agent Loop 任务编排（tool-calling 循环）
//! - 上下文管理与 System Prompt 组装
//!
//! CLI Agent（Claude/Hermes/CodeX）不受此模块影响，其上下文/记忆/技能/
//! 编排均由各自子进程内部管理。

pub mod agent_loop;
pub mod client;
pub mod context;
pub mod skills;
pub mod system_prompt;
pub mod types;
