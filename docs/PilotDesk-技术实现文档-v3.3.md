# PilotDesk 技术实现文档 v3.3

> 版本: v3.3 | 作者: 简意工作室 (jorryn) | 日期: 2026-06
> 基于 v3.2 更新，反映截至 2026-06-14 的完整技术架构与实现细节

> **v3.3 新增内容：**
> - **对话模式提示词用户自定义**：系统提示词从硬编码迁移到 SQLite 存储（`app_settings` 表），支持用户在设置页编辑修改
>   - 前端 `getModePrompt()` / `saveModePrompt()` 函数管理存储读写
>   - 后端 `InputBar` tooltip 增加模式编码显示（如 `fast`、`think`）
>   - Sidecar `buildPrompt()` 接受 `systemPrompt` 参数，优先使用前端传入的自定义提示词
>   - `settings` 页新增"对话模式"Tab（`ModePromptSettings` 组件）
> - **对话模式选择器 UI 优化**：tooltip 显示模式编码 + 提示词，下拉面板显示模式编码标签
>
> **v3.2 新增内容（保留）：**
> - **Hermes YAML 兼容性修复**：load() 改为 YAML 优先（config.yaml > config.json > .env），save() 同时更新 config.yaml（新版本）和 config.json（旧版本）
> - **Agent 配置表单切换/刷新修复**：key={activeAgent} 强制组件重新挂载解决切换时 state 残留；refreshKey 计数器解决同 agent 刷新不生效
> - **保存反馈优化**：savedMsg state + 绿色提示条实现保存反馈（3s 自动消失）
> - **字段重新编排**：顺序为 Base URL → API Key → 模型 → 自定义系统提示词；统一 systemPrompt 字段；去除 Temperature、最大 Tokens；允许空值保存
> - **HermesConfigPublic rename_all**：添加 `#[serde(rename_all = "camelCase")]` 修复 Hermes 字段序列化
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
    │   获取系统提示词: getModePrompt(mode) ← v3.3
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
        useWebSocket.sendApiChat(sessionId, message, apiEndpoint, ...
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
    │                        │   - setTimeout(120s)
    │ onDone/onError/stop    │
    │ 时清除 timeout         │ 超时 → throw new Error("超时")
    │                        │
    │ 超时 → 保存已收到内容   │
    │ 显示友好提示消息        │
```

### 2.4 对话模式提示词流程（v3.3 新增）

```
用户选择模式 → InputBar 显示 tooltip（模式编码 + 默认提示词）
    ↓
用户发送消息 → MainPanel.handleSend()
    ↓
getModePrompt(mode) → 查询 app_settings 表
    ├── 存在用户自定义 → 返回自定义提示词
    └── 不存在 → 返回 MODE_PROMPTS_DEFAULTS[mode]
    ↓
sendChat(sessionId, message, mode, agentType, cwd, systemPrompt)
    ↓
WebSocket → Sidecar server.ts
    ↓
buildPrompt(message, mode, systemPrompt) → base.ts
    ├── systemPrompt 存在 → 使用自定义提示词
    └── systemPrompt 为空 → 使用 MODE_PROMPTS[mode] 默认值
    ↓
格式化: `[系统指令：${prompt}]\n\n${message}`
    ↓
发送给 Agent (Claude Code / Hermes Agent)
```

用户自定义提示词：
```
设置页 → 对话模式 Tab → ModePromptSettings 组件
    ↓
编辑 textarea → saveModePrompt(mode, prompt)
    ↓
invoke('set_app_setting', { key: 'mode_prompt_fast', value: '...' })
    ↓
SQLite app_settings 表存储
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
│   │   │   ├── InputBar.tsx                # 输入栏（模式切换 + 灵感/技能选择 + tooltip 编码显示）← v3.3 修改
│   │   │   ├── RightPanel.tsx              # 右侧面板（灵感/技能/配置/记忆/Bot）
│   │   │   ├── InspirationPanel.tsx        # 灵感面板
│   │   │   └── StatusBar.tsx               # 状态栏（WebSocket 连接状态）
│   │   ├── message/
│   │   │   ├── MessageBubble.tsx           # 消息气泡（buildAgentLabel 前端拼接）← v3.0 修改
│   │   │   ├── MessageList.tsx             # 消息列表（typing indicator 颜色动态化）← v3.0 修改
│   │   │   └── MarkdownRenderer.tsx        # Markdown 渲染器
│   │   └── panels/
│   │       ├── ConfigEditor.tsx            # Agent 参数配置编辑器
│   │       ├── ModePromptSettings.tsx      # 对话模式提示词设置（v3.3 新增）← v3.3 新增
│   │       ├── SkillBrowser.tsx            # 技能浏览器
│   │       ├── MemoryBrowser.tsx           # 记忆浏览器
│   │       ├── BotSetup.tsx                # Bot 通道配置
│   │       └── UpdateChecker.tsx           # 更新检查
│   ├── hooks/
│   │   ├── useWebSocket.ts                 # WebSocket hook + SSE 流式 + API直连 ← v3.3 新增 systemPrompt 参数
│   │   ├── useTheme.ts                     # 主题切换 hook
│   │   └── useTauriCommand.ts             # Tauri command 通用调用 hook
│   ├── pages/
│   │   ├── SettingsPage.tsx                # 设置页（6 Tab，新增"对话模式"）← v3.3 修改
│   │   └── EnvPage.tsx                     # 环境配置独立页面
│   ├── stores/                             # Zustand 状态管理
│   │   ├── sessionStore.ts                 # 会话状态（invoke → SQLite）
│   │   ├── apiProviderStore.ts             # API 提供商状态（含 API_PROVIDERS fallback）← v3.0 修改
│   │   ├── inspirationStore.ts            # 灵感状态（invoke → SQLite）
│   │   ├── pendingInputStore.ts            # 输入桥接（Zustand 内存）
│   │   ├── skillStore.ts                   # 技能状态
│   │   └── configStore.ts                  # 配置状态（fetchConfig 启动时加载）← v3.0 修改
│   ├── types/
│   │   └── index.ts                        # 全局类型定义 ← v3.3 修改：MODE_PROMPTS 改为动态加载
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
│   │   │                                 #   + systemPrompt 透传 ← v3.3 新增
│   │   ├── types.ts                        # 类型定义 ← v3.3 修改
│   │   └── adapters/
│   │       ├── base.ts                     # Agent 适配器基类（buildPrompt 接受 systemPrompt）← v3.3 修改
│   │       ├── claude-code.ts              # Claude Code 适配器 ← v3.3 修改
│   │       └── hermes.ts                   # Hermes Agent 适配器 ← v3.3 修改
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
| key        | TEXT    | PRIMARY KEY         | 设置键                          |
| value      | TEXT    | NOT NULL DEFAULT '' | 设置值                          |
| updated_at | INTEGER | NOT NULL            | 更新时间                         |

> **v3.3 新增用途**：存储对话模式自定义提示词
> - `mode_prompt_native` — 原生模式提示词
> - `mode_prompt_fast` — 快速模式提示词
> - `mode_prompt_think` — 深度思考模式提示词
> - `mode_prompt_expert` — 专家模式提示词
> - 空值或不存在时使用内置默认值

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
| **应用设置**     | get_app_setting, set_app_setting                                                                                                                        | 2   | 通用 KV 设置（含模式提示词 v3.3） |
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
│  inspirationStore      数据源: SQLite            │
│  ├─ inspirations[]   inspirations + tags 表      │
│  ├─ fetch()          invoke: list/create/...     │
│  └─ search()         invoke: search_inspirations │
│                                                 │
│  pendingInputStore     Zustand 内存              │
│  ├─ pendingInput     从 MainPanel → InputBar    │
│  └─ consume()        消耗输入并清空              │
│                                                 │
│  skillStore            Zustand 内存 + Sidecar   │
│  ├─ skills[]           WebSocket skills:list    │
│  └─ fetch()            requestSkills()          │
│                                                 │
│  configStore           内存缓存                  │
│  ├─ claudeConfig       fetchConfig() 启动加载    │
│  ├─ hermesConfig       invoke('get_config')     │
│  └─ fetchConfig()      更新所有 agent 配置       │
└─────────────────────────────────────────────────┘
```

---

## 7. 对话模式系统（v3.3 新增）

### 7.1 类型定义

```typescript
// types/index.ts
export type ChatMode = 'native' | 'fast' | 'think' | 'expert';

// 模式标签
export const MODE_LABELS: Record<ChatMode, string> = {
  native: '原生',
  fast: '快速',
  think: '深度思考',
  expert: '专家',
};

// 模式默认提示词
export const MODE_PROMPTS_DEFAULTS: Record<ChatMode, string> = {
  native: '',
  fast: '快速简洁回答，直接给出结论，无需详细解释推理过程',
  think: '逐步分析推理，详细解释你的思路和过程，给出完整的推理链',
  expert: '以资深专家的视角，全面深入分析，考虑各种边界情况和潜在风险，给出专业的建议和方案',
};

// 模式颜色
export const MODE_COLORS: Record<ChatMode, string> = {
  native: 'var(--text-secondary)',
  fast: '#10B981',
  think: 'var(--accent)',
  expert: 'var(--hermes-tag)',
};
```

### 7.2 提示词存储与加载

```typescript
// types/index.ts
/** 获取某个模式的系统提示词（从存储或默认值） */
export async function getModePrompt(mode: ChatMode): Promise<string>

/** 保存某个模式的自定义提示词 */
export async function saveModePrompt(mode: ChatMode, prompt: string): Promise<void>

/** 一次性获取所有模式提示词 */
export async function getAllModePrompts(): Promise<Record<ChatMode, string>>
```

存储机制：通过 Tauri IPC `get_app_setting` / `set_app_setting` 写入 `app_settings` 表，键名为 `mode_prompt_{mode}`。

### 7.3 前端组件

- **InputBar.tsx**：模式选择器按钮 + 下拉面板
  - tooltip 显示 `${mode} — ${提示词}`
  - 下拉项显示模式标签 + 编码（如 `快速 (fast)`）
  - 使用 `MODE_PROMPTS_DEFAULTS` 展示默认提示词

- **MainPanel.tsx**：`handleSend` 中调用 `getModePrompt(mode)` 获取提示词，通过 `sendChat` 的 `systemPrompt` 参数传递给 Sidecar

- **ModePromptSettings.tsx**（v3.3 新增）：设置页"对话模式"Tab
  - 四个模式的编辑区域
  - 单条保存 / 重置 / 保存全部按钮
  - 保存反馈状态

### 7.4 Sidecar 适配器链路

```typescript
// types.ts — ChatRequest 新增 systemPrompt 字段
interface ChatRequest {
  sessionId: string;
  message: string;
  mode: ChatMode;
  cwd?: string;
  systemPrompt?: string;  // v3.3 新增
}

// server.ts — 透传 systemPrompt
const request = {
  sessionId: msg.sessionId,
  message: msg.message || '',
  mode: msg.mode || 'native',
  cwd: msg.cwd,
  systemPrompt: msg.systemPrompt,  // v3.3 新增
};

// adapters/base.ts — buildPrompt 接受 systemPrompt
export function buildPrompt(message: string, mode: ChatMode, systemPrompt?: string): string {
  const effectivePrompt = systemPrompt ?? MODE_PROMPTS[mode];
  return effectivePrompt ? `[系统指令：${effectivePrompt}]\n\n${message}` : message;
}
```

### 7.5 WebSocket 通信

```typescript
// useWebSocket.ts — sendChat 新增 systemPrompt 参数
const sendChat = (
  sessionId: string,
  message: string,
  mode?: string,
  agentType?: string,
  cwd?: string,
  systemPrompt?: string,  // v3.3 新增
) => {
  sendMessage({ type: 'chat', sessionId, message, mode, agentType, cwd, systemPrompt });
};
```

---

## 8. Sidecar 架构细节

### 8.1 WebSocket 消息协议

```typescript
// 前端 → Sidecar
interface WsMessage {
  type: 'chat' | 'stop' | 'session:create' | 'session:close' | 'ping' | 'skills:list' | 'skills:list-all';
  sessionId: string;
  agentType?: 'claude' | 'hermes';
  message?: string;
  mode?: ChatMode;
  cwd?: string;
  systemPrompt?: string;  // v3.3 新增
}

// Sidecar → 前端
interface ChatChunk {
  type: 'chunk' | 'done' | 'error' | 'status' | 'skills';
  sessionId: string;
  content?: string;
  error?: string;
  status?: string;
}
```

### 8.2 适配器基类

```typescript
interface AgentAdapter {
  agentType: 'claude' | 'hermes';
  createSession(sessionId: string, cwd?: string): Promise<void>;
  closeSession(sessionId: string): Promise<void>;
  sendMessage(request: ChatRequest): AsyncGenerator<string, void, unknown>;
  stopGeneration(sessionId: string): void;
  listSkills(): Promise<SkillInfo[]>;
}
```

---

## 9. 版本更新历史

| 版本 | 日期       | 内容摘要                                           |
| ---- | ---------- | ------------------------------------------------ |
| v3.3 | 2026-06-14 | 对话模式提示词用户自定义、模式选择器 tooltip 优化    |
| v3.2 | 2026-06-03 | Hermes YAML 兼容、Agent 配置表单修复、保存反馈优化   |
| v3.0 | 2026-05    | 双层超时、会话消息持久化、Sidecar 编译变更等        |
| v2.0 | —          | 安装日志表、Bot 通道、UpdateChecker               |
