/**
 * commandCenterStore — 全局指挥中心（顶栏「指挥中心」按钮触发）
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

/**
 * 「入口指引」是否已展示过（一次性）。
 *
 * 背景：空状态下面板是**自动打开**的，新用户关掉之后就再也找不到入口在哪了。
 * 所以第一次「显式关闭」时，顶栏图标会做一次脉冲 + 气泡指引（见 TitleBar），
 * 只展示一次（持久化），之后不再打扰。
 */
const ENTRY_HINT_STORAGE_KEY = 'pilotdesk.command-center.entry-hint-shown';

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
  /** 「入口在这里」指引是否正在展示（顶栏图标脉冲 + 气泡） */
  entryHint: boolean;
  /** 近 7 天三维归因（成本速览） */
  usage: UsageAttribution | null;
  loadingUsage: boolean;
  /** 打开面板并刷新全部数据源（顺带收起入口指引） */
  openCenter: () => Promise<void>;
  /**
   * 刷新全部数据源但**不改 `open` 状态**。
   *
   * 会话默认页内嵌的那个指挥中心实例是常驻的，不能调 `openCenter()`（会把模态盖上来），
   * 「刷新」按钮与内嵌实例的挂载刷新都走这里。
   */
  refreshCenter: () => Promise<void>;
  /** 纯关闭（跳转类操作用：用户已经找到下一步，不需要入口指引） */
  setOpen: (open: boolean) => void;
  /** 显式关闭（X / Esc / 遮罩 / 顶栏图标）：首次关闭时给出「入口在这里」动画指引 */
  closeCenter: () => void;
  /** 收起入口指引（气泡上的「知道了」/ 自动超时） */
  dismissEntryHint: () => void;
  markGuideSeen: () => void;
}

export const useCommandCenterStore = create<CommandCenterState>((set) => {
  /**
   * 刷新全部数据源（不改 `open`）。模态打开与内嵌实例共用同一份实现，
   * 避免两处各维护一遍「刷新哪些列表」。
   */
  const refresh = async () => {
    set({ loadingUsage: true });
    const wf = useWorkflowStore.getState();
    const gc = useGroupChatStore.getState();
    // 三处列表都静默刷新：面板自己展示 loading，不需要各 store 的全局 loading 态
    void wf.loadInstances(undefined, true);
    void wf.loadPendingInputs();
    void wf.loadPendingApprovals();
    // 使用向导的就绪判定要看「提供商是否已配 Key / 模型」，故顺带刷新
    void useApiProviderStore.getState().fetchProviders();
    void gc.loadRooms().then(() => {
      // 运行中的房间额外探测 Actor 存活：区分"真在跑"与"DB 残留 running（待恢复）"
      useGroupChatStore
        .getState()
        .rooms.filter((r) => r.status === 'running')
        .forEach((r) => void useGroupChatStore.getState().refreshActorAlive(r.id));
    });
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
    entryHint: false,
    usage: null,
    loadingUsage: false,

    openCenter: async () => {
      // 用户自己找到了入口：指引立刻收起，别再指着图标
      set({ open: true, entryHint: false });
      await refresh();
    },

    refreshCenter: refresh,

    setOpen: (open) => set({ open, entryHint: false }),

    closeCenter: () => {
      /**
       * 显式关闭 = 用户已经知道这个面板存在，立刻标记「使用指引已看过」。
       *
       * 不标记的话，会话默认页（内嵌指挥中心）里那块「开始使用」引导会一直以展开态常驻，
       * 用户每回默认页都被它顶掉半屏。标记后它收成一行常驻入口，想再看点开即可。
       */
      if (!loadFlag(GUIDE_STORAGE_KEY)) saveFlag(GUIDE_STORAGE_KEY);

      // 第一次显式关闭：用一次动画指引告诉用户入口在顶栏哪一格（之后不再出现）
      if (!loadFlag(ENTRY_HINT_STORAGE_KEY)) {
        saveFlag(ENTRY_HINT_STORAGE_KEY);
        set({ open: false, entryHint: true, guideSeen: true });
        return;
      }
      set({ open: false, entryHint: false, guideSeen: true });
    },

    dismissEntryHint: () => {
      // 兜底落一次盘：气泡被手动关掉时也算"见过的指引"，避免下次又冒出来
      if (!loadFlag(ENTRY_HINT_STORAGE_KEY)) saveFlag(ENTRY_HINT_STORAGE_KEY);
      set({ entryHint: false });
    },

    markGuideSeen: () => {
      saveFlag(GUIDE_STORAGE_KEY);
      set({ guideSeen: true });
    },
  };
});

/** 成本速览的统计窗口文案（与 `openCenter` 的取值保持一致）。 */
export const COMMAND_CENTER_USAGE_DAYS = USAGE_DAYS;
