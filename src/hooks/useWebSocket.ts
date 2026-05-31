import { useEffect, useRef, useState, useCallback } from 'react';
import type { ChatMode } from '../types';

interface WsMessage {
  type: 'chat' | 'stop' | 'session:create' | 'session:close' | 'ping';
  sessionId: string;
  agentType?: 'claude' | 'hermes';
  message?: string;
  mode?: ChatMode;
  cwd?: string;
}

interface WsHandlers {
  onChunk?: (sessionId: string, content: string) => void;
  onDone?: (sessionId: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onStatus?: (sessionId: string, status: string) => void;
}

export function useWebSocket(port: number = 19830, handlers?: WsHandlers) {
  const wsRef = useRef<WebSocket | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const reconnectCountRef = useRef(0);
  const handlersRef = useRef(handlers);

  // AbortController ref for API direct calls
  const abortRef = useRef<AbortController | null>(null);

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
    (sessionId: string, message: string, mode: ChatMode = 'native', agentType: 'claude' | 'hermes' = 'claude', cwd?: string) => {
      sendMessage({
        type: 'chat',
        sessionId,
        message,
        mode,
        agentType,
        cwd,
      });
    },
    [sendMessage]
  );

  const stopGeneration = useCallback(
    (sessionId: string, agentType: 'claude' | 'hermes' = 'claude') => {
      sendMessage({
        type: 'stop',
        sessionId,
        agentType,
      });
    },
    [sendMessage]
  );

  /**
   * Send message via direct API call (OpenAI-compatible or Anthropic Messages API).
   * @param sessionId - Session ID for callback routing
   * @param message - User message text
   * @param providerId - Provider ID (e.g. "anthropic", "openai")
   * @param model - Model name (e.g. "claude-sonnet-4-20250514")
   */
  const sendApiChat = useCallback(
    async (
      sessionId: string,
      message: string,
      providerId: string,
      model: string,
    ) => {
      const h = handlersRef.current;

      // Load provider config from localStorage
      const providersRaw = localStorage.getItem('pd-api-providers');
      if (!providersRaw) {
        h?.onError?.(sessionId, '未找到 API 提供商配置');
        return;
      }

      let providers: Array<{
        id: string;
        api_endpoint: string;
        api_key_masked: string | null;
        models: string[];
      }>;
      try {
        providers = JSON.parse(providersRaw);
      } catch {
        h?.onError?.(sessionId, 'API 提供商配置格式错误');
        return;
      }

      const provider = providers.find((p) => p.id === providerId);
      if (!provider) {
        h?.onError?.(sessionId, `未找到提供商: ${providerId}`);
        return;
      }

      // Retrieve the actual API key (stored separately)
      const key = localStorage.getItem(`pd-api-${providerId}-key`);
      if (!key) {
        h?.onError?.(sessionId, `未配置 API Key: ${provider.name}`);
        return;
      }

      const endpoint = provider.api_endpoint.replace(/\/+$/, '');
      const abort = new AbortController();
      abortRef.current = abort;

      h?.onStatus?.(sessionId, `调用 ${model}...`);

      try {
        if (providerId === 'anthropic') {
          // Anthropic Messages API format
          const res = await fetch(`${endpoint}/v1/messages`, {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              'x-api-key': key,
              'anthropic-version': '2023-06-01',
            },
            body: JSON.stringify({
              model,
              max_tokens: 4096,
              stream: true,
              messages: [{ role: 'user', content: message }],
            }),
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
                  h?.onDone?.(sessionId);
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
          h?.onDone?.(sessionId);
        } else {
          // OpenAI-compatible Chat Completions API format
          const res = await fetch(`${endpoint}/v1/chat/completions`, {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              'Authorization': `Bearer ${key}`,
            },
            body: JSON.stringify({
              model,
              stream: true,
              messages: [{ role: 'user', content: message }],
            }),
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
                h?.onDone?.(sessionId);
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

          h?.onDone?.(sessionId);
        }
      } catch (err: unknown) {
        if (err instanceof DOMException && err.name === 'AbortError') {
          h?.onDone?.(sessionId);
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

  // Auto-connect on mount, disconnect on unmount
  useEffect(() => {
    connect();
    return () => {
      disconnect();
      abortRef.current?.abort();
    };
  }, [connect, disconnect]);

  return {
    isConnected,
    sendMessage,
    sendChat,
    sendApiChat,
    stopGeneration,
    stopApiChat,
    disconnect,
  };
}
