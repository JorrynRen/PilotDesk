/**
 * 飞地统一顶部导航（三个实体页面共用）。
 *
 * 任务管理 / 组织与权限 / 团队知识库 是平级路由，用同一套 tab 承载；
 * 组织切换紧随其后，右侧统一放页面扩展位、团队通知与刷新。
 */

import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { ListTree, Building2, Library, RefreshCw, Loader2 } from 'lucide-react';
import { Select } from '../common/Select';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { useEnclaveTaskStore } from '../../stores/enclaveTaskStore';
import { NotificationBell } from './NotificationBell';

export type EnclaveNavKey = 'tasks' | 'org' | 'kb';

const TABS: { key: EnclaveNavKey; label: string; path: string; icon: typeof ListTree }[] = [
  { key: 'tasks', label: '任务管理', path: '/enclave', icon: ListTree },
  { key: 'kb', label: '团队知识库', path: '/enclave/kb', icon: Library },
  { key: 'org', label: '组织与权限', path: '/enclave/org', icon: Building2 },
];

export function EnclaveNavBar({
  active,
  onRefresh,
  refreshing,
  right,
}: {
  active: EnclaveNavKey;
  onRefresh?: () => void | Promise<void>;
  refreshing?: boolean;
  /** 页面级扩展按钮（放在通知/刷新之前） */
  right?: ReactNode;
}) {
  const navigate = useNavigate();
  const orgs = useEnclaveOrgStore((s) => s.orgs);
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);

  return (
    <div
      className="shrink-0 min-h-10 px-3 py-1 flex items-center gap-1.5 flex-wrap"
      style={{ borderBottom: '1px solid var(--border)' }}
    >
      {TABS.map((t) => {
        const Icon = t.icon;
        const on = active === t.key;
        return (
          <button
            key={t.key}
            onClick={() => navigate(t.path)}
            className="text-xs px-2.5 py-1 rounded-lg flex items-center gap-1"
            style={{
              backgroundColor: on ? 'var(--accent)' : 'transparent',
              color: on ? '#fff' : 'var(--text-secondary)',
              border: `1px solid ${on ? 'var(--accent)' : 'var(--border)'}`,
            }}
          >
            <Icon size={12} />
            {t.label}
          </button>
        );
      })}

      {orgs.length > 0 && (
        <>
          <span
            className="mx-1 h-4 w-px shrink-0"
            style={{ backgroundColor: 'var(--border)' }}
            aria-hidden
          />
          {/* 组织多时用下拉承载，避免把功能按钮栏撑成多行 */}
          <Select
            value={activeOrgId}
            onChange={(id) => void useEnclaveOrgStore.getState().selectOrg(id)}
            options={orgs.map((o) => ({ value: o.orgId, label: o.name || o.orgId }))}
            placeholder="选择组织"
            size="sm"
            className="w-[160px] shrink-0"
            title="切换组织"
          />
        </>
      )}

      <div className="flex-1" />
      {right}
      <NotificationBell
        orgId={activeOrgId}
        onOpenTask={(taskId) => {
          useEnclaveTaskStore.getState().selectTask(taskId);
          navigate('/enclave');
        }}
      />
      {onRefresh && (
        <button
          onClick={() => void onRefresh()}
          disabled={refreshing}
          className="text-xs px-2 py-1 rounded-lg flex items-center gap-1"
          style={{
            backgroundColor: 'var(--bg-tertiary)',
            color: 'var(--text-secondary)',
            border: '1px solid var(--border)',
            opacity: refreshing ? 0.6 : 1,
          }}
          title="重新校验本机身份，并刷新组织、权限与本页数据"
        >
          {refreshing ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
          刷新
        </button>
      )}
    </div>
  );
}
