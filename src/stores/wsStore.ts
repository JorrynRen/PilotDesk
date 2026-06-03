import { create } from 'zustand';

/**
 * WebSocket singleton store.
 * All components share a single WS connection to the sidecar,
 * avoiding React StrictMode double-mount from creating duplicate connections.
 */

export type WsMessage =
  | { type: 'chat'; sessionId: string; message: string; mode?: string; agentType?: string; cwd?: string }
  | { type: 'stop'; sessionId: string; agentType?: string }
  | { type: 'session:create'; sessionId: string; agentType: string; cwd?: string }
  | { type: 'session:close'; sessionId: string; agentType?: string }
  | { type: 'ping' }
  | { type: 'skills:list'; agentType: string }
  | { type: 'skills:list-all' };

export interface WsHandlers {
  onChunk?: (sessionId: string, content: string) => void;
  onDone?: (sessionId: string, content?: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onStatus?: (sessionId: string, status: string) => void;
  onSkills?: (agentType: string, skills: string[]) => void;
}

// Listener management: components register/unregister handlers
interface ListenerEntry {
  id: string;
  handlers: WsHandlers;
}

interface WsStoreState {
  isConnected: boolean;
  isInitialized: boolean;

  // Connection management
  init: (port: number) => void;
  send: (msg: WsMessage) => void;

  // Listener registration (each useWebSocket caller gets a unique id)
  addListener: (id: string, handlers: WsHandlers) => void;
  removeListener: (id: string) => void;

  // Internal: set connected state
  _setConnected: (v: boolean) => void;
}

let wsInstance: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let reconnectCount = 0;
let wsPort = 19830;
let initCalled = false;

// Global listeners
const listeners = new Map<string, ListenerEntry>();

function wsDebug(tag: string, ...args: unknown[]) {
  const el = document.getElementById('ws-debug-panel');
  if (el) {
    const text = args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ');
    el.textContent = `[${new Date().toLocaleTimeString()}] ${tag}: ${text}\n` + el.textContent;
  }
  console.log('[WS]', tag, ...args);
}

function broadcast(type: string, ...args: unknown[]) {
  for (const [, entry] of listeners) {
    const handler = entry.handlers[type as keyof WsHandlers] as Function | undefined;
    if (handler) handler(...args);
  }
}

function doConnect() {
  if (wsInstance?.readyState === WebSocket.OPEN) return;

  try {
    const ws = new WebSocket(`ws://127.0.0.1:${wsPort}`);
    wsInstance = ws;

    ws.onopen = () => {
      reconnectCount = 0;
      useWsStore.getState()._setConnected(true);
      wsDebug('onopen', `port=${wsPort}`);
    };

    ws.onmessage = (event) => {
      wsDebug('onmessage', (event.data as string)?.substring?.(0, 300) || event.data);
      try {
        const msg = JSON.parse(event.data as string) as Record<string, unknown>;
        wsDebug('parsed', msg.type, 'sid:', msg.sessionId);

        switch (msg.type) {
          case 'chunk':
            broadcast('onChunk', msg.sessionId as string, (msg.content as string) || '');
            break;
          case 'done':
            broadcast('onDone', msg.sessionId as string);
            break;
          case 'error':
            broadcast('onError', msg.sessionId as string, (msg.error as string) || 'Unknown error');
            break;
          case 'status':
            broadcast('onStatus', msg.sessionId as string, (msg.status as string) || '');
            break;
          case 'skills':
            broadcast('onSkills', (msg.agentType as string) || '', (msg.skills as string[]) || []);
            break;
        }
      } catch (err) {
        console.error('[WS] Failed to parse message:', err);
      }
    };

    ws.onclose = () => {
      useWsStore.getState()._setConnected(false);
      wsInstance = null;

      if (reconnectCount < 5) {
        const delay = Math.min(1000 * Math.pow(2, reconnectCount), 30000);
        reconnectCount += 1;
        console.log(`[WS] Reconnecting in ${delay}ms (attempt ${reconnectCount})`);
        reconnectTimer = setTimeout(doConnect, delay);
      }
    };

    ws.onerror = () => {
      ws.close();
    };
  } catch (err) {
    console.error('[WS] Connection error:', err);
  }
}

export const useWsStore = create<WsStoreState>((set) => ({
  isConnected: false,
  isInitialized: false,

  _setConnected: (v: boolean) => set({ isConnected: v }),

  init: (port: number) => {
    if (initCalled) return; // Only init once (singleton)
    initCalled = true;
    wsPort = port;
    set({ isInitialized: true });
    doConnect();
  },

  send: (msg: WsMessage) => {
    if (wsInstance?.readyState === WebSocket.OPEN) {
      wsInstance.send(JSON.stringify(msg));
      wsDebug('sent', msg.type, 'sid:', (msg as Record<string, unknown>).sessionId);
    } else {
      console.warn('[WS] Cannot send: not connected, state=', wsInstance?.readyState);
      wsDebug('send-failed', msg.type, 'state:', wsInstance?.readyState);
    }
  },

  addListener: (id: string, handlers: WsHandlers) => {
    listeners.set(id, { id, handlers });
  },

  removeListener: (id: string) => {
    listeners.delete(id);
  },
}));
