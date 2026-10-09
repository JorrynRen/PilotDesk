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
  /**
   * 当前**生效**的强调色（= CSS 里的 `--accent`）：
   * 用户自定义过就是 colors.accent，否则是**当前主题自带**的那一个
   * （深空天青 / 青瓷青绿 / 薄荷紫罗兰…）。
   * 主题自带的强调色只写在 globals.css 的各主题块里，JS 侧没有第二个来源，
   * 所以由 useTheme 在套用主题后调 syncEffectiveAccent() 读回同步 ——
   * 「主题色」区块靠它回显选中态；否则切到这些主题时会一直显示默认蓝，对不上。
   */
  effectiveAccent: string;
  loaded: boolean;
  loadColors: () => Promise<void>;
  setAccentColor: (color: string) => Promise<void>;
  resetColors: () => Promise<void>;
  /** 从 CSS 读回当前生效的强调色（见 effectiveAccent 的说明） */
  syncEffectiveAccent: () => void;
}

export const useThemeStore = create<ThemeStoreState>((set) => ({
  colors: DEFAULT_COLORS,
  effectiveAccent: DEFAULT_COLORS.accent,
  loaded: false,

  loadColors: async () => {
    try {
      const saved = await invoke<string | null>('get_app_setting', { key: 'theme_colors' });
      if (saved) {
        const parsed = JSON.parse(saved) as Partial<ThemeColors>;
        const colors = { ...DEFAULT_COLORS, ...parsed };
        set({ colors, effectiveAccent: colors.accent, loaded: true });
        // 已自定义过强调色：启动时也要套用，否则重启后用户选的色会丢
        applyThemeColors(colors);
        return;
      }
    } catch { /* ignore */ }
    // 未自定义：**不**写内联变量，让主题自带的强调色生效
    //（否则 深空 Nightfall 的天青会被这条默认蓝盖掉）
    clearThemeColorOverrides();
    // 生效色 = 主题自带的那个，读回来供「主题色」回显
    set({ loaded: true, effectiveAccent: readEffectiveAccent() });
  },

  setAccentColor: async (color: string) => {
    const colors: ThemeColors = {
      accent: color,
      accentHover: adjustColor(color, -20),
      accentLight: hexToRgba(color, 0.15),
    };
    set({ colors, effectiveAccent: colors.accent });
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
    // 重置 = 回到"跟随主题"，而不是回到某个写死的蓝
    clearThemeColorOverrides();
    set({ effectiveAccent: readEffectiveAccent() });
  },

  syncEffectiveAccent: () => {
    set({ effectiveAccent: readEffectiveAccent() });
  },
}));

/** 读当前生效的强调色（内联覆盖优先于主题块，getComputedStyle 天然按优先级给值） */
function readEffectiveAccent(): string {
  try {
    return getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() || DEFAULT_COLORS.accent;
  } catch {
    return DEFAULT_COLORS.accent;
  }
}

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

/**
 * 移除自定义强调色的内联覆盖，交回当前主题自带的 --accent。
 * 内联样式优先级高于任何样式表规则，不主动移除的话主题色永远被盖住。
 */
function clearThemeColorOverrides() {
  const root = document.documentElement;
  root.style.removeProperty('--accent');
  root.style.removeProperty('--accent-hover');
  root.style.removeProperty('--accent-light');
}
