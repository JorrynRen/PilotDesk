//! Agent 数据模型
//!
//! 从 commands/agents.rs 下沉的 Agent 配置相关数据结构定义。
//! 数据查询与操作仍保留在 commands/agents.rs 中。

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// Agent 类型标识（claude / hermes / codex 等）
    pub agent_type: String,
    /// 显示名称（如 "Claude Code"）
    pub display_name: String,
    /// 描述
    pub description: String,
    /// CLI 命令名
    pub cli_command: String,
    /// npm 包名（None 表示非 npm 安装）
    pub npm_package: Option<String>,
    /// pip 包名（None 表示非 pip 安装）
    pub pip_package: Option<String>,
    /// 安装命令
    pub install_cmd: String,
    /// 卸载命令
    pub uninstall_cmd: String,
    /// 更新命令
    pub update_cmd: String,
    /// 版本检测命令
    pub version_cmd: String,
    /// 最新版本查询命令
    pub latest_version_cmd: String,
    /// 启动命令模板，{message} 占位符替换
    pub run_cmd_template: String,
    /// 输出解析器类型
    pub output_parser: String,
    /// 噪声行过滤正则
    pub output_filter_regex: String,
    /// 版本号提取正则
    pub version_pattern: String,
    /// 是否支持会话延续
    pub supports_session_continuity: bool,
    /// session_id 来源
    pub session_id_source: String,
    /// JSON 事件类型
    pub session_id_event_type: String,
    /// JSON 字段名
    pub session_id_field: String,
    /// 恢复参数模板
    pub resume_arg_template: String,
    /// 技能目录路径（支持 {agent_type} 占位符），空字符串表示使用智能目录 ~/.{agent_type}/skills/
    pub skills_dir: String,
    /// 技能入口文件名（默认 SKILL.md）
    pub skill_entry_file: String,
    /// 技能显示模式：recursive（递归显示全部）或 collection（只显示集合名）
    pub skill_display_mode: String,
    /// UI 主题色
    pub color: String,
    /// UI 图标
    pub icon: String,
    /// 排序序号
    pub sort_order: i64,
    /// 是否启用
    pub is_enabled: bool,
    /// 是否为预置 Agent
    pub is_builtin: bool,
    /// 版本号（用于 Agent 市场更新检测）
    pub version: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentPayload {
    pub agent_type: String,
    pub display_name: String,
    pub description: Option<String>,
    pub cli_command: String,
    pub npm_package: Option<String>,
    pub pip_package: Option<String>,
    pub install_cmd: Option<String>,
    pub uninstall_cmd: Option<String>,
    pub update_cmd: Option<String>,
    pub version_cmd: Option<String>,
    pub latest_version_cmd: Option<String>,
    pub run_cmd_template: Option<String>,
    pub output_parser: Option<String>,
    pub output_filter_regex: Option<String>,
    pub version_pattern: Option<String>,
    pub supports_session_continuity: Option<bool>,
    pub session_id_source: Option<String>,
    pub session_id_event_type: Option<String>,
    pub session_id_field: Option<String>,
    pub resume_arg_template: Option<String>,
    pub skills_dir: Option<String>,
    pub skill_entry_file: Option<String>,
    pub skill_display_mode: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub sort_order: Option<i64>,
    pub is_enabled: Option<bool>,
    /// 版本号
    pub version: Option<String>,
}



#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentPayload {
    pub agent_type: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub cli_command: Option<String>,
    pub npm_package: Option<String>,
    pub pip_package: Option<String>,
    pub install_cmd: Option<String>,
    pub uninstall_cmd: Option<String>,
    pub update_cmd: Option<String>,
    pub version_cmd: Option<String>,
    pub latest_version_cmd: Option<String>,
    pub run_cmd_template: Option<String>,
    pub output_parser: Option<String>,
    pub output_filter_regex: Option<String>,
    pub version_pattern: Option<String>,
    pub supports_session_continuity: Option<bool>,
    pub session_id_source: Option<String>,
    pub session_id_event_type: Option<String>,
    pub session_id_field: Option<String>,
    pub resume_arg_template: Option<String>,
    pub skills_dir: Option<String>,
    pub skill_entry_file: Option<String>,
    pub skill_display_mode: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub sort_order: Option<i64>,
    pub is_enabled: Option<bool>,
    /// 版本号
    pub version: Option<String>,
}

// ──────────────────────────────────────────────
//  CRUD 命令
// ──────────────────────────────────────────────
