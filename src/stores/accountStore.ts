/**
 * 会员账号：登录状态与会员权益（entitlements）。
 *
 * 登录走平台（会员中心）的 PKCE 授权：Rust 侧起本地回环端口并把授权页地址给回来，
 * 这里用**系统浏览器**打开 —— 密码只在平台页面输入，产品端拿不到明文；
 * 令牌由 Rust 侧加密保存，前端只拿到「账号 + 等级 + 已解锁能力」。
 *
 * 新收费策略：**本地能力不设限**（本地功能对所有登录 / 未登录状态一律开放），
 * 云端服务、团队治理与服务权益由平台登记，产品端只负责按能力键展示和消费。
 */
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { useEnclaveOrgStore } from './enclaveOrgStore';
import { useEnclaveTaskStore } from './enclaveTaskStore';
import { useEnclaveKbStore } from './enclaveKbStore';

/** 可打开的平台页面：升级会员 / 会员中心（账号设置）/ 组织页 */
export type PlatformPage = 'upgrade' | 'portal' | 'organizations';

/** 客户端可主动推荐的付费档位 */
export type UpgradePlanKey = 'pro' | 'team';

/** 打开升级页时携带的上下文，用于官网直接定位推荐档位与能力 */
export interface UpgradeContext {
  plan?: UpgradePlanKey;
  capability?: string;
  source?: string;
}

/** 打开组织订阅页时携带的上下文，确保购买动作落在正确的组织上 */
export interface OrganizationUpgradeContext {
  organizationId?: number;
  capability?: string;
  source?: string;
}

/** 云端能力键（平台侧登记，产品端只消费）：本地能力已全部开放，个人云同步按能力门控 */
export const CAP_CLOUD_SYNC = 'sync.cloud';
/** 从其他设备发起本机工作流（仅专业版 / 团队版） */
export const CAP_WORKFLOW_REMOTE_RUN = 'workflow.remote-run';
/** 团队席位与成员治理（仅团队版） */
export const CAP_TEAM_MEMBERS = 'team.members';
/** 团队共享空间（仅团队版；组织共享工作流会校验该能力） */
export const CAP_TEAM_SHARED_SPACE = 'team.shared-space';
/** 团队用量看板（仅团队版；向组织上报用量必须先具备该能力） */
export const CAP_TEAM_USAGE_BOARD = 'team.usage-board';

/**
 * 已知平台能力键的中文名。
 *
 * **与平台登记表（`app/src/services/entitlements/capabilities.ts` 的 `label`）保持一致**——
 * 平台是唯一来源，这里只做展示；新增能力键时两边都要补，否则会显示成裸 key。
 * 未知键回退显示键本身。
 */
const CAPABILITY_LABELS: Record<string, string> = {
  'sync.cloud': '个人云同步',
  'workflow.remote-run': '远程发起工作流',
  'team.members': '团队席位与成员治理',
  'team.audit': '团队操作审计',
  'team.shared-space': '团队共享空间',
  'team.usage-board': '团队用量看板',
  'sso.login': '企业单点登录',
  'support.priority': '优先在线服务',
};

export function capabilityLabel(key: string): string {
  return CAPABILITY_LABELS[key] ?? key;
}

export interface AccountInfo {
  email: string;
  nickname: string;
  /** 内置头像键（null = 未选择，前端回退首字母头像） */
  avatarId?: string | null;
  planKey: string;
  planName: string;
  /** 到期时间（ISO；free 为空） */
  expiresAt: string | null;
  /** 已解锁的受限能力键 */
  capabilities: string[];
  /** 平台下发的配额（键为平台登记项；-1 = 不限；缺省键 = 未配置） */
  quotas: Record<string, number>;
  /** 权益来源：personal / organization / free */
  source: string;
  /** 来源组织（source === 'organization' 时非空） */
  organization: { id: number; name: string; role: string } | null;
  /** 是否已过期（此时平台按 free 返回） */
  expired: boolean;
  /** 是否来自离线缓存（平台不可达、仍在 7 天宽限期内） */
  stale: boolean;
}

interface AccountStoreState {
  /** 是否已完成一次启动检查（未登录时也置 true，避免界面闪烁） */
  loaded: boolean;
  account: AccountInfo | null;
  /** 正在等浏览器里的授权 */
  loggingIn: boolean;
  error: string;
  refresh: () => Promise<void>;
  login: () => Promise<void>;
  logout: () => Promise<void>;
  /**
   * 打开平台页面（**系统浏览器**）：登录就是在浏览器里完成的，会话 Cookie 天然在浏览器里，
   * 点开即用。不要改用应用内 WebView —— 那是另一套 Cookie jar，会要求重新登录。
   */
  openPlatform: (page: PlatformPage) => Promise<void>;
  /** 打开组织订阅页，并携带当前组织、缺失能力与来源；不会转成个人团队版购买 */
  openOrganization: (context?: OrganizationUpgradeContext) => Promise<void>;
  /**
   * 打开带购买理由的升级页。
   *
   * 官网据此高亮对应档位与能力，避免用户从「个人云同步 / 组织共享」受阻后
   * 只看到一个没有上下文的通用价格表。
   */
  openUpgrade: (context?: UpgradeContext) => Promise<void>;
  /** 取消等待（只影响界面；真正的回调结果会被丢弃） */
  cancelLogin: () => void;
}

/**
 * 登录代次：取消后自增，使仍在等待的 `account_login_complete` 结果被丢弃。
 * （Rust 侧的等待无法真正中断，但结果不会再写回状态。）
 */
let loginGeneration = 0;

/**
 * 清空飞地本地缓存（组织/任务/知识库）并重建身份。
 *
 * 换账号后必须**先清空再按新身份加载**：否则页面会继续展示上一个账号
 * 同步下来的本机快照。后端同时用 `viewer_id` 绑定快照，双保险。
 */
function resetEnclaveCaches() {
  useEnclaveOrgStore.getState().reset();
  useEnclaveTaskStore.getState().reset();
  useEnclaveKbStore.getState().reset();
}

async function reloadEnclaveForAccount() {
  const org = useEnclaveOrgStore.getState();
  await org.ensureOrgs();
}

async function openPlatformPath(path: string) {
  // 基址只在 Rust 一处维护，这里按页面拼路径
  const base = await invoke<string>('account_platform_base');
  await openUrl(`${base.replace(/\/+$/, '')}${path}`);
}

export const useAccountStore = create<AccountStoreState>((set, get) => ({
  loaded: false,
  account: null,
  loggingIn: false,
  error: '',

  refresh: async () => {
    try {
      const account = await invoke<AccountInfo | null>('account_status');
      const prevEmail = get().account?.email ?? null;
      const nextEmail = account?.email ?? null;
      set({ account, loaded: true, error: '' });
      // 账号在别处切换（浏览器登录 / 退出）后，必须按新身份重建飞地缓存：
      // 否则任务列表、组织详情与权限会继续沿用上一个账号的本机快照。
      if (prevEmail !== nextEmail) {
        if (account) {
          resetEnclaveCaches();
          void reloadEnclaveForAccount();
        } else {
          // 账号由「有」变「无」有两种来源：平台侧吊销（真退出）与断网超过离线宽限期
          // （`account_status` 取不到权益缓存而回落空）。后者本机身份仍然有效，
          // 若一刀切清缓存，断网时会把本机团队数据整块清空。
          const stillValid = await useEnclaveOrgStore.getState().ensureIdentity();
          if (!stillValid) resetEnclaveCaches();
        }
        return;
      }
      // 启动时把飞地本机身份对齐到当前账号（账号在别处切换过也不会沿用旧身份）
      if (account && !useEnclaveOrgStore.getState().identityVerified) {
        void useEnclaveOrgStore.getState().ensureIdentity();
      }
    } catch (e) {
      // 平台不可达时按未登录处理：不打断本地功能
      set({ loaded: true, error: errorMessage(e) });
    }
  },

  login: async () => {
    if (get().loggingIn) return;
    const generation = ++loginGeneration;
    set({ loggingIn: true, error: '' });
    try {
      const { authorizeUrl } = await invoke<{ authorizeUrl: string }>('account_login_begin');
      await openUrl(authorizeUrl);
      const account = await invoke<AccountInfo>('account_login_complete');
      if (generation !== loginGeneration) return; // 已被取消
      set({ account, loaded: true });
      // 账号已切换：清空上一个账号的飞地缓存，再按新身份重建
      resetEnclaveCaches();
      void reloadEnclaveForAccount();
    } catch (e) {
      if (generation === loginGeneration) set({ error: errorMessage(e) });
    } finally {
      if (generation === loginGeneration) set({ loggingIn: false });
    }
  },

  logout: async () => {
    const generation = ++loginGeneration;
    try {
      await invoke('account_logout');
    } catch (e) {
      if (generation === loginGeneration) set({ error: errorMessage(e) });
    }
    if (generation === loginGeneration) set({ account: null, loggingIn: false });
    // 退出后不得继续展示本机团队数据（后端已清空 local_identity）
    resetEnclaveCaches();
  },

  cancelLogin: () => {
    loginGeneration += 1;
    set({ loggingIn: false });
  },

  openPlatform: async (page) => {
    try {
      const path =
        page === 'upgrade'
          ? '/account/upgrade/'
          : page === 'organizations'
            ? '/account/organizations/'
            : '/account/';
      await openPlatformPath(path);
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },

  openOrganization: async (context) => {
    try {
      const params = new URLSearchParams();
      if (context?.organizationId && Number.isInteger(context.organizationId)) {
        params.set('org', String(context.organizationId));
        params.set('tab', 'billing');
      }
      if (context?.capability) {
        params.set('intent', context.capability);
      }
      if (context?.source) params.set('source', context.source);
      const query = params.toString();
      await openPlatformPath(`/account/organizations/${query ? `?${query}` : ''}`);
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },

  openUpgrade: async (context) => {
    try {
      const params = new URLSearchParams();
      if (context?.plan) params.set('plan', context.plan);
      if (context?.capability) params.set('capability', context.capability);
      if (context?.source) params.set('source', context.source);
      const query = params.toString();
      await openPlatformPath(`/account/upgrade/${query ? `?${query}` : ''}`);
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },
}));

/** 是否已解锁某能力（未登录 / 未订阅 = 未解锁） */
export function hasCapability(account: AccountInfo | null, key: string): boolean {
  return Boolean(account?.capabilities.includes(key));
}

/** 组件里判断当前是否解锁某能力（账号变化会自动重渲染） */
export function useCapability(key: string): boolean {
  const account = useAccountStore((s) => s.account);
  return hasCapability(account, key);
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
