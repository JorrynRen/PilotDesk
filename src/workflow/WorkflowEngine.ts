/**
 * WorkflowEngine — 工作流引擎
 *
 * 核心执行引擎，管理工作流定义和实例的生命周期。
 * 支持串行/并行执行、条件分支、重试、超时控制。
 *
 * 设计文档: docs/PilotDesk-插件系统架构设计-v2.0.md
 */

import type {
  WorkflowDefinition,
  WorkflowInstance,
  WorkflowNode,
  WorkflowEdge,
  WorkflowInstanceStatus,
  StepExecution,
} from '../types/workflow';
import { validateDefinition } from './WorkflowDefinition';
import { createInstance, updateStepStatus, canTransition, emitWorkflowEvent } from './WorkflowInstance';
import { commandDispatcher } from '../plugin/CommandDispatcher';

// ── 模板变量替换 ──

function resolveTemplate(template: string, context: Record<string, any>): string {
  return template.replace(/\$\{([^}]+)\}/g, (_, key) => {
    const value = key.split('.').reduce((obj: any, k: string) => obj?.[k], context);
    return value !== undefined ? String(value) : `$\{${key}}`;
  });
}

function resolveParams(params: Record<string, any>, context: Record<string, any>): Record<string, any> {
  const resolved: Record<string, any> = {};
  for (const [key, value] of Object.entries(params)) {
    if (typeof value === 'string') {
      resolved[key] = resolveTemplate(value, context);
    } else if (typeof value === 'object' && value !== null) {
      resolved[key] = resolveParams(value, context);
    } else {
      resolved[key] = value;
    }
  }
  return resolved;
}

// ── 工作流引擎 ──

class WorkflowEngine {
  /** 工作流定义 key: definitionId */
  private definitions: Map<string, WorkflowDefinition> = new Map();
  /** 工作流实例 key: instanceId */
  private instances: Map<string, WorkflowInstance> = new Map();
  /** 是否正在执行 */
  private running: Set<string> = new Set();

  // ── 定义管理 ──

  async createDefinition(def: WorkflowDefinition): Promise<string> {
    const errors = validateDefinition(def);
    if (errors.some((e) => e.severity === 'error')) {
      throw new Error(`工作流定义验证失败:\n${errors.map((e) => `  [${e.field}] ${e.message}`).join('\n')}`);
    }
    this.definitions.set(def.id, { ...def, updatedAt: new Date().toISOString() });
    return def.id;
  }

  async updateDefinition(id: string, updates: Partial<WorkflowDefinition>): Promise<void> {
    const existing = this.definitions.get(id);
    if (!existing) throw new Error(`工作流定义未找到: ${id}`);
    const updated = { ...existing, ...updates, updatedAt: new Date().toISOString() };
    const errors = validateDefinition(updated);
    if (errors.some((e) => e.severity === 'error')) {
      throw new Error(`工作流定义验证失败:\n${errors.map((e) => `  [${e.field}] ${e.message}`).join('\n')}`);
    }
    this.definitions.set(id, updated);
  }

  async deleteDefinition(id: string): Promise<void> {
    this.definitions.delete(id);
  }

  async getDefinition(id: string): Promise<WorkflowDefinition | undefined> {
    return this.definitions.get(id);
  }

  async listDefinitions(): Promise<WorkflowDefinition[]> {
    return Array.from(this.definitions.values());
  }

  // ── 执行控制 ──

  async start(definitionId: string, context?: Record<string, unknown>): Promise<string> {
    const def = this.definitions.get(definitionId);
    if (!def) throw new Error(`工作流定义未找到: ${definitionId}`);

    const instance = createInstance(def.id, def.name, 'manual');
    instance.context = { ...context };
    this.instances.set(instance.id, instance);
    emitWorkflowEvent('instance:created', instance.id);

    // 异步执行，不阻塞
    this.executeWorkflow(instance.id, def).catch((err) => {
      console.error(`[WorkflowEngine] 工作流 ${instance.id} 执行失败:`, err);
    });

    return instance.id;
  }

  async pause(instanceId: string): Promise<void> {
    const instance = this.instances.get(instanceId);
    if (!instance) throw new Error(`工作流实例未找到: ${instanceId}`);
    if (!canTransition(instance.status, 'paused')) {
      throw new Error(`无法从 ${instance.status} 状态切换到 paused`);
    }
    instance.status = 'paused';
    emitWorkflowEvent('instance:paused', instanceId);
  }

  async resume(instanceId: string): Promise<void> {
    const instance = this.instances.get(instanceId);
    if (!instance) throw new Error(`工作流实例未找到: ${instanceId}`);
    if (!canTransition(instance.status, 'running')) {
      throw new Error(`无法从 ${instance.status} 状态切换到 running`);
    }
    instance.status = 'running';
    emitWorkflowEvent('instance:resumed', instanceId);
  }

  async stop(instanceId: string): Promise<void> {
    const instance = this.instances.get(instanceId);
    if (!instance) throw new Error(`工作流实例未找到: ${instanceId}`);
    if (!canTransition(instance.status, 'cancelled')) {
      throw new Error(`无法从 ${instance.status} 状态切换到 cancelled`);
    }
    instance.status = 'cancelled';
    instance.completedAt = new Date().toISOString();
    this.running.delete(instanceId);
    emitWorkflowEvent('instance:cancelled', instanceId);
  }

  async retry(instanceId: string, stepId?: string): Promise<void> {
    const instance = this.instances.get(instanceId);
    if (!instance) throw new Error(`工作流实例未找到: ${instanceId}`);

    if (stepId) {
      // 重试指定步骤
      const step = instance.steps[stepId];
      if (step) {
        step.status = 'pending';
        step.error = undefined;
        step.retryCount = 0;
      }
    } else {
      // 重试整个工作流
      for (const step of Object.values(instance.steps)) {
        if (step.status === 'failed') {
          step.status = 'pending';
          step.error = undefined;
        }
      }
    }

    instance.status = 'running';
    instance.error = undefined;
    const def = this.definitions.get(instance.definitionId);
    if (def) {
      this.executeWorkflow(instanceId, def).catch((err) => {
        console.error(`[WorkflowEngine] 重试工作流 ${instanceId} 失败:`, err);
      });
    }
  }

  // ── 状态查询 ──

  async getInstance(instanceId: string): Promise<WorkflowInstance | undefined> {
    return this.instances.get(instanceId);
  }

  async listInstances(filter?: { status?: string; definitionId?: string }): Promise<WorkflowInstance[]> {
    let list = Array.from(this.instances.values());
    if (filter?.status) list = list.filter((i) => i.status === filter.status);
    if (filter?.definitionId) list = list.filter((i) => i.definitionId === filter.definitionId);
    return list.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
  }

  // ── 核心执行逻辑 ──

  private async executeWorkflow(instanceId: string, def: WorkflowDefinition): Promise<void> {
    const instance = this.instances.get(instanceId);
    if (!instance) return;

    if (this.running.has(instanceId)) return;
    this.running.add(instanceId);

    instance.status = 'running';
    instance.startedAt = new Date().toISOString();
    emitWorkflowEvent('instance:started', instanceId);

    try {
      // 构建邻接表
      const adjacency = new Map<string, string[]>();
      const inDegree = new Map<string, number>();
      for (const node of def.nodes) {
        adjacency.set(node.id, []);
        inDegree.set(node.id, 0);
      }
      for (const edge of def.edges) {
        adjacency.get(edge.source)?.push(edge.target);
        inDegree.set(edge.target, (inDegree.get(edge.target) || 0) + 1);
      }

      // 找到起始节点（入度为 0 的节点）
      const startNodes = def.nodes.filter((n) => (inDegree.get(n.id) || 0) === 0);

      // 从起始节点开始执行
      for (const startNode of startNodes) {
        if (instance.status !== 'running') break;
        await this.executeNode(instanceId, def, startNode.id, adjacency);
      }

      // 检查是否所有节点都执行完成
      const allDone = def.nodes.every((n) => {
        const step = instance.steps[n.id];
        return step && (step.status === 'success' || step.status === 'skipped');
      });

      if (allDone && instance.status === 'running') {
        instance.status = 'success';
        instance.completedAt = new Date().toISOString();
        emitWorkflowEvent('instance:completed', instanceId);
      }
    } catch (err: any) {
      instance.status = 'failed';
      instance.error = err.message || String(err);
      instance.completedAt = new Date().toISOString();
      emitWorkflowEvent('instance:failed', instanceId, undefined, { error: instance.error });
    } finally {
      this.running.delete(instanceId);
    }
  }

  private async executeNode(
    instanceId: string,
    def: WorkflowDefinition,
    nodeId: string,
    adjacency: Map<string, string[]>,
    visited: Set<string> = new Set(),
  ): Promise<void> {
    const instance = this.instances.get(instanceId);
    if (!instance || instance.status !== 'running') return;
    if (visited.has(nodeId)) return;
    visited.add(nodeId);

    const node = def.nodes.find((n) => n.id === nodeId);
    if (!node) return;

    // 更新步骤状态
    instance.currentNodeId = nodeId;
    updateStepStatus(instance, nodeId, 'running');
    emitWorkflowEvent('step:started', instanceId, nodeId);

    try {
      let result: any;

      switch (node.type) {
        case 'trigger:cron':
        case 'trigger:event':
        case 'trigger:manual':
          // 触发节点：直接通过
          result = { triggered: true };
          break;

        case 'plugin:command': {
          // 插件命令节点
          if (!node.pluginId || !node.commandId) {
            throw new Error(`节点 "${node.label}" 缺少插件 ID 或命令 ID`);
          }

          // 解析参数
          const params = node.params ? resolveParams(node.params, instance.context) : {};

          // 通过 CommandDispatcher 执行
          const cmdResult = await commandDispatcher.execute(
            node.pluginId,
            node.commandId,
            params,
            { timeout: node.timeoutMs },
          );

          if (!cmdResult.success) {
            throw new Error(cmdResult.error || '命令执行失败');
          }

          result = cmdResult.data;

          // 输出映射
          if (node.outputMapping) {
            for (const [key, path] of Object.entries(node.outputMapping)) {
              instance.context[path] = result?.[key];
            }
          }
          break;
        }

        case 'condition': {
          // 条件判断节点
          if (!node.condition) throw new Error('条件节点缺少条件表达式');
          const conditionResult = resolveTemplate(node.condition, instance.context);
          result = { condition: conditionResult === 'true' };
          break;
        }

        case 'delay': {
          // 延迟等待节点
          const delayMs = node.delayMs || 1000;
          await new Promise((resolve) => setTimeout(resolve, delayMs));
          result = { delayed: true, duration: delayMs };
          break;
        }

        case 'parallel': {
          // 并行执行：找到所有直接后继，并行执行
          const successors = adjacency.get(nodeId) || [];
          await Promise.all(
            successors.map((succId) =>
              this.executeNode(instanceId, def, succId, adjacency, new Set(visited)),
            ),
          );
          result = { parallel: true };
          break;
        }

        default:
          result = { skipped: true };
      }

      updateStepStatus(instance, nodeId, 'success', { output: result });
      emitWorkflowEvent('step:completed', instanceId, nodeId, { output: result });

      // 继续执行后继节点
      const successors = adjacency.get(nodeId) || [];
      for (const succId of successors) {
        if (instance.status !== 'running') break;
        await this.executeNode(instanceId, def, succId, adjacency, visited);
      }
    } catch (err: any) {
      const errorMsg = err.message || String(err);

      // 重试逻辑
      if (node.retryCount && node.retryCount > 0) {
        const step = instance.steps[nodeId];
        const currentRetry = (step?.retryCount || 0) + 1;

        if (currentRetry <= node.retryCount) {
          updateStepStatus(instance, nodeId, 'retrying', { retryCount: currentRetry, error: errorMsg });
          emitWorkflowEvent('step:retrying', instanceId, nodeId, { retryCount: currentRetry, error: errorMsg });

          // 等待重试间隔
          const delayMs = node.retryDelayMs || 1000;
          await new Promise((resolve) => setTimeout(resolve, delayMs));

          // 重试
          return this.executeNode(instanceId, def, nodeId, adjacency, visited);
        }
      }

      updateStepStatus(instance, nodeId, 'failed', { error: errorMsg });
      emitWorkflowEvent('step:failed', instanceId, nodeId, { error: errorMsg });

      // 节点失败，整个工作流失败
      instance.status = 'failed';
      instance.error = `节点 "${node.label}" 执行失败: ${errorMsg}`;
      instance.completedAt = new Date().toISOString();
      emitWorkflowEvent('instance:failed', instanceId);
    }
  }
}

/** 全局单例 */
export const workflowEngine = new WorkflowEngine();
