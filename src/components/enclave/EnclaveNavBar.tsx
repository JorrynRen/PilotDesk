/**
 * 飞地统一顶部导航（三个实体页面共用）。
 *
 * 任务管理 / 组织与权限 / 团队知识库 是平级路由，用同一套 tab 承载；
 * 组织切换紧随其后，右侧统一放页面扩展位、团队通知与刷新。
 */

import { useEffect, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { listen } from '@tauri-apps/api/event';
import { ListTree, Building2, Library, BarChart3, RefreshCw, Loader2, WifiOff, CloudOff, LogIn } from 'lucide-react';
import { Select } from '../common/Select';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { useEnclaveTaskStore } from '../../stores/enclaveTaskStore';
import { useAccountStore } from '../../stores/accountStore';
import { ALL_KB_ID, useEnclaveKbStore } from '../../stores/enclaveKbStore';
import { NotificationBell } from './NotificationBell';

export type EnclaveNavKey = 'tasks' | 'kb' | 'report' | 'org';

const TABS: { key: EnclaveNavKey; label: string; path: string; icon: typeof ListTree }[] = [
  { key: 'tasks', label: '任务管理', path: '/enclave', icon: ListTree },
  { key: 'kb', label: '团队知识库', path: '/enclave/kb', icon: Library },
  { key: 'report', label: '团队报告', path: '/enclave/reports', icon: BarChart3 },
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
  /** 平台不可达、按本机身份离线继续（不是未登录） */
  const identityOffline = useEnclaveOrgStore((s) => s.identityOffline);
  const account = useAccountStore((s) => s.account);
  const accountLoaded = useAccountStore((s) => s.loaded);
  /** 登录检查已完成但没有账号：飞地依赖平台身份，此时各页只会是空数据 */
  const notLoggedIn = accountLoaded && !account;
  /** 成员端与主机的实时通道是否断开（owner 不走常驻通道，永远是 false） */
  const [realtimeDown, setRealtimeDown] = useState(false);

  /**
   * 实时变更：主机广播 / 本机写入都会发 `enclave:changed`（只带主题，不带业务数据）。
   *
   * 三个页面共用这块导航，所以监听放在这里：按主题重拉对应数据，同类变更 400ms 合并，
   * 避免一轮操作触发多次并发请求。
   */
  useEffect(() => {
    if (!activeOrgId) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const lastAt = new Map<string, number>();
    setRealtimeDown(false);
    void listen<{
      orgId: string;
      topic?: string;
      type?: string;
      connected?: boolean;
      taskId?: string;
      kbId?: string;
    }>('enclave:changed', (ev) => {
      const { orgId, topic, type, connected, taskId, kbId } = ev.payload ?? {};
      if (orgId !== activeOrgId) return;
      // 连接状态：不走业务刷新，只更新提示
      if (type === 'realtime') {
        setRealtimeDown(!connected);
        return;
      }
      if (!topic) return;
      const now = Date.now();
      if (now - (lastAt.get(topic) ?? 0) < 400) return;
      lastAt.set(topic, now);
      if (topic === 'task') {
        void useEnclaveTaskStore.getState().loadTasks(orgId);
        // 只有事件指向当前任务（或未带 id）时才重拉详情，避免别人的任务把当前详情刷掉
        const activeTask = useEnclaveTaskStore.getState().activeTaskId;
        if (activeTask && (!taskId || taskId === activeTask)) {
          void useEnclaveTaskStore.getState().loadExtras(orgId, activeTask);
        }
      } else if (topic === 'notify') {
        void useEnclaveOrgStore.getState().loadNotifications(orgId);
      } else if (topic === 'org') {
        // 出境闸门 / 应急开关变化：成员端的治理视图跟着刷新
        void useEnclaveOrgStore.getState().loadGovernance(orgId);
      } else if (topic === 'kb') {
        const kb = useEnclaveKbStore.getState();
        // 「全部库」是只读聚合：任何库变化都要刷；具体库只刷自己
        if (!kb.activeKbId) return;
        if (kbId && kb.activeKbId !== ALL_KB_ID && kbId !== kb.activeKbId) return;
        void kb.search(orgId, kb.query);
        if (kb.activeKbId !== ALL_KB_ID) {
          void kb.loadCandidates(orgId, kb.activeKbId);
          void kb.loadQueue(orgId, kb.activeKbId);
          if (kb.files.length > 0) void kb.loadFiles(orgId, kb.activeKbId);
        }
      }
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [activeOrgId]);

  return (
    // 导航条本身**不画底边框**：四个页面都把它放在「带 1px 边框 + 圆角」的卡片里，
    // 这里的 borderBottom 会紧贴卡片自身的下边框，视觉上叠成一条更粗的横线。
    <div className="shrink-0 min-h-10 px-3 py-1 flex items-center gap-1.5 flex-wrap">
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

      {identityOffline && (
        <span
          className="shrink-0 flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px]"
          style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
          title="平台不可达：本机团队数据仍可读写，主机对外服务不受影响；组织 / 成员 / 角色快照停在最后一次同步，治理类操作可能已过期"
        >
          <CloudOff size={10} />
          离线（快照可能过期）
        </span>
      )}

      {notLoggedIn && (
        <button
          onClick={() => navigate('/settings?tab=account')}
          className="shrink-0 flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px]"
          style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
          title="飞地依赖平台账号身份；未登录时无法读取组织、任务与知识库。点击去登录"
        >
          <LogIn size={10} />
          未登录 · 去登录
        </button>
      )}

      {realtimeDown && (
        <span
          className="shrink-0 flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px]"
          style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
          title="与主机的实时通道已断开，正在自动重连；期间数据不会自动刷新，可点右侧「刷新」手动拉取"
        >
          <WifiOff size={10} />
          实时连接中断
        </span>
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
