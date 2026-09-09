import { create } from 'zustand';

/**
 * 全局图片放大预览状态（会话/群聊/工作流等所有消息图片统一走此预览）。
 * 由 ImagePreview 组件（Portal 到 body）消费；Markdown 图片与附件图片点击均调 open。
 */
interface ImagePreviewState {
  /** 当前预览的图片 src（null 表示关闭）。 */
  src: string | null;
  open: (src: string) => void;
  close: () => void;
}

export const useImagePreviewStore = create<ImagePreviewState>((set) => ({
  src: null,
  open: (src) => set({ src }),
  close: () => set({ src: null }),
}));
