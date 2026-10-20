/**
 * enclaveKbStore — 飞地组织知识库（数据在主机侧 enclave.db）。
 *
 * 条目由「任务汇报确认」自动沉淀；本 store 只负责浏览与检索（成员可读）。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage } from '../utils/errorMessage';
import type {
  EnclaveKbQueueItem,
  KbEntryView,
  KbFileListView,
  KbView,
  TeamIngestCapability,
  TeamIngestOutcome,
  TeamQueueRunOutcome,
} from '../types/enclave';

interface EnclaveKbState {
  kbs: KbView[];
  activeKbId: string;
  entries: KbEntryView[];
  activeEntry: KbEntryView | null;
  /** 「文件」页：去重后的交付物 */
  files: KbFileListView[];
  query: string;
  loading: boolean;
  error: string | null;
  /** 团队库加工能力（断网 / 闸门关闭时 canProcess = none，但仍可入库待整理） */
  capability: TeamIngestCapability | null;
  /** 当前库的待整理队列 */
  queue: EnclaveKbQueueItem[];
  /** 账号切换 / 退出：清空全部知识库缓存 */
  reset: () => void;
  loadKbs: (orgId: string) => Promise<void>;
  selectKb: (orgId: string, kbId: string) => Promise<void>;
  search: (orgId: string, query: string) => Promise<void>;
  loadFiles: (orgId: string, kbId: string) => Promise<void>;
  openEntry: (entry: KbEntryView | null) => void;
  /** 人工维护知识库信息（owner / admin） */
  updateKb: (orgId: string, kbId: string, name: string, description: string) => Promise<boolean>;
  /** 人工编辑条目（owner / admin） */
  updateEntry: (
    orgId: string,
    kbId: string,
    memoryId: string,
    input: { title: string; content: string; tags: string },
  ) => Promise<boolean>;
  /** 移除条目（owner / admin）；交付物原件仍保留在任务中 */
  removeEntry: (orgId: string, kbId: string, memoryId: string) => Promise<boolean>;
  /** 读取加工能力 */
  loadCapability: (orgId: string) => Promise<void>;
  /** 读取当前库的待整理队列 */
  loadQueue: (orgId: string, kbId: string) => Promise<void>;
  /** 片段投喂（内容立即入库/入队，整理按能力决定） */
  ingestSnippet: (
    orgId: string,
    kbId: string,
    title: string,
    text: string,
  ) => Promise<TeamIngestOutcome | null>;
  ingestUrl: (
    orgId: string,
    kbId: string,
    url: string,
    name?: string,
  ) => Promise<TeamIngestOutcome | null>;
  runQueue: (orgId: string) => Promise<TeamQueueRunOutcome | null>;
  retryQueueItem: (orgId: string, id: string) => Promise<void>;
  dropQueueItem: (orgId: string, id: string) => Promise<void>;
  /** 手工整理（仅片段）：不经模型，按原文直接写入组织库 */
  manualIngest: (orgId: string, id: string) => Promise<void>;
}

export const useEnclaveKbStore = create<EnclaveKbState>((set, get) => ({
  kbs: [],
  activeKbId: '',
  entries: [],
  activeEntry: null,
  files: [],
  query: '',
  loading: false,
  error: null,
  capability: null,
  queue: [],

  reset: () => {
    set({
      kbs: [],
      activeKbId: '',
      entries: [],
      activeEntry: null,
      files: [],
      query: '',
      loading: false,
      error: null,
      capability: null,
      queue: [],
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
    set({ activeKbId: kbId, activeEntry: null, files: [] });
    await get().search(orgId, get().query);
    void get().loadQueue(orgId, kbId);
    void get().loadCapability(orgId);
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
      set((s) => ({
        entries,
        // 展开行跟随最新数据（检索 / 刷新后不能继续展示旧对象）
        activeEntry: s.activeEntry
          ? entries.find((e) => e.memoryId === s.activeEntry?.memoryId) ?? null
          : null,
        loading: false,
        error: null,
      }));
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  loadFiles: async (orgId, kbId) => {
    set({ loading: true, error: null });
    try {
      const files = await invoke<KbFileListView[]>('enclave_kb_file_list', { orgId, kbId });
      set({ files, loading: false });
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  openEntry: (entry) => set({ activeEntry: entry }),

  updateKb: async (orgId, kbId, name, description) => {
    set({ loading: true, error: null });
    try {
      const next = await invoke<KbView>('enclave_kb_update', { orgId, kbId, name, description });
      set((s) => ({
        kbs: s.kbs.map((k) => (k.id === kbId ? next : k)),
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
      return false;
    }
  },

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

  loadCapability: async (orgId) => {
    try {
      const capability = await invoke<TeamIngestCapability>('enclave_kb_ingest_capability', {
        orgId,
      });
      set({ capability });
    } catch {
      // 能力探测失败不影响投喂：界面退回"未知"，提交仍会入库待整理
    }
  },

  loadQueue: async (orgId, kbId) => {
    try {
      const queue = await invoke<EnclaveKbQueueItem[]>('enclave_kb_queue_list', {
        orgId,
        kbId,
      });
      set({ queue });
    } catch {
      // 静默：队列面板为空即可
    }
  },

  ingestSnippet: async (orgId, kbId, title, text) => {
    try {
      const out = await invoke<TeamIngestOutcome>('enclave_kb_ingest_snippet', {
        orgId,
        kbId,
        title,
        text,
      });
      await get().loadQueue(orgId, kbId);
      void get().loadCapability(orgId);
      if (!out.queued) await get().search(orgId, get().query);
      return out;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  ingestUrl: async (orgId, kbId, url, name) => {
    try {
      const out = await invoke<TeamIngestOutcome>('enclave_kb_ingest_url', {
        orgId,
        kbId,
        url,
        name: name ?? null,
      });
      await get().loadQueue(orgId, kbId);
      void get().loadCapability(orgId);
      if (!out.queued) await get().search(orgId, get().query);
      return out;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  runQueue: async (orgId) => {
    try {
      const out = await invoke<TeamQueueRunOutcome>('enclave_kb_queue_run', { orgId });
      const kbId = get().activeKbId;
      if (kbId) await get().loadQueue(orgId, kbId);
      void get().loadCapability(orgId);
      if (out.done > 0) await get().search(orgId, get().query);
      return out;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  retryQueueItem: async (orgId, id) => {
    try {
      await invoke('enclave_kb_queue_retry', { orgId, queueId: id });
      const kbId = get().activeKbId;
      if (kbId) await get().loadQueue(orgId, kbId);
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },

  dropQueueItem: async (orgId, id) => {
    try {
      await invoke('enclave_kb_queue_drop', { orgId, queueId: id });
      const kbId = get().activeKbId;
      if (kbId) await get().loadQueue(orgId, kbId);
      void get().loadCapability(orgId);
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },

  manualIngest: async (orgId, id) => {
    try {
      await invoke('enclave_kb_ingest_manual', { orgId, queueId: id });
      const kbId = get().activeKbId;
      if (kbId) await get().loadQueue(orgId, kbId);
      void get().loadCapability(orgId);
      await get().search(orgId, get().query);
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },
}));
