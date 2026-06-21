/**
 * Workflow — 工作流类型定义
 *
 * 工作流编排系统类型体系。
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

// ── 节点类型 ──

/** 工作流节点类型 */
export type WorkflowNodeType =
  | 'trigger:cron'       // 定时触发
  | 'trigger:event'      // 事件触发
  | 'trigger:manual'     // 手动触发
  | 'plugin:command'     // 插件命令
  | 'condition'          // 条件判断
  | 'parallel'           // 并行执行
  | 'delay'              // 延迟等待
  | 'approval'           // 人工审批
  | 'subflow';           // 子工作流

/** 工作流节点定义 */
export interface WorkflowNode {
  id: string;
  type: WorkflowNodeType;
  label: string;
  /** 插件 ID（仅 plugin:command 类型） */
  pluginId?: string;
  /** 命令 ID（仅 plugin:command 类型） */
  commandId?: string;
  /** 执行参数 */
  params?: Record<string, any>;
  /** 条件表达式（仅 condition 类型） */
  condition?: string;
  /** cron 表达式（仅 trigger:cron 类型） */
  cron?: string;
  /** 事件名称（仅 trigger:event 类型） */
  eventName?: string;
  /** 延迟时间（毫秒，仅 delay 类型） */
  delayMs?: number;
  /** 超时时间（毫秒） */
  timeoutMs?: number;
  /** 重试次数 */
  retryCount?: number;
  /** 重试间隔（毫秒） */
  retryDelayMs?: number;
  /** 输入映射（从上下文或上游输出提取） */
  inputMapping?: Record<string, string>;
  /** 输出映射（将结果写入上下文） */
  outputMapping?: Record<string, string>;
  /** 位置信息（UI 画布） */
  position?: { x: number; y: number };
}

/** 节点连接（边） */
export interface WorkflowEdge {
  id: string;
  source: string;
  target: string;
  /** 条件分支标签（仅 condition 类型有多个分支时） */
  label?: string;
  /** 条件表达式（仅 condition 的分支） */
  condition?: string;
}

// ── 工作流定义 ──

/** 工作流定义 */
export interface WorkflowDefinition {
  id: string;
  name: string;
  version: string;
  description: string;
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  /** 输入 schema */
  inputSchema?: Record<string, { type: string; description?: string; default?: any }>;
  /** 输出 schema */
  outputSchema?: Record<string, { type: string; description?: string }>;
  /** 最大执行深度（防止循环） */
  maxDepth?: number;
  /** 创建时间 */
  createdAt: string;
  /** 更新时间 */
  updatedAt: string;
  /** 是否启用 */
  enabled: boolean;
}

// ── 工作流实例 ──

/** 工作流实例状态 */
export type WorkflowInstanceStatus =
  | 'pending'       // 待触发
  | 'running'       // 运行中
  | 'paused'        // 已暂停
  | 'success'       // 执行成功
  | 'failed'        // 执行失败
  | 'cancelled'     // 已取消
  | 'timeout';      // 超时

/** 步骤执行状态 */
export type StepStatus =
  | 'pending'
  | 'running'
  | 'success'
  | 'failed'
  | 'skipped'
  | 'retrying';

/** 步骤执行记录 */
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

/** 工作流实例 */
export interface WorkflowInstance {
  id: string;
  definitionId: string;
  definitionName: string;
  status: WorkflowInstanceStatus;
  /** 上下文数据 */
  context: Record<string, any>;
  /** 步骤执行记录 */
  steps: Record<string, StepExecution>;
  /** 当前执行的节点 ID */
  currentNodeId?: string;
  /** 触发方式 */
  trigger: 'manual' | 'cron' | 'event';
  /** 触发详情 */
  triggerDetail?: string;
  /** 开始时间 */
  startedAt?: string;
  /** 完成时间 */
  completedAt?: string;
  /** 预计剩余时间（毫秒） */
  estimatedRemaining?: number;
  /** 错误信息 */
  error?: string;
  /** 创建时间 */
  createdAt: string;
}

// ── 工作流事件 ──

/** 工作流事件类型 */
export type WorkflowEventType =
  | 'instance:created'
  | 'instance:started'
  | 'instance:paused'
  | 'instance:resumed'
  | 'instance:completed'
  | 'instance:failed'
  | 'instance:cancelled'
  | 'step:started'
  | 'step:completed'
  | 'step:failed'
  | 'step:retrying';

/** 工作流事件 */
export interface WorkflowEvent {
  type: WorkflowEventType;
  instanceId: string;
  nodeId?: string;
  data?: any;
  timestamp: string;
}

// ── 工作流模板 ──

/** 预定义工作流模板 */
export interface WorkflowTemplate {
  id: string;
  name: string;
  description: string;
  category: string;
  definition: Omit<WorkflowDefinition, 'id' | 'createdAt' | 'updatedAt' | 'enabled'>;
  tags?: string[];
}
