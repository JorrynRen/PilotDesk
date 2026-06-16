import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { PluginInstance, SandboxInfo } from '../types/plugin';

interface PluginStoreState {
  plugins: PluginInstance[];
  sandboxInfo: SandboxInfo | null;
  loading: boolean;
  error: string | null;
  discover: () => Promise<void>;
  list: () => Promise<void>;
  enable: (id: string) => Promise<void>;
  disable: (id: string) => Promise<void>;
  fetchSandboxInfo: () => Promise<void>;
}

export const usePluginStore = create<PluginStoreState>((set, get) => ({
  plugins: [],
  sandboxInfo: null,
  loading: false,
  error: null,

  discover: async () => {
    set({ loading: true, error: null });
    try {
      const plugins = await invoke<PluginInstance[]>('plugin_discover');
      set({ plugins, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  list: async () => {
    set({ loading: true, error: null });
    try {
      const plugins = await invoke<PluginInstance[]>('plugin_list');
      set({ plugins, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  enable: async (id: string) => {
    try {
      await invoke('plugin_enable', { id });
      await get().list();
    } catch (err) {
      set({ error: String(err) });
    }
  },

  disable: async (id: string) => {
    try {
      await invoke('plugin_disable', { id });
      await get().list();
    } catch (err) {
      set({ error: String(err) });
    }
  },

  fetchSandboxInfo: async () => {
    try {
      const info = await invoke<SandboxInfo>('plugin_get_sandbox_info');
      set({ sandboxInfo: info });
    } catch (err) {
      console.warn('Failed to fetch sandbox info:', err);
    }
  },
}));
