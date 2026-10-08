import { useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Users, Workflow } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { MessageList } from '../message/MessageList';
import { SessionToWorkflowModal } from '../session/SessionToWorkflowModal';
import { SessionToRoomModal } from '../session/SessionToRoomModal';
import { InputBar } from './InputBar';
import { CommandCenter } from './CommandCenter';
import type { SecurityModeValue } from '../security/SecurityModeSelector';
import { useSessionStore } from '../../stores/sessionStore';
import { useGroupChatStore } from '../../stores/groupChatStore';
import { useTerminal } from '../../TerminalManager';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { usePendingInputStore } from '../../stores/pendingInputStore';
import { useSessionDraftStore } from '../../stores/sessionDraftStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { useGeneratingStore } from '../../stores/generatingStore';
import { useAgentStreamStore, appendReasoningStep, appendToolStartStep, appendToolResultStep, appendFileDiffStep, appendSystemStep, appendApprovalStep, resolveApprovalStep, appendIterationLimitStep, resolveIterationLimitStep } from '../../stores/agentStreamStore';
import type { SessionStreamState } from '../../stores/agentStreamStore';
import { useApprovalStore, selectApprovalItems, selectIterationLimitItems } from '../../stores/approvalStore';
import type { IterationLimitItem } from '../../stores/approvalStore';
import { formatResponsesToText } from '../confirmation/confirmationUtils';

import type { ChatMode, Attachment, Session } from '../../types';
import type { GroupChatConfirmationRequest, GroupChatConfirmationResponseInput } from '../../types/groupchat';
import { getModePrompt } from '../../types';
import { isApiSession } from '../../utils/sessionType';

// ── State Machine ──

interface SessionGenerationState {
  streamingContent: string;
  streamingStatus: string;
  /** 工具执行进度（如 generate_video 任务创建/轮询状态），在流式助手消息气泡内展示，生成结束后不落库 */
  streamingProgress: string;
}

/** A single step in the thinking chain (reasoning or tool call) */
export interface ThinkingChainStep {
  id: string;
  type: 'reasoning' | 'tool_start' | 'tool_result' | 'file_diff' | 'approval' | 'iteration_limit' | 'system';
  content?: string;
  toolName?: string;
  toolArgs?: string;
  toolResult?: string;
  toolSuccess?: boolean;
  filePath?: string;
  fileDiff?: string;
  /** 内联审批卡：待决工具调用 id（与后端 call_id 一致） */
  approvalCallId?: string;
  /** 内联审批卡：用户决策结果（undefined 表示未决） */
  approvalApproved?: boolean;
  /** 内联审批卡：是否由超时自动裁决 */
  approvalTimedOut?: boolean;
  /** 内联审批卡：风险等级描述（复用后端 RiskLevel::description） */
  approvalRisk?: string;
  /** 内联审批卡：等待截止时间戳（ms） */
  approvalDeadline?: number;
  /** 内联迭代上限卡：已达成的迭代轮数 */
  iterCurrent?: number;
  /** 内联迭代上限卡：当前迭代上限 */
  iterMax?: number;
  /** 内联迭代上限卡：稳定标识（与审批集合 IterationLimitItem.id 一致，供回填去重） */
  iterId?: string;
  /** 内联迭代上限卡：等待截止时间戳（ms） */
  iterDeadline?: number;
  /** 内联迭代上限卡：用户决策结果（undefined 表示未决） */
  iterDecision?: 'continue' | 'stop';
  /** 内联迭代上限卡：是否由超时自动裁决 */
  iterTimedOut?: boolean;
  /** 系统步骤（轮前框架步骤，如自动记忆检索）：折叠行标题 */
  sysTitle?: string;
  /** 系统步骤：参数行（该步骤实际使用的模型与解析结果） */
  sysParams?: string;
  /** 系统步骤：展开后的明细（含失败原因） */
  sysDetail?: string;
  /** 系统步骤：是否正常完成 */
  sysSuccess?: boolean;
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
  | { type: 'SET_STREAMING_STATUS'; sessionId: string; status: string }
  | { type: 'SET_STREAMING_PROGRESS'; sessionId: string; content: string }
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
  | { type: 'ADD_FILE_DIFF'; sessionId: string; path: string; diff: string }
  | { type: 'ADD_APPROVAL'; sessionId: string; callId: string; toolName: string; toolArgs: string; risk: string; deadline: number }
  | { type: 'RESOLVE_APPROVAL'; sessionId: string; callId: string; approved: boolean; timedOut: boolean }
  | { type: 'ADD_ITERATION_LIMIT'; sessionId: string; current: number; max: number; id: string; deadline: number }
  | { type: 'RESOLVE_ITERATION_LIMIT'; sessionId: string; decision: 'continue' | 'stop'; timedOut: boolean }
  | { type: 'HYDRATE_APPROVALS'; sessionId: string; items: Array<{ callId: string; toolName: string; args: string; risk: string; deadline: number; ts: number; approved?: boolean; timedOut?: boolean }> }
  | { type: 'HYDRATE_ITERATION_LIMITS'; sessionId: string; items: IterationLimitItem[] }
  | { type: 'ADD_SYSTEM_STEP'; sessionId: string; stepId: string; title: string; params: string; detail: string; success: boolean }
  | { type: 'HYDRATE_STREAM'; sessionId: string; content: string; steps: ThinkingChainStep[] };

const initialState: MainPanelState = {
  generatingSessions: {},
  thinkingChains: {},
  pendingInput: null,
  pendingComplete: null,
  pendingSystemMessage: null,
};

// ── 会话安全网（"静默卡死"判定）──

/**
 * 多久没有任何流式事件才判定为卡死。
 * 上游假死（长时间无数据块）由后端 90s 流式空闲检测兜底；这里是"后端也没动静"的第二道网，
 * 所以取得比它宽。判定对象是静默时长，不是整轮耗时 —— 长任务只要仍在推进就不受此限。
 */
const STREAM_IDLE_TIMEOUT_MS = 180_000;

/** 发起静默取消后，等后端 agent-cancelled 回执的兜底时长；超时则前端自行收尾 */
const CANCEL_FALLBACK_MS = 15_000;

/** 静默卡死终止的用户可见说明（与"上游超时""用户停止"区分开） */
function idleTimeoutNotice(): string {
  return `⏱ 长时间无响应：已停止本轮（超过 ${STREAM_IDLE_TIMEOUT_MS / 1000} 秒没有任何进展）。请重试或检查 API 提供商状态。`;
}

/**
 * 是否有工具调用在跑（收到 tool_start 但还没有对应的 tool_result）。
 * 工具自带超时（兜底 60s / execute_* 600s / generate_video 15min），期间静默属正常，不该判卡死。
 */
function hasRunningTool(stream: SessionStreamState | undefined): boolean {
  if (!stream) return false;
  const finished = new Set(
    stream.steps.filter((step) => step.type === 'tool_result').map((step) => step.id),
  );
  return stream.steps.some((step) => step.type === 'tool_start' && !finished.has(`result-${step.id}`));
}

function reducer(state: MainPanelState, action: Action): MainPanelState {
  switch (action.type) {
    case 'SEND_START':
      return {
        ...state,
        generatingSessions: {
          ...state.generatingSessions,
          [action.sessionId]: { streamingContent: '', streamingStatus: action.status, streamingProgress: '' },
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

    case 'SET_STREAMING_STATUS': {
      const session = state.generatingSessions[action.sessionId];
      if (!session) return state;
      return {
        ...state,
        generatingSessions: {
          ...state.generatingSessions,
          [action.sessionId]: { ...session, streamingStatus: action.status },
        },
      };
    }

    case 'SET_STREAMING_PROGRESS': {
      const session = state.generatingSessions[action.sessionId];
      if (!session) return state;
      return {
        ...state,
        generatingSessions: {
          ...state.generatingSessions,
          [action.sessionId]: { ...session, streamingProgress: action.content },
        },
      };
    }

    case 'GENERATION_DONE': {
      const session = state.generatingSessions[action.sessionId];
      if (!session) return state;
      const { [action.sessionId]: _, ...restGen } = state.generatingSessions;
      // reasoning 存在时也应创建 assistant 消息（即使没有文本内容）
      const chain = state.thinkingChains[action.sessionId] || [];
      const hasContent = !!(session.streamingContent || chain.length > 0);
      // systemMessage：显式传入时始终使用；否则仅在无任何内容时使用默认提示
      const sysMsg = action.systemMessage
        || (!hasContent ? '⏱ 请求已终止：未收到模型有效响应。请重试或检查 API 配置。' : null);
      return {
        ...state,
        generatingSessions: restGen,
        // assistant 正文与思维链的落库已上移到 agentStreamStore（事件总线在收到 done 时直接写库），
        // 组件不再持有 pendingComplete —— 否则组件挂载与否会决定回答是否被保存。
        pendingComplete: null,
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
      // 步骤构造与 agentStreamStore 共用同一份纯函数：渲染链与落库链必须逐字一致
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: appendReasoningStep(chain, action.content),
        },
      };
    }

    case 'ADD_TOOL_START': {
      const chain = state.thinkingChains[action.sessionId] || [];
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: appendToolStartStep(chain, action.toolId, action.toolName, action.toolArgs),
        },
      };
    }

    case 'ADD_TOOL_RESULT': {
      const chain = state.thinkingChains[action.sessionId] || [];
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: appendToolResultStep(chain, action.toolId, action.toolName, action.result, action.success),
        },
      };
    }

    case 'ADD_FILE_DIFF': {
      const chain = state.thinkingChains[action.sessionId] || [];
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: appendFileDiffStep(chain, action.path, action.diff),
        },
      };
    }

    case 'ADD_SYSTEM_STEP': {
      const chain = state.thinkingChains[action.sessionId] || [];
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: appendSystemStep(chain, action.stepId, action.title, action.params, action.detail, action.success),
        },
      };
    }

    case 'HYDRATE_STREAM': {
      // 组件卸载期间到达的流式内容与思维链在总线层累积于 agentStreamStore：
      // 重新进入该会话时回填，避免"切走再回来，已输出的内容消失"。
      // 已在跟踪的会话不重复回填（首个 chunk 到达后由实时事件接管）。
      if (state.generatingSessions[action.sessionId]) return state;
      if (!action.content && action.steps.length === 0) return state;
      return {
        ...state,
        generatingSessions: {
          ...state.generatingSessions,
          [action.sessionId]: { streamingContent: action.content, streamingStatus: '', streamingProgress: '' },
        },
        thinkingChains: { ...state.thinkingChains, [action.sessionId]: action.steps },
      };
    }

    case 'ADD_APPROVAL': {
      const chain = state.thinkingChains[action.sessionId] || [];
      // 步骤构造与 agentStreamStore 共用同一份纯函数：渲染链与落库链必须逐字一致
      // （去重也在构造器内，重复收到同一审批请求时返回原链）。
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: appendApprovalStep(chain, action.callId, action.toolName, action.toolArgs, action.risk, action.deadline),
        },
      };
    }

    case 'RESOLVE_APPROVAL': {
      const chain = state.thinkingChains[action.sessionId];
      if (!chain) return state;
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: resolveApprovalStep(chain, action.callId, action.approved, action.timedOut),
        },
      };
    }

    case 'ADD_ITERATION_LIMIT': {
      const chain = state.thinkingChains[action.sessionId] || [];
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: appendIterationLimitStep(chain, action.id, action.current, action.max, action.deadline),
        },
      };
    }

    case 'RESOLVE_ITERATION_LIMIT': {
      const chain = state.thinkingChains[action.sessionId];
      if (!chain) return state;
      return {
        ...state,
        thinkingChains: {
          ...state.thinkingChains,
          [action.sessionId]: resolveIterationLimitStep(chain, action.decision, action.timedOut),
        },
      };
    }

    case 'HYDRATE_APPROVALS': {
      // 会话激活时回填：把审批集合中该会话尚未出现在链条上的审批项补进来
      // （覆盖 MainPanel 卸载期间——如用户切到群聊页——到达的审批请求）。
      const chain = state.thinkingChains[action.sessionId] || [];
      const missing = action.items.filter(
        (it) => !chain.some((s) => s.type === 'approval' && s.approvalCallId === it.callId),
      );
      if (missing.length === 0) return state;
      const restored: ThinkingChainStep[] = missing.map((it) => ({
        id: `approval-${it.callId}`,
        type: 'approval' as const,
        toolName: it.toolName,
        toolArgs: it.args,
        approvalCallId: it.callId,
        approvalRisk: it.risk,
        approvalDeadline: it.deadline,
        approvalApproved: it.approved,
        approvalTimedOut: it.timedOut,
        timestamp: it.ts,
      }));
      // 稳定排序按时间戳合并（等值时保持原有相对顺序），尽量还原审批在链条中的真实位置。
      const merged = [...chain, ...restored].sort((a, b) => a.timestamp - b.timestamp);
      return {
        ...state,
        thinkingChains: { ...state.thinkingChains, [action.sessionId]: merged },
      };
    }

    case 'HYDRATE_ITERATION_LIMITS': {
      // 会话激活时回填：把迭代上限集合中该会话尚未出现在链条上的项补进来
      // （覆盖 MainPanel 卸载期间——如用户切到群聊页——到达的请求）。
      const chain = state.thinkingChains[action.sessionId] || [];
      const missing = action.items.filter(
        (it) => !chain.some((s) => s.type === 'iteration_limit' && s.iterId === it.id),
      );
      if (missing.length === 0) return state;
      const restored: ThinkingChainStep[] = missing.map((it) => ({
        id: `iteration-limit-${it.id}`,
        type: 'iteration_limit' as const,
        iterId: it.id,
        iterCurrent: it.current,
        iterMax: it.max,
        iterDeadline: it.deadline,
        iterDecision: it.decision,
        iterTimedOut: it.timedOut,
        timestamp: it.ts,
      }));
      // 稳定排序按时间戳合并（等值时保持原有相对顺序），尽量还原迭代上限在链条中的真实位置。
      const merged = [...chain, ...restored].sort((a, b) => a.timestamp - b.timestamp);
      return {
        ...state,
        thinkingChains: { ...state.thinkingChains, [action.sessionId]: merged },
      };
    }

    default:
      return state;
  }
}

// ── Component ──

/** 会话默认页顶部的按时段问候：<12 上午 / <18 下午 / 其余晚上。 */
function greetingByHour(hour: number): string {
  if (hour < 12) return '上午好';
  if (hour < 18) return '下午好';
  return '晚上好';
}

export function MainPanel({ style }: { style?: React.CSSProperties } = {}) {
  
  
  const {
    currentSessionId,
    sessions,
    archivedSessions,
    messages,
    isLoadingMessages,
    addMessage,
    createSession,
    selectSession,
    startNewSession,
  } = useSessionStore();

  const [state, dispatch] = useReducer(reducer, initialState);
  const navigate = useNavigate();
  const { setMode } = useTerminal();

  // ── 会话 → 工作流：预览弹窗开关（仅当前会话可用）──
  const [showWorkflowModal, setShowWorkflowModal] = useState(false);
  // ── 会话 → 群聊：预览/成员配置弹窗开关（仅当前会话可用）──
  const [showRoomModal, setShowRoomModal] = useState(false);

  // ── 会话安全模式（本消息有效；默认标准，随发送传参，不持久化）──
  const [securityMode, setSecurityMode] = useState<SecurityModeValue>('standard');

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

  // 已完成的会话 ID 集合（防止重复处理）
  const doneSessionIdsRef = useRef<Set<string>>(new Set());
  const createdSessionsRef = useRef<Set<string>>(new Set());
  /** 本轮起点（发送时刻），作为"还没有任何事件"时的静默基准 */
  const genStartTimesRef = useRef<Record<string, number>>({});
  /** 已发起"静默取消"的会话 → 发起时间戳；等后端 agent-cancelled 收尾，超时则前端自行收尾 */
  const idleCancelRef = useRef<Record<string, number>>({});

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
    delete genStartTimesRef.current[sessionId];
    delete idleCancelRef.current[sessionId];
    dispatch({ type: 'GENERATION_DONE', sessionId, fallbackContent });
  }, []);

  const onCancelled = useCallback((sessionId: string) => {
    // 用户点"停止生成"：正文已由事件总线按部分内容收尾落库（agentStreamStore.finishRun），
    // 组件只需收起流式视图。复用错误路径的清理动作（清 generatingSessions 与思维链，不写任何消息）。
    // 若这次取消是本组件的静默安全网发起的，补一条说明 —— 否则用户不知道这轮为什么停了。
    const idleRequested = idleCancelRef.current[sessionId] !== undefined;
    delete genStartTimesRef.current[sessionId];
    delete idleCancelRef.current[sessionId];
    if (idleRequested) {
      dispatch({ type: 'GENERATION_DONE', sessionId, systemMessage: idleTimeoutNotice() });
      return;
    }
    dispatch({ type: 'GENERATION_ERROR', sessionId, error: '' });
  }, []);

  const onError = useCallback((sessionId: string, error: string) => {
    console.log('[MainPanel] onError received:', sessionId, error);
    delete genStartTimesRef.current[sessionId];
    delete idleCancelRef.current[sessionId];
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

  // ── Approval handlers: 内联审批卡（写入思维链条；审批集合同步在事件总线层完成，
  //    以便 MainPanel 卸载期间也能保留，见 approvalStore / agentEventBus）──
  const onApprovalRequired = useCallback(
    (sessionId: string, callId: string, toolName: string, args: string, riskDescription: string) => {
      const deadline = Date.now() + 120_000;
      dispatch({ type: 'ADD_APPROVAL', sessionId, callId, toolName, toolArgs: args, risk: riskDescription, deadline });
    },
    [],
  );

  const onApprovalResolved = useCallback(
    (sessionId: string, callId: string, _toolName: string, approved: boolean, timedOut: boolean) => {
      dispatch({ type: 'RESOLVE_APPROVAL', sessionId, callId, approved, timedOut });
    },
    [],
  );

  // ── 切换会话时清空 ask_user 确认块（确认属于原会话）──
  // 用「渲染期修正」而不是 effect：在 effect 里同步 setState 会多一轮级联渲染
  // （`react-hooks/set-state-in-effect`），而且会在切会话那一帧先渲染出旧确认块。
  const [confirmationSessionId, setConfirmationSessionId] = useState(currentSessionId);
  if (confirmationSessionId !== currentSessionId) {
    setConfirmationSessionId(currentSessionId);
    setConfirmation(null);
    setConfirmationCountdown(0);
    // 转换预览同样属于原会话：切走即关闭，避免预览与当前会话脱节
    setShowWorkflowModal(false);
  }

  // ── 会话激活时回填内联卡（审批 + 迭代上限）：覆盖 MainPanel 卸载期间
  //    （如切到群聊页）到达的请求 ──
  useEffect(() => {
    if (!currentSessionId) return;
    const items = selectApprovalItems(currentSessionId)(useApprovalStore.getState());
    if (items.length > 0) {
      dispatch({ type: 'HYDRATE_APPROVALS', sessionId: currentSessionId, items });
    }
    const iterItems = selectIterationLimitItems(currentSessionId)(useApprovalStore.getState());
    if (iterItems.length > 0) {
      dispatch({ type: 'HYDRATE_ITERATION_LIMITS', sessionId: currentSessionId, items: iterItems });
    }
  }, [currentSessionId]);

  // ── Iteration limit handlers：内联迭代上限卡（写入思维链条，与审批卡同构）──
  const onIterationLimit = useCallback(
    (sessionId: string, current: number, max: number, id: string, deadline: number) => {
      console.log('[MainPanel] onIterationLimit:', { sessionId, current, max });
      dispatch({ type: 'ADD_ITERATION_LIMIT', sessionId, current, max, id, deadline });
    },
    [],
  );

  const onIterationLimitResolved = useCallback(
    (sessionId: string, shouldContinue: boolean, timedOut: boolean) => {
      dispatch({ type: 'RESOLVE_ITERATION_LIMIT', sessionId, decision: shouldContinue ? 'continue' : 'stop', timedOut });
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

  // ── 工具执行进度（generate_video 等长耗时工具）：写入流式助手消息气泡（生成结束后不落库）──
  const onToolProgress = useCallback(
    (sessionId: string, _toolName: string, message: string) => {
      dispatch({ type: 'SET_STREAMING_PROGRESS', sessionId, content: message });
      // 同步更新状态文本（保留既有机制，供其他状态展示消费）
      dispatch({ type: 'SET_STREAMING_STATUS', sessionId, status: message });
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

  // ── 轮前系统步骤（如自动记忆检索）：不是模型发起的工具调用，独立成一步 ──
  const onSystemStep = useCallback((sessionId: string, stepId: string, title: string, params: string, detail: string, success: boolean) => {
    dispatch({ type: 'ADD_SYSTEM_STEP', sessionId, stepId, title, params, detail, success });
  }, []);

  const {
    sendChat,
    stopGeneration,
    createAgentSession,
    respondToConfirmation,
  } = useAgentEvent({ onChunk, onDone, onCancelled, onError, onSession, onApprovalRequired, onApprovalResolved, onIterationLimit, onIterationLimitResolved, onToolStart, onToolResult, onToolProgress, onFileDiff, onSystemStep, onReasoning, onConfirmationRequest });

  // ── Layout effect: atomically persist completed streaming content + system notification ──
  // assistant 正文与思维链的落库已上移到 agentStreamStore（事件总线在收到 done/cancelled 时
  // 直接写库，不依赖组件是否挂载）；此处只保留系统提示消息（超时/空响应）的写入，
  // 它需要紧跟在本轮 assistant 之后，故仍在组件内按 pending 状态落库。
  useLayoutEffect(() => {
    const hasPendingSysMsg = !!state.pendingSystemMessage;
    if (!hasPendingSysMsg) return;

    // ── 处理 pendingSystemMessage：紧跟在 assistant 之后写入 ──
    {
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
  }, [state.pendingSystemMessage, addMessage, dispatch]);

  // ── 重新进入会话时回填"卸载期间已输出的内容" ──
  // 事件总线在 MainPanel 卸载期间照常把 chunk/思维链写入 agentStreamStore（单例），
  // 这里把它回填进渲染状态；只在会话尚未被本组件跟踪时生效。
  const streamContent = useAgentStreamStore((s) => s.streams[currentSessionId ?? '']?.content ?? '');
  const streamSteps = useAgentStreamStore((s) => s.streams[currentSessionId ?? '']?.steps);
  useEffect(() => {
    if (!currentSessionId || !streamSteps) return;
    dispatch({ type: 'HYDRATE_STREAM', sessionId: currentSessionId, content: streamContent, steps: streamSteps });
  }, [currentSessionId, streamContent, streamSteps]);

  // ── 列表脉冲以后端运行态为准 ──
  // 后端运行记录随进程存亡，前端组件内的临时集合会随卸载丢失；进入会话页时对齐一次。
  useEffect(() => {
    void useGeneratingStore.getState().syncFromBackend();
  }, [currentSessionId]);

  // ── Side effect: ensure Rust backend session exists for Agent sessions ──

  useEffect(() => {
    if (currentSessionId && currentAgentType && currentAgentType !== 'api') {
      if (!createdSessionsRef.current.has(currentSessionId)) {
        createdSessionsRef.current.add(currentSessionId);
        createAgentSession(currentSessionId, currentAgentType);
      }
    }
  }, [currentSessionId, currentAgentType, createAgentSession]);

  // ── Side effect: 本组件跟踪的运行会话 → 点亮列表脉冲 ──
  // 只做"标记"不做"覆盖"：权威来源是后端运行态注册表（syncFromBackend 对齐，
  // done/error/cancelled 时由 agentStreamStore 移除）。组件卸载会丢失本地集合，
  // 覆盖式同步会把仍在后台运行的其它会话一并抹掉。
  useEffect(() => {
    const store = useGeneratingStore.getState();
    Object.keys(state.generatingSessions).forEach((id) => store.markGenerating(id));
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

  // ── 安全网：长时间没有任何流式事件才判定卡死，并真正取消后端 ──
  // 判定的是"静默时长"而非"整轮时长"：多轮 LLM 往返 + 长工具链只要仍在产出事件就不算卡死
  //（旧实现从发送起算固定 180s，会把正常的长任务一并杀掉）。两类正常静默另行豁免：
  // 工具调用在跑（工具自带超时：兜底 60s / execute_* 600s / generate_video 15min）、
  // 等待用户决策（审批卡 / 迭代上限卡 / ask_user 确认块）。
  useEffect(() => {
    const generatingIds = Object.keys(state.generatingSessions);
    if (generatingIds.length === 0) return;

    const timer = setInterval(() => {
      const now = Date.now();
      const streams = useAgentStreamStore.getState().streams;
      const approvals = useApprovalStore.getState();
      for (const sid of generatingIds) {
        const requestedAt = idleCancelRef.current[sid];
        if (requestedAt !== undefined) {
          if (now - requestedAt > CANCEL_FALLBACK_MS) {
            // 后端没回 agent-cancelled（进程卡死等）：前端自行收尾，别一直挂在"生成中"
            delete idleCancelRef.current[sid];
            console.warn('[MainPanel] 静默取消未收到后端回执，前端自行收尾:', sid);
            dispatch({ type: 'GENERATION_DONE', sessionId: sid, systemMessage: idleTimeoutNotice() });
          }
          continue;
        }

        const lastActivity = Math.max(
          genStartTimesRef.current[sid] ?? 0,
          useGeneratingStore.getState().lastEventAt[sid] ?? 0,
        );
        // 本组件既没有本轮起点、也没有任何事件记录（如别处发起的运行）：无从判断，不动它
        if (lastActivity === 0) continue;
        if (now - lastActivity <= STREAM_IDLE_TIMEOUT_MS) continue;
        if (hasRunningTool(streams[sid])) continue;
        const awaitingUser =
          (approvals.items[sid] ?? []).some((item) => item.approved === undefined)
          || (approvals.iterationLimits[sid] ?? []).some((item) => item.decision === undefined)
          || confirmation?.sessionId === sid;
        if (awaitingUser) continue;

        console.warn(
          `[MainPanel] 流式静默 ${Math.round((now - lastActivity) / 1000)}s 无任何事件，取消本轮:`,
          sid,
        );
        idleCancelRef.current[sid] = now;
        // 真取消后端：由 agent-cancelled 触发收尾落库，避免"前端判定已终止、后端仍在跑"的分叉
        void stopGeneration(sid);
      }
    }, 5_000);

    return () => clearInterval(timer);
    // 依赖刻意只取"会话 id 集合"（joined 字符串），不取 `state.generatingSessions` 本身：
    // 后者每个流式 token 都会换一个新对象，effect 会被高频重建，5 秒看门狗等于永远不会触发。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [Object.keys(state.generatingSessions).join(','), stopGeneration, confirmation]);

  // ── Send / Stop handlers ──

  const handleSend = useCallback(
    async (message: string, mode: ChatMode, attachments?: Attachment[]): Promise<boolean> => {
      // 会话可能刚由 InputBar 的「快捷开始」创建（selectSession 已写入 store），
      // 这里读**最新**的当前会话，而不是本次渲染闭包里的值。
      const store = useSessionStore.getState();
      const sess =
        store.sessions.find((s) => s.id === store.currentSessionId) ??
        store.archivedSessions.find((s) => s.id === store.currentSessionId);
      if (!sess) {
        showToast('会话未就绪，请重试', 'error');
        return false;
      }

      const sid = sess.id;

      // 清除当前会话的旧状态
      doneSessionIdsRef.current.delete(sid);

      // 记录生成开始时间（静默安全网的基准；随后每个流式事件都会刷新活动时间）
      genStartTimesRef.current[sid] = Date.now();
      // 新一轮开始，清掉上一轮遗留的静默取消标记
      delete idleCancelRef.current[sid];

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


      if (isApiSession(sess.agentType)) {
        // API Agent: 路由通过 Rust 后端 AgentLoop（替代前端直接 HTTP）
        if (!sess.apiProvider || !sess.apiModel) {
          showToast('API 会话缺少提供商或模型配置', 'error');
          dispatch({ type: 'GENERATION_ERROR', sessionId: sid, error: '缺少 API 配置' });
          return false;
        }
        const systemPrompt = await getModePrompt(mode);
        sendChat(
          sid, message, mode, 'api',
          sess.cwd || undefined,
          systemPrompt,
          undefined,
          sess.temperature,
          sess.maxTokens,
          attachments,
          securityMode,
        );
      } else {
        // Agent via Tauri Event
        const systemPrompt = await getModePrompt(mode);
        // Pass agent_session_id for session continuity (Claude Code --resume)
        const agentSessionId = sess.agentSessionId || undefined;
        sendChat(sid, message, mode, sess.agentType, sess.cwd || undefined, systemPrompt, agentSessionId, undefined, undefined, attachments, securityMode);
      }
      return true;
    },
    [sendChat, addMessage, securityMode],
  );

  /**
   * 「快捷开始」：无会话时按草稿（工作目录 + 会话方式）创建并选中会话。
   * 供 InputBar 在首次发送时调用；失败返回 null（InputBar 据此保留输入内容）。
   *
   * **幂等**：若当前会话就是上一次"快捷开始"刚建出来的那个，直接复用 —— 否则连按 Enter /
   * 重复触发会建出第二个会话并把 currentSessionId 切走，第一轮的回复就会落到"非当前会话"，
   * 表现为"助手回复不渲染、刷新后才可见"。
   */
  const autoCreatedSessionRef = useRef<string | null>(null);
  const ensureSession = useCallback(async (): Promise<Session | null> => {
    const store = useSessionStore.getState();
    if (store.currentSessionId && store.currentSessionId === autoCreatedSessionRef.current) {
      const existing = store.sessions.find((s) => s.id === store.currentSessionId);
      if (existing) return existing;
    }
    const params = useSessionDraftStore.getState().resolve();
    if (!params) {
      showToast('请先选择会话工作目录与会话方式', 'info');
      return null;
    }
    try {
      const session = await createSession(
        params.agentType,
        params.cwd || undefined,
        null,
        params.apiProvider || undefined,
        params.apiModel || undefined,
      );
      autoCreatedSessionRef.current = session.id;
      // CLI 会话还要把 sidecar 会话拉起来，否则首条消息发不出去
      if (session.agentType !== 'api') {
        await createAgentSession(session.id, session.agentType, params.cwd || undefined);
      }
      await selectSession(session.id);
      return session;
    } catch (err) {
      showToast(`创建会话失败: ${errorMessage(err)}`, 'error');
      return null;
    }
  }, [createSession, createAgentSession, selectSession]);


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

  const handleResendMessage = useCallback((content: string) => {
    dispatch({ type: 'SET_PENDING_INPUT', content });
  }, []);

  // ── 会话 → 工作流：创建成功后关闭弹窗并跳转工作流编辑器（与群聊页 handlePromote 一致）──
  const handleWorkflowCreated = useCallback((definitionId: string) => {
    setShowWorkflowModal(false);
    navigate(`/workflow/editor?id=${definitionId}`);
  }, [navigate]);

  // ── 会话 → 群聊：创建成功后刷新房间列表 → 选中新房间 → 切到群聊模式 → 关闭弹窗 ──
  // 顺序不能颠倒：GroupChatPage 挂载时只刷新列表、不自动选房，先选中才能让新房间立即可见。
  const handleRoomCreated = useCallback(async (roomId: string) => {
    const groupChat = useGroupChatStore.getState();
    await groupChat.loadRooms();
    await groupChat.selectRoom(roomId);
    setMode('groupchat');
    setShowRoomModal(false);
  }, [setMode]);

  // ── Build display messages（按当前会话过滤，防止会话间串消息）──
  // 生成过程中始终创建 assistant 占位消息，即使 streamingContent 为空
  // 否则思考链（reasoning）在内容到达前没有挂载的 MessageBubble，会导致"卡死"假象
  //
  // 依赖只用基本类型/原始值（不把占位消息对象放进 useMemo 的依赖）：
  // 那个对象每次渲染都是新的，放进依赖等于 memo 永远失效（且 `exhaustive-deps` 会报"条件依赖"）。
  const streamingContent = currentGenState?.streamingContent ?? '';
  const displayMessages = useMemo(() => {
    // 仅显示当前会话的消息（防御性过滤，正常情况下 selectSession 已保证）
    // 先按时间戳排序，系统消息保持在其实际发生位置
    const sessionMsgs = messages
      .filter((m) => m.sessionId === currentSessionId)
      .sort((a, b) => a.timestamp - b.timestamp);
    if (!currentGenState) return sessionMsgs;
    return [
      ...sessionMsgs,
      {
        role: 'assistant' as const,
        content: streamingContent,
        sessionId: currentSessionId || '',
        id: 'streaming',
        mode: 'native' as ChatMode,
        // 这个时间戳就是"流式占位气泡"的时间标签，必须是当下时间：改成 state/ref 反而要多一轮更新。
        // `react-hooks/purity` 禁止渲染期读时钟，这里是刻意的例外。
        // eslint-disable-next-line react-hooks/purity
        timestamp: Math.floor(Date.now() / 1000),
      },
    ];
  }, [messages, currentSessionId, currentGenState, streamingContent]);

  /**
   * 会话默认页顶部的问候语（那一屏顶部只有输入区，先给一句人话当门面）。
   *
   * 不需要定时器：跨时段后任何一次重渲染都会重算，粒度到"小时"足够。
   */
  const greeting = greetingByHour(new Date().getHours());

  /**
   * InputBar 的共用 props：聊天视图置于消息列表下方（bottom），
   * 会话默认页置于顶部、其下才是指挥中心内容（top），只有 placement 一处不同。
   */
  const inputBarProps = {
    session: currentSession,
    onSend: handleSend,
    onEnsureSession: ensureSession,
    onStop: handleStop,
    isGenerating: !!currentGenState,
    streamingStatus: currentGenState?.streamingStatus ?? '',
    pendingInput: state.pendingInput,
    onPendingConsumed: () => dispatch({ type: 'SET_PENDING_INPUT', content: null }),
    securityMode,
    onSecurityModeChange: setSecurityMode,
  };

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
      {currentSession ? (
        <>
        {/* 「转为工作流 / 转为群聊」以 headerActions 插槽交给 MessageList：与搜索框同行显示，
            不再单独占一行；会话标题也不再占这一行（标题在左侧列表里已有） */}
        {isLoadingMessages ? (
          <div className="flex-1 flex items-center justify-center">
            <div className="pilotdesk-spinner" />
            <span className="ml-2 text-xs" style={{ color: 'var(--text-secondary)' }}>加载消息中...</span>
          </div>
        ) : (
          <MessageList
            messages={displayMessages}
            session={currentSession}
            headerActions={
              <>
                {/* 与「整理为知识 / 多选 / 关闭会话」统一：**无边框**、同内边距与字号 */}
                <button
                  className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] shrink-0 transition-colors"
                  style={{ color: 'var(--text-secondary)' }}
                  onClick={() => setShowWorkflowModal(true)}
                  title="把本会话的任务清单转为可运行的工作流"
                >
                  <Workflow size={11} />
                  转为工作流
                </button>
                {/* 会话转房间：对所有用户开放（本地能力不设限） */}
                <button
                  className="pd-btn flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] shrink-0 transition-colors"
                  style={{ color: 'var(--text-secondary)' }}
                  onClick={() => setShowRoomModal(true)}
                  title="把本会话的任务清单转为群聊房间，由多 Agent 协作执行"
                >
                  <Users size={11} />
                  转为群聊
                </button>
              </>
            }
            onCloseSession={() => startNewSession()}
            isGenerating={!!currentGenState}
            streamingProgress={currentGenState?.streamingProgress ?? ''}
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
            onResendMessage={handleResendMessage}
          />
        )}
        <InputBar {...inputBarProps} placement="bottom" />
        </>
      ) : (
        <>
        {/* 会话默认页（未选中任何会话）：问候语 → 输入区置顶 → 其下常驻指挥中心内容。
            原 MessageList 的"无会话空态"由此取代 —— 一进来就能看到"什么在等我在跑"，
            发出第一条消息后 currentSession 落定，本页自动切回常规聊天视图（输入区回到底部）。 */}
        <div className="shrink-0 px-4 pt-4 pb-1 text-lg font-medium" style={{ color: 'var(--text-primary)' }}>
          {greeting}，欢迎回来！
        </div>
        <InputBar {...inputBarProps} placement="top" />
        <CommandCenter variant="inline" />
        </>
      )}
      </div>
      </div>

      {/* 会话 → 工作流：预览 + 创建落库（挂在最外层，覆盖整个会话面板） */}
      <SessionToWorkflowModal
        open={showWorkflowModal}
        sessionId={currentSession?.id ?? ''}
        sessionTitle={currentSession?.title ?? ''}
        onClose={() => setShowWorkflowModal(false)}
        onCreated={handleWorkflowCreated}
      />

      {/* 会话 → 群聊：预览 + 成员配置 + 创建房间（挂在最外层，覆盖整个会话面板） */}
      <SessionToRoomModal
        open={showRoomModal}
        sessionId={currentSession?.id ?? ''}
        sessionTitle={currentSession?.title ?? ''}
        onClose={() => setShowRoomModal(false)}
        onCreated={handleRoomCreated}
      />
    </div>
  );
}
