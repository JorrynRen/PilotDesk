/**
 * enclaveTaskStore — 飞地任务工作台状态（数据权威在主机侧的 enclave.db）。
 *
 * 所有写操作都走命令层，授权与状态机在 Rust 侧；前端只按 `effectiveActions` 渲染按钮。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage } from '../utils/errorMessage';
import { useEnclaveOrgStore } from './enclaveOrgStore';
import type {
  CreateTaskInput,
  DepsPreview,
  FeedbackView,
  FileAssetView,
  ProgressList,
  ReportView,
  TaskCommentView,
  TaskView,
  UpdateTaskInput,
} from '../types/enclave';

interface EnclaveTaskState {
  tasks: TaskView[];
  activeTaskId: string | null;
  loading: boolean;
  error: string | null;
  /** 本轮是否成功用当前账号刷新过身份（失败则禁用一切写操作） */
  identityVerified: boolean;
  /** 当前选中任务的汇报、附件与过程记录 */
  reports: ReportView[];
  files: FileAssetView[];
  progress: ProgressList | null;
  /** 任务讨论（沟通层） */
  comments: TaskCommentView[];
  /** 汇报留言 / 退回原因（按 reportId 索引） */
  feedback: Record<string, FeedbackView[]>;
  /** 账号切换 / 退出：清空全部任务缓存 */
  reset: () => void;
  loadTasks: (orgId: string) => Promise<void>;
  selectTask: (taskId: string | null) => void;
  createTask: (orgId: string, input: CreateTaskInput) => Promise<TaskView | null>;
  updateTask: (orgId: string, taskId: string, input: UpdateTaskInput) => Promise<boolean>;
  setStatus: (orgId: string, taskId: string, status: string) => Promise<boolean>;
  /** 硬删除终态任务及其全部记录（创建者 / 组织管理员） */
  deleteTask: (orgId: string, taskId: string) => Promise<boolean>;
  /** 汇总确认：子任务全部收敛后，由创建者 / 负责人收口完成 */
  confirmContainer: (orgId: string, taskId: string) => Promise<boolean>;
  previewDeps: (orgId: string, taskId: string, blockedBy: string[]) => Promise<DepsPreview | null>;
  setDeps: (orgId: string, taskId: string, blockedBy: string[]) => Promise<boolean>;
  /** 拉当前任务的汇报与附件 */
  loadExtras: (orgId: string, taskId: string) => Promise<void>;
  /** 发评论（可回复 / @ 成员）；成功返回评论 */
  addComment: (
    orgId: string,
    taskId: string,
    content: string,
    replyToId?: string | null,
    mentions?: string[],
  ) => Promise<TaskCommentView | null>;
  /** 删除评论（作者本人或组织管理者） */
  deleteComment: (orgId: string, taskId: string, commentId: string) => Promise<boolean>;
  saveReportDraft: (orgId: string, taskId: string, content: string) => Promise<boolean>;
  /** 基于「已退回」的汇报继续修改：新建草稿并复制正文与交付物，原记录保留为日志 */
  reviseReport: (orgId: string, taskId: string, reportId: string) => Promise<ReportView | null>;
  submitReport: (orgId: string, taskId: string, content: string) => Promise<boolean>;
  confirmReport: (orgId: string, taskId: string, reportId: string) => Promise<boolean>;
  returnReport: (orgId: string, taskId: string, reportId: string, reason: string) => Promise<boolean>;
  uploadFile: (
    orgId: string,
    taskId: string,
    srcPath: string,
    scope?: { reportId?: string },
  ) => Promise<boolean>;
  /** 解除附件挂载（过程记录附件不可解除；原件无挂载后自动清理） */
  detachFile: (
    orgId: string,
    taskId: string,
    fileId: string,
    scope?: { reportId?: string; progressId?: string },
  ) => Promise<boolean>;
  /** 追加进度说明（可同时挂载过程成果 / 支撑文件） */
  addProgress: (orgId: string, taskId: string, content: string, attachments: string[]) => Promise<boolean>;
  /** 设置任务参与者 */
  setCollaborators: (orgId: string, taskId: string, collaborators: string[]) => Promise<boolean>;
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
  reports: [],
  files: [],
  progress: null,
  comments: [],
  feedback: {},
  /** 本轮是否成功用当前账号刷新过身份（失败则禁用一切写操作） */
  identityVerified: false,

  reset: () => {
    set({
      tasks: [],
      activeTaskId: null,
      loading: false,
      error: null,
      reports: [],
      files: [],
      progress: null,
      comments: [],
      feedback: {},
      identityVerified: false,
    });
  },

  loadTasks: async (orgId) => {
    // 身份未校验（首次进入 / 换账号后）才联网刷新，避免每次拉任务都打一次 /me
    let identityVerified = get().identityVerified;
    if (!identityVerified) {
      set({ loading: true, error: null, identityVerified: false });
      // 账号切换后快照轮询可能还没跑，旧身份会让权限判定失真
      try {
        await invoke('enclave_identity_refresh');
        identityVerified = true;
        // 身份落库后重拉组织能力，避免页面沿用旧的 caps.identity
        await useEnclaveOrgStore.getState().loadGovernance(orgId);
      } catch (e) {
        set({ error: errorMessage(e) });
      }
    } else {
      set({ loading: true, error: null });
    }
    try {
      const tasks = await invoke<TaskView[]>('enclave_task_list', { orgId });
      const active = get().activeTaskId;
      set({
        tasks,
        activeTaskId: active && tasks.some((t) => t.id === active) ? active : null,
        loading: false,
        identityVerified,
      });
    } catch (e) {
      set({ error: errorMessage(e), loading: false, identityVerified });
    }
  },

  selectTask: (taskId) => set({ activeTaskId: taskId }),

  createTask: async (orgId, input) => {
    try {
      const task = await invoke<TaskView>('enclave_task_create', { orgId, input });
      set((s) => ({ tasks: upsert(s.tasks, task), activeTaskId: task.id, error: null }));
      // 新增子任务会改变父容器的派生状态
      if (task.parentTaskId) await get().loadTasks(orgId);
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
      // 子任务变化会带动父容器状态；容器作废/归档会级联子树 → 整体重拉
      if (task.parentTaskId || task.childCount > 0) await get().loadTasks(orgId);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  deleteTask: async (orgId, taskId) => {
    try {
      await invoke('enclave_task_delete', { orgId, taskId });
      set((s) => ({
        activeTaskId: s.activeTaskId === taskId ? null : s.activeTaskId,
        tasks: s.tasks.filter((t) => t.id !== taskId),
        error: null,
      }));
      // 删除子任务会影响父容器状态 / 依赖引用 → 整体重拉
      await get().loadTasks(orgId);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  confirmContainer: async (orgId, taskId) => {
    try {
      const task = await invoke<TaskView>('enclave_task_container_confirm', { orgId, taskId });
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

  loadExtras: async (orgId, taskId) => {
    const [reports, files, progress, comments] = await Promise.all([
      invoke<ReportView[]>('enclave_report_list', { orgId, taskId }).catch(() => []),
      invoke<FileAssetView[]>('enclave_task_file_list', { orgId, taskId }).catch(() => []),
      invoke<ProgressList>('enclave_task_progress_list', { orgId, taskId }).catch(() => null),
      invoke<TaskCommentView[]>('enclave_task_comment_list', { orgId, taskId }).catch(() => []),
    ]);
    // 提交日志的退回原因 / 确认留言：按汇报逐条拉取（数量很少）
    const logs = reports.filter((r) => r.status !== 'draft');
    const pairs = await Promise.all(
      logs.map(async (r) => {
        try {
          const list = await invoke<FeedbackView[]>('enclave_report_feedback_list', {
            orgId,
            reportId: r.id,
          });
          return [r.id, list] as const;
        } catch {
          return [r.id, [] as FeedbackView[]] as const;
        }
      }),
    );
    set({ reports, files, progress, comments, feedback: Object.fromEntries(pairs) });
  },

  addComment: async (orgId, taskId, content, replyToId, mentions) => {
    try {
      const comment = await invoke<TaskCommentView>('enclave_task_comment_add', {
        orgId,
        taskId,
        content,
        replyToId: replyToId ?? null,
        mentions: mentions ?? [],
      });
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return comment;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  deleteComment: async (orgId, taskId, commentId) => {
    try {
      await invoke('enclave_task_comment_delete', { orgId, commentId });
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  saveReportDraft: async (orgId, taskId, content) => {
    try {
      await invoke<ReportView>('enclave_report_save_draft', { orgId, taskId, content });
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  reviseReport: async (orgId, taskId, reportId) => {
    try {
      const draft = await invoke<ReportView>('enclave_report_revise', { orgId, taskId, reportId });
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return draft;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  submitReport: async (orgId, taskId, content) => {
    try {
      await invoke<ReportView>('enclave_report_submit', { orgId, taskId, content });
      await get().loadTasks(orgId);
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  confirmReport: async (orgId, taskId, reportId) => {
    try {
      await invoke<ReportView>('enclave_report_confirm', { orgId, taskId, reportId });
      await get().loadTasks(orgId);
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  returnReport: async (orgId, taskId, reportId, reason) => {
    try {
      await invoke<ReportView>('enclave_report_return', { orgId, taskId, reportId, reason });
      await get().loadTasks(orgId);
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  uploadFile: async (orgId, taskId, srcPath, scope) => {
    try {
      await invoke<FileAssetView>('enclave_file_upload', {
        orgId,
        taskId,
        srcPath,
        reportId: scope?.reportId ?? null,
      });
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  detachFile: async (orgId, taskId, fileId, scope) => {
    try {
      await invoke('enclave_file_detach', {
        orgId,
        taskId,
        fileId,
        reportId: scope?.reportId ?? null,
        progressId: scope?.progressId ?? null,
      });
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  addProgress: async (orgId, taskId, content, attachments) => {
    try {
      await invoke('enclave_task_progress_add', { orgId, taskId, content, attachments });
      await get().loadExtras(orgId, taskId);
      set({ error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  setCollaborators: async (orgId, taskId, collaborators) => {
    try {
      const task = await invoke<TaskView>('enclave_task_set_collaborators', {
        orgId,
        taskId,
        collaborators,
      });
      set((s) => ({ tasks: upsert(s.tasks, task), error: null }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },
}));
