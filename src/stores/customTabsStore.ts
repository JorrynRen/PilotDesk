import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

/** 自定义标签页：label 显示名，url 为网络地址(http/https)或本地文件路径 */
export interface CustomTab {
  id: string;
  label: string;
  url: string;
  /** 排序序号（显式持久化到 app_settings，保证排序字段落库） */
  order: number;
}

/** 存于 app_settings 的 key（复用现有 KV 表，不新增表/外部文件） */
const STORAGE_KEY = 'custom_tabs';

/* ───────────────────────── 录入约束（单一事实来源） ─────────────────────────
 * 设置页提交与 store 写入**双层**都调用 validateCustomTabInput，以 store 为准：
 * 页面调用只为即时反馈，store 调用才是最终把关（含数量、去重）。
 */
/** 标签名称长度上限 */
export const CUSTOM_TAB_LABEL_MAX = 64;
/** 地址长度上限 */
export const CUSTOM_TAB_URL_MAX = 2048;
/** 自定义标签数量上限 */
export const CUSTOM_TAB_MAX_COUNT = 50;

/** Windows 盘符绝对路径：C:\... 或 C:/... */
const WIN_DRIVE_RE = /^[a-zA-Z]:[\\/]/;
/** UNC 路径：\\server\share... 或 //server/share... */
const UNC_RE = /^(?:\\\\|\/\/)[^\\/]+[\\/][^\\/]+/;
/** 其它协议 scheme（javascript: / data: / chrome: 等），用于明确拒绝 */
const OTHER_SCHEME_RE = /^[a-zA-Z][a-zA-Z0-9+.-]*:/;
/** 无协议的裸域名：至少一个点 + 字母 TLD，可带端口 / 路径 / 查询 / 锚点 */
const BARE_DOMAIN_RE = /^(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?\.)+[a-zA-Z]{2,}(?::\d+)?(?:[/?#].*)?$/;

/** 校验结果：ok=true 时给出清洗（trim / 补协议）后的 label 与 url */
export type CustomTabValidation =
  | { ok: true; label: string; url: string }
  | { ok: false; error: string };

/** store 写操作结果：ok=false 时 error 为可直接展示的中文原因 */
export type CustomTabMutationResult = { ok: true } | { ok: false; error: string };

/** 本地路径口径与 CustomTabHost 一致：file:// 或 Windows 盘符 / UNC 绝对路径 */
function isLocalPath(u: string): boolean {
  return /^file:\/\//i.test(u) || WIN_DRIVE_RE.test(u) || UNC_RE.test(u);
}

/**
 * 校验并规范化自定义标签输入。
 * @param existingTabs 现有标签（用于数量上限与去重）
 * @param ignoreId     编辑场景传自身 id（去重与数量校验时跳过自身）
 */
export function validateCustomTabInput(
  rawLabel: string,
  rawUrl: string,
  existingTabs: CustomTab[],
  ignoreId?: string | null,
): CustomTabValidation {
  const label = (rawLabel ?? '').trim();
  const url = (rawUrl ?? '').trim();

  if (!label) return { ok: false, error: '标签名称不能为空' };
  if (label.length > CUSTOM_TAB_LABEL_MAX) {
    return { ok: false, error: `标签名称过长：最多 ${CUSTOM_TAB_LABEL_MAX} 个字符（当前 ${label.length} 个）` };
  }
  if (!url) return { ok: false, error: '地址不能为空' };
  if (url.length > CUSTOM_TAB_URL_MAX) {
    return { ok: false, error: `地址过长：最多 ${CUSTOM_TAB_URL_MAX} 个字符（当前 ${url.length} 个）` };
  }

  // 协议白名单：http(s) 直用；file:// 与本地绝对路径（盘符 / UNC）放行；其它 scheme 明确拒绝
  let normalizedUrl: string;
  if (/^https?:\/\//i.test(url) || isLocalPath(url)) {
    normalizedUrl = url;
  } else if (OTHER_SCHEME_RE.test(url)) {
    const scheme = url.slice(0, url.indexOf(':')).toLowerCase();
    return { ok: false, error: `不支持的协议「${scheme}:」：仅支持 http://、https:// 以及本地文件/目录路径` };
  } else if (BARE_DOMAIN_RE.test(url)) {
    // 无协议的域名（如 www.example.com）：自动补 https://，否则会被本应用当作本地文件路径
    normalizedUrl = `https://${url}`;
  } else {
    return {
      ok: false,
      error: '无法识别的地址：请输入 http:// 或 https:// 网址，或本地文件/目录的绝对路径（如 E:\\doc 或 E:\\doc\\index.html）',
    };
  }

  // 数量上限：仅新增时统计（编辑自身不新增）
  if (!ignoreId && existingTabs.length >= CUSTOM_TAB_MAX_COUNT) {
    return { ok: false, error: `自定义标签数量已达上限（最多 ${CUSTOM_TAB_MAX_COUNT} 条）` };
  }

  // 去重：同一地址已存在则拒绝（编辑自身跳过）
  const dup = existingTabs.find((t) => t.id !== ignoreId && t.url === normalizedUrl);
  if (dup) return { ok: false, error: `该地址已存在（「${dup.label}」），请勿重复添加` };

  return { ok: true, label, url: normalizedUrl };
}

interface CustomTabsState {
  tabs: CustomTab[];
  activeTabId: string | null;
  loaded: boolean;
  load: () => Promise<void>;
  addTab: (label: string, url: string) => Promise<CustomTabMutationResult>;
  updateTab: (id: string, patch: Partial<Pick<CustomTab, 'label' | 'url'>>) => Promise<CustomTabMutationResult>;
  removeTab: (id: string) => Promise<void>;
  reorderTabs: (from: number, to: number) => Promise<void>;
  setActiveTab: (id: string | null) => void;
}

function generateId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `tab_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;
}

async function persist(tabs: CustomTab[]): Promise<void> {
  try {
    await invoke('set_app_setting', { key: STORAGE_KEY, value: JSON.stringify(tabs) });
  } catch (e) {
    console.warn('[CustomTabs] 保存失败:', e);
  }
}

/** 依据 order 排序并重写为连续序号 0..n-1 */
function normalize(tabs: CustomTab[]): CustomTab[] {
  return tabs
    .slice()
    .sort((a, b) => (a.order ?? 0) - (b.order ?? 0))
    .map((t, i) => ({ ...t, order: i }));
}

export const useCustomTabsStore = create<CustomTabsState>((set, get) => ({
  tabs: [],
  activeTabId: null,
  loaded: false,

  load: async () => {
    try {
      const raw = await invoke<string | null>('get_app_setting', { key: STORAGE_KEY });
      if (typeof raw === 'string' && raw.trim()) {
        const parsed = JSON.parse(raw);
        if (Array.isArray(parsed)) {
          const tabs = normalize(
            parsed
              .filter((t) => t && typeof t.label === 'string' && typeof t.url === 'string')
              .map((t) => ({
                id: t.id || generateId(),
                label: t.label,
                url: t.url,
                order: typeof t.order === 'number' ? t.order : -1,
              }))
          );
          set({ tabs, loaded: true });
          return;
        }
      }
    } catch (e) {
      console.warn('[CustomTabs] 读取失败:', e);
    }
    set({ loaded: true });
  },

  addTab: async (label, url) => {
    // store 为最终把关：设置页虽已即时校验，但此处仍再校验一次（以 store 为准）
    const checked = validateCustomTabInput(label, url, get().tabs, null);
    if (!checked.ok) return { ok: false as const, error: checked.error };
    const nextOrder = get().tabs.reduce((max, t) => Math.max(max, t.order), -1) + 1;
    const tab: CustomTab = { id: generateId(), label: checked.label, url: checked.url, order: nextOrder };
    const tabs = normalize([...get().tabs, tab]);
    set({ tabs });
    await persist(tabs);
    return { ok: true as const };
  },

  updateTab: async (id, patch) => {
    const current = get().tabs.find((t) => t.id === id);
    if (!current) return { ok: false as const, error: '标签不存在或已被删除' };
    // 合并补丁后再校验：编辑自身 id 需排除，避免"只改名称却因原 URL 与自身重复"被误拒
    const checked = validateCustomTabInput(
      patch.label ?? current.label,
      patch.url ?? current.url,
      get().tabs,
      id,
    );
    if (!checked.ok) return { ok: false as const, error: checked.error };
    const tabs = get().tabs.map((t) =>
      t.id === id ? { ...t, label: checked.label, url: checked.url } : t
    );
    set({ tabs });
    await persist(tabs);
    return { ok: true as const };
  },

  removeTab: async (id) => {
    const tabs = normalize(get().tabs.filter((t) => t.id !== id));
    set({
      tabs,
      activeTabId: get().activeTabId === id ? null : get().activeTabId,
    });
    await persist(tabs);
  },

  reorderTabs: async (from, to) => {
    if (from === to || from < 0 || to < 0) return;
    const tabs = [...get().tabs];
    const [moved] = tabs.splice(from, 1);
    tabs.splice(to, 0, moved);
    const ordered = normalize(tabs);
    set({ tabs: ordered });
    await persist(ordered);
  },

  setActiveTab: (id) => set({ activeTabId: id }),
}));
