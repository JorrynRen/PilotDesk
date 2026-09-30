/**
 * 会话流式态单例（跨组件卸载存活）。
 *
 * 一次 Agent 运行是后端的独立后台任务（`run_api_agent` 内 `tokio::spawn`），生命周期与前端
 * 组件无关；但流式正文、思维链与"完成即落库"此前都长在 MainPanel 组件内部。用户切到别的
 * 页面会让 MainPanel 卸载 —— 累积内容随组件销毁、事件总线无订阅者、落库副作用永不执行，
 * 结果是"后端跑完了，回答也永久丢失"。
 *
 * 因此把「累积 + 收尾落库」上移到本模块：事件总线收到事件时直接写入这里，组件只读不拥有。
 * 无论 MainPanel 是否挂载，回答都会被保存进会话。
 *
 * 步骤构造与序列化（`append*Step` / `serializeChain`）是纯函数，同时被本模块与
 * MainPanel 的渲染 reducer 使用，避免两处各写一份。
 */
import { create } from 'zustand';
import type { ThinkingChainStep } from '../components/layout/MainPanel';
import { useApprovalStore } from './approvalStore';
import { useGeneratingStore } from './generatingStore';
import { useSessionStore } from './sessionStore';

/** 单会话的流式态：已累积正文 + 已累积思维链步骤。 */
export interface SessionStreamState {
  content: string;
  steps: ThinkingChainStep[];
}

const EMPTY_STREAM: SessionStreamState = { content: '', steps: [] };

/** 合并连续的 reasoning 分片为同一步骤（与流式渲染一致：同一段思考不应碎成多步）。 */
export function appendReasoningStep(steps: ThinkingChainStep[], content: string): ThinkingChainStep[] {
  const last = steps[steps.length - 1];
  if (last && last.type === 'reasoning') {
    return [...steps.slice(0, -1), { ...last, content: (last.content || '') + content }];
  }
  return [...steps, { id: `reasoning-${Date.now()}`, type: 'reasoning', content, timestamp: Date.now() }];
}

export function appendToolStartStep(
  steps: ThinkingChainStep[],
  toolId: string,
  toolName: string,
  toolArgs: string,
): ThinkingChainStep[] {
  return [
    ...steps,
    { id: toolId, type: 'tool_start', toolName, toolArgs, timestamp: Date.now() },
  ];
}

export function appendToolResultStep(
  steps: ThinkingChainStep[],
  toolId: string,
  toolName: string,
  result: string,
  success: boolean,
): ThinkingChainStep[] {
  return [
    ...steps,
    { id: `result-${toolId}`, type: 'tool_result', toolName, toolResult: result, toolSuccess: success, timestamp: Date.now() },
  ];
}

export function appendFileDiffStep(
  steps: ThinkingChainStep[],
  path: string,
  diff: string,
): ThinkingChainStep[] {
  return [
    ...steps,
    {
      // id 唯一化：同毫秒可能收到多个 file_diff（一次改多文件），Date.now() 会重复，加随机后缀。
      id: `diff-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      type: 'file_diff',
      filePath: path,
      fileDiff: diff,
      timestamp: Date.now(),
    },
  ];
}

export function appendSystemStep(
  steps: ThinkingChainStep[],
  stepId: string,
  title: string,
  params: string,
  detail: string,
  success: boolean,
): ThinkingChainStep[] {
  // 轮前系统步骤：单事件、无 start/result 配对，按 stepId 去重即可。
  if (steps.some((s) => s.type === 'system' && s.id === stepId)) return steps;
  return [
    ...steps,
    { id: stepId, type: 'system', sysTitle: title, sysParams: params, sysDetail: detail, sysSuccess: success, timestamp: Date.now() },
  ];
}

/** 内联审批卡步骤：同一 callId 可能重复到达，按 callId 去重。 */
export function appendApprovalStep(
  steps: ThinkingChainStep[],
  callId: string,
  toolName: string,
  toolArgs: string,
  risk: string,
  deadline: number,
): ThinkingChainStep[] {
  if (steps.some((s) => s.type === 'approval' && s.approvalCallId === callId)) return steps;
  return [
    ...steps,
    {
      id: `approval-${callId}`,
      type: 'approval',
      toolName,
      toolArgs,
      approvalCallId: callId,
      approvalRisk: risk,
      approvalDeadline: deadline,
      timestamp: Date.now(),
    },
  ];
}

/** 审批裁决写回链条；无对应步骤时原样返回。 */
export function resolveApprovalStep(
  steps: ThinkingChainStep[],
  callId: string,
  approved: boolean,
  timedOut: boolean,
): ThinkingChainStep[] {
  let found = false;
  const next = steps.map((s) => {
    if (s.type === 'approval' && s.approvalCallId === callId) {
      found = true;
      return { ...s, approvalApproved: approved, approvalTimedOut: timedOut };
    }
    return s;
  });
  return found ? next : steps;
}

/** 内联迭代上限卡步骤：同一会话已有未决卡时忽略重复事件。 */
export function appendIterationLimitStep(
  steps: ThinkingChainStep[],
  id: string,
  current: number,
  max: number,
  deadline: number,
): ThinkingChainStep[] {
  if (steps.some((s) => s.type === 'iteration_limit' && s.iterDecision === undefined)) return steps;
  return [
    ...steps,
    {
      id: `iteration-limit-${id}`,
      type: 'iteration_limit',
      iterId: id,
      iterCurrent: current,
      iterMax: max,
      iterDeadline: deadline,
      timestamp: Date.now(),
    },
  ];
}

/** 迭代上限裁决写回未决卡（按未决状态定位，与卡的 id 无关）；无未决卡时原样返回。 */
export function resolveIterationLimitStep(
  steps: ThinkingChainStep[],
  decision: 'continue' | 'stop',
  timedOut: boolean,
): ThinkingChainStep[] {
  let found = false;
  const next = steps.map((s) => {
    if (s.type === 'iteration_limit' && s.iterDecision === undefined) {
      found = true;
      return { ...s, iterDecision: decision, iterTimedOut: timedOut };
    }
    return s;
  });
  return found ? next : steps;
}

/** 序列化思维链为 `tool_calls` 列内容（该列承载完整思维链）；无步骤返回 undefined。 */
export function serializeChain(steps: ThinkingChainStep[]): string | undefined {
  if (steps.length === 0) return undefined;
  return JSON.stringify(
    steps.map((s) => ({
      id: s.id,
      type: s.type,
      content: s.type === 'reasoning' ? s.content : undefined,
      toolName: s.type === 'tool_start' || s.type === 'tool_result' || s.type === 'approval' ? s.toolName : undefined,
      args: s.type === 'tool_start' || s.type === 'approval' ? s.toolArgs : undefined,
      result: s.type === 'tool_result' ? s.toolResult : undefined,
      success: s.type === 'tool_result' ? s.toolSuccess : undefined,
      filePath: s.type === 'file_diff' ? s.filePath : undefined,
      fileDiff: s.type === 'file_diff' ? s.fileDiff : undefined,
      // 内联审批卡步骤：完整保留审批字段，供历史回看时重建卡片状态
      approvalCallId: s.type === 'approval' ? s.approvalCallId : undefined,
      approvalApproved: s.type === 'approval' ? s.approvalApproved : undefined,
      approvalTimedOut: s.type === 'approval' ? s.approvalTimedOut : undefined,
      approvalRisk: s.type === 'approval' ? s.approvalRisk : undefined,
      approvalDeadline: s.type === 'approval' ? s.approvalDeadline : undefined,
      // 内联迭代上限卡步骤：完整保留字段，供历史回看时重建卡片状态
      iterId: s.type === 'iteration_limit' ? s.iterId : undefined,
      iterCurrent: s.type === 'iteration_limit' ? s.iterCurrent : undefined,
      iterMax: s.type === 'iteration_limit' ? s.iterMax : undefined,
      iterDeadline: s.type === 'iteration_limit' ? s.iterDeadline : undefined,
      iterDecision: s.type === 'iteration_limit' ? s.iterDecision : undefined,
      iterTimedOut: s.type === 'iteration_limit' ? s.iterTimedOut : undefined,
      // 轮前系统步骤：完整保留，供历史回看时重建
      sysTitle: s.type === 'system' ? s.sysTitle : undefined,
      sysParams: s.type === 'system' ? s.sysParams : undefined,
      sysDetail: s.type === 'system' ? s.sysDetail : undefined,
      sysSuccess: s.type === 'system' ? s.sysSuccess : undefined,
      ts: s.timestamp,
    })),
  );
}

interface AgentStreamStore {
  streams: Record<string, SessionStreamState>;

  appendChunk: (sessionId: string, content: string) => void;
  appendReasoning: (sessionId: string, content: string) => void;
  startTool: (sessionId: string, toolId: string, toolName: string, toolArgs: string) => void;
  finishTool: (sessionId: string, toolId: string, toolName: string, result: string, success: boolean) => void;
  addFileDiff: (sessionId: string, path: string, diff: string) => void;
  addSystemStep: (sessionId: string, stepId: string, title: string, params: string, detail: string, success: boolean) => void;
  /**
   * 内联审批卡 / 迭代上限卡：与工具步骤一样必须落在本单例里 ——
   * 落库链取的是这里的 steps，只在组件 reducer 里记录会让卡片在消息中消失。
   */
  addApproval: (sessionId: string, callId: string, toolName: string, toolArgs: string, risk: string, deadline: number) => void;
  resolveApproval: (sessionId: string, callId: string, approved: boolean, timedOut: boolean) => void;
  addIterationLimit: (sessionId: string, id: string, current: number, max: number, deadline: number) => void;
  resolveIterationLimit: (sessionId: string, decision: 'continue' | 'stop', timedOut: boolean) => void;

  /**
   * 收尾落库：把本轮正文与思维链写成 assistant 消息；无正文也无思维链时写终止提示。
   * `notifyEmpty` 为 false 时不写终止提示（调用方另有系统消息要写，例如运行失败）。
   */
  finishRun: (sessionId: string, fallbackContent?: string, notifyEmpty?: boolean) => void;
}

export const useAgentStreamStore = create<AgentStreamStore>((set, get) => {
  /** 就地更新某会话的流式态（无记录时以空态起底）。 */
  const patch = (sessionId: string, fn: (prev: SessionStreamState) => SessionStreamState) => {
    set((state) => {
      const prev = state.streams[sessionId] ?? EMPTY_STREAM;
      return { streams: { ...state.streams, [sessionId]: fn(prev) } };
    });
  };

  const drop = (sessionId: string) => {
    set((state) => {
      if (!(sessionId in state.streams)) return state;
      const { [sessionId]: _removed, ...rest } = state.streams;
      return { streams: rest };
    });
  };

  return {
    streams: {},

    appendChunk: (sessionId, content) =>
      patch(sessionId, (p) => ({ ...p, content: p.content + content })),
    appendReasoning: (sessionId, content) =>
      patch(sessionId, (p) => ({ ...p, steps: appendReasoningStep(p.steps, content) })),
    startTool: (sessionId, toolId, toolName, toolArgs) =>
      patch(sessionId, (p) => ({ ...p, steps: appendToolStartStep(p.steps, toolId, toolName, toolArgs) })),
    finishTool: (sessionId, toolId, toolName, result, success) =>
      patch(sessionId, (p) => ({ ...p, steps: appendToolResultStep(p.steps, toolId, toolName, result, success) })),
    addFileDiff: (sessionId, path, diff) =>
      patch(sessionId, (p) => ({ ...p, steps: appendFileDiffStep(p.steps, path, diff) })),
    addSystemStep: (sessionId, stepId, title, params, detail, success) =>
      patch(sessionId, (p) => ({ ...p, steps: appendSystemStep(p.steps, stepId, title, params, detail, success) })),
    addApproval: (sessionId, callId, toolName, toolArgs, risk, deadline) =>
      patch(sessionId, (p) => ({ ...p, steps: appendApprovalStep(p.steps, callId, toolName, toolArgs, risk, deadline) })),
    resolveApproval: (sessionId, callId, approved, timedOut) =>
      patch(sessionId, (p) => ({ ...p, steps: resolveApprovalStep(p.steps, callId, approved, timedOut) })),
    addIterationLimit: (sessionId, id, current, max, deadline) =>
      patch(sessionId, (p) => ({ ...p, steps: appendIterationLimitStep(p.steps, id, current, max, deadline) })),
    resolveIterationLimit: (sessionId, decision, timedOut) =>
      patch(sessionId, (p) => ({ ...p, steps: resolveIterationLimitStep(p.steps, decision, timedOut) })),

    finishRun: (sessionId, fallbackContent, notifyEmpty = true) => {
      const stream = get().streams[sessionId] ?? EMPTY_STREAM;
      const toolCalls = serializeChain(stream.steps);
      // 事件携带的正文是 Rust 端 content_buffer 的完整克隆，比前端分块累积更可靠（可能漏 chunk）；
      // 与流式渲染一致，取更长的一份作为最终文本。
      const msgContent = (fallbackContent?.length ?? 0) >= stream.content.length
        ? (fallbackContent ?? stream.content)
        : stream.content;

      drop(sessionId);
      // 审批项已随本轮思维链持久化进 assistant 消息，会话级集合随之清理，
      // 避免下一轮激活时把上一轮的审批卡回填进新链条。
      useApprovalStore.getState().clearSession(sessionId);
      useGeneratingStore.getState().removeGenerating(sessionId);

      if (msgContent || toolCalls) {
        useSessionStore.getState().addMessage({
          id: `msg-${Date.now()}`,
          sessionId,
          role: 'assistant',
          content: msgContent || '',
          mode: 'native',
          timestamp: Math.floor(Date.now() / 1000),
          toolCalls,
        });
        return;
      }

      // 无正文也无思维链：默认写终止提示；调用方另有系统消息时（如运行失败）跳过。
      if (!notifyEmpty) return;

      useSessionStore.getState().addMessage({
        id: `msg-sys-${Date.now()}`,
        sessionId,
        role: 'system',
        content: '⏱ 请求已终止：未收到模型有效响应。请重试或检查 API 配置。',
        mode: 'native',
        // 延迟 1s 确保 timestamp 严格晚于 assistant 消息
        timestamp: Math.floor(Date.now() / 1000) + 1,
      });
    },
  };
});
