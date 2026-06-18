import { useState, useEffect, useRef } from 'react';
import { Lightbulb, Cpu, Bot, ChevronDown } from 'lucide-react';
import { SkillBrowser } from '../panels/SkillBrowser';
import { MemoryBrowser } from '../panels/MemoryBrowser';
import { useSessionStore } from '../../stores/sessionStore';
import { InspirationPanel } from './InspirationPanel';
import { PluginManager } from '../plugin/PluginManager';
import { PluginPanelRenderer } from '../plugin/PluginPanelRenderer';
import { PluginIcon } from '../plugin/PluginIcon';
import { usePluginStore } from '../../stores/pluginStore';

interface RightPanelProps {
  isOpen: boolean;
}

export function RightPanel({ isOpen }: RightPanelProps) {
  const [activeTab, setActiveTab] = useState<string>('inspiration');
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // 直接从 store 订阅 registeredPanels（精确订阅，仅当面板变化时重渲染）
  const registeredPanels = usePluginStore((s) => s.registeredPanels);
  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  // 将 Map 转为数组供渲染
  const pluginPanels = Array.from(registeredPanels.values()).map((p) => ({
    id: p.contribution.id,
    title: p.contribution.title,
    pluginId: p.pluginId,
    pluginPath: p.pluginPath,
    icon: p.contribution.icon,
    uniqueKey: p.pluginId + ':' + p.contribution.id,
  }));

  // 点击外部关闭下拉
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setDropdownOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  if (!isOpen) return null;

  const isPluginPanelActive = activeTab.startsWith('plugin:');
  const activePluginPanel = pluginPanels.find((p) => 'plugin:' + p.uniqueKey === activeTab);

  const renderContent = () => {
    switch (activeTab) {
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
      case 'plugins':
        return <PluginManager />;
      case 'inspiration':
        return <InspirationPanel />;
      default:
        if (isPluginPanelActive) {
          const parts = activeTab.split(':');
          const panelId = parts.slice(2).join(':');
          return <PluginPanelRenderer activePanelId={panelId} onPanelChange={(id) => setActiveTab('plugin:' + parts[1] + ':' + id)} />;
        }
        return <InspirationPanel />;
    }
  };

  return (
    <aside
      className="w-[340px] flex flex-col shrink-0"
      style={{ borderLeft: '1px solid var(--border)', backgroundColor: 'var(--bg-primary)' }}
    >
      {/* Header */}
      <div className="flex items-center px-3 h-9 gap-0.5" style={{ borderBottom: '1px solid var(--border)' }}>
        {/* Fixed tabs */}
        <button
          onClick={() => setActiveTab('inspiration')}
          className="pd-btn px-2 py-1 rounded text-xs shrink-0"
          style={{
            color: activeTab === 'inspiration' ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: activeTab === 'inspiration' ? 'var(--accent-light)' : 'transparent',
          }}
        >
          <Lightbulb size={12} />
          灵感
        </button>
        <button
          onClick={() => setActiveTab('skills')}
          className="pd-btn px-2 py-1 rounded text-xs shrink-0"
          style={{
            color: activeTab === 'skills' ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: activeTab === 'skills' ? 'var(--accent-light)' : 'transparent',
          }}
        >
          <Cpu size={12} />
          技能
        </button>
        <button
          onClick={() => setActiveTab('memory')}
          className="pd-btn px-2 py-1 rounded text-xs shrink-0"
          style={{
            color: activeTab === 'memory' ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: activeTab === 'memory' ? 'var(--accent-light)' : 'transparent',
          }}
        >
          <Bot size={12} />
          记忆
        </button>
        <button
          onClick={() => setActiveTab('plugins')}
          className="pd-btn px-2 py-1 rounded text-xs shrink-0"
          style={{
            color: activeTab === 'plugins' ? 'var(--accent)' : 'var(--text-secondary)',
            backgroundColor: activeTab === 'plugins' ? 'var(--accent-light)' : 'transparent',
          }}
        >
          <Cpu size={12} />
          插件
        </button>

        {/* Plugin panels dropdown */}
        {pluginPanels.length > 0 && (
          <div className="relative" ref={dropdownRef}>
            <button
              onClick={() => setDropdownOpen(!dropdownOpen)}
              className="pd-btn px-2 py-1 rounded text-xs"
              style={{
                color: isPluginPanelActive ? 'var(--accent)' : 'var(--text-secondary)',
                backgroundColor: isPluginPanelActive ? 'var(--accent-light)' : 'transparent',
              }}
            >
              <PluginIcon icon={activePluginPanel ? activePluginPanel.icon : undefined} pluginId={activePluginPanel ? activePluginPanel.pluginId : ""} size={12} />
              <span title={activePluginPanel ? activePluginPanel.title : '面板'} className="truncate" style={{ maxWidth: '5ch', display: 'inline-block', verticalAlign: 'middle' }}>
                {activePluginPanel ? activePluginPanel.title : '面板'}
              </span>
              <ChevronDown size={10} />
            </button>
            {dropdownOpen && (
              <div
                className="absolute top-full right-0 mt-1 w-56 py-1 rounded-lg shadow-lg z-50"
                style={{
                  backgroundColor: 'var(--bg-primary)',
                  border: '1px solid var(--border)',
                }}
              >
                {pluginPanels.map((panel) => (
                  <button
                    key={panel.id}
                    onClick={() => {
                      setActiveTab('plugin:' + panel.uniqueKey);
                      setDropdownOpen(false);
                    }}
                    className="w-full text-left px-3 py-1.5 text-xs transition-colors"
                    style={{
                      color: activeTab === 'plugin:' + panel.uniqueKey ? 'var(--accent)' : 'var(--text-primary)',
                      backgroundColor: activeTab === 'plugin:' + panel.uniqueKey ? 'var(--accent-light)' : 'transparent',
                    }}
                  >
                    <PluginIcon icon={panel.icon} pluginId={panel.pluginId} size={12} />
                    <span className="truncate">{panel.title}</span>
                    <span className="text-[9px] ml-auto shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                      · {panel.pluginId}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-hidden">
        {renderContent()}
      </div>
    </aside>
  );
}
