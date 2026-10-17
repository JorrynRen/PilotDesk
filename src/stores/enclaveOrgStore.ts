/**
 * enclaveOrgStore — 飞地「组织与权限」状态（真实数据，来自 enclave.db 的只读快照）。
 *
 * 只读展示：组织与成员数据来自平台，主机侧不写。刷新走 `enclave_snapshot_refresh`
 * （触发后端全量拉取），成功后重拉组织列表并保持选中项。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage } from '../utils/errorMessage';
import type { OrgDetailView, OrgSnapshotView, SnapshotOutcome } from '../types/enclave';

interface EnclaveOrgState {
  orgs: OrgSnapshotView[];
  activeOrgId: string;
  detail: OrgDetailView | null;
  loading: boolean;
  error: string | null;
  /** 最近一次治理快照成功同步时间（unix 秒，0 = 从未同步） */
  lastSnapshotAt: number;
  /** 最近一次刷新的结果（含成员列表拉取失败原因，供页面提示） */
  lastRefresh: SnapshotOutcome | null;
  loadOrgs: () => Promise<void>;
  selectOrg: (id: string) => Promise<void>;
  refresh: () => Promise<void>;
  loadFreshness: () => Promise<void>;
}

export const useEnclaveOrgStore = create<EnclaveOrgState>((set, get) => ({
  orgs: [],
  activeOrgId: '',
  detail: null,
  loading: false,
  error: null,
  lastSnapshotAt: 0,
  lastRefresh: null,

  /** 拉组织列表；默认选中首个（或保持原有选中项）。 */
  loadOrgs: async () => {
    set({ loading: true, error: null });
    try {
      const orgs = await invoke<OrgSnapshotView[]>('enclave_org_list');
      set({ orgs });
      const next = orgs.some((o) => o.orgId === get().activeOrgId)
        ? get().activeOrgId
        : orgs[0]?.orgId ?? '';
      if (next) {
        await get().selectOrg(next);
      } else {
        set({ activeOrgId: '', detail: null, loading: false });
      }
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  selectOrg: async (id) => {
    set({ activeOrgId: id, loading: true, error: null });
    try {
      const detail = await invoke<OrgDetailView | null>('enclave_org_detail', { orgId: id });
      set({ detail });
    } catch (e) {
      set({ error: errorMessage(e), detail: null });
    } finally {
      set({ loading: false });
    }
  },

  /** 触发一轮全量刷新（后端联网拉平台），成功后重拉列表。 */
  refresh: async () => {
    set({ loading: true, error: null });
    try {
      const outcome = await invoke<SnapshotOutcome>('enclave_snapshot_refresh', { orgId: null });
      set({ lastRefresh: outcome });
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
      return;
    }
    await get().loadFreshness();
    await get().loadOrgs();
  },

  loadFreshness: async () => {
    try {
      const t = await invoke<number>('enclave_snapshot_freshness');
      set({ lastSnapshotAt: t });
    } catch {
      // 新鲜度读取失败不影响主体展示，静默
    }
  },
}));
