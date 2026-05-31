import { useState, useCallback } from 'react';
import { Copy, Edit3, Bookmark, Check } from 'lucide-react';
import { MarkdownRenderer } from './MarkdownRenderer';
import { MODE_LABELS, MODE_COLORS } from '../../types';
import type { Message } from '../../types';

interface MessageBubbleProps {
  message: Message;
  agentType: 'claude' | 'hermes';
  onEdit?: (content: string) => void;
  onSaveInspiration?: (content: string) => void;
}

function formatTimestamp(ts: number): string {
  const date = new Date(ts * 1000);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();

  const timeStr = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });

  if (isToday) return timeStr;
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) return `昨天 ${timeStr}`;
  return date.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' }) + ' ' + timeStr;
}

export function MessageBubble({ message, agentType, onEdit, onSaveInspiration }: MessageBubbleProps) {
  const [copied, setCopied] = useState(false);
  const isUser = message.role === 'user';

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(message.content);
    } catch {
      const textarea = document.createElement('textarea');
      textarea.value = message.content;
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      document.body.removeChild(textarea);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [message.content]);

  const messageMode = message.mode as keyof typeof MODE_LABELS;
  const modeColor = MODE_COLORS[messageMode];
  const modeLabel = MODE_LABELS[messageMode];

  return (
    <div
      className="group flex gap-3 px-4 py-3"
      style={{
        backgroundColor: isUser ? 'transparent' : 'var(--bg-secondary)',
      }}
    >
      {/* Agent indicator */}
      <div className="shrink-0 pt-0.5">
        {isUser ? (
          <div
            className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-medium"
            style={{ backgroundColor: 'var(--border)', color: 'var(--text-secondary)' }}
          >
            U
          </div>
        ) : (
          <div
            className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-bold"
            style={{
              backgroundColor: agentType === 'claude'
                ? 'rgba(59,130,246,0.15)'
                : 'rgba(139,92,246,0.15)',
              color: agentType === 'claude' ? '#3B82F6' : '#8B5CF6',
            }}
          >
            {agentType === 'claude' ? 'C' : 'H'}
          </div>
        )}
      </div>

      {/* Message content */}
      <div className="flex-1 min-w-0">
        {/* Header with timestamp */}
        <div className="flex items-center gap-2 mb-1">
          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
            {isUser ? '你' : agentType === 'claude' ? 'Claude Code' : 'Hermes Agent'}
          </span>
          {message.mode !== 'native' && (
            <span
              className="text-[10px] px-1.5 py-0.5 rounded-full"
              style={{ backgroundColor: `${modeColor}22`, color: modeColor }}
            >
              {modeLabel}
            </span>
          )}
          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            {formatTimestamp(message.timestamp)}
          </span>
        </div>

        {/* Message body */}
        {isUser ? (
          <div className="text-sm leading-relaxed whitespace-pre-wrap" style={{ color: 'var(--text-primary)' }}>
            {message.content}
          </div>
        ) : (
          <MarkdownRenderer content={message.content} />
        )}

        {/* Action buttons */}
        <div className="flex items-center gap-1 mt-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
          <button
            onClick={handleCopy}
            className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
            style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)' }}
            title="复制"
          >
            {copied ? <Check size={11} /> : <Copy size={11} />}
            {copied ? '已复制' : '复制'}
          </button>
          {isUser && onEdit && (
            <button
              onClick={() => onEdit(message.content)}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
              style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)' }}
              title="编辑"
            >
              <Edit3 size={11} />
              编辑
            </button>
          )}
          {!isUser && onSaveInspiration && (
            <button
              onClick={() => onSaveInspiration(message.content)}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
              style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)' }}
              title="保存到灵感"
            >
              <Bookmark size={11} />
              收藏灵感
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
