import { create } from 'zustand';
import { listItems, saveItem, deleteItem, invokeAction, getItem } from '../utils/invokeHelper';

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
      const providers = await listItems<ApiProvider>('list_api_providers');
      set({ providers, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  saveProvider: async (data) => {
    set({ error: null });
    try {
      const result = await saveItem<ApiProvider>('upsert_api_provider', data);
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
      await deleteItem('delete_api_provider', id);
      await get().fetchProviders();
    } catch (err) {
      set({ error: String(err) });
      throw err;
    }
  },

  reorderProviders: async (ids) => {
    try {
      await invokeAction('reorder_api_providers', { ids });
      await get().fetchProviders();
    } catch (err) {
      set({ error: String(err) });
    }
  },
}));

/** Get raw API key for a provider */
export async function getApiKey(providerId: string): Promise<string | null> {
  return getItem<string>('get_api_key', { id: providerId });
}

/** Get API endpoint for a provider by ID */
export async function getApiEndpoint(providerId: string): Promise<string | null> {
  try {
    const provider = await getItem<ApiProvider>('get_api_provider', { id: providerId });
    return provider?.apiEndpoint ?? null;
  } catch {
    return null;
  }
}
