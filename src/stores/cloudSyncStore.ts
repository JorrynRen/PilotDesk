/**
 * 云同步状态：跨「我的云同步」与「账号」两个设置 tab 共享。
 *
 * 懒加载约定：只有「我的云同步」tab 挂载时才调用 `refresh()` 拉取状态；
 * 「账号」tab 仅读取缓存的 `status`，**不主动发请求** —— 打开设置页不会产生
 * `cloud_sync_status` 调用。
 */
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

/** 云同步一轮结果（后端 `CloudSyncResult`） */
export interface CloudSyncResult {
  pulled: number;
  pushed: number;
  conflicts: number;
  /** 本轮冲突对象清单（本地改动已留档为本地版本，主对象已跟随远端） */
  conflictItems?: { id: string; name: string }[];
  /** 本轮「被云端更新（覆盖本地）」的对象清单（本地无改动直接跟随远端；本地原内容已备份为本地版本） */
  updatedItems?: { id: string; name: string }[];
  /** 打包失败（未能上传）的对象数 */
  failed?: number;
  /** 失败对象与原因（如体积超限） */
  failures?: string[];
  error?: string;
}

/** 云同步状态（后端 `CloudSyncStatus`） */
export interface CloudSyncStatus {
  enabled: boolean;
  capabilityOk: boolean;
  lastAt: string | null;
  lastResult: string | null;
  objectCount: number;
  /** 本地根工作流数（未被任何工作流作为子流引用） */
  rootCount: number;
  /** 本地子工作流数（随主工作流一并同步，不单独同步） */
  subflowCount: number;
  cursor: number;
}

interface CloudSyncStoreState {
  status: CloudSyncStatus | null;
  /** 是否已完成一次拉取（区分「尚未同步」与「尚未拉取」） */
  loaded: boolean;
  /** 拉取最新状态；失败静默并返回 null（保持既有值，不让设置页整体失败） */
  refresh: () => Promise<CloudSyncStatus | null>;
}

export const useCloudSyncStore = create<CloudSyncStoreState>((set) => ({
  status: null,
  loaded: false,
  refresh: async () => {
    try {
      const status = await invoke<CloudSyncStatus>('cloud_sync_status');
      set({ status, loaded: true });
      return status;
    } catch {
      /* 忽略：读不到就保持默认关闭，不让设置页整体失败 */
      return null;
    }
  },
}));
