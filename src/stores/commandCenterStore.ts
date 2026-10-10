/**
 * commandCenterStore — 全局指挥中心
 *
 * 两个实例共用本 store：会话默认页内嵌的常驻内容（variant="inline"），
 * 以及工作流页「待处理」按钮打开的模态面板（variant="modal"）。
 *
 * 存在的理由：`什么在跑 / 什么在等我 / 今天花了多少` 分散在各页面（工作流页的实例列表、
 * 会话内的审批卡、群聊房间状态、设置页用量），切了模式就看不见。这里把它聚成一处。
 *
 * 数据读取全部复用现有 store / 命令（无新增后端接口）：
 * - 待处理：`workflowStore.pendingApprovals`（工具审批）+ `pendingInputs`（等待人工输入）；
 * - 进行中：`workflowStore.instances` + `groupChatStore.rooms`（含 Actor 存活探测）；
 * - 成本速览：`get_usage_attribution`（近 7 天，三维合计）。
 *
 * 刷新动作放在 store 的 `refreshCenter()` 里而不是组件 effect 的同步 setState ——
 * 组件只读状态，避免「effect 内同步 setState」的级联渲染问题。
 * `openCenter()` = 置 `open: true` + `refreshCenter()`；会话默认页内嵌的实例只用后者。
 */
import { create } from 'zustand';
import { getUsageAttribution, type UsageAttribution } from '../types';
import { useWorkflowStore } from './workflowStore';
import { useGroupChatStore } from './groupChatStore';
import { useApiProviderStore } from './apiProviderStore';

/** 首次使用指引的展示标记（纯前端，与 `pilotdesk.notifications` 同一约定）。 */
const GUIDE_STORAGE_KEY = 'pilotdesk.command-center.guide-seen';

/** 成本速览的统计窗口（天）。 */
const USAGE_DAYS = 7;

function loadFlag(key: string): boolean {
  try {
    return localStorage.getItem(key) === '1';
  } catch {
    return false;
  }
}

function saveFlag(key: string): void {
  try {
    localStorage.setItem(key, '1');
  } catch {
    /* 配额/隐私模式失败可忽略：至多再展示一次 */
  }
}

interface CommandCenterState {
  open: boolean;
  /** 一次性使用指引是否展示过 */
  guideSeen: boolean;
  /** 近 7 天三维归因（成本速览） */
  usage: UsageAttribution | null;
  loadingUsage: boolean;
  /** 打开模态面板并刷新全部数据源 */
  openCenter: () => Promise<void>;
  /**
   * 刷新全部数据源但**不改 `open` 状态**。
   *
   * 会话默认页内嵌的那个指挥中心实例是常驻的，不能调 `openCenter()`（会把模态盖上来），
   * 「刷新」按钮与内嵌实例的挂载刷新都走这里。
   */
  refreshCenter: () => Promise<void>;
  /**
   * 只刷新「进行中」相关的数据源（实例 / 房间 / 房间 Actor 存活）。
   *
   * 与 `refreshCenter()` 分开的原因：内嵌指挥中心在"确实有东西在跑"时会 4s 轮询一次，
   * 若每次都把待处理、提供商、用量一起重拉，代价与副作用都太大（等于每 4s 打一次用量接口）。
   */
  pollActive: () => Promise<void>;
  /** 纯关闭（跳转类操作用：用户已经找到下一步，不需要额外提示） */
  setOpen: (open: boolean) => void;
  /** 显式关闭（X / Esc / 遮罩）：标记「使用指引已看过」，之后默认页不再展开整块引导 */
  closeCenter: () => void;
  markGuideSeen: () => void;
}

export const useCommandCenterStore = create<CommandCenterState>((set) => {
  /**
   * 进行中的三处数据源：实例 / 房间 / 对运行中房间探测 Actor 存活
   * （区分"真在跑"与"DB 残留 running"）。`refresh()` 与 4s 轮询共用这一份。
   */
  const syncActive = async () => {
    const wf = useWorkflowStore.getState();
    // 静默刷新：面板自己展示 loading，不需要各 store 的全局 loading 态
    void wf.loadInstances(undefined, true);
    const gc = useGroupChatStore.getState();
    await gc.loadRooms();
    useGroupChatStore
      .getState()
      .rooms.filter((r) => r.status === 'running')
      .forEach((r) => void useGroupChatStore.getState().refreshActorAlive(r.id));
  };

  /**
   * 刷新全部数据源（不改 `open`）。模态打开与内嵌实例共用同一份实现，
   * 避免两处各维护一遍「刷新哪些列表」。
   */
  const refresh = async () => {
    set({ loadingUsage: true });
    const wf = useWorkflowStore.getState();
    void syncActive();
    void wf.loadPendingInputs();
    void wf.loadPendingApprovals();
    // 使用向导的就绪判定要看「提供商是否已配 Key / 模型」，故顺带刷新
    void useApiProviderStore.getState().fetchProviders();
    try {
      const usage = await getUsageAttribution(USAGE_DAYS);
      set({ usage, loadingUsage: false });
    } catch {
      // 成本速览是辅助信息：取不到就展示"暂无数据"，不打断面板
      set({ loadingUsage: false });
    }
  };

  return {
    open: false,
    guideSeen: loadFlag(GUIDE_STORAGE_KEY),
    usage: null,
    loadingUsage: false,

    openCenter: async () => {
      set({ open: true });
      await refresh();
    },

    refreshCenter: refresh,

    pollActive: syncActive,

    setOpen: (open) => set({ open }),

    closeCenter: () => {
      /**
       * 显式关闭 = 用户已经知道这个面板存在，立刻标记「使用指引已看过」。
       *
       * 不标记的话，会话默认页（内嵌指挥中心）里那块「开始使用」引导会一直以展开态常驻，
       * 用户每回默认页都被它顶掉半屏。标记后它收成一行常驻入口，想再看点开即可。
       */
      if (!loadFlag(GUIDE_STORAGE_KEY)) saveFlag(GUIDE_STORAGE_KEY);
      set({ open: false, guideSeen: true });
    },

    markGuideSeen: () => {
      saveFlag(GUIDE_STORAGE_KEY);
      set({ guideSeen: true });
    },
  };
});

/** 成本速览的统计窗口文案（与 `openCenter` 的取值保持一致）。 */
export const COMMAND_CENTER_USAGE_DAYS = USAGE_DAYS;
