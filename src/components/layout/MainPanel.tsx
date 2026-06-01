import { useState, useCallback, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { MessageList } from '../message/MessageList';
import { InputBar } from './InputBar';
import { useSessionStore } from '../../stores/sessionStore';
import { useWebSocket } from '../../hooks/useWebSocket';
import { usePendingInputStore } from '../../stores/pendingInputStore';
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
  const [streamingStatus, setStreamingStatus] = useState('');
  const [pendingInput, setPendingInput] = useState<string | null>(null);
  const doneCalledRef = useRef(false);

  const currentSession = sessions.find((s) => s.id === currentSessionId) || null;

  // Consume pending input from shared store
  const pendingFromStore = usePendingInputStore((s) => s.value);
  useEffect(() => {
    if (pendingFromStore) {
      setPendingInput(pendingFromStore);
      usePendingInputStore.getState().set(null);
    }
  }, [pendingFromStore]);

  // WebSocket handlers
  const onChunk = useCallback((sessionId: string, content: string) => {
    if (sessionId === currentSessionId) {
      setStreamingContent((prev) => prev + content);
    }
  }, [currentSessionId]);

  const onDone = useCallback((sessionId: string) => {
    if (doneCalledRef.current) return;
    doneCalledRef.current = true;
    if (sessionId === currentSessionId) {
      setIsGenerating(false);
      setStreamingStatus('');
      setStreamingContent((prev) => {
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
    }
  }, [currentSessionId, addMessage]);

  const wsHandlers = {
    onChunk,
    onDone,
    onError: (sessionId: string, error: string) => {
      if (sessionId === currentSessionId) {
        showToast(`错误: ${error}`, 'error');
        setIsGenerating(false);
        setStreamingContent('');
        setStreamingStatus('');
      }
    },
    onStatus: (sessionId: string, status: string) => {
      if (sessionId === currentSessionId) {
        setStreamingStatus(status);
      }
    },
  };

  const { isConnected, sendChat, sendApiChat, stopGeneration, stopApiChat } = useWebSocket(19830, wsHandlers);

  const handleSend = useCallback(
    async (message: string, mode: ChatMode) => {
      if (!currentSession) return;
      doneCalledRef.current = false;
      setIsGenerating(true);
      setStreamingContent('');
      setStreamingStatus('发送中...');

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

      if (currentSession.agentType === 'api') {
        // API direct call
        if (!currentSession.apiProvider || !currentSession.apiModel) {
          showToast('API 会话缺少提供商或模型配置', 'error');
          setIsGenerating(false);
          return;
        }
        // Look up full API URL from SQLite
        let apiEndpoint = '';
        try {
          const provider = await invoke<{ apiEndpoint: string } | null>('get_api_provider', { id: currentSession.apiProvider });
          if (provider) {
            apiEndpoint = provider.apiEndpoint;
          }
        } catch { /* ignore */ }
        if (!apiEndpoint) {
          showToast('未找到 API URL，请在设置中配置', 'error');
          setIsGenerating(false);
          return;
        }
        // Build message history for multi-turn API chat (exclude current user message, it's added below)
        const history = messages
          .filter((m) => m.role === 'user' || m.role === 'assistant')
          .map((m) => ({ role: m.role, content: m.content }));
        sendApiChat(currentSession.id, message, apiEndpoint, currentSession.apiProvider, currentSession.apiModel, history);
      } else {
        // Sidecar WebSocket (claude / hermes)
        sendChat(currentSession.id, message, mode, currentSession.agentType as 'claude' | 'hermes');
      }
    },
    [currentSession, sendChat, sendApiChat, addMessage]
  );

  const handleStop = useCallback(() => {
    if (currentSession) {
      if (currentSession.agentType === 'api') {
        stopApiChat();
      } else {
        stopGeneration(currentSession.id, currentSession.agentType as 'claude' | 'hermes');
      }
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
  }, [currentSession, stopGeneration, stopApiChat, streamingContent, addMessage]);

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
          isGenerating={isGenerating}
          streamingStatus={streamingStatus}
          onEditMessage={(content) => {
            setPendingInput(content);
            showToast('消息已填入输入框，可修改后重新发送', 'info');
          }}
          onSaveInspiration={(content) => {
            usePendingInputStore.getState().set(content);
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
        pendingInput={pendingInput}
        onPendingConsumed={() => setPendingInput(null)}
      />
    </div>
  );
}
