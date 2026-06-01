import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

export interface ApiProvider {
  id: string;
  name: string;
  apiEndpoint: string;
  apiKeyMasked: string;
  apiKeySet: boolean;
  models: string[];
  sortOrder: number;
  createdAt: number;
  updatedAt: number;
}

interface ApiProviderState {
  providers: ApiProvider[];
  loading: boolean;
  error: string | null;
  fetchProviders: () => Promise<void>;
  saveProvider: (data: {
    id: string;
    name: string;
    apiEndpoint: string;
    apiKey?: string;
    models: string[];
    sortOrder?: number;
  }) => Promise<ApiProvider>;
  deleteProvider: (id: string) => Promise<void>;
  reorderProviders: (ids: string[]) => Promise<void>;
}

export const useApiProviderStore = create<ApiProviderState>((set, get) => ({
  providers: [],
  loading: false,
  error: null,

  fetchProviders: async () => {
    set({ loading: true, error: null });
    try {
      const providers = await invoke<ApiProvider[]>('list_api_providers');
      set({ providers, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  saveProvider: async (data) => {
    set({ error: null });
    try {
      const result = await invoke<ApiProvider>('upsert_api_provider', { payload: data });
      // Refresh list after save
      await get().fetchProviders();
      return result;
    } catch (err) {
      set({ error: String(err) });
      throw err;
    }
  },

  deleteProvider: async (id) => {
    set({ error: null });
    try {
      await invoke('delete_api_provider', { id });
      await get().fetchProviders();
    } catch (err) {
      set({ error: String(err) });
      throw err;
    }
  },

  reorderProviders: async (ids) => {
    try {
      await invoke('reorder_api_providers', { ids });
      await get().fetchProviders();
    } catch (err) {
      set({ error: String(err) });
    }
  },
}));

/**
 * Get raw API key for a provider (from SQLite, not exposed in provider list)
 */
export async function getApiKey(providerId: string): Promise<string | null> {
  try {
    return await invoke<string | null>('get_api_key', { id: providerId });
  } catch {
    return null;
  }
}

/**
 * Get API endpoint for a provider by ID
 */
export async function getApiEndpoint(providerId: string): Promise<string | null> {
  try {
    const provider = await invoke<ApiProvider | null>('get_api_provider', { id: providerId });
    return provider?.apiEndpoint ?? null;
  } catch {
    return null;
  }
}
