import { create } from 'zustand';

/**
 * 会话列表的两种视图（左侧栏顶部下拉切换）。
 *
 * - `local`：本应用的会话（入库数据）
 * - `host`：ACP 宿主 Agent 侧已有的会话（只读预览 / 删除 / 导入继续）
 */
export type SessionListView = 'local' | 'host';

/** 预览中的一条宿主消息（`session/load` 只回 user / assistant 正文）。 */
export interface HostPreviewMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
}

/** 右侧只读预览的宿主会话。 */
export interface HostPreview {
  agentType: string;
  hostSessionId: string;
  title: string;
  cwd: string | null;
  messages: HostPreviewMessage[];
}

/** 回放进度：`revealed` = 已露出的条数。为空 = 当前不在回放。 */
export interface ReplayProgress {
  revealed: number;
  total: number;
}

interface HostPreviewStore {
  view: SessionListView;
  /** 宿主视图当前选中的 ACP Agent */
  agentType: string | null;
  /** 非空 = 右侧正在只读预览该宿主会话 */
  preview: HostPreview | null;
  /**
   * 逐条回放进度。放在 store 而不是预览组件内部：左侧列表也要据此显示"回放中"，
   * 两处若各存一份就会出现"列表转圈但预览已播完"这类不一致。
   */
  replay: ReplayProgress | null;
  setView: (view: SessionListView) => void;
  setAgentType: (agentType: string | null) => void;
  openPreview: (preview: HostPreview) => void;
  closePreview: () => void;
  /** 推进一步（回放结束自动置空） */
  advanceReplay: () => void;
}

/**
 * 宿主会话的视图/预览状态。
 *
 * 放在独立 store 而不是塞进 `sessionStore`：预览不是库里的会话（不落库、不参与
 * 发送/归档/搜索），混进会话 store 会到处需要判断"这是不是虚拟会话"。
 */
export const useHostPreviewStore = create<HostPreviewStore>((set) => ({
  view: 'local',
  agentType: null,
  preview: null,
  replay: null,
  // 切回本地视图时顺带关掉预览：否则右侧还停在宿主消息上，左侧却已换成自己的会话
  setView: (view) =>
    set((state) => ({
      view,
      preview: view === 'local' ? null : state.preview,
      replay: view === 'local' ? null : state.replay,
    })),
  setAgentType: (agentType) => set({ agentType }),
  // 首条立即露出（避免"点开一片空白"），其余逐条推进
  openPreview: (preview) =>
    set({
      preview,
      replay:
        preview.messages.length > 0
          ? { revealed: 1, total: preview.messages.length }
          : null,
    }),
  closePreview: () => set({ preview: null, replay: null }),
  advanceReplay: () =>
    set((state) => {
      if (!state.replay) return state;
      const next = state.replay.revealed + 1;
      return { replay: next >= state.replay.total ? null : { ...state.replay, revealed: next } };
    }),
}));
