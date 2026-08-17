import { useEffect, useRef, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAgentRegistry } from './useAgentRegistry';
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

  const unlistenRef = useRef<UnlistenFn[]>([]);

  // 注册 Tauri Event 监听器（仅一次）
  useEffect(() => {
    let cancelled = false;

    (async () => {
      const unlisteners: UnlistenFn[] = [];

      const chunkUnlisten = await listen<{ sessionId: string; content: string }>('agent-chunk', (event) => {
        if (cancelled) return;
        handlersRef.current?.onChunk?.(event.payload.sessionId, event.payload.content);
      });
      unlisteners.push(chunkUnlisten);

      const doneUnlisten = await listen<{ sessionId: string; content?: string }>('agent-done', (event) => {
        if (cancelled) return;
        handlersRef.current?.onDone?.(event.payload.sessionId, event.payload.content);
      });
      unlisteners.push(doneUnlisten);

      const errorUnlisten = await listen<{ sessionId: string; error: string }>('agent-error', (event) => {
        if (cancelled) return;
        handlersRef.current?.onError?.(event.payload.sessionId, event.payload.error);
      });
      unlisteners.push(errorUnlisten);

      const sessionUnlisten = await listen<{ sessionId: string; agentSessionId: string }>('agent-session', (event) => {
        if (cancelled) return;
        handlersRef.current?.onSession?.(event.payload.sessionId, event.payload.agentSessionId);
      });
      unlisteners.push(sessionUnlisten);

      const approvalUnlisten = await listen<{ sessionId: string; toolId: string; toolName: string; arguments: string; riskDescription: string }>('agent-approval-required', (event) => {
        if (cancelled) return;
        handlersRef.current?.onApprovalRequired?.(event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.arguments, event.payload.riskDescription);
      });
      unlisteners.push(approvalUnlisten);

      const confirmationUnlisten = await listen<{
        sessionId: string; callId: string; title?: string; prompt: string; replyMode: 'open' | 'structured';
        items: Array<{ id: string; label: string; inputType: 'text' | 'select' | 'confirm'; options: string[]; required: boolean; placeholder?: string }>;
      }>('agent-confirmation-request', (event) => {
        if (cancelled) return;
        handlersRef.current?.onConfirmationRequest?.(event.payload);
      });
      unlisteners.push(confirmationUnlisten);

      const toolStartUnlisten = await listen<{ sessionId: string; toolId: string; toolName: string; arguments: string }>('agent-tool-start', (event) => {
        if (cancelled) return;
        handlersRef.current?.onToolStart?.(event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.arguments);
      });
      unlisteners.push(toolStartUnlisten);

      const toolResultUnlisten = await listen<{ sessionId: string; toolId: string; toolName: string; result: string; success: boolean }>('agent-tool-result', (event) => {
        if (cancelled) return;
        handlersRef.current?.onToolResult?.(event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.result, event.payload.success);
      });
      unlisteners.push(toolResultUnlisten);

      const fileDiffUnlisten = await listen<{ sessionId: string; path: string; diff: string }>('agent-file-diff', (event) => {
        if (cancelled) return;
        handlersRef.current?.onFileDiff?.(event.payload.sessionId, event.payload.path, event.payload.diff);
      });
      unlisteners.push(fileDiffUnlisten);

      const reasoningUnlisten = await listen<{ sessionId: string; content: string }>('agent-reasoning', (event) => {
        if (cancelled) return;
        handlersRef.current?.onReasoning?.(event.payload.sessionId, event.payload.content);
      });
      unlisteners.push(reasoningUnlisten);

      const iterLimitUnlisten = await listen<{ sessionId: string; current: number; max: number }>('agent-iteration-limit', (event) => {
        if (cancelled) return;
        handlersRef.current?.onIterationLimit?.(event.payload.sessionId, event.payload.current, event.payload.max);
      });
      unlisteners.push(iterLimitUnlisten);

      const usageUnlisten = await listen<{ sessionId: string; promptTokens: number; completionTokens: number; totalTokens: number }>('agent-usage', (event) => {
        if (cancelled) return;
        handlersRef.current?.onUsage?.(event.payload.sessionId, {
          prompt: event.payload.promptTokens,
          completion: event.payload.completionTokens,
          total: event.payload.totalTokens,
        });
      });
      unlisteners.push(usageUnlisten);

      unlistenRef.current = unlisteners;
    })();

    return () => {
      cancelled = true;
      for (const unlisten of unlistenRef.current) {
        unlisten();
      }
    };
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
