//! 工具层：统一的工具协议、注册表与内建工具集。
//!
//! 本模块是 PilotDesk 工具架构统一的核心：
//! - 协议定义（`ToolHandler` / `RiskLevel` / `CommandRisk` / `classify_command` / `ToolTag`）
//! - 注册表（`ToolRegistry`，含按标签过滤的定义导出）
//! - 内建工具装配（`default_tools`，会话模式与群聊模式共用一套实现）
//!
//! 迁移说明（v1.0）：原协议定义与 `builtin_tool!` 宏位于 `api_agent/agent_loop.rs`，
//! 迁移到本模块后 `agent_loop.rs` 通过 `pub use crate::tools::*` 兼容既有引用。
//! `builtin_tool!` / `builtin_tool_risky!` 宏已随全部工具迁移为具名 struct 后删除。

use crate::api_agent::types::ToolDefinition;
use std::sync::Arc;

pub mod ask_user;
pub mod browser;
pub mod deps;
pub mod edit_file;
pub mod exec;
pub mod execute_command;
pub mod execute_python;
pub mod glob;
pub mod grep;
pub mod image_gen;
pub mod image_to_image;
pub mod list_files;
pub mod mcp;
pub mod memory;
pub mod read_file;
pub mod skills;
pub mod subagent;
pub mod todo_write;
pub mod web_fetch;
pub mod web_search;
pub mod write_file;

/// 工具风险等级
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// 低风险：读取文件、搜索等（静默执行）
    Low,
    /// 中风险：修改文件、网络请求等（确认后执行）
    Medium,
    /// 高风险：执行命令、删除文件等（严格确认）
    High,
}

impl RiskLevel {
    pub fn description(&self) -> &str {
        match self {
            RiskLevel::Low => "低风险操作",
            RiskLevel::Medium => "中风险操作（可能修改文件或访问网络）",
            RiskLevel::High => "高风险操作（执行系统命令或删除文件）",
        }
    }
}

/// Shell 命令风险等级（在命令执行前动态评估）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRisk {
    /// 安全：读操作，直接执行，无需审批
    Safe,
    /// 中等：写/改操作，需用户确认
    Medium,
    /// 高危：系统破坏性操作，直接拦截
    Blocked,
}

/// 分析一条 Shell 命令，返回其风险等级及越界路径提示（若有）。
/// 规则优先级：Blocked > Medium > Safe（Unknown → Medium 保守默认）
pub fn classify_command(cmd: &str) -> (CommandRisk, Option<String>) {
    let cmd_stripped = cmd.trim();
    if cmd_stripped.is_empty() {
        return (CommandRisk::Safe, None);
    }

    let cmd_lower = cmd_stripped.to_lowercase();

    // 取"&&"/"||"/";之前或第一个单词作为主命令
    let primary_cmd = cmd_stripped
        .split(|c| c == '&' || c == '|' || c == ';')
        .next()
        .unwrap_or(cmd_stripped)
        .trim()
        .to_lowercase();

    let exe = if primary_cmd.starts_with("cmd /c") {
        primary_cmd["cmd /c".len()..].trim().to_string()
    } else {
        primary_cmd.clone()
    };

    // ── Blocked：绝对拦截 ──
    let blocked_patterns: &[&str] = &[
        "format c:", "format d:", "format e:", "format /",
        "diskpart", "clean all",
        "del /f /s c:\\", "del /f /s d:\\", "del /f /s e:\\",
        "del /f /s %systemdrive%", "del /f /s %windir%", "del /f /s %systemroot%",
        "rmdir /s c:\\", "rmdir /s d:\\",
        "rd /s c:\\", "rd /s d:\\",
        "taskkill /f /im wininit", "taskkill /f /im csrss", "taskkill /f /im lsass",
        "taskkill /f /im smss", "taskkill /f /im winlogon", "taskkill /f /im services",
        "taskkill /f /im svchost", "taskkill /f /im system", "taskkill /f /im idle",
        "taskkill /f /im explorer", "taskkill /f /im dwm", "taskkill /f /im spoolsv",
        "taskkill /f /im taskmgr",
        "taskkill /f /fi \"status eq running\"",
        "taskkill /f /fi \"session eq", "taskkill /f /fi \"username eq",
        "reg delete hklm", "reg delete hkey_local_machine",
        "reg delete hkcr", "reg delete hkey_classes_root",
        "reg add hklm\\system\\currentcontrolset\\control",
        "bcdedit /delete", "bcdedit /set {default}",
        "bootsect /nt60", "bootrec /fixmbr",
        "shutdown /s", "shutdown /r", "shutdown /g",
        "shutdown /p", "shutdown /t 0",
        "schtasks /delete /tn \\microsoft", "schtasks /delete /f /tn \\microsoft",
        "net user administrator /delete",
        "takeown /f c:\\windows", "takeown /f %windir%",
        "icacls c:\\windows /grant", "icacls %windir% /grant",
        "cacls c:\\windows", "cacls %windir%",
        "echo > c:\\", "echo > %windir%", "echo > %systemroot%",
        "> c:\\windows\\", "> %windir%\\", ">> c:\\windows\\", ">> %windir%\\",
    ];
    for pat in blocked_patterns {
        if cmd_lower.contains(pat) {
            return (CommandRisk::Blocked, None);
        }
    }

    // ── 越界路径检查 ──
    if let Ok(workspace) = std::env::current_dir().and_then(|p| p.canonicalize()) {
        let ws = workspace.to_string_lossy().to_lowercase();
        for candidate in cmd_stripped.split_whitespace() {
            if candidate.starts_with("c:\\")
                || candidate.starts_with("d:\\")
                || candidate.starts_with("e:\\")
            {
                if let Ok(abs) = std::path::Path::new(candidate).canonicalize() {
                    let abs_str = abs.to_string_lossy().to_lowercase();
                    if !abs_str.starts_with(&ws) {
                        return (CommandRisk::Medium, Some(format!(
                            "路径 {} 超出工作区目录，是否仍要执行？", candidate
                        )));
                    }
                }
            }
        }
    }

    // ── Medium：写入/修改/删除操作 ──
    let medium_patterns: &[&str] = &[
        " del ", " rd ", " rmdir ",
        " copy ", " xcopy ", " move ", " robocopy ",
        " ren ", " rename ", " attrib ", " mklink ",
        " > ", " >> ",
        "reg add ", "reg delete ", "reg import ",
        "schtasks /create", "schtasks /change",
        "net user ", "net localgroup ",
        "powershell", "powershell.exe",
        "start ",
    ];
    for pat in medium_patterns {
        if cmd_lower.contains(pat) {
            return (CommandRisk::Medium, None);
        }
    }

    // ── Safe：只读/信息类操作 ──
    let safe_tokens: &[&str] = &[
        "dir", "type", "echo", "findstr", "find ",
        "where", "which", "assoc", "ftype",
        "tasklist", "taskmgr", "systeminfo", "ver",
        "vol", "fsutil", "fsstor",
        "ipconfig", "ping", "nslookup", "tracert",
        "netstat", "route", "net ",
        "sc query", "sc config",
        "title", "set", "cls", "color",
        "tree", "fc", "certutil", "signtool",
        "git ", "npm ", "pip ", "python ", "py ",
        "curl ", "wget ",
    ];
    let first_word = exe.split_whitespace().next().unwrap_or("");
    for safe in safe_tokens {
        let s = safe.trim_end_matches(' ');
        if first_word == s || first_word.starts_with(s) {
            return (CommandRisk::Safe, None);
        }
    }

    (CommandRisk::Medium, None)
}

/// 工具分类标签（域 + 能力 双维度，替代单值 category）。
///
/// 用途：
/// - 场景 allowlist 过滤（`ToolRegistry::get_definitions_filtered`）
/// - 权限分级（tags 与 `RiskLevel` 联动）
/// - 未来插件按标签暴露能力
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTag {
    // ── 域标签（能力归属）──
    /// 文件系统：read_file / list_files / glob / grep / write_file / edit_file
    Filesystem,
    /// 网络：web_search / web_fetch / browser
    Web,
    /// 命令执行：execute_command / execute_python
    Exec,
    /// 图像生成/编辑：generate_image / image_to_image / variations
    Image,
    /// MCP 外部服务器工具（运行时动态注册）
    Mcp,
    /// 代理能力：subagent / todo_write
    Agent,
    /// 人机交互：ask_user
    Interaction,

    // ── 能力标签（操作性质/成本）──
    /// 只读
    Read,
    /// 写入/修改
    Write,
    /// 执行
    Execute,
    /// 联网
    Network,
    /// 高成本（图像生成、子代理等）
    HighCost,
    /// 需要用户交互（阻塞等待）
    Interactive,
}

/// 工具执行器 trait
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    /// 工具名称
    fn name(&self) -> &str;
    /// 工具描述
    fn description(&self) -> &str;
    /// 参数定义（JSON Schema）
    fn parameters(&self) -> serde_json::Value;
    /// 工具风险等级（默认 Low）
    fn risk_level(&self) -> RiskLevel { RiskLevel::Low }
    /// 分类标签（默认空，用于场景 allowlist 过滤）
    fn tags(&self) -> &[ToolTag] { &[] }
    /// 执行工具
    async fn execute(&self, arguments: serde_json::Value) -> Result<String, String>;
}

/// 审批回调：返回 true 表示批准，false 表示拒绝
pub type ApprovalHandler = Box<dyn Fn(&str, &str, &str, RiskLevel) -> bool + Send + Sync>;

/// 迭代上限回调：返回 true 表示继续，false 表示终止
pub type ContinueHandler = Box<dyn Fn(usize, usize) -> bool + Send + Sync>;

/// 工具注册表
pub struct ToolRegistry {
    handlers: Vec<Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    pub fn register(&mut self, handler: Arc<dyn ToolHandler>) {
        self.handlers.push(handler);
    }

    /// 获取所有工具的 OpenAI 格式定义
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.handlers
            .iter()
            .map(|h| ToolDefinition::new(h.name(), h.description(), h.parameters()))
            .collect()
    }

    /// 按标签过滤导出工具定义。
    ///
    /// - `allowed` 为空数组 = 全量
    /// - 工具 `tags` 与 `allowed` 任一交集即保留
    pub fn get_definitions_filtered(&self, allowed: &[ToolTag]) -> Vec<ToolDefinition> {
        if allowed.is_empty() {
            return self.get_definitions();
        }
        self.handlers
            .iter()
            .filter(|h| h.tags().iter().any(|t| allowed.contains(t)))
            .map(|h| ToolDefinition::new(h.name(), h.description(), h.parameters()))
            .collect()
    }

    /// 执行指定工具
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String, String> {
        let args: serde_json::Value = serde_json::from_str(arguments)
            .unwrap_or(serde_json::Value::Null);

        for handler in &self.handlers {
            if handler.name() == name {
                return handler.execute(args).await;
            }
        }

        Err(format!("未知工具: {}", name))
    }

    #[allow(dead_code)]
    pub fn has_handler(&self, name: &str) -> bool {
        self.handlers.iter().any(|h| h.name() == name)
    }

    /// 获取工具的风险等级
    pub fn get_risk_level(&self, name: &str) -> Option<RiskLevel> {
        self.handlers.iter().find(|h| h.name() == name).map(|h| h.risk_level())
    }
}
