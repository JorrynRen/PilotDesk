/**

 * workflowStore — 工作流状态管理

 *

 * Zustand store，管理工作流定义列表和实例列表。

 */



import { create } from 'zustand';

import { invoke } from '@tauri-apps/api/core';

import type { WorkflowDefinition, WorkflowInstance, PendingHumanInput } from '../types/workflow';



interface WorkflowSchedule {

  id: string;

  workflowId: string;

  cronExpression: string;

  enabled: boolean;

  inputData: string;

  lastRunAt?: number;

  nextRunAt?: number;

  createdAt: number;

}



interface WorkflowStoreState {

  definitions: WorkflowDefinition[];

  instances: WorkflowInstance[];
  pendingInputs: PendingHumanInput[];

  selectedDefinitionId: string | null;

  selectedInstanceId: string | null;

  loading: boolean;

  error: string | null;



  // 定义管理

  loadDefinitions: () => Promise<void>;

  createDefinition: (def: WorkflowDefinition) => Promise<string>;

  updateDefinition: (id: string, updates: Partial<WorkflowDefinition>) => Promise<void>;

  deleteDefinition: (id: string) => Promise<void>;
  duplicateDefinition: (id: string, newName: string) => Promise<void>;

  selectDefinition: (id: string | null) => void;



  // 实例管理

  /**
   * @param definitionId 可选：仅拉取某个工作流下的实例
   * @param silent       静默刷新：不显示 loading 状态，避免列表区域闪白。
   *                     true = 后台静默刷新（用于定时器轮询 / 手动快速刷新）
   *                     false（默认）= 首次加载或显式展示 loading
   */
  loadInstances: (definitionId?: string, silent?: boolean) => Promise<void>;

  loadSchedules: () => Promise<WorkflowSchedule[]>;

  syncSchedule: (workflowId: string, trigger: { triggerType: string; cron?: string }) => Promise<void>;

  startWorkflow: (definitionId: string, context?: Record<string, unknown>, instanceId?: string) => Promise<string>;

  /** 安全启动工作流：自动清理旧的 running 实例后再执行 */

  safeStartWorkflow: (definitionId: string, context?: Record<string, unknown>, instanceId?: string) => Promise<string>;

  cancelWorkflow: (executionId: string) => Promise<void>;
  loadPendingInputs: () => Promise<void>;
  respondHumanInput: (executionId: string, nodeId: string, response: string) => Promise<void>;

  deleteExecution: (executionId: string) => Promise<void>;

  selectInstance: (id: string | null) => void;

}



export const useWorkflowStore = create<WorkflowStoreState>((set, get) => ({

  definitions: [],

  instances: [],
  pendingInputs: [],

  schedules: [] as WorkflowSchedule[],

  selectedDefinitionId: null,

  selectedInstanceId: null,

  loading: false,

  error: null,



  loadDefinitions: async () => {

    set({ loading: true, error: null });

    try {

      const definitions = await invoke<WorkflowDefinition[]>('list_workflows');

      set({ definitions, loading: false });

    } catch (err) {

      set({ error: String(err), loading: false });

    }

  },



  createDefinition: async (def: WorkflowDefinition) => {

    set({ loading: true, error: null });

    try {

      await invoke('save_workflow_definition', { definition: def });

      // 同步定时调度

      if (def.trigger) {

        await get().syncSchedule(def.id, def.trigger);

      }

      await get().loadDefinitions();

      set({ loading: false });

      return def.id;

    } catch (err) {

      set({ error: String(err), loading: false });

      throw err;

    }

  },



  updateDefinition: async (id: string, updates: Partial<WorkflowDefinition>) => {

    set({ loading: true, error: null });

    try {

      const defs = get().definitions;

      const existing = defs.find(d => d.id === id);

      if (existing) {

        const merged = { ...existing, ...updates };

        await invoke('save_workflow_definition', { definition: merged });

        // 同步定时调度

        if (merged.trigger) {

          await get().syncSchedule(id, merged.trigger);

        }

      }

      await get().loadDefinitions();

      set({ loading: false });

    } catch (err) {

      set({ error: String(err), loading: false });

    }

  },



  deleteDefinition: async (id: string) => {

    set({ loading: true, error: null });

    try {

      // 同步删除定时调度

      await get().syncSchedule(id, { triggerType: 'manual' });

      await invoke('delete_workflow', { id });

      await get().loadDefinitions();

      set({ loading: false });

    } catch (err) {

      set({ error: String(err), loading: false });

    }

  },

  duplicateDefinition: async (id: string, newName: string) => {
    set({ loading: true, error: null });
    try {
      await invoke('duplicate_workflow', { id, newName });
      await get().loadDefinitions();
      set({ loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },



  selectDefinition: (id: string | null) => {

    set({ selectedDefinitionId: id });

  },



  loadInstances: async (definitionId?: string, silent?: boolean) => {

    // 静默刷新不写 loading=true，避免列表闪白 / 展开状态丢失 / 滚动位置重置
    if (!silent) {
      set({ loading: true, error: null });
    }

    try {

      const instances = await invoke<WorkflowInstance[]>('list_executions', { definitionId: definitionId || null });

      set({ instances, loading: false });

    } catch (err) {

      set({ error: String(err), loading: false });

    }

  },



  loadSchedules: async () => {

    try {

      const schedules = await invoke<WorkflowSchedule[]>('list_schedules');

      set({ schedules });

      return schedules;

    } catch (err) {

      console.warn('[workflowStore] loadSchedules 失败:', err);

      return [];

    }

  },



  syncSchedule: async (workflowId: string, trigger: { triggerType: string; cron?: string }) => {

    try {

      // Delete existing schedule if any

      const existing = (await invoke<WorkflowSchedule[]>('list_schedules'))

        .filter(s => s.workflowId === workflowId);

      for (const s of existing) {

        await invoke('delete_schedule', { scheduleId: s.id });

      }

      // Create new schedule if cron

      if (trigger.triggerType === 'cron' && trigger.cron) {

        await invoke('create_schedule', {

          workflowId,

          cronExpression: trigger.cron,

          inputData: null,

        });

      }

      await get().loadSchedules();

    } catch (err) {

      console.warn('[workflowStore] syncSchedule 失败:', err);

    }

  },



  startWorkflow: async (definitionId: string, context?: Record<string, unknown>, instanceId?: string) => {

    const instance = await invoke<WorkflowInstance>('start_workflow', {

      workflowId: definitionId,

      version: null,

      inputData: context || null,

      instanceId: instanceId || null,

    });

    await get().loadInstances();

    return instance.id;

  },



  safeStartWorkflow: async (definitionId: string, context?: Record<string, unknown>, instanceId?: string) => {

    // 1. 主动清理该工作流所有旧的 running 实例

    const runningInstances = get().instances.filter(

      inst => inst.definitionId === definitionId && inst.status === 'running'

    );

    for (const inst of runningInstances) {

      console.log('[workflowStore] safeStartWorkflow: 清理旧的 running 实例:', inst.id);

      try {

        await invoke('cancel_workflow', { executionId: inst.id });

      } catch (e) {

        console.warn('[workflowStore] 取消旧实例失败（可忽略）:', e);

      }

    }

    // 2. 启动新执行

    return get().startWorkflow(definitionId, context, instanceId);

  },



  cancelWorkflow: async (executionId: string) => {

    await invoke('cancel_workflow', { executionId });

    await get().loadInstances();

  },

  loadPendingInputs: async () => {
    try {
      const inputs = await invoke<PendingHumanInput[]>('get_pending_human_inputs');
      set({ pendingInputs: inputs });
    } catch {
      set({ pendingInputs: [] });
    }
  },

  deleteExecution: async (executionId: string) => {

    await invoke('delete_execution', { executionId });

    await get().loadInstances();

  },



  respondHumanInput: async (executionId: string, nodeId: string, response: string) => {
    try {
      await invoke('respond_human_input', { executionId, nodeId, response });
      await get().loadPendingInputs();
      await get().loadInstances();
    } catch (err: any) {
      console.error('响应人工输入失败:', err);
    }
  },



  selectInstance: (id: string | null) => {

    set({ selectedInstanceId: id });

  },

  /** 单点执行：仅执行选中节点 */
  executeSingleNode: async (executionId: string, nodeId: string) => {
    await invoke('execute_workflow_mode', { executionId, mode: 'single', nodeId });
    await get().loadInstances();
  },

  /** 链式执行：从选中节点到后序链路末端 */
  executeChain: async (executionId: string, nodeId: string) => {
    await invoke('execute_workflow_mode', { executionId, mode: 'chain', nodeId });
    await get().loadInstances();
  },

  /** 补全执行：跳过已完成节点，执行未完成节点 */
  executeCompletion: async (executionId: string) => {
    await invoke('execute_workflow_mode', { executionId, mode: 'completion' });
    await get().loadInstances();
  },

}));

