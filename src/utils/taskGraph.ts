import type { GroupChatTask, TaskStatus } from '../types/groupchat';

/** 任务展示态：列表只表达「是否还需要动作」，原始状态留给拓扑图与明细。 */
export type TaskPhase = 'finished' | 'running' | 'ready' | 'waiting';

export const TASK_PHASE_LABEL: Record<TaskPhase, string> = {
  finished: '已结束',
  running: '执行中',
  ready: '就绪',
  waiting: '等待',
};

/** 解析 dependsOn 下标数组（脏数据一律按空依赖处理，不抛错中断渲染）。 */
export function parseDepIndices(task: GroupChatTask): number[] {
  try {
    const parsed: unknown = JSON.parse(task.dependsOn || '[]');
    return Array.isArray(parsed) ? parsed.filter((n): n is number => typeof n === 'number') : [];
  } catch {
    return [];
  }
}

/** 终态：不再参与调度（完成 / 失败 / 跳过 / 中止）。 */
export function isTerminalStatus(status: TaskStatus): boolean {
  return status === 'success' || status === 'failed' || status === 'skipped' || status === 'aborted';
}

/** 前置任务对象（下标越界/脏数据自动跳过）。 */
export function depTasksOf(task: GroupChatTask, tasks: GroupChatTask[]): GroupChatTask[] {
  return parseDepIndices(task)
    .map((i) => tasks[i])
    .filter((t): t is GroupChatTask => !!t);
}

/**
 * 派生展示态：
 * - finished：任意终态（完成/失败/跳过/中止）
 * - running：正在执行
 * - ready：未开始且前置全部成功（与后端 ready 判定同口径）
 * - waiting：未开始且仍有前置未成功
 */
export function taskPhase(task: GroupChatTask, tasks: GroupChatTask[]): TaskPhase {
  if (isTerminalStatus(task.status)) return 'finished';
  if (task.status === 'running') return 'running';
  return depTasksOf(task, tasks).every((d) => d.status === 'success') ? 'ready' : 'waiting';
}

export interface TaskPhaseStats {
  finished: number;
  running: number;
  ready: number;
  waiting: number;
  /** 已结束中的异常数（失败 / 跳过 / 中止），用于统计行提示，避免「已结束」掩盖失败。 */
  abnormal: number;
}

export function taskPhaseStats(tasks: GroupChatTask[]): TaskPhaseStats {
  const stats: TaskPhaseStats = { finished: 0, running: 0, ready: 0, waiting: 0, abnormal: 0 };
  tasks.forEach((task) => {
    stats[taskPhase(task, tasks)] += 1;
    if (task.status === 'failed' || task.status === 'skipped' || task.status === 'aborted') {
      stats.abnormal += 1;
    }
  });
  return stats;
}

/** 直接后继任务（依赖徽标 / 拓扑连线用），与「全部后代」区分。 */
export function directDependentsOf(task: GroupChatTask, tasks: GroupChatTask[]): GroupChatTask[] {
  const idx = tasks.findIndex((t) => t.id === task.id);
  if (idx < 0) return [];
  return tasks.filter((t) => parseDepIndices(t).includes(idx));
}

export interface TaskGraphMetrics {
  nodeWidth: number;
  nodeHeight: number;
  gapX: number;
  gapY: number;
  padding: number;
}

/** 缩略图：只放任务号，节点小、层级紧凑。 */
export const MINI_GRAPH_METRICS: TaskGraphMetrics = {
  nodeWidth: 34,
  nodeHeight: 22,
  gapX: 18,
  gapY: 12,
  padding: 8,
};

/** 全屏拓扑：节点容纳任务号 + 一段描述。 */
export const FULL_GRAPH_METRICS: TaskGraphMetrics = {
  nodeWidth: 124,
  nodeHeight: 54,
  gapX: 56,
  gapY: 24,
  padding: 20,
};

export interface TaskGraphNode {
  task: GroupChatTask;
  level: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface TaskGraphEdge {
  fromTaskId: string;
  toTaskId: string;
  /** 三次贝塞尔路径（前置右缘 → 后继左缘）。 */
  d: string;
}

export interface TaskGraphLayout {
  nodes: TaskGraphNode[];
  edges: TaskGraphEdge[];
  width: number;
  height: number;
}

/**
 * 分层布局：`level = max(前置 level) + 1`，无依赖为 0。
 * 后端已拒绝成环，这里仍做兜底（环内节点压到同层），避免渲染死循环。
 */
function computeLevels(tasks: GroupChatTask[]): number[] {
  const levels = new Array<number>(tasks.length).fill(-1);
  const visiting = new Set<number>();
  const resolve = (i: number): number => {
    if (levels[i] >= 0) return levels[i];
    if (visiting.has(i)) {
      levels[i] = 0;
      return 0;
    }
    visiting.add(i);
    let level = 0;
    parseDepIndices(tasks[i])
      .filter((d) => d >= 0 && d < tasks.length)
      .forEach((d) => {
        level = Math.max(level, resolve(d) + 1);
      });
    visiting.delete(i);
    levels[i] = level;
    return level;
  };
  tasks.forEach((_, i) => resolve(i));
  return levels;
}

/** 构建拓扑布局（纯前端，不依赖后端）。 */
export function buildTaskGraph(tasks: GroupChatTask[], metrics: TaskGraphMetrics): TaskGraphLayout {
  if (tasks.length === 0) return { nodes: [], edges: [], width: 0, height: 0 };
  const levels = computeLevels(tasks);
  const byLevel = new Map<number, number[]>();
  tasks.forEach((_, i) => {
    const list = byLevel.get(levels[i]) ?? [];
    list.push(i);
    byLevel.set(levels[i], list);
  });
  const levelKeys = [...byLevel.keys()].sort((a, b) => a - b);
  const maxRows = Math.max(...levelKeys.map((l) => byLevel.get(l)!.length));
  const maxLevel = Math.max(...levelKeys);
  const rowSpan = (rows: number) => rows * metrics.nodeHeight + Math.max(0, rows - 1) * metrics.gapY;
  const canvasWidth = metrics.padding * 2 + (maxLevel + 1) * metrics.nodeWidth + maxLevel * metrics.gapX;
  const canvasHeight = metrics.padding * 2 + rowSpan(maxRows);

  const positions = new Map<number, { x: number; y: number }>();
  levelKeys.forEach((level) => {
    const rows = byLevel.get(level)!;
    const top = metrics.padding + (rowSpan(maxRows) - rowSpan(rows.length)) / 2;
    rows.forEach((taskIndex, row) => {
      positions.set(taskIndex, {
        x: metrics.padding + level * (metrics.nodeWidth + metrics.gapX),
        y: top + row * (metrics.nodeHeight + metrics.gapY),
      });
    });
  });

  const nodes: TaskGraphNode[] = tasks.map((task, i) => {
    const pos = positions.get(i)!;
    return { task, level: levels[i], x: pos.x, y: pos.y, width: metrics.nodeWidth, height: metrics.nodeHeight };
  });

  const edges: TaskGraphEdge[] = [];
  const curve = Math.max(16, metrics.gapX * 0.55);
  tasks.forEach((task, i) => {
    parseDepIndices(task)
      .filter((d) => d >= 0 && d < tasks.length)
      .forEach((d) => {
        const from = positions.get(d)!;
        const to = positions.get(i)!;
        const x1 = from.x + metrics.nodeWidth;
        const y1 = from.y + metrics.nodeHeight / 2;
        const x2 = to.x;
        const y2 = to.y + metrics.nodeHeight / 2;
        edges.push({
          fromTaskId: tasks[d].id,
          toTaskId: task.id,
          d: `M ${x1} ${y1} C ${x1 + curve} ${y1} ${x2 - curve} ${y2} ${x2} ${y2}`,
        });
      });
  });

  return { nodes, edges, width: canvasWidth, height: canvasHeight };
}
