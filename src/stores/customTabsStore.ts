import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

/** 门户标签页：label 显示名，url 为网络地址(http/https)或本地文件路径 */
export interface CustomTab {
  id: string;
  label: string;
  url: string;
  /** 排序序号（显式持久化到 app_settings，保证排序字段落库） */
  order: number;
  /** 所属分组名（可选）：空 = 未分组；门户页按分组渲染卡片网格 */
  group?: string;
  /** 图标 key（可选）：取值限于 CUSTOM_TAB_ICONS；未设置 = 默认地球图标 */
  icon?: string;
}

/**
 * 门户页的哨兵「标签 id」：门户本身不是一条门户标签，仅用于表示「custom 模式下激活的是门户页」。
 * 用哨兵值而非新增布尔字段，是为了复用现有 activeTabId 的单一激活语义
 * （顶栏高亮、内容区切换、打开/关闭逻辑都只读它）。
 */
export const PORTAL_TAB_ID = '__portal__';

/** 存于 app_settings 的 key（复用现有 KV 表，不新增表/外部文件） */
const STORAGE_KEY = 'custom_tabs';
/** 顶栏「平铺显示个数」的独立 KV key（与标签列表分开存，互不影响） */
const TITLEBAR_LIMIT_KEY = 'custom_tabs_titlebar_limit';

/* ───────────────────────── 顶栏平铺显示个数 ───────────────────────── */
/** 默认值：顶栏最多平铺 3 个门户标签 */
export const DEFAULT_TITLEBAR_LIMIT = 3;
/** 可选范围下界（0 = 不在顶栏平铺，仅从「更多」进入） */
export const TITLEBAR_LIMIT_MIN = 0;
/** 可选范围上界（最多 3 个） */
export const TITLEBAR_LIMIT_MAX = 3;

/**
 * 顶栏平铺显示个数容错：非数字 / 非整数 / 越界 → 一律回落默认值 3。
 * 用于加载持久化值时的兜底，避免脏数据（手工改 KV、旧版本写入）把顶栏撑坏。
 */
export function normalizeTitleBarLimit(raw: unknown): number {
  const n = typeof raw === 'number' ? raw : typeof raw === 'string' ? Number(raw) : NaN;
  if (!Number.isFinite(n)) return DEFAULT_TITLEBAR_LIMIT;
  const i = Math.trunc(n);
  if (i < TITLEBAR_LIMIT_MIN || i > TITLEBAR_LIMIT_MAX) return DEFAULT_TITLEBAR_LIMIT;
  return i;
}

/* ───────────────────────── 录入约束（单一事实来源） ─────────────────────────
 * 设置页提交与 store 写入**双层**都调用 validateCustomTabInput，以 store 为准：
 * 页面调用只为即时反馈，store 调用才是最终把关（含数量、去重）。
 */
/** 标签名称长度上限 */
export const CUSTOM_TAB_LABEL_MAX = 64;
/** 地址长度上限 */
export const CUSTOM_TAB_URL_MAX = 2048;
/** 门户标签数量上限 */
export const CUSTOM_TAB_MAX_COUNT = 50;
/** 分组名长度上限 */
export const CUSTOM_TAB_GROUP_MAX = 32;

/**
 * 可选图标集（有限集合）。
 * 用有限集合而非任意字符串：图标渲染靠 key → 组件映射，任意字符串会渲染成空白，
 * 且用户输入的 emoji / 图标名都无法保证存在，故收敛为一组预置图标。
 */
export const CUSTOM_TAB_ICONS = [
  'globe', 'link', 'book', 'briefcase', 'code', 'chart', 'mail', 'calendar', 'file', 'star',
] as const;
export type CustomTabIcon = (typeof CUSTOM_TAB_ICONS)[number];

/** 图标 key 容错：非法 / 缺失一律返回 undefined（渲染时回落默认地球图标） */
export function normalizeTabIcon(raw: unknown): string | undefined {
  return typeof raw === 'string' && (CUSTOM_TAB_ICONS as readonly string[]).includes(raw) ? raw : undefined;
}

/** 图标展示名（设置页下拉用；与 CUSTOM_TAB_ICONS 一一对应） */
export const CUSTOM_TAB_ICON_LABELS: Record<CustomTabIcon, string> = {
  globe: '地球', link: '链接', book: '文档', briefcase: '工作', code: '代码',
  chart: '图表', mail: '邮件', calendar: '日历', file: '文件', star: '收藏',
};

/** 图标下拉选项：由图标集 + 展示名拼装，保证「可选值」与「渲染映射」同源 */
export const TAB_ICON_OPTIONS: { value: string; label: string }[] =
  CUSTOM_TAB_ICONS.map((v) => ({ value: v, label: CUSTOM_TAB_ICON_LABELS[v] }));

/** 分组名规范化：非字符串 / trim 后为空 → undefined（视为未分组）；超长截断到上限 */
export function normalizeTabGroup(raw: unknown): string | undefined {
  if (typeof raw !== 'string') return undefined;
  const g = raw.trim();
  if (!g) return undefined;
  return g.length > CUSTOM_TAB_GROUP_MAX ? g.slice(0, CUSTOM_TAB_GROUP_MAX) : g;
}

/**
 * 门户页分组推导（纯函数，便于复用与测试）：
 * - 组顺序 = 组内最小 order（按 order 升序扫描，首次出现的组即其最小 order 位置）；
 * - 组内元素按 order 升序；
 * - 未分组（group 为空）恒排最后，用 group=null 表示。
 * 不新增排序字段：组顺序完全由现有 order 推导，避免两套顺序源相互冲突。
 */
export function groupCustomTabs(tabs: CustomTab[]): { group: string | null; items: CustomTab[] }[] {
  const sorted = [...tabs].sort((a, b) => (a.order ?? 0) - (b.order ?? 0));
  const map = new Map<string, CustomTab[]>();
  const ungrouped: CustomTab[] = [];
  for (const t of sorted) {
    const g = normalizeTabGroup(t.group);
    if (!g) { ungrouped.push(t); continue; }
    const arr = map.get(g);
    if (arr) arr.push(t); else map.set(g, [t]);
  }
  const groups: { group: string | null; items: CustomTab[] }[] =
    [...map.entries()].map(([group, items]) => ({ group, items }));
  if (ungrouped.length) groups.push({ group: null, items: ungrouped });
  return groups;
}

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
 * 校验并规范化门户标签输入。
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
    return { ok: false, error: `门户标签数量已达上限（最多 ${CUSTOM_TAB_MAX_COUNT} 条）` };
  }

  // 去重：同一地址已存在则拒绝（编辑自身跳过）
  const dup = existingTabs.find((t) => t.id !== ignoreId && t.url === normalizedUrl);
  if (dup) return { ok: false, error: `该地址已存在（「${dup.label}」），请勿重复添加` };

  return { ok: true, label, url: normalizedUrl };
}

interface CustomTabsState {
  tabs: CustomTab[];
  activeTabId: string | null;
  /** 顶栏平铺显示的门户标签个数（0–3），持久化在独立 KV key */
  titleBarLimit: number;
  loaded: boolean;
  load: () => Promise<void>;
  addTab: (label: string, url: string, group?: string, icon?: string) => Promise<CustomTabMutationResult>;
  updateTab: (
    id: string,
    patch: Partial<Pick<CustomTab, 'label' | 'url' | 'group' | 'icon'>>,
  ) => Promise<CustomTabMutationResult>;
  removeTab: (id: string) => Promise<void>;
  reorderTabs: (from: number, to: number) => Promise<void>;
  setActiveTab: (id: string | null) => void;
  /** 设置顶栏平铺显示个数（自动夹取到 0–3 并落库） */
  setTitleBarLimit: (limit: number) => Promise<void>;
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

/** 持久化顶栏平铺显示个数（独立 KV key，纯数字字符串） */
async function persistTitleBarLimit(limit: number): Promise<void> {
  try {
    await invoke('set_app_setting', { key: TITLEBAR_LIMIT_KEY, value: String(limit) });
  } catch (e) {
    console.warn('[CustomTabs] 保存顶栏显示个数失败:', e);
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
  titleBarLimit: DEFAULT_TITLEBAR_LIMIT,
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
                // 分组 / 图标：旧数据（无这两字段）经规范化后为 undefined，即未分组 + 默认图标
                group: normalizeTabGroup(t.group),
                icon: normalizeTabIcon(t.icon),
              }))
          );
          set({ tabs });
        }
      }
    } catch (e) {
      console.warn('[CustomTabs] 读取失败:', e);
    }
    // 顶栏显示个数：独立 KV 读取，读不到 / 脏数据一律回落默认 3
    try {
      const rawLimit = await invoke<string | null>('get_app_setting', { key: TITLEBAR_LIMIT_KEY });
      set({ titleBarLimit: normalizeTitleBarLimit(rawLimit) });
    } catch (e) {
      console.warn('[CustomTabs] 读取顶栏显示个数失败:', e);
      set({ titleBarLimit: DEFAULT_TITLEBAR_LIMIT });
    }
    set({ loaded: true });
  },

  addTab: async (label, url, group, icon) => {
    // store 为最终把关：设置页虽已即时校验，但此处仍再校验一次（以 store 为准）
    const checked = validateCustomTabInput(label, url, get().tabs, null);
    if (!checked.ok) return { ok: false as const, error: checked.error };
    const nextOrder = get().tabs.reduce((max, t) => Math.max(max, t.order), -1) + 1;
    const tab: CustomTab = {
      id: generateId(),
      label: checked.label,
      url: checked.url,
      order: nextOrder,
      group: normalizeTabGroup(group),
      icon: normalizeTabIcon(icon),
    };
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
    // 分组 / 图标：设置页始终传入完整字段（清空即空串），故用 ?? 兜底"未传"分支即可
    const nextGroup = normalizeTabGroup(patch.group ?? current.group);
    const nextIcon = normalizeTabIcon(patch.icon ?? current.icon);
    const tabs = get().tabs.map((t) =>
      t.id === id
        ? { ...t, label: checked.label, url: checked.url, group: nextGroup, icon: nextIcon }
        : t
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

  setTitleBarLimit: async (limit) => {
    // 防御非法入参：夹取到 0–3 后落库（UI 只会给合法值，这里兜底）
    const clamped = Math.min(TITLEBAR_LIMIT_MAX, Math.max(TITLEBAR_LIMIT_MIN, Math.trunc(limit)));
    set({ titleBarLimit: clamped });
    await persistTitleBarLimit(clamped);
  },
}));
