import { useEffect, useRef, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAgentRegistry } from './useAgentRegistry';
import { subscribeAgentBus } from './agentEventBus';
import type { Attachment } from '../types';

/**
 * useAgentEvent — 替代 useWebSocket。
 *
 * 通过 Tauri Event 监听 Agent 流式输出，通过 invoke 发送命令。
 * 消除 WebSocket 中间层，前端直接与 Rust 后端通信。
 * API Agent 和 CLI Agent 统一使用 agent_send_message_with_config。
 */

export interface AgentEventHandlers {
  onChunk?: (sessionId: string, content: string) => void;
  onDone?: (sessionId: string, fallbackContent?: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onStatus?: (sessionId: string, status: string) => void;
  onSession?: (sessionId: string, agentSessionId: string) => void;
  onSkills?: (agentType: string, skills: Array<{ name: string; description: string; category?: string }>) => void;
  onApprovalRequired?: (sessionId: string, callId: string, toolName: string, toolArgs: string, riskDescription: string) => void;
  /** ask_user 工具确认请求（payload 与后端 agent-confirmation-request 事件一致）。 */
  onConfirmationRequest?: (payload: {
    sessionId: string;
    callId: string;
    title?: string;
    prompt: string;
    replyMode: 'open' | 'structured';
    items: Array<{ id: string; label: string; inputType: 'text' | 'select' | 'confirm'; options: string[]; required: boolean; placeholder?: string }>;
  }) => void;
  onToolStart?: (sessionId: string, toolId: string, toolName: string, toolArgs: string) => void;
  onToolResult?: (sessionId: string, toolId: string, toolName: string, result: string, success: boolean) => void;
  /** 工具执行进度提示（如 generate_video 任务创建成功 / 每 30s 状态）。 */
  onToolProgress?: (sessionId: string, toolName: string, message: string) => void;
  onFileDiff?: (sessionId: string, path: string, diff: string) => void;
  onReasoning?: (sessionId: string, content: string) => void;
  onIterationLimit?: (sessionId: string, current: number, max: number) => void;
  onUsage?: (sessionId: string, tokens: { prompt: number; completion: number; total: number }) => void;
}

export function useAgentEvent(handlers?: AgentEventHandlers) {
  const { agents: registryAgents } = useAgentRegistry();
  const agentTypes = useMemo(() => registryAgents.map(a => a.agentType), [registryAgents]);
  const defaultAgentType = useMemo(() => agentTypes[0] || '', [agentTypes]);
  const handlersRef = useRef(handlers);
  useEffect(() => {
    handlersRef.current = handlers;
  }, [handlers]);

  // 前端统一投影阶段①：agent-* 事件改经模块级单例总线广播（见 agentEventBus），
  // 各组件无需各自 listen —— 本组件只订阅一次，随卸载退订。
  useEffect(() => {
    return subscribeAgentBus(() => handlersRef.current);
  }, []);

  // ── Agent 命令 ──

  const sendChat = useCallback(
    async (
      sessionId: string,
      message: string,
      mode?: string,
      agentType?: string,
      cwd?: string,
      systemPrompt?: string,
      agentSessionId?: string,
      temperature?: number,
      maxTokens?: number,
      attachments?: Attachment[],
      securityMode?: string,
    ) => {
      try {
        await invoke('agent_send_message_with_config', {
          sessionId,
          agentType: agentType || defaultAgentType,
          message,
          mode: mode || 'native',
          cwd: cwd || null,
          systemPrompt: systemPrompt || null,
          agentSessionId: agentSessionId || null,
          temperature: temperature ?? null,
          maxTokens: maxTokens ?? null,
          attachments: attachments && attachments.length > 0 ? attachments : null,
          securityMode: securityMode || 'standard',
        });
      } catch (err) {
        handlersRef.current?.onError?.(sessionId, String(err));
      }
    },
    [],
  );

  const stopGeneration = useCallback(
    async (sessionId: string) => {
      try {
        await invoke('agent_stop_generation', { sessionId });
      } catch (err) {
        console.error('[Agent] stop failed:', err);
      }
    },
    [],
  );

  const createAgentSession = useCallback(
    async (sessionId: string, agentType: string, cwd?: string) => {
      try {
        await invoke('agent_create_session', {
          sessionId,
          agentType,
          cwd: cwd || null,
        });
      } catch (err) {
        console.error('[Agent] create session failed:', err);
      }
    },
    [],
  );

  const closeAgentSession = useCallback(
    async (sessionId: string, agentType?: string) => {
      try {
        await invoke('agent_close_session', {
          sessionId,
          agentType: agentType || defaultAgentType,
        });
      } catch (err) {
        console.error('[Agent] close session failed:', err);
      }
    },
    [],
  );

  const requestSkills = useCallback(
    async (agentType: string) => {
      try {
        const skills = await invoke<Array<{ name: string; description: string; category: string }>>('agent_list_skills', { agentType });
        handlersRef.current?.onSkills?.(agentType, skills);
      } catch (err) {
        console.error('[Agent] list skills failed:', err);
      }
    },
    [],
  );

  const requestAllSkills = useCallback(async () => {
    for (const agentType of agentTypes) {
      await requestSkills(agentType);
    }
  }, [requestSkills, agentTypes]);


  // ── API Agent（统一路由到 Rust 后端 AgentLoop）──

  const sendApiChat = useCallback(
    async (
      sessionId: string,
      message: string,
    ) => {
      // API Agent 统一通过 agent_send_message_with_config 路由到 Rust 后端
      try {
        await invoke('agent_send_message_with_config', {
          sessionId,
          agentType: 'api',
          message,
          mode: 'native',
          cwd: null,
          systemPrompt: null,
          agentSessionId: null,
        });
      } catch (err) {
        handlersRef.current?.onError?.(sessionId, String(err));
      }
    },
    [],
  );

  const stopApiChat = useCallback(
    async (sessionId: string) => {
      await stopGeneration(sessionId);
    },
    [stopGeneration],
  );

  // ── 审批响应 ──

  const respondToApproval = useCallback(
    async (sessionId: string, callId: string, approved: boolean) => {
      try {
        await invoke('agent_approve_tool', {
          sessionId,
          callId,
          approved,
        });
      } catch (err) {
        console.error('[Agent] approve failed:', err);
      }
    },
    [],
  );

  /** 响应迭代上限确认（继续或终止） */
  const respondToIterLimit = useCallback(
    async (sessionId: string, shouldContinue: boolean) => {
      try {
        await invoke('agent_continue_loop', {
          sessionId,
          shouldContinue,
        });
      } catch (err) {
        console.error('[Agent] continue_loop failed:', err);
      }
    },
    [],
  );

  /** 响应 ask_user 确认（content 为格式化后的用户回复文本） */
  const respondToConfirmation = useCallback(
    async (sessionId: string, callId: string, content: string) => {
      try {
        await invoke('agent_respond_confirmation', {
          sessionId,
          callId,
          content,
        });
      } catch (err) {
        console.error('[Agent] respond confirmation failed:', err);
      }
    },
    [],
  );

  return {
    isConnected: true, // Tauri Event 始终可用
    sendChat,
    sendApiChat,
    stopGeneration,
    stopApiChat,
    respondToApproval,
    respondToIterLimit,
    respondToConfirmation,
    requestSkills,
    requestAllSkills,
    createAgentSession,
    closeAgentSession,
  };
}

export default useAgentEvent;
