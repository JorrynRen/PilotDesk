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
  cancelWorkflow: (executionId: string) => Promise<void>;
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
      const definitions = await invoke<WorkflowDefinition[]>('workflow_list_definitions');
      set({ definitions, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  createDefinition: async (def: WorkflowDefinition) => {
    await invoke('workflow_create_definition', { definition: def });
    await get().loadDefinitions();
    return def.id;
  },

  updateDefinition: async (id: string, updates: Partial<WorkflowDefinition>) => {
    const defs = get().definitions;
    const existing = defs.find(d => d.id === id);
    if (existing) {
      await invoke('workflow_update_definition', { definition: { ...existing, ...updates } });
    }
    await get().loadDefinitions();
  },

  deleteDefinition: async (id: string) => {
    await invoke('workflow_delete_definition', { id });
    await get().loadDefinitions();
  },

  selectDefinition: (id: string | null) => {
    set({ selectedDefinitionId: id });
  },

  loadInstances: async (definitionId?: string) => {
    try {
      const instances = await invoke<WorkflowInstance[]>('workflow_list_instances', { definitionId: definitionId || null });
      set({ instances });
    } catch (err) {
      console.error('Failed to load instances:', err);
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

  cancelWorkflow: async (executionId: string) => {
    await invoke('cancel_workflow', { executionId });
    await get().loadInstances();
  },

  respondHumanInput: async (executionId: string, nodeId: string, response: string) => {
    await invoke('respond_human_input', { executionId, nodeId, response });
  },

  selectInstance: (id: string | null) => {
    set({ selectedInstanceId: id });
  },
}));
