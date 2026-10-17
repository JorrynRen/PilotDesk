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
  loadKbs: (orgId: string) => Promise<void>;
  selectKb: (orgId: string, kbId: string) => Promise<void>;
  search: (orgId: string, query: string) => Promise<void>;
  openEntry: (entry: KbEntryView | null) => void;
}

export const useEnclaveKbStore = create<EnclaveKbState>((set, get) => ({
  kbs: [],
  activeKbId: '',
  entries: [],
  activeEntry: null,
  query: '',
  loading: false,
  error: null,

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
}));
