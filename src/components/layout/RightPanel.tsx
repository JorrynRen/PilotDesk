import { useState } from 'react';
import { X, Lightbulb, Settings, Cpu, Bot } from 'lucide-react';
import { ConfigEditor } from '../panels/ConfigEditor';
import { useSessionStore } from '../../stores/sessionStore';

export function RightPanel() {
  const [isOpen, setIsOpen] = useState(false);
  const [activeTab, setActiveTab] = useState<string>('inspiration');
  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  if (!isOpen) {
    return (
      <button
        onClick={() => setIsOpen(true)}
        className="absolute right-2 top-1/2 -translate-y-1/2 p-1 rounded transition-colors"
        style={{ color: 'var(--text-secondary)' }}
        title="打开右侧面板"
      >
        <Lightbulb size={16} />
      </button>
    );
  }

  const tabs = [
    { id: 'inspiration', icon: Lightbulb, label: '灵感' },
    { id: 'skills', icon: Cpu, label: '技能' },
    { id: 'memory', icon: Bot, label: '记忆' },
    { id: 'config', icon: Settings, label: '配置' },
  ];

  const renderContent = () => {
    switch (activeTab) {
      case 'config':
        return <ConfigEditor agent={currentSession?.agentType ?? ''} />;
      case 'inspiration':
      case 'skills':
      case 'memory':
      default:
        return (
          <div className="flex items-center justify-center h-full">
            <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {tabs.find(t => t.id === activeTab)?.label}面板 - 待实现
            </p>
          </div>
        );
    }
  };

  return (
    <aside
      className="w-80 flex flex-col shrink-0"
      style={{ borderLeft: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-3 h-9" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="flex gap-0.5">
          {tabs.map(({ id, icon: Icon, label }) => (
            <button
              key={id}
              onClick={() => setActiveTab(id)}
              className="px-2 py-1 rounded text-xs transition-colors flex items-center gap-1"
              style={{
                color: activeTab === id ? 'var(--accent)' : 'var(--text-secondary)',
                backgroundColor: activeTab === id ? 'var(--border)' : 'transparent',
              }}
            >
              <Icon size={12} />
              {label}
            </button>
          ))}
        </div>
        <button onClick={() => setIsOpen(false)} className="p-1 rounded" style={{ color: 'var(--text-secondary)' }}>
          <X size={14} />
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-hidden">
        {renderContent()}
      </div>
    </aside>
  );
}
