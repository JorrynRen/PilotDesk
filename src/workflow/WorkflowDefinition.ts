/**
 * WorkflowDefinition — 工作流定义验证工具
 *
 * 验证工作流定义的完整性和正确性。
 * 检测循环引用、缺失节点、类型兼容性等。
 */

import type { WorkflowDefinition, WorkflowNode, WorkflowEdge, WorkflowNodeType } from '../types/workflow';

// ── 验证结果 ──

export interface ValidationError {
  field: string;
  message: string;
  severity: 'error' | 'warning';
  nodeId?: string;
  edgeId?: string;
}

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
  'trigger:cron': { label: '定时触发', color: '#10B981', icon: '⏰', canHaveInputs: false, canHaveOutputs: true, maxInputs: 0, maxOutputs: 1 },
  'trigger:event': { label: '事件触发', color: '#3B82F6', icon: '📡', canHaveInputs: false, canHaveOutputs: true, maxInputs: 0, maxOutputs: 1 },
  'trigger:manual': { label: '手动触发', color: '#8B5CF6', icon: '▶️', canHaveInputs: false, canHaveOutputs: true, maxInputs: 0, maxOutputs: 1 },
  'plugin:command': { label: '插件命令', color: '#F59E0B', icon: '⚡', canHaveInputs: true, canHaveOutputs: true, maxInputs: 1, maxOutputs: 1 },
  'condition': { label: '条件判断', color: '#EF4444', icon: '🔀', canHaveInputs: true, canHaveOutputs: true, maxInputs: 1, maxOutputs: 2 },
  'parallel': { label: '并行执行', color: '#6366F1', icon: '📋', canHaveInputs: true, canHaveOutputs: true, maxInputs: 1, maxOutputs: 0 },
  'delay': { label: '延迟等待', color: '#6B7280', icon: '⏳', canHaveInputs: true, canHaveOutputs: true, maxInputs: 1, maxOutputs: 1 },
  'approval': { label: '人工审批', color: '#EC4899', icon: '✅', canHaveInputs: true, canHaveOutputs: true, maxInputs: 1, maxOutputs: 2 },
  'human_input': { label: '人工介入', color: '#F97316', icon: '✋', canHaveInputs: true, canHaveOutputs: true, maxInputs: 1, maxOutputs: 1 },
  'plugin:node': { label: '插件节点', color: '#A855F7', icon: '🧩', canHaveInputs: true, canHaveOutputs: true, maxInputs: 1, maxOutputs: 1 },
  'subflow': { label: '子工作流', color: '#14B8A6', icon: '🔗', canHaveInputs: true, canHaveOutputs: true, maxInputs: 1, maxOutputs: 1 },
};

export function getNodeTypeMeta(type: WorkflowNodeType): NodeTypeMeta {
  return NODE_TYPE_META[type];
}

// ── 验证器 ──

export function validateDefinition(def: WorkflowDefinition): ValidationError[] {
  const errors: ValidationError[] = [];

  if (!def.id) errors.push({ field: 'id', message: '工作流 ID 不能为空', severity: 'error' });
  if (!def.name) errors.push({ field: 'name', message: '工作流名称不能为空', severity: 'error' });
  if (!def.version) errors.push({ field: 'version', message: '版本号不能为空', severity: 'error' });

  if (def.nodes.length === 0) {
    errors.push({ field: 'nodes', message: '工作流至少需要一个节点', severity: 'error' });
    return errors;
  }

  const nodeMap = new Map<string, WorkflowNode>();
  for (const node of def.nodes) {
    nodeMap.set(node.id, node);
    if (node.type === 'plugin:command') {
      if (!node.pluginId) errors.push({ field: 'pluginId', message: `节点 "${node.label}" 缺少插件 ID`, severity: 'error', nodeId: node.id });
      if (!node.commandId) errors.push({ field: 'commandId', message: `节点 "${node.label}" 缺少命令 ID`, severity: 'error', nodeId: node.id });
    }
    if (node.type === 'trigger:cron' && !node.cron) {
      errors.push({ field: 'cron', message: `定时触发节点 "${node.label}" 缺少 cron 表达式`, severity: 'error', nodeId: node.id });
    }
    if (node.type === 'delay' && !node.delayMs) {
      errors.push({ field: 'delayMs', message: `延迟节点 "${node.label}" 缺少延迟时间`, severity: 'error', nodeId: node.id });
    }
  }

  for (const edge of def.edges) {
    if (!nodeMap.has(edge.source)) {
      errors.push({ field: 'source', message: `边 "${edge.id}" 引用了不存在的源节点 "${edge.source}"`, severity: 'error', edgeId: edge.id });
    }
    if (!nodeMap.has(edge.target)) {
      errors.push({ field: 'target', message: `边 "${edge.id}" 引用了不存在的目标节点 "${edge.target}"`, severity: 'error', edgeId: edge.id });
    }
  }

  const cycle = detectCycle(def.nodes, def.edges);
  if (cycle) {
    errors.push({ field: 'edges', message: `检测到循环依赖: ${cycle.join(' → ')}`, severity: 'error' });
  }

  const connectedNodes = new Set<string>();
  for (const edge of def.edges) {
    connectedNodes.add(edge.source);
    connectedNodes.add(edge.target);
  }
  for (const node of def.nodes) {
    if (!connectedNodes.has(node.id) && def.nodes.length > 1) {
      errors.push({ field: 'nodes', message: `节点 "${node.label}" 未连接到任何其他节点`, severity: 'warning', nodeId: node.id });
    }
  }

  return errors;
}

function detectCycle(nodes: WorkflowNode[], edges: WorkflowEdge[]): string[] | null {
  const adjacency = new Map<string, string[]>();
  for (const node of nodes) adjacency.set(node.id, []);
  for (const edge of edges) adjacency.get(edge.source)?.push(edge.target);

  const visited = new Set<string>();
  const recStack = new Set<string>();
  const path: string[] = [];

  function dfs(nodeId: string): boolean {
    visited.add(nodeId);
    recStack.add(nodeId);
    path.push(nodeId);
    for (const neighbor of adjacency.get(nodeId) || []) {
      if (!visited.has(neighbor)) { if (dfs(neighbor)) return true; }
      else if (recStack.has(neighbor)) {
        const cycleStart = path.indexOf(neighbor);
        path.splice(0, cycleStart);
        path.push(neighbor);
        return true;
      }
    }
    path.pop();
    recStack.delete(nodeId);
    return false;
  }

  for (const node of nodes) {
    if (!visited.has(node.id)) {
      path.length = 0;
      if (dfs(node.id)) return [...path];
    }
  }
  return null;
}

export function generateId(): string {
  return 'wf_' + Date.now().toString(36) + '_' + Math.random().toString(36).substring(2, 8);
}

export function createEmptyDefinition(name?: string): WorkflowDefinition {
  const now = new Date().toISOString();
  return {
    id: generateId(),
    name: name || '新工作流',
    version: '1.0',
    description: '',
    nodes: [],
    edges: [],
    createdAt: now,
    updatedAt: now,
    enabled: false,
  };
}
