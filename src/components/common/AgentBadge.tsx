import clsx from 'clsx';

interface AgentBadgeProps {
  agentType: 'claude' | 'hermes' | 'api';
  size?: 'sm' | 'md';
}

const AGENT_STYLES: Record<string, { bg: string; color: string; label: string }> = {
  claude: { bg: 'rgba(59,130,246,0.15)', color: '#3B82F6', label: 'C' },
  hermes: { bg: 'rgba(139,92,246,0.15)', color: '#8B5CF6', label: 'H' },
  api:    { bg: 'rgba(16,185,129,0.15)', color: '#10B981', label: 'A' },
};

export function AgentBadge({ agentType, size = 'sm' }: AgentBadgeProps) {
  const style = AGENT_STYLES[agentType] || AGENT_STYLES.claude;
  return (
    <span
      className={clsx(
        'inline-flex items-center justify-center rounded font-semibold',
        size === 'sm' ? 'w-5 h-5 text-[10px]' : 'w-6 h-6 text-xs'
      )}
      style={{
        backgroundColor: style.bg,
        color: style.color,
      }}
    >
      {style.label}
    </span>
  );
}
