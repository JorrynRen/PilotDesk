import { Moon, Sun, Monitor, Settings, Lightbulb } from 'lucide-react';
import { useTheme } from '../../hooks/useTheme';

type Theme = 'light' | 'dark' | 'system';

interface TitleBarProps {
  onOpenEnv?: () => void;
  onOpenMarket?: () => void;
}

export function TitleBar({ onOpenEnv, onOpenMarket }: TitleBarProps) {
  const { theme, setTheme } = useTheme();

  const themeOptions: { value: Theme; icon: typeof Moon; label: string }[] = [
    { value: 'light', icon: Sun, label: '浅色' },
    { value: 'dark', icon: Moon, label: '深色' },
    { value: 'system', icon: Monitor, label: '跟随系统' },
  ];

  return (
    <header
      className="flex items-center justify-between px-4 h-10 select-none shrink-0"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      data-tauri-drag-region
    >
      <div className="flex items-center gap-2" data-tauri-drag-region>
        <div
          className="w-5 h-5 rounded"
          style={{ background: 'linear-gradient(135deg, #5B7FFF, #8B5CF6)' }}
        />
        <span className="text-sm font-semibold" data-tauri-drag-region>PilotDesk</span>
      </div>

      <div className="flex items-center gap-1">
        {onOpenMarket && (
          <button
            onClick={onOpenMarket}
            className="p-1.5 rounded transition-colors"
            style={{ color: 'var(--text-secondary)', background: 'transparent' }}
            title="灵感市集"
          >
            <Lightbulb size={14} />
          </button>
        )}
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
        {themeOptions.map(({ value, icon: Icon, label }) => (
          <button
            key={value}
            onClick={() => setTheme(value)}
            className="p-1.5 rounded transition-colors"
            style={{
              color: theme === value ? 'var(--accent)' : 'var(--text-secondary)',
              background: theme === value ? 'var(--border)' : 'transparent',
            }}
            title={label}
          >
            <Icon size={14} />
          </button>
        ))}
      </div>
    </header>
  );
}
