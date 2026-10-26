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
  BackupInfo,
  BackupStatusView,
  CacheInfoView,
  EmergencyView,
  EnclaveOrgCaps,
  HandoverOutcome,
  HostStatusView,
  LanOfferOutcome,
  NotificationView,
  OrgDetailView,
  OrgSnapshotView,
  PolicyView,
  SnapshotOutcome,
  TransferExportOutcome,
  TransferImportOutcome,
  TransferView,
} from '../types/enclave';

/** 变更审计每页条数（与成员列表一致，避免长表） */
export const AUDIT_PAGE_SIZE = 10;

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
  /** 本机文件缓存占用与上限（成员端打开/下载团队文件时产生） */
  cache: CacheInfoView | null;
  /** 本机侧主机转移记录 */
  transfers: TransferView[];
  /** 成员端：主机发给当前账号、待确认的转移请求 */
  lanPending: TransferView[];
  /** 拉取待确认请求失败原因（如未发现主机），仅卡片内提示 */
  lanPendingError: string | null;
  /** 策略 / 应急变更审计流水 */
  audit: AuditView[];
  /** 审计分页：当前偏移与是否还有下一页 */
  auditOffset: number;
  auditHasMore: boolean;
  /** 站内通知（当前组织；成员端经主机转发读取） */
  notifications: NotificationView[];
  /** 主机自动备份状态（本机设置） */
  backup: BackupStatusView | null;
  loading: boolean;
  error: string | null;
  /** 本轮是否已用当前账号刷新过本机身份（false 时不渲染本地团队数据） */
  identityVerified: boolean;
  /**
   * 平台不可达、已按本机身份离线继续。
   *
   * 离线**不等于**未登录：本机团队数据仍可读写，主机对外服务也不受影响，
   * 只是组织 / 成员 / 角色快照停在最后一次同步的状态（治理类操作应提示可能过期）。
   */
  identityOffline: boolean;
  /** 最近一次治理快照成功同步时间（unix 秒，0 = 从未同步） */
  lastSnapshotAt: number;
  /** 最近一次刷新的结果（含成员列表拉取失败原因，供页面提示） */
  lastRefresh: SnapshotOutcome | null;
  loadOrgs: () => Promise<void>;
  /** 用当前登录身份刷新本机身份；平台不可达时按已有身份离线继续（返回 true） */
  ensureIdentity: () => Promise<boolean>;
  /** 账号切换 / 退出：清空全部本地团队缓存 */
  reset: () => void;
  selectOrg: (id: string) => Promise<void>;
  refresh: () => Promise<void>;
  loadFreshness: () => Promise<void>;
  /** 拉组织能力 / 策略 / 应急 / 审计（切换组织时调用） */
  loadGovernance: (id: string) => Promise<void>;
  /** 审计翻页（offset 为起始条数） */
  loadAudit: (id: string, offset: number) => Promise<void>;
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
  /** 将本机登记为该组织主机（仅 owner；首次登记/同设备续登） */
  claimHost: (id: string) => Promise<boolean>;
  /** 从旧主机在线接管（迁移数据 → 校验 → 导入 → 登记本机） */
  takeoverHost: (id: string) => Promise<HandoverOutcome | null>;
  /** 强制接管（旧设备不可用）：不迁移数据，直接登记本机 */
  forceTakeoverHost: (id: string) => Promise<boolean>;
  /** 重拉本机文件缓存占用 */
  loadCache: () => Promise<void>;
  /** 调整本机文件缓存上限（MB）；成功返回 true */
  setCacheLimit: (maxMb: number) => Promise<boolean>;
  /** 清空本机文件缓存；成功返回 true */
  clearCache: () => Promise<boolean>;
  /** 拉通知列表（当前组织） */
  loadNotifications: (id: string) => Promise<void>;
  /** 标记通知已读：不传 ids = 全部未读；成功返回 true */
  markNotificationsRead: (id: string, ids?: string[]) => Promise<boolean>;
  /** 删除通知：不传 ids = 清空本账号全部已读；成功返回 true */
  deleteNotifications: (id: string, ids?: string[]) => Promise<boolean>;
  /** 读取本机自动备份状态 */
  loadBackup: () => Promise<void>;
  /** 保存自动备份设置；成功返回 true */
  saveBackupConfig: (enabled: boolean, intervalHours: number, keep: number) => Promise<boolean>;
  /** 立即备份一次；成功返回备份信息 */
  runBackupNow: () => Promise<BackupInfo | null>;
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
  /** 拒绝发给当前账号的局域网转移请求 */
  rejectLanTransfer: (orgId: string, transferId: string) => Promise<boolean>;
}

export const useEnclaveOrgStore = create<EnclaveOrgState>((set, get) => ({
  orgs: [],
  activeOrgId: '',
  detail: null,
  caps: null,
  policy: null,
  emergency: null,
  host: null,
  cache: null,
  transfers: [],
  lanPending: [],
  lanPendingError: null,
  audit: [],
  auditOffset: 0,
  auditHasMore: false,
  notifications: [],
  backup: null,
  loading: false,
  error: null,
  identityVerified: false,
  identityOffline: false,
  lastSnapshotAt: 0,
  lastRefresh: null,

  /**
   * 用当前登录身份刷新本机身份。
   *
   * 平台不可达时后端会沿用本机已有身份返回 `online: false`：仍然算「已校验」，
   * 只是标记为离线（治理快照可能过期）。只有未登录 / 登录失效才返回 false。
   */
  ensureIdentity: async () => {
    try {
      const r = await invoke<{ online: boolean; userId: string }>('enclave_identity_refresh');
      set({ identityVerified: true, identityOffline: !r.online });
      return true;
    } catch (e) {
      set({ identityVerified: false, identityOffline: false, error: errorMessage(e) });
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
      cache: null,
      transfers: [],
      lanPending: [],
      lanPendingError: null,
      audit: [],
      auditOffset: 0,
      auditHasMore: false,
      notifications: [],
      backup: null,
      loading: false,
      error: null,
      identityVerified: false,
      identityOffline: false,
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
    const [caps, policy, emergency, audit, host, transfers, cache] = await Promise.all([
      invoke<EnclaveOrgCaps>('enclave_org_caps', { orgId: id }).catch(() => null),
      invoke<PolicyView>('enclave_policy_get', { orgId: id }).catch(() => null),
      invoke<EmergencyView>('enclave_emergency_get', { orgId: id }).catch(() => null),
      invoke<AuditView[]>('enclave_audit_list', {
        orgId: id,
        limit: AUDIT_PAGE_SIZE,
        offset: 0,
      }).catch(() => []),
      invoke<HostStatusView>('enclave_host_status', { orgId: id }).catch(() => null),
      invoke<TransferView[]>('enclave_transfer_list', { orgId: id }).catch(() => []),
      invoke<CacheInfoView>('enclave_cache_info').catch(() => null),
    ]);
    set({
      caps,
      policy,
      emergency,
      audit,
      auditOffset: 0,
      auditHasMore: audit.length >= AUDIT_PAGE_SIZE,
      host,
      transfers,
      cache,
    });
  },

  loadAudit: async (id, offset) => {
    const nextOffset = Math.max(0, offset);
    try {
      const audit = await invoke<AuditView[]>('enclave_audit_list', {
        orgId: id,
        limit: AUDIT_PAGE_SIZE,
        offset: nextOffset,
      });
      set({
        audit,
        auditOffset: nextOffset,
        auditHasMore: audit.length >= AUDIT_PAGE_SIZE,
        error: null,
      });
    } catch (e) {
      set({ error: errorMessage(e) });
    }
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

  claimHost: async (id) => {
    try {
      const host = await invoke<HostStatusView>('enclave_host_claim', { orgId: id });
      set({ host, error: null });
      await get().loadGovernance(id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  takeoverHost: async (id) => {
    try {
      const outcome = await invoke<HandoverOutcome>('enclave_host_takeover', { orgId: id });
      await get().loadGovernance(id);
      return outcome;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  forceTakeoverHost: async (id) => {
    try {
      await invoke('enclave_host_takeover_force', { orgId: id, confirmation: id });
      await get().loadGovernance(id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  loadCache: async () => {
    try {
      const cache = await invoke<CacheInfoView>('enclave_cache_info');
      set({ cache });
    } catch {
      // 缓存统计失败不阻塞治理面板
    }
  },

  setCacheLimit: async (maxMb) => {
    try {
      const cache = await invoke<CacheInfoView>('enclave_cache_set_limit', { maxMb });
      set({ cache, error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  clearCache: async () => {
    try {
      const cache = await invoke<CacheInfoView>('enclave_cache_clear');
      set({ cache, error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  loadNotifications: async (id) => {
    try {
      const notifications = await invoke<NotificationView[]>('enclave_notification_list', {
        orgId: id,
        limit: 50,
        unreadOnly: false,
      });
      set({ notifications });
    } catch {
      // 通知是辅助信息：拉取失败不阻塞任务工作台
    }
  },

  markNotificationsRead: async (id, ids) => {
    try {
      await invoke<number>('enclave_notification_mark_read', { orgId: id, ids });
      await get().loadNotifications(id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  deleteNotifications: async (id, ids) => {
    try {
      await invoke<number>('enclave_notification_delete', { orgId: id, ids });
      await get().loadNotifications(id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  loadBackup: async () => {
    try {
      const backup = await invoke<BackupStatusView>('enclave_backup_status');
      set({ backup });
    } catch {
      // 非主机 / 未初始化时静默（备份卡只在主机侧展示）
    }
  },

  saveBackupConfig: async (enabled, intervalHours, keep) => {
    try {
      const backup = await invoke<BackupStatusView>('enclave_backup_config_set', {
        enabled,
        intervalHours,
        keep,
      });
      set({ backup, error: null });
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  runBackupNow: async () => {
    try {
      const info = await invoke<BackupInfo>('enclave_backup_now');
      await get().loadBackup();
      return info;
    } catch (e) {
      set({ error: errorMessage(e) });
      await get().loadBackup();
      return null;
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

  rejectLanTransfer: async (orgId, transferId) => {
    try {
      await invoke('enclave_transfer_lan_reject', { orgId, transferId });
      await Promise.all([get().loadTransfers(orgId), get().loadLanPending(orgId)]);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
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
