import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

export interface ThemeColors {
  accent: string;
  accentHover: string;
  accentLight: string;
}

const DEFAULT_COLORS: ThemeColors = {
  accent: '#3B82F6',
  accentHover: '#2563EB',
  accentLight: 'rgba(59, 130, 246, 0.15)',
};

interface ThemeStoreState {
  colors: ThemeColors;
  loaded: boolean;
  loadColors: () => Promise<void>;
  setAccentColor: (color: string) => Promise<void>;
  resetColors: () => Promise<void>;
}

export const useThemeStore = create<ThemeStoreState>((set, get) => ({
  colors: DEFAULT_COLORS,
  loaded: false,

  loadColors: async () => {
    try {
      const saved = await invoke<string | null>('get_app_setting', { key: 'theme_colors' });
      if (saved) {
        const parsed = JSON.parse(saved) as Partial<ThemeColors>;
        set({
          colors: { ...DEFAULT_COLORS, ...parsed },
          loaded: true,
        });
        return;
      }
    } catch { /* ignore */ }
    set({ loaded: true });
  },

  setAccentColor: async (color: string) => {
    const colors: ThemeColors = {
      accent: color,
      accentHover: adjustColor(color, -20),
      accentLight: hexToRgba(color, 0.15),
    };
    set({ colors });
    try {
      await invoke('set_app_setting', { key: 'theme_colors', value: JSON.stringify(colors) });
    } catch { /* ignore */ }
    applyThemeColors(colors);
  },

  resetColors: async () => {
    set({ colors: DEFAULT_COLORS });
    try {
      await invoke('set_app_setting', { key: 'theme_colors', value: '' });
    } catch { /* ignore */ }
    applyThemeColors(DEFAULT_COLORS);
  },
}));

function adjustColor(hex: string, amount: number): string {
  const num = parseInt(hex.replace('#', ''), 16);
  const r = Math.min(255, Math.max(0, ((num >> 16) & 0xFF) + amount));
  const g = Math.min(255, Math.max(0, ((num >> 8) & 0xFF) + amount));
  const b = Math.min(255, Math.max(0, (num & 0xFF) + amount));
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, '0')}`;
}

function hexToRgba(hex: string, alpha: number): string {
  const num = parseInt(hex.replace('#', ''), 16);
  const r = (num >> 16) & 0xFF;
  const g = (num >> 8) & 0xFF;
  const b = num & 0xFF;
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

function applyThemeColors(colors: ThemeColors) {
  document.documentElement.style.setProperty('--accent', colors.accent);
  document.documentElement.style.setProperty('--accent-hover', colors.accentHover);
  document.documentElement.style.setProperty('--accent-light', colors.accentLight);
}
