import { Loader2 } from 'lucide-react';
import { AGENT_THEMES } from '../../types';
import { useEnvInfo, type AgentDetectStatus } from '../../hooks/useEnvInfo';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';

interface StatusBarProps {
  onOpenSettings?: () => void;
  onOpenEnvSettings?: () => void;
}

export function StatusBar({ onOpenSettings, onOpenEnvSettings }: StatusBarProps) {
  const { envInfo, loading, agentStatus } = useEnvInfo();
  const { agents } = useAgentRegistry();

  // Show all enabled agents from DB, with versions from envInfo
  const agentEntries = agents
    .filter(a => a.isEnabled)
    .sort((a, b) => (a.sortOrder ?? 0) - (b.sortOrder ?? 0))
    .map(agent => [agent.agentType, envInfo?.agentVersions?.[agent.agentType] ?? null] as const);

  // Determine per-agent display state
  const getAgentState = (agentType: string, version: string | null): {
    status: AgentDetectStatus;
    label: string;
    dotColor: string;
  } => {
    const status = agentStatus[agentType];
    if (status === 'done' && version) {
      const theme = AGENT_THEMES[agentType];
      return { status: 'done', label: version, dotColor: theme?.color ?? '#6366F1' };
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
        <span className="flex items-center gap-1">Agent:</span>
        {agentEntries.length === 0 && (
          <span style={{ color: 'var(--text-tertiary)' }}>未检测</span>
        )}
        {agentEntries.map(([agentType, version]) => {
          const { status, label, dotColor } = getAgentState(agentType, version);
          const theme = AGENT_THEMES[agentType];
          const displayName = theme?.label ?? agentType;
          return (
            <button
              key={agentType}
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
