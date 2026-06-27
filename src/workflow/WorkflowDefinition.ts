/**
 * WorkflowDefinition — 工作流定义工具
 *
 * 节点类型元信息、验证、生成工具函数。
 * 适配 Structured Canvas 架构（6 种实体节点 + Stage/Gate）。
 */

import type { WorkflowDefinition, WorkflowNode, WorkflowEdge, WorkflowNodeType, Stage } from '../types/workflow';

// ── 节点类型元信息 ──

interface NodeTypeMeta {
  label: string;
  color: string;
  icon: string;
  canHaveInputs: boolean;
  canHaveOutputs: boolean;
  maxInputs: number;
  maxOutputs: number;
  /** 是否为边界节点（系统自动创建，不可删除） */
  isBoundary: boolean;
  /** 节点布局宽度（content box 坐标） */
  nodeW: number;
  /** 节点布局高度（content box 坐标） */
  nodeH: number;
}

const NODE_TYPE_META: Record<WorkflowNodeType, NodeTypeMeta> = {
  agent: { label: 'Agent 任务', color: '#58a6ff', icon: '⚡', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  api: { label: 'API 调用', color: '#a371f7', icon: '⬡', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  transform: { label: '代码转换', color: '#d29922', icon: '⟲', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  interact: { label: '人工交互', color: '#f85149', icon: '⚑', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  plugin: { label: '插件命令', color: '#3fb950', icon: '⊕', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  start: { label: '起始', color: '#3fb950', icon: '▶', canHaveInputs: false, canHaveOutputs: true, maxInputs: 0, maxOutputs: 10, isBoundary: true, nodeW: 160, nodeH: 60 },
  end: { label: '结束', color: '#f85149', icon: '■', canHaveInputs: true, canHaveOutputs: false, maxInputs: 10, maxOutputs: 0, isBoundary: true, nodeW: 160, nodeH: 60 },
  subflow: { label: '子工作流', color: '#79c0ff', icon: '⧉', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
};

export function getNodeTypeMeta(type: WorkflowNodeType): NodeTypeMeta {
  return NODE_TYPE_META[type] || { label: type, color: '#8b949e', icon: '❓', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 };
}

// ── ID 生成 ──

let _idCounter = 0;
export function generateId(): string {
  _idCounter++;
  return `node_${Date.now()}_${_idCounter}`;
}

export function generateEdgeId(source: string, target: string): string {
  return `edge_${source}_${target}`;
}

export function generateStageId(): string {
  return `stage_${Date.now()}_${_idCounter++}`;
}


// ── 节点布局常量 ──

/** 内容区 CSS padding（px） */
export const CPAD = 12;
/** 阶段窗体宽度（px） */
export const STAGE_W = 480;
/** 内容区可用宽度 = STAGE_W - 2 * CPAD */
export const CONTENT_BOX_W = STAGE_W - 2 * CPAD; // 456
/** 内容区高度（px） */
export const CONTENT_H = 500;
/** 网格吸附步长（px） */
export const SNAP_SIZE = 20;
/** 节点锚点超出节点边界的尺寸（px），clamp 时需纳入安全边距 */
export const ANCHOR_OVERFLOW = 5;
/** 节点在内容区内的安全边距（px）= ANCHOR_OVERFLOW，确保锚点不超出内容区 */
export const NODE_SAFE_MARGIN = ANCHOR_OVERFLOW;

/**
 * 节点位置 clamp（padding-box 坐标）
 *
 * 节点是 position:absolute，定位基准 = 内容区 div 的 padding-box 左上角。
 * clamp 确保节点（含锚点溢出）不超出内容区 padding-box。
 * x ∈ [MARGIN, STAGE_W - nodeW - MARGIN]
 * y ∈ [MARGIN, CONTENT_H - nodeH - MARGIN]
 */
export function clampNodePosition(
  x: number,
  y: number,
  nodeW: number,
  nodeH: number,
  scale: number = 1,
): { x: number; y: number } {
  const M = NODE_SAFE_MARGIN;
  const halfW = nodeW / 2;
  const halfH = nodeH / 2;

  // Scale 感知 clamp：确保缩放时视觉边距恒定为 M
  // 节点反向缩放 scale(1/scale)，画布正向缩放 scale(scale)，复合缩放 = 1
  // 视觉左边界 = (x + halfW) * scale - halfW >= M
  // 视觉右边界 = (STAGE_W * scale) - ((x + halfW) * scale + halfW) >= M
  const xMin = (M + halfW) / scale - halfW;
  const xMax = STAGE_W - (M + halfW) / scale - halfW;
  const yMin = (M + halfH) / scale - halfH;
  const yMax = CONTENT_H - (M + halfH) / scale - halfH;

  // 极端缩放时范围无效，退回居中
  if (xMin > xMax || yMin > yMax) {
    return {
      x: Math.round((STAGE_W - nodeW) / 2 / SNAP_SIZE) * SNAP_SIZE,
      y: Math.round((CONTENT_H - nodeH) / 2 / SNAP_SIZE) * SNAP_SIZE,
    };
  }

  return {
    x: Math.max(xMin, Math.min(xMax, Math.floor(x / SNAP_SIZE) * SNAP_SIZE)),
    y: Math.max(yMin, Math.min(yMax, Math.floor(y / SNAP_SIZE) * SNAP_SIZE)),
  };
}

/**
 * 计算节点默认初始位置（居中放置）
 * 用于边界节点和按钮手动添加的节点
 */
export function getDefaultNodePosition(nodeW: number, nodeH: number): { x: number; y: number } {
  return {
    x: Math.floor((STAGE_W - nodeW) / 2),
    y: Math.floor((CONTENT_H - nodeH) / 2),
  };
}

/**
 * 计算按钮手动添加节点的初始位置（避开已有节点）
 * 从居中位置开始，按网格偏移寻找空闲位置
 */
export function findFreePosition(
  existingPositions: { x: number; y: number }[],
  nodeW: number,
  nodeH: number,
  scale: number = 1,
): { x: number; y: number } {
  const center = getDefaultNodePosition(nodeW, nodeH);
  // 从居中位置开始，螺旋式搜索空闲位置
  for (let offset = 0; offset < 20; offset++) {
    for (let dx = -offset; dx <= offset; dx++) {
      for (let dy = -offset; dy <= offset; dy++) {
        if (Math.abs(dx) !== offset && Math.abs(dy) !== offset) continue;
        const px = Math.floor((center.x + dx * SNAP_SIZE * 2) / SNAP_SIZE) * SNAP_SIZE;
        const py = Math.floor((center.y + dy * SNAP_SIZE * 2) / SNAP_SIZE) * SNAP_SIZE;
        const clamped = clampNodePosition(px, py, nodeW, nodeH, scale);
        // 检查是否与已有节点重叠（含间距）
        const GAP = 20;
        const overlaps = existingPositions.some(p =>
          clamped.x < p.x + nodeW + GAP && clamped.x + nodeW + GAP > p.x &&
          clamped.y < p.y + nodeH + GAP && clamped.y + nodeH + GAP > p.y
        );
        if (!overlaps) return clamped;
      }
    }
  }
  return center;
}

// ── 统一节点创建方法 ──

/**
 * 创建工作流节点（统一入口）
 *
 * 所有节点创建（拖拽添加、按钮添加、默认节点）都通过此方法。
 * position 为可选，不传时自动计算居中/空闲位置。
 *
 * @param type  节点类型
 * @param position  可选的 content box 坐标（拖拽时传入鼠标位置）
 * @param existingPositions  阶段内已有节点位置列表（用于 findFreePosition）
 */
export function createWorkflowNode(
  type: WorkflowNodeType,
  position?: { x: number; y: number },
  existingPositions?: { x: number; y: number }[],
  scale: number = 1,
): WorkflowNode {
  const meta = getNodeTypeMeta(type);
  let finalPosition: { x: number; y: number };

  if (position !== undefined) {
    // 拖拽放置：使用传入位置 + clamp
    finalPosition = clampNodePosition(
      position.x - meta.nodeW / 2,
      position.y - meta.nodeH / 2,
      meta.nodeW,
      meta.nodeH,
      scale,
    );
  } else if (meta.isBoundary) {
    // 边界节点：居中放置
    finalPosition = getDefaultNodePosition(meta.nodeW, meta.nodeH);
  } else {
    // 普通节点按钮添加：寻找空闲位置
    finalPosition = findFreePosition(existingPositions || [], meta.nodeW, meta.nodeH, scale);
  }

  return {
    id: generateId(),
    type,
    label: meta.label,
    params: {},
    position: finalPosition,
    isBoundary: meta.isBoundary,
  };
}

// ── 默认工作流 ──

export function createDefaultWorkflow(name: string): WorkflowDefinition {
  const startNode = createWorkflowNode('start');
  const endNode = createWorkflowNode('end');
  const startStageId = generateStageId();
  const endStageId = generateStageId();
  return {
    id: generateId(),
    name,
    version: '1.0.0',
    description: '',
    trigger: { triggerType: 'manual' },
    stages: [
      {
        id: startStageId,
        name: '起始阶段',
        order: 0,
        nodes: [startNode],
        edges: [],
        gate: { strategy: 'all', mergeStrategy: 'merge' },
      },
      {
        id: endStageId,
        name: '结束阶段',
        order: 1,
        nodes: [endNode],
        edges: [],
        gate: { strategy: 'all', mergeStrategy: 'merge' },
      },
    ],
    createdAt: Math.floor(Date.now() / 1000),
    updatedAt: Math.floor(Date.now() / 1000),
    enabled: true,
  };
}

// ── 验证 ──

export interface ValidationError {
  field: string;
  message: string;
  severity: 'error' | 'warning';
  nodeId?: string;
  edgeId?: string;
  stageId?: string;
}

export function validateWorkflow(def: WorkflowDefinition): ValidationError[] {
  const errors: ValidationError[] = [];

  if (!def.name) {
    errors.push({ field: 'name', message: '工作流名称不能为空', severity: 'error' });
  }

  if (!def.stages || def.stages.length === 0) {
    errors.push({ field: 'stages', message: '至少需要一个阶段', severity: 'error' });
    return errors;
  }

  const allNodeIds = new Set<string>();
  for (const stage of def.stages) {
    for (const node of stage.nodes) {
      if (allNodeIds.has(node.id)) {
        errors.push({ field: 'nodes', message: `节点 ID 重复: ${node.id}`, severity: 'error', nodeId: node.id, stageId: stage.id });
      }
      allNodeIds.add(node.id);
    }
  }

  for (const stage of def.stages) {
    for (const edge of stage.edges) {
      if (!allNodeIds.has(edge.source)) {
        errors.push({ field: 'edges', message: `边 ${edge.id} 的源节点 ${edge.source} 不存在`, severity: 'error', edgeId: edge.id, stageId: stage.id });
      }
      if (!allNodeIds.has(edge.target)) {
        errors.push({ field: 'edges', message: `边 ${edge.id} 的目标节点 ${edge.target} 不存在`, severity: 'error', edgeId: edge.id, stageId: stage.id });
      }
    }
  }

  return errors;
}

// ── 智能连线 — 自动归入阶段 ──

export function autoAssignStage(stages: Stage[]): Stage[] {
  const nodeToStage = new Map<string, number>();
  for (const stage of stages) {
    for (const node of stage.nodes) {
      nodeToStage.set(node.id, stage.order);
    }
  }

  const nodeLeftmost = new Map<string, number>();
  for (const stage of stages) {
    for (const edge of stage.edges) {
      const sourceStage = nodeToStage.get(edge.source) ?? 0;
      const targetStage = nodeToStage.get(edge.target) ?? 0;
      const leftmost = Math.min(sourceStage, targetStage);
      nodeLeftmost.set(edge.source, Math.min(nodeLeftmost.get(edge.source) ?? leftmost, leftmost));
      nodeLeftmost.set(edge.target, Math.min(nodeLeftmost.get(edge.target) ?? leftmost, leftmost));
    }
  }

  const moves: { nodeId: string; targetOrder: number }[] = [];
  for (const [nodeId, targetOrder] of nodeLeftmost) {
    const currentOrder = nodeToStage.get(nodeId);
    if (currentOrder !== undefined && currentOrder !== targetOrder) {
      moves.push({ nodeId, targetOrder });
    }
  }

  if (moves.length === 0) return stages;

  const newStages = stages.map(s => ({ ...s, nodes: [...s.nodes], edges: [...s.edges] }));

  for (const { nodeId, targetOrder } of moves) {
    let movedNode: WorkflowNode | null = null;
    for (const stage of newStages) {
      const idx = stage.nodes.findIndex(n => n.id === nodeId);
      if (idx !== -1) {
        movedNode = stage.nodes.splice(idx, 1)[0];
        break;
      }
    }
    if (movedNode) {
      // 同时移动与该节点相关的边到目标阶段
      const relatedEdges: WorkflowEdge[] = [];
      for (const stage of newStages) {
        const keepEdges: WorkflowEdge[] = [];
        for (const edge of stage.edges) {
          if (edge.source === nodeId || edge.target === nodeId) {
            relatedEdges.push(edge);
          } else {
            keepEdges.push(edge);
          }
        }
        stage.edges = keepEdges;
      }
      const target = newStages.find(s => s.order === targetOrder);
      if (target) {
        target.nodes.push(movedNode);
        target.edges.push(...relatedEdges);
      }
    }
  }

  return newStages.filter(s => s.nodes.length > 0 || s.edges.length > 0)
    .map((s, i) => ({ ...s, order: i }));
}


