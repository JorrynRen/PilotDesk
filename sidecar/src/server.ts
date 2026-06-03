import { WebSocketServer, WebSocket } from 'ws';
import { ClaudeCodeAdapter } from './adapters/claude-code';
import { HermesAdapter } from './adapters/hermes';
import type { AgentAdapter } from './adapters/base';
import type { WsMessage, ChatChunk } from './types';

const PORT = process.env.PORT ? parseInt(process.env.PORT) : 19830;
const wss = new WebSocketServer({ port: PORT });

// Agent adapter registry
const claudeAdapter = new ClaudeCodeAdapter();
const hermesAdapter = new HermesAdapter();
const adapters: Record<string, AgentAdapter> = {
  claude: claudeAdapter,
  hermes: hermesAdapter,
};

function getAdapter(agentType: string): AgentAdapter {
  const adapter = adapters[agentType];
  if (!adapter) {
    throw new Error(`Unknown agent type: ${agentType}`);
  }
  return adapter;
}

function send(ws: WebSocket, msg: ChatChunk) {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(msg));
  }
}

/**
 * Wrap an AsyncGenerator with a timeout.
 * If no item is yielded within the timeout, throws an error.
 */
async function* withTimeout<T>(
  iterable: AsyncGenerator<T>,
  timeoutMs: number,
): AsyncGenerator<T> {
  const iterator = iterable[Symbol.asyncIterator]();
  while (true) {
    const result = await Promise.race([
      iterator.next(),
      new Promise<IteratorResult<T>>((_, reject) =>
        setTimeout(
          () => reject(new Error(`请求超时：智能体未在 ${timeoutMs / 1000} 秒内响应，请检查智能体状态后重试`)),
          timeoutMs,
        ),
      ),
    ]);
    if (result.done) break;
    yield result.value;
  }
}

wss.on('connection', (ws) => {
  console.log('[WS] Client connected');

  ws.on('message', async (raw) => {
    let msg: WsMessage;
    try {
      msg = JSON.parse(raw.toString());
    } catch {
      send(ws, { type: 'error', sessionId: '', error: 'Invalid message format' });
      return;
    }

    switch (msg.type) {
      case 'ping':
        send(ws, { type: 'status', sessionId: msg.sessionId, status: 'pong' });
        break;

      case 'session:create':
        try {
          const adapter = getAdapter(msg.agentType || 'claude');
          await adapter.createSession(msg.sessionId, msg.cwd);
          send(ws, {
            type: 'status',
            sessionId: msg.sessionId,
            status: `session_created:${msg.agentType || 'claude'}`,
          });
        } catch (err: any) {
          send(ws, { type: 'error', sessionId: msg.sessionId, error: err.message });
        }
        break;

      case 'session:close':
        try {
          const adapter = getAdapter(msg.agentType || 'claude');
          await adapter.closeSession(msg.sessionId);
          send(ws, {
            type: 'status',
            sessionId: msg.sessionId,
            status: 'session_closed',
          });
        } catch (err: any) {
          send(ws, { type: 'error', sessionId: msg.sessionId, error: err.message });
        }
        break;

      case 'chat': {
        const agentType = msg.agentType || 'claude';
        try {
          const adapter = getAdapter(agentType);
          const request = {
            sessionId: msg.sessionId,
            message: msg.message || '',
            mode: msg.mode || 'native',
            cwd: msg.cwd,
          };

          // 120s timeout for the entire generation process
          for await (const chunk of withTimeout(adapter.sendMessage(request), 120000)) {
            send(ws, {
              type: 'chunk',
              sessionId: msg.sessionId,
              content: chunk,
            });
          }

          send(ws, {
            type: 'done',
            sessionId: msg.sessionId,
            content: '',
          });
        } catch (err: any) {
          send(ws, {
            type: 'error',
            sessionId: msg.sessionId,
            error: err.message,
          });
        }
        break;
      }

      case 'stop': {
        const agentType = msg.agentType || 'claude';
        try {
          const adapter = getAdapter(agentType);
          adapter.stopGeneration(msg.sessionId);
          send(ws, {
            type: 'status',
            sessionId: msg.sessionId,
            status: 'generation_stopped',
          });
        } catch (err: any) {
          send(ws, { type: 'error', sessionId: msg.sessionId, error: err.message });
        }
        break;
      }

      case 'skills:list': {
        try {
          const agentType = msg.agentType || 'claude';
          const adapter = getAdapter(agentType);
          const skills = await adapter.listSkills();
          send(ws, {
            type: 'skills' as const,
            sessionId: '',
            agentType,
            skills,
          } as any);
        } catch (err: any) {
          send(ws, { type: 'error', sessionId: '', error: err.message });
        }
        break;
      }

      case 'skills:list-all': {
        try {
          const results: Array<{ agentType: string; skills: any[] }> = [];
          for (const [agentType, adapter] of Object.entries(adapters)) {
            const skills = await adapter.listSkills();
            results.push({ agentType, skills });
          }
          // Send results for each agent
          for (const result of results) {
            send(ws, {
              type: 'skills' as const,
              sessionId: '',
              agentType: result.agentType,
              skills: result.skills,
            } as any);
          }
          send(ws, { type: 'status', sessionId: '', status: 'skills:list-all:done' });
        } catch (err: any) {
          send(ws, { type: 'error', sessionId: '', error: err.message });
        }
        break;
      }

      default:
        send(ws, { type: 'error', sessionId: msg.sessionId, error: `Unknown message type: ${msg.type}` });
    }
  });

  ws.on('close', () => {
    console.log('[WS] Client disconnected');
  });
});

console.log(`PilotDesk Sidecar WebSocket server listening on port ${PORT}`);
