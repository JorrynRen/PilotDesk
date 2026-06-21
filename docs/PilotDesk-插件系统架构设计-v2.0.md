# PilotDesk 插件系统架构设计 v2.0

> 更新时间：2026-06-21
> 状态：已实现（v1.0）+ 规划中（v2.0 五个扩展方向）
> 本文档整合了插件系统核心架构、能力扩展、工作流编排、在线商店等全部设计

---

## 1. 概述

### 1.1 设计目标

| 目标 | 说明 |
|------|------|
| **可扩展** | 第三方开发者可独立开发插件，无需修改核心代码 |
| **安全** | 插件运行在沙箱环境中，不能直接访问系统资源 |
| **轻量** | 插件加载和卸载对主应用性能影响最小化 |
| **统一** | 所有插件遵循相同的 API 规范 |
| **可编排** | 插件贡献点可作为工作流节点被编排调度（v2.0） |
| **可发现** | 插件可通过在线商店浏览和安装（v2.0） |

### 1.2 版本里程碑

| 版本 | 日期 | 变更说明 | Git Commit |
|------|------|---------|------------|
| v1.0 | 2026-06-21 | 初始架构：面板扩展 + 沙箱 + 权限系统 | - |
| v2.0 | 2026-06-21 | 整合工作流编排 + 系统能力扩展 + 在线商店 + 实现路线图 | `e01b8eb` |

---

## 2. 系统架构总览

### 2.1 整体架构

```
+----------------------------------------------------------------------+
|                         PilotDesk Core                                |
+----------------------------------------------------------------------+
|                                                                      |
|  +------------------+  +------------------+  +-------------------+  |
|  |  PluginHost       |  |  PluginAPI       |  |  PluginStore      |  |
|  |  (Rust)           |  |  (运行时)        |  |  (Zustand)        |  |
|  |  发现/验证/沙箱   |  |  UI/Data/Event   |  |  状态管理         |  |
|  +--------+---------+  +--------+---------+  +---------+---------+  |
|           |                     |                       |            |
|  +--------v---------------------v-----------------------v---------+  |
|  |                    PluginRegistry                              |  |
|  |  面板组件注册 + JS 执行 + 生命周期 + 命令调度 + 事件分发       |  |
|  +----------------------------------------------------------------+  |
|                                                                      |
|  +----------------------------------------------------------------+  |
|  |              扩展模块（v2.0 新增）                              |  |
|  |  +------------+ +------------+ +------------+ +--------------+ |  |
|  |  | GlobalEvent| | FS/Shell   | | Workflow   | | OnlinePlugin | |  |
|  |  | Bus        | | Agent API  | | Engine     | | Store        | |  |
|  |  +------------+ +------------+ +------------+ +--------------+ |  |
|  +----------------------------------------------------------------+  |
+----------------------------------------------------------------------+
```

### 2.2 核心模块职责

| 模块 | 层级 | 职责 | 状态 |
|------|------|------|------|
| `PluginHost` | Rust | 插件发现、manifest 验证、沙箱检查、Tauri 命令 | 已实现 |
| `PluginAPI` | TS | 插件运行时 API（ui/data/events/storage） | 已实现 |
| `PluginRegistry` | TS | 面板组件注册、JS 执行、生命周期管理 | 已实现 |
| `PluginStore` | TS | Zustand 状态管理（插件列表 + 注册数据） | 已实现 |
| `CommandDispatcher` | TS | 命令调度执行、参数校验、返回值处理 | 规划中 |
| `EventDispatcher` | TS | 核心事件分发到插件 hook handler | 规划中 |
| `GlobalEventBus` | TS | 跨插件发布/订阅通信 | 规划中 |
| `FS/Shell/Agent API` | Rust+TS | 系统能力扩展 | 规划中 |
| `WorkflowEngine` | Rust+TS | 工作流定义解析、状态机、步骤执行 | 规划中 |
| `OnlinePluginStore` | Rust+TS | 在线商店浏览、安装、更新 | 规划中 |

### 2.3 核心变化（v1.0 → v2.0）

| 维度 | v1.0 | v2.0 |
|------|------|------|
| 贡献点用途 | UI 扩展（面板） | UI 扩展 + 工作流节点 |
| 命令状态 | 注册但不可执行 | 可调度执行，支持参数传递和返回值 |
| 事件钩子 | 声明但未分发 | 核心事件分发到插件 handler |
| 插件通信 | 无（插件内隔离） | 全局事件总线，支持发布/订阅 |
| 工作流 | 不支持 | 独立工作面板 + 工作流引擎 |
| 插件分发 | 仅本地 zip 安装 | 在线商店 + CI 自动索引 |
| 系统能力 | 仅 UI/Storage | 新增 FS/Shell/Agent API |

---

## 3. 插件生命周期

```
发现 (Discovery) → 加载 (Load) → 执行 JS → 运行 (Run) → 卸载 (Unload)
```

### 3.1 发现
- 扫描 `~/.pilotdesk/plugins/` 目录
- 每个插件是一个独立目录，包含 `manifest.json` + 代码文件
- 支持三种安装方式：
  - 管理面板上传 .zip 压缩包（已实现）
  - 在线商店一键安装（规划中）
  - 手动复制目录到 plugins_dir（已实现）

### 3.2 manifest.json 规范

```json
{
  "id": "hello-world",
  "name": "Hello World",
  "version": "1.0.0",
  "description": "示例插件",
  "author": "PilotDesk Team",
  "minAppVersion": "0.1.0",
  "permissions": ["ui:panel", "ui:toast"],
  "entry": { "main": "index.js" },
  "contributes": {
    "panels": [
      { "id": "hello-panel", "title": "Hello World", "icon": "icon.png" }
    ],
    "commands": [
      {
        "id": "hello.say",
        "title": "Say Hello",
        "input": {
          "type": "object",
          "properties": {
            "message": { "type": "string", "description": "要发送的消息" }
          }
        },
        "output": {
          "type": "object",
          "properties": {
            "result": { "type": "string" }
          }
        }
      }
    ],
    "hooks": [
      { "event": "message:before-send", "handler": "onBeforeSend" }
    ]
  }
}
```

### 3.3 贡献点字段说明

| 贡献点 | 字段 | 类型 | 必填 | 说明 |
|--------|------|------|------|------|
| `panels[].id` | string | 是 | 面板唯一标识 |
| `panels[].title` | string | 是 | 面板显示名称 |
| `panels[].icon` | string | 否 | 图标（网络URL/插件本地路径/空） |
| `commands[].id` | string | 是 | 命令唯一标识，如 `hello.say` |
| `commands[].title` | string | 是 | 命令显示名称 |
| `commands[].input` | JSON Schema | 否 | 命令输入参数 schema |
| `commands[].output` | JSON Schema | 否 | 命令输出 schema |
| `hooks[].event` | string | 是 | 监听的事件名称 |
| `hooks[].handler` | string | 是 | 事件处理函数名 |

### 3.4 icon 字段格式

| 格式 | 示例 | 渲染方式 |
|------|------|---------|
| 网络图片 | `"https://example.com/icon.png"` | `<img>` 标签 |
| 插件本地路径 | `"image/favicon.png"` | 拼接插件目录 + `convertFileSrc()` |
| 空/未定义 | 省略或 `""` | 显示默认图标 |

---

## 4. Plugin API

### 4.1 v1.0 API（已实现）

```typescript
interface PluginAPI {
  ui: {
    addPanel(config: PanelContribution & { component: React.ComponentType }): void;
    removePanel(id: string): void;
    showToast(message: string, type: 'info' | 'success' | 'error'): void;
  };
  data: {
    invoke<T>(cmd: string, params?: Record<string, unknown>): Promise<T>;
  };
  events: {
    on(event: string, handler: (...args: any[]) => void): () => void;
    emit(event: string, ...args: any[]): void;
  };
  storage: {
    get(key: string): Promise<string | null>;
    set(key: string, value: string): Promise<void>;
    delete(key: string): Promise<void>;
  };
}
```

### 4.2 v2.0 API 扩展（规划中）

```typescript
interface PluginAPI {
  // v1.0 已有
  ui: { ... };
  data: { ... };
  events: { ... };
  storage: { ... };

  // v2.0 新增：跨插件通信
  global: {
    on(event: string, handler: (payload: any) => void): () => void;
    emit(event: string, payload?: any): void;
    call<T>(pluginId: string, commandId: string, params?: any): Promise<T>;
  };

  // v2.0 新增：文件系统（沙箱禁用时可用）
  fs: {
    readText(path: string): Promise<string>;
    readBinary(path: string): Promise<string>;
    writeText(path: string, content: string): Promise<void>;
    writeBinary(path: string, content: string): Promise<void>;
    appendText(path: string, content: string): Promise<void>;
    delete(path: string): Promise<void>;
    createDir(path: string): Promise<void>;
    removeDir(path: string): Promise<void>;
    exists(path: string): Promise<boolean>;
    readDir(path: string): Promise<FileEntry[]>;
    copy(src: string, dest: string): Promise<void>;
    move(src: string, dest: string): Promise<void>;
    stat(path: string): Promise<FileStat>;
  };

  // v2.0 新增：Shell 命令（沙箱禁用 + 二次确认）
  shell: {
    exec(command: string, options?: ShellExecOptions): Promise<ShellResult>;
    spawn(command: string, options?: ShellSpawnOptions): ShellProcess;
  };

  // v2.0 新增：Agent 会话
  agent: {
    createSession(agentType: string, options?: SessionOptions): Promise<SessionInfo>;
    sendMessage(sessionId: string, content: string): Promise<AgentResponse>;
    sendMessageStream(sessionId: string, content: string, callbacks: StreamCallbacks): Promise<void>;
    getHistory(sessionId: string): Promise<Message[]>;
    listSessions(): Promise<SessionInfo[]>;
    deleteSession(sessionId: string): Promise<void>;
    listAgents(): Promise<AgentInfo[]>;
  };
}
```

### 4.3 插件入口格式

```javascript
// index.js — 纯 JS，通过 React.createElement 构建 UI
function MyPanel() {
  return React.createElement('div', null,
    React.createElement('h3', null, 'Hello from Plugin!')
  );
}

export default {
  onLoad: function(api) {
    api.ui.addPanel({ id: 'my-panel', title: 'My Plugin', component: MyPanel });
  },
  onUnload: function() {
    // 清理逻辑
  },
};
```

---

## 5. 安全模型

### 5.1 权限清单

| 权限 | 说明 | 风险等级 | 沙箱启用 | 沙箱禁用 | 默认 |
|------|------|---------|---------|---------|------|
| `ui:panel` | 添加/移除面板 | 低 | 放行 | 放行 | 需声明 |
| `ui:toast` | 显示通知 | 低 | 放行 | 放行 | 默认授权 |
| `ui:modal` | 打开模态框 | 低 | 放行 | 放行 | 需声明 |
| `storage:*` | 插件独立存储 | 低 | 放行 | 放行 | 默认授权 |
| `workflow:read` | 读取工作流定义和状态 | 低 | 放行 | 放行 | 需声明 |
| `session:read` | 读取会话和消息 | 中 | 提示 | 提示 | 需声明 |
| `session:write` | 创建/修改/删除会话 | 中 | 提示 | 提示 | 需声明 |
| `plugin:call` | 跨插件命令调用 | 中 | 提示 | 放行 | 需声明 |
| `plugin:events` | 跨插件事件通信 | 中 | 提示 | 放行 | 需声明 |
| `workflow:trigger` | 触发工作流执行 | 中 | 提示 | 提示 | 需声明 |
| `workflow:write` | 修改工作流定义 | 中 | 提示 | 提示 | 需声明 |
| `data:invoke` | 调用 Tauri 命令 | **高** | 警告 | 警告 | 需声明 |
| `fs:read` | 读取文件系统 | **高** | 拒绝 | 放行 | 需声明 |
| `fs:write` | 写入文件系统 | **高** | 拒绝 | 放行 | 需声明 |
| `session:execute` | 执行 Agent 会话 | **高** | 警告 | 警告 | 需声明 |
| `shell:exec` | 执行 Shell 命令 | **极高** | 拒绝 | 二次确认 | 需声明 |

### 5.2 沙箱规则

1. **清单验证**：manifest.json 大小限制 64KB，字段格式严格校验
2. **路径保护**：所有文件路径禁止包含 `..`，防止目录遍历攻击
3. **权限白名单**：未知权限自动拒绝，高风险权限标记警告
4. **入口验证**：入口文件必须存在，路径必须在插件目录内
5. **沙箱禁用时**：低/中风险权限放行，高风险权限仍需声明，极高风险仍需二次确认

### 5.3 沙箱启用/禁用行为对比

| 能力 | 沙箱启用 | 沙箱禁用 |
|------|---------|---------|
| UI 面板 | 正常 | 正常 |
| 数据 invoke | 需 `data:invoke` 权限 | 需 `data:invoke` 权限 |
| 文件系统 | 拒绝（即使有权限） | 需 `fs:read`/`fs:write` 权限 |
| Shell 命令 | 拒绝 | 需 `shell:exec` + 二次确认 |
| Agent 会话 | 需 `session:*` 权限 | 需 `session:*` 权限 |
| 跨插件通信 | 需 `plugin:call`/`plugin:events` | 放行 |

---

## 6. 数据流

### 6.1 面板注册数据流（已实现）

```
manifest.json (contributes.panels 静态声明)
  → Rust PluginHost 解析
  → PluginStore.refreshRegistrations()
    → registeredPanels Map → RightPanel 下拉菜单
    → registeredCommands Map → (预留)
    → registeredHooks Map → (预留)

index.js (运行时注册)
  → PluginRegistry.loadPlugin()
    → Rust: plugin_read_entry 读取文件
    → new Function('React', source) 执行
    → 调用 onLoad(api)
      → api.ui.addPanel() 注册真实 React 组件
      → api.events.on() 监听事件
      → api.storage.set/get() 存储数据
```

### 6.2 图标渲染数据流（已实现）

```
manifest.json contributes.panels[].icon
  → pluginStore.buildRegistrations()
    → registeredPanels Map
  → PluginIcon 组件
    → parsePluginIcon(icon, pluginPath)
      → 网络地址: 直接返回 URL
      → 本地路径: 拼接插件目录 + convertFileSrc()
    → 渲染 <img> → 加载失败 → 回退默认图标
```

### 6.3 命令调度数据流（规划中）

```
工作流引擎 / 命令面板
  → executeCommand(pluginId, commandId, params)
    → 权限检查：插件是否有 data:invoke 权限
    → CommandDispatcher.execute()
      → 从 registeredCommands 查找命令 handler
      → 参数校验（根据 input schema）
      → 调用 handler(params)
      → 返回 result
    → 工作流引擎接收 result，传递给下一节点
```

### 6.4 事件分发数据流（规划中）

```
核心应用事件（消息发送、会话创建等）
  → EventDispatcher.emit(event, payload)
    → 遍历 registeredHooks.get(event)
    → 按注册顺序串行调用各插件的 handler(payload)
    → 每个 handler 可返回修改后的 payload 或中断流程
  → 核心应用根据 handler 返回结果继续执行
```

### 6.5 跨插件通信数据流（规划中）

```
插件 A → api.global.emit('news:updated', { title: '...' })
  → GlobalEventBus.emit('news:updated', payload)
    → 遍历所有订阅了 'news:updated' 的插件
    → 并行调用各插件的 handler(payload)

插件 B → api.global.on('news:updated', handler)

插件 A → api.global.call('plugin-b', 'command-id', params)
  → CommandDispatcher 路由到插件 B 的命令 handler
  → 返回结果给插件 A
```

### 6.6 工作流执行数据流（规划中）

```
用户在工作面板中创建/编辑工作流定义
  → 保存到 workflowDefinitions (SQLite)
  → 手动触发 / 定时触发 / 事件触发
    → WorkflowEngine.start(definitionId)
      → 创建工作流实例
      → 按步骤顺序执行：
        1. Trigger 节点：等待事件/定时到达
        2. Action 节点：executeCommand(pluginId, commandId, params)
        3. Condition 节点：评估条件，决定分支
        4. 数据映射：${steps.xxx.output} 模板替换
      → 更新实例状态（运行中/暂停/失败/完成）
      → 发射进度事件到前端
  → Workflow Panel 实时展示执行状态
```

### 6.7 在线商店数据流（规划中）

```
用户打开插件管理 → 切换到"在线商店"
  → OnlinePluginStore 挂载
    → invoke('plugin_store_fetch_index', { forceRefresh: false })
      → Rust: 优先 GET CDN (jsDelivr)
        → 成功 → 返回 PluginIndex
        → 失败 → 降级 GET raw (raw.githubusercontent.com)
    → 前端对比本地插件版本，标记"已安装/可更新/未安装"

用户点击"安装"
  → invoke('plugin_store_install', { pluginId })
    → Rust: 获取 index → 找到 downloadUrl
    → Rust: HTTP GET zipball → 临时文件
    → Rust: PluginHost.install_from_zip() → 清理临时文件
    → 返回 PluginInstance
  → 前端调用 discover() 刷新本地插件列表
```

---

## 7. 扩展模块设计

### 7.1 模块 A：基础能力补齐（CommandDispatcher + EventDispatcher）

#### 7.1.1 CommandDispatcher

**职责**：让 `registeredCommands` 可执行，支持参数校验和返回值处理。

```typescript
interface CommandDispatcher {
  // 注册命令 handler（由插件 onLoad 时调用）
  register(pluginId: string, commandId: string, handler: CommandHandler): void;

  // 执行命令（由工作流引擎或跨插件调用触发）
  execute<T = any>(pluginId: string, commandId: string, params?: any): Promise<T>;

  // 命令执行上下文
  executeCommand(commandId: string, params?: any): Promise<any>;
}

interface CommandHandler {
  (params: any): Promise<any>;
}
```

**实现要点**：
- 命令 handler 在插件 `onLoad` 时通过 `api.commands.register()` 注册
- 执行时先校验参数（根据 manifest.json 中的 `input` schema）
- 返回值需匹配 `output` schema 声明
- 超时控制（默认 30s）

#### 7.1.2 EventDispatcher

**职责**：核心应用事件分发到插件 hook handler。

```typescript
interface EventDispatcher {
  // 注册事件 handler（由插件 onLoad 时调用）
  register(pluginId: string, event: string, handler: EventHandler): void;

  // 分发事件（由核心应用调用）
  emit(event: string, payload?: any): Promise<void>;

  // 事件 handler 签名
  interface EventHandler {
    (payload: any): Promise<any | void>;
  }
}
```

**实现要点**：
- 串行执行（保证顺序，支持中断）
- handler 返回 `false` 或抛出特定错误可中断事件传播
- handler 可修改 payload 传递给下一个 handler

### 7.2 模块 B：跨插件通信（GlobalEventBus）

**职责**：全局事件总线，支持跨插件发布/订阅和直接命令调用。

```typescript
interface GlobalEventBus {
  // 订阅全局事件
  on(event: string, handler: (payload: any) => void): () => void;

  // 发布全局事件
  emit(event: string, payload?: any): void;

  // 直接调用其他插件的命令
  call<T>(pluginId: string, commandId: string, params?: any): Promise<T>;
}
```

**实现要点**：
- 事件命名空间约定：`plugin:{pluginId}:{eventName}`
- 跨插件命令调用通过 CommandDispatcher 路由
- 权限控制：需要 `plugin:events`（事件）或 `plugin:call`（命令）权限

### 7.3 模块 C：系统能力扩展（FS + Shell + Agent）

#### 7.3.1 文件系统 API

| 权限 | 沙箱启用 | 沙箱禁用 |
|------|---------|---------|
| `fs:read` | 拒绝 | 允许（路径白名单） |
| `fs:write` | 拒绝 | 允许（路径白名单） |

**实现路径**：通过 Tauri invoke 调用 Rust 侧 fs 命令。新增 `src-tauri/src/plugin/fs.rs`。

#### 7.3.2 Shell 命令 API

| 权限 | 沙箱启用 | 沙箱禁用 |
|------|---------|---------|
| `shell:exec` | 拒绝 | 二次确认 |

**实现路径**：通过 Tauri invoke 调用 Rust 侧 shell 命令，Rust 侧使用 `std::process::Command`。新增 `src-tauri/src/plugin/shell.rs`。

#### 7.3.3 Agent 会话 API

**实现路径**：通过 Tauri invoke 走 Rust 侧，复用已有 Agent 会话系统（AgentManager）。插件拥有独立会话上下文。

### 7.4 模块 D：工作流编排

#### 7.4.1 工作流节点模型

| 插件贡献点 | 工作流节点类型 | 说明 |
|-----------|---------------|------|
| `commands`（带 input/output schema） | Action 节点 | 可执行的操作单元 |
| `hooks`（event 声明） | Trigger 节点 | 事件触发源 |
| `panels` | 人工审批节点 | 等待用户确认后继续 |
| `data.invoke` | 集成节点 | 调用系统 Tauri 命令 |

#### 7.4.2 工作流定义格式

```json
{
  "id": "daily-news-digest",
  "name": "每日资讯摘要",
  "version": "1.0",
  "description": "每天早上8点收集资讯并推送摘要",
  "triggers": [
    { "type": "cron", "config": "0 8 * * *" }
  ],
  "steps": [
    {
      "id": "fetch-news",
      "type": "plugin:command",
      "plugin": "news-collector",
      "command": "collect",
      "params": { "sources": ["36kr", "huxiu"] }
    },
    {
      "id": "summarize",
      "type": "plugin:command",
      "plugin": "ai-assistant",
      "command": "summarize",
      "params": { "input": "${steps.fetch-news.output}" }
    },
    {
      "id": "notify",
      "type": "plugin:command",
      "plugin": "messenger",
      "command": "send-message",
      "params": { "target": "wechat", "content": "${steps.summarize.output}" }
    }
  ]
}
```

#### 7.4.3 工作流引擎核心接口

```typescript
interface WorkflowEngine {
  // 定义管理
  createDefinition(def: WorkflowDefinition): Promise<string>;
  updateDefinition(id: string, def: Partial<WorkflowDefinition>): Promise<void>;
  deleteDefinition(id: string): Promise<void>;
  getDefinition(id: string): Promise<WorkflowDefinition>;
  listDefinitions(): Promise<WorkflowDefinition[]>;

  // 执行控制
  start(definitionId: string, context?: Record<string, unknown>): Promise<string>;
  pause(instanceId: string): Promise<void>;
  resume(instanceId: string): Promise<void>;
  stop(instanceId: string): Promise<void>;
  retry(instanceId: string, stepId?: string): Promise<void>;

  // 状态查询
  getInstance(instanceId: string): Promise<WorkflowInstance>;
  listInstances(filter?: { status?: string; definitionId?: string }): Promise<WorkflowInstance[]>;

  // 事件
  on(event: WorkflowEvent, handler: (data: any) => void): () => void;
}
```

#### 7.4.4 工作流实例状态机

```
待触发 → 运行中 → 成功
                → 暂停 → 运行中
                → 失败 → 重试 → 运行中
                → 失败 → 已终止
```

#### 7.4.5 工作面板布局

```
+--------------------------------------------------------------------+
|  Workflow Panel                                                     |
+--------------------------------------------------------------------+
| 左侧：节点面板         | 右侧：编排画布                             |
|                        |                                            |
| 触发器                 |  [定时触发] → [收集资讯] → [AI摘要]       |
|  +-- 定时触发 (cron)   |                |            |              |
|  +-- 事件触发 (hook)   |                +→ [存档]    +→ [重试]     |
|  +-- 手动触发          |                                            |
|                        |  底部：节点配置面板（选中节点时显示）       |
| 插件命令               |  +---------------------------------------+ |
|  +-- news-collector    |  | 节点: 收集资讯                        | |
|  +-- ai-assistant      |  | 插件: news-collector v1.2            | |
|  +-- messenger         |  | 命令: collect                        | |
|                        |  | 参数: sources = ["36kr", "huxiu"]    | |
| 流程控制               |  | 超时: 30s   重试: 3次                | |
|  +-- 条件判断          |  +---------------------------------------+ |
|  +-- 并行执行          |                                            |
|  +-- 延迟等待          |                                            |
+--------------------------------------------------------------------+
```

### 7.5 模块 E：在线插件商店

#### 7.5.1 仓库结构

```
JorrynRen/PilotDesk/
├── server/market/plugins/       # 插件目录（含索引）
│   ├── index.json               # CI 自动生成，勿手动编辑
│   ├── hello-world/             # 插件目录名 = 插件 ID
│   │   ├── manifest.json
│   │   ├── index.js
│   │   └── icon.png
│   └── news-collector/
│       ├── manifest.json
│       └── index.js
├── .github/workflows/
│   └── generate-index.yml       # CI: 自动扫描生成 index.json
```

**核心原则**：`server/market/plugins/` 下每个子目录即一个插件，目录名 = 插件 ID。`index.json` 放在 `plugins/` 目录内，与插件目录平级。

#### 7.5.2 CI 自动索引

```yaml
name: Generate Plugin Index
on:
  push:
    branches: [main]
    paths:
      - 'server/market/plugins/**'
  workflow_dispatch:

permissions:
  contents: write

jobs:
  generate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Generate index.json
        run: |
          echo '{"schemaVersion":"1.0","updatedAt":"'$(date -u +"%Y-%m-%dT%H:%M:%SZ")'","plugins":[' > server/market/plugins/index.json
          first=true
          for dir in server/market/plugins/*/; do
            [ ! -f "${dir}manifest.json" ] && continue
            [ "$first" = true ] && first=false || echo ',' >> server/market/plugins/index.json
            plugin_id=$(basename "$dir")
            size=$(du -sh "$dir" | cut -f1)
            cat "${dir}manifest.json" | jq --arg id "$plugin_id" \
              --arg path "plugins/$plugin_id" \
              --arg downloadUrl "https://api.github.com/repos/JorrynRen/PilotDesk/zipball/main?path=server/market/plugins/$plugin_id" \
              --arg icon "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/$plugin_id/icon.png" \
              --arg size "$size" \
              '. + {path: $path, downloadUrl: $downloadUrl, icon: $icon, size: $size}' >> server/market/plugins/index.json
          done
          echo ']}' >> server/market/plugins/index.json
      - name: Commit index.json
        run: |
          git config user.name "pilotdesk-bot"
          git config user.email "bot@pilotdesk.app"
          git add server/market/plugins/index.json
          git diff --quiet && git diff --staged --quiet || \
            git commit -m "chore: auto-generate plugin index [skip ci]"
          git push
```

#### 7.5.3 index.json 格式

```json
{
  "schemaVersion": "1.0",
  "updatedAt": "2026-06-21T00:00:00Z",
  "plugins": [
    {
      "id": "hello-world",
      "name": "Hello World",
      "version": "1.0.0",
      "description": "示例插件",
      "author": "PilotDesk Team",
      "minAppVersion": "0.1.0",
      "permissions": ["ui:panel", "ui:toast"],
      "entry": { "main": "index.js" },
      "contributes": {
        "panels": [{ "id": "hello-panel", "title": "Hello World", "icon": "icon.png" }]
      },
      "path": "plugins/hello-world",
      "downloadUrl": "https://api.github.com/repos/JorrynRen/PilotDesk/zipball/main?path=server/market/plugins/hello-world",
      "icon": "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/hello-world/icon.png",
      "size": "12KB",
      "tags": ["demo", "ui"],
      "createdAt": "2026-06-01T00:00:00Z",
      "updatedAt": "2026-06-21T00:00:00Z"
    }
  ]
}
```

**字段来源**：`id`~`contributes` 从 `manifest.json` 读取；`path`/`downloadUrl`/`icon`/`size`/`tags`/`createdAt`/`updatedAt` 由 CI 注入。

#### 7.5.4 索引读取策略

| 模式 | URL | 特点 | 使用场景 |
|------|-----|------|---------|
| **raw** | `https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/index.json` | 实时，无缓存 | 强制刷新、CDN 降级 |
| **CDN** | `https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main/server/market/plugins/index.json` | CDN 加速 | 默认读取 |

**读取策略**：
```
fetchIndex(forceRefresh):
  if forceRefresh → 直接走 raw
  优先请求 CDN → 成功则返回
  CDN 失败 → 降级 raw → raw 也失败 → 提示"商店暂不可用"
```

#### 7.5.5 下载机制

```
GET https://api.github.com/repos/JorrynRen/PilotDesk/zipball/main?path=server/market/plugins/hello-world
→ Rust: HTTP GET → 临时文件 → PluginHost.install_from_zip() → 清理 → discover()
```

**API 速率限制**：未认证 60 次/h，已认证 5,000 次/h。index.json 走 CDN 不消耗配额，仅安装操作消耗。

---

## 8. 实现路线图

### 8.1 阶段总览

```
Phase 1: 基础能力补齐（P0，预估 9-14h）
  ├── CommandDispatcher（4-6h）
  ├── EventDispatcher（3-5h）
  └── 权限模型扩展 + 类型更新（2-3h）

Phase 2: 跨插件通信（P1，预估 6-9h）
  ├── GlobalEventBus（3-4h）
  └── 跨插件命令调用（3-5h）

Phase 3: 系统能力扩展（P1，预估 13-20h）
  ├── FS API（Rust + TS，5-8h）
  ├── Shell API（Rust + TS，4-6h）
  └── Agent API（复用已有系统，4-6h）

Phase 4: 工作流编排（P2，预估 28-43h）
  ├── WorkflowEngine（10-15h）
  ├── WorkflowPanel + Editor（12-18h）
  └── Rust 侧后台触发器（6-10h）

Phase 5: 在线插件商店（P3，预估 8-12h）
  ├── CI 工作流 + 示例插件（1-2h）
  ├── Rust: fetch_index + install（5-7h）
  └── 前端: OnlinePluginStore 组件（2-3h）
```

### 8.2 依赖关系

```
Phase 1 (CommandDispatcher + EventDispatcher)
  ├── 被 Phase 2 (GlobalEventBus) 依赖
  │     └── 被 Phase 4 (WorkflowEngine) 依赖
  │           └── 工作流 Action 节点 = CommandDispatcher.execute()
  │           └── 工作流 Trigger 节点 = EventDispatcher 事件源
  └── 被 Phase 3 (FS/Shell/Agent) 依赖（仅权限模型扩展部分）

Phase 5 (在线商店) 独立于其他 Phase
  └── 仅依赖现有 PluginHost.install_from_zip()
```

### 8.3 工时汇总

| Phase | 方向 | 预估工时 | 优先级 |
|-------|------|---------|--------|
| P1 | 基础能力补齐 | 9-14h | P0 |
| P2 | 跨插件通信 | 6-9h | P1 |
| P3 | 系统能力扩展 | 13-20h | P1 |
| P4 | 工作流编排 | 28-43h | P2 |
| P5 | 在线插件商店 | 8-12h | P3 |
| **合计** | | **64-98h** | |

---

## 9. 文件清单

### 9.1 已有文件（v1.0）

| 文件 | 说明 |
|------|------|
| `src-tauri/src/plugin/mod.rs` | Rust PluginHost（扫描、验证、沙箱、Tauri 命令） |
| `src/plugin/PluginAPI.ts` | 插件运行时 API 实现 |
| `src/plugin/PluginRegistry.ts` | 面板组件注册表 + JS 执行 + 生命周期 |
| `src/stores/pluginStore.ts` | Zustand store（插件列表 + 注册数据） |
| `src/components/plugin/PluginManager.tsx` | 插件管理 UI |
| `src/components/plugin/PluginPanelRenderer.tsx` | 插件面板渲染器 |
| `src/components/plugin/PluginIcon.tsx` | 插件图标渲染组件 |
| `src/components/plugin/DefaultPluginPanel.tsx` | 默认面板组件 |
| `src/utils/pluginIcon.ts` | 插件图标解析工具 |
| `src/types/plugin.ts` | 类型定义 |
| `examples/plugins/hello-world/` | 示例插件 |
| `examples/plugins/malicious-sample/` | 恶意插件示例（沙箱测试） |

### 9.2 新增文件（v2.0）

| 文件 | 模块 | 说明 | 优先级 |
|------|------|------|--------|
| `src/plugin/CommandDispatcher.ts` | A | 命令调度器 | P0 |
| `src/plugin/EventDispatcher.ts` | A | 事件分发器 | P0 |
| `src/plugin/GlobalEventBus.ts` | B | 全局事件总线 | P1 |
| `src-tauri/src/plugin/fs.rs` | C | Rust 侧文件系统命令 | P1 |
| `src-tauri/src/plugin/shell.rs` | C | Rust 侧 Shell 命令 | P1 |
| `src/plugin/PluginAPI.fs.ts` | C | FS API 实现 | P1 |
| `src/plugin/PluginAPI.shell.ts` | C | Shell API 实现 | P1 |
| `src/plugin/PluginAPI.agent.ts` | C | Agent API 实现 | P1 |
| `src/workflow/WorkflowEngine.ts` | D | 工作流引擎 | P2 |
| `src/workflow/WorkflowDefinition.ts` | D | 工作流定义类型和验证 | P2 |
| `src/workflow/WorkflowInstance.ts` | D | 工作流实例状态管理 | P2 |
| `src/stores/workflowStore.ts` | D | Zustand store | P2 |
| `src/components/workflow/WorkflowPanel.tsx` | D | 工作面板入口 | P2 |
| `src/components/workflow/WorkflowEditor.tsx` | D | 可视化编辑器 | P2 |
| `src/components/workflow/WorkflowNodeConfig.tsx` | D | 节点配置面板 | P2 |
| `src/components/workflow/WorkflowMonitor.tsx` | D | 执行监控面板 | P2 |
| `src-tauri/src/workflow/mod.rs` | D | Rust 侧持久化 + 触发器 | P2 |
| `src/types/workflow.ts` | D | 工作流类型定义 | P2 |
| `src-tauri/src/plugin/store.rs` | E | Rust 侧商店命令 | P3 |
| `src/components/plugin/OnlinePluginStore.tsx` | E | 在线商店面板 | P3 |
| `.github/workflows/generate-index.yml` | E | CI 工作流 | P3 |

### 9.3 修改文件（v2.0）

| 文件 | 变更内容 | 优先级 |
|------|---------|--------|
| `src/plugin/PluginAPI.ts` | 新增 `global`/`fs`/`shell`/`agent` API | P1 |
| `src/components/plugin/PluginManager.tsx` | 新增"在线商店"标签切换 | P3 |
| `src/types/plugin.ts` | 新增 `OnlinePluginInfo`/`PluginIndex`/`PluginVersionCompare` | P3 |

---

## 10. 风险与约束

### 10.1 安全风险

| 风险 | 等级 | 说明 | 缓解措施 |
|------|------|------|---------|
| 恶意插件通过 Shell API 执行破坏命令 | **极高** | `rm -rf /` 等 | 沙箱默认启用 + 二次确认 |
| 恶意插件通过 FS API 读取敏感文件 | **高** | 读取密码文件 | 沙箱默认启用 + 路径遍历防护 |
| 恶意插件通过 Agent API 消耗 Token | **中** | 大量调用 Agent | 速率限制 + 配额控制 |
| 跨插件调用导致级联故障 | **中** | 插件 A 调用 B，B 崩溃 | 调用超时 + 错误隔离 |
| 工作流定义无限循环 | **中** | 步骤 A → B → A | 循环检测 + 最大执行深度 |
| 恶意插件提交到商店 | **高** | 恶意代码分发 | PR 审核制 + 分支保护 |

### 10.2 架构风险

| 风险 | 等级 | 说明 |
|------|------|------|
| Agent 会话系统集成复杂度 | **中** | 需理解现有会话系统完整实现 |
| 工作流持久化与 Tauri 状态管理 | **中** | 应用重启后需恢复未完成的工作流实例 |
| 插件升级导致工作流定义不兼容 | **低** | 命令签名变化导致执行失败 |
| 性能影响 | **低** | 大量插件注册事件监听器影响分发性能 |
| GitHub API 速率限制 | **中** | 未认证 60 次/h，index.json 走 CDN 缓解 |

### 10.3 约束条件

| 约束 | 说明 |
|------|------|
| 插件纯前端运行 | 所有系统操作必须通过 Tauri invoke 委托 Rust 侧 |
| 沙箱默认启用 | 所有改造必须考虑沙箱启用时的安全限制 |
| 向后兼容 | 现有插件必须在改造后继续正常工作 |
| 无 JSX 转译 | 插件入口使用纯 JS，通过 `React.createElement` 构建 UI |

---

## 11. 关键决策清单

| # | 决策点 | 推荐方案 | 理由 |
|---|--------|---------|------|
| 1 | 命令 handler 注册时机 | 运行时注册（`api.commands.register()`） | 更灵活，支持动态命令 |
| 2 | 事件分发顺序 | 串行 await | 保证顺序，支持中断 |
| 3 | 跨插件通信方式 | GlobalEventBus + 直接命令调用 | 兼顾广播和点对点 |
| 4 | FS/Shell 实现路径 | Rust 侧 Tauri command | 权限检查统一在 Rust 侧 |
| 5 | Agent 会话集成 | 通过 Tauri invoke 走 Rust 侧 | 统一权限检查 |
| 6 | 工作流持久化 | SQLite（复用现有数据库） | 无需新增存储引擎 |
| 7 | 沙箱默认策略 | 启用，FS/Shell 在沙箱启用时拒绝 | 安全优先 |
| 8 | 商店分发方式 | GitHub 仓库目录索引 | 代码可见，CI 自动索引 |
| 9 | 商店索引生成 | GitHub Actions 自动扫描 | 开发者只需提交插件目录 |
| 10 | 商店索引读取 | CDN 优先 + raw 降级双模式 | 兼顾速度和实时性 |
| 11 | 商店下载方式 | GitHub API zipball | 支持子目录下载 |
| 12 | 版本比较策略 | 精确字符串匹配 | 简单可靠 |
| 13 | 商店入口 | PluginManager 标签切换 | 与现有 UI 无缝集成 |
