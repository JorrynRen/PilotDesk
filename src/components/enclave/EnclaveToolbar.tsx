/**
 * 飞地页面顶部工具栏动作（任务管理 / 团队知识库 / 组织与权限 共用）。
 *
 * 三个页面共享「出境闸门状态 + 本机文件根」；差异只在末尾的创建按钮：
 * 任务管理 = 新建任务，团队知识库 = 新建知识库，组织与权限 = 无。
 * 本机文件根是**本机全局设置**，面板由各页面在导航下方渲染（`EnclaveFilesRootPanel`）。
 */

import { useEffect, useState } from 'react';
import { FolderOpen, Loader2, Plus } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { showToast } from '../../utils/toast';
import { confirmDialog } from '../../stores/confirmStore';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import type { TeamFilesRootView } from '../../types/enclave';

/** 页面级创建按钮（新建任务 / 新建知识库） */
export interface EnclaveCreateAction {
  label: string;
  /** 表单是否展开（决定按钮的按下态） */
  open: boolean;
  onToggle: () => void;
  disabled?: boolean;
}

export function EnclaveToolbarActions({
  create,
  rootOpen,
  onToggleRoot,
}: {
  create?: EnclaveCreateAction;
  rootOpen: boolean;
  onToggleRoot: () => void;
}) {
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);
  const policy = useEnclaveOrgStore((s) => s.policy);
  const caps = useEnclaveOrgStore((s) => s.caps);
  /** 本机团队文件根是主机本机设置：仅 owner 可见/可改 */
  const canManageRoot = !!caps?.actions.includes('files.root.write');

  return (
    <>
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
      {canManageRoot && (
        <button
          onClick={onToggleRoot}
          className="text-xs px-2 py-1 rounded-lg flex items-center gap-1"
          style={{
            backgroundColor: rootOpen ? 'var(--bg-tertiary)' : 'transparent',
            color: 'var(--text-secondary)',
            border: '1px solid var(--border)',
          }}
          title="本机团队文件根目录：仅影响本机的存放位置（全局设置）"
        >
          <FolderOpen size={12} /> 本机文件根
        </button>
      )}
      {create && (
        <button
          onClick={create.onToggle}
          disabled={create.disabled}
          className="text-xs px-2 py-1 rounded-lg flex items-center gap-1"
          style={{
            // 与相邻按钮同口径：未展开用透明底，展开时才加 bg-tertiary
            backgroundColor: create.open ? 'var(--bg-tertiary)' : 'transparent',
            color: 'var(--text-secondary)',
            border: '1px solid var(--border)',
            opacity: create.disabled ? 0.5 : 1,
            cursor: create.disabled ? 'not-allowed' : 'pointer',
          }}
        >
          <Plus size={12} /> {create.label}
        </button>
      )}
    </>
  );
}

/** 本机团队文件根目录面板：各页面在导航下方渲染同一份 */
export function EnclaveFilesRootPanel({ open }: { open: boolean }) {
  const caps = useEnclaveOrgStore((s) => s.caps);
  const canManageRoot = !!caps?.actions.includes('files.root.write');
  const [rootView, setRootView] = useState<TeamFilesRootView | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open || rootView) return;
    void invoke<TeamFilesRootView>('enclave_files_root_get').then(setRootView).catch(() => {});
  }, [open, rootView]);

  if (!canManageRoot || !open) return null;

  return (
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
            setBusy(true);
            try {
              const next = await invoke<TeamFilesRootView>('enclave_files_root_set', {
                path: picked,
              });
              setRootView(next);
              showToast('本机团队文件根目录已更新', 'success');
            } catch (e) {
              const msg = e instanceof Error ? e.message : String(e);
              showToast(`更改失败：${msg}`, 'error');
            } finally {
              setBusy(false);
            }
          }}
          disabled={busy}
          className="text-xs px-2 py-1 rounded flex items-center gap-1"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-secondary)',
            border: '1px solid var(--border)',
            opacity: busy ? 0.6 : 1,
          }}
        >
          {busy ? <Loader2 size={11} className="animate-spin" /> : <FolderOpen size={11} />}
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
  );
}
