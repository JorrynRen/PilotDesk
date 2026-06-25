import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useThemeStore } from '../stores/themeStore';

type Theme = 'light' | 'dark' | 'system';

/**
 * useTheme — 主题管理 hook。
 *
 * 持久化到 SQLite app_settings 表（而非 localStorage），
 * 确保与后端主题设置一致。
 */
export function useTheme() {
  const [theme, setThemeState] = useState<Theme>('system');
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
        if (saved && (saved === 'light' || saved === 'dark' || saved === 'system')) {
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
    const apply = () => {
      if (theme === 'system') {
        const isDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
        document.documentElement.setAttribute('data-theme', isDark ? 'dark' : 'light');
      } else {
        document.documentElement.setAttribute('data-theme', theme);
      }
    };
    apply();
    if (theme === 'system') {
      const mq = window.matchMedia('(prefers-color-scheme: dark)');
      mq.addEventListener('change', apply);
      return () => mq.removeEventListener('change', apply);
    }
  }, [theme, loaded]);

  return { theme, setTheme, loaded };
}
