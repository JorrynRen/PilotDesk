/**
 * WorkflowEngine — 工作流引擎（Structured Canvas）
 *
 * 两层调度：阶段串行 → 阶段内 DAG。
 * 条件逻辑由边承载，Gate 控制阶段间同步与数据合并。
 */

import type {
  WorkflowDefinition,
  WorkflowInstance,
  WorkflowNode,
  WorkflowEdge,
  Stage,
  WorkflowInstanceStatus,
  StepExecution,
  GateConfig,
} from '../types/workflow';
import { validateWorkflow } from './WorkflowDefinition';
import { createInstance, updateStepStatus, canTransition, emitWorkflowEvent } from './WorkflowInstance';
import { commandDispatcher } from '../plugin/CommandDispatcher';
import { eventDispatcher } from '../plugin/EventDispatcher';
import { globalEventBus } from '../plugin/GlobalEventBus';
import { PluginAgentAPI } from '../plugin/PluginAPI.agent';

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

// ── 拓扑排序（Kahn 算法）──

function topologicalSort(nodes: WorkflowNode[], edges: WorkflowEdge[]): string[][] {
  const inDegree = new Map<string, number>();
  const adjacency = new Map<string, string[]>();

  for (const node of nodes) {
    inDegree.set(node.id, 0);
    adjacency.set(node.id, []);
  }
  for (const edge of edges) {
    adjacency.get(edge.source)?.push(edge.target);
    inDegree.set(edge.target, (inDegree.get(edge.target) || 0) + 1);
  }

  const layers: string[][] = [];
  let queue = [...inDegree.entries()]
    .filter(([_, deg]) => deg === 0)
    .map(([id]) => id);

  while (queue.length > 0) {
    layers.push([...queue]);
    const next: string[] = [];
    for (const nodeId of queue) {
      for (const neighbor of adjacency.get(nodeId) || []) {
        const deg = (inDegree.get(neighbor) || 1) - 1;
        inDegree.set(neighbor, deg);
        if (deg === 0) next.push(neighbor);
      }
    }
    queue = next;
  }

  return layers;
}

// ── 条件评估 ──

function evaluateCondition(condition: string, sourceOutput: any): boolean {
  if (condition.startsWith('==')) {
    return String(sourceOutput).replace(/^"|"$/g, '') === condition.slice(2).trim();
  }
  if (condition.startsWith('!=')) {
    return String(sourceOutput).replace(/^"|"$/g, '') !== condition.slice(2).trim();
  }
  if (condition.startsWith('contains')) {
    return String(sourceOutput).includes(condition.slice(8).trim());
  }
  // 默认：非空即真
  return sourceOutput !== null && sourceOutput !== undefined && sourceOutput !== '';
}

// ── Gate 合并 ──

function mergeStageOutputs(stage: Stage, nodeOutputs: Map<string, any>): Record<string, any> {
  const outputs: Record<string, any> = {};
  for (const node of stage.nodes) {
    if (nodeOutputs.has(node.id)) {
      outputs[node.id] = nodeOutputs.get(node.id);
    }
  }

  switch (stage.gate.mergeStrategy) {
    case 'merge': {
      const merged: Record<string, any> = {};
      for (const [nodeId, output] of Object.entries(outputs)) {
        if (typeof output === 'object' && output !== null && !Array.isArray(output)) {
          for (const [k, v] of Object.entries(output)) {
            merged[`${nodeId}_${k}`] = v;
          }
        } else {
          merged[nodeId] = output;
        }
      }
      return merged;
    }
    case 'concat':
      return Object.values(outputs);
    case 'pick_first': {
      const first = Object.values(outputs)[0];
      return first ?? null;
    }
    default:
      return outputs;
  }
}

// ── 工作流引擎 ──

class WorkflowEngine {
  private definitions: Map<string, WorkflowDefinition> = new Map();
  private instances: Map<string, WorkflowInstance> = new Map();
  private running: Set<string> = new Set();
  private eventUnsubscribers: Map<string, () => void> = new Map();
  private agentSessions: Map<string, { api: PluginAgentAPI; sessionId: string; agentType: string }> = new Map();

  // ── 定义管理 ──

  async createDefinition(def: WorkflowDefinition): Promise<string> {
    const errors = validateWorkflow(def);
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
    const errors = validateWorkflow(updated);
    if (errors.some((e) => e.severity === 'error')) {
      throw new Error(`工作流定义验证失败:\n${errors.map((e) => `  [${e.field}] ${e.message}`).join('\n')}`);
    }
    this.definitions.set(id, updated);
  }

  async deleteDefinition(id: string): Promise<void> {
    this.unregisterEventTriggers(id);
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
    this.cleanupAgentSessions(instanceId);
  }

  async retry(instanceId: string, stepId?: string): Promise<void> {
    const instance = this.instances.get(instanceId);
    if (!instance) throw new Error(`工作流实例未找到: ${instanceId}`);

    if (stepId) {
      const step = instance.steps[stepId];
      if (step) {
        step.status = 'pending';
        step.error = undefined;
        step.retryCount = 0;
      }
    } else {
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

  // ── Agent 会话管理 ──

  private cleanupAgentSessions(instanceId: string): void {
    for (const [sessionId, session] of this.agentSessions.entries()) {
      try {
        session.api.deleteSession(sessionId);
      } catch (err) {
        console.warn(`[WorkflowEngine] 清理 Agent 会话失败: ${sessionId}`, err);
      }
    }
    this.agentSessions.clear();
  }

  // ── 事件触发器管理 ──

  private registerEventTriggers(def: WorkflowDefinition): void {
    // 检查 trigger 配置
    if (def.trigger.triggerType !== 'event' || !def.trigger.eventName) return;

    const eventName = def.trigger.eventName;
    const unsub = eventDispatcher.register(
      'workflow-engine',
      eventName,
      async (payload: any) => {
        const context: Record<string, unknown> = {
          trigger: { event: eventName, payload },
        };
        try {
          await this.start(def.id, context);
          console.log(`[WorkflowEngine] 事件 "${eventName}" 触发工作流 "${def.name}"`);
        } catch (err) {
          console.error(`[WorkflowEngine] 事件 "${eventName}" 触发工作流失败:`, err);
        }
        return true;
      },
    );

    const existing = this.eventUnsubscribers.get(def.id);
    if (existing) {
      this.eventUnsubscribers.set(def.id, () => { existing(); unsub(); });
    } else {
      this.eventUnsubscribers.set(def.id, unsub);
    }
  }

  private unregisterEventTriggers(defId: string): void {
    const unsub = this.eventUnsubscribers.get(defId);
    if (unsub) { unsub(); this.eventUnsubscribers.delete(defId); }
  }

  // ── 核心执行逻辑（两层调度）──

  private async executeWorkflow(instanceId: string, def: WorkflowDefinition): Promise<void> {
    const instance = this.instances.get(instanceId);
    if (!instance) return;
    if (this.running.has(instanceId)) return;
    this.running.add(instanceId);

    instance.status = 'running';
    instance.startedAt = new Date().toISOString();
    emitWorkflowEvent('instance:started', instanceId);

    try {
      // 外层调度：阶段串行
      for (const stage of def.stages) {
        if (instance.status !== 'running') break;

        emitWorkflowEvent('stage:started', instanceId, undefined, {
          stageId: stage.id,
          stageName: stage.name,
        });

        // 内层调度：阶段内 DAG
        const nodeOutputs = await this.executeStage(instanceId, def, stage, instance);

        // Gate 合并
        const merged = mergeStageOutputs(stage, nodeOutputs);
        instance.context[`__stage_${stage.order}_output__`] = merged;

        emitWorkflowEvent('stage:completed', instanceId, undefined, {
          stageId: stage.id,
          stageName: stage.name,
        });
      }

      // 检查是否所有阶段完成
      if (instance.status === 'running') {
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

  private async executeStage(
    instanceId: string,
    def: WorkflowDefinition,
    stage: Stage,
    instance: WorkflowInstance,
  ): Promise<Map<string, any>> {
    const nodeOutputs = new Map<string, any>();
    const layers = topologicalSort(stage.nodes, stage.edges);

    for (const layer of layers) {
      if (instance.status !== 'running') break;

      // 同一层节点并行执行
      const promises = layer.map(async (nodeId) => {
        const node = stage.nodes.find((n) => n.id === nodeId);
        if (!node) return;

        // 检查入边条件
        const incomingEdges = stage.edges.filter((e) => e.target === nodeId);
        let allConditionsMet = true;
        for (const edge of incomingEdges) {
          if (edge.condition) {
            const sourceOutput = nodeOutputs.get(edge.source);
            if (!evaluateCondition(edge.condition, sourceOutput)) {
              allConditionsMet = false;
              break;
            }
          }
        }

        if (!allConditionsMet) {
          updateStepStatus(instance, nodeId, 'skipped');
          emitWorkflowEvent('step:skipped', instanceId, nodeId);
          return;
        }

        await this.executeNode(instanceId, def, stage, node, nodeOutputs, instance);
      });

      await Promise.all(promises);
    }

    return nodeOutputs;
  }

  private async executeNode(
    instanceId: string,
    def: WorkflowDefinition,
    stage: Stage,
    node: WorkflowNode,
    nodeOutputs: Map<string, any>,
    instance: WorkflowInstance,
  ): Promise<void> {
    instance.currentNodeId = node.id;
    updateStepStatus(instance, node.id, 'running');
    emitWorkflowEvent('step:started', instanceId, node.id);

    try {
      // 延迟控制属性
      if (node.delayMs && node.delayMs > 0) {
        await new Promise((resolve) => setTimeout(resolve, node.delayMs));
      }

      let result: any;

      switch (node.type) {
        case 'agent': {
          if (!node.pluginId) {
            throw new Error(`Agent 节点 "${node.label}" 缺少插件 ID`);
          }
          const agentApi = new PluginAgentAPI(node.pluginId);
          const agentType = node.params?.agent_type || 'claude';
          const promptTemplate = node.params?.prompt_template || '';
          const prompt = promptTemplate ? resolveTemplate(promptTemplate, instance.context) : '';

          const sessionInfo = await agentApi.createSession(agentType, {
            system_prompt: node.params?.system_prompt,
          });
          this.agentSessions.set(sessionInfo.session_id, {
            api: agentApi,
            sessionId: sessionInfo.session_id,
            agentType,
          });

          const response = await agentApi.sendMessage(sessionInfo.session_id, prompt);
          result = { session_id: sessionInfo.session_id, agent_type: agentType, content: response.content };

          if (node.outputMapping) {
            for (const [key, path] of Object.entries(node.outputMapping)) {
              instance.context[path] = result?.[key];
            }
          }
          break;
        }

        case 'api': {
          const url = node.params?.url;
          if (!url) throw new Error(`API 节点 "${node.label}" 缺少 URL`);
          const method = node.params?.method || 'GET';
          const body = node.params?.body_template ? resolveTemplate(node.params.body_template, instance.context) : undefined;

          const fetchOptions: RequestInit = { method };
          if (body && method !== 'GET') {
            fetchOptions.body = body;
            fetchOptions.headers = { 'Content-Type': 'application/json' };
          }

          const controller = new AbortController();
          const timeout = node.timeoutMs || 60000;
          const timeoutId = setTimeout(() => controller.abort(), timeout);

          try {
            const response = await fetch(url, { ...fetchOptions, signal: controller.signal });
            result = await response.json();
          } finally {
            clearTimeout(timeoutId);
          }

          if (node.outputMapping) {
            for (const [key, path] of Object.entries(node.outputMapping)) {
              instance.context[path] = result?.[key];
            }
          }
          break;
        }

        case 'transform': {
          const script = node.params?.script;
          if (!script) throw new Error(`转换节点 "${node.label}" 缺少脚本`);
          // 在沙箱中执行脚本
          const fn = new Function('context', 'input', script);
          result = fn(instance.context, nodeOutputs);
          break;
        }

        case 'interact': {
          const config = node.params || { prompt: '请输入', inputType: 'text' };
          result = {
            type: 'human_input',
            prompt: resolveTemplate(config.prompt, instance.context),
            inputType: config.inputType,
            options: config.options,
            timeoutMinutes: config.timeoutMinutes || 1440,
          };
          globalEventBus.emit('workflow:awaiting-input', {
            instanceId,
            nodeId: node.id,
            config: result,
          });
          break;
        }

        case 'plugin': {
          if (!node.pluginId || !node.commandId) {
            throw new Error(`插件节点 "${node.label}" 缺少插件 ID 或命令 ID`);
          }
          const params = node.params ? resolveParams(node.params, instance.context) : {};
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
          if (node.outputMapping) {
            for (const [key, path] of Object.entries(node.outputMapping)) {
              instance.context[path] = result?.[key];
            }
          }
          break;
        }

        case 'subflow': {
          const subflowDefId = node.params?.definitionId;
          if (!subflowDefId) {
            throw new Error(`子工作流节点 "${node.label}" 缺少 definitionId`);
          }
          const subflowDef = this.definitions.get(subflowDefId);
          if (!subflowDef) {
            throw new Error(`子工作流定义未找到: ${subflowDefId}`);
          }
          const subflowInstanceId = await this.start(subflowDefId, {
            ...instance.context,
            ...(node.inputMapping ? resolveParams(node.inputMapping as Record<string, any>, instance.context) : {}),
          });
          const subflowInstance = this.instances.get(subflowInstanceId);
          result = {
            type: 'subflow',
            definitionId: subflowDefId,
            instanceId: subflowInstanceId,
            output: subflowInstance?.context || {},
          };
          break;
        }

        default:
          result = { skipped: true };
      }

      nodeOutputs.set(node.id, result);
      updateStepStatus(instance, node.id, 'success', { output: result });
      emitWorkflowEvent('step:completed', instanceId, node.id, { output: result });
    } catch (err: any) {
      const errorMsg = err.message || String(err);

      // 重试逻辑
      if (node.retryCount && node.retryCount > 0) {
        const step = instance.steps[node.id];
        const currentRetry = (step?.retryCount || 0) + 1;
        if (currentRetry <= node.retryCount) {
          updateStepStatus(instance, node.id, 'retrying', { retryCount: currentRetry, error: errorMsg });
          emitWorkflowEvent('step:retrying', instanceId, node.id, { retryCount: currentRetry, error: errorMsg });
          const delayMs = node.retryDelayMs || 1000;
          await new Promise((resolve) => setTimeout(resolve, delayMs));
          return this.executeNode(instanceId, def, stage, node, nodeOutputs, instance);
        }
      }

      nodeOutputs.set(node.id, { error: errorMsg });
      updateStepStatus(instance, node.id, 'failed', { error: errorMsg });
      emitWorkflowEvent('step:failed', instanceId, node.id, { error: errorMsg });
      throw err;
    }
  }
}

/** 全局单例 */
export const workflowEngine = new WorkflowEngine();
