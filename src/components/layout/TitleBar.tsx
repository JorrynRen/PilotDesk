import { useState, useCallback, useEffect, useRef } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Settings, PanelRightOpen, PanelRightClose, Minus, Square, X, Copy, ArrowLeft, Workflow } from 'lucide-react';

export type StatusHintState = 'loading' | 'ready' | 'error' | 'saving' | 'saved' | 'save-error' | 'idle';

export interface StatusHint {
  state: StatusHintState;
  /** 状态文本，留空则使用默认文本 */
  text?: string;
  /** idle状态时自动清除定时器（毫秒），默认常驻 */
  autoDismiss?: number;
}

interface TitleBarProps {
  onOpenSettings?: () => void;
  onOpenWorkflow?: () => void;
  onToggleRightPanel?: () => void;
  rightPanelOpen?: boolean;
  showBackButton?: boolean;
  titleText?: string;
  onBack?: () => void;
  /** 标题栏状态提示 */
  statusHint?: StatusHint | null;
}

/** 标题栏状态提示徽标组件 */
function StatusHintBadge({ hint }: { hint: StatusHint }) {
  const config: Record<StatusHintState, { icon: string; color: string; bg: string; defaultText: string }> = {
    loading:     { icon: '◎', color: 'var(--accent)',         bg: 'var(--accent-light)',           defaultText: '加载中...' },
    ready:       { icon: '✓', color: 'var(--status-success)', bg: 'var(--status-success-bg)',       defaultText: '已就绪' },
    error:       { icon: '✗', color: 'var(--status-danger)',  bg: 'var(--status-danger-bg)',        defaultText: '加载失败' },
    saving:      { icon: '◎', color: 'var(--accent)',         bg: 'var(--accent-light)',           defaultText: '保存中...' },
    saved:       { icon: '✓', color: 'var(--status-success)', bg: 'var(--status-success-bg)',       defaultText: '已保存' },
    'save-error': { icon: '✗', color: 'var(--status-danger)', bg: 'var(--status-danger-bg)',        defaultText: '保存失败' },
    idle:        { icon: '',   color: 'var(--text-tertiary)', bg: 'transparent',                    defaultText: '' },
  };

  const c = config[hint.state];
  const isLoading = hint.state === 'loading' || hint.state === 'saving';

  return (
    <span
      className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] font-medium pointer-events-none ml-2"
      style={{
        color: c.color,
        background: c.bg,
        border: `1px solid ${c.color}33`,
        whiteSpace: 'nowrap',
      }}
    >
      {isLoading && (
        <span className="inline-block" style={{ animation: 'pd-spin 1s linear infinite' }}>◎</span>
      )}
      {!isLoading && c.icon}
      <span>{hint.text || c.defaultText}</span>
    </span>
  );
}

export function TitleBar({ onOpenSettings, onOpenWorkflow, onToggleRightPanel, rightPanelOpen, showBackButton, titleText, onBack, statusHint }: TitleBarProps) {
  const PanelIcon = rightPanelOpen ? PanelRightClose : PanelRightOpen;
  const [isMaximized, setIsMaximized] = useState(false);
  const [tauriReady, setTauriReady] = useState(true);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        const win = getCurrentWindow();
        setIsMaximized(await win.isMaximized());

        const fn = await win.onResized(async () => {
          try {
            const maximized = await win.isMaximized();
            setIsMaximized(maximized);
          } catch { /* window closed */ }
        });
        unlisten = fn;
      } catch (err) {
        console.warn('[TitleBar] Tauri window API not available:', err);
        setTauriReady(false);
      }
    })();

    return () => { unlisten?.(); };
  }, []);

  const handleMinimize = useCallback(async () => {
    try { await getCurrentWindow().minimize(); } catch { /* ignore */ }
  }, []);

  const handleToggleMaximize = useCallback(async () => {
    try { await getCurrentWindow().toggleMaximize(); } catch { /* ignore */ }
  }, []);

  const handleClose = useCallback(async () => {
    try { await getCurrentWindow().close(); } catch { /* ignore */ }
  }, []);

  // Drag + double-click: only startDragging on mousemove beyond threshold,
  // so rapid double-clicks never trigger drag (mouse doesn't move).
  const clickCountRef = useRef(0);
  const clickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mouseDownPosRef = useRef<{ x: number; y: number } | null>(null);
  const draggingRef = useRef(false);

  const handleHeaderMouseDown = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    if (
      target.closest('button') ||
      target.closest('a') ||
      target.closest('input') ||
      target.closest('select') ||
      target.tagName === 'BUTTON' ||
      target.tagName === 'INPUT' ||
      target.tagName === 'SELECT'
    ) {
      return;
    }
    e.preventDefault();
    mouseDownPosRef.current = { x: e.clientX, y: e.clientY };
    draggingRef.current = false;
  }, []);

  const handleHeaderMouseUp = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const pos = mouseDownPosRef.current;
    mouseDownPosRef.current = null;
    if (draggingRef.current) {
      draggingRef.current = false;
      return;
    }
    if (!pos) return;
    // Count as click only if mouse didn't move much (< 5px)
    const dx = Math.abs(e.clientX - pos.x);
    const dy = Math.abs(e.clientY - pos.y);
    if (dx < 5 && dy < 5) {
      clickCountRef.current += 1;
      if (clickCountRef.current >= 2) {
        clickCountRef.current = 0;
        if (clickTimerRef.current) {
          clearTimeout(clickTimerRef.current);
          clickTimerRef.current = null;
        }
        try { getCurrentWindow().toggleMaximize(); } catch { /* ignore */ }
        return;
      }
      if (clickTimerRef.current) clearTimeout(clickTimerRef.current);
      clickTimerRef.current = setTimeout(() => {
        clickCountRef.current = 0;
        clickTimerRef.current = null;
      }, 400);
    }
  }, []);

  const handleHeaderMouseMove = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const pos = mouseDownPosRef.current;
    if (pos && !draggingRef.current) {
      // Start OS-level drag immediately to avoid cursor ghosting
      draggingRef.current = true;
      mouseDownPosRef.current = null;
      try { getCurrentWindow().startDragging(); } catch { /* ignore */ }
    }
  }, []);

  const handleHeaderMouseLeave = useCallback(() => {
    mouseDownPosRef.current = null;
    draggingRef.current = false;
  }, []);

  return (
    <header
      className="flex items-center justify-between px-3 h-12 shrink-0 select-none"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)', color: 'var(--text-primary)' }}
      onMouseDown={handleHeaderMouseDown}
      onMouseUp={handleHeaderMouseUp}
      onMouseMove={handleHeaderMouseMove}
      onMouseLeave={handleHeaderMouseLeave}
    >
      {/* Left: back button or logo + title */}
      <div className="flex items-center gap-2 h-full">
        {showBackButton ? (
          <>
            <button
              onClick={onBack || onOpenSettings}
              className="pd-btn p-1 rounded transition-colors hover:opacity-80"
              style={{ color: 'var(--text-secondary)' }}
              title="返回"
            >
              <ArrowLeft size={16} />
            </button>
            <span className="text-xs font-medium pointer-events-none" style={{ color: 'var(--text-primary)' }}>{titleText || '设置'}</span>
            {statusHint && statusHint.state !== 'idle' && (
              <StatusHintBadge hint={statusHint} />
            )}
          </>
        ) : (
          <>
            <img
              src="/logo.png"
              alt=""
              className="w-5 h-5 rounded pointer-events-none"
              draggable={false}
              onError={(e) => { (e.target as HTMLImageElement).style.display = 'none'; }}
            />
            <span className="text-xs font-medium pointer-events-none">PilotDesk</span>
          </>
        )}
      </div>

      {/* Right: settings + panel toggle | window controls */}
      <div className="flex items-center h-full">
        {!showBackButton && onOpenWorkflow && (
          <button
            onClick={onOpenWorkflow}
            className="pd-btn px-1.5 py-1 rounded transition-colors hover:opacity-80"
            style={{ color: 'var(--text-secondary)', background: 'transparent' }}
            title="工作流管理"
          >
            <span className="inline-flex items-center gap-1">
              <Workflow size={13} />
              <span className="text-[11px]">工作流</span>
            </span>
          </button>
        )}
        {!showBackButton && onOpenSettings && (
          <button
            onClick={onOpenSettings}
            className="pd-btn p-1 rounded transition-colors hover:opacity-80"
            style={{ color: 'var(--text-secondary)', background: 'transparent' }}
            title="设置"
          >
            <Settings size={13} />
          </button>
        )}
        {onToggleRightPanel && (
          <button
            onClick={onToggleRightPanel}
            className="pd-btn p-1 rounded transition-colors hover:opacity-80"
            style={{
              color: rightPanelOpen ? 'var(--accent)' : 'var(--text-secondary)',
              background: rightPanelOpen ? 'var(--border)' : 'transparent',
            }}
            title={rightPanelOpen ? '关闭侧边栏' : '打开侧边栏'}
          >
            <PanelIcon size={13} />
          </button>
        )}

        {/* Separator */}
        <div
          className="w-px h-4 mx-1.5"
          style={{ backgroundColor: 'var(--border)' }}
        />

        {/* Window controls */}
        {tauriReady && (
          <>
            <button
              onClick={handleMinimize}
              className="pd-btn w-8 h-full flex items-center justify-center transition-colors hover:bg-black/5"
              title="最小化"
            >
              <Minus size={13} style={{ color: 'var(--text-secondary)' }} />
            </button>
            <button
              onClick={handleToggleMaximize}
              className="pd-btn w-8 h-full flex items-center justify-center transition-colors hover:bg-black/5"
              title={isMaximized ? '还原' : '最大化'}
            >
              {isMaximized ? (
                <Copy size={11} style={{ color: 'var(--text-secondary)' }} />
              ) : (
                <Square size={11} style={{ color: 'var(--text-secondary)' }} />
              )}
            </button>
            <button
              onClick={handleClose}
              className="pd-btn w-8 h-full flex items-center justify-center transition-colors hover:bg-red-500 hover:text-white"
              title="关闭"
            >
              <X size={13} style={{ color: 'var(--text-secondary)' }} />
            </button>
          </>
        )}
      </div>
    </header>
  );
}
