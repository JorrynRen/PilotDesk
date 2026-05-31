import { Star, Send, Trash2 } from 'lucide-react';
import type { InspirationItem } from '../../stores/inspirationStore';

interface InspirationCardProps {
  inspiration: InspirationItem;
  onToggleFavorite: (id: string) => void;
  onSendToSession: (content: string) => void;
  onDelete: (id: string) => void;
  onEdit: (inspiration: InspirationItem) => void;
}

export function InspirationCard({ inspiration, onToggleFavorite, onSendToSession, onDelete, onEdit }: InspirationCardProps) {
  const sourceLabel = {
    claude: 'Claude',
    hermes: 'Hermes',
    manual: '手动',
  }[inspiration.source_agent] ?? inspiration.source_agent;

  const sourceColor = {
    claude: 'var(--claude-tag)',
    hermes: 'var(--hermes-tag)',
    manual: 'var(--text-tertiary)',
  }[inspiration.source_agent] ?? 'var(--text-tertiary)';

  const timeStr = new Date(inspiration.updated_at * 1000).toLocaleDateString('zh-CN', {
    month: 'short',
    day: 'numeric',
  });

  return (
    <div
      className="rounded-xl p-4 cursor-pointer transition-all hover:scale-[1.01]"
      style={{
        backgroundColor: 'var(--bg-secondary)',
        border: '1px solid var(--border)',
      }}
      onClick={() => onEdit(inspiration)}
    >
      {/* Header */}
      <div className="flex items-start justify-between mb-2">
        <div className="flex items-center gap-2">
          <span className="text-xl">{inspiration.icon}</span>
          <h4 className="text-sm font-medium leading-tight" style={{ color: 'var(--text-primary)' }}>
            {inspiration.title}
          </h4>
        </div>
        <div className="flex items-center gap-1 shrink-0">
          <button
            onClick={(e) => {
              e.stopPropagation();
              onToggleFavorite(inspiration.id);
            }}
            className="p-1 rounded transition-colors"
            style={{ color: inspiration.is_favorite ? '#FBBF24' : 'var(--text-tertiary)' }}
          >
            <Star size={14} fill={inspiration.is_favorite ? '#FBBF24' : 'none'} />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              onDelete(inspiration.id);
            }}
            className="p-1 rounded transition-colors"
            style={{ color: 'var(--text-tertiary)' }}
          >
            <Trash2 size={14} />
          </button>
        </div>
      </div>

      {/* Content preview */}
      <p
        className="text-xs leading-relaxed mb-3 line-clamp-3"
        style={{ color: 'var(--text-secondary)' }}
      >
        {inspiration.content.length > 150
          ? inspiration.content.slice(0, 150) + '...'
          : inspiration.content}
      </p>

      {/* Tags */}
      {inspiration.tags.length > 0 && (
        <div className="flex flex-wrap gap-1 mb-3">
          {inspiration.tags.map((tag) => (
            <span
              key={tag}
              className="px-1.5 py-0.5 rounded text-[10px]"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
            >
              #{tag}
            </span>
          ))}
        </div>
      )}

      {/* Footer */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span
            className="text-[10px] px-1.5 py-0.5 rounded"
            style={{ backgroundColor: sourceColor + '22', color: sourceColor }}
          >
            {sourceLabel}
          </span>
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            {timeStr}
          </span>
        </div>
        <button
          onClick={(e) => {
            e.stopPropagation();
            onSendToSession(inspiration.content);
          }}
          className="flex items-center gap-1 px-2 py-0.5 rounded text-[10px] transition-colors"
          style={{
            backgroundColor: 'var(--accent)',
            color: '#fff',
          }}
        >
          <Send size={10} />
          发送到会话
        </button>
      </div>
    </div>
  );
}
