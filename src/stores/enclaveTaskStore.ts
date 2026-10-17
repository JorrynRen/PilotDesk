/**
 * enclaveTaskStore — 飞地任务工作台状态（数据权威在主机侧的 enclave.db）。
 *
 * 所有写操作都走命令层，授权与状态机在 Rust 侧；前端只按 `effectiveActions` 渲染按钮。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage } from '../utils/errorMessage';
import type {
  CreateTaskInput,
  DepsPreview,
  TaskView,
  UpdateTaskInput,
} from '../types/enclave';

interface EnclaveTaskState {
  tasks: TaskView[];
  activeTaskId: string | null;
  loading: boolean;
  error: string | null;
  loadTasks: (orgId: string) => Promise<void>;
  selectTask: (taskId: string | null) => void;
  createTask: (orgId: string, input: CreateTaskInput) => Promise<TaskView | null>;
  updateTask: (orgId: string, taskId: string, input: UpdateTaskInput) => Promise<boolean>;
  setStatus: (orgId: string, taskId: string, status: string) => Promise<boolean>;
  previewDeps: (orgId: string, taskId: string, blockedBy: string[]) => Promise<DepsPreview | null>;
  setDeps: (orgId: string, taskId: string, blockedBy: string[]) => Promise<boolean>;
}

/** 用返回的最新任务替换列表中的同 id 项。 */
function upsert(tasks: TaskView[], next: TaskView): TaskView[] {
  const i = tasks.findIndex((t) => t.id === next.id);
  if (i < 0) return [...tasks, next];
  const copy = tasks.slice();
  copy[i] = next;
  return copy;
}

export const useEnclaveTaskStore = create<EnclaveTaskState>((set, get) => ({
  tasks: [],
  activeTaskId: null,
  loading: false,
  error: null,

  loadTasks: async (orgId) => {
    set({ loading: true, error: null });
    try {
      const tasks = await invoke<TaskView[]>('enclave_task_list', { orgId });
      const active = get().activeTaskId;
      set({
        tasks,
        activeTaskId: active && tasks.some((t) => t.id === active) ? active : null,
        loading: false,
      });
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  selectTask: (taskId) => set({ activeTaskId: taskId }),

  createTask: async (orgId, input) => {
    try {
      const task = await invoke<TaskView>('enclave_task_create', { orgId, input });
      set((s) => ({ tasks: upsert(s.tasks, task), activeTaskId: task.id, error: null }));
      return task;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  updateTask: async (orgId, taskId, input) => {
    try {
      const task = await invoke<TaskView>('enclave_task_update', { orgId, taskId, input });
      set((s) => ({ tasks: upsert(s.tasks, task), error: null }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  setStatus: async (orgId, taskId, status) => {
    try {
      const task = await invoke<TaskView>('enclave_task_set_status', { orgId, taskId, status });
      set((s) => ({ tasks: upsert(s.tasks, task), error: null }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  previewDeps: async (orgId, taskId, blockedBy) => {
    try {
      return await invoke<DepsPreview>('enclave_task_deps_preview', { orgId, taskId, blockedBy });
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  setDeps: async (orgId, taskId, blockedBy) => {
    try {
      const task = await invoke<TaskView>('enclave_task_set_deps', { orgId, taskId, blockedBy });
      set((s) => ({ tasks: upsert(s.tasks, task), error: null }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },
}));
