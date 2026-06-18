import { create } from 'zustand';

interface GeneratingState {
  generatingSessionIds: Set<string>;
  setGenerating: (sessionId: string, isGenerating: boolean) => void;
}

export const useGeneratingStore = create<GeneratingState>((set) => ({
  generatingSessionIds: new Set<string>(),
  setGenerating: (sessionId, isGenerating) =>
    set((state) => {
      const next = new Set(state.generatingSessionIds);
      if (isGenerating) {
        next.add(sessionId);
      } else {
        next.delete(sessionId);
      }
      return { generatingSessionIds: next };
    }),
}));
