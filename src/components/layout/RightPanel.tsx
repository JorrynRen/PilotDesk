import { useState } from 'react';
import { Lightbulb, Settings, Cpu, Bot } from 'lucide-react';
import { ConfigEditor } from '../panels/ConfigEditor';
import { SkillBrowser } from '../panels/SkillBrowser';
import { MemoryBrowser } from '../panels/MemoryBrowser';
import { useSessionStore } from '../../stores/sessionStore';

interface RightPanelProps {
  isOpen: boolean;
}

export function RightPanel({ isOpen }: RightPanelProps) {
  const [activeTab, setActiveTab] = useState<string>('inspiration');
  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  if (!isOpen) return null;

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
      case 'skills':
        return (
          <SkillBrowser
            agentType={currentSession?.agentType ?? 'claude'}
            onSkillSelect={(name) => {
              console.log('Skill selected:', name);
            }}
          />
        );
      case 'memory':
        return (
          <MemoryBrowser
            agentType={currentSession?.agentType}
            onSelect={(content) => {
              console.log('Memory selected:', content.slice(0, 50));
            }}
          />
        );
      case 'inspiration':
      default:
        return (
          <div className="flex flex-col items-center justify-center h-24 gap-2">
            <Lightbulb size={24} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              灵感面板 - 使用顶部菜单打开灵感市集
            </span>
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
      <div className="flex items-center px-3 h-9" style={{ borderBottom: '1px solid var(--border)' }}>
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
      </div>

      {/* Content */}
      <div className="flex-1 overflow-hidden">
        {renderContent()}
      </div>
    </aside>
  );
}
