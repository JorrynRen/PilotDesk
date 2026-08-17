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

interface CustomTabsState {
  tabs: CustomTab[];
  activeTabId: string | null;
  loaded: boolean;
  load: () => Promise<void>;
  addTab: (label: string, url: string) => Promise<void>;
  updateTab: (id: string, patch: Partial<Pick<CustomTab, 'label' | 'url'>>) => Promise<void>;
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
    const nextOrder = get().tabs.reduce((max, t) => Math.max(max, t.order), -1) + 1;
    const tab: CustomTab = { id: generateId(), label: label.trim(), url: url.trim(), order: nextOrder };
    if (!tab.label || !tab.url) return;
    const tabs = normalize([...get().tabs, tab]);
    set({ tabs });
    await persist(tabs);
  },

  updateTab: async (id, patch) => {
    const tabs = get().tabs.map((t) => (t.id === id ? { ...t, ...patch } : t));
    set({ tabs });
    await persist(tabs);
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
