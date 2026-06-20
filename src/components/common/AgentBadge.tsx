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

  // 有图标时：直接渲染图标，替换整个字母代号+主题色背景
  if (theme.icon) {
    return (
      <AgentIcon
        icon={theme.icon}
        size={size === 'sm' ? 16 : 20}
        className={isGenerating ? 'pd-animate-breathe' : ''}
        fallback={
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
        }
      />
    );
  }

  // 无图标时：显示字母代号+主题色背景
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
