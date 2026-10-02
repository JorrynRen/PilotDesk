import { X } from 'lucide-react';

/**
 * TagFilterBar — 灵感的标签**筛选条**（受控组件）。
 *
 * 与 TagChips 的区别：这个是能点的 —— 选中的标签高亮并可再点一次取消。
 * 做成受控（tags / activeTag / onSelect 都从外部传入），是为了让两处各自
 * 持有自己的筛选状态：独立页用 store 的 activeTag（它走后端按 tag 过滤），
 * 会话侧栏用组件本地 state（只在侧栏内过滤，不去干扰独立页）。
 * 之前 TagFilter 把 store 写死在组件里，侧栏就复用不了。
 */
interface TagFilterBarProps {
  /** 可选标签全集（通常来自 store.tags） */
  tags: string[];
  activeTag: string | null;
  onSelect: (tag: string | null) => void;
  /** 紧凑档：不换行、横向滚动、更小尺寸（会话侧栏 ~300px 用） */
  compact?: boolean;
}

export function TagFilterBar({ tags, activeTag, onSelect, compact }: TagFilterBarProps) {
  // 没有任何标签、也没选中项时整条不渲染，避免占一行空白
  if (tags.length === 0 && !activeTag) return null;

  const chipBase = compact ? 'text-[10px] px-1.5 py-0.5' : 'text-xs px-2 py-0.5';

  return (
    <div
      className={
        compact
          ? 'flex items-center gap-1 overflow-x-auto pd-scroll-none'
          : 'flex items-center gap-1.5 flex-wrap'
      }
    >
      {activeTag ? (
        <button
          onClick={() => onSelect(null)}
          title="取消标签筛选"
          className={`flex items-center gap-1 rounded-full shrink-0 ${chipBase}`}
          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
        >
          #{activeTag}
          <X size={10} />
        </button>
      ) : (
        // 未选中时给一个显式的"全部"，否则用户看不出这一排是筛选器
        compact && (
          <span
            className={`rounded-full shrink-0 ${chipBase}`}
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
          >
            全部
          </span>
        )
      )}
      {tags
        .filter((t) => t !== activeTag)
        .map((tag) => (
          <button
            key={tag}
            onClick={() => onSelect(tag)}
            title={`按标签筛选：${tag}`}
            className={`rounded-full shrink-0 transition-colors ${chipBase}`}
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
              border: '1px solid var(--border)',
            }}
          >
            #{tag}
          </button>
        ))}
    </div>
  );
}
