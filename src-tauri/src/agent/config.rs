//! Agent 数据模型
//!
//! 从 commands/agents.rs 下沉的 Agent 配置相关数据结构定义。
//! 数据查询与操作仍保留在 commands/agents.rs 中。
//!
//! v11 起生命周期/安装状态/来源从扁平列收敛为 JSON（见 `agent::lifecycle`）：
//! - `lifecycle`：装什么、怎么跑；
//! - `pkg_state`：本机安装到哪一步（pending_install / ready / failed）；
//! - `dist_source`：配置从哪来（官方 ACP 目录 / 内置预置 / 手工新建）。

use crate::agent::lifecycle::{DistSource, LifecycleSpec, PkgState};
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
    /// 生命周期规格（安装/更新/卸载/版本探测 + 运行时启动）
    pub lifecycle: Option<LifecycleSpec>,
    /// 本地包管理状态
    pub pkg_state: Option<PkgState>,
    /// 配置来源
    pub dist_source: Option<DistSource>,
    /// 启动命令模板，{message} 占位符替换（CLI 兼容模式）
    pub run_cmd_template: String,
    /// 输出解析器类型
    pub output_parser: String,
    /// 噪声行过滤正则
    pub output_filter_regex: String,
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
    /// 配置版本号（用于 Agent 市场更新检测）
    pub version: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl AgentConfig {
    /// CLI 可执行命令：优先取运行时规格里的 command，缺失时退回 agent_type。
    ///
    /// 取代旧的 `agents.cli_command` 列——同一份信息只在 `lifecycle` 一处维护。
    pub fn cli_command(&self) -> String {
        if let Some(spec) = &self.lifecycle {
            let bins = self
                .pkg_state
                .as_ref()
                .map(|s| s.bins.clone())
                .unwrap_or_default();
            if let Some(rt) = spec.resolved_runtime(&bins) {
                return rt.command;
            }
        }
        self.agent_type.clone()
    }

    /// 是否可以承载会话：API 行（transport='http'）恒可用；其余看包状态。
    ///
    /// 判定**只认 `pkg_state`**，不看 `lifecycle` 是否存在：
    /// - 手工在集成配置页填写的 Agent（生命周期为空、包状态为空）视为就绪——
    ///   命令写在 `run_cmd_template` / `conn` 里，由用户自行保证可用；
    /// - 市场「获取」创建的草稿一定带 `pending_install`，安装完成前拦下会话；
    /// - 安装失败带 `failed` + 错误原因。
    pub fn readiness(&self) -> Readiness {
        match &self.pkg_state {
            None => Readiness::Ready,
            Some(s) if s.is_ready() => Readiness::Ready,
            Some(s) if s.status == crate::agent::lifecycle::STATUS_PENDING_INSTALL => {
                Readiness::NotReady {
                    message: "该 Agent 尚未完成安装".to_string(),
                }
            }
            Some(s) => Readiness::NotReady {
                message: s
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "该 Agent 安装状态异常".to_string()),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub enum Readiness {
    Ready,
    NotReady { message: String },
}

/// 从运行命令模板里取可执行名（第一个 token）：`claude -p ...` → `claude`。
///
/// 手工配置的本地 CLI 没有生命周期规格，只能靠它推断「是否可用」。
pub fn executable_from_template(template: &str) -> Option<String> {
    let first = template
        .split_whitespace()
        .next()?
        .trim_matches(|c| c == '"' || c == '\'');
    if first.is_empty() || first.contains('{') {
        return None;
    }
    Some(first.to_string())
}

impl AgentConfig {
    /// 会话真正 spawn 的启动命令：
    /// - ACP：`conn.command`（安装完成时已回写为解析后的运行时可执行名）；
    /// - 本地 CLI：运行模板的第一个 token。
    ///
    /// 返回 `None` 表示这份配置没有可判定的启动命令（如在线 API 形态），
    /// 调用方不应据此判定不可用。
    pub fn launch_command(&self, transport: &str, conn: Option<&str>) -> Option<String> {
        if transport == "acp" {
            let raw = conn?;
            let v: serde_json::Value = serde_json::from_str(raw).ok()?;
            let cmd = v.get("command")?.as_str()?.trim().to_string();
            if cmd.is_empty() {
                return None;
            }
            Some(cmd)
        } else {
            executable_from_template(&self.run_cmd_template)
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentPayload {
    pub agent_type: String,
    pub display_name: String,
    pub description: Option<String>,
    /// 生命周期规格（市场获取/手工新建都会带；后端据此派生安装与运行命令）
    pub lifecycle: Option<LifecycleSpec>,
    /// 本地包管理状态（市场「获取」创建草稿时为 pending_install）
    pub pkg_state: Option<PkgState>,
    /// 配置来源
    pub dist_source: Option<DistSource>,
    pub run_cmd_template: Option<String>,
    pub output_parser: Option<String>,
    pub output_filter_regex: Option<String>,
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

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentPayload {
    pub agent_type: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// `None` = 保留原值；`Some` = 整体替换
    pub lifecycle: Option<LifecycleSpec>,
    /// `None` = 保留原值；`Some` = 整体替换
    pub pkg_state: Option<PkgState>,
    /// `None` = 保留原值；`Some` = 整体替换
    pub dist_source: Option<DistSource>,
    pub run_cmd_template: Option<String>,
    pub output_parser: Option<String>,
    pub output_filter_regex: Option<String>,
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
