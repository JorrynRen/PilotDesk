/**
 * workflowStore — 工作流状态管理
 *
 * Zustand store，管理工作流定义列表和实例列表。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { WorkflowDefinition, WorkflowInstance } from '../types/workflow';

interface WorkflowStoreState {
  definitions: WorkflowDefinition[];
  instances: WorkflowInstance[];
  selectedDefinitionId: string | null;
  selectedInstanceId: string | null;
  loading: boolean;
  error: string | null;

  // 定义管理
  loadDefinitions: () => Promise<void>;
  createDefinition: (def: WorkflowDefinition) => Promise<string>;
  updateDefinition: (id: string, updates: Partial<WorkflowDefinition>) => Promise<void>;
  deleteDefinition: (id: string) => Promise<void>;
  selectDefinition: (id: string | null) => void;

  // 实例管理
  loadInstances: (definitionId?: string) => Promise<void>;
  startWorkflow: (definitionId: string, context?: Record<string, unknown>) => Promise<string>;
  /** 安全启动工作流：自动清理旧的 running 实例后再执行 */
  safeStartWorkflow: (definitionId: string, context?: Record<string, unknown>) => Promise<string>;
  cancelWorkflow: (executionId: string) => Promise<void>;
  deleteExecution: (executionId: string) => Promise<void>;
  respondHumanInput: (executionId: string, nodeId: string, response: string) => Promise<void>;
  selectInstance: (id: string | null) => void;
}

export const useWorkflowStore = create<WorkflowStoreState>((set, get) => ({
  definitions: [],
  instances: [],
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
        await invoke('save_workflow_definition', { definition: { ...existing, ...updates } });
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
      await invoke('delete_workflow', { id });
      await get().loadDefinitions();
      set({ loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  selectDefinition: (id: string | null) => {
    set({ selectedDefinitionId: id });
  },

  loadInstances: async (definitionId?: string) => {
    set({ loading: true, error: null });
    try {
      const instances = await invoke<WorkflowInstance[]>('list_executions', { definitionId: definitionId || null });
      set({ instances, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  startWorkflow: async (definitionId: string, context?: Record<string, unknown>) => {
    const instance = await invoke<WorkflowInstance>('start_workflow', {
      workflowId: definitionId,
      version: null,
      inputData: context || null,
    });
    await get().loadInstances();
    return instance.id;
  },

  safeStartWorkflow: async (definitionId: string, context?: Record<string, unknown>) => {
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
    return get().startWorkflow(definitionId, context);
  },

  cancelWorkflow: async (executionId: string) => {
    await invoke('cancel_workflow', { executionId });
    await get().loadInstances();
  },

  deleteExecution: async (executionId: string) => {
    await invoke('delete_execution', { executionId });
    await get().loadInstances();
  },

  respondHumanInput: async (executionId: string, nodeId: string, response: string) => {
    await invoke('respond_human_input', { executionId, nodeId, response });
  },

  selectInstance: (id: string | null) => {
    set({ selectedInstanceId: id });
  },
}));
