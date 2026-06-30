# PilotDesk 工作流 — Structured Canvas 架构设计 v1.3

> 基于 Stage-Gate Swimlane 与自由画布的融合方案，兼顾结构秩序与连线直观性。

---

## 1. 设计哲学

### 1.1 核心原则

| 原则 | 说明 |
|------|------|
| **结构提供秩序** | 阶段（Stage）作为时间轴，从左到右推进，让复杂工作流有宏观骨架 |
| **连线提供直观** | 保留自由连线表达数据流向和控制条件，不因结构牺牲可读性 |
| **智能自适应** | 画布自动根据连线关系调整节点归属，减少用户手动操作 |
| **渐进式复杂度** | 简单工作流不显冗余，复杂工作流不乱不散 |

### 1.2 三层架构

```
┌──────────────────────────────────────────────────────────────────┐
│  Layer 1: Stage Bar（阶段栏）                                      │
│  工作流的宏观骨架，固定在画布顶部，可折叠/展开/拖拽排序               │
│  每个阶段代表一个逻辑处理环节，从左到右推进                          │
├──────────────────────────────────────────────────────────────────┤
│  Layer 2: Canvas Zone（画布区）                                    │
│  每个阶段下方是自由画布区域，节点可自由放置                          │
│  连线自由绘制，支持条件标签，智能吸附到网格                          │
├──────────────────────────────────────────────────────────────────┤
│  Layer 3: Gate（门控区）                                           │
│  阶段底部是 Gate，控制阶段出口条件与数据合并策略                     │
│  所有上游节点完成 → 满足门控条件 → 进入下一阶段                     │
└──────────────────────────────────────────────────────────────────┘
```

---

## 2. 节点体系

### 2.1 实体节点（6 种）

| 类型 | 视觉标识 | 说明 |
|------|---------|------|
| `agent` | 圆形 + 机器人图标 | AI Agent 任务，调用 Claude/Hermes/Codex 执行 |
| `api` | 六边形 + 链接图标 | API 调用，支持 GET/POST/PUT/DELETE |
| `transform` | 菱形 + 闪电图标 | 代码/数据转换，执行 JavaScript 脚本 |
| `interact` | 圆角矩形 + 用户图标 | 人工交互，合并人工输入与审批功能 |
| `plugin` | 八边形 + 拼图图标 | 插件命令，调用已安装插件的命令 |
| `subflow` | 嵌套方框 + 箭头图标 | 子工作流，引用另一个工作流定义，支持嵌套和复用 |
| `start` | 圆角矩形 + 播放图标 | **开始节点**，每个工作流必须有一个，不可删除，标记流程起点 |
| `end` | 圆角矩形 + 停止图标 | **结束节点**，每个工作流必须有一个，不可删除，标记流程终点 |

### 2.2 结构元素（2 种）

| 元素 | 视觉 | 说明 |
|------|------|------|
| `trigger` | 阶段栏左侧入口标记 | 工作流的起始属性，配置 cron/event/manual，**不是节点** |
| `gate` | 阶段底部的横条 | 门控条件 + 合并策略，**不是节点**，是阶段的出口边界 |
| `stage_edge` | 阶段间贝塞尔曲线连线 | 阶段间的拓扑连线（`WorkflowEdge`），`source`/`target` 指向 `stage.id`，替代隐式 order 排序，支持 DAG 拓扑 |

### 2.3 节点属性

每个实体节点均包含以下控制属性：

```typescript
interface Node {
  id: string;
  type: 'agent' | 'api' | 'transform' | 'interact' | 'plugin' | 'subflow' | 'start' | 'end';
  label: string;
  config: Record<string, any>;        // 类型特定的配置
  
  // 控制属性
  delay_ms?: number;                  // 执行前延迟
  retry_count?: number;               // 失败重试次数
  retry_delay_ms?: number;            // 重试间隔
  timeout_ms?: number;                // 超时时间
  
  // 输入输出规格
  input_schema?: JSONSchema;          // 输入参数定义
  output_schema?: JSONSchema;         // 输出参数定义
  input_mapping?: Record<string, string>;  // key=用户自定义参数名, value=引用路径（自动拼接工作流ID）
  output_mapping?: Record<string, string>; // key=用户自定义参数名, value=引用路径（自动拼接工作流ID）
  
  // 画布位置
  position: { x: number; y: number };
}
```

---

## 3. 控制逻辑体系

### 3.1 控制语义的承载方式

| 控制语义 | 承载位置 | 面板交互 |
|---------|---------|---------|
| **条件分支** | 边上的 `condition` + `label` | 从节点拖出连线，自动弹出条件编辑框 |
| **多路并行** | 同一阶段内的多条出线 | 一个节点连出多条线到不同节点，自动并行执行 |
| **聚合/合并** | Gate 的合并策略 | 阶段底部 Gate 配置 merge/concat/pick_first |
| **延迟** | 节点属性 `delay_ms` | 节点属性面板配置 |
| **循环/重试** | 节点属性 `retry_count` | 节点属性面板配置 |
| **阶段间同步** | Gate 的门控策略 | 配置 all/any/count/threshold |

### 3.2 条件分支

条件逻辑由**边**承载，而非独立节点：

```
                   条件: "score > 0.8"
          ┌──────────────────────────────┐
          │                              ▼
   ┌──────────┐                  ┌──────────┐
   │ Agent    │                  │ Agent B  │
   │ 分类     │                  │ 深度处理  │
   └──────────┘                  └──────────┘
          │
          │  条件: else
          ▼
   ┌──────────┐
   │ API     │
   │ 快速存档  │
   └──────────┘
```

```typescript
interface Edge {
  id: string;
  source: string;          // 源节点 ID
  target: string;          // 目标节点 ID
  label?: string;          // 条件标签（如 "score > 0.8"）
  condition?: string;      // 条件表达式
}
```

### 3.3 多路并行

同一节点的多条出线 = 并行执行，无需额外配置：

```
   ┌──────────┐
   │ Agent    │──────────────→┌──────────┐
   │ 数据获取   │              │ Agent A  │
   └──────────┘              │ 分析     │
          │                  └──────────┘
          │                  ┌──────────┐
          └─────────────────→│ Agent B  │
                             │ 归档     │
                             └──────────┘
```

### 3.4 阶段门控（Gate）

```
┌──────────────────────────────────────────────────────┐
│ Gate                                                    │
│ ┌──────────┐  ┌──────────┐  ┌──────────┐              │
│ │ Agent A  │  │ Agent B  │  │ API C    │  全部完成     │
│ │ 已完成    │  │ 已完成    │  │ 已完成    │  → 进入下阶段 │
│ └──────────┘  └──────────┘  └──────────┘              │
│                                                        │
│ 门控策略: [全部完成 ▼]  合并策略: [merge ▼]  3/3 就绪   │
└──────────────────────────────────────────────────────┘
```

就绪统计：
- 门控栏显示的就绪数（如 `3/3`）动态排除**不可达节点**
- 不可达节点 = 不在任何 start→end 拓扑路径上的节点（含跨阶段连通性）
- 不可达节点在画布中以**红色虚线边框**标记

门控策略：

| 策略 | 说明 |
|------|------|
| `all` | 全部完成 — 所有上游节点完成才进入下一阶段（默认） |
| `any` | 任一完成 — 任一上游节点完成即进入下一阶段 |
| `count` | 指定数量完成 — 指定数量的上游节点完成即进入（需配置完成节点数） |
| `threshold` | 按条件判断 — 根据输出数据的数值阈值判断（需配置阈值） |

### 3.5 门控编辑弹窗交互

门控编辑弹窗支持完整的聚合/合并策略配置：

**聚合策略条件显示：**
- 选择「全部完成」或「任一完成」时，不显示额外输入
- 选择「指定数量完成」时，显示「完成节点数」输入框
- 选择「按条件判断」时，显示「阈值」输入框

**自定义合并脚本（选择器模式）：**
```
过滤: [保留全部 ▼]  合并为: [合并成一个对象 ▼]  取值: [保留原始值 ▼]
```
- 过滤：保留全部 / 只保留成功
- 合并为：合并成一个对象 / 合并成一个数组 / 展开成一维数组
- 取值：保留原始值 / 取最大值 / 取最小值 / 取平均值 / 计算总和
- APP 根据三个维度自动拼接生成脚本代码

**自定义合并脚本（编辑器模式）：**
```
[textarea... 在此编写自定义脚本]
参数说明
  results — 上游节点输出数组，需编写完整函数: (results) => { /* 你的代码 */ }
  每个元素: { data, success, nodeId, nodeName }
常用数组方法
  results.map(r => r.data)          — 遍历转换，提取所有数据
  results.filter(r => r.success)    — 过滤，只保留符合条件的元素
  results.find(r => r.success)      — 查找，返回第一个匹配的元素
  results.reduce(...)               — 归并，累积合并为单个值/对象
  results.flatMap(r => r.data)      — 遍历展平，将嵌套数组展开一层
  results.sort((a, b) => b.data - a.data) — 排序，按数值降序排列
  results.slice(0, 3)               — 截取，取前 N 个元素
```

**模式持久化：** 用户选择的选择器/编辑器模式通过 `customMode` 字段持久化存储，重新打开弹窗时自动恢复上次使用的模式。

**门控栏显示：**
```
策略: 指定数量完成 (完成节点数: 3)    合并: 合并为对象
策略: 按条件判断 (阈值: 0.8)         合并: 取第一个结果
策略: 全部完成                       合并: 自定义处理 (脚本)
```

合并策略：

| 策略 | 说明 |
|------|------|
| `merge` | 合并为对象 — 合并所有上游输出为单个对象（默认） |
| `concat` | 合并为数组 — 将所有上游输出拼接为数组 |
| `pick_first` | 取第一个结果 — 取第一个完成的输出 |
| `pick_last` | 取最后一个结果 — 取最后一个完成的输出 |
| `custom` | 自定义处理 — 自定义合并脚本（支持选择器/编辑器两种模式） |

---

## 4. 智能连线与自动归入阶段

### 4.1 核心规则

当用户从节点 A 拖出连线连接到节点 B 时，系统自动判断并调整阶段归属：

```
规则 1: 同阶段连线
  A（Stage 2）──→ B（Stage 2）
  → 不做调整，两者已在同一阶段

规则 2: 向前连线（A 在 B 的右侧阶段）
  A（Stage 2）──→ B（Stage 1）
  → A 自动移动到 Stage 1，放置在 B 附近

规则 3: 向后连线（A 在 B 的左侧阶段）
  A（Stage 1）──→ B（Stage 2）
  → B 自动移动到 Stage 1，放置在 A 附近

规则 4: 跨多阶段连线
  A（Stage 3）──→ B（Stage 1）
  → A 移动到 Stage 1，同时 Stage 2 和 Stage 3 之间的 Gate 自动更新
```

### 4.2 交互细节

```
用户操作前（Stage 2 的节点连线到 Stage 1）:

  [Stage 1: 采集]          [Stage 2: 处理]
  ┌──────────┐            ┌──────────┐
  │ Trigger  │            │ Agent A  │──┐
  └──────────┘            │ 分析     │  │
                          └──────────┘  │
                                        │  ← 用户拖出连线到 Stage 1
                                        ▼
                                  ┌──────────┐
                                  │ Agent B  │
                                  │ 存档     │
                                  └──────────┘

用户操作后（Agent A 自动移动到 Stage 1）:

  [Stage 1: 采集+处理]     [Stage 2: 输出]
  ┌──────────┐  ┌──────────┐
  │ Trigger  │→│ Agent A  │──┐
  └──────────┘  │ 分析     │  │
                └──────────┘  │   ┌──────────┐
                              └──→│ Agent B  │
                                  │ 存档     │
                                  └──────────┘
```

### 4.3 触发时机

| 操作 | 行为 |
|------|------|
| 拖出连线并连接到目标节点 | 立即触发阶段归属调整 |
| 拖拽节点跨越阶段边界 | 节点自动吸附到目标阶段 |
| 批量粘贴节点 | 根据连线关系批量归入最左阶段 |
| 导入工作流 JSON | 自动计算最优阶段划分 |

---

## 5. 完整示例：每日舆情监控工作流

```
┌─────────────────────────────────────────────────────────────────────────┐
│  [Stage 1: 采集]        [Stage 2: 分析]           [Stage 3: 输出]      │
│                                                                         │
│   ┌──────────┐         ┌──────────┐                                    │
│   │ Trigger  │────────→│ Agent A  │──┐                                 │
│   │ cron/08:00│         │ 摘要生成   │  │                               │
│   └──────────┘         └──────────┘  │   ┌──────────┐                  │
│                                       ├──→│ Save     │                  │
│                            ┌────────┐ │   │ 数据库    │                  │
│                            │ API    │─┘   └──────────┘                  │
│                            │ 数据获取 │                                  │
│                            └───┬────┘                                   │
│                                │                                        │
│                                ▼                                        │
│                            ┌──────────┐                                 │
│                            │ Transform│                                 │
│                            │ 数据清洗   │                                 │
│                            └──────────┘                                 │
│                                                                         │
│   ┌──────────────────────────────────────────────────────────────┐     │
│   │ Gate: 全部完成 → 进入 Stage 3   合并策略: merge   3/3 就绪     │     │
│   └──────────────────────────────────────────────────────────────┘     │
│                                                                         │
│   ┌──────────┐         ┌──────────┐                                    │
│   │ Interact │────────→│ Agent C  │                                    │
│   │ 人工审核   │         │ 报告生成   │                                    │
│   │ 异常才触发  │         │          │                                    │
│   └──────────┘         └──────────┘                                    │
│                                                                         │
│   ┌──────────────────────────────────────────────────────────────┐     │
│   │ Gate: 全部完成 → 结束    合并策略: merge   2/2 就绪             │     │
│   └──────────────────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 6. 数据模型

> v1.3 更新：`WorkflowDefinition` 新增 `stageEdges` 字段；`Stage.edges`（节点级）与 `WorkflowDefinition.stageEdges`（阶段级）均为 `WorkflowEdge[]` 类型；新增 `start`/`end` 节点类型；`outputMapping` 语义从「字段名→语义类型」改为「用户参数名→引用路径」。

### 6.1 前端 TypeScript 接口（workflow.ts）

```typescript
/** 工作流定义（前端） */
interface WorkflowDefinition {
  id: string;
  name: string;
  description: string;
  stages: Stage[];                   // 阶段列表
  stageEdges: WorkflowEdge[];         // v1.3 新增：阶段间拓扑连线（source/target 指向 stage.id）
  // ...其他字段
}

/** 阶段 */
interface Stage {
  id: string;
  name: string;
  order: number;                     // 阶段序号（渲染排序，不决定执行顺序）
  nodes: WorkflowNode[];             // 阶段内节点
  edges: WorkflowEdge[];             // 阶段内节点连线（source/target 指向 node.id）
  gate: GateConfig;
}

/** 工作流边 — 同时用于节点级和阶段级连线 */
interface WorkflowEdge {
  id: string;
  source: string;                    // 节点级: node.id, 阶段级: stage.id
  target: string;                    // 节点级: node.id, 阶段级: stage.id
  condition?: string;
  label?: string;
}

/** 工作流节点（含 start/end 边界节点） */
interface WorkflowNode {
  id: string;
  type: 'agent' | 'api' | 'transform' | 'interact' | 'plugin' | 'subflow' | 'start' | 'end';
  label: string;
  config: Record<string, any>;
  position: { x: number; y: number };
  input_mapping?: Record<string, string>;   // key=用户参数名, value=引用路径
  output_mapping?: Record<string, string>;  // key=用户参数名, value=引用路径
  // 控制属性: delay_ms, retry_count, retry_delay_ms, timeout_ms
  // 规格属性: input_schema, output_schema
}
```

### 6.2 Rust 后端结构体

```rust
/// 工作流定义
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub trigger: TriggerConfig,      // cron / event / manual
    pub stages: Vec<Stage>,
    pub stage_edges: Vec<Edge>,     // v1.3 新增：阶段间拓扑连线
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
}

/// 阶段
pub struct Stage {
    pub id: String,
    pub name: String,
    pub order: usize,                // 渲染排序用（不决定执行顺序）
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,          // 阶段内节点连线
    pub gate: GateConfig,
}

/// 实体节点
pub struct Node {
    pub id: String,
    pub node_type: NodeType,         // agent | api | transform | interact | plugin | subflow | start | end
    pub label: String,
    pub config: Value,
    pub delay_ms: Option<u64>,
    pub retry_count: Option<u32>,
    pub retry_delay_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub input_mapping: Option<Value>,
    pub output_mapping: Option<Value>,
    pub position: Position,
}

/// 边（同时用于节点级和阶段级连线）
pub struct Edge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub label: Option<String>,       // 条件标签
    pub condition: Option<String>,   // 条件表达式
}

/// 阶段门控配置
pub struct GateConfig {
    pub strategy: GateStrategy,      // all | any | count | threshold
    pub merge_strategy: MergeStrategy, // merge | concat | pick_first | pick_last | custom
    pub threshold: Option<usize>,    // count 策略时为完成节点数，threshold 策略时为阈值
    pub custom_script: Option<String>, // 自定义合并脚本
    pub custom_mode: Option<String>, // 自定义脚本输入模式: "selector" | "editor"
}

pub enum GateStrategy {
    All,
    Any,
    Count(usize),
    Threshold(String),               // 阈值表达式
}

pub enum MergeStrategy {
    Merge,
    Concat,
    PickFirst,
    Custom(String),                  // 自定义合并脚本
}

/// 触发器配置
pub struct TriggerConfig {
    pub trigger_type: TriggerType,   // cron | event | manual
    pub cron: Option<String>,        // cron 表达式
    pub event_name: Option<String>,  // 事件名称
}

pub enum TriggerType {
    Cron,
    Event,
    Manual,
}
```

---

## 7. 引擎调度逻辑

### 7.1 两层调度

> v1.3 变更：阶段间不再是严格串行（隐式 order），而是基于显式 `stageEdges` 的 DAG 拓扑调度。

```
┌─────────────────────────────────────────────────────────────┐
│  外层调度：阶段 DAG（基于 stageEdges）                         │
│                                                             │
│  Stage 1 ──[Gate]──→ Stage 2                                │
│      └──────────[Gate]──→ Stage 3                           │
│  阶段间通过 stageEdges 声明拓扑关系，支持分支/合并              │
│  engine 按 stageEdges 的拓扑顺序调度阶段                       │
├─────────────────────────────────────────────────────────────┤
│  内层调度：阶段内 DAG                                         │
│                                                             │
│  阶段内节点构成 DAG，BFS 传递上游搜索后拓扑排序执行               │
│  无依赖关系的节点自动并行执行                                   │
│  边上的 condition 决定是否执行目标节点                          │
└─────────────────────────────────────────────────────────────┘
```

**阶段 DAG 视觉表现：**
- 阶段间连线从源阶段**门控栏右侧出口锚点**拖拽到目标阶段**标题栏左侧入口锚点**
- 连线为贝塞尔曲线（与节点连线风格一致），带方向箭头
- 拖拽时实时显示预览线，释放后闭环检测 + 重复检测


### 7.2 执行流程

> v1.3 变更：执行流程从「按 order 串行」改为「按 stageEdges 拓扑调度」。

```
1. 验证工作流：start/end 存在性 + 至少一条 start→end 拓扑路径 + 无不可达节点
2. 基于 stageEdges 构建阶段 DAG，拓扑排序确定执行顺序
3. 按阶段拓扑顺序调度：
   a. 对当前阶段内节点进行 BFS 传递上游搜索 + 拓扑排序
   b. 按拓扑顺序执行节点：
      - 检查入边条件是否满足
      - 满足则执行节点（含延迟/重试逻辑）
      - 不满足则跳过节点
   c. 所有节点执行完毕（或满足 Gate 策略）
   d. 执行 Gate 合并逻辑（gate_output 存储为 gate_output.{stageId}）
   e. 通过 stageEdges 找到下游阶段，继续执行
4. 所有可达阶段执行完毕，工作流完成
```

### 7.3 自动归入阶段的引擎支持

```rust
/// 根据连线关系自动调整节点阶段归属
fn auto_assign_stage(workflow: &mut Workflow) {
    // 1. 构建节点 → 阶段映射
    let mut node_to_stage: HashMap<String, usize> = HashMap::new();
    for stage in &workflow.stages {
        for node in &stage.nodes {
            node_to_stage.insert(node.id.clone(), stage.order);
        }
    }
    
    // 2. 遍历所有边，计算每个节点的"最左依赖阶段"
    let mut node_leftmost_stage: HashMap<String, usize> = HashMap::new();
    for stage in &workflow.stages {
        for edge in &stage.edges {
            let source_stage = node_to_stage[&edge.source];
            let target_stage = node_to_stage[&edge.target];
            let leftmost = source_stage.min(target_stage);
            node_leftmost_stage.insert(edge.source.clone(), leftmost);
            node_leftmost_stage.insert(edge.target.clone(), leftmost);
        }
    }
    
    // 3. 将节点移动到最左阶段
    for (node_id, target_stage) in node_leftmost_stage {
        let current_stage = node_to_stage[&node_id];
        if current_stage != target_stage {
            move_node_to_stage(workflow, &node_id, target_stage);
        }
    }
    
    // 4. 清理空阶段，合并相邻阶段
    workflow.stages.retain(|s| !s.nodes.is_empty());
}
```

---

## 8. 面板交互规范

### 8.1 节点拖拽

| 操作 | 行为 |
|------|------|
| 从工具栏拖出节点 | 拖入画布区，自动吸附到最近的网格位置 |
| 在画布内拖拽节点 | 自由移动，智能吸附到网格和附近节点对齐 |
| 拖拽节点跨越阶段边界 | 节点自动归入目标阶段，连线关系保持不变 |
| 多选拖拽 | 批量移动，保持相对位置 |

### 8.2 连线操作

| 操作 | 行为 |
|------|------|
| 从节点右侧锚点拖出 | 创建出线，自动弹出条件编辑框（可选） |
| 拖到目标节点左侧锚点 | 完成连线，触发自动归入阶段逻辑 |
| 双击连线 | 编辑条件表达式和标签 |
| 右键点击连线 | 删除连线菜单 |

**阶段连线操作（v1.3 新增）：**

| 操作 | 行为 |
|------|------|
| 从门控栏右侧出口锚点拖出 | 创建阶段连线预览线 |
| 拖到目标阶段标题栏左侧入口锚点释放 | 完成连线（闭环检测 + 重复检测） |
| 鼠标释放到非锚点区域 | 取消连线 |
| Esc 键 | 取消连线 |

### 8.3 阶段操作

| 操作 | 行为 |
|------|------|
| 点击 "+" 添加阶段 | 在右侧新增阶段，自动创建 Gate |
| 拖拽阶段标题 | 调整阶段顺序，节点和连线跟随移动 |
| 点击阶段标题 | 展开/折叠阶段内容 |
| 右键阶段标题 | 重命名/删除/清空阶段 |
| 标题栏左侧圆点 | 阶段入口锚点（接收阶段连线，拖拽连线时高亮） |
| 门控栏右侧圆点 | 阶段出口锚点（发起阶段连线拖拽） |

### 8.4 节点属性编辑

| 操作 | 行为 |
|------|------|
| 双击节点 | 打开属性面板（右侧滑出） |
| 属性面板 | 包含：基本信息、控制属性、输入输出规格、映射配置 |

---

## 9. 与现有代码的映射

> v1.3 更新：阶段拓扑连线 `stageEdges` 替代隐式 order 排序；节点类型新增 start/end。

| 当前架构 | Structured Canvas |
|---------|-----------------|
| `WorkflowNode` | `Stage.Node`（增加阶段嵌套） |
| `WorkflowEdge` | `Stage.Edge`（节点级连线）+ `WorkflowDefinition.stageEdges`（阶段级连线） |
| `WorkflowNodeType` 枚举 9 种 | 8 种实体（含 start/end 边界节点） + 3 种结构元素 |
| `engine.rs` DAG 调度 | 两层调度：阶段 DAG（stageEdges）+ 阶段内 DAG |
| `WorkflowDefinition.ts` | 含 `getReachableNodes`（BFS 从 start 可达性）、`getStageUpstreamMap`（阶段传递上游）、`validateWorkflowForExecution`（执行前验证） |
| `sanitizeMappingReferences` | 三层校验：节点拓扑前序 + gate_output 阶段拓扑前序 + session_id agent 类型一致性 |
| `condition_executor` | 边上的 condition（不再需要独立执行器） |
| `aggregator_executor` | Gate 的合并策略（不再需要独立执行器） |
| `scheduler.rs` | 保持不变（触发层） |
| `template.rs` | 三段式引用路径 `{{key.nodeId.stageId}}` + 旧格式兼容 |

---

## 10. 路线图

| 阶段 | 内容 | 预估工时 | 状态 |
|------|------|---------|------|
| P0 | 数据模型重构（Stage/Node/Edge/Gate） | 4-6h | 已完成 (v67迁移) |
| P0 | 引擎两层调度改造 | 6-8h | 已完成 (engine.rs) |
| P0 | 自动归入阶段逻辑 | 3-4h | 已完成 (前端WorkflowDefinition) |
| P0 | Subflow 并发控制（Semaphore 覆盖） | 0.5h | 已完成 (架构改进) |
| P0 | v68 迁移废弃 steps 字段 | 0.5h | 已完成 |
| P0 | InteractManager TTL 过期清理 | 1h | 已完成（基础结构） |
| P1 | 前端阶段栏组件 | 8-12h | 已完成 (WorkflowEditor + StageBar) |
| P1 | 前端智能连线 + 条件标签 | 6-8h | 已完成 (WorkflowEditor renderEdge) |
| P1 | Gate 组件 + 门控配置面板 | 4-6h | 已完成 (StageBar + WorkflowEditor Gate 区域) |
| P2 | 节点属性面板（含控制属性） | 6-8h | 待开发 |
| P2 | 阶段折叠/展开动画 | 3-4h | 待开发 |
| P2 | 工作流导入/导出（自动阶段划分） | 3-4h | 待开发 |
| P3 | subflow 嵌套编辑 | 6-8h | 待开发 |
| P3 | 工作流模板市场 | 4-6h | 待开发 |

### 10.1 已完成的架构改进（2026-06-24）

| 改进项 | 说明 | 涉及文件 |
|--------|------|---------|
| **v68 迁移废弃 steps 字段** | 引擎已不再写入 `workflow_instances.steps`（数据源改为 `node_executions` 表）。v68 迁移将现有 rows 的 steps 设为 NULL，struct 字段改为 `Option<Value>` 并标记 deprecated | `init.rs`, `mod.rs`, `commands/workflow.rs`, `scheduler.rs` |
| **Subflow 纳入 Semaphore 并发控制** | 将 `semaphore.acquire_owned()` 从普通节点分支移至 Subflow 分支之前，确保 Subflow 和普通节点共享并发限制。`_permit` 在 Subflow 执行完成后 drop 释放许可 | `engine.rs` |
| **InteractManager TTL 过期清理** | 新增 `PendingEntry` 结构体携带 `registered_at` 和 `ttl_secs`，`cleanup_expired()` 方法清理过期 pending 条目。供调度器定期调用 | `interact_executor.rs` |

### 10.2 前后端引擎职责边界

> 经过 7 轮深度审查后确认：Rust `engine.rs` 负责后端持久化执行，TypeScript `WorkflowEngine.ts` 负责前端内存态模拟（编辑器预览）。两者逻辑存在有意重复，各自职责已在代码注释中明确标注。

### 10.3 已完成的架构改进（2026-06-30）

| 改进项 | 说明 | 涉及文件 |
|--------|------|---------|
| **引用路径格式统一** | outputMapping 语义改为「key=用户参数名, value=引用路径」，支持三段式 `{{key.nodeId.stageId}}`；Rust template.rs 同步适配 | `WorkflowNodeConfig.tsx`, `WorkflowDefinition.ts`, `template.rs`, `engine.rs` |
| **sanitizeMapping 三层校验** | 1) 节点拓扑前序（BFS 传递上游）；2) gate_output 阶段拓扑前序（getStageUpstreamMap）；3) session_id agent 类型一致性 | `WorkflowDefinition.ts`, `WorkflowNodeConfig.tsx` |
| **显式阶段拓扑连线 stageEdges** | 新增 `WorkflowDefinition.stageEdges: WorkflowEdge[]`，source/target 指向 stage.id；替代隐式 order 排序，为阶段 N-N 扩展预留基础 | `workflow.ts`, `WorkflowDefinition.ts` |
| **工具函数** | `getStageUpstreamMap`（阶段传递上游 BFS）、`getReachableNodes`（从 start 可达性）、`validateWorkflowForExecution`（start/end + 路径 + 不可达） | `WorkflowDefinition.ts` |
| **start/end 节点约束** | 每个工作流必须有且仅有一个 start 和 end 节点，不可删除；至少一条拓扑路径贯通 start→end；不可达节点标记红色虚线 | `WorkflowDefinition.ts`, `WorkflowEditor.tsx`, `WorkflowNodeItem.tsx` |
| **阶段入口/出口锚点** | 标题栏左侧入口锚点（接收连线 onMouseUp）+ 门控栏右侧出口锚点（发起连线 onMouseDown）；拖拽预览线 + 闭环检测 | `WorkflowEditor.tsx` |
| **StageBar 就绪统计** | 门控栏就绪数动态排除不可达节点（reachableCount/nodeCount） | `StageBar.tsx` |
| **remapImportedWorkflowIds** | 返回 `{ stages, stageEdges }`，支持导入工作流时重映射阶段 ID | `WorkflowDefinition.ts` |

**关键设计决策记录：**

| 决策 | 选择 | 备选方案 |
|------|------|---------|
| 阶段拓扑数据结构 | 复用 `WorkflowEdge`（source/target 指向 stage.id） | 新建 `StageEdge` 类型 |
| 阶段连线创建方式 | 拖拽门控栏出口锚点→目标标题栏入口锚点 | 右键菜单选择目标 |
| 不可达节点标记 | 红色虚线边框（2px dashed #f85149） | 灰色半透明 + 删除线 |
| stageConnecting 取消策略 | `setTimeout(0)` 延迟取消（让入口锚点 onMouseUp 先处理） | `closest('[data-stage-entrance]')` 检测 |

---

> PilotDesk 工作流 — Structured Canvas 架构设计 v1.1 | 2026-06-24
