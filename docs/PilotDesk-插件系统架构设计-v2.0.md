# PilotDesk 插件系统架构设计 v2.0

> 更新时间：2026-06-21
> 状态：已实现（v1.0）+ 规划中（工作流编排集成）

---

## 1. 设计目标

- **可扩展**：第三方开发者可以独立开发插件，无需修改核心代码
- **安全**：插件运行在沙箱环境中，不能直接访问系统资源
- **轻量**：插件加载和卸载对主应用性能影响最小化
- **统一**：所有插件遵循相同的 API 规范
- **可编排**：插件贡献点可作为工作流节点被编排调度（v2.0 新增）

## 2. 架构总览

### 2.1 v1.0 架构（已实现）

```
+------------------------------------------------------------------+
|                       PilotDesk Core                              |
|                                                                   |
|   +--------------+  +--------------+  +--------------+            |
|   |  PluginHost   |  |  PluginAPI   |  |  PluginAPI   |            |
|   |  (Rust)       |  |  (运行时)    |  |  (事件总线)  |            |
|   +------+-------+  +------+-------+  +------+-------+            |
|          |                  |                  |                    |
|   +------+------------------+------------------+-------+            |
|   |                 Zustand PluginStore                    |        |
|   |   plugins[] + registeredPanels/Commands/Hooks         |        |
|   +---------------------------+---------------------------+        |
|                               |                                    |
|   +---------------------------+---------------------------+        |
|   |              PluginRegistry (组件注册表)                |        |
|   |   面板组件注册 + 加载状态 + JS 执行 + 生命周期          |        |
|   +-------------------------------------------------------+        |
+------------------------------------------------------------------+
```

### 2.2 v2.0 目标架构（含工作流编排）

```
+----------------------------------------------------------------------+
|                         PilotDesk Core                                |
+----------------------------------------------------------------------+
|                                                                      |
|  +--------------+  +--------------+  +--------------------------+    |
|  |  PluginHost   |  |  PluginAPI   |  |  WorkflowEngine          |    |
|  |  (Rust)       |  |  (运行时)    |  |  (工作流引擎)           |    |
|  +------+-------+  +------+-------+  +------------+-------------+    |
|         |                  |                       |                  |
|  +------+------------------+-----------------------+-------------+    |
|  |                    Zustand PluginStore                          |  |
|  |  plugins[] + registeredPanels/Commands/Hooks                   |  |
|  |  + workflowDefinitions[] + workflowInstances[]                 |  |
|  +---------------------------+-----------------------------------+  |
|                              |                                      |
|  +---------------------------+-----------------------------------+  |
|  |              PluginRegistry (组件注册表)                       |  |
|  |   面板组件注册 + 加载状态 + JS 执行 + 生命周期                |  |
|  +---------------------------------------------------------------+  |
|                                                                      |
|  +---------------------------------------------------------------+  |
|  |              Workflow Panel (独立工作面板)                     |  |
|  |   工作流定义编辑器 + 执行监控 + 节点配置 + 运行日志            |  |
|  +---------------------------------------------------------------+  |
+----------------------------------------------------------------------+
```

### 2.3 核心变化

| 维度 | v1.0 | v2.0 |
|------|------|------|
| 贡献点用途 | UI 扩展（面板） | UI 扩展 + 工作流节点 |
| 命令状态 | 注册但不可执行 | 可调度执行，支持参数传递和返回值 |
| 事件钩子 | 声明但未分发 | 核心事件分发到插件 handler |
| 插件通信 | 无（插件内隔离） | 全局事件总线，支持发布/订阅 |
| 工作流 | 不支持 | 独立工作面板 + 工作流引擎 |

---

## 3. 插件生命周期

```
发现 (Discovery) -> 加载 (Load) -> 执行 JS -> 运行 (Run) -> 卸载 (Unload)
```

### 3.1 发现
- 扫描 `~/.pilotdesk/plugins/` 目录
- 每个插件是一个独立目录，包含 `manifest.json` + 代码文件
- 支持通过管理面板的「+ 安装」按钮上传 .zip 压缩包安装

### 3.2 manifest.json 规范（v2.0 扩展）

```json
{
  "id": "hello-world",
  "name": "Hello World",
  "version": "1.0.0",
  "description": "示例插件",
  "author": "PilotDesk Team",
  "minAppVersion": "0.1.0",
  "permissions": ["ui:panel", "ui:toast", "session:read"],
  "entry": {
    "main": "index.js"
  },
  "contributes": {
    "panels": [
      { "id": "hello-panel", "title": "Hello World", "icon": "https://example.com/icon.png" }
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

### 3.3 命令贡献点扩展（v2.0）

命令贡献点新增 `input` 和 `output` schema 声明，使命令可作为工作流 Action 节点被编排：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `id` | string | 是 | 命令唯一标识，如 `hello.say` |
| `title` | string | 是 | 命令显示名称 |
| `input` | JSON Schema | 否 | 命令输入参数 schema，工作流引擎据此生成配置表单 |
| `output` | JSON Schema | 否 | 命令输出 schema，工作流引擎据此做数据映射 |

### 3.4 icon 字段说明

`contributes.panels[].icon` 字段支持三种格式：

| 格式 | 示例 | 说明 |
|------|------|------|
| 网络图片 | `"https://example.com/icon.png"` | 直接渲染为 `<img>` 标签 |
| 插件本地路径 | `"image/favicon.png"` | 相对于插件目录，通过 Tauri `convertFileSrc()` 转换 |
| 空/未定义 | 省略或 `""` | 显示默认图标 |

---

## 4. Plugin API

### 4.1 前端 API (TypeScript)

```typescript
interface PluginAPI {
  // UI 能力
  ui: {
    addPanel(config: PanelContribution & { component: React.ComponentType }): void;
    removePanel(id: string): void;
    showToast(message: string, type: 'info' | 'success' | 'error'): void;
  };

  // 数据访问
  data: {
    invoke<T>(cmd: string, params?: Record<string, unknown>): Promise<T>;
  };

  // 事件
  events: {
    on(event: string, handler: (...args: any[]) => void): () => void;
    emit(event: string, ...args: any[]): void;
  };

  // 存储
  storage: {
    get(key: string): Promise<string | null>;
    set(key: string, value: string): Promise<void>;
    delete(key: string): Promise<void>;
  };
}
```

### 4.2 插件入口（纯 JS 格式）

插件入口文件使用纯 JavaScript 编写，通过 `React.createElement` 构建 UI，无需 JSX 转译。

```javascript
// index.js
function MyPanel() {
  return React.createElement('div', null,
    React.createElement('h3', null, 'Hello from Plugin!')
  );
}

export default {
  onLoad: function(api) {
    api.ui.addPanel({
      id: 'my-panel',
      title: 'My Plugin',
      component: MyPanel,
    });
  },
  onUnload: function() {
    // 清理逻辑
  },
};
```

---

## 5. 安全模型

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
  | 'shell:exec'    // 极高（新增）
  | 'plugin:call'   // 中（新增）
  | 'plugin:events' // 中（新增）
  | 'session:execute' // 高（新增）
  | 'workflow:trigger'  // 中（新增）
  | 'workflow:read'     // 低（新增）
  | 'workflow:write'    // 中（新增）
;
```

### 5.2 风险等级划分

| 风险等级 | 权限 | 沙箱行为 |
|---------|------|---------|
| **默认** | `ui:toast`, `storage:*` | 自动授权，无需声明 |
| **低** | `ui:panel`, `ui:modal`, `workflow:read` | 需声明，沙箱放行 |
| **中** | `session:read`, `session:write`, `plugin:call`, `plugin:events`, `workflow:trigger`, `workflow:write` | 需声明，沙箱提示 |
| **高** | `data:invoke`, `fs:read`, `fs:write`, `session:execute` | 需声明，沙箱警告 |
| **极高** | `shell:exec` | 需声明，沙箱禁用+用户二次确认 |

### 5.3 沙箱规则

1. **清单验证**：manifest.json 大小限制 64KB，字段格式严格校验
2. **路径保护**：所有文件路径禁止包含 `..`，防止目录遍历攻击
3. **权限白名单**：未知权限自动拒绝，高风险权限标记警告
4. **入口验证**：入口文件必须存在，路径必须在插件目录内
5. **沙箱禁用时**：所有权限检查跳过，插件可正常加载

### 5.4 沙箱禁用时的行为变化

| 能力 | 沙箱启用 | 沙箱禁用 |
|------|---------|---------|
| UI 面板 | 正常 | 正常 |
| 数据 invoke | 需 `data:invoke` 权限 | 放行 |
| 文件系统 | 拒绝（即使有权限） | 需 `fs:read`/`fs:write` 权限 |
| Shell 命令 | 拒绝 | 需 `shell:exec` 权限 + 二次确认 |
| Agent 会话 | 需 `session:*` 权限 | 需 `session:*` 权限 |
| 跨插件通信 | 需 `plugin:call`/`plugin:events` 权限 | 放行 |


| 权限 | 说明 | 风险等级 | 默认 |
|------|------|---------|------|
| `ui:panel` | 添加/移除面板 | 低 | 需声明 |
| `ui:toast` | 显示通知 | 低 | 默认授权 |
| `ui:modal` | 打开模态框 | 低 | 需声明 |
| `session:read` | 读取会话和消息 | 中 | 需声明 |
| `session:write` | 创建/修改/删除会话 | 中 | 需声明 |
| `data:invoke` | 调用 Tauri 命令 | **高** | 需声明 |
| `storage:*` | 插件独立存储 | 低 | 默认授权 |
| `fs:read` | 读取文件系统 | **高** | 需声明 |
| `fs:write` | 写入文件系统 | **高** | 需声明 |
| `workflow:trigger` | 触发工作流执行（v2.0 新增） | 中 | 需声明 |
| `workflow:read` | 读取工作流定义和状态（v2.0 新增） | 低 | 需声明 |

## 6. 数据流

### 6.1 面板注册数据流

```
manifest.json (contributes.panels 静态声明)
  -> Rust PluginHost 解析
  -> Zustand PluginStore.refreshRegistrations()
    -> registeredPanels Map -> RightPanel 下拉菜单
    -> registeredCommands Map -> (预留)
    -> registeredHooks Map -> (预留)

index.js (运行时注册)
  -> PluginRegistry.loadPlugin()
    -> Rust: plugin_read_entry 读取文件
    -> new Function('React', source) 执行
    -> 调用 onLoad(api)
      -> api.ui.addPanel() 注册真实 React 组件
      -> api.ui.showToast() 显示通知
      -> api.events.on() 监听事件
      -> api.storage.set/get() 存储数据
```

### 6.2 图标渲染数据流

```
manifest.json contributes.panels[].icon
  -> pluginStore.buildRegistrations()
    -> registeredPanels Map (contribution 原样保留)
  -> PluginPanelRenderer / RightPanel
    -> PluginIcon 组件
      -> parsePluginIcon(icon, pluginPath)
        -> 网络地址: 直接返回 URL
        -> 本地路径: 拼接插件目录 + convertFileSrc()
      -> 渲染 <img> 标签 (14px)
      -> 加载失败 -> 回退默认图标
```

### 6.3 命令调度数据流（v2.0 新增）

```
工作流引擎 / 命令面板
  -> 调用 executeCommand(pluginId, commandId, params)
    -> 权限检查：插件是否有 data:invoke 权限
    -> PluginRegistry.executeCommand()
      -> 从 registeredCommands 查找命令 handler
      -> 调用 handler(params)
      -> 返回 result
    -> 工作流引擎接收 result，传递给下一节点
```

### 6.4 事件分发数据流（v2.0 新增）

```
核心应用事件（消息发送、会话创建等）
  -> GlobalEventBus.emit(event, payload)
    -> 遍历 registeredHooks.get(event)
    -> 按注册顺序调用各插件的 handler(payload)
    -> 每个 handler 可返回修改后的 payload 或中断流程
  -> 核心应用根据 handler 返回结果继续执行
```

### 6.5 工作流执行数据流（v2.0 新增）

```
用户在工作面板中创建/编辑工作流定义
  -> 保存到 workflowDefinitions (SQLite)
  -> 手动触发 / 定时触发 / 事件触发
    -> WorkflowEngine.start(definitionId)
      -> 创建工作流实例 (workflowInstances)
      -> 按步骤顺序执行：
        1. Trigger 节点：等待事件/定时到达
        2. Action 节点：executeCommand(pluginId, commandId, params)
        3. Condition 节点：评估条件，决定分支
        4. 数据映射：${steps.xxx.output} 模板替换
      -> 更新实例状态（运行中/暂停/失败/完成）
      -> 发射进度事件到前端
  -> Workflow Panel 实时展示执行状态
```

---

## 7. 工作流编排集成设计（v2.0 新增）

### 7.1 工作流节点模型

插件贡献点与工作流节点的映射关系：

| 插件贡献点 | 工作流节点类型 | 说明 |
|-----------|---------------|------|
| `commands` (带 input/output schema) | Action 节点 | 可执行的操作单元，如发送消息、调 API |
| `hooks` (event 声明) | Trigger 节点 | 事件触发源，如消息接收后触发 |
| `panels` | 人工审批节点 | 展示审批面板，等待用户确认后继续 |
| `data.invoke` | 集成节点 | 调用系统 Tauri 命令 |

### 7.2 工作流定义格式

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
      "params": {
        "target": "wechat",
        "content": "${steps.summarize.output}"
      }
    }
  ]
}
```

### 7.3 工作面板设计

工作流编排将使用**独立的工作面板**（Workflow Panel），与现有插件面板体系平行：

```
+--------------------------------------------------------------------+
|  Workflow Panel (独立工作面板)                                       |
+--------------------------------------------------------------------+
|                                                                     |
|  +--------------- 左侧：节点面板 ---------------------------------+ |
|  |                                                                 | |
|  |   触发器                                                        | |
|  |     +-- 定时触发 (cron)                                         | |
|  |     +-- 事件触发 (hook)                                         | |
|  |     +-- 手动触发                                                | |
|  |                                                                 | |
|  |   插件命令 (来自已安装插件的 commands)                           | |
|  |     +-- news-collector / collect                                | |
|  |     +-- ai-assistant / summarize                                | |
|  |     +-- messenger / send-message                                | |
|  |                                                                 | |
|  |   流程控制                                                      | |
|  |     +-- 条件判断 (if/else)                                      | |
|  |     +-- 并行执行                                                | |
|  |     +-- 延迟等待                                                | |
|  +-----------------------------------------------------------------+ |
|                                                                     |
|  +--------------- 右侧：编排画布 ---------------------------------+ |
|  |                                                                 | |
|  |  [定时触发] ---> [收集资讯] ---> [AI摘要] ---> [推送通知]       | |
|  |                    |                    |                        | |
|  |                    +---> [存档数据库]   +---> [失败重试]         | |
|  |                                                                 | |
|  |  底部：节点配置面板（选中节点时显示）                            | |
|  |  +-----------------------------------------------------------+  | |
|  |  |  节点: 收集资讯                                            |  | |
|  |  |  插件: news-collector v1.2                                |  | |
|  |  |  命令: collect                                            |  | |
|  |  |  参数: sources = ["36kr", "huxiu"]                        |  | |
|  |  |  超时: 30s   重试: 3次                                    |  | |
|  |  +-----------------------------------------------------------+  | |
|  +-----------------------------------------------------------------+ |
|                                                                     |
+---------------------------------------------------------------------+
```

### 7.4 工作流引擎核心接口

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
  on(event: 'step:start' | 'step:complete' | 'step:error' | 'workflow:complete' | 'workflow:error',
     handler: (data: any) => void): () => void;
}
```

### 7.5 工作流实例状态机

```
                +----------+
                |   待触发   |
                +----+-----+
                     | 触发条件满足
                +----v-----+
                |  运行中    |<------------+
                +----+-----+              |
                     |                    |
              +------+------+             |
              |      |      |             |
         +----v+ +--v--+ +-v--+          |
         | 成功 | |暂停 | |失败 |--重试---+
         +--+--+ +-----+ +--+-+
            |                |
       +----v----+     +----v-----+
       |  已完成   |     |  已终止   |
       +---------+     +----------+
```

---

## 8. 能力边界与集成方向

### 8.1 当前能力矩阵

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

### 8.2 集成方向优先级

| 优先级 | 方向 | 说明 | 前置依赖 |
|--------|------|------|---------|
| P0 | **命令调度器** | 让 `registeredCommands` 可执行，支持参数传递和返回值 | 无 |
| P0 | **事件分发器** | 核心应用向插件分发事件，触发 hook handler | 无 |
| P1 | **全局事件总线** | 跨插件发布/订阅，带 payload 类型 | P0 |
| P2 | **工作流引擎** | 定义解析、状态机、步骤执行、数据映射 | P0 + P1 |
| P3 | **工作面板** | 可视化编辑器、节点配置、执行监控 | P2 |

### 8.3 风险与约束

#### 安全风险

| 风险 | 等级 | 说明 | 缓解措施 |
|------|------|------|---------|
| 恶意插件通过 Shell API 执行破坏命令 | **极高** | `rm -rf /` 等 | 沙箱默认启用 + 极高风险权限二次确认 |
| 恶意插件通过 FS API 读取敏感文件 | **高** | 读取密码文件、配置等 | 沙箱默认启用 + 路径遍历防护 |
| 恶意插件通过 Agent API 消耗 Token | **中** | 大量调用 Agent 导致费用 | 速率限制 + 配额控制 |
| 跨插件调用导致级联故障 | **中** | 插件 A 调用 B，B 崩溃 | 调用超时 + 错误隔离 |
| 工作流定义无限循环 | **中** | 步骤 A 触发 B，B 触发 A | 循环检测 + 最大执行深度 |

#### 架构风险

| 风险 | 等级 | 说明 |
|------|------|------|
| Agent 会话系统与插件系统的集成复杂度 | **中** | 需要理解现有会话系统的完整实现，找到合适的集成点 |
| 工作流引擎的持久化与 Tauri 状态管理 | **中** | 应用重启后需恢复未完成的工作流实例 |
| 插件升级导致工作流定义不兼容 | **低** | 插件升级后命令签名变化，工作流执行失败 |
| 性能影响 | **低** | 大量插件注册事件监听器可能影响核心事件分发性能 |

#### 约束条件

| 约束 | 说明 |
|------|------|
| 插件纯前端运行 | 插件在浏览器上下文执行，无法直接访问系统资源，所有系统操作必须通过 Tauri invoke 委托 Rust 侧执行 |
| 沙箱默认启用 | 所有改造必须考虑沙箱启用时的安全限制 |
| 向后兼容 | 现有插件（如 hello-world）必须在改造后继续正常工作 |
| 无 JSX 转译 | 插件入口使用纯 JS，通过 React.createElement 构建 UI |



## 9. 文件清单

### 9.1 v1.0 已有文件

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

### 9.2 v2.0 新增/修改文件

| 文件 | 说明 | 状态 |
|------|------|------|
| `src/plugin/CommandDispatcher.ts` | 命令调度器：执行 registeredCommands，参数校验，返回值处理 | 规划中 |
| `src/plugin/EventDispatcher.ts` | 事件分发器：核心事件分发到插件 hook handler | 规划中 |
| `src/plugin/GlobalEventBus.ts` | 全局事件总线：跨插件发布/订阅 | 规划中 |
| `src/workflow/WorkflowEngine.ts` | 工作流引擎：定义解析、状态机、步骤执行 | 规划中 |
| `src/workflow/WorkflowDefinition.ts` | 工作流定义类型和验证 | 规划中 |
| `src/workflow/WorkflowInstance.ts` | 工作流实例状态管理 | 规划中 |
| `src/stores/workflowStore.ts` | Zustand store（工作流定义 + 实例） | 规划中 |
| `src/components/workflow/WorkflowPanel.tsx` | 独立工作面板入口 | 规划中 |
| `src/components/workflow/WorkflowEditor.tsx` | 工作流可视化编辑器（拖拽画布） | 规划中 |
| `src/components/workflow/WorkflowNodeConfig.tsx` | 节点配置面板 | 规划中 |
| `src/components/workflow/WorkflowMonitor.tsx` | 执行监控面板 | 规划中 |
| `src-tauri/src/workflow/mod.rs` | Rust 侧工作流持久化 + 后台触发器 | 规划中 |
| `src/types/workflow.ts` | 工作流类型定义 | 规划中 |

---


---

## 10. 系统能力扩展设计（FS、Shell、Agent 会话）

### 10.1 设计目标

扩展 PluginAPI 能力边界，使插件在沙箱禁用时可操作本地文件系统、执行 Shell 命令、调用 Agent 会话。

```
改造前：PluginAPI = { ui, data, events, storage }
改造后：PluginAPI = { ui, data, events, storage, fs, shell, agent }
```

### 10.2 文件系统 API（fs）

**权限模型**：

| 权限 | 沙箱启用 | 沙箱禁用 |
|------|---------|---------|
| `fs:read` | 拒绝（返回权限错误） | 允许（受路径白名单限制） |
| `fs:write` | 拒绝（返回权限错误） | 允许（受路径白名单限制） |

**API 设计**：

```typescript
interface PluginAPI {
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
}
```

**实现路径**：通过 Tauri invoke 调用 Rust 侧 fs 命令，Rust 侧执行实际文件操作。需新增 `src-tauri/src/plugin/fs.rs`。

### 10.3 Shell 命令执行 API（shell）

**权限模型**：

| 权限 | 说明 | 风险等级 |
|------|------|---------|
| `shell:exec` | 执行 Shell 命令 | **极高**（需用户明确确认） |

**API 设计**：

```typescript
interface PluginAPI {
  shell: {
    exec(command: string, options?: ShellExecOptions): Promise<ShellResult>;
    spawn(command: string, options?: ShellSpawnOptions): ShellProcess;
  };
}

interface ShellExecOptions {
  cwd?: string;
  timeout?: number;
  env?: Record<string, string>;
}

interface ShellResult {
  code: number;
  stdout: string;
  stderr: string;
}
```

**实现路径**：通过 Tauri invoke 调用 Rust 侧 shell 命令，Rust 侧使用 `std::process::Command` 执行。需新增 `src-tauri/src/plugin/shell.rs`。

### 10.4 Agent 会话调用 API（agent）

**背景**：Agent 会话系统已在项目其他部分实现（AgentManager、useAgentRegistry），插件系统需复用这些能力。

**API 设计**：

```typescript
interface PluginAPI {
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

**实现路径**：通过 Tauri invoke 走 Rust 侧，复用已有 Agent 会话系统的接口。插件应拥有独立会话上下文。


---

## 11. 实现路线图

### 11.1 阶段划分

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

### 11.2 依赖关系

```
Phase 1 (CommandDispatcher + EventDispatcher)
  +-- 被 Phase 2 (GlobalEventBus) 依赖
    +-- 被 Phase 4 (WorkflowEngine) 依赖
      +-- 工作流 Action 节点 = CommandDispatcher.execute()
      +-- 工作流 Trigger 节点 = EventDispatcher 事件源

Phase 3 (FS/Shell/Agent) 独立于其他 Phase
  +-- 仅依赖 Phase 1 的权限模型扩展
```

### 11.3 工时估算

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

## 12. 关键决策清单

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
| v2.0 | 2026-06-21 | 整合工作流编排集成设计 + 系统能力扩展设计 + 实现路线图，从 v1.0 升级 | `e01b8eb (e01b8eb84fa1059cbdf0324f36fa0e9fc6ecd656)` |
