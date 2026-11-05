/**
 * 飞地任务状态的展示口径（任务树、拓扑地图、详情共用）。
 *
 * 单独抽出是为了避免「地图组件 ← 工作台组件」互相 import 形成循环依赖。
 */
import type { TaskStatus } from '../../types/enclave';

export const STATUS_LABEL: Record<TaskStatus, string> = {
  pending: '待办',
  in_progress: '进行中',
  submitted: '待确认',
  done: '已完成',
  archived: '已归档',
  cancelled: '已作废',
};

export const STATUS_COLOR: Record<TaskStatus, string> = {
  pending: 'var(--text-tertiary)',
  in_progress: 'var(--accent)',
  submitted: 'var(--status-warning, #d08700)',
  done: 'var(--status-success, #2f9e44)',
  archived: 'var(--text-tertiary)',
  cancelled: 'var(--status-danger)',
};
