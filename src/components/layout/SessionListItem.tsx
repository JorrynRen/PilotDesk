import clsx from 'clsx';
import { MessageSquare, Archive, Trash2 } from 'lucide-react';
import { AgentBadge } from '../common/AgentBadge';
import type { Session } from '../../types';

interface SessionListItemProps {
  session: Session;
  isActive: boolean;
  onSelect: () => void;
  onArchive?: () => void;
  onDelete?: () => void;
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

export function SessionListItem({
  session,
  isActive,
  onSelect,
  onArchive,
  onDelete,
}: SessionListItemProps) {
  return (
    <div
      className={clsx(
        'group px-3 py-2 cursor-pointer transition-colors',
        isActive && 'border-l-2'
      )}
      style={{
        backgroundColor: isActive ? 'var(--border)' : 'transparent',
        borderLeftColor: isActive ? 'var(--accent)' : 'transparent',
      }}
      onClick={onSelect}
    >
      <div className="flex items-start gap-2">
        <AgentBadge agentType={session.agentType as 'claude' | 'hermes' | 'api'} />
        <div className="flex-1 min-w-0">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium truncate">{session.title}</span>
            <span className="text-[10px] shrink-0 ml-1" style={{ color: 'var(--text-secondary)' }}>
              {formatTime(session.updatedAt)}
            </span>
          </div>
          {session.lastMessagePreview && (
            <p className="text-[10px] mt-0.5 truncate" style={{ color: 'var(--text-secondary)' }}>
              {session.lastMessagePreview}
            </p>
          )}
          <div className="flex items-center gap-2 mt-0.5 text-[10px]" style={{ color: 'var(--text-secondary)' }}>
            <span className="flex items-center gap-0.5">
              <MessageSquare size={9} />
              {session.messageCount}
            </span>
          </div>
        </div>
        {(onArchive || onDelete) && (
          <div className="hidden group-hover:flex items-center gap-0.5 shrink-0">
            {onArchive && (
              <button
                onClick={(e) => { e.stopPropagation(); onArchive(); }}
                className="p-0.5 rounded hover:opacity-80"
                style={{ color: 'var(--text-secondary)' }}
                title="归档"
              >
                <Archive size={11} />
              </button>
            )}
            {onDelete && (
              <button
                onClick={(e) => { e.stopPropagation(); onDelete(); }}
                className="p-0.5 rounded hover:opacity-80"
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
}
