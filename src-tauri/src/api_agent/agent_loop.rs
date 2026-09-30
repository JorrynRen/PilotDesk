//! Agent Loop 任务编排
//!
//! 为 API Agent 提供 tool-calling 循环能力。
//! CLI Agent（Claude/Hermes/CodeX）不使用此模块。

use crate::api_agent::client::ApiClient;
use crate::api_agent::types::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// 工具协议自 `tools/` 模块迁入（工具架构统一 v1.0），re-export 保持既有引用兼容。
pub use crate::tools::{ApprovalHandler, ContinueHandler, RiskLevel, ToolHandler, ToolRegistry};

// ─────────────────────────────────────────────────────────────────────────────
// 会话安全模式（运行时选择，不落库）：
// 清单只分类（高风险/风险/安全），行为由「子策略矩阵」按模式等级决定。
// ─────────────────────────────────────────────────────────────────────────────

/// 会话安全模式（会话/群聊窗口选择，默认标准；随调用传参，不持久化）。
///
/// 每种模式 = 命令子策略 + 路径子策略的组合（当前为对角线组合 Lx+Lx；
/// 未来新增子策略维度只需定义该维度 L0-L3 并挂入各模式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecurityMode {
    /// 严格：命令L0 + 路径L0，所有命令和路径均严格审核
    Strict,
    /// 标准：命令L1 + 路径L1，只审核风险和未知命令和路径
    Standard,
    /// 宽松：命令L2 + 路径L2，只阻止风险命令和路径
    Relaxed,
    /// 无限制：命令L3 + 路径L3，所有命令和路径均可执行（高风险）
    Unrestricted,
}

impl Default for SecurityMode {
    fn default() -> Self {
        Self::Standard
    }
}

impl SecurityMode {
    /// 由前端传递的字符串解析（失败返回 None，调用方回退默认标准）。
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "strict" | "严格" => Some(Self::Strict),
            "standard" | "标准" => Some(Self::Standard),
            "relaxed" | "宽松" => Some(Self::Relaxed),
            "unrestricted" | "无限制" => Some(Self::Unrestricted),
            _ => None,
        }
    }

    /// 命令子策略等级（L0-L3）
    fn command_level(&self) -> u8 {
        match self {
            Self::Strict => 0,
            Self::Standard => 1,
            Self::Relaxed => 2,
            Self::Unrestricted => 3,
        }
    }

    /// 路径子策略等级（L0-L3，当前与命令等级对角线一致）
    fn path_level(&self) -> u8 {
        self.command_level()
    }
}

/// 命令/路径命中后的处理动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 禁止（直接拦截）
    Deny,
    /// 审批（需用户确认后执行）
    Approve,
    /// 放行（静默执行）
    Allow,
}

/// 命令分类（清单等级，只分类不定义行为）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandCategory {
    /// 高风险命令（deny_commands）
    HighRisk,
    /// 风险命令（risky_commands）
    Risky,
    /// 安全命令（allow_commands）
    Safe,
    /// 未命中命令
    Unknown,
}

/// 路径分类（清单等级，只分类不定义行为）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathCategory {
    /// 风险路径（deny_paths）
    HighRisk,
    /// 安全路径（allow_paths + 工作区 + 用户显式路径）
    Safe,
    /// 未命中路径
    Unknown,
}

/// 命令子策略行为矩阵（L0-L3）：
/// - 高风险命令：L0 禁 L1 禁 L2 禁 L3 放
/// - 风险命令：  L0 审 L1 审 L2 放 L3 放
/// - 安全命令：  L0 审 L1 放 L2 放 L3 放
/// - 未命中命令：L0 审 L1 审 L2 放 L3 放
fn command_action(level: u8, cat: CommandCategory) -> Action {
    match cat {
        CommandCategory::HighRisk => {
            if level <= 2 {
                Action::Deny
            } else {
                Action::Allow
            }
        }
        CommandCategory::Risky => {
            if level <= 1 {
                Action::Approve
            } else {
                Action::Allow
            }
        }
        CommandCategory::Safe => {
            if level == 0 {
                Action::Approve
            } else {
                Action::Allow
            }
        }
        CommandCategory::Unknown => {
            if level <= 1 {
                Action::Approve
            } else {
                Action::Allow
            }
        }
    }
}

/// 路径子策略行为矩阵（L0-L3）：
/// - 风险路径：L0 禁 L1 禁 L2 禁 L3 放
/// - 安全路径：L0 审 L1 放 L2 放 L3 放
/// - 未命中路径：L0 审 L1 审 L2 放 L3 放
fn path_action(level: u8, cat: PathCategory) -> Action {
    match cat {
        PathCategory::HighRisk => {
            if level <= 2 {
                Action::Deny
            } else {
                Action::Allow
            }
        }
        PathCategory::Safe => {
            if level == 0 {
                Action::Approve
            } else {
                Action::Allow
            }
        }
        PathCategory::Unknown => {
            if level <= 1 {
                Action::Approve
            } else {
                Action::Allow
            }
        }
    }
}

/// 持久化权限规则（清单分类，存于 app_settings `agent_permission_rules` key）。
///
/// 清单**只做分类**（高风险/风险/安全），**不定义行为**；
/// 行为由运行时注入的会话安全模式（`security_mode`，`#[serde(skip)]` 不落盘）按子策略矩阵决定。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRules {
    /// 安全路径
    #[serde(default)]
    pub allow_paths: Vec<String>,
    /// 风险路径
    #[serde(default)]
    pub deny_paths: Vec<String>,
    /// 安全命令
    #[serde(default)]
    pub allow_commands: Vec<String>,
    /// 高风险命令
    #[serde(default)]
    pub deny_commands: Vec<String>,
    /// 风险命令
    #[serde(default)]
    pub risky_commands: Vec<String>,
    /// 会话安全模式（运行时注入，不落盘；默认标准）
    #[serde(skip)]
    pub security_mode: SecurityMode,
}

/// 手动实现 Default（security_mode 默认标准；避免 derive 产生不确定默认值）。
impl Default for PermissionRules {
    fn default() -> Self {
        Self {
            allow_paths: Vec::new(),
            deny_paths: Vec::new(),
            allow_commands: Vec::new(),
            deny_commands: Vec::new(),
            risky_commands: Vec::new(),
            security_mode: SecurityMode::Standard,
        }
    }
}

impl PermissionRules {
    /// 命令分类 → 行为（按会话安全模式的命令子策略矩阵）。
    pub fn decide_command(&self, cmd: &str) -> Action {
        // 剥离 `cmd /c` 等包裹前缀，使清单按真实命令匹配
        let mut raw = cmd.trim().to_lowercase();
        for prefix in ["cmd /c ", "cmd.exe /c ", "cmd /d /c ", "cmd.exe /d /c "] {
            if raw.starts_with(prefix) {
                raw = raw[prefix.len()..].trim().to_string();
                break;
            }
        }
        let c = raw;
        if c.is_empty() {
            return command_action(self.security_mode.command_level(), CommandCategory::Unknown);
        }
        let level = self.security_mode.command_level();
        // 高风险命令：命令名/短语边界匹配（宽覆盖防遗漏）
        for d in &self.deny_commands {
            let p = d.trim().to_lowercase();
            if !p.is_empty() && boundary_contains(&c, &p) {
                return command_action(level, CommandCategory::HighRisk);
            }
        }
        // 风险命令：边界匹配
        for r in &self.risky_commands {
            let p = r.trim().to_lowercase();
            if !p.is_empty() && boundary_contains(&c, &p) {
                return command_action(level, CommandCategory::Risky);
            }
        }
        // 安全命令：「命令 + 首子命令」token 前缀精确匹配（`git status` 非裸 `git`）
        for a in &self.allow_commands {
            let p = a.trim().to_lowercase();
            if !p.is_empty() && cmd_token_prefix(&c, &p) {
                return command_action(level, CommandCategory::Safe);
            }
        }
        command_action(level, CommandCategory::Unknown)
    }

    /// 路径分类 → 行为（按会话安全模式的路径子策略矩阵）。
    /// `workspace_hit`：路径位于工作区/已授权缓存内（运行时安全路径，由 AgentLoop 判定传入）。
    pub fn decide_path(&self, path: &str, workspace_hit: bool) -> Action {
        let level = self.security_mode.path_level();
        if workspace_hit {
            return path_action(level, PathCategory::Safe);
        }
        let norm = path.replace('\\', "/").to_lowercase();
        if norm.is_empty() {
            return path_action(level, PathCategory::Unknown);
        }
        // 风险路径：glob 匹配
        for d in &self.deny_paths {
            let p = d.trim().replace('\\', "/").to_lowercase();
            if !p.is_empty() && glob_match(&p, &norm) {
                return path_action(level, PathCategory::HighRisk);
            }
        }
        // 安全路径：glob 匹配
        for a in &self.allow_paths {
            let p = a.trim().replace('\\', "/").to_lowercase();
            if !p.is_empty() && glob_match(&p, &norm) {
                return path_action(level, PathCategory::Safe);
            }
        }
        path_action(level, PathCategory::Unknown)
    }

    /// 脚本文件内容安全检查（write_file 工具）：高风险命令内容按命令子策略
    /// （L0-L2 禁止、L3 无限制放行），返回命中的模式串。
    pub fn deny_hit(&self, text: &str) -> Option<String> {
        if self.security_mode.command_level() >= 3 {
            return None;
        }
        let t = text.to_lowercase();
        for d in &self.deny_commands {
            let p = d.trim().to_lowercase();
            if !p.is_empty() && boundary_contains(&t, &p) {
                return Some(p);
            }
        }
        None
    }
}

/// allow 规则 token 前缀匹配：规则 token 必须逐词精确等于命令开头 token。
/// 如规则 `git status` 匹配 `git status --porcelain`，不匹配 `git` 或 `git clone`。
fn cmd_token_prefix(cmd_lower: &str, rule_lower: &str) -> bool {
    let cmd_tokens: Vec<&str> = cmd_lower.split_whitespace().collect();
    let rule_tokens: Vec<&str> = rule_lower.split_whitespace().collect();
    if rule_tokens.is_empty() || rule_tokens.len() > cmd_tokens.len() {
        return false;
    }
    rule_tokens
        .iter()
        .zip(cmd_tokens.iter())
        .all(|(r, c)| r == c)
}

/// 命令名/短语边界匹配：needle 在 hay 中出现，且两侧为分隔符/空白/字符串边界。
/// 用于 deny/risky 清单——`del` 不误伤 `model`、`rd` 不误伤 `word`；
/// 多词短语（含空格/`|`/`>` 等）按字面匹配。
fn boundary_contains(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let h: Vec<char> = hay.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    // 按字符数比较：命令串可能含中文（字节多、字符少），若只按字节长度预判，
    // 会出现"needle 字节 ≤ hay 字节但 needle 字符 > hay 字符"，下方减法下溢 panic。
    if n.len() > h.len() {
        return false;
    }
    let is_sep = |c: char| !c.is_alphanumeric();
    'outer: for start in 0..=(h.len() - n.len()) {
        // 前缀边界：串首或前一个字符非字母数字
        if start > 0 && !is_sep(h[start - 1]) {
            continue;
        }
        for (i, nc) in n.iter().enumerate() {
            if h[start + i] != *nc {
                continue 'outer;
            }
        }
        // 后缀边界：串尾或后一个字符非字母数字
        let end = start + n.len();
        if end < h.len() && !is_sep(h[end]) {
            continue;
        }
        return true;
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// 默认规则清单（启动种子写入；用户已有规则时 deny 强制并集，allow/risky 缺失填默认）
// ─────────────────────────────────────────────────────────────────────────────

/// 默认 deny_paths：系统目录（确定性破坏，禁止 Agent 触碰）
pub const DEFAULT_DENY_PATHS: &[&str] = &[
    "C:\\Windows\\**",
    "C:\\Program Files\\**",
    "C:\\Program Files (x86)\\**",
    "C:\\$Recycle.Bin\\**",
    "C:\\System Volume Information\\**",
    "C:\\Recovery\\**",
];

/// 默认 allow_paths：用户常用目录（免审批放行）。
/// 用 `*` 通配用户名（glob `*` 不跨 `/`），避免硬编码具体用户、换账号仍有效。
pub const DEFAULT_ALLOW_PATHS: &[&str] = &[
    "C:\\Users\\*\\Desktop\\**",
    "C:\\Users\\*\\Documents\\**",
    "C:\\Users\\*\\Downloads\\**",
    "C:\\Users\\*\\Pictures\\**",
    "C:\\Users\\*\\Music\\**",
    "C:\\Users\\*\\Videos\\**",
];

/// 默认 deny_commands：确定性破坏（命令名/短语边界匹配）
pub const DEFAULT_DENY_COMMANDS: &[&str] = &[
    // 系统级破坏
    "shutdown",
    "diskpart",
    "bootsect",
    "bootrec",
    "format c:",
    "format d:",
    "format e:",
    "format /",
    "regsvr32",
    "mshta",
    "rundll32",
    "bitsadmin",
    "bcdedit /delete",
    // PowerShell 任意执行面
    "powershell -enc",
    "powershell -e ",
    "powershell -executionpolicy bypass",
    "pwsh -enc",
    "pwsh -e ",
    "invoke-expression",
    "iex(",
    "iex ",
    "invoke-webrequest -outfile",
    "invoke-webrequest -o ",
    "downloadstring",
    "downloadfile",
    // Python 任意执行面
    "exec(open(",
    "os.system(",
    "subprocess",
    // 下载执行链
    "certutil -urlcache",
    "curl | bash",
    "wget | sh",
    "| powershell",
];

/// 默认 risky_commands：可能性安全（命令名/短语边界匹配，命中需审批）
pub const DEFAULT_RISKY_COMMANDS: &[&str] = &[
    // 文件写/删/移动
    "del",
    "erase",
    "rd",
    "rmdir",
    "copy",
    "xcopy",
    "move",
    "robocopy",
    "ren",
    "rename",
    "attrib",
    "mklink",
    "mkdir",
    "takeown",
    "icacls",
    "cacls",
    // 重定向/追加
    ">",
    ">>",
    "tee",
    // 注册表/计划任务/服务/账户
    "reg add",
    "reg delete",
    "reg import",
    "schtasks",
    "sc config",
    "net user",
    "net localgroup",
    "netsh",
    "setx",
    // 进程终止（普通；关键系统进程由 deny 覆盖）
    "taskkill",
    "tskill",
    // 包安装/卸载
    "npm install",
    "npm i ",
    "npm uninstall",
    "pip install",
    "pip uninstall",
    // git 写操作组
    "git add",
    "git commit",
    "git push",
    "git pull",
    "git clean",
    "git reset",
    "git checkout",
    "git merge",
    "git revert",
    "git stash",
    "git rm",
    "git fetch",
    // 下载写文件
    "curl -o",
    "curl --output",
    "wget -o",
    "wget -o ",
    "wget --output-document",
    "curl -d",
    "curl --data",
];

/// 默认 allow_commands：整体安全（「命令 + 首子命令」token 前缀匹配）
pub const DEFAULT_ALLOW_COMMANDS: &[&str] = &[
    // 目录/文件只读
    "dir",
    "ls",
    "tree",
    "type",
    "more",
    "find",
    "findstr",
    "fc",
    "where",
    "which",
    "cd",
    "pwd",
    // 系统信息只读
    "systeminfo",
    "ver",
    "hostname",
    "whoami",
    "tasklist",
    "ipconfig",
    "ping",
    "nslookup",
    "tracert",
    "pathping",
    "netstat",
    "getmac",
    "vol",
    "date",
    "time",
    "cls",
    "color",
    "title",
    // echo（无重定向；重定向由 risky 的 `>` 覆盖）
    "echo",
    // git 只读组
    "git status",
    "git log",
    "git diff",
    "git show",
    "git branch",
    "git remote",
    "git tag",
    "git help",
    "git ls-files",
    "git rev-parse",
    "git describe",
    "git config --get",
    // npm/pip 只读组
    "npm view",
    "npm list",
    "npm ls",
    "npm search",
    "npm info",
    "npm ping",
    "pip view",
    "pip list",
    "pip show",
    "pip search",
    "pip index",
    // powershell 只读 cmdlet
    "powershell get-process",
    "powershell get-service",
    "powershell get-childitem",
    "powershell get-content",
    "powershell get-item",
    "powershell get-command",
    "powershell get-location",
    "powershell get-date",
    "powershell get-help",
    "powershell get-alias",
    "powershell get-history",
    "powershell get-psdrive",
    "powershell get-wmiobject",
    "powershell get-ciminstance",
];

/// 默认权限规则（启动种子写入 agent_permission_rules；用户已有规则不被覆盖）。
pub fn default_permission_rules() -> PermissionRules {
    PermissionRules {
        allow_paths: DEFAULT_ALLOW_PATHS.iter().map(|s| s.to_string()).collect(),
        deny_paths: DEFAULT_DENY_PATHS.iter().map(|s| s.to_string()).collect(),
        allow_commands: DEFAULT_ALLOW_COMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        deny_commands: DEFAULT_DENY_COMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        risky_commands: DEFAULT_RISKY_COMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        security_mode: SecurityMode::Standard,
    }
}

/// 简单 glob 匹配：`*` 不跨 `/`，`**` 跨任意字符，`?` 匹配单个非 `/` 字符。
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // (pattern_idx, text_idx)
    let mut memo = std::collections::HashSet::new();
    fn matches(
        p: &[char],
        t: &[char],
        pi: usize,
        ti: usize,
        memo: &mut std::collections::HashSet<(usize, usize)>,
    ) -> bool {
        if !memo.insert((pi, ti)) {
            return false;
        }
        if pi == p.len() {
            return ti == t.len();
        }
        match p[pi] {
            '*' => {
                // `**` 跨任意，`*` 不跨 `/`
                let cross = pi + 1 < p.len() && p[pi + 1] == '*';
                let step = if cross { 2 } else { 1 };
                if matches(p, t, pi + step, ti, memo) {
                    return true;
                }
                if ti < t.len() && (cross || t[ti] != '/') {
                    if matches(p, t, pi, ti + 1, memo) {
                        return true;
                    }
                }
                false
            }
            '?' => {
                if ti < t.len() && t[ti] != '/' {
                    matches(p, t, pi + 1, ti + 1, memo)
                } else {
                    false
                }
            }
            c => {
                if ti < t.len() && t[ti] == c {
                    matches(p, t, pi + 1, ti + 1, memo)
                } else {
                    false
                }
            }
        }
    }
    matches(&p, &t, 0, 0, &mut memo)
}

/// 授权模式 → 需要用户确认的最低风险等级（`None` = 一律不审）。
///
/// 审批尺度**只由「工具授权」（`SecurityMode`）决定**，不再有第二套开关：
/// 模式越严，越低的风险等级也要问；无限制则全放。方向与命令/路径子策略矩阵一致
/// （L0 最严 → L3 全放），所以「严格」下任何工具调用都会先问一次。
fn approval_floor(mode: SecurityMode) -> Option<RiskLevel> {
    match mode {
        SecurityMode::Strict => Some(RiskLevel::Low),
        SecurityMode::Standard => Some(RiskLevel::Medium),
        SecurityMode::Relaxed => Some(RiskLevel::High),
        SecurityMode::Unrestricted => None,
    }
}

/// ask_user 工具的行为（区分会话与群聊场景）：
/// - Session：工具内阻塞等待用户回复（60s 超时后交还模型）——会话模式现状；
/// - TurnEnd：群聊模式，拦截 ask_user 调用 → 转为轮末确认请求（不阻塞、无超时，
///   由房间确认流落库等待 + 恢复现场），本轮以确认请求结束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskUserBehavior {
    Session,
    TurnEnd,
}

impl Default for AskUserBehavior {
    fn default() -> Self {
        Self::Session
    }
}

/// Agent Loop 配置
pub struct AgentLoopConfig {
    /// 最大迭代次数（软上限，达到后请求用户确认）
    pub max_iterations: usize,
    /// 系统提示词（含 MEMORY.md + USER.md + 技能列表）
    pub system_prompt: String,
    /// 可用工具列表
    pub tools: Vec<ToolDefinition>,
    /// 会话历史消息（不含 system prompt）
    pub messages: Vec<ChatMessage>,
    /// 模型温度（0.0-2.0），None 表示不传递
    pub temperature: Option<f64>,
    /// 最大生成 token 数，None 表示使用模型默认
    pub max_tokens: Option<u32>,
    /// 上下文窗口大小（token），None 使用默认
    #[allow(dead_code)]
    pub context_tokens: Option<usize>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 20,
            system_prompt: String::new(),
            tools: Vec::new(),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            context_tokens: None,
        }
    }
}

/// Agent Loop 执行结果
pub struct AgentLoopOutput {
    /// 最终回复内容
    pub content: String,
    /// 完整对话消息（不含 system prompt，供会话上下文持久化）
    pub messages: Vec<ChatMessage>,
    /// 本轮工具调用链（tool_start/tool_result 步骤，供群聊消息持久化溯源）。
    pub tool_calls: Vec<ThinkingChainStep>,
}

/// 统一用量写入口：INSERT `api_usage_log`（含 provider 归因）。
/// 缓存拆分：`cache_read`=缓存命中读取、`cache_write`=缓存写入；
/// `cached_tokens` 列恒等于两者之和（兼容既有汇总/展示）。
/// 单 Agent `AgentLoop`、群聊纯补全/工具两分支共用，避免重复 SQL。
/// 返回 `conn.execute` 结果；调用方在 Ok 时才向前端发射 `usage-recorded` 脏标记。
pub fn record_usage_row(
    conn: &rusqlite::Connection,
    scope_key: &str,
    provider: &str,
    model: &str,
    api_format: &ApiFormat,
    prompt: u32,
    completion: u32,
    total: u32,
    cache_read: u32,
    cache_write: u32,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO api_usage_log (session_id, provider, model, api_format, prompt_tokens, completion_tokens, total_tokens, cached_tokens, cache_read_tokens, cache_write_tokens, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            scope_key,
            provider,
            model,
            format!("{:?}", api_format),
            prompt,
            completion,
            total,
            cache_read + cache_write,
            cache_read,
            cache_write,
            crate::utils::now(),
        ],
    )
}

/// Agent Loop 编排器
///
/// 执行 tool-calling 循环：
///   用户输入 → LLM 调用 → 有 tool_calls?
///   ├─ 是 → 风险评估 → 执行工具 → 结果注入 → 继续 LLM
///   └─ 否 → 返回最终答案
pub struct AgentLoop {
    client: ApiClient,
    tool_registry: Arc<ToolRegistry>,
    model: String,
    temperature: f64,
    approval_handler: Option<ApprovalHandler>,
    /// 审批方身份标签（拒绝文案用，默认 "用户"；群聊场景由 Director 裁决时为 "主持人"）
    approval_label: String,
    /// 是否把审批请求作为"用户需亲自决策"事件发往前端（`agent-approval-required` 弹窗）。
    /// 仅用户亲自审批的路径置 true；程序化裁决（群聊主持人等）保持 false，
    /// 否则会在用户界面弹出与当前运行无关的审批弹窗（跨页面/跨会话审批混乱）。
    notify_user_approval: bool,
    /// 迭代上限回调（达到限制时请求用户确认是否继续）
    continue_handler: Option<ContinueHandler>,
    /// Tauri AppHandle，用于直接发送事件到前端（绕过广播 channel，确保实时性）
    app_handle: tauri::AppHandle,
    /// 当前会话 ID
    session_id: String,
    /// 用量归因的 provider id（群聊工具分支经 `run_agent_turn` 注入；空则回退查 `sessions.api_provider`）
    provider_label: String,
    /// 用量落库的 scope_key；None 时回退 `session_id`。
    usage_scope: Option<String>,
    /// 当前会话已授权的文件路径（避免同一文件反复弹窗）
    approved_files: std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// 当前会话已授权的目录（该目录下所有 write_file 自动放行）
    approved_dirs: std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// 已批准执行的 execute_command（按规范化命令字符串缓存，避免重复弹窗）
    approved_commands: std::sync::Mutex<std::collections::HashSet<String>>,
    /// 持久化权限规则（allow/deny 列表）
    permission_rules: PermissionRules,
    /// 工作区目录（会话 cwd / 群聊房间 cwd）：工作区内路径免审批（授权边界）
    workspace: Option<String>,
    /// API 格式（OpenAI / Anthropic）
    api_format: ApiFormat,
    /// 流式增量回调（逐 chunk 透出，供群聊框架 token_stream 使用）
    on_delta: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    /// "实质产出进展"心跳（每完成一轮有产出的迭代触发一次）：群聊执行期由任务级停滞
    /// 秒表订阅，区分"长任务正常推进"与"任务停滞"——有产出不误杀，仅零产出超时才中断。
    progress_cb: Option<Arc<dyn Fn() + Send + Sync>>,
    /// 协作式取消令牌（群聊场景由房间控制标志注入；会话模式为 None，零影响）。
    /// 在迭代边界检查：已取消则提前结束本轮工具循环（当前工具调用原子完成后退出）。
    cancel_token: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// ask_user 工具行为（会话=工具内阻塞+60s 超时；群聊=拦截转轮末确认）。
    ask_user_behavior: AskUserBehavior,
}

impl AgentLoop {
    pub fn new(
        client: ApiClient,
        tool_registry: Arc<ToolRegistry>,
        model: String,
        app_handle: tauri::AppHandle,
        session_id: String,
    ) -> Self {
        Self {
            client,
            tool_registry,
            model,
            temperature: 0.7,
            approval_handler: None,
            approval_label: "用户".to_string(),
            notify_user_approval: false,
            continue_handler: None,
            app_handle,
            session_id,
            provider_label: String::new(),
            usage_scope: None,
            approved_files: std::sync::Mutex::new(std::collections::HashSet::new()),
            approved_dirs: std::sync::Mutex::new(std::collections::HashSet::new()),
            approved_commands: std::sync::Mutex::new(std::collections::HashSet::new()),
            permission_rules: PermissionRules::default(),
            workspace: None,
            api_format: ApiFormat::default(),
            on_delta: None,
            progress_cb: None,
            cancel_token: None,
            ask_user_behavior: AskUserBehavior::Session,
        }
    }

    /// 设置工作区目录（工作区内路径免审批；会话传 session.cwd，群聊传房间 cwd）。
    pub fn with_workspace(mut self, workspace: Option<String>) -> Self {
        self.workspace = workspace;
        self
    }

    /// 判断路径是否位于工作区目录内（授权边界判定；相对路径基于工作区解析）。
    fn is_in_workspace(&self, raw_path: &str) -> bool {
        let Some(ws) = &self.workspace else {
            return false;
        };
        if ws.trim().is_empty() {
            return false;
        }
        let norm = normalize_path(raw_path, ws);
        let ws_norm = normalize_path(ws, "");
        norm.starts_with(&ws_norm)
    }

    /// 预缓存：用户消息中显式给出的绝对路径 → 写入本会话授权缓存（读写免审批）。
    /// 语义：用户亲自提供的路径视为已知安全行为；仅当前会话生效，不污染持久化规则。
    fn pre_cache_user_paths(&self, messages: &[ChatMessage]) {
        let cwd = self.workspace.as_deref().unwrap_or("");
        let mut found: Vec<String> = Vec::new();
        for m in messages {
            if m.role != "user" {
                continue;
            }
            let Some(content) = &m.content else { continue };
            for token in content.split_whitespace() {
                let t = trim_trailing_punct(token);
                if is_abs_windows_path(&t) {
                    found.push(t.to_string());
                }
            }
        }
        if found.is_empty() {
            return;
        }
        let mut files = self.approved_files.lock().unwrap();
        let mut dirs = self.approved_dirs.lock().unwrap();
        for p in &found {
            let norm = normalize_path(p, cwd);
            log::info!(
                "[AgentLoop] 用户显式路径预缓存（会话内免审批）: {}",
                norm.display()
            );
            files.insert(norm.clone());
            if let Some(parent) = norm.parent() {
                dirs.insert(parent.to_path_buf());
            }
        }
    }

    /// 设置协作式取消令牌：迭代边界检查，并注入客户端使流式读取也能被即时中断
    /// （否则"停止生成"要等整段生成结束才生效）。
    pub fn with_cancel_token(mut self, token: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.client = self.client.clone().with_abort(Arc::clone(&token));
        self.cancel_token = Some(token);
        self
    }

    /// 是否已收到协作式取消信号。
    fn cancelled(&self) -> bool {
        self.cancel_token
            .as_ref()
            .is_some_and(|ct| ct.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// 设置 ask_user 工具行为（会话默认=工具内阻塞；群聊=拦截转轮末确认）。
    pub fn with_ask_user_behavior(mut self, behavior: AskUserBehavior) -> Self {
        self.ask_user_behavior = behavior;
        self
    }

    /// 设置流式增量回调（每个 LLM 文本分片调用一次）
    pub fn with_on_delta(mut self, on_delta: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.on_delta = Some(on_delta);
        self
    }

    pub fn with_on_progress(mut self, progress_cb: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.progress_cb = Some(progress_cb);
        self
    }

    pub fn with_api_format(mut self, format: ApiFormat) -> Self {
        self.api_format = format;
        self
    }

    /// 设置用量归因的 provider id（群聊工具分支经 `run_agent_turn` 注入；单 Agent 保持默认空 → 查 `sessions`）。
    pub fn with_provider_label(mut self, label: &str) -> Self {
        self.provider_label = label.to_string();
        self
    }

    /// 设置用量落库的 scope_key（群聊房间级 `groupchat:{roomId}`；None 回退 `session_id`，
    /// 不影响 `agent-*` 事件使用的 `session_id`）。
    pub fn with_usage_scope(mut self, scope: &str) -> Self {
        self.usage_scope = Some(scope.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn with_temperature(mut self, temp: f64) -> Self {
        self.temperature = temp;
        self
    }

    /// 设置持久化权限规则（allow/deny 列表）
    pub fn with_permission_rules(mut self, rules: PermissionRules) -> Self {
        self.permission_rules = rules;
        self
    }

    /// 设置会话安全模式（运行时选择，写入 rules.security_mode；本次/本波次有效）。
    pub fn with_security_mode(mut self, mode: SecurityMode) -> Self {
        self.permission_rules.security_mode = mode;
        self
    }

    /// 设置审批回调
    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    /// 设置审批方身份标签（拒绝文案用，如 "用户" / "主持人"）
    pub fn with_approval_label(mut self, label: &str) -> Self {
        self.approval_label = label.to_string();
        self
    }

    /// 标记本循环的审批由用户亲自决策：向前端发 `agent-approval-required` 弹窗等待回复。
    /// 程序化裁决路径（如群聊主持人 `authorize_tool`）不要调用，否则会向用户弹出与本次
    /// 运行无关的审批（跨页面/跨会话混淆）。
    pub fn with_user_approval_notification(mut self) -> Self {
        self.notify_user_approval = true;
        self
    }

    /// 设置迭代上限回调（达到限制时阻塞等待用户确认）
    pub fn with_continue_handler(mut self, handler: ContinueHandler) -> Self {
        self.continue_handler = Some(handler);
        self
    }

    /// 检查是否需要用户审批（唯一依据：「工具授权」`security_mode`）
    fn needs_approval(&self, risk: &RiskLevel) -> bool {
        match approval_floor(self.permission_rules.security_mode) {
            Some(floor) => *risk >= floor,
            None => false,
        }
    }

    /// 直接发送事件到前端（不经过广播 channel，确保审批前实时送达）
    fn emit_to_frontend(&self, event: AgentLoopEvent) {
        use tauri::Emitter;
        let sid = &self.session_id;
        match event {
            AgentLoopEvent::Reasoning { content } => {
                let _ = self.app_handle.emit(
                    "agent-reasoning",
                    serde_json::json!({
                        "sessionId": sid,
                        "content": content,
                    }),
                );
            }
            AgentLoopEvent::Chunk { content } => {
                let _ = self.app_handle.emit(
                    "agent-chunk",
                    serde_json::json!({
                        "sessionId": sid,
                        "content": content,
                    }),
                );
            }
            AgentLoopEvent::ToolStart {
                id,
                name,
                arguments,
            } => {
                let _ = self.app_handle.emit(
                    "agent-tool-start",
                    serde_json::json!({
                        "sessionId": sid,
                        "toolId": id,
                        "toolName": name,
                        "arguments": arguments,
                    }),
                );
            }
            AgentLoopEvent::ToolResult {
                id,
                name,
                result,
                success,
            } => {
                let _ = self.app_handle.emit(
                    "agent-tool-result",
                    serde_json::json!({
                        "sessionId": sid,
                        "toolId": id,
                        "toolName": name,
                        "result": result,
                        "success": success,
                    }),
                );
            }
            AgentLoopEvent::ApprovalRequired {
                call_id,
                tool_name,
                arguments,
                risk_description,
            } => {
                let _ = self.app_handle.emit(
                    "agent-approval-required",
                    serde_json::json!({
                        "sessionId": sid,
                        "toolId": call_id,
                        "toolName": tool_name,
                        "arguments": arguments,
                        "riskDescription": risk_description,
                    }),
                );
            }
            AgentLoopEvent::Done { content } => {
                // 事件触发钩子：会话一轮生成结束（工作流内部会话会在派发器里被过滤掉）
                crate::workflow::triggers::dispatch_event(
                    &self.app_handle,
                    "session.completed",
                    serde_json::json!({
                        "sessionId": sid,
                        "content": content.clone(),
                    }),
                );
                let _ = self.app_handle.emit(
                    "agent-done",
                    serde_json::json!({
                        "sessionId": sid,
                        "content": content,
                    }),
                );
            }
            AgentLoopEvent::Cancelled { content } => {
                let _ = self.app_handle.emit(
                    "agent-cancelled",
                    serde_json::json!({
                        "sessionId": sid,
                        "content": content,
                    }),
                );
            }
            AgentLoopEvent::IterationLimit { current, max } => {
                let _ = self.app_handle.emit(
                    "agent-iteration-limit",
                    serde_json::json!({
                        "sessionId": sid,
                        "current": current,
                        "max": max,
                    }),
                );
            }
            AgentLoopEvent::Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
                cached_tokens,
            } => {
                let _ = self.app_handle.emit(
                    "agent-usage",
                    serde_json::json!({
                        "sessionId": sid,
                        "promptTokens": prompt_tokens,
                        "completionTokens": completion_tokens,
                        "totalTokens": total_tokens,
                        "cachedTokens": cached_tokens,
                    }),
                );
            }
        }
    }

    /// 记录一次 LLM 调用 token 用量到 `api_usage_log`（缓存读/写拆分落库，`cached_tokens` 列=两者之和）。
    /// 仅记录；失败（库未就绪等）静默跳过，不影响主流程。
    /// provider 优先取 `provider_label`（群聊工具分支注入），为空再查本会话 `sessions.api_provider`。
    /// 写库成功后才发射 `usage-recorded`（用量面板脏标记；`agent-usage` 仍保留用于逐 token 展示）。
    fn record_usage(
        &self,
        prompt: u32,
        completion: u32,
        total: u32,
        cache_read: u32,
        cache_write: u32,
    ) {
        use rusqlite::OptionalExtension;
        use tauri::Manager;
        let Some(state) = self.app_handle.try_state::<crate::DbState>() else {
            return;
        };
        let Ok(conn) = state.pool.get() else { return };
        let provider = if !self.provider_label.is_empty() {
            self.provider_label.clone()
        } else {
            conn.query_row(
                "SELECT api_provider FROM sessions WHERE id = ?1",
                rusqlite::params![self.session_id],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or_default()
        };
        let scope_key = self
            .usage_scope
            .clone()
            .unwrap_or_else(|| self.session_id.clone());
        if record_usage_row(
            &conn,
            &scope_key,
            &provider,
            &self.model,
            &self.api_format,
            prompt,
            completion,
            total,
            cache_read,
            cache_write,
        )
        .is_ok()
        {
            use tauri::Emitter;
            let _ = self.app_handle.emit(
                "usage-recorded",
                serde_json::json!({
                    "sessionId": self.session_id,
                    "provider": provider,
                    "model": self.model,
                }),
            );
        }
    }

    /// 执行 Agent Loop
    pub async fn run(&self, config: AgentLoopConfig) -> Result<AgentLoopOutput, String> {
        let mut messages: Vec<ChatMessage> = Vec::new();
        let mut max_iters = config.max_iterations;

        // 1. 注入 system prompt
        if !config.system_prompt.is_empty() {
            messages.push(ChatMessage::system(&config.system_prompt));
        }

        // 2. 添加历史消息 + 用户输入
        messages.extend(config.messages);

        // 用户显式提供的路径 → 预写入本会话授权缓存（读写该路径免审批）。
        // 语义：用户亲自给出的路径视为已知安全行为；作用域仅当前会话，不污染持久化规则。
        self.pre_cache_user_paths(&messages);

        let tools = if config.tools.is_empty() {
            None
        } else {
            Some(config.tools)
        };

        // 3. Agent Loop 主循环（软上限 + 用户确认可继续）
        let mut iteration: usize = 0;
        let mut empty_response_count: usize = 0; // 连续空响应计数
        let mut reasoning_only_count: usize = 0; // 连续只推理不调用工具的次数
                                                 // 进展感知动态收敛：连续无进展轮数。有进展（工具成功且结果非空 / 输出非空文本）即清零。
                                                 // 达到阈值先注入收敛提示，再超阈值则请求 continue_handler（群聊为 Director 裁决）决定收尾。
        const STALL_THRESHOLD: usize = 3;
        let mut stall_count: usize = 0;
        let mut stall_prompted: bool = false;
        // 上下文溢出恢复护栏：每轮 run 仅允许"强裁剪重试"一次，置位后本轮请求一律按 0.5 窗口发送。
        let mut overflow_recovered: bool = false;
        loop {
            // ── 协作式取消检查（迭代边界）：房间停止/暂停时提前结束工具循环，
            //    当前正在执行的工具调用已完成（原子），不再进入下一轮 LLM 迭代。 ──
            if self.cancelled() {
                log::info!(
                    "[AgentLoop] 收到协作式取消信号，提前结束工具循环（已执行 {} 轮）",
                    iteration
                );
                self.emit_to_frontend(AgentLoopEvent::Cancelled {
                    content: String::new(),
                });
                return Ok(AgentLoopOutput {
                    content: String::new(),
                    messages: strip_system(&messages),
                    tool_calls: extract_tool_calls(&messages),
                });
            }

            // ── 迭代上限检查：达到限制时请求用户确认 ──
            if iteration >= max_iters {
                log::warn!("[AgentLoop] 达到迭代上限: {}/{}", iteration, max_iters);
                self.emit_to_frontend(AgentLoopEvent::IterationLimit {
                    current: iteration,
                    max: max_iters,
                });

                let should_continue = if let Some(ref handler) = self.continue_handler {
                    handler(iteration, max_iters)
                } else {
                    false
                };

                if should_continue {
                    max_iters += 15; // 追加 15 轮
                    log::info!("[AgentLoop] 用户确认继续，新上限: {}", max_iters);
                } else {
                    log::info!("[AgentLoop] 用户选择终止，进入总结收尾（不返回空内容）");
                    self.emit_to_frontend(AgentLoopEvent::Done {
                        content: String::new(),
                    });
                    return self.finish_with_summary(&messages, iteration).await;
                }
            }

            // 1C 阶段一/二：请求前压力应急兜底（对"本次发送副本"生效，不动事实源）。
            // context_tokens 记录当前路由上下文窗口（仅会话模式传入；群聊为 None 不启用）。
            // 估算"非 system 消息"占用超过窗口 80%，或本轮已发生上下文溢出恢复（overflow_recovered）
            // 时，按窗口（正常=ctx；溢出恢复后=0.5*ctx 强裁剪）裁剪发送副本，防止长工具链中途打满
            // 真实上下文。messages（全量日志）保持不变，由外层在轮末按 CompactionPolicy 决定滚动摘要。
            let send_messages = match config.context_tokens {
                Some(ctx) if ctx > 0 => {
                    let used = estimate_non_system(&messages);
                    let over = overflow_recovered || used as f64 > ctx as f64 * 0.8;
                    if over {
                        let budget = if overflow_recovered {
                            (ctx / 2).max(1)
                        } else {
                            ctx
                        };
                        log::warn!(
                            "[AgentLoop] 请求前上下文压力 {}/{}（>80% 或溢出恢复中），对本次发送副本裁剪",
                            used,
                            ctx
                        );
                        crate::api_agent::context::SlidingWindow::new(budget).trim(&messages)
                    } else {
                        messages.clone()
                    }
                }
                _ => messages.clone(),
            };

            let request = ChatRequest {
                model: self.model.clone(),
                messages: send_messages,
                tools: tools.clone(),
                tool_choice: None,
                stream: true,
                temperature: config.temperature,
                max_tokens: config.max_tokens,
                response_format: None,
            };

            // 调用 LLM（流式）
            let mut content_buffer = String::new();
            // 思考模式（DeepSeek 等）：累积 reasoning_content，随 assistant 消息回传。
            let mut reasoning_buffer = String::new();
            // 本轮最终用量（usage 回调可能多次触发，最后一次为准）；None 表示 provider 未返回 usage。
            // 五元组：prompt / completion / total / cache_read / cache_write。
            let mut last_usage: Option<(u32, u32, u32, u32, u32)> = None;

            let slf = self;
            let stream_result = match self.api_format {
                ApiFormat::Anthropic => {
                    self.client
                        .chat_stream_anthropic(
                            &request,
                            |chunk| {
                                content_buffer.push_str(chunk);
                                slf.emit_to_frontend(AgentLoopEvent::Chunk {
                                    content: chunk.to_string(),
                                });
                                if let Some(cb) = &slf.on_delta {
                                    cb(chunk);
                                }
                            },
                            |_| {}, // Anthropic 不支持 reasoning_content
                            |call_id, name, arguments| {
                                slf.emit_to_frontend(AgentLoopEvent::ToolStart {
                                    id: call_id.to_string(),
                                    name: name.to_string(),
                                    arguments: arguments.to_string(),
                                });
                            },
                            |prompt, completion, total, cache_read, cache_write| {
                                last_usage =
                                    Some((prompt, completion, total, cache_read, cache_write));
                                slf.emit_to_frontend(AgentLoopEvent::Usage {
                                    prompt_tokens: prompt,
                                    completion_tokens: completion,
                                    total_tokens: total,
                                    // 前端仍为单值缓存口径：总缓存 = 缓存读取 + 缓存写入。
                                    cached_tokens: cache_read + cache_write,
                                });
                            },
                        )
                        .await
                }
                _ => {
                    self.client
                        .chat_stream(
                            &request,
                            |chunk| {
                                content_buffer.push_str(chunk);
                                slf.emit_to_frontend(AgentLoopEvent::Chunk {
                                    content: chunk.to_string(),
                                });
                                if let Some(cb) = &slf.on_delta {
                                    cb(chunk);
                                }
                            },
                            |reasoning| {
                                reasoning_buffer.push_str(reasoning);
                                slf.emit_to_frontend(AgentLoopEvent::Reasoning {
                                    content: reasoning.to_string(),
                                });
                            },
                            |call_id, name, arguments| {
                                slf.emit_to_frontend(AgentLoopEvent::ToolStart {
                                    id: call_id.to_string(),
                                    name: name.to_string(),
                                    arguments: arguments.to_string(),
                                });
                            },
                            |prompt, completion, total, cache_read, cache_write| {
                                last_usage =
                                    Some((prompt, completion, total, cache_read, cache_write));
                                slf.emit_to_frontend(AgentLoopEvent::Usage {
                                    prompt_tokens: prompt,
                                    completion_tokens: completion,
                                    total_tokens: total,
                                    // 前端仍为单值缓存口径：总缓存 = 缓存读取 + 缓存写入。
                                    cached_tokens: cache_read + cache_write,
                                });
                            },
                        )
                        .await
                }
            };

            // 上下文溢出恢复：命中 provider 溢出文案且尚未恢复时，强裁剪(0.5 窗口)发送副本并重试一次；
            // 必须证明 after < before（真的变小）才放行，否则保留原始错误大声失败。
            let response = match stream_result {
                Ok(r) => r,
                Err(e) => {
                    let recoverable = config.context_tokens.filter(|&c| c > 0).is_some_and(|_| {
                        !overflow_recovered && crate::api_agent::client::is_context_overflow(&e)
                    });
                    if recoverable {
                        let ctx = config.context_tokens.unwrap_or_default();
                        let before = estimate_non_system(&messages);
                        let strong =
                            crate::api_agent::context::SlidingWindow::new((ctx / 2).max(1))
                                .trim(&messages);
                        let after = estimate_non_system(&strong);
                        if after < before {
                            overflow_recovered = true;
                            log::warn!(
                                "[AgentLoop] 上下文溢出恢复：强裁剪后重试 iteration={}, before={}, after={}",
                                iteration + 1,
                                before,
                                after
                            );
                            iteration += 1;
                            continue;
                        }
                        log::warn!(
                            "[AgentLoop] 上下文溢出但强裁剪未缩小（before={}, after={}），放弃自动恢复",
                            before,
                            after
                        );
                    }
                    let prefix = if matches!(self.api_format, ApiFormat::Anthropic) {
                        "chat_stream_anthropic 失败"
                    } else {
                        "chat_stream 失败"
                    };
                    log::error!("[AgentLoop] {}: {}", prefix, e);
                    // 不在此发 agent-error：所有失败统一由 run_api_agent_inner 收到本函数的 Err 后
                    // 发一次（见 lib.rs）。这里再发一次，前端同一失败会收到两条完全相同的通知。
                    return Err(e);
                }
            };

            // P2-A 缓存命中测量：provider 返回了 usage 时落库（prompt/completion/cache_read/cache_write），
            // 供会话 token 统计与缓存命中率基线。群聊 Director/参与者经本 Loop 的请求同记录。
            if let Some((prompt, completion, total, cache_read, cache_write)) = last_usage {
                self.record_usage(prompt, completion, total, cache_read, cache_write);
            }

            // 用户请求停止：流式读取被取消令牌中断。保留已累积正文交由前端收尾落库，
            // 不当作错误（走 Err 会让前端弹出失败提示，并把已产出的内容丢掉）。
            if self.cancelled() {
                log::info!(
                    "[AgentLoop] 已取消，停止工具循环（已产出 {} 字）",
                    content_buffer.len()
                );
                self.emit_to_frontend(AgentLoopEvent::Cancelled {
                    content: content_buffer.clone(),
                });
                return Ok(AgentLoopOutput {
                    content: content_buffer,
                    messages: strip_system(&messages),
                    tool_calls: extract_tool_calls(&messages),
                });
            }

            log::info!(
                "[AgentLoop] chat_stream 返回: iteration={}/{}, tool_calls={}, content_len={}, finish_reason={}",
                iteration + 1, max_iters, response.tool_calls.len(), content_buffer.len(), response.finish_reason
            );

            // 无工具调用 → 检查是否有实际内容
            if response.tool_calls.is_empty() {
                // 关键区分：finish_reason == "tool_calls" 表示模型意图调用工具
                // 但某些模型（如 Agnes）在 reasoning 阶段只输出 reasoning_content 而
                // 不带 tool_calls，此时应继续迭代而不是当作完成
                let is_tool_call_finish = response.finish_reason == "tool_calls";

                if content_buffer.trim().is_empty() {
                    empty_response_count += 1;
                    if is_tool_call_finish {
                        // 模型发出了 tool_calls 信号但没有具体工具调用
                        // 说明模型还在"思考"阶段，继续迭代
                        reasoning_only_count += 1;
                        log::info!(
                            "[AgentLoop] LLM 发出 tool_calls 信号但无具体调用，继续迭代: {}/{}，连续空响应: {}，连续推理: {}",
                            iteration + 1, max_iters, empty_response_count, reasoning_only_count
                        );
                        // 连续推理 2 轮后注入更强提示
                        if reasoning_only_count >= 2 {
                            log::warn!(
                                "[AgentLoop] 连续 {} 轮只推理不调用工具，注入强制提示",
                                reasoning_only_count
                            );
                            messages.push(ChatMessage::system(
                                "你已连续多轮只输出思考内容而未推进任务。请基于当前任务直接给出结论；若确实需要查看文件、搜索或执行操作，再调用相应工具，不要继续描述计划。"
                            ));
                            reasoning_only_count = 0;
                        }
                        iteration += 1;
                        continue;
                    }
                    // 模型明确返回了 stop 但内容为空
                    reasoning_only_count = 0; // 非 tool_calls finish，重置推理计数
                    log::warn!(
                        "[AgentLoop] LLM 返回空内容+无工具调用, iteration={}/{}, finish_reason={}, 连续空响应={}",
                        iteration + 1, max_iters, response.finish_reason, empty_response_count
                    );
                    // 允许最多 2 次重试（总共 3 次尝试）
                    if empty_response_count <= 2 {
                        let retry_prompt = match empty_response_count {
                            1 => "请基于当前任务继续推进：若需要查看文件或目录、搜索或执行操作，请调用相应工具；否则请直接给出回答。".to_string(),
                            2 => "你尚未给出有效回复。请直接完成当前任务：需要时调用工具，不需要时直接输出结论。".to_string(),
                            // 上面的 `<= 2` 已排除其它取值；留兜底文案而不是 unreachable!，
                            // 免得将来放宽重试次数时这里变成一次崩溃
                            _ => "请直接输出本轮任务的结论。".to_string(),
                        };
                        log::info!(
                            "[AgentLoop] 空响应第 {} 次，注入提示重试",
                            empty_response_count
                        );
                        messages.push(ChatMessage::user(&retry_prompt));
                        iteration += 1;
                        continue;
                    }
                    // 超过重试次数 → 返回友好提示
                    let fallback = format!(
                        "模型未能生成有效回复（已连续 {} 次空响应）。请尝试重新提问，或更换支持工具调用的模型。",
                        empty_response_count
                    );
                    self.emit_to_frontend(AgentLoopEvent::Done {
                        content: fallback.clone(),
                    });
                    return Ok(AgentLoopOutput {
                        content: fallback,
                        messages: strip_system(&messages),
                        tool_calls: extract_tool_calls(&messages),
                    });
                }

                // 有文本内容，正常结束
                log::info!(
                    "[AgentLoop] 发送 Done 事件, content_len={}",
                    content_buffer.len()
                );
                self.emit_to_frontend(AgentLoopEvent::Done {
                    content: content_buffer.clone(),
                });
                log::info!("[AgentLoop] Done 事件已发送");
                // 将最终回复追加到消息历史，保证 AgentLoopOutput.messages 完整：
                // 会话连续性由事件派生重建，但滚动摘要的切分/增量合并仍基于 messages。
                messages.push(ChatMessage::assistant_with_reasoning(
                    &content_buffer,
                    &reasoning_buffer,
                ));
                return Ok(AgentLoopOutput {
                    content: content_buffer,
                    messages: strip_system(&messages),
                    tool_calls: extract_tool_calls(&messages),
                });
            }

            // 保存 assistant 消息（含 tool_calls，保留前缀文本）
            let mut assistant_msg = ChatMessage::assistant_with_tools_and_reasoning(
                response.tool_calls.clone(),
                &reasoning_buffer,
            );
            if !content_buffer.is_empty() {
                assistant_msg.content = Some(std::mem::take(&mut content_buffer));
            }
            messages.push(assistant_msg);

            // 本轮是否产生实质进展（用于停滞检测）：有非空文本输出，或任一工具执行成功且结果非空。
            let mut made_progress = !content_buffer.trim().is_empty();

            // 执行每个工具调用
            for tc in &response.tool_calls {
                // 群聊模式（TurnEnd）：拦截 ask_user 工具调用 → 转为轮末确认请求。
                // 不执行工具（避免工具内 60s 阻塞被轮级超时掐断、oneshot 不落库丢失恢复现场），
                // 而是把参数解析为群聊确认协议 JSON（{"askUser":true,...}）作为本轮内容返回，
                // 由框架层解析为 TurnResult.confirmation → 房间确认流（无限等待 + 落库恢复）。
                if self.ask_user_behavior == AskUserBehavior::TurnEnd
                    && tc.function.name == "ask_user"
                {
                    log::info!(
                        "[AgentLoop] 群聊拦截 ask_user，转为轮末确认请求: {}",
                        tc.function.arguments
                    );
                    let confirmation_json =
                        confirmation_json_from_ask_user_args(&tc.function.arguments)
                            .ok_or_else(|| "ask_user 缺少 prompt 参数".to_string())?;
                    // 实时工具链闭合（群聊前端按 event_session_id 组装 ThinkingChain）。
                    self.emit_to_frontend(AgentLoopEvent::ToolResult {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        result: "已转为确认请求，等待用户回复。".to_string(),
                        success: true,
                    });
                    return Ok(AgentLoopOutput {
                        content: confirmation_json,
                        messages: strip_system(&messages),
                        tool_calls: extract_tool_calls(&messages),
                    });
                }

                // 路径类工具的路径子策略动作（Allow=免审批，Deny=拦截，Approve=审批）
                let mut path_allow = false;
                // 查找工具的风险等级：execute_command 由命令子策略驱动，
                // 路径类工具（write_file/edit_file/read_file）由路径子策略驱动，
                // 其余工具使用注册时等级
                let risk = if tc.function.name == "execute_command" {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                    match self.permission_rules.decide_command(cmd) {
                        Action::Deny => {
                            // 禁止：直接拦截，无需走审批流程
                            let msg =
                                format!("安全拦截：命令命中禁止规则，已被阻止。命令: {}", cmd);
                            self.emit_to_frontend(AgentLoopEvent::ToolResult {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                result: msg.clone(),
                                success: false,
                            });
                            messages.push(ChatMessage::tool_result(&tc.id, &msg));
                            continue;
                        }
                        Action::Approve => RiskLevel::Medium,
                        Action::Allow => RiskLevel::Low,
                    }
                } else if matches!(
                    tc.function.name.as_str(),
                    "write_file" | "edit_file" | "read_file" | "read_image"
                ) {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    let action = args.get("path").and_then(|v| v.as_str()).map(|p| {
                        // 工作区 / 已授权缓存（用户显式预缓存、审批通过缓存）内的路径 = 运行时安全路径
                        let workspace_hit = self.is_in_workspace(p)
                            || (tc.function.name == "write_file"
                                && is_write_path_approved(
                                    &self.approved_files,
                                    &self.approved_dirs,
                                    &tc.function.arguments,
                                    self.workspace.as_deref().unwrap_or(""),
                                ));
                        self.permission_rules.decide_path(p, workspace_hit)
                    });
                    match action {
                        Some(Action::Deny) => {
                            let msg = "安全规则拦截：该操作命中风险路径，已被阻止。".to_string();
                            self.emit_to_frontend(AgentLoopEvent::ToolResult {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                result: msg.clone(),
                                success: false,
                            });
                            messages.push(ChatMessage::tool_result(&tc.id, &msg));
                            continue;
                        }
                        Some(Action::Approve) => RiskLevel::Medium,
                        Some(Action::Allow) => {
                            path_allow = true;
                            RiskLevel::Low
                        }
                        None => RiskLevel::Low,
                    }
                } else {
                    self.tool_registry
                        .get_risk_level(&tc.function.name)
                        .unwrap_or(RiskLevel::Medium)
                };

                // 风险审批（路径 Allow 动作豁免，其余按风险等级）
                if self.needs_approval(&risk) && !path_allow {
                    // ── 路径授权缓存：同一文件/目录不重复弹窗 ──
                    let path_cached = if tc.function.name == "write_file" {
                        is_write_path_approved(
                            &self.approved_files,
                            &self.approved_dirs,
                            &tc.function.arguments,
                            self.workspace.as_deref().unwrap_or(""),
                        )
                    } else {
                        false
                    };

                    // ── 命令授权缓存：同一 execute_command 不重复弹窗 ──
                    let cmd_cached =
                        if tc.function.name == "execute_command" && risk == RiskLevel::Medium {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                            is_execute_command_approved(&self.approved_commands, cmd)
                        } else {
                            false
                        };

                    let approved = if path_cached || cmd_cached {
                        log::info!(
                            "[AgentLoop] 缓存命中，免审批: {} (path_cache={}, cmd_cache={})",
                            tc.function.name,
                            path_cached,
                            cmd_cached
                        );
                        true
                    } else if let Some(ref handler) = self.approval_handler {
                        // 仅"用户亲自审批"的路径才发弹窗事件；程序化裁决（群聊主持人等）
                        // 由 handler 直接给出结论，不向前端请求审批。
                        if self.notify_user_approval {
                            self.emit_to_frontend(AgentLoopEvent::ApprovalRequired {
                                call_id: tc.id.clone(),
                                tool_name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                                risk_description: risk.description().to_string(),
                            });
                        }
                        handler(
                            &tc.id,
                            &tc.function.name,
                            &tc.function.arguments,
                            risk.clone(),
                        )
                    } else {
                        log::warn!(
                            "[AgentLoop] 无审批回调，拒绝高风险工具: {}",
                            tc.function.name
                        );
                        false
                    };

                    if !approved {
                        let msg = format!(
                            "{}拒绝了工具调用: {}",
                            self.approval_label, tc.function.name
                        );
                        self.emit_to_frontend(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: msg.clone(),
                            success: false,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &msg));
                        continue;
                    }

                    // 授权通过后缓存路径（下次同一文件免审批）
                    if tc.function.name == "write_file" {
                        cache_approved_path(
                            &self.approved_files,
                            &self.approved_dirs,
                            &tc.function.arguments,
                            self.workspace.as_deref().unwrap_or(""),
                        );
                    }
                    // 授权通过后缓存命令（下次同一命令免审批，与 cmd_cached 读侧配套）
                    if tc.function.name == "execute_command" && risk == RiskLevel::Medium {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                        cache_approved_command(&self.approved_commands, cmd);
                    }
                }

                // 执行工具。外层兜底超时的豁免判定集中为一个纯函数（tool_exec_timeout_secs，
                // 见文件尾部）：仅"无内部自管边界"的普通工具受 60s 盲超时保护，四类豁免工具
                // （ask_user/generate_video/execute_command/execute_python）内部已自管执行边界，
                // 外层不盲杀；豁免名单与时长只在该函数一处维护，与历史行为保持一致。
                log::info!(
                    "[AgentLoop] 开始执行工具: {}, args={}",
                    tc.function.name,
                    tc.function.arguments
                );
                let tool_name_clone = tc.function.name.clone();
                let exec_result = if let Some(secs) = tool_exec_timeout_secs(&tool_name_clone) {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(secs),
                        self.tool_registry
                            .execute(&tc.function.name, &tc.function.arguments),
                    )
                    .await
                    {
                        Ok(r) => r,
                        // 保留原超时文案（前缀"工具执行超时"被 extract_tool_calls 用于展示判定）。
                        Err(_elapsed) => Err(format!("工具执行超时（超过 {} 秒）", secs)),
                    }
                } else {
                    self.tool_registry
                        .execute(&tc.function.name, &tc.function.arguments)
                        .await
                };
                match exec_result {
                    Ok(result) => {
                        log::info!(
                            "[AgentLoop] 工具执行成功: {}, result_len={}",
                            tool_name_clone,
                            result.len()
                        );
                        self.emit_to_frontend(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: result.clone(),
                            success: true,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &result));
                        // 实质进展判定：仅"有实际产出"的工具成功计为进展。
                        // execute_command / execute_python 的输出仅为文本（echo/状态/命令回显等），
                        // 若计入进展会被"反复 echo 空转"的模型用于不断重置停滞检测，导致任务卡死；
                        // 不计入后由停滞检测（收敛提示 + 主持人裁决）尽快收尾，且不影响正常长任务
                        // （停滞收敛前有 3 轮缓冲 + 主持人可裁决续命，长工具本身仍有独立超时）。
                        if !result.trim().is_empty()
                            && tc.function.name != "execute_command"
                            && tc.function.name != "execute_python"
                        {
                            made_progress = true;
                        }
                    }
                    Err(err) => {
                        log::error!(
                            "[AgentLoop] 工具执行失败: {}, error={}",
                            tool_name_clone,
                            err
                        );
                        self.emit_to_frontend(AgentLoopEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            result: err.clone(),
                            success: false,
                        });
                        messages.push(ChatMessage::tool_result(&tc.id, &format!("错误: {}", err)));
                    }
                }
            }

            iteration += 1;

            // ── 停滞检测：进展感知动态收敛，替代对固定轮数的硬依赖 ──
            if made_progress {
                stall_count = 0;
                stall_prompted = false;
                // 实质产出 → 任务级进展心跳（上层停滞秒表据此重置，不误杀长任务）
                if let Some(cb) = self.progress_cb.as_ref() {
                    cb();
                }
            } else {
                stall_count += 1;
                if stall_count >= STALL_THRESHOLD {
                    if !stall_prompted {
                        // 第一次达到阈值：注入收敛提示，给模型最后一次推进机会。
                        stall_prompted = true;
                        stall_count = 0;
                        log::warn!(
                            "[AgentLoop] 连续 {} 轮无进展，注入收敛提示",
                            STALL_THRESHOLD
                        );
                        messages.push(ChatMessage::system(
                            "你已连续多轮未推进任务（未成功执行工具且未输出内容）。请基于已完成的工具结果直接给出最终结论；若确有必要，再调用具体工具继续推进，不要空转。",
                        ));
                    } else {
                        // 已提示一次仍无进展：请求 continue_handler（群聊为 Director 裁决）决定是否收尾。
                        let should_continue = self
                            .continue_handler
                            .as_ref()
                            .map(|h| h(iteration, STALL_THRESHOLD))
                            .unwrap_or(false);
                        if should_continue {
                            stall_prompted = false;
                            stall_count = 0;
                            log::info!("[AgentLoop] 停滞经裁决继续推进");
                        } else {
                            log::warn!(
                                "[AgentLoop] 连续无进展（{} 轮）且裁决收尾，进入总结收尾",
                                STALL_THRESHOLD + 1
                            );
                            return self.finish_with_summary(&messages, iteration).await;
                        }
                    }
                }
            }
        }
    }

    /// 优雅收尾：终止前总结已完成的工作，避免返回空内容，或二次执行时从头再来。
    ///
    /// 优先追加一轮非流式"总结轮"（基于最近工具结果归纳最终结论）；
    /// 总结轮失败或超时则退化为从工具调用链提取确定性摘要。
    async fn finish_with_summary(
        &self,
        messages: &[ChatMessage],
        iteration: usize,
    ) -> Result<AgentLoopOutput, String> {
        let tool_calls = extract_tool_calls(messages);
        let summary_text = self.summarize_work(messages).await;
        log::info!(
            "[AgentLoop] 收尾完成: iteration={}, summary_len={}",
            iteration,
            summary_text.len()
        );
        self.emit_to_frontend(AgentLoopEvent::Done {
            content: summary_text.clone(),
        });
        Ok(AgentLoopOutput {
            content: summary_text,
            messages: strip_system(messages),
            tool_calls,
        })
    }

    /// 尝试用 LLM 总结已完成的工作；失败时退化为工具调用链摘要。
    async fn summarize_work(&self, messages: &[ChatMessage]) -> String {
        let fallback = build_work_summary(messages);
        if fallback.is_empty() {
            return String::new();
        }
        // 追加总结指令，非流式单次调用（不触发 on_delta，避免前端流式与收尾文本混淆）。
        let mut summary_messages = messages.to_vec();
        summary_messages.push(ChatMessage::user(
            "请根据以上对话（尤其是已执行的工具调用及其结果）总结你到目前为止已完成的工作、获得的关键信息与结论，作为最终回答输出。不要执行任何新工具，不要继续原任务，不要输出计划。",
        ));
        let request = ChatRequest {
            model: self.model.clone(),
            messages: summary_messages,
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: Some(0.5),
            max_tokens: Some(2000),
            response_format: None,
        };
        match self.client.chat(&request).await {
            Ok(resp) if !resp.content.trim().is_empty() => resp.content,
            _ => fallback,
        }
    }
}

/// 从工具调用链生成确定性工作摘要（工具名 + 结果摘要），供收尾兜底与失败续接使用。
fn build_work_summary(messages: &[ChatMessage]) -> String {
    let steps = extract_tool_calls(messages);
    let mut lines: Vec<String> = Vec::new();
    for step in steps {
        if step.step_type == "tool_result" {
            let name = step.tool_name.as_deref().unwrap_or("tool");
            let result = step.tool_result.as_deref().unwrap_or("");
            let truncated = if result.chars().count() > 200 {
                let s: String = result.chars().take(200).collect();
                format!("{}…", s)
            } else {
                result.to_string()
            };
            lines.push(format!("- {}：{}", name, truncated));
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("已完成以下工作（工具调用记录）：\n{}", lines.join("\n"))
    }
}

/// 剔除 system 消息（主 system prompt 与循环中注入的强制提示），
/// 仅保留 user/assistant/tool 对话消息，供会话上下文持久化。
fn strip_system(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .filter(|m| m.role != "system")
        .cloned()
        .collect()
}

/// 群聊轮末确认：把 ask_user 工具参数解析为群聊确认协议 JSON（`{"askUser":true,...}`），
/// 与框架层 `parse_confirmation_request`（participant.rs）解析的字段约定保持一致，
/// 使框架将其解析为 `TurnResult.confirmation` 进入房间确认流。
fn confirmation_json_from_ask_user_args(args: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args).ok()?;
    let prompt = v["prompt"].as_str().unwrap_or("").trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    let reply_mode = v["replyMode"]
        .as_str()
        .or_else(|| v["reply_mode"].as_str())
        .unwrap_or("open");
    let reply_mode = if reply_mode == "structured" {
        "structured"
    } else {
        "open"
    };
    let title = v["title"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| crate::groupchat::models::summarize_confirmation_title(&s))
        .unwrap_or_else(|| crate::groupchat::models::summarize_confirmation_title(&prompt));
    // items 原样透传（key 约定与 ask_user 参数一致：id/label/inputType/options/required/placeholder）。
    let items = if reply_mode == "structured" {
        v["items"].as_array().cloned().unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(
        serde_json::json!({
            "askUser": true,
            "title": title,
            "prompt": prompt,
            "replyMode": reply_mode,
            "items": items,
        })
        .to_string(),
    )
}

/// 从 AgentLoop 累积的对话消息中还原工具调用链（reasoning/tool_start/tool_result），供消息持久化溯源。
///
/// 工具执行成功与否通过 tool_result 内容的错误前缀判定，仅用于前端展示（✅/❌）。
/// v3.4v：assistant 消息携带 reasoning_content（思考模型）时提取为 reasoning 步骤，
/// 使会话/群聊落库消息的思维链可回看推理过程（与实时 agent-reasoning 事件同源）。
fn extract_tool_calls(messages: &[ChatMessage]) -> Vec<ThinkingChainStep> {
    let mut steps: Vec<ThinkingChainStep> = Vec::new();
    let mut name_by_id: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let now = crate::utils::now();
    let mut reasoning_seq: usize = 0;

    for m in messages {
        match m.role.as_str() {
            "assistant" => {
                // 推理步骤（思考模型）：reasoning_content 随 assistant 消息原样回传，落库供回看。
                if let Some(rc) = m
                    .reasoning_content
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    reasoning_seq += 1;
                    steps.push(ThinkingChainStep {
                        id: format!("reasoning-{}", reasoning_seq),
                        step_type: "reasoning".to_string(),
                        content: Some(rc.to_string()),
                        tool_name: None,
                        tool_args: None,
                        tool_result: None,
                        tool_success: None,
                        file_path: None,
                        file_diff: None,
                        timestamp: now,
                    });
                }
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        name_by_id.insert(tc.id.clone(), tc.function.name.clone());
                        steps.push(ThinkingChainStep {
                            id: tc.id.clone(),
                            step_type: "tool_start".to_string(),
                            content: None,
                            tool_name: Some(tc.function.name.clone()),
                            tool_args: Some(tc.function.arguments.clone()),
                            tool_result: None,
                            tool_success: None,
                            file_path: None,
                            file_diff: None,
                            timestamp: now,
                        });
                    }
                }
            }
            "tool" => {
                if let Some(call_id) = &m.tool_call_id {
                    let tool_name = name_by_id.get(call_id).cloned();
                    let raw = m.content.clone().unwrap_or_default();
                    let success = !raw.starts_with("错误:")
                        && !raw.contains("拒绝了工具调用")
                        && !raw.starts_with("安全拦截")
                        && !raw.starts_with("安全规则拦截")
                        && !raw.starts_with("工具执行超时");
                    steps.push(ThinkingChainStep {
                        id: call_id.clone(),
                        step_type: "tool_result".to_string(),
                        content: None,
                        tool_name,
                        tool_args: None,
                        tool_result: Some(raw),
                        tool_success: Some(success),
                        file_path: None,
                        file_diff: None,
                        timestamp: now,
                    });
                }
            }
            _ => {}
        }
    }

    steps
}

/// 检查 write_file 的路径是否已在授权缓存中（命中文件精确项或父目录前缀项）
fn is_write_path_approved(
    approved_files: &std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    approved_dirs: &std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    arguments: &str,
    cwd: &str,
) -> bool {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let raw_path = match args["path"].as_str() {
        Some(p) => p,
        None => return false,
    };
    let check = normalize_path(raw_path, cwd);
    let files = approved_files.lock().unwrap();
    let dirs = approved_dirs.lock().unwrap();
    files.contains(&check) || dirs.iter().any(|d| check.starts_with(d))
}

/// 将 write_file 的路径加入授权缓存（文件本身 + 父目录）
fn cache_approved_path(
    approved_files: &std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    approved_dirs: &std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    arguments: &str,
    cwd: &str,
) {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(_) => return,
    };
    let raw_path = match args["path"].as_str() {
        Some(p) => p,
        None => return,
    };
    let canonical = normalize_path(raw_path, cwd);

    // 缓存文件路径
    approved_files.lock().unwrap().insert(canonical.clone());

    // 同时缓存父目录（该目录下所有 write_file 后续全免审批）
    if let Some(parent) = canonical.parent() {
        approved_dirs.lock().unwrap().insert(parent.to_path_buf());
        log::info!(
            "[AgentLoop] 路径授权缓存: file={}, dir={}",
            canonical.display(),
            parent.display()
        );
    }
}

/// 路径规范化（授权缓存匹配统一入口）：
/// 展开 `~`/`%USERPROFILE%` → 相对路径基于 cwd 绝对化 → 词法清理（`.`/`..`）→
/// 存在则 canonicalize（解析符号链接），不存在保留词法规范化结果（支持「创建新文件」场景）。
fn normalize_path(raw: &str, cwd: &str) -> std::path::PathBuf {
    let expanded = crate::tools::deps::expand_user_path(raw);
    let pb = std::path::PathBuf::from(expanded);
    let abs = if pb.is_absolute() {
        pb
    } else if cwd.is_empty() {
        pb
    } else {
        std::path::Path::new(cwd).join(pb)
    };
    let lexical = lexical_normalize(&abs);
    match lexical.canonicalize() {
        Ok(c) => c,
        Err(_) => lexical,
    }
}

/// 词法规范化：移除 `.` 组件、弹出 `..` 组件（不要求路径存在）。
fn lexical_normalize(p: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 去掉路径 token 尾部常见标点（逗号/句号/引号/括号等），避免误判。
fn trim_trailing_punct(s: &str) -> &str {
    s.trim().trim_end_matches(|c: char| {
        matches!(
            c,
            ',' | ';'
                | '.'
                | '。'
                | '，'
                | '；'
                | '：'
                | ')'
                | '）'
                | ']'
                | '】'
                | '"'
                | '\u{201C}'
                | '\u{201D}'
                | '\''
                | '`'
                | '>'
                | '}'
                | '!'
        )
    })
}

/// 是否 Windows 盘符绝对路径（如 `C:\foo\bar` / `D:/x`）。
fn is_abs_windows_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// 检查 execute_command 的命令是否已在授权缓存中
fn is_execute_command_approved(
    approved_commands: &std::sync::Mutex<std::collections::HashSet<String>>,
    cmd: &str,
) -> bool {
    let normalized = cmd.trim().to_lowercase();
    approved_commands.lock().unwrap().contains(&normalized)
}

/// 将 execute_command 的命令加入授权缓存
fn cache_approved_command(
    approved_commands: &std::sync::Mutex<std::collections::HashSet<String>>,
    cmd: &str,
) {
    let normalized = cmd.trim().to_lowercase();
    approved_commands.lock().unwrap().insert(normalized.clone());
    log::info!("[AgentLoop] 命令授权缓存: {}", normalized);
}

/// 工具外层兜底超时的集中判定出口：返回外层盲超时秒数，`None` = 该工具内部自管执行边界，
/// 外层不设时间限制（避免固定秒级盲杀误杀内部正常的长流程）。豁免名单集中在此维护，
/// 执行处只消费本函数结果，保证"豁免判定"与"超时时长"语义单一出处。
///
/// 豁免清单（内部自管，外层盲超时会误杀/失效）：
/// - ask_user：工具内阻塞等待用户回复，自带 60s 确认超时（超时后交还模型自行决策）；
/// - generate_video：自带任务轮询/等待超时（15 分钟级），远长于外层 60s；
/// - execute_command / execute_python：v3.5c 已异步化 + 活性检测（静默窗 + CPU 停滞复核）+
///   总执行上限（600s），真实卡死按"无输出 + CPU 停滞"终止进程树；有输出/有计算的长任务
///   不被外层 60s 盲杀（历史注释表明同步阻塞时代外层 timeout 也无效）。
///
/// 其余工具统一 60s 兜底；超时文案由执行处按返回秒数拼接（见执行处注释）。
fn tool_exec_timeout_secs(name: &str) -> Option<u64> {
    match name {
        "ask_user" | "generate_video" | "execute_command" | "execute_python" => None,
        _ => Some(60),
    }
}

/// 估算消息列表中"非 system"部分的 token 占用（用于请求前压力与溢出恢复的缩小证明；
/// 与 SlidingWindow 的"system 受保护不占预算"语义保持一致）。
fn estimate_non_system(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| crate::api_agent::context::TokenEstimator::estimate_message(m))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 审批尺度只由「工具授权」（`security_mode`）决定：模式越严，越低的风险等级也要问。
    /// 回归点：旧实现把阈值交给另一套 `AuthLevel`，导致「工具授权」对命令/路径以外的工具
    /// 完全不起作用（工作流里选任何模式都静默放行）。
    #[test]
    fn approval_floor_follows_security_mode() {
        assert_eq!(approval_floor(SecurityMode::Strict), Some(RiskLevel::Low));
        assert_eq!(
            approval_floor(SecurityMode::Standard),
            Some(RiskLevel::Medium)
        );
        assert_eq!(approval_floor(SecurityMode::Relaxed), Some(RiskLevel::High));
        assert_eq!(approval_floor(SecurityMode::Unrestricted), None);
    }

    /// 各模式的判定结果：严格全问、标准问中高、宽松只问高危、无限制不问。
    #[test]
    fn approval_decision_by_mode() {
        let needs = |mode: SecurityMode, risk: RiskLevel| matches!(approval_floor(mode), Some(floor) if risk >= floor);

        assert!(needs(SecurityMode::Strict, RiskLevel::Low));
        assert!(needs(SecurityMode::Strict, RiskLevel::High));

        assert!(!needs(SecurityMode::Standard, RiskLevel::Low));
        assert!(needs(SecurityMode::Standard, RiskLevel::Medium));
        assert!(needs(SecurityMode::Standard, RiskLevel::High));

        assert!(!needs(SecurityMode::Relaxed, RiskLevel::Medium));
        assert!(needs(SecurityMode::Relaxed, RiskLevel::High));

        assert!(!needs(SecurityMode::Unrestricted, RiskLevel::High));
    }
}
