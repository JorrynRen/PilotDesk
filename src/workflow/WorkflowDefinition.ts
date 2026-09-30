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

export const NODE_TYPE_META: Record<string, NodeTypeMeta> = {
  agent: { label: 'Agent 任务', color: '#58a6ff', icon: '⚡', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  api: { label: 'API 调用', color: '#a371f7', icon: '⬡', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  transform: { label: '代码转换', color: '#d29922', icon: '⟲', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  interact: { label: '人工交互', color: '#f85149', icon: '⚑', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  plugin: { label: '插件调用', color: '#3fb950', icon: '⊕', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
  start: { label: '起始', color: '#3fb950', icon: '▶', canHaveInputs: false, canHaveOutputs: true, maxInputs: 0, maxOutputs: 10, isBoundary: true, nodeW: 160, nodeH: 60 },
  end: { label: '结束', color: '#f85149', icon: '■', canHaveInputs: true, canHaveOutputs: false, maxInputs: 10, maxOutputs: 0, isBoundary: true, nodeW: 160, nodeH: 60 },
  subflow: { label: '子工作流', color: '#79c0ff', icon: '⧉', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 },
};

// 节点创建时的默认 params（对应 HumanInputConfig 的 camelCase 字段，
// 与 NODE_TYPE_CONFIG_MAP 的 field.key 保持一致）
export const NODE_DEFAULT_PARAMS: Record<string, Record<string, unknown>> = {
  agent: { agent_type: 'claude', prompt_template: '' },
  api: { method: 'GET', url: '', body_template: '' },
  transform: { script: '' },
  interact: { prompt: '请输入', inputType: 'text', timeoutMinutes: 30 },
  plugin: {},
  subflow: {},
  start: {},
  end: {},
};


/**
 * 由节点 id 反查归属，拼「阶段名·节点名」（界面显示用，多个同名节点据此区分）。
 * 查不到归属时退回调用方给的节点名，再退回节点 id。
 */
export function formatStageNodeLabel(stages: Stage[], nodeId: string, fallbackLabel?: string): string {
  for (const stage of stages) {
    const node = stage.nodes.find((n) => n.id === nodeId);
    if (node) {
      const where = [stage.name, node.label].filter(Boolean).join('·');
      if (where) return where;
    }
  }
  return fallbackLabel || nodeId;
}

export function getNodeTypeMeta(type: string): NodeTypeMeta {
  const key = (type || '').toLowerCase();
  return NODE_TYPE_META[key] || { label: type, color: '#8b949e', icon: '❓', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10, isBoundary: false, nodeW: 160, nodeH: 60 };
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
 * 阶段内画布平移量。
 *
 * 内容层整体按此平移，阶段框相当于一个**窗口**：
 * 平移后能看见的内容坐标区间会跟着移动（见 `visibleContentWindow`）。
 */
export interface StagePan {
  x: number;
  y: number;
}

/**
 * 阶段"可视窗口"在内容坐标下的范围。
 *
 * 内容层按 pan 平移，因此窗口在内容坐标里是**反向**移动的：
 * 平移 pan 后可见的内容坐标 = [-pan, -pan + 尺寸]。
 *
 * 这个窗口同时是两个边界：
 *   - **裁剪边界**：内容层 overflow:hidden，节点与连线都不会画到阶段框外（拖动画布可查看其余部分）；
 *   - **放置边界**：节点只能落在看得见的位置，避免出现"掉在看不见的地方"的节点。
 */
export function visibleContentWindow(pan: StagePan = { x: 0, y: 0 }): { left: number; top: number; right: number; bottom: number } {
  return {
    left: -pan.x,
    top: -pan.y,
    right: -pan.x + STAGE_W,
    bottom: -pan.y + CONTENT_H,
  };
}

/**
 * 节点位置 clamp（内容层坐标，网格对齐）
 *
 * 节点是 position:absolute，定位基准 = 内容层左上角；内容层已按 stagePan 平移，
 * 所以边界用的是**可视窗口**而不是固定 [0, STAGE_W] —— 否则平移过阶段后再放置节点会被"夹回"原位置。
 */
export function clampNodePosition(
  x: number,
  y: number,
  nodeW: number,
  nodeH: number,
  scale: number = 1,
  pan: StagePan = { x: 0, y: 0 },
): { x: number; y: number } {
  const M = NODE_SAFE_MARGIN;
  const halfW = nodeW / 2;
  const halfH = nodeH / 2;
  const win = visibleContentWindow(pan);

  // Scale 感知 clamp：确保缩放时视觉边距恒定为 M
  // 节点反向缩放 scale(1/scale)，画布正向缩放 scale(scale)，复合缩放 = 1
  const xMin = win.left + (M + halfW) / scale - halfW;
  const xMax = win.right - (M + halfW) / scale - halfW;
  const yMin = win.top + (M + halfH) / scale - halfH;
  const yMax = win.bottom - (M + halfH) / scale - halfH;

  // 极端缩放时范围无效，退回窗口居中
  if (xMin > xMax || yMin > yMax) {
    return {
      x: Math.round((win.left + (STAGE_W - nodeW) / 2) / SNAP_SIZE) * SNAP_SIZE,
      y: Math.round((win.top + (CONTENT_H - nodeH) / 2) / SNAP_SIZE) * SNAP_SIZE,
    };
  }

  return {
    x: Math.max(xMin, Math.min(xMax, Math.floor(x / SNAP_SIZE) * SNAP_SIZE)),
    y: Math.max(yMin, Math.min(yMax, Math.floor(y / SNAP_SIZE) * SNAP_SIZE)),
  };
}

/**
 * 拖动时限制节点位置（不做网格 snap，便于吸附对齐）。
 * 边界 = 阶段可视窗口（含锚点安全边距），与 `clampNodePosition` 同一套窗口。
 */
export function clampNodePositionNoSnap(
  x: number,
  y: number,
  nodeW: number,
  nodeH: number,
  pan: StagePan = { x: 0, y: 0 },
): { x: number; y: number } {
  const M = NODE_SAFE_MARGIN;
  const win = visibleContentWindow(pan);
  return {
    x: Math.max(win.left + M, Math.min(win.right - nodeW - M, x)),
    y: Math.max(win.top + M, Math.min(win.bottom - nodeH - M, y)),
  };
}

/**
 * 拖动位移的允许区间（与 `clampNodePositionNoSnap` 用同一套边界）。
 *
 * 拖动过程中就要把位移限制在区间内，否则节点会先跟着鼠标跑出窗口、松手才被夹回来
 * ——看起来就是"拖拽时跳动 / 节点溢出阶段框"。
 */
export function nodeDragOffsetRange(
  orig: { x: number; y: number },
  nodeW: number,
  nodeH: number,
  pan: StagePan = { x: 0, y: 0 },
): { minX: number; maxX: number; minY: number; maxY: number } {
  const M = NODE_SAFE_MARGIN;
  const win = visibleContentWindow(pan);
  return {
    minX: win.left + M - orig.x,
    maxX: win.right - nodeW - M - orig.x,
    minY: win.top + M - orig.y,
    maxY: win.bottom - nodeH - M - orig.y,
  };
}

/**
 * 计算节点默认初始位置（窗口居中）
 * 用于边界节点和按钮手动添加的节点
 */
export function getDefaultNodePosition(
  nodeW: number,
  nodeH: number,
  pan: StagePan = { x: 0, y: 0 },
): { x: number; y: number } {
  const win = visibleContentWindow(pan);
  return {
    x: Math.floor((win.left + (STAGE_W - nodeW) / 2)),
    y: Math.floor((win.top + (CONTENT_H - nodeH) / 2)),
  };
}

/**
 * 计算按钮手动添加节点的初始位置（避开已有节点）
 * 从窗口居中位置开始，按网格偏移寻找空闲位置
 */
export function findFreePosition(
  existingPositions: { x: number; y: number }[],
  nodeW: number,
  nodeH: number,
  scale: number = 1,
  pan: StagePan = { x: 0, y: 0 },
): { x: number; y: number } {
  const center = getDefaultNodePosition(nodeW, nodeH, pan);
  // 从居中位置开始，螺旋式搜索空闲位置
  for (let offset = 0; offset < 20; offset++) {
    for (let dx = -offset; dx <= offset; dx++) {
      for (let dy = -offset; dy <= offset; dy++) {
        if (Math.abs(dx) !== offset && Math.abs(dy) !== offset) continue;
        const px = Math.floor((center.x + dx * SNAP_SIZE * 2) / SNAP_SIZE) * SNAP_SIZE;
        const py = Math.floor((center.y + dy * SNAP_SIZE * 2) / SNAP_SIZE) * SNAP_SIZE;
        const clamped = clampNodePosition(px, py, nodeW, nodeH, scale, pan);
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
 * @param pan  目标阶段的画布平移量（决定可视窗口，落点按窗口夹紧，保证节点一定可见）
 */
export function createWorkflowNode(
  type: string,
  position?: { x: number; y: number },
  existingPositions?: { x: number; y: number }[],
  scale: number = 1,
  pan: StagePan = { x: 0, y: 0 },
): WorkflowNode {
  const meta = getNodeTypeMeta(type);
  let finalPosition: { x: number; y: number };

  if (position !== undefined) {
    // 拖拽放置：鼠标点居中 + 夹进阶段可视窗口（含锚点安全边距，不会溢出阶段框）
    finalPosition = clampNodePosition(
      position.x - meta.nodeW / 2,
      position.y - meta.nodeH / 2,
      meta.nodeW,
      meta.nodeH,
      scale,
      pan,
    );
  } else if (meta.isBoundary) {
    // 边界节点：窗口居中放置
    finalPosition = getDefaultNodePosition(meta.nodeW, meta.nodeH, pan);
  } else {
    // 普通节点按钮添加：寻找空闲位置
    finalPosition = findFreePosition(existingPositions || [], meta.nodeW, meta.nodeH, scale, pan);
  }

  return {
    id: generateId(),
    type: type as WorkflowNodeType,
    label: meta.label,
    params: { ...(NODE_DEFAULT_PARAMS[type] ?? {}) },
    position: finalPosition,
    isBoundary: meta.isBoundary,
    // interact 节点默认暴露用户输入为 output 字段，方便下游直接引用
    ...(type === 'interact' ? { outputMapping: { output: '{{content}}' } } : {}),
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
        stageEdges: [{ id: generateEdgeId(startStageId, endStageId), source: startStageId, target: endStageId }],
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

// ── 映射引用完整性保障 ──

/**
 * 正则匹配映射值中的节点 ID 引用
 *
/**
 * 匹配映射值中的 {{...}} 引用，提取内部内容
 * 按 "." 分隔后根据段数决定校验策略：
 *   - 段数 = 1：常量 / 短变量名（如 {{title}}），跳过强检查
 *   - 段数 = 2：门控引用（如 {{gate_output.stageId}}），检查阶段拓扑前序
 *   - 段数 = 3：节点引用（如 {{key.nodeId.stageId}}），预检第二段是否为 start 节点
 */
/**
 * 清理无效的映射引用
 *
 * 从映射值中提取所有 {{...}} 占位符，按 "." 分隔后根据段数决定校验策略：
 *   段数 = 1：常量 / 短变量名（如 {{title}}），跳过检查
 *   段数 = 2：两段引用（如 {{gate_output.stageId}}），统一检查第二段阶段拓扑前序
 *   段数 >= 3：节点引用（如 {{key.nodeId.xxx}}），第二段为 nodeId：
 *     - 其他（含 start 节点与 session_id）：预检 nodeId 是否为 start 节点，是则跳过，不是则强检查
 *
 * 注意：本函数只校验**输入映射**的取值可达性（节点存在 + 拓扑前序）。
 * 「agent 类型必须一致」不属于输入映射的约束（那只是取 session_id 值）；
 * 它是**延续会话**（params.resume_session_ref）的约束，在选择器侧生效
 * （见 WorkflowNodeConfig 里延续会话候选的过滤）。
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

  // 构建每个节点的上游 ID 集合（通过边 source→target 传递 + 跨阶段前序）
  // 注意：集合必须传递到收敛，单趟遍历会因连线数组顺序不同而漏掉隔层上游，
  // 进而把合法引用当作"非前序"静默清除。
  const upstreamMap = new Map<string, Set<string>>();
  for (const stage of stages) {
    for (const edge of stage.edges) {
      if (!upstreamMap.has(edge.target)) {
        upstreamMap.set(edge.target, new Set());
      }
      upstreamMap.get(edge.target)!.add(edge.source);
    }
  }
  let upstreamStable = false;
  while (!upstreamStable) {
    upstreamStable = true;
    for (const stage of stages) {
      for (const edge of stage.edges) {
        const srcUpstream = upstreamMap.get(edge.source);
        const tgtUpstream = upstreamMap.get(edge.target);
        if (srcUpstream && tgtUpstream) {
          for (const uid of srcUpstream) {
            if (!tgtUpstream.has(uid)) {
              tgtUpstream.add(uid);
              upstreamStable = false;
            }
          }
        }
      }
    }
  }
  // 补充跨阶段上游：前序阶段的所有节点视为当前阶段所有节点的上游
  for (let i = 1; i < stages.length; i++) {
    for (const node of stages[i].nodes) {
      if (!upstreamMap.has(node.id)) {
        upstreamMap.set(node.id, new Set());
      }
      for (let j = 0; j < i; j++) {
        for (const prevNode of stages[j].nodes) {
          upstreamMap.get(node.id)!.add(prevNode.id);
        }
      }
    }
  }

  // 收集所有 start 节点的 ID
  const startNodeIds = new Set<string>();
  for (const stage of stages) {
    for (const node of stage.nodes) {
      if (node.type === 'start') {
        startNodeIds.add(node.id);
      }
    }
  }

  // 构建阶段上游映射（通过 stageEdges 传递搜索）
  // 调用方多数只传 stages，此时回退到阶段自身携带的 stageEdges；否则校验拿到空集，
  // 会把所有 {{gate_output.阶段ID}} 引用当成非法引用清除。
  const effectiveStageEdges = stageEdges ?? stages.flatMap(s => s.stageEdges || []);
  const stageUpstreamMap = getStageUpstreamMap(stages, effectiveStageEdges);

  /** 从字符串中提取所有 {{...}} 占位符的内部内容 */
  const extractPlaceholders = (str: string): string[] => {
    const result: string[] = [];
    let start = 0;
    while (true) {
      const open = str.indexOf('{{', start);
      if (open === -1) break;
      const close = str.indexOf('}}', open);
      if (close === -1) break;
      const inner = str.substring(open + 2, close).trim();
      if (inner.length > 0) result.push(inner);
      start = close + 2;
    }
    return result;
  };

  // 检查并清理每个节点的 inputMapping
  return stages.map(stage => {
    let nodesChanged = false;
    const newNodes = stage.nodes.map(node => {
      const inputMapping = node.inputMapping;
      if (!inputMapping || typeof inputMapping !== 'object' || Object.keys(inputMapping).length === 0) {
        return node;
      }

      const nodeUpstream = upstreamMap.get(node.id);
      const stageUpstream = stageUpstreamMap.get(stage.id);
      const newMapping: Record<string, string> = {};
      let mappingChanged = false;

      for (const [key, value] of Object.entries(inputMapping)) {
        if (typeof value !== 'string') {
          newMapping[key] = value;
          continue;
        }

        const placeholders = extractPlaceholders(value);

        if (placeholders.length === 0) {
          newMapping[key] = value;
          continue;
        }

        let allValid = true;

        for (const placeholder of placeholders) {
          const parts = placeholder.split('.');

          if (parts.length === 1) {
            // 段数 = 1：常量 / 短变量名，跳过检查
            continue;
          } else if (parts.length === 2) {
            // 段数 = 2：按第一段区分语义，避免把节点引用误当门控引用清除
            //   {{gate_output.阶段ID}} → 校验阶段拓扑前序
            //   {{节点ID.字段}}        → 校验节点拓扑前序
            //   其它（子工作流参数、__input__/保留别名等）→ 不强校验，放行
            if (parts[0] === 'gate_output') {
              if (!stageUpstream?.has(parts[1])) {
                console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 两段引用 ' + parts.join('.') + ' 的阶段 ' + parts[1] + ' 不是阶段拓扑前序');
                allValid = false;
                break;
              }
            } else if (allNodeIds.has(parts[0])) {
              if (!(nodeUpstream?.has(parts[0]) ?? false)) {
                console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 两段引用 ' + parts.join('.') + ' 的节点 ' + parts[0] + ' 不是拓扑前序节点');
                allValid = false;
                break;
              }
            }
          } else {
            // 段数 >= 3：第二段 = nodeId
            const refNodeId = parts[1];

            // 门控合并值下钻：{{gate_output.阶段ID.节点ID.字段}} → 校验阶段拓扑前序即可
            // （节点ID 属于该阶段内部，不再逐节点校验，否则引用会被当成非法引用清除）
            if (parts[0] === 'gate_output') {
              if (!(stageUpstream?.has(parts[1]) ?? false)) {
                console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 引用 ' + placeholder + ' 的阶段 ' + parts[1] + ' 不是阶段拓扑前序');
                allValid = false;
                break;
              }
              continue;
            }

            // 输入映射里的 session_id 引用：与普通字段引用同规则——只是取上游节点暴露的会话 ID 值，
            // 不要求上游是同类型 agent（`{{session_id.上游agent.阶段}}` 在 api / CLI 节点里都能读）。
            // 「agent 类型必须一致」是**延续会话**（params.resume_session_ref）的约束：
            // 类型不一致无从延续，该约束在选择器侧生效（见 WorkflowNodeConfig 的延续会话候选过滤）。

            // 其他节点引用：预检是否为 start 节点
            if (startNodeIds.has(refNodeId)) {
              continue;
            }
            // 强检查：节点必须存在且为拓扑前序
            if (!allNodeIds.has(refNodeId) || !(nodeUpstream?.has(refNodeId) ?? false)) {
              console.log('[sanitizeMapping] 节点 ' + node.id + '(' + node.label + ') 引用节点 ' + refNodeId + ' 不是拓扑前序节点');
              allValid = false;
              break;
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

  // 2. 替换 mapping 对象中的 ID 引用（节点 ID + 阶段 ID）
  //    新引用格式: {{key.nodeId.stageId}}，两者都需要替换；gate_output 引用中无 nodeId，不替换
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
