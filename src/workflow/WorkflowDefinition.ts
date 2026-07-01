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
  const stageEdges: WorkflowEdge[] = [
    { id: generateEdgeId(startStageId, endStageId), source: startStageId, target: endStageId },
  ];

  return {
    id: generateId(),
    name,
    version: '1.0.0',
    description: '',
    trigger: { triggerType: 'manual' },
    stageEdges,
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

  
  // 检查 start/end 节点约束
  const flatNodes = def.stages.flatMap(s => s.nodes);
  const startNodes = flatNodes.filter(n => n.type === 'start');
  const endNodes = flatNodes.filter(n => n.type === 'end');

  if (startNodes.length === 0) {
    errors.push({ field: 'nodes', message: '工作流缺少起始节点（Start）', severity: 'error' });
  } else if (startNodes.length > 1) {
    errors.push({ field: 'nodes', message: '工作流只能有一个起始节点（Start）', severity: 'error' });
  }

  if (endNodes.length === 0) {
    errors.push({ field: 'nodes', message: '工作流缺少结束节点（End）', severity: 'error' });
  } else if (endNodes.length > 1) {
    errors.push({ field: 'nodes', message: '工作流只能有一个结束节点（End）', severity: 'error' });
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

// ── 阶段拓扑工具函数 ──

/**
 * 构建每个阶段的上游阶段 ID 集合（通过 stageEdges 传递搜索）
 *
 * 与节点级 upstreamMap 逻辑一致，但作用于阶段间连线。
 * source→target 表示数据流方向。
 *
 * @param stages  工作流阶段列表
 * @param stageEdges  阶段间连线列表（source/target 为 stage.id）
 * @returns Map<stageId, Set<上游stageId>>
 */
export function getStageUpstreamMap(
  stages: Stage[],
  stageEdges?: WorkflowEdge[],
): Map<string, Set<string>> {
  const stageIds = new Set(stages.map(s => s.id));
  const upstreamMap = new Map<string, Set<string>>();
  for (const stage of stages) {
    upstreamMap.set(stage.id, new Set());
  }

  // 确保 stageEdges 不为空
  const edges = stageEdges ?? [];

  // 遍历边构建上游关系
  for (const edge of edges) {
    // 跳过无效边（source/target 不是合法阶段 ID）
    if (!stageIds.has(edge.source) || !stageIds.has(edge.target)) continue;
    if (!upstreamMap.has(edge.target)) {
      upstreamMap.set(edge.target, new Set());
    }
    upstreamMap.get(edge.target)!.add(edge.source);
  }

  // 传递上游（BFS）
  const changed = true;
  let stable = false;
  while (!stable) {
    stable = true;
    for (const edge of edges) {
      if (!stageIds.has(edge.source) || !stageIds.has(edge.target)) continue;
      const srcUpstream = upstreamMap.get(edge.source);
      const tgtUpstream = upstreamMap.get(edge.target);
      if (srcUpstream && tgtUpstream) {
        for (const uid of srcUpstream) {
          if (!tgtUpstream.has(uid)) {
            tgtUpstream.add(uid);
            stable = false;
          }
        }
      }
    }
  }

  return upstreamMap;
}

/**
 * 获取从 start 节点出发通过拓扑路线可达的所有节点 ID 集合
 *
 * 全工作流 BFS：同时沿节点连线（阶段内）和阶段连线（阶段间）遍历，
 * 阶段连线是工作流拓扑结构的一部分。到达下游阶段时，该阶段所有节点
 * 进入 BFS 队列继续沿拓扑路线遍历。
 *
 * @param stages  工作流阶段列表
 * @param stageEdges  阶段间连线列表
 * @returns 从 start 节点可达的节点 ID 集合
 */
export function getReachableNodes(
  stages: Stage[],
  stageEdges?: WorkflowEdge[],
): Set<string> {
  // 1. 找到 start 节点所在阶段
  let startStageId: string | null = null;
  let startNodeId: string | null = null;
  for (const stage of stages) {
    for (const node of stage.nodes) {
      if (node.type === 'start') {
        startStageId = stage.id;
        startNodeId = node.id;
        break;
      }
    }
    if (startStageId) break;
  }
  if (!startStageId || !startNodeId) return new Set();

  // 2. 全工作流 BFS：沿节点连线 + 阶段连线遍历，收集从 start 可达的所有节点
  const reachable = new Set<string>();
  const visited = new Set<string>();
  const queue = [startNodeId];
  visited.add(startNodeId);

  // 构建阶段连线快速查找：从某阶段可达哪些下游阶段
  const se = stageEdges ?? [];
  const stageDownstream = new Map<string, string[]>();
  for (const edge of se) {
    const list = stageDownstream.get(edge.source) ?? [];
    if (!list.includes(edge.target)) list.push(edge.target);
    stageDownstream.set(edge.source, list);
  }

  while (queue.length > 0) {
    const current = queue.shift()!;
    reachable.add(current);
    // 找到当前节点所在阶段
    const currentStage = stages.find(s => s.nodes.some(n => n.id === current));
    if (!currentStage) continue;
    // 沿阶段内节点连线找下游节点
    for (const edge of currentStage.edges) {
      if (edge.source === current && !visited.has(edge.target)) {
        visited.add(edge.target);
        queue.push(edge.target);
      }
    }
    // 沿阶段连线找下游阶段，将下游阶段的所有节点加入队列
    const downstreams = stageDownstream.get(currentStage.id) ?? [];
    for (const dsId of downstreams) {
      const ds = stages.find(s => s.id === dsId);
      if (ds) {
        for (const node of ds.nodes) {
          if (!visited.has(node.id)) {
            visited.add(node.id);
            queue.push(node.id);
          }
        }
      }
    }
  }

  return reachable;
}

/**
 * 执行前验证：检查工作流是否满足执行条件
 *
 * 校验规则：
 * 1. 必须有且仅有一个 start 节点和一个 end 节点
 * 2. 至少存在一条从 start 到 end 的拓扑路径
 * 3. 所有节点必须与 start 建立拓扑连通（否则为"未就绪"节点）
 *
 * @param stages  工作流阶段列表
 * @param stageEdges  阶段间连线列表
 * @returns 验证错误列表（空数组表示可执行）
 */
export function validateWorkflowForExecution(
  stages: Stage[],
  stageEdges?: WorkflowEdge[],
): ValidationError[] {
  const errors: ValidationError[] = [];

  // 1. 检查 start 和 end 节点
  const allNodes = stages.flatMap(s => s.nodes);
  const startNodes = allNodes.filter(n => n.type === 'start');
  const endNodes = allNodes.filter(n => n.type === 'end');

  if (startNodes.length === 0) {
    errors.push({ field: 'nodes', message: '工作流缺少起始节点（Start）', severity: 'error' });
  } else if (startNodes.length > 1) {
    errors.push({ field: 'nodes', message: '工作流只能有一个起始节点（Start）', severity: 'error' });
  }

  if (endNodes.length === 0) {
    errors.push({ field: 'nodes', message: '工作流缺少结束节点（End）', severity: 'error' });
  } else if (endNodes.length > 1) {
    errors.push({ field: 'nodes', message: '工作流只能有一个结束节点（End）', severity: 'error' });
  }

  if (startNodes.length === 0 || endNodes.length === 0) return errors;

  // 2. 检查从 start 到 end 的连通性
  const reachable = getReachableNodes(stages, stageEdges);
  const endNodeId = endNodes[0].id;
  if (!reachable.has(endNodeId)) {
    errors.push({
      field: 'topology',
      message: '不存在从起始节点到结束节点的完整路径',
      severity: 'error',
    });
  }

  // 3. 检查未就绪节点（不在 start 可达路径上的节点）
  const unreachableNodes = allNodes.filter(n => !reachable.has(n.id));
  if (unreachableNodes.length > 0) {
    for (const node of unreachableNodes) {
      // start 节点不算未就绪（它是入口）
      if (node.type === 'start') continue;
      errors.push({
        field: 'topology',
        message: `节点 "${node.label}"(${node.id}) 未与起始节点建立拓扑关系`,
        severity: 'warning',
        nodeId: node.id,
        stageId: stages.find(s => s.nodes.some(n => n.id === node.id))?.id,
      });
    }
  }

  return errors;
}

// ── 映射引用完整性保障 ──

/**
 * 正则匹配映射值中的节点 ID 引用
 *
 * 新引用格式: {{key.节点ID.阶段ID}}，从中提取第二段（节点ID）
 * 旧引用格式: {{key.output.nodeId}}，向后兼容
 * 门控引用格式: {{gate_output.阶段ID}}，需特殊处理（不匹配节点ID模式）
 */
const MAPPING_REF_PATTERN = /\{\{[\w.]+?\.(node_[a-zA-Z0-9_]+)\.stage_[a-zA-Z0-9_]+\}\}/g;
/** 旧格式向后兼容: {{key.output.nodeId}} */
const MAPPING_REF_PATTERN_LEGACY = /\{\{[\w.]+?\.output\.(node_[a-zA-Z0-9_]+)\}\}/g;
/** 门控合并输出引用: {{gate_output.stageId}} */
const MAPPING_GATE_REF_PATTERN = /\{\{gate_output\.(stage_[a-zA-Z0-9_]+)\}\}/g;
/** session_id 引用: {{session_id.nodeId.stageId}}，需校验 agent 类型一致性 */
const MAPPING_SESSION_REF_PATTERN = /\{\{session_id\.(node_[a-zA-Z0-9_]+)\.stage_[a-zA-Z0-9_]+\}\}/g;

/**
 * 清理无效的映射引用
 *
 * 校验规则：
 * 1. 节点引用（content/session_id 等）：被引用节点必须存在且为当前节点的拓扑前序
 * 2. gate_output 引用：被引用阶段必须为当前阶段的阶段拓扑前序（通过 stageEdges 传递搜索）
 * 3. session_id 引用：被引用节点必须是 agent 类型且 agent_type 与当前节点一致
 *
 * 拓扑前序的定义：
 * - 节点级：通过边的 source→target 传递可达的上游节点
 * - 阶段级：通过 stageEdges 的 source→target 传递可达的上游阶段
 * 无效引用则清除该映射条目，强制用户重新设置。
 *
 * @param stages  工作流阶段列表（不会被修改，返回新的副本）
 * @returns 清理后的阶段列表
 */
export function sanitizeMappingReferences(stages: Stage[], stageEdges?: WorkflowEdge[]): Stage[] {
  // 收集所有节点信息
  const allNodeIds = new Set<string>();
  const allNodes = new Map<string, WorkflowNode>();
  for (const stage of stages) {
    for (const node of stage.nodes) {
      allNodeIds.add(node.id);
      allNodes.set(node.id, node);
    }
  }

  // 构建每个节点的上游 ID 集合（通过边 source→target 传递）
  const upstreamMap = new Map<string, Set<string>>();
  for (const stage of stages) {
    for (const edge of stage.edges) {
      if (!upstreamMap.has(edge.target)) {
        upstreamMap.set(edge.target, new Set());
      }
      upstreamMap.get(edge.target)!.add(edge.source);
      const srcUpstream = upstreamMap.get(edge.source);
      if (srcUpstream) {
        for (const uid of srcUpstream) {
          upstreamMap.get(edge.target)!.add(uid);
        }
      }
    }
  }

  // 构建阶段上游映射（通过 stageEdges 传递搜索）
  const stageUpstreamMap = getStageUpstreamMap(stages, stageEdges);

  // 收集阶段 order 映射
  const stageOrders = new Map<string, number>();
  for (const stage of stages) {
    stageOrders.set(stage.id, stage.order);
  }

  // 检查并清理每个节点的 inputMapping
  return stages.map(stage => {
    let nodesChanged = false;
    const newNodes = stage.nodes.map(node => {
      const inputMapping = node.inputMapping;
      if (!inputMapping || typeof inputMapping !== 'object' || Object.keys(inputMapping).length === 0) {
        return node;
      }

      const nodeUpstream = upstreamMap.get(node.id);
      const currentAgentType = node.type === 'agent' ? node.params?.agent_type : undefined;
      const stageUpstream = stageUpstreamMap.get(stage.id);
      const newMapping: Record<string, string> = {};
      let mappingChanged = false;

      for (const [key, value] of Object.entries(inputMapping)) {
        if (typeof value !== 'string') {
          newMapping[key] = value;
          continue;
        }

        // 提取各类引用
        const newRefs = value.match(MAPPING_REF_PATTERN) || [];
        const legacyRefs = value.match(MAPPING_REF_PATTERN_LEGACY) || [];
        const gateRefs = value.match(MAPPING_GATE_REF_PATTERN) || [];
        const sessionRefs = value.match(MAPPING_SESSION_REF_PATTERN) || [];

        const nodeRefs = [...newRefs, ...legacyRefs];

        if (nodeRefs.length === 0 && gateRefs.length === 0 && sessionRefs.length === 0) {
          newMapping[key] = value;
          continue;
        }

        let allValid = true;

        // ── 校验 1：节点引用必须为拓扑前序 ──
        for (const refId of nodeRefs) {
          if (!allNodeIds.has(refId) || !(nodeUpstream?.has(refId) ?? false)) {
            console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 引用节点 ' + refId + ' 不是拓扑前序节点');
            allValid = false;
            break;
          }
        }

        // ── 校验 2：gate_output 引用必须为阶段拓扑前序 ──
        //    使用 stageEdges 构建的上游关系，而非简单的 order 比较
        if (allValid) {
          for (const gateRefId of gateRefs) {
            if (!stageUpstream?.has(gateRefId)) {
              console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 引用 gate_output.' + gateRefId + ' 不是阶段拓扑前序');
              allValid = false;
              break;
            }
          }
        }

        // ── 校验 3：session_id 引用必须为拓扑前序 + agent 类型一致 ──
        if (allValid) {
          for (const sessionId of sessionRefs) {
            const refNode = allNodes.get(sessionId);
            if (!refNode || !(nodeUpstream?.has(sessionId) ?? false)) {
              console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 引用 session_id 节点 ' + sessionId + ' 不是拓扑前序节点');
              allValid = false;
              break;
            }
            if (refNode.type !== 'agent') {
              console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 引用 session_id 节点 ' + sessionId + ' 不是 agent 类型');
              allValid = false;
              break;
            }
            if (currentAgentType !== undefined) {
              if (refNode.params?.agent_type !== currentAgentType) {
                console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 引用 session_id 节点 ' + sessionId + ' agent_type 不一致（当前: ' + currentAgentType + ', 引用: ' + refNode.params?.agent_type + '）');
                allValid = false;
                break;
              }
            }
          }
        }

        if (allValid) {
          newMapping[key] = value;
        } else {
          mappingChanged = true;
          console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 的 inputMapping[' + key + '] 引用无效，已清除: ' + value);
        }
      }

      if (mappingChanged) {
        nodesChanged = true;
        return { ...node, inputMapping: Object.keys(newMapping).length > 0 ? newMapping : undefined };
      }
      return node;
    });

    return nodesChanged ? { ...stage, nodes: newNodes } : stage;
  });
}

/**
 * 为导入的工作流重新生成所有节点 ID，并更新映射引用
 *
 * 为所有节点生成新 ID，构建 old->new 映射表，
 * 更新 edges、inputMapping、outputMapping 中的 ID 引用。
 *
 * @param stages  原始阶段列表（不会被修改，返回新的副本）
 * @returns ID 重映射后的阶段列表
 */
export function remapImportedWorkflowIds(stages: Stage[], stageEdges?: WorkflowEdge[]): { stages: Stage[]; stageEdges: WorkflowEdge[] } {
  // 1. 收集所有节点 ID 并生成新 ID
  const idMap = new Map<string, string>();
  for (const stage of stages) {
    for (const node of stage.nodes) {
      idMap.set(node.id, generateId());
    }
  }

  // 1.5 统一生成阶段 ID 映射（供 stageEdges 和 mapping 引用替换使用）
  const stageIdMap = new Map<string, string>();
  for (const stage of stages) {
    stageIdMap.set(stage.id, generateStageId());
  }

  // 捕获原始阶段连线（在 generateId 产生副作用前）
  const originalStageEdges = stageEdges;

  // 2. 替换字符串中所有旧 ID 为新 ID
  //    新格式 {{key.nodeId.stageId}} 中 nodeId 在第二段
  //    旧格式 {{key.output.nodeId}} 中 nodeId 在第三段
  //    gate_output 引用中无 nodeId，不替换
  //    直接全局替换所有 nodeId 出现即可（阶段 ID 也会被替换，但 idMap 仅含节点 ID）
  const replaceIds = (str: string): string => {
    let result = str;
    for (const [oldId, newId] of idMap) {
      result = result.split(oldId).join(newId);
    }
    return result;
  };

  // 3. 替换 mapping 对象中的 ID 引用（节点 ID + 阶段 ID）
  //    新引用格式: {{key.nodeId.stageId}}，两者都需要替换
  const replaceMappingIds = (mapping: Record<string, string> | undefined, stageIdMap: Map<string, string>): Record<string, string> | undefined => {
    if (!mapping) return mapping;
    const replaceAllIds = (str: string): string => {
      let result = str;
      for (const [oldId, newId] of idMap) {
        result = result.split(oldId).join(newId);
      }
      for (const [oldId, newId] of stageIdMap) {
        result = result.split(oldId).join(newId);
      }
      return result;
    };
    const newMapping: Record<string, string> = {};
    for (const [key, value] of Object.entries(mapping)) {
      newMapping[key] = replaceAllIds(value);
    }
    return newMapping;
  };

  // 4. 重建阶段、节点、边、映射引用和阶段连线

  return {
    stageEdges: (originalStageEdges ?? []).map(edge => ({
      ...edge,
      id: generateEdgeId(stageIdMap.get(edge.source) || edge.source, stageIdMap.get(edge.target) || edge.target),
      source: stageIdMap.get(edge.source) || edge.source,
      target: stageIdMap.get(edge.target) || edge.target,
    })),
    stages: stages.map(stage => ({
      ...stage,
      id: stageIdMap.get(stage.id) || stage.id,
      nodes: stage.nodes.map(node => ({
        ...node,
        id: idMap.get(node.id)!,
        inputMapping: replaceMappingIds(node.inputMapping as Record<string, string> | undefined, stageIdMap),
        outputMapping: replaceMappingIds(node.outputMapping as Record<string, string> | undefined, stageIdMap),
      })),
      edges: stage.edges.map(edge => ({
        ...edge,
        source: idMap.get(edge.source) || edge.source,
        target: idMap.get(edge.target) || edge.target,
      })),
    })),
  };
}
