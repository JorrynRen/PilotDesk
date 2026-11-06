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
  // 完成 / 归档：不能依赖 --status-success 的兜底色 —— 部分主题没有定义该变量，
  // 会回落到偏暗的 #2f9e44，在深色底上几乎与待办的灰同色。这里改为固定亮色，
  // 并让归档用更浅的绿，二者与 pending（灰）都能一眼区分。
  done: '#10B981',
  archived: '#34D399',
  cancelled: 'var(--status-danger)',
};
