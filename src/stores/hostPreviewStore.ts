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

interface HostPreviewStore {
  view: SessionListView;
  /** 宿主视图当前选中的 ACP Agent */
  agentType: string | null;
  /** 非空 = 右侧正在只读预览该宿主会话 */
  preview: HostPreview | null;
  setView: (view: SessionListView) => void;
  setAgentType: (agentType: string | null) => void;
  openPreview: (preview: HostPreview) => void;
  closePreview: () => void;
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
  // 切回本地视图时顺带关掉预览：否则右侧还停在宿主消息上，左侧却已换成自己的会话
  setView: (view) =>
    set((state) => ({ view, preview: view === 'local' ? null : state.preview })),
  setAgentType: (agentType) => set({ agentType }),
  openPreview: (preview) => set({ preview }),
  closePreview: () => set({ preview: null }),
}));
