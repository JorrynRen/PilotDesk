import { useCallback, useEffect, useState } from 'react';
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

  useEffect(() => {
    const win = getCurrentWindow();
    // Initial state
    win.isMaximized().then(setIsMaximized);

    // Listen for maximize state changes
    let unlisten: (() => void) | undefined;
    win.onResized(async () => {
      const maximized = await win.isMaximized();
      setIsMaximized(maximized);
    }).then(fn => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  const handleMinimize = useCallback(async () => {
    await getCurrentWindow().minimize();
  }, []);

  const handleToggleMaximize = useCallback(async () => {
    const win = getCurrentWindow();
    await win.toggleMaximize();
  }, []);

  const handleClose = useCallback(async () => {
    await getCurrentWindow().close();
  }, []);

  const handleDragStart = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    if (e.target === e.currentTarget || (e.target as HTMLElement).hasAttribute('data-tauri-drag-region')) {
      e.preventDefault();
      getCurrentWindow().startDragging();
    }
  }, []);

  const handleDoubleClick = useCallback(() => {
    getCurrentWindow().toggleMaximize();
  }, []);

  return (
    <header
      className="flex items-center justify-between px-3 h-9 shrink-0 select-none"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      onDoubleClick={handleDoubleClick}
    >
      {/* Left: logo + title (drag region) */}
      <div
        className="flex items-center gap-2 h-full"
        onMouseDown={handleDragStart}
      >
        <img
          src="/logo.png"
          alt=""
          className="w-5 h-5 rounded pointer-events-none"
          draggable={false}
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
      </div>
    </header>
  );
}
