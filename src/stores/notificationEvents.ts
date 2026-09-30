/**
 * notificationEvents — 把"不走 Toast"的关键事件补进通知中心。
 *
 * 为什么单独一处：`showToast` 只能覆盖调用点当场触发的提示；而后台发生的事
 * （工作流节点在等人输入、后台工作流执行失败）发生时用户可能已经切到别的模式，
 * 甚至当时压根没打开过那个页面 —— 这类事件必须由全局常驻监听兜住，否则无人知晓。
 *
 * 全局单例：App 生命周期内注册一次，不随页面卸载（与 `subscribeGroupChat` 同一范式）。
 */
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useNotificationStore } from './notificationStore';
import type { ExecutionProgressPayload } from '../types/workflow';

/** 工作流节点等待人工输入（interact_executor 发出，节点会一直等，默认 30 分钟） */
interface AwaitingInputPayload {
  execution_id: string;
  node_id: string;
  prompt: string;
  input_type?: string;
  timeout_minutes?: number;
}

/** 统一执行进度事件（节点/阶段/执行三合一）直接用共享类型，避免两边各写一份再漂移 */

/** 工作流 Agent 节点命中「工具授权」策略，暂停等待用户裁决 */
interface ApprovalRequiredPayload {
  execution_id: string;
  node_id: string;
  /** 节点名（阶段名需查定义，通知里只显示节点名） */
  node_label?: string;
  call_id: string;
  tool_name: string;
  arguments: string;
  risk: string;
}

/** 审批已被裁决（用户点了批准/拒绝，或等待超时按拒绝处理） */
interface ApprovalResolvedPayload {
  execution_id: string;
  node_id: string;
  call_id: string;
  approved: boolean;
  timed_out: boolean;
}

const TERMINAL_STATUSES = ['success', 'failed', 'cancelled', 'timeout'];

/**
 * 「自动运行提醒」开关（默认开）。
 *
 * 存 localStorage 而不是 app_settings：这是本机 UI 偏好，与指挥中心入口指引同一范式，
 * 不必为一个开关走一次 IPC。关掉后定时/事件触发的启动不再进通知中心。
 */
const AUTO_RUN_NOTIFY_KEY = 'pd_workflow_auto_run_notify';

export const isAutoRunNotifyEnabled = () => localStorage.getItem(AUTO_RUN_NOTIFY_KEY) !== '0';

export const setAutoRunNotifyEnabled = (enabled: boolean) => {
  localStorage.setItem(AUTO_RUN_NOTIFY_KEY, enabled ? '1' : '0');
};

/** 同一定义 1 分钟内只提醒一次：定时任务密集触发时不该把通知中心刷满 */
const AUTO_RUN_NOTIFY_THROTTLE_MS = 60_000;
const lastAutoRunNotifyAt = new Map<string, number>();

let started = false;
const unlisteners: UnlistenFn[] = [];

/** 某个 execution 下所有"等待输入"未决项的前缀 */
const awaitingPrefix = (executionId: string) => `awaiting:${executionId}:`;

/**
 * 待输入的未决键：`awaiting:<executionId>:<nodeId>`。
 * 用户答复（workflowStore.respondHumanInput）与执行结束都要按它消解，故集中在此定义，
 * 避免两处各拼一份格式。
 */
export const awaitingPendingKey = (executionId: string, nodeId: string) =>
  `${awaitingPrefix(executionId)}${nodeId}`;

/**
 * 工作流工具审批的未决键：`wf-approval:<callId>`。
 *
 * 前缀刻意与会话审批的 `approval:<toolId>` 区分开：两者都用工具调用 id，若前缀相同，
 * 通知中心的消解会互相串（一边点了另一边也变已处理）。
 */
export const approvalPendingKey = (callId: string) => `wf-approval:${callId}`;

export function subscribeNotifications(): void {
  if (started) return;
  started = true;

  listen<AwaitingInputPayload>('workflow:awaiting-input', (event) => {
    const p = event.payload;
    useNotificationStore.getState().push({
      level: 'warning',
      title: '工作流节点等待人工输入',
      detail: p.prompt,
      pending: true,
      pendingKey: awaitingPendingKey(p.execution_id, p.node_id),
      target: { kind: 'workflow', id: p.execution_id },
    });
  }).then((fn) => unlisteners.push(fn));

  // 工作流 Agent 节点命中「工具授权」策略：节点会暂停等待，用户不处理就会一直挂着。
  // 与人工输入同为未决项（置顶、未读），点击跳到工作流页裁决。
  listen<ApprovalRequiredPayload>('workflow:approval-required', (event) => {
    const p = event.payload;
    useNotificationStore.getState().push({
      level: 'warning',
      title: '工作流节点等待工具审批',
      detail: `${p.node_label ? `${p.node_label} · ` : ''}${p.tool_name}（风险：${p.risk}）\n${p.arguments}`,
      pending: true,
      pendingKey: approvalPendingKey(p.call_id),
      target: { kind: 'workflow', id: p.execution_id },
    });
  }).then((fn) => unlisteners.push(fn));

  listen<ApprovalResolvedPayload>('workflow:approval-resolved', (event) => {
    const p = event.payload;
    // 裁决完成（含超时按拒绝）：未决项随之消解；超时需明确告知，否则用户以为自己的选择生效了
    useNotificationStore.getState().resolve(approvalPendingKey(p.call_id));
    if (p.timed_out) {
      useNotificationStore.getState().push({
        level: 'warning',
        title: '工具审批等待超时，已按拒绝处理',
        detail: `执行 ${p.execution_id} 的节点 ${p.node_id} 未在时限内得到裁决`,
        target: { kind: 'workflow', id: p.execution_id },
      });
    }
  }).then((fn) => unlisteners.push(fn));

  listen<ExecutionProgressPayload>('workflow:execution-progress', (event) => {
    const p = event.payload;
    const exec = p.execution;
    if (!exec?.status) return;

    // 自动触发（定时/事件）刚刚启动：用户没有点过"运行"，必须留一条"它跑起来了"的凭证，
    // 否则自动运行全程无感；手动运行本身有即时反馈，不提醒。
    // 用 info 级：进历史但不占未读徽标（见 notificationStore.push）。
    if (
      exec.status === 'running'
      && p.trigger_kind
      && p.trigger_kind !== 'manual'
      && isAutoRunNotifyEnabled()
    ) {
      const nowMs = Date.now();
      const lastAt = lastAutoRunNotifyAt.get(p.definition_id) ?? 0;
      if (nowMs - lastAt >= AUTO_RUN_NOTIFY_THROTTLE_MS) {
        lastAutoRunNotifyAt.set(p.definition_id, nowMs);
        useNotificationStore.getState().push({
          level: 'info',
          title: `${exec.definition_name || p.definition_id} 已由${p.trigger_kind === 'cron' ? '定时' : '事件'}触发启动`,
          detail: '自动运行已开始，可到工作流页查看运行进度',
          target: { kind: 'workflow', id: p.execution_id },
        });
      }
    }

    // 后台工作流失败：用户很可能已经切走，必须留一条可回放的通知。
    // dedupeKey 用 executionId：这是"一个执行一条"，同一次执行的事件被重复投递只会记一条；
    // 不同执行即便文案相同也是两件事，各自成条。
    if (exec.status === 'failed') {
      useNotificationStore.getState().push({
        level: 'error',
        title: `${exec.definition_name || p.definition_id} 执行失败`,
        detail: exec.error,
        dedupeKey: `exec-failed:${p.execution_id}`,
        target: { kind: 'workflow', id: p.execution_id },
      });
    }

    // 执行已结束：该执行下所有未决项随之消解（人工输入与工具审批都不再有人等）
    if (TERMINAL_STATUSES.includes(exec.status)) {
      const store = useNotificationStore.getState();
      const prefix = awaitingPrefix(p.execution_id);
      store.items
        .filter((i) => i.pending && (
          i.pendingKey?.startsWith(prefix)
          || (i.target?.kind === 'workflow' && i.target.id === p.execution_id)
        ))
        .forEach((i) => store.resolve(i.pendingKey!));
    }
  }).then((fn) => unlisteners.push(fn));
}
