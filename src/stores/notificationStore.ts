/**
 * notificationStore — 全局通知中心
 *
 * 背景：Toast 是命令式 DOM，8 秒后节点即被移除，错过就没有了；而"需要用户处理"的提示
 * （工具审批请求、工作流节点等待人工输入）只存在于当时那个页面（会话内联卡 / 工作流页横幅），
 * 用户切到别的模式就完全看不见。这里把所有通知收进一处，形成：
 * - 可回放的历史列表（点铃铛随时翻看）；
 * - 未决项（pending）置顶且保持未读，直到收到 `*-resolved` 才消解 —— 这样通知中心同时是待办入口。
 *
 * 写入来源：
 * - `showToast` 内部推送（现有约百处调用点零改动即入历史）；
 * - 事件订阅补充那些不走 Toast 的关键事件（见 `notificationEvents.ts`、`agentEventBus.ts`）。
 */
import { create } from 'zustand';

export type NotificationLevel = 'error' | 'warning' | 'info' | 'success';

/** 点击通知时的跳转目标 */
export interface NotificationTarget {
  /** session：跳到并选中该会话；workflow：切到工作流模式（处理待审批/待输入） */
  kind: 'session' | 'workflow';
  /** session id / execution id */
  id: string;
}

export interface NotificationItem {
  id: string;
  level: NotificationLevel;
  title: string;
  /** 明细：完整错误、审批工具参数等 */
  detail?: string;
  ts: number;
  /** 未决项：置顶显示，且在 `resolve` 之前不计入已读 */
  pending?: boolean;
  /** 未决项唯一键（`approval:<callId>` / `awaiting:<executionId>:<nodeId>`） */
  pendingKey?: string;
  /**
   * 事件唯一键（如 `exec-failed:<executionId>`）：**同一个事件**被重复投递时据此丢弃。
   * 刻意不按内容去重——两次不同执行撞了同样文案是两件事，合并会把独立消息吞掉。
   */
  dedupeKey?: string;
  target?: NotificationTarget;
  read: boolean;
}

export interface PushNotificationInput {
  level: NotificationLevel;
  title: string;
  detail?: string;
  pending?: boolean;
  pendingKey?: string;
  dedupeKey?: string;
  target?: NotificationTarget;
}

/** 保留上限：这里回答的是"最近发生了什么"，不追求永久留存 */
const MAX_ITEMS = 200;

/** 纯前端状态，localStorage 足够（与 `pilotdesk.session-list.*` 同一约定）。 */
const STORAGE_KEY = 'pilotdesk.notifications';

function loadItems(): NotificationItem[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((it): it is NotificationItem => !!it && typeof (it as NotificationItem).title === 'string');
  } catch {
    return [];
  }
}

function saveItems(items: NotificationItem[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(items));
  } catch {
    /* 配额/隐私模式失败可忽略：通知是尽力而为的辅助信息 */
  }
}

interface NotificationState {
  items: NotificationItem[];
  /** 抽屉展开态（纯 UI，不持久化） */
  open: boolean;
  push: (input: PushNotificationInput) => void;
  /** 消解未决项（收到 *-resolved / 执行结束时调用） */
  resolve: (pendingKey: string) => void;
  markRead: (id: string) => void;
  markAllRead: () => void;
  clearAll: () => void;
  setOpen: (open: boolean) => void;
}

export const useNotificationStore = create<NotificationState>((set) => ({
  items: loadItems(),
  open: false,

  push: (input) =>
    set((state) => {
      // 未决项去重：同一审批/同一节点的等待可能被重复投递（事件重放、页面重挂），只记一条
      if (input.pending && input.pendingKey && state.items.some((i) => i.pending && i.pendingKey === input.pendingKey)) {
        return state;
      }
      // 按**事件身份**去重：同一个事件被重复投递（事件重放、重复订阅、引擎多路广播）时丢弃。
      // 不做"同内容合并"：两次不同执行撞了相同文案是两件独立的事，合并会让用户以为只发生了一次
      // （而且真正的重复应该在生产端消除——一次失败只该由一个地方说一次）。
      if (input.dedupeKey && state.items.some((i) => i.dedupeKey === input.dedupeKey)) {
        return state;
      }
      const item: NotificationItem = {
        id: `${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
        level: input.level,
        title: input.title,
        detail: input.detail,
        ts: Date.now(),
        pending: input.pending,
        pendingKey: input.pendingKey,
        dedupeKey: input.dedupeKey,
        target: input.target,
        // success/info 是操作回执：进历史但不占未读徽标，避免徽标被日常操作刷满
        read: !input.pending && (input.level === 'success' || input.level === 'info'),
      };
      const items = [item, ...state.items].slice(0, MAX_ITEMS);
      saveItems(items);
      return { items };
    }),

  resolve: (pendingKey) =>
    set((state) => {
      const items = state.items.map((i) => (i.pending && i.pendingKey === pendingKey ? { ...i, pending: false, read: true } : i));
      saveItems(items);
      return { items };
    }),

  markRead: (id) =>
    set((state) => {
      const items = state.items.map((i) => (i.id === id ? { ...i, read: true } : i));
      saveItems(items);
      return { items };
    }),

  markAllRead: () =>
    set((state) => {
      // 未决项不算"已读"：它还需要用户处理，置顶与徽标必须保留
      const items = state.items.map((i) => (i.pending ? i : { ...i, read: true }));
      saveItems(items);
      return { items };
    }),

  clearAll: () =>
    set((state) => {
      // 清空只清历史；未决项（待审批/待输入）保留，否则用户会把待办一起清掉
      const items = state.items.filter((i) => i.pending);
      saveItems(items);
      return { items };
    }),

  setOpen: (open) => set({ open }),
}));

/** 未读条数（未决项恒未读）：用于顶栏铃铛徽标 */
export function countUnread(items: NotificationItem[]): number {
  return items.filter((i) => !i.read).length;
}

/** 展示顺序：未决项置顶，其余按时间倒序 */
export function sortForDisplay(items: NotificationItem[]): NotificationItem[] {
  return [...items].sort((a, b) => Number(!!b.pending) - Number(!!a.pending) || b.ts - a.ts);
}
