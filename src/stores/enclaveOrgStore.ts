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
  HostStatusView,
  LanOfferOutcome,
  OrgDetailView,
  OrgSnapshotView,
  PolicyView,
  SnapshotOutcome,
  TransferExportOutcome,
  TransferImportOutcome,
  TransferView,
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
  /** 主机服务 / 发现状态（成员端含手动地址） */
  host: HostStatusView | null;
  /** 本机侧主机转移记录 */
  transfers: TransferView[];
  /** 成员端：主机发给当前账号、待确认的转移请求 */
  lanPending: TransferView[];
  /** 拉取待确认请求失败原因（如未发现主机），仅卡片内提示 */
  lanPendingError: string | null;
  /** 策略 / 应急变更审计流水 */
  audit: AuditView[];
  loading: boolean;
  error: string | null;
  /** 本轮是否已用当前账号刷新过本机身份（false 时不渲染本地团队数据） */
  identityVerified: boolean;
  /** 最近一次治理快照成功同步时间（unix 秒，0 = 从未同步） */
  lastSnapshotAt: number;
  /** 最近一次刷新的结果（含成员列表拉取失败原因，供页面提示） */
  lastRefresh: SnapshotOutcome | null;
  loadOrgs: () => Promise<void>;
  /** 用当前登录账号刷新本机身份；失败返回 false（默认拒绝） */
  ensureIdentity: () => Promise<boolean>;
  /** 账号切换 / 退出：清空全部本地团队缓存 */
  reset: () => void;
  selectOrg: (id: string) => Promise<void>;
  refresh: () => Promise<void>;
  loadFreshness: () => Promise<void>;
  /** 拉组织能力 / 策略 / 应急 / 审计（切换组织时调用） */
  loadGovernance: (id: string) => Promise<void>;
  /** 设置数据出境总闸；成功返回 true */
  setPolicy: (id: string, allow: boolean) => Promise<boolean>;
  /** 设置主机应急开关；成功返回 true */
  setEmergency: (id: string, disabled: boolean) => Promise<boolean>;
  /** 重拉主机服务 / 发现状态 */
  loadHost: (id: string) => Promise<void>;
  /** 成员端设置手动主机地址（留空 = 清除，回到自动发现）；成功返回 true */
  setHostAddr: (id: string, addr: string) => Promise<boolean>;
  /** 清除成员端固定的主机证书指纹（主机重装/更换后使用） */
  resetHostFp: (id: string) => Promise<boolean>;
  /** 重拉转移记录 */
  loadTransfers: (id: string) => Promise<void>;
  /** 导出线下加密迁移包（仅 owner） */
  exportTransfer: (id: string, passphrase: string, outPath?: string) => Promise<TransferExportOutcome | null>;
  /** 导入加密迁移包（接收方为组织成员） */
  importTransfer: (path: string, passphrase: string) => Promise<TransferImportOutcome | null>;
  /** 推进转移状态 */
  advanceTransfer: (transferId: string, phase: string) => Promise<boolean>;
  /** 销毁旧主机数据（仅 completed 的转出记录） */
  destroyTransfer: (transferId: string, orgId: string) => Promise<boolean>;
  /** 发起局域网自动迁移（owner 选受让人；生成迁移包与一次性口令） */
  offerLanTransfer: (orgId: string, targetUserId: string) => Promise<LanOfferOutcome | null>;
  /** 拉取发给当前账号的转移请求（成员端，经主机 WS） */
  loadLanPending: (orgId: string) => Promise<void>;
  /** 接受并自动完成局域网迁移（下载 → 校验 → 合并 → 主机确认） */
  acceptLanTransfer: (orgId: string, transferId: string) => Promise<TransferImportOutcome | null>;
}

export const useEnclaveOrgStore = create<EnclaveOrgState>((set, get) => ({
  orgs: [],
  activeOrgId: '',
  detail: null,
  caps: null,
  policy: null,
  emergency: null,
  host: null,
  transfers: [],
  lanPending: [],
  lanPendingError: null,
  audit: [],
  loading: false,
  error: null,
  identityVerified: false,
  lastSnapshotAt: 0,
  lastRefresh: null,

  /** 用当前账号刷新本机登录身份（`/api/v1/me`）。失败视为未校验，隐藏一切本地团队数据。 */
  ensureIdentity: async () => {
    try {
      await invoke('enclave_identity_refresh');
      set({ identityVerified: true });
      return true;
    } catch (e) {
      set({ identityVerified: false, error: errorMessage(e) });
      return false;
    }
  },

  /** 账号切换 / 退出：清空全部本地团队缓存，避免沿用上一个账号的数据。 */
  reset: () => {
    set({
      orgs: [],
      activeOrgId: '',
      detail: null,
      caps: null,
      policy: null,
      emergency: null,
      host: null,
      transfers: [],
      lanPending: [],
      lanPendingError: null,
      audit: [],
      loading: false,
      error: null,
      identityVerified: false,
      lastSnapshotAt: 0,
      lastRefresh: null,
    });
  },

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

  /** 拉组织能力 / 策略 / 应急 / 审计 / 主机状态。单项失败不拖垮其它项。 */
  loadGovernance: async (id) => {
    const [caps, policy, emergency, audit, host, transfers] = await Promise.all([
      invoke<EnclaveOrgCaps>('enclave_org_caps', { orgId: id }).catch(() => null),
      invoke<PolicyView>('enclave_policy_get', { orgId: id }).catch(() => null),
      invoke<EmergencyView>('enclave_emergency_get', { orgId: id }).catch(() => null),
      invoke<AuditView[]>('enclave_audit_list', { orgId: id, limit: 50 }).catch(() => []),
      invoke<HostStatusView>('enclave_host_status', { orgId: id }).catch(() => null),
      invoke<TransferView[]>('enclave_transfer_list', { orgId: id }).catch(() => []),
    ]);
    set({ caps, policy, emergency, audit, host, transfers });
  },

  loadHost: async (id) => {
    try {
      const host = await invoke<HostStatusView>('enclave_host_status', { orgId: id });
      set({ host });
    } catch {
      // 状态读取失败不阻塞治理面板
    }
  },

  setHostAddr: async (id, addr) => {
    try {
      await invoke('enclave_host_addr_set', { orgId: id, addr });
      await get().loadHost(id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  resetHostFp: async (id) => {
    try {
      await invoke('enclave_host_fp_reset', { orgId: id });
      await get().loadHost(id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  loadTransfers: async (id) => {
    try {
      const transfers = await invoke<TransferView[]>('enclave_transfer_list', { orgId: id });
      set({ transfers });
    } catch {
      // 记录读取失败不阻塞治理面板
    }
  },

  exportTransfer: async (id, passphrase, outPath) => {
    try {
      const outcome = await invoke<TransferExportOutcome>('enclave_transfer_export', {
        orgId: id,
        passphrase,
        outPath: outPath?.trim() ? outPath.trim() : null,
      });
      await get().loadTransfers(id);
      return outcome;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  importTransfer: async (path, passphrase) => {
    try {
      const outcome = await invoke<TransferImportOutcome>('enclave_transfer_import', {
        path: path.trim(),
        passphrase,
      });
      // 包内组织可能不是当前选中组织：按返回值刷新对应列表，并补一次组织列表
      await get().loadTransfers(outcome.orgId);
      return outcome;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  advanceTransfer: async (transferId, phase) => {
    try {
      await invoke<TransferView>('enclave_transfer_advance', { transferId, toPhase: phase });
      await get().loadTransfers(get().activeOrgId);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  destroyTransfer: async (transferId, orgId) => {
    try {
      await invoke('enclave_transfer_destroy', {
        transferId,
        orgId,
        confirmation: orgId,
      });
      await get().loadTransfers(orgId);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  offerLanTransfer: async (orgId, targetUserId) => {
    try {
      const outcome = await invoke<LanOfferOutcome>('enclave_transfer_lan_offer', {
        orgId,
        targetUserId,
      });
      await get().loadTransfers(orgId);
      return outcome;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  loadLanPending: async (orgId) => {
    try {
      const lanPending = await invoke<TransferView[]>('enclave_transfer_lan_pending', { orgId });
      set({ lanPending, lanPendingError: null });
    } catch (e) {
      set({ lanPending: [], lanPendingError: errorMessage(e) });
    }
  },

  acceptLanTransfer: async (orgId, transferId) => {
    try {
      const outcome = await invoke<TransferImportOutcome>('enclave_transfer_lan_accept', {
        orgId,
        transferId,
      });
      await Promise.all([get().loadTransfers(orgId), get().loadLanPending(orgId)]);
      return outcome;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
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
