/**
 * 飞地任务拓扑地图的布局计算（纯函数，无 React 依赖）。
 *
 * 布局口径：
 * - 依赖边来自 `blockedBy`（纯 AND 前置），按最长路径分层，从左到右表示推进方向；
 * - 父子汇总关系用分组框表达（父任务节点 + 子任务节点同框）；
 * - 同一层的分组超过 `MAX_GROUPS_PER_COLUMN` 时换列，避免单层纵向无限延伸；
 * - 筛选导致前置任务不在当前集合时，忽略该条边，而不是把节点推到画布外的层。
 */
import type { TaskView } from '../../types/enclave';

export const NODE_W = 196;
export const NODE_H = 46;
const NODE_GAP = 8;
const GROUP_PAD = 8;
const COL_GAP = 48;
const ROW_GAP = 12;
const CANVAS_PAD = 20;
const MAX_GROUPS_PER_COLUMN = 4;

export interface TopoNode {
  id: string;
  task: TaskView;
  x: number;
  y: number;
  width: number;
  height: number;
  /** 汇总任务：已完成子任务数（非汇总任务为 0） */
  childDone: number;
  childTotal: number;
}

export interface TopoGroupBox {
  key: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** false = 该组只有一个任务，不画分组框 */
  framed: boolean;
}

export interface TopoEdge {
  key: string;
  from: string;
  to: string;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  /** 'h' = 源在左、目标在右；'v' = 同列内竖向连接 */
  direction: 'h' | 'v';
  /** 目标当前被未完成前置阻塞 */
  blocked: boolean;
}

export interface TopoLayout {
  nodes: TopoNode[];
  edges: TopoEdge[];
  groups: TopoGroupBox[];
  width: number;
  height: number;
}

interface RawGroup {
  key: string;
  leader: TaskView;
  members: TaskView[];
  framed: boolean;
}

function buildGroups(tasks: TaskView[], byId: Map<string, TaskView>, flat: boolean): RawGroup[] {
  if (flat) {
    return tasks.map((t) => ({ key: t.id, leader: t, members: [t], framed: false }));
  }

  const childrenByParent = new Map<string, TaskView[]>();
  for (const t of tasks) {
    if (!t.parentTaskId) continue;
    const list = childrenByParent.get(t.parentTaskId) ?? [];
    list.push(t);
    childrenByParent.set(t.parentTaskId, list);
  }

  const used = new Set<string>();
  const groups: RawGroup[] = [];
  for (const t of tasks) {
    // 顶端任务：没有父任务，或父任务不在当前（筛选后）集合里
    if (t.parentTaskId && byId.has(t.parentTaskId)) continue;
    if (used.has(t.id)) continue;
    const children = (childrenByParent.get(t.id) ?? []).filter((c) => !used.has(c.id));
    used.add(t.id);
    children.forEach((c) => used.add(c.id));
    groups.push({ key: t.id, leader: t, members: [t, ...children], framed: children.length > 0 });
  }
  // 兜底：父任务本身不在集合里、但子任务在（例如按状态筛选后的残留）
  for (const t of tasks) {
    if (used.has(t.id)) continue;
    groups.push({ key: t.id, leader: t, members: [t], framed: false });
  }
  return groups;
}

function computeLayers(tasks: TaskView[], byId: Map<string, TaskView>): Map<string, number> {
  const layer = new Map<string, number>();
  tasks.forEach((t) => layer.set(t.id, 0));

  // 最长路径分层；迭代上限 = 节点数，遇到环时自然收敛为一个有限值
  for (let round = 0; round < tasks.length; round++) {
    let changed = false;
    for (const t of tasks) {
      let next = 0;
      for (const pid of t.blockedBy) {
        if (!byId.has(pid)) continue;
        next = Math.max(next, (layer.get(pid) ?? 0) + 1);
      }
      if (next > (layer.get(t.id) ?? 0)) {
        layer.set(t.id, next);
        changed = true;
      }
    }
    if (!changed) break;
  }
  return layer;
}

function chunk<T>(list: T[], size: number): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < list.length; i += size) out.push(list.slice(i, i + size));
  return out;
}

export function buildTaskTopology(tasks: TaskView[], flat = false): TopoLayout {
  if (tasks.length === 0) {
    return { nodes: [], edges: [], groups: [], width: 0, height: 0 };
  }

  const byId = new Map(tasks.map((t) => [t.id, t]));
  const rawGroups = buildGroups(tasks, byId, flat);
  const layers = computeLayers(tasks, byId);
  const groupLayer = (g: RawGroup) => Math.max(...g.members.map((m) => layers.get(m.id) ?? 0));

  const sorted = [...rawGroups].sort(
    (a, b) =>
      groupLayer(a) - groupLayer(b) ||
      a.leader.createdAt - b.leader.createdAt ||
      a.leader.id.localeCompare(b.leader.id),
  );

  const slotW = NODE_W + GROUP_PAD * 2;
  const heightOf = (g: RawGroup) =>
    g.members.length * (NODE_H + NODE_GAP) - NODE_GAP + GROUP_PAD * 2;

  const nodes: TopoNode[] = [];
  const groups: TopoGroupBox[] = [];

  let cursorX = CANVAS_PAD;
  let maxBottom = CANVAS_PAD;

  const layerValues = Array.from(new Set(sorted.map(groupLayer))).sort((a, b) => a - b);
  for (const value of layerValues) {
    const inLayer = sorted.filter((g) => groupLayer(g) === value);
    for (const column of chunk(inLayer, MAX_GROUPS_PER_COLUMN)) {
      let cursorY = CANVAS_PAD;
      for (const g of column) {
        const height = heightOf(g);
        const box: TopoGroupBox = {
          key: g.key,
          x: cursorX,
          y: cursorY,
          width: slotW,
          height,
          framed: g.framed,
        };
        groups.push(box);

        const childTotal = g.members.length - 1;
        g.members.forEach((task, index) => {
          const nodeY = cursorY + GROUP_PAD + index * (NODE_H + NODE_GAP);
          nodes.push({
            id: task.id,
            task,
            x: cursorX + GROUP_PAD,
            y: nodeY,
            width: NODE_W,
            height: NODE_H,
            childDone: index === 0 ? g.members.slice(1).filter((c) => c.status === 'done').length : 0,
            childTotal: index === 0 ? childTotal : 0,
          });
        });

        cursorY += height + ROW_GAP;
        maxBottom = Math.max(maxBottom, cursorY - ROW_GAP);
      }
      cursorX += slotW + COL_GAP;
    }
  }

  const nodeById = new Map(nodes.map((n) => [n.id, n]));
  const edges: TopoEdge[] = [];
  for (const task of tasks) {
    for (const pid of task.blockedBy) {
      const source = nodeById.get(pid);
      const target = nodeById.get(task.id);
      if (!source || !target) continue;
      const horizontal = target.x > source.x + source.width / 2;
      edges.push({
        key: `${pid}->${task.id}`,
        from: pid,
        to: task.id,
        x1: horizontal ? source.x + source.width : source.x + source.width / 2,
        y1: horizontal ? source.y + source.height / 2 : source.y + source.height,
        x2: horizontal ? target.x : target.x + target.width / 2,
        y2: horizontal ? target.y + target.height / 2 : target.y,
        direction: horizontal ? 'h' : 'v',
        blocked: task.blocked,
      });
    }
  }

  return {
    nodes,
    edges,
    groups,
    width: cursorX - COL_GAP + CANVAS_PAD,
    height: maxBottom + CANVAS_PAD,
  };
}
