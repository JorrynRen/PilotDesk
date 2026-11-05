import { useEffect, useRef, useState, type ReactNode } from 'react';
import { MoreHorizontal } from 'lucide-react';

/**
 * 页面顶部工具栏的统一容器：优先级 + 宽度感知折叠 + 溢出菜单。
 *
 * 分级约定：
 * - 0 主操作：始终「图标 + 文字」，任何宽度都不收起；
 * - 1 常用：宽度 < 700px 时收成图标（保留 tooltip）；
 * - 2 低频：宽度 < 900px 收成图标，< 700px 进溢出菜单。
 *
 * 折叠依据宿主行宽度（`parentElement`），因此调用方需把本组件直接放进工具栏行里。
 * 图标的 tooltip 由全局 TooltipLayer 统一接管（读 `title`）。
 */
export interface ToolbarAction {
  key: string;
  label: string;
  icon?: ReactNode;
  /** 0 主操作 / 1 常用 / 2 低频，默认 1。 */
  priority?: 0 | 1 | 2;
  /** 强制只显示图标（例如行尾关闭按钮），不受优先级影响。 */
  iconOnly?: boolean;
  /** 悬停提示；缺省用 label。 */
  title?: string;
  /** 溢出菜单中展示的快捷键提示（如 ⌘K）。 */
  shortcut?: string;
  active?: boolean;
  disabled?: boolean;
  /** 文字/图标颜色覆盖（如危险操作用 --status-danger）。 */
  tone?: string;
  onClick: () => void;
}

interface Props {
  actions: ToolbarAction[];
  className?: string;
}

const LABEL_BREAKPOINT = 900;
const ICON_BREAKPOINT = 700;

export function PageToolbar({ actions, className }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [hostWidth, setHostWidth] = useState(9999);
  const [overflowOpen, setOverflowOpen] = useState(false);

  // 观察宿主工具栏行的宽度（本组件自身宽度由内容决定，不能作为折叠依据）。
  useEffect(() => {
    const host = rootRef.current?.parentElement;
    if (!host) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? 0;
      if (w > 0) setHostWidth(w);
    });
    ro.observe(host);
    setHostWidth(host.getBoundingClientRect().width);
    return () => ro.disconnect();
  }, []);

  // 溢出菜单：点击外部 / Esc 关闭
  useEffect(() => {
    if (!overflowOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOverflowOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOverflowOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [overflowOpen]);

  const isCompact = (action: ToolbarAction) => {
    if (action.iconOnly) return true;
    if (action.priority === 0) return false;
    if (action.priority === 2) return hostWidth < LABEL_BREAKPOINT;
    return hostWidth < ICON_BREAKPOINT;
  };

  const inOverflow = (action: ToolbarAction) =>
    !action.iconOnly && action.priority === 2 && hostWidth < ICON_BREAKPOINT;

  const visible = actions.filter((a) => !inOverflow(a));
  const overflow = actions.filter((a) => inOverflow(a));

  return (
    <div ref={rootRef} className={`relative flex items-center gap-1 shrink-0 ${className ?? ''}`}>
      {visible.map((action) => {
        const compact = isCompact(action);
        const tone = action.tone ?? (action.active ? 'var(--accent)' : 'var(--text-secondary)');
        return (
          <button
            key={action.key}
            type="button"
            onClick={action.onClick}
            disabled={action.disabled}
            title={action.title ?? action.label}
            aria-label={action.label}
            aria-pressed={action.active}
            className={`pd-btn flex items-center shrink-0 rounded transition-colors ${
              compact ? 'justify-center w-6 h-6' : 'gap-1 px-1.5 h-6'
            }`}
            style={{
              color: tone,
              backgroundColor: action.active ? 'var(--accent-light)' : 'transparent',
              opacity: action.disabled ? 0.4 : 1,
              fontSize: 10,
            }}
          >
            {action.icon}
            {!compact && <span>{action.label}</span>}
          </button>
        );
      })}

      {overflow.length > 0 && (
        <button
          type="button"
          onClick={() => setOverflowOpen((v) => !v)}
          title="更多操作"
          aria-label="更多操作"
          aria-expanded={overflowOpen}
          className="pd-btn flex items-center justify-center shrink-0 rounded transition-colors w-6 h-6"
          style={{
            color: overflowOpen ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: overflowOpen ? 'var(--accent-light)' : 'transparent',
          }}
        >
          <MoreHorizontal size={13} />
        </button>
      )}

      {overflowOpen && overflow.length > 0 && (
        <div
          className="absolute right-0 top-full mt-1 z-50 rounded-lg shadow-lg py-1 text-[11px] min-w-[140px]"
          style={{
            backgroundColor: 'var(--bg-primary)',
            border: '1px solid var(--border)',
            color: 'var(--text-primary)',
          }}
        >
          {overflow.map((action) => (
            <button
              key={action.key}
              type="button"
              onClick={() => {
                setOverflowOpen(false);
                action.onClick();
              }}
              disabled={action.disabled}
              className="pd-btn w-full flex items-center gap-2 px-2 py-1.5 transition-colors hover:bg-[var(--bg-tertiary)] disabled:opacity-40"
              style={{ color: 'var(--text-secondary)', textAlign: 'left' }}
            >
              {action.icon}
              <span className="flex-1">{action.label}</span>
              {action.shortcut && (
                <span style={{ color: 'var(--text-tertiary)' }}>{action.shortcut}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
