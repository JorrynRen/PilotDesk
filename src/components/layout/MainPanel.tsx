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

interface SessionGenerationState {
  streamingContent: string;
  streamingStatus: string;
}

interface MainPanelState {
  generatingSessions: Record<string, SessionGenerationState>;
  pendingInput: string | null;
  /** Content that finished streaming, awaiting message creation */
  pendingComplete: { sessionId: string; content: string } | null;
}

type Action =
  | { type: 'SEND_START'; sessionId: string; status: string }
  | { type: 'APPEND_CHUNK'; sessionId: string; content: string }
  | { type: 'GENERATION_DONE'; sessionId: string }
  | { type: 'GENERATION_ERROR'; sessionId: string; error: string }
  | { type: 'GENERATION_TIMEOUT'; sessionId: string }
  | { type: 'STOP_GENERATION'; sessionId: string }
  | { type: 'CLEAR_SESSION'; sessionId: string }
  | { type: 'CLEAR_PENDING_COMPLETE' }
  | { type: 'SET_PENDING_INPUT'; content: string | null };

const initialState: MainPanelState = {
  generatingSessions: {},
  pendingInput: null,
  pendingComplete: null,
};

function reducer(state: MainPanelState, action: Action): MainPanelState {
  switch (action.type) {
    case 'SEND_START':
      return {
        ...state,
        generatingSessions: {
          ...state.generatingSessions,
          [action.sessionId]: { streamingContent: '', streamingStatus: action.status },
        },
        pendingComplete: null,
      };

    case 'APPEND_CHUNK': {
      const session = state.generatingSessions[action.sessionId];
      if (!session) return state;
      return {
        ...state,
        generatingSessions: {
          ...state.generatingSessions,
          [action.sessionId]: {
            ...session,
            streamingContent: session.streamingContent + action.content,
          },
        },
      };
    }

    case 'GENERATION_DONE': {
      const session = state.generatingSessions[action.sessionId];
      if (!session) return state;
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      return {
        ...state,
        generatingSessions: rest,
        pendingComplete: session.streamingContent
          ? { sessionId: action.sessionId, content: session.streamingContent }
          : null,
      };
    }

    case 'GENERATION_ERROR': {
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      return { ...state, generatingSessions: rest };
    }

    case 'GENERATION_TIMEOUT': {
      const session = state.generatingSessions[action.sessionId];
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      const timeoutMsg = '*(请求超时：智能体未在 60 秒内响应，请检查智能体状态后重试)*';
      return {
        ...state,
        generatingSessions: rest,
        pendingComplete: {
          sessionId: action.sessionId,
          content: session?.streamingContent
            ? session.streamingContent + '\n\n' + timeoutMsg
            : timeoutMsg,
        },
      };
    }

    case 'STOP_GENERATION': {
      const session = state.generatingSessions[action.sessionId];
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      return {
        ...state,
        generatingSessions: rest,
        pendingComplete: session?.streamingContent
          ? { sessionId: action.sessionId, content: session.streamingContent + '\n\n*(已停止生成)*' }
          : null,
      };
    }

    case 'CLEAR_SESSION': {
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      return { ...state, generatingSessions: rest };
    }

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

  // 每个会话独立的超时计时器
  const timeoutRefs = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  // 已完成的会话 ID 集合（防止重复处理）
  const doneSessionIdsRef = useRef<Set<string>>(new Set());
  const createdSessionsRef = useRef<Set<string>>(new Set());

  const currentSession = sessions.find((s) => s.id === currentSessionId)
    || archivedSessions.find((s) => s.id === currentSessionId)
    || null;

  const currentAgentType = currentSession?.agentType ?? null;

  // 当前会话的生成状态
  const currentGenState = currentSessionId
    ? state.generatingSessions[currentSessionId]
    : undefined;

  // ── 超时管理 ──

  const clearSessionTimeout = useCallback((sessionId: string) => {
    if (timeoutRefs.current[sessionId]) {
      clearTimeout(timeoutRefs.current[sessionId]);
      delete timeoutRefs.current[sessionId];
    }
  }, []);

  // ── Agent Event handlers ──

  const onChunk = useCallback((sessionId: string, content: string) => {
    // 无条件写入对应会话的 streamingContent
    dispatch({ type: 'APPEND_CHUNK', sessionId, content });
  }, []);

  const onDone = useCallback((sessionId: string) => {
    // 防止重复处理
    if (doneSessionIdsRef.current.has(sessionId)) return;
    doneSessionIdsRef.current.add(sessionId);

    clearSessionTimeout(sessionId);
    dispatch({ type: 'GENERATION_DONE', sessionId });
  }, [clearSessionTimeout]);

  const onError = useCallback((sessionId: string, error: string) => {
    clearSessionTimeout(sessionId);

    // 只有当前可见会话的错误才弹 Toast
    if (sessionId === currentSessionId) {
      showToast(`错误: ${error}`, 'error');
    }

    // 向对应会话添加错误消息
    addMessage({
      id: `msg-err-${Date.now()}`,
      sessionId,
      role: 'system',
      content: `❗ 请求失败: ${error}`,
      mode: 'native',
      timestamp: Math.floor(Date.now() / 1000),
    });

    dispatch({ type: 'GENERATION_ERROR', sessionId, error });
  }, [currentSessionId, addMessage, clearSessionTimeout]);

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

    if (!content) return;

    addMessage({
      id: `msg-${Date.now()}`,
      sessionId,
      role: 'assistant',
      content,
      mode: 'native',
      timestamp: Math.floor(Date.now() / 1000),
    });
  }, [state.pendingComplete, addMessage]);

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

      const sid = currentSession.id;

      // 清除当前会话的旧状态
      doneSessionIdsRef.current.delete(sid);
      clearSessionTimeout(sid);

      dispatch({ type: 'SEND_START', sessionId: sid, status: '发送中...' });

      // Save user message to store
      addMessage({
        id: `msg-${Date.now()}`,
        sessionId: sid,
        role: 'user',
        content: message,
        mode,
        timestamp: Math.floor(Date.now() / 1000),
      });

      // 每个会话独立的 60s 超时计时器
      const timeoutId = setTimeout(() => {
        dispatch({ type: 'GENERATION_TIMEOUT', sessionId: sid });
        if (currentSession.agentType === 'api') {
          stopApiChat();
        } else {
          stopGeneration(sid);
        }
        showToast('请求超时：智能体未在 60 秒内响应', 'error');
      }, 60000);
      timeoutRefs.current[sid] = timeoutId;

      if (currentSession.agentType === 'api') {
        // API direct call
        if (!currentSession.apiProvider || !currentSession.apiModel) {
          showToast('API 会话缺少提供商或模型配置', 'error');
          clearSessionTimeout(sid);
          dispatch({ type: 'GENERATION_ERROR', sessionId: sid, error: '缺少 API 配置' });
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
          clearSessionTimeout(sid);
          dispatch({ type: 'GENERATION_ERROR', sessionId: sid, error: '未找到 API URL' });
          return;
        }
        const history = messages
          .filter((m) => m.role === 'user' || m.role === 'assistant')
          .map((m) => ({ role: m.role, content: m.content }));
        sendApiChat(sid, message, apiEndpoint, currentSession.apiProvider, currentSession.apiModel, history);
      } else {
        // Agent via Tauri Event
        const systemPrompt = await getModePrompt(mode);
        sendChat(sid, message, mode, currentSession.agentType, undefined, systemPrompt);
      }
    },
    [currentSession, sendChat, sendApiChat, addMessage, clearSessionTimeout, stopApiChat, stopGeneration, messages],
  );

  const handleStop = useCallback(() => {
    if (!currentSession) return;
    const sid = currentSession.id;

    clearSessionTimeout(sid);

    if (currentSession.agentType === 'api') {
      stopApiChat();
    } else {
      stopGeneration(sid);
    }

    dispatch({ type: 'STOP_GENERATION', sessionId: sid });
  }, [currentSession, stopGeneration, stopApiChat, clearSessionTimeout]);

  // ── Build display messages ──

  const streamingMsg = currentGenState?.streamingContent
    ? {
        role: 'assistant' as const,
        content: currentGenState.streamingContent,
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
          isGenerating={!!currentGenState}
          streamingStatus={currentGenState?.streamingStatus ?? ''}
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
        isGenerating={!!currentGenState}
        streamingStatus={currentGenState?.streamingStatus ?? ''}
        pendingInput={state.pendingInput}
        onPendingConsumed={() => dispatch({ type: 'SET_PENDING_INPUT', content: null })}
      />
    </div>
  );
}
