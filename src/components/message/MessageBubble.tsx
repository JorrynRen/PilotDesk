import { useState, useEffect, useCallback, useRef } from 'react';
import { Copy, Edit3, Pencil, Bookmark, Check, User } from 'lucide-react';
import { MarkdownRenderer } from './MarkdownRenderer';
import { AGENT_THEMES, MODE_LABELS, MODE_COLORS } from '../../types';
import { useInspirationStore } from '../../stores/inspirationStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { showToast } from '../../utils/toast';
import { useSessionStore } from '../../stores/sessionStore';
import { type Message } from '../../types';
import { isApiSession } from '../../utils/sessionType';

interface MessageBubbleProps {
  message: Message;
  agentType: string;
  apiProviderId?: string;
  apiModel?: string;
  onEdit?: (content: string) => void;
  onSaveInspiration?: (content: string) => void;
  onResend?: (content: string) => void;
}

function formatTimestamp(ts: number): string {
  const date = new Date(ts * 1000);
  const time = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}/${m}/${d} ${time}`;
}



export function MessageBubble({ message, agentType, apiProviderId, apiModel, onEdit, onSaveInspiration, onResend }: MessageBubbleProps) {
  const [copied, setCopied] = useState(false);
  const [isEditing, setIsEditing] = useState(false);
  const [editContent, setEditContent] = useState('');
  const editRef = useRef<HTMLTextAreaElement>(null);
  const { updateMessage } = useSessionStore();
  const isUser = message.role === 'user';

  // Resolve provider name for API sessions (only from user-configured providers list)
  const providers = useApiProviderStore((s) => s.providers);
  const { fetchProviders } = useApiProviderStore();
  const providerName = apiProviderId
    ? providers.find(p => p.id === apiProviderId)?.name
    : undefined;

  // Auto-fetch providers if not loaded yet
  useEffect(() => {
    if (apiProviderId && providers.length === 0) {
      fetchProviders().catch(() => {});
    }
  }, [apiProviderId, providers.length, fetchProviders]);

  // Build agent label: "AgentType | Provider | Model Time"
  const buildAgentLabel = () => {
    const typeLabel = AGENT_THEMES[agentType]?.label || agentType;
    const time = formatTimestamp(message.timestamp);
    const parts = [typeLabel];
    if (isApiSession(agentType)) {
      // API type: provider name comes from user-configured providers list
      if (providerName) parts.push(providerName);
      if (apiModel) parts.push(apiModel);
    } else {
      // Claude/Hermes: no config store anymore, just show type + time
    }
    return parts.join(' | ') + ' ' + time;
  };

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

  const handleSaveInspiration = useCallback(async () => {
    const createInspiration = useInspirationStore.getState().createInspiration;
    const preview = message.content.slice(0, 30).replace(/\n/g, ' ');
    await createInspiration({
      title: preview.length >= 30 ? preview + '...' : preview,
      content: message.content,
      sourceAgent: agentType,
    });
    showToast('已添加到灵感市集', 'success');
  }, [message.content, agentType]);

  const handleStartEdit = useCallback(() => {
    setEditContent(message.content);
    setIsEditing(true);
    setTimeout(() => editRef.current?.focus(), 0);
  }, [message.content]);

  const handleSaveEdit = useCallback(async () => {
    if (!editContent.trim()) return;
    await updateMessage(message.id, editContent);
    setIsEditing(false);
    showToast('消息已更新', 'success');
  }, [message.id, editContent, updateMessage]);

  const handleCancelEdit = useCallback(() => {
    setIsEditing(false);
    setEditContent('');
  }, []);

  const handleResend = useCallback(() => {
    onResend?.(message.content);
  }, [message.content, onResend]);

  const handleEditKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSaveEdit();
    }
    if (e.key === 'Escape') {
      handleCancelEdit();
    }
  }, [handleSaveEdit, handleCancelEdit]);

  const messageMode = message.mode as keyof typeof MODE_LABELS;
  const modeColor = MODE_COLORS[messageMode];
  const modeLabel = MODE_LABELS[messageMode];



  // Build agent label: "AgentType | Provider | Model Time"

  if (isUser) {
    return (
      <div className="group flex gap-2.5 px-4 py-[3px]">
        {/* User avatar */}
        <div className="shrink-0 pt-0.5">
          <div
            className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-semibold"
            style={{
              backgroundColor: 'rgba(59, 130, 246, 0.15)',
              color: '#3B82F6',
            }}
          >
            <User size={14} />
          </div>
        </div>

        {/* Content */}
        <div className="flex-1 min-w-0">
          {/* User label + time */}
          <div className="flex items-center gap-2 mb-1">
            <span className="text-[11px] " style={{ color: 'var(--text-primary)' }}>
              User
            </span>
            <span className="text-[11px] " style={{ color: 'var(--text-tertiary)' }}>
              {formatTimestamp(message.timestamp)}
            </span>
          </div>

          {/* Card with accent background */}
          <div
            className="rounded-xl px-3.5 py-2.5"
            style={{
              backgroundColor: 'var(--accent)',
              color: '#fff',
            }}
          >
            {isEditing ? (
            <textarea
              ref={editRef}
              value={editContent}
              onChange={(e) => setEditContent(e.target.value)}
              onKeyDown={handleEditKeyDown}
              className="w-full text-[13px] leading-relaxed outline-none resize-none"
              style={{
                color: '#fff',
                backgroundColor: 'transparent',
                minHeight: '60px',
              }}
            />
          ) : (
            <p className="text-[13px] leading-relaxed whitespace-pre-wrap m-0">
              {message.content}
            </p>
          )}
          </div>

          {/* Actions */}
          <div className="flex items-center gap-1 mt-1 opacity-0 group-hover:opacity-100 transition-opacity">
            <button
              onClick={handleCopy}
              className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95"
              style={{ color: 'var(--text-secondary)' }}
              title="复制"
            >
              {copied ? <Check size={11} /> : <Copy size={11} />}
              {copied ? '已复制' : '复制'}
            </button>
            {isEditing ? (
              <>
                <button
                  onClick={handleSaveEdit}
                  className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95"
                  style={{ color: '#22c55e' }}
                  title="保存"
                >
                  <Check size={11} />
                  保存
                </button>
                <button
                  onClick={handleCancelEdit}
                  className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95"
                  style={{ color: 'var(--text-secondary)' }}
                  title="取消"
                >
                  <Edit3 size={11} />
                  取消
                </button>
              </>
            ) : (
              <>
                {onEdit && (
                  <button
                    onClick={handleStartEdit}
                    className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95"
                    style={{ color: 'var(--text-secondary)' }}
                    title="编辑"
                  >
                    <Pencil size={11} />
                    编辑
                  </button>
                )}
              </>
            )}
            <button
              onClick={handleSaveInspiration}
              className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95"
              style={{ color: 'var(--text-secondary)' }}
              title="收藏灵感"
            >
              <Bookmark size={11} />
              收藏灵感
            </button>
            {onResend && (
              <button
                onClick={handleResend}
                className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95"
                style={{ color: 'var(--accent)' }}
                title="重发"
              >
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
                重发
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  // Assistant message
  const agentTheme = AGENT_THEMES[agentType] || AGENT_THEMES.claude;
  const agentColor = agentTheme.color;
  const agentInitial = agentTheme.initial;

  return (
    <div className="group flex gap-2.5 px-4 py-[3px]">
      {/* Avatar */}
      <div className="shrink-0 pt-0.5">
        <div
          className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-semibold"
          style={{
            backgroundColor: `${agentColor}20`,
            color: agentColor,
          }}
        >
          {agentInitial}
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 min-w-0">
        {/* Agent label */}
        <div className="flex items-center gap-2 mb-1">
          <span className="text-[11px] " style={{ color: 'var(--text-primary)' }}>
            {buildAgentLabel()}
          </span>
          {message.mode !== 'native' && (
            <span
              className="text-[10px] px-1.5 py-0.5 rounded-full"
              style={{ backgroundColor: `${modeColor}22`, color: modeColor }}
            >
              {modeLabel}
            </span>
          )}
        </div>

        {/* Message body in card */}
        <div
          className="rounded-xl px-3.5 py-2.5"
          style={{
            backgroundColor: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
          }}
        >
          <MarkdownRenderer content={message.content} />
        </div>

        {/* Action buttons */}
        <div className="flex items-center gap-1 mt-1 opacity-0 group-hover:opacity-100 transition-opacity">
          <button
            onClick={handleCopy}
            className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95"
            style={{ color: 'var(--text-secondary)' }}
            title="复制"
          >
            {copied ? <Check size={11} /> : <Copy size={11} />}
            {copied ? '已复制' : '复制'}
          </button>
          <button
            onClick={handleSaveInspiration}
            className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-all hover:bg-gray-200/60 dark:hover:bg-white/10 active:scale-95"
            style={{ color: 'var(--text-secondary)' }}
            title="收藏灵感"
          >
            <Bookmark size={11} />
            收藏灵感
          </button>
        </div>
      </div>
    </div>
  );
}
