/**
 * workflowStore — 工作流状态管理
 *
 * Zustand store，管理工作流定义列表和实例列表。
 */

import { create } from 'zustand';
import { workflowEngine } from '../workflow/WorkflowEngine';
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
  loadInstances: () => Promise<void>;
  startWorkflow: (definitionId: string, context?: Record<string, unknown>) => Promise<string>;
  pauseWorkflow: (instanceId: string) => Promise<void>;
  resumeWorkflow: (instanceId: string) => Promise<void>;
  stopWorkflow: (instanceId: string) => Promise<void>;
  retryWorkflow: (instanceId: string, stepId?: string) => Promise<void>;
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
      const definitions = await workflowEngine.listDefinitions();
      set({ definitions, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  createDefinition: async (def: WorkflowDefinition) => {
    const id = await workflowEngine.createDefinition(def);
    await get().loadDefinitions();
    return id;
  },

  updateDefinition: async (id: string, updates: Partial<WorkflowDefinition>) => {
    await workflowEngine.updateDefinition(id, updates);
    await get().loadDefinitions();
  },

  deleteDefinition: async (id: string) => {
    await workflowEngine.deleteDefinition(id);
    await get().loadDefinitions();
  },

  selectDefinition: (id: string | null) => {
    set({ selectedDefinitionId: id });
  },

  loadInstances: async () => {
    try {
      const instances = await workflowEngine.listInstances();
      set({ instances });
    } catch (err) {
      console.error('Failed to load instances:', err);
    }
  },

  startWorkflow: async (definitionId: string, context?: Record<string, unknown>) => {
    const instanceId = await workflowEngine.start(definitionId, context);
    await get().loadInstances();
    return instanceId;
  },

  pauseWorkflow: async (instanceId: string) => {
    await workflowEngine.pause(instanceId);
    await get().loadInstances();
  },

  resumeWorkflow: async (instanceId: string) => {
    await workflowEngine.resume(instanceId);
    await get().loadInstances();
  },

  stopWorkflow: async (instanceId: string) => {
    await workflowEngine.stop(instanceId);
    await get().loadInstances();
  },

  retryWorkflow: async (instanceId: string, stepId?: string) => {
    await workflowEngine.retry(instanceId, stepId);
    await get().loadInstances();
  },

  selectInstance: (id: string | null) => {
    set({ selectedInstanceId: id });
  },
}));
