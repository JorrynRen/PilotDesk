import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/**
 * agentEventBus — 模块级单例"agent-*"事件总线（前端统一投影阶段①）。
 *
 * 背景：`useAgentEvent` 旧实现每个 hook 实例都各自 `listen('agent-*')` 一次，
 * MainPanel / App / SessionList / SkillBrowser 等多处并发使用时同一后端事件被重复
 * 注册/派发（重复状态刷新、多余开销）。本总线把 12 类 agent-* 事件收敛为：
 *   注册一次（首次订阅时惰性启动，进程生命周期内不注销）→ 广播给所有订阅者。
 * 订阅者各自持有最新的 handlers 引用（get() 闭包），组件卸载即退订，互不干扰。
 *
 * 仅收敛 Tauri `agent-*` 实时事件；命令发送（invoke）与群聊 room 通道不在本层。
 */

export interface AgentBusHandlers {
  onChunk?: (sessionId: string, content: string) => void;
  onDone?: (sessionId: string, content?: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onSession?: (sessionId: string, agentSessionId: string) => void;
  onApprovalRequired?: (sessionId: string, callId: string, toolName: string, toolArgs: string, riskDescription: string) => void;
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
  onToolProgress?: (sessionId: string, toolName: string, message: string) => void;
  onFileDiff?: (sessionId: string, path: string, diff: string) => void;
  onReasoning?: (sessionId: string, content: string) => void;
  onIterationLimit?: (sessionId: string, current: number, max: number) => void;
  onUsage?: (sessionId: string, tokens: { prompt: number; completion: number; total: number }) => void;
}

/** 订阅者：返回其当前 handlers 的闭包（避免每次 render 重复注册/退订）。 */
type Subscriber = () => AgentBusHandlers | undefined;

const subscribers = new Set<Subscriber>();
let started = false;
let unlisteners: UnlistenFn[] = [];

/** 向所有订阅者派发某类回调。 */
function dispatch(method: keyof AgentBusHandlers, ...args: unknown[]): void {
  for (const get of subscribers) {
    const h = get();
    const fn = h?.[method];
    if (fn) {
      (fn as (...a: unknown[]) => void)(...args);
    }
  }
}

/** 惰性注册全部 agent-* 监听（仅一次；进程生命周期内常驻）。 */
function ensureStarted(): void {
  if (started) return;
  started = true;

  const registrations: Array<Promise<UnlistenFn>> = [
    listen<{ sessionId: string; content: string }>('agent-chunk', (event) => {
      dispatch('onChunk', event.payload.sessionId, event.payload.content);
    }),
    listen<{ sessionId: string; content?: string }>('agent-done', (event) => {
      dispatch('onDone', event.payload.sessionId, event.payload.content);
    }),
    listen<{ sessionId: string; error: string }>('agent-error', (event) => {
      dispatch('onError', event.payload.sessionId, event.payload.error);
    }),
    listen<{ sessionId: string; agentSessionId: string }>('agent-session', (event) => {
      dispatch('onSession', event.payload.sessionId, event.payload.agentSessionId);
    }),
    listen<{ sessionId: string; toolId: string; toolName: string; arguments: string; riskDescription: string }>(
      'agent-approval-required',
      (event) => {
        dispatch('onApprovalRequired', event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.arguments, event.payload.riskDescription);
      },
    ),
    listen<{
      sessionId: string;
      callId: string;
      title?: string;
      prompt: string;
      replyMode: 'open' | 'structured';
      items: Array<{ id: string; label: string; inputType: 'text' | 'select' | 'confirm'; options: string[]; required: boolean; placeholder?: string }>;
    }>('agent-confirmation-request', (event) => {
      dispatch('onConfirmationRequest', event.payload);
    }),
    listen<{ sessionId: string; toolId: string; toolName: string; arguments: string }>('agent-tool-start', (event) => {
      dispatch('onToolStart', event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.arguments);
    }),
    listen<{ sessionId: string; toolId: string; toolName: string; result: string; success: boolean }>('agent-tool-result', (event) => {
      dispatch('onToolResult', event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.result, event.payload.success);
    }),
    listen<{ sessionId: string; toolName: string; message: string }>('agent-tool-progress', (event) => {
      dispatch('onToolProgress', event.payload.sessionId, event.payload.toolName, event.payload.message);
    }),
    listen<{ sessionId: string; path: string; diff: string }>('agent-file-diff', (event) => {
      dispatch('onFileDiff', event.payload.sessionId, event.payload.path, event.payload.diff);
    }),
    listen<{ sessionId: string; content: string }>('agent-reasoning', (event) => {
      dispatch('onReasoning', event.payload.sessionId, event.payload.content);
    }),
    listen<{ sessionId: string; current: number; max: number }>('agent-iteration-limit', (event) => {
      dispatch('onIterationLimit', event.payload.sessionId, event.payload.current, event.payload.max);
    }),
    listen<{ sessionId: string; promptTokens: number; completionTokens: number; totalTokens: number }>('agent-usage', (event) => {
      dispatch('onUsage', event.payload.sessionId, {
        prompt: event.payload.promptTokens,
        completion: event.payload.completionTokens,
        total: event.payload.totalTokens,
      });
    }),
  ];

  // 进程生命周期内常驻：注册失败不致命，仅记录。
  Promise.allSettled(registrations).then((results) => {
    unlisteners = results
      .filter((r): r is PromiseFulfilledResult<UnlistenFn> => r.status === 'fulfilled')
      .map((r) => r.value);
  });
}

/** 订阅 agent-* 事件；返回退订函数（组件卸载时调用）。 */
export function subscribeAgentBus(get: Subscriber): () => void {
  subscribers.add(get);
  ensureStarted();
  return () => {
    subscribers.delete(get);
  };
}

export default subscribeAgentBus;
