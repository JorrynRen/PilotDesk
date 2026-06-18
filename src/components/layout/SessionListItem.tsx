import { useState, memo } from 'react';
import { Archive, Trash2, Pencil, Check, X } from 'lucide-react';
import { AGENT_THEMES } from '../../types';
import { AgentBadge } from '../common/AgentBadge';
import { useGeneratingStore } from '../../stores/generatingStore';
import type { Session } from '../../types';

interface SessionListItemProps {
  session: Session;
  isActive: boolean;
  onSelect: () => void;
  onArchive?: () => void;
  onRename?: (id: string, newTitle: string) => void;
  onDelete?: () => void;
  batchMode?: boolean;
  selected?: boolean;
  onToggleSelect?: (id: string) => void;
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();

  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  const isYesterday = date.toDateString() === yesterday.toDateString();

  const timeStr = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });

  if (isToday) return `今天 ${timeStr}`;
  if (isYesterday) return `昨天 ${timeStr}`;
  return date.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
}

export const SessionListItem = memo(function SessionListItem({
  session,
  isActive,
  onSelect,
  onArchive,
  onRename,
  onDelete,
  batchMode,
  selected,
  onToggleSelect,
}: SessionListItemProps) {
  const [isEditing, setIsEditing] = useState(false);
  const isGenerating = useGeneratingStore((s) => !!s.generatingMap[session.id]);
  const [editTitle, setEditTitle] = useState(session.title);

  // Fallback display title when title is empty
  const displayTitle = session.title || (() => {
    const theme = AGENT_THEMES[session.agentType as keyof typeof AGENT_THEMES] || AGENT_THEMES.claude;
    return `${theme.label} 新会话`;
  })();

  const handleStartEdit = (e: React.MouseEvent) => {
    e.stopPropagation();
    setEditTitle(session.title);
    setIsEditing(true);
  };

  const handleConfirmEdit = (e: React.MouseEvent) => {
    e.stopPropagation();
    const trimmed = editTitle.trim();
    if (trimmed && trimmed !== session.title && onRename) {
      onRename(session.id, trimmed);
    }
    setIsEditing(false);
  };

  const handleCancelEdit = (e: React.MouseEvent) => {
    e.stopPropagation();
    setIsEditing(false);
    setEditTitle(session.title);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.stopPropagation();
      handleConfirmEdit(e as unknown as React.MouseEvent);
    } else if (e.key === 'Escape') {
      e.stopPropagation();
      handleCancelEdit(e as unknown as React.MouseEvent);
    }
  };

  return (
    <div
      className="group px-3 py-2 cursor-pointer transition-colors"
      style={{
        backgroundColor: selected ? 'var(--accent-light)' : isActive ? 'var(--border)' : 'transparent',
        borderBottom: '1px solid var(--border)',
      }}
      onClick={isEditing ? undefined : batchMode ? () => onToggleSelect?.(session.id) : onSelect}
    >
      <div className="flex items-start gap-2">
        {batchMode && (
          <div className="shrink-0 pt-1" onClick={(e) => { e.stopPropagation(); onToggleSelect?.(session.id); }}>
            <div
              className="w-4 h-4 rounded border flex items-center justify-center transition-colors cursor-pointer"
              style={{
                borderColor: selected ? 'var(--accent)' : 'var(--border)',
                backgroundColor: selected ? 'var(--accent)' : 'transparent',
              }}
            >
              {selected && <Check size={10} color="#fff" />}
            </div>
          </div>
        )}
        <div className="shrink-0 flex flex-col items-center gap-1">
          <AgentBadge agentType={session.agentType as 'claude' | 'hermes' | 'api'} isGenerating={isGenerating} />
          {!isEditing && (
            <span className="text-[9px] leading-none" style={{ color: 'var(--text-tertiary)' }}>
              {session.messageCount}
            </span>
          )}
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center justify-between">
            {isEditing ? (
              <div className="flex items-center gap-1 flex-1 mr-1" onClick={(e) => e.stopPropagation()}>
                <input
                  type="text"
                  value={editTitle}
                  onChange={(e) => setEditTitle(e.target.value)}
                  onKeyDown={handleKeyDown}
                  className="flex-1 px-1.5 py-0.5 rounded text-xs"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--accent)',
                    outline: 'none',
                    boxShadow: 'none',
                  }}
                  autoFocus
                  onClick={(e) => e.stopPropagation()}
                />
                <button
                  onClick={handleConfirmEdit}
                  className="pd-btn p-0.5 rounded"
                  style={{ color: AGENT_THEMES.api.color }}
                  title="确认"
                >
                  <Check size={11} />
                </button>
                <button
                  onClick={handleCancelEdit}
                  className="pd-btn p-0.5 rounded"
                  style={{ color: 'var(--text-secondary)' }}
                  title="取消"
                >
                  <X size={11} />
                </button>
              </div>
            ) : (
              <>
                <span className="text-xs  truncate">{displayTitle}</span>
                <span className="text-[10px] shrink-0 ml-1" style={{ color: 'var(--text-secondary)' }}>
                  {formatTime(session.updatedAt)}
                </span>
              </>
            )}
          </div>
          {!isEditing && (
            <>
              {session.lastMessagePreview && (
                <p className="text-[10px] mt-0.5 truncate" style={{ color: 'var(--text-secondary)' }}>
                  {session.lastMessagePreview}
                </p>
              )}

            </>
          )}
        </div>
        {(onArchive || onRename || onDelete) && !isEditing && (
          <div className="hidden group-hover:flex items-center gap-0.5 shrink-0">
            {onRename && (
              <button
                onClick={handleStartEdit}
                className="pd-btn p-0.5 rounded transition-all hover:bg-black/10 dark:hover:bg-white/10 active:scale-90"
                style={{ color: 'var(--accent)' }}
                title="重命名"
              >
                <Pencil size={11} />
              </button>
            )}
            {onArchive && (
              <button
                onClick={(e) => { e.stopPropagation(); onArchive(); }}
                className="p-0.5 rounded transition-all hover:bg-black/10 dark:hover:bg-white/10 active:scale-90"
                style={{ color: 'var(--text-secondary)' }}
                title="归档"
              >
                <Archive size={11} />
              </button>
            )}
            {onDelete && (
              <button
                onClick={(e) => { e.stopPropagation(); onDelete(); }}
                className="p-0.5 rounded transition-all hover:bg-red-500/20 active:scale-90"
                style={{ color: '#EF4444' }}
                title="删除"
              >
                <Trash2 size={11} />
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
});
