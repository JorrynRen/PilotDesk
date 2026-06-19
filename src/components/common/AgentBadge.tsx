import clsx from 'clsx';
import { AGENT_THEMES } from '../../types';

interface AgentBadgeProps {
  agentType: string;
  size?: 'sm' | 'md';
  isGenerating?: boolean;
}

export function AgentBadge({ agentType, size = 'sm', isGenerating }: AgentBadgeProps) {
  const theme = AGENT_THEMES[agentType] || AGENT_THEMES.claude;
  return (
    <span
      className={clsx(
        'inline-flex items-center justify-center rounded font-semibold',
        size === 'sm' ? 'px-1.5 py-0.5 text-[10px]' : 'px-2 py-1 text-xs',
        isGenerating && 'pd-animate-breathe'
      )}
      style={{
        backgroundColor: theme.bg,
        color: theme.color,
        transition: 'background-color 0.3s, color 0.3s',
      }}
    >
      {theme.initial}
    </span>
  );
}
