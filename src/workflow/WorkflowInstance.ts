/**
 * WorkflowInstance — 工作流实例状态管理
 *
 * 管理工作流实例的生命周期和状态转换。
 */

import { globalEventBus } from '../plugin/GlobalEventBus';
import type {
  WorkflowInstance,
  WorkflowInstanceStatus,
  StepExecution,
  StepStatus,
  WorkflowEventType,
} from '../types/workflow';

// ── 状态机 ──

const VALID_TRANSITIONS: Record<WorkflowInstanceStatus, WorkflowInstanceStatus[]> = {
  'pending': ['running', 'cancelled'],
  'running': ['paused', 'success', 'failed', 'cancelled', 'timeout'],
  'paused': ['running', 'cancelled'],
  'success': [],
  'failed': ['running'], // retry
  'cancelled': [],
  'timeout': ['running'], // retry
};

export function canTransition(from: WorkflowInstanceStatus, to: WorkflowInstanceStatus): boolean {
  return VALID_TRANSITIONS[from]?.includes(to) ?? false;
}

// ── 实例创建 ──

export function createInstance(
  definitionId: string,
  definitionName: string,
  trigger: WorkflowInstance['trigger'] = 'manual',
  triggerDetail?: string,
): WorkflowInstance {
  return {
    id: 'wfi_' + Date.now().toString(36) + '_' + Math.random().toString(36).substring(2, 8),
    definitionId,
    definitionName,
    status: 'pending',
    context: {},
    steps: {},
    trigger,
    triggerDetail,
    createdAt: new Date().toISOString(),
  };
}

// ── 步骤管理 ──

export function createStepExecution(nodeId: string): StepExecution {
  return {
    nodeId,
    status: 'pending',
    retryCount: 0,
  };
}

export function updateStepStatus(
  instance: WorkflowInstance,
  nodeId: string,
  status: StepStatus,
  extra?: Partial<StepExecution>,
): WorkflowInstance {
  const step = instance.steps?.[nodeId] || createStepExecution(nodeId);
  const now = new Date().toISOString();

  if (status === 'running' && !step.startedAt) step.startedAt = now;
  if (['success', 'failed', 'skipped'].includes(status)) step.completedAt = now;
  if (status === 'success' || status === 'failed') {
    step.duration = step.startedAt
      ? Date.now() - new Date(step.startedAt).getTime()
      : 0;
  }

  return {
    ...instance,
    steps: {
      ...instance.steps,
      [nodeId]: { ...step, ...extra, status },
    },
  };
}

// ── 事件发射 ──

export type WorkflowEventHandler = (event: { type: WorkflowEventType; instanceId: string; nodeId?: string; data?: any; timestamp: string }) => void;

let globalHandler: WorkflowEventHandler | null = null;

export function setWorkflowEventHandler(handler: WorkflowEventHandler | null): void {
  globalHandler = handler;
}

export function emitWorkflowEvent(
  type: WorkflowEventType,
  instanceId: string,
  nodeId?: string,
  data?: any,
): void {
  const event = { type, instanceId, nodeId, data, timestamp: new Date().toISOString() };

  // 通知内部 handler
  if (globalHandler) {
    globalHandler(event);
  }

  // 通过 GlobalEventBus 广播到插件
  // 命名空间: workflow:{type}
  globalEventBus.emit(`workflow:${type}`, {
    instanceId,
    nodeId,
    data,
    timestamp: event.timestamp,
  });
}
