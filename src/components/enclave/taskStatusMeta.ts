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
  // 进行中不再直接用 --accent：主题色是用户可改的，若改成绿/琥珀就会与完成/待确认撞色
  in_progress: 'var(--status-running)',
  submitted: 'var(--status-warning, #d08700)',
  done: 'var(--status-success)',
  // 归档态此前与 pending 同为 --text-tertiary，地图上完全无法区分；改用主题的归档色
  archived: 'var(--status-archived)',
  cancelled: 'var(--status-danger)',
};
