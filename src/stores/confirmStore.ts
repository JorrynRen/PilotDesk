import { create } from 'zustand';

/** 确认弹窗选项。 */
export interface ConfirmOptions {
  /** 标题（默认「确认操作」）。 */
  title?: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  /** 危险操作：确认按钮用告警色（默认 true——目前调用方都是删除类操作）。 */
  danger?: boolean;
}

interface ConfirmState {
  options: ConfirmOptions | null;
  resolve: ((ok: boolean) => void) | null;
  request: (options: ConfirmOptions) => Promise<boolean>;
  settle: (ok: boolean) => void;
}

/**
 * 全局确认弹窗状态。
 *
 * 存在的理由：Tauri WebView 下 `window.confirm` 常被禁用/不触发（用户点击无反应），
 * 故应用内所有二次确认统一走这里，由 App 顶层挂载的 ConfirmDialog 渲染。
 */
export const useConfirmStore = create<ConfirmState>((set, get) => ({
  options: null,
  resolve: null,
  request: (options) => {
    // 已有弹窗未决时先按「取消」结算，避免上一个调用方的 await 悬挂
    get().resolve?.(false);
    return new Promise<boolean>((resolve) => set({ options, resolve }));
  },
  settle: (ok) => {
    const { resolve } = get();
    set({ options: null, resolve: null });
    resolve?.(ok);
  },
}));

/** 命令式确认入口：`if (!(await confirmDialog({ message: '...' }))) return;` */
export function confirmDialog(options: ConfirmOptions): Promise<boolean> {
  return useConfirmStore.getState().request(options);
}
