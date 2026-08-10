import { useCallback, useEffect, useLayoutEffect, useReducer, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { MessageList } from '../message/MessageList';
import { InputBar } from './InputBar';
import { useSessionStore } from '../../stores/sessionStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { usePendingInputStore } from '../../stores/pendingInputStore';
import { showToast } from '../../utils/toast';
import { useGeneratingStore } from '../../stores/generatingStore';

import type { ChatMode, Message } from '../../types';
import { getModePrompt } from '../../types';
import { isApiSession } from '../../utils/sessionType';

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
  | { type: 'STOP_GENERATION'; sessionId: string }
  | { type: 'CLEAR_SESSION'; sessionId: string }
  | { type: 'CLEAR_GENERATING_SESSION'; sessionId: string }
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
      return {
        ...state,
        pendingComplete: session.streamingContent
          ? { sessionId: action.sessionId, content: session.streamingContent }
          : null,
      };
    }

    case 'GENERATION_ERROR': {
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      return { ...state, generatingSessions: rest };
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

    case 'CLEAR_GENERATING_SESSION': {
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

export function MainPanel({ style }: { style?: React.CSSProperties } = {}) {
  
  
  const {
    currentSessionId,
    sessions,
    archivedSessions,
    messages,
    isLoadingMessages,
    addMessage,
  } = useSessionStore();

  const [state, dispatch] = useReducer(reducer, initialState);

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

  // ── Agent Event handlers ──

  const onChunk = useCallback((sessionId: string, content: string) => {
    // 无条件写入对应会话的 streamingContent
    dispatch({ type: 'APPEND_CHUNK', sessionId, content });
  }, []);

  const onDone = useCallback((sessionId: string) => {
    // 防止重复处理
    if (doneSessionIdsRef.current.has(sessionId)) return;
    doneSessionIdsRef.current.add(sessionId);

    dispatch({ type: 'GENERATION_DONE', sessionId });
  }, []);

  const onError = useCallback((sessionId: string, error: string) => {
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
  }, [currentSessionId, addMessage]);

  // ── Agent session ID handler: save agent-side session ID to database ──
  const onSession = useCallback(async (sessionId: string, agentSessionId: string) => {
    // Persist agent_session_id to database first
    try {
      await invoke('update_session_agent_id', { sessionId, agentSessionId });
      // 局部更新：仅修改目标 session 的 agentSessionId，不触发全量刷新
      useSessionStore.setState((state) => ({
        sessions: state.sessions.map((s) =>
          s.id === sessionId ? { ...s, agentSessionId } : s
        ),
      }));
    } catch (err) {
      console.error('[Session] Failed to save agent session ID:', err);
    }
  }, []);

  const {
    sendChat,
    stopGeneration,
    createAgentSession,
    respondToApproval,
  } = useAgentEvent({ onChunk, onDone, onError, onSession });

  // ── Layout effect: atomically persist completed streaming content ──

  useLayoutEffect(() => {
    if (!state.pendingComplete) return;
    const { sessionId, content: msgContent } = state.pendingComplete;

    // 先清除 pendingComplete 防止重复执行
    dispatch({ type: 'CLEAR_PENDING_COMPLETE' });
    // 同步清除 generatingSessions 中的 session（React 18+ 批处理合并）
    dispatch({ type: 'CLEAR_GENERATING_SESSION', sessionId });

    if (!msgContent) return;

    addMessage({
      id: `msg-${Date.now()}`,
      sessionId,
      role: 'assistant',
      content: msgContent,
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

  // ── Side effect: sync generating state to shared store for SessionList indicator ──
  useEffect(() => {
    const currentGenerating = new Set(Object.keys(state.generatingSessions));
    useGeneratingStore.getState().syncGeneratingSessions(currentGenerating);
    // 仅在 generatingSessions 的 key 集合变化时同步（忽略 streaming content 变化）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [Object.keys(state.generatingSessions).join(',')]);

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


      if (isApiSession(currentSession.agentType)) {
        // API Agent: 路由通过 Rust 后端 AgentLoop（替代前端直接 HTTP）
        if (!currentSession.apiProvider || !currentSession.apiModel) {
          showToast('API 会话缺少提供商或模型配置', 'error');
          dispatch({ type: 'GENERATION_ERROR', sessionId: sid, error: '缺少 API 配置' });
          return;
        }
        const systemPrompt = await getModePrompt(mode);
        sendChat(sid, message, mode, 'api', currentSession.cwd || undefined, systemPrompt);
      } else {
        // Agent via Tauri Event
        const systemPrompt = await getModePrompt(mode);
        // Pass agent_session_id for session continuity (Claude Code --resume)
        const agentSessionId = currentSession.agentSessionId || undefined;
        sendChat(sid, message, mode, currentSession.agentType, currentSession.cwd || undefined, systemPrompt, agentSessionId);
      }
    },
    [currentSession, sendChat, addMessage, stopGeneration, messages],
  );

  const handleStop = useCallback(() => {
    if (!currentSession) return;
    const sid = currentSession.id;

    // API 和 CLI Agent 统一通过 stopGeneration 停止（Rust 后端处理）
    stopGeneration(sid);

    dispatch({ type: 'STOP_GENERATION', sessionId: sid });
  }, [currentSession, stopGeneration]);

  // ── Stable callbacks for MessageList ──

  const handleEditMessage = useCallback((content: string) => {
    dispatch({ type: 'SET_PENDING_INPUT', content });
    showToast('消息已填入输入框，可修改后重新发送', 'info');
  }, []);

  const handleSaveInspiration = useCallback((content: string) => {
    usePendingInputStore.getState().set(content);
    showToast('灵感内容已准备好，前往灵感市集保存', 'success');
  }, []);

  const handleResendMessage = useCallback((content: string) => {
    dispatch({ type: 'SET_PENDING_INPUT', content });
  }, []);

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
    <div className="flex-1 flex flex-col overflow-hidden relative" style={style}>
      <div
        style={{
          height: '100%',
          width: '100%',
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
        }}
      >
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
          onEditMessage={handleEditMessage}
          onSaveInspiration={handleSaveInspiration}
          onResendMessage={handleResendMessage}
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
      </div>
    </div>
  );
}
