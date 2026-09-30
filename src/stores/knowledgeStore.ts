/**
 * knowledgeStore — 知识库状态（真实数据，来自 MEMORY.db 的 kb_* 命令）
 *
 * 只保留「当前选中库」的条目/文件/候选：切库时重新拉取，避免把整库都堆在内存里
 * （后端单次列表上限 500 条，够用且可预期）。所有写操作成功后统一 refresh，
 * 不在前端做计数/关联的本地推演 —— 多对多的关联真相只有库里有。
 */

import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../utils/toast';
import { errorMessage } from '../utils/errorMessage';
import {
  toBase,
  toCandidate,
  toEntry,
  toFile,
  toFileRelation,
  type KnowledgeBase,
  type KnowledgeBaseView,
  type KnowledgeCandidate,
  type KnowledgeCandidateView,
  type KnowledgeEdgeKind,
  type KnowledgeEntry,
  type KnowledgeEntryView,
  type KnowledgeFieldDef,
  type KnowledgeFile,
  type KnowledgeFileAttr,
  type KnowledgeFileRelation,
  type KnowledgeFileRelationKind,
  type KnowledgeFileRelationView,
  type KnowledgeFileView,
  type KnowledgeGraphNode,
  type KnowledgeMeta,
  type KnowledgeRelation,
  type IngestOutcome,
} from '../types/knowledge';

export type KnowledgeTab = 'entries' | 'files' | 'ingest' | 'graph';

/** 新建库的 id 由前端生成（slug），后端只做唯一性校验 */
export function makeBaseId(name: string): string {
  const slug = name.trim().toLowerCase().replace(/[^a-z0-9\u4e00-\u9fa5]+/g, '-').replace(/^-|-$/g, '');
  return `kb-${slug || 'new'}`;
}

interface KnowledgeState {
  bases: KnowledgeBase[];
  entries: KnowledgeEntry[];
  files: KnowledgeFile[];
  /**
   * 文件级专属属性（按 `path` 索引）——由该文件的分块聚合而来（`kb_files` 上没有属性列）。
   * 文件视图「按属性分目录」用；没有分块的文件不会出现在这里（必然未分类）。
   */
  fileAttrs: Record<string, KnowledgeFileAttr>;
  /** 当前库内文件之间的关系（正文—附件 / 版本替代） */
  relations: KnowledgeFileRelation[];
  candidates: KnowledgeCandidate[];
  activeBaseId: string;
  activeTab: KnowledgeTab;
  /** 条目筛选条件：列表与图谱共用（图谱据此限流，见 deriveGraph） */
  entryFilters: EntryFilters;
  setEntryFilters: (patch: Partial<EntryFilters>) => void;
  loading: boolean;
  /**
   * 图谱「已展开文件」的片段：`relPath` → 该原文的片段条目。**键的集合就是"已展开"的集合**。
   *
   * 按需从 `kb_list_file_chunks` 拉取，不复用 `entries` —— 条目列表有整库 500 条的闸门，
   * 一个上千块的文件在前端根本拿不全（那正是"展开看不到全部片段"的根因）。
   */
  graphChunks: Record<string, KnowledgeEntry[]>;
  /** 正在拉取片段的原文路径（空 = 没有在拉） */
  graphChunkLoading: string;
  /**
   * 展开 / 收起一个文件节点的片段（**切换**语义：已展开的再调会收起）。
   * 返回是否处于展开态（被片段总量额度挡住时返回 false）。
   * 图谱现在只在"点文件聚焦"时展开它一个，走的是下面的 `expandGraphFiles`（幂等）；
   * `silent` 是为"一次要连开几个文件"的调用方准备的，避免逐个弹提示。
   */
  toggleGraphFile: (relPath: string, chunkCount: number, opts?: { silent?: boolean }) => Promise<boolean>;
  /**
   * 依次展开一批文件的分块。
   *
   * 与 `toggleGraphFile` 的分工：这个是**展开**语义、幂等；那个是**切换**语义。
   * 已展开的直接跳过（否则对已展开的调切换会把它收起来）；片段总量到顶就停下，
   * 返回"还有几个文件没展开"（0 = 全部展开完了）。
   */
  expandGraphFiles: (
    targets: { relPath: string; chunkCount: number }[],
    opts?: { silent?: boolean },
  ) => Promise<number>;
  /** 收起全部已展开的文件 */
  collapseGraphFiles: () => void;
  /**
   * 展开轮次的"代"号：`collapseGraphFiles` 把它 +1，正在跑的 `expandGraphFiles` 每轮比对，
   * 不一致就立刻收工。
   *
   * 为什么需要：展开是**逐个文件带间隔**做的（小库自动展开要跑两秒多）。用户在这个窗口里点
   * 「收起全部分块」，收起本身生效了，但那个循环还在后台一个一个往回加 —— 看起来就是"收起点了没反应"。
   * **任何"能取消的后台任务"都要有一个能被外部打断的令牌**，不能只靠"下一轮自己发现状态变了"。
   */
  expandEpoch: number;

  loadBases: () => Promise<void>;
  /** 关掉当前打开的知识库、回到默认页（只清"打开了哪个"，库列表与库本身都不动） */
  closeBase: () => void;
  selectBase: (id: string) => Promise<void>;
  /** 只重新拉取文件关系（改关系后不必整库刷新） */
  reloadRelations: () => Promise<void>;
  /** 建立文件关系：attachment = from(正文)→to(附件)；supersedes = from(新版)→to(旧版) */
  linkFiles: (kind: KnowledgeFileRelationKind, fromFileId: string, toFileId: string) => Promise<boolean>;
  /** 解除文件关系 */
  unlinkFiles: (kind: KnowledgeFileRelationKind, fromFileId: string, toFileId: string) => Promise<boolean>;
  setTab: (tab: KnowledgeTab) => void;
  createBase: (
    input: { name: string; description: string; fields: KnowledgeFieldDef[] },
    id: string,
  ) => Promise<boolean>;
  updateBase: (
    id: string,
    input: { name: string; description: string; fields: KnowledgeFieldDef[] },
  ) => Promise<void>;
  deleteBase: (id: string) => Promise<{ entries: number; files: number } | null>;
  /** 投喂：一条内容投到一个或多个库（后端按库各生成一条待确认候选） */
  addCandidates: (input: {
    key: string;
    value: string;
    baseIds: string[];
    origin: 'snippet' | 'work';
    sourceRef?: string;
  }) => Promise<void>;
  /** 把一段**带角色的对话**交给 AI 结构化整理成 0..N 条待确认候选（origin=work）。
   *
   *  与 `addCandidates` 的区别：那个存的是**对话原文**（噪声大），这个产出的是**提炼后的干净知识**
   *  （标题/正文/标签/专属属性由 AI 填，并带原文摘录可溯源）。`kbId` 是**单库** ——
   *  专属属性字段定义随库而异，一次整理只能对着一套字段填。
   *  返回：产出的候选条数；`null` = 失败（已提示）。0 是合法结果（这段对话没有可沉淀的知识）。 */
  digestConversation: (
    kbId: string,
    messages: { role: string; content: string }[],
  ) => Promise<number | null>;
  /** 投喂本地文件（多库逐个投：第一次落盘抽取，后续复用原文只补关联）。
   *
   *  `items[].name` 是「允许重命名」下用户为该文件填的名字（空 = 交给 AI 取）；
   *  开关关掉时这个名字会被忽略（界面上输入框此时是禁用的）。 */
  ingestFiles: (
    items: { path: string; name: string }[],
    baseIds: string[],
    opts: { asMarkdown: boolean; rename: boolean },
  ) => Promise<void>;
  /** 投喂网页：抓正文 → 落盘为 .md → 由 AI 整理成规范知识入库。`name` 是用户给这份原文起的名字 */
  ingestUrl: (url: string, baseIds: string[], name?: string) => Promise<void>;
  /** 投喂云文档（第一期：语雀）：逐篇取正文 → AI 整理 → 入库，来源记为 `cloud`。
   *
   *  `accountId` 指定用哪个已配置账号（账号可以有多个）；
   *  逐篇串行并留出间隔：文档列表接口不带正文，N 篇就是 N 次请求，并发打过去会撞平台限流。 */
  ingestCloudDocs: (
    items: { id: string; title: string }[],
    baseIds: string[],
    opts: { accountId: string; namespace: string },
  ) => Promise<void>;
  /** AI 生成：按主题产出一条草稿（进「待确认」，核对后才入库） */
  generate: (topic: string) => Promise<void>;
  /** 投喂进行中（片段 / 文件 / 网页 / 云文档 / AI 生成共用）。
   *  投喂会调模型、可能分批调用长文档，必须有可见反馈，否则用户以为没在执行。 */
  ingestBusy: { kind: 'snippet' | 'file' | 'url' | 'cloud' | 'ai'; label: string; step: string } | null;
  /** AI 整理进行中（文件行上的手动补整理）：界面据此显示进度 */
  enriching: { index: number; total: number; name: string } | null;
  /** 单个原文的 AI 补整理（投喂时 AI 未生效、或想重新打标时的补救入口） */
  enrichFile: (kbId: string, fileId: string) => Promise<void>;
  /** 从库中移除文件（只解关联，**不动库定义**）；返回删除统计 */
  removeFiles: (kbId: string, fileIds: string[]) => Promise<{ entries: number; files: number } | null>;
  adoptCandidate: (id: number, patch: { key: string; value: string; meta: KnowledgeMeta }) => Promise<void>;
  rejectCandidate: (id: number) => Promise<void>;
  /**
   * 保存条目的正文 / 标签 / 专属属性（一次调用、一个事务）。
   *
   * **三样必须一起传**：`kb_save_entry` 会把空标签归一化成空串（等于清空标签），
   * 所以"只改正文"时也必须把当前标签原样带上。
   * `origin` / `source_ref` 传 null —— 后端对空值保留原值，不会动这条知识的来源信息。
   */
  updateEntry: (
    key: string,
    patch: { value: string; tags: string[]; meta: KnowledgeMeta },
  ) => Promise<boolean>;
  removeFromBase: (key: string) => Promise<void>;
  /** 批量解除/删除（多选后一次处理）：一个事务内完成，与单条同一口径 */
  removeEntries: (keys: string[]) => Promise<void>;
}

export const useKnowledgeStore = create<KnowledgeState>((set, get) => ({
  bases: [],
  entries: [],
  files: [],
  fileAttrs: {},
  relations: [],
  candidates: [],
  activeBaseId: '',
  activeTab: 'entries',
  entryFilters: { query: '', origin: 'all', pinOnly: false, fields: {} },
  loading: false,
  graphChunks: {},
  graphChunkLoading: '',
  expandEpoch: 0,
  ingestBusy: null,
  enriching: null,

  loadBases: async () => {
    try {
      const views = await invoke<KnowledgeBaseView[]>('kb_list_bases');
      const bases = views.map(toBase);
      set((s) => ({
        bases,
        // 当前选中的库被删掉时**回到"没打开"**，而不是自动跳到第一个 ——
        // 自动打开会让人以为"知识库默认就是开着的"，也看不到那个默认页（涉密提示在那里）。
        activeBaseId: bases.some((b) => b.id === s.activeBaseId) ? s.activeBaseId : '',
      }));
      const { activeBaseId } = get();
      if (activeBaseId) await get().selectBase(activeBaseId);
      else set({ entries: [], files: [], fileAttrs: {}, candidates: [], relations: [], graphChunks: {}, graphChunkLoading: '' });
    } catch (e) {
      showToast(`读取知识库列表失败: ${errorMessage(e)}`, 'error');
    }
  },

  /** 关掉当前打开的知识库：回到默认页（**只清"打开了哪个"**，库列表与库本身都不动） */
  closeBase: () =>
    set((s) => ({
      activeBaseId: '',
      entries: [],
      files: [],
      fileAttrs: {},
      candidates: [],
      relations: [],
      graphChunks: {},
      graphChunkLoading: '',
      expandEpoch: s.expandEpoch + 1,
    })),

  selectBase: async (id) => {
    // 只重新拉数据时**不要**清展开态：这会在每次"保存条目 / 改专属属性"后把图谱的展开全收掉。
    // 只有真的换库（id 变了）才清 —— 片段是按库拉的，留着会张冠李戴。
    const same = get().activeBaseId === id;
    set({ activeBaseId: id, loading: true, ...(same ? {} : { graphChunks: {}, graphChunkLoading: '', expandEpoch: get().expandEpoch + 1 }) });
    try {
      const [entries, files, candidates, relations, fileAttrs] = await Promise.all([
        // 可选参数显式传全（null 而不是省略）：省略键在反序列化时是否等价于 None 取决于
        // 后端的 serde 默认值，不赌这个 —— 传 null 一定被解析成 None
        invoke<KnowledgeEntryView[]>('kb_list_entries', {
          kbId: id,
          query: null,
          origin: null,
          pinOnly: false,
          metaFilter: null,
        }),
        invoke<KnowledgeFileView[]>('kb_list_files', { kbId: id }),
        invoke<KnowledgeCandidateView[]>('kb_list_candidates', { kbId: id }),
        invoke<KnowledgeFileRelationView[]>('kb_list_file_relations', { kbId: id }),
        // 文件级属性（由分块聚合）：文件视图「按属性分目录」的数据源
        invoke<KnowledgeFileAttr[]>('kb_list_file_attrs', { kbId: id }),
      ]);
      set({
        entries: entries.map(toEntry),
        files: files.map(toFile),
        candidates: candidates.map(toCandidate),
        relations: relations.map(toFileRelation),
        fileAttrs: Object.fromEntries(fileAttrs.map((a) => [a.sourceRef, a])),
      });
    } catch (e) {
      showToast(`读取知识库内容失败: ${errorMessage(e)}`, 'error');
    } finally {
      set({ loading: false });
    }
  },

  setTab: (tab) => set({ activeTab: tab }),

  setEntryFilters: (patch) => set((s) => ({ entryFilters: { ...s.entryFilters, ...patch } })),

  toggleGraphFile: async (relPath, chunkCount, _opts) => {
    const { activeBaseId, graphChunks } = get();
    if (!activeBaseId || !relPath) return false;

    // 已展开 → 收起。刻意不留缓存：再展开就是一次本地 SQLite 读，比维护一份
    // "缓存与展开态谁是真相"的额外状态便宜得多。
    if (graphChunks[relPath]) {
      const next = { ...graphChunks };
      delete next[relPath];
      set({ graphChunks: next });
      return false;
    }

    // 抽不出正文的文件（无条目）没有可展开的东西，也不占额度
    if (chunkCount === 0) return false;

    set({ graphChunkLoading: relPath });
    try {
      // **不传 limit**：后端不设上限，一个文件有多少块就拿回多少块。
      // 要不要展开，由前端按整库规模自己决定（见 KnowledgeGraph 的自动展开），
      // 不在这里截断 —— 截断会让人以为"这个文件就这么多块"。
      const rows = await invoke<KnowledgeEntryView[]>('kb_list_file_chunks', {
        kbId: activeBaseId,
        sourceRef: relPath,
      });
      set({ graphChunks: { ...get().graphChunks, [relPath]: rows.map(toEntry) } });
      return true;
    } catch (e) {
      showToast(`读取文件分块失败: ${errorMessage(e)}`, 'error');
      return false;
    } finally {
      set({ graphChunkLoading: '' });
    }
  },

  expandGraphFiles: async (targets, opts) => {
    const list = targets.filter((t) => t.relPath && t.chunkCount > 0);
    if (list.length === 0) return 0;
    // 间隔随文件数递减：文件少时看得清"一个一个来"，文件多时总时长收敛在两秒多。
    // 固定 280ms 会让 100 个文件的库干等 45 秒。
    const gap = Math.max(40, Math.min(GRAPH_EXPAND_GAP_MS, Math.round(GRAPH_EXPAND_TOTAL_MS / list.length)));
    const baseId = get().activeBaseId;
    const epoch = get().expandEpoch;

    for (let i = 0; i < list.length; i++) {
      // 换库了就别再往下展开：剩下的 relPath 属于上一个库，展开它们只会张冠李戴
      if (get().activeBaseId !== baseId) return 0;
      // 用户点了「收起全部分块」：立刻收工，别再往回加（否则看起来就是"收起没反应"）
      if (get().expandEpoch !== epoch) return 0;
      // 幂等：已展开的跳过（`toggleGraphFile` 是切换语义，对已展开的调它会收起）
      if (get().graphChunks[list[i].relPath]) continue;
      const ok = await get().toggleGraphFile(list[i].relPath, list[i].chunkCount, opts);
      if (!ok) return list.length - i;
      if (i < list.length - 1) await new Promise((r) => setTimeout(r, gap));
    }
    return 0;
  },

  collapseGraphFiles: () =>
    set((s) => ({ graphChunks: {}, graphChunkLoading: '', expandEpoch: s.expandEpoch + 1 })),

  reloadRelations: async () => {
    const { activeBaseId } = get();
    if (!activeBaseId) return;
    try {
      const rows = await invoke<KnowledgeFileRelationView[]>('kb_list_file_relations', {
        kbId: activeBaseId,
      });
      set({ relations: rows.map(toFileRelation) });
    } catch {
      // 关系读不出来不影响主体功能，静默（下次切库/刷新会再拉一次）
    }
  },

  linkFiles: async (kind, fromFileId, toFileId) => {
    try {
      await invoke('kb_link_files', { kind, from: fromFileId, to: toFileId });
      await get().reloadRelations();
      return true;
    } catch (e) {
      // 后端的拒绝理由（已被别的文件认领 / 成环）是给用户看的，原样透出
      showToast(errorMessage(e), 'error');
      return false;
    }
  },

  unlinkFiles: async (kind, fromFileId, toFileId) => {
    try {
      await invoke('kb_unlink_files', { kind, from: fromFileId, to: toFileId });
      await get().reloadRelations();
      return true;
    } catch (e) {
      showToast(errorMessage(e), 'error');
      return false;
    }
  },

  createBase: async (input, id) => {
    try {
      await invoke('kb_create_base', {
        id,
        name: input.name,
        description: input.description,
        fieldSchema: JSON.stringify(input.fields),
      });
    } catch (e) {
      showToast(`创建知识库失败: ${errorMessage(e)}`, 'error');
      return false;
    }
    set({ activeBaseId: id });
    await get().loadBases();
    showToast('已创建知识库', 'success');
    return true;
  },

  updateBase: async (id, input) => {
    try {
      await invoke('kb_update_base', {
        id,
        name: input.name,
        description: input.description,
        fieldSchema: JSON.stringify(input.fields),
      });
    } catch (e) {
      showToast(`保存知识库失败: ${errorMessage(e)}`, 'error');
      return;
    }
    await get().loadBases();
    showToast('已保存知识库定义', 'success');
  },

  deleteBase: async (id) => {
    try {
      const out = await invoke<{ entries: number; files: number }>('kb_delete_base', { id });
      await get().loadBases();
      return out;
    } catch (e) {
      showToast(`删除知识库失败: ${errorMessage(e)}`, 'error');
      return null;
    }
  },

  addCandidates: async ({ key, value, baseIds, origin, sourceRef }) => {
    set({
      ingestBusy: {
        kind: 'snippet',
        label: origin === 'work' ? 'AI 正在整理本轮沉淀…' : 'AI 正在整理片段…',
        step: key || '标题由 AI 生成',
      },
    });
    try {
      for (const baseId of baseIds) {
        await invoke('kb_add_candidate', {
          kbId: baseId,
          key,
          value,
          metaJson: '{}',
          tags: '',
          origin,
          sourceRef: sourceRef ?? '',
          reason: origin === 'work' ? '本轮会话沉淀' : '手动投喂片段',
        });
      }
    } catch (e) {
      showToast(`提交失败: ${errorMessage(e)}`, 'error');
      return;
    } finally {
      set({ ingestBusy: null });
    }
    showToast(`已提交 ${baseIds.length} 个目标，整理结果在「待确认」里`, 'success');
    // 刷新**放在提示之后且不等**：写入已经成功，"页面什么时候看到"与"这次提交成不成功"无关。
    // 之前 `await` 它，等于让调用方（沉淀弹窗）的「整理中…」一直挂到整库刷新跑完 ——
    // 工作流运行时主库连接紧张、或库很大时，看着就是"卡死"，其实早已写完（见 digestConversation 同条注释）。
    for (const baseId of baseIds) void get().selectBase(baseId);
  },

  digestConversation: async (kbId, messages) => {
    try {
      const out = await invoke<{ count: number; ids: number[]; truncated: boolean }>(
        'kb_digest_conversation',
        {
          kbId,
          messages: messages.map((m) => ({ role: m.role, content: m.content })),
        },
      );
      // 写入已完成 → 到这里就该把结果报出去。刷新**不 await**：它只决定"知识库页显示什么"，
      // 与本次沉淀成不成功无关；await 它会让弹窗的「整理中…」一直挂到 4 条整库查询跑完
      // （选中条目 500 条、主库连接池被工作流占着时尤其久），表现为"卡死但其实早就完成了"。
      void get().selectBase(kbId);
      const cut = out.truncated ? '（内容过长，仅整理了前一部分）' : '';
      if (out.count === 0) {
        showToast(`这段内容里没有可沉淀成知识的部分（闲聊或尚无结论）${cut}`, 'info');
      } else {
        showToast(`已整理出 ${out.count} 条候选${cut}，去「待确认」里核对`, 'success');
      }
      return out.count;
    } catch (e) {
      showToast(`整理失败：${errorMessage(e)}`, 'error');
      return null;
    }
  },

  ingestFiles: async (items, baseIds, opts) => {
    // 已经在投喂就挡掉，避免两次投喂互相覆盖进度状态
    if (get().ingestBusy) {
      showToast('上一个投喂还在进行中，请稍候', 'info');
      return;
    }
    const outcomes: IngestOutcome[] = [];
    const errors: string[] = [];
    const total = items.length * baseIds.length;
    let done = 0;
    // 逐文件推进度：投喂要调模型（长文档还会分批），没有反馈时用户不知道在跑
    const label = opts.asMarkdown ? 'AI 正在整理并转成 markdown…' : 'AI 正在切分与整理文件…';
    set({ ingestBusy: { kind: 'file', label, step: `0/${total}` } });
    try {
      for (const item of items) {
        for (const baseId of baseIds) {
          done += 1;
          const short = item.path.split(/[\\/]/).pop() ?? item.path;
          set({
            ingestBusy: { kind: 'file', label, step: `${done}/${total} · ${short}` },
          });
          try {
            const o = await invoke<IngestOutcome>('kb_ingest_file', {
              kbId: baseId,
              path: item.path,
              asMarkdown: opts.asMarkdown,
              rename: opts.rename,
              // 空名字传 null：后端据此判断"交给 AI 取"，而不是把它当成一个空标题
              name: opts.rename && item.name.trim() ? item.name.trim() : null,
            });
            outcomes.push(o);
          } catch (e) {
            errors.push(`${short}: ${errorMessage(e)}`);
          }
        }
      }
    } finally {
      set({ ingestBusy: null });
    }
    if (outcomes.length === 0) {
      showToast(errors[0] ?? '没有可投喂的文件', 'error');
      return;
    }
    // 同一份原文投给多个库时会复用，去重后只汇报一次
    const uniq = new Map(outcomes.map((o) => [o.relPath, o]));
    const files = [...uniq.values()];
    const chunks = files.reduce((n, o) => n + o.chunks, 0);
    const reused = files.filter((o) => o.reused).length;
    const unsupported = files.filter((o) => o.status === 'unsupported').length;
    const truncated = files.some((o) => o.truncated);
    // AI 相关的提醒（未走 AI 分组 / AI 没能取名）必须**逐条说出来**：
    // 这里曾经只报第一条，剩下的同类问题全被吞掉 —— 用户看到的是"勾了开关没生效"。
    const notes = [...new Set(files.map((o) => o.aiNote).filter(Boolean))];
    const extra = [
      // 开关生效时也要正面确认一次：否则用户无法判断它到底有没有起作用
      opts.asMarkdown ? '已按 markdown 整理入库' : '',
      reused > 0 ? `${reused} 个内容已存在（只加了关联）` : '',
      unsupported > 0 ? `${unsupported} 个格式暂不支持抽取正文` : '',
      truncated ? '条目数超过上限已截断' : '',
      errors.length > 0 ? `${errors.length} 个失败` : '',
    ].filter(Boolean);
    // 提醒单独一条 toast：混在"已入库…"的括号里会被当成噪声跳过
    if (errors.length > 0) {
      // 同一份文件投多个库时失败原因会重复，去重后再说
      const uniqErrors = [...new Set(errors)];
      showToast(
        uniqErrors.slice(0, 3).join('；') + (uniqErrors.length > 3 ? `（共 ${uniqErrors.length} 个失败）` : ''),
        'error',
      );
    }
    if (notes.length > 0) showToast(notes.join('；'), 'info');
    showToast(
      `已入库 ${files.length} 个文件、${chunks} 条知识${extra.length ? `（${extra.join('；')}）` : ''}`,
      unsupported > 0 || errors.length > 0 || notes.length > 0 ? 'info' : 'success',
    );
    await get().loadBases();
    set({ activeTab: 'files' });
  },

  ingestUrl: async (url, baseIds, name) => {
    const outcomes: IngestOutcome[] = [];
    const errors: string[] = [];
    set({
      ingestBusy: { kind: 'url', label: '正在抓取网页并由 AI 整理正文…', step: url },
    });
    try {
      for (const baseId of baseIds) {
        try {
          const o = await invoke<IngestOutcome>('kb_ingest_url', { kbId: baseId, url, name: name ?? null });
          outcomes.push(o);
        } catch (e) {
          errors.push(errorMessage(e));
        }
      }
    } finally {
      set({ ingestBusy: null });
    }
    if (outcomes.length === 0) {
      showToast(errors[0] ?? '抓取失败', 'error');
      return;
    }
    const first = outcomes[0];
    const extra = [
      first.reused ? '该页面已在库中，只加了关联' : '',
      first.truncated ? '条目数超过上限已截断' : '',
      first.aiNote ? `未走 AI 清洗与分组：${first.aiNote}` : '',
    ].filter(Boolean);
    showToast(
      `已抓取「${first.fileName}」并入库 ${first.chunks} 条知识${extra.length ? `（${extra.join('；')}）` : ''}`,
      first.aiNote ? 'info' : 'success',
    );
    await get().loadBases();
    set({ activeTab: 'files' });
  },

  ingestCloudDocs: async (items, baseIds, opts) => {
    // 已经在投喂就挡掉，避免两次投喂互相覆盖进度状态
    if (get().ingestBusy) {
      showToast('上一个投喂还在进行中，请稍候', 'info');
      return;
    }
    const outcomes: IngestOutcome[] = [];
    const errors: string[] = [];
    const total = items.length * baseIds.length;
    let done = 0;
    const label = '正在拉取云文档并由 AI 整理…';
    set({ ingestBusy: { kind: 'cloud', label, step: `0/${total}` } });
    try {
      for (const item of items) {
        for (const baseId of baseIds) {
          done += 1;
          set({ ingestBusy: { kind: 'cloud', label, step: `${done}/${total} · ${item.title}` } });
          try {
            const o = await invoke<IngestOutcome>('kb_cloud_ingest_doc', {
              kbId: baseId,
              accountId: opts.accountId,
              namespace: opts.namespace,
              docId: item.id,
              name: null,
            });
            outcomes.push(o);
          } catch (e) {
            errors.push(`${item.title}: ${errorMessage(e)}`);
          }
          // 每篇之间留一点间隔：列表接口不带正文，N 篇就是 N 次请求，串行 + 间隔才不会撞平台限流
          if (done < total) await new Promise((r) => setTimeout(r, 200));
        }
      }
    } finally {
      set({ ingestBusy: null });
    }
    if (outcomes.length === 0) {
      showToast(errors[0] ?? '没有可投喂的文档', 'error');
      return;
    }
    // 同一份原文投给多个库时会复用，去重后只汇报一次
    const uniq = new Map(outcomes.map((o) => [o.relPath, o]));
    const files = [...uniq.values()];
    const chunks = files.reduce((n, o) => n + o.chunks, 0);
    const reused = files.filter((o) => o.reused).length;
    const truncated = files.some((o) => o.truncated);
    const notes = [...new Set(files.map((o) => o.aiNote).filter(Boolean))];
    const extra = [
      reused > 0 ? `${reused} 篇内容已存在（只加了关联）` : '',
      truncated ? '条目数超过上限已截断' : '',
      errors.length > 0 ? `${errors.length} 篇失败` : '',
    ].filter(Boolean);
    // 失败与 AI 降级各自单独一条：混在"已入库…"的括号里会被当成噪声跳过
    if (errors.length > 0) {
      const uniqErrors = [...new Set(errors)];
      showToast(
        uniqErrors.slice(0, 3).join('；') + (uniqErrors.length > 3 ? `（共 ${uniqErrors.length} 篇失败）` : ''),
        'error',
      );
    }
    if (notes.length > 0) showToast(notes.join('；'), 'info');
    showToast(
      `已从云文档入库 ${files.length} 篇、${chunks} 条知识${extra.length ? `（${extra.join('；')}）` : ''}`,
      errors.length > 0 || notes.length > 0 ? 'info' : 'success',
    );
    await get().loadBases();
    set({ activeTab: 'files' });
  },

  removeFiles: async (kbId, fileIds) => {
    if (fileIds.length === 0) return null;
    let out: { entries: number; files: number };
    try {
      out = await invoke<{ entries: number; files: number }>('kb_remove_files', { kbId, fileIds });
    } catch (e) {
      showToast(`移除失败: ${errorMessage(e)}`, 'error');
      return null;
    }
    await get().loadBases();
    showToast(
      `已从库中移除：删除条目 ${out.entries} 条、删除文件 ${out.files} 个`
      + '（仍被其它库引用的已保留；库定义不受影响）',
      'info',
    );
    return out;
  },

  enrichFile: async (kbId, fileId) => {
    set({ enriching: { index: 1, total: 1, name: '' } });
    try {
      const r = await invoke<{ chunks: number; total: number; failed: number; error: string; fields: string[] }>('kb_enrich_file', {
        kbId,
        fileId,
      });
      // 覆盖要可见：AI 主导维护之下，用户至少要看到"刚才动了哪些字段"，
      // 而不是只看到"整理了 N 条"（那样改了什么完全不可知）
      const fields = r.fields.length > 0 ? `（字段：${r.fields.join('、')}）` : '';
      showToast(
        r.failed > 0
          ? `AI 整理未完成（${r.chunks}/${r.total} 块）：${r.error}`
          : `AI 整理完成：${r.chunks} 个知识块已补标签与专属属性${fields}`,
        r.failed > 0 ? 'error' : 'success',
      );
    } catch (e) {
      showToast(`AI 整理失败: ${errorMessage(e)}`, 'error');
    } finally {
      set({ enriching: null });
    }
    if (get().activeBaseId) await get().selectBase(get().activeBaseId);
  },

  generate: async (topic) => {
    const { activeBaseId } = get();
    if (!topic.trim()) return;
    set({ ingestBusy: { kind: 'ai', label: 'AI 正在生成知识草稿…', step: topic.trim() } });
    try {
      await invoke('kb_generate', { kbId: activeBaseId, topic: topic.trim() });
    } catch (e) {
      showToast(`AI 生成失败: ${errorMessage(e)}`, 'error');
      return;
    } finally {
      set({ ingestBusy: null });
    }
    await get().selectBase(activeBaseId);
    showToast('已生成草稿，请在「待确认」里核对后采纳', 'success');
  },

  adoptCandidate: async (id, patch) => {
    try {
      await invoke('kb_adopt_candidate', {
        id,
        key: patch.key,
        value: patch.value,
        metaJson: JSON.stringify(patch.meta),
      });
    } catch (e) {
      showToast(`采纳失败: ${errorMessage(e)}`, 'error');
      return;
    }
    await get().loadBases();
    showToast('已入库', 'success');
  },

  rejectCandidate: async (id) => {
    try {
      await invoke('kb_reject_candidate', { id });
    } catch (e) {
      showToast(`丢弃失败: ${errorMessage(e)}`, 'error');
      return;
    }
    await get().selectBase(get().activeBaseId);
  },

  updateEntry: async (key, patch) => {
    const { activeBaseId } = get();
    try {
      await invoke('kb_save_entry', {
        kbId: activeBaseId,
        key,
        value: patch.value,
        tags: patch.tags.join(','),
        // null 而不是省略：后端按"空值保留原值"处理，来源信息与分块序号都不会被抹掉
        origin: null,
        sourceRef: null,
        metaJson: JSON.stringify(patch.meta),
      });
    } catch (e) {
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
      return false;
    }
    await get().selectBase(activeBaseId);
    showToast('已保存', 'success');
    return true;
  },

  removeFromBase: async (key) => {
    const { activeBaseId } = get();
    try {
      const deleted = await invoke<boolean>('kb_unlink_entry', { kbId: activeBaseId, key });
      await get().loadBases();
      showToast(deleted ? '该条目仅属于本库，已一并删除' : '已解除与本库的关联（条目仍属其它库）', 'info');
    } catch (e) {
      showToast(`操作失败: ${errorMessage(e)}`, 'error');
    }
  },

  removeEntries: async (keys) => {
    const { activeBaseId } = get();
    if (!activeBaseId || keys.length === 0) return;
    let out: { unlinked: number; deleted: number };
    try {
      out = await invoke<{ unlinked: number; deleted: number }>('kb_unlink_entries', {
        kbId: activeBaseId,
        keys,
      });
    } catch (e) {
      showToast(`批量操作失败: ${errorMessage(e)}`, 'error');
      return;
    }
    await get().loadBases();
    // 如实体现在结果里：删了多少条、多少条只是解除了关联（仍属别的库）
    showToast(
      `已处理 ${out.unlinked} 条：删除 ${out.deleted} 条、解除关联 ${out.unlinked - out.deleted} 条`,
      'info',
    );
  },
}));

/**
 * 条目筛选条件：列表页与图谱**共用同一套口径** —— 这样"图上看到的"就是"列表里看到的"，
 * 顺带让图谱有个用户可控的规模闸门（筛完再看图，比在图里硬塞几百个节点有用）。
 */
export interface EntryFilters {
  query: string;
  /** 'all' 或某个来源 */
  origin: string;
  pinOnly: boolean;
  /** 专属属性筛选（字段 key → 期望值） */
  fields: Record<string, string>;
}

export const EMPTY_ENTRY_FILTERS: EntryFilters = { query: '', origin: 'all', pinOnly: false, fields: {} };

export function hasActiveEntryFilters(f: EntryFilters): boolean {
  return Boolean(f.query.trim()) || f.origin !== 'all' || f.pinOnly
    || Object.values(f.fields).some((v) => v);
}

export function filterEntries(entries: KnowledgeEntry[], f: EntryFilters): KnowledgeEntry[] {
  const q = f.query.trim().toLowerCase();
  return entries.filter((e) => {
    if (f.pinOnly && !e.pin) return false;
    if (f.origin !== 'all' && e.origin !== f.origin) return false;
    if (q && !(
      e.key.toLowerCase().includes(q)
      || e.value.toLowerCase().includes(q)
      || e.tags.some((t) => t.toLowerCase().includes(q))
    )) return false;
    for (const [k, v] of Object.entries(f.fields)) {
      if (!v) continue;
      if (String(e.meta[k] ?? '') !== v) return false;
    }
    return true;
  });
}

/* ────────────────────────── 图谱节点与边（自动派生） ────────────────────────── */

const FILE_NODE_PREFIX = 'file:';
const ENTRY_NODE_PREFIX = 'entry:';

/**
 * 共标签配对时，成员数超过这个数的标签直接跳过不连边。
 *
 * 这个常量是 4b 线性复杂度的**保证**：有了它，配对总数被 Σ C(8,2) 顶住；
 * 去掉它，一个被 500 个节点共享的标签会当场生成 12 万条边，复杂度回到 O(n²)。
 * 语义上也对：人人都有的大标签（"制度""公文"）本来就不区分谁和谁相似。
 */
const MAX_TAG_GROUP = 8;

/** 落盘名是 `<sha8>-<原名>`，展示时去掉指纹前缀 */
export function fileLabelFromSourceRef(relPath: string): string {
  const base = relPath.split(/[\\/]/).pop() ?? relPath;
  return base.replace(/^[0-9a-f]{8}-/, '') || base;
}

/**
 * 文件节点的 id 口径。图谱里多处要自己造这个 id（展开片段时要指向父节点、
 * 文件关系要映射到节点），必须与 `deriveGraph` 用同一个函数，不能各写一遍字符串拼接。
 */
export function fileNodeId(relPath: string): string {
  return `${FILE_NODE_PREFIX}${relPath}`;
}

/**
 * 片段节点的 id。以条目 key 为唯一源：一个 key 在库里只有一行，天然唯一。
 * 用 `chunk:` 前缀而不是 `entry:`，是为了和"独立条目节点"区分开 ——
 * 同一条知识在别的库里可能是独立条目，两种节点不该撞 id。
 */
export function chunkNodeId(entryKey: string): string {
  return `chunk:${entryKey}`;
}

/**
 * 依次展开时相邻两个文件之间的间隔上限。
 * 实际间隔随文件数递减（见 `expandGraphFiles`）—— 文件多时"每个都等满"会变成几十秒的等待。
 */
const GRAPH_EXPAND_GAP_MS = 280;
/** 展开整轮的目标时长：文件再多也让总时长收敛在这个量级，而不是随文件数线性增长 */
const GRAPH_EXPAND_TOTAL_MS = 2400;

export interface GraphModel {
  nodes: KnowledgeGraphNode[];
  relations: KnowledgeRelation[];
  /** 折叠后、截断前的节点总数（用于「已显示 N / 共 M」提示） */
  totalNodes: number;
}

/**
 * 聚合一条或多条条目的专属属性：取值一致时给出该值，不一致时给 `null`（多值）。
 *
 * 专属属性是按分块存在关联行上的，所以文件节点要把所有分块的值并起来才能回答
 * "这个文件的『密级』是什么" —— 块间不一致时不能随便挑一个，必须承认它是多值。
 * 空串按"没填"处理（不参与聚合），布尔值统一成「是 / 否」。
 *
 * 图谱的节点与详情面板都走这一个函数：两处各写一遍必然漂移成两种口径。
 */
export function aggregateMeta(
  entries: KnowledgeEntry[],
  fields: KnowledgeFieldDef[],
): Record<string, string | null> {
  const agg = new Map<string, Set<string>>();
  for (const e of entries) {
    for (const f of fields) {
      const v = e.meta[f.key];
      if (v === undefined || v === '') continue;
      const s = agg.get(f.key) ?? new Set<string>();
      s.add(typeof v === 'boolean' ? (v ? '是' : '否') : String(v));
      agg.set(f.key, s);
    }
  }
  const meta: Record<string, string | null> = {};
  for (const [k, s] of agg) meta[k] = s.size === 1 ? [...s][0] : null;
  return meta;
}

/**
 * 从条目派生图谱：**节点粒度是文件**（同一 `sourceRef` 的全部条目折叠成一个节点）。
 *
 * 为什么不是一个分块一个节点：同源关系是**归属**，不是两两相关。用全连通团表达归属，
 * 边数必然 O(n²)（400 块的文件 = 79,800 条边），图上是一坨等粗的毛线球、没有信息量，
 * 而且 1000 个节点会把布局与 SVG 一起拖死。折叠成文件节点后：「同源」成为节点的自明属性，
 * 图谱表达的是**知识与知识之间的关系**。
 *
 * 文件节点**由 `files` 派生**，不是由条目派生 —— docx/xlsx/图片等抽不出正文的文件
 * （`chunkCount = 0`）没有任何条目，只按条目建节点会让它们在图谱上彻底消失，
 * 表现就是"「文件」页有 5 个文件、图谱上一个节点都没有"。条目只是挂在文件节点上的内容。
 *
 * 注意这里**只产核心节点与语义边**：展开出来的片段节点不进这个函数（见组件的 chunkNodes），
 * 所以「共标签」「专属属性相同」永远只在文件与独立条目之间算 —— 片段一律参与的话，
 * 1000 个高相似片段两两配对，等于把上面刚删掉的同源爆炸换个名字请回来。
 */
export function deriveGraph(
  entries: KnowledgeEntry[],
  base: KnowledgeBase,
  files: KnowledgeFile[],
  fileRelations: KnowledgeFileRelation[],
  /** 条目筛选是否生效：决定"没有命中条目的原文"要不要留在图上，见 2a */
  filtersActive: boolean,
  /**
   * 截断上限。**由调用方给**，不给默认值 —— 图谱的规模现在完全由 LOD 决定（见 graphModel），
   * 这里藏一个默认数字只会变成"两套规模口径"里的第二套。
   */
  maxNodes: number,
): GraphModel {
  // 1. 条目按 sourceRef 归类：有 sourceRef 的属于某个原文，没有的是独立条目
  const bySource = new Map<string, KnowledgeEntry[]>();
  const standalone: KnowledgeEntry[] = [];
  for (const e of entries) {
    if (!e.sourceRef) {
      standalone.push(e);
      continue;
    }
    const list = bySource.get(e.sourceRef) ?? [];
    list.push(e);
    bySource.set(e.sourceRef, list);
  }

  const nodes: KnowledgeGraphNode[] = [];

  // 2a. 文件节点：由 `files` 派生（本库的全部原文，含抽不出正文的那些）
  for (const f of files) {
    if (!f.path) continue; // 没有落盘路径就无法与条目的 sourceRef 对齐
    const list = bySource.get(f.path) ?? [];
    bySource.delete(f.path);
    // 筛选生效时，没有任何命中条目的原文不进图：否则一筛选，库里所有文件还杵在图上，
    // 筛选就失去了"缩小图谱规模"的作用（它是用户唯一的规模闸门）。
    // 但 `chunkCount = 0` 的原文在任何筛选下都留下 —— 它本来就没有内容可筛，
    // 消失只会让人以为文件丢了。
    if (filtersActive && list.length === 0 && f.chunkCount > 0) continue;
    nodes.push({
      id: fileNodeId(f.path),
      label: f.name || fileLabelFromSourceRef(f.path),
      kind: 'file',
      origin: list[0]?.origin ?? (f.url ? 'url' : 'file'),
      sourceRef: f.path,
      chunkCount: f.chunkCount,
      file: f,
      entries: list,
      tags: [...new Set(list.flatMap((e) => e.tags))],
      meta: aggregateMeta(list, base.fields),
      pin: list.some((e) => e.pin),
    });
  }

  // 2b. 剩下的 sourceRef 在 `files` 里找不到对应原文（原文行已被移除、条目还留着）：
  //     仍按文件节点呈现，只是没有原文信息可展示 —— 直接丢掉等于"图上凭空少一块"，无从解释。
  for (const [ref, list] of bySource) {
    nodes.push({
      id: fileNodeId(ref),
      label: fileLabelFromSourceRef(ref),
      kind: 'file',
      origin: list[0].origin,
      sourceRef: ref,
      // 没有原文行可查，只能拿"已加载的条目数"当分块数（这份数据本来就已经不完整了）
      chunkCount: list.length,
      entries: list,
      tags: [...new Set(list.flatMap((e) => e.tags))],
      meta: aggregateMeta(list, base.fields),
      pin: list.some((e) => e.pin),
    });
  }

  // 2c. 独立条目（无 sourceRef）：自身就是一个节点
  for (const e of standalone) {
    nodes.push({
      id: `${ENTRY_NODE_PREFIX}${e.key}`,
      label: e.key,
      kind: 'entry',
      origin: e.origin,
      sourceRef: '',
      chunkCount: 1,
      entries: [e],
      tags: e.tags,
      meta: aggregateMeta([e], base.fields),
      pin: e.pin,
    });
  }

  // 3. 规模封顶：每个节点与边都要占 DOM，画布也放不下无限多的点，必须封顶。
  //    排序给"pin → 分块多（信息量大）→ 标签名"，既优先留重要的，也保证结果确定（同输入同图）。
  //    注意这里封的是**核心节点**（文件 / 独立条目）；展开出来的片段不参与截断，
  //    它有自己的额度（同时展开的文件数 × 单文件片段上限）。
  const totalNodes = nodes.length;
  const kept = totalNodes > maxNodes
    ? [...nodes]
        .sort(
          (a, b) =>
            Number(b.pin) - Number(a.pin)
            || b.chunkCount - a.chunkCount
            || a.label.localeCompare(b.label),
        )
        .slice(0, maxNodes)
    : nodes;
  const keptIds = new Set(kept.map((n) => n.id));

  // 4. 派生边（只在保留的节点之间）
  const map = new Map<string, KnowledgeRelation>();
  const add = (a: string, b: string, kind: KnowledgeEdgeKind, weight: number) => {
    if (a === b || !keptIds.has(a) || !keptIds.has(b)) return;
    const [from, to] = a < b ? [a, b] : [b, a];
    const id = `${from}\u0000${to}`;
    const cur = map.get(id);
    if (cur) {
      if (!cur.kinds.includes(kind)) cur.kinds.push(kind);
      cur.weight += weight;
    } else {
      map.set(id, { from, to, kinds: [kind], weight });
    }
  };

  // 4a. 专属属性相同（只对单选/是否字段；自由文本不连边）；组过大=全连通团，跳过
  const comparable = base.fields.filter((f) => f.type === 'select' || f.type === 'boolean');
  for (const f of comparable) {
    const groups = new Map<string, KnowledgeGraphNode[]>();
    for (const n of kept) {
      const v = n.meta[f.key];
      if (v === undefined || v === null) continue; // 多值不当成"同一个取值"
      const g = groups.get(v) ?? [];
      g.push(n);
      groups.set(v, g);
    }
    for (const g of groups.values()) {
      if (g.length < 2 || g.length > 8) continue;
      for (let i = 0; i < g.length; i++) {
        for (let j = i + 1; j < g.length; j++) add(g[i].id, g[j].id, 'same_field', 1.2);
      }
    }
  }

  // 4b. 共标签（≥2 个），权重 = 共享数
  //
  // **不用全对两两比**（那是 O(n²)：库一大就肉眼可见地卡）。改成倒排索引：
  // 先建 `标签 → 节点`，只在**组内**配对，并跳过成员过多的标签。
  // 跳过是这套算法成为 O(n) 的关键 —— 一个被上百个节点共享的标签本来就没有区分度
  // （人人都有它，连出来只会是几百条等粗的废边），这与 4a 里 `g.length > 8` 是同一个取舍。
  //
  // 成本：Σ C(组大小, 2) ≤ 标签总数 × C(8,2)，而标签总数 = O(n × 平均标签数) ⇒ O(n)。
  const tagGroups = new Map<string, KnowledgeGraphNode[]>();
  for (const n of kept) {
    for (const t of n.tags) {
      const g = tagGroups.get(t) ?? [];
      g.push(n);
      tagGroups.set(t, g);
    }
  }
  // 一对节点共享 ≥2 个标签才算，所以要累计"共享了几个"
  const sharedCount = new Map<string, number>();
  for (const g of tagGroups.values()) {
    if (g.length < 2 || g.length > MAX_TAG_GROUP) continue;
    for (let i = 0; i < g.length; i++) {
      for (let j = i + 1; j < g.length; j++) {
        const a = g[i].id;
        const b = g[j].id;
        const id = a < b ? `${a}\u0000${b}` : `${b}\u0000${a}`;
        sharedCount.set(id, (sharedCount.get(id) ?? 0) + 1);
      }
    }
  }
  for (const [id, shared] of sharedCount) {
    if (shared < 2) continue;
    const sep = id.indexOf('\u0000');
    add(id.slice(0, sep), id.slice(sep + 1), 'shared_tag', shared * 0.5);
  }

  // 4c. 文件之间的关系（附件 / 版本替代）：上一轮建立的关系在这里显形
  const nodeIdByFileId = new Map(
    files.filter((f) => f.path).map((f) => [f.id, fileNodeId(f.path)]),
  );
  for (const r of fileRelations) {
    const from = nodeIdByFileId.get(r.fromFileId);
    const to = nodeIdByFileId.get(r.toFileId);
    if (!from || !to) continue; // 另一端不在当前图谱里（被筛掉/不属于本库）
    add(from, to, r.kind, 2);
  }

  return { nodes: kept, relations: [...map.values()], totalNodes };
}
