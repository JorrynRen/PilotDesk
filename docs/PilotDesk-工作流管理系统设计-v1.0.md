# PilotDesk 工作流管理系统设计方案 v1.1

> **项目**: PilotDesk | **架构**: Tauri 2.0 + React 19 + TypeScript + Rust + SQLite
> **版本**: v1.2 | **日期**: 2026-06-21 | **状态**: 设计方案
> **关联文档**: [架构与技术实现-v4.5.md](PilotDesk-架构与技术实现-v4.5.md)、[需求分析文档-v2.0.md](PilotDesk-需求分析文档-v2.0.md)、[设计规格-v1.0.md](PilotDesk-设计规格-v1.0.md)、[插件系统架构设计-v1.0.md](PilotDesk-插件系统架构设计-v1.0.md)、[Agent会话延续能力设计-v2.0.md](PilotDesk-Agent会话延续能力设计-v2.0.md)

---

## 目录

1. [设计目标](#1-设计目标)
2. [功能需求](#2-功能需求)
3. [整体架构](#3-整体架构)
4. [数据模型](#4-数据模型)
5. [Rust 后端设计](#5-rust-后端设计)
6. [前端设计](#6-前端设计)
7. [通信协议](#7-通信协议)
8. [状态管理](#8-状态管理)
9. [与现有系统的集成](#9-与现有系统的集成)
10. [分阶段实施计划](#10-分阶段实施计划)
11. [文件清单](#11-文件清单)

---

## 1. 设计目标

### 1.1 定位

工作流管理系统是 PilotDesk 的**核心生产力扩展**，将 Agent 从"对话模式"提升为"可编排的自动化流水线"。用户通过可视化 DAG 编辑器定义任务流程，每个节点调用 Agent 完成子任务，节点间通过连接线建立依赖关系和参数传递，实现复杂任务的端到端自动化。

### 1.2 设计原则

| 原则          | 说明                                                |
| ----------- | ------------------------------------------------- |
| **可视化优先**   | 工作流编排以可视化 DAG 编辑器为核心交互方式，JSON 配置为辅助               |
| **执行可观测**   | 每个节点的执行状态、输入输出、日志均可实时查看和追溯                        |
| **容错设计**    | 节点级重试、超时控制、失败跳过，确保长工作流的可靠性                        |
| **与现有架构一致** | 复用 AgentManager、Tauri Events、Zustand、SQLite 等基础设施 |
| **渐进式复杂度**  | 简单线性流程和复杂条件分支使用同一套引擎，用户按需使用                       |

### 1.3 目标用户场景

| 场景         | 示例                                | 价值           |
| ---------- | --------------------------------- | ------------ |
| 批量内容生成     | 选题 → 调研 → 撰写 → 翻译 → 校对            | 一次编排，多次执行    |
| 代码审查流水线    | 代码扫描 → 静态分析 → 生成报告                | 标准化流程，结果可追溯  |
| 数据处理管道     | 数据提取 → 清洗 → 分析 → 可视化              | 中间产物可检查      |
| 多 Agent 协作 | Claude 写代码 → CodeX 审查 → Hermes 测试 | 发挥各 Agent 优势 |

---

## 2. 功能需求

### 2.1 核心功能（P0）

#### 2.1.1 工作流 CRUD

- 创建、编辑、删除、复制工作流
- 工作流列表展示（名称、状态、最后运行时间、节点数）
- 工作流版本管理（每次保存生成新版本，支持回滚）

#### 2.1.2 可视化 DAG 编辑器

- 拖拽创建节点（从节点类型面板拖入画布）
- 节点间连线建立依赖关系（从输出端口拖到输入端口）
- 画布缩放/平移（鼠标滚轮缩放 + 拖拽平移）
- 节点选择、移动、删除
- 自动布局（拓扑排序自动排列节点位置）
- 迷你地图（大型工作流导航）
- 撤销/重做（编辑器操作历史）

#### 2.1.3 节点类型

| 节点类型         | 说明                | 配置项                          |
| ------------ | ----------------- | ---------------------------- |
| **Agent 任务** | 调用 Agent 执行一次任务   | Agent 类型、Prompt 模板、模型参数、工作目录 |
| **API 调用**   | 调用外部 HTTP API（注：区别于 API Agent 调用——后者调用大模型 API 并传递上下文，标记为未来扩展，将复用 API 会话的上下文传递逻辑） | URL、Method、Headers、Body 模板   |
| **条件分支**     | 基于上游输出做条件判断       | 条件表达式（JavaScript 语法）、分支输出    |
| **代码转换**     | 用 JavaScript 转换数据 | 代码脚本、输入映射、输出定义               |
| **聚合**       | 合并多个上游输出          | 聚合策略（合并/选择/拼接）               |
| **人工介入**     | 工作流执行到此处暂停，等待用户输入内容或做出选择后继续 | 提示文案、输入类型（文本/选择/确认）、选项列表、默认值 |
| **触发器**      | 工作流启动入口           | 手动/定时/事件                     |

#### 2.1.4 执行引擎

- **拓扑排序调度**：按依赖关系自动确定执行顺序
- **并行执行**：无依赖关系的节点同时执行
- **串行执行**：有依赖关系的节点按序执行
- **条件分支**：根据上游结果动态选择执行路径
- **超时控制**：每个节点可配置超时时间（默认 300s）
- **重试机制**：失败节点自动重试（可配置次数和间隔）
- **Checkpoint**：每个节点执行完成后持久化状态，应用重启后可恢复
- **挂起/恢复**：人工介入节点执行时工作流暂停，等待用户响应后自动恢复；支持超时自动决策

#### 2.1.5 参数传递

- **模板变量语法**：`{{node_id.output}}`、`{{node_id.output.field}}`
- **JSONPath 提取**：从 JSON 输出中提取嵌套字段
- **类型转换**：自动类型推断 + 显式类型转换
- **默认值**：变量不存在时使用默认值

#### 2.1.6 执行监控

- **实时 DAG 状态**：节点颜色标识（待运行/运行中/成功/失败/跳过）
- **节点详情面板**：点击节点查看输入/输出/日志/耗时
- **进度指示**：已完成节点数 / 总节点数
- **执行历史**：每次运行的完整记录，可回溯查看
- **中止执行**：用户可手动中止正在运行的工作流
- **人工介入响应**：工作流执行到人工介入节点时，前端弹出输入面板（文本输入/下拉选择/确认对话框），用户提交后工作流自动恢复

### 2.2 重要功能（P1）

#### 2.2.1 工作流模板

- 内置模板库（内容生成、代码审查、数据处理等）
- 用户可将工作流另存为模板
- 从模板创建工作流

#### 2.2.2 定时触发

- 支持 cron 表达式定时执行工作流
- 与现有定时任务系统集成

#### 2.2.3 通知

- 工作流执行完成/失败时发送通知
- 支持桌面通知和状态栏提示

#### 2.2.4 导出/导入

- 工作流定义导出为 JSON 文件
- 从 JSON 文件导入工作流

### 2.3 辅助功能（P2）

- 节点注释/备注
- 工作流标签/分类
- 执行耗时统计
- 节点执行日志搜索
- 工作流运行历史图表

---

## 3. 整体架构

### 3.1 架构总览

```
+------------------------------------------------------+
|                    PilotDesk App                       |
|  +------------------------------------------------+  |
|  |              前端 (React + TypeScript)           |  |
|  |  +------------------+  +---------------------+  |  |
|  |  |  DAG 编辑器       |  |  执行监控面板        |  |  |
|  |  |  (react-flow)     |  |  (实时状态 + 日志)   |  |  |
|  |  +--------+---------+  +---------+-----------+  |  |
|  |           |                       |              |  |
|  |  +--------v-----------------------v-----------+  |  |
|  |  |         workflowStore (Zustand)            |  |  |
|  |  +-------------------+-----------------------+  |  |
|  +----------------------+--------------------------+  |
|                         |                              |
|  +----------------------+--------------------------+  |
|  |     Tauri IPC (invoke)    |  Tauri Events        |  |
|  |                          |  (执行状态推送)        |  |
|  +----------------------+--------------------------+  |
|                         |                              |
|  +----------------------+--------------------------+  |
|  |              Rust 后端                            |  |
|  |                                                   |  |
|  |  +------------------+  +----------------------+  |  |
|  |  | WorkflowEngine    |  | NodeExecutor         |  |  |
|  |  | (DAG 调度器)      |  | (节点执行器)         |  |  |
|  |  |  + 拓扑排序       |  |  + AgentExecutor     |  |  |
|  |  |  + 并行调度       |  |  + ApiExecutor       |  |  |
|  |  |  + 状态持久化     |  |  + ConditionExecutor |  |  |
|  |  |  + Checkpoint     |  |  + TransformExecutor |  |  |
|  |  +------------------+  +----------------------+  |  |
|  |                                                   |  |
|  |  +------------------+  +----------------------+  |  |
|  |  | TemplateEngine    |  | AgentManager (复用)   |  |  |
|  |  | (参数传递引擎)    |  | (进程管理)           |  |  |
|  |  +------------------+  +----------------------+  |  |
|  |                                                   |  |
|  |  +------------------+                             |  |
|  |  | SQLite (r2d2)    |                             |  |
|  |  |  + workflows     |                             |  |
|  |  |  + nodes         |                             |  |
|  |  |  + edges         |                             |  |
|  |  |  + executions    |                             |  |
|  |  |  + node_results  |                             |  |
|  |  +------------------+                             |  |
|  +----------------------+--------------------------+  |
+-------------------------+----------------------------+
                          |
                 +--------v--------+
                 |   Agent CLI     |
                 |  (本地安装)      |
                 +-----------------+
```

### 3.2 架构分层

| 层           | 组件                      | 职责                         |
| ----------- | ----------------------- | -------------------------- |
| **前端编辑器**   | DAGEditor (react-flow)  | 可视化工作流编排                   |
| **前端监控**    | ExecutionMonitor        | 实时执行状态展示                   |
| **前端状态**    | workflowStore (Zustand) | 工作流定义 + 执行状态               |
| **IPC 层**   | Tauri invoke + Events   | 命令调用 + 状态推送                |
| **Rust 调度** | WorkflowEngine          | DAG 调度 + 并行控制 + Checkpoint |
| **Rust 执行** | NodeExecutor            | 各类型节点执行                    |
| **Rust 模板** | TemplateEngine          | 参数传递 + 变量解析                |
| **Rust 进程** | AgentManager (复用)       | Agent CLI 进程管理             |
| **持久化**     | SQLite (r2d2)           | 工作流定义 + 执行记录               |

### 3.3 与现有架构的关系

```
现有组件                    新增组件
──────────────────────────────────────────────
AgentManager (agent/mod.rs) ── 复用，作为 NodeExecutor 的子执行器
Tauri Events                 ── 复用，新增 workflow:chunk/done/status 事件
Zustand Stores               ── 新增 workflowStore
SQLite (r2d2)                ── 新增 5 张表
invokeHelper                 ── 复用，新增 workflow CRUD 命令
useReducer                   ── 复用模式，用于执行状态管理
react-virtuoso               ── 复用，用于执行日志列表
MarkdownRenderer             ── 复用，用于渲染节点输出
useHumanInput                ── 新增 Hook，管理人工介入面板的显示/隐藏和响应提交
WorkflowNodeTypeRegistry     ── 新增 Registry，统一管理内置 + 插件节点类型的注册和发现
```

---

## 4. 数据模型

### 4.1 新增表结构

#### workflows（工作流定义表）

```sql
CREATE TABLE workflows (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT DEFAULT '',
    status TEXT DEFAULT 'draft' CHECK(status IN ('draft', 'active', 'archived')),
    version INTEGER DEFAULT 1,
    tags TEXT DEFAULT '[]',           -- JSON 数组
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

#### workflow_versions（工作流版本表）

```sql
CREATE TABLE workflow_versions (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    snapshot TEXT NOT NULL,           -- 完整工作流定义 JSON（含 nodes + edges）
    created_at INTEGER NOT NULL,
    UNIQUE(workflow_id, version)
);

CREATE INDEX IF NOT EXISTS idx_wf_versions_workflow ON workflow_versions(workflow_id, version DESC);
```

**snapshot JSON 结构**：

```json
{
  "nodes": [
    {
      "id": "node_001",
      "type": "agent_task",
      "label": "撰写文章大纲",
      "position": { "x": 100, "y": 200 },
      "config": {
        "agent_type": "claude",
        "prompt_template": "请为以下主题撰写文章大纲：{{trigger.output.topic}}",
        "model_params": { "mode": "think" },
        "cwd": "",
        "timeout_seconds": 300,
        "retry_count": 2,
        "retry_interval_ms": 5000
      }
    }
  ],
  "edges": [
    {
      "id": "edge_001",
      "source": "trigger",
      "target": "node_001",
      "source_handle": "output",
      "target_handle": "input",
      "param_mappings": {
        "topic": "{{trigger.output.topic}}"
      }
    }
  ],
  "trigger_config": {
    "type": "manual",
    "input_schema": {
      "type": "object",
      "properties": {
        "topic": { "type": "string", "description": "文章主题" }
      }
    }
  }
}
```

#### workflow_nodes（节点定义表）

```sql
CREATE TABLE workflow_nodes (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    type TEXT NOT NULL CHECK(type IN ('trigger', 'agent_task', 'api_call', 'condition', 'transform', 'aggregator', 'human_input')),
    label TEXT NOT NULL DEFAULT '',
    position_x REAL DEFAULT 0,
    position_y REAL DEFAULT 0,
    config TEXT NOT NULL DEFAULT '{}',  -- JSON: 节点类型特定配置
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wf_nodes_workflow ON workflow_nodes(workflow_id);
```

**config JSON 按节点类型**：

```jsonc
// agent_task
{
  "agent_type": "claude",           // claude / hermes / codex / api
  "prompt_template": "...",         // 支持 {{var}} 模板语法
  "model_params": { "mode": "think" },
  "cwd": "",
  "timeout_seconds": 300,
  "retry_count": 2,
  "retry_interval_ms": 5000,
  "output_parser": "json"           // 输出解析方式: text / json / markdown
}

// api_call
{
  "url_template": "https://api.example.com/{{trigger.output.endpoint}}",
  "method": "POST",
  "headers_template": { "Authorization": "Bearer {{env.API_KEY}}" },
  "body_template": "{\"query\": \"{{node_001.output}}\"}",
  "timeout_seconds": 60
}

// condition
{
  "expression": "node_001.output.includes('success')",
  "output_true": "条件满足时的输出文本",
  "output_false": "条件不满足时的输出文本"
}

// transform
{
  "script": "const data = JSON.parse(inputs.node_001); return data.items.map(i => i.name);",
  "input_mappings": { "node_001": "{{node_001.output}}" }
}

// aggregator
{
  "strategy": "merge",              // merge / concat / pick_first
  "input_sources": ["node_001", "node_002"]
}

// human_input
{
  "prompt": "请选择下一步操作：",      // 展示给用户的提示文案
  "input_type": "select",            // text / select / confirm / file
  "options": [                       // input_type=select 时有效
    { "label": "继续执行", "value": "continue" },
    { "label": "跳过此步", "value": "skip" },
    { "label": "终止工作流", "value": "abort" }
  ],
  "default_value": "continue",       // 超时未响应时的默认值
  "timeout_minutes": 30,             // 等待超时（分钟），超时后使用默认值
  "allow_custom": false,             // 是否允许用户输入自定义文本
  "placeholder": "输入说明..."        // input_type=text 时的占位符
}
```

#### workflow_edges（边定义表）

```sql
CREATE TABLE workflow_edges (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    source_node_id TEXT NOT NULL,
    target_node_id TEXT NOT NULL,
    source_handle TEXT DEFAULT 'output',
    target_handle TEXT DEFAULT 'input',
    label TEXT DEFAULT '',
    param_mappings TEXT DEFAULT '{}',  -- JSON: 参数映射规则
    condition_expression TEXT DEFAULT NULL,  -- 条件边：仅当表达式为 true 时执行
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wf_edges_workflow ON workflow_edges(workflow_id);
```

#### workflow_executions（工作流执行实例表）

```sql
CREATE TABLE workflow_executions (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    workflow_version INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'running', 'paused', 'completed', 'failed', 'cancelled')),
    trigger_type TEXT DEFAULT 'manual' CHECK(trigger_type IN ('manual', 'scheduled', 'event')),
    input_data TEXT DEFAULT '{}',       -- JSON: 触发器输入
    output_data TEXT DEFAULT NULL,      -- JSON: 最终输出
    started_at INTEGER,
    finished_at INTEGER,
    error_message TEXT DEFAULT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_wf_exec_workflow ON workflow_executions(workflow_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_wf_exec_status ON workflow_executions(status, created_at DESC);
```

#### node_executions（节点执行记录表）

```sql
CREATE TABLE node_executions (
    id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL REFERENCES workflow_executions(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'running', 'completed', 'failed', 'skipped', 'cancelled')),
    input_data TEXT DEFAULT NULL,       -- JSON: 节点输入
    output_data TEXT DEFAULT NULL,      -- JSON: 节点输出
    error_message TEXT DEFAULT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    retry_count INTEGER DEFAULT 0,
    duration_ms INTEGER DEFAULT 0,
    artifacts_path TEXT DEFAULT NULL,   -- 过程产物文件路径
    agent_session_id TEXT DEFAULT NULL, -- Agent 侧会话 ID（Claude UUID / Hermes 时间戳 / Codex thread_id），用于调试追溯
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_node_exec_execution ON node_executions(execution_id);
CREATE INDEX IF NOT EXISTS idx_node_exec_status ON node_executions(execution_id, status);
```

#### node_execution_logs（节点执行日志表）

```sql
CREATE TABLE node_execution_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    execution_id TEXT NOT NULL,
    node_execution_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    level TEXT NOT NULL DEFAULT 'info' CHECK(level IN ('debug', 'info', 'warn', 'error')),
    message TEXT NOT NULL,
    metadata TEXT DEFAULT NULL          -- JSON: 附加元数据
);

CREATE INDEX IF NOT EXISTS idx_node_logs_exec ON node_execution_logs(node_execution_id, timestamp);
```

### 4.2 实体关系图（ER）

```
workflows 1 ---- * workflow_versions      (workflow_id FK, ON DELETE CASCADE)
workflows 1 ---- * workflow_nodes         (workflow_id FK, ON DELETE CASCADE)
workflows 1 ---- * workflow_edges         (workflow_id FK, ON DELETE CASCADE)
workflows 1 ---- * workflow_executions    (workflow_id FK, ON DELETE CASCADE)
workflow_executions 1 ---- * node_executions  (execution_id FK, ON DELETE CASCADE)
node_executions 1 ---- * node_execution_logs  (node_execution_id FK, ON DELETE CASCADE)
```

### 4.3 Rust 数据模型

```rust
// db/models.rs 新增

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub version: i64,
    pub tags: String,         // JSON array
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowVersion {
    pub id: String,
    pub workflow_id: String,
    pub version: i64,
    pub snapshot: String,     // JSON: full workflow definition
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    pub workflow_id: String,
    pub node_type: String,
    pub label: String,
    pub position_x: f64,
    pub position_y: f64,
    pub config: String,       // JSON
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub id: String,
    pub workflow_id: String,
    pub source_node_id: String,
    pub target_node_id: String,
    pub source_handle: String,
    pub target_handle: String,
    pub label: String,
    pub param_mappings: String,    // JSON
    pub condition_expression: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowExecution {
    pub id: String,
    pub workflow_id: String,
    pub workflow_version: i64,
    pub status: String,
    pub trigger_type: String,
    pub input_data: String,       // JSON
    pub output_data: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeExecution {
    pub id: String,
    pub execution_id: String,
    pub node_id: String,
    pub status: String,
    pub input_data: Option<String>,
    pub output_data: Option<String>,
    pub error_message: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub retry_count: i64,
    pub duration_ms: i64,
    pub artifacts_path: Option<String>,
    pub agent_session_id: Option<String>,  // Agent 侧会话 ID，用于调试追溯
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeExecutionLog {
    pub id: i64,
    pub execution_id: String,
    pub node_execution_id: String,
    pub timestamp: i64,
    pub level: String,
    pub message: String,
    pub metadata: Option<String>,
}
```

---

## 5. Rust 后端设计

### 5.1 模块结构

```
src-tauri/src/
├── workflow/                    # 新增：工作流模块
│   ├── mod.rs                   # 模块声明 + WorkflowEngine
│   ├── engine.rs                # DAG 调度引擎（拓扑排序 + 并行调度 + Checkpoint）
│   ├── executor.rs              # NodeExecutor 分发器（基于注册表）
│   ├── registry.rs              # WorkflowNodeTypeRegistry 节点类型注册表
│   ├── agents/                  # 节点执行器
│   │   ├── mod.rs
│   │   ├── agent_executor.rs    # Agent 任务执行器
│   │   ├── api_executor.rs      # HTTP API 调用执行器
│   │   ├── condition_executor.rs # 条件分支执行器
│   │   ├── transform_executor.rs # 代码转换执行器
| `src-tauri/src/workflow/agents/human_input_executor.rs` | ~120 | 人工介入执行器（挂起/恢复 + 超时） |
| `src-tauri/src/workflow/agents/plugin_executor.rs` | ~100 | 插件节点执行器（通过 PluginHost 转发到 JS） |
│   │   └── human_input_executor.rs # 人工介入执行器（挂起/恢复）
│   └── template.rs              # TemplateEngine 参数传递引擎
├── commands/
│   ├── mod.rs
│   ├── workflow.rs              # 新增：工作流 CRUD + 执行控制命令
│   └── ...                      # 现有命令
└── db/
    ├── init.rs                  # 新增迁移：v8 工作流表
    ├── models.rs                # 新增数据模型
    └── ...
```

### 5.2 WorkflowEngine（DAG 调度引擎）

**文件**：`src-tauri/src/workflow/engine.rs`

#### 核心职责

1. **拓扑排序**：基于 edges 计算节点的执行顺序，检测环
2. **并行调度**：无依赖的节点同时启动 tokio task
3. **状态持久化**：每个节点完成后写入 SQLite
4. **Checkpoint 恢复**：应用重启后从上次中断处恢复
5. **中止控制**：通过 CancellationToken 实现

#### 核心结构

```rust
pub struct WorkflowEngine {
    pool: DbPool,
    active_executions: Arc<Mutex<HashMap<String, ExecutionContext>>>,
}

struct ExecutionContext {
    execution_id: String,
    workflow_snapshot: WorkflowSnapshot,  // 从 workflow_versions 加载
    cancel_token: CancellationToken,
    state: ExecutionState,
}

struct ExecutionState {
    node_states: HashMap<String, NodeRuntimeState>,
    completed_count: AtomicUsize,
    total_count: usize,
}

struct NodeRuntimeState {
    status: NodeStatus,
    input: Option<Value>,
    output: Option<Value>,
    error: Option<String>,
    retry_count: u32,
}
```

#### 调度算法

```
execute_workflow(execution_id, snapshot, input_data)
  │
  ├─ 1. 拓扑排序（Kahn 算法）
  │    → sorted_nodes: Vec<Vec<NodeRef>>
  │    → 每层一个 Vec，层内节点无依赖关系
  │
  ├─ 2. 初始化 ExecutionContext
  │    → 所有节点状态设为 pending
  │    → 写入 workflow_executions (status=running)
  │
  ├─ 3. 逐层执行
  │    for each layer in sorted_nodes:
  │      ├─ 并行启动层内所有节点
  │      │   for each node in layer:
  │      │     ├─ 解析输入（TemplateEngine）
  │      │     ├─ 更新状态为 running
  │      │     ├─ spawn tokio task:
  │      │     │   ├─ 调用 NodeExecutor.execute()
  │      │     │   ├─ 超时控制（tokio::time::timeout）
  │      │     │   ├─ 失败重试（按配置）
  │      │     │   ├─ 写入 node_executions
  │      │     │   ├─ emit Tauri Event
  │      │     │   └─ 更新 ExecutionContext
  │      │     └─ await all tasks in layer
  │      │
  │      │  [人工介入节点特殊处理]
  │      │  if node.type == "human_input":
  │      │     ├─ emit "workflow:awaiting-input" 事件（含 prompt/options/input_type）
  │      │     ├─ 状态设为 paused（挂起）
  │      │     ├─ 启动超时计时器（tokio::time::timeout）
  │      │     ├─ 等待用户响应（通过 Tauri Command: respond_human_input）
  │      │     │   ├─ 用户响应 → 恢复执行，输出 = 用户输入
  │      │     │   ├─ 超时 → 使用 default_value 继续
  │      │     │   └─ 用户取消 → 节点标记为 cancelled
  │      │     └─ 继续后续流程
  │      │
  │      └─ 检查条件分支
  │          └─ 根据 condition_expression 决定哪些边激活
  │
  ├─ 4. 完成处理
  │    ├─ 所有节点完成 → status=completed
  │    ├─ 有节点失败 → status=failed
  │    ├─ 用户取消 → status=cancelled
  │    └─ emit workflow:completed/failed/cancelled 事件
  │
  └─ 5. 清理
       └─ 从 active_executions 移除
```

#### 环检测（Kahn 算法）

```rust
fn topological_sort(nodes: &[NodeDef], edges: &[EdgeDef]) -> Result<Vec<Vec<String>>, AppError> {
    // 1. 构建邻接表和入度表
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();

    for node in nodes {
        in_degree.entry(node.id.clone()).or_insert(0);
        adjacency.entry(node.id.clone()).or_default();
    }

    for edge in edges {
        adjacency.get_mut(&edge.source_node_id).unwrap().push(edge.target_node_id.clone());
        *in_degree.get_mut(&edge.target_node_id).unwrap() += 1;
    }

    // 2. Kahn 算法：逐层提取入度为 0 的节点
    let mut layers: Vec<Vec<String>> = Vec::new();
    let mut queue: Vec<String> = in_degree.iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let mut processed = 0;

    while !queue.is_empty() {
        layers.push(queue.clone());
        let mut next_queue = Vec::new();

        for node_id in &queue {
            processed += 1;
            for neighbor in adjacency.get(node_id).unwrap_or(&vec![]) {
                let deg = in_degree.get_mut(neighbor).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    next_queue.push(neighbor.clone());
                }
            }
        }
        queue = next_queue;
    }

    if processed != nodes.len() {
        return Err(AppError::InvalidInput("工作流包含循环依赖".into()));
    }

    Ok(layers)
}
```

#### Checkpoint 恢复

```rust
fn recover_execution(execution_id: &str) -> Result<WorkflowRecovery, AppError> {
    // 1. 查询 execution 状态
    let execution = get_execution(execution_id)?;

    if execution.status != "running" {
        return Err(AppError::InvalidInput("只能恢复运行中的执行".into()));
    }

    // 2. 查询所有 node_executions
    let node_results = list_node_executions(execution_id)?;

    // 3. 确定恢复点
    let completed_nodes: HashSet<String> = node_results.iter()
        .filter(|n| n.status == "completed")
        .map(|n| n.node_id.clone())
        .collect();

    let failed_nodes: HashSet<String> = node_results.iter()
        .filter(|n| n.status == "failed")
        .map(|n| n.node_id.clone())
        .collect();

    // 4. 从 snapshot 重建 DAG，跳过已完成节点
    let snapshot: WorkflowSnapshot = serde_json::from_str(&execution.snapshot)?;

    Ok(WorkflowRecovery {
        snapshot,
        completed_nodes,
        failed_nodes,  // 这些节点需要重试
        last_checkpoint: node_results.iter()
            .filter(|n| n.status == "completed")
            .max_by_key(|n| n.finished_at)
            .and_then(|n| n.finished_at),
    })
}
```

### 5.3 NodeExecutor（节点执行器分发）

**文件**：`src-tauri/src/workflow/executor.rs`

```rust
pub struct NodeExecutor {
    registry: Arc<Mutex<WorkflowNodeTypeRegistry>>,
    agent_manager: Arc<AgentManager>,
    plugin_host: Arc<PluginHost>,
    http_client: reqwest::Client,
    pool: DbPool,
}

impl NodeExecutor {
    /// 从注册表查找执行器并执行
    /// 内置节点和插件节点走同一套分发逻辑
    pub async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::Emitter,
    ) -> Result<NodeOutput, AppError> {
        let registry = self.registry.lock().unwrap();
        let executor = registry.get_executor(&node.node_type)
            .ok_or_else(|| AppError::InvalidInput(format!("未知节点类型: {}", node.node_type)))?;
        executor.execute(node, resolved_input, execution_id, emitter).await
    }

    /// 插件动态注册节点类型
    pub fn register_plugin_node_type(&self, registration: NodeTypeRegistration) {
        let mut registry = self.registry.lock().unwrap();
        registry.register(registration);
    }

    /// 插件卸载时注销节点类型
    pub fn unregister_plugin_node_type(&self, type_id: &str) {
        let mut registry = self.registry.lock().unwrap();
        registry.unregister(type_id);
    }

    /// 人工介入节点：挂起工作流，等待用户输入后恢复
    /// 通过 workflow:awaiting-input 事件通知前端展示输入面板
    /// 通过 respond_human_input 命令接收用户响应
    async fn execute_human_input(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::Emitter,
    ) -> Result<NodeOutput, AppError> {
        let config: HumanInputConfig = serde_json::from_str(&node.config)
            .map_err(|e| AppError::Config(format!("human_input 配置解析失败: {}", e)))?;

        // 1. 解析提示文案中的模板变量
        let prompt = TemplateEngine::resolve(&config.prompt, &resolved_input)?;

        // 2. 发射 awaiting-input 事件到前端
        emitter.emit("workflow:awaiting-input", serde_json::json!({
            "execution_id": execution_id,
            "node_id": node.id,
            "prompt": prompt,
            "input_type": config.input_type,
            "options": config.options,
            "allow_custom": config.allow_custom,
            "placeholder": config.placeholder,
            "timeout_minutes": config.timeout_minutes,
        })).ok();

        // 3. 挂起当前任务，等待用户响应
        //    通过 oneshot channel 实现：Rust 等待 → 前端通过 respond_human_input 命令发送响应
        let rx = self.register_human_input_wait(execution_id, &node.id)?;

        let result = tokio::time::timeout(
            Duration::from_secs(config.timeout_minutes as u64 * 60),
            rx,
        ).await;

        let user_input = match result {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return Err(AppError::Cancelled("用户取消了人工介入".into())),
            Err(_) => {
                emitter.emit("workflow:log", serde_json::json!({
                    "execution_id": execution_id,
                    "node_execution_id": node.id,
                    "level": "warn",
                    "message": format!("人工介入超时（{}分钟），使用默认值: {}", config.timeout_minutes, config.default_value),
                })).ok();
                config.default_value.unwrap_or_default()
            }
        };

        Ok(NodeOutput {
            output: serde_json::json!({ "user_input": user_input }),
        })
    }
}
```

### 5.3.1 WorkflowNodeTypeRegistry（节点类型注册表）

**文件**：`src-tauri/src/workflow/registry.rs`

所有节点类型（内置 + 插件）通过统一的注册表管理，消除硬编码的 `match` 分发，实现插件动态注册节点类型。

```rust
use async_trait::async_trait;

/// 节点执行器 trait — 所有节点类型实现此接口
#[async_trait]
pub trait NodeExecutorTrait: Send + Sync {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::Emitter,
    ) -> Result<NodeOutput, AppError>;
}

/// 节点类型注册信息
pub struct NodeTypeRegistration {
    pub type_id: String,
    pub name: String,
    pub category: NodeCategory,       // Builtin | Plugin(String)
    pub executor: Box<dyn NodeExecutorTrait>,
    pub config_schema: Option<Value>, // JSON Schema，用于前端动态生成配置表单
    pub permissions: Vec<String>,     // 插件节点所需的权限
}

pub enum NodeCategory {
    Builtin,
    Plugin(String),  // 插件 ID
}

/// 节点类型注册表
pub struct WorkflowNodeTypeRegistry {
    entries: HashMap<String, NodeTypeRegistration>,
}

impl WorkflowNodeTypeRegistry {
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// 注册节点类型（内置节点在应用启动时调用，插件节点在加载时调用）
    pub fn register(&mut self, registration: NodeTypeRegistration) {
        self.entries.insert(registration.type_id.clone(), registration);
    }

    /// 注销节点类型（插件卸载时调用）
    pub fn unregister(&mut self, type_id: &str) {
        self.entries.remove(type_id);
    }

    /// 获取执行器
    pub fn get_executor(&self, type_id: &str) -> Option<&Box<dyn NodeExecutorTrait>> {
        self.entries.get(type_id).map(|r| &r.executor)
    }

    /// 获取所有注册类型（用于前端同步）
    pub fn get_all_registrations(&self) -> Vec<NodeTypeRegistrationInfo> {
        self.entries.iter().map(|(id, reg)| NodeTypeRegistrationInfo {
            type_id: id.clone(),
            name: reg.name.clone(),
            category: match &reg.category {
                NodeCategory::Builtin => "builtin".to_string(),
                NodeCategory::Plugin(pid) => format!("plugin:{}", pid),
            },
            config_schema: reg.config_schema.clone(),
        }).collect()
    }
}
```

#### 初始化流程

```
应用启动
  ├─ 1. 创建 WorkflowNodeTypeRegistry
  ├─ 2. 注册内置节点类型
  │   ├─ agent_task  → AgentExecutor
  │   ├─ api_call    → ApiExecutor
  │   ├─ condition   → ConditionExecutor
  │   ├─ transform   → TransformExecutor
  │   ├─ aggregator  → AggregatorExecutor
  │   └─ human_input → HumanInputExecutor
  ├─ 3. 扫描插件目录
  │   └─ 对每个启用的插件
  │       └─ 如果 manifest 声明了 node_types
  │           └─ 注册到 registry（executor 为 PluginNodeExecutor）
  └─ 4. 同步注册信息到前端（通过 Tauri Command: list_node_types）
```

#### 新增 Tauri 命令

| 命令 | 参数 | 返回值 | 说明 |
|------|------|--------|------|
| `list_node_types` | 无 | `Vec<NodeTypeRegistrationInfo>` | 获取所有注册的节点类型（内置 + 插件） |

### 5.4 AgentExecutor（Agent 任务执行器）

**文件**：`src-tauri/src/workflow/agents/agent_executor.rs`

#### 关键设计：单次执行模式

工作流中的 Agent 调用与现有会话模式不同——工作流节点是**单次任务执行**，而非持续对话。需要新增"单次执行"模式：

> **输出解析复用**：Agent 的 stdout/stderr 解析逻辑（Claude JSON stream、Hermes 文本行、Codex JSONL）直接复用 `Agent会话延续能力设计-v2.0.md` 中已在 `agent/mod.rs` 实现的解析代码，避免重复实现。`execute_once` 作为 `AgentManager` 的方法实现，内部调用已有的进程管理和输出解析代码。
>
> **工作目录体系**：节点级 `cwd` 为可选覆盖，默认继承全局 `pilotdesk-workspace` 设置（三层兜底：用户输入路径 → 全局设置 → Rust current_dir()），路径校验规则（`ensure_dir`）复用会话延续设计中的实现。详见 `Agent会话延续能力设计-v2.0.md` 第3.1节。
>
> **agent_session_id 追溯**：Agent 进程启动后会生成 `agent_session_id`（Claude UUID / Hermes 时间戳 / Codex thread_id），节点执行完成后存入 `node_executions.agent_session_id` 字段，用于调试时回溯 Agent 侧会话日志。

```rust
pub struct AgentExecutor {
    agent_manager: Arc<AgentManager>,
}

impl AgentExecutor {
    pub async fn execute_once(
        &self,
        agent_type: &str,
        prompt: &str,
        params: &ModelParams,
        cwd: &str,
        execution_id: &str,
        node_execution_id: &str,
        emitter: &tauri::Emitter,
    ) -> Result<String, AppError> {
        // 1. 构造临时会话 ID（与工作流执行绑定）
        let temp_session_id = format!("wf_{}_{}", execution_id, node_execution_id);

        // 2. 调用 AgentManager 执行单次任务
        let output = self.agent_manager.execute_once(
            agent_type,
            prompt,
            params,
            cwd,
            temp_session_id,
            |chunk| {
                emitter.emit("workflow:chunk", serde_json::json!({
                    "execution_id": execution_id,
                    "node_execution_id": node_execution_id,
                    "content": chunk,
                })).ok();
            },
        ).await?;

        // 3. 解析输出（根据 output_parser 配置）
        let parsed = match params.output_parser.as_deref() {
            Some("json") => extract_json(&output),
            Some("markdown") => output,
            _ => output,
        };

        Ok(parsed)
    }
}
```

#### AgentManager 新增方法

需要在现有 `AgentManager` 中新增 `execute_once` 方法：

```rust
// agent/mod.rs 新增
impl AgentManager {
    /// 单次执行模式：启动 Agent → 发送消息 → 等待完成 → 关闭进程
    ///
    /// 与持续会话模式的区别：
    /// - 不使用 --resume，每次启动新进程
    /// - 输出解析复用 agent/mod.rs 中已有的实现（Claude JSON stream / Hermes 文本 / Codex JSONL）
    /// - 工作目录继承全局 pilotdesk-workspace 设置（三层兜底）
    /// - 提取 agent_session_id 并存入 node_executions 表，用于调试追溯
    pub async fn execute_once(
        &self,
        agent_type: &str,
        prompt: &str,
        params: &ModelParams,
        cwd: &str,
        execution_id: &str,
        node_execution_id: &str,
        emitter: &tauri::Emitter,
    ) -> Result<(String, Option<String>), AppError> {
        // 1. 构造 CLI 命令（根据 agent_type，复用 build_args 中的已验证参数）
        let (cmd, args) = self.build_once_command(agent_type, prompt, params, cwd)?;

        // 2. 确定工作目录（三层兜底：节点cwd → 全局设置 → current_dir）
        let effective_cwd = resolve_work_dir(cwd);

        // 3. spawn 子进程
        let child = tokio::process::Command::new(&cmd)
            .args(&args)
            .current_dir(&effective_cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| AppError::External(format!("启动 Agent 失败: {}", e)))?;

        // 4. 读取 stdout/stderr，复用 agent/mod.rs 中的解析逻辑
        //    - Claude: JSON stream 解析 type=system/subtype=init 提取 session_id
        //    - Codex: JSONL 解析 type=thread.started 提取 thread_id
        //    - Hermes: stderr 文本行提取 session_id: xxxxx
        let stdout = child.stdout.unwrap();
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut full_output = String::new();
        let mut agent_session_id: Option<String> = None;

        while let Some(line) = lines.next_line().await.map_err(|e| AppError::Io(e.to_string()))? {
            // 尝试解析 session_id（复用 agent/mod.rs 中的提取逻辑）
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
                if agent_type == "claude" && event["type"] == "system" && event["subtype"] == "init" {
                    agent_session_id = event["session_id"].as_str().map(String::from);
                }
                if agent_type == "codex" && event["type"] == "thread.started" {
                    agent_session_id = event["thread_id"].as_str().map(String::from);
                }
            }
            emitter.emit("workflow:chunk", serde_json::json!({
                "execution_id": execution_id,
                "node_execution_id": node_execution_id,
                "content": line.clone(),
            })).ok();
            full_output.push_str(&line);
            full_output.push('\n');
        }

        // 5. 等待进程退出
        let status = child.wait().await.map_err(|e| AppError::External(e.to_string()))?;

        if !status.success() {
            return Err(AppError::External(format!("Agent 执行失败: exit code {:?}", status.code())));
        }

        // 6. 解析输出（根据 output_parser 配置）
        let parsed = match params.output_parser.as_deref() {
            Some("json") => extract_json(&full_output.trim()),
            Some("markdown") => full_output.trim().to_string(),
            _ => full_output.trim().to_string(),
        };

        Ok((parsed, agent_session_id))
    }

    fn build_once_command(&self, agent_type: &str, prompt: &str, params: &ModelParams, cwd: &str) -> Result<(String, Vec<String>), AppError> {
        // CLI 命令与会话延续设计（Agent会话延续能力设计-v2.0.md 2.1节）对齐
        // 使用已验证通过的完整参数，确保 Agent 非交互式执行
        match agent_type {
            "claude" => Ok(("claude".into(), vec![
                "-p".into(),
                "--output-format".into(), "stream-json".into(),
                "--verbose".into(),
                "--dangerously-skip-permissions".into(),
                prompt.into(),
            ])),
            "hermes" => Ok(("hermes".into(), vec![
                "chat".into(), "-q".into(), prompt.into(), "-Q".into(),
            ])),
            "codex" => Ok(("codex".into(), vec![
                "exec".into(),
                "--json".into(),
                "--skip-git-repo-check".into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
                prompt.into(),
            ])),
            _ => Err(AppError::InvalidInput(format!("不支持的 Agent 类型: {}", agent_type))),
        }
    }
}
```

### 5.5 TemplateEngine（参数传递引擎）

**文件**：`src-tauri/src/workflow/template.rs`

```rust
pub struct TemplateEngine;

impl TemplateEngine {
    /// 解析模板字符串，替换所有 {{variable}} 占位符
    pub fn resolve(
        template: &str,
        context: &HashMap<String, Value>,
    ) -> Result<String, AppError> {
        let re = Regex::new(r"\{\{(.+?)\}\}").map_err(|e| AppError::Config(e.to_string()))?;
        let mut result = template.to_string();

        for cap in re.captures_iter(template) {
            let expression = cap.get(1).unwrap().as_str().trim();
            let resolved = Self::resolve_expression(expression, context)?;
            result = result.replace(&cap[0], &resolved);
        }

        Ok(result)
    }

    /// 解析单个表达式，支持 JSONPath
    fn resolve_expression(expr: &str, context: &HashMap<String, Value>) -> Result<String, AppError> {
        let parts: Vec<&str> = expr.splitn(2, '.').collect();
        if parts.len() < 2 {
            return Err(AppError::InvalidInput(format!("无效的模板变量: {}", expr)));
        }

        let node_id = parts[0];
        let path = parts[1];

        let value = context.get(node_id)
            .ok_or_else(|| AppError::NotFound(format!("节点 {} 的输出不存在", node_id)))?;

        Self::jsonpath_extract(value, path)
    }

    fn jsonpath_extract(value: &Value, path: &str) -> Result<String, AppError> {
        let mut current = value.clone();

        for segment in path.split('.') {
            if segment.is_empty() { continue; }

            if let Some(idx_start) = segment.find('[') {
                let field = &segment[..idx_start];
                let idx_str = &segment[idx_start+1..segment.len()-1];

                if !field.is_empty() {
                    current = current.get(field)
                        .ok_or_else(|| AppError::NotFound(format!("字段 {} 不存在", field)))?
                        .clone();
                }

                let idx: usize = idx_str.parse()
                    .map_err(|_| AppError::InvalidInput(format!("无效的数组索引: {}", idx_str)))?;
                current = current.get(idx)
                    .ok_or_else(|| AppError::NotFound(format!("数组索引 {} 越界", idx)))?
                    .clone();
            } else {
                current = current.get(segment)
                    .ok_or_else(|| AppError::NotFound(format!("字段 {} 不存在", segment)))?
                    .clone();
            }
        }

        Ok(match &current {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            other => serde_json::to_string(other)
                .map_err(|e| AppError::Json(e.to_string()))?,
        })
    }
}
```

### 5.6 TransformExecutor（代码转换执行器）

**文件**：`src-tauri/src/workflow/agents/transform_executor.rs`

```rust
pub struct TransformExecutor;

impl TransformExecutor {
    /// 执行 JavaScript 转换脚本
    /// 使用 boa_engine 作为嵌入式 JS 运行时
    pub fn execute(script: &str, inputs: &HashMap<String, Value>) -> Result<Value, AppError> {
        let mut engine = boa_engine::Context::default();

        let inputs_json = serde_json::to_string(inputs)
            .map_err(|e| AppError::Json(e.to_string()))?;
        engine.eval::<boa_engine::JsValue>(
            &format!("const inputs = {};", inputs_json)
        ).map_err(|e| AppError::External(format!("JS 执行错误: {}", e)))?;

        let result = engine.eval::<boa_engine::JsValue>(script)
            .map_err(|e| AppError::External(format!("JS 执行错误: {}", e)))?;

        let result_json = result.to_json(&mut engine)
            .map_err(|e| AppError::External(format!("JS 序列化错误: {}", e)))?;

        Ok(result_json)
    }
}
```

**依赖项**（Cargo.toml 新增）：

```toml
boa_engine = "0.19"    # 嵌入式 JavaScript 引擎
regex = "1"            # 模板变量正则解析
```

### 5.7 Tauri Commands

**文件**：`src-tauri/src/commands/workflow.rs`

#### 工作流 CRUD 命令

| 命令                         | 参数                             | 返回值                    | 说明                       |
| -------------------------- | ------------------------------ | ---------------------- | ------------------------ |
| `create_workflow`          | name, description?, tags?      | `Workflow`             | 创建工作流                    |
| `list_workflows`           | status?                        | `Vec<Workflow>`        | 获取工作流列表                  |
| `get_workflow`             | id                             | `WorkflowDetail`       | 获取工作流详情（含 nodes + edges） |
| `update_workflow`          | id, name?, description?, tags? | `Workflow`             | 更新工作流元数据                 |
| `delete_workflow`          | id                             | `()`                   | 删除工作流                    |
| `duplicate_workflow`       | id, new_name                   | `Workflow`             | 复制工作流                    |
| `save_workflow_dag`        | id, nodes, edges               | `WorkflowVersion`      | 保存 DAG 并创建新版本            |
| `list_workflow_versions`   | workflow_id                    | `Vec<WorkflowVersion>` | 获取版本列表                   |
| `restore_workflow_version` | workflow_id, version           | `WorkflowVersion`      | 恢复到指定版本                  |

#### 执行控制命令

| 命令                            | 参数                                 | 返回值                      | 说明       |
| ----------------------------- | ---------------------------------- | ------------------------ | -------- |
| `start_workflow`              | workflow_id, version?, input_data? | `WorkflowExecution`      | 启动工作流执行  |
| `cancel_workflow`             | execution_id                       | `()`                     | 中止工作流执行  |
| `get_execution`               | execution_id                       | `WorkflowExecution`      | 获取执行状态   |
| `list_executions`             | workflow_id, limit?                | `Vec<WorkflowExecution>` | 获取执行历史   |
| `get_node_executions`         | execution_id                       | `Vec<NodeExecution>`     | 获取节点执行详情 |
| `get_node_execution_logs`     | node_execution_id                  | `Vec<NodeExecutionLog>`  | 获取节点执行日志 |
| `list_recoverable_executions` | 无                                  | `Vec<WorkflowExecution>` | 获取可恢复的执行 |
| `recover_execution`           | execution_id                       | `WorkflowExecution`      | 恢复中断的执行  |
| `respond_human_input`         | execution_id, node_id, response   | `()`                     | 提交人工介入节点的用户响应，恢复工作流执行 |
| `get_pending_human_inputs`    | 无                                  | `Vec<PendingInput>`      | 获取所有等待用户响应的人工介入节点 |

### 5.8 数据库迁移

**文件**：`src-tauri/src/db/init.rs`

新增迁移函数 `migrate_add_workflow_tables()`（v8）：

```rust
pub fn migrate_add_workflow_tables(conn: &Connection) -> Result<(), AppError> {
    let table_exists = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='workflows'")
        .and_then(|mut stmt| stmt.exists([]))
        .unwrap_or(false);

    if table_exists {
        return Ok(());
    }

    conn.execute_batch("
        CREATE TABLE workflows ( ... );
        CREATE TABLE workflow_versions ( ... );
        CREATE TABLE workflow_nodes ( ... );
        CREATE TABLE workflow_edges ( ... );
        CREATE TABLE workflow_executions ( ... );
        CREATE TABLE node_executions ( ... );
        CREATE TABLE node_execution_logs ( ... );
    ")?;

    Ok(())
}
```

---

## 6. 前端设计

### 6.1 新增目录结构

```
src/
├── components/
│   ├── workflow/                    # 新增：工作流模块
│   │   ├── DAGEditor.tsx            # DAG 编辑器主组件（react-flow）
│   │   ├── NodePalette.tsx          # 节点类型面板（从 Registry 获取列表，插件节点带徽章）
│   │   ├── NodeConfigPanel.tsx      # 节点配置面板（内置节点用定制表单，插件节点根据 configSchema 动态生成）
│   │   ├── WorkflowToolbar.tsx      # 编辑器工具栏
│   │   ├── WorkflowList.tsx         # 工作流列表
│   │   ├── WorkflowCard.tsx         # 工作流卡片
│   │   ├── ExecutionMonitor.tsx     # 执行监控面板
│   │   ├── ExecutionTimeline.tsx    # 执行时间线
│   │   ├── NodeStatusBadge.tsx      # 节点状态徽章
│   │   ├── VariableSelector.tsx     # 变量选择器
│   │   ├── TriggerConfigPanel.tsx   # 触发器配置面板
│   │   ├── HumanInputPanel.tsx      # 人工介入输入面板（文本/选择/确认）
│   │   └── nodes/                   # 自定义节点组件
│   │       ├── TriggerNode.tsx
│   │       ├── AgentTaskNode.tsx
│   │       ├── ApiCallNode.tsx
│   │       ├── ConditionNode.tsx
│   │       ├── TransformNode.tsx
│   │       ├── AggregatorNode.tsx
│   │       ├── HumanInputNode.tsx
│   │       └── index.ts
│   └── ...
├── pages/
│   ├── WorkflowPage.tsx             # 新增：工作流列表页
│   ├── WorkflowEditorPage.tsx       # 新增：工作流编辑器页
│   ├── ExecutionDetailPage.tsx      # 新增：执行详情页
│   └── ...
├── stores/
│   ├── workflowStore.ts             # 新增：工作流状态管理
│   └── ...
├── types/
│   ├── workflow.ts                  # 新增：工作流类型定义
│   └── ...
├── hooks/
│   ├── useWorkflowExecution.ts      # 新增：执行状态 hook
│   └── ...
└── styles/
    ├── workflow.css                 # 新增：工作流组件样式
    └── ...
```

### 6.2 页面路由设计

```typescript
// App.tsx 新增路由
<Routes>
  {/* 现有路由 */}
  <Route path="/" element={<MainLayout />} />
  <Route path="/settings" element={<SettingsPage />} />
  <Route path="/env" element={<EnvPage />} />

  {/* 新增工作流路由 */}
  <Route path="/workflows" element={<WorkflowPage />} />
  <Route path="/workflows/:id/edit" element={<WorkflowEditorPage />} />
  <Route path="/workflows/:id/executions/:execId" element={<ExecutionDetailPage />} />
</Routes>
```

### 6.3 页面布局

#### 工作流列表页（WorkflowPage）

```
┌──────────────────────────────────────────────────────────┐
│ ◉ PilotDesk          📦 👁⚙  │ — □ ✕ │
├──────────────────────────────────────────────────────────┤
│ ← 返回    工作流管理                                      │
├──────────────────────────────────────────────────────────┤
│ [+ 新建工作流]  [搜索工作流...]                            │
├──────────────────────────────────────────────────────────┤
│ ┌──────────────────────────────────────────────────────┐ │
│ │  ┌──────────┐  ┌──────────┐  ┌──────────┐          │ │
│ │  │ 内容生成   │  │ 代码审查  │  │ 数据清洗  │          │ │
│ │  │ 流水线     │  │ 流水线    │  │ 流水线    │          │ │
│ │  │ 3 节点     │  │ 5 节点    │  │ 2 节点    │          │ │
│ │  │ 上次运行   │  │ 上次运行  │  │ 上次运行  │          │ │
│ │  │ 成功       │  │ 失败      │  │ 成功      │          │ │
│ │  └──────────┘  └──────────┘  └──────────┘          │ │
│ │                                                      │ │
│ │  ┌──────────┐  ┌──────────┐                          │ │
│ │  │ 周报生成   │  │ 翻译校对  │                          │ │
│ │  │ 流水线     │  │ 流水线    │                          │ │
│ │  │ 4 节点     │  │ 2 节点    │                          │ │
│ │  │ 从未运行   │  │ 上次运行  │                          │ │
│ │  │            │  │ 成功      │                          │ │
│ │  └──────────┘  └──────────┘                          │ │
│ └──────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────┤
│ 🟢 已连接  |  设置                                        │
└──────────────────────────────────────────────────────────┘
```

#### 工作流编辑器页（WorkflowEditorPage）

```
┌──────────────────────────────────────────────────────────┐
│ ◉ PilotDesk          📦 👁⚙  │ — □ ✕ │
├──────────────────────────────────────────────────────────┤
│ ← 返回    内容生成流水线  [保存] [运行] [版本 v3]         │
├──────────────────────────────────────────────────────────┤
│ ┌──────────┐  ┌──────────────────────────────────────┐  │
│ │ 节点类型   │  │                                      │  │
│ │ ────────  │  │    ┌──────────┐                      │  │
│ │ 🔵 触发器  │  │    │ 触发器    │                      │  │
│ │           │  │    │ (手动)    │                      │  │
│ │ 🤖 Agent  │  │    └────┬─────┘                      │  │
│ │   任务     │  │         │                            │  │
│ │           │  │    ┌────▼─────┐  ┌──────────┐        │  │
│ │ 🌐 API    │  │    │ 调研主题   │  │ 条件分支   │        │  │
│ │   调用     │  │    │ (Claude)  │  │ (if成功)  │        │  │
│ │           │  │    └────┬─────┘  └────▲─────┘        │  │
│ │ 🔀 条件    │  │         │             │              │  │
│ │   分支     │  │    ┌────▼─────┐       │              │  │
│ │           │  │    │ 撰写内容   │       │              │  │
│ │ 📝 代码    │  │    │ (Claude)  ├───────┘              │  │
│ │   转换     │  │    └────┬─────┘                      │  │
│ │           │  │         │                            │  │
│ │ 🔗 聚合    │  │    ┌────▼─────┐                      │  │
│ │           │  │    │ 翻译校对   │                      │  │
│ │           │  │    │ (Hermes)  │                      │  │
│ │           │  │    └──────────┘                      │  │
│ │           │  │                                      │  │
│ └──────────┘  │          [迷你地图]                    │  │
│               └──────────────────────────────────────┘  │
├──────────────────────────────────────────────────────────┤
│ 🟢 已连接  |  设置                                        │
└──────────────────────────────────────────────────────────┘
```

### 6.4 DAGEditor 组件设计

**文件**：`src/components/workflow/DAGEditor.tsx`

#### 技术选型

使用 **react-flow**（xyflow）库：

```json
{
  "dependencies": {
    "@xyflow/react": "^12.5.0"
  }
}
```

#### 核心结构

```typescript
import {
  ReactFlow, Background, Controls, MiniMap,
  useNodesState, useEdgesState, addEdge,
  Connection, Node, Edge, NodeTypes, EdgeTypes,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';

// nodeTypes 从 WorkflowNodeTypeRegistry 动态构建
// 内置节点在应用启动时注册，插件节点在加载时注册
// 确保插件节点与内置节点在编辑器中行为完全一致
const nodeTypes: NodeTypes = workflowNodeRegistry.getNodeComponents();
  trigger: TriggerNode,
  agent_task: AgentTaskNode,
  api_call: ApiCallNode,
  condition: ConditionNode,
  transform: TransformNode,
  aggregator: AggregatorNode,
  human_input: HumanInputNode,
};

function DAGEditor({ workflowId }: { workflowId: string }) {
  const [nodes, setNodes, onNodesChange] = useNodesState([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState([]);
  const [selectedNode, setSelectedNode] = useState<Node | null>(null);
  const { loadWorkflow } = useWorkflowStore();

  useEffect(() => {
    loadWorkflow(workflowId).then((data) => {
      setNodes(data.nodes);
      setEdges(data.edges);
    });
  }, [workflowId]);

  const onConnect = useCallback(
    (params: Connection) => setEdges((eds) => addEdge(params, eds)),
    [setEdges],
  );

  const onDrop = useCallback((event: DragEvent) => {
    const type = event.dataTransfer?.getData('application/reactflow');
    if (!type) return;
    const position = screenToFlowPosition({ x: event.clientX, y: event.clientY });
    setNodes((nds) => nds.concat(createNode(type, position)));
  }, [screenToFlowPosition]);

  const onNodeClick = useCallback((_: React.MouseEvent, node: Node) => {
    setSelectedNode(node);
  }, []);

  return (
    <div className="workflow-editor">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onDrop={onDrop}
        onNodeClick={onNodeClick}
        nodeTypes={nodeTypes}
        fitView
        deleteKeyCode="Delete"
        multiSelectionKeyCode="Shift"
      >
        <Background />
        <Controls />
        <MiniMap />
      </ReactFlow>

      {selectedNode && (
        <NodeConfigPanel
          node={selectedNode}
          onUpdate={(updates) => updateNode(selectedNode.id, updates)}
          onClose={() => setSelectedNode(null)}
        />
      )}
    </div>
  );
}
```

#### 节点视觉样式

```css
.workflow-node {
  @apply bg-[var(--bg-secondary)] border border-[var(--border-default)] rounded-lg
         min-w-[180px] shadow-sm transition-shadow duration-150;
}
.workflow-node:hover {
  @apply border-[var(--border-hover)] shadow-md;
}

.workflow-node--idle     { @apply border-[var(--border-default)]; }
.workflow-node--running  { @apply border-[#5B7FFF] shadow-[0_0_12px_rgba(91,127,255,0.3)]; }
.workflow-node--completed { @apply border-[#34D399]; }
.workflow-node--failed   { @apply border-[#F87171]; }
.workflow-node--skipped  { @apply border-[#8B8B9E] opacity-60; }

.workflow-node--trigger    { --node-accent: #60A5FA; }
.workflow-node--agent_task { --node-accent: #8B5CF6; }
.workflow-node--api_call   { --node-accent: #F59E0B; }
.workflow-node--condition  { --node-accent: #F87171; }
.workflow-node--transform  { --node-accent: #34D399; }
.workflow-node--aggregator { --node-accent: #38BDF8; }
.workflow-node--human_input { --node-accent: #F472B6; }
```

### 6.5 ExecutionMonitor（执行监控面板）

**文件**：`src/components/workflow/ExecutionMonitor.tsx`

```
┌──────────────────────────────────────────────────────────┐
│ 执行监控 — 内容生成流水线 #exec_20260618_001             │
│ [中止]                                                    │
├──────────────────────────────────────────────────────────┤
│ 进度: ████████████░░░░░░░ 3/5 节点                       │
│ 状态: ● 运行中   已耗时: 45s                              │
├──────────────────────────────────────────────────────────┤
│                                                          │
│    ┌──────────┐                                          │
│    │ 触发器    │ ● 已完成 (0.5s)                          │
│    └────┬─────┘                                          │
│         │                                                │
│    ┌────▼─────┐  ┌──────────┐                            │
│    │ 调研主题   │  │ 条件分支   │ ● 等待中                  │
│    │ ● 运行中   │  └──────────┘                            │
│    │ ⏱ 23s     │                                          │
│    └────┬─────┘                                          │
│         │                                                │
│    ┌────▼─────┐                                          │
│    │ 撰写内容   │ ○ 等待中                                 │
│    └──────────┘                                          │
│                                                          │
├──────────────────────────────────────────────────────────┤
│ 节点详情: 调研主题                                         │
├──────────────────────────────────────────────────────────┤
│ 输入:                                                     │
│ ┌──────────────────────────────────────────────────────┐ │
│ │ { "topic": "AI Agent 在软件开发中的应用" }            │ │
│ └──────────────────────────────────────────────────────┘ │
│                                                          │
│ 输出（流式）:                                              │
│ ┌──────────────────────────────────────────────────────┐ │
│ │ 1. AI Agent 概述                                     │ │
│ │ 2. 代码自动生成                                       │ │
│ │ 3. 智能调试与错误修复...                               │ │
│ └──────────────────────────────────────────────────────┘ │
│                                                          │
│ 日志:                                                    │
│ ┌──────────────────────────────────────────────────────┐ │
│ │ [14:23:01] INFO  开始执行节点 "调研主题"               │ │
│ │ [14:23:02] INFO  启动 Claude Code 进程               │ │
│ │ [14:23:05] INFO  开始接收流式输出                     │ │
│ │ [14:23:28] INFO  节点执行完成，耗时 27s               │ │
│ └──────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
```

### 6.6 workflowStore（状态管理）

**文件**：`src/stores/workflowStore.ts`

```typescript
interface WorkflowStoreState {
  workflows: Workflow[];
  loading: boolean;
  currentWorkflow: WorkflowDetail | null;
  nodes: NodeDef[];
  edges: EdgeDef[];
  isDirty: boolean;
  activeExecution: WorkflowExecution | null;
  nodeExecutionStates: Map<string, NodeExecutionState>;

  loadWorkflows: () => Promise<void>;
  loadWorkflow: (id: string) => Promise<void>;
  saveWorkflow: () => Promise<void>;
  createWorkflow: (name: string) => Promise<Workflow>;

  addNode: (type: string, position: Position) => void;
  updateNode: (id: string, updates: Partial<NodeDef>) => void;
  removeNode: (id: string) => void;
  addEdge: (source: string, target: string) => void;
  updateEdge: (id: string, updates: Partial<EdgeDef>) => void;
  removeEdge: (id: string) => void;

  startExecution: (inputData?: Record<string, unknown>) => Promise<void>;
  cancelExecution: () => Promise<void>;
  updateNodeExecutionState: (nodeId: string, state: NodeExecutionState) => void;
}
```

### 6.7 useWorkflowExecution Hook

**文件**：`src/hooks/useWorkflowExecution.ts`

```typescript
function useWorkflowExecution(executionId: string | null) {
  const [execution, setExecution] = useState<WorkflowExecution | null>(null);
  const [nodeStates, setNodeStates] = useState<Map<string, NodeExecutionState>>(new Map());
  const [streamingOutputs, setStreamingOutputs] = useState<Map<string, string>>(new Map());

  useEffect(() => {
    if (!executionId) return;

    const unlisteners = [
      listen<WorkflowChunkPayload>('workflow:chunk', (event) => {
        const { nodeExecutionId, content } = event.payload;
        setStreamingOutputs(prev => {
          const next = new Map(prev);
          next.set(nodeExecutionId, (prev.get(nodeExecutionId) || '') + content);
          return next;
        });
      }),

      listen<WorkflowNodeStatusPayload>('workflow:node-status', (event) => {
        const { nodeId, status, output, error } = event.payload;
        setNodeStates(prev => {
          const next = new Map(prev);
          next.set(nodeId, { status, output, error, updatedAt: Date.now() });
          return next;
        });
      }),

      listen<WorkflowExecutionStatusPayload>('workflow:execution-status', (event) => {
        setExecution(event.payload.execution);
      }),
    ];

    return () => { unlisteners.forEach(u => u.then(fn => fn())); };
  }, [executionId]);

  return { execution, nodeStates, streamingOutputs };
}
```

---

## 7. 通信协议

### 7.1 新增 Tauri Events

| 事件名                         | 载荷                                                    | 方向        | 说明           |
| --------------------------- | ----------------------------------------------------- | --------- | ------------ |
| `workflow:chunk`            | `{ execution_id, node_execution_id, content }`        | Rust → 前端 | Agent 流式输出片段 |
| `workflow:node-status`      | `{ execution_id, node_id, status, output?, error? }`  | Rust → 前端 | 节点状态变更       |
| `workflow:execution-status` | `{ execution: WorkflowExecution }`                    | Rust → 前端 | 工作流整体状态变更    |
| `workflow:log`              | `{ execution_id, node_execution_id, level, message }` | Rust → 前端 | 节点执行日志       |
| `workflow:progress`         | `{ execution_id, completed, total }`                  | Rust → 前端 | 进度更新         |
| `workflow:awaiting-input`   | `{ execution_id, node_id, prompt, input_type, options?, allow_custom?, placeholder?, timeout_minutes }` | Rust → 前端 | 人工介入节点等待用户输入 |
| `workflow:input-resolved`   | `{ execution_id, node_id, response }`                 | Rust → 前端 | 人工介入已收到响应，工作流恢复 |

### 7.2 事件序列

```
用户点击"运行"
  → 前端 invoke('start_workflow', { workflowId, inputData })
    → Rust: WorkflowEngine.execute()
      → emit "workflow:execution-status" { status: "running" }
      → 逐层执行节点

      层 1:
        → emit "workflow:node-status" { nodeId: "trigger", status: "running" }
        → 执行 trigger 节点
        → emit "workflow:node-status" { nodeId: "trigger", status: "completed", output }
        → emit "workflow:progress" { completed: 1, total: 5 }

      层 2（并行）:
        → emit "workflow:node-status" { nodeId: "node_001", status: "running" }
        → emit "workflow:node-status" { nodeId: "node_002", status: "running" }
        → emit "workflow:chunk" { nodeId: "node_001", content: "..." }  (多次)
        → emit "workflow:log" { nodeId: "node_001", level: "info", message: "..." }
        → emit "workflow:node-status" { nodeId: "node_001", status: "completed" }
        → emit "workflow:node-status" { nodeId: "node_002", status: "completed" }
        → emit "workflow:progress" { completed: 3, total: 5 }

      ...

      完成:
        → emit "workflow:execution-status" { status: "completed" }

### 7.3 人工介入事件序列

```
工作流执行到 human_input 节点
  → emit "workflow:node-status" { nodeId: "node_003", status: "paused" }
  → emit "workflow:awaiting-input" {
      execution_id, node_id: "node_003",
      prompt: "请选择下一步操作：",
      input_type: "select",
      options: [{ label: "继续执行", value: "continue" }, { label: "终止", value: "abort" }],
      timeout_minutes: 30
    }
  → 前端展示输入面板（根据 input_type 渲染对应控件）
    → 用户提交响应
      → 前端 invoke('respond_human_input', { executionId, nodeId: "node_003", response: "continue" })
        → Rust: 通过 oneshot channel 恢复挂起的 task
        → emit "workflow:input-resolved" { execution_id, node_id: "node_003", response: "continue" }
        → emit "workflow:node-status" { nodeId: "node_003", status: "completed", output: { user_input: "continue" } }
        → 工作流继续执行后续节点

  [超时场景]
  → 30 分钟无响应
    → Rust: tokio::time::timeout 触发
    → emit "workflow:log" { level: "warn", message: "人工介入超时，使用默认值" }
    → 使用 config.default_value 继续执行
    → emit "workflow:node-status" { nodeId: "node_003", status: "completed", output: { user_input: "continue" } }
```

---

## 8. 状态管理

### 8.1 Zustand Store 扩展

| Store               | 现有职责  | 新增职责                   |
| ------------------- | ----- | ---------------------- |
| `workflowStore`     | —     | 工作流 CRUD、DAG 编辑状态、执行状态 |
| `sessionStore`      | 会话管理  | 不变                     |
| `pendingInputStore` | 待发送输入 | 不变                     |

### 8.2 编辑器状态管理

DAG 编辑器内部使用 react-flow 自带的 `useNodesState` / `useEdgesState` 管理画布状态，通过 `workflowStore` 与 SQLite 同步：

```
用户操作画布
  → react-flow 内部状态更新（即时响应）
  → 用户点击"保存"
    → workflowStore.saveWorkflow()
      → invoke('save_workflow_dag', { id, nodes, edges })
        → Rust: 创建新版本
        → 返回 WorkflowVersion
      → isDirty = false
```

### 8.3 执行状态管理

执行状态使用独立的 Hook `useWorkflowExecution` 管理，通过 Tauri Events 实时更新：

```
Tauri Events (Rust → 前端)
  → useWorkflowExecution Hook
    → nodeStates: Map<nodeId, { status, output, error }>
    → streamingOutputs: Map<nodeId, string>
    → execution: WorkflowExecution

组件通过 Hook 返回值订阅所需数据
  → ExecutionMonitor: 全量状态
  → DAGEditor: 节点颜色状态
  → NodeConfigPanel: 当前选中节点详情
```

---

## 9. 与现有系统的集成

### 9.1 与 AgentManager 集成

| 集成点        | 方式              | 说明                               |
| ---------- | --------------- | -------------------------------- |
| Agent 进程管理 | 复用 AgentManager | 新增 `execute_once()` 方法，区别于持续会话模式 |
| Agent 类型枚举 | 复用              | claude / hermes / codex / api    |
| 环境检测       | 复用              | 执行前检查 Agent 是否已安装                |

### 9.2 与 SQLite 集成

| 集成点  | 方式                                                                  |
| ---- | ------------------------------------------------------------------- |
| 连接池  | 复用 r2d2 Pool                                                        |
| 迁移机制 | 新增 v8 迁移函数                                                          |
| 事务   | 工作流保存使用事务（workflow_nodes + workflow_edges + workflow_versions 同时写入） |

### 9.3 与前端基础设施集成

| 集成点              | 方式                         |
| ---------------- | -------------------------- |
| invokeHelper     | 复用，新增 workflow 命令          |
| useReducer       | 复用模式，ExecutionMonitor 内部使用 |
| react-virtuoso   | 复用，执行日志列表                  |
| MarkdownRenderer | 复用，节点输出渲染                  |
| AGENT_THEMES     | 复用，节点颜色标识                  |
| CSS 设计令牌         | 复用，工作流 UI 使用现有变量体系         |
| 主题系统             | 复用，工作流页面自动适配深色/浅色          |

### 9.4 与设置系统集成

工作流相关的全局设置存入 `app_settings` 表：

| key                        | value | 说明       |
| -------------------------- | ----- | -------- |
| `workflow_default_timeout` | `300` | 默认节点超时时间 |
| `workflow_max_concurrency` | `5`   | 最大并行节点数  |

> **过程产物存储目录**：复用已有的 `pilotdesk-workspace` 设置（SettingsPage 中配置的工作区目录），不另设独立配置项。节点级 `cwd` 为可选覆盖，默认继承该工作区目录（三层兜底：节点 cwd → `pilotdesk-workspace` → Rust `current_dir()`）。

### 9.5 与定时任务系统集成

工作流可作为定时任务的目标：

```
定时任务系统
  → 触发时调用 start_workflow(workflow_id, input_data)
  → 执行完成后通过 Tauri Event 通知
  → 执行结果记录到 workflow_executions
```

### 9.6 与插件系统集成

#### 9.6.1 插件节点类型注册

插件通过 `manifest.json` 的 `contributes.node_types` 字段声明节点类型：

```json
{
  "contributes": {
    "node_types": [
      {
        "type_id": "my-plugin.sentiment",
        "name": "情感分析",
        "config_schema": {
          "type": "object",
          "properties": {
            "api_key": { "type": "string", "description": "API 密钥" }
          }
        },
        "permissions": ["network:http"]
      }
    ]
  }
}
```

插件加载流程：

```
插件加载
  ├─ 1. PluginHost 扫描插件目录，解析 manifest.json
  ├─ 2. 检查 contributes.node_types 字段
  ├─ 3. 构建 NodeTypeRegistration
  │   ├─ type_id: "my-plugin.sentiment"
  │   ├─ name: "情感分析"
  │   ├─ category: Plugin("my-plugin")
  │   ├─ executor: PluginNodeExecutor(plugin_id, type_id)
  │   └─ config_schema: { ... }
  ├─ 4. 调用 NodeExecutor.register_plugin_node_type(registration)
  └─ 5. 同步注册信息到前端（list_node_types 命令）
```

#### 9.6.2 PluginNodeExecutor（插件节点执行器）

**文件**：`src-tauri/src/workflow/agents/plugin_executor.rs`

插件节点执行器通过 PluginHost 将节点执行转发到插件的 JS 运行时：

```rust
pub struct PluginNodeExecutor {
    plugin_id: String,
    type_id: String,
    plugin_host: Arc<PluginHost>,
}

impl NodeExecutorTrait for PluginNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        resolved_input: Value,
        execution_id: &str,
        emitter: &tauri::Emitter,
    ) -> Result<NodeOutput, AppError> {
        // 1. 构造执行上下文
        let context = serde_json::json!({
            "node_id": node.id,
            "config": node.config,
            "input": resolved_input,
            "execution_id": execution_id,
        });

        // 2. 通过 PluginHost 调用插件 JS 的 executeNode 方法
        let result = self.plugin_host.call_plugin_fn(
            &self.plugin_id,
            "executeNode",
            &context,
        ).await?;

        // 3. 解析返回结果
        let output: Value = serde_json::from_str(&result)
            .map_err(|e| AppError::External(format!("插件节点返回无效 JSON: {}", e)))?;

        Ok(NodeOutput { output })
    }
}
```

#### 9.6.3 前端集成

| 集成点 | 方式 |
|--------|------|
| NodePalette | 从 `list_node_types` 命令获取完整列表，插件节点带 `[插件]` 徽章 |
| NodeConfigPanel | 内置节点用定制表单，插件节点根据 `config_schema` 动态生成表单 |
| DAGEditor | 插件节点与内置节点使用同一套 `nodeTypes` 注册机制 |
| WorkflowNodeTypeRegistry（前端） | 封装 Rust 端 `list_node_types` 命令，提供 `getNodeComponents()` 方法 |

#### 9.6.4 权限管理

插件节点执行时需要的权限在 `manifest.json` 的 `permissions` 字段声明，并在 `NodeTypeRegistration.permissions` 中记录。工作流执行前检查：

- 插件节点所需的权限是否已被用户授权
- 未授权的权限在首次执行时弹出确认对话框
- 权限拒绝则节点执行失败，工作流进入 `failed` 状态

#### 9.6.5 卸载处理

插件卸载时，PluginHost 调用 `NodeExecutor.unregister_plugin_node_type(type_id)` 从注册表中移除对应的节点类型。正在执行的工作流不受影响（已加载的节点执行器仍在内存中），但新工作流无法使用已卸载的节点类型。

---

## 10. 分阶段实施计划

### Phase 1：数据层 + 后端引擎（8-10 天）

| 任务                           | 预估    | 产出                |
| ---------------------------- | ----- | ----------------- |
| 数据模型 + 迁移                    | 1 天   | 7 张表 + Rust 模型    |
| WorkflowEngine 拓扑排序 + 并行调度   | 3 天   | DAG 调度器核心         |
| NodeExecutor + AgentExecutor | 2 天   | Agent 单次执行模式      |
| TemplateEngine 参数传递          | 1.5 天 | 模板变量解析 + JSONPath |
| TransformExecutor（JS 引擎）     | 1.5 天 | 嵌入式 JS 执行         |
| Tauri Commands（CRUD + 执行控制）  | 1 天   | 20+ 个命令           |

**交付物**：可通过 Rust 测试或 CLI 运行工作流（JSON 配置），验证 DAG 调度 + Agent 调用链 + 参数传递。

### Phase 2：前端 DAG 编辑器（6-8 天）

| 任务                       | 预估  | 产出                              |
| ------------------------ | --- | ------------------------------- |
| react-flow 集成 + 自定义节点    | 2 天 | 6 种节点类型渲染                       |
| NodePalette 拖拽面板         | 1 天 | 从侧边栏拖入画布                        |
| NodeConfigPanel（6 种配置表单） | 2 天 | 完整的节点配置 UI                      |
| 连线 + 参数映射 UI             | 1 天 | VariableSelector + ParamMapping |
| 撤销/重做 + 自动布局             | 1 天 | 编辑器体验完善                         |
| 工作流列表页                   | 1 天 | CRUD + 卡片展示                     |

**交付物**：完整的可视化工作流编辑器，可创建/编辑/保存工作流。

### Phase 3：执行监控 + 体验完善（5-7 天）

| 任务                  | 预估    | 产出               |
| ------------------- | ----- | ---------------- |
| ExecutionMonitor 面板 | 2 天   | 实时 DAG 状态 + 节点详情 |
| 流式输出展示              | 1 天   | 节点输出实时渲染         |
| 执行历史 + 日志浏览         | 1 天   | 历史执行回溯           |
| Checkpoint 恢复       | 1 天   | 应用重启后恢复执行        |
| 条件分支前端支持            | 0.5 天 | 条件边渲染 + 配置       |
| 工作流模板               | 0.5 天 | 内置模板 + 模板管理      |

**交付物**：完整的工作流运行和监控体验。

### Phase 4：增强功能（4-6 天）

| 任务         | 预估  | 产出          |
| ---------- | --- | ----------- |
| 定时触发集成     | 1 天 | cron 表达式配置  |
| 导出/导入 JSON | 1 天 | 工作流迁移       |
| 执行统计图表     | 1 天 | 耗时/成功率统计    |
| 节点测试功能     | 1 天 | 单节点独立测试     |
| 性能优化       | 1 天 | 50+ 节点工作流性能 |

**交付物**：生产级工作流管理系统。

### 总计预估：23-31 人天

---

## 11. 文件清单

### 11.1 Rust 后端新增

| 文件                                                    | 预估行数 | 职责                       |
| ----------------------------------------------------- | ---- | ------------------------ |
| `src-tauri/src/workflow/mod.rs`                       | ~50  | 模块声明 + WorkflowEngine 结构 |
| `src-tauri/src/workflow/engine.rs`                    | ~400 | DAG 调度引擎                 |
| `src-tauri/src/workflow/executor.rs`                  | ~100 | NodeExecutor 分发器         |
| `src-tauri/src/workflow/agents/mod.rs`                | ~30  | 执行器子模块声明                 |
| `src-tauri/src/workflow/agents/agent_executor.rs`     | ~200 | Agent 任务执行器              |
| `src-tauri/src/workflow/agents/api_executor.rs`       | ~100 | HTTP API 调用执行器           |
| `src-tauri/src/workflow/agents/condition_executor.rs` | ~60  | 条件分支执行器                  |
| `src-tauri/src/workflow/agents/transform_executor.rs` | ~80  | 代码转换执行器                  |
| `src-tauri/src/workflow/agents/human_input_executor.rs` | ~120 | 人工介入执行器（挂起/恢复 + 超时） |
| `src-tauri/src/workflow/agents/plugin_executor.rs`    | ~100 | 插件节点执行器（通过 PluginHost 转发到 JS） |
| `src-tauri/src/workflow/registry.rs`                  | ~120 | WorkflowNodeTypeRegistry 节点类型注册表 |
| `src-tauri/src/workflow/template.rs`                  | ~150 | TemplateEngine 参数传递引擎    |
| `src-tauri/src/commands/workflow.rs`                  | ~350 | 工作流 CRUD + 执行控制命令        |
| `src-tauri/src/db/init.rs`                            | +~80 | 新增 v8 迁移                 |

**Rust 新增合计：~1,900 行**

### 11.2 前端新增

| 文件                                                 | 预估行数 | 职责           |
| -------------------------------------------------- | ---- | ------------ |
| `src/pages/WorkflowPage.tsx`                       | ~150 | 工作流列表页       |
| `src/pages/WorkflowEditorPage.tsx`                 | ~100 | 工作流编辑器页      |
| `src/pages/ExecutionDetailPage.tsx`                | ~80  | 执行详情页        |
| `src/components/workflow/DAGEditor.tsx`            | ~300 | DAG 编辑器主组件   |
| `src/components/workflow/NodePalette.tsx`          | ~100 | 节点类型面板       |
| `src/components/workflow/NodeConfigPanel.tsx`      | ~350 | 节点配置面板       |
| `src/components/workflow/WorkflowToolbar.tsx`      | ~80  | 编辑器工具栏       |
| `src/components/workflow/WorkflowList.tsx`         | ~80  | 工作流列表组件      |
| `src/components/workflow/WorkflowCard.tsx`         | ~60  | 工作流卡片        |
| `src/components/workflow/ExecutionMonitor.tsx`     | ~300 | 执行监控面板       |
| `src/components/workflow/ExecutionTimeline.tsx`    | ~100 | 执行时间线        |
| `src/components/workflow/NodeStatusBadge.tsx`      | ~40  | 节点状态徽章       |
| `src/components/workflow/VariableSelector.tsx`     | ~120 | 变量选择器        |
| `src/components/workflow/TriggerConfigPanel.tsx`   | ~80  | 触发器配置面板      |
| `src/components/workflow/nodes/TriggerNode.tsx`    | ~60  | 触发器节点组件      |
| `src/components/workflow/nodes/AgentTaskNode.tsx`  | ~80  | Agent 任务节点组件 |
| `src/components/workflow/nodes/ApiCallNode.tsx`    | ~60  | API 调用节点组件   |
| `src/components/workflow/nodes/ConditionNode.tsx`  | ~60  | 条件分支节点组件     |
| `src/components/workflow/nodes/TransformNode.tsx`  | ~60  | 代码转换节点组件     |
| `src/components/workflow/nodes/AggregatorNode.tsx` | ~60  | 聚合节点组件       |
| `src/components/workflow/nodes/HumanInputNode.tsx`  | ~60  | 人工介入节点组件     |
| `src/components/workflow/nodes/index.ts`           | ~20  | 节点类型注册       |
| `src/components/workflow/HumanInputPanel.tsx`      | ~120 | 人工介入输入面板（文本/选择/确认/文件） |
| `src/utils/WorkflowNodeTypeRegistry.ts`            | ~80  | 前端节点类型注册表封装（缓存 + getNodeComponents） |
| `src/stores/workflowStore.ts`                      | ~250 | 工作流状态管理      |
| `src/hooks/useWorkflowExecution.ts`                | ~100 | 执行状态 Hook    |
| `src/types/workflow.ts`                            | ~120 | 工作流类型定义      |
| `src/styles/workflow.css`                          | ~200 | 工作流组件样式      |

**前端新增合计：~3,210 行**

### 11.3 文档

| 文件                                   | 说明      |
| ------------------------------------ | ------- |
| `docs/PilotDesk-工作流管理系统设计方案-v1.0.md` | **本文档** |

### 11.4 依赖变更

**Cargo.toml 新增**：

```toml
boa_engine = "0.19"      # 嵌入式 JavaScript 引擎（Transform 节点）
regex = "1"              # 模板变量正则解析
```

**package.json 新增**：

```json
"@xyflow/react": "^12.5.0"
```

---

> PilotDesk 工作流管理系统设计方案 v1.0 | 2026-06-18

---

## 版本里程碑

| 版本 | 日期 | 变更说明 | Git Commit |
|------|------|---------|------------|
| v1.0 | 2026-06-18 | 工作流管理系统完整设计方案 | `3297403 (32974036b1533427f24820582d3d9eed984412f0)` |
