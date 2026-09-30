import { create } from 'zustand';

/** 一条工具审批（内联审批卡与会话列表徽标共享）。
 *
 * 保留已决项（而非移除）的原因：用户可能切到群聊页导致 MainPanel 卸载，
 * 期间到达的审批请求在事件总线上写入本集合；切回会话页后据此回填内联卡，
 * 保证"切换页面/会话后审批不丢失、且能看到它已被如何处理"。 */
export interface ApprovalItem {
  callId: string;
  toolName: string;
  args: string;
  risk: string;
  /** 等待截止时间戳（ms），与后端 120s 超时对齐 */
  deadline: number;
  /** 事件到达时间戳（ms），用于回填时按序插入思维链 */
  ts: number;
  /** 已决结果：undefined = 待审批 */
  approved?: boolean;
  /** 是否因超时自动裁决 */
  timedOut?: boolean;
}

/** 一条"迭代上限"确认（内联迭代上限卡与审批卡同构）。
 *
 * 保留已决项（而非移除）的原因：用户可能切到群聊页导致 MainPanel 卸载，
 * 期间到达的迭代上限请求在事件总线上写入本集合；切回会话页后据此回填内联卡，
 * 保证"切换页面/会话后迭代上限不丢失、且能看到它已被如何处理"。 */
export interface IterationLimitItem {
  /** 稳定标识：由总线在收到 agent-iteration-limit 时生成，链路步骤与 store 共用同一 id */
  id: string;
  current: number;
  max: number;
  /** 等待截止时间戳（ms），与后端 120s 超时对齐 */
  deadline: number;
  /** 事件到达时间戳（ms），用于回填时按序插入思维链 */
  ts: number;
  /** 已决结果：undefined = 未决 */
  decision?: 'continue' | 'stop';
  /** 是否因超时自动裁决 */
  timedOut?: boolean;
}

interface ApprovalState {
  /** 会话 ID → 该会话的审批项（含已决项） */
  items: Record<string, ApprovalItem[]>;
  /** 会话 ID → 该会话的迭代上限项（含已决项） */
  iterationLimits: Record<string, IterationLimitItem[]>;
  add: (sessionId: string, item: Omit<ApprovalItem, 'approved' | 'timedOut'>) => void;
  resolve: (sessionId: string, callId: string, approved: boolean, timedOut: boolean) => void;
  addIterationLimit: (sessionId: string, item: Omit<IterationLimitItem, 'decision' | 'timedOut'>) => void;
  resolveIterationLimit: (sessionId: string, decision: 'continue' | 'stop', timedOut: boolean) => void;
  clearSession: (sessionId: string) => void;
}

export const useApprovalStore = create<ApprovalState>((set) => ({
  items: {},
  iterationLimits: {},
  add: (sessionId, item) => set((state) => {
    const list = state.items[sessionId] ?? [];
    // 按 callId 去重：重复收到同一审批请求时忽略
    if (list.some((x) => x.callId === item.callId)) return state;
    return { items: { ...state.items, [sessionId]: [...list, item] } };
  }),
  resolve: (sessionId, callId, approved, timedOut) => set((state) => {
    const list = state.items[sessionId];
    if (!list) return state;
    let hit = false;
    const next = list.map((x) => {
      if (x.callId !== callId) return x;
      hit = true;
      return { ...x, approved, timedOut };
    });
    if (!hit) return state;
    return { items: { ...state.items, [sessionId]: next } };
  }),
  addIterationLimit: (sessionId, item) => set((state) => {
    const list = state.iterationLimits[sessionId] ?? [];
    // 按 id 去重：重复收到同一迭代上限请求时忽略
    if (list.some((x) => x.id === item.id)) return state;
    return { iterationLimits: { ...state.iterationLimits, [sessionId]: [...list, item] } };
  }),
  resolveIterationLimit: (sessionId, decision, timedOut) => set((state) => {
    const list = state.iterationLimits[sessionId];
    if (!list) return state;
    // 后端 resolved 事件不带 id：标记该会话第一个未决项（同一会话同一时刻最多一个未决）
    let hit = false;
    const next = list.map((x) => {
      if (hit || x.decision !== undefined) return x;
      hit = true;
      return { ...x, decision, timedOut };
    });
    if (!hit) return state;
    return { iterationLimits: { ...state.iterationLimits, [sessionId]: next } };
  }),
  clearSession: (sessionId) => set((state) => {
    if (!state.items[sessionId] && !state.iterationLimits[sessionId]) return state;
    const items = { ...state.items };
    const iterationLimits = { ...state.iterationLimits };
    delete items[sessionId];
    delete iterationLimits[sessionId];
    return { items, iterationLimits };
  }),
}));

/** 选择器：指定会话待审批数量（返回 number，Zustand 可正确比较）。 */
export const selectPendingCount = (sessionId: string) => (state: ApprovalState) =>
  (state.items[sessionId] ?? []).filter((x) => x.approved === undefined).length;

/** 选择器：指定会话的审批项（含已决），供会话激活时回填内联卡。 */
export const selectApprovalItems = (sessionId: string) => (state: ApprovalState) =>
  state.items[sessionId] ?? [];

/** 选择器：指定会话的迭代上限项（含已决），供会话激活时回填内联卡。 */
export const selectIterationLimitItems = (sessionId: string) => (state: ApprovalState) =>
  state.iterationLimits[sessionId] ?? [];
