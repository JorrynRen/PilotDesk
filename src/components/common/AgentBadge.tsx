import { memo } from 'react';
import clsx from 'clsx';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { AgentIcon } from './AgentIcon';

interface AgentBadgeProps {
  agentType: string;
  /** sm（消息/工具条，图标 16）/ md（图标 20）/ lg（会话列表，图标 28） */
  size?: 'sm' | 'md' | 'lg';
  isGenerating?: boolean;
}

/**
 * 字母代号的尺寸档：
 * - lg 用固定 28×28 方框，保证与图标路径（AgentIcon 28 + 底板）外框同尺寸，
 *   否则两行内容（标题/预览）会因头像高度不一而在列表里错位。
 */
const LETTER_LG_STYLE = { width: 28, height: 28, borderRadius: 7 } as const;

export const AgentBadge = memo(function AgentBadge({ agentType, size = 'sm', isGenerating }: AgentBadgeProps) {
  const { getTheme } = useAgentRegistry();
  const theme = getTheme(agentType);
  const letterClass = size === 'lg'
    ? 'text-xs'
    : size === 'sm' ? 'px-1.5 py-0.5 text-[10px]' : 'px-2 py-1 text-xs';
  const letterStyle = size === 'lg' ? LETTER_LG_STYLE : undefined;

  // 有图标时：直接渲染图标，替换整个字母代号+主题色背景
  if (theme.icon) {
    return (
      <AgentIcon
        icon={theme.icon}
        size={size === 'sm' ? 16 : size === 'md' ? 20 : 28}
        accentColor={theme.color}
        className={isGenerating ? 'pd-animate-breathe' : ''}
        fallback={
          <span
            className={clsx(
              'inline-flex items-center justify-center rounded font-semibold',
              letterClass,
              isGenerating && 'pd-animate-breathe'
            )}
            style={{
              ...letterStyle,
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
        letterClass,
        isGenerating && 'pd-animate-breathe'
      )}
      style={{
        ...letterStyle,
        backgroundColor: theme.bg,
        color: theme.color,
        transition: 'background-color 0.3s, color 0.3s',
      }}
    >
      {theme.initial}
    </span>
  );
});