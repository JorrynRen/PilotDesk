import { useState, useCallback, useEffect, useRef } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Settings, PanelRightOpen, PanelRightClose, Minus, Square, X, Copy } from 'lucide-react';

interface TitleBarProps {
  onOpenSettings?: () => void;
  onToggleRightPanel?: () => void;
  rightPanelOpen?: boolean;
}

export function TitleBar({ onOpenSettings, onToggleRightPanel, rightPanelOpen }: TitleBarProps) {
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
      const dx = Math.abs(e.clientX - pos.x);
      const dy = Math.abs(e.clientY - pos.y);
      if (dx > 5 || dy > 5) {
        // Mouse moved beyond threshold — start OS-level drag
        draggingRef.current = true;
        mouseDownPosRef.current = null;
        try { getCurrentWindow().startDragging(); } catch { /* ignore */ }
      }
    }
  }, []);

  const handleHeaderMouseLeave = useCallback(() => {
    mouseDownPosRef.current = null;
    draggingRef.current = false;
  }, []);

  return (
    <header
      className="flex items-center justify-between px-3 h-12 shrink-0 select-none"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      onMouseDown={handleHeaderMouseDown}
      onMouseUp={handleHeaderMouseUp}
      onMouseMove={handleHeaderMouseMove}
      onMouseLeave={handleHeaderMouseLeave}
    >
      {/* Left: logo + title */}
      <div className="flex items-center gap-2 h-full">
        <img
          src="/logo.png"
          alt=""
          className="w-5 h-5 rounded pointer-events-none"
          draggable={false}
          onError={(e) => { (e.target as HTMLImageElement).style.display = 'none'; }}
        />
        <span className="text-xs font-semibold pointer-events-none">PilotDesk</span>
      </div>

      {/* Right: settings + panel toggle | window controls */}
      <div className="flex items-center h-full">
        {onOpenSettings && (
          <button
            onClick={onOpenSettings}
            className="p-1 rounded transition-colors hover:opacity-80"
            style={{ color: 'var(--text-secondary)', background: 'transparent' }}
            title="设置"
          >
            <Settings size={13} />
          </button>
        )}
        {onToggleRightPanel && (
          <button
            onClick={onToggleRightPanel}
            className="p-1 rounded transition-colors hover:opacity-80"
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
              className="w-8 h-full flex items-center justify-center transition-colors hover:bg-black/5"
              title="最小化"
            >
              <Minus size={13} style={{ color: 'var(--text-secondary)' }} />
            </button>
            <button
              onClick={handleToggleMaximize}
              className="w-8 h-full flex items-center justify-center transition-colors hover:bg-black/5"
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
              className="w-8 h-full flex items-center justify-center transition-colors hover:bg-red-500 hover:text-white"
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
