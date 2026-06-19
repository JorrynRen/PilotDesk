import { Loader2 } from 'lucide-react';
import { AGENT_THEMES } from '../../types';
import { useEnvInfo } from '../../hooks/useEnvInfo';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';

interface StatusBarProps {
  onOpenSettings?: () => void;
  onOpenEnvSettings?: () => void;
}

export function StatusBar({ onOpenSettings, onOpenEnvSettings }: StatusBarProps) {
  const { envInfo, loading } = useEnvInfo();
  const { agents } = useAgentRegistry();

  const pendingLabel = loading && !envInfo ? '查询中…' : '未安装';
  const pendingColor = loading && !envInfo ? '#9CA3AF' : '#6B7280';

  // Only show enabled agents that have been detected
  const enabledAgentTypes = agents.filter(a => a.isEnabled).map(a => a.agentType);
  const agentEntries = envInfo?.agentVersions
    ? Object.entries(envInfo.agentVersions).filter(([agentType]) => enabledAgentTypes.includes(agentType))
    : [];

  // Show loading state when envInfo is being fetched
  const showLoading = loading && !envInfo;
  // Show fallback when envInfo loaded but no enabled agents detected
  const showEmpty = !loading && envInfo && agentEntries.length === 0;

  return (
    <footer
      className="flex items-center justify-between px-4 h-8 text-[10px] shrink-0 select-none"
      style={{ borderTop: '1px solid var(--border)', color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
    >
      <div className="flex items-center gap-3">
        {/* Agent status — no longer needs Sidecar/WS indicator */}
        <span className="flex items-center gap-1">
          Agent:
        </span>
        {showLoading && (
          <span className="flex items-center gap-1" style={{ color: 'var(--text-tertiary)' }}>
            <Loader2 size={10} className="animate-spin" /> 查询中…
          </span>
        )}
        {showEmpty && (
          <span style={{ color: 'var(--text-tertiary)' }}>未检测</span>
        )}
        {agentEntries.map(([agentType, version]) => {
          const theme = AGENT_THEMES[agentType];
          const color = theme?.color ?? '#6366F1';
          const label = version ?? pendingLabel;
          const dotColor = version ? color : pendingColor;
          const displayName = theme?.label ?? agentType;
          return (
            <button
              key={agentType}
              onClick={onOpenEnvSettings ?? onOpenSettings}
              className="pd-btn flex items-center gap-1 transition-colors hover:opacity-80"
              title={`点击查看环境检测`}
            >
              <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: dotColor }} />
              {displayName}: {label}
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
