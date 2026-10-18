/**
 * enclaveKbStore — 飞地组织知识库（数据在主机侧 enclave.db）。
 *
 * 条目由「任务汇报确认」自动沉淀；本 store 只负责浏览与检索（成员可读）。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage } from '../utils/errorMessage';
import type { KbEntryView, KbView } from '../types/enclave';

interface EnclaveKbState {
  kbs: KbView[];
  activeKbId: string;
  entries: KbEntryView[];
  activeEntry: KbEntryView | null;
  query: string;
  loading: boolean;
  error: string | null;
  /** 账号切换 / 退出：清空全部知识库缓存 */
  reset: () => void;
  loadKbs: (orgId: string) => Promise<void>;
  selectKb: (orgId: string, kbId: string) => Promise<void>;
  search: (orgId: string, query: string) => Promise<void>;
  openEntry: (entry: KbEntryView | null) => void;
  /** 人工编辑条目（owner / admin） */
  updateEntry: (
    orgId: string,
    kbId: string,
    memoryId: string,
    input: { title: string; content: string; tags: string },
  ) => Promise<boolean>;
  /** 移除条目（owner / admin）；交付物原件仍保留在任务中 */
  removeEntry: (orgId: string, kbId: string, memoryId: string) => Promise<boolean>;
}

export const useEnclaveKbStore = create<EnclaveKbState>((set, get) => ({
  kbs: [],
  activeKbId: '',
  entries: [],
  activeEntry: null,
  query: '',
  loading: false,
  error: null,

  reset: () => {
    set({
      kbs: [],
      activeKbId: '',
      entries: [],
      activeEntry: null,
      query: '',
      loading: false,
      error: null,
    });
  },

  loadKbs: async (orgId) => {
    set({ loading: true, error: null });
    try {
      const kbs = await invoke<KbView[]>('enclave_kb_list', { orgId });
      const next = kbs.some((k) => k.id === get().activeKbId)
        ? get().activeKbId
        : kbs[0]?.id ?? '';
      set({ kbs, activeKbId: next, loading: false });
      if (next) await get().selectKb(orgId, next);
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  selectKb: async (orgId, kbId) => {
    set({ activeKbId: kbId, activeEntry: null });
    await get().search(orgId, get().query);
  },

  search: async (orgId, query) => {
    if (!get().activeKbId) return;
    set({ loading: true, query });
    try {
      const entries = await invoke<KbEntryView[]>('enclave_kb_search', {
        orgId,
        kbId: get().activeKbId,
        query,
        limit: 100,
      });
      set({ entries, loading: false, error: null });
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  openEntry: (entry) => set({ activeEntry: entry }),

  updateEntry: async (orgId, kbId, memoryId, input) => {
    set({ loading: true, error: null });
    try {
      const next = await invoke<KbEntryView>('enclave_kb_entry_update', {
        orgId,
        kbId,
        memoryId,
        title: input.title,
        content: input.content,
        tags: input.tags,
      });
      set((s) => ({
        entries: s.entries.map((e) => (e.memoryId === memoryId ? next : e)),
        activeEntry: s.activeEntry?.memoryId === memoryId ? next : s.activeEntry,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
      return false;
    }
  },

  removeEntry: async (orgId, kbId, memoryId) => {
    set({ error: null });
    try {
      await invoke('enclave_kb_entry_remove', { orgId, kbId, memoryId });
      set((s) => ({
        entries: s.entries.filter((e) => e.memoryId !== memoryId),
        activeEntry: s.activeEntry?.memoryId === memoryId ? null : s.activeEntry,
        kbs: s.kbs.map((k) =>
          k.id === kbId ? { ...k, entryCount: Math.max(0, k.entryCount - 1) } : k,
        ),
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },
}));
