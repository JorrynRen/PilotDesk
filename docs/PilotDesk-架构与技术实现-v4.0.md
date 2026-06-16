# PilotDesk 架构与技术实现 v4.0

> 更新时间：2026-06-16
> 版本：v4.0
> 变更：消除 Sidecar 架构负债，引入 r2d2 连接池，重构前端状态管理

---

## 1. 项目概述

PilotDesk 是一个基于 **Tauri 2.0** 的桌面应用，作为 Claude Code、Hermes Agent、Codex 三种 AI Agent 的统一桌面客户端。用户可以在统一的界面中创建会话、切换 Agent、管理灵感/技能/记忆，并通过 API 直连模式使用任意兼容的 LLM 提供商。

### 1.1 技术栈

| 层 | 技术 | 版本 | 说明 |
|---|------|------|------|
| 桌面框架 | Tauri 2.0 | 2.x | 跨平台桌面容器，Rust 后端 + WebView 前端 |
| 前端框架 | React 19 | 19.x | UI 渲染 |
| 前端状态 | Zustand | 5.x | 轻量状态管理 |
| 构建工具 | Vite | 6.x | 前端构建 |
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
│  │    ...               │  │    ├── env                │  │
│  │                      │  │    ├── theme              │  │
│  │  Stores/             │  │    ├── update             │  │
│  │    sessionStore      │  │    ├── install_log        │  │
│  │    inspirationStore  │  │    └── app_settings       │  │
│  │    apiProviderStore  │  │                           │  │
│  │    skillStore        │  │  agent/                   │  │
│  │                      │  │    mod.rs (AgentManager)  │  │
│  │  Hooks/              │  │      ├── AgentType enum   │  │
│  │    useAgentEvent     │  │      ├── spawn/kill       │  │
│  │    useTheme          │  │      ├── output parser    │  │
│  │    useEnvInfo        │  │      └── error mapper     │  │
│  │                      │  │                           │  │
│  │  Utils/              │  │  db/                      │  │
│  │    invokeHelper      │  │    init.rs (migrations)   │  │
│  │    apiClient         │  │    models.rs (structs)    │  │
│  │    toast             │  │                           │  │
│  │                      │  │  utils/                   │  │
│  │  Types/              │  │    errors.rs (AppError)   │  │
│  │    index.ts          │  │    crypto.rs (DPAPI)      │  │
│  └─────────┬───────────┘  │    paths.rs               │  │
│            │              └──────────┬────────────────┘  │
│            │  Tauri IPC (invoke)     │                   │
│            │  Tauri Event (stream)   │                   │
│            ▼                         ▼                   │
│  ┌──────────────────────────────────────────┐            │
│  │            SQLite (WAL mode)              │            │
│  │        r2d2 Pool (max 8 conn)            │            │
│  └──────────────────────────────────────────┘            │
│                                                          │
│  ┌──────────────────────────────────────────┐            │
│  │         Agent 子进程 (tokio::process)     │            │
│  │  ┌─────────┐  ┌──────────┐  ┌─────────┐  │            │
│  │  │ Claude  │  │  Hermes  │  │  Codex  │  │            │
│  │  │  Code   │  │  Agent   │  │         │  │            │
│  │  └─────────┘  └──────────┘  └─────────┘  │            │
│  └──────────────────────────────────────────┘            │
└─────────────────────────────────────────────────────────┘
```

### 2.1 核心架构决策

| 决策 | 选择 | 替代方案 | 理由 |
|------|------|---------|------|
| 进程管理 | Rust `tokio::process::Command` | Node.js Sidecar | 消除序列化层、减少崩溃点、减少 72% 代码 |
| 流式通信 | Tauri Event | WebSocket | 原生 IPC、无需额外端口、无序列化开销 |
| 数据库 | r2d2 连接池 | Mutex<Connection> | 并发读写不互斥、支持 8 连接并行 |
| 状态管理 | Zustand | Redux / Context | 零样板代码、选择器自动优化重渲染 |
| 消息去重 | ID-based Set | 2 秒时间戳窗口 | 确定性去重、无竞态条件 |
| Agent 抽象 | AgentType 枚举 | 3 个独立 Adapter | 新增 Agent 只需添加枚举变体 |
| 主题存储 | app_settings 表 | theme.txt 文件 | 统一存储入口、支持迁移 |

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
│   ├── session.rs      # 会话 CRUD
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
└── sidecar/            # [已删除] 旧 Sidecar 架构
```

### 3.2 AgentManager 设计

```rust
// agent/mod.rs 核心结构
pub enum AgentType {
    Claude, Hermes, Codex,
}

impl AgentType {
    pub fn cli_command(&self) -> &'static str;  // 可执行文件名称
    pub fn build_args(&self, message, mode, system_prompt) -> Vec<String>;  // CLI 参数
    pub fn parse_output_line(&self, line) -> Option<String>;  // 输出解析
    pub fn friendly_error(&self, exit_code, stderr) -> String;  // 错误映射
}

pub struct AgentManager {
    sessions: HashMap<String, AgentSession>,
}

struct AgentSession {
    child: Option<Child>,
    cancel_flag: Arc<AtomicBool>,
    agent_type: AgentType,
}
```

**关键设计点：**
- `AgentType` 枚举封装所有 Agent 差异（CLI 命令、参数、输出格式、错误信息）
- `cancel_flag` 使用 `Arc<AtomicBool>` 实现跨线程取消
- 输出通过 `tauri::Emitter::emit` 推送 Tauri Event，前端通过 `listen()` 接收
- 新增 Agent 只需添加枚举变体 + 实现对应方法

### 3.3 数据库层

```rust
// 连接池管理
pub type DbPool = Pool<SqliteConnectionManager>;

pub struct DbState {
    pub pool: DbPool,
}

impl DbState {
    pub fn get_conn(&self) -> Result<PooledConnection<...>, AppError> {
        self.pool.get().map_err(|e| AppError::Lock(...))
    }
}
```

**表结构：**
- `sessions` — 会话（agent_type, title, cwd, status, api_provider, api_model）
- `messages` — 消息（role, content, mode, reasoning_content, tool_calls）
- `inspirations` — 灵感（icon, title, content, source_agent, is_favorite）
- `inspiration_tags` — 灵感标签
- `inspirations_fts` — FTS5 全文索引
- `bot_channels` — Bot 频道配置
- `api_providers` — API 提供商（含加密 API Key）
- `app_settings` — 应用设置 KV 存储
- `install_logs` — 安装日志

**迁移策略：** 版本化迁移，通过 `PRAGMA user_version` 追踪，当前版本 v6。

### 3.4 统一错误处理

```rust
pub enum AppError {
    Db(String), Io(String), Lock(String), NotFound(String),
    InvalidInput(String), External(String), Config(String),
    Network(String), Json(String),
}
```

所有 DB 命令函数返回 `Result<T, AppError>`，通过 `From` 自动转换：
- `rusqlite::Error` → `AppError::Db`
- `std::io::Error` → `AppError::Io`
- `r2d2::Error` → `AppError::Lock`
- `serde_json::Error` → `AppError::Json`

---

## 4. 前端架构

### 4.1 组件树

```
App
├── TitleBar                  # 窗口标题栏 + 菜单
├── SessionList               # 左侧会话列表
│   └── SessionListItem       # 单个会话项
├── MainPanel                 # 主聊天面板
│   ├── MessageList           # 消息列表
│   │   └── MessageBubble     # 消息气泡
│   │       └── MarkdownRenderer  # Markdown 渲染
│   └── InputBar              # 输入栏
├── RightPanel                # 右侧面板
│   ├── InspirationPanel      # 灵感面板
│   │   ├── InspirationCard   # 灵感卡片
│   │   └── InspirationForm   # 灵感编辑表单
│   ├── SkillBrowser          # 技能浏览器
│   └── MemoryBrowser         # 记忆浏览器
└── StatusBar                 # 底部状态栏
```

### 4.2 状态管理

所有 store 使用 Zustand，遵循统一模式：

```
Store 模式:
  1. 状态定义 (items, loading, error)
  2. 异步 action (fetch, create, update, delete)
  3. 同步 action (setFilter, toggleFlag)
  4. 副作用分离 (UI 更新 + 异步持久化分离)
```

| Store | 职责 | 关键特性 |
|-------|------|---------|
| `sessionStore` | 会话 + 消息管理 | ID-based 去重，持久化分离 |
| `inspirationStore` | 灵感 CRUD + 搜索 | 标签过滤，FTS5 搜索 |
| `apiProviderStore` | API 提供商管理 | 加密 Key 存储 |
| `skillStore` | Agent 技能缓存 | 按 agentType 分组 |
| `pendingInputStore` | 跨组件输入传递 | 极简单值 store |

### 4.3 消息流状态机 (useReducer)

```typescript
type Action =
  | { type: 'SEND_START'; status: string }
  | { type: 'APPEND_CHUNK'; content: string }
  | { type: 'GENERATION_DONE'; sessionId: string }
  | { type: 'GENERATION_ERROR'; sessionId: string; error: string }
  | { type: 'GENERATION_TIMEOUT'; sessionId: string }
  | { type: 'STOP_GENERATION'; sessionId: string }
  | { type: 'CLEAR_PENDING_COMPLETE' }
  | { type: 'SET_PENDING_INPUT'; content: string | null };
```

替代了原来的 3 个 `useState` + 3 个 `useRef`，状态变更集中可追踪。

### 4.4 Agent 通信 (useAgentEvent)

```
前端                           Rust 后端
  │                              │
  │── invoke("agent_send_message") ──→  spawn Agent 子进程
  │                              │
  │←── listen("agent-chunk") ──────  stdout 逐行推送
  │←── listen("agent-done") ───────  进程正常退出
  │←── listen("agent-error") ──────  进程异常退出
  │                              │
  │── invoke("agent_stop_generation") →  AtomicBool 置位
```

### 4.5 工具函数层

```typescript
// invokeHelper.ts — 通用 Tauri invoke 包装
listItems<T>(cmd, params?)    → invoke<T[]>
getItem<T>(cmd, params)       → invoke<T> | null
saveItem<T>(cmd, payload)     → invoke<T>
deleteItem(cmd, id)           → invoke<void>
invokeAction<T>(cmd, params?) → invoke<T>
```

消除所有 store 中重复的 `try-catch` 和类型标注。

---

## 5. 数据流

### 5.1 发送消息

```
用户输入 → InputBar.onSend
  → MainPanel.handleSend
    → addMessage(userMsg) [立即更新 UI]
    → invoke("agent_send_message") [Rust]
      → AgentManager.spawn [tokio::process::Command]
        → Agent CLI 子进程
          → stdout 逐行输出
            → parse_output_line [AgentType 分发]
              → app.emit("agent-chunk") [Tauri Event]
                → useAgentEvent.onChunk
                  → dispatch(APPEND_CHUNK)
                    → MessageList 实时渲染
    → Agent 进程退出
      → app.emit("agent-done")
        → dispatch(GENERATION_DONE)
          → addMessage(assistantMsg) [持久化到 SQLite]
```

### 5.2 API 直连模式

```
用户输入 → InputBar.onSend
  → MainPanel.handleSend
    → addMessage(userMsg)
    → fetch(apiEndpoint/chat/completions) [SSE]
      → readSSEStream [前端直接解析]
        → onChunk → dispatch(APPEND_CHUNK)
    → stream 结束
      → dispatch(GENERATION_DONE)
        → addMessage(assistantMsg)
```

---

## 6. 关键设计模式

### 6.1 枚举驱动的多态 (Rust)

```rust
// AgentType 枚举封装所有差异点
impl AgentType {
    fn cli_command(&self) -> &'static str { ... }
    fn build_args(&self, ...) -> Vec<String> { ... }
    fn parse_output_line(&self, line: &str) -> Option<String> { ... }
    fn friendly_error(&self, ...) -> String { ... }
}
```

**收益：** 新增 Agent 只需添加枚举变体 + 实现 4 个方法，无需新增文件或修改流程逻辑。

### 6.2 状态机驱动 UI (React)

```typescript
// useReducer 替代多个 useState
function reducer(state, action) {
  switch (action.type) {
    case 'SEND_START':     // 清空流式内容，设置 generating
    case 'APPEND_CHUNK':   // 追加内容
    case 'GENERATION_DONE': // 完成 → 触发持久化
    case 'GENERATION_TIMEOUT': // 超时 → 追加错误信息
    case 'STOP_GENERATION': // 手动停止
  }
}
```

**收益：** 所有状态变更路径可追踪，消除闭包陷阱，测试友好。

### 6.3 连接池模式 (Rust)

```rust
// r2d2 连接池替代 Mutex<Connection>
pub type DbPool = Pool<SqliteConnectionManager>;

// 所有 command 函数：
fn some_command(state: State<DbState>) -> Result<...> {
    let conn = state.get_conn()?;  // 从池中获取连接
    // 使用 conn（自动 Deref 到 rusqlite::Connection）
}
```

**收益：** 8 连接并行处理，读写不互斥，自动回收。

### 6.4 ID-based 去重 (TypeScript)

```typescript
// sessionStore.ts
addMessage: (msg) => {
    const state = useSessionStore.getState();
    if (state.messageIds.has(msg.id)) return;  // 确定性去重
    set(state => ({
        messages: [...state.messages, msg],
        messageIds: new Set(state.messageIds).add(msg.id),
    }));
    persistMessage(msg);  // 异步持久化，不阻塞 UI
}
```

**收益：** 替代脆弱的 2 秒时间戳窗口，消除竞态条件。

---

## 7. 安全性

| 安全措施 | 实现 | 说明 |
|---------|------|------|
| API Key 加密 | AES-256-GCM | 密钥由系统 DPAPI 保护 |
| SQLite WAL 模式 | PRAGMA journal_mode=WAL | 并发读写安全 |
| 外键约束 | PRAGMA foreign_keys=ON | 数据完整性 |
| 输入过滤 | Rust 端类型校验 | 拒绝无效输入 |
| 子进程隔离 | tokio::process | 独立进程空间 |

---

## 8. 架构评估

### 8.1 合理性

- **职责边界清晰**：Rust 后端负责系统级操作（进程管理、数据库、加密），React 前端负责 UI 渲染和交互逻辑，严格通过 Tauri IPC 通信
- **分层合理**：`commands/` 层处理 IPC 请求，`agent/` 层管理 Agent 生命周期，`db/` 层处理持久化，`utils/` 层提供通用能力
- **状态管理一致**：所有 store 遵循相同的 fetch→invoke→set 模式，新开发者可快速上手

### 8.2 可移植性

- **跨平台基础**：Tauri 2.0 天然支持 Windows/macOS/Linux
- **数据库无关**：SQLite 通过 `r2d2_sqlite` 抽象，更换数据库只需替换 manager 实现
- **Agent 无关**：`AgentType` 枚举将 Agent 差异封装在 4 个方法中，新增 Agent 不影响现有流程
- **前端无关**：React 组件通过 `useAgentEvent` hook 与后端解耦，替换 UI 框架只需重写组件层

### 8.3 可扩展性

- **新增 Agent**：添加 `AgentType` 枚举变体 + 实现 4 个方法，无需修改任何其他文件
- **新增命令**：在 `commands/` 下新建文件，在 `lib.rs` 中注册 `#[tauri::command]` 和 `generate_handler!`
- **新增数据表**：在 `init.rs` 中添加建表语句 + 迁移函数，递增 `MIGRATION_VERSION`
- **新增 UI 面板**：在 `components/` 下新建组件，在 `RightPanel` 或路由中注册
- **连接池扩容**：修改 `Pool::builder().max_size(N)` 即可

### 8.4 可维护性

- **代码量精简**：消除 Sidecar 后 Rust 后端 ~53KB，前端 ~70KB，无遗留负债
- **无宏抽象**：移除 `db_command!` 宏，全部使用直接 `#[tauri::command]` 函数，IDE 支持完整
- **统一错误处理**：`AppError` 枚举 + 4 个 `From` 实现，所有命令返回一致错误类型
- **版本化迁移**：`PRAGMA user_version` 追踪迁移状态，支持增量升级
- **状态机可追踪**：`useReducer` 的 action 类型枚举所有状态变更路径

### 8.5 代码复用性

- **invokeHelper.ts**：5 个通用函数消除所有 store 中重复的 invoke 调用模式
- **constants.ts**：共享常量（EMOJI_OPTIONS）消除跨文件重复定义
- **AgentType 枚举**：进程管理、参数构建、输出解析、错误映射全部复用同一套流程
- **AppError**：8 个命令模块共享同一错误类型，前端统一处理
- **sessionStore**：ID-based 去重逻辑可复用于其他列表型 store

### 8.6 高效性

- **数据库并发**：r2d2 连接池（max 8）替代 Mutex 串行化，读写不互斥
- **流式渲染**：Tauri Event 推送 + React 增量渲染，用户即时看到输出
- **消息去重**：`Set<string>` 是 O(1) 操作，替代 O(n) 遍历
- **Store 选择器**：Zustand 自动跳过无关更新，只有订阅的字段变化时重渲染
- **WAL 模式**：SQLite WAL 允许并发读写，写入不阻塞读取
- **零序列化开销**：Tauri Event 直接传递 Rust 字符串到前端，无 JSON 序列化/反序列化

---

## 9. 与 v3.5 的变更对比

| 维度 | v3.5 | v4.0 | 收益 |
|------|------|------|------|
| 进程管理 | Node.js Sidecar (3 进程) | Rust AgentManager (1 模块) | -72% 代码，消除崩溃点 |
| 通信方式 | WebSocket (ws://127.0.0.1:19830) | Tauri Event | 零序列化，无端口管理 |
| 数据库 | Mutex<Connection> 串行 | r2d2 Pool (max 8) | 并发读写不互斥 |
| 状态管理 | 3 useState + 3 useRef | useReducer 状态机 | 可追踪，无闭包陷阱 |
| 消息去重 | 2 秒时间戳窗口 | ID-based Set | 确定性，无竞态 |
| 主题存储 | theme.txt 文件 | app_settings 表 | 统一存储 |
| 时间戳 | f64 / i64 混用 | 全 i64 | 消除精度隐患 |
| 错误处理 | 8 变体 + 2 From | 9 变体 + 4 From | 更全面 |
| 代码复用 | 无工具层 | invokeHelper + constants | 消除重复 |
| 遗留代码 | sidecar/ + wsStore + useWebSocket | 全部清理 | -30KB |

---

## 10. 未来规划

### Phase 4 (待实施)
- MainPanel 消息流性能优化（虚拟滚动）
- 会话搜索功能
- 消息编辑/重发

### Phase 5 (待实施)
- 主题系统完善（CSS 变量标准化）
- 多语言支持
- 插件系统架构设计

---

*本文档对应代码版本：PilotDesk v0.1.0，基于 Tauri 2.0 + React 19 + Zustand + r2d2/SQLite*
