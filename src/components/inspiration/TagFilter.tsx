import { X } from 'lucide-react';
import { useInspirationStore } from '../../stores/inspirationStore';

export function TagFilter() {
  const { tags, activeTag, setActiveTag } = useInspirationStore();

  if (tags.length === 0 && !activeTag) return null;

  return (
    <div className="flex items-center gap-1.5 flex-wrap">
      {activeTag && (
        <button
          onClick={() => setActiveTag(null)}
          className="flex items-center gap-1 px-2 py-0.5 rounded-full text-xs"
          style={{
            backgroundColor: 'var(--accent)',
            color: '#fff',
          }}
        >
          {activeTag}
          <X size={10} />
        </button>
      )}
      {tags
        .filter((t) => t !== activeTag)
        .map((tag) => (
          <button
            key={tag}
            onClick={() => setActiveTag(tag)}
            className="px-2 py-0.5 rounded-full text-xs transition-colors"
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
