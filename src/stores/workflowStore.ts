/**
 * workflowStore — 工作流状态管理
 *
 * Zustand store，管理工作流定义列表和实例列表。
 */
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { WorkflowDefinition, WorkflowInstance, PendingHumanInput, PendingToolApproval } from '../types/workflow';
import { useNotificationStore } from './notificationStore';
import { awaitingPendingKey, approvalPendingKey } from './notificationEvents';
import { showToast } from '../utils/toast';
import { errorMessage } from '../utils/errorMessage';
import { emitWorkflowEvent } from '../workflow/WorkflowInstance';

/**
 * 删除执行记录的结果（后端 delete_executions / delete_workflow 共用）。
 * `skipped` 是未结束、被刻意保留的实例——删掉它们的执行记录会让实例从列表消失却仍在跑，
 * 而且再也取消不掉（取消链路依赖事件里的节点状态）。
 */
export interface DeleteExecutionsResult {
  deleted: number;
  skipped: { id: string; status: string }[];
}

export interface WorkflowSchedule {
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

  /**
   * 待裁决的工具审批（含 `stale` 失效记录）。与 `pendingInputs` 并列：一个是 interact 节点
   * 的人工输入，一个是 Agent 节点命中「工具授权」策略后暂停等待的工具调用。
   */
  pendingApprovals: PendingToolApproval[];
  schedules: WorkflowSchedule[];
  selectedDefinitionId: string | null;
  selectedInstanceId: string | null;
  loading: boolean;
  error: string | null;

  // 定义管理
  loadDefinitions: () => Promise<void>;
  createDefinition: (def: WorkflowDefinition) => Promise<string>;
  updateDefinition: (id: string, updates: Partial<WorkflowDefinition>) => Promise<void>;
  deleteDefinition: (id: string) => Promise<DeleteExecutionsResult>;
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
  loadPendingApprovals: () => Promise<void>;

  /** 裁决一次工具审批（`approved=false` 即拒绝）；失效/已超时的审批会提示并刷新列表 */
  respondToolApproval: (callId: string, approved: boolean) => Promise<void>;
  deleteExecution: (executionId: string) => Promise<DeleteExecutionsResult>;

  /** 批量删除执行记录：只删已结束的实例，未结束的由后端跳过并在结果里回报 */
  deleteExecutions: (executionIds: string[]) => Promise<DeleteExecutionsResult>;
  selectInstance: (id: string | null) => void;

  /** 单点执行：仅执行选中节点 */
  executeSingleNode: (executionId: string, nodeId: string) => Promise<void>;

  /** 链式执行：从选中节点到后序链路末端 */
  executeChain: (executionId: string, nodeId: string) => Promise<void>;

  /** 补全执行：跳过已完成节点，执行未完成节点 */
  executeCompletion: (executionId: string) => Promise<void>;
}

export const useWorkflowStore = create<WorkflowStoreState>((set, get) => ({
  definitions: [],
  instances: [],
  pendingInputs: [],
  pendingApprovals: [],
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
      set({ error: errorMessage(err), loading: false });
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
      set({ error: errorMessage(err), loading: false });
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
      set({ error: errorMessage(err), loading: false });
    }
  },

  deleteDefinition: async (id: string) => {
    set({ loading: true, error: null });
    try {
      // 同步删除定时调度
      await get().syncSchedule(id, { triggerType: 'manual' });

      // 后端连带清理该工作流**已结束**的执行记录（未结束的保留，
      // 否则实例会从列表消失却仍在跑、且再也取消不掉）
      const result = await invoke<DeleteExecutionsResult>('delete_workflow', { id });
      await get().loadDefinitions();
      await get().loadInstances();
      set({ loading: false });
      return result;
    } catch (err) {
      set({ error: errorMessage(err), loading: false });
      return { deleted: 0, skipped: [] };
    }
  },

  duplicateDefinition: async (id: string, newName: string) => {
    set({ loading: true, error: null });
    try {
      await invoke('duplicate_workflow', { id, newName });
      await get().loadDefinitions();
      set({ loading: false });
    } catch (err) {
      set({ error: errorMessage(err), loading: false });
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
      set({ error: errorMessage(err), loading: false });
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
      const existing = (await invoke<WorkflowSchedule[]>('list_schedules'))
        .filter(s => s.workflowId === workflowId);
      const wantCron = trigger.triggerType === 'cron' && !!trigger.cron;

      // 幂等：表达式没变（且调度仍启用）就保留原调度——重建会把 last_run_at / next_run_at 重置，
      // 于是每次保存定义都像"刚创建"一样重新排期。
      // 被停用的调度（只有"表达式非法被调度器自动停用"这一种来路）走重建：现在没有单独的
      // 定时任务管理界面，重建是用户重新启用它的唯一途径。
      if (wantCron) {
        const same = existing.find(s => s.cronExpression === trigger.cron && s.enabled);
        if (same) {
          for (const s of existing) {
            if (s.id !== same.id) await invoke('delete_schedule', { id: s.id });
          }
          await get().loadSchedules();
          return;
        }
      }
      for (const s of existing) {
        await invoke('delete_schedule', { id: s.id });
      }
      if (wantCron) {
        // 表达式非法时后端会拒绝（旧实现会落库并让调度每分钟重复触发），这里把原因告诉用户
        await invoke('create_schedule', {
          workflowId,
          cronExpression: trigger.cron,
          inputData: null,
        });
      }
      await get().loadSchedules();
    } catch (err) {
      console.warn('[workflowStore] syncSchedule 失败:', err);
      showToast(`定时调度保存失败: ${errorMessage(err)}`, 'error');
    }
  },

  startWorkflow: async (definitionId: string, context?: Record<string, unknown>, instanceId?: string) => {
    const instance = await invoke<WorkflowInstance>('start_workflow', {
      workflowId: definitionId,
      inputData: context || null,
      instanceId: instanceId || null,
    });
    // 广播 `workflow:instance:started` 给插件（emitWorkflowEvent 内部走宿主事件总线）。
    // 只发得出去前端确知的这一种：节点级 / 完成 / 失败的状态真身在 Rust 侧，
    // 前端只是轮询读取（loadInstances），所以那些事件目前没有投递点。
    emitWorkflowEvent('instance:started', instance.id);
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
    return get().deleteExecutions([executionId]);
  },

  deleteExecutions: async (executionIds: string[]) => {
    const result = await invoke<DeleteExecutionsResult>('delete_executions', { executionIds });
    await get().loadInstances();
    return result;
  },

  respondHumanInput: async (executionId: string, nodeId: string, response: string) => {
    try {
      await invoke('respond_human_input', { executionId, nodeId, response });

      // 已答复：通知中心里该节点的「待处理」随之消解（执行本身可能还要跑很久）
      useNotificationStore.getState().resolve(awaitingPendingKey(executionId, nodeId));
      await get().loadPendingInputs();
      await get().loadInstances();
    } catch (err) {
      // 不能只 console.error：提交失败时界面既无提示、卡片也不会消失，
      // 用户看到的就是"点了没反应"。典型场景：执行已异常退出/等待已超时，后端已无等待者。
      console.error('响应人工输入失败:', err);
      showToast(`提交失败：${errorMessage(err)}`, 'error');

      // 刷新待办：后端按"此刻真在等"过滤，失效项会随之从列表消失（卡片自动收走）
      await get().loadPendingInputs();
    }
  },

  loadPendingApprovals: async () => {
    try {
      const approvals = await invoke<PendingToolApproval[]>('get_pending_tool_approvals');
      set({ pendingApprovals: approvals });
    } catch {
      set({ pendingApprovals: [] });
    }
  },

  respondToolApproval: async (callId: string, approved: boolean) => {
    try {
      await invoke('respond_tool_approval', { callId, approved });

      // 已裁决：通知中心里该调用的「待处理」随之消解
      useNotificationStore.getState().resolve(approvalPendingKey(callId));
      await get().loadPendingApprovals();
      await get().loadInstances();
    } catch (err) {
      // 典型场景：审批已超时（后端按拒绝续跑）或进程重启后等待者已消失。
      // 此时界面必须说话，否则用户看到的是"点了没反应"。
      console.error('提交工具审批失败:', err);
      showToast(`提交失败：${errorMessage(err)}`, 'error');
      await get().loadPendingApprovals();
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
