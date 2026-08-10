/**

 * Workflow — 工作流类型定义（Structured Canvas 架构）

 *

 * 架构设计: docs/PilotDesk-工作流-StructuredCanvas架构设计-v1.0.md

 */



// ── 实体节点类型（6种）──



/** 节点类型（含边界节点） */

export type WorkflowNodeType =

  | 'start'          // 起始节点（系统自动创建，不可删除）

  | 'end'            // 结束节点（系统自动创建，不可删除）

  | 'agent'          // AI Agent 任务

  | 'api'            // API 调用

  | 'transform'      // 代码/数据转换

  | 'interact'       // 人工交互（输入 + 审批）

  | 'plugin'         // 插件命令

  | 'subflow';       // 子工作流



/** 触发器类型 */

export type TriggerType = 'cron' | 'event' | 'manual';



/** 触发器配置 */

export interface TriggerConfig {

  triggerType: TriggerType;

  cron?: string;

  eventName?: string;

}



/** 工作流节点定义 */

export interface WorkflowNode {

  id: string;

  type: string;

  label: string;

  pluginId?: string;

  commandId?: string;

  params?: Record<string, any>;



  // 控制属性

  delayMs?: number;

  timeoutMs?: number;

  retryCount?: number;

  retryDelayMs?: number;



  // 输入输出规格

  inputSchema?: Record<string, { type: string; description?: string; default?: any }>;

  outputSchema?: Record<string, { type: string; description?: string }>;

  inputMapping?: Record<string, string>;

  outputMapping?: Record<string, string>;



  // 画布位置

  position?: { x: number; y: number };



  /** 边界节点标记（Start/End 由系统自动创建，不可删除） */

  isBoundary?: boolean;



}



/** 节点连接（边）— 数据流 + 控制流 */

export interface WorkflowEdge {

  id: string;

  source: string;

  target: string;

  label?: string;       // 条件标签（如 "score > 0.8"）

  condition?: string;   // 条件表达式

}



// ── 门控配置 ──



export type GateStrategy = 'all' | 'count' | 'threshold';

export type MergeStrategy = 'merge' | 'concat' | 'pick_first' | 'pick_last' | 'custom';



export interface GateConfig {

  strategy: GateStrategy;

  mergeStrategy: MergeStrategy;

  threshold?: string;

  customScript?: string;

  /** 自定义脚本输入模式: 'selector' | 'editor' */

  customMode?: 'selector' | 'editor';

}



// ── 阶段定义 ──



/** 阶段 — 工作流的基本组织单元 */

export interface Stage {

  id: string;

  name: string;

  order: number;

  nodes: WorkflowNode[];

  edges: WorkflowEdge[];

  stageEdges?: WorkflowEdge[];

  gate: GateConfig;

  collapsed?: boolean;

  offsetX?: number;

  offsetY?: number;

}



// ── 工作流定义 ──



/** 工作流定义 */

export interface WorkflowDefinition {

  id: string;

  name: string;

  version: string;

  description: string;

  trigger: TriggerConfig;

  stages: Stage[];

  icon?: string;                 // emoji 图标

  inputSchema?: Record<string, { type: string; description?: string; default?: any }>;

  outputSchema?: Record<string, { type: string; description?: string }>;

  createdAt: number;

  updatedAt: number;

  enabled: boolean;

}



// ── 工作流实例 ──



export type WorkflowInstanceStatus =

  | 'pending'

  | 'running'

  | 'paused'

  | 'success'

  | 'failed'

  | 'cancelled'

  | 'timeout';



export interface WorkflowInstance {

  id: string;

  definitionId: string;

  definitionName: string;

  status: WorkflowInstanceStatus;

  context: Record<string, any>;

  trigger: string;

  triggerDetail?: string;

  startedAt?: number;

  completedAt?: number;

  completionRate: number;

  error?: string;

  createdAt: number;

}



// ── 统计 ──



export interface WorkflowStats {

  totalExecutions: number;

  successCount: number;

  failedCount: number;

  cancelledCount: number;

  successRate: number;

  avgDurationMs: number;

  maxDurationMs: number;

  minDurationMs: number;

  totalNodeExecutions: number;

  nodeFailedCount: number;

  last7DaysCount: number;

  last30DaysCount: number;

}



export interface ExecutionTimelinePoint {

  date: string;

  total: number;

  success: number;

  failed: number;

  avgDurationMs: number;

}



export interface NodeTypeStat {

  nodeType: string;

  count: number;

  failedCount: number;

  avgDurationMs: number;

}



// ── 工作流事件类型 ──



export type WorkflowEventType =

  | 'instance:created'

  | 'instance:started'

  | 'instance:paused'

  | 'instance:resumed'

  | 'instance:completed'

  | 'instance:failed'

  | 'instance:cancelled'

  | 'stage:started'

  | 'stage:completed'

  | 'step:started'

  | 'step:completed'

  | 'step:failed'

  | 'step:skipped'

  | 'step:retrying'

  | 'human_input'

  | 'error';





// ── 执行模式（预留：支持完整执行、单点执行、断点执行）──



export type ExecutionMode =

  | { type: 'full' }

  | { type: 'single_node'; nodeId: string }

  | { type: 'from_node'; nodeId: string };



// ── 工作流校验结果 ──



export interface ValidationCheck {

  /** 校验类型标识 */

  checkType: string;

  /** 严重级别: "error" | "warning" | "info" */

  severity: string;

  /** 可读消息 */

  message: string;

  /** 可选详情（如涉及的节点/阶段 ID） */

  details?: Record<string, any>;

}



export interface ValidationResult {

  /** 是否通过（无 error 级别检查项） */

  ok: boolean;

  /** 校验详情列表 */

  checks: ValidationCheck[];

}



// ── 统一执行进度事件 payload ──



export interface ExecutionProgressPayload {

  executionId: string;

  definitionId: string;

  /** 执行模式 */

  mode: ExecutionMode;

  /** 节点状态变更（可选） */

  node?: {

    id: string;

    status: string;

    output?: any;

    error?: string;

  };

  /** 阶段状态变更（可选） */

  stage?: {

    id: string;

    name?: string;

    status: string;

    reason?: string;

    error?: string;

  };

  /** 执行状态变更（可选） */

  execution?: {

    status: string;

    definitionName?: string;

    error?: string;

    message?: string;

  };

  /** 进度统计（可选） */

  progress?: {

    completed: number;

    total: number;

  };

}

/** 待处理的人工输入请求 */
export interface PendingHumanInput {
  execution_id: string;
  node_id: string;
  node_label: string;
  prompt: string;
  input_type: string;
  created_at: number;
}
