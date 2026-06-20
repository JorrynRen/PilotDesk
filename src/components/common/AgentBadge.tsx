import clsx from 'clsx';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { AgentIcon } from './AgentIcon';

interface AgentBadgeProps {
  agentType: string;
  size?: 'sm' | 'md';
  isGenerating?: boolean;
}

export function AgentBadge({ agentType, size = 'sm', isGenerating }: AgentBadgeProps) {
  const { getTheme } = useAgentRegistry();
  const theme = getTheme(agentType);
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
      {theme.icon ? <AgentIcon icon={theme.icon} size={size === 'sm' ? 10 : 12} fallback={theme.initial} /> : theme.initial}
    </span>
  );
}
