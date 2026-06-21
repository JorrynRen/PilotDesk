# PilotDesk 插件系统改造分析 v1.0

> 分析日期：2026-06-21
> 状态：分析报告（不涉及代码调整）
> 改造方向：
>   1. 插件系统既可独立完成任务，也可为工作流编排执行提供支持
>   2. 插件间具备相互通信、相互调用的能力
>   3. 插件系统具备文件系统操作、Shell 命令执行、Agent 会话调用能力

---

## 1. 当前系统全景评估

### 1.1 现有架构总览

```
+------------------------------------------------------------------+
|                         PilotDesk Core                           |
|                                                                  |
|  +-----------------------+  +----------------------------------+  |
|  |  PluginHost (Rust)     |  |  PluginRegistry (TS)             |  |
|  |  - 扫描/发现            |  |  - 面板组件注册表                |  |
|  |  - 清单验证              |  |  - JS 入口执行                  |  |
|  |  - 权限/沙箱            |  |  - 生命周期管理 (load/unload)    |  |
|  |  - 安装/卸载            |  |  - 运行时实例管理                |  |
|  +-----------------------+  +----------------------------------+  |
|                                                                  |
|  +-----------------------+  +----------------------------------+  |
|  |  PluginAPI (TS)        |  |  PluginStore (Zustand)           |  |
|  |  - ui: addPanel/       |  |  - plugins[]                    |  |
|  |    removePanel/        |  |  - registeredPanels Map          |  |
|  |    showToast           |  |  - registeredCommands Map        |  |
|  |  - data: invoke        |  |  - registeredHooks Map           |  |
|  |  - events: on/emit     |  |  - discover/list/enable/disable  |  |
|  |    (插件内隔离)         |  |  - refreshRegistrations()       |  |
|  |  - storage: get/set/   |  +----------------------------------+  |
|  |    delete              |                                         |
|  +-----------------------+                                          |
|                                                                  |
|  +------------------------------------------------------------+  |
|  |  Agent 会话系统 (已有，在 AgentManager / useAgentRegistry)  |  |
|  |  - 创建会话、发送消息、接收流式响应                          |  |
|  |  - 管理 Agent 配置、会话列表                                 |  |
|  +------------------------------------------------------------+  |
+------------------------------------------------------------------+
```

### 1.2 当前能力矩阵

| 能力维度 | 状态 | 说明 |
|---------|------|------|
| **UI 面板扩展** | 完整实现 | panels 贡献点 -> 注册组件 -> RightPanel 渲染 |
| **命令注册** | 半完成 | commands 被解析到 registeredCommands Map，但**不可执行** |
| **事件钩子** | 半完成 | hooks 被解析到 registeredHooks Map，但**未分发** |
| **跨插件通信** | 不支持 | 每个插件有独立 PluginEventBus，事件隔离 |
| **文件系统操作** | 权限定义但无实现 | `fs:read`/`fs:write` 权限已定义，但 PluginAPI 未暴露任何 FS 方法 |
| **Shell 命令执行** | 不支持 | 无相关权限定义，无 API 暴露 |
| **Agent 会话调用** | 不支持 | Agent 会话系统已存在但未集成到插件 API |
| **后台常驻** | 不支持 | 插件纯前端运行，无 Rust 侧后台进程 |
| **工作流编排** | 不支持 | 无工作流引擎、无工作面板 |

### 1.3 关键代码位置

| 文件 | 行数 | 核心职责 |
|------|------|---------|
| `src-tauri/src/plugin/mod.rs` | 775 | Rust PluginHost：扫描、验证、沙箱、安装/卸载 |
| `src/plugin/PluginAPI.ts` | 111 | 插件运行时 API 实现 |
| `src/plugin/PluginRegistry.ts` | 193 | 面板组件注册表 + JS 执行 + 生命周期 |
| `src/stores/pluginStore.ts` | 187 | Zustand store（插件列表 + 注册数据） |
| `src/types/plugin.ts` | 118 | 类型定义 |

---

## 2. 方向一：独立任务 + 工作流编排支持

### 2.1 核心问题

当前插件只能做一件事：**贡献 UI 面板**。commands 和 hooks 两个贡献点虽然被解析存储，但从未被实际调用。这意味着：
- 插件无法作为"独立任务单元"运行（无后台逻辑、无命令执行）
- 插件无法作为工作流节点被编排（无命令调度、无参数传递、无返回值）

### 2.2 改造目标

```
改造前：插件 = UI 面板贡献者
改造后：插件 = 独立任务单元 + 工作流节点
         |-- 独立模式：插件自身 UI + 命令直接触发
         +-- 编排模式：插件命令作为工作流 Action 节点
```

### 2.3 需要新增/修改的组件

#### 2.3.1 CommandDispatcher（命令调度器）— 新增

**职责**：让 `registeredCommands` 真正可执行，支持参数传递和返回值。

```
PluginRegistry 持有 registeredCommands 的 handler 映射
  +-- 每个命令对应一个 (params: any) => Promise<any> 函数
  +-- 支持参数校验（基于 input schema）
  +-- 支持返回值传递
  +-- 支持超时控制
  +-- 支持错误处理（失败时返回结构化错误）
```

**关键设计决策**：

| 决策点 | 选项 A | 选项 B | 推荐 |
|--------|--------|--------|------|
| handler 注册时机 | manifest.json 静态声明 | index.js 运行时注册 | **B**（动态注册更灵活） |
| 参数校验 | 运行时 JSON Schema 校验 | TypeScript 编译时校验 | **A**（插件是纯 JS） |
| 返回值格式 | 自由格式 | 统一包装 `{ success, data, error }` | **B**（工作流需要统一格式） |

**推荐方案**：在 PluginAPI 中新增 `commands.register(id, handler)` 方法，插件在 `onLoad` 中注册命令 handler。manifest.json 中的 commands 贡献点作为**静态声明**（用于 UI 展示和权限提示），运行时通过 `api.commands.register()` 提供实际实现。

```typescript
// PluginAPI 新增
interface PluginAPI {
  commands: {
    register(id: string, handler: (params: any) => Promise<any>): void;
    unregister(id: string): void;
  };
}
```

**manifest.json 变化**：commands 贡献点扩展 `input`/`output` schema（v2.0 已设计）：

```json
{
  "contributes": {
    "commands": [
      {
        "id": "news.collect",
        "title": "收集资讯",
        "input": {
          "type": "object",
          "properties": {
            "sources": { "type": "array", "items": { "type": "string" } }
          }
        },
        "output": {
          "type": "object",
          "properties": {
            "articles": { "type": "array" }
          }
        }
      }
    ]
  }
}
```

#### 2.3.2 EventDispatcher（事件分发器）— 新增

**职责**：核心应用向插件分发事件，触发 hook handler。

```
核心应用事件（消息发送前/后、会话创建、Agent 响应等）
  +-- EventDispatcher.emit(event, payload)
    +-- 遍历 registeredHooks.get(event)
      +-- 按注册顺序调用各插件的 handler(payload)
        +-- 每个 handler 可返回修改后的 payload 或中断流程
  +-- 核心应用根据 handler 返回结果继续执行
```

**关键设计决策**：

| 决策点 | 选项 | 推荐 |
|--------|------|------|
| 事件优先级 | 按注册顺序 / 显式优先级 | **按注册顺序**（简单可靠） |
| 中断机制 | handler 返回 `{ cancel: true }` / 抛异常 | **返回 `{ cancel: true }`**（可控） |
| 异步支持 | 串行 await / 并行 Promise.all | **串行 await**（保证顺序） |

**事件列表（需定义）**：

| 事件名 | payload | 用途 |
|--------|---------|------|
| `app:startup` | `{}` | 应用启动完成 |
| `app:shutdown` | `{}` | 应用即将关闭 |
| `message:before-send` | `{ sessionId, content }` | 消息发送前拦截/修改 |
| `message:after-send` | `{ sessionId, content, response }` | 消息发送后处理 |
| `message:stream-chunk` | `{ sessionId, chunk }` | 流式响应每块到达 |
| `session:created` | `{ sessionId, agentType }` | 会话创建 |
| `session:deleted` | `{ sessionId }` | 会话删除 |
| `agent:response` | `{ sessionId, content }` | Agent 完整响应 |
| `plugin:installed` | `{ pluginId }` | 插件安装完成 |
| `plugin:uninstalled` | `{ pluginId }` | 插件卸载完成 |

#### 2.3.3 WorkflowEngine（工作流引擎）— 新增

**职责**：工作流定义解析、状态机管理、步骤执行、数据映射。

**核心接口**（v2.0 已设计，此处确认）：

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
}
```

**步骤执行流程**：

```
WorkflowEngine.start(definitionId)
  +-- 1. 解析定义，创建工作流实例
  +-- 2. 遍历 steps 数组：
    |-- Trigger 节点：等待事件/定时到达
    |-- Action 节点：
    |   +-- CommandDispatcher.execute(pluginId, commandId, params)
    |     +-- 参数模板替换: ${steps.xxx.output}
    |     +-- 执行 handler(params)
    |     +-- 返回 result
    |-- Condition 节点：评估条件表达式
    |   +-- 支持 ${steps.xxx.output.field} 引用
    |   +-- 支持 > < >= <= == != && || 等运算符
    +-- 3. 更新实例状态（运行中/暂停/失败/完成）
    +-- 4. 发射进度事件到前端
```

**持久化策略**：

| 数据 | 存储位置 | 说明 |
|------|---------|------|
| 工作流定义 | SQLite（workflow_definitions 表） | 结构化 JSON 存储 |
| 工作流实例 | SQLite（workflow_instances 表） | 含当前步骤、上下文快照 |
| 执行日志 | SQLite（workflow_logs 表） | 每个步骤的执行记录 |
| 定时触发器 | Rust 侧 tokio 定时任务 | 应用启动时加载未完成的定时工作流 |

#### 2.3.4 WorkflowPanel（独立工作面板）— 新增

**职责**：可视化工作流编辑器 + 执行监控。

**面板布局**（v2.0 已设计）：

```
+--------------------------------------------------------------------+
|  Workflow Panel                                                     |
+-------------------+-------------------------------------------------+
|  左侧：节点面板    |  右侧：编排画布                                |
|                   |                                                 |
|  触发器           |  [定时触发] -> [收集资讯] -> [AI摘要]           |
|  |-- 定时触发     |                              |                  |
|  |-- 事件触发     |                         [推送通知]              |
|  +-- 手动触发     |                                                 |
|                   |  +-- 节点配置面板（选中时显示）----------------+ |
|  插件命令         |  | 节点: 收集资讯                            | |
|  |-- news-collect |  | 插件: news-collector v1.2                | |
|  |-- ai-summarize |  | 参数: sources = [...]                    | |
|  +-- send-message |  | 超时: 30s  重试: 3次                     | |
|                   |  +------------------------------------------+ |
|  流程控制         |                                                 |
|  |-- 条件判断     |                                                 |
|  |-- 并行执行     |                                                 |
|  +-- 延迟等待     |                                                 |
+-------------------+-------------------------------------------------+
```

### 2.4 插件独立任务模式

插件除了作为工作流节点外，还应能**独立完成任务**。这需要：

1. **后台命令触发**：用户可通过命令面板（Command Palette）直接触发插件的命令
2. **独立 UI 入口**：插件面板中可添加"执行"按钮，直接运行插件逻辑
3. **定时自执行**：插件可注册定时任务（通过工作流引擎的定时触发器）

```
独立任务模式示例：
  +---------------------------------------------------+
  |  Plugin: news-collector                            |
  |                                                    |
  |  +-- 手动触发 -----------------------------------+ |
  |  |  [收集今日资讯] -> 执行命令 -> 显示结果        | |
  |  +------------------------------------------------+ |
  |                                                    |
  |  +-- 定时触发（通过工作流引擎）------------------+ |
  |  |  每天早上8点自动执行 -> 结果推送到面板         | |
  |  +------------------------------------------------+ |
  +---------------------------------------------------+
```

---

## 3. 方向二：插件间相互通信与调用

### 3.1 核心问题

当前每个插件拥有独立的 `PluginEventBus` 实例：

```typescript
// PluginAPI.ts — 当前实现
class PluginAPI {
  readonly events: PluginEventBus;  // 每个插件独立实例
  constructor(pluginPath, pluginId, pluginName) {
    this.events = new PluginEventBus();  // 隔离的
  }
}
```

Plugin A 调用 `api.events.emit('data:ready', data)` 只有 Plugin A 自己的监听器能收到。**插件间完全隔离，无法通信**。

### 3.2 改造目标

```
改造前：插件 A ---[独立 EventBus]--- 插件 A 内部
改造后：插件 A ---+
                 |-- GlobalEventBus --- 插件 B
                 |-- CrossPluginInvoke --- 插件 C
                 +-- DirectCommandCall --- 插件 D
```

### 3.3 需要新增/修改的组件

#### 3.3.1 GlobalEventBus（全局事件总线）— 新增

**职责**：跨插件发布/订阅，带 payload 类型。

**设计**：

```typescript
class GlobalEventBus {
  private listeners: Map<string, Set<{
    pluginId: string;
    handler: (payload: any) => void | Promise<void>;
  }>>;

  /** 订阅全局事件 */
  subscribe(pluginId: string, event: string, handler: (payload: any) => void): () => void;

  /** 发布全局事件 */
  publish(senderId: string, event: string, payload: any): void;

  /** 清除某插件的所有订阅 */
  clearPlugin(pluginId: string): void;

  /** 清除所有订阅 */
  clear(): void;
}
```

**与 PluginEventBus 的关系**：

| 维度 | PluginEventBus（现有） | GlobalEventBus（新增） |
|------|----------------------|----------------------|
| 作用域 | 插件内部 | 全局跨插件 |
| 用途 | 插件自身模块间通信 | 插件间消息传递 |
| 生命周期 | 随插件创建/销毁 | 随应用启动/关闭 |
| 隔离性 | 完全隔离 | 所有插件可见 |

**建议**：保留 PluginEventBus 作为插件内部通信，新增 GlobalEventBus 作为跨插件通信。PluginAPI 中新增 `api.global` 命名空间：

```typescript
interface PluginAPI {
  // 现有
  events: PluginEventBus;  // 插件内部
  // 新增
  global: {
    on(event: string, handler: (payload: any) => void): () => void;
    emit(event: string, payload: any): void;
    call(pluginId: string, commandId: string, params: any): Promise<any>;
  };
}
```

#### 3.3.2 跨插件命令调用

**场景**：Plugin A 需要调用 Plugin B 的某个命令。

**实现路径**：

```
Plugin A 调用 api.global.call('plugin-b', 'data.query', { id: 123 })
  +-- GlobalEventBus 查找 plugin-b 是否已加载
    +-- CommandDispatcher 查找 plugin-b 的 data.query handler
      +-- 执行 handler({ id: 123 })
      +-- 返回结果给 Plugin A
```

**安全约束**：

| 条件 | 行为 |
|------|------|
| 目标插件已加载且命令已注册 | 正常执行 |
| 目标插件已加载但命令未注册 | 返回 `{ error: 'command not found' }` |
| 目标插件未加载 | 返回 `{ error: 'plugin not loaded' }` |
| 目标插件禁用 | 返回 `{ error: 'plugin disabled' }` |
| 调用方无 `plugin:call` 权限 | 拒绝执行 |

#### 3.3.3 通信协议定义

插件间通信应遵循统一的协议格式：

```typescript
/** 跨插件调用请求 */
interface CrossPluginRequest {
  source: string;           // 调用方 pluginId
  target: string;           // 目标方 pluginId
  command: string;          // 目标命令 ID
  params: Record<string, unknown>;  // 参数
  requestId: string;        // 请求 ID（用于追踪）
  timestamp: number;        // 时间戳
}

/** 跨插件调用响应 */
interface CrossPluginResponse {
  requestId: string;
  success: boolean;
  data?: unknown;
  error?: {
    code: string;           // 错误码
    message: string;        // 错误描述
  };
  timestamp: number;
}
```

---

## 4. 方向三：系统能力扩展（FS、Shell、Agent 会话）

### 4.1 核心问题

当前 PluginAPI 只暴露了 UI、数据（Tauri invoke）、事件、存储四个维度的能力。**文件系统、Shell 命令、Agent 会话三个关键能力完全缺失**。

虽然权限模型中已定义了 `fs:read`/`fs:write` 权限，但：
1. PluginAPI 中没有对应的 API 方法
2. 没有 Shell 命令相关的权限定义
3. Agent 会话系统已存在但未集成到插件 API

### 4.2 改造目标

```
改造前：PluginAPI = { ui, data, events, storage }
改造后：PluginAPI = { ui, data, events, storage, fs, shell, agent }
```

### 4.3 需要新增/修改的组件

#### 4.3.1 文件系统 API（fs）— 新增

**职责**：在沙箱禁用时，允许插件操作本地文件系统。

**权限模型**：

| 权限 | 沙箱启用 | 沙箱禁用 |
|------|---------|---------|
| `fs:read` | 拒绝（返回权限错误） | 允许（受路径白名单限制） |
| `fs:write` | 拒绝（返回权限错误） | 允许（受路径白名单限制） |

**API 设计**：

```typescript
interface PluginAPI {
  fs: {
    /** 读取文件内容（文本） */
    readText(path: string): Promise<string>;
    /** 读取文件内容（二进制，返回 base64） */
    readBinary(path: string): Promise<string>;
    /** 写入文件内容（文本） */
    writeText(path: string, content: string): Promise<void>;
    /** 写入文件内容（二进制，接收 base64） */
    writeBinary(path: string, content: string): Promise<void>;
    /** 追加文件内容 */
    appendText(path: string, content: string): Promise<void>;
    /** 删除文件 */
    delete(path: string): Promise<void>;
    /** 创建目录 */
    createDir(path: string): Promise<void>;
    /** 删除目录 */
    removeDir(path: string): Promise<void>;
    /** 检查路径是否存在 */
    exists(path: string): Promise<boolean>;
    /** 读取目录列表 */
    readDir(path: string): Promise<FileEntry[]>;
    /** 复制文件/目录 */
    copy(src: string, dest: string): Promise<void>;
    /** 移动文件/目录 */
    move(src: string, dest: string): Promise<void>;
    /** 获取文件信息 */
    stat(path: string): Promise<FileStat>;
  };
}

interface FileEntry {
  name: string;
  path: string;
  isDirectory: boolean;
  size: number;
  modifiedAt: string;
}

interface FileStat {
  isDirectory: boolean;
  isFile: boolean;
  size: number;
  createdAt: string;
  modifiedAt: string;
}
```

**实现路径**：

```
PluginAPI.fs.readText(path)
  +-- 权限检查：沙箱启用？-> 拒绝
  +-- 路径安全检查：禁止 .. 路径遍历
  +-- 通过 Tauri invoke 调用 Rust 侧 fs 命令
    +-- Rust 侧执行实际文件操作
    +-- 返回结果给前端
```

**Rust 侧新增命令**：

```rust
// src-tauri/src/plugin/fs.rs（新增）
#[tauri::command]
pub fn plugin_fs_read_text(path: String) -> Result<String, String>;
#[tauri::command]
pub fn plugin_fs_write_text(path: String, content: String) -> Result<(), String>;
#[tauri::command]
pub fn plugin_fs_delete(path: String) -> Result<(), String>;
// ... 其他命令
```

**路径白名单策略**：

| 沙箱状态 | 路径限制 |
|---------|---------|
| 沙箱启用 | 所有 FS 操作拒绝 |
| 沙箱禁用 | 禁止 `..` 路径遍历，其他路径允许（用户需确认高风险权限） |

#### 4.3.2 Shell 命令执行 API（shell）— 新增

**职责**：在沙箱禁用时，允许插件执行 Shell 命令。

**权限模型**：

| 权限 | 说明 | 风险等级 |
|------|------|---------|
| `shell:exec` | 执行 Shell 命令 | **极高**（需用户明确确认） |

**API 设计**：

```typescript
interface PluginAPI {
  shell: {
    /** 执行 Shell 命令，返回输出 */
    exec(command: string, options?: ShellExecOptions): Promise<ShellResult>;
    /** 执行 Shell 命令（流式输出） */
    spawn(command: string, options?: ShellSpawnOptions): ShellProcess;
  };
}

interface ShellExecOptions {
  cwd?: string;           // 工作目录
  timeout?: number;       // 超时（毫秒）
  env?: Record<string, string>;  // 环境变量
}

interface ShellResult {
  code: number;           // 退出码
  stdout: string;         // 标准输出
  stderr: string;         // 错误输出
}

interface ShellSpawnOptions {
  cwd?: string;
  env?: Record<string, string>;
  onStdout?: (chunk: string) => void;  // 实时输出回调
  onStderr?: (chunk: string) => void;
  onExit?: (code: number) => void;
}

interface ShellProcess {
  kill(): void;
  pid: number;
}
```

**安全约束**：

| 条件 | 行为 |
|------|------|
| 沙箱启用 | 拒绝执行 |
| 沙箱禁用 + 无 `shell:exec` 权限 | 拒绝执行 |
| 沙箱禁用 + 有 `shell:exec` 权限 | 执行，记录日志 |
| 命令包含危险字符（`rm -rf /` 等） | 警告日志，仍执行（权限已确认） |

**实现路径**：

```
PluginAPI.shell.exec('ls -la', { cwd: '/tmp' })
  +-- 权限检查：沙箱启用？-> 拒绝
  +-- 权限检查：有 shell:exec 权限？-> 拒绝
  +-- 通过 Tauri invoke 调用 Rust 侧 shell 命令
    +-- Rust 侧使用 std::process::Command 执行
    +-- 设置超时，防止无限阻塞
    +-- 返回 stdout/stderr/exit_code
```

**Rust 侧新增命令**：

```rust
// src-tauri/src/plugin/shell.rs（新增）
#[tauri::command]
pub fn plugin_shell_exec(command: String, cwd: Option<String>, timeout: Option<u64>)
    -> Result<ShellResult, String>;
```

#### 4.3.3 Agent 会话调用 API（agent）— 新增

**职责**：允许插件创建 Agent 会话、发送消息、接收响应。

**背景**：Agent 会话系统已在项目其他部分实现（AgentManager、useAgentRegistry、Agent 会话管理相关代码），插件系统需要复用这些能力。

**API 设计**：

```typescript
interface PluginAPI {
  agent: {
    /** 创建新会话 */
    createSession(agentType: string, options?: SessionOptions): Promise<SessionInfo>;
    /** 发送消息并等待完整响应 */
    sendMessage(sessionId: string, content: string): Promise<AgentResponse>;
    /** 发送消息并流式接收响应 */
    sendMessageStream(sessionId: string, content: string, callbacks: StreamCallbacks): Promise<void>;
    /** 获取会话历史 */
    getHistory(sessionId: string): Promise<Message[]>;
    /** 列出所有会话 */
    listSessions(): Promise<SessionInfo[]>;
    /** 删除会话 */
    deleteSession(sessionId: string): Promise<void>;
    /** 获取可用 Agent 列表 */
    listAgents(): Promise<AgentInfo[]>;
  };
}

interface SessionOptions {
  systemPrompt?: string;
  temperature?: number;
  maxTokens?: number;
}

interface SessionInfo {
  id: string;
  agentType: string;
  createdAt: string;
  messageCount: number;
}

interface AgentResponse {
  content: string;
  sessionId: string;
  usage?: {
    inputTokens: number;
    outputTokens: number;
  };
}

interface StreamCallbacks {
  onChunk: (chunk: string) => void;
  onComplete: (fullContent: string) => void;
  onError: (error: string) => void;
}

interface Message {
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: string;
}

interface AgentInfo {
  agentType: string;
  displayName: string;
  status: 'ready' | 'not_installed' | 'error';
}
```

**实现路径**：

```
PluginAPI.agent.sendMessage(sessionId, content)
  +-- 权限检查：有 session:read/write 权限？-> 拒绝
  +-- 调用已有 Agent 会话系统的接口
    +-- 通过 Tauri invoke 调用 Rust 侧 agent 命令
    +-- 或通过前端 store/hook 直接调用
  +-- 返回 Agent 响应
```

**关键设计决策**：

| 决策点 | 选项 A | 选项 B | 推荐 |
|--------|--------|--------|------|
| 调用方式 | 通过 Tauri invoke 走 Rust 侧 | 直接调用前端 store/hook | **A**（统一走 Rust，权限检查在 Rust 侧） |
| 流式实现 | Rust 侧 Tauri Event 推送 | 前端 WebSocket | **A**（已有 Tauri Event 机制） |
| 会话隔离 | 插件共享用户会话 | 插件拥有独立会话 | **B**（插件应有独立会话上下文） |

**权限模型扩展**：

| 权限 | 说明 | 风险等级 |
|------|------|---------|
| `session:read` | 读取会话和消息 | 中（已有） |
| `session:write` | 创建/修改/删除会话 | 中（已有） |
| `session:execute` | 向 Agent 发送消息并接收响应 | **高**（新增） |

---

## 5. 权限模型全面扩展

### 5.1 当前权限列表

```typescript
type PluginPermission =
  | 'ui:panel'      // 低
  | 'ui:toast'      // 低（默认）
  | 'ui:modal'      // 低
  | 'session:read'  // 中
  | 'session:write' // 中
  | 'data:invoke'   // 高
  | 'storage:*'     // 低（默认）
  | 'fs:read'       // 高
  | 'fs:write'      // 高
```

### 5.2 扩展后权限列表

```typescript
type PluginPermission =
  // --- UI 能力（低风险）---
  | 'ui:panel'      // 添加/移除面板
  | 'ui:toast'      // 显示通知（默认）
  | 'ui:modal'      // 打开模态框

  // --- 会话能力（中风险）---
  | 'session:read'    // 读取会话和消息
  | 'session:write'   // 创建/修改/删除会话
  | 'session:execute' // 向 Agent 发送消息（高，新增）

  // --- 数据能力（高风险）---
  | 'data:invoke'   // 调用 Tauri 命令

  // --- 存储能力（低风险）---
  | 'storage:*'     // 插件独立存储（默认）

  // --- 文件系统能力（高风险）---
  | 'fs:read'       // 读取文件系统
  | 'fs:write'      // 写入文件系统

  // --- Shell 能力（极高风险，新增）---
  | 'shell:exec'    // 执行 Shell 命令

  // --- 插件通信能力（中风险，新增）---
  | 'plugin:call'   // 调用其他插件的命令
  | 'plugin:events' // 监听/发送全局事件

  // --- 工作流能力（中风险，新增）---
  | 'workflow:trigger'  // 触发工作流执行
  | 'workflow:read'     // 读取工作流定义和状态
  | 'workflow:write'    // 创建/修改工作流定义
```

### 5.3 风险等级重新划分

| 风险等级 | 权限 | 沙箱行为 |
|---------|------|---------|
| **默认** | `ui:toast`, `storage:*` | 自动授权，无需声明 |
| **低** | `ui:panel`, `ui:modal`, `workflow:read` | 需声明，沙箱放行 |
| **中** | `session:read`, `session:write`, `plugin:call`, `plugin:events`, `workflow:trigger`, `workflow:write` | 需声明，沙箱提示 |
| **高** | `data:invoke`, `fs:read`, `fs:write`, `session:execute` | 需声明，沙箱警告 |
| **极高** | `shell:exec` | 需声明，沙箱禁用+用户二次确认 |

### 5.4 沙箱禁用时的行为变化

| 能力 | 沙箱启用 | 沙箱禁用 |
|------|---------|---------|
| UI 面板 | 正常 | 正常 |
| 数据 invoke | 需 `data:invoke` 权限 | 放行 |
| 文件系统 | 拒绝（即使有权限） | 需 `fs:read`/`fs:write` 权限 |
| Shell 命令 | 拒绝 | 需 `shell:exec` 权限 + 二次确认 |
| Agent 会话 | 需 `session:*` 权限 | 需 `session:*` 权限 |
| 跨插件通信 | 需 `plugin:call`/`plugin:events` 权限 | 放行 |

---

## 6. 实现路线图

### 6.1 阶段划分

```
Phase 1: 基础能力补齐（P0）
|-- CommandDispatcher — 让 registeredCommands 可执行
|-- EventDispatcher — 核心事件分发到插件 hooks
|-- 权限模型扩展 — 新增 shell:exec / plugin:call / plugin:events / session:execute
+-- 类型定义更新 — plugin.ts 新增所有新类型

Phase 2: 跨插件通信（P1）
|-- GlobalEventBus — 全局事件总线
|-- PluginAPI.global — 对外暴露 on/emit/call
|-- 跨插件命令调用 — CommandDispatcher 支持跨插件路由
+-- 通信协议定义 — CrossPluginRequest/Response

Phase 3: 系统能力扩展（P1）
|-- FS API — Rust 侧 fs 命令 + PluginAPI.fs
|-- Shell API — Rust 侧 shell 命令 + PluginAPI.shell
|-- Agent API — 复用已有会话系统 + PluginAPI.agent
+-- 沙箱联动 — 沙箱启用时拒绝 FS/Shell

Phase 4: 工作流编排（P2）
|-- WorkflowEngine — 定义解析、状态机、步骤执行
|-- WorkflowDefinition/Instance — 类型和持久化
|-- workflowStore — Zustand store
|-- WorkflowPanel — 可视化编辑器
|-- WorkflowEditor — 拖拽画布
|-- WorkflowNodeConfig — 节点配置面板
|-- WorkflowMonitor — 执行监控
+-- Rust 侧后台触发器 — 定时任务、状态持久化
```

### 6.2 依赖关系

```
Phase 1 (CommandDispatcher + EventDispatcher)
  +-- 被 Phase 2 (GlobalEventBus) 依赖
    +-- 被 Phase 4 (WorkflowEngine) 依赖
      +-- 工作流 Action 节点 = CommandDispatcher.execute()
      +-- 工作流 Trigger 节点 = EventDispatcher 事件源

Phase 3 (FS/Shell/Agent) 独立于其他 Phase
  +-- 仅依赖 Phase 1 的权限模型扩展
```

### 6.3 工时估算

| Phase | 组件 | 预估工时 |
|-------|------|---------|
| P1 | CommandDispatcher | 4-6h |
| P1 | EventDispatcher | 3-5h |
| P1 | 权限模型扩展 + 类型更新 | 2-3h |
| P2 | GlobalEventBus | 3-4h |
| P2 | 跨插件命令调用 | 3-5h |
| P3 | FS API（Rust + TS） | 5-8h |
| P3 | Shell API（Rust + TS） | 4-6h |
| P3 | Agent API（复用已有系统） | 4-6h |
| P4 | WorkflowEngine | 10-15h |
| P4 | WorkflowPanel + Editor | 12-18h |
| P4 | Rust 侧后台触发器 | 6-10h |
| **合计** | | **56-86h** |

---

## 7. 风险与约束

### 7.1 安全风险

| 风险 | 等级 | 说明 | 缓解措施 |
|------|------|------|---------|
| 恶意插件通过 Shell API 执行破坏命令 | **极高** | `rm -rf /` 等 | 沙箱默认启用 + 极高风险权限二次确认 |
| 恶意插件通过 FS API 读取敏感文件 | **高** | 读取密码文件、配置等 | 沙箱默认启用 + 路径遍历防护 |
| 恶意插件通过 Agent API 消耗 Token | **中** | 大量调用 Agent 导致费用 | 速率限制 + 配额控制 |
| 跨插件调用导致级联故障 | **中** | 插件 A 调用 B，B 崩溃 | 调用超时 + 错误隔离 |
| 工作流定义无限循环 | **中** | 步骤 A 触发 B，B 触发 A | 循环检测 + 最大执行深度 |

### 7.2 架构风险

| 风险 | 等级 | 说明 |
|------|------|------|
| Agent 会话系统与插件系统的集成复杂度 | **中** | 需要理解现有会话系统的完整实现，找到合适的集成点 |
| 工作流引擎的持久化与 Tauri 状态管理 | **中** | 应用重启后需恢复未完成的工作流实例 |
| 插件升级导致工作流定义不兼容 | **低** | 插件升级后命令签名变化，工作流执行失败 |
| 性能影响 | **低** | 大量插件注册事件监听器可能影响核心事件分发性能 |

### 7.3 约束条件

| 约束 | 说明 |
|------|------|
| 插件纯前端运行 | 插件在浏览器上下文执行，无法直接访问系统资源，所有系统操作必须通过 Tauri invoke 委托 Rust 侧执行 |
| 沙箱默认启用 | 所有改造必须考虑沙箱启用时的安全限制 |
| 向后兼容 | 现有插件（如 hello-world）必须在改造后继续正常工作 |
| 无 JSX 转译 | 插件入口使用纯 JS，通过 React.createElement 构建 UI |

---

## 8. 总结

### 8.1 三个方向的改造范围对比

| 方向 | 核心改动 | 新增文件数 | 预估工时 | 风险 |
|------|---------|-----------|---------|------|
| **方向一**：独立任务 + 工作流编排 | CommandDispatcher + EventDispatcher + WorkflowEngine + WorkflowPanel | 10-12 个 | 38-56h | 中 |
| **方向二**：跨插件通信 | GlobalEventBus + 跨插件调用 + 通信协议 | 2-3 个 | 6-9h | 低 |
| **方向三**：系统能力扩展 | FS API + Shell API + Agent API + 权限扩展 | 4-6 个 | 13-20h | 高（安全） |

### 8.2 建议执行顺序

```
Phase 1 ----> Phase 2 ----> Phase 4
  |                           |
  +--> Phase 3 (可并行) ------+

推荐顺序：
  1. Phase 1（基础能力）：CommandDispatcher + EventDispatcher + 权限扩展
  2. Phase 2（跨插件通信）：GlobalEventBus + 跨插件调用
  3. Phase 3（系统能力扩展）：FS + Shell + Agent API（可与 Phase 2 并行）
  4. Phase 4（工作流编排）：WorkflowEngine + WorkflowPanel（依赖 Phase 1+2）
```

### 8.3 关键决策清单

| # | 决策 | 推荐方案 | 理由 |
|---|------|---------|------|
| 1 | 命令 handler 注册时机 | 运行时注册（`api.commands.register()`） | 更灵活，支持动态命令 |
| 2 | 事件分发顺序 | 串行 await | 保证顺序，支持中断 |
| 3 | 跨插件通信方式 | GlobalEventBus + 直接命令调用 | 兼顾广播和点对点 |
| 4 | FS/Shell 实现路径 | Rust 侧 Tauri command | 权限检查统一在 Rust 侧 |
| 5 | Agent 会话集成 | 通过 Tauri invoke 走 Rust 侧 | 统一权限检查 |
| 6 | 工作流持久化 | SQLite（复用现有数据库） | 无需新增存储引擎 |
| 7 | 沙箱默认策略 | 启用，FS/Shell 在沙箱启用时拒绝 | 安全优先 |

---

## 版本里程碑

| 版本 | 日期 | 变更说明 | Git Commit |
|------|------|---------|------------|
| v1.0 | 2026-06-21 | 插件系统三个方向改造分析报告 | `3297403 (32974036b1533427f24820582d3d9eed984412f0)` |
