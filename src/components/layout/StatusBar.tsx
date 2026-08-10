import { Loader2, Settings } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { useEnvInfo, type AgentDetectStatus } from '../../hooks/useEnvInfo';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import type { AgentConfig } from '../../types';

interface StatusBarProps {
  onOpenSettings?: () => void;
  onOpenEnvSettings?: () => void;
}

export function StatusBar({ onOpenSettings, onOpenEnvSettings }: StatusBarProps) {
  const navigate = useNavigate();
  const { envInfo, loading, agentStatus } = useEnvInfo();
  const { agents } = useAgentRegistry();

  // Show all enabled agents from DB, sorted by sortOrder
  const enabledAgents = agents
    .filter(a => a.isEnabled)
    .sort((a, b) => (a.sortOrder ?? 0) - (b.sortOrder ?? 0));

  // Determine per-agent display state using the agent's own color from DB
  const getAgentState = (agent: AgentConfig, version: string | null): {
    status: AgentDetectStatus;
    label: string;
    dotColor: string;
  } => {
    const status = agentStatus[agent.agentType];
    if (status === 'done' && version) {
      return { status: 'done', label: version, dotColor: agent.color || '#9CA3AF' };
    }
    if (status === 'error' || (status === 'done' && !version)) {
      return { status: 'error', label: '未安装', dotColor: '#9CA3AF' };
    }
    // pending or detecting — colon only, spinner indicates progress
    return { status: 'detecting', label: '', dotColor: '#9CA3AF' };
  };

  return (
    <footer
      className="flex items-center justify-between px-4 h-8 text-[10px] shrink-0 select-none"
      style={{ borderTop: '1px solid var(--border)', color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
    >
      <div className="flex items-center gap-3">
        {/* 最左端：设置按钮（原放在 TitleBar 右上角） */}
        {onOpenSettings && (
          <button
            onClick={onOpenSettings}
            className="pd-btn flex items-center gap-1 rounded transition-colors hover:opacity-80 p-0.5"
            title="设置"
            style={{ color: 'var(--text-secondary)', background: 'transparent' }}
          >
            <Settings size={12} />
            <span>设置</span>
          </button>
        )}
        {/* 分隔符（设置 vs Agent 状态区） */}
        <div className="w-px h-3" style={{ backgroundColor: 'var(--border)' }} />
        <span className="flex items-center gap-1">Agent:</span>
        {enabledAgents.length === 0 && (
          <button
            onClick={() => navigate('/settings?tab=agents')}
            className="pd-btn flex items-center gap-1 transition-colors hover:opacity-80"
            title="点击前往设置页配置 Agent 集成"
            style={{ color: 'var(--text-tertiary)' }}
          >
            <Settings size={10} />
            未安装或未集成配置
          </button>
        )}
        {enabledAgents.map((agent) => {
          const version = envInfo?.agentVersions?.[agent.agentType] ?? null;
          const { status, label, dotColor } = getAgentState(agent, version);
          const displayName = agent.displayName || agent.agentType;
          return (
            <button
              key={agent.agentType}
              onClick={() => {
                (onOpenEnvSettings ?? onOpenSettings)?.();
              }}
              className="pd-btn flex items-center gap-1 transition-colors hover:opacity-80"
              title={status === 'detecting' ? '点击刷新环境检测' : '点击查看环境检测'}
            >
              <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: dotColor }} />
              {displayName}:{status === 'detecting' ? <Loader2 size={10} className="animate-spin" style={{ color: '#9CA3AF' }} /> : label ? ` ${label}` : ''}
            </button>
          );
        })}
      </div>
      <div className="flex items-center gap-2">
        <span style={{ color: 'var(--text-tertiary)' }}>PilotDesk v0.1.0</span>
      </div>
    </footer>
  );
}
