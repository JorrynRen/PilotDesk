import { useState, useCallback, useEffect, useRef } from 'react';
import { MessageList } from '../message/MessageList';
import { InputBar } from './InputBar';
import { useSessionStore } from '../../stores/sessionStore';
import { useWebSocket } from '../../hooks/useWebSocket';
import { showToast } from '../../utils/toast';
import type { ChatMode, Message } from '../../types';

interface StreamingMessage {
  role: 'assistant';
  content: string;
  sessionId: string;
  id: string;
  mode: ChatMode;
  timestamp: number;
}

export function MainPanel() {
  const {
    currentSessionId,
    sessions,
    messages,
    isLoadingMessages,
    addMessage,
  } = useSessionStore();

  const [isGenerating, setIsGenerating] = useState(false);
  const [streamingContent, setStreamingContent] = useState('');

  const currentSession = sessions.find((s) => s.id === currentSessionId) || null;
  const pendingInputRef = useRef<string | null>(null);

  // Consume pending input from sessionStorage (bridging from inspiration picker etc.)
  useEffect(() => {
    const pending = sessionStorage.getItem('pilotdesk_pending_input');
    if (pending) {
      sessionStorage.removeItem('pilotdesk_pending_input');
      pendingInputRef.current = pending;
    }
  }, [currentSessionId]);

  // WebSocket handlers
  const onChunk = useCallback((sessionId: string, content: string) => {
    if (sessionId === currentSessionId) {
      setStreamingContent((prev) => prev + content);
    }
  }, [currentSessionId]);

  const onDone = useCallback((sessionId: string) => {
    if (sessionId === currentSessionId) {
      setIsGenerating(false);
      // Save the complete streamed message to store
      const finalContent = streamingContent;
      setStreamingContent((prev) => {
        // Use the latest content at the moment of callback
        if (prev && currentSessionId) {
          const msg: Message = {
            id: `msg-${Date.now()}`,
            sessionId: currentSessionId,
            role: 'assistant',
            content: prev,
            mode: 'native',
            timestamp: Math.floor(Date.now() / 1000),
          };
          addMessage(msg);
        }
        return '';
      });
      if (finalContent && currentSessionId) {
        const msg: Message = {
          id: `msg-${Date.now()}`,
          sessionId: currentSessionId,
          role: 'assistant',
          content: finalContent,
          mode: 'native',
          timestamp: Math.floor(Date.now() / 1000),
        };
        addMessage(msg);
      }
    }
  }, [currentSessionId, streamingContent, addMessage]);

  const useWebSocketProps = {
    onChunk,
    onDone,
    onError: (sessionId: string, error: string) => {
      if (sessionId === currentSessionId) {
        showToast(`连接错误: ${error}`, 'error');
        setIsGenerating(false);
        setStreamingContent('');
      }
    },
    onStatus: (sessionId: string, status: string) => {
      if (sessionId === currentSessionId) {
        console.log(`[WS Status] ${sessionId}: ${status}`);
      }
    },
  };

  const { isConnected, sendChat, stopGeneration } = useWebSocket(19830, useWebSocketProps);

  const handleSend = useCallback(
    (message: string, mode: ChatMode) => {
      if (!currentSession) return;
      setIsGenerating(true);
      setStreamingContent('');

      // Save user message to store
      const userMsg: Message = {
        id: `msg-${Date.now()}`,
        sessionId: currentSession.id,
        role: 'user',
        content: message,
        mode,
        timestamp: Math.floor(Date.now() / 1000),
      };
      addMessage(userMsg);

      sendChat(currentSession.id, message, mode, currentSession.agentType);
    },
    [currentSession, sendChat, addMessage]
  );

  const handleStop = useCallback(() => {
    if (currentSession) {
      stopGeneration(currentSession.id, currentSession.agentType);
      setIsGenerating(false);
      // Save partial streaming content
      if (streamingContent) {
        const msg: Message = {
          id: `msg-${Date.now()}`,
          sessionId: currentSession.id,
          role: 'assistant',
          content: streamingContent + '\n\n*(已停止生成)*',
          mode: 'native',
          timestamp: Math.floor(Date.now() / 1000),
        };
        addMessage(msg);
        setStreamingContent('');
      }
    }
  }, [currentSession, stopGeneration, streamingContent, addMessage]);

  // Build display messages
  const streamingMsg: StreamingMessage | null =
    isGenerating && streamingContent
      ? {
          role: 'assistant',
          content: streamingContent,
          sessionId: currentSessionId || '',
          id: 'streaming',
          mode: 'native',
          timestamp: Math.floor(Date.now() / 1000),
        }
      : null;

  const displayMessages = streamingMsg ? [...messages, streamingMsg] : messages;

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* Connection status indicator */}
      {!isConnected && (
        <div
          className="px-4 py-1 text-[10px] text-center shrink-0"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: '#F59E0B' }}
        >
          Sidecar 未连接 — 消息无法发送，请检查环境
        </div>
      )}

      {/* Message list */}
      {isLoadingMessages ? (
        <div className="flex-1 flex items-center justify-center">
          <div className="pilotdesk-spinner" />
          <span className="ml-2 text-xs" style={{ color: 'var(--text-secondary)' }}>加载消息中...</span>
        </div>
      ) : (
        <MessageList
          messages={displayMessages}
          session={currentSession}
          onEditMessage={(content) => {
            // Pre-fill input bar with edited content for re-send
            pendingInputRef.current = content;
            showToast('消息已填入输入框，可修改后重新发送', 'info');
          }}
          onSaveInspiration={(content) => {
            // Navigate to market with prefill
            sessionStorage.setItem('pilotdesk_pending_input', content);
            showToast('灵感内容已准备好，前往灵感市集保存', 'success');
          }}
        />
      )}

      {/* Input bar */}
      <InputBar
        session={currentSession}
        onSend={handleSend}
        onStop={handleStop}
        isGenerating={isGenerating}
        pendingInput={pendingInputRef.current}
        onPendingConsumed={() => { pendingInputRef.current = null; }}
      />
    </div>
  );
}
