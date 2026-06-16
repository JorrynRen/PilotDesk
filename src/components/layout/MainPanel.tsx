import { useCallback, useEffect, useReducer, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { MessageList } from '../message/MessageList';
import { InputBar } from './InputBar';
import { useSessionStore } from '../../stores/sessionStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { usePendingInputStore } from '../../stores/pendingInputStore';
import { showToast } from '../../utils/toast';
import type { ChatMode, Message } from '../../types';
import { getModePrompt } from '../../types';

// ── State Machine ──

interface MainPanelState {
  isGenerating: boolean;
  streamingContent: string;
  streamingStatus: string;
  pendingInput: string | null;
  /** Content that finished streaming, awaiting message creation */
  pendingComplete: { sessionId: string; content: string } | null;
}

type Action =
  | { type: 'SEND_START'; status: string }
  | { type: 'APPEND_CHUNK'; content: string }
  | { type: 'GENERATION_DONE'; sessionId: string }
  | { type: 'GENERATION_ERROR'; sessionId: string; error: string }
  | { type: 'GENERATION_TIMEOUT'; sessionId: string }
  | { type: 'STOP_GENERATION'; sessionId: string }
  | { type: 'CLEAR_PENDING_COMPLETE' }
  | { type: 'SET_PENDING_INPUT'; content: string | null };

const initialState: MainPanelState = {
  isGenerating: false,
  streamingContent: '',
  streamingStatus: '',
  pendingInput: null,
  pendingComplete: null,
};

function reducer(state: MainPanelState, action: Action): MainPanelState {
  switch (action.type) {
    case 'SEND_START':
      return { ...state, isGenerating: true, streamingContent: '', streamingStatus: action.status, pendingComplete: null };

    case 'APPEND_CHUNK':
      return { ...state, streamingContent: state.streamingContent + action.content };

    case 'GENERATION_DONE':
      return {
        ...state,
        isGenerating: false,
        streamingStatus: '',
        pendingComplete: { sessionId: action.sessionId, content: state.streamingContent },
        streamingContent: '',
      };

    case 'GENERATION_ERROR':
      return {
        ...state,
        isGenerating: false,
        streamingStatus: '',
        streamingContent: '',
      };

    case 'GENERATION_TIMEOUT':
      return {
        ...state,
        isGenerating: false,
        streamingStatus: '',
        pendingComplete: {
          sessionId: action.sessionId,
          content: state.streamingContent
            ? state.streamingContent + '\n\n*(请求超时：智能体未在 60 秒内响应，请检查智能体状态后重试)*'
            : '*(请求超时：智能体未在 60 秒内响应，请检查智能体状态后重试)*',
        },
        streamingContent: '',
      };

    case 'STOP_GENERATION':
      return {
        ...state,
        isGenerating: false,
        streamingStatus: '',
        pendingComplete: state.streamingContent
          ? { sessionId: action.sessionId, content: state.streamingContent + '\n\n*(已停止生成)*' }
          : null,
        streamingContent: '',
      };

    case 'CLEAR_PENDING_COMPLETE':
      return { ...state, pendingComplete: null };

    case 'SET_PENDING_INPUT':
      return { ...state, pendingInput: action.content };

    default:
      return state;
  }
}

// ── Component ──

export function MainPanel() {
  const {
    currentSessionId,
    sessions,
    archivedSessions,
    messages,
    isLoadingMessages,
    addMessage,
  } = useSessionStore();

  const [state, dispatch] = useReducer(reducer, initialState);

  // Side-effect refs (not UI state)
  const doneSessionIdRef = useRef<string | null>(null);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const createdSessionsRef = useRef<Set<string>>(new Set());

  const currentSession = sessions.find((s) => s.id === currentSessionId)
    || archivedSessions.find((s) => s.id === currentSessionId)
    || null;

  const currentAgentType = currentSession?.agentType ?? null;

  // ── Agent Event handlers ──

  const onChunk = useCallback((sessionId: string, content: string) => {
    if (sessionId === currentSessionId) {
      dispatch({ type: 'APPEND_CHUNK', content });
    }
  }, [currentSessionId]);

  const clearTimeoutSafe = useCallback(() => {
    if (timeoutRef.current !== null) {
      clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
  }, []);

  const onDone = useCallback((sessionId: string) => {
    if (doneSessionIdRef.current === sessionId) return;
    doneSessionIdRef.current = sessionId;
    clearTimeoutSafe();
    if (sessionId === currentSessionId) {
      dispatch({ type: 'GENERATION_DONE', sessionId });
    }
  }, [currentSessionId, clearTimeoutSafe]);

  const onError = useCallback((sessionId: string, error: string) => {
    if (sessionId === currentSessionId) {
      clearTimeoutSafe();
      showToast(`错误: ${error}`, 'error');
      if (currentSessionId) {
        addMessage({
          id: `msg-err-${Date.now()}`,
          sessionId: currentSessionId,
          role: 'system',
          content: `❗ 请求失败: ${error}`,
          mode: 'native',
          timestamp: Math.floor(Date.now() / 1000),
        });
      }
      dispatch({ type: 'GENERATION_ERROR', sessionId, error });
    }
  }, [currentSessionId, addMessage, clearTimeoutSafe]);

  const {
    sendChat,
    sendApiChat,
    stopGeneration,
    stopApiChat,
    createAgentSession,
  } = useAgentEvent({ onChunk, onDone, onError });

  // ── Side effect: persist completed streaming content as a message ──

  useEffect(() => {
    if (!state.pendingComplete) return;
    const { sessionId, content } = state.pendingComplete;
    dispatch({ type: 'CLEAR_PENDING_COMPLETE' });

    if (sessionId !== currentSessionId) return;
    if (!content) return;

    addMessage({
      id: `msg-${Date.now()}`,
      sessionId,
      role: 'assistant',
      content,
      mode: 'native',
      timestamp: Math.floor(Date.now() / 1000),
    });
  }, [state.pendingComplete, currentSessionId, addMessage]);

  // ── Side effect: ensure Rust backend session exists for Agent sessions ──

  useEffect(() => {
    if (currentSessionId && currentAgentType && currentAgentType !== 'api') {
      if (!createdSessionsRef.current.has(currentSessionId)) {
        createdSessionsRef.current.add(currentSessionId);
        createAgentSession(currentSessionId, currentAgentType);
      }
    }
  }, [currentSessionId, currentAgentType, createAgentSession]);

  // ── Side effect: consume pending input from shared store ──

  const pendingFromStore = usePendingInputStore((s) => s.value);
  useEffect(() => {
    if (pendingFromStore) {
      dispatch({ type: 'SET_PENDING_INPUT', content: pendingFromStore });
      usePendingInputStore.getState().set(null);
    }
  }, [pendingFromStore]);

  // ── Send / Stop handlers ──

  const handleSend = useCallback(
    async (message: string, mode: ChatMode) => {
      if (!currentSession) return;
      doneSessionIdRef.current = null;
      clearTimeoutSafe();
      dispatch({ type: 'SEND_START', status: '发送中...' });

      // Save user message to store
      addMessage({
        id: `msg-${Date.now()}`,
        sessionId: currentSession.id,
        role: 'user',
        content: message,
        mode,
        timestamp: Math.floor(Date.now() / 1000),
      });

      // Start 60s frontend timeout
      timeoutRef.current = setTimeout(() => {
        dispatch({ type: 'GENERATION_TIMEOUT', sessionId: currentSession.id });
        if (currentSession.agentType === 'api') {
          stopApiChat();
        } else {
          stopGeneration(currentSession.id);
        }
        showToast('请求超时：智能体未在 60 秒内响应', 'error');
      }, 60000);

      if (currentSession.agentType === 'api') {
        // API direct call
        if (!currentSession.apiProvider || !currentSession.apiModel) {
          showToast('API 会话缺少提供商或模型配置', 'error');
          clearTimeoutSafe();
          dispatch({ type: 'GENERATION_ERROR', sessionId: currentSession.id, error: '缺少 API 配置' });
          return;
        }
        let apiEndpoint = '';
        try {
          const provider = await invoke<{ apiEndpoint: string } | null>('get_api_provider', { id: currentSession.apiProvider });
          if (provider) {
            apiEndpoint = provider.apiEndpoint;
          }
        } catch { /* ignore */ }
        if (!apiEndpoint) {
          showToast('未找到 API URL，请在设置中配置', 'error');
          clearTimeoutSafe();
          dispatch({ type: 'GENERATION_ERROR', sessionId: currentSession.id, error: '未找到 API URL' });
          return;
        }
        const history = messages
          .filter((m) => m.role === 'user' || m.role === 'assistant')
          .map((m) => ({ role: m.role, content: m.content }));
        sendApiChat(currentSession.id, message, apiEndpoint, currentSession.apiProvider, currentSession.apiModel, history);
      } else {
        // Agent via Tauri Event
        const systemPrompt = await getModePrompt(mode);
        sendChat(currentSession.id, message, mode, currentSession.agentType, undefined, systemPrompt);
      }
    },
    [currentSession, sendChat, sendApiChat, addMessage, clearTimeoutSafe, stopApiChat, stopGeneration, messages],
  );

  const handleStop = useCallback(() => {
    clearTimeoutSafe();
    if (currentSession) {
      if (currentSession.agentType === 'api') {
        stopApiChat();
      } else {
        stopGeneration(currentSession.id);
      }
      dispatch({ type: 'STOP_GENERATION', sessionId: currentSession.id });
    }
  }, [currentSession, stopGeneration, stopApiChat, clearTimeoutSafe]);

  // ── Build display messages ──

  const streamingMsg = state.isGenerating && state.streamingContent
    ? {
        role: 'assistant' as const,
        content: state.streamingContent,
        sessionId: currentSessionId || '',
        id: 'streaming',
        mode: 'native' as ChatMode,
        timestamp: Math.floor(Date.now() / 1000),
      }
    : null;

  const displayMessages = streamingMsg ? [...messages, streamingMsg] : messages;

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {isLoadingMessages ? (
        <div className="flex-1 flex items-center justify-center">
          <div className="pilotdesk-spinner" />
          <span className="ml-2 text-xs" style={{ color: 'var(--text-secondary)' }}>加载消息中...</span>
        </div>
      ) : (
        <MessageList
          messages={displayMessages}
          session={currentSession}
          isGenerating={state.isGenerating}
          streamingStatus={state.streamingStatus}
          onEditMessage={(content) => {
            dispatch({ type: 'SET_PENDING_INPUT', content });
            showToast('消息已填入输入框，可修改后重新发送', 'info');
          }}
          onSaveInspiration={(content) => {
            usePendingInputStore.getState().set(content);
            showToast('灵感内容已准备好，前往灵感市集保存', 'success');
          }}
          onResendMessage={(content) => {
            dispatch({ type: 'SET_PENDING_INPUT', content });
          }}
        />
      )}

      <InputBar
        session={currentSession}
        onSend={handleSend}
        onStop={handleStop}
        isGenerating={state.isGenerating}
        pendingInput={state.pendingInput}
        onPendingConsumed={() => dispatch({ type: 'SET_PENDING_INPUT', content: null })}
      />
    </div>
  );
}
