import { getCurrentWindow } from '@tauri-apps/api/window';
import { Settings, PanelRightOpen, PanelRightClose, Minus, Square, X } from 'lucide-react';

interface TitleBarProps {
  onOpenEnv?: () => void;
  onToggleRightPanel?: () => void;
  rightPanelOpen?: boolean;
}

export function TitleBar({ onOpenEnv, onToggleRightPanel, rightPanelOpen }: TitleBarProps) {
  const appWindow = getCurrentWindow();
  const PanelIcon = rightPanelOpen ? PanelRightClose : PanelRightOpen;

  return (
    <header
      className="flex items-center justify-between px-3 h-9 shrink-0 select-none"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      data-tauri-drag-region
    >
      {/* Left: logo + title */}
      <div className="flex items-center gap-2" data-tauri-drag-region>
        <img
          src="/logo.png"
          alt=""
          className="w-5 h-5 rounded"
          draggable={false}
        />
        <span className="text-xs font-semibold" data-tauri-drag-region>PilotDesk</span>
      </div>

      {/* Center: functional buttons */}
      <div className="flex items-center gap-1" data-tauri-drag-region>
        {onOpenEnv && (
          <button
            onClick={onOpenEnv}
            className="p-1 rounded transition-colors hover:opacity-80"
            style={{ color: 'var(--text-secondary)', background: 'transparent' }}
            title="环境管理"
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
      </div>

      {/* Right: window controls */}
      <div className="flex items-center" data-tauri-drag-region>
        <button
          onClick={() => appWindow.minimize()}
          className="w-8 h-8 flex items-center justify-center transition-colors hover:bg-black/5"
          data-tauri-drag-region
          title="最小化"
        >
          <Minus size={13} style={{ color: 'var(--text-secondary)' }} />
        </button>
        <button
          onClick={() => appWindow.toggleMaximize()}
          className="w-8 h-8 flex items-center justify-center transition-colors hover:bg-black/5"
          data-tauri-drag-region
          title="最大化"
        >
          <Square size={11} style={{ color: 'var(--text-secondary)' }} />
        </button>
        <button
          onClick={() => appWindow.close()}
          className="w-8 h-8 flex items-center justify-center transition-colors hover:bg-red-500 hover:text-white"
          data-tauri-drag-region
          title="关闭"
        >
          <X size={13} style={{ color: 'var(--text-secondary)' }} />
        </button>
      </div>
    </header>
  );
}
