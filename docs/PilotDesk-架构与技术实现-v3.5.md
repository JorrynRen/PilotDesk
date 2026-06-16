# PilotDesk 架构与技术实现

> **项目**: PilotDesk | **架构**: Tauri 2.0 + React 19 + TypeScript + Rust + SQLite + Node.js Sidecar
> **版本**: v3.5 | **日期**: 2026-06-16 | **状态**: 定稿
> **代码仓库**: `E:\WorkSpace_HermesAgent\pilotdeskProject\pilotdesk`

---

## 目录

1. [项目概述](#1-项目概述)
2. [整体架构](#2-整体架构)
3. [前端架构](#3-前端架构)
4. [Rust 后端](#4-rust-后端)
5. [Sidecar 层](#5-sidecar-层)
6. [数据库设计](#6-数据库设计)
7. [Agent 集成](#7-agent-集成)
8. [状态管理](#8-状态管理)
9. [通信协议](#9-通信协议)
10. [环境检测与版本管理](#10-环境检测与版本管理)
11. [关键问题记录](#11-关键问题记录)
12. [文件清单](#12-文件清单)

---

## 1. 项目概述

### 1.1 定位

PilotDesk 是一个 **Agent 统一桌面客户端**，将多个 AI Agent（Claude Code、Hermes Agent、CodeX）集成到单一桌面应用中，提供统一的会话管理、消息交互和环境管理体验。

### 1.2 技术栈

| 层 | 技术 | 说明 |
|---|------|------|
| 桌面框架 | Tauri 2.0 | Rust 驱动的跨平台桌面框架 |
| 前端 | React 19 + TypeScript | 用户界面 |
| 构建工具 | Vite + Rolldown | 前端构建 |
| 样式 | Tailwind CSS 4 | 原子化 CSS |
| 状态管理 | Zustand | 轻量级状态管理 |
| Rust 后端 | Tauri Commands | 数据库、文件系统、进程管理 |
| 数据库 | SQLite (rusqlite) | 本地持久化 |
| Sidecar | Node.js + ws | Agent 通信中间层 |
| Agent CLI | claude / hermes / codex | AI Agent 命令行工具 |

### 1.3 核心功能

- **多 Agent 会话管理**：创建、切换、归档、删除会话
- **统一消息交互**：统一的输入/输出界面，支持流式响应
- **环境检测与管理**：检测本地 Agent 安装状态、版本号
- **Agent 安装向导**：一键安装/更新 Claude Code、Hermes、CodeX
- **API 直连模式**：支持 OpenAI 兼容 API 的直接调用
- **灵感市集**：灵感/提示词管理与搜索（FTS5 全文搜索）
- **技能系统**：Agent 技能列表与调用
- **Bot 频道**：Agent Bot 频道配置管理

---

## 2. 整体架构

### 2.1 三层架构

```
+------------------------------------------------------+
|                    PilotDesk App                       |
|  +------------------------------------------------+  |
|  |              前端 (React + TypeScript)           |  |
|  |  +---------+ +----------+ +----------------+  |  |
|  |  | 会话管理 | | 消息交互  | | 环境管理/设置   |  |  |
|  |  +----+----+ +----+-----+ +-------+--------+  |  |
|  |       |           |               |            |  |
|  |  +----v-----------+---------------v--------+  |  |
|  |  |           Zustand Stores                |  |  |
|  |  +----------------+-----------------------+  |  |
|  +-------------------+--------------------------+  |
|                      |                              |
|  +-------------------+--------------------------+  |
|  |     Tauri IPC (invoke)    |  WebSocket        |  |
|  |                          |  (ws://127.0.0.1:  |  |
|  |                          |   19830)           |  |
|  +-------------------+--------------------------+  |
|                      |                              |
|  +-------------------+--------------------------+  |
|  |    Rust 后端       |    Node.js Sidecar       |  |
|  |  (Tauri Commands)  |  (WebSocket Server)     |  |
|  |                    |                          |  |
|  |  +--------------+  |  +------------------+   |  |
|  |  | SQLite 数据库  |  |  | Claude Adapter   |   |  |
|  |  | (rusqlite)    |  |  | Hermes Adapter   |   |  |
|  |  +--------------+  |  | CodeX Adapter    |   |  |
|  |                    |  +------------------+   |  |
|  |  +--------------+  |        | spawn           |  |
|  |  | Sidecar 管理器 |  |        v                |  |
|  |  | (进程生命周期) |  |  +------------------+   |  |
|  |  +--------------+  |  |  claude / hermes  |   |  |
|  |                    |  |  / codex CLI      |   |  |
|  |  +--------------+  |  +------------------+   |  |
|  |  | 环境检测/版本  |  |                          |  |
|  |  +--------------+  |                          |  |
|  +-------------------+--------------------------+  |
+----------------------+-----------------------------+
                       |
              +--------v--------+
              |   Agent CLI     |
              |  (本地安装)      |
              +-----------------+
```

### 2.2 数据流

#### 2.2.1 会话创建

```
用户点击"新建会话"
  -> SessionList.tsx
    -> sessionStore.createSession(agentType)
      -> invoke('create_session', { agentType })
        -> Rust: INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at)
        -> 返回 Session 对象（含默认标题、时间戳）
    -> 如果是 Agent 类型（非 api）：
      -> useWebSocket.createAgentSession(sessionId, agentType)
        -> WebSocket send: { type: 'session:create', sessionId, agentType }
          -> Sidecar: adapter.createSession(sessionId)
```

#### 2.2.2 消息发送（Agent 模式）

```
用户输入消息 -> InputBar.tsx
  -> MainPanel.handleSend(message, mode)
    -> addMessage(userMsg) -> sessionStore
    -> sendChat(sessionId, message, mode, agentType, cwd, systemPrompt)
      -> WebSocket send: { type: 'chat', sessionId, message, mode, agentType, systemPrompt }
        -> Sidecar: adapter.sendMessage(request)
          -> spawn Agent CLI (shell: true)
          -> stdout pipe -> yield chunks
          -> WebSocket send: { type: 'chunk', content }
    -> onChunk 回调 -> setStreamingContent -> MessageList 实时渲染
    -> onDone 回调 -> addMessage(assistantMsg) -> sessionStore
    -> invoke('save_message', { sessionId, role: 'assistant', content, mode })
      -> Rust: INSERT INTO messages
      -> Rust: UPDATE sessions SET last_message_preview, message_count, updated_at
```

#### 2.2.3 消息发送（API 直连模式）

```
用户输入消息 -> InputBar.tsx
  -> MainPanel.handleSend(message, mode)
    -> sendApiChat(sessionId, message, endpoint, providerId, model, history)
      -> fetch(apiEndpoint) + SSE stream
      -> 逐行解析 SSE -> onChunk 回调
      -> onDone 回调 -> addMessage(assistantMsg)
```

---

## 3. 前端架构

### 3.1 目录结构

```
src/
+-- App.tsx                    # 根组件，主题初始化
+-- main.tsx                   # 入口
+-- components/
|   +-- common/
|   |   +-- AgentBadge.tsx     # Agent 类型徽章（C/H/X/A）
|   +-- env/
|   |   +-- EnvManager.tsx     # 环境检测与 Agent 安装管理
|   |   +-- InstallLog.tsx     # 安装日志
|   +-- input/
|   |   +-- InspirationPicker.tsx  # 灵感选择器
|   |   +-- SkillPicker.tsx       # 技能选择器
|   +-- inspiration/           # 灵感市集相关组件
|   +-- layout/
|   |   +-- MainPanel.tsx      # 主面板（消息列表 + 输入栏）
|   |   +-- InputBar.tsx       # 输入栏（模式选择 + 发送）
|   |   +-- MessageList.tsx    # 消息列表
|   |   +-- SessionList.tsx    # 会话列表
|   |   +-- StatusBar.tsx      # 底部状态栏
|   |   +-- TitleBar.tsx       # 自定义标题栏
|   +-- message/
|   |   +-- MessageBubble.tsx  # 消息气泡
|   |   +-- MarkdownRenderer.tsx # Markdown 渲染
|   +-- panels/
|       +-- SkillBrowser.tsx   # 技能浏览器
|       +-- UpdateChecker.tsx  # 更新检查
|       +-- ModePromptSettings.tsx # 模式提示词设置
+-- hooks/
|   +-- useWebSocket.ts        # WebSocket 连接管理
|   +-- useEnvInfo.ts          # 环境信息共享 hook
|   +-- useTheme.ts            # 主题切换
|   +-- useTauriCommand.ts     # Tauri command 封装
+-- pages/
|   +-- SettingsPage.tsx       # 设置页
|   +-- EnvPage.tsx            # 环境检测页
+-- stores/
|   +-- sessionStore.ts        # 会话/消息状态
|   +-- wsStore.ts             # WebSocket 连接状态
|   +-- skillStore.ts          # 技能列表状态
|   +-- apiProviderStore.ts    # API 提供商状态
|   +-- inspirationStore.ts    # 灵感状态
|   +-- pendingInputStore.ts   # 待发送输入状态
+-- types/
|   +-- index.ts               # 类型定义 + AGENT_THEMES
+-- utils/
|   +-- apiClient.ts           # API 调用工具（SSE 解析）
|   +-- toast.ts               # 通知提示
+-- styles/
    +-- globals.css            # 全局样式 + CSS 变量
```

### 3.2 组件层级

```
App
+-- TitleBar                    # 自定义标题栏（无边框窗口拖拽区域）
+-- SessionList (左侧面板)      # 会话列表
|   +-- SessionListItem x N     # 单个会话项（标题、Agent 类型徽章、预览、时间）
+-- MainPanel (主区域)          # 主内容区
|   +-- MessageList             # 消息列表（自动滚动到底部）
|   |   +-- MessageBubble x N   # 消息气泡（角色标识、内容、时间、推理内容）
|   +-- InputBar                # 输入栏
|       +-- 模式选择器          # native / fast / think / expert
|       +-- 灵感/技能按钮       # 快捷插入灵感或技能
|       +-- 文本输入框 + 发送按钮（支持 Enter 发送）
+-- RightPanel (右侧面板)       # 右侧辅助面板
|   +-- InspirationPanel        # 灵感市集（搜索、标签筛选、收藏）
|   +-- SkillBrowser            # 技能浏览器（按 Agent 类型分组）
+-- StatusBar (底部)            # 状态栏
|   +-- Sidecar 连接状态        # 已连接 / 未连接（圆点指示器）
|   +-- Agent 安装状态          # 查询中 / C v2.1.177 / H (未安装) / X v0.140.0
+-- SettingsPage (独立路由)     # 设置页
    +-- API 提供商配置          # 增删改 API Provider
    +-- Agent 配置              # Agent 相关设置
    +-- 环境检测                # 环境检测详情（各组件版本号）
```

### 3.3 AGENT_THEMES 统一颜色体系

集中定义在 `src/types/index.ts` 中，所有组件统一引用：

```typescript
export const AGENT_THEMES: Record<string, AgentTheme> = {
  claude: { color: '#3B82F6', bg: 'rgba(59,130,246,0.15)', label: 'Claude Code', initial: 'C', cssVar: 'var(--claude-tag)' },
  hermes: { color: '#8B5CF6', bg: 'rgba(139,92,246,0.15)', label: 'Hermes Agent', initial: 'H', cssVar: 'var(--hermes-tag)' },
  api:    { color: '#10B981', bg: 'rgba(16,185,129,0.15)', label: 'API 直连', initial: 'A', cssVar: 'var(--api-tag)' },
  codex:  { color: '#F59E0B', bg: 'rgba(245,158,11,0.15)', label: 'codeX', initial: 'X', cssVar: 'var(--codex-tag)' },
  manual: { color: '#6B7280', bg: 'rgba(107,114,128,0.15)', label: '手动', initial: 'M', cssVar: 'var(--text-tertiary)' },
};
```

CSS 变量定义在 `globals.css` 中，`:root` 和 `[data-theme="dark"]` 各一份。

---

## 4. Rust 后端

### 4.1 目录结构

```
src-tauri/src/
+-- main.rs                    # Tauri 入口
+-- lib.rs                     # 模块注册 + 命令注册 + Sidecar 启动
+-- commands/                  # Tauri Commands
|   +-- mod.rs
|   +-- session.rs             # 会话 CRUD（create/list/get/rename/archive/delete）
|   +-- api_provider.rs        # API 提供商管理（list/get/upsert/delete/reorder）
|   +-- app_settings.rs        # 应用设置 KV（get/set）
|   +-- env.rs                 # 环境检测 + Agent 安装（detect_env/install_*）
|   +-- update.rs              # npm/pypi 版本检测
|   +-- install_log.rs         # 安装日志（insert/list/clear）
|   +-- inspiration.rs         # 灵感 CRUD（list/get/create/update/delete/search）
|   +-- bot.rs                 # Bot 频道管理（list/save/delete）
|   +-- theme.rs               # 主题管理（get/set）
+-- db/
|   +-- mod.rs
|   +-- init.rs                # 数据库初始化 + 迁移（6 个迁移函数）
|   +-- models.rs              # 数据模型（Session, Message, Inspiration, BotChannel, EnvInfo）
+-- sidecar/
|   +-- mod.rs
|   +-- manager.rs             # Sidecar 进程管理（启动/监控/重启/停止）
+-- utils/
    +-- mod.rs
    +-- paths.rs               # 路径工具（db_path, app_data_dir）
    +-- errors.rs              # 错误类型枚举（AppError, 8 个变体）
```

### 4.2 命令注册

所有 Tauri Commands 在 `lib.rs` 的 `invoke_handler` 中注册：

```rust
invoke_handler(tauri::generate_handler![
    // 环境检测
    commands::env::detect_env,
    commands::env::install_claude_code,
    commands::env::install_hermes,
    commands::env::install_codex,

    // 安装日志
    commands::install_log::insert_log,
    commands::install_log::list_logs,
    commands::install_log::clear_logs,

    // 版本更新检测
    commands::update::check_pilotdesk_update,
    commands::update::check_single_npm,
    commands::update::check_single_pypi,

    // 会话管理
    commands::session::list_sessions,
    commands::session::list_archived_sessions,
    commands::session::create_session,
    commands::session::get_session,
    commands::session::get_session_messages,
    commands::session::rename_session,
    commands::session::archive_session,
    commands::session::delete_session,
    commands::session::save_message,

    // 灵感
    list_inspirations, get_inspiration, create_inspiration,
    update_inspiration, delete_inspiration, search_inspirations, list_tags,

    // Bot 频道
    list_bot_channels, save_bot_channel, delete_bot_channel,

    // API 提供商
    list_api_providers, get_api_provider, upsert_api_provider,
    delete_api_provider, get_api_key, reorder_api_providers,

    // 应用设置
    get_app_setting, set_app_setting,

    // 主题
    get_theme, set_theme_cmd,
])
```

### 4.3 关键命令

| 命令 | 文件 | 参数 | 返回值 | 说明 |
|------|------|------|--------|------|
| `detect_env` | env.rs | 无 | `EnvInfo` | 检测 Node/Git/Python/Agent 版本 |
| `install_claude_code` | env.rs | 无 | `()` | 通过 npm 安装 Claude Code |
| `install_hermes` | env.rs | 无 | `()` | 通过 pip 安装 Hermes Agent |
| `install_codex` | env.rs | 无 | `()` | 通过 npm 安装 CodeX |
| `create_session` | session.rs | agent_type, cwd?, title?, api_provider?, api_model? | `Session` | 创建新会话 |
| `list_sessions` | session.rs | 无 | `Vec<Session>` | 获取活跃会话列表 |
| `get_session` | session.rs | id | `Session` | 获取单个会话 |
| `get_session_messages` | session.rs | session_id | `Vec<Message>` | 获取会话消息列表 |
| `save_message` | session.rs | session_id, role, content, mode | `Message` | 保存消息并更新会话预览 |
| `rename_session` | session.rs | id, title | `()` | 重命名会话 |
| `archive_session` | session.rs | id | `()` | 归档会话 |
| `delete_session` | session.rs | id | `()` | 删除会话（级联删除消息） |
| `check_single_npm` | update.rs | package_name | `Option<String>` | 检查 npm 包最新版本 |
| `check_single_pypi` | update.rs | package_name | `Option<String>` | 检查 PyPI 包最新版本 |
| `get_app_setting` | app_settings.rs | key | `Option<String>` | 获取应用设置 |
| `set_app_setting` | app_settings.rs | key, value | `()` | 设置应用设置 |
| `list_api_providers` | api_provider.rs | 无 | `Vec<ApiProvider>` | 获取 API 提供商列表 |
| `upsert_api_provider` | api_provider.rs | provider | `()` | 创建或更新 API 提供商 |
| `list_inspirations` | inspiration.rs | 无 | `Vec<Inspiration>` | 获取灵感列表 |
| `search_inspirations` | inspiration.rs | query | `Vec<Inspiration>` | FTS5 全文搜索灵感 |
| `list_bot_channels` | bot.rs | 无 | `Vec<BotChannel>` | 获取 Bot 频道列表 |

### 4.4 环境检测（env.rs）

环境检测是 PilotDesk 的核心能力之一，v3.3 版本对 env.rs 进行了全面重构，采用**集中配置表 + 泛化安装 + 动态路径探测**的设计模式。

#### 集中配置表

所有 Agent 的元数据集中在 `AGENTS` 常量表中，消除重复代码：

```rust
pub const AGENTS: &[AgentConfig] = &[
    AgentConfig { id: "claude", name: "Claude Code",  manager: PackageManager::Npm, package: "@anthropic-ai/claude-code" },
    AgentConfig { id: "hermes", name: "Hermes Agent", manager: PackageManager::Pip, package: "hermes-agent" },
    AgentConfig { id: "codex",  name: "codeX",        manager: PackageManager::Npm, package: "@openai/codex" },
];
```

新增 Agent 只需在表中添加一行，无需新增函数或修改检测逻辑。

#### 动态路径探测

`resolve_in_path()` 通过 `where` 命令（Windows）动态解析工具路径，替代硬编码路径：

```rust
fn resolve_in_path(name: &str) -> Option<String> {
    // 1. 尝试 where <name> 获取完整路径
    // 2. 解析输出，取第一个有效路径
    // 3. 缓存结果到 RESOLVED_PATHS 避免重复查询
}
```

| 检测项 | 检测方式 | 路径策略 | 说明 |
|--------|---------|---------|------|
| Node.js 版本 | `node --version` | `where node` -> PATH | 基础运行时 |
| Git 版本 | `git --version` | `where git` -> PATH | 版本控制 |
| Python 版本 | `python --version` | `where python` -> PATH（fallback python3） | Hermes 运行时 |
| Claude Code 版本 | `claude --version` | `where claude` -> PATH | CLI 版本检测 |
| Hermes 版本 | `hermes --version` | `where hermes` -> PATH | CLI 版本检测 |
| CodeX 版本 | `codex --version` | `where codex` -> PATH | CLI 版本检测 |

#### 泛化安装函数

单一 `run_install()` 函数处理所有 Agent 安装，通过 `PackageManager` 枚举区分 npm/pip：

```rust
fn run_install(app: &tauri::AppHandle, manager: PackageManager, package: &str, agent: &str) -> Result<(), AppError> {
    let cmd = match manager {
        PackageManager::Npm => format!("npm install -g {}", package),
        PackageManager::Pip => format!("pip install {}", package),
    };
    // cmd /C 执行 + tauri::Emitter 发送进度事件 + 清除缓存
}
```

三个安装命令函数简化为单行调用：

```rust
pub async fn install_claude_code(app: tauri::AppHandle) -> Result<(), AppError> {
    run_install(&app, PackageManager::Npm, "@anthropic-ai/claude-code", "Claude Code")
}
pub async fn install_hermes(app: tauri::AppHandle) -> Result<(), AppError> {
    run_install(&app, PackageManager::Pip, "hermes-agent", "Hermes Agent")
}
pub async fn install_codex(app: tauri::AppHandle) -> Result<(), AppError> {
    run_install(&app, PackageManager::Npm, "@openai/codex", "codeX")
}
```

#### 版本号提取逻辑

```rust
fn clean_version_string(raw: &str) -> String {
    // 按空格和左括号分割
    // 优先级 1: "v" + 数字开头（如 "v0.16.0"）
    // 优先级 2: 纯数字开头（如 "2.1.177", "0.140.0-alpha.2"）
    // 避免匹配日期字符串（如 "2026.6.5"）
    //
    // 处理格式:
    //   "2.1.177 (Claude Code)"       -> "2.1.177"
    //   "codex-cli 0.140.0-alpha.2"   -> "0.140.0-alpha.2"
    //   "Hermes Agent v0.16.0 ..."    -> "0.16.0"
}
```

#### 缓存机制

```rust
static LAST_DETECT: std::sync::RwLock<Option<(Instant, EnvInfo)>> = std::sync::RwLock::new(None);
```

- 缓存有效期：30 秒（`CACHE_TTL` 常量）
- 安装/更新完成后主动清除缓存，下次 `detect_env` 重新检测
- 读缓存使用 `RwLock` 实现无阻塞并发读取

### 4.5 泛化安装

v3.4 引入 `AGENTS` 集中配置表和泛化 `install_agent` 命令，所有 Agent 安装统一处理：

```rust
```rust
#[tauri::command]
pub async fn install_agent(app: tauri::AppHandle, agent_type: String) -> Result<(), AppError> {
    let config = AGENTS.iter().find(|a| a.agent_type == agent_type)
        .ok_or_else(|| AppError::InvalidInput(format!("未知 Agent 类型: {}", agent_type)))?;
    let (manager, package) = match (config.npm_package, config.pip_package) {
        (Some(pkg), _) => (PackageManager::Npm, pkg),
        (_, Some(pkg)) => (PackageManager::Pip, pkg),
        _ => return Err(AppError::Config(format!("{} 无可用的包管理器", agent_type))),
    };
    run_install(&app, manager, package, &agent_type)
}
```

旧命令保留为别名（兼容前端调用）：

```rust
pub async fn install_claude_code(app: tauri::AppHandle) -> Result<(), AppError> {
    install_agent(app, "claude".to_string()).await
}
pub async fn install_hermes(app: tauri::AppHandle) -> Result<(), AppError> {
    install_agent(app, "hermes".to_string()).await
}
pub async fn install_codex(app: tauri::AppHandle) -> Result<(), AppError> {
    install_agent(app, "codex".to_string()).await
}
```
```

### 4.6 Agent 集中配置表

| Agent | npm/pip 包名 | 安装命令 |
|-------|-------------|---------|
| Claude Code | `@anthropic-ai/claude-code` | `npm install -g @anthropic-ai/claude-code` |
| Hermes | `hermes-agent` | `pip install hermes-agent` |
| CodeX | `@openai/codex` | `npm install -g @openai/codex` |
```

### 4.7 错误处理（errors.rs）

v3.3 版本将 `AppError` 从 struct 重构为 Rust 枚举（enum），利用类型系统精确表达错误类别，同时通过自定义 `Serialize` 实现保持与前端的 JSON 兼容。

#### 枚举变体

```rust
#[derive(Debug, Clone)]
pub enum AppError {
    Db(String),            // 数据库操作失败
    Io(String),            // 文件/IO 操作失败
    Lock(String),          // 资源锁定失败（如 Mutex 锁）
    NotFound(String),      // 资源未找到
    InvalidInput(String),  // 输入参数无效
    External(String),      // 外部服务/进程错误
    Config(String),        // 配置错误
    Network(String),       // 网络请求错误
}
```

#### 序列化兼容

自定义 `Serialize` 实现确保前端收到的 JSON 格式与旧版 struct 完全一致：

```json
{
  "code": "ERR_DB",
  "message": "数据库操作失败",
  "details": "UNIQUE constraint failed: sessions.id"
}
```

| 枚举变体 | code | message |
|---------|------|---------|
| `Db(String)` | `ERR_DB` | 数据库操作失败 |
| `Io(String)` | `ERR_IO` | 文件操作失败 |
| `Lock(String)` | `ERR_LOCK` | 资源锁定失败 |
| `NotFound(String)` | `ERR_NOT_FOUND` | 资源未找到 |
| `InvalidInput(String)` | `ERR_INVALID_INPUT` | 输入参数无效 |
| `External(String)` | `ERR_EXTERNAL` | 外部服务错误 |
| `Config(String)` | `ERR_CONFIG` | 配置错误 |
| `Network(String)` | `ERR_NETWORK` | 网络错误 |

#### From 实现

保留 `From<rusqlite::Error>` 和 `From<std::io::Error>` trait 实现，允许通过 `?` 操作符自动转换：

```rust
impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        AppError::Db(err.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Io(err.to_string())
    }
}
```

#### 使用模式

```rust
// 自动转换（? 操作符）
let conn = conn.conn.lock().map_err(|e| AppError::Lock(...))?;
let rows = conn.execute(sql, params)?;  // 自动 From<rusqlite::Error>

// 显式构造
return Err(AppError::NotFound("资源不存在".to_string()));
return Err(AppError::InvalidInput(format!("无效参数: {}", value)));

// 匹配处理
match error {
    AppError::NotFound(msg) => { /* 404 处理 */ }
    AppError::Network(msg) => { /* 重试逻辑 */ }
    _ => { /* 通用错误处理 */ }
}
```

---

## 5. Sidecar 层

### 5.1 架构

Sidecar 是一个独立的 Node.js WebSocket 服务器，作为 PilotDesk 与 Agent CLI 之间的通信中间层。

```
PilotDesk App (前端)
    |
    | WebSocket (ws://127.0.0.1:19830)
    v
Sidecar (Node.js)
    +-- server.ts          # WebSocket 服务器（消息路由、超时控制、错误处理）
    +-- index.ts           # 入口
    +-- types.ts           # 消息类型定义
    +-- adapters/
        +-- base.ts        # AgentAdapter 接口
        +-- claude-code.ts # Claude Code 适配器
        +-- hermes.ts      # Hermes 适配器
        +-- codex.ts       # CodeX 适配器
```

### 5.2 进程管理

Sidecar 进程由 Rust 端的 `SidecarManager` 管理：

```rust
pub struct SidecarManager {
    process: Option<Child>,
    port: u16,
    restart_count: Arc<AtomicU32>,
    is_running: Arc<AtomicBool>,
}
```

**启动流程**：

1. `resolve_sidecar_path()` -- 多策略探测 sidecar/dist/index.js 路径
2. `resolve_node_path()` -- 通过 `where node` 动态探测 Node.js 路径
3. 设置 `PORT` 环境变量 -> Sidecar 监听指定端口
4. `wait_for_port()` -- 轮询端口就绪（最多 10 次 x 200ms）
5. `spawn_watchdog()` -- 后台监控进程，异常退出时自动重启（最多 5 次）

**路径探测策略**（优先级，v3.4 消除硬编码路径）：
1. `CARGO_MANIFEST_DIR` 编译时环境变量
2. exe 路径向上遍历（target/debug/ -> target/ -> src-tauri/ -> project_root）
3. 当前工作目录
4. Tauri resource_dir（生产环境）
5. 回退路径 `../sidecar/dist/index.js`

**Setup 阶段启动**（在 `lib.rs` 的 `.setup()` 回调中）：

```rust
.setup(|app| {
    let app_handle = app.handle().clone();
    let mut sidecar = SidecarManager::new(19830);
    match sidecar.start(app_handle) {
        Ok(port) => log::info!("[Sidecar] WebSocket server started on port {}", port),
        Err(e) => log::warn!("[Sidecar] Failed to start: {} — WebSocket features will be unavailable", e),
    }
    app.manage(std::sync::Mutex::new(sidecar));
})
```

### 5.3 WebSocket 协议

**消息格式**：

```typescript
interface WsMessage {
  type: 'ping' | 'session:create' | 'session:close' | 'chat' | 'stop'
      | 'skills:list' | 'skills:list-all';
  sessionId: string;
  agentType?: string;
  message?: string;
  mode?: string;
  cwd?: string;
  systemPrompt?: string;
}
```

**响应格式**：

```typescript
type ChatChunk =
  | { type: 'chunk', sessionId: string, content: string }
  | { type: 'done', sessionId: string, content: string }
  | { type: 'error', sessionId: string, error: string }
  | { type: 'status', sessionId: string, status: string }
  | { type: 'skills', sessionId: string, agentType: string, skills: SkillInfo[] };
```

### 5.4 Agent 适配器

#### 5.4.1 AgentAdapter 接口

```typescript
interface AgentAdapter {
  agentType: 'claude' | 'hermes' | 'codex';
  createSession(sessionId: string, cwd?: string): Promise<void>;
  closeSession(sessionId: string): Promise<void>;
  sendMessage(request: ChatRequest): AsyncGenerator<string, void, unknown>;
  stopGeneration(sessionId: string): void;
  listSkills(): Promise<SkillInfo[]>;
}
```

#### 5.4.2 Claude Code 适配器

- 使用 `claude` CLI，通过 `shell: true` 模式 spawn
- 命令格式：`claude --print "${escapedMsg}"`
- 设置 `process.env.CLAUDE_CODE_OVERRIDE` 避免版本冲突
- 输出逐行 yield

#### 5.4.3 Hermes 适配器

- 使用 `hermes` CLI，通过 `shell: true` 模式 spawn
- 命令格式：`hermes --no-color --no-stream ${fullMessage}`
- 关键：spawn 时清除 `PYTHONHOME` 环境变量
  ```typescript
  const child = spawn(cmd, [], {
    shell: true,
    env: { ...process.env, PYTHONHOME: undefined },
  });
  ```
- 输出逐行 yield

#### 5.4.4 CodeX 适配器

- 使用 `codex` CLI，通过 `shell: true` 模式 spawn
- 命令格式：`codex exec "${escapedMsg}"`（非交互模式，v3.3 从 `--pipe` 修复为 `exec`）
- 输出逐行 yield
- 异常退出时提取 stderr 前 300 字符作为错误详情

### 5.5 超时机制

Sidecar 端对所有 `chat` 消息应用 120 秒超时：

```typescript
async function* withTimeout<T>(iterable: AsyncGenerator<T>, timeoutMs: number) {
  const iterator = iterable[Symbol.asyncIterator]();
  while (true) {
    const result = await Promise.race([
      iterator.next(),
      new Promise<IteratorResult<T>>((_, reject) =>
        setTimeout(() => reject(new Error(`请求超时：智能体未在 ${timeoutMs / 1000} 秒内响应`)), timeoutMs),
      ),
    ]);
    if (result.done) break;
    yield result.value;
  }
}
```

前端同时维护 60 秒超时作为兜底。

---

## 6. 数据库设计

### 6.1 数据库配置

```sql
PRAGMA journal_mode = WAL;    -- WAL 模式，提升并发读写性能
PRAGMA foreign_keys = ON;     -- 外键约束
```

数据库文件路径：`{app_data_dir}/PilotDesk/pilotdesk.db`

### 6.2 表结构

#### sessions（会话表）

存储所有 Agent 会话记录。

```sql
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    agent_type TEXT NOT NULL CHECK(agent_type IN ('claude', 'hermes', 'api', 'codex')),
    title TEXT NOT NULL DEFAULT '',
    cwd TEXT DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_message_preview TEXT DEFAULT '',
    message_count INTEGER DEFAULT 0,
    status TEXT DEFAULT 'active' CHECK(status IN ('active', 'archived')),
    api_provider TEXT,
    api_model TEXT
);

CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_type, updated_at);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status, updated_at);
```

**字段说明**：

| 字段 | 类型 | 约束 | 默认值 | 说明 |
|------|------|------|--------|------|
| `id` | TEXT | PK | - | UUID v4，创建时生成 |
| `agent_type` | TEXT | NOT NULL, CHECK | - | Agent 类型：claude / hermes / api / codex |
| `title` | TEXT | NOT NULL | `''` | 会话标题，创建时根据 agent_type 自动生成默认标题 |
| `cwd` | TEXT | - | `''` | 工作目录路径，用于 Agent CLI 的工作上下文 |
| `created_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒），创建时设置 |
| `updated_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒），每次消息发送时更新 |
| `last_message_preview` | TEXT | - | `''` | 最后一条消息的前 100 字符（UTF-8 安全截断），用于列表预览 |
| `message_count` | INTEGER | - | 0 | 消息总数，每次 save_message 自增 |
| `status` | TEXT | CHECK | `'active'` | active（活跃）/ archived（归档） |
| `api_provider` | TEXT | - | NULL | 仅 api 类型使用，关联 api_providers.id |
| `api_model` | TEXT | - | NULL | 仅 api 类型使用，指定模型名称 |

**默认标题生成规则**：

| agent_type | 默认标题 |
|-----------|---------|
| claude | `Claude Code 新会话` |
| hermes | `Hermes Agent 新会话` |
| codex | `codeX 新会话` |
| api | `API 直连会话` |
| 其他 | `新会话` |

**Rust 模型**：

```rust
pub struct Session {
    pub id: String,
    pub agent_type: String,
    pub title: String,
    pub cwd: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_message_preview: String,
    pub message_count: i64,
    pub status: String,
    pub api_provider: Option<String>,
    pub api_model: Option<String>,
}
```

---

#### messages（消息表）

存储会话中的每条消息。

```sql
CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
    content TEXT NOT NULL DEFAULT '',
    mode TEXT DEFAULT 'native' CHECK(mode IN ('native', 'fast', 'think', 'expert')),
    timestamp INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);
```

**字段说明**：

| 字段 | 类型 | 约束 | 默认值 | 说明 |
|------|------|------|--------|------|
| `id` | TEXT | PK | - | UUID v4，创建时生成 |
| `session_id` | TEXT | NOT NULL, FK -> sessions(id) ON DELETE CASCADE | - | 所属会话 ID，级联删除 |
| `role` | TEXT | NOT NULL, CHECK | - | user（用户）/ assistant（AI 回复）/ system（系统指令） |
| `content` | TEXT | NOT NULL | `''` | 消息文本内容 |
| `mode` | TEXT | CHECK | `'native'` | native（标准）/ fast（快速）/ think（思考）/ expert（专家） |
| `timestamp` | INTEGER | NOT NULL | - | Unix 时间戳（秒），创建时设置 |

**Rust 模型**（v3.4 添加扩展字段）：

```rust
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,         // 'user' | 'assistant' | 'system'
    pub content: String,
    pub mode: String,         // 'native' | 'fast' | 'think' | 'expert'
    pub timestamp: i64,
    pub reasoning_content: Option<String>,  // 推理/思考内容
    pub tool_calls: Option<String>,         // 工具调用（JSON 数组）
    pub tool_call_id: Option<String>,       // 工具调用 ID
    pub tool_name: Option<String>,          // 工具名称
}
```

**前端 Message 类型**（v3.4 已持久化到 SQLite）：

```typescript
interface Message {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  mode: 'native' | 'fast' | 'think' | 'expert';
  timestamp: number;
  /** 推理/思考内容（如 DeepSeek reasoning_content），已持久化 */
  reasoningContent?: string;
  /** 模型请求的工具调用（JSON 数组字符串），已持久化 */
  toolCalls?: string;
  /** 工具调用 ID（role='tool' 时使用），已持久化 */
  toolCallId?: string;
  /** 工具名称（role='tool' 时使用），已持久化 */
  toolName?: string;
}
```

**消息保存逻辑**（`save_message` 命令）：

```rust
fn save_message(session_id, role, content, mode) -> Message {
    // 1. 生成 UUID v4
    // 2. INSERT INTO messages (id, session_id, role, content, mode, timestamp)
    // 3. 截取 content 前 100 字符作为预览（UTF-8 安全截断）
    // 4. UPDATE sessions SET last_message_preview, message_count + 1, updated_at
}
```

---

#### inspirations（灵感表）

存储灵感/提示词模板。

```sql
CREATE TABLE inspirations (
    id TEXT PRIMARY KEY,
    icon TEXT NOT NULL DEFAULT '💡',
    title TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    source_agent TEXT DEFAULT 'manual' CHECK(source_agent IN ('claude', 'hermes', 'codex', 'manual')),
    is_favorite INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_inspirations_favorite ON inspirations(is_favorite, updated_at);
```

**字段说明**：

| 字段 | 类型 | 约束 | 默认值 | 说明 |
|------|------|------|--------|------|
| `id` | TEXT | PK | - | UUID v4 |
| `icon` | TEXT | NOT NULL | `'💡'` | 图标（Emoji 字符） |
| `title` | TEXT | NOT NULL | - | 灵感标题 |
| `content` | TEXT | NOT NULL | `''` | 提示词模板内容 |
| `source_agent` | TEXT | CHECK | `'manual'` | 来源 Agent：claude / hermes / codex / manual |
| `is_favorite` | INTEGER | - | 0 | 是否收藏：0（否）/ 1（是） |
| `created_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |
| `updated_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |

**Rust 模型**：

```rust
pub struct Inspiration {
    pub id: String,
    pub icon: String,
    pub title: String,
    pub content: String,
    pub source_agent: String,
    pub is_favorite: bool,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
```

---

#### inspiration_tags（灵感标签表）

灵感与标签的多对多关联表。

```sql
CREATE TABLE inspiration_tags (
    inspiration_id TEXT NOT NULL REFERENCES inspirations(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (inspiration_id, tag)
);

CREATE INDEX IF NOT EXISTS idx_inspirations_tags ON inspiration_tags(tag);
```

**字段说明**：

| 字段 | 类型 | 约束 | 说明 |
|------|------|------|------|
| `inspiration_id` | TEXT | NOT NULL, FK -> inspirations(id) ON DELETE CASCADE | 灵感 ID，级联删除 |
| `tag` | TEXT | NOT NULL | 标签名（如 "编程"、"写作"、"分析"） |

**复合主键**：`(inspiration_id, tag)`，确保一个灵感不会重复打同一标签。

---

#### inspirations_fts（灵感全文搜索表）

基于 FTS5 的全文搜索虚拟表，支持 title 和 content 的快速搜索。

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS inspirations_fts USING fts5(
    title, content,
    content=inspirations,
    content_rowid=rowid
);
```

**字段说明**：

| 字段 | 类型 | 说明 |
|------|------|------|
| `title` | TEXT | 外部内容表 `inspirations.title` 的全文索引 |
| `content` | TEXT | 外部内容表 `inspirations.content` 的全文索引 |

> 使用 `content=` 外部内容表模式，FTS 表本身不存储数据，而是引用 `inspirations` 表的 rowid。灵感数据变更时需手动同步 FTS 索引。

---

#### bot_channels（Bot 频道表）

存储 Agent Bot 的频道配置。

```sql
CREATE TABLE bot_channels (
    id TEXT PRIMARY KEY,
    agent_type TEXT NOT NULL CHECK(agent_type IN ('claude', 'hermes', 'codex')),
    platform TEXT NOT NULL DEFAULT 'wechat',
    method TEXT DEFAULT 'clawbot',
    status TEXT DEFAULT 'disconnected',
    trigger_prefix TEXT DEFAULT '',
    response_format TEXT DEFAULT 'markdown',
    config TEXT DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

**字段说明**：

| 字段 | 类型 | 约束 | 默认值 | 说明 |
|------|------|------|--------|------|
| `id` | TEXT | PK | - | UUID v4 |
| `agent_type` | TEXT | NOT NULL, CHECK | - | 关联的 Agent 类型：claude / hermes / codex |
| `platform` | TEXT | NOT NULL | `'wechat'` | 平台名称（如 wechat、dingtalk、feishu） |
| `method` | TEXT | - | `'clawbot'` | 接入方式（如 clawbot、webhook） |
| `status` | TEXT | - | `'disconnected'` | 连接状态：disconnected / connected |
| `trigger_prefix` | TEXT | - | `''` | 触发前缀，只有以此前缀开头的消息才触发 Bot |
| `response_format` | TEXT | - | `'markdown'` | 响应格式（markdown / text / json） |
| `config` | TEXT | - | `'{}'` | 平台特定配置（JSON 字符串） |
| `created_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |
| `updated_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |

**Rust 模型**：

```rust
pub struct BotChannel {
    pub id: String,
    pub agent_type: String,
    pub platform: String,
    pub method: String,
    pub status: String,
    pub trigger_prefix: String,
    pub response_format: String,
    pub config: serde_json::Value,  // JSON 对象
    pub created_at: i64,
    pub updated_at: i64,
}
```

---

#### api_providers（API 提供商表）

存储 OpenAI 兼容 API 的提供商配置。通过迁移从 localStorage 迁移而来。

```sql
CREATE TABLE api_providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    api_endpoint TEXT NOT NULL DEFAULT '',
    api_key TEXT DEFAULT '',
    api_key_masked TEXT DEFAULT '',
    api_key_set INTEGER DEFAULT 0,
    models TEXT NOT NULL DEFAULT '[]',
    sort_order INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

**字段说明**：

| 字段 | 类型 | 约束 | 默认值 | 说明 |
|------|------|------|--------|------|
| `id` | TEXT | PK | - | UUID v4 |
| `name` | TEXT | NOT NULL | `''` | 提供商显示名称（如 "OpenAI"、"DeepSeek"） |
| `api_endpoint` | TEXT | NOT NULL | `''` | API 端点 URL（如 `https://api.openai.com/v1`） |
| `api_key` | TEXT | - | `''` | API Key（明文存储，仅用于请求） |
| `api_key_masked` | TEXT | - | `''` | 掩码后的 API Key（如 `sk-****1234`，用于显示） |
| `api_key_set` | INTEGER | - | 0 | 是否已设置 Key：0（未设置）/ 1（已设置） |
| `models` | TEXT | NOT NULL | `'[]'` | 支持的模型列表（JSON 数组字符串，如 `["gpt-4","gpt-3.5-turbo"]`） |
| `sort_order` | INTEGER | - | 0 | 排序序号，数值越小越靠前 |
| `created_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |
| `updated_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |

---

#### app_settings（应用设置表）

键值对存储的应用设置。通过迁移从 localStorage 迁移而来。

```sql
CREATE TABLE app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL
);
```

**字段说明**：

| 字段 | 类型 | 约束 | 说明 |
|------|------|------|------|
| `key` | TEXT | PK | 设置键名（如 `mode_prompt_native`、`theme`） |
| `value` | TEXT | NOT NULL | 设置值（字符串） |
| `updated_at` | INTEGER | NOT NULL | Unix 时间戳（秒） |

**默认种子数据**（模式提示词，通过 `INSERT OR IGNORE` 在首次启动时写入）：

| key | value | 说明 |
|-----|-------|------|
| `mode_prompt_native` | （空字符串） | 标准模式，无额外提示词 |
| `mode_prompt_fast` | 快速简洁回答，直接给出结论，无需详细解释推理过程 | 快速模式提示词 |
| `mode_prompt_think` | 逐步分析推理，详细解释你的思路和过程，给出完整的推理链 | 思考模式提示词 |
| `mode_prompt_expert` | 以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案 | 专家模式提示词 |

---

#### install_logs（安装日志表）

记录环境检测和 Agent 安装过程中的日志。

```sql
CREATE TABLE install_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    message TEXT NOT NULL DEFAULT '',
    level TEXT NOT NULL DEFAULT 'info' CHECK(level IN ('info', 'warn', 'error', 'success'))
);

CREATE INDEX IF NOT EXISTS idx_install_logs_time ON install_logs(timestamp);
```

**字段说明**：

| 字段 | 类型 | 约束 | 默认值 | 说明 |
|------|------|------|--------|------|
| `id` | INTEGER | PK, AUTOINCREMENT | - | 自增主键 |
| `timestamp` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |
| `message` | TEXT | NOT NULL | `''` | 日志内容（如 "开始安装 Claude Code..."） |
| `level` | TEXT | NOT NULL, CHECK | `'info'` | 日志级别：info（信息）/ warn（警告）/ error（错误）/ success（成功） |

---

### 6.3 实体关系图（ER）

```
sessions 1 ---- * messages              (session_id FK, ON DELETE CASCADE)
inspirations 1 ---- * inspiration_tags  (inspiration_id FK, ON DELETE CASCADE)
inspirations 1 ---- 1 inspirations_fts  (content=inspirations, content_rowid=rowid)
```

各表独立（无外键关联）：
- `api_providers` -- 独立表，sessions.api_provider 逻辑关联但不设 FK
- `app_settings` -- 独立 KV 表
- `install_logs` -- 独立日志表
- `bot_channels` -- 独立配置表

### 6.4 迁移机制

所有迁移在 `init_db()` 中顺序执行，每个迁移函数职责单一：

```rust
pub fn init_db() -> Result<Connection, AppError> {
    // 1. 创建初始表结构（sessions, messages, inspirations, bot_channels, FTS）
    conn.execute_batch("CREATE TABLE IF NOT EXISTS ...")?;

    // 2. 迁移：添加 api_provider / api_model 列到 sessions 表
    migrate_add_api_columns(&conn)?;

    // 3. 迁移：统一检查所有 agent_type 的 CHECK 约束
    migrate_add_type(&conn)?;

    // 4. 迁移：创建 api_providers 表
    migrate_add_api_providers(&conn)?;

    // 5. 迁移：创建 app_settings 表（含种子数据）
    migrate_add_app_settings(&conn)?;

    // 6. 迁移：创建 install_logs 表
    migrate_add_install_logs(&conn)?;

    // 7. 迁移：添加 messages 表扩展字段
    migrate_add_message_extensions(&conn)?;

    Ok(conn)
}
```

#### migrate_add_api_columns

检测 `sessions` 表是否已有 `api_provider` 和 `api_model` 列：

```rust
fn migrate_add_api_columns(conn: &Connection) -> Result<(), AppError> {
    // 尝试 SELECT api_provider FROM sessions LIMIT 0
    // 成功 -> 列已存在，跳过
    // 失败 -> ALTER TABLE ADD COLUMN
}
```

#### migrate_add_type

SQLite 不支持 `ALTER TABLE ... ALTER CONSTRAINT`，通过重建表方式处理 CHECK 约束：

```rust
fn migrate_add_type(conn: &Connection) -> Result<(), AppError> {
    // 1. 尝试插入测试行 ('api') 和 ('codex')
    // 2. 两者都接受 -> 跳过（表已最新）
    // 3. 任一不接受 -> 重建表：
    //    - CREATE TABLE sessions_new（含完整 CHECK 约束）
    //    - INSERT OR IGNORE INTO sessions_new SELECT ... FROM sessions
    //    - DROP TABLE sessions
    //    - ALTER TABLE sessions_new RENAME TO sessions
    //    - 重建索引
}
```

#### migrate_add_api_providers

```rust
fn migrate_add_api_providers(conn: &Connection) -> Result<(), AppError> {
    // CREATE TABLE IF NOT EXISTS api_providers (...)
}
```

#### migrate_add_app_settings

```rust
fn migrate_add_app_settings(conn: &Connection) -> Result<(), AppError> {
    // 1. CREATE TABLE IF NOT EXISTS app_settings (...)
    // 2. INSERT OR IGNORE 4 条默认模式提示词种子数据
}
```

#### migrate_add_install_logs

```rust
fn migrate_add_install_logs(conn: &Connection) -> Result<(), AppError> {
    // CREATE TABLE IF NOT EXISTS install_logs (...)
    // CREATE INDEX IF NOT EXISTS idx_install_logs_time
}
```

---

## 7. Agent 集成

### 7.1 支持的 Agent 类型

| Agent | 类型标识 | 安装方式 | CLI 命令 | 通信方式 | 状态 |
|-------|---------|---------|---------|---------|------|
| Claude Code | `claude` | npm | `claude` | Sidecar WebSocket | 已支持 |
| Hermes Agent | `hermes` | pip | `hermes` | Sidecar WebSocket | 已支持 |
| CodeX | `codex` | npm | `codex` | Sidecar WebSocket | 已支持 |
| API 直连 | `api` | 无 | 无 | 前端 fetch SSE | 已支持 |

### 7.2 会话创建流程

```
SessionList.tsx
  -> sessionStore.createSession(agentType, cwd?, title?, apiProvider?, apiModel?)
    -> invoke('create_session', { agentType, cwd, title, apiProvider, apiModel })
      -> Rust: 生成 UUID v4
      -> Rust: 根据 agentType 生成默认标题
      -> Rust: INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at, api_provider, api_model)
      -> 返回 Session 对象
    -> 更新本地 sessionStore 状态
  -> 如果是 Agent 类型（非 api）：
    -> useWebSocket.createAgentSession(sessionId, agentType)
      -> WebSocket: { type: 'session:create', sessionId, agentType }
        -> Sidecar: adapter.createSession(sessionId)
```

### 7.3 会话切换

切换会话时，MainPanel 的 `useEffect` 自动通知 Sidecar 创建 Agent 会话：

```typescript
useEffect(() => {
  if (currentSessionId && agentType !== 'api' && isConnected) {
    if (!createdSessionsRef.current.has(currentSessionId)) {
      createdSessionsRef.current.add(currentSessionId);
      createAgentSession(currentSessionId, agentType);
    }
  }
}, [currentSessionId, isConnected]);
```

使用 `createdSessionsRef` (Set) 防止重复创建。

### 7.4 消息保存流程

```
Sidecar 返回 { type: 'done', content }
  -> MainPanel.onDone
    -> addMessage(assistantMsg)  // 更新前端状态
    -> invoke('save_message', { sessionId, role: 'assistant', content, mode })
      -> Rust: INSERT INTO messages
      -> Rust: UPDATE sessions SET last_message_preview, message_count + 1, updated_at
```

---

## 8. 状态管理

### 8.1 Zustand Stores

| Store | 文件 | 主要状态 | 说明 |
|-------|------|---------|------|
| `sessionStore` | `sessionStore.ts` | sessions, currentSessionId, messages, streamingContent | 会话列表、当前会话、消息列表、流式内容 |
| `wsStore` | `wsStore.ts` | connected, connecting, error, lastPong | WebSocket 连接状态、错误信息 |
| `skillStore` | `skillStore.ts` | skillsByAgent, loading | 技能列表（按 Agent 类型分组） |
| `apiProviderStore` | `apiProviderStore.ts` | providers, loading | API 提供商列表 |
| `inspirationStore` | `inspirationStore.ts` | inspirations, tags, searchQuery | 灵感列表、标签、搜索 |
| `pendingInputStore` | `pendingInputStore.ts` | text, mode | 待发送输入内容 |

### 8.2 useEnvInfo 共享 Hook

`src/hooks/useEnvInfo.ts` -- 模块级单例模式：

- **获取时机**：组件挂载时 + 手动调用 `refreshEnvInfo()`
- **不轮询**：环境信息变化频率极低，无需定时刷新
- **共享**：StatusBar 和 EnvManager 统一调用同一 hook，避免重复请求

```typescript
// 模块级单例
let cachedPromise: Promise<EnvInfo> | null = null;

export function useEnvInfo() {
  const [envInfo, setEnvInfo] = useState<EnvInfo>({ ... });
  const [loading, setLoading] = useState(true);

  const fetchEnv = useCallback(async () => {
    setLoading(true);
    const result = await invoke<EnvInfo>('detect_env');
    setEnvInfo(result);
    setLoading(false);
  }, []);

  useEffect(() => {
    fetchEnv();
  }, [fetchEnv]);

  return { envInfo, loading, refreshEnvInfo: fetchEnv };
}
```

### 8.3 wsStore 状态管理

`src/stores/wsStore.ts` -- 集中管理 WebSocket 连接状态：

```typescript
interface WsState {
  connected: boolean;
  connecting: boolean;
  error: string | null;
  lastPong: number | null;
}
```

- 所有组件共享单一 WebSocket 连接
- 连接状态通过 Zustand store 订阅，无需轮询
- 错误信息序列化修复：`(msg.error as string)` -> `typeof msg.error === 'string' ? msg.error : JSON.stringify(msg.error)`

---

## 9. 通信协议

### 9.1 Tauri IPC (invoke)

前端通过 `@tauri-apps/api/core` 的 `invoke` 函数调用 Rust 命令：

```typescript
import { invoke } from '@tauri-apps/api/core';

// 示例：创建会话
const session = await invoke<Session>('create_session', {
  agentType: 'claude',
  title: '新会话',
});

// 示例：保存消息
const msg = await invoke<Message>('save_message', {
  sessionId: 'xxx',
  role: 'assistant',
  content: '回复内容',
  mode: 'native',
});
```

### 9.2 WebSocket 协议

前端通过 `useWebSocket` hook 与 Sidecar 通信：

```typescript
// useWebSocket.ts
function useWebSocket(port: number = 19830, handlers?: WsHandlers) {
  // 自动连接 ws://127.0.0.1:{port}
  // 自动重连（指数退避：1s -> 2s -> 4s -> ... -> 30s 上限）
  // 心跳保活（每 30s ping，超时 10s 断开重连）
  // 消息路由到对应 handlers
}
```

**连接生命周期**：

```
连接建立
  -> 注册消息处理器（onChunk, onDone, onError, onStatus, onSkills）
  -> 启动心跳定时器（每 30s ping）
  -> 断线自动重连（指数退避）
  -> 组件卸载时断开
```

---

## 10. 环境检测与版本管理

### 10.1 检测流程

```
detect_env (Rust command)
  +-- node --version
  |     +-- resolve_in_path("node") -> where node -> PATH
  +-- git --version
  |     +-- resolve_in_path("git") -> where git -> PATH
  +-- python --version
  |     +-- resolve_in_path("python") -> where python -> PATH (fallback python3)
  +-- claude --version
  |     +-- resolve_in_path("claude") -> where claude -> PATH
  |     +-- clean_version_string 提取版本号
  +-- hermes --version
  |     +-- resolve_in_path("hermes") -> where hermes -> PATH
  |     +-- clean_version_string 提取版本号
  +-- codex --version
        +-- resolve_in_path("codex") -> where codex -> PATH
        +-- clean_version_string 提取版本号
```

### 10.2 安装/更新流程

```
install_agent(agentType)
  +-- 查找 AGENTS 配置表获取包管理器 + 包名
  +-- run_install(app, manager, package, agent)
  |     +-- cmd /C npm install -g <package> 或 pip install <package>
  |     +-- 成功后 emit "install-progress" 事件
  |     +-- 清除 ENV_CACHE
  +-- 旧命令保留为别名（install_claude_code / install_hermes / install_codex）
```

### 10.3 版本号显示

StatusBar 底部显示各 Agent 安装状态：

| 状态 | 显示 |
|------|------|
| 查询中 | `Agent: 查询中...` |
| 已安装 | `Agent: C v2.1.177 / H v0.16.0 / X v0.140.0` |
| 未安装 | `Agent: C (未安装) / H (未安装) / X (未安装)` |

---

## 11. 关键问题记录

### 11.1 PYTHONHOME 环境变量污染

**问题**：WPS 灵犀设置了 `PYTHONHOME=C:\Users\Administrator\AppData\Roaming\WPS 灵犀\python-env`，所有子进程继承此变量，导致 Hermes 等依赖自身 Python venv 的工具出现 SRE 模块不匹配。

**根因**：WPS 灵犀在 spawn 外部进程前未清除 `PYTHONHOME`。

**处理策略**：
- 不在 PilotDesk 中添加补丁
- 此问题应由 WPS 灵犀在源头修复（spawn 外部进程前清除 PYTHONHOME）
- 后续遇到相同问题影响时，告知用户"这是 PYTHONHOME 环境变量问题"即可

**已在 PilotDesk 中移除所有 PYTHONHOME 补丁**（2026-06-15）。

### 11.2 Agent 版本检测规则

**决策**（2026-06-15 确定）：

- 所有 Agent 版本检测仅基于 CLI 版本，不再检测 Desktop 版本
- Claude Code: `claude --version`（npm 包 `@anthropic-ai/claude-code`）
- Hermes: `hermes --version`（pip 包 `hermes-agent`，检测路径含项目 venv + 全局 pip + PATH）
- CodeX: `codex --version`（npm 包 `@openai/codex`，仅 CLI）

### 11.3 错误序列化

**问题**：WebSocket 错误消息中的 `error` 字段可能是 Error 对象而非字符串，导致前端显示 `[object Object]`。

**修复**：在 `wsStore.ts`、`server.ts`、`SessionList.tsx` 中批量修复：

```typescript
// 修复前
send(ws, { type: 'error', sessionId, error: err.message });

// 修复后
send(ws, { type: 'error', sessionId, error: err?.message || String(err) });
```

### 11.4 Hermes 命令参数引号嵌套

**问题**：`shell: true` 模式下 Node.js 自动为参数加引号，手动加引号导致嵌套，Hermes 收到带引号的字符串。

**修复**：

```typescript
// 修复前
const cmd = `hermes --no-color --no-stream "${fullMessage}"`;

// 修复后
const cmd = `hermes --no-color --no-stream ${fullMessage}`;
```

### 11.5 SkillBrowser 轮询优化

**问题**：SkillBrowser 使用 500ms `setInterval` 轮询检测技能列表变化。

**修复**：改为 Zustand store 订阅模式，技能列表变化时 store 自动通知组件更新。

### 11.6 useEnvInfo 轮询优化

**问题**：`useEnvInfo` 每 60 秒轮询环境信息，但 Agent 版本更新频率远低于 1 分钟。

**修复**：移除自动轮询，改为启动时获取 + 手动刷新（安装/更新完成后主动调用 `refreshEnvInfo()`）。

### 11.7 CodeX --pipe 参数错误

**问题**：codex CLI 0.140.0-alpha.2 不支持 `--pipe` 参数，导致 `unexpected argument '--pipe' found` 错误。

**修复**：`codex --pipe "..."` -> `codex exec "..."`（使用 `exec` 子命令进行非交互式调用）。

### 11.8 CodeX 数据库 CHECK 约束缺失

**问题**：旧数据库的 sessions 表 CHECK 约束为 `('claude', 'hermes', 'api')`，缺少 `'codex'`，导致创建 codeX 会话时触发 `CHECK constraint failed`。

**修复**：`migrate_add_type()` 函数统一检查 `'api'` 和 `'codex'`，任一不接受时重建表。

### 11.9 CSS 缺少 --codex-tag 变量

**问题**：`globals.css` 中 `:root` 和 `[data-theme="dark"]` 都缺少 `--codex-tag` CSS 变量定义，导致 codeX 会话的圆点指示器不显示。

**修复**：两处均添加 `--codex-tag: #F59E0B`。

---

## 12. 文件清单

### 12.1 核心文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `src-tauri/src/lib.rs` | ~195 | Tauri 命令注册、模块初始化、Sidecar 启动 |
| `src-tauri/src/main.rs` | ~20 | 入口 |
| `src-tauri/src/db/init.rs` | ~224 | 数据库初始化 + 6 个迁移函数 |
| `src-tauri/src/db/models.rs` | ~75 | 统一数据模型（Session, Message, Inspiration, BotChannel, EnvInfo, LogEntry） |
| `src-tauri/src/commands/env.rs` | ~270 | 环境检测 + Agent 安装（集中配置表 + 泛化安装 + 动态路径探测） |
| `src-tauri/src/commands/session.rs` | ~230 | 会话/消息 CRUD（8 个命令） |
| `src-tauri/src/commands/update.rs` | ~170 | npm/pypi 版本检测（Registry 枚举 + http_get_json 共享） |
| `src-tauri/src/commands/api_provider.rs` | ~200 | API 提供商管理 |
| `src-tauri/src/commands/app_settings.rs` | ~80 | 应用设置 KV |
| `src-tauri/src/commands/install_log.rs` | ~74 | 安装日志 |
| `src-tauri/src/commands/inspiration.rs` | ~200 | 灵感 CRUD + FTS5 搜索（参数化查询防注入） |
| `src-tauri/src/commands/bot.rs` | ~90 | Bot 频道管理（去重默认值） |
| `src-tauri/src/commands/theme.rs` | ~60 | 主题管理 |
| `src-tauri/src/sidecar/manager.rs` | ~340 | Sidecar 进程管理（动态 Node 路径 + 去重路径探测） |
| `src-tauri/src/utils/paths.rs` | ~12 | 路径工具（app_data_dir, db_path, resolve_in_path） |
| `src-tauri/src/utils/errors.rs` | ~81 | 错误类型枚举（AppError, 8 变体 + Serialize + From 实现） |
| `src-tauri/src/utils/crypto.rs` | ~130 | AES-256-GCM 加密/解密（DPAPI 密钥保护） |
| `sidecar/src/server.ts` | ~210 | WebSocket 服务器 |
| `sidecar/src/adapters/base.ts` | ~30 | AgentAdapter 接口 |
| `sidecar/src/adapters/claude-code.ts` | ~100 | Claude Code 适配器 |
| `sidecar/src/adapters/hermes.ts` | ~120 | Hermes 适配器 |
| `sidecar/src/adapters/codex.ts` | ~140 | CodeX 适配器 |
| `sidecar/src/types.ts` | ~50 | 消息类型定义 |
| `src/App.tsx` | ~60 | 根组件 |
| `src/types/index.ts` | ~220 | 类型定义 + AGENT_THEMES |
| `src/hooks/useWebSocket.ts` | ~200 | WebSocket 连接管理 |
| `src/hooks/useEnvInfo.ts` | ~50 | 环境信息共享 hook |
| `src/stores/sessionStore.ts` | ~200 | 会话/消息状态 |
| `src/stores/wsStore.ts` | ~160 | WebSocket 连接状态 |
| `src/components/layout/MainPanel.tsx` | ~320 | 主面板 |
| `src/components/layout/InputBar.tsx` | ~280 | 输入栏 |
| `src/components/layout/MessageList.tsx` | ~130 | 消息列表 |
| `src/components/layout/SessionList.tsx` | ~200 | 会话列表 |
| `src/components/layout/StatusBar.tsx` | ~100 | 状态栏 |
| `src/components/message/MessageBubble.tsx` | ~300 | 消息气泡 |
| `src/components/env/EnvManager.tsx` | ~260 | 环境管理 |
| `src/styles/globals.css` | ~170 | 全局样式 |

### 12.2 文档

| 文件 | 说明 |
|------|------|
| `PilotDesk-架构与技术实现-v3.4.md` | 本文档 |
| `pilotdesk/docs/PilotDesk-FunctionCalling-联网搜索技术方案-v1.1.md` | Function Calling 与联网搜索方案 |
| `pilotdesk/docs/PilotDesk-内置搜索方案-必应国内版.md` | 必应搜索实现方案 |

---

> PilotDesk 架构与技术实现 v3.5 | 2026-06-16




### 11.16 v3.5 深度架构优化（第三阶段）

**日期**: 2026-06-16

**变更内容**:

1. **UUID/now 辅助函数提取**（P3-1）
   - `utils/mod.rs` 新增 `new_id()`、`now()`、`now_millis()` 三个辅助函数
   - 消除 6 个文件中 17 处 `uuid::Uuid::new_v4()` 和 `chrono::Utc::now()` 重复代码
   - 涉及文件：`bot.rs`、`inspiration.rs`、`session.rs`、`install_log.rs`、`api_provider.rs`、`app_settings.rs`、`init.rs`

2. **结构化日志**（P2-1）
   - `main.rs` 初始化 `env_logger`，默认日志级别 `info`
   - 所有 `println!`/`eprintln!` 替换为 `log::info!`/`log::warn!`/`log::error!`
   - 涉及文件：`lib.rs`（3 处）、`manager.rs`（23 处）、`update.rs`（2 处）
   - `env.rs` 自定义 `log_env!` 宏从 `writeln!(stderr)` 改为 `log::debug!`

3. **crypto.rs DPAPI 密钥保护**（P2-4）
   - 密钥文件写入前通过 Windows `CryptProtectData` 加密，绑定当前用户
   - 读取时通过 `CryptUnprotectData` 解密
   - 非 Windows 平台回退为明文存储
   - 消除密钥文件被其他用户读取的安全风险

4. **sessionStore 竞态修复**（P3-2）
   - `addMessage` 的 `save_message` 回调中增加会话存在性检查
   - 防止会话删除后回调仍在更新已不存在的会话状态

5. **文档更新**
   - 更新环境检测流程图（消除硬编码路径）
   - 更新安装流程图（反映泛化安装）
   - 更新 Message 模型文档（反映扩展字段已持久化）
   - 更新迁移机制文档（反映版本化迁移）
   - 更新文件清单

## 11. 变更记录

### 11.15 v3.4 深度架构优化（第二阶段）

**日期**: 2026-06-16

**变更内容**:

1. **Message 扩展字段持久化**（P1）
   - messages 表新增 `reasoning_content`、`tool_calls`、`tool_call_id`、`tool_name` 四列
   - 自动迁移：`migrate_add_message_extensions()` 检测并添加缺失列
   - `models.rs` Message 结构体添加对应 `Option<String>` 字段
   - `save_message` 命令接受 4 个可选参数
   - `row_to_message` 和 `get_session_messages` 查询包含新列
   - 前端 `sessionStore.ts` 将扩展字段传入 `save_message`

2. **Sidecar 启动失败 emit 事件到前端**（P2）
   - `manager.rs` 中 4 处失败路径添加 `app_handle.emit("sidecar-error", ...)`
   - 覆盖场景：文件不存在、spawn 失败、端口超时、看门狗重启失败

3. **WebSocket 断连发送队列**（P2）
   - `wsStore.ts` 添加 `sendQueue`，断连时消息入队，重连后自动刷新

4. **ENV_CACHE RwLock**（P2）
   - `env.rs` `LAST_DETECT` 从 `Mutex` 改为 `RwLock`，读缓存无阻塞并发

5. **前端 HashRouter 路由**（P3）
   - 安装 `react-router-dom`，`<HashRouter>` 包裹根组件
   - `App.tsx` 使用 `<Routes>` 定义 `/`、`/market`、`/settings` 路由

6. **硬编码 Node.js 路径替换为动态探测**（P1）
   - `manager.rs` 中两处 `"F:\soft\nodejs\node.exe"` 替换为 `resolve_node_path()`

7. **清理 `#[allow(dead_code)]` 注解**（P3）
   - `manager.rs` 中 4 处误标 `#[allow(dead_code)]` 已移除

8. **bot.rs 默认值去重**（P0-4）
   - `save_bot_channel` 中 6 个默认值计算提取到 `match` 前统一执行，消除 12 行重复代码
