/**
 * 飞地任务工作台：左任务树 / 中详情 / 右属性与依赖。
 *
 * 权限口径：所有动作按钮都看后端返回的 `effectiveActions`（授权器唯一来源）；
 * 状态流转的合法性也由后端状态机兜底，前端只负责把可选项呈现出来。
 */

import { useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import {
  Plus, ListTree, Link2, AlertTriangle, ChevronRight,
  Info, FileText, ListChecks, ClipboardCheck, Paperclip, Flag, History, Trash2,
  Wand2, Loader2,
} from 'lucide-react';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { useEnclaveTaskStore, type TaskDetailTab } from '../../stores/enclaveTaskStore';
import { useAccountStore } from '../../stores/accountStore';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { Select } from '../common/Select';
import { EnclaveNavBar } from './EnclaveNavBar';
import { EnclaveToolbarActions, EnclaveFilesRootPanel } from './EnclaveToolbar';
import { TASK_TEMPLATES, type TaskTemplate } from './taskTemplates';
import type {
  DepsPreview, KbView, MemberView, TaskCommentView, TaskSplitDraft, TaskStatus, TaskView,
} from '../../types/enclave';

/** 表单控件口径：与个人知识库一致（参见 KnowledgeBaseDialog） */
const FIELD_CLASS = 'w-full px-2.5 py-1.5 rounded-lg text-xs outline-none';
const FIELD_STYLE = {
  backgroundColor: 'var(--bg-tertiary)',
  color: 'var(--text-primary)',
  border: '1px solid var(--border)',
} as const;
const TEXTAREA_CLASS = 'w-full p-3 rounded-lg text-xs leading-relaxed outline-none resize-y';

const STATUS_LABEL: Record<TaskStatus, string> = {
  pending: '待办',
  in_progress: '进行中',
  submitted: '待确认',
  done: '已完成',
  archived: '已归档',
  cancelled: '已作废',
};

const STATUS_COLOR: Record<TaskStatus, string> = {
  pending: 'var(--text-tertiary)',
  in_progress: 'var(--accent)',
  submitted: 'var(--status-warning, #d08700)',
  done: 'var(--status-success, #2f9e44)',
  archived: 'var(--text-tertiary)',
  cancelled: 'var(--status-danger)',
};

const PRIORITY_LABEL: Record<string, string> = {
  low: '低',
  normal: '普通',
  high: '高',
  urgent: '紧急',
};

/** 每个状态可执行的流转（合法性最终由后端 `can_transition` 判定）。
 *
 * 「待确认」不在此列：它只由「完成汇报」驱动——提交汇报进入，确认/退回汇报离开。
 */
const TRANSITIONS: Record<TaskStatus, { to: TaskStatus; label: string }[]> = {
  pending: [{ to: 'in_progress', label: '开始' }],
  in_progress: [
    { to: 'pending', label: '放回待办' },
    { to: 'cancelled', label: '作废' },
  ],
  submitted: [],
  done: [
    { to: 'archived', label: '归档' },
    { to: 'in_progress', label: '恢复汇报' },
  ],
  archived: [{ to: 'in_progress', label: '恢复汇报' }],
  cancelled: [],
};

/**
 * 汇总容器（有子任务）的可用状态操作：
 * 自身不执行，只保留「作废」与「归档」；完成走「汇总确认」。
 */
const CONTAINER_TRANSITIONS: Record<TaskStatus, { to: TaskStatus; label: string }[]> = {
  pending: [{ to: 'cancelled', label: '作废' }],
  in_progress: [{ to: 'cancelled', label: '作废' }],
  submitted: [{ to: 'cancelled', label: '作废' }],
  done: [{ to: 'archived', label: '归档' }],
  archived: [],
  cancelled: [],
};

function toDateInput(unixSec: number | null): string {
  if (!unixSec) return '';
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function fromDateInput(v: string): number | null {
  if (!v) return null;
  const t = new Date(`${v}T00:00:00`).getTime();
  return Number.isFinite(t) ? Math.floor(t / 1000) : null;
}

function formatDateTime(unixSec: number): string {
  if (!unixSec) return '-';
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}


export function TaskWorkspace() {
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);
  const detail = useEnclaveOrgStore((s) => s.detail);

  const tasks = useEnclaveTaskStore((s) => s.tasks);
  const activeTaskId = useEnclaveTaskStore((s) => s.activeTaskId);
  const loading = useEnclaveTaskStore((s) => s.loading);
  const error = useEnclaveTaskStore((s) => s.error);

  const [newOpen, setNewOpen] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [newParentId, setNewParentId] = useState('');
  const [newAssignee, setNewAssignee] = useState('');
  const [newPriority, setNewPriority] = useState('normal');
  const [newDue, setNewDue] = useState('');
  /** 产出目标库（空 = 默认库） */
  const [newKb, setNewKb] = useState('');
  const [creating, setCreating] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  /** 任务列表筛选：默认「活跃」= 不含已归档；也可按单一状态筛选 */
  const [statusFilter, setStatusFilter] = useState<'active' | 'all' | TaskStatus>('active');
  /** 负责人筛选：all / unassigned / userId */
  const [assigneeFilter, setAssigneeFilter] = useState('all');
  /** 只看逾期（有截止时间、已过期且未完成） */
  const [overdueOnly, setOverdueOnly] = useState(false);
  /** 批量操作：勾选模式 + 选中集 */
  const [batchMode, setBatchMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [batchAssignee, setBatchAssignee] = useState('');
  const [batchDue, setBatchDue] = useState('');
  const [batchBusy, setBatchBusy] = useState(false);
  /** 任务模板：选模板 → 预览 / 修改草稿 → 一次生成父任务与子任务 */
  const [tplOpen, setTplOpen] = useState(false);
  const [tplStage, setTplStage] = useState<'pick' | 'edit'>('pick');
  const [tplId, setTplId] = useState<string>(TASK_TEMPLATES[0]?.id ?? '');
  const [tplTitle, setTplTitle] = useState('');
  const [tplDrafts, setTplDrafts] = useState<
    { title: string; description: string; priority: string; checked: boolean }[]
  >([]);
  const [tplBusy, setTplBusy] = useState(false);

  /** 产出目标库候选（直接查主机，不走知识库页的 store —— 避免打开任务页就把知识库"打开"） */
  const [kbOptions, setKbOptions] = useState<{ id: string; name: string; isImplicit: boolean }[]>([]);

  // 本机团队文件根目录：**全局设置**，与具体组织无关（按钮与面板三个页面共用）
  const [rootOpen, setRootOpen] = useState(false);

  // 登录账号变化 → 先校验新身份，再拉组织（会自动选中并加载成员）
  const accountKey = useAccountStore((s) => s.account?.email ?? '');
  /** 登录检查已完成但没有账号：此时任务列表为空是「没登录」，不是「没任务」 */
  const notLoggedIn = useAccountStore((s) => s.loaded) && !useAccountStore((s) => s.account);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const st = useEnclaveOrgStore.getState();
      if (!(await st.ensureIdentity()) || cancelled) return;
      await st.loadOrgs();
      if (cancelled) return;
      const cur = useEnclaveOrgStore.getState();
      if (cur.orgs.length === 0 && !cur.lastRefresh) await cur.refresh();
    })();
    return () => {
      cancelled = true;
    };
  }, [accountKey]);

  // 组织变化或账号变化 → 重拉任务；无组织时清空，避免展示上一账号的数据
  useEffect(() => {
    if (!activeOrgId) {
      useEnclaveTaskStore.getState().reset();
      return;
    }
    void useEnclaveTaskStore.getState().loadTasks(activeOrgId);
  }, [activeOrgId, accountKey]);

  // 产出目标库候选：按组织拉一次（失败静默，任务照常可用，目标库只显示为未知）
  useEffect(() => {
    if (!activeOrgId) {
      setKbOptions([]);
      return;
    }
    let cancelled = false;
    void invoke<KbView[]>('enclave_kb_list', { orgId: activeOrgId })
      .then((list) => {
        if (cancelled) return;
        setKbOptions(list.map((k) => ({ id: k.id, name: k.name, isImplicit: k.isImplicit })));
      })
      .catch(() => {
        if (!cancelled) setKbOptions([]);
      });
    return () => {
      cancelled = true;
    };
  }, [activeOrgId]);

  const members: MemberView[] = detail?.members ?? [];
  const memberLabel = (id: string | null): string => {
    if (!id) return '未指派';
    const m = members.find((x) => x.userId === id);
    return m?.displayName || m?.email || id;
  };

  /** 成员负载：在办任务数（待办 / 进行中 / 待确认），用于负责人下拉与详情提示 */
  const memberLoad = useMemo(() => {
    const m: Record<string, number> = {};
    for (const t of tasks) {
      if (!t.assigneeId) continue;
      if (t.status === 'pending' || t.status === 'in_progress' || t.status === 'submitted') {
        m[t.assigneeId] = (m[t.assigneeId] ?? 0) + 1;
      }
    }
    return m;
  }, [tasks]);
  const memberOptionLabel = (m: MemberView): string =>
    `${m.displayName || m.email}${memberLoad[m.userId] ? `（在办 ${memberLoad[m.userId]}）` : ''}`;

  /** 任务树筛选：状态 / 负责人 / 是否逾期；默认「活跃」= 不含已归档 */
  const nowSec = Math.floor(Date.now() / 1000);
  const visibleTasks = useMemo(() => {
    let list = tasks;
    if (statusFilter === 'active') list = list.filter((t) => t.status !== 'archived');
    else if (statusFilter !== 'all') list = list.filter((t) => t.status === statusFilter);
    if (assigneeFilter === 'unassigned') list = list.filter((t) => !t.assigneeId);
    else if (assigneeFilter !== 'all') list = list.filter((t) => t.assigneeId === assigneeFilter);
    if (overdueOnly) {
      list = list.filter(
        (t) =>
          !!t.dueDate &&
          t.dueDate < nowSec &&
          (t.status === 'pending' || t.status === 'in_progress' || t.status === 'submitted'),
      );
    }
    return list;
  }, [tasks, statusFilter, assigneeFilter, overdueOnly, nowSec]);
  // 单状态 / 负责人 / 逾期筛选时扁平展示（否则父任务被过滤后会丢掉匹配的子任务）
  const isFlatFilter =
    (statusFilter !== 'active' && statusFilter !== 'all') ||
    assigneeFilter !== 'all' ||
    overdueOnly;
  const roots = useMemo(
    () => (isFlatFilter ? visibleTasks : visibleTasks.filter((t) => !t.parentTaskId)),
    [visibleTasks, isFlatFilter],
  );
  const childrenOf = (id: string) =>
    isFlatFilter ? [] : visibleTasks.filter((t) => t.parentTaskId === id);
  const task = tasks.find((t) => t.id === activeTaskId) ?? null;

  // 选中集只在当前可见列表里生效（筛选变化后清掉不可见项）
  useEffect(() => {
    setSelectedIds((prev) => prev.filter((id) => visibleTasks.some((t) => t.id === id)));
  }, [visibleTasks]);

  /** 批量改派（`__none__` = 未指派）：逐个调用既有编辑命令，无权修改的任务计入失败 */
  const batchAssign = async (picked: string) => {
    if (!activeOrgId || !picked || selectedIds.length === 0) return;
    const assigneeId = picked === '__none__' ? null : picked;
    const total = selectedIds.length;
    setBatchBusy(true);
    let ok = 0;
    for (const id of selectedIds) {
      const done = await useEnclaveTaskStore
        .getState()
        .updateTask(activeOrgId, id, { assigneeId });
      if (done) ok += 1;
    }
    setBatchBusy(false);
    setBatchAssignee('');
    const failed = total - ok;
    showToast(
      failed > 0
        ? `已改派 ${ok} 个任务，${failed} 个失败（可能无权修改或任务已归档）`
        : `已改派 ${ok} 个任务`,
      failed > 0 ? 'error' : 'success',
    );
  };

  /** 批量改期（同一天）；日期按本地日期 00:00 存，和单任务编辑口径一致 */
  const batchSetDue = async () => {
    const due = fromDateInput(batchDue);
    if (!activeOrgId || due === null || selectedIds.length === 0) return;
    const total = selectedIds.length;
    setBatchBusy(true);
    let ok = 0;
    for (const id of selectedIds) {
      const done = await useEnclaveTaskStore.getState().updateTask(activeOrgId, id, { dueDate: due });
      if (done) ok += 1;
    }
    setBatchBusy(false);
    setBatchDue('');
    const failed = total - ok;
    showToast(
      failed > 0
        ? `已改期 ${ok} 个任务，${failed} 个失败（可能无权修改或任务已归档）`
        : `已改期 ${ok} 个任务`,
      failed > 0 ? 'error' : 'success',
    );
  };

  // 当前选中任务被筛选条件排除时，详情自动让位
  useEffect(() => {
    if (task && !visibleTasks.some((t) => t.id === task.id)) {
      useEnclaveTaskStore.getState().selectTask(null);
    }
  }, [visibleTasks, task?.id]);

  const submitCreate = async () => {
    if (!activeOrgId) return;
    if (!newTitle.trim()) {
      setLocalError('请填写任务标题');
      return;
    }
    const parent = newParentId ? tasks.find((t) => t.id === newParentId) : null;
    if (parent?.status === 'done') {
      const ok = await confirmDialog({
        title: '拆分为汇总任务',
        message:
          `父任务「${parent.title}」已完成。添加子任务后它会转为汇总任务并重新进入进行中；`
          + '已有的过程记录与完成汇报会保留为只读历史。\n\n是否继续？',
        confirmText: '继续创建',
      });
      if (!ok) return;
    }
    setCreating(true);
    setLocalError(null);
    const created = await useEnclaveTaskStore.getState().createTask(activeOrgId, {
      title: newTitle.trim(),
      parentTaskId: newParentId || null,
      assigneeId: newAssignee || null,
      priority: newPriority,
      dueDate: fromDateInput(newDue),
      targetKbId: newKb,
    });
    setCreating(false);
    if (created) cancelCreate();
  };

  /** 取消新建：清空草稿并收起，避免下次打开残留上次输入 */
  const cancelCreate = () => {
    setNewOpen(false);
    setNewTitle('');
    setNewParentId('');
    setNewAssignee('');
    setNewDue('');
    setNewPriority('normal');
    setNewKb('');
    setLocalError(null);
  };

  /** 关闭模板弹窗并复位（下次打开从「选模板」开始） */
  const closeTemplate = () => {
    setTplOpen(false);
    setTplStage('pick');
    setTplDrafts([]);
    setTplTitle('');
    setLocalError(null);
  };

  /** 选好模板后进入预览：把模板子任务变成可勾选 / 可改的草稿 */
  const startTemplatePreview = () => {
    const tpl = TASK_TEMPLATES.find((t) => t.id === tplId);
    if (!tpl) return;
    if (!tplTitle.trim()) {
      setLocalError('请填写父任务标题');
      return;
    }
    setTplDrafts(
      tpl.children.map((c) => ({
        title: c.title,
        description: c.description,
        priority: c.priority ?? 'normal',
        checked: true,
      })),
    );
    setTplStage('edit');
    setLocalError(null);
  };

  /** 按预览里的草稿创建：父任务 + 勾选且填了标题的子任务 */
  const createFromTemplate = async () => {
    if (!activeOrgId) return;
    if (!tplTitle.trim()) {
      setLocalError('请填写父任务标题');
      setTplStage('pick');
      return;
    }
    const picked = tplDrafts.filter((d) => d.checked && d.title.trim());
    if (picked.length === 0) {
      setLocalError('请至少勾选一个子任务');
      return;
    }
    setTplBusy(true);
    setLocalError(null);
    const parent = await useEnclaveTaskStore.getState().createTask(activeOrgId, {
      title: tplTitle.trim(),
      priority: 'normal',
    });
    if (!parent) {
      setTplBusy(false);
      return;
    }
    let ok = 0;
    for (const d of picked) {
      const created = await useEnclaveTaskStore.getState().createTask(activeOrgId, {
        title: d.title.trim(),
        description: d.description.trim(),
        parentTaskId: parent.id,
        priority: d.priority,
      });
      if (created) ok += 1;
    }
    setTplBusy(false);
    showToast(`已按模板创建「${parent.title}」及 ${ok} 个子任务`, ok > 0 ? 'success' : 'error');
    closeTemplate();
    setNewOpen(false);
    await useEnclaveTaskStore.getState().loadTasks(activeOrgId);
    useEnclaveTaskStore.getState().selectTask(parent.id);
  };

  const [refreshing, setRefreshing] = useState(false);
  /** 手动刷新：重新校验本机身份 → 重拉组织与治理 → 重拉任务与通知。 */
  const refreshAll = async () => {
    setRefreshing(true);
    try {
      const org = useEnclaveOrgStore.getState();
      if (!(await org.ensureIdentity())) return;
      await org.loadOrgs();
      const next = useEnclaveOrgStore.getState().activeOrgId;
      if (next) {
        await useEnclaveTaskStore.getState().loadTasks(next);
        await useEnclaveOrgStore.getState().loadNotifications(next);
      }
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
      <div
        className="shrink-0 rounded-lg overflow-hidden"
        style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
      >
        <EnclaveNavBar
          active="tasks"
          onRefresh={refreshAll}
          refreshing={refreshing || loading}
          right={
            <EnclaveToolbarActions
              rootOpen={rootOpen}
              onToggleRoot={() => setRootOpen((v) => !v)}
              create={{
                label: '新建任务',
                open: newOpen,
                onToggle: () => setNewOpen((v) => !v),
                disabled: !activeOrgId,
              }}
            />
          }
        />
      </div>

      <div className="flex-1 min-h-0 pt-2 flex flex-col overflow-hidden">
        <div
          className="flex-1 min-h-0 rounded-lg overflow-hidden flex flex-col"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >

      {/* 本机团队文件根目录（全局设置，仅影响本机存放位置；面板与其它页面共用） */}
      <EnclaveFilesRootPanel open={rootOpen} />

      {/* 新建表单 */}
      {newOpen && (
        <div
          className="shrink-0 px-3 py-2 flex items-end gap-2 flex-wrap"
          style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-side)' }}
        >
          <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
            任务标题
            <input
              autoFocus
              className={FIELD_CLASS}
              style={{ ...FIELD_STYLE, width: 220 }}
              placeholder="任务标题"
              value={newTitle}
              onChange={(e) => setNewTitle(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') void submitCreate(); }}
            />
          </label>
          <div className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
            <span>父任务</span>
            <Select
              value={newParentId}
              onChange={setNewParentId}
              style={{ width: 150, flexShrink: 0 }}
              options={[
                { value: '', label: '（顶层任务）' },
                ...roots
                  .filter(
                    (t) => t.status !== 'archived' && t.status !== 'cancelled',
                  )
                  .map((t) => ({ value: t.id, label: t.title })),
              ]}
              title="父任务"
            />
          </div>
          <div className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
            <span>负责人</span>
            <Select
              value={newAssignee}
              onChange={setNewAssignee}
              style={{ width: 170, flexShrink: 0 }}
              options={[
                { value: '', label: '未指派' },
                ...members
                  .filter((m) => m.seatStatus !== 'removed')
                  .map((m) => ({ value: m.userId, label: memberOptionLabel(m) })),
              ]}
              title="负责人（括号内为该成员当前在办任务数）"
            />
          </div>
          <div className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
            <span>优先级</span>
            <Select
              value={newPriority}
              onChange={setNewPriority}
              style={{ width: 90, flexShrink: 0 }}
              options={Object.entries(PRIORITY_LABEL).map(([k, v]) => ({ value: k, label: v }))}
              title="优先级"
            />
          </div>
          <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
            截止日期
            <input
              type="date"
              className={FIELD_CLASS}
              style={{ ...FIELD_STYLE, width: 140 }}
              value={newDue}
              onChange={(e) => setNewDue(e.target.value)}
            />
          </label>
          <div className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
            <span>产出目标库</span>
            <Select
              value={newKb}
              onChange={setNewKb}
              style={{ width: 170, flexShrink: 0 }}
              options={[
                { value: '', label: '默认库（团队知识库）' },
                ...kbOptions
                  .filter((k) => !k.isImplicit)
                  .map((k) => ({ value: k.id, label: k.name })),
              ]}
              title="汇报确认后，任务产出沉淀到哪个知识库（与任务状态「归档」无关）"
            />
          </div>
          <button
            onClick={() => {
              // 每次打开都从「选模板」开始，避免残留上一次的草稿
              setTplStage('pick');
              setTplId(TASK_TEMPLATES[0]?.id ?? '');
              setTplDrafts([]);
              setTplTitle('');
              setLocalError(null);
              setTplOpen(true);
            }}
            disabled={!!newParentId}
            className="pd-btn text-xs px-3 py-1.5 rounded-lg flex items-center gap-1"
            style={{
              color: 'var(--text-secondary)',
              backgroundColor: 'var(--bg-tertiary)',
              border: '1px solid var(--border)',
              opacity: newParentId ? 0.5 : 1,
            }}
            title={
              newParentId
                ? '模板会创建新的顶层任务：请先把父任务设为「（顶层任务）」'
                : '按模板一键创建父任务与可交付子任务'
            }
          >
            <ListChecks size={11} />
            从模板创建
          </button>
          <button
            onClick={() => void submitCreate()}
            disabled={creating}
            className="pd-btn text-xs px-3 py-1.5 rounded-lg"
            style={{ backgroundColor: 'var(--accent)', color: '#fff', opacity: creating ? 0.6 : 1 }}
          >
            {creating ? '创建中…' : '创建'}
          </button>
          <button
            onClick={cancelCreate}
            className="pd-btn text-xs px-3 py-1.5 rounded-lg"
            style={{
              color: 'var(--text-secondary)',
              backgroundColor: 'var(--bg-tertiary)',
              border: '1px solid var(--border)',
            }}
            title="放弃本次填写"
          >
            取消
          </button>
          {(localError || error) && (
            <span className="text-[11px]" style={{ color: 'var(--status-danger)' }}>
              {localError || error}
            </span>
          )}
        </div>
      )}

      <div className="flex-1 min-h-0 flex overflow-hidden">
        {/* 左：任务树（顶部按状态筛选） */}
        <div
          className="w-[280px] shrink-0 flex flex-col overflow-hidden"
          style={{ borderRight: '1px solid var(--border)' }}
        >
          <div
            className="shrink-0 px-2 py-1.5 flex flex-col gap-1.5"
            style={{ borderBottom: '1px solid var(--border)' }}
          >
            <Select
              value={statusFilter}
              onChange={(v) => setStatusFilter(v as 'active' | 'all' | TaskStatus)}
              className="w-full"
              options={[
                { value: 'active', label: '活跃任务（不含已归档）' },
                { value: 'all', label: '全部状态' },
                { value: 'pending', label: '待办' },
                { value: 'in_progress', label: '进行中' },
                { value: 'submitted', label: '待确认 / 待汇总确认' },
                { value: 'done', label: '已完成' },
                { value: 'archived', label: '已归档' },
                { value: 'cancelled', label: '已作废' },
              ]}
              title="按任务状态筛选"
            />
            <div className="flex items-center gap-1">
              <Select
                value={assigneeFilter}
                onChange={setAssigneeFilter}
                className="flex-1 min-w-0"
                options={[
                  { value: 'all', label: '全部负责人' },
                  { value: 'unassigned', label: '未指派' },
                  ...members
                    .filter((m) => m.seatStatus !== 'removed')
                    .map((m) => ({ value: m.userId, label: m.displayName || m.email })),
                ]}
                title="按负责人筛选"
              />
              <button
                onClick={() => setOverdueOnly((v) => !v)}
                className="shrink-0 text-[10px] px-1.5 py-1 rounded"
                style={{
                  backgroundColor: overdueOnly ? 'var(--accent)' : 'var(--bg-tertiary)',
                  color: overdueOnly ? '#fff' : 'var(--text-secondary)',
                  border: `1px solid ${overdueOnly ? 'var(--accent)' : 'var(--border)'}`,
                }}
                title="只看已过截止时间且未完成的任务"
              >
                逾期
              </button>
              <button
                onClick={() => {
                  setBatchMode((v) => !v);
                  setSelectedIds([]);
                  setBatchAssignee('');
                  setBatchDue('');
                }}
                className="shrink-0 text-[10px] px-1.5 py-1 rounded"
                style={{
                  backgroundColor: batchMode ? 'var(--accent)' : 'var(--bg-tertiary)',
                  color: batchMode ? '#fff' : 'var(--text-secondary)',
                  border: `1px solid ${batchMode ? 'var(--accent)' : 'var(--border)'}`,
                }}
                title="批量改派 / 改期"
              >
                批量
              </button>
            </div>
            {batchMode && (
              <div className="flex flex-col gap-1 pt-0.5">
                <div className="flex items-center gap-1.5 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  <span>已选 {selectedIds.length}</span>
                  <button
                    onClick={() => setSelectedIds(visibleTasks.map((t) => t.id))}
                    style={{ color: 'var(--accent)' }}
                    title="全选当前筛选结果"
                  >
                    全选
                  </button>
                  <button
                    onClick={() => setSelectedIds([])}
                    style={{ color: 'var(--text-tertiary)' }}
                    disabled={selectedIds.length === 0}
                  >
                    清空
                  </button>
                </div>
                <Select
                  value={batchAssignee}
                  onChange={(v) => void batchAssign(v)}
                  className="w-full"
                  options={[
                    { value: '', label: batchBusy ? '处理中…' : '批量改派到…' },
                    { value: '__none__', label: '未指派' },
                    ...members
                      .filter((m) => m.seatStatus !== 'removed')
                      .map((m) => ({ value: m.userId, label: memberOptionLabel(m) })),
                  ]}
                  title="把选中的任务改派给该成员（括号内为在办任务数）"
                />
                <div className="flex items-center gap-1">
                  <input
                    type="date"
                    className={FIELD_CLASS}
                    style={{ ...FIELD_STYLE, flex: 1, minWidth: 0 }}
                    value={batchDue}
                    onChange={(e) => setBatchDue(e.target.value)}
                    title="批量设置同一截止日期"
                  />
                  <button
                    onClick={() => void batchSetDue()}
                    disabled={batchBusy || !batchDue || selectedIds.length === 0}
                    className="shrink-0 text-[10px] px-1.5 py-1 rounded"
                    style={{
                      backgroundColor: 'var(--bg-tertiary)',
                      color: 'var(--text-secondary)',
                      border: '1px solid var(--border)',
                      opacity: batchBusy || !batchDue || selectedIds.length === 0 ? 0.5 : 1,
                    }}
                    title="把选中任务的截止日期改成该日期"
                  >
                    改期
                  </button>
                </div>
              </div>
            )}
          </div>
          <div className="flex-1 overflow-auto py-1">
            {roots.length === 0 && (
              <div className="px-3 py-6 text-xs text-center" style={{ color: 'var(--text-tertiary)' }}>
                {notLoggedIn
                  ? '未登录：登录平台账号后才会加载团队任务'
                  : tasks.length === 0
                    ? '还没有任务，点右上角「新建任务」'
                    : '没有符合筛选条件的任务'}
              </div>
            )}
            {roots.map((root) => (
              <div key={root.id}>
                <TaskNode
                  task={root}
                  active={root.id === activeTaskId}
                  onClick={() => useEnclaveTaskStore.getState().selectTask(root.id)}
                  onAddChild={() => {
                    setNewParentId(root.id);
                    setNewOpen(true);
                  }}
                  selectable={batchMode}
                  selected={selectedIds.includes(root.id)}
                  onToggleSelect={() =>
                    setSelectedIds((prev) =>
                      prev.includes(root.id) ? prev.filter((x) => x !== root.id) : [...prev, root.id],
                    )
                  }
                />
                {childrenOf(root.id).map((child) => (
                  <TaskNode
                    key={child.id}
                    task={child}
                    child
                    active={child.id === activeTaskId}
                    onClick={() => useEnclaveTaskStore.getState().selectTask(child.id)}
                    selectable={batchMode}
                    selected={selectedIds.includes(child.id)}
                    onToggleSelect={() =>
                      setSelectedIds((prev) =>
                        prev.includes(child.id)
                          ? prev.filter((x) => x !== child.id)
                          : [...prev, child.id],
                      )
                    }
                  />
                ))}
              </div>
            ))}
          </div>
        </div>

        {/* 中：详情（属性 / 依赖 / 过程记录 / 汇报都在这里，右侧不常驻面板） */}
        <div className="flex-1 min-w-0 overflow-auto">
          {!task && (
            <div className="h-full flex items-center justify-center text-xs" style={{ color: 'var(--text-tertiary)' }}>
              选择左侧任务查看详情
            </div>
          )}
          {task && activeOrgId && (
            <TaskDetail
              key={task.id}
              orgId={activeOrgId}
              task={task}
              tasks={tasks}
              members={members}
              memberLabel={memberLabel}
              kbOptions={kbOptions}
              myRole={detail?.org.myRole ?? ''}
              memberLoad={memberLoad}
            />
          )}
        </div>
      </div>

      {error && !newOpen && (
        <div className="shrink-0 px-3 py-1 text-[11px]" style={{ color: 'var(--status-danger)', borderTop: '1px solid var(--border)' }}>
          {error}
        </div>
      )}
        </div>
      </div>

      {/* 任务模板：选模板 + 填父任务标题 → 预览草稿（可改）→ 一次生成父任务与子任务 */}
      {tplOpen &&
        createPortal(
          <div
            className="fixed inset-0 z-[60] flex items-center justify-center"
            style={{ backgroundColor: 'rgba(0,0,0,0.35)' }}
          >
            <div
              className="w-[560px] max-h-[80vh] flex flex-col rounded-xl overflow-hidden"
              style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
            >
              <div
                className="shrink-0 px-4 py-2.5 flex items-center gap-2"
                style={{ borderBottom: '1px solid var(--border)' }}
              >
                <ListChecks size={13} style={{ color: 'var(--accent)' }} />
                <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                  {tplStage === 'pick' ? '从模板创建' : '模板草稿'}
                </span>
                <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  {tplStage === 'pick'
                    ? '模板会生成一个父任务 + 若干可交付子任务'
                    : '勾选需要创建的子任务；标题与描述可先改再创建'}
                </span>
                <div className="flex-1" />
                <button
                  onClick={closeTemplate}
                  className="text-xs"
                  style={{ color: 'var(--text-tertiary)' }}
                >
                  ✕
                </button>
              </div>
              <div className="flex-1 min-h-0 overflow-y-auto px-4 py-3 flex flex-col gap-2">
                <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
                  父任务标题
                  <input
                    autoFocus
                    className={FIELD_CLASS}
                    style={FIELD_STYLE}
                    value={tplTitle}
                    onChange={(e) => setTplTitle(e.target.value)}
                    placeholder="例如：2026 年一季度数据报送"
                  />
                </label>
                {tplStage === 'pick' ? (
                  TASK_TEMPLATES.map((tpl: TaskTemplate) => {
                    const on = tpl.id === tplId;
                    return (
                      <button
                        key={tpl.id}
                        onClick={() => setTplId(tpl.id)}
                        className="text-left rounded-lg p-2.5 flex flex-col gap-1"
                        style={{
                          backgroundColor: on ? 'var(--accent-light)' : 'var(--bg-field)',
                          border: `1px solid ${on ? 'var(--accent)' : 'var(--border)'}`,
                        }}
                      >
                        <span className="flex items-center gap-2">
                          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                            {tpl.name}
                          </span>
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                            {tpl.children.length} 个子任务
                          </span>
                        </span>
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                          {tpl.description}
                        </span>
                        <span className="text-[10px] truncate" style={{ color: 'var(--text-secondary)' }}>
                          {tpl.children.map((c) => c.title).join(' · ')}
                        </span>
                      </button>
                    );
                  })
                ) : (
                  <>
                    <div className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      <span>
                        已选 {tplDrafts.filter((d) => d.checked && d.title.trim()).length} / {tplDrafts.length}
                      </span>
                      <button
                        onClick={() => setTplDrafts((prev) => prev.map((d) => ({ ...d, checked: true })))}
                        style={{ color: 'var(--accent)' }}
                      >
                        全选
                      </button>
                      <button onClick={() => setTplDrafts((prev) => prev.map((d) => ({ ...d, checked: false })))}>
                        全不选
                      </button>
                    </div>
                    {tplDrafts.map((d, i) => (
                      <div
                        key={i}
                        className="flex items-start gap-2 rounded-lg p-2"
                        style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
                      >
                        <input
                          type="checkbox"
                          checked={d.checked}
                          onChange={() =>
                            setTplDrafts((prev) =>
                              prev.map((x, j) => (j === i ? { ...x, checked: !x.checked } : x)),
                            )
                          }
                          className="mt-1"
                          style={{ accentColor: 'var(--accent)' }}
                        />
                        <div className="flex-1 min-w-0 flex flex-col gap-1">
                          <input
                            value={d.title}
                            onChange={(e) =>
                              setTplDrafts((prev) =>
                                prev.map((x, j) => (j === i ? { ...x, title: e.target.value } : x)),
                              )
                            }
                            className="w-full px-1.5 py-0.5 rounded text-xs outline-none"
                            style={{
                              backgroundColor: 'var(--bg-content)',
                              color: 'var(--text-primary)',
                              border: '1px solid var(--border)',
                            }}
                          />
                          <input
                            value={d.description}
                            placeholder="交付物 / 验收点（可留空）"
                            onChange={(e) =>
                              setTplDrafts((prev) =>
                                prev.map((x, j) => (j === i ? { ...x, description: e.target.value } : x)),
                              )
                            }
                            className="w-full px-1.5 py-0.5 rounded text-[11px] outline-none"
                            style={{
                              backgroundColor: 'var(--bg-content)',
                              color: 'var(--text-secondary)',
                              border: '1px solid var(--border)',
                            }}
                          />
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                            优先级：{PRIORITY_LABEL[d.priority] ?? d.priority}
                          </span>
                        </div>
                      </div>
                    ))}
                  </>
                )}
                {localError && (
                  <span className="text-[11px]" style={{ color: 'var(--status-danger)' }}>
                    {localError}
                  </span>
                )}
              </div>
              <div
                className="shrink-0 px-4 py-2.5 flex items-center gap-2"
                style={{ borderTop: '1px solid var(--border)' }}
              >
                <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  {tplStage === 'pick'
                    ? '下一步可先核对 / 修改子任务，再创建'
                    : '创建后仍可指派负责人与截止时间'}
                </span>
                <div className="flex-1" />
                {tplStage === 'pick' ? (
                  <>
                    <button
                      onClick={closeTemplate}
                      className="text-xs px-2 py-1 rounded"
                      style={{
                        backgroundColor: 'var(--bg-tertiary)',
                        color: 'var(--text-secondary)',
                        border: '1px solid var(--border)',
                      }}
                    >
                      取消
                    </button>
                    <button
                      onClick={startTemplatePreview}
                      disabled={!tplTitle.trim()}
                      className="text-xs px-2 py-1 rounded"
                      style={{
                        backgroundColor: 'var(--accent)',
                        color: '#fff',
                        opacity: tplTitle.trim() ? 1 : 0.5,
                      }}
                    >
                      下一步：预览
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      onClick={() => {
                        setTplStage('pick');
                        setLocalError(null);
                      }}
                      className="text-xs px-2 py-1 rounded"
                      style={{
                        backgroundColor: 'var(--bg-tertiary)',
                        color: 'var(--text-secondary)',
                        border: '1px solid var(--border)',
                      }}
                    >
                      返回选择模板
                    </button>
                    <button
                      onClick={() => void createFromTemplate()}
                      disabled={
                        tplBusy || tplDrafts.filter((d) => d.checked && d.title.trim()).length === 0
                      }
                      className="text-xs px-2 py-1 rounded"
                      style={{
                        backgroundColor: 'var(--accent)',
                        color: '#fff',
                        opacity:
                          tplBusy || tplDrafts.filter((d) => d.checked && d.title.trim()).length === 0
                            ? 0.5
                            : 1,
                      }}
                    >
                      {tplBusy
                        ? '创建中…'
                        : `创建（${tplDrafts.filter((d) => d.checked && d.title.trim()).length} 个子任务）`}
                    </button>
                  </>
                )}
              </div>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}

function TaskNode(props: {
  task: TaskView;
  active: boolean;
  child?: boolean;
  onClick: () => void;
  /** 顶层任务行悬停时提供「添加子任务」快捷入口（预选父任务） */
  onAddChild?: () => void;
  /** 批量模式：显示勾选框 */
  selectable?: boolean;
  selected?: boolean;
  onToggleSelect?: () => void;
}) {
  const { task, active, child, onClick, onAddChild, selectable, selected, onToggleSelect } = props;
  const canAddChild =
    !child && !!onAddChild && task.status !== 'archived' && task.status !== 'cancelled';
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onClick();
        }
      }}
      className="group w-full text-left px-3 py-1.5 flex items-center gap-1.5 cursor-pointer"
      style={{
        backgroundColor: active ? 'var(--bg-tertiary)' : 'transparent',
        color: active ? 'var(--text-primary)' : 'var(--text-secondary)',
        paddingLeft: child ? 26 : 12,
      }}
      title={task.title}
    >
      {selectable && (
        <input
          type="checkbox"
          checked={!!selected}
          onClick={(e) => e.stopPropagation()}
          onChange={() => onToggleSelect?.()}
          className="shrink-0"
          style={{ accentColor: 'var(--accent)' }}
          title={selected ? '取消选择' : '选择该任务'}
        />
      )}
      {child ? (
        <ChevronRight size={11} style={{ color: 'var(--text-tertiary)' }} />
      ) : (
        <span className="w-[11px]" />
      )}
      <span
        className="shrink-0 w-1.5 h-1.5 rounded-full"
        style={{ backgroundColor: STATUS_COLOR[task.status] }}
      />
      <span className="text-xs truncate flex-1">{task.title}</span>
      {canAddChild && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onAddChild?.();
          }}
          className="shrink-0 p-0.5 rounded opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
          style={{ color: 'var(--text-tertiary)' }}
          title="添加子任务"
        >
          <Plus size={11} />
        </button>
      )}
      {task.childCount > 0 && (
        <span
          className="shrink-0 flex items-center gap-0.5 text-[9px]"
          style={{ color: 'var(--text-tertiary)' }}
          title={`汇总任务：${task.childCount} 个子任务`}
        >
          <ListTree size={10} />
          {task.childCount}
        </span>
      )}
      {task.blocked && <AlertTriangle size={11} style={{ color: 'var(--status-danger)' }} />}
    </div>
  );
}

interface Milestone {
  key: string;
  label: string;
  /** null = 尚未达成 */
  at: number | null;
}

/** 任务生命周期时间线：已达成的节点用主题色，未达成的留空。 */
function TaskTimeline({ milestones }: { milestones: Milestone[] }) {
  return (
    <div className="flex items-start">
      {milestones.map((m, i) => {
        const reached = m.at !== null;
        return (
          <div key={m.key} className="flex-1 min-w-0 flex flex-col items-center relative">
            {i > 0 && (
              <span
                style={{
                  position: 'absolute',
                  top: 4,
                  right: '50%',
                  width: '100%',
                  height: 1,
                  backgroundColor: reached ? 'var(--accent)' : 'var(--border)',
                }}
              />
            )}
            <span
              className="rounded-full"
              style={{
                position: 'relative',
                zIndex: 1,
                width: 9,
                height: 9,
                backgroundColor: reached ? 'var(--accent)' : 'var(--bg-content)',
                border: reached ? '1px solid var(--accent)' : '1px solid var(--border)',
              }}
            />
            <span
              className="text-[10px] mt-1.5 truncate max-w-full"
              style={{ color: reached ? 'var(--text-primary)' : 'var(--text-tertiary)' }}
            >
              {m.label}
            </span>
            <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
              {m.at ? formatDateTime(m.at) : '—'}
            </span>
          </div>
        );
      })}
    </div>
  );
}

function TaskDetail(props: {
  orgId: string;
  task: TaskView;
  tasks: TaskView[];
  members: MemberView[];
  memberLabel: (id: string | null) => string;
  /** 产出目标库候选（默认库 + 本组织自定义库） */
  kbOptions: { id: string; name: string; isImplicit: boolean }[];
  /** 当前登录用户在组织中的角色（仅 owner 可以使用 AI 拆解） */
  myRole: string;
  /** 成员在办任务数（userId → 数量）：负责人下拉里提示负载 */
  memberLoad: Record<string, number>;
}) {
  const { orgId, task, tasks, members, memberLabel, kbOptions, myRole, memberLoad } = props;
  // 权限按「当前登录身份 + 任务字段」本地复核（同时兼容快照未及时刷新）：
  // 非参与者不出现任何提交表单；创建者 / 负责人才能编辑与流转状态。
  const caps = useEnclaveOrgStore((s) => s.caps);
  const identityVerified = useEnclaveTaskStore((s) => s.identityVerified);
  const me = caps?.identity?.userId ?? '';
  const isCreator = !!me && me === task.creatorId;
  const isAssignee = !!me && me === task.assigneeId;
  const isCollaborator = !!me && task.collaborators.includes(me);
  const isParticipant = isCreator || isAssignee || isCollaborator;
  /** 已完成 / 已归档 / 已作废：内容只读（状态操作仍可归档、恢复汇报） */
  const isClosed = task.status === 'done' || task.status === 'archived' || task.status === 'cancelled';
  // 身份未校验（刷新失败 / 账号刚切换）时一律不出现写入口，避免旧身份越权
  const canEdit = identityVerified && (isCreator || isAssignee) && !isClosed;
  const canStatus = identityVerified && (isCreator || isAssignee);
  /** 后端只在「待办 / 已作废 / 已归档」且当前用户可删时返回 task.delete */
  const canDelete = identityVerified && task.effectiveActions.includes('task.delete');
  const canAssign = identityVerified && isCreator && !isClosed;
  const canReport = identityVerified && isParticipant;
  const canUpload = identityVerified && isParticipant && !isClosed;
  /** 有子任务 = 汇总容器：状态由子任务派生，自身不执行 */
  const isContainer = task.childCount > 0;
  /** AI 拆解只对主机 owner 开放（团队数据出境闸门在主机侧判定），且只拆顶层、未终态的任务 */
  const canSplit =
    identityVerified && myRole === 'owner' && !isContainer && !isClosed && !task.parentTaskId;

  const requestSplit = async () => {
    setSplitLoading(true);
    try {
      const drafts = await invoke<TaskSplitDraft[]>('enclave_task_split_preview', {
        orgId,
        taskId: task.id,
      });
      setSplitDrafts(drafts.map((d) => ({ ...d, checked: true })));
      setSplitOpen(true);
    } catch (e) {
      showToast(errorMessage(e), 'error');
    } finally {
      setSplitLoading(false);
    }
  };

  const createSplitTasks = async () => {
    const picked = splitDrafts.filter((d) => d.checked && d.title.trim());
    if (picked.length === 0) return;
    setSplitCreating(true);
    let done = 0;
    for (const d of picked) {
      const created = await useEnclaveTaskStore.getState().createTask(orgId, {
        title: d.title.trim(),
        description: d.description,
        parentTaskId: task.id,
        priority: d.priority,
      });
      if (created) done += 1;
    }
    setSplitCreating(false);
    setSplitOpen(false);
    setSplitDrafts([]);
    if (done > 0) {
      showToast(`已创建 ${done} 个子任务`, 'success');
      await useEnclaveTaskStore.getState().loadTasks(orgId);
      await useEnclaveTaskStore.getState().loadExtras(orgId, task.id);
    } else {
      showToast('子任务创建失败', 'error');
    }
  };

  /** 产出目标库显示名：空 = 默认库 */
  const kbLabel = (id: string): string =>
    id ? kbOptions.find((k) => k.id === id)?.name ?? '未知库' : '默认库';
  const transitions = isContainer ? CONTAINER_TRANSITIONS[task.status] : TRANSITIONS[task.status];
  const statusText =
    isContainer && task.status === 'submitted'
      ? '待汇总确认'
      : isContainer && task.status === 'pending'
        // 容器没有「开始」动作，「待办」容易被误读成可以执行 → 显示为未开始
        ? '未开始'
        : STATUS_LABEL[task.status];
  const blocked = task.blocked;
  /** 当前用户是否能自己把任务从待办推进到进行中（决定提示文案是「点开始」还是中性描述） */
  const canStartNow = canStatus && !blocked;
  const taskError = useEnclaveTaskStore((s) => s.error);
  const [desc, setDesc] = useState(task.description);
  const [editingDesc, setEditingDesc] = useState(false);
  const [saving, setSaving] = useState(false);

  // 属性 / 依赖：只读展示，点「修改」才出现编辑表单
  const [editingProps, setEditingProps] = useState(false);
  const [assignee, setAssignee] = useState(task.assigneeId ?? '');
  const [priority, setPriority] = useState(task.priority);
  const [due, setDue] = useState(toDateInput(task.dueDate));
  const [collaborators, setCollaborators] = useState<string[]>(task.collaborators);
  const [deps, setDeps] = useState<string[]>(task.blockedBy);
  /** 产出目标库草稿（空 = 默认库） */
  const [kbTarget, setKbTarget] = useState(task.targetKbId);
  const [preview, setPreview] = useState<DepsPreview | null>(null);

  // AI 拆解：模型只产出草稿，勾选后逐个 created（父任务仍是普通任务时会被自动转成汇总容器）
  const [splitLoading, setSplitLoading] = useState(false);
  const [splitOpen, setSplitOpen] = useState(false);
  const [splitCreating, setSplitCreating] = useState(false);
  const [splitDrafts, setSplitDrafts] = useState<
    { title: string; description: string; priority: string; checked: boolean }[]
  >([]);

  const reports = useEnclaveTaskStore((s) => s.reports);
  const files = useEnclaveTaskStore((s) => s.files);
  const progress = useEnclaveTaskStore((s) => s.progress);
  // 沟通层：任务讨论（与过程记录分开，任何非终态都可讨论）
  const comments = useEnclaveTaskStore((s) => s.comments);
  /** 详情分组：概览 / 协作 / 交付，避免单列承载全部内容 */
  const detailTabRequest = useEnclaveTaskStore((s) => s.detailTabRequest);
  const [detailTab, setDetailTab] = useState<TaskDetailTab>('overview');
  /** 通知跳转请求优先于本地选择；用户手动切换分组时清掉请求（见下方 tab 的 onClick） */
  const activeTab: TaskDetailTab =
    detailTabRequest?.taskId === task.id ? detailTabRequest.tab : detailTab;
  const [commentText, setCommentText] = useState('');
  const [replyTo, setReplyTo] = useState<TaskCommentView | null>(null);
  const [mentionIds, setMentionIds] = useState<string[]>([]);
  const discussionReadOnly = task.status === 'archived' || task.status === 'cancelled';
  const canComment =
    identityVerified && !discussionReadOnly &&
    (isParticipant || !!caps?.actions.includes('org.admin'));
  const canManageComments = identityVerified && !!caps?.actions.includes('org.admin');
  /** 只有「进行中」才能写过程记录 / 完成汇报：未开始时先走「开始」；容器只读历史 */
  const canWriteProgress =
    canReport && !isContainer && task.status === 'in_progress' && !blocked && !progress?.locked;
  /** 拆分转换后的容器：有历史执行记录就保留只读展示 */
  const hasExecutionHistory = (progress?.items.length ?? 0) > 0 || reports.length > 0;
  /** 任务资料：不挂汇报、不挂过程记录的独立附件 */
  const taskFiles = files.filter((f) => !f.reportId && !f.progressId);
  const [progressText, setProgressText] = useState('');
  const [composing, setComposing] = useState(false);
  const [pendingFiles, setPendingFiles] = useState<string[]>([]);

  const feedback = useEnclaveTaskStore((s) => s.feedback);
  // 当前草稿（唯一可编辑）；已退回/已提交/已确认都只作为提交日志展示
  const editable = useMemo(() => reports.find((r) => r.status === 'draft') ?? null, [reports]);
  const logs = useMemo(() => reports.filter((r) => r.status !== 'draft'), [reports]);
  /** 拆分转换后的容器：汇报区只读展示全部历史（含拆分前尚未提交的草稿） */
  const historyReports = isContainer ? reports : logs;
  const [reportText, setReportText] = useState('');
  // 等待审核或已确认时不允许再编辑 / 提交
  const lockedReport = reports.find((r) => r.status === 'submitted' || r.status === 'confirmed') ?? null;

  /** 生命周期里程碑：容器看子任务，普通任务看汇报与完成时间 */
  const milestones = useMemo<Milestone[]>(() => {
    const created = { key: 'created', label: '创建', at: task.createdAt };
    if (isContainer) {
      const children = tasks.filter((t) => t.parentTaskId === task.id);
      // 拆分前执行过的容器：把父任务自己的开始时间也算进去
      const startedCandidates = [task.startedAt, ...children.map((c) => c.startedAt)].filter(
        (v): v is number => v !== null,
      );
      const startedAt = startedCandidates.length > 0 ? Math.min(...startedCandidates) : null;
      if (task.status === 'cancelled') {
        return [
          created,
          { key: 'started', label: '子任务开始', at: startedAt },
          { key: 'cancelled', label: '作废', at: task.updatedAt },
        ];
      }
      const closedAt = children.reduce<number | null>(
        (max, c) => (c.completedAt !== null && (max === null || c.completedAt > max) ? c.completedAt : max),
        null,
      );
      const reachedConfirm =
        task.status === 'submitted' || task.status === 'done' || task.status === 'archived';
      return [
        created,
        { key: 'started', label: '子任务开始', at: startedAt },
        { key: 'ready', label: '待汇总确认', at: reachedConfirm ? closedAt : null },
        { key: 'done', label: '完成', at: task.completedAt },
        {
          key: 'archived',
          label: '归档',
          at: task.status === 'archived' ? task.updatedAt : null,
        },
      ];
    }

    if (task.status === 'cancelled') {
      return [
        created,
        { key: 'started', label: '开始', at: task.startedAt },
        { key: 'cancelled', label: '作废', at: task.updatedAt },
      ];
    }
    const submitted =
      logs.find((r) => r.status === 'submitted' || r.status === 'confirmed') ?? null;
    return [
      created,
      { key: 'started', label: '开始', at: task.startedAt },
      {
        key: 'submitted',
        label: '提交汇报',
        at: submitted ? (submitted.confirmedAt ?? submitted.updatedAt) : null,
      },
      { key: 'done', label: '完成', at: task.completedAt },
      {
        key: 'archived',
        label: '归档',
        at: task.status === 'archived' ? task.updatedAt : null,
      },
    ];
  }, [isContainer, task, tasks, logs]);

  const [reportBusy, setReportBusy] = useState(false);
  const [returningId, setReturningId] = useState<string | null>(null);
  const [returnReason, setReturnReason] = useState('');

  /** 最近一次退回原因（用于编辑器顶部提示） */
  const latestReturnReason = useMemo(() => {
    const returned = logs.find((r) => r.status === 'returned');
    if (!returned) return '';
    const list = feedback[returned.id] ?? [];
    return (
      list
        .filter((f) => f.action === 'return')
        .slice(-1)[0]?.content ?? ''
    );
  }, [logs, feedback]);

  useEffect(() => {
    void useEnclaveTaskStore.getState().loadExtras(orgId, task.id);
  }, [orgId, task.id]);

  // 任务收敛（完成 / 归档 / 作废）后，收起可能仍打开的编辑态
  useEffect(() => {
    if (isClosed) {
      setEditingProps(false);
      setEditingDesc(false);
    }
  }, [isClosed]);

  // 草稿到达后填入编辑器（仅首次，避免覆盖用户正在输入的内容）
  const [seededReportId, setSeededReportId] = useState<string | null>(null);
  useEffect(() => {
    if (editable && seededReportId !== editable.id) {
      setSeededReportId(editable.id);
      setReportText(editable.content);
    }
  }, [editable, seededReportId]);

  const change = async (status: TaskStatus) => {
    if (status === 'cancelled') {
      const unfinishedChildren = isContainer
        ? tasks.filter(
            (t) =>
              t.parentTaskId === task.id
              && t.status !== 'done'
              && t.status !== 'archived'
              && t.status !== 'cancelled',
          ).length
        : 0;
      const ok = await confirmDialog({
        title: '作废任务',
        message: isContainer
          ? `作废后将放弃整个汇总任务：其中 ${unfinishedChildren} 个未完成子任务（待办 / 进行中 / 待确认）会一并作废；已完成子任务的产出与知识库沉淀保留。\n\n是否继续？`
          : '作废后任务终止，不能再提交过程记录或完成汇报；已沉淀进知识库的历史产出不受影响。\n\n是否继续？',
        confirmText: '确认作废',
      });
      if (!ok) return;
    }
    if (status === 'archived' && isContainer) {
      const ok = await confirmDialog({
        title: '归档汇总任务',
        message:
          `归档后整棵子树会从「活跃任务」中收起：${task.childCount} 个子任务里已完成的会一并归档；`
          + '数据与知识库不受影响。',
        confirmText: '确认归档',
      });
      if (!ok) return;
    }
    await useEnclaveTaskStore.getState().setStatus(orgId, task.id, status);
  };

  /** 删除任务（待办 / 已作废 / 已归档）：任务本体 + 过程记录 / 讨论 / 汇报反馈 一并删除 */
  const removeTask = async () => {
    const ok = await confirmDialog({
      title: '删除任务',
      message:
        `将永久删除「${task.title}」及其全部记录（过程记录、讨论、完成汇报与反馈、附件挂载）`
        + (task.childCount > 0 ? `，以及 ${task.childCount} 个子任务` : '')
        + '。\n文件原件不会被删除（失去引用后可在知识库「文件」页清理）。此操作不可恢复。',
      confirmText: '永久删除',
    });
    if (!ok) return;
    const done = await useEnclaveTaskStore.getState().deleteTask(orgId, task.id);
    if (done) showToast('任务已删除', 'success');
    else showToast(useEnclaveTaskStore.getState().error ?? '删除失败', 'error');
  };

  /** 汇总确认：子任务全部收敛后收口完成（容器不单独沉淀知识库） */
  const confirmContainer = async () => {
    const ok = await confirmDialog({
      title: '汇总确认',
      message:
        `将检查 ${task.childCount} 个子任务的完成情况，确认后该汇总任务完成。\n`
        + '汇总任务不单独沉淀知识库，产出已由各子任务分别沉淀。',
      confirmText: '确认完成',
    });
    if (!ok) return;
    await useEnclaveTaskStore.getState().confirmContainer(orgId, task.id);
  };

  const uploadFile = async () => {
    const picked = await openDialog({ multiple: false, directory: false });
    if (typeof picked !== 'string') return;
    await useEnclaveTaskStore.getState().uploadFile(orgId, task.id, picked);
  };

  /** 添加汇报交付物：退回态先转草稿；还没有草稿时先落一份草稿，再挂文件 */
  const addDeliverable = async (reportId?: string) => {
    const st = useEnclaveTaskStore.getState();
    const target = reportId ? st.reports.find((r) => r.id === reportId) : null;
    let rid = reportId;
    if (!rid || (target && target.status !== 'draft')) {
      const ok = await st.saveReportDraft(orgId, task.id, reportText);
      if (!ok) return;
      rid = useEnclaveTaskStore
        .getState()
        .reports.find((r) => r.status === 'draft')?.id;
      if (!rid) return;
    }
    const picked = await openDialog({ multiple: false, directory: false });
    if (typeof picked !== 'string') return;
    await useEnclaveTaskStore.getState().uploadFile(orgId, task.id, picked, { reportId: rid });
  };

  /** 解除任务资料挂载（汇报交付物走 reportId 分支） */
  const removeTaskFile = async (fileId: string) => {
    const ok = await confirmDialog({
      title: '移除任务资料',
      message: '仅解除该文件与任务的挂载；若已无其他挂载，原件会一并删除。是否继续？',
      confirmText: '移除',
    });
    if (!ok) return;
    await useEnclaveTaskStore.getState().detachFile(orgId, task.id, fileId);
  };

  /** 选择过程记录的附件（提交时一并入库） */
  const pickProgressFiles = async () => {
    const picked = await openDialog({ multiple: true, directory: false });
    const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
    setPendingFiles((prev) => Array.from(new Set([...prev, ...list])));
  };

  const openFile = async (fileId: string) => {
    try {
      const path = await invoke<string>('enclave_file_abs_path', { orgId, fileId });
      await invoke('open_path', { path });
    } catch {
      // 单个文件打不开不影响列表；错误由 store 统一提示
    }
  };

  /** 发评论（支持回复与 @ 提及） */
  const submitComment = async () => {
    const text = commentText.trim();
    if (!text) return;
    const created = await useEnclaveTaskStore
      .getState()
      .addComment(orgId, task.id, text, replyTo?.id ?? null, mentionIds);
    if (created) {
      setCommentText('');
      setReplyTo(null);
      setMentionIds([]);
    } else {
      showToast(useEnclaveTaskStore.getState().error ?? '发送评论失败', 'error');
    }
  };

  const removeComment = async (c: TaskCommentView) => {
    const ok = await confirmDialog({
      title: '删除评论',
      message: '删除后不再显示（保留软删留痕）。是否继续？',
      confirmText: '删除',
    });
    if (!ok) return;
    await useEnclaveTaskStore.getState().deleteComment(orgId, task.id, c.id);
  };

  const mentionable = members.filter((m) => m.seatStatus !== 'removed' && m.userId !== me);

  return (
    <div className="p-4 flex flex-col gap-4">
      {/* 任务信息区：标题行作为 fieldset 标题 */}
      <fieldset
        className="m-0 min-w-0 p-0"
        style={{
          border: '1px solid var(--border)',
          borderRadius: 'var(--radius-lg)',
          padding: '10px 12px 12px',
        }}
      >
        <legend className="px-1.5 m-0">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
          {task.title}
        </span>
        <span className="text-[11px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: STATUS_COLOR[task.status] }}>
          {statusText}
        </span>
        {isContainer && (
          <span
            className="text-[11px] px-1.5 py-0.5 rounded flex items-center gap-1"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
            title="状态由子任务派生；汇总任务不单独执行、不单独沉淀知识库"
          >
            <ListTree size={10} /> 汇总 · {task.childCount} 个子任务
          </span>
        )}
        <span className="text-[11px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
          {PRIORITY_LABEL[task.priority] ?? task.priority}
        </span>
        {task.blocked && (
          <span className="text-[11px] px-1.5 py-0.5 rounded flex items-center gap-1" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--status-danger)' }}>
            <AlertTriangle size={10} /> 被前置任务阻塞
          </span>
        )}
      </div>
        </legend>
        <div className="flex flex-col gap-3">

      {/* 详情分组：概览 / 协作 / 交付 */}
      <div
        role="tablist"
        className="flex items-center gap-0.5 p-0.5 rounded-lg self-start"
        style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
      >
        {([
          { key: 'overview', label: '概览' },
          { key: 'collab', label: '协作', count: comments.length + (progress?.items.length ?? 0) },
          { key: 'delivery', label: '交付', count: reports.length },
        ] as const).map((t) => {
          const on = activeTab === t.key;
          return (
            <button
              key={t.key}
              role="tab"
              aria-selected={on}
              onClick={() => {
                setDetailTab(t.key);
                // 手动切分组后，通知跳转请求不再生效
                useEnclaveTaskStore.getState().clearDetailTabRequest();
              }}
              className="text-[11px] px-2.5 py-1 rounded-md flex items-center gap-1"
              style={{
                backgroundColor: on ? 'var(--bg-content)' : 'transparent',
                color: on ? 'var(--text-primary)' : 'var(--text-secondary)',
                border: 'none',
                boxShadow: on ? '0 1px 2px rgba(0,0,0,0.12)' : 'none',
              }}
            >
              {t.label}
              {'count' in t && t.count > 0 && (
                <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                  {t.count}
                </span>
              )}
            </button>
          );
        })}
      </div>

      {/* 生命周期时间线：当前节点与全流程里程碑一眼可见 */}
      <div
        className="rounded-lg px-4 py-3"
        style={{ backgroundColor: 'var(--bg-side)', border: '1px solid var(--border)' }}
      >
        <TaskTimeline milestones={milestones} />
      </div>

      {taskError && (
        <div
          className="text-[11px] px-2 py-1 rounded"
          style={{ color: 'var(--status-danger)', backgroundColor: 'var(--bg-tertiary)' }}
        >
          {taskError}
        </div>
      )}

      {!identityVerified && (
        <div
          className="text-[11px] px-2 py-1 rounded"
          style={{ color: 'var(--status-danger)', backgroundColor: 'var(--bg-tertiary)' }}
        >
          身份未校验（刷新失败或账号刚切换），已暂时隐藏所有编辑与提交入口
        </div>
      )}

      {activeTab === 'overview' && (
        <>
      {/* 任务信息（属性 + 依赖都直接展示；修改时才出现表单） */}
      <div className="flex flex-col gap-2">
        <div className="flex items-center gap-1.5">
          <Info size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>任务信息</span>
          <div className="flex-1" />
          {canSplit && !editingProps && (
            <button
              onClick={() => void requestSplit()}
              disabled={splitLoading}
              className="text-[11px] px-2 py-0.5 rounded flex items-center gap-1"
              style={{
                backgroundColor: 'var(--accent-light)',
                color: 'var(--accent)',
                border: '1px solid var(--border)',
                opacity: splitLoading ? 0.6 : 1,
              }}
              title="用模型把当前任务拆成子任务草稿（仅主机可用，需开启「团队数据可发送至云端模型」）"
            >
              {splitLoading ? <Loader2 size={11} className="animate-spin" /> : <Wand2 size={11} />}
              AI 拆解
            </button>
          )}
          {!editingProps && (canEdit || canAssign) && (
            <button
              onClick={() => setEditingProps(true)}
              className="text-[11px] px-2 py-0.5 rounded"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
            >
              修改
            </button>
          )}
        </div>

        {!editingProps ? (
          // 允许换行：每项独立成行并带列表标记，长内容自然折行
          <ul
            className="m-0 pl-4 list-disc flex flex-col gap-0.5 text-[11px] leading-5"
            style={{ color: 'var(--text-secondary)' }}
          >
            <li>创建者：{memberLabel(task.creatorId)}</li>
            <li>负责人：{memberLabel(task.assigneeId)}</li>
            <li>优先级：{PRIORITY_LABEL[task.priority] ?? task.priority}</li>
            <li>截止：{task.dueDate ? toDateInput(task.dueDate) : '未设置'}</li>
            {!isContainer && (
              <li>产出目标库：{kbLabel(task.targetKbId)}</li>
            )}
            <li>
              {`参与者：${
                task.collaborators.length
                  ? task.collaborators.map((id) => memberLabel(id)).join('、')
                  : '无'
              }`}
            </li>
            <li>
              {`前置依赖：${
                task.blockedBy.length
                  ? task.blockedBy
                    .map((id) => {
                      const dep = tasks.find((t) => t.id === id);
                      return dep ? `${dep.title}（${STATUS_LABEL[dep.status]}）` : id;
                    })
                    .join('、')
                  : '无'
              }`}
            </li>
          </ul>
        ) : (
          <>
            <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
              负责人
              <Select
                value={assignee}
                onChange={setAssignee}
                disabled={!canAssign}
                className="w-full"
                options={[
                  { value: '', label: '未指派' },
                  ...members
                    .filter((m) => m.seatStatus !== 'removed')
                    .map((m) => ({
                      value: m.userId,
                      // 负载提示：换人前能直接看到谁手里活多
                      label: `${m.displayName || m.email}${
                        memberLoad[m.userId] ? `（在办 ${memberLoad[m.userId]}）` : ''
                      }`,
                    })),
                ]}
                title="负责人（括号内为该成员当前在办任务数）"
              />
              {!canAssign && <span className="text-[10px]">仅创建者可修改负责人</span>}
            </label>
            <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
              优先级
              <Select
                value={priority}
                onChange={setPriority}
                className="w-full"
                options={Object.entries(PRIORITY_LABEL).map(([k, v]) => ({ value: k, label: v }))}
              />
            </label>
            <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
              截止日期
              <input
                type="date"
                className={FIELD_CLASS}
                style={FIELD_STYLE}
                value={due}
                onChange={(e) => setDue(e.target.value)}
              />
            </label>
            {!isContainer && (
              <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
                产出目标库
                <Select
                  value={kbTarget}
                  onChange={setKbTarget}
                  className="w-full"
                  options={[
                    { value: '', label: '默认库（团队知识库）' },
                    ...kbOptions
                      .filter((k) => !k.isImplicit)
                      .map((k) => ({ value: k.id, label: k.name })),
                  ]}
                  title="汇报确认后，任务产出沉淀到哪个知识库（与任务状态「归档」无关；已沉淀的条目不会自动搬家）"
                />
              </label>
            )}

            <div className="flex flex-col gap-1">
              <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                参与者（可提交汇报 / 上传附件）
              </span>
              <div className="max-h-[140px] overflow-auto rounded" style={{ border: '1px solid var(--border)' }}>
                {members
                  .filter((m) => m.seatStatus !== 'removed')
                  .filter((m) => m.userId !== task.creatorId && m.userId !== task.assigneeId)
                  .map((m) => (
                    <label
                      key={m.userId}
                      className="flex items-center gap-2 px-2 py-1 text-[11px] cursor-pointer"
                      style={{ color: 'var(--text-secondary)' }}
                    >
                      <input
                        type="checkbox"
                        checked={collaborators.includes(m.userId)}
                        onChange={() =>
                          setCollaborators((prev) =>
                            prev.includes(m.userId)
                              ? prev.filter((x) => x !== m.userId)
                              : [...prev, m.userId],
                          )
                        }
                      />
                      <span className="truncate">{m.displayName || m.email}</span>
                    </label>
                  ))}
                {members.filter(
                  (m) => m.seatStatus !== 'removed'
                    && m.userId !== task.creatorId
                    && m.userId !== task.assigneeId,
                ).length === 0 && (
                  <div className="px-2 py-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    没有可添加的成员
                  </div>
                )}
              </div>
            </div>

            <div className="flex flex-col gap-1">
              <span className="text-[11px] flex items-center gap-1" style={{ color: 'var(--text-tertiary)' }}>
                <Link2 size={11} style={{ color: 'var(--accent)' }} /> 前置依赖（全部完成才解锁）
              </span>
              <div className="max-h-[180px] overflow-auto rounded" style={{ border: '1px solid var(--border)' }}>
                {tasks.filter((t) => t.id !== task.id && t.parentTaskId === task.parentTaskId).length === 0 && (
                  <div className="px-2 py-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    同层没有其它任务
                  </div>
                )}
                {tasks
                  .filter((t) => t.id !== task.id && t.parentTaskId === task.parentTaskId)
                  .map((t) => (
                    <label
                      key={t.id}
                      className="flex items-center gap-2 px-2 py-1 text-[11px] cursor-pointer"
                      style={{ color: 'var(--text-secondary)' }}
                    >
                      <input
                        type="checkbox"
                        checked={deps.includes(t.id)}
                        onChange={() => {
                          const next = deps.includes(t.id)
                            ? deps.filter((x) => x !== t.id)
                            : [...deps, t.id];
                          setDeps(next);
                          void useEnclaveTaskStore
                            .getState()
                            .previewDeps(orgId, task.id, next)
                            .then(setPreview);
                        }}
                      />
                      <span className="truncate flex-1">{t.title}</span>
                      <span style={{ color: STATUS_COLOR[t.status] }}>
                        {t.childCount > 0 && t.status === 'pending' ? '未开始' : STATUS_LABEL[t.status]}
                      </span>
                    </label>
                  ))}
              </div>
              {preview && (
                <div
                  className="text-[10px] leading-4"
                  style={{ color: preview.error ? 'var(--status-danger)' : 'var(--text-tertiary)' }}
                >
                  {preview.error
                    ? preview.error
                    : [
                      preview.newlyBlockedIds.length ? '保存后将被阻塞' : '',
                      preview.newlyUnblockedIds.length ? '保存后将解锁' : '',
                    ].filter(Boolean).join(' · ') || '无影响'}
                  {preview.cycleIds.length > 0 && `｜环：${preview.cycleIds.join(' → ')}`}
                </div>
              )}
            </div>

            <div className="flex items-center gap-2">
              <button
                onClick={async () => {
                  const ok = await useEnclaveTaskStore.getState().updateTask(orgId, task.id, {
                    ...(canAssign ? { assigneeId: assignee || null } : {}),
                    priority,
                    dueDate: fromDateInput(due),
                    ...(isContainer ? {} : { targetKbId: kbTarget }),
                  });
                  const ok2 = await useEnclaveTaskStore
                    .getState()
                    .setCollaborators(orgId, task.id, collaborators);
                  const ok3 = await useEnclaveTaskStore.getState().setDeps(orgId, task.id, deps);
                  if (ok && ok2 && ok3) {
                    setEditingProps(false);
                    setPreview(null);
                  }
                }}
                className="text-xs px-2 py-1 rounded"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
              >
                保存
              </button>
              <button
                onClick={() => {
                  setAssignee(task.assigneeId ?? '');
                  setPriority(task.priority);
                  setDue(toDateInput(task.dueDate));
                  setKbTarget(task.targetKbId);
                  setCollaborators(task.collaborators);
                  setDeps(task.blockedBy);
                  setPreview(null);
                  setEditingProps(false);
                }}
                className="text-xs px-2 py-1 rounded"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                取消
              </button>
            </div>
          </>
        )}
      </div>

      <div className="flex flex-col gap-1">
        <div className="flex items-center gap-2">
          <FileText size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>任务描述</span>
          <div className="flex-1" />
          {canEdit && !editingDesc && (
            <button
              onClick={() => { setDesc(task.description); setEditingDesc(true); }}
              className="text-[11px] px-2 py-0.5 rounded"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
            >
              修改
            </button>
          )}
        </div>
        {editingDesc ? (
          <>
            <textarea
              className={TEXTAREA_CLASS}
              style={{ ...FIELD_STYLE, minHeight: 120 }}
              value={desc}
              onChange={(e) => setDesc(e.target.value)}
              autoFocus
            />
            <div className="flex items-center gap-2">
              <button
                onClick={async () => {
                  setSaving(true);
                  const ok = await useEnclaveTaskStore.getState().updateTask(orgId, task.id, { description: desc });
                  setSaving(false);
                  if (ok) setEditingDesc(false);
                }}
                disabled={saving}
                className="text-xs px-2 py-1 rounded"
                style={{ backgroundColor: 'var(--accent)', color: '#fff', opacity: saving ? 0.6 : 1 }}
              >
                {saving ? '保存中…' : '保存'}
              </button>
              <button
                onClick={() => setEditingDesc(false)}
                className="text-xs px-2 py-1 rounded"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                取消
              </button>
            </div>
          </>
        ) : (
          <div
            className="text-xs whitespace-pre-wrap leading-6"
            style={{ color: task.description ? 'var(--text-secondary)' : 'var(--text-tertiary)' }}
          >
            {task.description || '（无描述）'}
          </div>
        )}
      </div>

      {/* 任务资料（标题与其它任务内容平级，附件列表独立成块） */}
      <div className="flex flex-col gap-2">
        <div className="flex items-center gap-2">
          <Paperclip size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>任务资料</span>
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{taskFiles.length}</span>
          <div className="flex-1" />
          {canUpload && (
            <button
              onClick={() => void uploadFile()}
              className="text-xs px-2 py-1 rounded"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
            >
              上传文件
            </button>
          )}
        </div>
        <div
          className="flex flex-col gap-2 rounded-lg p-3"
          style={{ backgroundColor: 'var(--bg-side)', border: '1px solid var(--border)' }}
        >
          {taskFiles.length === 0 ? (
            <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              暂无任务资料（汇报交付物在完成汇报里，过程附件在过程记录里）
            </div>
          ) : (
            taskFiles.map((f) => (
              <div key={f.id} className="attachment-row flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                <span className="truncate flex-1">{f.fileName}</span>
                <span style={{ color: 'var(--text-tertiary)' }}>{Math.max(1, Math.round(f.fileSize / 1024))} KB</span>
                <button
                  onClick={() => void openFile(f.id)}
                  className="px-1.5 py-0.5 rounded"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                >
                  打开
                </button>
                {canUpload && (
                  <button
                    onClick={() => void removeTaskFile(f.id)}
                    className="px-1.5 py-0.5 rounded"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--status-danger)', border: '1px solid var(--border)' }}
                  >
                    移除
                  </button>
                )}
              </div>
            ))
          )}
        </div>
      </div>

        </>
      )}

      {activeTab === 'collab' && (
        <>
      {/* 沟通层：任务讨论（支持回复与 @ 提及；归档 / 作废后只读） */}
      <div
        className="rounded-lg p-3 flex flex-col gap-2"
        style={{ backgroundColor: 'var(--bg-side)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2">
          <Info size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
            讨论
          </span>
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            {comments.length}
          </span>
          <div className="flex-1" />
          {discussionReadOnly && (
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              任务已归档 / 作废，讨论只读
            </span>
          )}
        </div>

        {comments.length === 0 && (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            还没有讨论。任务相关的问题、决策与澄清都可以留在这里。
          </div>
        )}

        {comments.map((c) => {
          const parent = c.replyToId ? comments.find((x) => x.id === c.replyToId) : null;
          const canDelete = canManageComments || c.authorId === me;
          return (
            <div
              key={c.id}
              className="rounded-lg px-2.5 py-2 flex flex-col gap-1"
              style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
            >
              <div className="flex items-center gap-1.5 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                <span className="font-medium" style={{ color: 'var(--text-secondary)' }}>
                  {memberLabel(c.authorId)}
                </span>
                <span>{formatDateTime(c.createdAt)}</span>
                {parent && <span>· 回复 {memberLabel(parent.authorId)}</span>}
                <div className="flex-1" />
                {canComment && (
                  <button
                    onClick={() => setReplyTo(c)}
                    className="px-1 rounded"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    回复
                  </button>
                )}
                {canDelete && !discussionReadOnly && (
                  <button
                    onClick={() => void removeComment(c)}
                    className="px-1 rounded"
                    style={{ color: 'var(--status-danger)' }}
                  >
                    删除
                  </button>
                )}
              </div>
              <div
                className="text-[11px] whitespace-pre-wrap break-words"
                style={{ color: 'var(--text-primary)' }}
              >
                {c.content}
              </div>
              {c.mentions.length > 0 && (
                <div className="text-[10px]" style={{ color: 'var(--accent)' }}>
                  {c.mentions.map((id) => `@${memberLabel(id)}`).join(' ')}
                </div>
              )}
            </div>
          );
        })}

        {canComment ? (
          <div className="flex flex-col gap-1.5">
            {replyTo && (
              <div className="flex items-center gap-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                <span>回复 {memberLabel(replyTo.authorId)}</span>
                <button onClick={() => setReplyTo(null)} style={{ color: 'var(--text-tertiary)' }}>
                  取消
                </button>
              </div>
            )}
            {mentionIds.length > 0 && (
              <div className="flex items-center gap-1 flex-wrap text-[10px]">
                {mentionIds.map((id) => (
                  <button
                    key={id}
                    onClick={() => setMentionIds((prev) => prev.filter((x) => x !== id))}
                    className="px-1.5 py-0.5 rounded"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--accent)' }}
                    title="点击移除提及"
                  >
                    @{memberLabel(id)} ×
                  </button>
                ))}
              </div>
            )}
            <textarea
              value={commentText}
              onChange={(e) => setCommentText(e.target.value)}
              placeholder="写下讨论内容（Ctrl+Enter 发送）"
              rows={2}
              onKeyDown={(e) => {
                if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
                  e.preventDefault();
                  void submitComment();
                }
              }}
              className="w-full p-2 rounded-lg text-[11px] leading-relaxed outline-none resize-y"
              style={{
                backgroundColor: 'var(--bg-content)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
              }}
            />
            <div className="flex items-center gap-1.5">
              {mentionable.length > 0 && (
                <Select
                  value=""
                  onChange={(id) => {
                    if (id && !mentionIds.includes(id)) {
                      setMentionIds((prev) => [...prev, id]);
                    }
                  }}
                  options={mentionable.map((m) => ({
                    value: m.userId,
                    label: m.displayName || m.email || m.userId,
                  }))}
                  placeholder="@提及成员"
                  size="xs"
                  className="w-[110px]"
                />
              )}
              <div className="flex-1" />
              <button
                onClick={() => void submitComment()}
                disabled={!commentText.trim()}
                className="h-6 px-2 rounded text-[10px]"
                style={{
                  backgroundColor: 'var(--accent)',
                  color: '#fff',
                  opacity: commentText.trim() ? 1 : 0.5,
                }}
              >
                发送
              </button>
            </div>
          </div>
        ) : (
          !discussionReadOnly && (
            <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              只有任务参与者与组织管理者可以参与讨论
            </div>
          )
        )}
      </div>
        </>
      )}

      {activeTab === 'overview' && (
        <>
      {/* 汇总容器：状态由子任务派生，全部收敛后可汇总确认 */}
      {isContainer && (
        <div
          className="flex items-center gap-2 flex-wrap rounded-lg p-3"
          style={{ backgroundColor: 'var(--bg-side)', border: '1px solid var(--border)' }}
        >
          <ListTree size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
            汇总任务
          </span>
          <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            {hasExecutionHistory
              ? `状态由 ${task.childCount} 个子任务派生；拆分前的执行记录已保留为只读历史`
              : `状态由 ${task.childCount} 个子任务派生；自身不执行、不单独沉淀知识库`}
          </span>
          <div className="flex-1" />
          {task.status === 'submitted' && canStatus && (
            <button
              onClick={() => void confirmContainer()}
              className="pd-btn text-xs px-2.5 py-1 rounded-lg"
              style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
              title="检查子任务产出后收口完成"
            >
              汇总确认
            </button>
          )}
        </div>
      )}

      {/* 状态操作：无权限或无可用流转时整块不显示（待确认只由完成汇报驱动） */}
      {canStatus && transitions.length > 0 && (
        <div className="flex items-center gap-2 flex-wrap">
          <Flag size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>状态操作</span>
          {transitions.map((t) => (
            <button
              key={t.to}
              onClick={() => void change(t.to)}
              className="text-xs px-2 py-1 rounded"
              title={
                t.to === 'archived'
                  ? '归档：从任务树隐藏并保持只读，可在「显示已归档」中查看'
                  : t.to === 'in_progress' && (task.status === 'done' || task.status === 'archived')
                    ? '恢复汇报：重新进入进行中，可补充正文 / 交付物后重新提交'
                    : undefined
              }
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-secondary)',
                border: '1px solid var(--border)',
              }}
            >
              {t.label}
            </button>
          ))}
        </div>
      )}

      {/* 删除：待办 / 已作废 / 已归档可用，权限由后端 effectiveActions 判定 */}
      {canDelete && (
        <div className="flex items-center gap-2 flex-wrap">
          <Trash2 size={12} style={{ color: 'var(--status-danger)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>任务清理</span>
          <button
            onClick={() => void removeTask()}
            className="text-xs px-2 py-1 rounded"
            title="永久删除任务本体、过程记录、讨论、完成汇报与反馈；文件原件保留"
            style={{
              color: 'var(--status-danger)',
              backgroundColor: 'var(--bg-tertiary)',
              border: '1px solid var(--border)',
            }}
          >
            删除任务
          </button>
        </div>
      )}
        </>
      )}
        </div>
      </fieldset>

      {/* 提交任务区：普通任务可写；拆分后的容器只读展示历史记录 */}
      {activeTab === 'collab' && (!isContainer || hasExecutionHistory) && (
      <fieldset
        className="m-0 min-w-0 p-0"
        style={{
          border: '1px solid var(--border)',
          borderRadius: 'var(--radius-lg)',
          padding: '10px 12px 12px',
        }}
      >
        <legend className="px-1.5 m-0 text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
          {isContainer ? '历史过程记录' : '过程记录'}
        </legend>
        <div className="flex flex-col gap-3">
      {(!isContainer || (progress?.items.length ?? 0) > 0) && (
      <div
        className="flex flex-col gap-2 rounded-lg p-3"
        style={{ backgroundColor: 'var(--bg-side)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2">
          <ListChecks size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
            过程记录
          </span>
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            {progress?.items.length ?? 0}
          </span>
          <div className="flex-1" />
          {progress?.locked && (
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              {progress.lockReason}
            </span>
          )}
        </div>

        {canReport && task.status === 'pending' && (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            {canStartNow
              ? '任务尚未开始：点上方「开始」后才能记录过程。'
              : '任务尚未开始，开始后才能记录过程。'}
          </div>
        )}

        {/* 默认只展示列表；点「添加过程记录」才展开表单（前置未完成时不允许记录） */}
        {canWriteProgress && !composing && (
          <button
            onClick={() => setComposing(true)}
            className="self-start text-xs px-2 py-1 rounded flex items-center gap-1"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
          >
            <Plus size={12} /> 添加过程记录
          </button>
        )}

        {canWriteProgress && composing && (
          <div
            className="flex flex-col gap-2 rounded p-2"
            style={{ border: '1px solid var(--border)', backgroundColor: 'var(--bg-field)' }}
          >
            <textarea
              className={TEXTAREA_CLASS}
              style={{ ...FIELD_STYLE, minHeight: 70, backgroundColor: 'var(--bg-content)' }}
              placeholder="记录本次进度：做了什么、遇到什么、下一步（可多次提交）"
              value={progressText}
              onChange={(e) => setProgressText(e.target.value)}
              autoFocus
            />
            <div className="flex items-center gap-2">
              <button
                onClick={() => void pickProgressFiles()}
                className="text-[11px] px-2 py-0.5 rounded flex items-center gap-1"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                <Paperclip size={11} /> 选择附件
              </button>
              <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                {pendingFiles.length ? `已选 ${pendingFiles.length} 个文件` : '可附过程成果 / 支撑文件'}
              </span>
            </div>
            {pendingFiles.map((f) => (
              <div key={f} className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                <span className="truncate flex-1">{f.split(/[\\/]/).pop() || f}</span>
                <button
                  onClick={() => setPendingFiles((prev) => prev.filter((x) => x !== f))}
                  className="px-1.5 py-0.5 rounded"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                >
                  移除
                </button>
              </div>
            ))}
            <div className="flex items-center gap-2">
              <button
                onClick={async () => {
                  const ok = await useEnclaveTaskStore
                    .getState()
                    .addProgress(orgId, task.id, progressText, pendingFiles);
                  if (ok) {
                    setProgressText('');
                    setPendingFiles([]);
                    setComposing(false);
                  }
                }}
                disabled={!progressText.trim()}
                className="text-xs px-3 py-1 rounded"
                style={{
                  backgroundColor: 'var(--accent)',
                  color: '#fff',
                  opacity: progressText.trim() ? 1 : 0.5,
                }}
              >
                提交
              </button>
              <button
                onClick={() => {
                  setProgressText('');
                  setPendingFiles([]);
                  setComposing(false);
                }}
                className="text-xs px-2 py-1 rounded"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                取消
              </button>
            </div>
          </div>
        )}

        {progress?.items.map((p) => (
          <div
            key={p.id}
            className="rounded px-2 py-1.5 flex flex-col gap-1"
            style={{ backgroundColor: 'var(--bg-tertiary)' }}
          >
            <div className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              <span>{memberLabel(p.authorId)}</span>
              <span>{formatDateTime(p.createdAt)}</span>
            </div>
            <div className="text-[11px] whitespace-pre-wrap" style={{ color: 'var(--text-secondary)' }}>
              {p.content}
            </div>
            {p.attachments.length > 0 && (
              <div
                className="flex flex-col gap-1 rounded-md px-2 py-1.5"
                style={{ border: '1px dashed var(--border)' }}
              >
                {p.attachments.map((f) => (
                  <div key={f.id} className="attachment-row flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                    <Paperclip size={9} style={{ color: 'var(--text-tertiary)' }} />
                    <span className="truncate flex-1">{f.fileName}</span>
                    <button
                      onClick={() => void openFile(f.id)}
                      className="px-1.5 py-0.5 rounded"
                      style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                    >
                      打开
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}

        {(!progress || progress.items.length === 0) && (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            还没有过程记录
          </div>
        )}
      </div>
      )}
        </div>
      </fieldset>
      )}

      {/* 完成汇报（普通任务可写；容器只读展示历史） */}
      {activeTab === 'delivery' && (!isContainer || reports.length > 0) && (
      <fieldset
        className="m-0 min-w-0 p-0"
        style={{
          border: '1px solid var(--border)',
          borderRadius: 'var(--radius-lg)',
          padding: '10px 12px 12px',
        }}
      >
        <legend
          className="px-1.5 m-0 flex items-center gap-1 text-[11px] font-medium"
          style={{ color: 'var(--text-secondary)' }}
        >
          <ClipboardCheck size={12} style={{ color: 'var(--accent)' }} />
          完成汇报
        </legend>
        <div className="flex flex-col gap-2">
        {!isContainer && lockedReport && (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            {lockedReport.status === 'submitted'
              ? '完成汇报已提交，等待创建者 / 负责人确认（不可重复提交）'
              : '完成汇报已确认，任务已收敛'}
          </div>
        )}
        {!isContainer && blocked && (
          <div className="text-[11px]" style={{ color: 'var(--status-danger)' }}>
            存在未完成的前置任务，暂不能开始 / 提交
          </div>
        )}
        {!isContainer && canReport && task.status === 'pending' && (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            {canStartNow
              ? '任务尚未开始：点上方「开始」后才能填写完成汇报。'
              : '任务尚未开始，开始后才能填写完成汇报。'}
          </div>
        )}
        {!isContainer && canReport && task.status === 'in_progress' && !lockedReport && !blocked && (editable || logs.length === 0) && (
          <>
            {latestReturnReason && (
              <div
                className="text-[11px] px-2 py-1 rounded"
                style={{ color: 'var(--status-danger)', backgroundColor: 'var(--bg-tertiary)' }}
              >
                上次退回原因：{latestReturnReason}
              </div>
            )}
            <textarea
              className={TEXTAREA_CLASS}
              style={{ ...FIELD_STYLE, minHeight: 90 }}
              placeholder="写清做了什么、产出在哪（确认后会沉淀进组织知识库）"
              value={reportText}
              onChange={(e) => setReportText(e.target.value)}
            />
            <div className="flex items-center gap-2">
              <button
                onClick={() => void addDeliverable(editable?.id)}
                disabled={reportBusy}
                className="text-xs px-2 py-1 rounded flex items-center gap-1"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                <Paperclip size={11} /> 添加交付物
              </button>
              <div className="flex-1" />
              <button
                onClick={async () => {
                  setReportBusy(true);
                  await useEnclaveTaskStore.getState().saveReportDraft(orgId, task.id, reportText);
                  setReportBusy(false);
                }}
                disabled={reportBusy}
                className="text-xs px-2 py-1 rounded"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                保存草稿
              </button>
              <button
                onClick={async () => {
                  setReportBusy(true);
                  await useEnclaveTaskStore.getState().submitReport(orgId, task.id, reportText);
                  setReportBusy(false);
                }}
                disabled={reportBusy || task.status !== 'in_progress'}
                className="text-xs px-2 py-1 rounded"
                style={{
                  backgroundColor: 'var(--accent)',
                  color: '#fff',
                  opacity: reportBusy || task.status !== 'in_progress' ? 0.5 : 1,
                }}
                title={task.status !== 'in_progress' ? '请先把任务置为「进行中」' : ''}
              >
                提交汇报
              </button>
            </div>

            {/* 当前草稿的交付物（与下方「汇报记录」分开） */}
            {editable && (() => {
              const deliverables = files.filter((f) => f.reportId === editable.id);
              return (
                <div
                  className="flex flex-col gap-1 rounded-md px-2 py-1.5"
                  style={{ border: '1px dashed var(--border)' }}
                >
                  <div className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    <Paperclip size={10} />
                    <span>本条交付物 {deliverables.length}</span>
                  </div>
                  {deliverables.map((f) => (
                    <div key={f.id} className="attachment-row flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                      <span className="truncate flex-1">{f.fileName}</span>
                      <button
                        onClick={() => void openFile(f.id)}
                        className="px-1.5 py-0.5 rounded"
                        style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                      >
                        打开
                      </button>
                      <button
                        onClick={() =>
                          void useEnclaveTaskStore
                            .getState()
                            .detachFile(orgId, task.id, f.id, { reportId: editable.id })
                        }
                        className="px-1.5 py-0.5 rounded"
                        style={{ backgroundColor: 'var(--bg-field)', color: 'var(--status-danger)', border: '1px solid var(--border)' }}
                      >
                        移除
                      </button>
                    </div>
                  ))}
                  {deliverables.length === 0 && (
                    <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      尚未添加交付物（点上方「添加交付物」）
                    </div>
                  )}
                </div>
              );
            })()}
          </>
        )}

        {/* 汇报提交日志：只读留痕（含每次退回原因），与上方编辑区分离 */}
        {historyReports.length > 0 && (
          <div className="flex flex-col gap-2" style={{ borderTop: '1px solid var(--border)', paddingTop: 8 }}>
            <div className="flex items-center gap-2">
              <History size={11} style={{ color: 'var(--accent)' }} />
              <span className="text-[10px] font-medium" style={{ color: 'var(--text-tertiary)' }}>
                汇报记录（{historyReports.length}）
              </span>
            </div>
            {historyReports.map((r) => (
              <div
                key={r.id}
                className="rounded px-2 py-1.5 flex flex-col gap-1"
                style={{ backgroundColor: 'var(--bg-tertiary)' }}
              >
                <div className="flex items-center gap-2 text-[11px]">
                  <span style={{ color: STATUS_COLOR[r.status === 'confirmed' ? 'done' : r.status === 'returned' ? 'pending' : r.status === 'submitted' ? 'submitted' : 'pending'] }}>
                    {r.status === 'submitted'
                      ? '已提交'
                      : r.status === 'confirmed'
                        ? '已确认'
                        : r.status === 'returned'
                          ? '已退回'
                          : '草稿（未提交）'}
                  </span>
                  <span style={{ color: 'var(--text-tertiary)' }}>
                    {memberLabel(r.reporterId)} · {formatDateTime(r.updatedAt)}
                  </span>
                  <div className="flex-1" />
                  {r.status === 'returned' && !isContainer && canReport && !lockedReport && task.status === 'in_progress' && (
                    <button
                      onClick={() => void useEnclaveTaskStore.getState().reviseReport(orgId, task.id, r.id)}
                      className="px-1.5 py-0.5 rounded"
                      style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                    >
                      继续修改
                    </button>
                  )}
                  {r.status === 'submitted' && !isContainer && canStatus && (
                    <>
                      <button
                        onClick={() => void useEnclaveTaskStore.getState().confirmReport(orgId, task.id, r.id)}
                        className="px-1.5 py-0.5 rounded"
                        style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                      >
                        确认完成
                      </button>
                      <button
                        onClick={() => { setReturningId(r.id); setReturnReason(''); }}
                        className="px-1.5 py-0.5 rounded"
                        style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                      >
                        退回
                      </button>
                    </>
                  )}
                </div>
                {returningId === r.id && (
                  <div className="flex items-center gap-2">
                    <input
                      className="flex-1 px-2.5 py-1.5 rounded-lg text-xs outline-none"
                      style={{ ...FIELD_STYLE, backgroundColor: 'var(--bg-primary)' }}
                      placeholder="退回原因（记录在汇报留言里）"
                      value={returnReason}
                      onChange={(e) => setReturnReason(e.target.value)}
                    />
                    <button
                      onClick={async () => {
                        await useEnclaveTaskStore.getState().returnReport(orgId, task.id, r.id, returnReason);
                        setReturningId(null);
                        setReturnReason('');
                      }}
                      className="px-1.5 py-0.5 rounded text-[11px]"
                      style={{ backgroundColor: 'var(--status-danger)', color: '#fff' }}
                    >
                      确认退回
                    </button>
                    <button
                      onClick={() => setReturningId(null)}
                      className="px-1.5 py-0.5 rounded text-[11px]"
                      style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                    >
                      取消
                    </button>
                  </div>
                )}
                {r.content && (
                  <div className="text-[11px] whitespace-pre-wrap" style={{ color: 'var(--text-secondary)' }}>
                    {r.content}
                  </div>
                )}
                {/* 退回原因（消费 report_feedback 的 return 记录） */}
                {r.status === 'returned' && (() => {
                  const reason = (feedback[r.id] ?? [])
                    .filter((f) => f.action === 'return')
                    .slice(-1)[0]?.content;
                  if (!reason) return null;
                  return (
                    <div
                      className="text-[10px] px-1.5 py-0.5 rounded"
                      style={{ color: 'var(--status-danger)', backgroundColor: 'var(--bg-field)' }}
                    >
                      退回原因：{reason}
                    </div>
                  );
                })()}
                {/* 该次提交的交付物（只读留痕，增删在上方草稿区） */}
                {(() => {
                  const deliverables = files.filter((f) => f.reportId === r.id);
                  if (deliverables.length === 0) return null;
                  return (
                    <div
                      className="flex flex-col gap-1 rounded-md px-2 py-1.5"
                      style={{ border: '1px dashed var(--border)' }}
                    >
                      <div className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                        <Paperclip size={10} />
                        <span>交付物 {deliverables.length}</span>
                      </div>
                      {deliverables.map((f) => (
                        <div key={f.id} className="attachment-row flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                          <span className="truncate flex-1">{f.fileName}</span>
                          <button
                            onClick={() => void openFile(f.id)}
                            className="px-1.5 py-0.5 rounded"
                            style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                          >
                            打开
                          </button>
                        </div>
                      ))}
                    </div>
                  );
                })()}
                {r.status === 'confirmed' && (
                  <div className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    <span>{r.sedimentedAt ? '已沉淀进组织知识库' : (r.sedimentError ? `归档失败：${r.sedimentError}` : '产出待归档')}</span>
                    {!r.sedimentedAt && (
                      <button
                        onClick={async () => {
                          try {
                            await invoke('enclave_task_sediment_retry', {
                              orgId,
                              taskId: task.id,
                              reportId: r.id,
                            });
                            showToast('已重新归档进组织知识库', 'success');
                          } catch (e) {
                            const msg = typeof e === 'string' ? e : e instanceof Error ? e.message : String(e);
                            showToast(`重试归档失败：${msg}`, 'error');
                          } finally {
                            await useEnclaveTaskStore.getState().loadExtras(orgId, task.id);
                          }
                        }}
                        className="px-1.5 py-0.5 rounded"
                        style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                      >
                        重试归档
                      </button>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
        </div>
      </fieldset>
      )}

      {/* AI 拆解草稿：模型产出先过人工，勾选后才创建。
          用 portal 挂到 body，并放在所有 tab 条件之外 —— 否则在「概览」点拆解后弹窗永远不渲染 */}
      {splitOpen &&
        createPortal(
          <div
            className="fixed inset-0 z-[60] flex items-center justify-center"
            style={{ backgroundColor: 'rgba(0,0,0,0.35)' }}
          >
          <div
            className="w-[560px] max-h-[80vh] flex flex-col rounded-xl overflow-hidden"
            style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
          >
            <div
              className="shrink-0 px-4 py-2.5 flex items-center gap-2"
              style={{ borderBottom: '1px solid var(--border)' }}
            >
              <Wand2 size={13} style={{ color: 'var(--accent)' }} />
              <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                AI 拆解草稿
              </span>
              <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                每个子任务应是一份可交付、可指派的成果；勾选后创建，标题与描述可先改
              </span>
              <div className="flex-1" />
              <button
                onClick={() => {
                  setSplitOpen(false);
                  setSplitDrafts([]);
                }}
                className="text-xs"
                style={{ color: 'var(--text-tertiary)' }}
              >
                ✕
              </button>
            </div>
            <div className="flex-1 min-h-0 overflow-y-auto px-4 py-3 flex flex-col gap-2">
              {splitDrafts.map((d, i) => (
                <div
                  key={i}
                  className="flex items-start gap-2 rounded-lg p-2"
                  style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
                >
                  <input
                    type="checkbox"
                    checked={d.checked}
                    onChange={() =>
                      setSplitDrafts((prev) =>
                        prev.map((x, j) => (j === i ? { ...x, checked: !x.checked } : x)),
                      )
                    }
                    className="mt-1"
                  />
                  <div className="flex-1 min-w-0 flex flex-col gap-1">
                    <input
                      value={d.title}
                      onChange={(e) =>
                        setSplitDrafts((prev) =>
                          prev.map((x, j) => (j === i ? { ...x, title: e.target.value } : x)),
                        )
                      }
                      className="w-full px-1.5 py-0.5 rounded text-xs outline-none"
                      style={{
                        backgroundColor: 'var(--bg-content)',
                        color: 'var(--text-primary)',
                        border: '1px solid var(--border)',
                      }}
                    />
                    <input
                      value={d.description}
                      placeholder="描述（可留空）"
                      onChange={(e) =>
                        setSplitDrafts((prev) =>
                          prev.map((x, j) => (j === i ? { ...x, description: e.target.value } : x)),
                        )
                      }
                      className="w-full px-1.5 py-0.5 rounded text-[11px] outline-none"
                      style={{
                        backgroundColor: 'var(--bg-content)',
                        color: 'var(--text-secondary)',
                        border: '1px solid var(--border)',
                      }}
                    />
                    <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      优先级：{PRIORITY_LABEL[d.priority] ?? d.priority}
                    </span>
                  </div>
                </div>
              ))}
              {splitDrafts.length === 0 && (
                <div className="text-xs text-center py-6" style={{ color: 'var(--text-tertiary)' }}>
                  模型没有给出可用的子任务
                </div>
              )}
            </div>
            <div
              className="shrink-0 px-4 py-2.5 flex items-center gap-2"
              style={{ borderTop: '1px solid var(--border)' }}
            >
              <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                草稿由模型生成，请核对后再创建
              </span>
              <div className="flex-1" />
              <button
                onClick={() => void requestSplit()}
                disabled={splitLoading || splitCreating}
                className="text-xs px-2 py-1 rounded flex items-center gap-1"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: 'var(--text-secondary)',
                  border: '1px solid var(--border)',
                  opacity: splitLoading || splitCreating ? 0.5 : 1,
                }}
                title="结果不符合交付物口径时，可让模型重新拆一次"
              >
                {splitLoading ? <Loader2 size={11} className="animate-spin" /> : <Wand2 size={11} />}
                重新拆解
              </button>
              <button
                onClick={() => {
                  setSplitOpen(false);
                  setSplitDrafts([]);
                }}
                className="text-xs px-2 py-1 rounded"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: 'var(--text-secondary)',
                  border: '1px solid var(--border)',
                }}
              >
                取消
              </button>
              <button
                onClick={() => void createSplitTasks()}
                disabled={
                  splitCreating || splitDrafts.filter((d) => d.checked && d.title.trim()).length === 0
                }
                className="text-xs px-2 py-1 rounded"
                style={{
                  backgroundColor: 'var(--accent)',
                  color: '#fff',
                  opacity:
                    splitCreating ||
                    splitDrafts.filter((d) => d.checked && d.title.trim()).length === 0
                      ? 0.5
                      : 1,
                }}
              >
                {splitCreating ? '创建中…' : '创建子任务'}
              </button>
            </div>
          </div>
          </div>,
          document.body,
        )}
    </div>
  );
}
