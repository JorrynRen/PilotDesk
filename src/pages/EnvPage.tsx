import { ArrowLeft } from 'lucide-react';
import { EnvManager } from '../components/env/EnvManager';
import { useTheme } from '../hooks/useTheme';
import { Sun, Moon, Monitor } from 'lucide-react';

interface EnvPageProps {
  onBack: () => void;
}

export function EnvPage({ onBack }: EnvPageProps) {
  const { theme, setTheme } = useTheme();
  const themeOptions = [
    { value: 'dark' as const, icon: Moon, label: '深色' },
    { value: 'light' as const, icon: Sun, label: '浅色' },
    { value: 'system' as const, icon: Monitor, label: '跟随系统' },
  ];

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* Header */}
      <div className="shrink-0 px-4 py-3 flex items-center gap-3" style={{ borderBottom: '1px solid var(--border)' }}>
        <button onClick={onBack} className="p-1 rounded" style={{ color: 'var(--text-secondary)' }}>
          <ArrowLeft size={16} />
        </button>
        <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>环境管理</h2>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4 max-w-2xl mx-auto w-full">
        {/* Theme Settings */}
        <section style={{ marginBottom: '1.5rem' }}>
          <h3 className="text-xs font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>
            主题设置
          </h3>
          <div className="flex gap-1">
            {themeOptions.map(({ value, icon: Icon, label }) => (
              <button
                key={value}
                onClick={() => setTheme(value)}
                className="flex items-center gap-1 px-3 py-2 rounded-lg text-xs transition-colors"
                style={{
                  color: theme === value ? '#fff' : 'var(--text-secondary)',
                  backgroundColor: theme === value ? 'var(--accent)' : 'var(--bg-tertiary)',
                  border: '1px solid var(--border)',
                }}
              >
                <Icon size={12} />
                {label}
              </button>
            ))}
          </div>
        </section>

        <EnvManager />
      </div>
    </div>
  );
}
