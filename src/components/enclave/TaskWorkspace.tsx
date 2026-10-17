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
} from 'lucide-react';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { useEnclaveTaskStore } from '../../stores/enclaveTaskStore';
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
};

const STATUS_COLOR: Record<TaskStatus, string> = {
  pending: 'var(--text-tertiary)',
  in_progress: 'var(--accent)',
  submitted: 'var(--status-warning, #d08700)',
  done: 'var(--status-success, #2f9e44)',
  archived: 'var(--text-tertiary)',
};

const PRIORITY_LABEL: Record<string, string> = {
  low: '低',
  normal: '普通',
  high: '高',
  urgent: '紧急',
};

/** 每个状态可执行的流转（合法性最终由后端 `can_transition` 判定）。 */
const TRANSITIONS: Record<TaskStatus, { to: TaskStatus; label: string }[]> = {
  pending: [{ to: 'in_progress', label: '开始' }],
  in_progress: [
    { to: 'submitted', label: '提交' },
    { to: 'pending', label: '放回待办' },
  ],
  submitted: [
    { to: 'done', label: '确认完成' },
    { to: 'in_progress', label: '退回' },
  ],
  done: [
    { to: 'archived', label: '归档' },
    { to: 'in_progress', label: '重新打开' },
  ],
  archived: [{ to: 'pending', label: '恢复待办' }],
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

export function TaskWorkspace(props: { onOpenOrg?: () => void; onOpenKb?: () => void }) {
  const orgs = useEnclaveOrgStore((s) => s.orgs);
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
  const [creating, setCreating] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);

  // 团队文件根：**全局设置**，与具体组织无关
  const [rootOpen, setRootOpen] = useState(false);
  const [rootView, setRootView] = useState<TeamFilesRootView | null>(null);
  const [rootBusy, setRootBusy] = useState(false);

  useEffect(() => {
    if (!rootOpen || rootView) return;
    void invoke<TeamFilesRootView>('enclave_files_root_get').then(setRootView).catch(() => {});
  }, [rootOpen, rootView]);

  // 首次进入：拉组织（会自动选中并加载成员）
  useEffect(() => {
    void useEnclaveOrgStore.getState().loadOrgs();
  }, []);

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
        <ListTree size={13} style={{ color: 'var(--text-tertiary)' }} />
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
          title="团队文件根：全局设置（与具体团队无关）"
        >
          <FolderOpen size={12} /> 文件根
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

      {/* 团队文件根（全局设置，与具体团队无关） */}
      {rootOpen && (
        <div
          className="shrink-0 px-3 py-2 flex flex-col gap-1"
          style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-side)' }}
        >
          <div className="flex items-center gap-2">
            <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
              团队文件根（全局）
            </span>
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              与具体团队无关；所有团队共用此基础目录
            </span>
            <div className="flex-1" />
            <button
              onClick={async () => {
                const picked = await openDialog({ directory: true, multiple: false });
                if (typeof picked !== 'string') return;
                const ok = await confirmDialog({
                  title: '更改团队文件根目录',
                  message: `团队文件将从当前位置迁移到：\n${picked}\n\n迁移会逐个文件做 sha256 校验，失败会自动回滚。是否继续？`,
                  confirmText: '开始迁移',
                });
                if (!ok) return;
                setRootBusy(true);
                try {
                  const next = await invoke<TeamFilesRootView>('enclave_files_root_set', { path: picked });
                  setRootView(next);
                  showToast('团队文件根已更新', 'success');
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
            {rootView?.base ?? '读取中…'}
          </div>
          {rootView && (
            <div className="text-[10px] font-mono break-all" style={{ color: 'var(--text-tertiary)' }}>
              实际存放：{rootView.root}（其下按组织分子目录）
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

        {/* 中：详情 */}
        <div className="flex-1 min-w-0 overflow-auto">
          {!task && (
            <div className="h-full flex items-center justify-center text-xs" style={{ color: 'var(--text-tertiary)' }}>
              选择左侧任务查看详情
            </div>
          )}
          {task && activeOrgId && (
            <TaskDetail key={task.id} orgId={activeOrgId} task={task} memberLabel={memberLabel} />
          )}
        </div>

        {/* 右：属性 / 依赖 */}
        <div
          className="w-[300px] shrink-0 overflow-auto p-3 flex flex-col gap-3"
          style={{ borderLeft: '1px solid var(--border)' }}
        >
          {task && activeOrgId ? (
            <TaskSidePanel
              key={task.id}
              orgId={activeOrgId}
              task={task}
              tasks={tasks}
              members={members}
            />
          ) : (
            <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>—</div>
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
  memberLabel: (id: string | null) => string;
}) {
  const { orgId, task, memberLabel } = props;
  const canEdit = task.effectiveActions.includes('task.edit');
  const canStatus = task.effectiveActions.includes('task.status');
  const [desc, setDesc] = useState(task.description);
  const [saving, setSaving] = useState(false);

  const reports = useEnclaveTaskStore((s) => s.reports);
  const files = useEnclaveTaskStore((s) => s.files);
  const caps = useEnclaveOrgStore((s) => s.caps);

  // 汇报内容：取自己可编辑的草稿（draft / returned），否则空
  const editable = useMemo(
    () => reports.find((r) => r.status === 'draft' || r.status === 'returned') ?? null,
    [reports],
  );
  const [reportText, setReportText] = useState('');
  const [reportBusy, setReportBusy] = useState(false);
  const [returningId, setReturningId] = useState<string | null>(null);
  const [returnReason, setReturnReason] = useState('');

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

  // 确认/退回权限：管理视角（org owner/admin）或任务创建者
  const isManager =
    caps?.role === 'owner' ||
    caps?.role === 'admin' ||
    (!!caps?.identity && caps.identity.userId === task.creatorId);
  const isAssignee = !!caps?.identity && caps.identity.userId === task.assigneeId;

  const change = async (status: TaskStatus) => {
    await useEnclaveTaskStore.getState().setStatus(orgId, task.id, status);
  };

  const uploadFile = async () => {
    const picked = await openDialog({ multiple: false, directory: false });
    if (typeof picked !== 'string') return;
    await useEnclaveTaskStore.getState().uploadFile(orgId, task.id, picked);
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

      <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
        创建 {memberLabel(task.creatorId)} · 负责人 {memberLabel(task.assigneeId)}
        {task.dueDate ? ` · 截止 ${toDateInput(task.dueDate)}` : ''}
      </div>

      <div className="flex flex-col gap-1">
        <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>描述</span>
        <textarea
          className="search-input w-full"
          style={{ minHeight: 120, backgroundColor: 'var(--bg-field)', resize: 'vertical' }}
          value={desc}
          disabled={!canEdit}
          onChange={(e) => setDesc(e.target.value)}
        />
        {canEdit && desc !== task.description && (
          <button
            onClick={async () => {
              setSaving(true);
              await useEnclaveTaskStore.getState().updateTask(orgId, task.id, { description: desc });
              setSaving(false);
            }}
            disabled={saving}
            className="self-start text-xs px-2 py-1 rounded"
            style={{ backgroundColor: 'var(--accent)', color: '#fff', opacity: saving ? 0.6 : 1 }}
          >
            {saving ? '保存中…' : '保存描述'}
          </button>
        )}
      </div>

      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>状态</span>
        {TRANSITIONS[task.status].map((t) => (
          <button
            key={t.to}
            onClick={() => void change(t.to)}
            disabled={!canStatus}
            className="text-xs px-2 py-1 rounded"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
              border: '1px solid var(--border)',
              opacity: canStatus ? 1 : 0.5,
              cursor: canStatus ? 'pointer' : 'not-allowed',
            }}
            title={canStatus ? '' : '当前角色无权变更状态'}
          >
            {t.label}
          </button>
        ))}
      </div>

      {/* 完成汇报 */}
      <div className="flex flex-col gap-2 pt-2" style={{ borderTop: '1px solid var(--border)' }}>
        <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
          完成汇报
        </span>
        {(isAssignee || editable) && task.status !== 'done' && task.status !== 'archived' && (
          <>
            <textarea
              className="search-input w-full"
              style={{ minHeight: 90, backgroundColor: 'var(--bg-field)', resize: 'vertical' }}
              placeholder="写清做了什么、产出在哪（确认后会沉淀进组织知识库）"
              value={reportText}
              onChange={(e) => setReportText(e.target.value)}
            />
            <div className="flex items-center gap-2">
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
          </>
        )}

        {reports.length > 0 && (
          <div className="flex flex-col gap-1">
            {reports.map((r) => (
              <div
                key={r.id}
                className="rounded px-2 py-1.5 flex flex-col gap-1"
                style={{ backgroundColor: 'var(--bg-tertiary)' }}
              >
                <div className="flex items-center gap-2 text-[11px]">
                  <span style={{ color: STATUS_COLOR[r.status === 'confirmed' ? 'done' : r.status === 'returned' ? 'pending' : r.status === 'submitted' ? 'submitted' : 'pending'] }}>
                    {r.status === 'draft' ? '草稿' : r.status === 'submitted' ? '已提交' : r.status === 'confirmed' ? '已确认' : '已退回'}
                  </span>
                  <span style={{ color: 'var(--text-tertiary)' }}>
                    {memberLabel(r.reporterId)} · {toDateInput(r.updatedAt)}
                  </span>
                  <div className="flex-1" />
                  {r.status === 'submitted' && isManager && (
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
                      style={{ backgroundColor: 'var(--bg-field)' }}
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

      {/* 附件 */}
      <div className="flex flex-col gap-2 pt-2" style={{ borderTop: '1px solid var(--border)' }}>
        <div className="flex items-center gap-2">
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>附件</span>
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{files.length}</span>
          <div className="flex-1" />
          <button
            onClick={() => void uploadFile()}
            className="text-xs px-2 py-1 rounded"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
          >
            上传文件
          </button>
        </div>
        {files.length === 0 ? (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>暂无附件</div>
        ) : (
          files.map((f) => (
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
            </div>
          ))
        )}
      </div>
    </div>
  );
}

function TaskSidePanel(props: {
  orgId: string;
  task: TaskView;
  tasks: TaskView[];
  members: MemberView[];
}) {
  const { orgId, task, tasks, members } = props;
  const canAssign = task.effectiveActions.includes('task.assign');
  const canEdit = task.effectiveActions.includes('task.edit');

  const [assignee, setAssignee] = useState(task.assigneeId ?? '');
  const [priority, setPriority] = useState(task.priority);
  const [due, setDue] = useState(toDateInput(task.dueDate));
  const [deps, setDeps] = useState<string[]>(task.blockedBy);
  const [preview, setPreview] = useState<DepsPreview | null>(null);

  const others = tasks.filter((t) => t.id !== task.id && t.parentTaskId === task.parentTaskId);

  const toggleDep = async (id: string) => {
    const next = deps.includes(id) ? deps.filter((x) => x !== id) : [...deps, id];
    setDeps(next);
    setPreview(await useEnclaveTaskStore.getState().previewDeps(orgId, task.id, next));
  };

  return (
    <>
      <div className="flex flex-col gap-2">
        <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>属性</span>
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
        </label>
        <label className="text-[11px] flex flex-col gap-1" style={{ color: 'var(--text-tertiary)' }}>
          优先级
          <select
            className="search-input"
            style={{ backgroundColor: 'var(--bg-field)' }}
            value={priority}
            disabled={!canEdit}
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
            disabled={!canEdit}
            onChange={(e) => setDue(e.target.value)}
          />
        </label>
        <button
          onClick={async () => {
            await useEnclaveTaskStore.getState().updateTask(orgId, task.id, {
              assigneeId: assignee || null,
              priority,
              dueDate: fromDateInput(due),
            });
          }}
          disabled={!canEdit && !canAssign}
          className="text-xs px-2 py-1 rounded self-start"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-secondary)',
            border: '1px solid var(--border)',
            opacity: canEdit || canAssign ? 1 : 0.5,
          }}
        >
          保存属性
        </button>
      </div>

      <div className="flex flex-col gap-1">
        <span className="text-[11px] font-medium flex items-center gap-1" style={{ color: 'var(--text-secondary)' }}>
          <Link2 size={11} /> 前置依赖（全部完成才解锁）
        </span>
        <div className="max-h-[220px] overflow-auto rounded" style={{ border: '1px solid var(--border)' }}>
          {others.length === 0 && (
            <div className="px-2 py-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              同层没有其它任务
            </div>
          )}
          {others.map((t) => (
            <label
              key={t.id}
              className="flex items-center gap-2 px-2 py-1 text-[11px] cursor-pointer"
              style={{ color: 'var(--text-secondary)' }}
            >
              <input
                type="checkbox"
                checked={deps.includes(t.id)}
                disabled={!canEdit}
                onChange={() => void toggleDep(t.id)}
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
          </div>
        )}
        {preview?.cycleIds.length ? (
          <div className="text-[10px]" style={{ color: 'var(--status-danger)' }}>
            环：{preview.cycleIds.join(' → ')}
          </div>
        ) : null}
        <button
          onClick={async () => {
            const ok = await useEnclaveTaskStore.getState().setDeps(orgId, task.id, deps);
            if (ok) setPreview(null);
          }}
          disabled={!canEdit}
          className="text-xs px-2 py-1 rounded self-start"
          style={{
            backgroundColor: 'var(--accent)',
            color: '#fff',
            opacity: canEdit ? 1 : 0.5,
          }}
        >
          保存依赖
        </button>
        {!canEdit && (
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>仅负责人 / 创建者 / 管理员可改</span>
        )}
      </div>
    </>
  );
}
