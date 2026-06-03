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
  const doneCalledRef = useRef<string | null>(null);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Map raw status strings to friendly display text
  const statusToFriendly = useCallback((status: string): string => {
    const STATUS_MAP: Record<string, string> = {
      'session_created:claude': '向 Claude Code 发送消息中，请等待回复…',
      'session_created:hermes': '向 Hermes Agent 发送消息中，请等待回复…',
      'session_closed': '会话已关闭',
      'generation_stopped': '已停止生成',
      'pong': '',
    };
    return STATUS_MAP[status] || status;
  }, []);

  const currentSession = sessions.find((s) => s.id === currentSessionId) || null;
  // Stable refs for session create effect (avoid object reference changes on every render)
  const currentAgentTypeRef = useRef<string | null>(null);
  currentAgentTypeRef.current = currentSession?.agentType ?? null;
  const createdSessionsRef = useRef<Set<string>>(new Set());

  // WebSocket handlers (must be defined before useWebSocket call)
  const onChunk = useCallback((sessionId: string, content: string) => {
    if (sessionId === currentSessionId) {
      setStreamingContent((prev) => prev + content);
    }
  }, [currentSessionId]);

  const clearTimeoutSafe = useCallback(() => {
    if (timeoutRef.current !== null) {
      clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
  }, []);

  const onDone = useCallback((sessionId: string) => {
    if (doneCalledRef.current === sessionId) return;
    doneCalledRef.current = sessionId;
    clearTimeoutSafe();
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
  }, [currentSessionId, addMessage, clearTimeoutSafe]);

  const wsHandlers = {
    onChunk,
    onDone,
    onError: (sessionId: string, error: string) => {
      if (sessionId === currentSessionId) {
        clearTimeoutSafe();
        showToast(`错误: ${error}`, 'error');
        // Save error as a persistent message in the session
        if (currentSessionId) {
          const errorMsg: Message = {
            id: `msg-err-${Date.now()}`,
            sessionId: currentSessionId,
            role: 'system',
            content: `❗ 请求失败: ${error}`,
            mode: 'native',
            timestamp: Math.floor(Date.now() / 1000),
          };
          addMessage(errorMsg);
        }
        setIsGenerating(false);
        setStreamingContent('');
        setStreamingStatus('');
      }
    },
    onStatus: (sessionId: string, status: string) => {
      if (sessionId === currentSessionId) {
        setStreamingStatus(statusToFriendly(status));
      }
    },
  };

  const { isConnected, sendChat, sendApiChat, stopGeneration, stopApiChat, createAgentSession } = useWebSocket(19830, wsHandlers);

  // When switching to an Agent session, ensure Sidecar has the session created
  // Only fires when currentSessionId changes (not on every render)
  useEffect(() => {
    const agentType = currentAgentTypeRef.current;
    if (currentSessionId && agentType && agentType !== 'api' && isConnected) {
      // Only create once per session
      if (!createdSessionsRef.current.has(currentSessionId)) {
        createdSessionsRef.current.add(currentSessionId);
        createAgentSession(currentSessionId, agentType);
      }
    }
  }, [currentSessionId, isConnected, createAgentSession]);

  // Consume pending input from shared store
  const pendingFromStore = usePendingInputStore((s) => s.value);
  useEffect(() => {
    if (pendingFromStore) {
      setPendingInput(pendingFromStore);
      usePendingInputStore.getState().set(null);
    }
  }, [pendingFromStore]);

  const handleSend = useCallback(
    async (message: string, mode: ChatMode) => {
      if (!currentSession) return;
      doneCalledRef.current = null;
      clearTimeoutSafe();
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

      // Start 60s frontend timeout
      timeoutRef.current = setTimeout(() => {
        if (currentSession) {
          setIsGenerating(false);
          setStreamingStatus('');
          // Always save a timeout message, even when streamingContent is empty
          setStreamingContent((prev) => {
            const sid = currentSession.id;
            const content = prev
              ? prev + '\n\n*(请求超时：智能体未在 60 秒内响应，请检查智能体状态后重试)*'
              : '*(请求超时：智能体未在 60 秒内响应，请检查智能体状态后重试)*';
            const msg: Message = {
              id: `msg-${Date.now()}`,
              sessionId: sid,
              role: 'assistant',
              content,
              mode: 'native',
              timestamp: Math.floor(Date.now() / 1000),
            };
            addMessage(msg);
            return '';
          });
          // Send stop signal to Sidecar
          if (currentSession.agentType === 'api') {
            stopApiChat();
          } else {
            stopGeneration(currentSession.id, currentSession.agentType as 'claude' | 'hermes');
          }
          showToast('请求超时：智能体未在 60 秒内响应', 'error');
        }
      }, 60000);

      if (currentSession.agentType === 'api') {
        // API direct call
        if (!currentSession.apiProvider || !currentSession.apiModel) {
          showToast('API 会话缺少提供商或模型配置', 'error');
          setIsGenerating(false);
          clearTimeoutSafe();
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
          clearTimeoutSafe();
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
    [currentSession, sendChat, sendApiChat, addMessage, clearTimeoutSafe]
  );

  const handleStop = useCallback(() => {
    clearTimeoutSafe();
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
  }, [currentSession, stopGeneration, stopApiChat, streamingContent, addMessage, clearTimeoutSafe]);

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
