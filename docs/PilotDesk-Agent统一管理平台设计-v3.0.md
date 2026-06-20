# PilotDesk Agent 统一管理平台设计

> **版本**: v3.3 | **日期**: 2026-06-21 | **状态**: 已实施（技能配置种子数据补全 + 拖拽排序 + Agent 市场 + 导入隐藏删除 + 卸载命令优化）

---

## 一、背景与目标

### 1.1 现状问题

当前 PilotDesk 对 Agent 的支持采用**硬编码**方式，Agent 信息散落在 **4 个 Rust 文件 + 18 个前端文件** 中：

| 层面 | 硬编码点 | 涉及文件 |
|------|---------|---------|
| Rust 后端 | `AGENTS` 配置表 | `commands/env.rs` |
| Rust 后端 | 独立安装命令（`install_claude_code` 等 3 个） | `commands/env.rs` |
| Rust 后端 | 命令注册 | `lib.rs` |
| Rust 后端 | 数据库 CHECK 约束 | `db/init.rs` |
| Rust 后端 | `AgentType` 枚举 + `build_args` + `parse_output_line` | `agent/mod.rs` |
| 前端类型 | `AGENT_THEMES` 对象（5 个条目） | `types/index.ts` |
| 前端组件 | `EnvManager` 每个 agent 的安装/检测/更新逻辑 | `components/env/EnvManager.tsx` |
| 前端组件 | `SessionList` 新建会话下拉 | `components/layout/SessionList.tsx` |
| 前端组件 | `useAgentEvent` agent 类型分发 | `hooks/useAgentEvent.ts` |
| 前端组件 | 各组件中 `agentType === 'claude'` 等条件判断 | ~20 处 |

新增或移除一个 Agent 需要修改约 **20+ 处代码**，作为 Agent 统一管理平台的定位，极不科学。

### 1.2 设计目标

1. **用户级管理**：用户可通过界面新增、移除、启用/禁用 Agent，无需修改代码
2. **元信息驱动**：后端和前端通过统一的元数据查询接口驱动运行
3. **渐进式迁移**：分阶段实施，每阶段可独立交付
4. **向后兼容**：现有数据库和用户数据无损迁移
5. **资源统一管理**：内置资源（打包携带）与用户资源（可读写）分离管理
6. **图标系统标准化**：支持内置图标文件、网络图片、Emoji 三种来源

### 1.3 设计边界

> PilotDesk 的假设前提是 Agent 已正确安装和配置。Agent 自身的认证（API Key 等）、模型选择、对话能力属于 Agent 端管理范畴，不在本软件考虑范围内。

---

## 二、Agent 元信息模型

### 2.1 核心元信息定义

所有 Agent 元信息存储在数据库 `agents` 表中，预置数据作为种子，用户可自定义新增。

```sql
CREATE TABLE IF NOT EXISTS agents (
    -- 标识
    agent_id        TEXT PRIMARY KEY,          -- 唯一标识符，如 "claude", "hermes", "custom-agent"
    name            TEXT NOT NULL,             -- 显示名称，如 "Claude Code", "Hermes Agent"
    description     TEXT NOT NULL DEFAULT '',  -- 简短描述

    -- 生命周期命令（直接存储可执行的 shell 命令，后端零推导）
    install_cmd         TEXT NOT NULL DEFAULT '',  -- 安装命令，如 "npm install -g @anthropic-ai/claude-code"
    uninstall_cmd       TEXT NOT NULL DEFAULT '',  -- 卸载命令，如 "claude uninstall"
    update_cmd          TEXT NOT NULL DEFAULT '',  -- 更新命令，如 "claude update"
    version_cmd         TEXT NOT NULL DEFAULT '',  -- 版本检测命令，如 "claude --version"
    latest_version_cmd  TEXT NOT NULL DEFAULT '',  -- 最新版本查询命令，如 "npm view @anthropic-ai/claude-code version"

    -- 进程交互
    run_cmd_template    TEXT NOT NULL DEFAULT '',  -- 启动命令模板，{message} 和 {session_id} 占位符替换
    output_parser       TEXT NOT NULL DEFAULT 'raw-text'  -- 输出解析器类型
                        CHECK(output_parser IN ('json-stream', 'ansi-text', 'raw-text')),
    output_filter_regex TEXT NOT NULL DEFAULT '',  -- 噪声行过滤正则
    version_pattern     TEXT NOT NULL DEFAULT 'v?(\d+\.\d+\.\d+[\w.-]*)',  -- 版本号提取正则

    -- 会话延续
    supports_session_continuity INTEGER NOT NULL DEFAULT 0,  -- 是否支持会话延续
    session_id_source            TEXT NOT NULL DEFAULT 'none',  -- session_id 来源：'stdout-json' / 'stderr-text' / 'none'
    session_id_event_type        TEXT NOT NULL DEFAULT '',  -- JSON 事件类型，如 'system/init' / 'thread.started'
    session_id_field             TEXT NOT NULL DEFAULT '',  -- JSON 字段名，如 'session_id' / 'thread_id'
    resume_arg_template          TEXT NOT NULL DEFAULT '',  -- 恢复参数模板，{session_id} 占位符

    -- 技能管理
    skills_dir          TEXT NOT NULL DEFAULT '',          -- 技能目录路径（支持 {agent_type} 占位符），空字符串表示使用智能目录 ~/.{agent_type}/skills/
    skill_entry_file    TEXT NOT NULL DEFAULT 'SKILL.md',  -- 技能入口文件名
    skill_display_mode  TEXT NOT NULL DEFAULT 'recursive', -- 技能显示模式：recursive（递归显示全部）或 collection（只显示集合名）

    -- 显示
    color       TEXT NOT NULL DEFAULT '#6366F1',
    icon        TEXT NOT NULL DEFAULT '\U0001F916',
    sort_order  INTEGER NOT NULL DEFAULT 0,

    -- 状态
    enabled     INTEGER NOT NULL DEFAULT 1,
    is_builtin  INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
```

### 2.2 字段说明

| 字段 | 必要性 | 说明 |
|------|--------|------|
| `agent_id` | 核心 | 唯一标识，预置 Agent 使用固定 ID（`claude`/`hermes`/`codex`），自定义 Agent 由用户指定 |
| `name` | 核心 | 用户可见的显示名称 |
| `description` | 建议 | 帮助用户了解 Agent 用途 |
| `install_cmd` | 核心 | 安装命令，直接可执行的 shell 命令 |
| `uninstall_cmd` | 核心 | 卸载命令 |
| `update_cmd` | 核心 | 更新命令 |
| `version_cmd` | 核心 | 版本检测命令 |
| `latest_version_cmd` | 核心 | 最新版本查询命令 |
| `run_cmd_template` | 核心 | 完整启动命令模板（含 CLI 命令名），`{message}` 占位符在运行时替换为用户消息 |
| `output_parser` | 核心 | 输出解析器类型：`json-stream` / `ansi-text` / `raw-text` |
| `output_filter_regex` | 建议 | 过滤噪声行的正则（匹配的行跳过不显示） |
| `version_pattern` | 核心 | 从 `version_cmd` 输出中提取版本号的正则 |
| `supports_session_continuity` | 核心 | 是否支持会话延续（`--resume` / `exec resume` 等） |
| `session_id_source` | 核心 | session_id 来源：`stdout-json`（从 stdout JSON 解析）、`stderr-text`（从 stderr 文本行解析）、`none`（不支持） |
| `session_id_event_type` | 核心 | JSON 事件类型路径，如 `system/init`、`thread.started` |
| `session_id_field` | 核心 | JSON 字段名，如 `session_id`、`thread_id` |
| `resume_arg_template` | 核心 | 恢复参数模板，`{session_id}` 占位符，如 `--resume {session_id}`、`exec resume {session_id}` |
| `skills_dir` | 建议 | 技能目录路径，支持 `{agent_type}` 占位符和 `~` 扩展；空字符串表示使用智能目录 `~/.{agent_type}/skills/` |
| `skill_entry_file` | 建议 | 技能入口文件名，默认 `SKILL.md` |
| `skill_display_mode` | 建议 | 技能显示模式：`recursive`（递归显示全部技能文件）或 `collection`（只显示集合技能名称），内置 Agent 默认 `collection` |
| `color` | 建议 | UI 主题色，用于 AgentBadge、消息气泡等 |
| `icon` | 建议 | UI 图标，支持三种值类型（见第 2.4 节） |
| `sort_order` | 建议 | 排序优先级，决定 UI 中的展示顺序 |
| `enabled` | 核心 | 用户可禁用不需要的 Agent，禁用的 Agent 不参与环境检测和会话创建 |
| `is_builtin` | 核心 | 预置 Agent 不可删除，但可禁用 |
| `created_at`/`updated_at` | 建议 | 审计 |

### 2.3 不纳入元信息的字段

以下字段**不属于** Agent 元信息：

| 字段 | 原因 |
|------|------|
| `installed_version` | 运行时状态，由 `version_cmd` 检测获取，可内存缓存 |
| `latest_version` | 运行时状态，由 `latest_version_cmd` 查询获取，可内存缓存 |
| `auth_type` / `auth_config` | Agent 自身管理范畴 |
| `supports_model_selection` / `model_list_url` | Agent 自身管理范畴 |
| `supports_tools` / `supports_skills` | Agent 自身能力范畴 |
| `max_context_length` | Agent 自身配置范畴 |

### 2.4 图标字段值类型

`icon` 字段支持三种值类型：

| 值类型 | 格式 | 示例 | 渲染方式 |
|--------|------|------|---------|
| 内置图标文件 | `file:filename.ico` | `file:claude_icon.ico` | Rust `read_agent_icon` 命令读取文件，返回 base64 data URL |
| 网络图片 | `https://...` 或 `http://...` | `https://example.com/icon.png` | 直接渲染 `<img>` 标签 |
| Emoji / 文本字符 | 任意非 `file:`/`http` 前缀文本 | `\U0001F916` | 直接渲染文本节点 |

**优先级**：`file:` 前缀 > 网络图片 > Emoji/文本。`AgentIcon` 组件按此顺序检测并渲染。

---

## 三、当前三个 Agent 的元信息

### 3.1 Claude Code

| 字段 | 值 |
|------|-----|
| `agent_id` | `claude` |
| `name` | Claude Code |
| `description` | Anthropic 官方 AI 编程助手，支持代码生成、调试、重构 |
| `install_cmd` | `npm install -g @anthropic-ai/claude-code` |
| `uninstall_cmd` | `npm uninstall -g @anthropic-ai/claude-code` |
| `update_cmd` | `claude update` |
| `version_cmd` | `claude --version` |
| `latest_version_cmd` | `npm view @anthropic-ai/claude-code version` |
| `run_cmd_template` | `claude -p --output-format stream-json --verbose --dangerously-skip-permissions {message}` |
| `output_parser` | `json-stream` |
| `output_filter_regex` | `` |
| `version_pattern` | `v?(\d+\.\d+\.\d+[\w.-]*)` |
| `supports_session_continuity` | `1` |
| `session_id_source` | `stdout-json` |
| `session_id_event_type` | `system/init` |
| `session_id_field` | `session_id` |
| `resume_arg_template` | `--resume {session_id}` |
| `skills_dir` | `~/.claude/skills/`（显式指定） |
| `skill_entry_file` | `SKILL.md` |
| `skill_display_mode` | `collection` |
| `color` | `#3B82F6`（蓝色） |
| `icon` | `file:claude_icon.ico` |
| `sort_order` | `1` |
| `is_builtin` | `1` |

**进程参数**：
- 启动命令：`claude -p --output-format stream-json --verbose --dangerously-skip-permissions {message}`
- 输出解析：JSON stream 格式，解析 `{"type": "assistant", "message": {"content": [{"type": "text", "text": "..."}]}}`
- session_id 提取：从 `{"type": "system", "subtype": "init", "session_id": "..."}` 事件提取
- 会话恢复：`--resume <session_id>` 参数
- 安装命令：`npm install -g @anthropic-ai/claude-code`
- 版本查询：`npm view @anthropic-ai/claude-code version`

### 3.2 Hermes Agent

| 字段 | 值 |
|------|-----|
| `agent_id` | `hermes` |
| `name` | Hermes Agent |
| `description` | 开源 AI 编程助手，支持多模型后端 |
| `install_cmd` | `pip install hermes-agent` |
| `uninstall_cmd` | `pip uninstall hermes-agent -y` |
| `update_cmd` | `hermes update` |
| `version_cmd` | `hermes --version` |
| `latest_version_cmd` | `powershell -NoProfile -Command (Invoke-RestMethod 'https://pypi.org/pypi/hermes-agent/json').info.version` |
| `run_cmd_template` | `hermes chat -q {message} -Q` |
| `output_parser` | `ansi-text` |
| `output_filter_regex` | `^(Initializing agent|Resume this session|hermes --resume|Session:|Duration:|Messages:|Query:)` |
| `version_pattern` | `v?(\d+\.\d+\.\d+[\w.-]*)` |
| `supports_session_continuity` | `1` |
| `session_id_source` | `stderr-text` |
| `session_id_event_type` | ``（文本行） |
| `session_id_field` | ``（前缀匹配 `session_id: `） |
| `resume_arg_template` | `--resume {session_id}` |
| `skills_dir` | `~/.codex/skills/`（显式指定） |
| `skill_entry_file` | `SKILL.md` |
| `skill_display_mode` | `collection` |
| `color` | `#8B5CF6`（紫色） |
| `icon` | `file:hermes_icon.ico` |
| `sort_order` | `2` |
| `is_builtin` | `1` |

**进程参数**：
- 启动命令：`hermes chat -q {message} -Q`
- 输出解析：ANSI 文本流，需 strip ANSI 转义码后过滤非内容行
- session_id 提取：从 stderr 文本行 `session_id: xxxxx` 提取
- 会话恢复：`--resume <session_id>` 参数
- 安装命令：`pip install hermes-agent`
- 版本查询：`powershell -NoProfile -Command (Invoke-RestMethod 'https://pypi.org/pypi/hermes-agent/json').info.version`

### 3.3 codeX

| 字段 | 值 |
|------|-----|
| `agent_id` | `codex` |
| `name` | Codex CLI |
| `description` | OpenAI 官方 AI 编程助手 |
| `install_cmd` | `npm install -g @openai/codex` |
| `uninstall_cmd` | `npm uninstall -g @openai/codex` |
| `update_cmd` | `codex update` |
| `version_cmd` | `codex --version` |
| `latest_version_cmd` | `npm view @openai/codex version` |
| `run_cmd_template` | `codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox {message}` |
| `output_parser` | `json-stream` |
| `output_filter_regex` | `` |
| `version_pattern` | `v?(\d+\.\d+\.\d+[\w.-]*)` |
| `supports_session_continuity` | `1` |
| `session_id_source` | `stdout-json` |
| `session_id_event_type` | `thread.started` |
| `session_id_field` | `thread_id` |
| `resume_arg_template` | `exec resume {session_id}` |
| `skills_dir` | `~/AppData/Local/hermes/skills/`（显式指定） |
| `skill_entry_file` | `SKILL.md` |
| `skill_display_mode` | `collection` |
| `color` | `#F59E0B`（琥珀色） |
| `icon` | `file:codex_icon.ico` |
| `sort_order` | `3` |
| `is_builtin` | `1` |

**进程参数**：
- 启动命令：`codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox {message}`
- 输出解析：JSONL 格式，解析 `{"type": "item.completed", "item": {"text": "..."}}`
- session_id 提取：从 `{"type": "thread.started", "thread_id": "..."}` 事件提取
- 会话恢复：`exec resume <session_id>` 子命令
- 安装命令：`npm install -g @openai/codex`
- 版本查询：`npm view @openai/codex version`

---

## 四、资源目录架构

### 4.1 目录结构

```
src-tauri/
├── resources/                      # 内置资源目录（打包携带，只读）
│   ├── agents/                     # 内置 Agent 配置模板（JSON）
│   ├── icons/                      # 内置 Agent 图标文件
│   │   ├── claude_icon.ico         # Claude Code 图标（33 KB）
│   │   ├── hermes_icon.ico         # Hermes Agent 图标（15 KB）
│   │   └── codex_icon.ico          # Codex CLI 图标（15 KB）
│   └── assets/                     # 其他内置资源文件
│
├── tauri.conf.json                 # bundle.resources 配置
└── capabilities/default.json       # 权限声明
```

### 4.2 内置资源 vs 用户资源

| 维度 | 内置资源（Builtin） | 用户资源（User） |
|------|--------------------|-----------------|
| 用途 | 打包携带的 Agent 配置、默认图标等 | 自定义图标、用户上传文件等 |
| 访问权限 | 只读 | 可读写 |
| 运行时路径 | `app.path().resource_dir()` | `app.path().app_data_dir().join("resources")` |
| 开发模式路径 | `src-tauri/resources/` | `%APPDATA%/PilotDesk/resources/` |
| Windows MSI | `%LOCALAPPDATA%\\com.pilotdesk.app\\resources\\` | `%APPDATA%\\PilotDesk\\resources\\` |
| Windows NSIS | `%APPDATA%\\com.pilotdesk.app\\resources\\` | `%APPDATA%\\PilotDesk\\resources\\` |

### 4.3 ResourcePaths 结构体

```rust
/// 资源路径管理
pub struct ResourcePaths {
    /// 内置资源目录（打包携带的 Agent 配置、默认图标等，只读）
    pub builtin: std::path::PathBuf,
    /// 用户资源目录（自定义图标、用户上传文件等，可读写）
    pub user: std::path::PathBuf,
}
```

**初始化流程**（在 `setup` 钩子中执行）：

```rust
.setup(|app| {
    // 初始化资源路径
    let builtin = app.path().resource_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("resources"));
    let user = app.path().app_data_dir()
        .unwrap_or_else(|_| dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join("PilotDesk"))
        .join("resources");

    // 确保用户资源子目录存在（首次运行时创建）
    for sub in &["agents", "icons", "assets"] {
        let dir = user.join(sub);
        if !dir.exists() {
            let _ = std::fs::create_dir_all(&dir);
        }
    }

    app.manage(ResourcePaths { builtin, user });
})
```

### 4.4 路径工具函数

```rust
// src-tauri/src/utils/paths.rs

/// 获取内置资源目录（打包携带的 Agent 配置、默认图标等，只读）
pub fn builtin_resources_dir() -> PathBuf {
    let mut dir = std::env::current_exe()
        .expect("Cannot determine executable path");
    dir.pop(); // 移除 exe 文件名
    dir.join("resources")
}

/// 获取用户资源目录（自定义图标、用户上传文件等，可读写）
pub fn user_resources_dir() -> PathBuf {
    app_data_dir().join("resources")
}
```

### 4.5 打包配置

在 `tauri.conf.json` 中配置 `bundle.resources`，确保内置资源随安装包分发：

```json
{
  "bundle": {
    "resources": {
      "resources/icons/*": "resources/icons/",
      "resources/agents/*": "resources/agents/"
    }
  }
}
```

---

## 五、Agent 图标系统

### 5.1 架构概览

```
+-----------------------------------------------------------------+
|                       前端 (React)                               |
|                                                                 |
|  AgentIcon 组件                                                  |
|    +-- file: 前缀 -> invoke('read_agent_icon') -> base64 data URL |
|    +-- http(s):// -> <img src="...">                              |
|    +-- Emoji/文本 -> <span>{icon}</span>                          |
|                                                                 |
|  使用场景：                                                       |
|    +-- AgentBadge（消息气泡标识）                                  |
|    +-- AgentManager（Agent 列表）                                 |
|    +-- StatusBar（底部状态栏）                                    |
|    +-- EnvManager（环境检测列表）                                 |
+---------------------------+-------------------------------------+
                            | Tauri IPC
+---------------------------v-------------------------------------+
|                     后端 (Rust)                                   |
|                                                                 |
|  read_agent_icon(icon_name, resources: ResourcePaths)            |
|    +-- 从 resources.builtin/icons/ 读取文件                      |
|    +-- 检测 MIME 类型（ico/png/svg/jpg/gif/webp）                |
|    +-- 返回 data:image/xxx;base64,...                            |
+-----------------------------------------------------------------+
```

### 5.2 AgentIcon 组件

**文件**：`src/components/common/AgentIcon.tsx`

**核心逻辑**：

```typescript
export function AgentIcon({ icon, size = 14, className = '' }: AgentIconProps) {
  // 1. file: 前缀 -> 调用 Rust read_agent_icon 命令
  // 2. http(s):// -> 直接渲染 <img>
  // 3. Emoji/文本 -> 渲染 <span>
}
```

**三种渲染模式**：

| 模式 | 触发条件 | 渲染方式 | 加载状态 |
|------|---------|---------|---------|
| 内置图标 | `icon.startsWith('file:')` | `<img src={dataUrl}>`，dataUrl 来自 Rust 命令 | 加载中显示灰色占位块 |
| 网络图片 | `icon.startsWith('http://')` 或 `https://` | `<img src={icon}>` | 加载失败时隐藏 |
| 文本模式 | 其他情况 | `<span style={{ fontSize: size }}>{icon}</span>` | 无加载状态 |

### 5.3 read_agent_icon Rust 命令

**文件**：`src-tauri/src/commands/agents.rs`

```rust
#[tauri::command]
pub fn read_agent_icon(
    icon_name: String,
    resources: tauri::State<'_, crate::ResourcePaths>
) -> Result<String, AppError> {
    let icon_path = resources.builtin.join("icons").join(&icon_name);
    let data = std::fs::read(&icon_path)?;
    let mime = match icon_path.extension().and_then(|e| e.to_str()) {
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok(format!("data:{};base64,{}", mime, b64))
}
```

### 5.4 图标文件清单

| 文件名 | 对应 Agent | 大小 | 格式 |
|--------|-----------|------|------|
| `claude_icon.ico` | Claude Code | 33 KB | ICO |
| `hermes_icon.ico` | Hermes Agent | 15 KB | ICO |
| `codex_icon.ico` | Codex CLI | 15 KB | ICO |

### 5.5 集成点

| 集成位置 | 文件 | 说明 |
|---------|------|------|
| `AGENT_THEMES` | `types/index.ts` | 每个主题条目新增 `icon` 字段 |
| `AgentBadge` | `components/common/AgentBadge.tsx` | 使用 `AgentIcon` 渲染图标 |
| `useAgentRegistry` | `hooks/useAgentRegistry.ts` | `getTheme` 返回的 theme 包含 icon |
| 种子数据 | `db/init.rs` | 预置 Agent 的 icon 字段设为 `file:xxx.ico` |
| 市场模板 | `commands/agents.rs` | 市场模板 icon 字段设为 `file:xxx.ico` |

---

## 六、表单分类设计

### 6.1 分类概览

Agent 配置表单（添加/编辑）的 28 个字段按语义分为 **6 个分类区域**，每个区域有图标标题：

| 序号 | 分类名称 | 图标 | 包含字段 | 字段数 |
|------|---------|------|---------|-------|
| 1 | 基础信息 | `Info` | agentType, displayName, description, cliCommand, color, icon, sortOrder | 7 |
| 2 | 包参数配置 | `Package` | npmPackage, pipPackage | 2 |
| 3 | 会话参数配置 | `Terminal` | runCmdTemplate, outputParser, outputFilterRegex | 3 |
| 4 | 延续会话配置 | `Repeat` | sessionIdSource, sessionIdEventType, sessionIdField, resumeArgTemplate | 4 |
| 5 | 生命周期命令配置 | `Activity` | installCmd, uninstallCmd, updateCmd, versionCmd, latestVersionCmd, versionPattern | 6 |
| 6 | 技能引用配置 | `BookOpen` | skillsDir, skillEntryFile, skillDisplayMode | 3 |

### 6.2 组件结构

```
AgentForm (统一组件，mode='add' | 'edit')
+-- 基础信息 (Info 图标)
|   +-- Agent标识（不可重复）-- FormField, edit 模式 readOnly
|   +-- 显示名称 -- FormField
|   +-- 描述 -- FormField
|   +-- CLI 命令 -- FormField
|   +-- 主题色 -- FormField
|   +-- 图标（支持 Emoji / 图片 URL）-- FormField
|   +-- 排序序号 -- FormField
|
+-- 包参数配置 (Package 图标)
|   +-- npm 包名 -- FormField
|   +-- pip 包名 -- FormField
|
+-- 会话参数配置 (Terminal 图标)
|   +-- 运行命令模板 -- FormField
|   +-- 输出解析器 -- SelectField
|   +-- 输出过滤正则 -- FormField
|
+-- 延续会话配置 (Repeat 图标)
|   +-- Session ID 来源 -- SelectField
|   +-- Session ID 事件类型 -- FormField
|   +-- Session ID 字段名 -- FormField
|   +-- 恢复参数模板 -- FormField
|
+-- 生命周期命令配置 (Activity 图标)
|   +-- 安装命令 -- FormField
|   +-- 卸载命令 -- FormField
|   +-- 更新命令 -- FormField
|   +-- 版本检测命令 -- FormField
|   +-- 最新版本查询命令 -- FormField
|   +-- 版本号提取正则 -- FormField
|
+-- 技能引用配置 (BookOpen 图标)
    +-- 技能目录路径 -- FormField
    +-- 技能入口文件名 -- FormField
    +-- 技能显示模式 -- SelectField
```

### 6.3 子组件

**FormField** -- 通用文本输入框：

```tsx
function FormField({ label, value, onChange, placeholder, readOnly }) {
  return (
    <div>
      <label>{label}</label>
      <input
        type="text"
        value={value}
        onChange={...}
        placeholder={placeholder}
        readOnly={readOnly}
        style={{ backgroundColor: readOnly ? 'var(--bg-tertiary)' : 'var(--bg-primary)' }}
      />
    </div>
  );
}
```

**SelectField** -- 枚举值下拉选择器：

```tsx
function SelectField({ label, value, onChange, options }) {
  return (
    <div>
      <label>{label}</label>
      <select value={value} onChange={...}>
        {options.map(opt => <option key={opt.value} value={opt.value}>{opt.label}</option>)}
      </select>
    </div>
  );
}
```

### 6.4 枚举字段列表

以下字段使用 `SelectField` 组件（下拉列表），禁止自由输入：

| 字段 | 可选值 |
|------|--------|
| `outputParser` | `raw-text`（原始文本）、`json-stream`（JSON 流）、`ansi-text`（ANSI 文本） |
| `sessionIdSource` | `none`（不支持会话延续）、`stdout-json`（标准输出 JSON）、`stderr-text`（标准错误文本行） |
| `skillDisplayMode` | `collection`（只显示集合名，默认）、`recursive`（递归显示全部） |

### 6.5 Placeholder 规范

所有 FormField 的 placeholder 遵循以下规范：

| 字段 | Placeholder 示例 |
|------|-----------------|
| agentType | `如 claude-code` |
| displayName | `如 Claude Code` |
| description | `如 Anthropic 官方 CLI Agent，支持多文件编辑` |
| cliCommand | `如 claude` |
| color | `如 #6366F1` |
| icon | `如 \U0001F916` |
| sortOrder | `如 0` |
| npmPackage | `如 @anthropic-ai/claude-code` |
| pipPackage | `如 hermes-agent` |
| runCmdTemplate | `如 claude --print {message}` |
| outputFilterRegex | `如 ^(?:\\[info\\]|DEBUG)` |
| versionPattern | `如 (\\d+\\.\\d+\\.\\d+)` |
| installCmd | `如 npm install -g @anthropic-ai/claude-code` |
| uninstallCmd | `如 npm uninstall -g @anthropic-ai/claude-code` |
| updateCmd | `如 npm update -g @anthropic-ai/claude-code` |
| versionCmd | `如 claude --version` |
| latestVersionCmd | `如 npm view @anthropic-ai/claude-code version` |
| sessionIdEventType | `如 system/init` |
| sessionIdField | `如 session_id` |
| resumeArgTemplate | `如 --resume {session_id}` |
| skillsDir | `如 ~/.claude/skills` 或 `~/AppData/Local/hermes/skills/` |

### 6.6 分类标题样式

每个分类标题的样式规范：

```tsx
<div className="flex items-center gap-1.5 text-[10px] font-semibold mb-2"
     style={{ color: 'var(--text-secondary)' }}>
  <Icon size={11} style={{ color: 'var(--accent)' }} />
  <span>分类名称</span>
</div>
```

- 字体：`text-[10px] font-semibold`（10px 加粗）
- 图标颜色：`var(--accent)`（主题色，非 `accent-color`）
- 标题颜色：`var(--text-secondary)`

---

## 七、UI/UX 打磨

### 7.1 Tab 命名

| 原名称 | 新名称 | 说明 |
|--------|--------|------|
| Agent 管理 | Agent 集成配置 | 强调是配置管理而非 Agent 本身管理 |
| API 配置 | API 集成配置 | 与 Agent tab 命名风格统一 |

### 7.2 布局调整

```
+-----------------------------------------------------+
|  [导出配置] [导入配置]              [+ 添加自定义 Agent] |  <- 顶部操作行
+-----------------------------------------------------+
|  添加自定义 Agent 表单（条件显示）                      |  <- 表单在按钮下方、列表上方
+-----------------------------------------------------+
|  Agent 列表                                           |
|  +-- Claude Code  [已启用] [编辑]                     |
|  +-- Hermes Agent [已启用] [编辑]                     |
|  +-- Codex CLI    [已启用] [编辑]                     |
+-----------------------------------------------------+
```

### 7.3 按钮样式

| 按钮 | 样式规则 |
|------|---------|
| 启用/禁用切换 | 启用时 `variant='primary'`（主题色），禁用时 `variant='secondary'` |
| 编辑 | `variant='secondary'` |
| 删除（自定义 Agent） | `variant='danger'`，预置 Agent 不显示删除按钮 |
| 添加自定义 Agent | `variant='primary'`，带 `Plus` 图标 |

### 7.4 悬停提示

所有操作按钮添加 `title` 属性：

| 按钮 | title 内容 |
|------|-----------|
| 启用/禁用 | `点击禁用此 Agent` / `点击启用此 Agent` |
| 编辑 | `编辑此 Agent 配置` |
| 删除 | `删除此 Agent 配置` |

### 7.5 删除二次确认

使用自定义 Modal（非系统对话框），匹配 EnvManager 风格：

```tsx
{deleteConfirm && (
  <div className="fixed inset-0 z-50 flex items-center justify-center"
       style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}>
    <div className="rounded-xl p-5 shadow-xl max-w-sm w-full mx-4"
         style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}>
      <div className="text-sm font-medium mb-2">确认删除</div>
      <div className="text-xs mb-4">
        确定要删除 <strong>{agentType}</strong> 的配置吗？此操作不可撤销。
      </div>
      <div className="flex justify-end gap-2">
        <SettingsButton variant="secondary">取消</SettingsButton>
        <SettingsButton variant="danger">确认删除</SettingsButton>
      </div>
    </div>
  </div>
)}
```

### 7.6 Agent 列表展示

每个 Agent 条目显示：

```
● Claude Code  [预置]                    [已启用] [编辑]
  claude . Anthropic 官方 AI 编程助手
```

- 第一行：主题色圆点 + 显示名称 + 预置标签 + 操作按钮
- 第二行（小字）：`CLI 命令 . 描述`

### 7.7 StatusBar 数据源

StatusBar 中 Agent 的显示顺序与 Agent 管理页面一致，均以数据库 `agents` 表的 `sort_order` 字段排序：

| 组件 | 数据源 | 排序字段 |
|------|--------|---------|
| StatusBar | DB agents（`list_agents`） | `sort_order` |
| EnvManager 环境检测列表 | DB agents（`list_agents`） | `sort_order` |
| AgentManager 列表 | DB agents（`list_agents`） | `sort_order` |
| useAgentRegistry | DB agents（`list_agents`） | `sort_order` |

### 7.8 会话创建逻辑

| 场景 | 行为 |
|------|------|
| 有启用的 Agent | 显示 Agent 选择下拉，包含所有 enabled Agent |
| 无启用的 Agent，有 API 配置 | 显示 "API 直连" 选项 |
| 无启用的 Agent 且无 API 配置 | 显示引导提示："请先在设置中配置 Agent 或 API 集成" |

---

## 八、架构设计

### 8.1 数据流

```
+-----------------------------------------------------------+
|                      前端 (React)                           |
|                                                           |
|  useAgentRegistry() <--- invoke('list_agents')             |
|       |                                                    |
|       +--- SessionList (新建会话下拉)                       |
|       +--- EnvManager (安装/检测/更新)                     |
|       +--- AgentBadge (主题色/图标)                        |
|       +--- AgentIcon (图标渲染)                            |
|       +--- MessageBubble (主题色)                          |
|       +--- StatusBar (版本显示)                            |
+--------------------------+--------------------------------+
                           | Tauri IPC
+--------------------------v--------------------------------+
|                    后端 (Rust)                              |
|                                                           |
|  list_agents() ---> agents 表 ---> 内存缓存                |
|  get_agent(id)                                            |
|  add_agent() ---> 写入 agents 表 ---> 失效缓存             |
|  update_agent() ---> 写入 agents 表 ---> 失效缓存          |
|  delete_agent() ---> 删除 agents 表 ---> 失效缓存          |
|  read_agent_icon() ---> ResourcePaths.builtin/icons/      |
|                                                           |
|  detect_env() ---> 循环 enabled agents ---> 检测 CLI       |
|  install_agent(id) ---> 从元信息读取包管理器/包名 -> 安装   |
|  check_update(id) ---> 从元信息读取 latest_version_cmd -> 直接执行命令 |
|                                                           |
|  send_message() ---> 从元信息读取 cli_command/build_args   |
|       |                                                    |
|       +--- 首次消息：run_cmd_template（无 session_id）       |
|       +--- 后续消息：run_cmd_template + resume_arg_template |
|       +--- 输出解析：根据 session_id_source 提取 session_id |
|       +--- 发射 agent-session 事件到前端                    |
+--------------------------+--------------------------------+
                           | rusqlite
+--------------------------v--------------------------------+
|                    SQLite                                  |
|                                                           |
|  agents 表 (元信息)                                        |
|  sessions 表 (会话) <- agent_type 改为 TEXT (移除 CHECK)   |
|  messages 表 (消息)                                        |
|  inspirations 表 (灵感) <- source_agent 改为 TEXT (移除 CHECK)|
+-----------------------------------------------------------+
```

### 8.2 缓存策略

```rust
// 懒加载 + 写穿透缓存
static AGENT_CACHE: Lazy<RwLock<Option<Vec<AgentConfig>>>> = Lazy::new(|| RwLock::new(None));

fn get_agents(conn: &Connection) -> Vec<AgentConfig> {
    if let Some(cache) = AGENT_CACHE.read().unwrap().as_ref() {
        return cache.clone();
    }
    let agents = load_from_db(conn);
    *AGENT_CACHE.write().unwrap() = Some(agents.clone());
    agents
}

// 写操作时失效
fn update_agent(conn: &Connection, id: &str, ...) {
    conn.execute(...);
    *AGENT_CACHE.write().unwrap() = None;
}

// 后端执行逻辑：读命令 -> 替换 {message} -> 执行 -> 返回结果
// 无需任何包管理器推导逻辑
fn exec_agent_cmd(cmd: &str) -> Result<String, AppError> {
    let parts = shlex::split(cmd).ok_or(AppError::InvalidCommand)?;
    let (program, args) = parts.split_first().unwrap();
    let output = Command::new(program).args(args).output()?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
```

### 8.3 工作目录体系

工作目录采用三层兜底策略，与元信息模型解耦，独立存储于 `app_settings` 表。

#### 默认值

- **全局默认工作区**：`~\AppData\Roaming\PilotDesk`
- **存储**：`app_settings` 表，key = `pilotdesk-workspace`
- **种子数据**：首次启动时由 `init.rs` 的 `migrate_add_app_settings` 写入

#### 三层兜底

```
创建会话时：
  用户输入路径 -> ensure_dir 校验（含 ~ 展开） -> 存入 sessions.cwd
  用户未输入   -> 读取 pilotdesk-workspace 全局设置 -> ensure_dir
  全局无设置   -> 返回 null，Rust 端回退 current_dir()

发送消息时：
  sessions.cwd 有值 -> child.current_dir(cwd)
  sessions.cwd 无值 -> child.current_dir(current_dir())
```

#### 路径校验（ensure_dir Rust 命令）

| 规则 | 说明 |
|------|------|
| 空值 | 拒绝 |
| `~` 前缀 | 展开为用户主目录（`dirs::home_dir()`） |
| 非法字符 | 拒绝 `< > " | ? *` |
| 裸盘符 | 拒绝 `C:`、`D:` |
| 无效盘符 | 拒绝非 A-Z |
| 已存在路径 | 必须是目录 |
| 不存在路径 | 自动创建 |

#### 前端校验（仅浏览选择）

- **SettingsPage**：工作区目录为**只读展示**，用户无法手动输入，仅通过"浏览"按钮调用系统文件夹选择器选择目录后直接保存
- **SessionList**：创建会话对话框中的工作目录同样为只读展示，仅可通过浏览按钮选择

### 8.4 会话延续在架构中的位置

```
首次消息发送流程：
  send_message(session_id, agent_type, message, agent_session_id=null)
    -> 从 agents 表读取 run_cmd_template
    -> 替换 {message} 占位符
    -> 启动子进程
    -> 读取 stdout/stderr
    -> 根据 session_id_source 提取 session_id
    -> 发射 agent-session 事件（含 agentSessionId）
    -> 前端调用 update_session_agent_id 持久化

后续消息发送流程：
  send_message(session_id, agent_type, message, agent_session_id="xxx")
    -> 从 agents 表读取 run_cmd_template
    -> 从 agents 表读取 resume_arg_template
    -> 替换 {message} 和 {session_id} 占位符
    -> 启动子进程（带 --resume/exec resume 参数）
    -> agent 恢复上下文，返回有记忆的响应
```

### 8.5 数据库迁移

#### Migration v11 -- 技能管理元数据化（2026-06-20）

**变更内容**：
1. 新增 `skills_dir` / `skill_entry_file` / `skill_display_mode` 三个字段
2. 移除 `version_flag` 字段（已有 `version_cmd` 后无实际意义）

**迁移方式**：重建表（SQLite 不支持 DROP COLUMN，通过 CREATE TABLE agents_new + DROP + ALTER RENAME 实现）

```sql
-- 重建 agents 表，新增技能字段，移除 version_flag
CREATE TABLE IF NOT EXISTS agents_new (
    agent_type TEXT PRIMARY KEY,
    display_name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    cli_command TEXT NOT NULL DEFAULT '',
    npm_package TEXT,
    pip_package TEXT,
    install_cmd TEXT NOT NULL DEFAULT '',
    uninstall_cmd TEXT NOT NULL DEFAULT '',
    update_cmd TEXT NOT NULL DEFAULT '',
    version_cmd TEXT NOT NULL DEFAULT '',
    latest_version_cmd TEXT NOT NULL DEFAULT '',
    run_cmd_template TEXT NOT NULL DEFAULT '',
    output_parser TEXT NOT NULL DEFAULT 'raw-text',
    output_filter_regex TEXT NOT NULL DEFAULT '',
    version_pattern TEXT NOT NULL DEFAULT 'v?(\d+\.\d+\.\d+[\w.-]*)',
    supports_session_continuity INTEGER NOT NULL DEFAULT 0,
    session_id_source TEXT NOT NULL DEFAULT 'none',
    session_id_event_type TEXT NOT NULL DEFAULT '',
    session_id_field TEXT NOT NULL DEFAULT '',
    resume_arg_template TEXT NOT NULL DEFAULT '',
    skills_dir TEXT NOT NULL DEFAULT '',
    skill_entry_file TEXT NOT NULL DEFAULT 'SKILL.md',
    skill_display_mode TEXT NOT NULL DEFAULT 'collection',
    color TEXT NOT NULL DEFAULT '#6366F1',
    icon TEXT NOT NULL DEFAULT '\U0001F916',
    sort_order INTEGER DEFAULT 0,
    is_enabled INTEGER DEFAULT 1,
    is_builtin INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

INSERT INTO agents_new SELECT
    agent_type, display_name, description, cli_command, npm_package, pip_package,
    install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd,
    run_cmd_template, output_parser, output_filter_regex, version_pattern,
    supports_session_continuity, session_id_source, session_id_event_type,
    session_id_field, resume_arg_template,
    '' AS skills_dir, 'SKILL.md' AS skill_entry_file, 'recursive' AS skill_display_mode,
    color, icon, sort_order, is_enabled, is_builtin, created_at, updated_at
FROM agents;

DROP TABLE agents;
ALTER TABLE agents_new RENAME TO agents;
```

**种子数据更新**：种子数据中移除 `version_flag` 列，新增 `skills_dir` / `skill_entry_file` / `skill_display_mode` 列，`icon` 字段从 Emoji 更新为 `file:xxx.ico`。

#### Migration v15 -- 内置 Agent 技能配置 + 卸载命令补全（2026-06-21）

**变更内容**：
1. 所有内置 Agent 显式设置 `skills_dir`（claude: `~/.claude/skills/`，hermes: `~/AppData/Local/hermes/skills/`，codex: `~/.codex/skills/`）
2. `skill_display_mode` 统一设为 `collection`
3. 内置 Agent 卸载命令改用包管理器直接卸载（`-y` 自动确认），避免交互式终端检测

#### Migration v16 -- 修复 claude 技能配置遗留数据（2026-06-21）

**变更内容**：修复 v15 执行时未覆盖的 claude 的 `skills_dir` 和 `skill_display_mode` 字段。

---

```sql
-- 1. 创建 agents 表
CREATE TABLE IF NOT EXISTS agents (
    agent_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    install_cmd TEXT NOT NULL DEFAULT '',
    uninstall_cmd TEXT NOT NULL DEFAULT '',
    update_cmd TEXT NOT NULL DEFAULT '',
    version_cmd TEXT NOT NULL DEFAULT '',
    latest_version_cmd TEXT NOT NULL DEFAULT '',
    run_cmd_template TEXT NOT NULL DEFAULT '',
    output_parser TEXT NOT NULL DEFAULT 'raw-text' CHECK(output_parser IN ('json-stream', 'ansi-text', 'raw-text')),
    output_filter_regex TEXT NOT NULL DEFAULT '',
    version_pattern TEXT NOT NULL DEFAULT 'v?(\d+\.\d+\.\d+[\w.-]*)',
    supports_session_continuity INTEGER NOT NULL DEFAULT 0,
    session_id_source TEXT NOT NULL DEFAULT 'none',
    session_id_event_type TEXT NOT NULL DEFAULT '',
    session_id_field TEXT NOT NULL DEFAULT '',
    resume_arg_template TEXT NOT NULL DEFAULT '',
    skills_dir TEXT NOT NULL DEFAULT '',
    skill_entry_file TEXT NOT NULL DEFAULT 'SKILL.md',
    skill_display_mode TEXT NOT NULL DEFAULT 'collection',
    color TEXT NOT NULL DEFAULT '#6366F1',
    icon TEXT NOT NULL DEFAULT '\U0001F916',
    sort_order INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    is_builtin INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 2. 插入预置 Agent 种子数据（含图标文件引用）
INSERT INTO agents (agent_id, name, description,
    install_cmd, uninstall_cmd, update_cmd, version_cmd, latest_version_cmd,
    run_cmd_template, output_parser, output_filter_regex,
    supports_session_continuity, session_id_source, session_id_event_type, session_id_field, resume_arg_template,
    skills_dir, skill_entry_file, skill_display_mode,
    color, icon, sort_order, is_builtin, created_at, updated_at)
VALUES
    ('claude', 'Claude Code', 'Anthropic 官方 AI 编程助手',
     'npm install -g @anthropic-ai/claude-code',
     'npm uninstall -g @anthropic-ai/claude-code',
     'claude update',
     'claude --version',
     'npm view @anthropic-ai/claude-code version',
     'claude -p --output-format stream-json --verbose --dangerously-skip-permissions {message}',
     'json-stream', '',
     1, 'stdout-json', 'system/init', 'session_id', '--resume {session_id}',
     '~/.claude/skills/', 'SKILL.md', 'collection',
     '#3B82F6', 'file:claude_icon.ico', 1, 1, 1718640000000, 1718640000000),

    ('hermes', 'Hermes Agent', '开源 AI 编程助手，支持多模型后端',
     'pip install hermes-agent',
     'pip uninstall hermes-agent -y',
     'hermes update',
     'hermes --version',
     'powershell -NoProfile -Command (Invoke-RestMethod ''https://pypi.org/pypi/hermes-agent/json'').info.version',
     'hermes chat -q {message} -Q',
     'ansi-text',
     '^(Initializing agent|Resume this session|hermes --resume|Session:|Duration:|Messages:|Query:)',
     1, 'stderr-text', '', '', '--resume {session_id}',
     '~/AppData/Local/hermes/skills/', 'SKILL.md', 'collection',
     '#8B5CF6', 'file:hermes_icon.ico', 2, 1, 1718640000000, 1718640000000),

    ('codex', 'Codex CLI', 'OpenAI 官方 AI 编程助手',
     'npm install -g @openai/codex',
     'npm uninstall -g @openai/codex',
     'codex update',
     'codex --version',
     'npm view @openai/codex version',
     'codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox {message}',
     'json-stream', '',
     1, 'stdout-json', 'thread.started', 'thread_id', 'exec resume {session_id}',
     '~/.codex/skills/', 'SKILL.md', 'collection',
     '#F59E0B', 'file:codex_icon.ico', 3, 1, 1718640000000, 1718640000000);

-- 3. 移除 sessions 表的 CHECK 约束（SQLite 需重建表）
-- 4. 移除 inspirations 表的 CHECK 约束
```

---

## 九、分阶段实施路径

### Phase 1 -- 元信息存储与查询（纯后端）

**目标**：创建 `agents` 表，迁移 `AGENTS` 常量到数据库，新增查询命令。

**状态**：:white_check_mark: **已完成**（2026-06-19）

| 任务 | 涉及文件 | 说明 | 状态 |
|------|---------|------|------|
| 创建 `agents` 表（精简版） + 种子数据 | `db/init.rs` | migration v8 | :white_check_mark: |
| 补全 `agents` 表为完整元信息模型 | `db/init.rs` | migration v9：重建 agents 表，含 install_cmd/run_cmd_template/output_parser/session_id_source/resume_arg_template/color/icon/is_builtin 等全部字段 | :white_check_mark: |
| 更新 `AgentConfig` 结构体匹配完整表 | `commands/agents.rs` | 30 个字段（新增 `skills_dir`/`skill_entry_file`/`skill_display_mode`，移除 `version_flag`） | :white_check_mark: |
| 新增 `list_agents` / `get_agent` 命令 | `commands/agents.rs` | 含完整 CRUD | :white_check_mark: |
| 新增 `add_agent` / `update_agent` / `delete_agent` 命令 | `commands/agents.rs` | delete 阻止删除预置 Agent | :white_check_mark: |
| 注册命令到 `lib.rs` | `lib.rs` | 5 个 agents 命令 | :white_check_mark: |
| 移除 `AGENTS` 常量 | `commands/env.rs` | 待 Phase 2 实施 | :hourglass: |
| 移除 CHECK 约束 | `db/init.rs` | 待 Phase 2 实施 | :hourglass: |

**实际实施的表结构**（与设计文档的完整元信息模型一致，migration v8 创建，v9 补全）：

```sql
CREATE TABLE IF NOT EXISTS agents (
    agent_type TEXT PRIMARY KEY,
    display_name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    cli_command TEXT NOT NULL DEFAULT '',
    npm_package TEXT,
    pip_package TEXT,
    install_cmd TEXT NOT NULL DEFAULT '',
    uninstall_cmd TEXT NOT NULL DEFAULT '',
    update_cmd TEXT NOT NULL DEFAULT '',
    version_cmd TEXT NOT NULL DEFAULT '',
    latest_version_cmd TEXT NOT NULL DEFAULT '',
    run_cmd_template TEXT NOT NULL DEFAULT '',
    output_parser TEXT NOT NULL DEFAULT 'raw-text',
    output_filter_regex TEXT NOT NULL DEFAULT '',
    version_pattern TEXT NOT NULL DEFAULT 'v?(\\d+\\.\\d+\\.\\d+[\\w.-]*)',
    supports_session_continuity INTEGER NOT NULL DEFAULT 0,
    session_id_source TEXT NOT NULL DEFAULT 'none',
    session_id_event_type TEXT NOT NULL DEFAULT '',
    session_id_field TEXT NOT NULL DEFAULT '',
    resume_arg_template TEXT NOT NULL DEFAULT '',
    skills_dir TEXT NOT NULL DEFAULT '',
    skill_entry_file TEXT NOT NULL DEFAULT 'SKILL.md',
    skill_display_mode TEXT NOT NULL DEFAULT 'collection',
    color TEXT NOT NULL DEFAULT '#6366F1',
    icon TEXT NOT NULL DEFAULT '\U0001F916',
    sort_order INTEGER DEFAULT 0,
    is_enabled INTEGER DEFAULT 1,
    is_builtin INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

**种子数据**（v3.0 更新：icon 字段从 Emoji 改为 `file:xxx.ico`）：

| agent_type | display_name | 包管理器 | run_cmd_template | output_parser | session_id_source | session_id_field | resume_arg_template | skills_dir | skill_entry_file | skill_display_mode | color | icon | sort_order |
|-----------|-------------|---------|-----------------|--------------|-----------------|-----------------|-------------------|-----------|----------------|---------------------|-------|------|-----------|
| claude | Claude Code | npm | `claude -p --output-format stream-json --verbose --dangerously-skip-permissions {message}` | json-stream | stdout-json | session_id | `--resume {session_id}` | `~/.claude/skills/`（显式） | SKILL.md | collection | #3B82F6 | `file:claude_icon.ico` | 1 |
| hermes | Hermes Agent | pip | `hermes chat -q {message} -Q` | ansi-text | stderr-text | (前缀匹配) | `--resume {session_id}` | `~/AppData/Local/hermes/skills/`（显式） | SKILL.md | collection | #8B5CF6 | `file:hermes_icon.ico` | 2 |
| codex | Codex CLI | npm | `codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox {message}` | json-stream | stdout-json | thread_id | `exec resume {session_id}` | `~/.codex/skills/`（显式） | SKILL.md | collection | #F59E0B | `file:codex_icon.ico` | 3 |

**交付物**：
- `agents` 表创建成功，种子数据可查询
- `list_agents` 返回预置三条记录
- 现有功能不受影响（后端仍使用 `AGENTS` 常量，逐步切换）

### Phase 2 -- 安装/检测/更新泛化（后端重构）

**目标**：将三个独立安装命令合并为一个泛化命令，环境检测和版本查询由元信息驱动。

**状态**：:white_check_mark: **已完成**（2026-06-19）

| 任务 | 涉及文件 | 说明 | 状态 |
|------|---------|------|------|
| 移除 `AGENTS` 常量 | `commands/env.rs` | 改为从 DB 读取 | :white_check_mark: |
| 泛化 `install_agent(agent_id)` 命令 | `commands/env.rs` | 从 DB 读取 `install_cmd` 并直接执行 `cmd /C` | :white_check_mark: |
| 泛化 `uninstall_agent(agent_id)` 命令 | `commands/env.rs` | 从 DB 读取 `uninstall_cmd` 并直接执行 | :white_check_mark: |
| 泛化 `check_agent_update(agent_id)` 命令 | `commands/update.rs` | 从 DB 读取 `latest_version_cmd` 并直接执行，不区分命令类型，保证通用性和可拓展性 | :white_check_mark: |
| 移除 `install_claude_code` / `install_hermes` / `install_codex` | `commands/env.rs` + `lib.rs` | 合并为 1 个泛化命令 | :white_check_mark: |
| `detect_env` 改为循环 `enabled` agents | `commands/env.rs` | 从 DB 读取 enabled agents，循环执行各 agent 的 `version_cmd` | :white_check_mark: |
| 新增 `run_shell_cmd` 公共函数 | `commands/env.rs` | 供 update.rs 等模块调用 | :white_check_mark: |
| `AgentType` 枚举改为从元信息动态构建 | `agent/mod.rs` | 待 Phase 3 实施（`build_args` 从 `run_cmd_template` 读取） | :hourglass: |

**交付物**：
- `install_agent('claude')` 等效于原 `install_claude_code`
- `detect_env` 自动检测所有 `enabled` 的 Agent
- 新增 Agent 无需修改 Rust 代码即可安装和检测

### Phase 3 -- 前端元信息驱动（前端重构）

**目标**：前端从后端动态获取 Agent 元信息，消除所有硬编码条件判断。

**工作量**：约 12-15 小时

| 任务 | 涉及文件 | 说明 |
|------|---------|------|
| 新增 `useAgentRegistry` hook | `hooks/useAgentRegistry.ts`（新建） | 封装 `list_agents` 调用 |
| `AGENT_THEMES` 替换为动态数据 | `types/index.ts` | 保留兼容层 |
| `EnvManager` 改为循环渲染 | `components/env/EnvManager.tsx` | 安装/检测/更新由元信息驱动 |
| `SessionList` 新建会话下拉动态生成 | `components/layout/SessionList.tsx` | |
| 消除所有 `agentType === 'claude'` 条件判断 | 各组件 ~20 处 | 改为元信息字段判断 |

**交付物**：
- 前端无硬编码 Agent 类型字符串
- 新增 Agent 自动出现在新建会话下拉和 EnvManager 中
- 现有 UI 行为完全一致

### Phase 4 -- 用户管理界面

**目标**：提供用户可操作的 Agent 管理界面。

**状态**：:white_check_mark: **已完成**（2026-06-19，UI/UX 打磨于 2026-06-20 完成）

| 任务 | 涉及文件 | 说明 | 状态 |
|------|---------|------|------|
| 设置页新增 "Agent 集成配置" tab | `pages/SettingsPage.tsx` | 新增 'agents' tab，Bot 图标 | :white_check_mark: |
| Agent 列表（启用/禁用、排序） | `components/env/AgentManager.tsx` | 列表渲染 + 启用禁用切换 | :white_check_mark: |
| 添加自定义 Agent 表单（6 分类） | `components/env/AgentManager.tsx` | AgentForm 组件，6 个分类区域 + 图标标题 | :white_check_mark: |
| 编辑 Agent 配置（统一表单） | `components/env/AgentManager.tsx` | 与添加表单共用 AgentForm 组件，mode='edit' 区分 | :white_check_mark: |
| 删除自定义 Agent（二次确认弹窗） | `components/env/AgentManager.tsx` | 自定义 Modal，预置 Agent 不可删除 | :white_check_mark: |
| 导入/导出 Agent 配置（JSON） | `commands/agents.rs` + `AgentManager.tsx` | export_agents_json / import_agents_json 命令 + 前端按钮 | :white_check_mark: |
| Tab 重命名 | `pages/SettingsPage.tsx` | "Agent 管理" -> "Agent 集成配置"，"API 配置" -> "API 集成配置" | :white_check_mark: |
| 布局调整 | `AgentManager.tsx` | 添加按钮靠右与导出/导入同行，表单在按钮下方列表上方 | :white_check_mark: |
| 按钮样式 | `AgentManager.tsx` | 启用按钮主题色、悬停提示、删除 danger 样式 | :white_check_mark: |
| StatusBar 数据源修复 | `StatusBar.tsx` | 以 DB agents 为数据源，按 sort_order 排序 | :white_check_mark: |
| 枚举字段下拉化 | `AgentManager.tsx` | outputParser/sessionIdSource/skillDisplayMode 改为 SelectField | :white_check_mark: |
| Placeholder 优化 | `AgentManager.tsx` | 所有输入框 placeholder 改为具体示例 | :white_check_mark: |
| 图标字段值类型说明 | `AgentManager.tsx` | "图标（支持 Emoji / 图片 URL）" | :white_check_mark: |

**交付物**：
- 用户可在设置页管理 Agent
- 新增 Agent 无需任何代码修改
- 统一的表单体验（添加/编辑共用代码）
- 完整的 UI/UX 打磨

### Phase 5 -- 进程协议抽象（可选）

**目标**：支持更多 CLI 交互模式，降低新增 Agent 的进程管理门槛。

**状态**：:white_check_mark: **已完成**（2026-06-19）

| 任务 | 涉及文件 | 说明 | 状态 |
|------|---------|------|------|
| 定义 `ProcessHandler` trait | `agent/handler.rs`（新建） | stdio 模式实现，支持 json-stream/ansi-text/raw-text 三种解析器 | :white_check_mark: |
| `build_args` 从元信息读取 | `agent/handler.rs` | 从 `run_cmd_template` 读取，`{message}` 和 `{session_id}` 占位符替换 | :white_check_mark: |
| `parse_output_line` 从元信息读取 | `agent/handler.rs` | 从 `output_parser` 字段读取，支持 `output_filter_regex` 过滤 | :white_check_mark: |
| `extract_session_id` 从元信息读取 | `agent/handler.rs` | 从 `session_id_source`/`session_id_event_type`/`session_id_field` 读取 | :white_check_mark: |
| `send_message_with_config` 方法 | `agent/mod.rs` | 使用 StdioHandler 驱动进程交互 | :white_check_mark: |
| 技能管理元数据化：`list_skills`/`scan_skills_dir` 从 DB 读取配置 | `agent/mod.rs` | 移除 `claude_skills`/`hermes_skills`/`codex_skills` 硬编码函数 | :white_check_mark: |
| 技能字段前端表单支持 | `components/env/AgentManager.tsx` | 编辑/添加表单新增 `skills_dir`/`skill_entry_file`/`skill_display_mode` | :white_check_mark: |
| `SkillBrowser` 动态数据源 | `components/panels/SkillBrowser.tsx` | 使用 `useAgentRegistry` 动态数据，移除 `AGENT_LABELS` 硬编码 | :white_check_mark: |
| `agent_send_message_with_config` 命令 | `lib.rs` | 从 DB 加载 AgentConfig 并调用新方法 | :white_check_mark: |
| Agent 市场：从远程源下载配置 | `commands/agents.rs` + `AgentManager.tsx` | list_agent_market 命令 + 内置模板 + 前端一键安装 | :white_check_mark: |

### Phase 6 -- 资源目录与图标系统

**目标**：建立统一的资源目录架构，实现 Agent 图标系统。

**状态**：:white_check_mark: **已完成**（2026-06-20）

| 任务 | 涉及文件 | 说明 | 状态 |
|------|---------|------|------|
| 新增 `resources/` 目录结构 | `src-tauri/resources/{agents,icons,assets}/` | 内置资源目录 | :white_check_mark: |
| `ResourcePaths` 结构体 | `lib.rs` | builtin + user 双路径管理 | :white_check_mark: |
| `builtin_resources_dir()` / `user_resources_dir()` | `utils/paths.rs` | 路径工具函数 | :white_check_mark: |
| 用户资源目录初始化 | `lib.rs` setup 钩子 | 首次运行时创建 agents/icons/assets 子目录 | :white_check_mark: |
| `bundle.resources` 配置 | `tauri.conf.json` | 确保图标文件随安装包分发 | :white_check_mark: |
| 内置 Agent 图标文件 | `resources/icons/{claude,hermes,codex}_icon.ico` | 3 个 .ico 文件 | :white_check_mark: |
| `read_agent_icon` 命令 | `commands/agents.rs` | 读取图标文件返回 base64 data URL | :white_check_mark: |
| `AgentIcon` 组件 | `components/common/AgentIcon.tsx` | 三种图标来源渲染 | :white_check_mark: |
| `AgentBadge` 集成 | `components/common/AgentBadge.tsx` | 使用 AgentIcon 渲染 | :white_check_mark: |
| `AGENT_THEMES` 添加 icon 字段 | `types/index.ts` | 主题条目包含 icon | :white_check_mark: |
| 种子数据 icon 字段更新 | `db/init.rs` | `\U0001F916` -> `file:xxx.ico` | :white_check_mark: |
| 种子数据技能字段补全 | `db/init.rs` | 新增 `skills_dir`/`skill_display_mode` 字段，默认 `collection` | :white_check_mark: |
| Migration v15 | `db/init.rs` | 所有内置 Agent 显式设置 skills_dir + uninstall_cmd | :white_check_mark: |
| Tilde 扩展支持 | `agent/mod.rs` | `skillsDir` 中 `~` 展开为 home 目录 | :white_check_mark: |
| 后端默认值更新 | `commands/agents.rs` | `skill_display_mode` 默认 `collection` | :white_check_mark: |
| 前端默认值更新 | `AgentManager.tsx` | 表单默认 `collection` | :white_check_mark: |
| 市场模板 icon 字段更新 | `commands/agents.rs` | `\U0001F916` -> `file:xxx.ico` | :white_check_mark: |
| Migration v16 | `db/init.rs` | 修复 claude 技能配置遗留数据（skills_dir/skill_display_mode） | :white_check_mark: |
| 导入隐藏删除功能 | `commands/agents.rs` | `__action` 字段支持删除任意 Agent（含内置） | :white_check_mark: |

**交付物**：
- 内置 Agent 显示对应图标（Claude 蓝、Hermes 紫、Codex 琥珀）
- 自定义 Agent 支持 Emoji / 网络图片 / 内置图标文件
- 资源目录架构支持后续扩展

---

## 十、性能影响分析

### 10.1 查询路径对比

| 场景 | 当前 | 改造后 | 影响 |
|------|------|--------|------|
| Agent 元信息读取 | 编译期常量，O(1) | 首次 SQLite 查询 ~1ms，后续缓存命中 0ms | **可忽略** |
| 环境检测 | 循环 3 次，硬编码 | 从 DB 读 enabled agents -> 循环 N 次（N <= 10） | **可忽略** |
| 新建会话下拉 | 硬编码 4 项 | 从缓存读取 | **无差异** |
| 消息气泡主题色 | 编译期 switch | 从缓存 Map 查找 | **无差异** |
| 安装 Agent | 3 个独立命令 | 1 个泛化命令 + DB 查询元信息 ~0.5ms | **可忽略** |
| 发送消息 | 编译期枚举匹配 | 从缓存读取 cli_command/build_args | **无差异** |
| 图标读取 | N/A | 首次 `read_agent_icon` ~1ms 磁盘读取 + base64 编码，后续浏览器缓存 | **可忽略** |
| 内存占用 | 0 | 缓存 ~2KB（10 条配置）+ 图标 base64 ~100KB | **可忽略** |
| 启动耗时 | 0 | 首次查询 agents 表 ~1-2ms | **可忽略** |

### 10.2 瓶颈不在元信息查询

PilotDesk 当前的性能瓶颈在于：
1. **Agent 子进程管理**（启动/通信/销毁）-- 毫秒到秒级
2. **消息流式渲染**（Markdown 解析 + Virtuoso 虚拟滚动）
3. **SQLite 写入**（消息持久化）

Agent 元信息读取的开销（~1ms 首次，0ms 缓存命中）完全在噪声范围内。

### 10.3 代码量变化预估

| 指标 | 当前 | 改造后 | 变化 |
|------|------|--------|------|
| Rust 后端 | ~200 行硬编码 + 推导逻辑 | ~100 行命令执行器 + ~50 行资源路径管理 | -50 行 |
| 前端组件 | ~500 行条件判断 | ~200 行循环渲染 + ~200 行 AgentForm 组件 | -100 行 |
| 新增 Agent | 修改 20+ 处 | 填写表单或导入 JSON | **大幅简化** |

**净效果**：代码量减少，可维护性显著提升。

---

## 十一、风险与缓解措施

| 风险 | 等级 | 缓解措施 |
|------|------|---------|
| 现有数据库已有 CHECK 约束 | 中 | 迁移脚本重建表，数据无损 |
| 不同 Agent 的进程管理差异大 | 高 | Phase 1-3 仅覆盖 stdio 模式，http/sse 留到 Phase 5 |
| 用户误配置导致 Agent 不可用 | 中 | 输入校验 + 预置 Agent 不可删除 + 恢复默认功能 |
| 动态类型导致 TypeScript 类型安全降低 | 中 | 运行时校验 + Zod schema 验证 |
| 升级时预置 Agent 配置覆盖用户修改 | 低 | upsert 策略：新增字段写入，已有字段保留用户值 |
| 内置图标文件丢失或损坏 | 低 | AgentIcon 组件有 fallback 机制（加载失败显示占位块，Emoji 回退） |
| 用户资源目录创建失败 | 低 | setup 钩子中 try-catch，失败不影响主流程 |

---

## 十二、附录

### 12.1 当前硬编码引用清单

| 文件 | 硬编码内容 | 改造方式 |
|------|-----------|---------|
| `src-tauri/src/commands/env.rs` | `AGENTS` 常量 + 安装/检测硬编码 | 改为从 DB 加载命令并执行 |
| `src-tauri/src/commands/env.rs` | `install_claude_code`/`install_hermes`/`install_codex` | 合并为 `install_agent(id)` |
| `src-tauri/src/commands/update.rs` | `check_single_npm`/`check_single_pypi` | 已移除，统一使用 `check_agent_update(id)` 直接执行命令 |
| `src-tauri/src/agent/mod.rs` | `AgentType` 枚举 + `build_args` + `parse_output_line` + 硬编码技能函数 | **已移除**：废弃 `AgentType` 枚举和旧 `send_message`，`list_skills`/`scan_skills_dir` 从 DB 读取配置 |
| `src-tauri/src/db/init.rs` | CHECK 约束 + `version_flag` 列 | 已移除；Migration v11 新增技能字段，移除 `version_flag` |
| `src-tauri/src/lib.rs` | 3 个安装命令 + 2 个版本查询命令注册 | 替换为 2-3 个泛化命令 |
| `src/types/index.ts` | `AGENT_THEMES` 对象 + `versionFlag` | 从后端动态获取；`versionFlag` 已移除；`AGENT_THEMES` 新增 `icon` 字段 |
| `src/components/env/EnvManager.tsx` | 每个 agent 独立安装/检测/更新逻辑 | 循环渲染 |
| `src/components/layout/SessionList.tsx` | 硬编码下拉选项 | 动态生成 |
| `src/hooks/useAgentEvent.ts` | agent 类型分发 | 元信息驱动 |
| 各组件 ~20 处 | `agentType === 'claude'` 等 | 改为元信息字段判断 |

### 12.2 会话延续实施总结

**已实施的会话延续能力**（2026-06-19）：

| Agent | session_id 来源 | 提取方式 | 恢复参数 | 状态 |
|-------|----------------|---------|---------|------|
| Claude Code | stdout JSON `system/init` -> `session_id` | 解析 JSON 事件 | `--resume <uuid>` | :white_check_mark: 已验证 |
| Hermes Agent | stderr `session_id: xxxxx` | 文本行前缀匹配 | `--resume <id>` | :white_check_mark: 已验证 |
| Codex CLI | stdout JSONL `thread.started` -> `thread_id` | 解析 JSON 事件 | `exec resume <uuid>` | :white_check_mark: 已验证 |

**统一数据流**：
1. 首次消息 -> `build_args(agent_session_id=None)` -> agent 创建新会话
2. stdout/stderr 解析 -> 提取 session_id -> 发射 `agent-session` 事件
3. 前端 `onSession` -> `update_session_agent_id` -> 数据库持久化
4. 后续消息 -> `build_args(agent_session_id=xxx)` -> `--resume`/`exec resume` 延续上下文

### 12.3 资源目录实施总结

**已实施的资源目录架构**（2026-06-20）：

| 组件 | 文件 | 说明 |
|------|------|------|
| 内置资源目录 | `src-tauri/resources/` | 打包携带，只读 |
| 用户资源目录 | `%APPDATA%/PilotDesk/resources/` | 可读写，首次运行时创建子目录 |
| ResourcePaths | `lib.rs` | Tauri State，setup 钩子初始化 |
| 路径工具函数 | `utils/paths.rs` | `builtin_resources_dir()` / `user_resources_dir()` |
| 打包配置 | `tauri.conf.json` | `bundle.resources` 确保图标分发 |

### 12.4 图标系统实施总结

**已实施的 Agent 图标系统**（2026-06-20）：

| 组件 | 文件 | 说明 |
|------|------|------|
| 图标文件 | `resources/icons/{claude,hermes,codex}_icon.ico` | 3 个内置 Agent 图标 |
| Rust 命令 | `commands/agents.rs` `read_agent_icon` | 读取图标返回 base64 data URL |
| 前端组件 | `components/common/AgentIcon.tsx` | 三种图标来源渲染 |
| 集成 | `AgentBadge` / `AGENT_THEMES` / 种子数据 / 市场模板 | 全链路打通 |

### 12.5 表单分类实施总结

**已实施的表单分类设计**（2026-06-20）：

| 分类 | 图标 | 字段数 | 说明 |
|------|------|-------|------|
| 基础信息 | `Info` | 7 | agentType 在 edit 模式只读 |
| 包参数配置 | `Package` | 2 | npm + pip |
| 会话参数配置 | `Terminal` | 3 | 含 SelectField 枚举 |
| 延续会话配置 | `Repeat` | 4 | 含 SelectField 枚举 |
| 生命周期命令配置 | `Activity` | 6 | 6 个命令字段 |
| 技能引用配置 | `BookOpen` | 3 | 含 SelectField 枚举 |

### 12.6 术语表

| 术语 | 说明 |
|------|------|
| Agent | AI 编程助手，如 Claude Code、Hermes Agent、codeX |
| 预置 Agent | PilotDesk 内置的 Agent，不可删除但可禁用 |
| 自定义 Agent | 用户自行添加的 Agent |
| 元信息 | Agent 的静态描述性信息，存储在 `agents` 表中 |
| 运行时状态 | Agent 的版本信息，由检测逻辑实时获取 |
| stdio 模式 | 通过 stdin/stdout 与子进程交互的通信模式 |
| 命令模板 | 存储在数据库中的 shell 命令字符串，`run_cmd_template` 支持 `{message}` 占位符 |
| 输出解析器 | 预定义的输出解析策略（`json-stream` / `ansi-text` / `raw-text`），用于将 CLI 输出转换为结构化消息 |
| 会话延续 | 后续消息能引用前文上下文的能力 |
| 内置资源 | 打包在安装包中的只读资源（图标、默认配置等） |
| 用户资源 | 存储在用户数据目录的可读写资源（自定义图标、上传文件等） |
| agent_session_id | Agent 侧生成的会话 ID，与 PilotDesk 的 session.id 不同 |
| JSON stream | Claude Code 的流式 JSON 输出格式 |
| JSONL | Codex 的 JSON 行格式，每行一个 JSON 对象 |
