/**
 * 运行态提示：进程是否以管理员权限运行。
 *
 * 为什么需要它：Windows 的 UIPI 会拦掉「中完整性进程 → 高完整性进程」的 OLE 拖放，
 * 以管理员权限启动时，从资源管理器往窗口里拖文件**事件根本到不了应用**（没有任何报错）。
 * 这不是缺陷而是系统安全边界，只能在界面上把原因说清楚，避免用户以为功能坏了。
 */
import { invoke } from '@tauri-apps/api/core';

import { showToast } from './toast';

let probe: Promise<boolean> | null = null;
/** 进程内只提示一次（StrictMode 重挂 / 多次调用都不会重复弹） */
let notified = false;

/** 进程是否以管理员/高完整性级别运行（非 Windows 恒为 false） */
export function isProcessElevated(): Promise<boolean> {
  if (!probe) {
    probe = invoke<boolean>('is_process_elevated').catch(() => false);
  }
  return probe;
}

/** 启动时调用一次：提权运行时提示「拖拽会被系统拦截」及替代操作。 */
export async function notifyElevationOnce(): Promise<void> {
  if (notified) return;
  notified = true;
  if (!(await isProcessElevated())) return;
  showToast(
    '当前以管理员权限运行：系统会拦截从资源管理器拖入的文件。请改用粘贴或「选择文件 / 添加附件」按钮；要恢复拖拽，请以普通权限重新启动 PilotDesk。',
    'warning',
    15000,
  );
}
