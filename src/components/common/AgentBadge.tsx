import clsx from 'clsx';

interface AgentBadgeProps {
  agentType: 'claude' | 'hermes';
  size?: 'sm' | 'md';
}

export function AgentBadge({ agentType, size = 'sm' }: AgentBadgeProps) {
  const isClaude = agentType === 'claude';
  return (
    <span
      className={clsx(
        'inline-flex items-center justify-center rounded font-semibold',
        size === 'sm' ? 'w-5 h-5 text-[10px]' : 'w-6 h-6 text-xs'
      )}
      style={{
        backgroundColor: isClaude ? 'rgba(59,130,246,0.15)' : 'rgba(139,92,246,0.15)',
        color: isClaude ? '#3B82F6' : '#8B5CF6',
      }}
    >
      {isClaude ? 'C' : 'H'}
    </span>
  );
}
