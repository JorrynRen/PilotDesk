import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAgentStreamStore } from '../stores/agentStreamStore';
import { useApprovalStore } from '../stores/approvalStore';
import { useGeneratingStore } from '../stores/generatingStore';
import { useNotificationStore } from '../stores/notificationStore';

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
  /** 运行被取消（用户点"停止生成"）：已产出的正文已在总线层落库，组件只需收尾自己视图。 */
  onCancelled?: (sessionId: string, content?: string) => void;
  onError?: (sessionId: string, error: string) => void;
  onSession?: (sessionId: string, agentSessionId: string) => void;
  onApprovalRequired?: (sessionId: string, callId: string, toolName: string, toolArgs: string, riskDescription: string) => void;
  onApprovalResolved?: (sessionId: string, callId: string, toolName: string, approved: boolean, timedOut: boolean, risk: string) => void;
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
  /** 轮前系统步骤（如自动记忆检索）：非模型发起的工具调用，在思维链中独立成一步。 */
  onSystemStep?: (sessionId: string, stepId: string, title: string, params: string, detail: string, success: boolean) => void;
  onReasoning?: (sessionId: string, content: string) => void;
  onIterationLimit?: (sessionId: string, current: number, max: number, id: string, deadline: number) => void;
  onIterationLimitResolved?: (sessionId: string, shouldContinue: boolean, timedOut: boolean) => void;
  onUsage?: (sessionId: string, tokens: { prompt: number; completion: number; total: number }) => void;
}

/** 订阅者：返回其当前 handlers 的闭包（避免每次 render 重复注册/退订）。 */
type Subscriber = () => AgentBusHandlers | undefined;

const subscribers = new Set<Subscriber>();
let started = false;

/** 终态事件：记录它们会让已结束的会话留下"刚有活动"的痕迹，故不打活动时间戳。 */
const TERMINAL_METHODS: ReadonlySet<string> = new Set(['onDone', 'onCancelled', 'onError']);

/** 取事件所属会话：除 onConfirmationRequest 是单对象载荷外，其余第一个参数都是 sessionId。 */
function eventSessionId(method: keyof AgentBusHandlers, args: unknown[]): string | undefined {
  if (method === 'onConfirmationRequest') {
    return (args[0] as { sessionId?: string } | undefined)?.sessionId;
  }
  const sessionId = args[0];
  return typeof sessionId === 'string' && sessionId ? sessionId : undefined;
}

/** 向所有订阅者派发某类回调。 */
function dispatch(method: keyof AgentBusHandlers, ...args: unknown[]): void {
  // 统一记录"该会话刚有事件"，供会话页安全网区分"静默卡死"与"整轮耗时长"。
  const sessionId = eventSessionId(method, args);
  if (sessionId && !TERMINAL_METHODS.has(method)) {
    useGeneratingStore.getState().touch(sessionId);
  }
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
      // 流式正文在总线层写单例 store：MainPanel 卸载期间事件没有订阅者（总线无监听者即丢弃），
      // 内容与思维链会随组件销毁而丢失；写入单例后重挂可直接回填，且收尾落库不依赖组件存活。
      useAgentStreamStore.getState().appendChunk(event.payload.sessionId, event.payload.content);
      useGeneratingStore.getState().markGenerating(event.payload.sessionId);
      dispatch('onChunk', event.payload.sessionId, event.payload.content);
    }),
    listen<{ sessionId: string; content?: string }>('agent-done', (event) => {
      // 收尾落库在总线层完成（组件挂不挂载都要保存），组件只负责清掉自己的流式视图。
      useAgentStreamStore.getState().finishRun(event.payload.sessionId, event.payload.content);
      dispatch('onDone', event.payload.sessionId, event.payload.content);
    }),
    listen<{ sessionId: string; content?: string }>('agent-cancelled', (event) => {
      // 用户点"停止生成"：中断时已产出的正文照常收尾落库，而不是按失败丢弃。
      useAgentStreamStore.getState().finishRun(event.payload.sessionId, event.payload.content);
      dispatch('onCancelled', event.payload.sessionId, event.payload.content);
    }),
    listen<{ sessionId: string; error: string }>('agent-error', (event) => {
      // 运行失败：已产出的内容照常收尾落库（与既有行为一致——失败前的内容不丢），
      // 但不写"未收到响应"提示：失败提示由组件按 agent-error 写入，避免两条系统消息。
      useAgentStreamStore.getState().finishRun(event.payload.sessionId, undefined, false);
      dispatch('onError', event.payload.sessionId, event.payload.error);
    }),
    listen<{ sessionId: string; agentSessionId: string }>('agent-session', (event) => {
      dispatch('onSession', event.payload.sessionId, event.payload.agentSessionId);
    }),
    listen<{ sessionId: string; toolId: string; toolName: string; arguments: string; riskDescription: string }>(
      'agent-approval-required',
      (event) => {
        // 审批集合在总线层写单例 store，不依赖 MainPanel 是否挂载：
        // 用户切到群聊/自定义页时 MainPanel 会卸载，若只在组件里记录，审批会随卸载丢失。
        const deadline = Date.now() + 120_000;
        useApprovalStore.getState().add(event.payload.sessionId, {
          callId: event.payload.toolId,
          toolName: event.payload.toolName,
          args: event.payload.arguments,
          risk: event.payload.riskDescription,
          deadline,
          ts: Date.now(),
        });
        // 审批卡同样是思维链的一步：落库链取自 agentStreamStore，只写集合会让卡片在消息里消失。
        useAgentStreamStore.getState().addApproval(
          event.payload.sessionId,
          event.payload.toolId,
          event.payload.toolName,
          event.payload.arguments,
          event.payload.riskDescription,
          deadline,
        );
        // 通知中心：审批是"未决项"，用户切走也要看得到，直到 resolved 才消解
        useNotificationStore.getState().push({
          level: 'warning',
          title: `${event.payload.toolName} 需要审批`,
          detail: [event.payload.riskDescription, event.payload.arguments].filter(Boolean).join('\n'),
          pending: true,
          pendingKey: `approval:${event.payload.toolId}`,
          target: { kind: 'session', id: event.payload.sessionId },
        });
        dispatch('onApprovalRequired', event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.arguments, event.payload.riskDescription);
      },
    ),
    listen<{ sessionId: string; toolId: string; toolName: string; approved: boolean; timedOut: boolean; risk: string }>(
      'agent-approval-resolved',
      (event) => {
        useApprovalStore.getState().resolve(event.payload.sessionId, event.payload.toolId, event.payload.approved, event.payload.timedOut);
        useAgentStreamStore.getState().resolveApproval(
          event.payload.sessionId,
          event.payload.toolId,
          event.payload.approved,
          event.payload.timedOut,
        );
        // 审批已决（含超时）：通知中心的未决项随之消解
        useNotificationStore.getState().resolve(`approval:${event.payload.toolId}`);
        dispatch('onApprovalResolved', event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.approved, event.payload.timedOut, event.payload.risk);
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
      useAgentStreamStore.getState().startTool(event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.arguments);
      dispatch('onToolStart', event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.arguments);
    }),
    listen<{ sessionId: string; toolId: string; toolName: string; result: string; success: boolean }>('agent-tool-result', (event) => {
      useAgentStreamStore.getState().finishTool(event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.result, event.payload.success);
      dispatch('onToolResult', event.payload.sessionId, event.payload.toolId, event.payload.toolName, event.payload.result, event.payload.success);
    }),
    listen<{ sessionId: string; toolName: string; message: string }>('agent-tool-progress', (event) => {
      dispatch('onToolProgress', event.payload.sessionId, event.payload.toolName, event.payload.message);
    }),
    listen<{ sessionId: string; path: string; diff: string }>('agent-file-diff', (event) => {
      useAgentStreamStore.getState().addFileDiff(event.payload.sessionId, event.payload.path, event.payload.diff);
      dispatch('onFileDiff', event.payload.sessionId, event.payload.path, event.payload.diff);
    }),
    listen<{ sessionId: string; stepId: string; title: string; params: string; detail: string; success: boolean }>('agent-system-step', (event) => {
      useAgentStreamStore.getState().addSystemStep(event.payload.sessionId, event.payload.stepId, event.payload.title, event.payload.params, event.payload.detail, event.payload.success);
      dispatch('onSystemStep', event.payload.sessionId, event.payload.stepId, event.payload.title, event.payload.params, event.payload.detail, event.payload.success);
    }),
    listen<{ sessionId: string; content: string }>('agent-reasoning', (event) => {
      useAgentStreamStore.getState().appendReasoning(event.payload.sessionId, event.payload.content);
      dispatch('onReasoning', event.payload.sessionId, event.payload.content);
    }),
    listen<{ sessionId: string; current: number; max: number }>('agent-iteration-limit', (event) => {
      // 迭代上限集合在总线层写单例 store，不依赖 MainPanel 是否挂载：
      // 用户切到群聊/自定义页时 MainPanel 会卸载，若只在组件里记录，迭代上限会随卸载丢失。
      const { sessionId, current, max } = event.payload;
      // 稳定 id 在此生成，同时交给 store 与链路步骤，保证卸载期间到达的请求回填时能对应上。
      const id = `iter-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const deadline = Date.now() + 120_000;
      useApprovalStore.getState().addIterationLimit(sessionId, { id, current, max, deadline, ts: Date.now() });
      // 同审批卡：迭代上限卡也是思维链的一步，落库链取自 agentStreamStore。
      useAgentStreamStore.getState().addIterationLimit(sessionId, id, current, max, deadline);
      // 通知中心：同样是"未决项"。iteration-limit-resolved 不带 id，故按会话作未决键。
      useNotificationStore.getState().push({
        level: 'warning',
        title: '已达到迭代上限，等待确认是否继续',
        detail: `已执行 ${current} / 上限 ${max} 轮`,
        pending: true,
        pendingKey: `iterlimit:${sessionId}`,
        target: { kind: 'session', id: sessionId },
      });
      dispatch('onIterationLimit', sessionId, current, max, id, deadline);
    }),
    listen<{ sessionId: string; shouldContinue: boolean; timedOut: boolean }>('agent-iteration-limit-resolved', (event) => {
      const { sessionId, shouldContinue, timedOut } = event.payload;
      const decision = shouldContinue ? 'continue' : 'stop';
      useApprovalStore.getState().resolveIterationLimit(sessionId, decision, timedOut);
      useAgentStreamStore.getState().resolveIterationLimit(sessionId, decision, timedOut);
      useNotificationStore.getState().resolve(`iterlimit:${sessionId}`);
      dispatch('onIterationLimitResolved', sessionId, shouldContinue, timedOut);
    }),
    listen<{ sessionId: string; promptTokens: number; completionTokens: number; totalTokens: number }>('agent-usage', (event) => {
      dispatch('onUsage', event.payload.sessionId, {
        prompt: event.payload.promptTokens,
        completion: event.payload.completionTokens,
        total: event.payload.totalTokens,
      });
    }),
  ];

  // 进程生命周期内常驻，无需退订：注册失败不致命，仅吞掉 rejection。
  void Promise.allSettled(registrations);
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
