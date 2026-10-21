/**
 * 飞地团队通知（沟通层 §10.1）。
 *
 * 与 app 顶栏的「通知中心」语义不同：
 * - 这里只承载**团队业务事件**（任务指派、汇报提交/确认/退回、新讨论），数据来自主机
 *   `enclave.db`，按成员持久化，成员端经局域网 WS 拉取；
 * - app 顶栏通知中心是**本机事件流**（工作流/Agent/错误提示），不落团队库、不跨设备。
 */

import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Bell } from 'lucide-react';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { useEnclaveTaskStore } from '../../stores/enclaveTaskStore';
import type { NotificationView } from '../../types/enclave';

function formatTime(unixSec: number): string {
  if (!unixSec) return '-';
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function NotificationBell({
  orgId,
  onOpenTask,
}: {
  orgId: string;
  /** 点击任务类通知时跳转（缺省只切换当前页选中任务） */
  onOpenTask?: (taskId: string) => void;
}) {
  const notifications = useEnclaveOrgStore((s) => s.notifications);
  const [open, setOpen] = useState(false);
  /** 面板用 portal 挂到 body：导航栏外层有 `overflow-hidden`，就地渲染会被裁掉（点开看不到内容） */
  const [pos, setPos] = useState<{ top: number; right: number } | null>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const unread = notifications.filter((n) => !n.readAt).length;

  useEffect(() => {
    if (!orgId) return;
    void useEnclaveOrgStore.getState().loadNotifications(orgId);
    const timer = window.setInterval(
      () => void useEnclaveOrgStore.getState().loadNotifications(orgId),
      60_000,
    );
    return () => window.clearInterval(timer);
  }, [orgId]);

  // 点面板外部 / Esc 关闭
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (panelRef.current?.contains(t) || btnRef.current?.contains(t)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const toggle = () => {
    if (!open) {
      const r = btnRef.current?.getBoundingClientRect();
      if (r) setPos({ top: r.bottom + 6, right: Math.max(8, window.innerWidth - r.right) });
    }
    setOpen((v) => !v);
  };

  if (!orgId) return null;

  const openItem = async (n: NotificationView) => {
    if (!n.readAt) {
      await useEnclaveOrgStore.getState().markNotificationsRead(orgId, [n.id]);
    }
    if (n.targetType === 'task' && n.targetId) {
      if (onOpenTask) {
        onOpenTask(n.targetId);
      } else {
        useEnclaveTaskStore.getState().selectTask(n.targetId);
      }
      setOpen(false);
    }
  };

  return (
    <div className="relative">
      <button
        ref={btnRef}
        onClick={toggle}
        className="text-xs px-2 py-1 rounded-lg flex items-center gap-1"
        style={{
          backgroundColor: open ? 'var(--bg-tertiary)' : 'transparent',
          color: 'var(--text-secondary)',
          border: '1px solid var(--border)',
        }}
        title="团队通知：任务指派 / 汇报提交、确认、退回 / 新讨论"
      >
        <Bell size={12} />
        团队通知
        {unread > 0 && (
          <span
            className="text-[9px] px-1 rounded-full"
            style={{ backgroundColor: 'var(--status-danger)', color: '#fff' }}
          >
            {unread > 99 ? '99+' : unread}
          </span>
        )}
      </button>
      {open &&
        pos &&
        createPortal(
          <div
            ref={panelRef}
            className="fixed z-[9999] w-[320px] max-h-[360px] overflow-auto rounded-lg shadow-lg"
            style={{
              top: pos.top,
              right: pos.right,
              backgroundColor: 'var(--bg-content)',
              border: '1px solid var(--border)',
            }}
          >
            <div
              className="h-8 px-2.5 flex items-center gap-2 text-[11px]"
              style={{ borderBottom: '1px solid var(--border)', color: 'var(--text-secondary)' }}
            >
              <span className="font-medium">团队通知</span>
              <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                仅团队业务事件
              </span>
              <div className="flex-1" />
              {unread > 0 && (
                <button
                  onClick={() => void useEnclaveOrgStore.getState().markNotificationsRead(orgId)}
                  className="text-[10px]"
                  style={{ color: 'var(--accent)' }}
                >
                  全部已读
                </button>
              )}
            </div>
            {notifications.length === 0 && (
              <div className="px-3 py-4 text-[11px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                暂无通知。
                <br />
                提醒只发给「其他成员」（任务指派、汇报提交 / 确认 / 退回、新讨论）；单人组织不给自己发提醒。
              </div>
            )}
            {notifications.map((n) => (
              <button
                key={n.id}
                onClick={() => void openItem(n)}
                className="w-full text-left px-2.5 py-2 flex flex-col gap-0.5"
                style={{
                  borderBottom: '1px solid var(--border)',
                  backgroundColor: n.readAt ? 'transparent' : 'var(--bg-tertiary)',
                }}
              >
                <div className="flex items-center gap-1.5">
                  <span className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>
                    {n.title}
                  </span>
                  <div className="flex-1" />
                  <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                    {formatTime(n.createdAt)}
                  </span>
                </div>
                {n.body && (
                  <span className="text-[10px] truncate" style={{ color: 'var(--text-tertiary)' }}>
                    {n.body}
                  </span>
                )}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}
