/**
 * 语音输入配置（可选覆盖项）
 *
 * 语音转写默认用「当前会话所选模型」，但多数对话模型（gpt-4o / deepseek-chat / qwen-max…）
 * 没有 `/audio/transcriptions` 能力，实测会直接 404/500。这里允许指定一个**专用转写模型**，
 * 配置后语音输入优先用它；不配置则与会话模型保持一致（原行为）。
 *
 * 持久化复用 app_settings 的 KV（与 customTabsStore 同一范式，不新增表）。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

const STORAGE_KEY = 'voice_input_config';

export interface VoiceInputConfig {
  /** 转写用的提供商 id；空 = 未配置（跟随当前会话所选模型） */
  providerId: string;
  /** 转写模型名，如 whisper-1 */
  model: string;
}

const EMPTY: VoiceInputConfig = { providerId: '', model: '' };

interface VoiceInputState {
  config: VoiceInputConfig;
  loaded: boolean;
  /** 读取配置（幂等；组件挂载时调用即可） */
  load: () => Promise<void>;
  save: (config: VoiceInputConfig) => Promise<void>;
}

export const useVoiceInputStore = create<VoiceInputState>((set, get) => ({
  config: EMPTY,
  loaded: false,

  load: async () => {
    if (get().loaded) return;
    try {
      const raw = await invoke<string | null>('get_app_setting', { key: STORAGE_KEY });
      const parsed = raw ? (JSON.parse(raw) as Partial<VoiceInputConfig>) : null;
      set({
        config: {
          providerId: typeof parsed?.providerId === 'string' ? parsed.providerId : '',
          model: typeof parsed?.model === 'string' ? parsed.model : '',
        },
        loaded: true,
      });
    } catch {
      // 读失败按"未配置"处理，不阻塞语音入口
      set({ loaded: true });
    }
  },

  save: async (config) => {
    await invoke('set_app_setting', { key: STORAGE_KEY, value: JSON.stringify(config) });
    set({ config, loaded: true });
  },
}));

/** 是否配置了专用转写模型（provider 与 model 都齐全才算） */
export function hasVoiceOverride(config: VoiceInputConfig): boolean {
  return Boolean(config.providerId && config.model);
}
