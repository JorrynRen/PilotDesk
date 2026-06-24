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
}

const NODE_TYPE_META: Record<WorkflowNodeType, NodeTypeMeta> = {
  agent: { label: 'Agent 任务', color: '#58a6ff', icon: '🤖', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10 },
  api: { label: 'API 调用', color: '#a371f7', icon: '🔗', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10 },
  transform: { label: '代码转换', color: '#d29922', icon: '⚡', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10 },
  interact: { label: '人工交互', color: '#f85149', icon: '👤', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10 },
  plugin: { label: '插件命令', color: '#3fb950', icon: '🧩', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10 },
  start: { label: '起始', color: '#3fb950', icon: '▶', canHaveInputs: false, canHaveOutputs: true, maxInputs: 0, maxOutputs: 10 },
  end: { label: '结束', color: '#f85149', icon: '■', canHaveInputs: true, canHaveOutputs: false, maxInputs: 10, maxOutputs: 0 },
  subflow: { label: '子工作流', color: '#79c0ff', icon: '📦', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10 },
};

export function getNodeTypeMeta(type: WorkflowNodeType): NodeTypeMeta {
  return NODE_TYPE_META[type] || { label: type, color: '#8b949e', icon: '❓', canHaveInputs: true, canHaveOutputs: true, maxInputs: 10, maxOutputs: 10 };
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

// ── 默认工作流 ──

export function createDefaultWorkflow(name: string): WorkflowDefinition {
  const startNodeId = generateId();
  const startStageId = generateStageId();
  const endNodeId = generateId();
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
        nodes: [{
          id: startNodeId,
          type: 'start' as WorkflowNodeType,
          label: '起始',
          position: { x: 60, y: 40 },
          isBoundary: true,
        }],
        edges: [],
        gate: { strategy: 'all', mergeStrategy: 'merge' },
      },
      {
        id: endStageId,
        name: '结束阶段',
        order: 1,
        nodes: [{
          id: endNodeId,
          type: 'end' as WorkflowNodeType,
          label: '结束',
          position: { x: 20, y: 200 },
          isBoundary: true,
        }],
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
      const target = newStages.find(s => s.order === targetOrder);
      if (target) {
        target.nodes.push(movedNode);
      }
    }
  }

  return newStages.filter(s => s.nodes.length > 0 || s.edges.length > 0)
    .map((s, i) => ({ ...s, order: i }));
}


