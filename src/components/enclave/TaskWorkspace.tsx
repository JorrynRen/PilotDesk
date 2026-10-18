/**
 * 飞地任务工作台：左任务树 / 中详情 / 右属性与依赖。
 *
 * 权限口径：所有动作按钮都看后端返回的 `effectiveActions`（授权器唯一来源）；
 * 状态流转的合法性也由后端状态机兜底，前端只负责把可选项呈现出来。
 */

import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import {
  Plus, Loader2, ListTree, Link2, AlertTriangle, ChevronRight, FolderOpen, Library,
  Info, FileText, ListChecks, ClipboardCheck, Paperclip, Flag, History,
} from 'lucide-react';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { useEnclaveTaskStore } from '../../stores/enclaveTaskStore';
import { useAccountStore } from '../../stores/accountStore';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import type {
  DepsPreview, MemberView, TaskStatus, TaskView, TeamFilesRootView,
} from '../../types/enclave';

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

export function TaskWorkspace(props: { onOpenOrg?: () => void; onOpenKb?: () => void }) {
  const orgs = useEnclaveOrgStore((s) => s.orgs);
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);
  const detail = useEnclaveOrgStore((s) => s.detail);
  const policy = useEnclaveOrgStore((s) => s.policy);

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
  const [creating, setCreating] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);

  // 本机团队文件根目录：**全局设置**，与具体组织无关
  const [rootOpen, setRootOpen] = useState(false);
  const [rootView, setRootView] = useState<TeamFilesRootView | null>(null);
  const [rootBusy, setRootBusy] = useState(false);

  useEffect(() => {
    if (!rootOpen || rootView) return;
    void invoke<TeamFilesRootView>('enclave_files_root_get').then(setRootView).catch(() => {});
  }, [rootOpen, rootView]);

  // 登录账号变化 → 先校验新身份，再拉组织（会自动选中并加载成员）
  const accountKey = useAccountStore((s) => s.account?.email ?? '');
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

  // 组织变化 → 重拉任务
  useEffect(() => {
    if (!activeOrgId) return;
    void useEnclaveTaskStore.getState().loadTasks(activeOrgId);
  }, [activeOrgId]);

  const members: MemberView[] = detail?.members ?? [];
  const memberLabel = (id: string | null): string => {
    if (!id) return '未指派';
    const m = members.find((x) => x.userId === id);
    return m?.displayName || m?.email || id;
  };

  const roots = useMemo(() => tasks.filter((t) => !t.parentTaskId), [tasks]);
  const childrenOf = (id: string) => tasks.filter((t) => t.parentTaskId === id);
  const task = tasks.find((t) => t.id === activeTaskId) ?? null;

  const submitCreate = async () => {
    if (!activeOrgId) return;
    if (!newTitle.trim()) {
      setLocalError('请填写任务标题');
      return;
    }
    setCreating(true);
    setLocalError(null);
    const created = await useEnclaveTaskStore.getState().createTask(activeOrgId, {
      title: newTitle.trim(),
      parentTaskId: newParentId || null,
      assigneeId: newAssignee || null,
      priority: newPriority,
      dueDate: fromDateInput(newDue),
    });
    setCreating(false);
    if (created) {
      setNewOpen(false);
      setNewTitle('');
      setNewParentId('');
      setNewAssignee('');
      setNewDue('');
      setNewPriority('normal');
    }
  };

  return (
    <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
      {/* 组织选择 + 新建 */}
      <div
        className="shrink-0 h-9 px-3 flex items-center gap-2"
        style={{ borderBottom: '1px solid var(--border)' }}
      >
        <ListTree size={13} style={{ color: 'var(--accent)' }} />
        {orgs.map((o) => (
          <button
            key={o.orgId}
            onClick={() => void useEnclaveOrgStore.getState().selectOrg(o.orgId)}
            className="text-xs px-2 py-0.5 rounded"
            style={{
              backgroundColor: o.orgId === activeOrgId ? 'var(--accent)' : 'var(--bg-tertiary)',
              color: o.orgId === activeOrgId ? '#fff' : 'var(--text-secondary)',
              border: '1px solid var(--border)',
            }}
          >
            {o.name || o.orgId}
          </button>
        ))}
        <div className="flex-1" />
        {/* 出境闸门常驻状态：默认关闭，Agent 能否检索团队知识库受此约束 */}
        {activeOrgId && (
          <span
            className="text-[11px] px-1.5 py-0.5 rounded"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: policy?.allowCloudModel ? 'var(--status-danger)' : 'var(--text-tertiary)',
            }}
            title="团队数据发送至云端模型的开关（在「组织与权限」页修改）"
          >
            出境闸门：{policy?.allowCloudModel ? '允许' : '禁止'}
          </span>
        )}
        {loading && <Loader2 size={12} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />}
        {props.onOpenKb && (
          <button
            onClick={props.onOpenKb}
            className="text-xs px-2 py-1 rounded flex items-center gap-1"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
              border: '1px solid var(--border)',
            }}
          >
            <Library size={12} /> 团队知识库
          </button>
        )}
        <button
          onClick={() => setRootOpen((v) => !v)}
          className="text-xs px-2 py-1 rounded flex items-center gap-1"
          style={{
            backgroundColor: rootOpen ? 'var(--bg-tertiary)' : 'transparent',
            color: 'var(--text-secondary)',
            border: '1px solid var(--border)',
          }}
          title="本机团队文件根目录：仅影响本机的存放位置（全局设置）"
        >
          <FolderOpen size={12} /> 本机文件根
        </button>
        {props.onOpenOrg && (
          <button
            onClick={props.onOpenOrg}
            className="text-xs px-2 py-1 rounded"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
              border: '1px solid var(--border)',
            }}
          >
            组织与权限
          </button>
        )}
        <button
          onClick={() => setNewOpen((v) => !v)}
          disabled={!activeOrgId}
          className="text-xs px-2 py-1 rounded flex items-center gap-1"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-secondary)',
            border: '1px solid var(--border)',
            opacity: activeOrgId ? 1 : 0.5,
          }}
        >
          <Plus size={12} /> 新建任务
        </button>
      </div>

      {/* 本机团队文件根目录（全局设置，仅影响本机存放位置） */}
      {rootOpen && (
        <div
          className="shrink-0 px-3 py-2 flex flex-col gap-1"
          style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-side)' }}
        >
          <div className="flex items-center gap-2">
            <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
              本机团队文件根目录（全局）
            </span>
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              仅影响本机；所有团队的本地文件共用此基础目录
            </span>
            <div className="flex-1" />
            <button
              onClick={async () => {
                const picked = await openDialog({ directory: true, multiple: false });
                if (typeof picked !== 'string') return;
                const ok = await confirmDialog({
                  title: '更改本机团队文件根目录',
                  message: `本机团队文件将从当前位置迁移到：\n${picked}\n\n迁移会逐个文件做 sha256 校验，失败会自动回滚。是否继续？`,
                  confirmText: '开始迁移',
                });
                if (!ok) return;
                setRootBusy(true);
                try {
                  const next = await invoke<TeamFilesRootView>('enclave_files_root_set', { path: picked });
                  setRootView(next);
                  showToast('本机团队文件根目录已更新', 'success');
                } catch (e) {
                  const msg = typeof e === 'string' ? e : e instanceof Error ? e.message : String(e);
                  showToast(`更改失败：${msg}`, 'error');
                } finally {
                  setRootBusy(false);
                }
              }}
              disabled={rootBusy}
              className="text-xs px-2 py-1 rounded flex items-center gap-1"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-secondary)',
                border: '1px solid var(--border)',
                opacity: rootBusy ? 0.6 : 1,
              }}
            >
              {rootBusy ? <Loader2 size={11} className="animate-spin" /> : <FolderOpen size={11} />}
              选择目录
            </button>
          </div>
          <div className="text-[11px] font-mono break-all" style={{ color: 'var(--text-tertiary)' }}>
            本机基础目录：{rootView?.base ?? '读取中…'}
          </div>
          {rootView && (
            <div className="text-[10px] font-mono break-all" style={{ color: 'var(--text-tertiary)' }}>
              本机实际存放：{rootView.root}（其下按组织分子目录）
            </div>
          )}
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            会在此目录下自动创建 enclave/files；与个人知识库完全独立。仅支持本地固定盘，盘不可用时会拒绝写入而不是回退。
          </div>
        </div>
      )}

      {/* 新建表单 */}
      {newOpen && (
        <div
          className="shrink-0 px-3 py-2 flex items-end gap-2 flex-wrap"
          style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-side)' }}
        >
          <input
            autoFocus
            className="search-input"
            style={{ width: 240, backgroundColor: 'var(--bg-field)' }}
            placeholder="任务标题"
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') void submitCreate(); }}
          />
          <select
            className="search-input"
            style={{ width: 150, backgroundColor: 'var(--bg-field)' }}
            value={newParentId}
            onChange={(e) => setNewParentId(e.target.value)}
          >
            <option value="">（顶层任务）</option>
            {roots.map((t) => (
              <option key={t.id} value={t.id}>{t.title}</option>
            ))}
          </select>
          <select
            className="search-input"
            style={{ width: 150, backgroundColor: 'var(--bg-field)' }}
            value={newAssignee}
            onChange={(e) => setNewAssignee(e.target.value)}
          >
            <option value="">未指派</option>
            {members.filter((m) => m.seatStatus !== 'removed').map((m) => (
              <option key={m.userId} value={m.userId}>{m.displayName || m.email}</option>
            ))}
          </select>
          <select
            className="search-input"
            style={{ width: 90, backgroundColor: 'var(--bg-field)' }}
            value={newPriority}
            onChange={(e) => setNewPriority(e.target.value)}
          >
            {Object.entries(PRIORITY_LABEL).map(([k, v]) => (
              <option key={k} value={k}>{v}</option>
            ))}
          </select>
          <input
            type="date"
            className="search-input"
            style={{ width: 140, backgroundColor: 'var(--bg-field)' }}
            value={newDue}
            onChange={(e) => setNewDue(e.target.value)}
          />
          <button
            onClick={() => void submitCreate()}
            disabled={creating}
            className="text-xs px-3 py-1.5 rounded"
            style={{ backgroundColor: 'var(--accent)', color: '#fff', opacity: creating ? 0.6 : 1 }}
          >
            {creating ? '创建中…' : '创建'}
          </button>
          {(localError || error) && (
            <span className="text-[11px]" style={{ color: 'var(--status-danger)' }}>
              {localError || error}
            </span>
          )}
        </div>
      )}

      <div className="flex-1 min-h-0 flex overflow-hidden">
        {/* 左：任务树 */}
        <div
          className="w-[280px] shrink-0 overflow-auto py-1"
          style={{ borderRight: '1px solid var(--border)' }}
        >
          {tasks.length === 0 && (
            <div className="px-3 py-6 text-xs text-center" style={{ color: 'var(--text-tertiary)' }}>
              还没有任务，点右上角「新建任务」
            </div>
          )}
          {roots.map((root) => (
            <div key={root.id}>
              <TaskNode
                task={root}
                active={root.id === activeTaskId}
                onClick={() => useEnclaveTaskStore.getState().selectTask(root.id)}
              />
              {childrenOf(root.id).map((child) => (
                <TaskNode
                  key={child.id}
                  task={child}
                  child
                  active={child.id === activeTaskId}
                  onClick={() => useEnclaveTaskStore.getState().selectTask(child.id)}
                />
              ))}
            </div>
          ))}
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
  );
}

function TaskNode(props: {
  task: TaskView;
  active: boolean;
  child?: boolean;
  onClick: () => void;
}) {
  const { task, active, child, onClick } = props;
  return (
    <button
      onClick={onClick}
      className="w-full text-left px-3 py-1.5 flex items-center gap-1.5"
      style={{
        backgroundColor: active ? 'var(--bg-tertiary)' : 'transparent',
        color: active ? 'var(--text-primary)' : 'var(--text-secondary)',
        paddingLeft: child ? 26 : 12,
      }}
      title={task.title}
    >
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
      {task.blocked && <AlertTriangle size={11} style={{ color: 'var(--status-danger)' }} />}
    </button>
  );
}

function TaskDetail(props: {
  orgId: string;
  task: TaskView;
  tasks: TaskView[];
  members: MemberView[];
  memberLabel: (id: string | null) => string;
}) {
  const { orgId, task, tasks, members, memberLabel } = props;
  // 权限按「当前登录身份 + 任务字段」本地复核（同时兼容快照未及时刷新）：
  // 非参与者不出现任何提交表单；创建者 / 负责人才能编辑与流转状态。
  const caps = useEnclaveOrgStore((s) => s.caps);
  const identityVerified = useEnclaveTaskStore((s) => s.identityVerified);
  const me = caps?.identity?.userId ?? '';
  const isCreator = !!me && me === task.creatorId;
  const isAssignee = !!me && me === task.assigneeId;
  const isCollaborator = !!me && task.collaborators.includes(me);
  const isParticipant = isCreator || isAssignee || isCollaborator;
  // 身份未校验（刷新失败 / 账号刚切换）时一律不出现写入口，避免旧身份越权
  const canEdit = identityVerified && (isCreator || isAssignee);
  const canStatus = identityVerified && (isCreator || isAssignee);
  const canAssign = identityVerified && isCreator;
  const canReport = identityVerified && isParticipant;
  const canUpload = identityVerified && isParticipant;
  const blocked = task.blocked;
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
  const [preview, setPreview] = useState<DepsPreview | null>(null);

  const reports = useEnclaveTaskStore((s) => s.reports);
  const files = useEnclaveTaskStore((s) => s.files);
  const progress = useEnclaveTaskStore((s) => s.progress);
  /** 任务资料：不挂汇报、不挂过程记录的独立附件 */
  const taskFiles = files.filter((f) => !f.reportId && !f.progressId);
  const [progressText, setProgressText] = useState('');
  const [composing, setComposing] = useState(false);
  const [pendingFiles, setPendingFiles] = useState<string[]>([]);

  const feedback = useEnclaveTaskStore((s) => s.feedback);
  // 当前草稿（唯一可编辑）；已退回/已提交/已确认都只作为提交日志展示
  const editable = useMemo(() => reports.find((r) => r.status === 'draft') ?? null, [reports]);
  const logs = useMemo(() => reports.filter((r) => r.status !== 'draft'), [reports]);
  const [reportText, setReportText] = useState('');
  // 等待审核或已确认时不允许再编辑 / 提交
  const lockedReport = reports.find((r) => r.status === 'submitted' || r.status === 'confirmed') ?? null;
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
      const ok = await confirmDialog({
        title: '作废任务',
        message:
          '作废后任务终止，不能再提交过程记录或完成汇报；已沉淀进知识库的历史产出不受影响。\n\n是否继续？',
        confirmText: '确认作废',
      });
      if (!ok) return;
    }
    await useEnclaveTaskStore.getState().setStatus(orgId, task.id, status);
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

  return (
    <div className="p-4 flex flex-col gap-3">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
          {task.title}
        </span>
        <span className="text-[11px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: STATUS_COLOR[task.status] }}>
          {STATUS_LABEL[task.status]}
        </span>
        <span className="text-[11px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
          {PRIORITY_LABEL[task.priority] ?? task.priority}
        </span>
        {task.blocked && (
          <span className="text-[11px] px-1.5 py-0.5 rounded flex items-center gap-1" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--status-danger)' }}>
            <AlertTriangle size={10} /> 被前置任务阻塞
          </span>
        )}
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

      {/* 任务信息（属性 + 依赖都直接展示；修改时才出现表单） */}
      <div className="flex flex-col gap-2">
        <div className="flex items-center gap-1.5">
          <Info size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>任务信息</span>
          <div className="flex-1" />
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
          // 平铺单行，用「·」分隔，避免占多行
          <div className="text-[11px] leading-5" style={{ color: 'var(--text-secondary)' }}>
            {[
              `创建者：${memberLabel(task.creatorId)}`,
              `负责人：${memberLabel(task.assigneeId)}`,
              `优先级：${PRIORITY_LABEL[task.priority] ?? task.priority}`,
              `截止：${task.dueDate ? toDateInput(task.dueDate) : '未设置'}`,
              `参与者：${task.collaborators.length ? task.collaborators.map((id) => memberLabel(id)).join('、') : '无'}`,
              `前置依赖：${
                task.blockedBy.length
                  ? task.blockedBy
                    .map((id) => {
                      const dep = tasks.find((t) => t.id === id);
                      return dep ? `${dep.title}（${STATUS_LABEL[dep.status]}）` : id;
                    })
                    .join('、')
                  : '无'
              }`,
            ].join(' · ')}
          </div>
        ) : (
          <>
            <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
              负责人
              <select
                className="search-input"
                style={{ backgroundColor: 'var(--bg-field)' }}
                value={assignee}
                disabled={!canAssign}
                onChange={(e) => setAssignee(e.target.value)}
              >
                <option value="">未指派</option>
                {members.filter((m) => m.seatStatus !== 'removed').map((m) => (
                  <option key={m.userId} value={m.userId}>{m.displayName || m.email}</option>
                ))}
              </select>
              {!canAssign && <span className="text-[10px]">仅创建者可修改负责人</span>}
            </label>
            <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
              优先级
              <select
                className="search-input"
                style={{ backgroundColor: 'var(--bg-field)' }}
                value={priority}
                onChange={(e) => setPriority(e.target.value)}
              >
                {Object.entries(PRIORITY_LABEL).map(([k, v]) => (
                  <option key={k} value={k}>{v}</option>
                ))}
              </select>
            </label>
            <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
              截止日期
              <input
                type="date"
                className="search-input"
                style={{ backgroundColor: 'var(--bg-field)' }}
                value={due}
                onChange={(e) => setDue(e.target.value)}
              />
            </label>

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
                      <span style={{ color: STATUS_COLOR[t.status] }}>{STATUS_LABEL[t.status]}</span>
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
              className="search-input w-full"
              style={{ minHeight: 120, backgroundColor: 'var(--bg-field)', resize: 'vertical' }}
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

      {/* 任务资料（描述之后：先看说明，再看支撑文件） */}
      <div
        className="flex flex-col gap-2 rounded-lg p-3"
        style={{ backgroundColor: 'var(--bg-side)', border: '1px solid var(--border)' }}
      >
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
        {taskFiles.length === 0 ? (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            暂无任务资料（汇报交付物在完成汇报里，过程附件在过程记录里）
          </div>
        ) : (
          taskFiles.map((f) => (
            <div key={f.id} className="flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
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

      {/* 状态操作：无权限或无可用流转时整块不显示（待确认只由完成汇报驱动） */}
      {canStatus && TRANSITIONS[task.status].length > 0 && (
        <div className="flex items-center gap-2 flex-wrap">
          <Flag size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>状态操作</span>
          {TRANSITIONS[task.status].map((t) => (
            <button
              key={t.to}
              onClick={() => void change(t.to)}
              className="text-xs px-2 py-1 rounded"
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

      {/* 过程记录（可多次；提交完成汇报后锁定） */}
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

        {/* 默认只展示列表；点「添加过程记录」才展开表单（前置未完成时不允许记录） */}
        {canReport && !blocked && !progress?.locked && !composing && (
          <button
            onClick={() => setComposing(true)}
            className="self-start text-xs px-2 py-1 rounded flex items-center gap-1"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
          >
            <Plus size={12} /> 添加过程记录
          </button>
        )}

        {canReport && !blocked && !progress?.locked && composing && (
          <div
            className="flex flex-col gap-2 rounded p-2"
            style={{ border: '1px solid var(--border)', backgroundColor: 'var(--bg-field)' }}
          >
            <textarea
              className="search-input w-full"
              style={{ minHeight: 70, backgroundColor: 'var(--bg-content)', resize: 'vertical' }}
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
            {p.attachments.map((f) => (
              <div key={f.id} className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-secondary)' }}>
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
        ))}

        {(!progress || progress.items.length === 0) && (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            还没有过程记录
          </div>
        )}
      </div>

      {/* 完成汇报 */}
      <div
        className="flex flex-col gap-2 rounded-lg p-3"
        style={{ backgroundColor: 'var(--bg-side)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2">
          <ClipboardCheck size={12} style={{ color: 'var(--accent)' }} />
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
            完成汇报
          </span>
        </div>
        {lockedReport && (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            {lockedReport.status === 'submitted'
              ? '完成汇报已提交，等待创建者 / 负责人确认（不可重复提交）'
              : '完成汇报已确认，任务已收敛'}
          </div>
        )}
        {blocked && (
          <div className="text-[11px]" style={{ color: 'var(--status-danger)' }}>
            存在未完成的前置任务，暂不能开始 / 提交
          </div>
        )}
        {canReport && !lockedReport && !blocked && task.status !== 'done' && task.status !== 'archived' && task.status !== 'cancelled' && (editable || logs.length === 0) && (
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
              className="search-input w-full"
              style={{ minHeight: 90, backgroundColor: 'var(--bg-field)', resize: 'vertical' }}
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
                <div className="flex flex-col gap-1">
                  <div className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    <Paperclip size={10} />
                    <span>本条交付物 {deliverables.length}</span>
                  </div>
                  {deliverables.map((f) => (
                    <div key={f.id} className="flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
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
        {logs.length > 0 && (
          <div className="flex flex-col gap-2" style={{ borderTop: '1px solid var(--border)', paddingTop: 8 }}>
            <div className="flex items-center gap-2">
              <History size={11} style={{ color: 'var(--text-tertiary)' }} />
              <span className="text-[10px] font-medium" style={{ color: 'var(--text-tertiary)' }}>
                汇报记录（{logs.length}）
              </span>
            </div>
            {logs.map((r) => (
              <div
                key={r.id}
                className="rounded px-2 py-1.5 flex flex-col gap-1"
                style={{ backgroundColor: 'var(--bg-tertiary)' }}
              >
                <div className="flex items-center gap-2 text-[11px]">
                  <span style={{ color: STATUS_COLOR[r.status === 'confirmed' ? 'done' : r.status === 'returned' ? 'pending' : r.status === 'submitted' ? 'submitted' : 'pending'] }}>
                    {r.status === 'submitted' ? '已提交' : r.status === 'confirmed' ? '已确认' : '已退回'}
                  </span>
                  <span style={{ color: 'var(--text-tertiary)' }}>
                    {memberLabel(r.reporterId)} · {formatDateTime(r.updatedAt)}
                  </span>
                  <div className="flex-1" />
                  {r.status === 'returned' && canReport && !lockedReport && task.status !== 'done' && task.status !== 'archived' && task.status !== 'cancelled' && (
                    <button
                      onClick={() => void useEnclaveTaskStore.getState().reviseReport(orgId, task.id, r.id)}
                      className="px-1.5 py-0.5 rounded"
                      style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
                    >
                      继续修改
                    </button>
                  )}
                  {r.status === 'submitted' && canStatus && (
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
                      className="search-input flex-1"
                      style={{ backgroundColor: 'var(--bg-primary)' }}
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
                    <div className="flex flex-col gap-1">
                      <div className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                        <Paperclip size={10} />
                        <span>交付物 {deliverables.length}</span>
                      </div>
                      {deliverables.map((f) => (
                        <div key={f.id} className="flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
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
                            await useEnclaveTaskStore.getState().loadExtras(orgId, task.id);
                          } catch {
                            // 错误会写入 sediment_error，列表刷新后可见
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
    </div>
  );
}
