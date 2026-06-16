# PilotDesk 架构与技术实现 v4.1

> 更新时间：2026-06-16
> 版本：v4.1
> 变更：虚拟滚动、消息编辑、会话搜索、CSS 设计令牌系统、主题 SQLite 持久化

---

## 1. 项目概述

PilotDesk 是一个基于 **Tauri 2.0** 的桌面应用，作为 Claude Code、Hermes Agent、Codex 三种 AI Agent 的统一桌面客户端。用户可以在统一的界面中创建会话、切换 Agent、管理灵感/技能/记忆，并通过 API 直连模式使用任意兼容的 LLM 提供商。

### 1.1 技术栈

| 层 | 技术 | 版本 | 说明 |
|---|------|------|------|
| 桌面框架 | Tauri 2.0 | 2.x | 跨平台桌面容器，Rust 后端 + WebView 前端 |
| 前端框架 | React 19 | 19.x | UI 渲染 |
| 前端状态 | Zustand | 5.x | 轻量状态管理 |
| 虚拟滚动 | react-virtuoso | 4.x | 消息列表虚拟化 |
| 构建工具 | Vite | 8.x | 前端构建 |
| 后端语言 | Rust | 2021 edition | 系统级能力 |
| 数据库 | SQLite (rusqlite) | 0.32 | 本地持久化 |
| 连接池 | r2d2 + r2d2_sqlite | 0.8 / 0.25 | 数据库并发连接 |
| 子进程 | tokio::process | 1.x | 异步 Agent 进程管理 |
| 前后端通信 | Tauri IPC (invoke + Event) | 2.x | 替代 WebSocket |
| 加密 | aes-gcm + base64 | 0.10 / 0.22 | API Key 加密存储 |

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
│  │    InspirationPanel  │  │    ├── api_provider       │  │
│  │    MessageList       │  │    ├── bot                │  │
│  │      (Virtuoso)      │  │    ├── env                │  │
│  │    MessageBubble     │  │    ├── theme              │  │
│  │      (inline edit)   │  │    ├── update             │  │
│  │    ...               │  │    ├── install_log        │  │
│  │                      │  │    └── app_settings       │  │
│  │  Stores/             │  │                           │  │
│  │    sessionStore      │  │  agent/                   │  │
│  │    inspirationStore  │  │    mod.rs (AgentManager)  │  │
│  │    apiProviderStore  │  │      ├── AgentType enum   │  │
│  │    skillStore        │  │      ├── spawn/kill       │  │
│  │                      │  │      ├── output parser    │  │
│  │  Hooks/              │  │      └── error mapper     │  │
│  │    useAgentEvent     │  │                           │  │
│  │    useTheme (SQLite) │  │  db/                      │  │
│  │    useEnvInfo        │  │    init.rs (migrations)   │  │
│  │                      │  │    models.rs (structs)    │  │
│  │  Utils/              │  │                           │  │
│  │    invokeHelper      │  │  utils/                   │  │
│  │    apiClient         │  │    errors.rs (AppError)   │  │
│  │    toast             │  │    crypto.rs (DPAPI)      │  │
│  │                      │  │    paths.rs               │  │
│  │  Types/              │  │                           │  │
│  │    index.ts          │  └──────────┬────────────────┘  │
│  │                      │             │                   │
│  │  Styles/             │  Tauri IPC (invoke)             │
│  │    globals.css       │  Tauri Event (stream)           │
│  │      (Design Tokens) │                                  │
│  └─────────┬───────────┘                                  │
│            │              ┌──────────────────────────┐    │
│            │              │  SQLite (WAL mode)        │    │
│            │              │  r2d2 Pool (max 8 conn)  │    │
│            │              └──────────────────────────┘    │
│            ▼                                               │
│  ┌──────────────────────────────────────────┐             │
│  │  Agent 子进程 (tokio::process)            │             │
│  │  ┌─────────┐  ┌──────────┐  ┌─────────┐  │             │
│  │  │ Claude  │  │  Hermes  │  │  Codex  │  │             │
│  │  │  Code   │  │  Agent   │  │         │  │             │
│  │  └─────────┘  └──────────┘  └─────────┘  │             │
│  └──────────────────────────────────────────┘             │
└─────────────────────────────────────────────────────────┘
```

---

## 3. Rust 后端架构

### 3.1 模块结构

```
src-tauri/src/
├── lib.rs              # 应用入口，Tauri command 注册
├── main.rs             # 启动入口
├── agent/
│   └── mod.rs          # AgentManager — 统一 Agent 进程管理
├── commands/
│   ├── mod.rs          # 模块声明
│   ├── session.rs      # 会话 CRUD + 搜索 + 消息编辑
│   ├── inspiration.rs  # 灵感 CRUD + FTS5 搜索
│   ├── api_provider.rs # API 提供商管理
│   ├── bot.rs          # Bot 频道管理
│   ├── env.rs          # 环境检测与 Agent 安装
│   ├── theme.rs        # 主题设置（app_settings 表）
│   ├── update.rs       # 版本更新检查
│   ├── install_log.rs  # 安装日志
│   └── app_settings.rs # 应用设置 KV 存储
├── db/
│   ├── mod.rs          # 模块声明
│   ├── init.rs         # 数据库初始化 + 版本化迁移
│   └── models.rs       # 数据结构定义
├── utils/
│   ├── mod.rs          # 工具函数（new_id, now, now_millis）
│   ├── errors.rs       # AppError 统一错误类型
│   ├── crypto.rs       # AES-256-GCM + DPAPI 加密
│   └── paths.rs        # 路径解析
```

### 3.2 新增命令

**v4.1 新增：**

```rust
// 消息编辑 — 更新已有消息内容
#[tauri::command]
pub fn update_message(state, message_id, content) -> Result<Message, AppError>;

// 会话搜索 — 按标题模糊匹配
#[tauri::command]
pub fn search_sessions(state, query) -> Result<Vec<Session>, AppError>;
```

---

## 4. 前端架构

### 4.1 组件树

```
App
├── TitleBar                  # 窗口标题栏 + 菜单
├── SessionList               # 左侧会话列表（含搜索）
│   └── SessionListItem       # 单个会话项
├── MainPanel                 # 主聊天面板
│   ├── MessageList           # 消息列表（Virtuoso 虚拟滚动）
│   │   └── MessageBubble     # 消息气泡（支持内联编辑）
│   │       └── MarkdownRenderer  # Markdown 渲染
│   └── InputBar              # 输入栏
├── RightPanel                # 右侧面板
│   ├── InspirationPanel      # 灵感面板
│   ├── SkillBrowser          # 技能浏览器
│   └── MemoryBrowser         # 记忆浏览器
└── StatusBar                 # 底部状态栏
```

### 4.2 消息列表虚拟滚动

```typescript
// MessageList.tsx — 使用 react-virtuoso
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso';

export function MessageList({ messages, ... }) {
  const virtuosoRef = useRef<VirtuosoHandle>(null);

  // 自动滚动到底部
  useEffect(() => {
    if (messages.length > prevLengthRef.current) {
      virtuosoRef.current?.scrollToIndex({ index: messages.length - 1, behavior: 'smooth' });
    }
  }, [messages.length]);

  return (
    <Virtuoso
      ref={virtuosoRef}
      data={messages}
      itemContent={itemContent}
      followOutput="smooth"
      increaseViewportBy={{ top: 200, bottom: 200 }}
      components={{ Footer: TypingIndicator }}
    />
  );
}
```

**收益：** 仅渲染可见区域的消息 DOM 节点，消息数量从 O(n) DOM 节点降为 O(视口高度/消息高度)，长对话性能提升 10-100 倍。

### 4.3 消息内联编辑

```typescript
// MessageBubble.tsx — 用户消息支持内联编辑
const [isEditing, setIsEditing] = useState(false);
const [editContent, setEditContent] = useState('');

// 编辑模式：textarea 替代纯文本展示
// 快捷键：Enter 保存，Escape 取消
// 后端：invoke('update_message', { messageId, content })
```

**流程：** 用户点击"修改" → textarea 替换文本 → 编辑内容 → Enter 保存 → invoke Rust → 更新 SQLite → Zustand store 更新 → 重新渲染。

### 4.4 会话搜索

```typescript
// SessionList.tsx — 防抖搜索
const [searchQuery, setSearchQuery] = useState('');
const [searchResults, setSearchResults] = useState<typeof sessions>([]);

// 300ms 防抖后调用后端模糊搜索
useEffect(() => {
  if (!searchQuery.trim()) { setSearchResults([]); return; }
  debounce(async () => {
    const results = await invoke('search_sessions', { query });
    setSearchResults(results);
  }, 300);
}, [searchQuery]);
```

**后端：** `SELECT ... FROM sessions WHERE title LIKE '%query%' ORDER BY updated_at DESC LIMIT 50`

---

## 5. CSS 设计令牌系统

### 5.1 令牌分类

| 类别 | 前缀 | 示例 | 用途 |
|------|------|------|------|
| 背景 | `--bg-*` | `--bg-primary`, `--bg-hover` | 所有背景色 |
| 文本 | `--text-*` | `--text-primary`, `--text-inverse` | 所有文本色 |
| 边框 | `--border*` | `--border`, `--border-light` | 边框和分割线 |
| 强调 | `--accent*` | `--accent`, `--accent-hover` | 交互元素 |
| Agent 标签 | `--tag-*` | `--tag-claude`, `--tag-hermes` | Agent 类型标识 |
| 状态 | `--status-*` | `--status-success`, `--status-danger` | 反馈状态 |
| 圆角 | `--radius-*` | `--radius-sm`, `--radius-full` | 所有 border-radius |
| 阴影 | `--shadow-*` | `--shadow-sm`, `--shadow-lg` | 所有 box-shadow |
| 字体 | `--font-*` | `--font-sans`, `--font-mono` | font-family |
| 尺寸 | `--*-width/height` | `--sidebar-width` | 布局尺寸 |

### 5.2 使用规范

```css
/* 正确：使用设计令牌 */
.button {
  background-color: var(--accent);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-sm);
}

/* 禁止：硬编码值 */
.button {
  background-color: #2563EB;        /* ✗ */
  border-radius: 8px;               /* ✗ */
}
```

### 5.3 主题持久化

```typescript
// useTheme.ts — SQLite 替代 localStorage
useEffect(() => {
  const saved = await invoke('get_app_setting', { key: 'theme' });
  if (saved) setThemeState(saved as Theme);
}, []);

const setTheme = async (t: Theme) => {
  await invoke('set_app_setting', { key: 'theme', value: t });
};
```

**收益：** 主题设置与后端一致，多窗口/多进程场景下主题同步。

---

## 6. 架构评估

### 6.1 合理性

- **职责边界清晰**：Rust 后端负责系统级操作（进程管理、数据库、加密），React 前端负责 UI 渲染和交互逻辑，严格通过 Tauri IPC 通信
- **分层合理**：`commands/` 层处理 IPC 请求，`agent/` 层管理 Agent 生命周期，`db/` 层处理持久化，`utils/` 层提供通用能力
- **状态管理一致**：所有 store 遵循相同的 fetch→invoke→set 模式，新开发者可快速上手

### 6.2 可移植性

- **跨平台基础**：Tauri 2.0 天然支持 Windows/macOS/Linux
- **数据库无关**：SQLite 通过 `r2d2_sqlite` 抽象，更换数据库只需替换 manager 实现
- **Agent 无关**：`AgentType` 枚举将 Agent 差异封装在 4 个方法中，新增 Agent 不影响现有流程
- **前端无关**：React 组件通过 `useAgentEvent` hook 与后端解耦，替换 UI 框架只需重写组件层

### 6.3 可扩展性

- **新增 Agent**：添加 `AgentType` 枚举变体 + 实现 4 个方法，无需修改任何其他文件
- **新增命令**：在 `commands/` 下新建文件，在 `lib.rs` 中注册 `#[tauri::command]` 和 `generate_handler!`
- **新增数据表**：在 `init.rs` 中添加建表语句 + 迁移函数，递增 `MIGRATION_VERSION`
- **新增 UI 面板**：在 `components/` 下新建组件，在 `RightPanel` 或路由中注册
- **连接池扩容**：修改 `Pool::builder().max_size(N)` 即可

### 6.4 可维护性

- **代码量精简**：Rust 后端 ~53KB，前端 ~70KB，无遗留负债
- **无宏抽象**：全部使用直接 `#[tauri::command]` 函数，IDE 支持完整
- **统一错误处理**：`AppError` 枚举 + 4 个 `From` 实现，所有命令返回一致错误类型
- **版本化迁移**：`PRAGMA user_version` 追踪迁移状态，支持增量升级
- **状态机可追踪**：`useReducer` 的 action 类型枚举所有状态变更路径
- **CSS 设计令牌**：所有样式值通过语义化变量引用，修改主题只需更改变量定义

### 6.5 代码复用性

- **invokeHelper.ts**：5 个通用函数消除所有 store 中重复的 invoke 调用模式
- **constants.ts**：共享常量（EMOJI_OPTIONS）消除跨文件重复定义
- **AgentType 枚举**：进程管理、参数构建、输出解析、错误映射全部复用同一套流程
- **AppError**：8 个命令模块共享同一错误类型，前端统一处理
- **sessionStore**：ID-based 去重逻辑可复用于其他列表型 store

### 6.6 高效性

- **数据库并发**：r2d2 连接池（max 8）替代 Mutex 串行化，读写不互斥
- **虚拟滚动**：react-virtuoso 仅渲染可见 DOM 节点，长对话性能提升 10-100 倍
- **流式渲染**：Tauri Event 推送 + React 增量渲染，用户即时看到输出
- **消息去重**：`Set<string>` 是 O(1) 操作，替代 O(n) 遍历
- **Store 选择器**：Zustand 自动跳过无关更新，只有订阅的字段变化时重渲染
- **WAL 模式**：SQLite WAL 允许并发读写，写入不阻塞读取
- **零序列化开销**：Tauri Event 直接传递 Rust 字符串到前端，无 JSON 序列化/反序列化

---

## 7. 版本演进

| 版本 | 核心变更 | 日期 |
|------|---------|------|
| v1.0 | 初始架构：Node.js Sidecar + WebSocket | - |
| v2.0 | 引入 AgentType 枚举，重构 Adapter | - |
| v3.0 | 前端 Zustand 状态管理，ID-based 去重 | - |
| v3.5 | UUID 辅助函数，DPAPI 加密，结构化日志 | - |
| v4.0 | **消除 Sidecar**：AgentManager + Tauri Event + r2d2 连接池 + useReducer 状态机 + 统一时间戳 | 2026-06-16 |
| v4.1 | 虚拟滚动 + 消息编辑 + 会话搜索 + CSS 设计令牌 + 主题 SQLite 持久化 | 2026-06-16 |

---

## 8. 未来规划

### Phase 6 (待实施)
- 消息重发功能（基于编辑 + 重新发送）
- 会话批量操作（批量归档/删除）
- 消息搜索（FTS5 全文搜索消息内容）

### Phase 7 (待实施)
- 插件系统架构设计
- 多语言支持（i18n）
- 自定义主题色

---

*本文档对应代码版本：PilotDesk v0.1.0，基于 Tauri 2.0 + React 19 + Zustand + r2d2/SQLite + react-virtuoso*
