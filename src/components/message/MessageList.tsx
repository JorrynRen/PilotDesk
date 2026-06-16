import { useRef, useEffect, useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso';
import { MessageBubble } from './MessageBubble';
import { AGENT_THEMES } from '../../types';
import type { Message, Session } from '../../types';

interface MessageListProps {
  messages: Message[];
  session: Session | null;
  isGenerating?: boolean;
  streamingStatus?: string;
  onEditMessage?: (content: string) => void;
  onSaveInspiration?: (content: string) => void;
  onResendMessage?: (content: string) => void;
}

export function MessageList({ messages, session, isGenerating, streamingStatus, onEditMessage, onSaveInspiration, onResendMessage }: MessageListProps) {
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  const prevLengthRef = useRef(messages.length);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<Message[] | null>(null);
  const [isSearchingMessages, setIsSearchingMessages] = useState(false);

  const handleMessageSearch = useCallback(async (query: string) => {
    setSearchQuery(query);
    if (!query.trim()) {
      setSearchResults(null);
      return;
    }
    setIsSearchingMessages(true);
    try {
      const results = await invoke<Message[]>('search_messages', {
        sessionId: session?.id ?? null,
        query: query.trim(),
        limit: 50,
      });
      setSearchResults(results);
    } catch { /* ignore */ }
    setIsSearchingMessages(false);
  }, [session?.id]);

  // Auto-scroll to bottom when new messages arrive
  useEffect(() => {
    if (messages.length > prevLengthRef.current) {
      virtuosoRef.current?.scrollToIndex({ index: messages.length - 1, behavior: 'smooth' });
    }
    prevLengthRef.current = messages.length;
  }, [messages.length]);

  const itemContent = useCallback((index: number) => {
    const msg = messages[index];
    if (!msg) return null;
    return (
      <MessageBubble
        message={msg}
        agentType={session?.agentType ?? 'claude'}
        apiProviderId={session?.apiProvider}
        apiModel={session?.apiModel}
        onEdit={onEditMessage}
        onSaveInspiration={onSaveInspiration}
        onResend={onResendMessage}
      />
    );
  }, [messages, session, onEditMessage, onSaveInspiration]);

  const showTypingIndicator = isGenerating && messages.length > 0 && messages[messages.length - 1].role === 'user';

  if (!session) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center px-8 max-w-xs">
          <img
            src="/logo-lg.png"
            alt="PilotDesk"
            className="w-16 h-16 mx-auto mb-5 rounded-2xl opacity-90"
            draggable={false}
          />
          <h2 className="text-base font-semibold mb-2" style={{ color: 'var(--text-primary)' }}>
            PilotDesk
          </h2>
          <p className="text-xs leading-relaxed mb-4" style={{ color: 'var(--text-secondary)' }}>
            Agent 统一桌面客户端
          </p>
          <div
            className="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg text-[11px]"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
            从左侧创建或选择会话开始对话
          </div>
        </div>
      </div>
    );
  }

  if (messages.length === 0) {
    const agentTheme = AGENT_THEMES[session.agentType] || AGENT_THEMES.claude;
    const agentLabel = agentTheme.label;
    const modelInfo = session.agentType === 'api' && session.apiModel
      ? ` · ${session.apiModel}`
      : '';
    const dotColor = agentTheme.cssVar;

    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center px-8">
          <div
            className="inline-flex items-center gap-2 px-3 py-1.5 rounded-full text-xs mb-4"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            <span className="w-2 h-2 rounded-full" style={{ backgroundColor: dotColor }} />
            {agentLabel}{modelInfo}
          </div>
          <p className="text-sm font-medium mb-1" style={{ color: 'var(--text-primary)' }}>
            开始新的对话
          </p>
          <p className="text-xs leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            {session.agentType === 'api'
              ? '消息将通过 API 直连发送'
              : '输入消息或使用技能与 Agent 交互'}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      <Virtuoso
        ref={virtuosoRef}
        className="flex-1"
        data={messages}
        itemContent={itemContent}
        followOutput="smooth"
        increaseViewportBy={{ top: 200, bottom: 200 }}
        components={{
          Header: () => (
            <div className="px-4 py-2 border-b" style={{ borderColor: 'var(--border)' }}>
              <div className="relative">
                <input
                  type="text"
                  value={searchQuery}
                  onChange={(e) => handleMessageSearch(e.target.value)}
                  placeholder="搜索消息..."
                  className="w-full text-xs px-3 py-1.5 rounded-lg outline-none"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                  }}
                />
                {searchQuery && (
                  <button
                    onClick={() => { setSearchQuery(''); setSearchResults(null); }}
                    className="absolute right-2 top-1/2 -translate-y-1/2"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                  </button>
                )}
                {isSearchingMessages && (
                  <div className="absolute right-6 top-1/2 -translate-y-1/2">
                    <div className="w-3 h-3 border-2 border-t-transparent rounded-full animate-spin" style={{ borderColor: 'var(--text-tertiary)', borderTopColor: 'transparent' }} />
                  </div>
                )}
              </div>
              {searchResults !== null && (
                <div className="text-[10px] mt-1" style={{ color: 'var(--text-tertiary)' }}>
                  找到 {searchResults.length} 条结果
                </div>
              )}
            </div>
          ),
          Footer: () => showTypingIndicator ? (
            <div className="flex gap-3 px-4 py-[3px]" style={{ backgroundColor: 'var(--bg-secondary)' }}>
              <div className="shrink-0 pt-0.5">
                <div
                  className="w-7 h-7 rounded-full flex items-center justify-center text-xs font-bold"
                  style={{
                    backgroundColor: (AGENT_THEMES[session?.agentType ?? ''] ?? AGENT_THEMES.claude).bg,
                    color: (AGENT_THEMES[session?.agentType ?? ''] ?? AGENT_THEMES.claude).color,
                  }}
                >
                  {(AGENT_THEMES[session?.agentType ?? ''] ?? AGENT_THEMES.claude).initial}
                </div>
              </div>
              <div className="flex items-center gap-1.5 py-2">
                <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: 'var(--text-tertiary)', animationDelay: '0ms' }} />
                <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: 'var(--text-tertiary)', animationDelay: '150ms' }} />
                <span className="inline-block w-1.5 h-1.5 rounded-full animate-bounce" style={{ backgroundColor: 'var(--text-tertiary)', animationDelay: '300ms' }} />
                <span className="text-xs ml-2" style={{ color: 'var(--text-tertiary)' }}>
                  {streamingStatus || '思考中...'}
                </span>
              </div>
            </div>
          ) : null,
        }}
      />
    </div>
  );
}
