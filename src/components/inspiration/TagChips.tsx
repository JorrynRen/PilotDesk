import type { CSSProperties } from 'react';

/**
 * TagChips — 灵感的标签**只读展示**（`#tag` 小药丸）。
 *
 * 灵感页、会话侧栏、灵感市场三处共用同一份实现：
 * 之前独立页自己渲染一遍、侧栏完全不渲染，"标签能力"就是这么漂开的。
 * 这里只负责"看得见"，编辑归 TagEditor、筛选归 TagFilterBar。
 */
interface TagChipsProps {
  tags: string[];
  /** 紧凑档：字号与内边距更小，用于会话侧栏这类窄容器 */
  compact?: boolean;
  /** 最多显示几个，其余折叠成 `+N`（窄容器里避免撑爆） */
  max?: number;
  className?: string;
}

export function TagChips({ tags, compact, max, className = '' }: TagChipsProps) {
  if (!tags || tags.length === 0) return null;

  const shown = max && tags.length > max ? tags.slice(0, max) : tags;
  const rest = tags.length - shown.length;

  const chipStyle: CSSProperties = compact
    ? { backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', fontSize: 9, padding: '0 4px', borderRadius: 4 }
    : { backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)', fontSize: 10, padding: '2px 6px', borderRadius: 4 };

  return (
    <div className={`flex flex-wrap items-center gap-1 ${className}`}>
      {shown.map((tag) => (
        <span key={tag} style={chipStyle}>#{tag}</span>
      ))}
      {rest > 0 && <span style={chipStyle}>+{rest}</span>}
    </div>
  );
}
