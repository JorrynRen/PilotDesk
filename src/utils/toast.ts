import { useNotificationStore } from '../stores/notificationStore';

let toastContainer: HTMLDivElement | null = null;

function getContainer(): HTMLDivElement {
  if (toastContainer && document.body.contains(toastContainer)) {
    return toastContainer;
  }
  toastContainer = document.createElement('div');
  toastContainer.id = 'pilotdesk-toast-container';
  document.body.appendChild(toastContainer);
  return toastContainer;
}

export type ToastType = 'error' | 'success' | 'info' | 'warning';

/**
 * 弹一条 Toast（默认同时写入通知中心历史）。
 *
 * `record: false` 用于"现场看一眼就够"的提示——运行期过程信息（阶段门控未通过、运行中禁止操作、
 * 启动前校验等）。这类事件的**权威记录**由执行终态那条通知承担，再记一条就是"同一件事说两遍"。
 * 需要这种语义时用 `showLiveToast`。
 */
export function showToast(message: string, type: ToastType = 'error', duration = 8000, opts?: { record?: boolean }) {
  // 同步进通知中心：Toast 节点 8 秒后即被移除，错过就没有了（历史与未决项见 notificationStore）
  if (opts?.record !== false) {
    useNotificationStore.getState().push({ level: type, title: message });
  }

  const container = getContainer();
  const toast = document.createElement('div');
  toast.className = `pilotdesk-toast pilotdesk-toast-${type}`;
  toast.textContent = message;
  container.appendChild(toast);

  setTimeout(() => {
    if (document.body.contains(toast)) {
      toast.remove();
    }
  }, duration);
}

/** 现场提示：只弹 Toast，不进通知中心历史（理由见 `showToast` 的 `record` 说明） */
export function showLiveToast(message: string, type: ToastType = 'error', duration = 8000) {
  showToast(message, type, duration, { record: false });
}

/**
 * 已弹过的运行期提示键（进程内去重）。
 *
 * 同一帧进度事件可能被**多份监听**收到（页面/编辑器各自的订阅、StrictMode 重挂、
 * HMR 重新求值留下的旧监听、事件重放…），不去重就会一次失败弹出好几条同样的提示。
 * 通知中心那边靠 `dedupeKey` 兜底，Toast 这一层此前没有任何去重，所以这里补一个。
 */
const liveToastSeen = new Set<string>();
const LIVE_TOAST_SEEN_MAX = 200;

/** 现场提示（按 key 去重）：同一个 key 只弹一次，用于"同一帧可能被重复投递"的运行期提示 */
export function showLiveToastOnce(key: string, message: string, type: ToastType = 'error', duration = 8000) {
  if (liveToastSeen.has(key)) return;
  if (liveToastSeen.size >= LIVE_TOAST_SEEN_MAX) liveToastSeen.clear();
  liveToastSeen.add(key);
  showLiveToast(message, type, duration);
}
