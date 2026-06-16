import { create } from 'zustand';
import { listItems, saveItem, deleteItem, invokeAction } from '../utils/invokeHelper';

export interface InspirationItem {
  id: string;
  icon: string;
  title: string;
  content: string;
  sourceAgent: string;
  isFavorite: boolean;
  tags: string[];
  createdAt: number;
  updatedAt: number;
}

interface InspirationState {
  inspirations: InspirationItem[];
  tags: string[];
  loading: boolean;
  error: string | null;
  searchQuery: string;
  activeTag: string | null;
  favoriteOnly: boolean;

  fetchInspirations: () => Promise<void>;
  fetchTags: () => Promise<void>;
  createInspiration: (data: {
    icon?: string;
    title: string;
    content: string;
    sourceAgent?: string;
    tags?: string[];
  }) => Promise<InspirationItem | null>;
  updateInspiration: (data: {
    id: string;
    icon?: string;
    title?: string;
    content?: string;
    sourceAgent?: string;
    isFavorite?: boolean;
    tags?: string[];
  }) => Promise<void>;
  deleteInspiration: (id: string) => Promise<void>;
  searchInspirations: (query: string) => Promise<void>;
  toggleFavorite: (id: string) => Promise<void>;
  setActiveTag: (tag: string | null) => void;
  setFavoriteOnly: (v: boolean) => void;
  setSearchQuery: (q: string) => void;
}

export const useInspirationStore = create<InspirationState>((set, get) => ({
  inspirations: [],
  tags: [],
  loading: false,
  error: null,
  searchQuery: '',
  activeTag: null,
  favoriteOnly: false,

  fetchInspirations: async () => {
    set({ loading: true, error: null });
    try {
      const { activeTag, favoriteOnly } = get();
      const inspirations = await listItems<InspirationItem>('list_inspirations', {
        tag: activeTag,
        favoriteOnly,
      });
      set({ inspirations, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  fetchTags: async () => {
    try {
      const tags = await listItems<string>('list_tags');
      set({ tags });
    } catch { /* silent */ }
  },

  createInspiration: async (data) => {
    try {
      const result = await saveItem<InspirationItem>('create_inspiration', data);
      await get().fetchInspirations();
      await get().fetchTags();
      return result;
    } catch (err) {
      set({ error: String(err) });
      return null;
    }
  },

  updateInspiration: async (data) => {
    try {
      await invokeAction('update_inspiration', { payload: data });
      await get().fetchInspirations();
      await get().fetchTags();
    } catch (err) {
      set({ error: String(err) });
    }
  },

  deleteInspiration: async (id) => {
    try {
      await deleteItem('delete_inspiration', id);
      await get().fetchInspirations();
    } catch (err) {
      set({ error: String(err) });
    }
  },

  searchInspirations: async (query) => {
    set({ loading: true, error: null });
    try {
      if (!query.trim()) {
        await get().fetchInspirations();
        return;
      }
      const results = await listItems<InspirationItem>('search_inspirations', {
        query: query.trim(),
        limit: 50,
      });
      set({ inspirations: results, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  toggleFavorite: async (id) => {
    const insp = get().inspirations.find((i) => i.id === id);
    if (!insp) return;
    await get().updateInspiration({ id, isFavorite: !insp.isFavorite });
  },

  setActiveTag: (tag) => {
    set({ activeTag: tag });
    get().fetchInspirations();
  },

  setFavoriteOnly: (v) => {
    set({ favoriteOnly: v });
    get().fetchInspirations();
  },

  setSearchQuery: (q) => {
    set({ searchQuery: q });
  },
}));
