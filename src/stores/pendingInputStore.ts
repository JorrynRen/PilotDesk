import { create } from 'zustand';

interface PendingInputState {
  value: string | null;
  set: (v: string | null) => void;
}

export const usePendingInputStore = create<PendingInputState>((set) => ({
  value: null,
  set: (v) => set({ value: v }),
}));
