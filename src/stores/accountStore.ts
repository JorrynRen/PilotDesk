/**
 * 会员账号：登录状态与会员权益（entitlements）。
 *
 * 登录走平台（会员中心）的 PKCE 授权：Rust 侧起本地回环端口并把授权页地址给回来，
 * 这里用**系统浏览器**打开 —— 密码只在平台页面输入，产品端拿不到明文；
 * 令牌由 Rust 侧加密保存，前端只拿到「账号 + 等级 + 已解锁能力」。
 *
 * 新收费策略：**本地能力不设限**（本地功能对所有登录 / 未登录状态一律开放），
 * 仅**云端服务与内容**仍按能力门控（目前只有 `sync.cloud`）。
 */
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { open as openUrl } from '@tauri-apps/plugin-shell';

/** 可打开的平台页面：升级会员 / 会员中心（账号设置）/ 组织页 */
export type PlatformPage = 'upgrade' | 'portal' | 'organizations';

/** 云端能力键（平台侧登记，产品端只消费）：本地能力已全部开放，只剩云同步受门控 */
export const CAP_CLOUD_SYNC = 'sync.cloud';

/**
 * 已知云端能力键的中文名。
 *
 * **与平台登记表（`app/src/services/entitlements/capabilities.ts` 的 `label`）保持一致**——
 * 平台是唯一来源，这里只做展示；新增能力键时两边都要补，否则会显示成裸 key。
 * 未知键回退显示键本身。
 */
const CAPABILITY_LABELS: Record<string, string> = {
  'sync.cloud': '云同步',
  'team.audit': '团队操作审计',
  'team.shared-space': '团队共享空间',
  'team.usage-board': '团队用量看板',
  'team.provider-credentials': '团队共享凭据',
  'sso.login': '企业单点登录',
};

export function capabilityLabel(key: string): string {
  return CAPABILITY_LABELS[key] ?? key;
}

export interface AccountInfo {
  email: string;
  nickname: string;
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
  /** 取消等待（只影响界面；真正的回调结果会被丢弃） */
  cancelLogin: () => void;
}

/**
 * 登录代次：取消后自增，使仍在等待的 `account_login_complete` 结果被丢弃。
 * （Rust 侧的等待无法真正中断，但结果不会再写回状态。）
 */
let loginGeneration = 0;

export const useAccountStore = create<AccountStoreState>((set, get) => ({
  loaded: false,
  account: null,
  loggingIn: false,
  error: '',

  refresh: async () => {
    try {
      const account = await invoke<AccountInfo | null>('account_status');
      set({ account, loaded: true, error: '' });
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
  },

  cancelLogin: () => {
    loginGeneration += 1;
    set({ loggingIn: false });
  },

  openPlatform: async (page) => {
    try {
      // 基址只在 Rust 一处维护，这里按页面拼路径
      const base = await invoke<string>('account_platform_base');
      const path =
        page === 'upgrade'
          ? '/account/upgrade/'
          : page === 'organizations'
            ? '/account/organizations/'
            : '/account/';
      await openUrl(`${base.replace(/\/+$/, '')}${path}`);
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
