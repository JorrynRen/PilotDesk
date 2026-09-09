import { useState } from 'react';
import { Lightbulb, Cpu, Brain, Package } from 'lucide-react';
import { SkillBrowser } from '../panels/SkillBrowser';
import { ProjectMemoryPreview } from '../panels/ProjectMemoryPreview';
import { useSessionStore } from '../../stores/sessionStore';
import { InspirationPanel } from './InspirationPanel';
import { PluginManager, type PluginPanelEntry } from '../plugin/PluginManager';
import { PluginPanelRenderer } from '../plugin/PluginPanelRenderer';
import { usePluginStore } from '../../stores/pluginStore';

interface RightPanelProps {
  isOpen: boolean;
  /** 当前主视图模式：终端模式下隐藏与 Agent 会话无关的 tab（记忆/插件） */
  mode: 'session' | 'terminal';
}

export function RightPanel({ isOpen, mode }: RightPanelProps) {
  const isTerminalMode = mode === 'terminal';
  const [activeTab, setActiveTab] = useState<string>('inspiration');

  // 直接从 store 订阅 registeredPanels（精确订阅，仅当面板变化时重渲染）
  const registeredPanels = usePluginStore((s) => s.registeredPanels);
  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  // 将面板按插件分组，供「插件」管理列表直接跳转打开（替代原头部「面板」下拉入口）
  const panelsByPlugin = new Map<string, PluginPanelEntry[]>();
  for (const p of registeredPanels.values()) {
    const entry: PluginPanelEntry = {
      uniqueKey: p.pluginId + ':' + p.contribution.id,
      panelId: p.contribution.id,
      title: p.contribution.title,
      icon: p.contribution.icon || p.pluginIcon,
      pluginId: p.pluginId,
    };
    const arr = panelsByPlugin.get(p.pluginId);
    if (arr) arr.push(entry);
    else panelsByPlugin.set(p.pluginId, [entry]);
  }

  if (!isOpen) return null;

  const isPluginPanelActive = activeTab.startsWith('plugin:');

  const renderContent = () => {
    switch (activeTab) {
      case 'skills':
        return (
          <SkillBrowser
            agentType={currentSession?.agentType ?? ''}
            onSkillSelect={(name) => {
              console.log('Skill selected:', name);
            }}
          />
        );
      case 'memory':
        return <ProjectMemoryPreview />;
      case 'plugins':
        return (
          <PluginManager
            panelsByPluginId={panelsByPlugin}
            onOpenPanel={(uniqueKey) => setActiveTab('plugin:' + uniqueKey)}
          />
        );
      case 'inspiration':
        return <InspirationPanel />;
      default:
        if (isPluginPanelActive) {
          const parts = activeTab.split(':');
          const panelId = parts.slice(2).join(':');
          return (
            <PluginPanelRenderer
              activePanelId={panelId}
              onPanelChange={(id) => setActiveTab('plugin:' + parts[1] + ':' + id)}
              onBack={() => setActiveTab('plugins')}
            />
          );
        }
        return <InspirationPanel />;
    }
  };

  return (
    <aside
      className="w-[280px] flex flex-col shrink-0"
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
        {!isTerminalMode && (
          <button
            onClick={() => setActiveTab('memory')}
            className="pd-btn px-2 py-1 rounded text-xs shrink-0"
            style={{
              color: activeTab === 'memory' ? 'var(--accent)' : 'var(--text-secondary)',
              backgroundColor: activeTab === 'memory' ? 'var(--accent-light)' : 'transparent',
            }}
          >
            <Brain size={12} />
            记忆
          </button>
        )}
        {!isTerminalMode && (
          <button
            onClick={() => setActiveTab('plugins')}
            className="pd-btn px-2 py-1 rounded text-xs shrink-0"
            style={{
              // 插件面板为插件列表的二级视图，切到面板时高亮「插件」tab
              color: activeTab === 'plugins' || isPluginPanelActive ? 'var(--accent)' : 'var(--text-secondary)',
              backgroundColor: activeTab === 'plugins' || isPluginPanelActive ? 'var(--accent-light)' : 'transparent',
            }}
          >
            <Package size={12} />
            插件
          </button>
        )}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-hidden">
        {renderContent()}
      </div>
    </aside>
  );
}
