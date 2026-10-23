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
  KbCandidateView,
  KbEntryView,
  KbFileListView,
  KbOrphanFile,
  KbView,
  TeamIngestCapability,
  TeamIngestOutcome,
  TeamIngestPreview,
  TeamQueueRunOutcome,
} from '../types/enclave';

/** 图谱分页：单批 200 是后端 `search_entries` 的硬上限（clamp 到 200） */
const GRAPH_PAGE = 200;
/** 进入图谱时自动续拉到的上限；再往后由用户点「加载更多」逐批追加 */
const GRAPH_AUTO_MAX = 1000;

/** 拉图谱的一页（不带状态，首屏自动续拉与手动加载更多共用） */
function fetchGraphPage(orgId: string, kbId: string, offset: number): Promise<KbEntryView[]> {
  return invoke<KbEntryView[]>('enclave_kb_search', {
    orgId,
    kbId,
    query: '',
    limit: GRAPH_PAGE,
    offset,
  });
}

interface EnclaveKbState {
  kbs: KbView[];
  activeKbId: string;
  entries: KbEntryView[];
  activeEntry: KbEntryView | null;
  /** 「文件」页：去重后的交付物 */
  files: KbFileListView[];
  /** 无引用文件（条目已移除，原件仍在磁盘）：仅用于管理清理 */
  orphans: KbOrphanFile[];
  query: string;
  loading: boolean;
  error: string | null;
  /** 团队库加工能力（断网 / 闸门关闭时 canProcess = none，但仍可入库待整理） */
  capability: TeamIngestCapability | null;
  /** 当前库的待整理队列 */
  queue: EnclaveKbQueueItem[];
  /** 当前库的待确认候选（模型产出） */
  candidates: KbCandidateView[];
  /** 图谱专用条目（分页拉取，不覆盖列表/搜索状态） */
  graphEntries: KbEntryView[];
  /** 图谱是否还有更早的条目没拉（把这一批拉满了就说明可能还有） */
  graphHasMore: boolean;
  /** 图谱正在加载（首次自动续拉 / 手动加载更多共用） */
  graphLoading: boolean;
  /** 账号切换 / 退出：清空全部知识库缓存 */
  reset: () => void;
  loadKbs: (orgId: string) => Promise<void>;
  selectKb: (orgId: string, kbId: string) => Promise<void>;
  search: (orgId: string, query: string) => Promise<void>;
  /** 图谱：进页面自动续拉到 1000 条（每批 200，后端单批上限），失败只记错误、不影响列表 */
  loadGraphEntries: (orgId: string, kbId: string) => Promise<void>;
  /** 图谱：手动再拉一批（200 条），上限与个人库一致（渲染层安全阀） */
  loadMoreGraphEntries: (orgId: string, kbId: string) => Promise<void>;
  loadFiles: (orgId: string, kbId: string) => Promise<void>;
  /** 解除条目与一份原件的关联（owner / admin）；返回错误文案，成功为 null */
  detachEntryFile: (
    orgId: string,
    kbId: string,
    memoryId: string,
    fileId: string,
  ) => Promise<string | null>;
  /** 删除原件（owner / admin；仍有引用时后端拒绝）；返回错误文案，成功为 null */
  deleteFile: (orgId: string, fileId: string) => Promise<string | null>;
  /** 批量清理无引用文件（owner / admin）；返回删除数量 */
  cleanupOrphans: (orgId: string) => Promise<number>;
  openEntry: (entry: KbEntryView | null) => void;
  /** 人工维护知识库信息（owner / admin） */
  updateKb: (
    orgId: string,
    kbId: string,
    name: string,
    description: string,
    fieldSchema: string,
  ) => Promise<boolean>;
  /** 新建自定义库（默认库由系统自动创建） */
  createKb: (
    orgId: string,
    name: string,
    description: string,
    fieldSchema: string,
  ) => Promise<boolean>;
  /** 删除自定义库（默认库与有内容的库会被后端拒绝） */
  deleteKb: (orgId: string, kbId: string) => Promise<boolean>;
  /** 人工编辑条目（owner / admin）；`fields` 是专属属性 JSON 字符串 */
  updateEntry: (
    orgId: string,
    kbId: string,
    memoryId: string,
    input: { title: string; content: string; tags: string; fields: string },
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
  /** 手工整理第一步：提取原文回填表单（不写库；失败时抛错，由调用方降级为空白表单） */
  previewManualIngest: (orgId: string, id: string) => Promise<TeamIngestPreview>;
  /** 手工整理第二步：确认（可改过）标题/正文/标签/专属属性后入库 */
  commitManualIngest: (
    orgId: string,
    id: string,
    title: string,
    content: string,
    tags: string,
    fields: string,
  ) => Promise<void>;
  /** 文件投喂（主机本机直读；成员经主机上传通道） */
  ingestFile: (
    orgId: string,
    kbId: string,
    path: string,
  ) => Promise<TeamIngestOutcome | null>;
  /** AI 生成草稿（落待确认） */
  generate: (orgId: string, kbId: string, topic: string) => Promise<boolean>;
  loadCandidates: (orgId: string, kbId: string) => Promise<void>;
  adoptCandidate: (orgId: string, kbId: string, id: string) => Promise<boolean>;
  rejectCandidate: (orgId: string, kbId: string, id: string) => Promise<void>;
}

export const useEnclaveKbStore = create<EnclaveKbState>((set, get) => ({
  kbs: [],
  activeKbId: '',
  entries: [],
  activeEntry: null,
  files: [],
  orphans: [],
  query: '',
  loading: false,
  error: null,
  capability: null,
  queue: [],
  candidates: [],
  graphEntries: [],
  graphHasMore: false,
  graphLoading: false,

  reset: () => {
    set({
      kbs: [],
      activeKbId: '',
      entries: [],
      activeEntry: null,
      files: [],
      orphans: [],
      query: '',
      loading: false,
      error: null,
      capability: null,
      queue: [],
      candidates: [],
      graphEntries: [],
      graphHasMore: false,
      graphLoading: false,
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
    set({
      activeKbId: kbId,
      activeEntry: null,
      files: [],
      orphans: [],
      graphEntries: [],
      graphHasMore: false,
      graphLoading: false,
    });
    await get().search(orgId, get().query);
    void get().loadQueue(orgId, kbId);
    void get().loadCandidates(orgId, kbId);
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

  loadGraphEntries: async (orgId, kbId) => {
    set({ graphLoading: true, graphEntries: [], graphHasMore: false });
    try {
      const all: KbEntryView[] = [];
      let hasMore = false;
      // 自动续拉到 GRAPH_AUTO_MAX：单批 200 是后端上限，所以只能分批
      while (all.length < GRAPH_AUTO_MAX) {
        const page = await fetchGraphPage(orgId, kbId, all.length);
        all.push(...page);
        hasMore = page.length === GRAPH_PAGE;
        if (!hasMore) break;
      }
      // 切库 / 重复进入时，晚到的旧响应不能盖掉新库的数据
      if (get().activeKbId !== kbId) return;
      set({ graphEntries: all, graphHasMore: hasMore, graphLoading: false, error: null });
    } catch (e) {
      if (get().activeKbId !== kbId) return;
      set({
        error: errorMessage(e),
        graphEntries: [],
        graphHasMore: false,
        graphLoading: false,
      });
    }
  },

  loadMoreGraphEntries: async (orgId, kbId) => {
    if (get().graphLoading) return;
    set({ graphLoading: true });
    try {
      const page = await fetchGraphPage(orgId, kbId, get().graphEntries.length);
      if (get().activeKbId !== kbId) return;
      set((s) => ({
        graphEntries: [...s.graphEntries, ...page],
        graphHasMore: page.length === GRAPH_PAGE,
        graphLoading: false,
        error: null,
      }));
    } catch (e) {
      set({ error: errorMessage(e), graphLoading: false });
    }
  },

  loadFiles: async (orgId, kbId) => {
    set({ loading: true, error: null });
    try {
      const files = await invoke<KbFileListView[]>('enclave_kb_file_list', { orgId, kbId });
      // 无引用文件是管理信息：拿不到（旧版主机 / 权限不足）不影响文件列表本身
      let orphans: KbOrphanFile[] = [];
      try {
        orphans = await invoke<KbOrphanFile[]>('enclave_kb_orphan_files', { orgId });
      } catch {
        orphans = [];
      }
      set({ files, orphans, loading: false });
      // 文件页要提示「待整理」的文件投喂（它们还没有条目，但不是无引用冗余）
      void get().loadQueue(orgId, kbId);
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  /** 解除条目与一份原件的关联（owner / admin）；返回错误文案，成功为 null */
  detachEntryFile: async (orgId, kbId, memoryId, fileId) => {
    try {
      await invoke('enclave_kb_entry_file_detach', { orgId, kbId, memoryId, fileId });
      await get().search(orgId, get().query);
      await get().loadFiles(orgId, kbId);
      return null;
    } catch (e) {
      return errorMessage(e);
    }
  },

  /** 删除原件（owner / admin；仍有引用时后端拒绝）；返回错误文案，成功为 null */
  deleteFile: async (orgId, fileId) => {
    try {
      await invoke('enclave_kb_file_delete', { orgId, fileId });
      const kbId = get().activeKbId;
      if (kbId) await get().loadFiles(orgId, kbId);
      return null;
    } catch (e) {
      return errorMessage(e);
    }
  },

  /** 批量清理无引用文件（owner / admin）；返回删除数量 */
  cleanupOrphans: async (orgId) => {
    try {
      const n = await invoke<number>('enclave_kb_orphan_cleanup', { orgId });
      const kbId = get().activeKbId;
      if (kbId) await get().loadFiles(orgId, kbId);
      return n;
    } catch (e) {
      set({ error: errorMessage(e) });
      return 0;
    }
  },

  openEntry: (entry) => set({ activeEntry: entry }),

  updateKb: async (orgId, kbId, name, description, fieldSchema) => {
    set({ loading: true, error: null });
    try {
      const next = await invoke<KbView>('enclave_kb_update', {
        orgId,
        kbId,
        name,
        description,
        fieldSchema,
      });
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

  createKb: async (orgId, name, description, fieldSchema) => {
    set({ loading: true, error: null });
    try {
      const created = await invoke<KbView>('enclave_kb_create', {
        orgId,
        name,
        description,
        fieldSchema,
      });
      const kbs = await invoke<KbView[]>('enclave_kb_list', { orgId });
      set({ kbs, loading: false });
      // 建完直接切过去，用户马上能配置/投喂
      await get().selectKb(orgId, created.id);
      return true;
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
      return false;
    }
  },

  deleteKb: async (orgId, kbId) => {
    set({ loading: true, error: null });
    try {
      await invoke('enclave_kb_delete', { orgId, kbId });
      const kbs = await invoke<KbView[]>('enclave_kb_list', { orgId });
      const next = kbs[0]?.id ?? '';
      set({ kbs, loading: false });
      if (get().activeKbId === kbId) await get().selectKb(orgId, next);
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
        fields: input.fields,
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

  previewManualIngest: async (orgId, id) =>
    invoke<TeamIngestPreview>('enclave_kb_ingest_preview', { orgId, queueId: id }),

  commitManualIngest: async (orgId, id, title, content, tags, fields) => {
    await invoke('enclave_kb_ingest_commit', {
      orgId,
      queueId: id,
      title,
      content,
      tags,
      fields,
    });
    const kbId = get().activeKbId;
    if (kbId) await get().loadQueue(orgId, kbId);
    void get().loadCapability(orgId);
    await get().search(orgId, get().query);
  },

  ingestFile: async (orgId, kbId, path) => {
    try {
      const out = await invoke<TeamIngestOutcome>('enclave_kb_ingest_file', {
        orgId,
        kbId,
        path,
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

  generate: async (orgId, kbId, topic) => {
    try {
      await invoke<KbCandidateView>('enclave_kb_generate', { orgId, kbId, topic });
      await get().loadCandidates(orgId, kbId);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  loadCandidates: async (orgId, kbId) => {
    try {
      const candidates = await invoke<KbCandidateView[]>('enclave_kb_candidate_list', {
        orgId,
        kbId,
      });
      set({ candidates });
    } catch {
      // 静默：候选面板为空即可
    }
  },

  adoptCandidate: async (orgId, kbId, id) => {
    try {
      await invoke('enclave_kb_candidate_adopt', { orgId, kbId, candidateId: id });
      await get().loadCandidates(orgId, kbId);
      await get().search(orgId, get().query);
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  rejectCandidate: async (orgId, kbId, id) => {
    try {
      await invoke('enclave_kb_candidate_reject', { orgId, kbId, candidateId: id });
      await get().loadCandidates(orgId, kbId);
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },
}));
