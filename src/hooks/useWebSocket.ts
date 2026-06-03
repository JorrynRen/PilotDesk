import { useEffect, useRef, useCallback } from 'react';
import { getApiKey } from '../stores/apiProviderStore';
import { inferApiFormat, resolveChatUrl, buildHeaders, buildBody } from '../utils/apiClient';
import { useWsStore, type WsMessage, type WsHandlers } from '../stores/wsStore';

let listenerIdCounter = 0;

/**
 * useWebSocket - thin wrapper around the singleton wsStore.
 * All components sharing the same port will use the same WS connection.
 * React StrictMode double-mount won't create duplicate connections because
 * the store manages a single WebSocket instance at module level.
 */
function useWebSocket(port: number = 19830, handlers?: WsHandlers) {
  const storeInit = useWsStore((s) => s.init);
  const storeIsConnected = useWsStore((s) => s.isConnected);
  const storeSend = useWsStore((s) => s.send);
  const storeAddListener = useWsStore((s) => s.addListener);
  const storeRemoveListener = useWsStore((s) => s.removeListener);

  // Unique listener id for this hook instance
  const listenerIdRef = useRef<string>(`ws-listener-${++listenerIdCounter}`);

  // Keep handlers ref updated without re-registering
  const handlersRef = useRef(handlers);
  useEffect(() => {
    handlersRef.current = handlers;
  }, [handlers]);

  // Register listener on mount, unregister on unmount
  useEffect(() => {
    const id = listenerIdRef.current;
    storeAddListener(id, {
      onChunk: (...args) => handlersRef.current?.onChunk?.(...args as [string, string]),
      onDone: (...args) => handlersRef.current?.onDone?.(...args as [string, string]),
      onError: (...args) => handlersRef.current?.onError?.(...args as [string, string]),
      onStatus: (...args) => handlersRef.current?.onStatus?.(...args as [string, string]),
      onSkills: (...args) => handlersRef.current?.onSkills?.(...args as [string, string[]]),
    });
    return () => {
      storeRemoveListener(id);
    };
  }, [storeAddListener, storeRemoveListener]);

  // Initialize the singleton connection (only the first call actually connects)
  useEffect(() => {
    storeInit(port);
  }, [storeInit, port]);

  const sendMessage = useCallback(
    (msg: WsMessage) => {
      storeSend(msg);
    },
    [storeSend]
  );

  const sendChat = useCallback(
    (sessionId: string, message: string, mode?: string, agentType?: string, cwd?: string) => {
      sendMessage({ type: 'chat', sessionId, message, mode, agentType, cwd });
    },
    [sendMessage]
  );

  const stopGeneration = useCallback(
    (sessionId: string, agentType?: string) => {
      sendMessage({ type: 'stop', sessionId, agentType });
    },
    [sendMessage]
  );

  const requestSkills = useCallback(
    (agentType: string) => {
      sendMessage({ type: 'skills:list', agentType });
    },
    [sendMessage]
  );

  const requestAllSkills = useCallback(() => {
    sendMessage({ type: 'skills:list-all' });
  }, [sendMessage]);

  const createAgentSession = useCallback(
    (sessionId: string, agentType: string, cwd?: string) => {
      sendMessage({ type: 'session:create', sessionId, agentType, cwd });
    },
    [sendMessage]
  );

  const closeAgentSession = useCallback(
    (sessionId: string, agentType?: string) => {
      sendMessage({ type: 'session:close', sessionId, agentType });
    },
    [sendMessage]
  );

  // --- Direct API chat (no sidecar, uses fetch) ---
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
      console.log('[API] Sending to:', chatUrl, 'model:', model);

      const abort = new AbortController();
      abortRef.current = abort;

      h?.onStatus?.(sessionId, `调用 ${model}...`);

      const allMessages = [
        ...(history || []).map((m) => ({ role: m.role, content: m.content })),
        { role: 'user', content: message },
      ];

      try {
        if (fmt === 'anthropic') {
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

          const decoder = new TextDecoder();
          let buffer = '';

          while (true) {
            const { done, value } = await reader.read();
            if (done) break;

            buffer += decoder.decode(value, { stream: true });
            const lines = buffer.split('\n');
            buffer = lines.pop() || '';

            for (const line of lines) {
              if (!line.startsWith('data: ')) continue;
              const data = line.slice(6).trim();
              if (data === '[DONE]') continue;

              try {
                const event = JSON.parse(data);
                if (event.type === 'content_block_delta' && event.delta?.text) {
                  h?.onChunk?.(sessionId, event.delta.text);
                } else if (event.type === 'message_stop') {
                  safeOnDone(sessionId);
                  return;
                } else if (event.type === 'error') {
                  h?.onError?.(sessionId, event.error?.message || 'Unknown API error');
                  return;
                }
              } catch {
                // skip unparseable lines
              }
            }
          }

          safeOnDone(sessionId);
        } else {
          const res = await fetch(chatUrl, {
            method: 'POST',
            headers: buildHeaders(fmt, key),
            body: JSON.stringify(buildBody(fmt, { model, messages: allMessages, stream: true })),
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

          const decoder = new TextDecoder();
          let buffer = '';

          while (true) {
            const { done, value } = await reader.read();
            if (done) break;

            buffer += decoder.decode(value, { stream: true });
            const lines = buffer.split('\n');
            buffer = lines.pop() || '';

            for (const line of lines) {
              if (!line.startsWith('data: ')) continue;
              const data = line.slice(6).trim();
              if (data === '[DONE]') {
                safeOnDone(sessionId);
                return;
              }

              try {
                const event = JSON.parse(data);
                const delta = event.choices?.[0]?.delta?.content;
                if (delta) {
                  h?.onChunk?.(sessionId, delta);
                }
              } catch {
                // skip unparseable lines
              }
            }
          }

          safeOnDone(sessionId);
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
    []
  );

  const stopApiChat = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
  }, []);

  return {
    isConnected: storeIsConnected,
    sendMessage,
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

export default useWebSocket;
export { useWebSocket };
