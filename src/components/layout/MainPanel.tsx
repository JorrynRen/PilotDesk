import { useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { MessageList } from '../message/MessageList';
import { InputBar } from './InputBar';
import { useSessionStore } from '../../stores/sessionStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { usePendingInputStore } from '../../stores/pendingInputStore';
import { showToast } from '../../utils/toast';
import { useGeneratingStore } from '../../stores/generatingStore';
import { formatResponsesToText } from '../confirmation/ConfirmationCard';

import type { ChatMode, Message, Attachment } from '../../types';
import type { GroupChatConfirmationRequest, GroupChatConfirmationResponseInput } from '../../types/groupchat';
import { getModePrompt } from '../../types';
import { isApiSession } from '../../utils/sessionType';

// ── State Machine ──

interface SessionGenerationState {
  streamingContent: string;
  streamingStatus: string;
}

/** A single step in the thinking chain (reasoning or tool call) */
export interface ThinkingChainStep {
  id: string;
  type: 'reasoning' | 'tool_start' | 'tool_result' | 'file_diff';
  content?: string;
  toolName?: string;
  toolArgs?: string;
  toolResult?: string;
  toolSuccess?: boolean;
  filePath?: string;
  fileDiff?: string;
  timestamp: number;
}

interface MainPanelState {
  generatingSessions: Record<string, SessionGenerationState>;
  /** Thinking chain steps per session (reasoning + tool calls, displayed as collapsible panel) */
  thinkingChains: Record<string, ThinkingChainStep[]>;
  pendingInput: string | null;
  /** Content that finished streaming, awaiting message creation */
  pendingComplete: { sessionId: string; content: string } | null;
  /** System-level notification message (timeout, empty response, etc.) */
  pendingSystemMessage: { sessionId: string; content: string } | null;
}

type Action =
  | { type: 'SEND_START'; sessionId: string; status: string }
  | { type: 'APPEND_CHUNK'; sessionId: string; content: string }
  | { type: 'GENERATION_DONE'; sessionId: string; fallbackContent?: string; systemMessage?: string }
  | { type: 'GENERATION_ERROR'; sessionId: string; error: string }
  | { type: 'STOP_GENERATION'; sessionId: string }
  | { type: 'CLEAR_SESSION'; sessionId: string }
  | { type: 'CLEAR_GENERATING_SESSION'; sessionId: string }
  | { type: 'CLEAR_PENDING_COMPLETE' }
  | { type: 'CLEAR_PENDING_SYSTEM_MESSAGE' }
  | { type: 'SET_PENDING_INPUT'; content: string | null }
  | { type: 'APPEND_REASONING'; sessionId: string; content: string }
  | { type: 'ADD_TOOL_START'; sessionId: string; toolId: string; toolName: string; toolArgs: string }
  | { type: 'ADD_TOOL_RESULT'; sessionId: string; toolId: string; toolName: string; result: string; success: boolean }
  | { type: 'ADD_FILE_DIFF'; sessionId: string; path: string; diff: string };

const initialState: MainPanelState = {
  generatingSessions: {},
  thinkingChains: {},
  pendingInput: null,
  pendingComplete: null,
  pendingSystemMessage: null,
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
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: [],
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
      const { [action.sessionId]: _, ...restGen } = state.generatingSessions;
      // ── 内容完整性保障 ──
      // Done 事件的 fallbackContent 是 Rust 端 content_buffer 的完整克隆，
      // 比 React 流式分块累积（可能因批处理丢 chunk）更可靠。
      // 优先选更长的内容作为最终文本。
      const streamingLen = session.streamingContent?.length || 0;
      const fallbackLen = action.fallbackContent?.length || 0;
      const msgContent = fallbackLen >= streamingLen
        ? (action.fallbackContent || session.streamingContent || '')
        : (session.streamingContent || '');
      // reasoning 存在时也应创建 assistant 消息（即使没有文本内容）
      const chain = state.thinkingChains[action.sessionId] || [];
      const hasReasoning = chain.length > 0;
      const hasContent = !!msgContent || hasReasoning;
      // systemMessage：显式传入时始终使用；否则仅在无任何内容时使用默认提示
      const sysMsg = action.systemMessage
        || (!hasContent ? '⏱ 请求已终止：未收到模型有效响应。请重试或检查 API 配置。' : null);
      return {
        ...state,
        generatingSessions: restGen,
        pendingComplete: hasContent
          ? { sessionId: action.sessionId, content: msgContent || '' }
          : null,
        pendingSystemMessage: sysMsg
          ? { sessionId: action.sessionId, content: sysMsg }
          : state.pendingSystemMessage,
      };
    }

    case 'GENERATION_ERROR': {
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      const { [action.sessionId]: __, ...restChains } = state.thinkingChains;
      return { ...state, generatingSessions: rest, thinkingChains: restChains };
    }

    case 'STOP_GENERATION': {
      const session = state.generatingSessions[action.sessionId];
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      // 不在此清除 thinkingChains，交给 useLayoutEffect 统一处理（确保 reasoning 数据被持久化）
      return {
        ...state,
        generatingSessions: rest,
        pendingComplete: session?.streamingContent
          ? { sessionId: action.sessionId, content: session.streamingContent + '\n\n*(已停止生成)*' }
          : null,
        // 如果没有任何内容就被停止了，显示系统提示
        pendingSystemMessage: session?.streamingContent
          ? state.pendingSystemMessage
          : { sessionId: action.sessionId, content: '⏸ 已停止生成：模型尚未返回任何内容。' },
      };
    }

    case 'CLEAR_SESSION': {
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      const { [action.sessionId]: __, ...restChains } = state.thinkingChains;
      return { ...state, generatingSessions: rest, thinkingChains: restChains };
    }

    case 'CLEAR_GENERATING_SESSION': {
      const { [action.sessionId]: _, ...rest } = state.generatingSessions;
      const { [action.sessionId]: __, ...restChains } = state.thinkingChains;
      return { ...state, generatingSessions: rest, thinkingChains: restChains };
    }

    case 'CLEAR_PENDING_COMPLETE':
      return { ...state, pendingComplete: null };

    case 'CLEAR_PENDING_SYSTEM_MESSAGE':
      return { ...state, pendingSystemMessage: null };

    case 'SET_PENDING_INPUT':
      return { ...state, pendingInput: action.content };

    case 'APPEND_REASONING': {
      const chain = state.thinkingChains[action.sessionId] || [];
      // Merge with last reasoning step if it exists (streaming reasoning chunks)
      const last = chain[chain.length - 1];
      if (last && last.type === 'reasoning') {
        return {
          ...state,
          thinkingChains: {
            ...state.thinkingChains,
            [action.sessionId]: [
              ...chain.slice(0, -1),
              { ...last, content: (last.content || '') + action.content },
            ],
          },
        };
      }
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: [
            ...chain,
            {
              id: `reasoning-${Date.now()}`,
              type: 'reasoning' as const,
              content: action.content,
              timestamp: Date.now(),
            },
          ],
        },
      };
    }

    case 'ADD_TOOL_START': {
      const chain = state.thinkingChains[action.sessionId] || [];
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: [
            ...chain,
            {
              id: action.toolId,
              type: 'tool_start' as const,
              toolName: action.toolName,
              toolArgs: action.toolArgs,
              timestamp: Date.now(),
            },
          ],
        },
      };
    }

    case 'ADD_TOOL_RESULT': {
      const chain = state.thinkingChains[action.sessionId] || [];
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: [
            ...chain,
            {
              id: `result-${action.toolId}`,
              type: 'tool_result' as const,
              toolName: action.toolName,
              toolResult: action.result,
              toolSuccess: action.success,
              timestamp: Date.now(),
            },
          ],
        },
      };
    }

    case 'ADD_FILE_DIFF': {
      const chain = state.thinkingChains[action.sessionId] || [];
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: [
            ...chain,
            {
              id: `diff-${Date.now()}`,
              type: 'file_diff' as const,
              filePath: action.path,
              fileDiff: action.diff,
              timestamp: Date.now(),
            },
          ],
        },
      };
    }

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

  // ── 审批状态 ──
  const [approval, setApproval] = useState<{
    sessionId: string;
    callId: string;
    toolName: string;
    arguments: string;
    riskDescription: string;
    deadline: number; // 超时时间戳（ms）
  } | null>(null);

  // ── ask_user 确认状态（内嵌到最后一条 assistant 消息；提交/超时后即移除，不持久化）──
  const [confirmation, setConfirmation] = useState<{
    sessionId: string;
    callId: string;
    request: GroupChatConfirmationRequest;
  } | null>(null);
  const [confirmationCountdown, setConfirmationCountdown] = useState(0);
  const confirmationDeadlineRef = useRef(0);
  const confirmationSubmittedRef = useRef(false);

  // 确认等待倒计时：到 0 即移除确认块（后端 60s 超时返回后 tool_result 会兜底）
  useEffect(() => {
    if (!confirmation) return;
    const update = () => {
      const remaining = Math.max(0, Math.ceil((confirmationDeadlineRef.current - Date.now()) / 1000));
      setConfirmationCountdown(remaining);
      if (remaining <= 0) {
        setConfirmation(null);
        setConfirmationCountdown(0);
      }
    };
    update();
    const timer = setInterval(update, 250);
    return () => clearInterval(timer);
  }, [confirmation]);

  // ── 审批倒计时 ──
  const [countdown, setCountdown] = useState(120);
  useEffect(() => {
    if (!approval) {
      setCountdown(120);
      return;
    }
    const update = () => {
      const remaining = Math.max(0, Math.ceil((approval.deadline - Date.now()) / 1000));
      setCountdown(remaining);
    };
    update(); // 立即更新一次
    const timer = setInterval(update, 200);
    return () => clearInterval(timer);
  }, [approval]);

  // ── 迭代上限状态 ──
  const [iterLimit, setIterLimit] = useState<{
    sessionId: string;
    current: number;
    max: number;
    deadline: number;
  } | null>(null);

  // ── 迭代上限倒计时 ──
  const [iterCountdown, setIterCountdown] = useState(120);
  useEffect(() => {
    if (!iterLimit) {
      setIterCountdown(120);
      return;
    }
    const update = () => {
      const remaining = Math.max(0, Math.ceil((iterLimit.deadline - Date.now()) / 1000));
      setIterCountdown(remaining);
    };
    update();
    const timer = setInterval(update, 200);
    return () => clearInterval(timer);
  }, [iterLimit]);

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

  // 当前会话的思考链
  const currentThinkingChain = currentSessionId
    ? state.thinkingChains[currentSessionId] || []
    : [];

  // ── Agent Event handlers ──

  const onChunk = useCallback((sessionId: string, content: string) => {
    // 无条件写入对应会话的 streamingContent
    dispatch({ type: 'APPEND_CHUNK', sessionId, content });
  }, []);

  const onDone = useCallback((sessionId: string, fallbackContent?: string) => {
    console.log('[MainPanel] onDone received:', sessionId, 'fallbackLen:', fallbackContent?.length ?? 0);
    // 防止重复处理
    if (doneSessionIdsRef.current.has(sessionId)) {
      console.log('[MainPanel] onDone skipped (duplicate):', sessionId);
      return;
    }
    doneSessionIdsRef.current.add(sessionId);
    dispatch({ type: 'GENERATION_DONE', sessionId, fallbackContent });
  }, []);

  const onError = useCallback((sessionId: string, error: string) => {
    console.log('[MainPanel] onError received:', sessionId, error);
    // 只有当前可见会话的错误才弹 Toast
    if (sessionId === currentSessionId) {
      showToast(`错误: ${error}`, 'error');
    }

    // ── 通过 GENERATION_DONE 统一走 pendingComplete/pendingSystemMessage 机制 ──
    // 这样前端已累积的 streamingContent 和 thinkingChains（reasoning/tool_calls）会先
    // 被保存为 assistant 消息，然后紧跟着写入 system 错误通知。直接 addMessage +
    // GENERATION_ERROR 会先清除状态导致思考链数据丢失。
    dispatch({
      type: 'GENERATION_DONE',
      sessionId,
      systemMessage: `请求失败: ${error}`,
    });
  }, [currentSessionId]);

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

  // ── Approval handler: show dialog when tool needs user confirmation ──
  const onApprovalRequired = useCallback(
    (sessionId: string, callId: string, toolName: string, args: string, riskDescription: string) => {
      console.log('[MainPanel] onApprovalRequired:', { sessionId, callId, toolName, args, riskDescription });
      setApproval({ sessionId, callId, toolName, arguments: args, riskDescription, deadline: Date.now() + 120_000 });
    },
    [],
  );

  // ── 切换会话时清空 ask_user 确认块（确认属于原会话）──
  useEffect(() => {
    setConfirmation(null);
    setConfirmationCountdown(0);
  }, [currentSessionId]);

  // ── Iteration limit handler ──
  const onIterationLimit = useCallback(
    (sessionId: string, current: number, max: number) => {
      console.log('[MainPanel] onIterationLimit:', { sessionId, current, max });
      setIterLimit({ sessionId, current, max, deadline: Date.now() + 120_000 });
    },
    [],
  );

  // ── Tool event handlers: dispatch to thinkingChain instead of addMessage ──
  const onToolStart = useCallback(
    (sessionId: string, toolId: string, toolName: string, args: string) => {
      dispatch({ type: 'ADD_TOOL_START', sessionId, toolId, toolName, toolArgs: args });
    },
    [],
  );

  const onToolResult = useCallback(
    (sessionId: string, toolId: string, toolName: string, result: string, success: boolean) => {
      dispatch({ type: 'ADD_TOOL_RESULT', sessionId, toolId, toolName, result, success });
      // ask_user 工具已返回（回复注入或 60s 超时）：确认块使命结束，移除（不持久化）。
      setConfirmation((c) => {
        if (c && c.sessionId === sessionId && c.callId === toolId && !confirmationSubmittedRef.current) {
          return null;
        }
        return c;
      });
    },
    [],
  );

  // ── ask_user 确认请求：内嵌到最后一条 assistant 消息 ──
  const onConfirmationRequest = useCallback(
    (payload: {
      sessionId: string;
      callId: string;
      title?: string;
      prompt: string;
      replyMode: 'open' | 'structured';
      items: Array<{ id: string; label: string; inputType: 'text' | 'select' | 'confirm'; options: string[]; required: boolean; placeholder?: string }>;
    }) => {
      confirmationSubmittedRef.current = false;
      confirmationDeadlineRef.current = Date.now() + 60_000;
      setConfirmationCountdown(60);
      setConfirmation({
        sessionId: payload.sessionId,
        callId: payload.callId,
        request: {
          requestId: payload.callId,
          taskId: '',
          title: payload.title,
          prompt: payload.prompt,
          replyMode: payload.replyMode,
          items: payload.items,
        },
      });
    },
    [],
  );

  // ── Reasoning handler: dispatch reasoning chunks to thinkingChain ──
  const onReasoning = useCallback((sessionId: string, content: string) => {
    dispatch({ type: 'APPEND_REASONING', sessionId, content });
  }, []);

  // ── File diff handler: display write_file / edit_file changes ──
  const onFileDiff = useCallback((sessionId: string, path: string, diff: string) => {
    dispatch({ type: 'ADD_FILE_DIFF', sessionId, path, diff });
  }, []);

  const {
    sendChat,
    stopGeneration,
    createAgentSession,
    respondToApproval,
    respondToIterLimit,
    respondToConfirmation,
  } = useAgentEvent({ onChunk, onDone, onError, onSession, onApprovalRequired, onIterationLimit, onToolStart, onToolResult, onFileDiff, onReasoning, onConfirmationRequest });

  // ── Layout effect: atomically persist completed streaming content + system notification ──
  // 将 pendingComplete 和 pendingSystemMessage 合并写入，确保系统消息（超时/空响应）始终
  // 出现在 assistant 回复之后，而非之前。
  useLayoutEffect(() => {
    const hasPendingComplete = !!state.pendingComplete;
    const hasPendingSysMsg = !!state.pendingSystemMessage;
    if (!hasPendingComplete && !hasPendingSysMsg) return;

    // ── 处理 pendingComplete：assistant 正文 ──
    if (hasPendingComplete) {
      const { sessionId, content: msgContent } = state.pendingComplete!;

      // ── 在清除前从 thinkingChains 提取完整思维链（reasoning/tool/file_diff，保持步骤顺序）──
      const chain = state.thinkingChains[sessionId] || [];

      // 序列化完整思维链为 JSON 存入 tool_calls 列（该列现承载完整思维链）
      const toolCalls = chain.length > 0
        ? JSON.stringify(chain.map(s => ({
            id: s.id,
            type: s.type,
            content: s.type === 'reasoning' ? s.content : undefined,
            toolName: (s.type === 'tool_start' || s.type === 'tool_result') ? s.toolName : undefined,
            args: s.type === 'tool_start' ? s.toolArgs : undefined,
            result: s.type === 'tool_result' ? s.toolResult : undefined,
            success: s.type === 'tool_result' ? s.toolSuccess : undefined,
            filePath: s.type === 'file_diff' ? s.filePath : undefined,
            fileDiff: s.type === 'file_diff' ? s.fileDiff : undefined,
            ts: s.timestamp,
          })))
        : undefined;

      // 先清除 pendingComplete 防止重复执行
      dispatch({ type: 'CLEAR_PENDING_COMPLETE' });
      // 同步清除 generatingSessions 和 thinkingChains（React 18+ 批处理合并）
      dispatch({ type: 'CLEAR_GENERATING_SESSION', sessionId });

      // 即使 content 为空，只要有思维链也要持久化
      if (msgContent || toolCalls) {
        addMessage({
          id: `msg-${Date.now()}`,
          sessionId,
          role: 'assistant',
          content: msgContent || '',
          mode: 'native',
          timestamp: Math.floor(Date.now() / 1000),
          toolCalls,
        });
      }
    }

    // ── 处理 pendingSystemMessage：紧跟在 assistant 之后写入 ──
    if (hasPendingSysMsg) {
      const { sessionId, content } = state.pendingSystemMessage!;
      dispatch({ type: 'CLEAR_PENDING_SYSTEM_MESSAGE' });

      addMessage({
        id: `msg-sys-${Date.now()}`,
        sessionId,
        role: 'system',
        content,
        mode: 'native',
        // 延迟 1s 确保 timestamp 严格晚于刚写入的 assistant 消息
        timestamp: Math.floor(Date.now() / 1000) + 1,
      });
    }
  }, [state.pendingComplete, state.pendingSystemMessage, addMessage, dispatch]);

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

  // ── Safety net: auto-clear generating sessions stuck for > 180s
  // API Agent sessions with tool calls need multiple LLM round trips,
  // so 60s is too short. CLI agents are managed separately. ──
  const genStartTimesRef = useRef<Record<string, number>>({});
  useEffect(() => {
    const generatingIds = Object.keys(state.generatingSessions);
    if (generatingIds.length === 0) return;

    const timer = setInterval(() => {
      const now = Date.now();
      const staleIds = Object.keys(genStartTimesRef.current).filter(
        (sid) => generatingIds.includes(sid) && now - genStartTimesRef.current[sid] > 180_000,
      );
      for (const sid of staleIds) {
        console.warn('[MainPanel] 生成超时，自动清除 session:', sid);
        // 通过 GENERATION_DONE.systemMessage 统一走 pendingSystemMessage 机制持久化，
        // 避免与 useLayoutEffect 双重写入产生重复消息
        dispatch({ type: 'GENERATION_DONE', sessionId: sid, systemMessage: '⏱ 请求超时：模型响应时间超过 180 秒，已自动终止。请重试或检查 API 提供商状态。' });
      }
    }, 5_000);

    return () => clearInterval(timer);
  }, [Object.keys(state.generatingSessions).join(',')]);

  // ── Send / Stop handlers ──

  const handleSend = useCallback(
    async (message: string, mode: ChatMode, attachments?: Attachment[]) => {
      if (!currentSession) return;

      const sid = currentSession.id;

      // 清除当前会话的旧状态
      doneSessionIdsRef.current.delete(sid);

      // 记录生成开始时间（用于超时安全网）
      genStartTimesRef.current[sid] = Date.now();

      dispatch({ type: 'SEND_START', sessionId: sid, status: '发送中...' });

      // Save user message to store
      addMessage({
        id: `msg-${Date.now()}`,
        sessionId: sid,
        role: 'user',
        content: message,
        mode,
        timestamp: Math.floor(Date.now() / 1000),
        attachments: attachments && attachments.length > 0 ? attachments : undefined,
      });


      if (isApiSession(currentSession.agentType)) {
        // API Agent: 路由通过 Rust 后端 AgentLoop（替代前端直接 HTTP）
        if (!currentSession.apiProvider || !currentSession.apiModel) {
          showToast('API 会话缺少提供商或模型配置', 'error');
          dispatch({ type: 'GENERATION_ERROR', sessionId: sid, error: '缺少 API 配置' });
          return;
        }
        const systemPrompt = await getModePrompt(mode);
        sendChat(
          sid, message, mode, 'api',
          currentSession.cwd || undefined,
          systemPrompt,
          undefined,
          currentSession.temperature,
          currentSession.maxTokens,
          attachments,
        );
      } else {
        // Agent via Tauri Event
        const systemPrompt = await getModePrompt(mode);
        // Pass agent_session_id for session continuity (Claude Code --resume)
        const agentSessionId = currentSession.agentSessionId || undefined;
        sendChat(sid, message, mode, currentSession.agentType, currentSession.cwd || undefined, systemPrompt, agentSessionId, undefined, undefined, attachments);
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

  // ── Build display messages（按当前会话过滤，防止会话间串消息）──
  // 生成过程中始终创建 assistant 占位消息，即使 streamingContent 为空
  // 否则思考链（reasoning）在内容到达前没有挂载的 MessageBubble，会导致"卡死"假象
  const streamingMsg = currentGenState
    ? {
        role: 'assistant' as const,
        content: currentGenState.streamingContent || '',
        sessionId: currentSessionId || '',
        id: 'streaming',
        mode: 'native' as ChatMode,
        timestamp: Math.floor(Date.now() / 1000),
      }
    : null;

  const displayMessages = useMemo(() => {
    // 仅显示当前会话的消息（防御性过滤，正常情况下 selectSession 已保证）
    const sessionMsgs = messages.filter(m => m.sessionId === currentSessionId);
    if (streamingMsg) {
      return [...sessionMsgs, streamingMsg];
    }
    // 先按时间戳排序，系统消息保持在其实际发生位置
    const sorted = [...sessionMsgs].sort((a, b) => a.timestamp - b.timestamp);
    return sorted;
  }, [messages, currentSessionId, streamingMsg]);

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
          thinkingChain={currentThinkingChain}
          confirmation={confirmation ? {
            request: confirmation.request,
            countdown: confirmationCountdown,
            onSubmit: async (responses: GroupChatConfirmationResponseInput[]) => {
              confirmationSubmittedRef.current = true;
              const content = formatResponsesToText(confirmation.request.items, responses);
              await respondToConfirmation(confirmation.sessionId, confirmation.callId, content);
              // 提交后即移除确认块（会话模式不持久化）
              setConfirmation(null);
              setConfirmationCountdown(0);
            },
          } : null}
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

      {/* ── 高风险操作审批对话框（Portal 到 body 避免被 overflow-hidden 裁剪）── */}
      {approval && createPortal(
        <div className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/40">
          <div
            className="rounded-lg shadow-2xl w-[440px] max-h-[80vh] overflow-hidden flex flex-col"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border-primary)' }}
          >
            {/* Header */}
            <div
              className="px-5 py-3.5 flex items-center gap-3 shrink-0"
              style={{ borderBottom: '1px solid var(--border-primary)' }}
            >
              <div className="w-8 h-8 rounded-full flex items-center justify-center" style={{ backgroundColor: 'var(--warning-bg, rgba(234,179,8,0.15))' }}>
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--warning, #eab308)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
                  <line x1="12" y1="9" x2="12" y2="13" />
                  <line x1="12" y1="17" x2="12.01" y2="17" />
                </svg>
              </div>
              <div>
                <div className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>高风险操作确认</div>
                <div className="text-xs mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
                  {approval.riskDescription}
                </div>
              </div>
            </div>

            {/* Body */}
            <div className="px-5 py-4 overflow-y-auto">
              <div className="space-y-3">
                {/* Tool Name */}
                <div>
                  <div className="text-xs mb-1 font-medium" style={{ color: 'var(--text-tertiary)' }}>工具</div>
                  <div className="text-sm font-mono px-3 py-2 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}>
                    {approval.toolName}
                  </div>
                </div>

                {/* Arguments */}
                {approval.arguments && approval.arguments !== 'null' && (
                  <div>
                    <div className="text-xs mb-1 font-medium" style={{ color: 'var(--text-tertiary)' }}>参数</div>
                    <pre
                      className="text-xs font-mono px-3 py-2 rounded overflow-x-auto whitespace-pre-wrap break-all"
                      style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', maxHeight: '200px' }}
                    >
                      {(() => {
                        try {
                          return JSON.stringify(JSON.parse(approval.arguments), null, 2);
                        } catch {
                          return approval.arguments;
                        }
                      })()}
                    </pre>
                  </div>
                )}
              </div>
            </div>

            {/* Footer */}
            <div
              className="px-5 py-3.5 flex items-center justify-between shrink-0 gap-3"
              style={{ borderTop: '1px solid var(--border-primary)' }}
            >
              {/* 倒计时 */}
              <div className="flex items-center gap-1.5 text-xs" style={{ color: countdown <= 10 ? 'var(--danger, #ef4444)' : 'var(--text-tertiary)' }}>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <circle cx="12" cy="12" r="10" />
                  <polyline points="12 6 12 12 16 14" />
                </svg>
                <span>{countdown <= 0 ? '已超时' : `${countdown} 秒后默认允许`}</span>
              </div>
              <div className="flex gap-3">
                <button
                  className="px-4 py-2 text-sm rounded-md font-medium transition-colors hover:opacity-80"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-secondary)',
                  }}
                  onClick={() => {
                    respondToApproval(approval.sessionId, approval.callId, false);
                    setApproval(null);
                  }}
                >
                  拒绝
                </button>
                <button
                  className="px-4 py-2 text-sm rounded-md font-medium transition-colors hover:opacity-80"
                  style={{
                    backgroundColor: 'var(--accent, #7c3aed)',
                    color: '#fff',
                  }}
                  onClick={() => {
                    respondToApproval(approval.sessionId, approval.callId, true);
                    setApproval(null);
                  }}
                >
                  允许执行
                </button>
              </div>
            </div>
          </div>
        </div>
      , document.body)}

      {/* ── 迭代上限确认对话框（Portal 到 body）── */}
      {iterLimit && createPortal(
        <div className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/40">
          <div
            className="rounded-lg shadow-2xl w-[400px] overflow-hidden flex flex-col"
            style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border-primary)' }}
          >
            {/* Header */}
            <div
              className="px-5 py-3.5 flex items-center gap-3 shrink-0"
              style={{ borderBottom: '1px solid var(--border-primary)' }}
            >
              <div className="w-8 h-8 rounded-full flex items-center justify-center" style={{ backgroundColor: 'var(--info-bg, rgba(59,130,246,0.15))' }}>
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--info, #3b82f6)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="1 4 1 10 7 10" />
                  <path d="M3.51 15a9 9 0 102.13-9.36L1 10" />
                </svg>
              </div>
              <div>
                <div className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>已达到迭代上限</div>
                <div className="text-xs mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
                  当前 {iterLimit.current} / {iterLimit.max} 轮，是否继续执行？
                </div>
              </div>
            </div>

            {/* Footer */}
            <div
              className="px-5 py-3.5 flex items-center justify-between shrink-0 gap-3"
              style={{ borderTop: '1px solid var(--border-primary)' }}
            >
              {/* 倒计时 */}
              <div className="flex items-center gap-1.5 text-xs" style={{ color: iterCountdown <= 10 ? 'var(--danger, #ef4444)' : 'var(--text-tertiary)' }}>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <circle cx="12" cy="12" r="10" />
                  <polyline points="12 6 12 12 16 14" />
                </svg>
                <span>{iterCountdown <= 0 ? '已超时' : `${iterCountdown} 秒后默认继续`}</span>
              </div>
              <div className="flex gap-3">
                <button
                  className="px-4 py-2 text-sm rounded-md font-medium transition-colors hover:opacity-80"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-secondary)',
                  }}
                  onClick={() => {
                    respondToIterLimit(iterLimit.sessionId, false);
                    setIterLimit(null);
                  }}
                >
                  终止
                </button>
                <button
                  className="px-4 py-2 text-sm rounded-md font-medium transition-colors hover:opacity-80"
                  style={{
                    backgroundColor: 'var(--accent, #7c3aed)',
                    color: '#fff',
                  }}
                  onClick={() => {
                    respondToIterLimit(iterLimit.sessionId, true);
                    setIterLimit(null);
                  }}
                >
                  继续（+15 轮）
                </button>
              </div>
            </div>
          </div>
        </div>
      , document.body)}
    </div>
  );
}
