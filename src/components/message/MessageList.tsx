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
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<Message[] | null>(null);
  const [isSearchingMessages, setIsSearchingMessages] = useState(false);

  const searchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleMessageSearch = useCallback((query: string) => {
    setSearchQuery(query);
    // Clear previous timer
    if (searchTimerRef.current) {
      clearTimeout(searchTimerRef.current);
      searchTimerRef.current = null;
    }
    if (!query.trim()) {
      setSearchResults(null);
      return;
    }
    // Debounce search to avoid invoke on every keystroke
    searchTimerRef.current = setTimeout(async () => {
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
    }, 300);
  }, [session?.id]);

  // Cleanup timer on unmount
  useEffect(() => {
    return () => {
      if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    };
  }, []);

  // 记录上一个会话 ID，用于检测会话切换
  const prevSessionIdRef = useRef<string | null>(null);

  // 切换会话时 → 滚动到底部
  useEffect(() => {
    if (session?.id && session.id !== prevSessionIdRef.current) {
      prevSessionIdRef.current = session.id;
      if (messages.length > 0) {
        setTimeout(() => {
          virtuosoRef.current?.scrollToIndex({ index: messages.length - 1, behavior: 'auto' });
        }, 100);
      }
    }
  }, [session?.id]);

  // 收到消息时 → 滚动到底部
  useEffect(() => {
    if (messages.length > 0) {
      requestAnimationFrame(() => {
        virtuosoRef.current?.scrollToIndex({ index: messages.length - 1, behavior: 'smooth' });
      });
    }
  }, [messages]);

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
          <h2 className="text-base font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
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
    const dotColor = agentTheme.color;

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
          <p className="text-sm  mb-1" style={{ color: 'var(--text-primary)' }}>
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
      {/* Floating search bar - outside Virtuoso, stays at top */}
      {messages.length > 0 && (
        <div className="shrink-0 px-4 flex items-center border-b" style={{ borderColor: 'var(--border)', backgroundColor: 'var(--bg-primary)', height: '36px' }}>
          <div className="flex items-center gap-2 flex-1">
            <div className="relative flex-1">
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => handleMessageSearch(e.target.value)}
                placeholder="搜索消息..."
                className="w-full text-xs px-3 py-1 rounded-lg outline-none"
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
              <div className="flex items-center gap-1 shrink-0">
                <span className="pd-text-10" style={{ color: 'var(--text-tertiary)' }}>
                  {searchResults.length} 条
                </span>
                {searchResults.length > 0 && (
                  <button
                    onClick={() => {
                      const first = messages.findIndex(m => m.id === searchResults[0].id);
                      if (first >= 0) virtuosoRef.current?.scrollToIndex({ index: first, behavior: 'smooth' });
                    }}
                    className="pd-text-10 px-1.5 py-0.5 rounded transition-colors hover:opacity-80"
                    style={{ color: 'var(--accent)' }}
                    title="定位到第一条"
                  >
                    定位
                  </button>
                )}
              </div>
            )}
          </div>
        </div>
      )}

      <Virtuoso
        ref={virtuosoRef}
        className="flex-1"
        data={messages}
        itemContent={itemContent}
        followOutput="smooth"
        increaseViewportBy={{ top: 200, bottom: 200 }}
        components={{
          Footer: () => null,
        }}
      />
    </div>
  );
}
