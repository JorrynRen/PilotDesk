import { useState, useCallback } from 'react';
import { Copy, Edit3, Bookmark, Check } from 'lucide-react';
import { MarkdownRenderer } from './MarkdownRenderer';
import { MODE_LABELS, MODE_COLORS } from '../../types';
import { useInspirationStore } from '../../stores/inspirationStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { showToast } from '../../utils/toast';
import type { Message } from '../../types';

interface MessageBubbleProps {
  message: Message;
  agentType: 'claude' | 'hermes' | 'api';
  apiProviderId?: string;
  apiModel?: string;
  onEdit?: (content: string) => void;
  onSaveInspiration?: (content: string) => void;
}

function formatTimestamp(ts: number): string {
  const date = new Date(ts * 1000);
  return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

const AGENT_TYPE_LABELS: Record<string, string> = {
  claude: 'Claude Code',
  hermes: 'Hermes Agent',
  api: 'API',
};

const AGENT_TYPE_COLORS: Record<string, string> = {
  claude: '#3B82F6',
  hermes: '#8B5CF6',
  api: '#10B981',
};

export function MessageBubble({ message, agentType, apiProviderId, apiModel, onEdit, onSaveInspiration }: MessageBubbleProps) {
  const [copied, setCopied] = useState(false);
  const isUser = message.role === 'user';

  // Resolve provider name for API sessions
  const providers = useApiProviderStore((s) => s.providers);
  const providerName = apiProviderId ? providers.find(p => p.id === apiProviderId)?.name : undefined;

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
    const result = await createInspiration({
      title: preview.length >= 30 ? preview + '...' : preview,
      content: message.content,
      sourceAgent: agentType,
    });
    if (result) {
      showToast('已添加到灵感市集', 'success');
    }
  }, [message.content, agentType]);

  const messageMode = message.mode as keyof typeof MODE_LABELS;
  const modeColor = MODE_COLORS[messageMode];
  const modeLabel = MODE_LABELS[messageMode];

  // Build agent label: "AgentType | Provider | Model HH:MM"
  const buildAgentLabel = () => {
    const typeLabel = AGENT_TYPE_LABELS[agentType] || agentType;
    const time = formatTimestamp(message.timestamp);
    const parts = [typeLabel];
    if (agentType === 'api') {
      if (providerName) parts.push(providerName);
      if (apiModel) parts.push(apiModel);
    }
    return parts.join(' | ') + ' ' + time;
  };

  if (isUser) {
    return (
      <div className="group flex justify-end gap-2.5 px-4 py-[3px]">
        <div style={{ maxWidth: '75%' }}>
          {/* User label + time */}
          <div className="flex items-center gap-2 mb-1 justify-end">
            <span className="text-[11px] font-medium" style={{ color: 'var(--text-tertiary)' }}>
              {formatTimestamp(message.timestamp)}
            </span>
            <span className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>
              User
            </span>
          </div>
          {/* Bubble */}
          <div
            className="rounded-2xl px-3.5 py-2.5"
            style={{
              color: '#fff',
              backgroundColor: 'var(--accent)',
              borderRadius: '16px 4px 16px 16px',
            }}
          >
            <p className="text-[13px] leading-relaxed whitespace-pre-wrap m-0">
              {message.content}
            </p>
          </div>
          {/* Actions */}
          <div className="flex items-center gap-1 mt-1 justify-end opacity-0 group-hover:opacity-100 transition-opacity">
            <button
              onClick={handleCopy}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
              style={{ color: 'var(--text-secondary)' }}
              title="复制"
            >
              {copied ? <Check size={11} /> : <Copy size={11} />}
              {copied ? '已复制' : '复制'}
            </button>
            {onEdit && (
              <button
                onClick={() => onEdit(message.content)}
                className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
                style={{ color: 'var(--text-secondary)' }}
                title="编辑"
              >
                <Edit3 size={11} />
                编辑
              </button>
            )}
            <button
              onClick={handleSaveInspiration}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
              style={{ color: 'var(--text-secondary)' }}
              title="收藏灵感"
            >
              <Bookmark size={11} />
              收藏灵感
            </button>
          </div>
        </div>
        {/* User avatar */}
        <div className="shrink-0 pt-4">
          <div
            className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-bold"
            style={{
              backgroundColor: 'rgba(59, 130, 246, 0.15)',
              color: '#3B82F6',
            }}
          >
            U
          </div>
        </div>
      </div>
    );
  }

  // Assistant message
  const agentColor = AGENT_TYPE_COLORS[agentType] || '#3B82F6';
  const agentInitial = agentType === 'claude' ? 'C' : agentType === 'api' ? 'A' : 'H';

  return (
    <div className="group flex gap-2.5 px-4 py-[3px]">
      {/* Avatar */}
      <div className="shrink-0 pt-0.5">
        <div
          className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-bold"
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
          <span className="text-[11px] font-medium" style={{ color: 'var(--text-primary)' }}>
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
            className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
            style={{ color: 'var(--text-secondary)' }}
            title="复制"
          >
            {copied ? <Check size={11} /> : <Copy size={11} />}
            {copied ? '已复制' : '复制'}
          </button>
          <button
            onClick={handleSaveInspiration}
            className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors"
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
