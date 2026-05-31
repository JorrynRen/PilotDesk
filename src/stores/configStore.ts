import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

export interface ClaudeConfigPublic {
  model: string | null;
  api_endpoint: string | null;
  api_key_masked: string | null;
  api_key_set: boolean;
  mcp_servers: Record<string, unknown> | null;
  custom_instructions: string | null;
  theme: string | null;
  max_tokens: number | null;
  extra: Record<string, string> | null;
}

export interface HermesConfigPublic {
  model: string | null;
  api_endpoint: string | null;
  api_key_masked: string | null;
  api_key_set: boolean;
  temperature: number | null;
  max_tokens: number | null;
  system_prompt: string | null;
  mcp_servers: Record<string, unknown> | null;
  skills_dir: string | null;
  extra: Record<string, string> | null;
}

export interface ConfigResult {
  claude: ClaudeConfigPublic | null;
  hermes: HermesConfigPublic | null;
  claude_installed: boolean;
  hermes_installed: boolean;
}

export interface TestResult {
  agent_type: string;
  success: boolean;
  message: string;
  latency_ms: number | null;
}

interface ConfigState {
  config: ConfigResult | null;
  loading: boolean;
  error: string | null;
  saving: boolean;
  testResult: TestResult | null;

  fetchConfig: () => Promise<void>;
  saveClaudeConfig: (update: Partial<ClaudeConfigPublic> & { api_key?: string; api_endpoint?: string | null }) => Promise<void>;
  saveHermesConfig: (update: Partial<HermesConfigPublic> & { api_key?: string }) => Promise<void>;
  testConnection: (agentType: string) => Promise<TestResult>;
  clearError: () => void;
}

export const useConfigStore = create<ConfigState>((set, get) => ({
  config: null,
  loading: false,
  error: null,
  saving: false,
  testResult: null,

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

  testConnection: async (agentType: string) => {
    try {
      const result = await invoke<TestResult>('test_api_connection', { agentType });
      set({ testResult: result });
      return result;
    } catch (err) {
      set({ error: String(err) });
      return { agent_type: agentType, success: false, message: String(err), latency_ms: null };
    }
  },

  clearError: () => set({ error: null }),
}));
