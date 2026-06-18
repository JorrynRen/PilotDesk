import { create } from 'zustand';

interface GeneratingState {
  /** 正在生成的会话 ID → true 的映射 */
  generatingMap: Record<string, boolean>;
  syncGeneratingSessions: (sessionIds: Set<string>) => void;
}

export const useGeneratingStore = create<GeneratingState>((set) => ({
  generatingMap: {},
  syncGeneratingSessions: (sessionIds) => {
    const next: Record<string, boolean> = {};
    sessionIds.forEach((id) => { next[id] = true; });
    set({ generatingMap: next });
  },
}));

/** 选择器：指定会话是否正在生成中（返回 boolean，Zustand 可正确比较） */
export const selectIsGenerating = (sessionId: string) => (state: GeneratingState) =>
  !!state.generatingMap[sessionId];
