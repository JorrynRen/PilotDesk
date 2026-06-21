# PilotDesk 架构与技术实现

> **项目**: PilotDesk | **架构**: Tauri 2.0 + React 19 + TypeScript + Rust + SQLite
> **版本**: v4.6 | **日期**: 2026-06-18 | **状态**: 定稿
> **代码仓库**: `E:\WorkSpace_HermesAgent\pilotdeskProject\pilotdesk`

---

## 目录

1. [项目概述](#1-项目概述)
2. [整体架构](#2-整体架构)
3. [前端架构](#3-前端架构)
4. [Rust 后端](#4-rust-后端)
5. [Agent 集成（原 Sidecar 层）](#5-agent-集成原-sidecar-层)
6. [数据库设计](#6-数据库设计)
7. [状态管理](#7-状态管理)
8. [通信协议](#8-通信协议)
9. [环境检测与版本管理](#9-环境检测与版本管理)
10. [插件系统](#10-插件系统)
11. [主题系统](#11-主题系统)
12. [国际化（i18n）](#12-国际化i18n)
13. [关键问题记录](#13-关键问题记录)
14. [文件清单](#14-文件清单)
15. [变更记录](#15-变更记录)

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
| 样式 | Tailwind CSS 4 + CSS 设计令牌 | 原子化 CSS + 语义化变量 |
| 状态管理 | Zustand + useReducer | 轻量级状态管理 + 状态机 |
| Rust 后端 | Tauri Commands + Tauri Events | 数据库、文件系统、进程管理、流式推送 |
| 数据库 | SQLite (rusqlite 0.32 + r2d2 连接池) | 本地持久化 |
| 进程管理 | tokio::process::Command (Rust) | Agent CLI 进程生命周期管理 |
| Agent CLI | claude / hermes / codex | AI Agent 命令行工具 |

### 1.3 核心功能

- **多 Agent 会话管理**：创建、切换、归档、删除会话
- **统一消息交互**：统一的输入/输出界面，支持流式响应
- **环境检测与管理**：检测本地 Agent 安装状态、版本号
- **Agent 安装向导**：一键安装/更新 Claude Code、Hermes、CodeX
- **API 直连模式**：支持 OpenAI 兼容 API 的直接调用
- **灵感市集**：灵感/提示词管理与搜索（FTS5 全文搜索）
- **技能系统**：Agent 技能列表与调用
- **插件系统**：支持第三方插件发现、启用/禁用
- **自定义主题色**：9 种预设颜色 + 自定义颜色选择器
- **国际化基础**：中英文语言包（zh-CN / en-US）

---

## 2. 整体架构

### 2.1 三层架构（当前）

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
|  |     Tauri IPC (invoke)    |  Tauri Events     |  |
|  |                          |  (流式推送替代     |  |
|  |                          |   WebSocket)       |  |
|  +-------------------+--------------------------+  |
|                      |                              |
|  +-------------------+--------------------------+  |
|  |              Rust 后端                        |  |
|  |  (Tauri Commands + AgentManager)             |  |
|  |                                               |  |
|  |  +--------------+  +----------------------+  |  |
|  |  | SQLite 数据库  |  | AgentManager        |  |  |
|  |  | (r2d2 连接池) |  | (tokio::process)    |  |  |
|  |  +--------------+  |  +-- ClaudeManager    |  |  |
|  |                    |  +-- HermesManager    |  |  |
|  |  +--------------+  |  +-- CodexManager     |  |  |
|  |  | 环境检测/版本  |  +----------------------+  |  |
|  |  +--------------+  |                          |  |
|  |                    |  +----------------------+  |  |
|  |  +--------------+  |  | PluginHost           |  |  |
|  |  | 主题/设置 KV  |  |  | (插件发现/管理)     |  |  |
|  |  +--------------+  |  +----------------------+  |  |
|  +-------------------+--------------------------+  |
+----------------------+-----------------------------+
                       |
              +--------v--------+
              |   Agent CLI     |
              |  (本地安装)      |
              +-----------------+
```

### 2.2 架构演进（v1.0 → v4.4）

| 版本 | 架构特征 | 说明 |
|------|---------|------|
| v1.0 | Node.js Sidecar + WebSocket | 3 进程架构：PilotDesk + Sidecar + Agent CLI |
| v2.0 | AgentType 枚举 | 统一 Agent 类型标识 |
| v3.0 | Zustand + ID-based 去重 | 状态管理重构 |
| v3.5 | DPAPI 加密 + 结构化日志 | 安全加固 |
| **v4.0** | **消除 Sidecar + r2d2 + useReducer** | **架构简化：2 进程架构** |
| v4.1 | 虚拟滚动 + 消息编辑 + 设计令牌 | 性能优化 |
| v4.2 | 消息重发 + 批量操作 + 插件架构 | 功能增强 |
| v4.3 | 插件系统运行时 + 自定义主题色 | 扩展能力 |
| **v4.4** | **综合文档合并** | 合并 v3.5~v4.3 为一份文档 |
| **v4.5** | **插件系统完善** | 插件系统完善 |
| **v4.6** | **UI 修复 + 环境检测修复** | **本文档** |

### 2.3 数据流

#### 2.3.1 会话创建

```
用户点击"新建会话"
  -> SessionList.tsx
    -> sessionStore.createSession(agentType)
      -> invoke('create_session', { agentType })
        -> Rust: INSERT INTO sessions (id, agent_type, title, cwd, created_at, updated_at)
        -> 返回 Session 对象（含默认标题、时间戳）
    -> 如果是 Agent 类型（非 api）：
      -> useAgentEvent.createAgentSession(sessionId, agentType)
        -> Tauri Event: { type: 'session:create', sessionId, agentType }
          -> AgentManager: 注册 sessionId 到对应 Agent 管理器
```

#### 2.3.2 消息发送（Agent 模式）

```
用户输入消息 -> InputBar.tsx
  -> MainPanel.handleSend(message, mode)
    -> addMessage(userMsg) -> sessionStore
    -> sendChat(sessionId, message, mode, agentType, cwd, systemPrompt)
      -> Tauri Event: { type: 'chat', sessionId, message, mode, agentType, systemPrompt }
        -> AgentManager: adapter.sendMessage(request)
          -> spawn Agent CLI (tokio::process::Command)
          -> stdout pipe -> yield chunks
          -> Tauri Event emit: { type: 'chunk', content }
    -> onChunk 回调 -> setStreamingContent -> MessageList 实时渲染
    -> onDone 回调 -> addMessage(assistantMsg) -> sessionStore
    -> invoke('save_message', { sessionId, role: 'assistant', content, mode })
      -> Rust: INSERT INTO messages
      -> Rust: UPDATE sessions SET last_message_preview, message_count, updated_at
```

#### 2.3.3 消息发送（API 直连模式）

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
+-- App.tsx                    # 根组件，主题初始化 + 路由
+-- main.tsx                   # 入口
+-- constants.ts               # 共享常量（EMOJI_OPTIONS 等）
+-- components/
|   +-- common/
|   |   +-- AgentBadge.tsx     # Agent 类型徽章（AgentIcon + 首字母回退）
|   +-- env/
|   |   +-- EnvManager.tsx     # 环境检测与 Agent 安装管理
|   |   +-- InstallLog.tsx     # 安装日志
|   +-- input/
|   |   +-- InspirationPicker.tsx  # 灵感选择器
|   |   +-- SkillPicker.tsx       # 技能选择器
|   +-- inspiration/           # 灵感市集相关组件
|   +-- layout/
|   |   +-- MainPanel.tsx      # 主面板（useReducer 状态机）
|   |   +-- InputBar.tsx       # 输入栏（模式选择 + 发送）
|   |   +-- MessageList.tsx    # 消息列表（react-virtuoso 虚拟滚动）
|   |   +-- SessionList.tsx    # 会话列表（搜索 + 批量操作）
|   |   +-- SessionListItem.tsx # 会话项（批量选择 checkbox）
|   |   +-- StatusBar.tsx      # 底部状态栏
|   |   +-- TitleBar.tsx       # 自定义标题栏
|   |   +-- RightPanel.tsx     # 右侧面板（灵感/技能/记忆/插件）
|   +-- message/
|   |   +-- MessageBubble.tsx  # 消息气泡（AgentIcon 角色标识 + 内联编辑 + 重发）
|   |   +-- MarkdownRenderer.tsx # Markdown 渲染
|   +-- panels/
|   |   +-- SkillBrowser.tsx   # 技能浏览器
|   |   +-- UpdateChecker.tsx  # 更新检查
|   |   +-- ModePromptSettings.tsx # 模式提示词设置
|   +-- plugin/
|   |   +-- PluginManager.tsx  # 插件管理 UI
|   |   +-- PluginIcon.tsx     # 插件图标渲染组件（支持图片URL/本地路径）
|   +-- settings/
|       +-- ThemeCustomizer.tsx # 自定义主题色 UI
+-- hooks/
|   +-- useAgentEvent.ts       # Tauri Event 替代 useWebSocket
|   +-- useEnvInfo.ts          # 环境信息共享 hook
|   +-- useTheme.ts            # 主题切换（SQLite 持久化）
|   +-- useI18n.ts             # 国际化 hook
|   +-- useTauriCommand.ts     # Tauri command 封装
+-- pages/
|   +-- SettingsPage.tsx       # 设置页（含 ThemeCustomizer）
|   +-- EnvPage.tsx            # 环境检测页
+-- stores/
|   +-- sessionStore.ts        # 会话/消息状态（ID-based 去重）
|   +-- skillStore.ts          # 技能列表状态
|   +-- apiProviderStore.ts    # API 提供商状态（使用 invokeHelper）
|   +-- inspirationStore.ts    # 灵感状态（使用 invokeHelper）
|   +-- pendingInputStore.ts   # 待发送输入状态
|   +-- pluginStore.ts         # 插件状态
|   +-- themeStore.ts          # 自定义主题色状态
+-- types/
|   +-- index.ts               # 类型定义 + AGENT_THEMES
|   +-- plugin.ts              # 插件系统类型定义
+-- utils/
|   +-- invokeHelper.ts        # 5 个通用 Tauri invoke 包装函数
|   +-- pluginIcon.ts          # 插件图标解析工具（网络地址/本地路径）
|   +-- apiClient.ts           # API 调用工具（SSE 解析）
|   +-- toast.ts               # 通知提示
+-- locales/
|   +-- zh-CN.json             # 简体中文翻译（35 条）
|   +-- en-US.json             # 英文翻译（35 条）
+-- styles/
    +-- globals.css            # 全局样式 + 42 个 CSS 设计令牌
```

### 3.2 组件层级

```
App
+-- TitleBar                    # 自定义标题栏（无边框窗口拖拽区域）
+-- SessionList (左侧面板)      # 会话列表
|   +-- SessionListItem x N     # 单个会话项（标题、Agent 类型徽章、预览、时间、批量选择 checkbox）
+-- MainPanel (主区域)          # 主内容区（useReducer 状态机管理消息流）
|   +-- MessageList             # 消息列表（react-virtuoso 虚拟滚动 + 消息搜索栏）
|   |   +-- MessageBubble x N   # 消息气泡（角色标识、内容、时间、推理内容、内联编辑、重发按钮）
|   +-- InputBar                # 输入栏
|       +-- 模式选择器          # native / fast / think / expert
|       +-- 灵感/技能按钮       # 快捷插入灵感或技能
|       +-- 文本输入框 + 发送按钮（支持 Enter 发送）
+-- RightPanel (右侧面板)       # 右侧辅助面板
|   +-- InspirationPanel        # 灵感市集（搜索、标签筛选、收藏）
|   +-- SkillBrowser            # 技能浏览器（按 Agent 类型分组）
|   +-- PluginManager           # 插件管理（发现/启用/禁用）
+-- StatusBar (底部)            # 状态栏
|   +-- Agent 安装状态          # 查询中 / C v2.1.177 / H (未安装) / X v0.140.0
+-- SettingsPage (独立路由)     # 设置页
    +-- API 提供商配置          # 增删改 API Provider
    +-- Agent 配置              # Agent 相关设置
    +-- 环境检测                # 环境检测详情（各组件版本号）
    +-- ThemeCustomizer         # 自定义主题色（预设色块 + 颜色选择器）
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

### 3.4 MainPanel useReducer 状态机

替代了 v3.5 中的 3 个 useState + 3 个 useRef，状态变更路径可追踪，消除闭包陷阱：

```typescript
type MessageAction =
  | { type: 'SET_STREAMING'; payload: string }
  | { type: 'APPEND_STREAM'; payload: string }
  | { type: 'CLEAR_STREAM' }
  | { type: 'SET_LOADING'; payload: boolean }
  | { type: 'SET_ERROR'; payload: string | null }
  | { type: 'SET_AGENT_TYPE'; payload: string }
  | { type: 'SET_MODE'; payload: string }
  | { type: 'SET_SESSION_ID'; payload: string | null };

interface MessageState {
  streamingContent: string;
  isLoading: boolean;
  error: string | null;
  agentType: string;
  mode: string;
  currentSessionId: string | null;
}
```

### 3.5 消息列表虚拟滚动

使用 `react-virtuoso` 替代普通列表渲染，仅渲染可见 DOM 节点：

```typescript
import { Virtuoso } from 'react-virtuoso';

<Virtuoso
  ref={virtuosoRef}
  data={messages}
  itemContent={(index, message) => <MessageBubble ... />}
  followOutput="smooth"
  atBottomStateChange={(atBottom) => { /* 控制自动滚动 */ }}
/>
```

长对话性能提升 10-100 倍。

---

## 4. Rust 后端

### 4.1 目录结构

```
src-tauri/
+-- resources/                 # 资源目录（打包携带）
|   +-- agents/                # 内置 Agent 配置
|   +-- icons/                 # 内置 Agent 图标文件
|   +-- assets/                # 其他资源
+-- src/
|   +-- main.rs                # Tauri 入口
|   +-- lib.rs                 # 模块注册 + 命令注册 + setup（含 ResourcePaths 初始化）
|   +-- commands/              # Tauri Commands
|   |   +-- mod.rs
|   |   +-- session.rs         # 会话 CRUD + 消息 CRUD + 搜索
|   |   +-- api_provider.rs    # API 提供商管理
|   |   +-- app_settings.rs    # 应用设置 KV
|   |   +-- env.rs             # 环境检测 + Agent 安装
|   |   +-- update.rs          # npm/pypi 版本检测
|   |   +-- install_log.rs     # 安装日志
|   |   +-- inspiration.rs     # 灵感 CRUD + FTS5 搜索
|   |   +-- bot.rs             # Bot 频道管理
|   |   +-- theme.rs           # 主题管理（SQLite 持久化）
|   +-- agent/
|   |   +-- mod.rs             # AgentManager（统一管理 Agent 进程生命周期）
|   +-- db/
|   |   +-- mod.rs
|   |   +-- init.rs            # 数据库初始化 + 迁移（v6，返回 r2d2 Pool）
|   |   +-- models.rs          # 数据模型
|   +-- plugin/
|   |   +-- mod.rs             # PluginHost（插件发现/启用/禁用）
|   +-- utils/
|       +-- mod.rs             # new_id(), now(), now_millis()
|       +-- paths.rs           # 路径工具
|       +-- errors.rs          # AppError 枚举（9 个变体）
```

### 4.2 命令注册

所有 Tauri Commands 在 `lib.rs` 的 `invoke_handler` 中注册：

```rust
invoke_handler(tauri::generate_handler![
    // 环境检测
    commands::env::detect_env,
    commands::env::install_agent,
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
    commands::session::update_message,
    commands::session::search_sessions,
    commands::session::search_messages,

    // 灵感
    commands::inspiration::list_inspirations,
    commands::inspiration::get_inspiration,
    commands::inspiration::create_inspiration,
    commands::inspiration::update_inspiration,
    commands::inspiration::delete_inspiration,
    commands::inspiration::search_inspirations,
    commands::inspiration::list_tags,

    // Bot 频道
    commands::bot::list_bot_channels,
    commands::bot::save_bot_channel,
    commands::bot::delete_bot_channel,

    // API 提供商
    commands::api_provider::list_api_providers,
    commands::api_provider::get_api_provider,
    commands::api_provider::upsert_api_provider,
    commands::api_provider::delete_api_provider,
    commands::api_provider::get_api_key,
    commands::api_provider::reorder_api_providers,

    // 应用设置
    commands::app_settings::get_app_setting,
    commands::app_settings::set_app_setting,

    // 主题
    commands::theme::get_theme,
    commands::theme::set_theme_cmd,

    // 插件
    plugin::plugin_discover,
    plugin::plugin_list,
    plugin::plugin_enable,
    plugin::plugin_disable,
])
```

### 4.3 关键命令

| 命令 | 文件 | 参数 | 返回值 | 说明 |
|------|------|------|--------|------|
| `detect_env` | env.rs | 无 | `EnvInfo` | 检测 Node/Git/Python/Agent 版本 |
| `install_agent` | env.rs | agent_type | `()` | 泛化安装（通过 AGENTS 配置表） |
| `create_session` | session.rs | agent_type, cwd?, title?, api_provider?, api_model? | `Session` | 创建新会话 |
| `list_sessions` | session.rs | 无 | `Vec<Session>` | 获取活跃会话列表 |
| `get_session_messages` | session.rs | session_id | `Vec<Message>` | 获取会话消息列表 |
| `save_message` | session.rs | session_id, role, content, mode | `Message` | 保存消息并更新会话预览 |
| `update_message` | session.rs | id, content | `()` | 编辑消息内容（Phase 4 新增） |
| `search_sessions` | session.rs | query | `Vec<Session>` | 按标题搜索会话（Phase 4 新增） |
| `search_messages` | session.rs | session_id, query | `Vec<Message>` | 按内容搜索消息（LIKE %query%，Phase 6 新增） |
| `get_app_setting` | app_settings.rs | key | `Option<String>` | 获取应用设置 |
| `set_app_setting` | app_settings.rs | key, value | `()` | 设置应用设置 |
| `list_inspirations` | inspiration.rs | 无 | `Vec<Inspiration>` | 获取灵感列表 |
| `search_inspirations` | inspiration.rs | query | `Vec<Inspiration>` | FTS5 全文搜索灵感 |
| `plugin_discover` | plugin/mod.rs | 无 | `Vec<PluginInstance>` | 扫描插件目录 |
| `plugin_list` | plugin/mod.rs | 无 | `Vec<PluginInstance>` | 列出所有插件 |
| `plugin_enable` | plugin/mod.rs | id | `()` | 启用插件 |
| `plugin_disable` | plugin/mod.rs | id | `()` | 禁用插件 |

### 4.4 环境检测（env.rs）

v3.3 版本对 env.rs 进行了全面重构，采用**集中配置表 + 泛化安装 + 动态路径探测**的设计模式。

#### 集中配置表

所有 Agent 的元数据集中在 `AGENTS` 常量表中：

```rust
pub const AGENTS: &[AgentConfig] = &[
    AgentConfig { id: "claude", name: "Claude Code",  manager: PackageManager::Npm, package: "@anthropic-ai/claude-code" },
    AgentConfig { id: "hermes", name: "Hermes Agent", manager: PackageManager::Pip, package: "hermes-agent" },
    AgentConfig { id: "codex",  name: "codeX",        manager: PackageManager::Npm, package: "@openai/codex" },
];
```

新增 Agent 只需在表中添加一行。

#### 动态路径探测

`resolve_in_path()` 通过 `where` 命令（Windows）动态解析工具路径：

```rust
fn resolve_in_path(name: &str) -> Option<String> {
    // 1. 尝试 where <name> 获取完整路径
    // 2. 解析输出，取第一个有效路径
    // 3. 缓存结果到 RESOLVED_PATHS 避免重复查询
}
```

#### 泛化安装函数

```rust
fn run_install(app: &tauri::AppHandle, manager: PackageManager, package: &str, agent: &str) -> Result<(), AppError> {
    let cmd = match manager {
        PackageManager::Npm => format!("npm install -g {}", package),
        PackageManager::Pip => format!("pip install {}", package),
    };
    // cmd /C 执行 + tauri::Emitter 发送进度事件 + 清除缓存
}
```

#### 缓存机制

```rust
static LAST_DETECT: std::sync::RwLock<Option<(Instant, EnvInfo)>> = std::sync::RwLock::new(None);
```

- 缓存有效期：30 秒
- 使用 `RwLock` 实现无阻塞并发读取（v3.4 从 Mutex 优化为 RwLock）

### 4.5 Agent 集中配置表

| Agent | npm/pip 包名 | 安装命令 |
|-------|-------------|---------|
| Claude Code | `@anthropic-ai/claude-code` | `npm install -g @anthropic-ai/claude-code` |
| Hermes | `hermes-agent` | `pip install hermes-agent` |
| CodeX | `@openai/codex` | `npm install -g @openai/codex` |

### 4.6 错误处理（errors.rs）

v3.3 版本将 `AppError` 重构为 Rust 枚举，v4.0 新增 `Json` 变体：

```rust
#[derive(Debug, Clone)]
pub enum AppError {
    Db(String),            // 数据库操作失败
    Io(String),            // 文件/IO 操作失败
    Lock(String),          // 资源锁定失败
    NotFound(String),      // 资源未找到
    InvalidInput(String),  // 输入参数无效
    External(String),      // 外部服务/进程错误
    Config(String),        // 配置错误
    Network(String),       // 网络请求错误
    Json(String),          // JSON 序列化/反序列化错误（v4.0 新增）
}
```

**From 实现**：`rusqlite::Error`、`std::io::Error`、`r2d2::Error`、`serde_json::Error` 自动转换。

### 4.7 数据库连接池（v4.0 新增）

v4.0 将 `Mutex<Connection>` 替换为 `r2d2` 连接池：

```rust
pub type DbPool = r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>;

pub struct DbState {
    pub pool: DbPool,
}

impl DbState {
    pub fn get_conn(&self) -> Result<PooledConnection<r2d2_sqlite::SqliteConnectionManager>, AppError> {
        self.pool.get().map_err(|e| AppError::Db(e.to_string()))
    }
}
```

### 4.8 资源路径管理（v4.5 新增）

v4.5 新增 `ResourcePaths` 结构体，统一管理内置资源和用户资源路径：

```rust
pub struct ResourcePaths {
    pub builtin: PathBuf,  // 内置资源（打包携带，只读）
    pub user: PathBuf,     // 用户资源（可读写）
}
```

**路径规则**：

| 类型 | 开发模式 | 生产模式（Windows） |
|------|---------|-------------------|
| 内置资源 | `src-tauri/resources/` | `%APPDATA%\com.pilotdesk.app\resources\` |
| 用户资源 | `app_data_dir/resources/` | `%APPDATA%\com.pilotdesk.app\resources\`（Roaming） |

**目录结构**：

```
resources/
├── agents/      # 内置 Agent 配置（JSON/YAML）
├── icons/       # Agent 图标文件（.ico/.png/.svg）
└── assets/      # 其他资源
```

**初始化**：在 `setup()` 阶段通过 `app.path().resource_dir()` 和 `app.path().app_data_dir()` 获取路径，通过 `app.manage(ResourcePaths { ... })` 注册为 Tauri State，所有命令通过 `State<ResourcePaths>` 访问。

**图标加载流程**：

1. Agent 配置中 `icon` 字段以 `file:claude_icon.ico` 格式存储
2. 前端 `AgentIcon` 组件检测到 `file:` 前缀，调用 `read_agent_icon` Rust 命令
3. Rust 端从 `ResourcePaths.builtin/icons/` 目录读取文件，返回 base64 data URL
4. 前端渲染 `<img>` 标签显示图标

**AgentIcon 集成点**：

| 组件 | 文件 | 显示逻辑 |
|------|------|---------|
| `AgentBadge` | `components/common/AgentBadge.tsx` | 始终渲染 `AgentIcon`，`fallback` 为 Agent 首字母（C/H/X） |
| `MessageBubble`（Assistant） | `components/message/MessageBubble.tsx` | 圆形头像区域渲染 `AgentIcon`，`fallback` 为首字母 + 主题色背景 |
| `MessageBubble`（User） | `components/message/MessageBubble.tsx` | 硬编码 `User` 图标（待实施项：接入用户系统后替换为用户头像/昵称） |

**Agent 图标显示规则**（非用户消息气泡标识）：

1. `icon` 字段值不为空且图标存在（文件存在或为有效 Emoji/图片）→ 显示为 icon 图标
2. `icon` 字段为空或图标不存在 → 以 Agent 名称（`name` 字段）的第一个字符（英文字符格式化为大写）+ Agent 主题色背景
3. 主题色亦为空或无效 → 使用系统全局主题色

**用户消息气泡标识**（待实施项）：

- 预留拓展接口，当前硬编码 `User` 图标
- 后续用户系统（用户昵称和用户头像等）完成后替换

- 最大连接数：8
- 读写不再互斥，并行查询性能提升显著
- 所有命令通过 `get_conn()` 获取连接，替代旧 `lock_db()` 模式

### 4.8 辅助函数（v3.5 新增）

`utils/mod.rs` 提供三个辅助函数，消除 6 个文件中 17 处重复代码：

```rust
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
```

### 4.9 结构化日志（v3.5 新增）

- `main.rs` 初始化 `env_logger`，默认日志级别 `info`
- 所有 `println!`/`eprintln!` 替换为 `log::info!`/`log::warn!`/`log::error!`
- `env.rs` 自定义 `log_env!` 宏从 `writeln!(stderr)` 改为 `log::debug!`

### 4.10 DPAPI 加密（v3.5 新增）

- 密钥文件写入前通过 Windows `CryptProtectData` 加密，绑定当前用户
- 读取时通过 `CryptUnprotectData` 解密
- 非 Windows 平台回退为明文存储



## 5. Agent 集成（原 Sidecar 层）

### 5.1 架构演进：Sidecar → AgentManager

**v3.5 及之前**：使用独立的 Node.js WebSocket 服务器（Sidecar）作为 Agent CLI 通信中间层。

```
PilotDesk App (前端)
    |
    | WebSocket (ws://127.0.0.1:19830)
    v
Sidecar (Node.js)          ← 已废弃（v4.0 消除）
    +-- server.ts
    +-- adapters/
        +-- base.ts
        +-- claude-code.ts
        +-- hermes.ts
        +-- codex.ts
```

**v4.0 及之后**：使用 Rust AgentManager 直接管理 Agent CLI 进程，通过 Tauri Event 推送流式输出。

```
PilotDesk App (前端)
    |
    | Tauri Events (流式推送)
    v
AgentManager (Rust)        ← 当前架构
    +-- tokio::process::Command
    +-- AgentType 枚举方法
    +-- Tauri Emitter
```

### 5.2 AgentManager

**文件：** `src-tauri/src/agent/mod.rs`

AgentManager 是 Rust 端的 Agent 进程管理器，统一管理 Claude/Hermes/Codex 三个 Agent 的生命周期。

**核心结构**：

```rust
pub struct AgentManager {
    sessions: Arc<Mutex<HashMap<String, AgentSession>>>,
}

struct AgentSession {
    agent_type: AgentType,
    child: Option<Child>,
    created_at: Instant,
}
```

**AgentType 枚举**：

```rust
pub enum AgentType {
    Claude,
    Hermes,
    Codex,
}
```

**生命周期**：

```
create_session(sessionId, agentType)
  -> 注册 sessionId 到 sessions HashMap
  -> 初始化 AgentSession { agent_type, child: None, created_at: now }

send_message(sessionId, message, mode, cwd, systemPrompt, app_handle)
  -> 获取 AgentSession
  -> 根据 AgentType 构造 CLI 命令
  -> tokio::process::Command::new(cmd).args(args).spawn()
  -> 异步读取 stdout
  -> 逐行 emit Tauri Event { type: 'chunk', content }
  -> 完成后 emit { type: 'done', content }

stop_generation(sessionId)
  -> 终止对应子进程

close_session(sessionId)
  -> 从 HashMap 移除
```

### 5.3 消除 Sidecar 的收益

| 指标 | 消除前（v3.5） | 消除后（v4.0） | 改进 |
|------|---------------|---------------|------|
| 进程数 | 3（App + Sidecar + Agent CLI） | 2（App + Agent CLI） | -33% |
| 通信延迟 | WebSocket 序列化/反序列化 | 进程内 Tauri Event | 低延迟 |
| 崩溃点 | Sidecar 进程崩溃 | 无中间进程 | 更可靠 |
| 代码量 | sidecar/ 目录 ~598 个文件 | agent/mod.rs ~300 行 | -72% |
| 维护成本 | Node.js + Rust 双栈 | 纯 Rust | 统一技术栈 |

### 5.4 已废弃的 Sidecar 组件

以下组件在 v4.0 中已全部删除：

| 文件 | 职责 | 删除原因 |
|------|------|---------|
| `sidecar/src/server.ts` | WebSocket 服务器 | 由 AgentManager + Tauri Event 替代 |
| `sidecar/src/adapters/base.ts` | AgentAdapter 接口 | 由 AgentType 枚举方法替代 |
| `sidecar/src/adapters/claude-code.ts` | Claude Code 适配器 | 由 AgentManager 统一管理 |
| `sidecar/src/adapters/hermes.ts` | Hermes 适配器 | 由 AgentManager 统一管理 |
| `sidecar/src/adapters/codex.ts` | CodeX 适配器 | 由 AgentManager 统一管理 |
| `sidecar/src/types.ts` | 消息类型定义 | 由 Rust 类型替代 |
| `src-tauri/src/sidecar/manager.rs` | Sidecar 进程管理 | 由 AgentManager 替代 |
| `src/hooks/useWebSocket.ts` | WebSocket 连接管理 | 由 useAgentEvent 替代 |
| `src/stores/wsStore.ts` | WebSocket 状态 | 由 useAgentEvent 管理 |
| `sidecar/` 目录 | 整个 Node.js 项目 | 598 个文件全部删除 |

### 5.5 前端 Agent 通信 Hook

**文件：** `src/hooks/useAgentEvent.ts`

替代了 v3.5 中的 `useWebSocket.ts`，基于 Tauri Event 实现：

```typescript
function useAgentEvent(handlers?: AgentEventHandlers) {
  // 监听 Tauri Events: 'chunk', 'done', 'error', 'status', 'skills'
  // 消息路由到对应 handlers
  // 组件卸载时自动取消监听
}
```

**事件类型**：

| 事件名 | 载荷 | 说明 |
|--------|------|------|
| `chunk` | `{ sessionId, content }` | 流式输出片段 |
| `done` | `{ sessionId, content }` | 生成完成 |
| `error` | `{ sessionId, error }` | 错误信息 |
| `status` | `{ sessionId, status }` | 状态更新 |
| `skills` | `{ sessionId, agentType, skills }` | 技能列表 |

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
| `title` | TEXT | NOT NULL | `''` | 会话标题 |
| `cwd` | TEXT | - | `''` | 工作目录路径 |
| `created_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |
| `updated_at` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |
| `last_message_preview` | TEXT | - | `''` | 最后一条消息的前 100 字符 |
| `message_count` | INTEGER | - | 0 | 消息总数 |
| `status` | TEXT | CHECK | `'active'` | active（活跃）/ archived（归档） |
| `api_provider` | TEXT | - | NULL | 仅 api 类型使用 |
| `api_model` | TEXT | - | NULL | 仅 api 类型使用 |

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
| `id` | TEXT | PK | - | UUID v4 |
| `session_id` | TEXT | NOT NULL, FK -> sessions(id) ON DELETE CASCADE | - | 所属会话 ID |
| `role` | TEXT | NOT NULL, CHECK | - | user / assistant / system |
| `content` | TEXT | NOT NULL | `''` | 消息文本内容 |
| `mode` | TEXT | CHECK | `'native'` | native / fast / think / expert |
| `timestamp` | INTEGER | NOT NULL | - | Unix 时间戳（秒） |

**扩展字段**（v3.4 迁移添加）：

```sql
-- 通过 migrate_add_message_extensions() 添加
reasoning_content TEXT,  -- 推理/思考内容
tool_calls TEXT,         -- 工具调用（JSON 数组）
tool_call_id TEXT,       -- 工具调用 ID
tool_name TEXT           -- 工具名称
```

**Rust 模型**：

```rust
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub mode: String,
    pub timestamp: i64,
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
}
```

---

#### inspirations（灵感表）

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

---

#### inspiration_tags（灵感标签表）

```sql
CREATE TABLE inspiration_tags (
    inspiration_id TEXT NOT NULL REFERENCES inspirations(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (inspiration_id, tag)
);

CREATE INDEX IF NOT EXISTS idx_inspirations_tags ON inspiration_tags(tag);
```

---

#### inspirations_fts（灵感全文搜索表）

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS inspirations_fts USING fts5(
    title, content,
    content=inspirations,
    content_rowid=rowid
);
```

使用 `content=` 外部内容表模式，灵感数据变更时需手动同步 FTS 索引。

---

#### bot_channels（Bot 频道表）

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

---

#### api_providers（API 提供商表）

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

---

#### app_settings（应用设置表）

```sql
CREATE TABLE app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL
);
```

**默认种子数据**：

| key | value | 说明 |
|-----|-------|------|
| `mode_prompt_native` | （空字符串） | 标准模式 |
| `mode_prompt_fast` | 快速简洁回答... | 快速模式 |
| `mode_prompt_think` | 逐步分析推理... | 思考模式 |
| `mode_prompt_expert` | 以资深专家的视角... | 专家模式 |
| `theme` | light / dark | 主题模式（v4.0 迁移到 app_settings） |
| `theme_colors` | JSON 字符串 | 自定义主题色（v4.3 新增） |

---

#### install_logs（安装日志表）

```sql
CREATE TABLE install_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    message TEXT NOT NULL DEFAULT '',
    level TEXT NOT NULL DEFAULT 'info' CHECK(level IN ('info', 'warn', 'error', 'success'))
);

CREATE INDEX IF NOT EXISTS idx_install_logs_time ON install_logs(timestamp);
```

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

### 6.4 迁移机制

所有迁移在 `init_db()` 中顺序执行，返回 `r2d2::Pool`（v4.0 改为连接池模式）：

```rust
pub fn init_db() -> Result<DbPool, AppError> {
    // 1. 创建初始表结构
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

    Ok(pool)
}
```

迁移函数通过探测式检测（尝试查询/插入）判断是否需要执行迁移，而非维护版本号。

---

## 7. 状态管理

### 7.1 Zustand Stores

| Store | 文件 | 主要状态 | 说明 |
|-------|------|---------|------|
| `sessionStore` | `sessionStore.ts` | sessions, currentSessionId, messages, streamingContent | 会话列表、当前会话、消息列表、流式内容（ID-based 去重） |
| `skillStore` | `skillStore.ts` | skillsByAgent, loading | 技能列表（按 Agent 类型分组） |
| `apiProviderStore` | `apiProviderStore.ts` | providers, loading | API 提供商列表（使用 invokeHelper） |
| `inspirationStore` | `inspirationStore.ts` | inspirations, tags, searchQuery | 灵感列表、标签、搜索（使用 invokeHelper） |
| `pendingInputStore` | `pendingInputStore.ts` | text, mode | 待发送输入内容 |
| `pluginStore` | `pluginStore.ts` | plugins, loading | 插件列表（v4.3 新增） |
| `themeStore` | `themeStore.ts` | accent, accentHover, accentLight | 自定义主题色（v4.3 新增） |

### 7.2 已移除的 Store

| Store | 移除版本 | 移除原因 |
|-------|---------|---------|
| `wsStore` | v4.0 | WebSocket 已消除，由 useAgentEvent 替代 |

### 7.3 ID-based 消息去重

v3.0 引入，替代 v2.x 的 2 秒时间戳窗口去重：

```typescript
// sessionStore.ts
const processedIds = new Set<string>();

addMessage(msg) {
  if (processedIds.has(msg.id)) return;  // 已处理，跳过
  processedIds.add(msg.id);
  // ... 正常处理
}
```

### 7.4 useReducer 状态机

MainPanel 使用 useReducer 替代 3 个 useState + 3 个 useRef：

```typescript
type MessageAction =
  | { type: 'SET_STREAMING'; payload: string }
  | { type: 'APPEND_STREAM'; payload: string }
  | { type: 'CLEAR_STREAM' }
  | { type: 'SET_LOADING'; payload: boolean }
  | { type: 'SET_ERROR'; payload: string | null }
  | { type: 'SET_AGENT_TYPE'; payload: string }
  | { type: 'SET_MODE'; payload: string }
  | { type: 'SET_SESSION_ID'; payload: string | null };
```

### 7.5 invokeHelper 通用封装

`src/utils/invokeHelper.ts` 提供 5 个通用 Tauri invoke 包装函数，消除所有 store 中的重复 try-catch：

```typescript
export async function listItems<T>(command: string): Promise<T[]> { ... }
export async function getItem<T>(command: string, id: string): Promise<T | null> { ... }
export async function saveItem<T>(command: string, item: Partial<T>): Promise<T> { ... }
export async function deleteItem(command: string, id: string): Promise<void> { ... }
export async function invokeAction<T>(command: string, args?: Record<string, unknown>): Promise<T> { ... }
```

使用 invokeHelper 后，`inspirationStore.ts` 精简 ~135 行，`apiProviderStore.ts` 精简 ~204 行。

### 7.6 useEnvInfo 共享 Hook

`src/hooks/useEnvInfo.ts` -- 模块级单例模式：

- **获取时机**：组件挂载时 + 手动调用 `refreshEnvInfo()`
- **不轮询**：环境信息变化频率极低，无需定时刷新
- **共享**：StatusBar 和 EnvManager 统一调用同一 hook

---

## 8. 通信协议

### 8.1 Tauri IPC (invoke)

前端通过 `@tauri-apps/api/core` 的 `invoke` 函数调用 Rust 命令：

```typescript
import { invoke } from '@tauri-apps/api/core';

// 示例：创建会话
const session = await invoke<Session>('create_session', {
  agentType: 'claude',
  title: '新会话',
});
```

### 8.2 Tauri Events（流式推送）

替代 v3.5 的 WebSocket 协议，AgentManager 通过 Tauri Event 向前端推送流式输出：

```typescript
// Rust 端（AgentManager）
app_handle.emit("chunk", serde_json::json!({
    "sessionId": session_id,
    "content": chunk_text
})).ok();

// 前端（useAgentEvent）
import { listen } from '@tauri-apps/api/event';

useEffect(() => {
  const unlisten = listen<ChunkPayload>('chunk', (event) => {
    handlers.onChunk?.(event.payload);
  });
  return () => { unlisten.then(fn => fn()); };
}, []);
```

**事件类型**：

| 事件名 | 载荷 | 说明 |
|--------|------|------|
| `chunk` | `{ sessionId, content }` | 流式输出片段 |
| `done` | `{ sessionId, content }` | 生成完成 |
| `error` | `{ sessionId, error }` | 错误信息 |
| `status` | `{ sessionId, status }` | 状态更新 |
| `skills` | `{ sessionId, agentType, skills }` | 技能列表 |

### 8.3 已废弃的 WebSocket 协议

v3.5 及之前使用 WebSocket 协议，v4.0 已全部废弃：

**请求格式**：

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
  | { type: 'chunk', sessionId, content }
  | { type: 'done', sessionId, content }
  | { type: 'error', sessionId, error }
  | { type: 'status', sessionId, status }
  | { type: 'skills', sessionId, agentType, skills };
```

### 8.4 API 直连模式（前端 SSE）

非 Agent 模式（`api` 类型）通过前端 `fetch` + SSE 流式解析直接调用 API：

```typescript
// apiClient.ts
async function* streamChat(endpoint, apiKey, model, messages) {
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'Authorization': `Bearer ${apiKey}` },
    body: JSON.stringify({ model, messages, stream: true }),
  });
  const reader = response.body!.getReader();
  // 逐行解析 SSE: "data: {...}"
  // yield 每个 chunk
}
```



## 9. 环境检测与版本管理

### 9.1 检测流程

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

### 9.2 版本号提取逻辑

```rust
fn clean_version_string(raw: &str) -> String {
    // 按空格和左括号分割
    // 优先级 1: "v" + 数字开头（如 "v0.16.0"）
    // 优先级 2: 纯数字开头（如 "2.1.177", "0.140.0-alpha.2"）
    // 避免匹配日期字符串（如 "2026.6.5"）
}
```

### 9.3 安装/更新流程

```
install_agent(agentType)
  +-- 查找 AGENTS 配置表获取包管理器 + 包名
  +-- run_install(app, manager, package, agent)
  |     +-- cmd /C npm install -g <package> 或 pip install <package>
  |     +-- 成功后 emit "install-progress" 事件
  |     +-- 清除 ENV_CACHE
  +-- 旧命令保留为别名（install_claude_code / install_hermes / install_codex）
```

### 9.4 版本号显示

StatusBar 底部显示各 Agent 安装状态：

| 状态 | 显示 |
|------|------|
| 查询中 | `Agent: 查询中...` |
| 已安装 | `Agent: C v2.1.177 / H v0.16.0 / X v0.140.0` |
| 未安装 | `Agent: C (未安装) / H (未安装) / X (未安装)` |

---

## 10. 插件系统

### 10.1 架构

插件系统采用**Rust PluginHost + Zustand PluginStore + PluginRegistry 组件注册表 + PluginAPI 运行时**的四层架构。

```
Rust PluginHost (沙箱/验证/文件操作)
  ↓ Tauri invoke
Zustand PluginStore (插件列表 + 注册数据)
  ↓ selector 精确订阅
PluginRegistry (面板组件注册 + JS 执行 + 生命周期)
  ↓
PluginAPI (运行时 API，每个插件独立实例)
```

### 10.2 设计文档

**文件：** `docs/PilotDesk-插件系统架构设计-v1.0.md`

### 10.3 类型定义

**文件：** `src/types/plugin.ts`

```typescript
export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  minAppVersion: string;
  permissions: PluginPermission[];
  entry: { main: string; styles?: string };
  contributes?: {
    panels?: PanelContribution[];
    commands?: CommandContribution[];
    hooks?: HookContribution[];
  };
}

export interface PluginInstance {
  manifest: PluginManifest;
  enabled: boolean;
  loaded: boolean;
  path: string;
  error?: string;
  permission_checks: PermissionCheck[];
  has_unauthorized_permissions: boolean;
}
```

### 10.4 Rust PluginHost

**文件：** `src-tauri/src/plugin/mod.rs`

```rust
pub struct PluginHost {
    plugins_dir: PathBuf,
    plugins: HashMap<String, PluginInstance>,
    sandbox_enabled: bool,
    allowed_permissions: Vec<String>,
    high_risk_permissions: Vec<String>,
}
```

**Tauri 命令**：

| 命令 | 参数 | 返回 |
|------|------|------|
| `plugin_discover` | 无 | `Vec<PluginInstance>` |
| `plugin_list` | 无 | `Vec<PluginInstance>` |
| `plugin_enable` | `id: String` | `()` |
| `plugin_disable` | `id: String` | `()` |
| `plugin_install_zip` | `zip_path: String` | `PluginInstance` |
| `plugin_uninstall` | `id: String` | `()` |
| `plugin_get_sandbox_info` | 无 | `SandboxInfo` |
| `plugin_set_sandbox_enabled` | `enabled: bool` | `()` |
| `plugin_read_entry` | `plugin_id: String` | `String`（入口文件源码） |
| `plugin_get_panel_content` | `plugin_id, panel_id` | `String` |

### 10.5 前端 PluginStore

**文件：** `src/stores/pluginStore.ts`

Zustand store，统一管理插件数据和注册信息：

```typescript
interface PluginStoreState {
  plugins: PluginInstance[];
  sandboxInfo: SandboxInfo | null;
  registeredPanels: Map<string, RegisteredPanel>;
  registeredCommands: Map<string, RegisteredCommand>;
  registeredHooks: Map<string, RegisteredHook[]>;
  // actions: discover, list, enable, disable, installZip, uninstall
  //          fetchSandboxInfo, setSandboxEnabled, refreshRegistrations
}
```

**关键设计：** `refreshRegistrations()` 在每次 `discover`/`list` 后自动调用，从 `plugins[]` 的 `manifest.contributes` 重建 `registeredPanels/Commands/Hooks`。组件通过 Zustand selector 精确订阅所需数据，无需手动订阅/通知。

### 10.6 前端 PluginManager UI

**文件：** `src/components/plugin/PluginManager.tsx`

- 自动扫描插件列表
- 每个插件显示：名称、版本、描述、作者、权限标签、贡献点摘要
- 启用/禁用切换按钮（含沙箱权限检查）
- 沙箱面板（状态标签、启用/禁用按钮、目录、权限列表）
- 空状态提示
- 安装/卸载/刷新按钮

### 10.7 RightPanel 集成

**文件：** `src/components/layout/RightPanel.tsx`

右侧面板"插件"标签页，与灵感、技能、记忆并列。插件面板下拉菜单直接从 `usePluginStore((s) => s.registeredPanels)` 精确订阅。

### 10.8 安全沙箱

#### 权限系统

| 权限 | 说明 | 风险等级 |
|------|------|---------|
| `ui:panel` | 添加/移除面板 | 低 |
| `ui:toast` | 显示通知（默认授权） | 低 |
| `ui:modal` | 打开模态框 | 低 |
| `session:read` | 读取会话和消息 | 中 |
| `session:write` | 创建/修改/删除会话 | 中 |
| `data:invoke` | 调用 Tauri 命令 | **高** |
| `storage:*` | 插件独立存储（默认授权） | 低 |
| `fs:read` | 读取文件系统 | **高** |
| `fs:write` | 写入文件系统 | **高** |

#### 沙箱规则

1. **清单验证**：manifest.json 大小限制 64KB，字段格式严格校验
2. **路径保护**：所有文件路径禁止包含 `..`，防止目录遍历攻击
3. **权限白名单**：未知权限自动拒绝，高风险权限标记警告
4. **入口验证**：入口文件必须存在，路径必须在插件目录内
5. **沙箱禁用时**：所有权限检查跳过，插件可正常加载

### 10.8.1 插件图标系统

**文件：** `src/utils/pluginIcon.ts`、`src/components/plugin/PluginIcon.tsx`

插件清单 `manifest.json` 的 `contributes.panels[].icon` 字段支持三种格式：

| 格式 | 示例 | 说明 |
|------|------|------|
| 网络图片 | `"https://example.com/icon.png"` | 直接渲染为 `<img>` |
| 本地路径 | `"image/favicon.png"` | 相对于插件目录，通过 Tauri `convertFileSrc()` 转换 |
| 空/未定义 | `""` 或省略 | 显示默认图标 `📦` |

**渲染流程：**
```
manifest.json icon 字段
  → parsePluginIcon(icon, pluginPath)
    → 检测是否以 http:// 或 https:// 开头
      → 是：直接返回 URL
      → 否：拼接插件目录路径 + convertFileSrc() 转换
  → PluginIcon 组件
    → 渲染 <img> 标签（14px 尺寸）
    → 加载失败时自动回退到默认图标 📦
```

**安全配置：** 需要在 `tauri.conf.json` 中配置 asset protocol 白名单：

```json
{
  "app": {
    "security": {
      "assetProtocol": {
        "enable": true,
        "scope": ["$APPDATA/PilotDesk/plugins/**"]
      },
      "csp": "img-src 'self' data: https: asset: https://asset.localhost"
    }
  }
}
```

作用域限定在 `plugins/**` 目录，风险可控。本地图片通过 `convertFileSrc()` 转换为 `https://asset.localhost/...` 格式供 WebView 加载。

### 10.9 示例插件

**位置：** `examples/plugins/hello-world/`

```
hello-world/
├── manifest.json    # 插件清单（静态声明面板/命令/钩子）
├── index.js        # 插件入口（纯 JS，运行时注册面板组件）
└── README.md        # 插件开发文档
```

**功能：**
- 注册"Hello World"面板到右侧面板（含实时时钟 + 计数器）
- 加载/卸载通知
- 监听 `message:before-send` 事件

**恶意插件示例：** `examples/plugins/malicious-sample/`

故意违反所有沙箱规则的插件，用于验证沙箱防护效果。

### 10.10 插件开发文档

`examples/plugins/hello-world/README.md` 包含完整的插件开发指南。

### 10.11 前端插件入口（PluginRegistry）

**文件：** `src/plugin/PluginRegistry.ts`

面板组件注册表 + 插件 JS 执行 + 生命周期管理：

```typescript
class PluginRegistry {
  // 面板组件管理
  setPanelComponent(pluginPath, panelId, component): void
  getPanelComponent(pluginPath, panelId): React.ComponentType | undefined
  unsetPluginPanelComponents(pluginPath): void

  // 插件生命周期（async）
  async loadPlugin(plugin): void       // 读取入口 → 执行 JS → 调用 onLoad
  async unloadPlugin(pluginPath): void // 调用 onUnload → 清理 API → 注销组件
  async loadAllPlugins(plugins): void  // 批量加载
  async unloadAllPlugins(): void       // 批量卸载
}
```

**JS 执行流程：**
```
loadPlugin()
  → 1. 注册默认面板组件 (DefaultPluginPanel)
  → 2. Rust: plugin_read_entry 读取 index.js 源码
  → 3. new Function('React', source.replace('export default', 'return '))
  → 4. 执行获取模块对象 { onLoad, onUnload }
  → 5. 调用 onLoad(api)
    → api.ui.addPanel() → 注册真实 React 组件（覆盖默认面板）
    → api.ui.showToast() → 显示通知
    → api.events.on() → 监听事件
    → api.storage.set/get() → 存储数据
```

**文件：** `src/plugin/PluginAPI.ts`

每个插件实例拥有独立的 `PluginAPI` 实例，包含：
- `ui.addPanel/removePanel/showToast` — UI 操作
- `data.invoke` — Tauri 命令调用
- `events.on/emit` — 事件总线（每个插件独立）
- `storage.get/set/delete` — 基于 localStorage 的插件独立存储

**文件：** `src/components/plugin/PluginPanelRenderer.tsx`

渲染已注册的插件面板，通过 `pluginRegistry.getPanelComponent()` 获取组件。

**文件：** `src/components/plugin/DefaultPluginPanel.tsx`

默认面板组件，当插件未提供自定义组件时展示插件基本信息。

### 10.12 本地上传安装

**Rust 命令：** `plugin_install_zip` / `plugin_uninstall`

**安装流程：**
```
用户点击「+ 安装」按钮
  → Tauri dialog 选择 .zip 文件
  → Rust: 扫描并验证 manifest.json
  → Rust: 解压到 ~/.pilotdesk/plugins/<plugin-id>/
  → Rust: 沙箱验证
  → 前端: 刷新插件列表 → PluginRegistry.loadPlugin() 执行 JS
```

### 10.13 待实施

| 项目 | 状态 | 说明 |
|------|------|------|
| 插件市场 | 待实施 | 在线插件浏览和安装 |
| 命令面板 | 待实施 | 注册的命令在前端可执行 |
| 事件钩子管道 | 待实施 | 钩子事件的实际触发链路 |
| 用户系统 | 待实施 | 用户昵称、头像管理，替换消息气泡中 User 图标 |
| 跨平台适配（macOS/Linux） | 待实施 | 内置图标格式从 .ico 扩展为 .png/.svg；种子数据中 PowerShell 命令替换为跨平台方案；Dev 路径回退逻辑验证 |

---

## 11. 主题系统

### 11.1 CSS 设计令牌系统（v4.1）

**文件：** `src/styles/globals.css`

42 个语义化 CSS 变量，分为 10 个类别：

| 类别 | 变量数量 | 前缀 | 示例 |
|------|---------|------|------|
| 背景色 | 5 | `--bg-*` | `--bg-primary`, `--bg-secondary` |
| 前景色 | 5 | `--text-*` | `--text-primary`, `--text-secondary` |
| 边框色 | 3 | `--border-*` | `--border-color`, `--border-hover` |
| 品牌色 | 3 | `--accent-*` | `--accent`, `--accent-hover`, `--accent-light` |
| Agent 标签色 | 4 | `--*-tag` | `--claude-tag`, `--hermes-tag` |
| 消息气泡 | 4 | `--msg-*` | `--msg-user-bg`, `--msg-assistant-bg` |
| 阴影 | 4 | `--shadow-*` | `--shadow-sm`, `--shadow-lg` |
| 圆角 | 4 | `--radius-*` | `--radius-sm`, `--radius-md` |
| 间距 | 6 | `--space-*` | `--space-xs`, `--space-md` |
| 过渡 | 4 | `--transition-*` | `--transition-fast`, `--transition-normal` |

### 11.2 主题持久化（v4.0）

**文件：** `src/hooks/useTheme.ts`

- 主题模式（light/dark）持久化到 SQLite `app_settings` 表（`theme` key）
- 替代 v3.5 的 `theme.txt` 文件存储
- 自动迁移回退：首次启动时从 `theme.txt` 读取并迁移到 SQLite

### 11.3 自定义主题色（v4.3）

#### themeStore

**文件：** `src/stores/themeStore.ts`

```typescript
interface ThemeColors {
  accent: string;       // 主色
  accentHover: string;  // 悬停色（自动变暗 20）
  accentLight: string;  // 浅色（15% 透明度）
}
```

- 9 种预设颜色 + 自定义颜色选择器
- 持久化到 SQLite `app_settings` 表（`theme_colors` key）
- 通过 CSS 变量 `--accent`、`--accent-hover`、`--accent-light` 即时生效
- 重置功能恢复默认蓝色主题

#### ThemeCustomizer 组件

**文件：** `src/components/settings/ThemeCustomizer.tsx`

- 预设色块网格（9 种颜色）
- 自定义颜色 input[type="color"]
- 重置按钮

#### SettingsPage 集成

在"通用设置 → 主题设置"区域下方嵌入 ThemeCustomizer。

---

## 12. 国际化（i18n）

### 12.1 架构

轻量级国际化实现，无第三方依赖，JSON 文件 + 动态 import。

### 12.2 语言文件

**文件：** `src/locales/zh-CN.json`（35 条翻译）
**文件：** `src/locales/en-US.json`（35 条翻译）

覆盖范围：会话管理、消息交互、设置页面、环境管理、通用 UI 文本。

### 12.3 useI18n Hook

**文件：** `src/hooks/useI18n.ts`

```typescript
function useI18n() {
  const [locale, setLocale] = useState<'zh-CN' | 'en-US'>('zh-CN');
  const [messages, setMessages] = useState<Record<string, string>>({});

  // 动态 import JSON 文件
  useEffect(() => {
    import(`../locales/${locale}.json`).then((mod) => setMessages(mod.default));
  }, [locale]);

  const t = (key: string) => messages[key] || key;

  return { t, locale, setLocale };
}
```

### 12.4 当前状态

| 项目 | 状态 | 说明 |
|------|------|------|
| 语言文件 | 已完成 | zh-CN.json + en-US.json |
| useI18n hook | 已完成 | 动态 import + locale 切换 |
| 组件集成 | **暂缓** | 组件硬编码文本待替换为 `t()` 调用 |

---

## 13. 关键问题记录

### 13.1 PYTHONHOME 环境变量污染

**问题**：WPS 灵犀设置了 `PYTHONHOME=C:\Users\Administrator\AppData\Roaming\WPS 灵犀\python-env`，所有子进程继承此变量，导致 Hermes 等依赖自身 Python venv 的工具出现 SRE 模块不匹配。

**根因**：WPS 灵犀在 spawn 外部进程前未清除 `PYTHONHOME`。

**处理策略**：
- 不在 PilotDesk 中添加补丁
- 此问题应由 WPS 灵犀在源头修复（spawn 外部进程前清除 PYTHONHOME）
- 已在 PilotDesk 中移除所有 PYTHONHOME 补丁（2026-06-15）

### 13.2 Agent 版本检测规则

**决策**（2026-06-15 确定）：
- 所有 Agent 版本检测仅基于 CLI 版本，不再检测 Desktop 版本
- Claude Code: `claude --version`
- Hermes: `hermes --version`
- CodeX: `codex --version`

### 13.3 错误序列化

**问题**：WebSocket 错误消息中的 `error` 字段可能是 Error 对象而非字符串，导致前端显示 `[object Object]`。

**修复**：在相关文件中批量修复为 `err?.message || String(err)`。

### 13.4 Hermes 命令参数引号嵌套

**问题**：`shell: true` 模式下 Node.js 自动为参数加引号，手动加引号导致嵌套。

**修复**：移除手动引号。

### 13.5 SkillBrowser 轮询优化

**问题**：SkillBrowser 使用 500ms `setInterval` 轮询检测技能列表变化。

**修复**：改为 Zustand store 订阅模式。

### 13.6 useEnvInfo 轮询优化

**问题**：`useEnvInfo` 每 60 秒轮询环境信息。

**修复**：移除自动轮询，改为启动时获取 + 手动刷新。

### 13.7 CodeX --pipe 参数错误

**问题**：codex CLI 0.140.0-alpha.2 不支持 `--pipe` 参数。

**修复**：`codex --pipe` → `codex exec`。

### 13.8 CodeX 数据库 CHECK 约束缺失

**问题**：旧数据库的 sessions 表 CHECK 约束缺少 `'codex'`。

**修复**：`migrate_add_type()` 函数统一检查并重建表。

### 13.9 CSS 缺少 --codex-tag 变量

**问题**：`globals.css` 中缺少 `--codex-tag` CSS 变量定义。

**修复**：`:root` 和 `[data-theme="dark"]` 两处均添加。

### 13.10 Sidecar 消除后的遗留引用

**问题**：v4.0 消除 Sidecar 后，`SettingsPage.tsx:888` 仍有 "Node.js Sidecar + WebSocket" 文本引用。

**修复**：更新为 "Rust AgentManager + Tauri Event"。

### 13.11 前端代码去重

**问题**：`inspirationStore.ts` 和 `apiProviderStore.ts` 存在大量重复的 try-catch + invoke 模板代码。

**修复**：创建 `invokeHelper.ts` 提供 5 个通用函数，两个 store 合计精简 ~339 行。

### 13.12 消息去重逻辑脆弱

**问题**：v2.x 使用 2 秒时间戳窗口去重，不可靠。

**修复**：v3.0 改为 ID-based 去重（`Set<string>`），确定性方案。

### 13.13 MainPanel 状态管理复杂

**问题**：3 个 useState + 3 个 useRef 管理消息流状态，存在闭包陷阱。

**修复**：v4.0 改用 useReducer 状态机（8 种 Action），状态变更路径可追踪。

### 13.14 主题存储不一致

**问题**：主题模式存储在 `theme.txt` 文件，与其他设置存储在 SQLite 不一致。

**修复**：v4.0 迁移到 `app_settings` 表，含自动迁移回退。

---

## 14. 文件清单

### 14.1 Rust 后端

| 文件 | 行数（约） | 职责 |
|------|-----------|------|
| `src-tauri/src/main.rs` | ~20 | Tauri 入口 |
| `src-tauri/src/lib.rs` | ~200 | 模块注册 + 命令注册 + setup |
| `src-tauri/src/agent/mod.rs` | ~300 | AgentManager（统一进程管理） |
| `src-tauri/src/plugin/mod.rs` | ~550 | PluginHost（插件发现/管理/沙箱/zip安装/卸载） |
| `src-tauri/src/db/init.rs` | ~230 | 数据库初始化 + 7 个迁移函数 |
| `src-tauri/src/db/models.rs` | ~80 | 统一数据模型 |
| `src-tauri/src/commands/env.rs` | ~270 | 环境检测 + Agent 安装 |
| `src-tauri/src/commands/session.rs` | ~280 | 会话/消息 CRUD + 搜索 |
| `src-tauri/src/commands/update.rs` | ~170 | npm/pypi 版本检测 |
| `src-tauri/src/commands/api_provider.rs` | ~200 | API 提供商管理 |
| `src-tauri/src/commands/app_settings.rs` | ~80 | 应用设置 KV |
| `src-tauri/src/commands/install_log.rs` | ~74 | 安装日志 |
| `src-tauri/src/commands/inspiration.rs` | ~200 | 灵感 CRUD + FTS5 搜索 |
| `src-tauri/src/commands/theme.rs` | ~60 | 主题管理（SQLite 持久化） |
| `src-tauri/src/utils/mod.rs` | ~30 | new_id(), now(), now_millis() |
| `src-tauri/src/utils/paths.rs` | ~12 | 路径工具 |
| `src-tauri/src/utils/errors.rs` | ~90 | 错误类型枚举（9 变体） |
| `src-tauri/src/utils/crypto.rs` | ~130 | DPAPI 加密/解密 |

### 14.2 前端核心

| 文件 | 行数（约） | 职责 |
|------|-----------|------|
| `src/App.tsx` | ~60 | 根组件 + 路由 |
| `src/main.tsx` | ~20 | 入口 |
| `src/constants.ts` | ~30 | 共享常量 |
| `src/types/index.ts` | ~220 | 类型定义 + AGENT_THEMES |
| `src/types/plugin.ts` | ~50 | 插件系统类型定义 |
| `src/hooks/useAgentEvent.ts` | ~120 | Tauri Event 通信 hook |
| `src/hooks/useEnvInfo.ts` | ~50 | 环境信息共享 hook |
| `src/hooks/useTheme.ts` | ~80 | 主题切换（SQLite 持久化） |
| `src/hooks/useI18n.ts` | ~40 | 国际化 hook |
| `src/utils/invokeHelper.ts` | ~60 | 5 个通用 invoke 包装函数 |
| `src/utils/apiClient.ts` | ~80 | SSE 流式解析 |
| `src/stores/sessionStore.ts` | ~220 | 会话/消息状态 |
| `src/stores/apiProviderStore.ts` | ~80 | API 提供商状态 |
| `src/stores/inspirationStore.ts` | ~80 | 灵感状态 |
| `src/stores/pluginStore.ts` | ~100 | 插件状态 + registeredPanels/Commands/Hooks + refreshRegistrations |
| `src/stores/themeStore.ts` | ~50 | 自定义主题色状态 |
| `src/stores/skillStore.ts` | ~40 | 技能列表状态 |
| `src/stores/pendingInputStore.ts` | ~30 | 待发送输入状态 |

### 14.3 前端组件

| 文件 | 行数（约） | 职责 |
|------|-----------|------|
| `src/components/layout/MainPanel.tsx` | ~350 | 主面板（useReducer 状态机） |
| `src/components/layout/InputBar.tsx` | ~280 | 输入栏 |
| `src/components/layout/MessageList.tsx` | ~180 | 消息列表（虚拟滚动 + 搜索） |
| `src/components/layout/SessionList.tsx` | ~250 | 会话列表（搜索 + 批量操作） |
| `src/components/layout/SessionListItem.tsx` | ~100 | 会话项（批量选择 checkbox） |
| `src/components/layout/StatusBar.tsx` | ~80 | 状态栏 |
| `src/components/layout/TitleBar.tsx` | ~60 | 自定义标题栏 |
| `src/components/layout/RightPanel.tsx` | ~80 | 右侧面板（含插件标签页） |
| `src/components/layout/InspirationPanel.tsx` | ~150 | 灵感面板 |
| `src/components/message/MessageBubble.tsx` | ~350 | 消息气泡（内联编辑 + 重发） |
| `src/components/message/MarkdownRenderer.tsx` | ~100 | Markdown 渲染 |
| `src/components/plugin/PluginManager.tsx` | ~300 | 插件管理 UI（含 zip 安装/卸载/注册表集成） |
| `src/components/settings/ThemeCustomizer.tsx` | ~80 | 自定义主题色 UI |
| `src/components/env/EnvManager.tsx` | ~260 | 环境管理 |
| `src/pages/SettingsPage.tsx` | ~300 | 设置页 |
| `src/pages/EnvPage.tsx` | ~100 | 环境检测页 |
| `src/styles/globals.css` | ~250 | 全局样式 + 42 CSS 设计令牌 |
| `src/plugin/PluginRegistry.ts` | ~200 | 插件贡献点全局注册表 |
| `src/plugin/PluginAPI.ts` | ~100 | 插件运行时 API（ui/events/storage） |
| `src/components/plugin/PluginPanelRenderer.tsx` | ~80 | 插件面板渲染器 |
| `src/components/plugin/PluginIcon.tsx` | ~40 | 插件图标渲染组件 |
| `src/components/plugin/DefaultPluginPanel.tsx` | ~40 | 默认面板组件 |

### 14.4 文档

| 文件 | 说明 |
|------|------|
| `docs/PilotDesk-架构与技术实现-v4.4.md` | **本文档** |
| `docs/PilotDesk-架构与技术实现-v3.5.md` | 旧版架构文档（含 Sidecar 完整描述） |
| `docs/PilotDesk-架构与技术实现-v4.0.md` | v4.0 变更文档 |
| `docs/PilotDesk-架构与技术实现-v4.1.md` | v4.1 变更文档 |
| `docs/PilotDesk-架构与技术实现-v4.2.md` | v4.2 变更文档 |
| `docs/PilotDesk-架构与技术实现-v4.3.md` | v4.3 变更文档 |
| `docs/PilotDesk-插件系统架构设计-v1.0.md` | 插件系统设计文档 |
| `docs/PilotDesk-FunctionCalling-联网搜索技术方案-v1.0.md` | Function Calling 与联网搜索方案 |
| `docs/PilotDesk-内置搜索方案-必应国内版.md` | 必应搜索实现方案 |

### 14.5 已删除文件

以下文件在 v4.0 架构重构中已删除：

| 文件 | 行数（约） | 删除原因 |
|------|-----------|---------|
| `src-tauri/src/sidecar/manager.rs` | ~340 | Sidecar 进程管理 → AgentManager |
| `src-tauri/src/sidecar/mod.rs` | ~10 | 模块声明 |
| `src/hooks/useWebSocket.ts` | ~200 | WebSocket 连接管理 → useAgentEvent |
| `src/stores/wsStore.ts` | ~160 | WebSocket 状态 |
| `sidecar/` 目录（598 个文件） | - | 整个 Node.js Sidecar 项目 |

---

## 15. 变更记录

### 15.1 v4.6（本文档）

**日期**：2026-06-18

**变更内容**：
1. **StatusBar 优化**："Agent: 就绪" 改为纯文本 "Agent: "，三个 agent 版本号按钮新增 `onOpenEnvSettings` 回调，点击直接导航至设置页的环境检测 tab
2. **SettingsPage URL 参数支持**：新增 `useSearchParams` 读取 `?tab=environment` 参数，自动切换到环境检测 tab
3. **环境检测修复**：
   - `run_cmd` 中清除 `PYTHONHOME`/`PYTHONPATH` 环境变量，消除 WPS 灵犀环境变量污染
   - Python 检测新增 `py`（Python Launcher）作为 Windows fallback
   - 新增 `clear_env_detect_cache` Tauri 命令
   - `useEnvInfo` 的 `refresh` 回调改为返回 Promise
4. **npm 版本查询修复**：
   - `reqwest` 新增 `gzip` 和 `native-tls` 特性，修复 HTTPS 请求解码失败
   - HTTP 超时从 10s 增加到 30s
   - 错误消息简化为 `检查 {key} 更新失败`
5. **操作日志重构**：
   - 标题 "安装日志" → "操作日志"
   - 时间格式增加年月日（`yyyy/MM/dd HH:mm:ss`）
   - 查询改为 `ORDER BY timestamp DESC`，最新日志显示在最上方
   - 移除自动滚动到底部逻辑
   - 新增 `log-updated` 事件监听，每次写入日志后自动刷新列表
   - InstallLog 组件独立从数据库加载全部日志，不再依赖外部 props
6. **API 配置卡片拖拽优化**：拖动排序图标从卡片左侧独立容器移到卡片头部、API 供应商名称之前

### 15.2 v4.5

**日期**：2026-06-17

**变更内容**：
1. **PluginRegistry 重构**：移除订阅/通知机制，合并注册数据到 Zustand PluginStore，组件通过 selector 精确订阅
2. **插件 JS 执行支持**：新增 PluginAPI 运行时（ui.addPanel/showToast、events、storage），PluginRegistry 执行入口 JS 并调用 onLoad/onUnload 生命周期
3. **Rust 新增命令**：`plugin_read_entry`（读取入口文件）、`plugin_set_sandbox_enabled`（切换沙箱状态）、`plugin_get_panel_content`
4. **沙箱增强**：沙箱禁用时所有权限检查跳过；沙箱状态指示器动态显示启用/禁用
5. **面板渲染优化**：点击沙箱按钮隐藏插件列表仅显示沙箱信息；沙箱信息字体放大
6. **插件格式简化**：入口文件使用纯 JS（React.createElement 替代 JSX），无需 esbuild 转译
7. **插件图标系统**：新增 `pluginIcon.ts` 解析工具和 `PluginIcon.tsx` 渲染组件，支持网络图片地址和插件本地路径作为图标
8. **安全配置**：tauri.conf.json 新增 asset protocol 白名单（`$APPDATA/PilotDesk/plugins/**`），CSP 增加 `https:` 和 `asset:` 到 img-src
9. **Windows 路径兼容**：RegisteredPanel 新增 `pluginPath` 字段，修复 Windows 下路径解析 `split(':')[0]` 只取到盘符的问题
10. **示例插件更新**：hello-world/index.js 改为纯 JS 格式；README.md 补充 contributes 说明和生命周期章节
11. **文档更新**：插件系统架构设计文档状态改为"已实现"；架构文档 v4.4→v4.5

### 15.3 v4.4

**日期**：2026-06-16

**变更内容**：
1. 合并 v3.5 完整架构描述与 v4.3 最新状态为一份综合文档
2. 保留 v3.5 的完整结构（三层架构、数据库设计、状态管理、通信协议、关键问题记录、文件清单）
3. Sidecar 相关内容标注为"已废弃/已消除"，在"Agent 集成"章节展示架构演进
4. 新增插件系统、主题系统、国际化三个独立章节
5. 更新文件清单反映当前代码状态
6. **插件安全沙箱**：权限系统（9 种权限 + 3 级风险）、清单验证（7 项检查）、路径遍历防护、入口验证、未知权限拒绝、高风险权限标记
7. **示例插件**：hello-world 示例插件（含完整开发文档）+ malicious-sample 恶意插件示例（沙箱验证）
8. **前端插件入口**：PluginRegistry 全局注册表（面板/命令/钩子管理）、PluginPanelRenderer 面板渲染器、RightPanel 动态标签页集成
9. **本地上传安装**：plugin_install_zip / plugin_uninstall Rust 命令、zip 解压安装、沙箱验证、卸载时目录清理

### 15.4 v4.3

**日期**：2026-06-16

**变更内容**：
1. **插件系统运行时**：Rust PluginHost + 前端 PluginStore + PluginManager UI + RightPanel 集成
2. **自定义主题色集成**：themeStore + ThemeCustomizer + SettingsPage 集成

### 15.5 v4.2

**日期**：2026-06-16

**变更内容**：
1. **Phase 6**：消息重发功能、会话批量操作（批量归档/删除）、消息搜索（LIKE %query%）
2. **Phase 7 设计**：插件系统架构设计文档 + 类型定义、i18n 基础设施（hook + JSON 文件）、自定义主题色（themeStore + ThemeCustomizer）

### 15.6 v4.1

**日期**：2026-06-16

**变更内容**：
1. **Phase 4**：react-virtuoso 虚拟滚动、消息内联编辑、会话搜索（search_sessions 命令）
2. **Phase 5**：42 个 CSS 设计令牌系统、主题 SQLite 持久化（useTheme 重写）

### 15.7 v4.5

**日期**：2026-06-20

**变更内容**：
1. **资源目录架构**
   - 新增 `src-tauri/resources/` 目录（agents/ icons/ assets/）
   - 新增 `ResourcePaths` 结构体（Tauri State），管理内置/用户资源路径
   - 新增 `paths.rs` 中 `builtin_resources_dir()` / `user_resources_dir()` 函数
   - 打包配置 `tauri.conf.json bundle.resources`
2. **Agent 图标系统**
   - 三个内置 Agent 图标文件：`claude_icon.ico`、`hermes_icon.ico`、`codex_icon.ico`
   - 新增 `read_agent_icon` Rust 命令（读取图标文件返回 base64 data URL）
   - 新增 `AgentIcon` 前端组件（支持 file: 前缀/网络图片/Emoji，含 fallback 机制）
   - Agent 配置 icon 字段格式：`file:claude_icon.ico`
   - Agent 市场模板和数据库种子数据 icon 字段更新
   - `AgentBadge` 集成 AgentIcon（图标加载失败回退首字母）
   - `MessageBubble` Assistant 消息集成 AgentIcon（圆形头像区域）
   - `MessageBubble` User 消息标记为待实施项（后续用户系统接入）
3. **Agent 表单优化**
   - 字段按 6 个分类分组（基础信息/包参数/会话参数/延续会话/生命周期/技能引用）
   - 分类标题加粗 + 主题色图标
   - 输入框 placeholder 统一为具体示例
   - 图标字段 label 补全值类型说明

### 15.8 v4.0

**日期**：2026-06-16

**变更内容**：
1. **Phase 1：消除 Sidecar 架构负债**
   - 创建 Rust AgentManager（tokio::process + Tauri Event）
   - 删除 sidecar/ 目录（598 个文件）
   - 删除 useWebSocket.ts、wsStore.ts
   - 创建 useAgentEvent.ts 替代 WebSocket
2. **Phase 2：后端架构改造**
   - r2d2 连接池替代 Mutex<Connection>
   - MainPanel useReducer 状态机替代 3 useState + 3 useRef
   - 主题存储迁移（theme.txt → app_settings 表）
   - 时间戳统一（f64 → i64）
   - 错误处理增强（新增 Json 变体 + r2d2 From 实现）
3. **Phase 3：前端代码去重**
   - invokeHelper.ts（5 个通用函数）
   - constants.ts（EMOJI_OPTIONS 共享常量）
   - inspirationStore.ts 精简 ~135 行
   - apiProviderStore.ts 精简 ~204 行

### 15.9 v3.5

**日期**：2026-06-16

**变更内容**：
1. UUID/now 辅助函数提取（消除 6 个文件中 17 处重复代码）
2. 结构化日志（println! → log::info!/warn!/error!）
3. DPAPI 密钥保护（Windows CryptProtectData）
4. sessionStore 竞态修复（会话存在性检查）

### 15.10 v3.4

**日期**：2026-06-16

**变更内容**：
1. Message 扩展字段持久化（reasoning_content, tool_calls, tool_call_id, tool_name）
2. Sidecar 启动失败 emit 事件到前端
3. WebSocket 断连发送队列
4. ENV_CACHE RwLock（Mutex → RwLock）
5. 前端 HashRouter 路由
6. 硬编码 Node.js 路径替换为动态探测
7. bot.rs 默认值去重

---

> PilotDesk 架构与技术实现 v4.5 | 2026-06-20

## 版本里程碑

| 版本 | 日期 | 变更说明 | Git Commit |
|------|------|---------|------------|
| v4.5 | 2026-06-20 | 架构与技术实现文档 v4.5 | `f38849d (f38849dce2eb732d60cdac42b9dbda5b03000479)` |
