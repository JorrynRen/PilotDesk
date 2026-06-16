# PilotDesk 技术实现文档 v2.0

> 版本: v2.0 | 作者: 简意工作室 (jorryn) | 日期: 2026-06
> 基于 v1.0 更新，反映截至 2026-06-01 的完整技术架构与实现细节

---

## 1. 项目信息

- **项目名称**: PilotDesk — AI Agent 统一桌面客户端
- **技术栈**: Tauri 2.0 + React 19 + TypeScript 6.0 + TailwindCSS v4 + Zustand 5 + Rust
- **开源协议**: MIT
- **版本号**: v0.1.0
- **代码仓库**: pilotdesk/

---

## 2. 架构设计

### 2.1 三层架构总览

```
┌────────────────────────────────────────────────────   ─┐
│                     React 前端                                                                                                    │
│                                                                                                                                          │
│  ┌──────────┐ ┌───────────────┐ ┌─────────────────────┐ │
│  │  Pages              │ │  Components                │ │      Stores                                      │ │
│  │ (Main/               │ │ (TitleBar/                       │ │ (Zustand:                                       │ │
│  │  Market/            │ │  SessionList/                 │ │  sessionStore                                │ │
│  │  Settings           │ │  MainPanel/                  │ │  apiProviderStore ←new               │ │
│  │  Env)                │ │  MessageList/                │ │  inspirationStore                           │ │
│  │                          │ │  InputBar/                      │ │  pendingInputStore                       │ │
│  │                          │ │  RightPanel/                  │ │  skillStore                                      │ │
│  │                          │ │  StatusBar)                    │ │  configStore                                  │ │
│  └────┬─────┘ └──────┬────────┘ └──────────┬──────────┘ │
│               │                                  │                                                     │                             │
│               └─────────────┼─────────────────────┘                            │
│                                                  │ Tauri IPC (invoke)                                                        │
├───────────────────┼──────────────────────────────────┤
│                   Rust 后端                                                                                                        │
│                                                                                                                                           │
│  ┌────────────────┐ ┌───────────────────┐ ┌────────────┐ │
│  │  Tauri Commands            │ │  SQLite (rusqlite)                     │ │  Sidecar                 │ │
│  │  (38 个 IPC)                     │ │                                                 │ │  Manager               │ │
│  │                                         │ │  Tables:                                    │ │                               │ │
│  │  - session ×6                   │ │  - sessions                                │ │  - start()                 │ │
│  │  - message ×1                │ │  - messages                              │ │  - stop()                 │ │
│  │  - inspiration                   │ │  - inspirations                            │ │  - restart                │ │
│  │    ×7                                │ │  - bot_channels                        │ │                               │ │
│  │  - api_provider                │ │  - api_providers←new             │ │                                │ │
│  │    ×6                                │ │  - app_settings  ←new            │ │                                 │ │
│  │  - app_setting                 │ │                                                 │ │                                 │ │
│  │    ×2                                │ │  FTS5:                                     │ │                                 │ │
│  │  - bot ×2                          │ │  inspirations_fts                       │ │                                 │ │
│  │  - theme ×2                     │ │                                                  │ │                                │ │
│  │  - env ×2                         │ │                                                  │ │                                │ │
│  │  - config ×3                      │ │                                                 │ │                                │ │
│  └────────────────┘ └───────────────────┘ └─────┬──────┘ │
└──────────────────────────────────────────────┼────────┘
                                                                                                                        │
                                              ┌────────────────────────────┘
                                              │
                    ┌─────────┴──────────┐
                    │   Node.js Sidecar                       │
                    │  (WebSocket Server)                 │
                    │  Port: 19830                               │
                    │                                                    │
                    │  Adapters:                                   │
                    │  - ClaudeCodeAdapter               │
                    │  - HermesAdapter                      │
                    └────────────────────┘
```

### 2.2 数据流模型

```
用户输入 → InputBar
    ↓
MainPanel.handleSend()
    ↓
判断会话类型
    ├── Agent 会话 (claude/hermes)
    │   ↓
    │   WebSocket → Sidecar → Agent Adapter
    │   ↓
    │   Sidecar SSE chunks → useWebSocket.onChunk()
    │   ↓
    │   sessionStore.addMessage()
    │   ↓
    │   invoke('save_message') → SQLite
    │
    └── API 直连会话 (api) ← v2.0 新增
        ↓
        invoke('get_api_provider') → 获取 endpoint
        ↓
        useWebSocket.sendApiChat(sessionId, message, apiEndpoint, ...)
        ↓
        fetch(endpoint) → SSE 流式解析
        ↓
        onChunk() → sessionStore.addMessage()
        ↓
        invoke('save_message') → SQLite
```

---

## 3. 目录结构

```
pilotdesk/
├── src/                                    # React 前端源码
│   ├── components/
│   │   ├── common/
│   │   │   └── AgentBadge.tsx              # Agent 类型徽标
│   │   ├── env/
│   │   │   ├── EnvManager.tsx              # 环境检测与安装管理
│   │   │   └── InstallLog.tsx              # 安装日志输出
│   │   ├── input/
│   │   │   ├── InspirationPicker.tsx       # 灵感搜索选择器（本地过滤）
│   │   │   └── SkillPicker.tsx             # 技能选择器
│   │   ├── inspiration/
│   │   │   ├── InspirationCard.tsx         # 灵感卡片
│   │   │   ├── InspirationForm.tsx         # 灵感创建/编辑表单
│   │   │   ├── MarketPage.tsx              # 灵感市集独立页面
│   │   │   └── TagFilter.tsx               # 标签过滤器
│   │   ├── layout/                         # 布局组件
│   │   │   ├── TitleBar.tsx                # 自定义标题栏（拖动+双击最大化+窗口控制）
│   │   │   ├── SessionList.tsx             # 会话列表（新建/删除/重命名/归档）
│   │   │   ├── SessionListItem.tsx         # 会话列表项（内联重命名编辑）
│   │   │   ├── MainPanel.tsx               # 主面板（消息区域 + API直连入口）
│   │   │   ├── InputBar.tsx                # 输入栏（模式切换 + 灵感/技能选择）
│   │   │   ├── RightPanel.tsx              # 右侧面板（灵感/技能/配置/记忆/Bot）
│   │   │   ├── InspirationPanel.tsx        # 灵感面板
│   │   │   └── StatusBar.tsx               # 状态栏（WebSocket 连接状态）
│   │   ├── message/
│   │   │   ├── MessageBubble.tsx           # 消息气泡（用户靠右/Agent靠左/hover按钮）
│   │   │   ├── MessageList.tsx             # 消息列表（虚拟滚动）
│   │   │   └── MarkdownRenderer.tsx        # Markdown 渲染器
│   │   └── panels/
│   │       ├── ConfigEditor.tsx            # Agent 参数配置编辑器
│   │       ├── SkillBrowser.tsx            # 技能浏览器
│   │       ├── MemoryBrowser.tsx           # 记忆浏览器
│   │       ├── BotSetup.tsx                # Bot 通道配置
│   │       └── UpdateChecker.tsx           # 更新检查
│   ├── hooks/
│   │   ├── useWebSocket.ts                 # WebSocket hook + SSE 流式 + API直连
│   │   ├── useTheme.ts                     # 主题切换 hook
│   │   └── useTauriCommand.ts             # Tauri command 通用调用 hook
│   ├── pages/
│   │   ├── SettingsPage.tsx                # 设置页（5 Tab）← v2.0 重写
│   │   └── EnvPage.tsx                     # 环境配置独立页面
│   ├── stores/                             # Zustand 状态管理
│   │   ├── sessionStore.ts                 # 会话状态（invoke → SQLite）
│   │   ├── apiProviderStore.ts ← v2.0 新建 # API 提供商状态（invoke → SQLite）
│   │   ├── inspirationStore.ts            # 灵感状态（invoke → SQLite）
│   │   ├── pendingInputStore.ts            # 输入桥接（Zustand 内存）
│   │   ├── skillStore.ts                   # 技能状态
│   │   └── configStore.ts                  # 配置状态
│   ├── types/
│   │   └── index.ts                        # 全局类型定义
│   ├── utils/
│   │   └── toast.ts                        # Toast 通知工具
│   ├── styles/
│   │   └── ui.css                          # CSS 变量 + 全局样式
│   ├── App.tsx                             # 主应用（页面路由）
│   └── main.tsx                            # 入口
│
├── src-tauri/                              # Tauri / Rust 后端
│   ├── src/
│   │   ├── lib.rs                          # 主入口（38 个 Tauri commands 注册）
│   │   ├── main.rs                         # Rust 入口
│   │   ├── agent_config/                   # Agent 配置读写
│   │   │   ├── claude.rs                   # Claude Code 配置
│   │   │   ├── hermes.rs                   # Hermes Agent 配置
│   │   │   └── mod.rs
│   │   ├── commands/                       # Tauri IPC 命令处理
│   │   │   ├── api_provider.rs ← v2.0 新建 # API 提供商 CRUD（6 个命令）
│   │   │   ├── app_settings.rs ← v2.0 新建 # 通用 KV 设置（2 个命令）
│   │   │   ├── session.rs                  # 会话 CRUD（6 个命令）
│   │   │   ├── inspiration.rs              # 灵感 CRUD + 搜索（7 个命令）
│   │   │   ├── bot.rs                      # Bot 通道（2 个命令）
│   │   │   ├── config.rs                   # Agent 配置（3 个命令）
│   │   │   ├── env.rs                      # 环境检测（2 个命令）
│   │   │   ├── theme.rs                    # 主题管理（2 个命令）
│   │   │   └── mod.rs
│   │   ├── db/
│   │   │   ├── init.rs                     # 数据库初始化 + Migration
│   │   │   ├── models.rs                   # 数据模型
│   │   │   └── mod.rs
│   │   ├── sidecar/
│   │   │   ├── manager.rs                  # Sidecar 进程管理器
│   │   │   └── mod.rs
│   │   └── utils/
│   │       ├── errors.rs                   # 统一错误类型 (AppError)
│   │       ├── paths.rs                    # 路径工具（DB 路径等）
│   │       └── mod.rs
│   ├── capabilities/
│   │   └── default.json                    # Tauri 权限配置
│   ├── Cargo.toml                          # Rust 依赖
│   └── tauri.conf.json                     # Tauri 应用配置
│
├── sidecar/                                # Node.js Sidecar
│   ├── src/
│   │   ├── index.ts                        # 入口
│   │   ├── server.ts                       # WebSocket 服务端（端口 19830）
│   │   ├── types.ts                        # 类型定义
│   │   └── adapters/
│   │       ├── base.ts                     # Agent 适配器基类
│   │       ├── claude-code.ts              # Claude Code 适配器
│   │       └── hermes.ts                   # Hermes Agent 适配器
│   ├── dist/                               # 编译输出
│   └── package.json
│
├── image/                                  # 品牌图标资源
│   ├── logo.svg
│   ├── icon-*.png (16/32/64/128/256/512)
│   └── tray-icon.png
│
├── package.json
├── tsconfig.json
├── tailwind.config.js
├── LICENSE                                 # MIT License
└── ...
```

---

## 4. SQLite 数据库设计

### 4.1 表结构

#### sessions（会话）

| 字段                   | 类型      | 约束                                           | 说明           |
| -------------------- | ------- | -------------------------------------------- | ------------ |
| id                   | TEXT    | PRIMARY KEY                                  | UUID         |
| agent_type           | TEXT    | NOT NULL, CHECK('claude','hermes','api')     | Agent 类型     |
| title                | TEXT    | NOT NULL DEFAULT ''                          | 会话标题         |
| cwd                  | TEXT    | DEFAULT ''                                   | 工作目录         |
| created_at           | INTEGER | NOT NULL                                     | 创建时间戳        |
| updated_at           | INTEGER | NOT NULL                                     | 更新时间戳        |
| last_message_preview | TEXT    | DEFAULT ''                                   | 最新消息预览       |
| message_count        | INTEGER | DEFAULT 0                                    | 消息计数         |
| status               | TEXT    | DEFAULT 'active', CHECK('active','archived') | 状态           |
| api_provider         | TEXT    | NULL                                         | API 直连提供商 ID |
| api_model            | TEXT    | NULL                                         | API 直连模型     |

#### messages（消息）

| 字段         | 类型      | 约束                                                        | 说明   |
| ---------- | ------- | --------------------------------------------------------- | ---- |
| id         | TEXT    | PRIMARY KEY                                               | UUID |
| session_id | TEXT    | NOT NULL, FK → sessions(id) CASCADE                       | 所属会话 |
| role       | TEXT    | NOT NULL, CHECK('user','assistant','system')              | 角色   |
| content    | TEXT    | NOT NULL DEFAULT ''                                       | 消息内容 |
| mode       | TEXT    | DEFAULT 'native', CHECK('native','fast','think','expert') | 对话模式 |
| timestamp  | INTEGER | NOT NULL                                                  | 时间戳  |

#### inspirations（灵感）

| 字段           | 类型      | 约束                  | 说明       |
| ------------ | ------- | ------------------- | -------- |
| id           | TEXT    | PRIMARY KEY         | UUID     |
| icon         | TEXT    | NOT NULL DEFAULT '' | 图标       |
| title        | TEXT    | NOT NULL            | 标题       |
| content      | TEXT    | NOT NULL DEFAULT '' | 内容       |
| source_agent | TEXT    | DEFAULT 'manual'    | 来源 Agent |
| is_favorite  | INTEGER | DEFAULT 0           | 是否收藏     |
| created_at   | INTEGER | NOT NULL            | 创建时间     |
| updated_at   | INTEGER | NOT NULL            | 更新时间     |

#### inspiration_tags（灵感标签）

| 字段             | 类型   | 约束                                      | 说明    |
| -------------- | ---- | --------------------------------------- | ----- |
| inspiration_id | TEXT | NOT NULL, FK → inspirations(id) CASCADE | 灵感 ID |
| tag            | TEXT | NOT NULL                                | 标签名   |
| PRIMARY KEY    |      | (inspiration_id, tag)                   | 复合主键  |

#### api_providers（API 提供商，v2.0 新增）

| 字段             | 类型      | 约束                    | 说明                                            |
| -------------- | ------- | --------------------- | --------------------------------------------- |
| id             | TEXT    | PRIMARY KEY           | 提供商 ID（如 'anthropic', 'openai', 'custom_xxx'） |
| name           | TEXT    | NOT NULL DEFAULT ''   | 显示名称                                          |
| api_endpoint   | TEXT    | NOT NULL DEFAULT ''   | 完整 API URL                                    |
| api_key        | TEXT    | DEFAULT ''            | 原始 API Key（仅 Rust 端访问）                        |
| api_key_masked | TEXT    | DEFAULT ''            | 脱敏 Key（如 'sk-a****b2cd'）                      |
| api_key_set    | INTEGER | DEFAULT 0             | 是否已配置 Key                                     |
| models         | TEXT    | NOT NULL DEFAULT '[]' | 模型列表（JSON 数组）                                 |
| sort_order     | INTEGER | DEFAULT 0             | 排序权重                                          |
| created_at     | INTEGER | NOT NULL              | 创建时间                                          |
| updated_at     | INTEGER | NOT NULL              | 更新时间                                          |

#### app_settings（应用设置，v2.0 新增）

| 字段         | 类型      | 约束                  | 说明                           |
| ---------- | ------- | ------------------- | ---------------------------- |
| key        | TEXT    | PRIMARY KEY         | 设置键（如 'pilotdesk-workspace'） |
| value      | TEXT    | NOT NULL DEFAULT '' | 设置值                          |
| updated_at | INTEGER | NOT NULL            | 更新时间                         |

#### bot_channels（Bot 通道）

| 字段              | 类型      | 约束                        | 说明       |
| --------------- | ------- | ------------------------- | -------- |
| id              | TEXT    | PRIMARY KEY               | UUID     |
| agent_type      | TEXT    | NOT NULL                  | Agent 类型 |
| platform        | TEXT    | NOT NULL DEFAULT 'wechat' | 平台       |
| method          | TEXT    | DEFAULT 'clawbot'         | 方法       |
| status          | TEXT    | DEFAULT 'disconnected'    | 状态       |
| trigger_prefix  | TEXT    | DEFAULT ''                | 触发前缀     |
| response_format | TEXT    | DEFAULT 'markdown'        | 响应格式     |
| config          | TEXT    | DEFAULT '{}'              | 配置 JSON  |
| created_at      | INTEGER | NOT NULL                  | 创建时间     |
| updated_at      | INTEGER | NOT NULL                  | 更新时间     |

### 4.2 索引与 FTS

```
-- 索引
idx_sessions_agent         ON sessions(agent_type, updated_at)
idx_sessions_status        ON sessions(status, updated_at)
idx_messages_session       ON messages(session_id, timestamp)
idx_inspirations_favorite   ON inspirations(is_favorite, updated_at)
idx_inspirations_tags       ON inspiration_tags(tag)

-- FTS5 全文搜索
inspirations_fts            ON inspirations(title, content)
```

### 4.3 Migration 策略

数据库初始化在 `db::init::init_db()` 中执行，采用 **CREATE IF NOT EXISTS** + 条件 ALTER TABLE 策略：

1. 建表（含 CHECK 约束的完整 DDL）
2. `migrate_add_api_columns` — 为 sessions 表添加 api_provider / api_model 列
3. `migrate_add_api_agent_type` — 重建 sessions 表支持 'api' agent_type
4. `migrate_add_api_providers` — 创建 api_providers 表
5. `migrate_add_app_settings` — 创建 app_settings 表

---

## 5. Tauri IPC 命令注册（38 个）

### 5.1 命令分类

| 类别           | 命令                                                                                                                                                      | 数量  | 说明                |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | --- | ----------------- |
| **会话**       | list_sessions, list_archived_sessions, create_session, get_session, get_session_messages, rename_session, archive_session, delete_session, save_message | 9   | 会话 CRUD           |
| **灵感**       | list_inspirations, get_inspiration, create_inspiration, update_inspiration, delete_inspiration, search_inspirations, list_tags                          | 7   | 灵感 CRUD + 搜索 + 标签 |
| **API 提供商**  | list_api_providers, get_api_provider, upsert_api_provider, delete_api_provider, get_api_key, reorder_api_providers                                      | 6   | API 配置管理（v2.0 新增） |
| **应用设置**     | get_app_setting, set_app_setting                                                                                                                        | 2   | 通用 KV 设置（v2.0 新增） |
| **Bot 通道**   | list_bot_channels, save_bot_channel, delete_bot_channel                                                                                                 | 3   | Bot 管理            |
| **Agent 配置** | get_config, save_claude_config, save_hermes_config, test_api_connection                                                                                 | 4   | 配置读写 + 测试         |
| **环境**       | detect_env, install_claude_code, install_hermes                                                                                                         | 3   | 环境检测与安装           |
| **主题**       | get_theme, set_theme_cmd                                                                                                                                | 2   | 主题管理              |
| **其他**       | greet                                                                                                                                                   | 1   | 测试命令              |

### 5.2 Rust 端命令签名示例

```rust
// API 提供商 CRUD（api_provider.rs）
#[tauri::command]
fn list_api_providers(conn: State<'_, DbState>) -> Result<Vec<ApiProvider>, AppError>
#[tauri::command]
fn get_api_provider(conn: State<'_, DbState>, id: String) -> Result<Option<ApiProvider>, AppError>
#[tauri::command]
fn upsert_api_provider(conn: State<'_, DbState>, payload: CreateOrUpdateProvider) -> Result<ApiProvider, AppError>
#[tauri::command]
fn delete_api_provider(conn: State<'_, DbState>, id: String) -> Result<(), AppError>
#[tauri::command]
fn get_api_key(conn: State<'_, DbState>, id: String) -> Result<Option<String>, AppError>
#[tauri::command]
fn reorder_api_providers(conn: State<'_, DbState>, ids: Vec<String>) -> Result<(), AppError>

// 通用设置（app_settings.rs）
#[tauri::command]
fn get_app_setting(conn: State<'_, DbState>, key: String) -> Result<Option<String>, AppError>
#[tauri::command]
fn set_app_setting(conn: State<'_, DbState>, key: String, value: String) -> Result<(), AppError>
```

### 5.3 前端调用方式

```typescript
// Zustand Store 封装（apiProviderStore.ts）
const { providers, fetchProviders, saveProvider, deleteProvider, reorderProviders } = useApiProviderStore();

// 直接 invoke（app_settings 等）
const workspace = await invoke<string | null>('get_app_setting', { key: 'pilotdesk-workspace' });
await invoke('set_app_setting', { key: 'pilotdesk-workspace', value: selected });

// 获取 API Key（不在 provider 列表中暴露）
const apiKey = await getApiKey(providerId);  // 调用 invoke('get_api_key', { id })
```

---

## 6. 前端状态管理（Zustand Stores）

### 6.1 Store 架构

```
┌─────────────────────────────────────────────────┐
│                  Zustand Stores                                                                                   │
│                                                                                                                             │
│  sessionStore          数据源: SQLite sessions                                                     │
│  ├─ sessions[]         + messages 表                                                                   │
│  ├─ currentSessionId   invoke: list_sessions,                                                    │
│  ├─ messages[]          create_session,                                                              │
│  └─ CRUD ops            rename_session, ...                                                       │
│                                                                                                                            │
│  apiProviderStore ←new  数据源: SQLite                                                          │
│  ├─ providers[]         api_providers 表                                                                │
│  ├─ fetchProviders()    invoke: list/upsert/                                                          │
│  ├─ saveProvider()      delete/reorder                                                                 │
│  ├─ deleteProvider()    _api_providers                                                                │
│  └─ reorderProviders()                                                                                        │
│                                                                                                                             │
│  getApiKey() ←new        invoke: get_api_key                                                     │
│  getApiEndpoint() ←new   invoke: get_api_provider                                           │
│                                                                                                                             │
│  inspirationStore      数据源: SQLite                                                                    │
│  ├─ inspirations[]      inspirations 表                                                                    │
│  └─ CRUD ops            invoke: create/update/...                                                 │
│                                                                                                                             │
│  pendingInputStore     数据源: 内存                                                                    │
│  └─ pendingInput        用于灵感→输入框桥接                                                   │
│                                                                                                                            │
│  skillStore            数据源: WebSocket Sidecar                                                   │
│  ├─ agentSkills         从 Sidecar 加载                                                                │
│  └─ setAgentSkills()                                                                                           │
│                                                                                                                            │
│  configStore            数据源: invoke                                                                    │
│  └─ Agent 配置数据                                                                                           │
└──────────────────────────────────────────────── ┘
```

### 6.2 apiProviderStore（v2.0 新增）

```typescript
interface ApiProviderState {
  providers: ApiProvider[];
  loading: boolean;
  error: string | null;
  fetchProviders: () => Promise<void>;
  saveProvider: (data: UpsertPayload) => Promise<ApiProvider>;
  deleteProvider: (id: string) => Promise<void>;
  reorderProviders: (ids: string[]) => Promise<void>;
}

// 独立工具函数
async function getApiKey(providerId: string): Promise<string | null>
async function getApiEndpoint(providerId: string): Promise<string | null>
```

---

## 7. 关键技术实现

### 7.1 API 直连会话（v2.0）

**流程：**

1. 用户在 SessionList 新建会话时选择 "API 直连" 类型

2. 弹出对话框选择提供商和模型（数据从 `apiProviderStore` 加载）

3. 创建前调用 `getApiKey()` 校验 Key 是否存在

4. 通过 `createSession('api', undefined, title, providerId, model)` 创建会话

5. 发送消息时，MainPanel 调用 `invoke('get_api_provider')` 获取 endpoint

6. 调用 `sendApiChat(sessionId, message, apiEndpoint, providerId, model, history)`

7. `sendApiChat` 内部：
   
   - 调用 `getApiKey(providerId)` 获取原始 Key
   
   - `fetch(apiEndpoint)` 发送请求（Bearer/x-api-key 认证）
   
   - 流式解析 SSE 响应（支持 Anthropic + OpenAI 两种格式）
   
   - 通过 `onChunk` / `onDone` 回调更新 UI

**协议自动识别：**

```typescript
function inferApiFormat(providerId: string, endpoint: string): 'anthropic' | 'openai'
// providerId 含 'anthropic' 或 endpoint 含 'anthropic.com' → anthropic 格式
// 否则 → openai 格式（Bearer 认证）
```

### 7.2 localStorage → SQLite 迁移（v2.0）

**迁移范围：**

| 文件                 | 迁移项                  | 替换方式                                   |
| ------------------ | -------------------- | -------------------------------------- |
| `SettingsPage.tsx` | API 提供商 CRUD（17 处）   | `useApiProviderStore`                  |
| `SettingsPage.tsx` | 工作区目录（2 处）           | `invoke('get/set_app_setting')`        |
| `SessionList.tsx`  | 提供商列表加载（2 处）         | `useApiProviderStore.fetchProviders()` |
| `SessionList.tsx`  | API Key 校验（1 处）      | `getApiKey()` invoke                   |
| `MainPanel.tsx`    | API endpoint 查找（1 段） | `invoke('get_api_provider')`           |
| `useWebSocket.ts`  | API Key 读取（1 处）      | `getApiKey()` invoke                   |

**共 23 处 localStorage 调用已全部替换为 Tauri invoke 调用。**

### 7.3 消息气泡布局

```
┌────────────────────────────────────────────┐
│  Agent 消息（靠左）                                                                               │
│  ┌──────────────────────────────────┐                   │
│  │ 💬 Agent 回复内容...                                                     │                  │
│  └──────────────────────────────────┘                  │
│                                                                                                                │
│              用户消息（靠右）                                                                     │
│         ┌──────────────────────┐                                         │
│         │ 💬 用户输入内容...                         │ [编辑][收藏]                      │
│         └──────────────────────┘                                         │
└─────────────────────────────────────────── ┘

用户消息布局实现：
<div className="flex justify-end">           ← 全宽行，内容靠右
  <div style={{ maxWidth: '80%' }}>         ← 内层容器限制宽度
    <div className="rounded-lg ...">        ← 气泡
      {content}
    </div>
    <div className="group ...">              ← 操作按钮组
      <button className="opacity-0 group-hover:opacity-100">编辑</button>
      <button className="opacity-0 group-hover:opacity-100">收藏</button>
    </div>
  </div>
</div>
```

### 7.4 主题系统

```css
:root {
  --bg-primary: #1a1a2e;
  --bg-secondary: #16213e;
  --bg-tertiary: #0f3460;
  --text-primary: #e0e0e0;
  --text-secondary: #a0a0a0;
  --text-tertiary: #606060;
  --accent: #5B7FFF;
  --border: #2a2a4a;
  --success: #10B981;
  --hermes-tag: #8B5CF6;
}
```

- 深色/浅色/跟随系统三种模式
- Rust 端通过 `commands::theme` 读写主题配置
- `useTheme` hook 提供 `theme` / `setTheme`

### 7.5 WebSocket 通信协议

```typescript
// 客户端 → Sidecar 消息类型
type WsMessage =
  | { type: 'chat'; sessionId: string; message: string }
  | { type: 'stop'; sessionId: string }
  | { type: 'session:create'; sessionId: string; agentType: string; cwd?: string }
  | { type: 'session:close'; sessionId: string }
  | { type: 'ping' }
  | { type: 'skills:list'; agentType: string }
  | { type: 'skills:list-all' };

// Sidecar → 客户端 响应类型
interface ChatChunk {
  type: 'chunk' | 'done' | 'error' | 'status' | 'skills';
  sessionId: string;
  content?: string;
  error?: string;
  status?: string;
}
```

### 7.6 Sidecar 进程管理

```rust
pub struct SidecarManager {
    process: Option<Child>,           // Node.js 子进程
    port: u16,                         // WebSocket 端口 (19830)
    restart_count: Arc<AtomicU32>,    // 重启计数
    is_running: Arc<AtomicBool>,      // 运行状态
}
// 方法: start(), stop(), restart(), is_running(), port()
// Drop trait: 自动清理子进程
```

---

## 8. Rust 依赖（Cargo.toml）

| 依赖                  | 版本         | 用途               |
| ------------------- | ---------- | ---------------- |
| tauri               | 2          | 桌面框架核心           |
| tauri-plugin-shell  | 2          | 进程管理（Sidecar 启动） |
| tauri-plugin-dialog | 2.7.1      | 原生对话框（目录选择）      |
| serde / serde_json  | 1          | JSON 序列化         |
| rusqlite (bundled)  | 0.32       | SQLite 数据库       |
| tokio (full)        | 1          | 异步运行时            |
| chrono (serde)      | 0.4        | 时间戳处理            |
| uuid (v4)           | 1          | UUID 生成          |
| dirs                | 6          | 系统目录路径           |
| log / env_logger    | 0.4 / 0.11 | 日志               |

---

## 9. 前端依赖（package.json）

| 依赖                                   | 版本                     | 用途               |
| ------------------------------------ | ---------------------- | ---------------- |
| react / react-dom                    | 19.x                   | UI 框架            |
| typescript                           | 6.0                    | 类型系统             |
| tailwindcss                          | v4 + @tailwindcss/vite | 样式               |
| zustand                              | 5.x                    | 状态管理             |
| @tauri-apps/api                      | 2.11                   | Tauri IPC 前端 SDK |
| @tauri-apps/plugin-dialog            | 2.7                    | 原生对话框            |
| @dnd-kit/core + sortable + utilities | 6.x / 10.x             | 拖拽排序             |
| lucide-react                         | latest                 | 图标库              |
| react-markdown                       | 10.x                   | Markdown 渲染      |
| remark-gfm                           | 4.x                    | GFM 扩展           |
| rehype-highlight                     | 7.x                    | 代码高亮             |
| react-virtuoso                       | 4.x                    | 虚拟滚动列表           |
| clsx                                 | 2.x                    | 类名合并             |
| vite                                 | 8.x                    | 构建工具             |

---

## 10. 开发环境配置

| 工具          | 路径                                                                            | 说明                    |
| ----------- | ----------------------------------------------------------------------------- | --------------------- |
| Node.js     | `F:\soft\nodejs`                                                              | 前端构建 + Sidecar 运行     |
| Rust stable | `C:\Users\Administrator\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin` | 不在系统 PATH，需手动设置       |
| Vite 启动     | `F:\soft\nodejs\node.exe node_modules\vite\bin\vite.js --host`                | 使用 CREATE_NEW_CONSOLE |
| Cargo 构建    | `cargo.exe` (完整路径) + shell=True + 手动 PATH                                     | 同上                    |
| tsc 检查      | `F:\soft\nodejs\node.exe node_modules/typescript/bin/tsc --noEmit`            | 类型检查                  |
| Sidecar 端口  | 19830                                                                         | WebSocket 监听          |
| 前端 Dev 端口   | 1420                                                                          | Vite HMR              |

---

## 11. 命名规范

### 11.1 前后端统一 camelCase

- **Rust struct** 使用 camelCase 字段名（通过 `#[serde(rename_all = "camelCase")]` 自动转换）
- **TypeScript interface** 使用 camelCase
- **SQLite 列名** 使用 snake_case（标准 SQL 规范）
- **Rust → JSON** 序列化时自动将 snake_case 映射为 camelCase
- **前端** 直接使用 camelCase，无需手动映射

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiProvider {
    pub id: String,
    pub api_endpoint: String,  // SQLite 列: snake_case
    // JSON 输出: { "id": "...", "apiEndpoint": "..." }
}
```

### 11.2 Git 提交规范

```
feat: 新功能
fix: 修复 bug
refactor: 重构（如 localStorage → SQLite 迁移）
style: 样式调整
docs: 文档更新
chore: 构建/工具链变更
```

---

## 12. 已实现功能清单

### 12.1 Phase 1 — 基础架构 ✅

- [x] Tauri 2.0 + React 19 项目初始化
- [x] TypeScript 6.0 + TailwindCSS v4 配置
- [x] Zustand 5 状态管理集成
- [x] CSS 变量系统（深色/浅色/跟随系统主题）
- [x] 自定义标题栏（拖动、双击最大化/还原、窗口控制）
- [x] 无边框窗口（`decorations: false`）

### 12.2 Phase 2 — 会话系统 ✅

- [x] 会话列表（创建、删除、切换、重命名、归档）
- [x] 会话模式切换（Claude Code / Hermes Agent / **API 直连**）
- [x] 消息气泡（用户靠右 flex+max-width、Agent 靠左）
- [x] 消息编辑（填充输入框）
- [x] 用户消息收藏灵感（hover 按钮 → createInspiration + toast）
- [x] SSE 流式输出（Anthropic + OpenAI 格式双协议）
- [x] **Markdown 渲染**（react-markdown + remark-gfm + rehype-highlight）
- [x] **虚拟滚动消息列表**（react-virtuoso）
- [x] 消息操作按钮 hover 显示（opacity-0 → group-hover:opacity-100）

### 12.3 Phase 3 — 灵感系统 ✅

- [x] 独立灵感面板
- [x] 灵感 CRUD（标题 + 内容 + 24 图标选择）
- [x] 灵感搜索（本地过滤 + FTS5 全文搜索）
- [x] 灵感应用（点击填入输入框）
- [x] 灵感市集独立页面
- [x] pendingInputStore（Zustand 替代 sessionStorage）

### 12.4 Phase 4 — API 配置 + 设置 ✅

- [x] API 配置（列表式新增/编辑/删除）
- [x] **完整 API URL** 填写，系统直接调用
- [x] API 拖拽排序（@dnd-kit）
- [x] API 测试连接（max_tokens=1，返回延迟）
- [x] 设置页（**5 Tab**：通用/环境/Agent 参数/API 配置/关于）
- [x] Agent 参数配置（Claude Code + Hermes Agent ConfigEditor）
- [x] 通用设置 - 工作区目录（原生目录浏览，**SQLite 存储**）
- [x] 环境配置页（检测 Node/Git/Python/Claude/Hermes 版本）
- [x] 关于页（Logo + 版本 + 技术栈 + MIT 版权）

### 12.5 Phase 5 — API 直连会话 ✅

- [x] 新建 API 直连会话（选择提供商 + 模型）
- [x] API Key 校验（创建前检查）
- [x] 端到端 API 通信（fetch → SSE 流式）
- [x] 多轮对话（自动携带历史）
- [x] 会话标题自动格式化

### 12.6 Phase 6 — 存储迁移 ✅

- [x] Rust 后端 api_provider.rs（CRUD 6 个命令）
- [x] Rust 后端 app_settings.rs（KV 2 个命令）
- [x] SQLite Migration（api_providers + app_settings 表）
- [x] 前端 apiProviderStore.ts（Zustand store）
- [x] **23 处 localStorage 全部替换为 invoke 调用**
- [x] tsc + cargo build 验证通过

### 12.7 Phase 7 — UI 打磨 ✅

- [x] 标题栏按钮紧邻窗口操作按钮
- [x] 最大化后图标变为还原图标
- [x] 会话模式下拉向上展开
- [x] 设置按钮 toggle 切换
- [x] 4 种对话模式（原生/快速/深度思考/专家）+ 颜色标识
- [x] 技能系统（WebSocket 加载 + 技能浏览器）
- [x] Bot 通道配置管理
- [x] Toast 通知系统
- [x] MIT License 文件
- [x] 前后端命名统一 camelCase

---

## 13. 待完成功能

### 13.1 Sidecar 端到端测试

- [ ] Claude Code Agent 端到端通信验证
- [ ] Hermes Agent 端到端通信验证
- [ ] Sidecar 自动重启与心跳监控完善

### 13.2 UI 细节

- [ ] 会话初始界面空状态引导优化
- [ ] 关于页/初始界面 Logo 图标显示
- [ ] 标题栏双击最大化/还原验证

### 13.3 打包发布

- [ ] Windows 安装包构建
- [ ] 应用签名

---

## 14. Tauri 应用配置

```json
{
  "productName": "PilotDesk",
  "version": "0.1.0",
  "identifier": "com.pilotdesk.app",
  "app": {
    "windows": [{
      "title": "PilotDesk",
      "width": 1280,
      "height": 800,
      "minWidth": 1024,
      "minHeight": 680,
      "decorations": false
    }],
    "security": {
      "csp": "default-src 'self'; style-src 'self' 'unsafe-inline'; 
              img-src 'self' data: https://api.qrserver.com; 
              connect-src 'self' ws://localhost:* http://localhost:*"
    }
  }
}
```

---

*Copyright (c) 2026 PilotDesk by 简意工作室 (jorryn)*
*本项目代码基于 MIT 协议开源*
