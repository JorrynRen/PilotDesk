/**
 * 消息多选的状态机与剪贴板工具（**纯逻辑，无组件**）。
 *
 * 之所以与 `MessageSelectionBar` 分开放：组件文件里再导出 hook / 函数会破坏 Fast Refresh
 * （`react-refresh/only-export-components`）—— 一个文件要么只导出组件，要么只导出逻辑。
 *
 * 顺序**不由本模块决定**：`selectedIds` 只记勾了谁，调用方按自己的消息顺序过滤
 * （勾选顺序 ≠ 对话顺序，交给 AI 的必须是后者）。
 */

import { useCallback, useState } from 'react';
import { showToast } from '../../utils/toast';

/** 多选状态机（会话模式与群聊共用） */
export function useMessageSelection() {
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);

  const enter = useCallback(() => setSelectMode(true), []);
  /** 退出多选并清空（换会话 / 换房间 / 沉淀成功都走它，避免把上一批勾选带过来） */
  const exit = useCallback(() => {
    setSelectMode(false);
    setSelectedIds([]);
  }, []);
  const clear = useCallback(() => setSelectedIds([]), []);
  const toggle = useCallback((id: string) => {
    setSelectedIds((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));
  }, []);

  return { selectMode, selectedIds, enter, exit, clear, toggle };
}

/** 复制文本（navigator.clipboard 不可用时退回 execCommand，与会话页/知识页同一套降级） */
export async function copyToClipboard(text: string, okMsg: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement('textarea');
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    document.body.removeChild(ta);
  }
  showToast(okMsg, 'success');
}
