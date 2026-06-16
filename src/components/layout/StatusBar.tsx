import { AGENT_THEMES } from '../../types';
import { useEnvInfo } from '../../hooks/useEnvInfo';

interface StatusBarProps {
  onOpenSettings?: () => void;
}

export function StatusBar({ onOpenSettings }: StatusBarProps) {
  const { envInfo, loading } = useEnvInfo();

  const pendingLabel = loading && !envInfo ? '查询中…' : '未安装';
  const pendingColor = loading && !envInfo ? '#9CA3AF' : '#6B7280';

  const claudeStatus = envInfo?.claudeCodeVersion
    ? { color: AGENT_THEMES.claude.color, label: envInfo.claudeCodeVersion }
    : { color: pendingColor, label: pendingLabel };

  const hermesStatus = envInfo?.hermesVersion
    ? { color: AGENT_THEMES.hermes.color, label: envInfo.hermesVersion }
    : { color: pendingColor, label: pendingLabel };

  const codexStatus = envInfo?.codexVersion
    ? { color: AGENT_THEMES.codex.color, label: envInfo.codexVersion }
    : { color: pendingColor, label: pendingLabel };

  return (
    <footer
      className="flex items-center justify-between px-4 h-8 text-[10px] shrink-0 select-none"
      style={{ borderTop: '1px solid var(--border)', color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
    >
      <div className="flex items-center gap-3">
        {/* Agent status — no longer needs Sidecar/WS indicator */}
        <span className="flex items-center gap-1">
          <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: '#10B981' }} />
          Agent: 就绪
        </span>
        <button
          onClick={onOpenSettings}
          className="flex items-center gap-1 transition-colors hover:opacity-80"
          title="点击打开设置"
        >
          <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: claudeStatus.color }} />
          Claude: {claudeStatus.label}
        </button>
        <button
          onClick={onOpenSettings}
          className="flex items-center gap-1 transition-colors hover:opacity-80"
          title="点击打开设置"
        >
          <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: hermesStatus.color }} />
          Hermes: {hermesStatus.label}
        </button>
        <button
          onClick={onOpenSettings}
          className="flex items-center gap-1 transition-colors hover:opacity-80"
          title="点击打开设置"
        >
          <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: codexStatus.color }} />
          codeX: {codexStatus.label}
        </button>
      </div>
      <div className="flex items-center gap-2">
        <span style={{ color: 'var(--text-tertiary)' }}>PilotDesk v0.1.0</span>
      </div>
    </footer>
  );
}
