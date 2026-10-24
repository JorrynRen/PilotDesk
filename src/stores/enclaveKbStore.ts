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
  KbFileRelationView,
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
/** 条目列表每批条数（后端上限 200；100 条一屏足够，其余按需追加） */
const ENTRY_PAGE = 100;
/** 文件列表（文件 × 条目关联行）每批条数 */
const FILE_PAGE = 200;

/** 「全部库」聚合视图的伪库 id：只读跨库检索，与后端 `ALL_KB_PARAM` 一致 */
export const ALL_KB_ID = '__all__';

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
  /** 条目列表是否还有更早的条目没拉（本批拉满就说明可能还有） */
  entriesHasMore: boolean;
  entriesLoadingMore: boolean;
  activeEntry: KbEntryView | null;
  /** 「文件」页：去重后的交付物 */
  files: KbFileListView[];
  /** 文件之间的标注（正文—附件 / 新版—旧版）：只在当前库内成立 */
  relations: KbFileRelationView[];
  filesHasMore: boolean;
  filesLoadingMore: boolean;
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
  /** 条目列表：按当前检索词再拉一批（offset = 已加载条数） */
  loadMoreEntries: (orgId: string) => Promise<void>;
  /** 图谱：进页面自动续拉到 1000 条（每批 200，后端单批上限），失败只记错误、不影响列表 */
  loadGraphEntries: (orgId: string, kbId: string) => Promise<void>;
  /** 图谱：手动再拉一批（200 条），上限与个人库一致（渲染层安全阀） */
  loadMoreGraphEntries: (orgId: string, kbId: string) => Promise<void>;
  loadFiles: (orgId: string, kbId: string) => Promise<void>;
  /** 文件列表：再拉一批（offset = 已加载行数） */
  loadMoreFiles: (orgId: string) => Promise<void>;
  /** 文件关系选择器：把文件补满到上限，避免下拉里只有已加载的前几页 */
  ensureAllFiles: (orgId: string) => Promise<void>;
  /** 建立文件关系（owner / admin）；返回错误文案，成功为 null */
  linkFileRelation: (
    orgId: string,
    kbId: string,
    kind: KbFileRelationView['kind'],
    from: string,
    to: string,
  ) => Promise<string | null>;
  /** 解除文件关系（owner / admin）；返回错误文案，成功为 null */
  unlinkFileRelation: (
    orgId: string,
    kbId: string,
    kind: KbFileRelationView['kind'],
    from: string,
    to: string,
  ) => Promise<string | null>;
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
  /** 采纳候选：`patch` 是人工修正后的标题 / 正文 / 标签 / 专属属性（fields 为 JSON 对象字符串） */
  adoptCandidate: (
    orgId: string,
    kbId: string,
    id: string,
    patch: { title: string; content: string; tags: string; fields: string },
  ) => Promise<boolean>;
  rejectCandidate: (orgId: string, kbId: string, id: string) => Promise<void>;
}

export const useEnclaveKbStore = create<EnclaveKbState>((set, get) => ({
  kbs: [],
  activeKbId: '',
  entries: [],
  entriesHasMore: false,
  entriesLoadingMore: false,
  activeEntry: null,
  files: [],
  relations: [],
  filesHasMore: false,
  filesLoadingMore: false,
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
      entriesHasMore: false,
      entriesLoadingMore: false,
      activeEntry: null,
      files: [],
      relations: [],
      filesHasMore: false,
      filesLoadingMore: false,
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
      const cur = get().activeKbId;
      // 「全部库」不是真实库，刷新时要保留这个选择
      const next =
        cur === ALL_KB_ID || kbs.some((k) => k.id === cur) ? cur : kbs[0]?.id ?? '';
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
      relations: [],
      filesHasMore: false,
      filesLoadingMore: false,
      orphans: [],
      graphEntries: [],
      graphHasMore: false,
      graphLoading: false,
    });
    await get().search(orgId, get().query);
    // 待整理 / 待确认 / 加工能力都是「按库」的；聚合视图只读，不加载
    if (kbId !== ALL_KB_ID) {
      void get().loadQueue(orgId, kbId);
      void get().loadCandidates(orgId, kbId);
      void get().loadCapability(orgId);
    }
  },

  search: async (orgId, query) => {
    if (!get().activeKbId) return;
    set({ loading: true, query });
    try {
      const entries = await invoke<KbEntryView[]>('enclave_kb_search', {
        orgId,
        kbId: get().activeKbId,
        query,
        limit: ENTRY_PAGE,
        offset: 0,
      });
      set((s) => ({
        entries,
        entriesHasMore: entries.length === ENTRY_PAGE,
        entriesLoadingMore: false,
        // 展开行跟随最新数据（检索 / 刷新后不能继续展示旧对象）；
        // 聚合视图下 memory_id 可能跨库重复，按「库 + 条目」匹配
        activeEntry: s.activeEntry
          ? entries.find(
              (e) =>
                e.memoryId === s.activeEntry?.memoryId && e.kbId === s.activeEntry?.kbId,
            ) ?? null
          : null,
        loading: false,
        error: null,
      }));
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  loadMoreEntries: async (orgId) => {
    const { activeKbId, query, entries, entriesHasMore, entriesLoadingMore } = get();
    if (!activeKbId || !entriesHasMore || entriesLoadingMore) return;
    set({ entriesLoadingMore: true });
    try {
      const page = await invoke<KbEntryView[]>('enclave_kb_search', {
        orgId,
        kbId: activeKbId,
        query,
        limit: ENTRY_PAGE,
        offset: entries.length,
      });
      set((s) => ({
        entries: [...s.entries, ...page],
        entriesHasMore: page.length === ENTRY_PAGE,
        entriesLoadingMore: false,
        error: null,
      }));
    } catch (e) {
      set({ error: errorMessage(e), entriesLoadingMore: false });
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
      // 同一库内刷新（解除关联 / 重试归档后）保留已加载页数，避免跳回第一页
      const limit = Math.max(FILE_PAGE, get().files.length);
      const files = await invoke<KbFileListView[]>('enclave_kb_file_list', {
        orgId,
        kbId,
        limit,
        offset: 0,
      });
      // 无引用文件是管理信息：拿不到（旧版主机 / 权限不足）不影响文件列表本身
      let orphans: KbOrphanFile[] = [];
      try {
        orphans = await invoke<KbOrphanFile[]>('enclave_kb_orphan_files', { orgId });
      } catch {
        orphans = [];
      }
      // 文件之间的关系同为增强信息：旧版主机没有这条命令时降级为空，不影响列表
      let relations: KbFileRelationView[] = [];
      try {
        relations = await invoke<KbFileRelationView[]>('enclave_kb_file_relations', {
          orgId,
          kbId,
        });
      } catch {
        relations = [];
      }
      set({
        files,
        relations,
        filesHasMore: files.length === limit,
        filesLoadingMore: false,
        orphans,
        loading: false,
      });
      // 文件页要提示「待整理」的文件投喂（它们还没有条目，但不是无引用冗余）
      void get().loadQueue(orgId, kbId);
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  loadMoreFiles: async (orgId) => {
    const { activeKbId, files, filesHasMore, filesLoadingMore } = get();
    if (!activeKbId || !filesHasMore || filesLoadingMore) return;
    set({ filesLoadingMore: true });
    try {
      const page = await invoke<KbFileListView[]>('enclave_kb_file_list', {
        orgId,
        kbId: activeKbId,
        limit: FILE_PAGE,
        offset: files.length,
      });
      set((s) => ({
        files: [...s.files, ...page],
        filesHasMore: page.length === FILE_PAGE,
        filesLoadingMore: false,
      }));
    } catch (e) {
      set({ filesLoadingMore: false, error: errorMessage(e) });
    }
  },

  ensureAllFiles: async (orgId) => {
    // 关系选择器要能搜到整库文件：打开面板时把分页补满（与文件页同一上限口径）
    while (get().filesHasMore && !get().filesLoadingMore && get().files.length < 5000) {
      const before = get().files.length;
      await get().loadMoreFiles(orgId);
      // 出错时 loadMoreFiles 不追加数据，避免死循环
      if (get().files.length === before) break;
    }
  },

  /** 建立文件关系（owner / admin）：attachment = 正文→附件，supersedes = 新版→旧版 */
  linkFileRelation: async (orgId, kbId, kind, from, to) => {
    try {
      await invoke('enclave_kb_file_link', { orgId, kbId, kind, fromFileId: from, toFileId: to });
      const relations = await invoke<KbFileRelationView[]>('enclave_kb_file_relations', {
        orgId,
        kbId,
      });
      set({ relations });
      return null;
    } catch (e) {
      return errorMessage(e);
    }
  },

  /** 解除文件关系（owner / admin） */
  unlinkFileRelation: async (orgId, kbId, kind, from, to) => {
    try {
      await invoke('enclave_kb_file_unlink', {
        orgId,
        kbId,
        kind,
        fromFileId: from,
        toFileId: to,
      });
      const relations = await invoke<KbFileRelationView[]>('enclave_kb_file_relations', {
        orgId,
        kbId,
      });
      set({ relations });
      return null;
    } catch (e) {
      return errorMessage(e);
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

  adoptCandidate: async (orgId, kbId, id, patch) => {
    try {
      await invoke('enclave_kb_candidate_adopt', {
        orgId,
        kbId,
        candidateId: id,
        title: patch.title,
        content: patch.content,
        tags: patch.tags,
        fields: patch.fields,
      });
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
