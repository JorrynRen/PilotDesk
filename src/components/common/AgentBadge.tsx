import clsx from 'clsx';
import { AGENT_THEMES } from '../../types';

interface AgentBadgeProps {
  agentType: 'claude' | 'hermes' | 'api';
  size?: 'sm' | 'md';
}

export function AgentBadge({ agentType, size = 'sm' }: AgentBadgeProps) {
  const theme = AGENT_THEMES[agentType] || AGENT_THEMES.claude;
  return (
    <span
      className={clsx(
        'inline-flex items-center justify-center rounded font-semibold',
        size === 'sm' ? 'px-1.5 py-0.5 text-[10px]' : 'px-2 py-1 text-xs'
      )}
      style={{
        backgroundColor: theme.bg,
        color: theme.color,
      }}
    >
      {theme.initial}
    </span>
  );
}
