/**
 * enclaveOrgStore — 飞地「组织与权限」状态（真实数据，来自 enclave.db 的只读快照）。
 *
 * 组织与成员数据来自平台，主机侧只读；刷新走 `enclave_snapshot_refresh`（触发后端全量拉取）。
 * 治理按钮一律由后端 `enclave_org_caps.actions` 驱动（授权器的唯一来源），前端不自行判断角色。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage } from '../utils/errorMessage';
import type {
  AuditView,
  EmergencyView,
  EnclaveOrgCaps,
  OrgDetailView,
  OrgSnapshotView,
  PolicyView,
  SnapshotOutcome,
} from '../types/enclave';

interface EnclaveOrgState {
  orgs: OrgSnapshotView[];
  activeOrgId: string;
  detail: OrgDetailView | null;
  /** 组织级能力（身份 / 角色 / 可执行动作） */
  caps: EnclaveOrgCaps | null;
  /** 数据出境总闸 */
  policy: PolicyView | null;
  /** 主机应急开关 */
  emergency: EmergencyView | null;
  /** 策略 / 应急变更审计流水 */
  audit: AuditView[];
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
  /** 拉组织能力 / 策略 / 应急 / 审计（切换组织时调用） */
  loadGovernance: (id: string) => Promise<void>;
  /** 设置数据出境总闸；成功返回 true */
  setPolicy: (id: string, allow: boolean) => Promise<boolean>;
  /** 设置主机应急开关；成功返回 true */
  setEmergency: (id: string, disabled: boolean) => Promise<boolean>;
}

export const useEnclaveOrgStore = create<EnclaveOrgState>((set, get) => ({
  orgs: [],
  activeOrgId: '',
  detail: null,
  caps: null,
  policy: null,
  emergency: null,
  audit: [],
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
    // 组织级能力 / 策略 / 应急 / 审计：与详情并行无关，选中后统一拉一次
    await get().loadGovernance(id);
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

  /** 拉组织能力 / 策略 / 应急 / 审计。单项失败不拖垮其它项。 */
  loadGovernance: async (id) => {
    const [caps, policy, emergency, audit] = await Promise.all([
      invoke<EnclaveOrgCaps>('enclave_org_caps', { orgId: id }).catch(() => null),
      invoke<PolicyView>('enclave_policy_get', { orgId: id }).catch(() => null),
      invoke<EmergencyView>('enclave_emergency_get', { orgId: id }).catch(() => null),
      invoke<AuditView[]>('enclave_audit_list', { orgId: id, limit: 50 }).catch(() => []),
    ]);
    set({ caps, policy, emergency, audit });
  },

  setPolicy: async (id, allow) => {
    try {
      const policy = await invoke<PolicyView>('enclave_policy_set', { orgId: id, allow });
      set({ policy, error: null });
      await get().loadGovernance(id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  setEmergency: async (id, disabled) => {
    try {
      const emergency = await invoke<EmergencyView>('enclave_emergency_set', { orgId: id, disabled });
      set({ emergency, error: null });
      await get().loadGovernance(id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },
}));
