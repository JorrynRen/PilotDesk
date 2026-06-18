import { create } from 'zustand';

interface GeneratingState {
  /** 正在生成的会话 ID → true 的映射 */
  generatingMap: Record<string, boolean>;
  syncGeneratingSessions: (sessionIds: Set<string>) => void;
}

function areEqual(a: Record<string, boolean>, b: Record<string, boolean>): boolean {
  const aKeys = Object.keys(a);
  const bKeys = Object.keys(b);
  if (aKeys.length !== bKeys.length) return false;
  return aKeys.every((k) => k in b);
}

export const useGeneratingStore = create<GeneratingState>((set, get) => ({
  generatingMap: {},
  syncGeneratingSessions: (sessionIds) => {
    const next: Record<string, boolean> = {};
    sessionIds.forEach((id) => { next[id] = true; });
    const prev = get().generatingMap;
    if (!areEqual(prev, next)) {
      set({ generatingMap: next });
    }
  },
}));

/** 选择器：指定会话是否正在生成中（返回 boolean，Zustand 可正确比较） */
export const selectIsGenerating = (sessionId: string) => (state: GeneratingState) =>
  !!state.generatingMap[sessionId];
