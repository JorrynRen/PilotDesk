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

  // Auto-connect on mount, disconnect on unmount
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
    stopGeneration,
    disconnect,
  };
}
