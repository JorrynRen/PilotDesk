import { useState, useRef, useCallback, useEffect } from 'react';
import { getApiKey } from '../stores/apiProviderStore';
import { inferApiFormat, resolveChatUrl, buildHeaders, buildBody } from '../utils/apiClient';

type WsMessage =
  | { type: 'chat'; sessionId: string; message: string }
  | { type: 'stop'; sessionId: string }
  | { type: 'session:create'; sessionId: string; agentType: string; cwd?: string }
  | { type: 'session:close'; sessionId: string }
  | { type: 'ping' }
  | { type: 'skills:list'; agentType: string }
  | { type: 'skills:list-all' };

interface WsHandlers {
  onChunk?: (sessionId: string, content: string) => void;
  onDone?: (sessionId: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onStatus?: (sessionId: string, status: string) => void;
  onSkills?: (agentType: string, skills: string[]) => void;
}

function useWebSocket(port: number = 19830, handlers?: WsHandlers) {
  const wsRef = useRef<WebSocket | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const reconnectCountRef = useRef(0);
  const handlersRef = useRef(handlers);

  // AbortController ref for API direct calls
  const abortRef = useRef<AbortController | null>(null);
  // Per-session done guard to prevent double onDone calls
  const apiDoneFiredRef = useRef<string | null>(null);

  // Keep handlers ref updated
  useEffect(() => {
    handlersRef.current = handlers;
  }, [handlers]);

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    try {
      const ws = new WebSocket(`ws://localhost:${port}`);
      wsRef.current = ws;

      ws.onopen = () => {
        setIsConnected(true);
        reconnectCountRef.current = 0;
        console.log(`[WS] Connected to sidecar on port ${port}`);
      };

      ws.onmessage = (event) => {
        try {
          const msg = JSON.parse(event.data);
          const h = handlersRef.current;

          switch (msg.type) {
            case 'chunk':
              h?.onChunk?.(msg.sessionId, msg.content || '');
              break;
            case 'done':
              h?.onDone?.(msg.sessionId);
              break;
            case 'error':
              h?.onError?.(msg.sessionId, msg.error || 'Unknown error');
              break;
            case 'status':
              h?.onStatus?.(msg.sessionId, msg.status || '');
              break;
            case 'skills':
              h?.onSkills?.(msg.agentType || '', msg.skills || []);
              break;
          }
        } catch (err) {
          console.error('[WS] Failed to parse message:', err);
        }
      };

      ws.onclose = () => {
        setIsConnected(false);
        wsRef.current = null;

        // Exponential backoff reconnect (max 5 attempts)
        if (reconnectCountRef.current < 5) {
          const delay = Math.min(1000 * Math.pow(2, reconnectCountRef.current), 30000);
          reconnectCountRef.current += 1;
          console.log(`[WS] Reconnecting in ${delay}ms (attempt ${reconnectCountRef.current})`);
          reconnectTimerRef.current = setTimeout(connect, delay);
        }
      };

      ws.onerror = () => {
        ws.close();
      };
    } catch (err) {
      console.error('[WS] Connection error:', err);
    }
  }, [port]);

  const disconnect = useCallback(() => {
    if (reconnectTimerRef.current) {
      clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
    reconnectCountRef.current = 5; // Prevent auto-reconnect
    wsRef.current?.close();
    wsRef.current = null;
    setIsConnected(false);
  }, []);

  const sendMessage = useCallback((msg: WsMessage) => {
    const ws = wsRef.current;
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    } else {
      console.warn('[WS] Cannot send: not connected');
    }
  }, []);

  const sendChat = useCallback(
    (sessionId: string, message: string) => {
      sendMessage({ type: 'chat', sessionId, message });
    },
    [sendMessage]
  );

  const stopGeneration = useCallback(
    (sessionId: string) => {
      sendMessage({ type: 'stop', sessionId });
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

  /**
   * Send API chat directly to LLM provider (no sidecar)
   * @param sessionId - Session ID
   * @param message - User message text
   * @param apiEndpoint - Full API URL (e.g. https://api.siliconflow.cn/v1/chat/completions)
   * @param providerId - Provider ID for API key lookup
   * @param model - Model name
   * @param history - Previous messages
   * @param providerName - Display name for error messages
   */
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
      // Reset done guard for this session
      apiDoneFiredRef.current = null;
      const safeOnDone = (sid: string) => {
        if (apiDoneFiredRef.current === sid) return;
        apiDoneFiredRef.current = sid;
        h?.onDone?.(sid);
      };

      // Retrieve the actual API key from SQLite
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

          // Parse SSE stream
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

          // Stream ended without explicit message_stop
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

          // Parse SSE stream
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

  // Auto-connect on mount
  useEffect(() => {
    connect();
    return () => {
      disconnect();
    };
  }, [connect, disconnect]);

  return {
    isConnected,
    sendMessage,
    sendChat,
    sendApiChat,
    stopGeneration,
    stopApiChat,
    requestSkills,
    requestAllSkills,
    disconnect,
  };
}

export default useWebSocket;
// ... (文件末尾)
// 添加命名导出
export { useWebSocket };
