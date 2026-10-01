import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useThemeStore } from '../stores/themeStore';

export type Theme = 'light' | 'dark' | 'nightfall' | 'system';

/** 可选主题白名单（读取持久化设置与切换时都要校验，避免脏值落到 data-theme 上） */
export const THEMES: Theme[] = ['light', 'dark', 'nightfall', 'system'];

/**
 * 默认主题（用户从未设置过主题时生效）。
 *
 * 单一事实来源：main.tsx 首屏前应用与 useTheme 的初值都必须取这里，
 * 否则两处各写一个兜底值会出现"首屏一个主题、挂载后跳成另一个"。
 */
export const DEFAULT_THEME: Theme = 'nightfall';

/** 主题是否为深色族：所有"按深浅分叉"的地方都该走这里，而不是逐处写 `theme === 'dark'` */
export function isDarkTheme(theme: Theme, systemPrefersDark = false): boolean {
  if (theme === 'nightfall' || theme === 'dark') return true;
  if (theme === 'system') return systemPrefersDark;
  return false;
}

/**
 * 把主题写到 <html> 上：`data-theme` 决定具体配色，`data-theme-kind` 只标记深浅族。
 * 样式里凡是"深色下要不同的非配色行为"（按钮 hover 叠白、#root 描边、prose 覆盖、
 * toast 配色）都挂在 data-theme-kind 上，这样以后再加深色主题不必回来改这些选择器。
 */
export function applyThemeToDocument(theme: Theme, systemPrefersDark: boolean): void {
  const resolved = theme === 'system' ? (systemPrefersDark ? 'dark' : 'light') : theme;
  document.documentElement.setAttribute('data-theme', resolved);
  document.documentElement.setAttribute('data-theme-kind', isDarkTheme(theme, systemPrefersDark) ? 'dark' : 'light');
}

/**
 * useTheme — 主题管理 hook。
 *
 * 持久化到 SQLite app_settings 表（而非 localStorage），
 * 确保与后端主题设置一致。
 */
export function useTheme() {
  /**
   * 初值取 localStorage 缓存（与 main.tsx 首屏前应用的是同一份），读不到才落 DEFAULT_THEME：
   * useTheme 是"每个调用方各持一份状态"的 hook（设置页、关于页各一次），
   * 若初值写死成 DEFAULT_THEME，后挂载的实例会在读到数据库之前先把 DOM 刷成默认主题，
   * 出现"进设置页主题跳一下"的闪变。
   */
  const [theme, setThemeState] = useState<Theme>(() => {
    try {
      const cached = localStorage.getItem('pilotdesk-theme');
      return cached && THEMES.includes(cached as Theme) ? (cached as Theme) : DEFAULT_THEME;
    } catch { return DEFAULT_THEME; }
  });
  const [loaded, setLoaded] = useState(false);

  // Sync localStorage cache so main.tsx applyThemeEarly() stays consistent
  const syncToCache = (t: Theme) => {
    try { localStorage.setItem('pilotdesk-theme', t); } catch { /* ignore */ }
  };

  // Load theme from SQLite on mount
  useEffect(() => {
    (async () => {
      try {
        const saved = await invoke<string | null>('get_app_setting', { key: 'theme' });
        if (saved && THEMES.includes(saved as Theme)) {
          setThemeState(saved as Theme);
          syncToCache(saved as Theme);
        }
      } catch { /* fallback to default */ }
      setLoaded(true);
    })();
  }, []);

  // Load custom theme colors
  const { loadColors } = useThemeStore();
  useEffect(() => {
    loadColors();
  }, [loadColors]);

  const setTheme = async (t: Theme) => {
    setThemeState(t);
    syncToCache(t);
    try {
      await invoke('set_app_setting', { key: 'theme', value: t });
    } catch { /* ignore */ }
  };

  // Apply theme to document
  useEffect(() => {
    if (!loaded) return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = () => applyThemeToDocument(theme, mq.matches);
    apply();
    if (theme === 'system') {
      mq.addEventListener('change', apply);
      return () => mq.removeEventListener('change', apply);
    }
  }, [theme, loaded]);

  return { theme, setTheme, loaded };
}
