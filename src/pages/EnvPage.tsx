import { ArrowLeft } from 'lucide-react';
import { EnvManager } from '../components/env/EnvManager';

interface EnvPageProps {
  onBack: () => void;
}

export function EnvPage({ onBack }: EnvPageProps) {
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
        <EnvManager />
      </div>
    </div>
  );
}
