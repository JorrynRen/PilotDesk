import { create } from 'zustand';

export type FeedbackCategory = 'bug' | 'suggestion' | 'other';

/**
 * 打开反馈弹窗时携带的预填信息。
 *
 * 入口不同、预填不同，但弹窗只有一个：
 * - 通知中心：带通知标题/明细与时间点（日志聚焦到那一刻附近）；
 * - 设置 › 关于：无预填，等同空白表单。
 */
export interface FeedbackPreset {
  category?: FeedbackCategory;
  /** 预填标题（通常取通知标题） */
  title?: string;
  /** 预填描述（通常取通知明细） */
  detail?: string;
  /** 聚焦时间点（毫秒）：日志只展示该时刻前后窗口 */
  focusTs?: number;
}

interface FeedbackState {
  open: boolean;
  preset: FeedbackPreset | null;
  openFeedback: (preset?: FeedbackPreset) => void;
  close: () => void;
}

export const useFeedbackStore = create<FeedbackState>((set) => ({
  open: false,
  preset: null,
  openFeedback: (preset) => set({ open: true, preset: preset ?? null }),
  close: () => set({ open: false }),
}));
