/**
 * Workflow — 工作流类型定义（Structured Canvas 架构）
 *
 * 架构设计: docs/PilotDesk-工作流-StructuredCanvas架构设计-v1.0.md
 */

// ── 实体节点类型（6种）──

/** 实体节点类型 */
export type WorkflowNodeType =
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
  type: WorkflowNodeType;
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

  // 节点特定
  cron?: string;
  eventName?: string;
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

export type GateStrategy = 'all' | 'any' | 'count' | 'threshold';
export type MergeStrategy = 'merge' | 'concat' | 'pick_first' | 'custom';

export interface GateConfig {
  strategy: GateStrategy;
  mergeStrategy: MergeStrategy;
  threshold?: number;
  customScript?: string;
}

// ── 阶段定义 ──

/** 阶段 — 工作流的基本组织单元 */
export interface Stage {
  id: string;
  name: string;
  order: number;
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  gate: GateConfig;
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
  inputSchema?: Record<string, { type: string; description?: string; default?: any }>;
  outputSchema?: Record<string, { type: string; description?: string }>;
  maxDepth?: number;
  createdAt: string;
  updatedAt: string;
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

export type StepStatus =
  | 'pending'
  | 'running'
  | 'success'
  | 'failed'
  | 'skipped'
  | 'retrying';

export interface StepExecution {
  nodeId: string;
  status: StepStatus;
  startedAt?: string;
  completedAt?: string;
  duration?: number;
  input?: any;
  output?: any;
  error?: string;
  retryCount: number;
}

export interface WorkflowInstance {
  id: string;
  definitionId: string;
  definitionName: string;
  status: WorkflowInstanceStatus;
  context: Record<string, any>;
  steps: Record<string, StepExecution>;
  currentNodeId?: string;
  trigger: string;
  triggerDetail?: string;
  startedAt?: string;
  completedAt?: string;
  estimatedRemaining?: number;
  error?: string;
  createdAt: string;
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

// ── 兼容函数 ──

/** 将旧版 nodes+edges 转换为新版 stages */
export function legacyToStages(
  nodes: WorkflowNode[],
  edges: WorkflowEdge[]
): Stage[] {
  if (nodes.length === 0) return [];
  return [{
    id: crypto.randomUUID?.() || `${Date.now()}-default`,
    name: '默认阶段',
    order: 0,
    nodes,
    edges,
    gate: { strategy: 'all', mergeStrategy: 'merge' },
  }];
}
