# PilotDesk 架构与技术实现 v4.2

> 更新时间：2026-06-16
> 版本：v4.2
> 变更：消息重发、会话批量操作、消息搜索、插件系统架构、i18n 多语言、自定义主题色

---

## 1. 项目概述

PilotDesk 是一个基于 **Tauri 2.0** 的桌面应用，作为 Claude Code、Hermes Agent、Codex 三种 AI Agent 的统一桌面客户端。

### 1.1 技术栈

| 层 | 技术 | 版本 |
|---|------|------|
| 桌面框架 | Tauri 2.0 | 2.x |
| 前端框架 | React 19 | 19.x |
| 前端状态 | Zustand | 5.x |
| 虚拟滚动 | react-virtuoso | 4.x |
| 构建工具 | Vite | 8.x |
| 后端语言 | Rust | 2021 edition |
| 数据库 | SQLite (rusqlite) | 0.32 |
| 连接池 | r2d2 + r2d2_sqlite | 0.8 / 0.25 |
| 子进程 | tokio::process | 1.x |
| 前后端通信 | Tauri IPC (invoke + Event) | 2.x |
| 加密 | aes-gcm + base64 | 0.10 / 0.22 |

---

## 2. 架构总览

```
┌─────────────────────────────────────────────────────────┐
│                    PilotDesk Desktop                     │
│                                                          │
│  ┌─────────────────────┐  ┌──────────────────────────┐  │
│  │   React Frontend     │  │   Rust Backend            │  │
│  │                      │  │                           │  │
│  │  Components/         │  │  lib.rs (commands)        │  │
│  │    MainPanel         │  │    ├── session            │  │
│  │    SessionList       │  │    ├── inspiration        │  │
│  │      (batch ops)     │  │    ├── api_provider       │  │
│  │    MessageList       │  │    ├── bot                │  │
│  │      (Virtuoso)      │  │    ├── env                │  │
│  │      (msg search)    │  │    ├── theme              │  │
│  │    MessageBubble     │  │    ├── update             │  │
│  │      (inline edit)   │  │    ├── install_log        │  │
│  │      (resend)        │  │    └── app_settings       │  │
│  │    ThemeCustomizer   │  │                           │  │
│  │    ...               │  │  agent/                   │  │
│  │                      │  │    mod.rs (AgentManager)  │  │
│  │  Stores/             │  │                           │  │
│  │    sessionStore      │  │  db/                      │  │
│  │    inspirationStore  │  │    init.rs (migrations)   │  │
│  │    apiProviderStore  │  │    models.rs (structs)    │  │
│  │    themeStore        │  │                           │  │
│  │    skillStore        │  │  utils/                   │  │
│  │                      │  │    errors.rs (AppError)   │  │
│  │  Hooks/              │  │    crypto.rs (DPAPI)      │  │
│  │    useAgentEvent     │  │    paths.rs               │  │
│  │    useTheme (SQLite) │  │                           │  │
│  │    useI18n           │  └──────────────────────────┘  │
│  │    useEnvInfo        │                                │
│  │                      │                                │
│  │  Locales/            │  ┌──────────────────────────┐  │
│  │    zh-CN.json        │  │  SQLite (WAL mode)        │  │
│  │    en-US.json        │  │  r2d2 Pool (max 8 conn)  │  │
│  │                      │  └──────────────────────────┘  │
│  │  Types/              │                                │
│  │    index.ts          │  ┌──────────────────────────┐  │
│  │    plugin.ts         │  │  Agent 子进程             │  │
│  │                      │  │  (tokio::process)        │  │
│  │  Styles/             │  └──────────────────────────┘  │
│  │    globals.css       │                                │
│  │      (Design Tokens) │                                │
│  └─────────────────────┘                                │
└─────────────────────────────────────────────────────────┘
```

---

## 3. Phase 6 — 消息流优化

### 3.1 消息重发

用户消息气泡增加"重发"按钮，点击后直接将消息内容填入输入框，用户可修改后重新发送。

**数据流：**
```
MessageBubble.onResend → MessageList.onResendMessage
  → MainPanel: dispatch(SET_PENDING_INPUT)
    → InputBar 自动填充消息
      → 用户修改 → 点击发送
```

### 3.2 会话批量操作

SessionList 增加批量模式，支持：
- 进入/退出批量模式
- 全选/取消全选
- 批量归档（调用 `archive_session`）
- 批量删除（调用 `delete_session`）

SessionListItem 在批量模式下显示复选框，点击切换选中状态。

### 3.3 消息搜索

MessageList 顶部增加搜索栏，支持按消息内容模糊搜索：

```rust
#[tauri::command]
pub fn search_messages(state, session_id, query, limit) -> Result<Vec<Message>, AppError>;
```

- 可选按 session 过滤（当前会话）或全局搜索
- `LIKE %query%` 模糊匹配
- 搜索结果直接替换消息列表显示

---

## 4. Phase 7 — 扩展能力

### 4.1 插件系统架构

**设计文档：** `docs/PilotDesk-插件系统架构设计-v1.0.md`

**类型定义：** `src/types/plugin.ts`

核心概念：
- **PluginManifest**：插件清单（id, name, version, permissions, entry）
- **PluginAPI**：插件可用的核心 API（ui, data, events, storage）
- **PluginInstance**：插件运行时状态
- **生命周期**：发现 → 加载 → 初始化 → 运行 → 卸载

权限模型：
| 权限 | 说明 |
|------|------|
| `ui:panel` | 添加/移除面板 |
| `ui:toast` | 显示通知 |
| `session:read` | 读取会话 |
| `session:write` | 修改会话 |
| `data:invoke` | 调用 Tauri 命令 |
| `storage:*` | 插件独立存储 |

### 4.2 多语言支持（i18n）

**实现：** `src/hooks/useI18n.ts` + `src/locales/{zh-CN,en-US}.json`

- 轻量实现，无第三方依赖
- JSON 文件存储翻译，动态 import 按需加载
- 语言偏好持久化到 localStorage
- 支持参数插值（`{count}`）

**支持的语言：**
- `zh-CN` — 简体中文（默认）
- `en-US` — 英文

### 4.3 自定义主题色

**实现：** `src/stores/themeStore.ts` + `src/components/settings/ThemeCustomizer.tsx`

- 9 种预设颜色 + 自定义颜色选择器
- 颜色持久化到 SQLite `app_settings` 表
- 自动计算 `accentHover`（变暗 20）和 `accentLight`（15% 透明度）
- 通过 CSS 变量 `--accent`、`--accent-hover`、`--accent-light` 即时生效
- 重置功能恢复默认蓝色主题

---

## 5. 版本演进

| 版本 | 核心变更 | 日期 |
|------|---------|------|
| v1.0 | 初始架构：Node.js Sidecar + WebSocket | - |
| v2.0 | AgentType 枚举 | - |
| v3.0 | Zustand + ID-based 去重 | - |
| v3.5 | DPAPI 加密 + 结构化日志 | - |
| v4.0 | 消除 Sidecar + r2d2 + useReducer | 2026-06-16 |
| v4.1 | 虚拟滚动 + 消息编辑 + 设计令牌系统 | 2026-06-16 |
| v4.2 | 消息重发 + 批量操作 + 消息搜索 + 插件架构 + i18n + 自定义主题色 | 2026-06-16 |

---

## 6. 架构评估

### 6.1 合理性
职责边界清晰，Rust 后端负责系统级操作，React 前端负责 UI 渲染，严格通过 Tauri IPC 通信。

### 6.2 可移植性
Tauri 2.0 天然跨平台，AgentType 枚举封装 Agent 差异，i18n 支持多语言。

### 6.3 可扩展性
插件系统提供标准化扩展点，新增 Agent 只需添加枚举变体，新增命令只需在 commands/ 下新建文件。

### 6.4 可维护性
无宏抽象，统一错误处理，版本化迁移，CSS 设计令牌系统，useReducer 状态机可追踪。

### 6.5 代码复用性
invokeHelper 消除重复 invoke 调用，constants 共享常量，AgentType 复用进程管理流程。

### 6.6 高效性
r2d2 连接池并发读写，Virtuoso 虚拟滚动仅渲染可见 DOM，Tauri Event 零序列化流式推送。

---

*本文档对应代码版本：PilotDesk v0.1.0*
