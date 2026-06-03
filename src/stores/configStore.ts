import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

export interface ClaudeConfigPublic {
  model: string | null;
  apiEndpoint: string | null;
  apiKeyMasked: string | null;
  apiKeySet: boolean;
  mcpServers: Record<string, unknown> | null;
  customInstructions: string | null;
  theme: string | null;
  maxTokens: number | null;
  extra: Record<string, string> | null;
}

export interface HermesConfigPublic {
  model: string | null;
  apiEndpoint: string | null;
  apiKeyMasked: string | null;
  apiKeySet: boolean;
  temperature: number | null;
  maxTokens: number | null;
  systemPrompt: string | null;
  mcpServers: Record<string, unknown> | null;
  skillsDir: string | null;
  extra: Record<string, string> | null;
}

export interface ConfigResult {
  claude: ClaudeConfigPublic | null;
  hermes: HermesConfigPublic | null;
  claude_installed: boolean;
  hermes_installed: boolean;
}

interface ConfigState {
  config: ConfigResult | null;
  loading: boolean;
  error: string | null;
  saving: boolean;
  fetchConfig: () => Promise<void>;
  saveClaudeConfig: (update: Partial<ClaudeConfigPublic> & { api_key?: string; api_endpoint?: string | null }) => Promise<void>;
  saveHermesConfig: (update: Partial<HermesConfigPublic> & { api_key?: string }) => Promise<void>;
  clearError: () => void;
}

export const useConfigStore = create<ConfigState>((set, get) => ({
  config: null,
  loading: false,
  error: null,
  saving: false,
  fetchConfig: async () => {
    set({ loading: true, error: null });
    try {
      const config = await invoke<ConfigResult>('get_config');
      set({ config, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  saveClaudeConfig: async (update) => {
    set({ saving: true, error: null });
    try {
      // If api_key is not provided, mark as UNCHANGED to preserve existing
      const payload = {
        ...update,
        api_key: update.api_key ?? 'UNCHANGED',
      };
      const result = await invoke<ClaudeConfigPublic>('save_claude_config', { update: payload });
      const prev = get().config;
      if (prev) {
        set({ config: { ...prev, claude: result }, saving: false });
      } else {
        set({ saving: false });
      }
    } catch (err) {
      set({ error: String(err), saving: false });
    }
  },

  saveHermesConfig: async (update) => {
    set({ saving: true, error: null });
    try {
      const payload = {
        ...update,
        api_key: update.api_key ?? 'UNCHANGED',
      };
      const result = await invoke<HermesConfigPublic>('save_hermes_config', { update: payload });
      const prev = get().config;
      if (prev) {
        set({ config: { ...prev, hermes: result }, saving: false });
      } else {
        set({ saving: false });
      }
    } catch (err) {
      set({ error: String(err), saving: false });
    }
  },

  clearError: () => set({ error: null }),
}));
