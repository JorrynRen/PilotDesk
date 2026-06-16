# PilotDesk 技术实现文档 v3.1

> 版本: v3.1 | 作者: 简意工作室 (jorryn) | 日期: 2026-06
> 基于 v3.0 更新，反映截至 2026-06-03 的完整技术架构与实现细节
>
> **v3.1 新增内容：**
> - **ClaudeConfigForm / HermesConfigForm 合并为统一 AgentConfigForm**（AGENT_FORM_CONFIGS 配置驱动，代码量减少约 35%）
> - **连接测试复用 sendApiRequest**：移除 Rust 端 test_api_connection 重复实现，前端直接调用 sendApiRequest + get_agent_api_key
> - **新增 get_agent_api_key Rust 命令**：从 claude.rs/hermes.rs 读取原始 API Key 返回前端
> - **按钮布局优化**：whitespace-nowrap 防止按钮被挤压换行，status banner 独立展示测试结果
> - **消息名称显示修复**：按 `智能体名称 | 厂商 | 模型 时间` 格式统一显示
> - **Hermes 会话超时友好提示**：错误消息以消息气泡形式持久展示在会话窗口中
> - **移除软件右下角错误检测信息框**
> - **Agent 环境检测 stdout/stderr 管道错误处理**：捕获 os error 232 避免 panic
> - **环境检测去重**：修复自动检测多次重复触发的问题
> - **Agent 参数配置读写**：实现配置文件的读取回填与保存更新
>
> **v3.0 新增内容（保留）：**
> - config 加载时机修复（App.tsx 启动时调用 fetchConfig）
> - 消息名称格式化（buildAgentLabel 统一前端拼接）
> - AgentBadge 首字母显示（C/H/A）
> - 双层超时保护机制（前端 60s + Sidecar 120s）
> - typing indicator 头像颜色动态化
> - API 类型厂商名称显示修复
> - agentType 始终显示 hermes 的 bug 修复
> - Sidecar 编译方式变更（node tsc.js 替代 npx tsc）
> - 安装日志表 install_logs + 3 个 IPC 命令
> - 更新检查 3 个 IPC 命令（check_pilotdesk_update / check_single_npm / check_single_pypi）
> - 会话消息持久化（addMessage 去重 + fire-and-forget 写入）

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
┌──────────────────────────────────────────────────────┐
│                     React 前端                       │
│                                                      │
│  ┌──────────┐ ┌───────────────┐ ┌─────────────────┐ │
│  │  Pages   │ │  Components   │ │  Stores         │ │
│  │ (Main/   │ │ (TitleBar/    │ │ (Zustand:       │ │
│  │  Market/ │ │  SessionList/ │ │  sessionStore   │ │
│  │  Settings│ │  MainPanel/   │ │  apiProviderStore│ │
│  │  Env)    │ │  MessageList/ │ │  inspirationStore│ │
│  │          │ │  InputBar/    │ │  pendingInputStore│ │
│  │          │ │  RightPanel/  │ │  skillStore     │ │
│  │          │ │  StatusBar)   │ │  configStore    │ │
│  └────┬─────┘ └──────┬────────┘ └────────┬─────────┘ │
│       │              │                    │           │
│       └──────────────┼────────────────────┘           │
│                      │ Tauri IPC (invoke)             │
├──────────────────────┼────────────────────────────────┤
│                   Rust 后端                           │
│                                                      │
│  ┌────────────────┐ ┌───────────────────┐ ┌────────┐ │
│  │ Tauri Commands │ │ SQLite (rusqlite) │ │Sidecar │ │
│  │ (44 个 IPC)    │ │                   │ │Manager │ │
│  │                │ │ Tables:           │ │        │ │
│  │ - session ×9   │ │ - sessions       │ │ start()│ │
│  │ - message ×1   │ │ - messages       │ │ stop() │ │
│  │ - inspiration  │ │ - inspirations   │ │ restart│ │
│  │   ×7           │ │ - bot_channels   │ │        │ │
│  │ - api_provider │ │ - api_providers  │ │        │ │
│  │   ×6           │ │ - app_settings   │ │        │ │
│  │ - install_log  │ │                  │ │        │ │
│  │   ×3           │ │ FTS5:            │ │        │ │
│  │ - update ×3    │ │                  │ │        │ │
│  │ - app_setting  │ │                  │ │        │ │
│  │   ×2           │ │ FTS5:            │ │        │ │
│  │ - bot ×2       │ │ inspirations_fts │ │        │ │
│  │ - theme ×2     │ │                  │ │        │ │
│  │ - env ×2       │ │                  │ │        │ │
│  │ - config ×3    │ │                  │ │        │ │
│  └────────────────┘ └───────────────────┘ └───┬────┘ │
└───────────────────────────────────────────────┼───────┘
                                                │
                    ┌───────────────────────────┘
                    │
                    ┌─────────┴──────────┐
                    │   Node.js Sidecar  │
                    │  (WebSocket Server)│
                    │  Port: 19830       │
                    │                    │
                    │  Adapters:         │
                    │  - ClaudeCodeAdapter│
                    │  - HermesAdapter   │
                    │  - withTimeout     │ ← v3.0 新增
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
    │   ↓ (withTimeout 120s 超时保护) ← v3.0
    │   Sidecar SSE chunks → useWebSocket.onChunk()
    │   ↓
    │   sessionStore.addMessage()
    │   ↓
    │   invoke('save_message') → SQLite
    │
    └── API 直连会话 (api)
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

### 2.3 超时保护机制（v3.0 新增）

```
前端超时 (60s)          Sidecar 超时 (120s)
    │                        │
    │ timeoutRef             │ withTimeout(asyncGenerator, 120000)
    │ setTimeout(60s)        │ Promise.race:
    │                        │   - for await (chunk of generator)
    │ onDone/onError/stop    │   - setTimeout(120s)
    │ 时清除 timeout         │
    │                        │ 超时 → throw new Error("超时")
    │ 超时 → 保存已收到内容   │
    │ 显示友好提示消息        │
```

---

## 3. 目录结构

```
pilotdesk/
├── src/                                    # React 前端源码
│   ├── components/
│   │   ├── common/
│   │   │   └── AgentBadge.tsx              # Agent 类型徽标（首字母 C/H/A）← v3.0 修改
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
│   │   │   ├── MainPanel.tsx               # 主面板（消息区域 + 60s 前端超时）← v3.0 修改
│   │   │   ├── InputBar.tsx                # 输入栏（模式切换 + 灵感/技能选择）
│   │   │   ├── RightPanel.tsx              # 右侧面板（灵感/技能/配置/记忆/Bot）
│   │   │   ├── InspirationPanel.tsx        # 灵感面板
│   │   │   └── StatusBar.tsx               # 状态栏（WebSocket 连接状态）
│   │   ├── message/
│   │   │   ├── MessageBubble.tsx           # 消息气泡（buildAgentLabel 前端拼接）← v3.0 修改
│   │   │   ├── MessageList.tsx             # 消息列表（typing indicator 颜色动态化）← v3.0 修改
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
│   │   ├── SettingsPage.tsx                # 设置页（5 Tab）
│   │   └── EnvPage.tsx                     # 环境配置独立页面
│   ├── stores/                             # Zustand 状态管理
│   │   ├── sessionStore.ts                 # 会话状态（invoke → SQLite）
│   │   ├── apiProviderStore.ts             # API 提供商状态（含 API_PROVIDERS fallback）← v3.0 修改
│   │   ├── inspirationStore.ts            # 灵感状态（invoke → SQLite）
│   │   ├── pendingInputStore.ts            # 输入桥接（Zustand 内存）
│   │   ├── skillStore.ts                   # 技能状态
│   │   └── configStore.ts                  # 配置状态（fetchConfig 启动时加载）← v3.0 修改
│   ├── types/
│   │   └── index.ts                        # 全局类型定义
│   ├── utils/
│   │   └── toast.ts                        # Toast 通知工具
│   ├── styles/
│   │   └── ui.css                          # CSS 变量 + 全局样式
│   ├── App.tsx                             # 主应用（启动时调用 fetchConfig）← v3.0 修改
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
│   │   │   ├── api_provider.rs             # API 提供商 CRUD（6 个命令）
│   │   │   ├── app_settings.rs             # 通用 KV 设置（2 个命令）
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
│   │   ├── server.ts                       # WebSocket 服务端（含 withTimeout）← v3.0 修改
│   │   ├── types.ts                        # 类型定义
│   │   └── adapters/
│   │       ├── base.ts                     # Agent 适配器基类
│   │       ├── claude-code.ts              # Claude Code 适配器
│   │       └── hermes.ts                   # Hermes Agent 适配器
│   ├── dist/                               # 编译输出（node tsc.js 编译）← v3.0 修改
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

#### api_providers（API 提供商）

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

#### app_settings（应用设置）

| 字段         | 类型      | 约束                  | 说明                           |
| ---------- | ------- | ------------------- | ---------------------------- |
| key        | TEXT    | PRIMARY KEY         | 设置键（如 'pilotdesk-workspace'） |
| value      | TEXT    | NOT NULL DEFAULT '' | 设置值                          |
| updated_at | INTEGER | NOT NULL            | 更新时间                         |

#### install_logs（安装日志，v2.0 新增）

| 字段      | 类型    | 约束                                                   | 说明             |
| --------- | ------- | ------------------------------------------------------ | ---------------- |
| id        | INTEGER | PRIMARY KEY AUTOINCREMENT                              | 自增 ID          |
| timestamp | INTEGER | NOT NULL                                               | 日志时间戳        |
| message   | TEXT    | NOT NULL DEFAULT ''                                    | 日志消息内容      |
| level     | TEXT    | NOT NULL DEFAULT 'info', CHECK('info','warn','error','success') | 日志级别  |

```
CREATE INDEX IF NOT EXISTS idx_install_logs_time ON install_logs(timestamp);
```

> 日志自动清理：超过 7 天的日志在插入新日志时自动删除。

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
6. `migrate_add_install_logs` — 创建 install_logs 表（含自动清理 7 天过期日志）

---

## 5. Tauri IPC 命令注册（44 个）

### 5.1 命令分类

| 类别           | 命令                                                                                                                                                      | 数量  | 说明                |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | --- | ----------------- |
| **会话**       | list_sessions, list_archived_sessions, create_session, get_session, get_session_messages, rename_session, archive_session, delete_session, save_message | 9   | 会话 CRUD           |
| **灵感**       | list_inspirations, get_inspiration, create_inspiration, update_inspiration, delete_inspiration, search_inspirations, list_tags                          | 7   | 灵感 CRUD + 搜索 + 标签 |
| **API 提供商**  | list_api_providers, get_api_provider, upsert_api_provider, delete_api_provider, get_api_key, reorder_api_providers                                      | 6   | API 配置管理          |
| **应用设置**     | get_app_setting, set_app_setting                                                                                                                        | 2   | 通用 KV 设置          |
| **Bot 通道**   | list_bot_channels, save_bot_channel, delete_bot_channel                                                                                                 | 3   | Bot 管理            |
| **安装日志**     | insert_log, list_logs, clear_logs                                                                                                                       | 3   | 环境检测与安装日志      |
| **Agent 配置** | get_config, save_claude_config, save_hermes_config, get_agent_api_key                                                                                 | 4   | 配置读写 + API Key 读取 |
| **环境**       | detect_env, install_claude_code, install_hermes                                                                                                         | 3   | 环境检测与安装           |
| **更新检查**     | check_pilotdesk_update, check_single_npm, check_single_pypi                                                                                             | 3   | 版本更新检查            |
| **主题**       | get_theme, set_theme_cmd                                                                                                                                | 2   | 主题管理              |
| **其他**       | greet                                                                                                                                                   | 1   | 测试命令              |

### 5.2 命令总数统计

| 类别         | 数量  |
| ------------ | ----- |
| 会话         | 9     |
| 灵感         | 7     |
| API 提供商   | 6     |
| 应用设置     | 2     |
| Bot 通道     | 3     |
| 安装日志     | 3     |
| Agent 配置   | 4     |
| 环境         | 3     |
| 更新检查     | 3     |
| 主题         | 2     |
| 其他         | 1     |
| **合计**     | **44** |

### 5.3 Rust 端命令签名示例

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

// Agent 配置（config.rs — v3.1 新增）
#[tauri::command]
fn get_agent_api_key(agent_type: String) -> Result<Option<String>, AppError>
// 注：test_api_connection 已在 v3.1 移除
```

### 5.4 前端调用方式

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
│                  Zustand Stores                  │
│                                                 │
│  sessionStore          数据源: SQLite sessions   │
│  ├─ sessions[]         + messages 表             │
│  ├─ currentSessionId   invoke: list_sessions,    │
│  ├─ messages[]         create_session,           │
│  └─ CRUD ops           rename_session, ...       │
│                                                 │
│  apiProviderStore      数据源: SQLite            │
│  ├─ providers[]        api_providers 表          │
│  ├─ fetchProviders()   invoke: list/upsert/      │
│  ├─ saveProvider()     delete/reorder            │
│  ├─ deleteProvider()   _api_providers            │
│  └─ reorderProviders()                          │
│  └─ API_PROVIDERS fallback  ← v3.0              │
│                                                 │
│  getApiKey()             invoke: get_api_key     │
│  getApiEndpoint()        invoke: get_api_provider│
│                                                 │
│  inspirationStore      数据源: SQLite            │
│  ├─ inspirations[]      inspirations 表          │
│  └─ CRUD ops            invoke: create/update/..│
│                                                 │
│  pendingInputStore     数据源: 内存              │
│  └─ pendingInput        用于灵感→输入框桥接      │
│                                                 │
│  skillStore            数据源: WebSocket Sidecar │
│  ├─ agentSkills         从 Sidecar 加载          │
│  └─ setAgentSkills()                            │
│                                                 │
│  configStore            数据源: invoke           │
│  ├─ config              Agent 配置数据           │
│  ├─ fetchConfig()       启动时加载 ← v3.0       │
│  └─ ...                                         │
└─────────────────────────────────────────────────┘
```

### 6.2 apiProviderStore（v3.0 增强）

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

// API_PROVIDERS fallback 表（v3.0 新增）
// 当 invoke('list_api_providers') 失败时，使用内置 fallback 数据
// 确保 MessageBubble 能正常显示厂商名称
const API_PROVIDERS: Record<string, string> = {
  'anthropic': 'Anthropic',
  'openai': 'OpenAI',
  'google': 'Google',
  'zhipu': '智谱AI',
  'baidu': '百度',
  'aliyun': '阿里云',
  'deepseek': '深度求索',
  'moonshot': '月之暗面',
  'minimax': 'MiniMax',
  'zeroone': '零一万物',
};

// 独立工具函数
async function getApiKey(providerId: string): Promise<string | null>
async function getApiEndpoint(providerId: string): Promise<string | null>
```

### 6.3 configStore（v3.1 修改 — 移除 testConnection）

```typescript
interface ConfigState {
  config: AgentConfig | null;  // 初始为 null
  loading: boolean;
  fetchConfig: () => Promise<void>;  // 需在 App 启动时调用
  saveClaudeConfig: (config: ClaudeConfigPublic) => Promise<void>;
  saveHermesConfig: (config: HermesConfigPublic) => Promise<void>;
}
```

**关键变更：**
- `config` 初始值为 `null`。`fetchConfig()` 原本仅在 `ConfigEditor` 中调用，导致 `MessageBubble` 渲染时 `config` 为 `null`，无法读取 `config.claude.model` / `config.hermes.model`，Agent 消息名称中缺少模型名。v3.0 修复为在 `App.tsx` 启动时调用 `fetchConfig()`
- **v3.1 移除：** `TestResult` 接口、`testResult` 状态、`testConnection` 方法。连接测试功能已迁移到 ConfigEditor 组件内部，通过 `sendApiRequest` + `get_agent_api_key` 实现

---

## 7. 关键技术实现

### 7.1 API 直连会话

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

### 7.2 localStorage → SQLite 迁移

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

### 7.3 消息气泡布局与消息名称格式化（v3.0 修改）

#### 7.3.1 消息名称格式（v3.1 修复）

```
智能体名称 | 厂商 | 模型 时间
```

**示例：**
- `Hermes Agent | 智谱AI | GLM-4-Flash 2026/06/02 23:34`
- `Claude Code | 智谱AI | GLM-4-Flash 2026/06/02 23:34`
- `API | 智谱AI | GLM-4-Flash 2026/06/02 23:34`

**规则：**
- 日期始终显示完整格式（含前导零），如 `2026/06/02` 而非 `2026/6/2`
- 非当天消息显示完整年-月-日-时-分
- 当天消息显示时-分（如 `23:34`）

#### 7.3.2 buildAgentLabel 实现（v3.0 重写）

```typescript
// MessageBubble.tsx — 前端统一拼接消息名称
function buildAgentLabel(
  session: Session,
  config: AgentConfig | null,
  providers: ApiProvider[],
  timestamp: number
): string {
  const agentType = session.agentType;
  
  // 1. 确定智能体名称
  const agentName = agentType === 'claude' ? 'Claude Code'
    : agentType === 'hermes' ? 'Hermes Agent'
    : 'API';
  
  // 2. 确定厂商名称
  let providerName = '';
  if (agentType === 'api' && session.apiProvider) {
    // 从 providers 列表查找，或使用 API_PROVIDERS fallback
    const provider = providers.find(p => p.id === session.apiProvider);
    providerName = provider?.name || API_PROVIDERS[session.apiProvider] || session.apiProvider;
  } else if (agentType === 'claude') {
    providerName = config?.claude?.provider || '';
  } else if (agentType === 'hermes') {
    providerName = config?.hermes?.provider || '';
  }
  
  // 3. 确定模型名称
  const modelName = agentType === 'claude' ? config?.claude?.model
    : agentType === 'hermes' ? config?.hermes?.model
    : session.apiModel || '';
  
  // 4. 格式化时间
  const date = new Date(timestamp);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();
  const timeStr = isToday
    ? `${pad(date.getHours())}:${pad(date.getMinutes())}`
    : `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
  
  // 5. 拼接
  const parts = [agentName];
  if (providerName) parts.push(providerName);
  if (modelName) parts.push(modelName);
  return `${parts.join(' | ')} ${timeStr}`;
}
```

#### 7.3.3 消息气泡布局

```
┌────────────────────────────────────────────┐
│  Agent 消息（靠左）                         │
│  ┌──────────────────────────────────┐      │
│  │ 💬 Agent 回复内容...             │      │
│  └──────────────────────────────────┘      │
│                                            │
│              用户消息（靠右）               │
│         ┌──────────────────────┐           │
│         │ 💬 用户输入内容...   │ [编辑][收藏]│
│         └──────────────────────┘           │
└───────────────────────────────────────────┘

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

### 7.4 AgentBadge 首字母显示（v3.0 修改）

```typescript
// AgentBadge.tsx
// 根据 agentType 显示首字母：C (Claude) / H (Hermes) / A (API)
const labelMap: Record<string, string> = {
  claude: 'C',
  hermes: 'H',
  api: 'A',
};

// 颜色配置
const colorMap: Record<string, { bg: string; text: string }> = {
  claude: { bg: 'rgba(59,130,246,0.15)', text: '#3B82F6' },   // 蓝色
  hermes: { bg: 'rgba(139,92,246,0.15)', text: '#8B5CF6' },   // 紫色
  api:    { bg: 'rgba(16,185,129,0.15)', text: '#10B981' },   // 绿色
};
```

### 7.5 Typing Indicator 头像颜色动态化（v3.0 修复）

```typescript
// MessageList.tsx
// 修复前：硬编码蓝色 rgba(59,130,246,0.15)
// 修复后：根据 session.agentType 动态判断
const avatarBg = session.agentType === 'claude'
  ? 'rgba(59,130,246,0.15)'   // 蓝色
  : session.agentType === 'hermes'
    ? 'rgba(139,92,246,0.15)'  // 紫色
    : 'rgba(16,185,129,0.15)'; // 绿色
```

### 7.6 超时保护机制（v3.0 新增）

#### 7.6.1 Sidecar 端 withTimeout（120s）

```typescript
// server.ts
async function* withTimeout<T>(
  generator: AsyncGenerator<T>,
  timeoutMs: number
): AsyncGenerator<T> {
  let timer: NodeJS.Timeout;
  
  const timeoutPromise = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      reject(new Error(`Agent 响应超时 (${timeoutMs / 1000}s)`));
    }, timeoutMs);
  });

  try {
    while (true) {
      const result = await Promise.race([
        generator.next(),
        timeoutPromise,
      ]);
      
      if (result.done) break;
      yield result.value;
    }
  } finally {
    clearTimeout(timer!);
  }
}

// 使用方式
const stream = withTimeout(
  adapter.sendMessage(message, sessionId),
  120_000  // 120s 超时
);
for await (const chunk of stream) {
  // 处理每个 chunk
}
```

**设计要点：**
- 使用 `Promise.race` + `setTimeout` 实现
- 在 AsyncGenerator 的每个 `yield` 之间设置超时
- 超时时抛出明确错误消息，而非静默挂起
- `finally` 块确保定时器被清理

#### 7.6.2 前端超时（60s）

```typescript
// MainPanel.tsx
const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

// 启动超时
timeoutRef.current = setTimeout(() => {
  // 保存已收到的内容
  // 显示友好提示消息
  sendStop(sessionId);
  addMessage(sessionId, {
    role: 'system',
    content: `请求超时（60s），已保存当前回复内容。`,
  });
}, 60_000);

// 清除超时的四个入口
const clearTimeoutSafe = () => {
  if (timeoutRef.current !== null) {
    clearTimeout(timeoutRef.current);
    timeoutRef.current = null;
  }
};

// onDone → clearTimeoutSafe
// onError → clearTimeoutSafe
// stop → clearTimeoutSafe
// 新请求 → clearTimeoutSafe（先清除旧超时再启动新超时）
```

#### 7.6.3 双层保护策略

| 层级     | 超时时间 | 位置          | 行为                                       |
| ------ | ---- | ----------- | ---------------------------------------- |
| 前端     | 60s  | MainPanel   | 保存已收到内容，显示友好提示，发送 stop 信号               |
| Sidecar | 120s | server.ts   | 抛出明确错误消息，返回给前端                           |

前端超时先触发（60s），保存已收到内容并发送 stop。如果 stop 未及时生效，Sidecar 超时（120s）作为兜底，返回明确错误。

### 7.7 config 加载时机修复（v3.0 新增）

**问题：** `configStore.config` 初始为 `null`，`fetchConfig()` 仅在 `ConfigEditor` 中调用。当 `MessageBubble` 渲染时 `config` 为 `null`，导致 Agent 消息名称中缺少厂商和模型名。

**修复：** 在 `App.tsx` 启动时调用 `fetchConfig()`

```typescript
// App.tsx
import { useConfigStore } from './stores/configStore';

function App() {
  const fetchConfig = useConfigStore(s => s.fetchConfig);
  
  useEffect(() => {
    fetchConfig();
  }, [fetchConfig]);
  
  // ...
}
```

**设计决策：**
- 选择在 `App.tsx` 启动时调用，而非在 `MessageBubble` 挂载时
- 避免每个气泡都触发请求，减少网络开销
- 确保 `config` 在 `MessageBubble` 渲染前已加载

### 7.8 API 类型厂商名称显示修复（v3.0 新增）

**问题：** API 类型会话的消息名称中缺少厂商名称。

**根因：** `apiProviderStore.providers` 初始为空数组，`fetchProviders()` 仅在 `SettingsPage` 中调用。`MessageBubble` 渲染时无法从 providers 列表中找到对应的厂商名。

**修复：**
1. 在 `apiProviderStore.ts` 中添加 `API_PROVIDERS` fallback 映射表
2. 在 `SessionList.tsx` 等组件中自动调用 `fetchProviders()`
3. `buildAgentLabel` 中优先从 providers 列表查找，失败时使用 fallback

### 7.9 agentType 始终显示 hermes 的 bug 修复（v3.0 新增）

**问题：** 无论使用 claude 还是 hermes 建立会话，Agent 消息始终显示为 hermes 类型。

**根因：** `SessionList.tsx` 中 `useEffect` 依赖 `currentSession`，当 `currentSession` 变化时重复发送 `session:create` 消息，覆盖了正确的 agentType。

**修复：**
- 调整 `useEffect` 依赖项，避免 `currentSession` 变化时重复触发
- 确保 `session:create` 仅在真正创建新会话时发送一次

### 7.10 主题系统

```css
:root {
  --bg-primary: #1a1a2e;
  --bg-secondary: #16213e;
  --bg-tertiary: #0f3460;
  --text-primary: #e0e0e0;
  --text-secondary: #a0a0a0;
  --text-tertiary: #606060;
  --accent: #5B7FFF;
  --accent: #5B7FFF;
  --border: #2a2a4a;
  --success: #10B981;
  --hermes-tag: #8B5CF6;
}
```

- 深色/浅色/跟随系统三种模式
- Rust 端通过 `commands::theme` 读写主题配置
- `useTheme` hook 提供 `theme` / `setTheme`

### 7.11 WebSocket 通信协议

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

### 7.12 Sidecar 进程管理

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

### 7.13 Sidecar 编译方式（v3.0 变更）

**问题：** `npx tsc` 在 Windows 上输出为空（编码问题），实际编译错误被静默吞掉。

**修复：** 改用 `node` 直接运行 TypeScript 编译器：

```
F:\soft\nodejs\node.exe node_modules/typescript/lib/tsc.js
```

**编译命令：**
```bash
cd sidecar
F:\soft\nodejs\node.exe node_modules/typescript/lib/tsc.js
# 输出到 dist/ 目录
```

### 7.14 统一 AgentConfigForm（v3.1 新增）

**目标：** 消除 ClaudeConfigForm 和 HermesConfigForm 的重复实现，通过配置驱动方式统一组件。

#### 7.14.1 AGENT_FORM_CONFIGS 配置驱动

```typescript
const AGENT_FORM_CONFIGS: Record<AgentType, AgentFormConfig> = {
  claude: {
    type: 'claude',
    themeVar: AGENT_THEMES.claude.cssVar,
    defaultEndpoint: 'https://api.anthropic.com',
    defaultModel: 'claude-sonnet-4-20250514',
    providerId: 'anthropic',
    fields: [
      { key: 'model', label: '模型', type: 'text', placeholder: 'claude-sonnet-4-20250514' },
      { key: 'apiEndpoint', label: 'API 端点', type: 'text', placeholder: 'https://api.anthropic.com' },
      { key: 'apiKey', label: 'API Key', type: 'password', placeholder: '未设置', hint: '留空保持不变...' },
      { key: 'customInstructions', label: '自定义指令', type: 'textarea', placeholder: '...', rows: 3 },
      { key: 'maxTokens', label: '最大 Tokens', type: 'number', placeholder: '8192' },
    ],
  },
  hermes: {
    type: 'hermes',
    themeVar: AGENT_THEMES.hermes.cssVar,
    defaultEndpoint: 'https://api.siliconflow.cn/v1',
    defaultModel: 'deepseek-ai/DeepSeek-V3',
    providerId: 'openai',
    fields: [
      { key: 'model', label: '模型', type: 'text', placeholder: 'hermes-default' },
      { key: 'apiEndpoint', label: 'API 端点', type: 'text', placeholder: 'https://api.example.com/v1' },
      { key: 'apiKey', label: 'API Key', type: 'password', placeholder: '未设置', hint: '留空保持不变...' },
      { key: 'temperature', label: 'Temperature', type: 'number', placeholder: '0.7', min: 0, max: 1, step: 0.1 },
      { key: 'systemPrompt', label: '系统提示词', type: 'textarea', placeholder: '...', rows: 3 },
      { key: 'maxTokens', label: '最大 Tokens', type: 'number', placeholder: '8192' },
    ],
  },
};
```

**设计要点：**
- `AgentFormConfig` 接口定义字段列表、默认值、主题色、providerId
- `AgentConfigForm` 组件根据 `agentType` 从配置中读取字段定义，动态渲染表单
- 表单状态管理使用独立的 `useState` hooks，通过 `renderField` 函数统一渲染
- `handleSave` 仅提交有变更的字段（diff 比较），未修改字段不发送
- `handleTestConnection` 调用 `sendApiRequest` + `get_agent_api_key` 实现

#### 7.14.2 连接测试复用

**v3.0 实现（已移除）：**
- Rust 端：`test_api_connection` 命令，独立实现 Claude/Hermes HTTP 请求逻辑
- 前端：`configStore.testConnection` 方法，调用 `invoke('test_api_connection')`
- 问题：与 `apiClient.ts` 的 `sendApiRequest` 存在重复实现

**v3.1 实现（当前）：**
- Rust 端：新增 `get_agent_api_key` 命令，从 claude.rs/hermes.rs 读取原始 API Key
- 前端：`ConfigEditor.handleTestConnection` 直接调用 `sendApiRequest`
- 优势：统一 API 调用逻辑，减少重复代码，降低维护成本

```typescript
const handleTestConnection = async () => {
  const key = apiKey || await invoke<string | null>('get_agent_api_key', { agentType });
  if (!key) {
    setTestMsg('未配置 API Key');
    return;
  }
  const result = await sendApiRequest({
    endpoint: ep,
    providerId: cfg.providerId,
    apiKey: key,
    model: mdl,
    messages: [{ role: 'user', content: 'hi' }],
    maxTokens: 1,
    timeout: 15000,
  });
  setTestOk(result.ok);
  setTestMsg(result.ok
    ? `连接成功 - 模型: ${mdl} (${result.latency}ms)`
    : result.message);
};
```

#### 7.14.3 按钮布局优化

**问题：** 错误提示内容较长时，保存设置和测试连接按钮被挤压换行。

**修复：**
- 按钮添加 `whitespace-nowrap` 类，防止文本换行
- 测试结果改为独立 `status banner`（`mt-2 px-3 py-2 rounded-lg`），带背景色和边框
- 成功：绿色背景 `rgba(52, 211, 153, 0.08)` + 绿色边框
- 失败：红色背景 `rgba(239, 68, 68, 0.08)` + 红色边框

---

### 7.15 会话消息持久化（v3.0 新增）

**设计目标：** 消息写入 SQLite 不阻塞 UI 渲染，同时避免重复写入。

**实现方式：** fire-and-forget 异步写入 + 去重保护。

```typescript
// sessionStore.ts — addMessage
addMessage: (msg: Message) => {
  // 1. 去重：与最后一条消息比较，相同角色+内容+时间差≤2ms 则跳过
  const skip = useSessionStore.getState().messages.slice(-1)[0];
  if (skip && skip.role === msg.role && skip.content === msg.content && Math.abs(skip.timestamp - msg.timestamp) <= 2) {
    return;
  }

  // 2. 立即更新内存状态（UI 即时响应）
  set((state) => ({ messages: [...state.messages, msg] }));

  // 3. 异步写入数据库（不阻塞 UI）
  invoke<Message>('save_message', {
    sessionId: msg.sessionId,
    role: msg.role,
    content: msg.content,
    mode: msg.mode,
  }).then((saved) => {
    // 4. 更新会话列表预览和计数
    set((state) => ({
      sessions: state.sessions.map((s) =>
        s.id === msg.sessionId
          ? { ...s, lastMessagePreview: saved.content.slice(0, 100), messageCount: s.messageCount + 1, updatedAt: saved.timestamp }
          : s
      ),
    }));
  }).catch((err) => {
    console.error('Failed to persist message:', err);
  });
},
```

**关键设计决策：**

| 决策 | 说明 |
| ---- | ---- |
| fire-and-forget | `invoke('save_message')` 不 await，UI 无需等待数据库写入完成 |
| 去重保护 | 比较最后一条消息的角色+内容+时间戳，避免 SSE 流式输出中的重复 chunk 写入 |
| 异步更新预览 | 写入成功后异步更新会话列表的 `lastMessagePreview` 和 `messageCount` |
| 错误容忍 | 写入失败仅 console.error，不中断用户操作 |

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
| tsc 编译      | `F:\soft\nodejs\node.exe node_modules/typescript/bin/tsc.js`                  | Sidecar 编译（非 npx）← v3.0 |
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
- [x] 会话模式切换（Claude Code / Hermes Agent / API 直连）
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

### 12.8 Phase 8 — 稳定性与显示修复（v3.0 新增） ✅

- [x] **config 加载时机修复**：App.tsx 启动时调用 fetchConfig()，确保 MessageBubble 渲染前 config 已加载
- [x] **消息名称格式化**：buildAgentLabel 在前端统一拼接，格式为 `智能体名称 | 厂商 | 模型 时间`
- [x] **AgentBadge 首字母显示**：C (Claude) / H (Hermes) / A (API)
- [x] **双层超时保护**：前端 60s + Sidecar 120s，超时保存已收到内容并显示友好提示
- [x] **Typing indicator 颜色动态化**：claude 蓝色 / hermes 紫色 / api 绿色
- [x] **API 类型厂商名称显示**：添加 API_PROVIDERS fallback + 自动 fetchProviders
- [x] **agentType 始终显示 hermes 的 bug 修复**：调整 useEffect 依赖，避免重复 session:create
- [x] **Sidecar 编译方式修复**：node tsc.js 替代 npx tsc（解决 Windows 编码问题）
- [x] **日期格式统一**：始终显示完整格式（含前导零），如 `2026/06/02`
- [x] **安装日志系统**：install_logs 表 + insert_log/list_logs/clear_logs 3 个 IPC 命令
- [x] **更新检查**：check_pilotdesk_update / check_single_npm / check_single_pypi 3 个 IPC 命令
### 12.9 Phase 9 — 统一 Agent 颜色体系与白屏修复（v3.0 追加） ✅
### 12.10 Phase 10 — 统一 AgentConfigForm 与连接测试复用（v3.1 追加） ✅

- [x] **ClaudeConfigForm / HermesConfigForm 合并为统一 AgentConfigForm**：配置驱动设计，AGENT_FORM_CONFIGS 对象定义字段列表、默认值、主题色、providerId，代码量减少约 35%
- [x] **连接测试复用 sendApiRequest**：移除 Rust 端 test_api_connection + TestConnectionResult 重复实现，前端通过 get_agent_api_key 获取原始 Key 后直接调用 sendApiRequest
- [x] **新增 get_agent_api_key Rust 命令**：从 claude.rs/hermes.rs 读取原始 API Key 返回前端，解决前端无法直接获取 Rust 端保存的 Key 的问题
- [x] **按钮布局优化**：whitespace-nowrap 防止按钮被挤压换行，测试结果改为独立 status banner（mt-2 px-3 py-2 rounded-lg，带背景色和边框）
- [x] **消息名称显示修复**：按 `智能体名称 | 厂商 | 模型 时间` 格式统一显示，buildAgentLabel 在前端拼接
- [x] **Hermes 会话超时友好提示**：错误消息以消息气泡形式持久展示在会话窗口中，而非仅 console.error
- [x] **移除软件右下角错误检测信息框**
- [x] **Agent 环境检测 stdout/stderr 管道错误处理**：捕获 os error 232 避免 panic
- [x] **环境检测去重**：修复自动检测多次重复触发的问题
- [x] **Agent 参数配置读写**：实现配置文件的读取回填与保存更新



- [x] **AGENT_THEMES 集中颜色体系**：在 `src/types/index.ts` 中定义 `AgentTheme` 接口和 `AGENT_THEMES` 配置（claude 蓝色 #3B82F6 / hermes 紫色 #8B5CF6 / api 绿色 #10B981 / manual 灰色 #6B7280），替换分散在 16 个组件中的硬编码颜色
- [x] **白屏修复**：ErrorBoundary 中 `onClick={handleReset}` 缺少 `this.` 导致 fallback 渲染时再次崩溃，React 卸载整个组件树。修复为 `onClick={this.handleReset}`
- [x] **InspirationPanel sourceColor/sourceLabel 修复**：重构后 `sourceColor`/`sourceLabel` 从对象变为字符串，`sourceColor[inspiration.sourceAgent]` 按对象访问导致 `Cannot read properties of undefined`。修复为直接使用字符串值
- [x] **AGENT_THEMES is not defined 修复**：6 个组件（SessionList.tsx, SkillPicker.tsx, InputBar.tsx, ConfigEditor.tsx, MemoryBrowser.tsx, SkillBrowser.tsx）引用了 AGENT_THEMES 但未 import。批量添加 `import { AGENT_THEMES } from '../../types'`
- [x] **CSS 变量补充**：在 `globals.css` 中添加 `--api-tag: #10B981`，亮/暗双主题支持
- [x] **CSP 配置调整**：添加 `'unsafe-inline'` `'unsafe-eval'` 和 Vite 开发服务器 URL 到 CSP，确保 WebView2 正确加载内联脚本和 HMR
- [x] **移除 beforeDevCommand**：避免 `npm run dev` 通过 WPS 灵犀路径启动失败，改为手动管理 Vite 开发服务器
- [x] **窗口透明关闭**：设置 `transparent: false`，避免窗口背景透明导致的渲染问题
- [x] **诊断代码定位白屏根因**：在 main.tsx 添加全局错误捕获 + mount 指示器，通过截图确认 ErrorBoundary 错误页面，定位到 InspirationRow 函数中的类型错误

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

### 13.3 架构优化

- [ ] 超时时间可配置化（设置页）

### 13.4 打包发布

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

## 15. 变更日志

| 版本    | 日期         | 变更内容                                                                     |
| ------- | ------------ | ---------------------------------------------------------------------------- |
| v1.0    | 2026-05-31   | 初始版本，基础架构 + 会话系统 + 灵感系统 + API 配置                          |
| v2.0    | 2026-06-01   | API 直连会话 + localStorage→SQLite 迁移 + 设置页重写 + 命名规范统一          |
| **v3.1** | **2026-06-03** | **统一 AgentConfigForm + 连接测试复用 sendApiRequest + 新增 get_agent_api_key + 按钮布局优化 + 消息名称显示修复 + Hermes 超时友好提示 + 移除错误检测框 + 环境检测管道错误修复 + 环境检测去重 + Agent 参数配置读写** |
| **v3.0** | **2026-06-03** | **config 加载修复 + 消息名称格式化 + AgentBadge 首字母 + 超时保护 + typing indicator 修复 + API 厂商名显示 + agentType bug 修复 + Sidecar 编译修复 + install_logs 表 + 更新检查命令 + 会话消息持久化** |

---

*Copyright (c) 2026 PilotDesk by 简意工作室 (jorryn)*
*本项目代码基于 MIT 协议开源*
