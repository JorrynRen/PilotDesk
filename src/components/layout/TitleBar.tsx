import { Settings, PanelRightOpen, PanelRightClose } from 'lucide-react';

interface TitleBarProps {
  onOpenEnv?: () => void;
  onToggleRightPanel?: () => void;
  rightPanelOpen?: boolean;
}

export function TitleBar({ onOpenEnv, onToggleRightPanel, rightPanelOpen }: TitleBarProps) {
  const PanelIcon = rightPanelOpen ? PanelRightClose : PanelRightOpen;

  return (
    <header
      className="flex items-center justify-between px-4 h-10 select-none shrink-0"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      data-tauri-drag-region
    >
      <div className="flex items-center gap-2" data-tauri-drag-region>
        <img
          src="/logo.png"
          alt=""
          className="w-7 h-7 rounded-md"
          draggable={false}
        />
        <span className="text-sm font-semibold" data-tauri-drag-region>PilotDesk</span>
      </div>

      <div className="flex items-center gap-1">
        {onOpenEnv && (
          <button
            onClick={onOpenEnv}
            className="p-1.5 rounded transition-colors"
            style={{ color: 'var(--text-secondary)', background: 'transparent' }}
            title="环境管理"
          >
            <Settings size={14} />
          </button>
        )}
        {onToggleRightPanel && (
          <button
            onClick={onToggleRightPanel}
            className="p-1.5 rounded transition-colors"
            style={{
              color: rightPanelOpen ? 'var(--accent)' : 'var(--text-secondary)',
              background: rightPanelOpen ? 'var(--border)' : 'transparent',
            }}
            title={rightPanelOpen ? '关闭侧边栏' : '打开侧边栏'}
          >
            <PanelIcon size={14} />
          </button>
        )}
      </div>
    </header>
  );
}
