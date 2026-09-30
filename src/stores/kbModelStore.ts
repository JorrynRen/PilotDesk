/**
 * 知识库模型配置（设置 › 知识库）
 *
 * 用于知识库的两件 LLM 事：**投喂后的整理**（标题/标签/专属属性）与 **AI 生成**（按主题产出知识）。
 * 不配置也能用：后端会回落到"第一个 OpenAI 兼容且配了 Key 的提供商"，与语音识别的专用覆盖项同一思路。
 *
 * 持久化复用 app_settings 的 KV（与 voiceInputStore / customTabsStore 同一范式，不新增表）。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

const STORAGE_KEY = 'kb_model_config';

export interface KbModelConfig {
  /** 提供商 id；空 = 未指定（走回落） */
  providerId: string;
  /** 模型名，如 deepseek-chat */
  model: string;
}

const EMPTY: KbModelConfig = { providerId: '', model: '' };

interface KbModelState {
  config: KbModelConfig;
  loaded: boolean;
  /** 读取配置（幂等；组件挂载时调用即可） */
  load: () => Promise<void>;
  save: (config: KbModelConfig) => Promise<void>;
}

export const useKbModelStore = create<KbModelState>((set, get) => ({
  config: EMPTY,
  loaded: false,

  load: async () => {
    if (get().loaded) return;
    try {
      const raw = await invoke<string | null>('get_app_setting', { key: STORAGE_KEY });
      const parsed = raw ? (JSON.parse(raw) as Partial<KbModelConfig>) : null;
      set({
        config: {
          providerId: typeof parsed?.providerId === 'string' ? parsed.providerId : '',
          model: typeof parsed?.model === 'string' ? parsed.model : '',
        },
        loaded: true,
      });
    } catch {
      // 读失败按"未指定"处理，不阻塞知识库入口
      set({ loaded: true });
    }
  },

  save: async (config) => {
    await invoke('set_app_setting', { key: STORAGE_KEY, value: JSON.stringify(config) });
    set({ config, loaded: true });
  },
}));

/** 是否显式指定了知识库模型（provider 与 model 都齐全才算） */
export function hasKbModelOverride(config: KbModelConfig): boolean {
  return Boolean(config.providerId && config.model);
}
