import { useState } from 'react';
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

export function MessageBubble({ message, agentType, onEdit, onSaveInspiration }: MessageBubbleProps) {
  const [copied, setCopied] = useState(false);
  const isUser = message.role === 'user';

  const handleCopy = async () => {
    await navigator.clipboard.writeText(message.content);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const modeColor = MODE_COLORS[message.mode as keyof typeof MODE_COLORS];
  const modeLabel = MODE_LABELS[message.mode as keyof typeof MODE_LABELS];

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
        {/* Header */}
        <div className="flex items-center gap-2 mb-1">
          <span className="text-xs font-medium">
            {isUser ? '你' : agentType === 'claude' ? 'Claude Code' : 'Hermes Agent'}
          </span>
          {!isUser && message.mode !== 'native' && (
            <span
              className="text-[10px] px-1.5 py-0.5 rounded"
              style={{ color: modeColor, backgroundColor: 'rgba(128,128,128,0.1)' }}
            >
              {modeLabel}
            </span>
          )}
        </div>

        {/* Content */}
        <div className="text-sm leading-relaxed">
          {isUser ? (
            <p style={{ whiteSpace: 'pre-wrap' }}>{message.content}</p>
          ) : (
            <MarkdownRenderer content={message.content} />
          )}
        </div>

        {/* Action buttons */}
        <div
          className="flex items-center gap-1 mt-1 opacity-0 group-hover:opacity-100 transition-opacity"
        >
          {isUser ? (
            <>
              <button
                onClick={() => onEdit?.(message.content)}
                className="p-1 rounded hover:opacity-80 transition-colors"
                style={{ color: 'var(--text-secondary)' }}
                title="编辑"
              >
                <Edit3 size={12} />
              </button>
              <button
                onClick={() => onSaveInspiration?.(message.content)}
                className="p-1 rounded hover:opacity-80 transition-colors"
                style={{ color: 'var(--text-secondary)' }}
                title="保存到灵感"
              >
                <Bookmark size={12} />
              </button>
            </>
          ) : (
            <>
              <button
                onClick={handleCopy}
                className="p-1 rounded hover:opacity-80 transition-colors flex items-center gap-1"
                style={{ color: 'var(--text-secondary)' }}
                title="复制"
              >
                {copied ? <Check size={12} /> : <Copy size={12} />}
                {copied && <span className="text-[10px]">已复制</span>}
              </button>
              <button
                onClick={() => onSaveInspiration?.(message.content)}
                className="p-1 rounded hover:opacity-80 transition-colors"
                style={{ color: 'var(--text-secondary)' }}
                title="保存到灵感"
              >
                <Bookmark size={12} />
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
