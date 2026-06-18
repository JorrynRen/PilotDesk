import { useEffect, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getApiKey } from '../stores/apiProviderStore';
import { inferApiFormat, resolveChatUrl, buildHeaders, buildBody } from '../utils/apiClient';

/**
 * useAgentEvent — 替代 useWebSocket。
 *
 * 通过 Tauri Event 监听 Agent 流式输出，通过 invoke 发送命令。
 * 消除 WebSocket 中间层，前端直接与 Rust 后端通信。
 */

export interface AgentEventHandlers {
  onChunk?: (sessionId: string, content: string) => void;
  onDone?: (sessionId: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onStatus?: (sessionId: string, status: string) => void;
  onSkills?: (agentType: string, skills: Array<{ name: string; description: string; category?: string }>) => void;
}

/**
 * 共享 SSE 流解析器 — 处理 API 直连模式的 SSE 响应
 */
async function readSSEStream(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  onChunk: (text: string) => void,
  onDone: () => void,
  onError: (msg: string) => void,
  parseDelta: (event: Record<string, unknown>) => string | null,
  signal?: AbortSignal,
): Promise<void> {
  const decoder = new TextDecoder();
  let buffer = '';

  while (true) {
    if (signal?.aborted) {
      onDone();
      return;
    }
    const { done, value } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split('\n');
    buffer = lines.pop() || '';

    for (const line of lines) {
      if (!line.startsWith('data: ')) continue;
      const data = line.slice(6).trim();
      if (data === '[DONE]') {
        onDone();
        return;
      }

      try {
        const event = JSON.parse(data);
        const delta = parseDelta(event);
        if (delta) onChunk(delta);
        if ((event as Record<string, unknown>).type === 'error') {
          const errEvent = event as Record<string, { message?: string }>;
          onError(errEvent.error?.message || 'Unknown API error');
          return;
        }
      } catch {
        // skip unparseable lines
      }
    }
  }
  onDone();
}

export function useAgentEvent(handlers?: AgentEventHandlers) {
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

      const doneUnlisten = await listen<{ sessionId: string }>('agent-done', (event) => {
        if (cancelled) return;
        handlersRef.current?.onDone?.(event.payload.sessionId);
      });
      unlisteners.push(doneUnlisten);

      const errorUnlisten = await listen<{ sessionId: string; error: string }>('agent-error', (event) => {
        if (cancelled) return;
        handlersRef.current?.onError?.(event.payload.sessionId, event.payload.error);
      });
      unlisteners.push(errorUnlisten);

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
    ) => {
      try {
        await invoke('agent_send_message', {
          sessionId,
          agentType: agentType || 'claude',
          message,
          mode: mode || 'native',
          cwd: cwd || null,
          systemPrompt: systemPrompt || null,
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
          agentType: agentType || 'claude',
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
    const agents = ['claude', 'hermes', 'codex'];
    for (const agentType of agents) {
      await requestSkills(agentType);
    }
  }, [requestSkills]);

  // ── API 直连（保持现有 fetch SSE 逻辑） ──

  const abortRef = useRef<AbortController | null>(null);
  const apiDoneFiredRef = useRef<string | null>(null);

  const sendApiChat = useCallback(
    async (
      sessionId: string,
      message: string,
      apiEndpoint: string,
      providerId: string,
      model: string,
      history?: Array<{ role: string; content: string }>,
      providerName?: string,
    ) => {
      const h = handlersRef.current;
      apiDoneFiredRef.current = null;
      const safeOnDone = (sid: string) => {
        if (apiDoneFiredRef.current === sid) return;
        apiDoneFiredRef.current = sid;
        h?.onDone?.(sid);
      };

      const key = await getApiKey(providerId);
      if (!key) {
        h?.onError?.(sessionId, `未配置 API Key: ${providerName || providerId}`);
        return;
      }

      const fmt = inferApiFormat(providerId, apiEndpoint);
      const chatUrl = resolveChatUrl(apiEndpoint, fmt);

      const abort = new AbortController();
      abortRef.current = abort;

      h?.onStatus?.(sessionId, `调用 ${model}...`);

      const allMessages = [
        ...(history || []).map((m) => ({ role: m.role, content: m.content })),
        { role: 'user', content: message },
      ];

      try {
        const res = await fetch(chatUrl, {
          method: 'POST',
          headers: buildHeaders(fmt, key),
          body: JSON.stringify(buildBody(fmt, { model, messages: allMessages, stream: true, maxTokens: 4096 })),
          signal: abort.signal,
        });

        if (!res.ok) {
          const errText = await res.text();
          h?.onError?.(sessionId, `API 错误 (${res.status}): ${errText}`);
          return;
        }

        const reader = res.body?.getReader();
        if (!reader) {
          h?.onError?.(sessionId, '无法读取响应流');
          return;
        }

        if (fmt === 'anthropic') {
          await readSSEStream(
            reader,
            (text) => h?.onChunk?.(sessionId, text),
            () => safeOnDone(sessionId),
            (msg) => h?.onError?.(sessionId, msg),
            (event) => {
              const e = event as { type?: string; delta?: { text?: string } };
              if (e.type === 'content_block_delta' && e.delta?.text) return e.delta.text;
              return null;
            },
            abort.signal,
          );
        } else {
          await readSSEStream(
            reader,
            (text) => h?.onChunk?.(sessionId, text),
            () => safeOnDone(sessionId),
            (msg) => h?.onError?.(sessionId, msg),
            (event) => {
              const e = event as { choices?: Array<{ delta?: { content?: string } }> };
              return e.choices?.[0]?.delta?.content ?? null;
            },
            abort.signal,
          );
        }
      } catch (err: unknown) {
        if (err instanceof DOMException && err.name === 'AbortError') {
          safeOnDone(sessionId);
        } else {
          const msg = err instanceof Error ? err.message : String(err);
          h?.onError?.(sessionId, `请求失败: ${msg}`);
        }
      }
    },
    [],
  );

  const stopApiChat = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
  }, []);

  return {
    isConnected: true, // Tauri Event 始终可用
    sendChat,
    sendApiChat,
    stopGeneration,
    stopApiChat,
    requestSkills,
    requestAllSkills,
    createAgentSession,
    closeAgentSession,
  };
}

export default useAgentEvent;
