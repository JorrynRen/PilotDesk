/**
 * NotificationCenter — 全局通知中心抽屉（顶栏铃铛触发）
 *
 * 解决"Toast 错过就没了"与"待办只存在于当时那个页面"：把全部通知（含历史）收在一处，
 * 未决项（工具审批、工作流待输入）置顶并可直接跳去处理。
 * 弹层挂载在 App 级，因此会话/群聊/工作流/终端/设置页都能打开。
 */
import { useMemo, useState, type ReactNode } from 'react';
import { Bell, X, CheckCheck, Trash2, AlertCircle, AlertTriangle, CheckCircle2, Info } from 'lucide-react';
import { useTerminal } from '../../TerminalManager';
import { useSessionStore } from '../../stores/sessionStore';
import {
  useNotificationStore,
  countUnread,
  sortForDisplay,
  type NotificationItem,
  type NotificationLevel,
} from '../../stores/notificationStore';

const LEVEL_STYLE: Record<NotificationLevel, { color: string; bg: string; icon: ReactNode }> = {
  error: { color: '#EF4444', bg: 'rgba(239,68,68,0.08)', icon: <AlertCircle size={12} /> },
  warning: { color: '#F59E0B', bg: 'rgba(245,158,11,0.08)', icon: <AlertTriangle size={12} /> },
  success: { color: '#10B981', bg: 'rgba(16,185,129,0.08)', icon: <CheckCircle2 size={12} /> },
  info: { color: 'var(--accent)', bg: 'var(--accent-light)', icon: <Info size={12} /> },
};

type FilterKey = 'all' | 'unread' | 'pending';

function formatTs(ts: number): string {
  const diff = Date.now() - ts;
  if (diff < 60_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, '0');
  const hhmm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return new Date().toDateString() === d.toDateString()
    ? hhmm
    : `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hhmm}`;
}

export function NotificationCenter() {
  const items = useNotificationStore((s) => s.items);
  const open = useNotificationStore((s) => s.open);
  const setOpen = useNotificationStore((s) => s.setOpen);
  const markRead = useNotificationStore((s) => s.markRead);
  const markAllRead = useNotificationStore((s) => s.markAllRead);
  const clearAll = useNotificationStore((s) => s.clearAll);
  const { setMode } = useTerminal();
  const [filter, setFilter] = useState<FilterKey>('all');

  const shown = useMemo(() => {
    const list = sortForDisplay(items);
    if (filter === 'unread') return list.filter((i) => !i.read);
    if (filter === 'pending') return list.filter((i) => i.pending);
    return list;
  }, [items, filter]);

  if (!open) return null;

  const unread = countUnread(items);
  const pendingCount = items.filter((i) => i.pending).length;

  /** 回放跳转：有归属的通知直接送到对应的会话/工作流 */
  const jump = (item: NotificationItem) => {
    const t = item.target;
    if (!t) return;
    if (t.kind === 'session') {
      useSessionStore.getState().selectSession(t.id);
      setMode('session');
    } else {
      // 工作流待审批/待输入都在工作流页处理（实例列表 + 待处理横幅）
      setMode('workflow');
    }
    markRead(item.id);
    setOpen(false);
  };

  const filterBtn = (key: FilterKey, label: string, count?: number) => (
    <button
      key={key}
      onClick={() => setFilter(key)}
      className="text-[10px] px-2 py-0.5 rounded-full transition-colors"
      style={{
        border: `1px solid ${filter === key ? 'var(--accent)' : 'transparent'}`,
        backgroundColor: filter === key ? 'var(--accent-light)' : 'transparent',
        color: filter === key ? 'var(--accent)' : 'var(--text-tertiary)',
      }}
    >
      {label}{count !== undefined ? ` ${count}` : ''}
    </button>
  );

  return (
    <>
      {/* 点击外部关闭：透明遮罩，不拦截标题栏以外的视觉 */}
      <div className="fixed inset-0 z-[69]" onClick={() => setOpen(false)} />
      <div
        className="fixed right-3 top-[52px] z-[70] w-[360px] flex flex-col rounded-lg overflow-hidden"
        style={{
          maxHeight: '72vh',
          backgroundColor: 'var(--bg-primary)',
          border: '1px solid var(--border)',
          boxShadow: '0 8px 28px rgba(0,0,0,0.28)',
        }}
      >
        {/* 头部：标题 + 未读 + 操作 */}
        <div className="flex items-center gap-2 px-3 h-9 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <Bell size={12} style={{ color: 'var(--text-secondary)' }} />
          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>通知</span>
          {unread > 0 && (
            <span className="text-[10px] px-1.5 rounded-full" style={{ backgroundColor: 'rgba(239,68,68,0.12)', color: '#EF4444' }}>
              {unread} 未读
            </span>
          )}
          <div className="flex-1" />
          <button
            onClick={markAllRead}
            disabled={unread === 0}
            className="pd-btn p-1 rounded transition-colors"
            style={{ color: 'var(--text-secondary)', opacity: unread === 0 ? 0.4 : 1 }}
            title="全部标为已读（待处理项除外）"
          >
            <CheckCheck size={12} />
          </button>
          <button
            onClick={clearAll}
            disabled={items.length === 0}
            className="pd-btn p-1 rounded transition-colors"
            style={{ color: 'var(--text-secondary)', opacity: items.length === 0 ? 0.4 : 1 }}
            title="清空已通知（保留待处理项）"
          >
            <Trash2 size={12} />
          </button>
          <button
            onClick={() => setOpen(false)}
            className="pd-btn p-1 rounded transition-colors"
            style={{ color: 'var(--text-secondary)' }}
            title="关闭"
          >
            <X size={12} />
          </button>
        </div>

        {/* 筛选 */}
        <div className="flex items-center gap-1 px-3 py-1.5 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          {filterBtn('all', '全部')}
          {filterBtn('unread', '未读', unread)}
          {filterBtn('pending', '待处理', pendingCount)}
        </div>

        {/* 列表 */}
        <div className="flex-1 min-h-0 overflow-y-auto pd-scroll-stable">
          {shown.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-10 gap-1">
              <Bell size={18} style={{ color: 'var(--text-tertiary)', opacity: 0.5 }} />
              <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                {filter === 'all' ? '暂无通知' : filter === 'unread' ? '没有未读通知' : '没有待处理事项'}
              </span>
            </div>
          ) : (
            shown.map((item) => {
              const st = LEVEL_STYLE[item.level];
              return (
                <div
                  key={item.id}
                  className="px-3 py-2"
                  // 点击整条即标记已读（单个处理），不跳转、不关闭抽屉；
                  // 跳转仍走下方「查看会话 ›」按钮——两件事分开，否则点一下就被带走、
                  // 且已读只能靠顶部「全部已读」。
                  title={item.read ? undefined : '点击标记为已读'}
                  onClick={() => { if (!item.read) markRead(item.id); }}
                  style={{
                    borderBottom: '1px solid var(--border)',
                    backgroundColor: item.pending ? 'rgba(245,158,11,0.06)' : 'transparent',
                    cursor: item.read ? 'default' : 'pointer',
                  }}
                >
                  <div className="flex items-start gap-2">
                    <span style={{ color: st.color, flexShrink: 0, marginTop: 1 }}>{st.icon}</span>
                    <div className="flex-1 min-w-0">
                      {/* 标题与明细都完整展示、按行折行：通知的价值在于"看全内容"，
                          省略号/截断会让用户还得去别处找原文。整条列表可滚动。 */}
                      <div className="flex items-start gap-1.5">
                        {item.pending && (
                          <span className="text-[9px] px-1 rounded shrink-0" style={{ backgroundColor: 'rgba(245,158,11,0.16)', color: '#F59E0B', marginTop: 2 }}>
                            待处理
                          </span>
                        )}
                        <span
                          className={`text-[11px] flex-1 min-w-0 ${item.read ? 'font-normal' : 'font-semibold'}`}
                          style={{
                            // 未读/已读：粗体 vs 常规为主信号（11px 小字上光靠颜色差别感觉不到），
                            // 颜色作辅助（未读=主色，已读=次级色）。也不再画等级色圆点——
                            // 那会和左侧等级图标同色，看起来是同一信息画两遍。
                            color: item.read ? 'var(--text-secondary)' : 'var(--text-primary)',
                            whiteSpace: 'pre-wrap',
                            wordBreak: 'break-word',
                          }}
                        >
                          {item.title}
                        </span>
                      </div>
                      {item.detail && (
                        <div
                          className="text-[10px] mt-0.5"
                          style={{
                            color: 'var(--text-tertiary)',
                            whiteSpace: 'pre-wrap',
                            wordBreak: 'break-word',
                          }}
                        >
                          {item.detail}
                        </div>
                      )}
                      <div className="flex items-center gap-2 mt-1">
                        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{formatTs(item.ts)}</span>
                        {item.target && (
                          <button
                            onClick={(e) => { e.stopPropagation(); jump(item); }}
                            className="text-[10px] transition-colors hover:opacity-80"
                            style={{ color: 'var(--accent)' }}
                          >
                            {item.target.kind === 'session' ? '查看会话 ›' : '去工作流处理 ›'}
                          </button>
                        )}
                      </div>
                    </div>
                  </div>
                </div>
              );
            })
          )}
        </div>
      </div>
    </>
  );
}
