import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Library,
  Search,
  Loader2,
  FileText,
  X,
  Paperclip,
  Trash2,
  Save,
  Check,
  AlertTriangle,
  ArrowUpRight,
  ChevronRight,
  Files,
  Sparkles,
  Network,
  FolderOpen,
  Folder,
  Pencil,
  Plus,
  FileUp,
  Wand2,
  Scissors,
  Globe,
  Info,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { TitleBar, StatusBar } from '../components/layout';
import { TagEditor } from '../components/knowledge/TagEditor';
import { FieldSchemaEditor, MetaEditor } from '../components/knowledge/KnowledgeFieldInputs';
import { Select } from '../components/common/Select';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useEnclaveKbStore } from '../stores/enclaveKbStore';
import { useEnclaveTaskStore } from '../stores/enclaveTaskStore';
import { useAccountStore } from '../stores/accountStore';
import { confirmDialog } from '../stores/confirmStore';
import { EnclaveNavBar } from '../components/enclave/EnclaveNavBar';
import { buildTeamGraph, normalizeOrigin } from '../components/enclave/teamGraph';
import { KnowledgeGraph } from '../components/knowledge/KnowledgeGraph';
import { showToast } from '../utils/toast';
import { errorMessage } from '../utils/errorMessage';
import type { EnclaveKbQueueItem, KbEntryView, KbFileListView } from '../types/enclave';
import type { IngestCapability, KnowledgeBase, KnowledgeEntry, KnowledgeFieldDef, KnowledgeFieldType, KnowledgeMeta, KnowledgeOrigin } from '../types/knowledge';

/** 表单控件口径：与个人知识库一致（参见 KnowledgeBaseDialog） */
const FIELD_CLASS = 'w-full px-2.5 py-1.5 rounded-lg text-xs outline-none';
const FIELD_STYLE = {
  backgroundColor: 'var(--bg-tertiary)',
  color: 'var(--text-primary)',
  border: '1px solid var(--border)',
} as const;

/** 待整理条目的类型标签 */
const QUEUE_KIND_LABEL: Record<string, string> = {
  snippet: '片段',
  file: '文件',
  url: '网址',
};

/** 待整理条目的展示名：从 payload 里取原始输入（不额外存冗余字段） */
function queueTitle(item: EnclaveKbQueueItem): string {
  try {
    const p = JSON.parse(item.payload) as Record<string, unknown>;
    // 网址：抓到原文后优先显示页面标题，其次用户填的名，最后才回落到 URL
    if (item.kind === 'url') return String(p.title ?? p.name ?? p.url ?? item.sourceRef);
    if (item.kind === 'file') {
      const name = String(p.fileName ?? '');
      if (name) return name;
      const rel = String(p.relPath ?? item.sourceRef);
      return rel.split(/[\\/]/).pop() || rel;
    }
    const text = String(p.title ?? p.text ?? '');
    return text.slice(0, 40) || '片段';
  } catch {
    return item.sourceRef;
  }
}

/**
 * 待整理 → 手工整理的内联表单：先把原文提取回填，用户确认后才写组织库。
 * 提取失败（如网址还没抓到原文）不挡路 —— 表单仍可手工填写后提交。
 */
type ManualDraft = {
  id: string;
  title: string;
  content: string;
  tags: string[];
  /** 正在提取原文 */
  loading: boolean;
  /** 提取失败的原因 */
  notice: string;
  saving: boolean;
  error: string;
};

function formatTime(unixSec: number): string {
  if (!unixSec) return '-';
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 列表里用相对时间（与个人知识库一致），详情里用绝对时间。 */
function formatRelative(unixSec: number): string {
  if (!unixSec) return '-';
  const diff = Math.floor(Date.now() / 1000) - unixSec;
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 86400 * 30) return `${Math.floor(diff / 86400)} 天前`;
  return formatTime(unixSec);
}

function formatSize(bytes: number): string {
  if (!bytes) return '';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function splitTags(raw: string): string[] {
  return raw
    .split(/[,，]/)
    .map((t) => t.trim().replace(/^#/, '').trim())
    .filter(Boolean);
}

function joinTags(tags: string[]): string {
  return tags.join(',');
}

/** 团队库只有 5 种真实来源；`manual` 是早期数据的错误值，读时统一经 normalizeOrigin 还原 */
const ORIGIN_LABEL: Record<string, string> = {
  task_output: '任务产出',
  file: '文件投喂',
  url: '网址投喂',
  snippet: '片段',
  ai: 'AI 生成',
};

/** 条目来源的展示文案（先还原历史 'manual'，再取标签） */
function entryOriginLabel(e: KbEntryView): string {
  const o = normalizeOrigin(e);
  return ORIGIN_LABEL[o] ?? o;
}

type KbTab = 'entries' | 'files' | 'ingest' | 'graph';

/** 文件目录树的分组维度：系统维度 + 当前库的专属属性字段 */
type FileGroupBy =
  | { kind: 'entry' }
  | { kind: 'assignee' }
  | { kind: 'taskPriority' }
  | { kind: 'filetype' }
  | { kind: 'month' }
  | { kind: 'field'; key: string };

/** Select 只吃字符串：专属属性编码成 `field:<key>` */
function encodeGroupBy(g: FileGroupBy): string {
  return g.kind === 'field' ? `field:${g.key}` : g.kind;
}

function decodeGroupBy(v: string): FileGroupBy {
  if (v.startsWith('field:')) return { kind: 'field', key: v.slice(6) };
  switch (v) {
    case 'assignee':
    case 'taskPriority':
    case 'filetype':
    case 'month':
      return { kind: v };
    default:
      return { kind: 'entry' };
  }
}

/**
 * 专属属性值 → 目录。
 *
 * 空值单独成组（「未标注」），布尔按是/否，日期按月归并 ——
 * 与「按月份」维度同口径；其余类型直接用原值当目录名。
 */
function fieldGroup(
  f: KbFileListView,
  key: string,
  type?: KnowledgeFieldType,
): { id: string; title: string } {
  const raw = f.metadata?.[key];
  if (raw === undefined || raw === null || String(raw).trim() === '') {
    return { id: '__empty__', title: '未标注' };
  }
  if (type === 'boolean') {
    const on = raw === true || raw === 'true' || raw === 1 || raw === '1';
    return { id: on ? 'true' : 'false', title: on ? '是' : '否' };
  }
  if (type === 'date') {
    const d = new Date(String(raw));
    if (!Number.isNaN(d.getTime())) {
      const m = d.getMonth() + 1;
      return {
        id: `${d.getFullYear()}-${String(m).padStart(2, '0')}`,
        title: `${d.getFullYear()} 年 ${m} 月`,
      };
    }
  }
  const text = String(raw);
  return { id: text, title: text };
}

const PRIORITY_LABEL: Record<string, string> = {
  low: '低',
  normal: '普通',
  high: '高',
  urgent: '紧急',
};

const PRIORITY_ORDER: Record<string, number> = {
  urgent: 0,
  high: 1,
  normal: 2,
  low: 3,
};

function fileExt(name: string): string {
  const i = name.lastIndexOf('.');
  return i > 0 && i < name.length - 1 ? name.slice(i + 1).toLowerCase() : '';
}

/** 条目上属于当前库专属属性的取值（只取标量，忽略 reportId 这类溯源信息） */
function pickFields(entry: KbEntryView | null, fields: KnowledgeFieldDef[]): KnowledgeMeta {
  const out: KnowledgeMeta = {};
  if (!entry) return out;
  for (const f of fields) {
    const v = entry.metadata?.[f.key];
    if (typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean') {
      out[f.key] = v;
    }
  }
  return out;
}

/** 专属属性草稿是否有改动（逐键比较，不受键顺序影响） */
function sameFields(a: KnowledgeMeta, b: KnowledgeMeta): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const k of keys) {
    if (String(a[k] ?? '') !== String(b[k] ?? '')) return false;
  }
  return true;
}

/** 搜索框口径：与个人知识库一致（pd-field 容器 + 内部透明输入） */
function SearchField({
  value,
  onChange,
  onEnter,
  onClear,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  onEnter?: () => void;
  onClear: () => void;
  placeholder: string;
}) {
  return (
    <div
      className="pd-field flex items-center gap-1.5 px-2 py-1 rounded-lg shrink-0 w-[280px]"
      style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
    >
      <Search size={12} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
      <input
        className="flex-1 min-w-0 bg-transparent outline-none text-xs"
        style={{ color: 'var(--text-primary)' }}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter') onEnter?.(); }}
        placeholder={placeholder}
      />
      {value && (
        <button onClick={onClear} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} title="清空搜索">
          <X size={11} />
        </button>
      )}
    </div>
  );
}

/** 成员端：文件会被下载到本机缓存，给一个常驻说明图标（主机本机读文件无需提示） */
function CacheHintIcon({ visible }: { visible: boolean }) {
  if (!visible) return null;
  return (
    <span
      className="shrink-0 flex items-center"
      style={{ color: 'var(--text-tertiary)' }}
      title="打开会把副本下载到本机缓存；可在「团队 › 主机与连接」查看用量或清空"
    >
      <Info size={11} />
    </span>
  );
}

const TABS: { key: KbTab; label: string; icon: typeof Search; enabled: boolean }[] = [
  { key: 'entries', label: '知识', icon: Search, enabled: true },
  { key: 'files', label: '文件', icon: Files, enabled: true },
  { key: 'ingest', label: '投喂', icon: Sparkles, enabled: true },
  { key: 'graph', label: '图谱', icon: Network, enabled: true },
];

/** 图谱组件的 `entries` 入参：团队数据走 `source`，这里给一个稳定空数组即可 */
const NO_ENTRIES: KnowledgeEntry[] = [];

/** 成员端首次打开文件时提示「会下载副本」的一次性标记 */
const CACHE_HINT_KEY = 'pilotdesk.enclave.cacheHint.v1';

/** 成员端（非主机）打开文件会把副本下载到本机缓存：只说明一次 */
function showCacheHintOnce(): void {
  try {
    if (localStorage.getItem(CACHE_HINT_KEY)) return;
    localStorage.setItem(CACHE_HINT_KEY, '1');
  } catch {
    // 隐私模式等拿不到 localStorage：照常提示，只是下次还会再提示
  }
  showToast(
    '已把文件副本下载到本机缓存；可在「团队 › 主机与连接」查看用量或清空',
    'info',
    9000,
  );
}

/**
 * 飞地团队知识库 —— 独立路由 `/enclave/kb`。
 *
 * 布局参照个人知识库：上部是知识库信息（owner / admin 可修改），
 * 下部是「知识 / 文件 / 投喂 / 图谱」四个 tab。本期只实现知识与文件，
 * 投喂与图谱留到下一阶段（tab 置灰）。
 *
 * 数据来自主机 `enclave.db`：任务汇报「确认」后自动沉淀为条目；
 * 检索走 FTS5 trigram（≥3 字），短词退化为子串匹配。
 */
export function EnclaveKbPage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();

  const orgs = useEnclaveOrgStore((s) => s.orgs);
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);
  const orgDetail = useEnclaveOrgStore((s) => s.detail);
  // 不要在选择器里写 `s.detail?.members ?? []`：detail 为空时每次都会返回**新的空数组**，
  // useSyncExternalStore 会认为快照一直在变 → 提交阶段反复触发重渲染（Maximum update depth exceeded）。
  const members = useMemo(() => orgDetail?.members ?? [], [orgDetail]);

  const kbs = useEnclaveKbStore((s) => s.kbs);
  const activeKbId = useEnclaveKbStore((s) => s.activeKbId);
  const entries = useEnclaveKbStore((s) => s.entries);
  const activeEntry = useEnclaveKbStore((s) => s.activeEntry);
  const files = useEnclaveKbStore((s) => s.files);
  const orphans = useEnclaveKbStore((s) => s.orphans);
  /** 图谱专用条目（分页拉取，与列表/搜索互不影响） */
  const graphEntries = useEnclaveKbStore((s) => s.graphEntries);
  const graphHasMore = useEnclaveKbStore((s) => s.graphHasMore);
  const graphLoading = useEnclaveKbStore((s) => s.graphLoading);
  const query = useEnclaveKbStore((s) => s.query);
  const loading = useEnclaveKbStore((s) => s.loading);
  const error = useEnclaveKbStore((s) => s.error);

  const [tab, setTab] = useState<KbTab>('entries');
  const [keyword, setKeyword] = useState('');
  const [fileKeyword, setFileKeyword] = useState('');
  /** 文件目录树的分组维度 */
  const [fileGroupBy, setFileGroupBy] = useState<FileGroupBy>({ kind: 'entry' });
  /** 文件目录树里被折叠的分组 */
  const [collapsedGroups, setCollapsedGroups] = useState<string[]>([]);
  // 知识库定义表单（owner / admin）：新建与编辑共用一套草稿
  const [kbForm, setKbForm] = useState<null | {
    mode: 'create' | 'edit';
    name: string;
    description: string;
    fields: KnowledgeFieldDef[];
  }>(null);
  // 展开行的草稿：正文 / 标签 / 标题一起保存（与个人知识库一致）
  const [draftTitle, setDraftTitle] = useState('');
  const [draftContent, setDraftContent] = useState('');
  const [draftTags, setDraftTags] = useState<string[]>([]);
  /** 专属属性草稿（只含当前库 field_schema 的键） */
  const [draftMeta, setDraftMeta] = useState<KnowledgeMeta>({});
  const [saving, setSaving] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  /* ── 投喂（人工入库）：内容照收，整理按加工能力降级 ── */
  const capability = useEnclaveKbStore((s) => s.capability);
  const queue = useEnclaveKbStore((s) => s.queue);
  const candidates = useEnclaveKbStore((s) => s.candidates);
  /** 投喂队列里的文件（待整理 / 待重试）：已归档但还没产出条目 —— 文件页要显式提示，避免被当成冗余 */
  const pendingFileQueue = useMemo(
    () => queue.filter((q) => q.kind === 'file'),
    [queue],
  );
  const [ingestMode, setIngestMode] = useState<'snippet' | 'file' | 'url' | 'ai'>('snippet');
  const [snipTitle, setSnipTitle] = useState('');
  const [snipText, setSnipText] = useState('');
  const [ingestUrl, setIngestUrl] = useState('');
  const [ingestUrlName, setIngestUrlName] = useState('');
  const [filePaths, setFilePaths] = useState<string[]>([]);
  /** 每个文件的投喂结果（提交后保留，便于逐条看失败原因） */
  const [fileOutcome, setFileOutcome] = useState<
    Record<string, { status: 'ok' | 'queued' | 'error'; message?: string }>
  >({});
  const [fileProgress, setFileProgress] = useState<{ done: number; total: number } | null>(null);
  const [topic, setTopic] = useState('');
  const [picking, setPicking] = useState(false);
  const [ingesting, setIngesting] = useState(false);
  const [manualDraft, setManualDraft] = useState<ManualDraft | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const [dragCount, setDragCount] = useState(0);
  /** 本机模型（AI 生成用）：与团队加工能力分开判 —— 生成只把主题发给模型，不需要主机能力 */
  const [localCap, setLocalCap] = useState<IngestCapability | null>(null);
  const ingestPanelRef = useRef<HTMLDivElement>(null);
  const modelBlocked = capability !== null && capability.canProcess === 'none';
  const aiBlocked = localCap === null || localCap.canProcess === 'none';
  const canSubmitIngest =
    ingestMode === 'snippet'
      ? Boolean(snipText.trim())
      : ingestMode === 'file'
        ? filePaths.length > 0
        : ingestMode === 'url'
          ? Boolean(ingestUrl.trim())
          : Boolean(topic.trim()) && !aiBlocked;

  /** 手工整理第一步：打开内联表单并回填提取结果（提取失败则留空白表单） */
  const openManualDraft = async (item: EnclaveKbQueueItem) => {
    if (!activeOrgId) return;
    setManualDraft({
      id: item.id,
      title: '',
      content: '',
      tags: [],
      loading: true,
      notice: '',
      saving: false,
      error: '',
    });
    try {
      const preview = await useEnclaveKbStore
        .getState()
        .previewManualIngest(activeOrgId, item.id);
      setManualDraft((d) =>
        d && d.id === item.id ? { ...d, ...preview, loading: false } : d,
      );
    } catch (e) {
      const notice = errorMessage(e);
      setManualDraft((d) => (d && d.id === item.id ? { ...d, loading: false, notice } : d));
    }
  };

  /** 手工整理第二步：确认表单内容后入库 */
  const submitManualDraft = async () => {
    if (!manualDraft || !activeOrgId) return;
    if (!manualDraft.content.trim()) {
      setManualDraft({ ...manualDraft, error: '正文不能为空' });
      return;
    }
    setManualDraft({ ...manualDraft, saving: true, error: '' });
    try {
      await useEnclaveKbStore
        .getState()
        .commitManualIngest(
          activeOrgId,
          manualDraft.id,
          manualDraft.title,
          manualDraft.content,
          joinTags(manualDraft.tags),
        );
      setManualDraft(null);
      showToast('已入组织库', 'success');
    } catch (e) {
      const msg = errorMessage(e);
      setManualDraft((d) => (d ? { ...d, saving: false, error: msg } : d));
    }
  };

  useEffect(() => {
    if (tab !== 'ingest' || !activeOrgId || !activeKbId) return;
    const st = useEnclaveKbStore.getState();
    void st.loadCapability(activeOrgId);
    void st.loadQueue(activeOrgId, activeKbId);
    void st.loadCandidates(activeOrgId, activeKbId);
    // AI 生成用本机模型：单独探测（与团队加工能力不是一回事）
    void invoke<IngestCapability>('kb_ingest_capability')
      .then(setLocalCap)
      .catch(() => setLocalCap(null));
  }, [tab, activeOrgId, activeKbId]);

  /** 拖拽投喂：拖入即切到文件模式；只认投喂面板内的落点 */
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const isOverPanel = (x: number, y: number): boolean => {
      const el = ingestPanelRef.current;
      if (!el) return false;
      const scale = window.devicePixelRatio || 1;
      const r = el.getBoundingClientRect();
      return x / scale >= r.left && x / scale <= r.right && y / scale >= r.top && y / scale <= r.bottom;
    };
    (async () => {
      try {
        unlisten = await getCurrentWebview().onDragDropEvent((event) => {
          const p = event.payload;
          if (p.type === 'enter') {
            const over = isOverPanel(p.position.x, p.position.y);
            setDragOver(over);
            setDragCount(over ? p.paths.length : 0);
            if (over) setIngestMode('file');
          } else if (p.type === 'over') {
            setDragOver(isOverPanel(p.position.x, p.position.y));
          } else if (p.type === 'leave') {
            setDragOver(false);
            setDragCount(0);
          } else if (p.type === 'drop') {
            const over = isOverPanel(p.position.x, p.position.y);
            setDragOver(false);
            setDragCount(0);
            if (over && p.paths.length > 0) {
              setIngestMode('file');
              setFilePaths((prev) => {
                const seen = new Set(prev);
                return [...prev, ...p.paths.filter((x) => !seen.has(x))];
              });
            }
          }
        });
        if (disposed) unlisten();
      } catch (e) {
        console.warn('[EnclaveKbPage] 注册拖拽监听失败:', e);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const pickFiles = async () => {
    setPicking(true);
    try {
      const picked = await openDialog({
        multiple: true,
        title: '选择要投喂到团队知识库的文件（可多选）',
      });
      const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
      setFilePaths((prev) => {
        const seen = new Set(prev);
        return [...prev, ...list.filter((x) => !seen.has(x))];
      });
    } finally {
      setPicking(false);
    }
  };

  const submitIngest = async () => {
    if (!activeOrgId || !activeKbId || ingesting) return;
    const st = useEnclaveKbStore.getState();
    setIngesting(true);
    try {
      if (ingestMode === 'ai') {
        const ok = await st.generate(activeOrgId, activeKbId, topic.trim());
        if (ok) {
          showToast('已生成草稿，请到「待确认」核对后采纳', 'success');
          setTopic('');
        }
        return;
      }
      if (ingestMode === 'file') {
        // 串行逐个提交：每个文件独立入队/独立整理，失败互不影响
        const paths = [...filePaths];
        /** 本轮提交成功（入库或排队）的文件：提交后从清单移除 */
        const donePaths = new Set<string>();
        let ok = 0;
        let queued = 0;
        let failed = 0;
        setFileOutcome({});
        setFileProgress({ done: 0, total: paths.length });
        for (let i = 0; i < paths.length; i += 1) {
          const p = paths[i];
          const out = await st.ingestFile(activeOrgId, activeKbId, p);
          if (!out) {
            failed += 1;
            setFileOutcome((prev) => ({ ...prev, [p]: { status: 'error', message: '提交失败' } }));
          } else if (out.queued) {
            queued += 1;
            donePaths.add(p);
            setFileOutcome((prev) => ({ ...prev, [p]: { status: 'queued' } }));
          } else {
            ok += 1;
            donePaths.add(p);
            setFileOutcome((prev) => ({
              ...prev,
              [p]: { status: 'ok', message: `${out.entries} 条` },
            }));
          }
          setFileProgress({ done: i + 1, total: paths.length });
        }
        setFileProgress(null);
        // 已提交成功的文件从清单里移除（否则用户会误以为没提交成功）；失败的保留，便于重试
        const failedPaths = paths.filter((p) => !donePaths.has(p));
        setFilePaths(failedPaths);
        setFileOutcome((prev) => {
          const next: typeof prev = {};
          for (const p of failedPaths) if (prev[p]) next[p] = prev[p];
          return next;
        });
        showToast(
          `已提交 ${paths.length} 个文件：入库 ${ok}，待整理 ${queued}${failed > 0 ? `，失败 ${failed}` : ''}`,
          failed > 0 ? 'error' : queued > 0 ? 'info' : 'success',
        );
        return;
      }
      const out =
        ingestMode === 'snippet'
          ? await st.ingestSnippet(activeOrgId, activeKbId, snipTitle.trim(), snipText)
          : await st.ingestUrl(
              activeOrgId,
              activeKbId,
              ingestUrl.trim(),
              ingestUrlName.trim() || undefined,
            );
      if (!out) return;
      if (out.queued) {
        showToast('已入库，等待主机模型整理（见「待整理」）', 'info');
      } else {
        showToast(`已整理入库 ${out.entries} 条知识`, 'success');
      }
      setSnipTitle('');
      setSnipText('');
      setIngestUrl('');
      setIngestUrlName('');
    } finally {
      setIngesting(false);
    }
  };

  /** 手动刷新：身份 → 组织 → 知识库 / 文件 / 当前检索。 */
  const refreshKb = async () => {
    setRefreshing(true);
    try {
      const org = useEnclaveOrgStore.getState();
      if (!(await org.ensureIdentity())) return;
      await org.loadOrgs();
      const next = useEnclaveOrgStore.getState().activeOrgId;
      if (!next) return;
      await useEnclaveKbStore.getState().loadKbs(next);
      const kbId = useEnclaveKbStore.getState().activeKbId;
      if (!kbId) return;
      await useEnclaveKbStore.getState().loadFiles(next, kbId);
      await useEnclaveKbStore.getState().search(next, useEnclaveKbStore.getState().query);
    } finally {
      setRefreshing(false);
    }
  };

  // 登录账号变化 → 先校验新身份，再加载组织与知识库
  const accountKey = useAccountStore((s) => s.account?.email ?? '');

  const myRole = orgs.find((o) => o.orgId === activeOrgId)?.myRole ?? '';
  const canEdit = myRole === 'owner' || myRole === 'admin';
  /** 手工入库：绕过模型直接写共享库，与条目编辑同权限（owner / admin） */
  const canManual = canEdit;
  /** 当前库：默认库排最前，切换后按 activeKbId 取 */
  const kb = kbs.find((k) => k.id === activeKbId) ?? kbs[0] ?? null;
  /** 当前库的专属属性定义（图谱详情按它渲染属性区） */
  const kbFields = useMemo<KnowledgeFieldDef[]>(() => {
    try {
      const parsed = JSON.parse(kb?.fieldSchema ?? '[]');
      return Array.isArray(parsed) ? (parsed as KnowledgeFieldDef[]) : [];
    } catch {
      return [];
    }
  }, [kb?.fieldSchema]);

  /* ── 图谱：团队条目 → 个人库图谱组件认识的 GraphModel（复用 3D 渲染） ── */
  const teamGraph = useMemo(
    () => buildTeamGraph(graphEntries, kbFields.map((f) => f.key)),
    [graphEntries, kbFields],
  );
  const graphBase: KnowledgeBase = useMemo(
    () => ({
      id: kb?.id ?? '',
      name: kb?.name ?? '',
      description: kb?.description ?? '',
      fields: kbFields,
      entryCount: kb?.entryCount ?? 0,
      pinnedCount: 0,
      fileCount: kb?.fileCount ?? 0,
      pendingCount: 0,
      createdAt: 0,
      updatedAt: kb?.lastEntryAt ?? 0,
    }),
    [kb, kbFields],
  );
  const graphSource = useMemo(
    () => ({
      core: teamGraph,
      onGoEntries: () => setTab('entries'),
      // 团队文件节点的计数口径是"被几个条目引用"，不是分块
      fileCountLabel: '个关联条目',
      // 团队库投喂只有 文本/文件/网址/AI，没有「云文档」；task_output 在团队语义里是「任务产出」
      legend: {
        origins: ['work', 'snippet', 'file', 'url', 'ai'] as KnowledgeOrigin[],
        labels: { work: '任务产出' } as Partial<Record<KnowledgeOrigin, string>>,
      },
      // 条目分页拉：首屏自动续拉到 1000 条，之后由用户点「加载更多」逐批追加
      loadMore: {
        hasMore: graphHasMore,
        loading: graphLoading,
        loaded: graphEntries.length,
        onLoad: () => {
          if (activeOrgId && activeKbId) {
            void useEnclaveKbStore.getState().loadMoreGraphEntries(activeOrgId, activeKbId);
          }
        },
      },
    }),
    [teamGraph, graphHasMore, graphLoading, graphEntries.length, activeOrgId, activeKbId],
  );

  const memberName = (raw: unknown): string => {
    const id = typeof raw === 'string' ? raw : '';
    if (!id) return '';
    const m = members.find((x) => x.userId === id);
    return m?.displayName || m?.email || `${id.slice(0, 8)}…`;
  };

  const metaStr = (entry: KbEntryView | null, key: string): string => {
    const v = entry?.metadata?.[key];
    return typeof v === 'string' ? v : '';
  };
  const metaNum = (entry: KbEntryView | null, key: string): number => {
    const v = entry?.metadata?.[key];
    return typeof v === 'number' ? v : 0;
  };

  /** 展开 / 收起：同一个条目再点一次即收起。 */
  const toggleEntry = (entry: KbEntryView) => {
    const cur = useEnclaveKbStore.getState().activeEntry;
    useEnclaveKbStore.getState().openEntry(cur?.memoryId === entry.memoryId ? null : entry);
  };

  useEffect(() => {
    setDraftTitle(activeEntry?.title ?? '');
    setDraftContent(activeEntry?.content ?? '');
    setDraftTags(splitTags(activeEntry?.tags ?? ''));
    setDraftMeta(pickFields(activeEntry, kbFields));
  }, [activeEntry, kbFields]);

  // 切换组织 / 账号 / 知识库：退出定义编辑态
  useEffect(() => {
    setKbForm(null);
  }, [kb?.id, activeOrgId]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const st = useEnclaveOrgStore.getState();
      if (!(await st.ensureIdentity()) || cancelled) return;
      await st.loadOrgs();
      if (cancelled) return;
      const cur = useEnclaveOrgStore.getState();
      if (cur.orgs.length === 0 && !cur.lastRefresh) await cur.refresh();
    })();
    return () => {
      cancelled = true;
    };
  }, [accountKey]);

  useEffect(() => {
    if (activeOrgId) void useEnclaveKbStore.getState().loadKbs(activeOrgId);
  }, [activeOrgId]);

  // 「文件」tab 懒加载（条目页不需要先拉全量文件）
  useEffect(() => {
    if (tab === 'files' && activeOrgId && activeKbId) {
      void useEnclaveKbStore.getState().loadFiles(activeOrgId, activeKbId);
    }
  }, [tab, activeOrgId, activeKbId]);

  // 「图谱」tab 懒加载：单独多取一批条目（不覆盖列表/搜索状态）
  useEffect(() => {
    if (tab === 'graph' && activeOrgId && activeKbId) {
      void useEnclaveKbStore.getState().loadGraphEntries(activeOrgId, activeKbId);
    }
  }, [tab, activeOrgId, activeKbId]);

  // 切换分组维度后旧的折叠状态不再适用
  useEffect(() => {
    setCollapsedGroups([]);
  }, [fileGroupBy]);

  // 切库后旧库的属性 key 可能已不存在：回到默认维度
  useEffect(() => {
    setFileGroupBy({ kind: 'entry' });
  }, [activeKbId]);

  // 当前库删掉了正在使用的属性：同样退回默认维度（否则整页都归「未标注」）
  useEffect(() => {
    if (fileGroupBy.kind === 'field' && !kbFields.some((f) => f.key === fileGroupBy.key)) {
      setFileGroupBy({ kind: 'entry' });
    }
  }, [kbFields, fileGroupBy]);

  const handleModeChange = useCallback(
    (mode: ViewMode) => {
      setMode(mode);
      navigate('/');
    },
    [setMode, navigate],
  );

  const runSearch = async (value?: string) => {
    if (!activeOrgId) return;
    await useEnclaveKbStore.getState().search(activeOrgId, value ?? keyword);
  };

  const openFile = async (fileId: string) => {
    if (!activeOrgId) return;
    // 主机（owner）直接读本机文件；其余角色会先把副本下载到本机缓存
    if (myRole && myRole !== 'owner') showCacheHintOnce();
    try {
      const path = await invoke<string>('enclave_file_abs_path', { orgId: activeOrgId, fileId });
      await invoke('open_path', { path });
    } catch {
      // 单个文件打不开不影响列表；错误由 store 统一提示
    }
  };

  const startEditKb = () => {
    if (!kb) return;
    setKbForm({ mode: 'edit', name: kb.name, description: kb.description, fields: kbFields });
  };

  const startCreateKb = () => {
    setKbForm({ mode: 'create', name: '', description: '', fields: [] });
  };

  const saveKb = async () => {
    if (!activeOrgId || !kbForm) return;
    const name = kbForm.name.trim();
    if (!name) {
      showToast('知识库名称不能为空', 'error');
      return;
    }
    const schema = JSON.stringify(kbForm.fields);
    const store = useEnclaveKbStore.getState();
    if (kbForm.mode === 'create') {
      const ok = await store.createKb(activeOrgId, name, kbForm.description, schema);
      if (ok) {
        setKbForm(null);
        showToast('已创建知识库', 'success');
      }
      return;
    }
    if (!kb) return;
    const ok = await store.updateKb(activeOrgId, kb.id, name, kbForm.description, schema);
    if (ok) {
      setKbForm(null);
      showToast('已保存知识库定义', 'success');
    }
  };

  const cancelKbForm = () => setKbForm(null);

  /** 删除自定义库：默认库与有内容的库由后端拒绝 */
  const removeKb = async () => {
    if (!activeOrgId || !kb || kb.isImplicit) return;
    const ok = await confirmDialog({
      title: '删除知识库',
      message: `确定删除「${kb.name}」？\n只有空库能删除：库里还有知识条目 / 待整理 / 待确认草稿时会被拒绝。`,
      confirmText: '删除',
      danger: true,
    });
    if (!ok) return;
    const done = await useEnclaveKbStore.getState().deleteKb(activeOrgId, kb.id);
    if (done) showToast('已删除知识库', 'success');
  };

  const saveEntry = async () => {
    if (!activeOrgId || !activeEntry || saving) return;
    const kbId = useEnclaveKbStore.getState().activeKbId;
    setSaving(true);
    try {
      await useEnclaveKbStore.getState().updateEntry(activeOrgId, kbId, activeEntry.memoryId, {
        title: draftTitle,
        content: draftContent,
        tags: joinTags(draftTags),
        fields: JSON.stringify(draftMeta),
      });
    } finally {
      setSaving(false);
    }
  };

  /** 取消：丢弃未保存的草稿并收起 —— 按钮在展开区底部，不必滚回顶部 */
  const cancelEdit = () => {
    if (activeEntry) {
      setDraftTitle(activeEntry.title);
      setDraftContent(activeEntry.content);
      setDraftTags(splitTags(activeEntry.tags));
      setDraftMeta(pickFields(activeEntry, kbFields));
    }
    useEnclaveKbStore.getState().openEntry(null);
  };

  const removeEntry = async () => {
    if (!activeOrgId || !activeEntry) return;
    const ok = await confirmDialog({
      title: '移除知识条目',
      message: `确定从团队知识库移除「${activeEntry.title || activeEntry.memoryId}」？\n条目引用的交付物 / 来源文件原件不会被删除。`,
      confirmText: '移除',
    });
    if (!ok) return;
    const kbId = useEnclaveKbStore.getState().activeKbId;
    await useEnclaveKbStore.getState().removeEntry(activeOrgId, kbId, activeEntry.memoryId);
  };

  /** 解除条目与一份原件的关联（任务产出的附件由汇报维护，入口不渲染） */
  const detachEntryFile = async (entry: KbEntryView, fileId: string, fileName: string) => {
    if (!activeOrgId) return;
    const ok = await confirmDialog({
      title: '解除文件关联',
      message: `解除「${entry.title || entry.memoryId}」与「${fileName}」的关联？\n原件仍保留在团队文件中，不影响其它条目。`,
      confirmText: '解除',
      danger: false,
    });
    if (!ok) return;
    const kbId = useEnclaveKbStore.getState().activeKbId;
    const err = await useEnclaveKbStore
      .getState()
      .detachEntryFile(activeOrgId, kbId, entry.memoryId, fileId);
    if (err) showToast(err, 'error');
    else showToast('已解除关联（原件仍保留）', 'success');
  };

  /** 删除文件原件：仍有任务 / 条目引用时后端拒绝，并把原因原样提示给用户 */
  const deleteAsset = async (file: KbFileListView) => {
    if (!activeOrgId) return;
    const ok = await confirmDialog({
      title: '删除文件原件',
      message: `确定删除「${file.fileName}」？\n只有完全没有任务挂载、也没有任何知识条目引用时才会真正删除，且不可恢复。`,
      confirmText: '删除',
    });
    if (!ok) return;
    const err = await useEnclaveKbStore.getState().deleteFile(activeOrgId, file.fileId);
    if (err) showToast(err, 'error');
    else showToast('文件已删除', 'success');
  };

  /** 清理无引用文件（删条目后留下的孤儿原件，回收磁盘） */
  const cleanupOrphans = async () => {
    if (!activeOrgId || orphans.length === 0) return;
    const ok = await confirmDialog({
      title: '清理无引用文件',
      message: `将删除 ${orphans.length} 个已无任何条目 / 任务引用的原件，释放磁盘空间。\n`
        + '「待整理」中的投喂文件仍在队列里，不会计入本次清理。此操作不可恢复。',
      confirmText: '清理',
    });
    if (!ok) return;
    const removed = await useEnclaveKbStore.getState().cleanupOrphans(activeOrgId);
    showToast(removed > 0 ? `已清理 ${removed} 个文件` : '没有可清理的文件', removed > 0 ? 'success' : 'info');
  };

  /** 跳回来源任务：先加载任务列表再选中，避免选中态被列表刷新覆盖。 */
  const jumpToTask = async () => {
    // 只有任务产出才有「来源任务」；投喂/手工/AI 条目的 source_ref 不是任务 id
    if (activeEntry?.origin !== 'task_output') return;
    const taskId = activeEntry.sourceRef;
    if (!activeOrgId || !taskId) return;
    await useEnclaveTaskStore.getState().loadTasks(activeOrgId);
    useEnclaveTaskStore.getState().selectTask(taskId);
    navigate('/enclave');
  };

  const dirty =
    !!activeEntry &&
    canEdit &&
    (draftTitle.trim() !== activeEntry.title ||
      draftContent !== activeEntry.content ||
      joinTags(draftTags) !== joinTags(splitTags(activeEntry.tags)) ||
      !sameFields(draftMeta, pickFields(activeEntry, kbFields)));

  /** 文件页目录树：按所选维度分组（系统维度 / 当前库的专属属性） */
  const fileGroups = useMemo(() => {
    const q = fileKeyword.trim().toLowerCase();
    const fieldDef =
      fileGroupBy.kind === 'field'
        ? kbFields.find((f) => f.key === fileGroupBy.key)
        : undefined;
    const map = new Map<
      string,
      { id: string; title: string; files: typeof files; latest: number }
    >();
    for (const f of files) {
      if (q && !f.fileName.toLowerCase().includes(q)) continue;
      let id: string;
      let title: string;
      if (fileGroupBy.kind === 'field') {
        ({ id, title } = fieldGroup(f, fileGroupBy.key, fieldDef?.type));
      } else {
        switch (fileGroupBy.kind) {
          case 'assignee':
            id = f.assigneeId || '__unassigned__';
            title = f.assigneeId ? memberName(f.assigneeId) : '未指派';
            break;
          case 'taskPriority': {
            const p = f.priority || 'normal';
            id = p;
            title = `任务优先级：${PRIORITY_LABEL[p] ?? p}`;
            break;
          }
          case 'filetype': {
            const ext = fileExt(f.fileName);
            id = ext || '__noext__';
            title = ext ? `${ext.toUpperCase()} 文件` : '无扩展名';
            break;
          }
          case 'month': {
            const d = new Date(f.addedAt * 1000);
            const m = d.getMonth() + 1;
            id = `${d.getFullYear()}-${String(m).padStart(2, '0')}`;
            title = `${d.getFullYear()} 年 ${m} 月`;
            break;
          }
          default:
            id = f.memoryId || f.sourceRef || '__orphan__';
            title = f.entryTitle || f.sourceRef || '未关联条目';
        }
      }
      let g = map.get(id);
      if (!g) {
        g = { id, title, files: [], latest: 0 };
        map.set(id, g);
      }
      // 换维度后同一份原件可能被多个条目引用而重复命中同一组，组内去重
      if (!g.files.some((x) => x.fileId === f.fileId)) g.files.push(f);
      g.latest = Math.max(g.latest, f.addedAt);
    }
    const groups = [...map.values()];
    groups.sort((a, b) => {
      if (fileGroupBy.kind === 'month') return b.id.localeCompare(a.id);
      if (fileGroupBy.kind === 'taskPriority') {
        return (PRIORITY_ORDER[a.id] ?? 9) - (PRIORITY_ORDER[b.id] ?? 9);
      }
      if (fileGroupBy.kind === 'field') {
        // 「未标注」永远排最后：它是缺口提示，不该抢占首屏
        if (a.id === '__empty__' || b.id === '__empty__') {
          return a.id === '__empty__' ? 1 : -1;
        }
        const type = fieldDef?.type ?? 'text';
        if (type === 'number') return Number(a.id) - Number(b.id);
        if (type === 'boolean') return a.id === 'true' ? -1 : 1;
        if (type === 'select') {
          const opts = fieldDef?.options ?? [];
          return opts.indexOf(a.title) - opts.indexOf(b.title);
        }
        if (type === 'date') return b.id.localeCompare(a.id);
        return b.files.length - a.files.length || b.latest - a.latest;
      }
      return b.latest - a.latest;
    });
    return groups;
  }, [files, fileKeyword, fileGroupBy, members, kbFields]);

  const matchedFileCount = useMemo(
    () => new Set(fileGroups.flatMap((g) => g.files.map((f) => f.fileId))).size,
    [fileGroups],
  );
  const totalFileCount = kb?.fileCount ?? new Set(files.map((f) => f.fileId)).size;

  const toggleGroup = (id: string) =>
    setCollapsedGroups((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-canvas)' }}>
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        rightPanelOpen={false}
        showBackButton={false}
        enclaveOpen
        onOpenEnclave={() => navigate('/enclave')}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenSettings={() => navigate('/settings')}
        onOpenMarket={() => navigate('/market')}
        onOpenInspirations={() => navigate('/inspirations')}
      />

      <div className="flex-1 min-h-0 px-2 flex flex-col overflow-hidden">
        <div
          className="shrink-0 rounded-lg overflow-hidden"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >
          <EnclaveNavBar
            active="kb"
            onRefresh={refreshKb}
            refreshing={refreshing || loading}
          />
        </div>

        <div className="flex-1 min-h-0 pt-2 flex flex-col overflow-hidden">
          <div
            className="flex-1 min-h-0 rounded-lg overflow-hidden flex flex-col"
            style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
          >

          {/* 知识库信息（owner / admin 可修改） */}
          <div
            className="shrink-0 px-4 pt-2.5 pb-2"
            style={{ borderBottom: '1px solid var(--border)' }}
          >
            {kbForm ? (
              <div className="flex flex-col gap-2">
                <div className="flex items-center gap-2">
                  <span className="w-14 shrink-0 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    {kbForm.mode === 'create' ? '新建库' : '名称'}
                  </span>
                  <input
                    className={FIELD_CLASS + ' flex-1'}
                    style={FIELD_STYLE}
                    value={kbForm.name}
                    onChange={(e) =>
                      setKbForm((f) => (f ? { ...f, name: e.target.value } : f))
                    }
                    placeholder="知识库名称"
                    autoFocus
                  />
                </div>
                <div className="flex items-start gap-2">
                  <span
                    className="w-14 shrink-0 text-[11px] leading-6"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    描述
                  </span>
                  <textarea
                    className="flex-1 p-2.5 rounded-lg text-xs leading-relaxed outline-none resize-y"
                    style={{ ...FIELD_STYLE, minHeight: 56 }}
                    rows={2}
                    value={kbForm.description}
                    onChange={(e) =>
                      setKbForm((f) => (f ? { ...f, description: e.target.value } : f))
                    }
                    placeholder="知识库描述（用途、收录范围等）"
                  />
                </div>
                <div className="flex items-start gap-2">
                  <span
                    className="w-14 shrink-0 text-[11px] leading-6"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    专属属性
                  </span>
                  <div className="flex-1 min-w-0">
                    <FieldSchemaEditor
                      fields={kbForm.fields}
                      onChange={(fields) => setKbForm((f) => (f ? { ...f, fields } : f))}
                    />
                    <div className="mt-1 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                      不同库可以有不同属性；模型整理与手工入库都按当前库的定义抽取。
                    </div>
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <span className="w-14 shrink-0" />
                  <button
                    onClick={() => void saveKb()}
                    className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                    style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
                  >
                    <Save size={11} />
                    {kbForm.mode === 'create' ? '创建' : '保存'}
                  </button>
                  <button
                    onClick={cancelKbForm}
                    className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                    style={{
                      color: 'var(--text-secondary)',
                      backgroundColor: 'var(--bg-field)',
                      border: '1px solid var(--border)',
                    }}
                  >
                    <X size={11} />
                    取消
                  </button>
                </div>
              </div>
            ) : (
              <>
                <div className="flex items-center gap-2 min-w-0">
                  <Library size={12} style={{ color: 'var(--accent)' }} />
                  <Select
                    value={kb?.id ?? ''}
                    onChange={(v) => {
                      if (activeOrgId && v !== activeKbId) {
                        void useEnclaveKbStore.getState().selectKb(activeOrgId, v);
                      }
                    }}
                    size="sm"
                    className="w-[240px] shrink-0"
                    options={kbs.map((k) => ({
                      value: k.id,
                      label: k.isImplicit ? `${k.name}（默认）` : k.name,
                    }))}
                  />
                  <span
                    className="text-[10px] min-w-0 truncate"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    {kb?.entryCount ?? entries.length} 条知识
                    {' · '}
                    {kb?.fileCount ?? files.length} 个文件
                    {kb && kb.lastEntryAt > 0 && ` · 最近更新 ${formatTime(kb.lastEntryAt)}`}
                  </span>
                  <div className="flex-1" />
                  {canEdit && (
                    <>
                      <button
                        onClick={startCreateKb}
                        className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] shrink-0"
                        style={{
                          color: 'var(--text-secondary)',
                          backgroundColor: 'var(--bg-field)',
                          border: '1px solid var(--border)',
                        }}
                        title="新建自定义库（owner / admin）"
                      >
                        <Plus size={11} />
                        新建库
                      </button>
                      <button
                        onClick={startEditKb}
                        className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] shrink-0"
                        style={{
                          color: 'var(--text-secondary)',
                          backgroundColor: 'var(--bg-field)',
                          border: '1px solid var(--border)',
                        }}
                        title="修改名称 / 描述 / 专属属性（owner / admin）"
                      >
                        <Pencil size={11} />
                        编辑定义
                      </button>
                      {kb && !kb.isImplicit && (
                        <button
                          onClick={() => void removeKb()}
                          className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] shrink-0"
                          style={{
                            color: 'var(--status-danger)',
                            backgroundColor: 'var(--bg-field)',
                            border: '1px solid var(--border)',
                          }}
                          title="删除这个自定义库（只有空库可删）"
                        >
                          <Trash2 size={11} />
                          删除
                        </button>
                      )}
                    </>
                  )}
                </div>
                <div
                  className="text-[11px] mt-1 truncate"
                  style={{ color: 'var(--text-secondary)' }}
                  title={kb?.description}
                >
                  {kb?.description ||
                    (kb?.isImplicit
                      ? '默认库：任务产出自动沉淀；团队共享（可改定义，不可删除）'
                      : '自定义库：投喂与手工入库的目标库')}
                </div>
              </>
            )}
          </div>

          {/* tab：知识 / 文件 / 投喂 / 图谱 */}
          <div className="shrink-0 px-3 flex items-center gap-1" style={{ borderBottom: '1px solid var(--border)' }}>
            {TABS.map((t) => {
              const active = tab === t.key;
              const Icon = t.icon;
              return (
                <button
                  key={t.key}
                  onClick={() => t.enabled && setTab(t.key)}
                  disabled={!t.enabled}
                  className="flex items-center gap-1 px-2.5 py-1.5 text-[11px] transition-colors"
                  style={{
                    color: active ? 'var(--accent)' : 'var(--text-secondary)',
                    borderBottom: active ? '2px solid var(--accent)' : '2px solid transparent',
                    fontWeight: active ? 600 : 500,
                    opacity: t.enabled ? 1 : 0.45,
                    cursor: t.enabled ? 'pointer' : 'not-allowed',
                  }}
                  title={t.enabled ? t.label : `${t.label}（下一阶段实现）`}
                >
                  <Icon size={11} />
                  {t.label}
                  {t.key === 'ingest' && pendingFileQueue.length > 0 && (
                    <span
                      className="text-[9px] px-1 rounded-full"
                      style={{
                        backgroundColor: 'var(--status-warning-bg)',
                        color: 'var(--status-warning)',
                      }}
                      title={`${pendingFileQueue.length} 个文件投喂待整理`}
                    >
                      {pendingFileQueue.length}
                    </span>
                  )}
                </button>
              );
            })}
          </div>

          {/* 投喂：内容先入库，整理按加工能力降级（没有模型时进「待整理」，不产出机械切片） */}
          {tab === 'ingest' && (
            <div className="flex-1 min-h-0 flex overflow-hidden">
              <div
                ref={ingestPanelRef}
                className="w-[400px] shrink-0 flex flex-col gap-2 p-3 overflow-y-auto"
                style={{ backgroundColor: 'var(--bg-side)' }}
              >
                {capability && (
                  <div
                    className="flex items-start gap-2 px-2.5 py-2 rounded-lg text-[11px]"
                    style={{
                      backgroundColor: modelBlocked
                        ? 'var(--status-warning-bg)'
                        : 'var(--bg-tertiary)',
                      color: modelBlocked ? 'var(--status-warning)' : 'var(--text-secondary)',
                      border: '1px solid var(--border)',
                    }}
                  >
                    {modelBlocked ? (
                      <AlertTriangle size={12} className="mt-0.5 shrink-0" />
                    ) : (
                      <Sparkles size={12} className="mt-0.5 shrink-0" />
                    )}
                    <div className="min-w-0 flex-1">
                      {modelBlocked ? (
                        <>
                          暂无可用模型：内容会先入库并进入「待整理」，不会生成机械切片；
                          主机配好本地模型或开启闸门后自动整理。
                        </>
                      ) : (
                        <>
                          加工能力：
                          {capability.canProcess === 'local' ? '本地模型' : '云端模型'}
                          {capability.model ? ` · ${capability.model}` : ''}
                          {capability.providerName ? `（${capability.providerName}）` : ''}
                        </>
                      )}
                      {modelBlocked && capability.reason && (
                        <div className="mt-0.5 text-[10px] opacity-80 break-all">
                          原因：{capability.reason}
                        </div>
                      )}
                    </div>
                  </div>
                )}

                {/* 投喂目标库：多库后必须让用户看到内容会落到哪个库 */}
                <div className="px-0.5 text-[10px] flex items-center gap-1 min-w-0" style={{ color: 'var(--text-tertiary)' }}>
                  <Library size={10} className="shrink-0" />
                  <span className="shrink-0">投喂到：</span>
                  <span className="truncate" style={{ color: 'var(--text-secondary)' }}>
                    {kb?.name ?? '-'}
                    {kb?.isImplicit ? '（默认库）' : ''}
                  </span>
                </div>

                <div
                  className="flex items-center gap-1 p-0.5 rounded-lg"
                  style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
                >
                  {([
                    { key: 'snippet', label: '片段', icon: Scissors },
                    { key: 'file', label: '文件', icon: FileUp },
                    { key: 'url', label: '网址', icon: Globe },
                    { key: 'ai', label: 'AI 生成', icon: Wand2 },
                  ] as const).map((m) => {
                    const on = ingestMode === m.key;
                    const blocked = m.key === 'ai' && aiBlocked;
                    const Icon = m.icon;
                    return (
                      <button
                        key={m.key}
                        onClick={() => setIngestMode(m.key)}
                        disabled={blocked}
                        title={
                          blocked
                            ? `AI 生成需要本机可用模型${localCap?.reason ? `：${localCap.reason}` : ''}`
                            : undefined
                        }
                        className="flex-1 flex items-center justify-center gap-1 px-2 py-1 rounded-md text-[11px]"
                        style={{
                          backgroundColor: on ? 'var(--bg-primary)' : 'transparent',
                          color: on ? 'var(--accent)' : 'var(--text-secondary)',
                          fontWeight: on ? 600 : 500,
                          border: 'none',
                          opacity: blocked ? 0.45 : 1,
                          cursor: blocked ? 'not-allowed' : 'pointer',
                        }}
                      >
                        <Icon size={11} />
                        {m.label}
                      </button>
                    );
                  })}
                </div>

                {ingestMode === 'snippet' ? (
                  <>
                    <div className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                      粘贴结论 / 摘录 / 规则。原文立即入库；由主机模型整理成规范知识（标题、标签、
                      专属属性），没有模型时先排队。
                    </div>
                    <input
                      value={snipTitle}
                      onChange={(e) => setSnipTitle(e.target.value)}
                      placeholder="标题（可留空，由模型生成）"
                      className={FIELD_CLASS}
                      style={FIELD_STYLE}
                    />
                    <textarea
                      value={snipText}
                      onChange={(e) => setSnipText(e.target.value)}
                      placeholder="把内容贴进来……"
                      className="flex-1 min-h-[160px] p-2.5 rounded-lg text-xs leading-relaxed outline-none resize-none"
                      style={FIELD_STYLE}
                    />
                  </>
                ) : ingestMode === 'file' ? (
                  <>
                    <div className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                      文件会先复制进主机团队文件根，再由模型整理成知识条目；没有模型时先排队。
                      支持多选：逐个提交，某个失败不影响其它文件。
                    </div>
                    {filePaths.length === 0 ? (
                      <div
                        className="flex-1 min-h-[140px] flex flex-col items-center justify-center gap-1.5 py-4 rounded-lg text-xs"
                        style={{
                          border: `1px ${dragOver ? 'solid' : 'dashed'} ${dragOver ? 'var(--accent)' : 'var(--border)'}`,
                          backgroundColor: dragOver ? 'var(--accent-light)' : 'var(--bg-tertiary)',
                          color: dragOver ? 'var(--accent)' : 'var(--text-tertiary)',
                        }}
                      >
                        <FileUp size={18} />
                        <div className="font-medium min-w-0 max-w-full truncate px-2">
                          {dragOver
                            ? `松开即加入清单${dragCount > 1 ? `（${dragCount} 个）` : ''}`
                            : '把文件拖到这里，或点击选择'}
                        </div>
                        <button
                          onClick={() => void pickFiles()}
                          disabled={picking || ingesting}
                          className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                          style={{
                            color: 'var(--text-secondary)',
                            backgroundColor: 'var(--bg-primary)',
                            border: '1px solid var(--border)',
                          }}
                        >
                          {picking ? <Loader2 size={11} className="animate-spin" /> : <FolderOpen size={11} />}
                          选择文件
                        </button>
                      </div>
                    ) : (
                      <>
                        <div
                          className="shrink-0 flex items-center justify-between text-[10px]"
                          style={{ color: 'var(--text-tertiary)' }}
                        >
                          <span>
                            {fileProgress
                              ? `处理中 ${fileProgress.done}/${fileProgress.total}`
                              : `待投喂 ${filePaths.length} 个`}
                          </span>
                          <button
                            className="pd-btn"
                            disabled={ingesting}
                            onClick={() => {
                              setFilePaths([]);
                              setFileOutcome({});
                            }}
                          >
                            清空
                          </button>
                        </div>
                        <div className="flex-1 min-h-0 overflow-y-auto space-y-1">
                          {filePaths.map((p) => {
                            const res = fileOutcome[p];
                            const name = p.split(/[\\/]/).pop() ?? p;
                            return (
                              <div
                                key={p}
                                className="flex items-center gap-2 px-2 py-1 rounded text-[11px]"
                                style={{
                                  backgroundColor: 'var(--bg-content)',
                                  border: '1px solid var(--border)',
                                }}
                              >
                                <FileText size={11} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
                                <span className="flex-1 min-w-0 truncate" title={p}>
                                  {name}
                                </span>
                                {res && (
                                  <span
                                    className="shrink-0 text-[10px]"
                                    style={{
                                      color:
                                        res.status === 'error'
                                          ? 'var(--status-danger)'
                                          : res.status === 'queued'
                                            ? 'var(--status-warning)'
                                            : 'var(--status-success, var(--accent))',
                                    }}
                                  >
                                    {res.status === 'error'
                                      ? '失败'
                                      : res.status === 'queued'
                                        ? '待整理'
                                        : `入库 ${res.message ?? ''}`}
                                  </span>
                                )}
                                <button
                                  disabled={ingesting}
                                  style={{ color: 'var(--text-tertiary)', flexShrink: 0 }}
                                  title="移出清单"
                                  onClick={() => setFilePaths((prev) => prev.filter((x) => x !== p))}
                                >
                                  <X size={10} />
                                </button>
                              </div>
                            );
                          })}
                        </div>
                        <div
                          className="shrink-0 flex items-center justify-center gap-1 py-2 rounded-lg text-[10px]"
                          style={{
                            border: `1px ${dragOver ? 'solid' : 'dashed'} ${dragOver ? 'var(--accent)' : 'var(--border)'}`,
                            backgroundColor: dragOver ? 'var(--accent-light)' : 'transparent',
                            color: dragOver ? 'var(--accent)' : 'var(--text-tertiary)',
                          }}
                        >
                          <FileUp size={12} />
                          {dragOver ? '松开即加入清单' : '继续拖入，或'}
                          <button
                            className="pd-btn"
                            onClick={() => void pickFiles()}
                            disabled={picking || ingesting}
                          >
                            选择文件
                          </button>
                        </div>
                      </>
                    )}
                  </>
                ) : ingestMode === 'ai' ? (
                  <>
                    <div className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                      输入主题或要求，由模型生成一条知识草稿。草稿先落「待确认」，核对后采纳才进库；
                      生成只发送主题，不携带团队数据。
                      <div className="mt-1" style={{ color: aiBlocked ? 'var(--status-warning)' : 'var(--text-secondary)' }}>
                        {aiBlocked
                          ? `当前无可用模型${localCap?.reason ? `（${localCap.reason}）` : ''}：请到「设置 › 知识库」配置模型，或先配置本地 Provider。`
                          : `将使用本机模型：${localCap?.model || localCap?.providerName || '-'}`}
                      </div>
                    </div>
                    <textarea
                      value={topic}
                      onChange={(e) => setTopic(e.target.value)}
                      placeholder="例如：整理一份团队周会纪要模板"
                      className="flex-1 min-h-[160px] p-2.5 rounded-lg text-xs leading-relaxed outline-none resize-none"
                      style={FIELD_STYLE}
                    />
                  </>
                ) : (
                  <>
                    <div className="text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
                      抓取正文后由主机模型整理（去噪、分节、补全标点，不改事实）。纯 JS 渲染的站点
                      可能抓不到内容；没有模型时先排队。
                    </div>
                    <input
                      value={ingestUrl}
                      onChange={(e) => setIngestUrl(e.target.value)}
                      placeholder="https://example.com/article"
                      className={FIELD_CLASS}
                      style={FIELD_STYLE}
                    />
                    <input
                      value={ingestUrlName}
                      onChange={(e) => setIngestUrlName(e.target.value)}
                      placeholder="文件名（可留空，由模型取）"
                      className={FIELD_CLASS}
                      style={FIELD_STYLE}
                    />
                  </>
                )}

                <button
                  onClick={() => void submitIngest()}
                  disabled={ingesting || !canSubmitIngest}
                  className="shrink-0 flex items-center justify-center gap-1.5 px-3 py-2 rounded-lg text-xs"
                  style={{
                    backgroundColor: 'var(--accent)',
                    color: '#fff',
                    border: 'none',
                    opacity: ingesting || !canSubmitIngest ? 0.5 : 1,
                  }}
                >
                  {ingesting ? (
                    <>
                      <Loader2 size={12} className="animate-spin" />
                      处理中…
                    </>
                  ) : (
                    <>
                      {ingestMode === 'ai' ? <Wand2 size={12} /> : <Sparkles size={12} />}
                      {ingestMode === 'ai'
                        ? '生成草稿'
                        : modelBlocked
                          ? '提交入库（待整理）'
                          : '提交整理'}
                      {ingestMode === 'file' && filePaths.length > 0 ? `（${filePaths.length}）` : ''}
                    </>
                  )}
                </button>
              </div>

              <div className="flex-1 min-w-0 flex flex-col overflow-hidden">
                {/* 待确认：模型产出先落这里，人工采纳才进组织库 */}
                {candidates.length > 0 && (
                  <div
                    className="shrink-0 flex flex-col overflow-hidden"
                    style={{ borderBottom: '1px solid var(--border)' }}
                  >
                    <div className="px-4 py-2 flex items-center gap-2">
                      <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                        待确认
                      </span>
                      <span
                        className="px-1.5 py-0.5 rounded-full text-[10px]"
                        style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
                      >
                        {candidates.length}
                      </span>
                      <span className="text-[10px] min-w-0 truncate" style={{ color: 'var(--text-tertiary)' }}>
                        模型产出，核对后采纳才进库
                      </span>
                    </div>
                    <div className="max-h-[220px] overflow-y-auto px-3 pb-2 space-y-1">
                      {candidates.map((c) => (
                        <div
                          key={c.id}
                          className="flex items-center gap-2 px-2 py-1.5 rounded text-[11px]"
                          style={{
                            backgroundColor: 'var(--bg-content)',
                            border: '1px solid var(--border)',
                          }}
                        >
                          <span
                            className="shrink-0 px-1 rounded text-[9px]"
                            style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
                          >
                            AI
                          </span>
                          <span
                            className="flex-1 min-w-0 truncate"
                            style={{ color: 'var(--text-primary)' }}
                            title={c.content.slice(0, 400)}
                          >
                            {c.title || '未命名草稿'}
                          </span>
                          <button
                            className="shrink-0 text-[10px]"
                            style={{ color: 'var(--accent)' }}
                            onClick={() => {
                              if (!activeOrgId || !activeKbId) return;
                              void useEnclaveKbStore
                                .getState()
                                .adoptCandidate(activeOrgId, activeKbId, c.id)
                                .then((ok) => ok && showToast('已采纳入库', 'success'));
                            }}
                          >
                            采纳
                          </button>
                          <button
                            className="shrink-0 text-[10px]"
                            style={{ color: 'var(--text-tertiary)' }}
                            onClick={() => {
                              if (!activeOrgId || !activeKbId) return;
                              void useEnclaveKbStore
                                .getState()
                                .rejectCandidate(activeOrgId, activeKbId, c.id);
                            }}
                          >
                            拒绝
                          </button>
                        </div>
                      ))}
                    </div>
                  </div>
                )}

                <div
                  className="shrink-0 px-4 py-2 flex items-center gap-2"
                  style={{ borderBottom: '1px solid var(--border)' }}
                >
                  <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                    待整理
                  </span>
                  <span
                    className="px-1.5 py-0.5 rounded-full text-[10px]"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
                  >
                    {queue.length}
                  </span>
                  <span className="text-[10px] min-w-0 truncate" style={{ color: 'var(--text-tertiary)' }}>
                    原文已入库，主机模型可用后自动转成知识条目
                  </span>
                  <div className="flex-1" />
                  <button
                    onClick={() => {
                      if (!activeOrgId) return;
                      void useEnclaveKbStore.getState().runQueue(activeOrgId).then((o) => {
                        if (o?.stopped) showToast('模型暂不可用，内容仍保留在待整理里', 'info');
                        else if (o && o.done > 0) showToast(`已整理 ${o.done} 条`, 'success');
                      });
                    }}
                    disabled={modelBlocked || queue.length === 0}
                    className="text-[10px] px-2 py-0.5 rounded"
                    style={{
                      backgroundColor: 'var(--bg-tertiary)',
                      color: 'var(--text-secondary)',
                      border: '1px solid var(--border)',
                      opacity: modelBlocked || queue.length === 0 ? 0.5 : 1,
                    }}
                  >
                    立即整理
                  </button>
                </div>
                <div className="flex-1 overflow-y-auto p-3 space-y-1.5">
                  {queue.length === 0 ? (
                    <div
                      className="h-full flex flex-col items-center justify-center gap-2 text-xs"
                      style={{ color: 'var(--text-tertiary)' }}
                    >
                      <Check size={18} />
                      没有待整理的内容
                    </div>
                  ) : (
                    queue.map((item) => (
                      <div key={item.id}>
                        <div
                          className="flex items-center gap-2 px-2 py-1.5 rounded text-[11px]"
                          style={{
                            backgroundColor: 'var(--bg-content)',
                            border: '1px solid var(--border)',
                          }}
                        >
                          <span
                            className="shrink-0 px-1 rounded text-[9px]"
                            style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
                          >
                            {QUEUE_KIND_LABEL[item.kind] ?? item.kind}
                          </span>
                          <span
                            className="flex-1 min-w-0 truncate"
                            style={{ color: 'var(--text-primary)' }}
                            title={queueTitle(item)}
                          >
                            {queueTitle(item)}
                          </span>
                          {item.status === 'failed' ? (
                            <span
                              className="shrink-0 text-[10px]"
                              style={{ color: 'var(--status-danger)' }}
                              title={item.lastError}
                            >
                              失败{item.attempts > 1 ? ` ×${item.attempts}` : ''}
                            </span>
                          ) : (
                            <span className="shrink-0 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                              {item.status === 'running' ? '整理中' : '等待模型'}
                            </span>
                          )}
                          {canManual && (
                            <button
                              className="shrink-0 text-[10px]"
                              style={{ color: 'var(--text-secondary)' }}
                              title="不经模型：先把原文提取到表单，确认后再写入组织库（owner / admin）"
                              onClick={() => {
                                if (manualDraft?.id === item.id) setManualDraft(null);
                                else void openManualDraft(item);
                              }}
                            >
                              {manualDraft?.id === item.id ? '收起' : '手工入库'}
                            </button>
                          )}
                          <button
                            className="shrink-0 text-[10px]"
                            style={{ color: 'var(--accent)' }}
                            onClick={() => {
                              if (activeOrgId) void useEnclaveKbStore.getState().retryQueueItem(activeOrgId, item.id);
                            }}
                          >
                            重试
                          </button>
                          <button
                            className="shrink-0"
                            style={{ color: 'var(--text-tertiary)' }}
                            title="移出队列"
                            onClick={() => {
                              if (activeOrgId) void useEnclaveKbStore.getState().dropQueueItem(activeOrgId, item.id);
                            }}
                          >
                            <Trash2 size={10} />
                          </button>
                        </div>

                        {manualDraft?.id === item.id && (
                          <div
                            className="mt-1 p-2 rounded-lg space-y-1.5"
                            style={{
                              // 表单本身是一张卡（与上方队列行同档），
                              // 输入框用凹槽档 —— 两者同档时输入框会"消失"在表单里
                              backgroundColor: 'var(--bg-content)',
                              border: '1px solid var(--border)',
                            }}
                          >
                            <input
                              value={manualDraft.title}
                              onChange={(e) =>
                                setManualDraft((d) => (d ? { ...d, title: e.target.value } : d))
                              }
                              placeholder="标题（可留空，自动取正文开头）"
                              className={FIELD_CLASS}
                              style={FIELD_STYLE}
                              disabled={manualDraft.loading || manualDraft.saving}
                            />
                            <textarea
                              value={manualDraft.content}
                              onChange={(e) =>
                                setManualDraft((d) => (d ? { ...d, content: e.target.value } : d))
                              }
                              placeholder={manualDraft.loading ? '正在提取原文…' : '正文'}
                              rows={8}
                              className="w-full p-2 rounded-lg text-[11px] leading-relaxed outline-none resize-y"
                              style={FIELD_STYLE}
                              disabled={manualDraft.loading || manualDraft.saving}
                            />
                            <div>
                              <div
                                className="text-[10px] mb-1"
                                style={{ color: 'var(--text-secondary)' }}
                              >
                                标签（用于检索与图谱连线）
                              </div>
                              <TagEditor
                                tags={manualDraft.tags}
                                onChange={(tags) =>
                                  setManualDraft((d) => (d ? { ...d, tags } : d))
                                }
                              />
                            </div>
                            {manualDraft.notice && (
                              <div
                                className="text-[10px] leading-relaxed"
                                style={{ color: 'var(--status-warning)' }}
                              >
                                没能自动提取：{manualDraft.notice}。可直接手工填写后提交。
                              </div>
                            )}
                            {manualDraft.error && (
                              <div className="text-[10px]" style={{ color: 'var(--status-danger)' }}>
                                {manualDraft.error}
                              </div>
                            )}
                            <div className="flex items-center gap-2">
                              <span className="flex-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                                确认后写入组织库，之后仍可人工编辑
                              </span>
                              <button
                                onClick={() => setManualDraft(null)}
                                disabled={manualDraft.saving}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  color: 'var(--text-secondary)',
                                  backgroundColor: 'var(--bg-field)',
                                  border: '1px solid var(--border)',
                                }}
                              >
                                取消
                              </button>
                              <button
                                onClick={() => void submitManualDraft()}
                                disabled={manualDraft.loading || manualDraft.saving}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  backgroundColor: 'var(--accent)',
                                  color: '#fff',
                                  border: 'none',
                                  opacity: manualDraft.loading || manualDraft.saving ? 0.6 : 1,
                                }}
                              >
                                {manualDraft.saving ? '入库中…' : '确认入库'}
                              </button>
                            </div>
                          </div>
                        )}
                      </div>
                    ))
                  )}
                </div>
              </div>
            </div>
          )}

          {/* 知识：条目列表（展开行即详情） */}
          {tab === 'entries' && (
            <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
              <div
                className="shrink-0 px-4 py-1.5 flex items-center gap-2"
                style={{ borderBottom: '1px solid var(--border)' }}
              >
                <SearchField
                  value={keyword}
                  onChange={setKeyword}
                  onEnter={() => void runSearch()}
                  onClear={() => {
                    setKeyword('');
                    void runSearch('');
                  }}
                  placeholder="搜索标题 / 正文 / 标签 / 文件（回车）"
                />
                <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                  {entries.length} 条{query && ` · 匹配「${query}」`}
                </span>
                <div className="flex-1" />
                <span className="text-[10px] shrink-0 whitespace-nowrap" style={{ color: 'var(--text-tertiary)' }}>
                  汇报确认后自动沉淀 · 点击条目展开
                </span>
              </div>

              <div className="flex-1 overflow-y-auto">
                {entries.length === 0 && (
                  <div
                    className="h-full flex flex-col items-center justify-center gap-2 text-xs"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    <Search size={18} />
                    {query ? `没有匹配「${query}」的条目` : '还没有条目：任务汇报「确认完成」后会自动沉淀到这里'}
                  </div>
                )}

                {entries.map((e) => {
                  const open = activeEntry?.memoryId === e.memoryId;
                  const tags = splitTags(e.tags);
                  const taskTitle = metaStr(e, 'taskTitle');
                  const confirmedAt = metaNum(e, 'confirmedAt');
                  return (
                    <div
                      key={e.memoryId}
                      className="px-4 py-2.5 transition-colors"
                      style={{
                        borderBottom: '1px solid var(--border)',
                        backgroundColor: open ? 'var(--bg-secondary)' : 'transparent',
                      }}
                    >
                      <div className="flex items-start gap-2">
                        <button
                          onClick={() => toggleEntry(e)}
                          className="shrink-0 mt-0.5"
                          title={open ? '收起' : '展开'}
                        >
                          <ChevronRight
                            size={12}
                            className="transition-transform"
                            style={{
                              color: 'var(--text-tertiary)',
                              transform: open ? 'rotate(90deg)' : 'none',
                            }}
                          />
                        </button>
                        <div className="flex-1 min-w-0">
                          <div
                            className="flex items-center gap-1.5 min-w-0"
                            style={{ cursor: open && canEdit ? 'default' : 'pointer' }}
                            onClick={() => { if (!(open && canEdit)) toggleEntry(e); }}
                          >
                            {open && canEdit ? (
                              <input
                                className={FIELD_CLASS + ' flex-1'}
                                style={FIELD_STYLE}
                                value={draftTitle}
                                onChange={(ev) => setDraftTitle(ev.target.value)}
                                onClick={(ev) => ev.stopPropagation()}
                                placeholder="条目标题"
                              />
                            ) : (
                              <span
                                className="text-xs font-medium truncate"
                                style={{ color: 'var(--text-primary)' }}
                                title={e.title}
                              >
                                {e.title}
                              </span>
                            )}
                            <span
                              className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                              style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
                              title={`来源：${entryOriginLabel(e)}`}
                            >
                              {entryOriginLabel(e)}
                            </span>
                            {e.stale && (
                              <span
                                className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                                style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                                title="来源任务已恢复汇报，等待重新确认"
                              >
                                待更新
                              </span>
                            )}
                            {metaNum(e, 'sourceTaskCancelledAt') > 0 && (
                              <span
                                className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
                                title="来源任务已作废：该条目为作废前的产出版本"
                              >
                                来源已作废
                              </span>
                            )}
                            {e.files.length > 0 && (
                              <span
                                className="shrink-0 flex items-center gap-0.5 text-[9px]"
                                style={{ color: 'var(--text-tertiary)' }}
                                title={`${e.files.length} 个文件`}
                              >
                                <Paperclip size={9} />
                                {e.files.length}
                              </span>
                            )}
                          </div>

                          {!open && (
                            <div className="text-[11px] mt-1 line-clamp-2" style={{ color: 'var(--text-secondary)' }}>
                              {e.content || '（无正文）'}
                            </div>
                          )}

                          {!open && (
                            <div className="flex items-center gap-1.5 mt-1.5 flex-wrap">
                              {tags.map((t) => (
                                <span
                                  key={t}
                                  className="px-1.5 py-[1px] rounded text-[9px]"
                                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                                >
                                  #{t}
                                </span>
                              ))}
                              <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                                更新于 {formatRelative(e.updatedAt)}
                              </span>
                            </div>
                          )}
                        </div>
                      </div>

                      {/* 展开：溯源 + 正文 / 标签 / 交付物 */}
                      {open && (
                        <div className="mt-2 ml-5 space-y-3">
                          {e.stale && (
                            <div
                              className="text-[11px] px-2 py-1 rounded flex items-center gap-1"
                              style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
                            >
                              <AlertTriangle size={11} />
                              来源任务已恢复汇报：当前内容为上次确认版本，等待重新确认后更新
                            </div>
                          )}

                          {/* 作废任务的知识条目保留（与删除同口径），但要明确提示产出来源已终止 */}
                          {metaNum(e, 'sourceTaskCancelledAt') > 0 && (
                            <div
                              className="text-[11px] px-2 py-1 rounded flex items-center gap-1"
                              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                              title={`标记于 ${formatTime(metaNum(e, 'sourceTaskCancelledAt'))}`}
                            >
                              <AlertTriangle size={11} />
                              来源任务已作废：这是作废前沉淀的产出版本，内容未必仍然有效
                            </div>
                          )}

                          <div
                            className="text-[10px] flex items-center gap-3 flex-wrap"
                            style={{ color: 'var(--text-tertiary)' }}
                          >
                            {e.origin === 'task_output' ? (
                              <>
                                {/* 任务删除后条目会脱钩（sourceRef 置空），来源降级为历史信息 */}
                                <span>
                                  {e.sourceRef
                                    ? `任务「${taskTitle || '已删除或不可见'}」`
                                    : `来源任务「${taskTitle || '未知'}」（已删除）`}
                                </span>
                                {confirmedAt > 0 && (
                                  <span>
                                    确认人 {memberName(metaStr(e, 'confirmedBy')) || '-'} · 确认于 {formatTime(confirmedAt)}
                                  </span>
                                )}
                                <span>第 {metaNum(e, 'revision') || 1} 次确认</span>
                              </>
                            ) : (
                              // 投喂 / 手工 / AI 条目没有来源任务，只标来源类型
                              <span>来源 {entryOriginLabel(e)}</span>
                            )}
                            <span>首次沉淀 {formatTime(e.addedAt)}</span>
                            <span>最近更新 {formatTime(e.updatedAt)}</span>
                            {metaStr(e, 'editedBy') && (
                              <span>
                                人工编辑 {memberName(metaStr(e, 'editedBy'))} · {formatTime(metaNum(e, 'editedAt'))}
                              </span>
                            )}
                          </div>

                          <div>
                            <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                              正文{canEdit ? '（可编辑）' : ''}
                            </div>
                            {canEdit ? (
                              <textarea
                                value={draftContent}
                                onChange={(ev) => setDraftContent(ev.target.value)}
                                rows={Math.min(18, Math.max(6, draftContent.split('\n').length))}
                                className="w-full p-3 rounded-lg text-xs leading-relaxed outline-none resize-y"
                                style={{
                                  backgroundColor: 'var(--bg-field)',
                                  color: 'var(--text-primary)',
                                  border: '1px solid var(--border)',
                                }}
                                placeholder="知识正文"
                              />
                            ) : (
                              <div
                                className="text-xs whitespace-pre-wrap leading-6"
                                style={{ color: 'var(--text-secondary)' }}
                              >
                                {e.content || '（无正文）'}
                              </div>
                            )}
                          </div>

                          <div>
                            <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                              标签（用于检索）
                            </div>
                            {canEdit ? (
                              <TagEditor tags={draftTags} onChange={setDraftTags} />
                            ) : (
                              <div className="flex items-center gap-1.5 flex-wrap">
                                {tags.length === 0 && (
                                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>-</span>
                                )}
                                {tags.map((t) => (
                                  <span
                                    key={t}
                                    className="px-1.5 py-[1px] rounded text-[10px]"
                                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                                  >
                                    #{t}
                                  </span>
                                ))}
                              </div>
                            )}
                          </div>

                          {kbFields.length > 0 && (
                            <div>
                              <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                                专属属性{canEdit ? '（可编辑）' : ''}
                              </div>
                              {canEdit ? (
                                <MetaEditor
                                  fields={kbFields}
                                  value={draftMeta}
                                  onChange={setDraftMeta}
                                />
                              ) : (
                                <div className="grid grid-cols-2 gap-x-3 gap-y-1">
                                  {kbFields.map((f) => {
                                    const v = e.metadata?.[f.key];
                                    const text =
                                      typeof v === 'string' || typeof v === 'number'
                                        ? String(v)
                                        : typeof v === 'boolean'
                                          ? v
                                            ? '是'
                                            : '否'
                                          : '-';
                                    return (
                                      <div
                                        key={f.key}
                                        className="flex items-baseline gap-1.5 min-w-0 text-[11px]"
                                      >
                                        <span className="shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                                          {f.label || f.key}
                                        </span>
                                        <span className="truncate" style={{ color: 'var(--text-secondary)' }}>
                                          {text}
                                        </span>
                                      </div>
                                    );
                                  })}
                                </div>
                              )}
                            </div>
                          )}

                          <div>
                            <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                              {e.origin === 'task_output' ? '交付物' : '来源文件'}（{e.files.length}）
                            </div>
                            <div
                              className="rounded-md p-2 flex flex-col gap-1"
                              style={{ border: '1px dashed var(--border)' }}
                            >
                              {e.files.length === 0 && (
                                <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                                  {e.origin === 'task_output' ? '该条目没有交付物' : '该条目没有关联原文'}
                                </span>
                              )}
                              {e.files.map((f) => (
                                <div key={f.fileId} className="flex items-center gap-2">
                                  <button
                                    onClick={() => void openFile(f.fileId)}
                                    className="attachment-row flex-1 min-w-0 text-left flex items-center gap-2 px-1 py-1"
                                    title="打开文件"
                                  >
                                    <FileText size={11} className="shrink-0" style={{ color: 'var(--accent)' }} />
                                    <span
                                      className="text-[11px] truncate flex-1"
                                      style={{ color: 'var(--text-secondary)' }}
                                    >
                                      {f.fileName}
                                    </span>
                                    <span
                                      className="text-[10px] shrink-0"
                                      style={{ color: 'var(--text-tertiary)' }}
                                    >
                                      {formatSize(f.fileSize)}
                                    </span>
                                    <CacheHintIcon visible={myRole !== '' && myRole !== 'owner'} />
                                  </button>
                                  {canEdit && e.origin !== 'task_output' && (
                                    <button
                                      onClick={() => void detachEntryFile(e, f.fileId, f.fileName)}
                                      className="shrink-0 text-[10px] px-1"
                                      style={{ color: 'var(--text-tertiary)' }}
                                      title="解除这条知识与该文件的关联（原件保留；任务产出的交付物不提供此操作）"
                                    >
                                      解除
                                    </button>
                                  )}
                                </div>
                              ))}
                            </div>
                          </div>

                          <div className="flex items-center gap-2 flex-wrap">
                            {canEdit && (
                              <button
                                onClick={() => void saveEntry()}
                                disabled={saving || !dirty}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  backgroundColor: 'var(--accent)',
                                  color: '#fff',
                                  border: 'none',
                                  opacity: saving || !dirty ? 0.5 : 1,
                                  cursor: saving || !dirty ? 'not-allowed' : 'pointer',
                                }}
                                title={dirty ? '保存标题 / 正文 / 标签' : '没有改动'}
                              >
                                <Save size={11} />
                                {saving ? '保存中…' : dirty ? '保存' : '已保存'}
                              </button>
                            )}
                            {canEdit && (
                              <button
                                onClick={cancelEdit}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  color: 'var(--text-secondary)',
                                  backgroundColor: 'var(--bg-field)',
                                  border: '1px solid var(--border)',
                                }}
                                title="放弃未保存的改动并收起（不必滚回顶部）"
                              >
                                <X size={11} />
                                取消
                              </button>
                            )}
                            {e.origin === 'task_output' && e.sourceRef && (
                              <button
                                onClick={() => void jumpToTask()}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  color: 'var(--text-secondary)',
                                  backgroundColor: 'var(--bg-field)',
                                  border: '1px solid var(--border)',
                                }}
                                title="跳转到产出该知识的任务"
                              >
                                <ArrowUpRight size={11} />
                                查看来源任务
                              </button>
                            )}
                            {canEdit && (
                              <button
                                onClick={() => void removeEntry()}
                                className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                                style={{
                                  color: 'var(--status-danger)',
                                  backgroundColor: 'var(--bg-field)',
                                  border: '1px solid var(--border)',
                                }}
                                title="从团队知识库移除（交付物原件仍保留在任务中）"
                              >
                                <Trash2 size={11} />
                                移除
                              </button>
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {/* 文件：知识库全部原文（任务交付物 + 投喂文件） */}
          {tab === 'files' && (
            <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
              <div
                className="shrink-0 px-4 py-1.5 flex items-center gap-2"
                style={{ borderBottom: '1px solid var(--border)' }}
              >
                <div className="w-[200px] shrink-0">
                  <Select
                    value={encodeGroupBy(fileGroupBy)}
                    onChange={(v) => setFileGroupBy(decodeGroupBy(v))}
                    size="sm"
                    className="w-full"
                    options={[
                      { value: 'entry', label: '按来源条目' },
                      { value: 'assignee', label: '按任务负责人' },
                      { value: 'taskPriority', label: '按任务优先级' },
                      { value: 'filetype', label: '按文件类型' },
                      { value: 'month', label: '按归档月份' },
                      // 当前库的专属属性：跟着库定义走，换库后选项自然变化
                      ...kbFields.map((f) => ({
                        value: `field:${f.key}`,
                        label: `按属性 · ${f.label || f.key}`,
                      })),
                    ]}
                    title="切换文件分组方式"
                  />
                </div>
                <SearchField
                  value={fileKeyword}
                  onChange={setFileKeyword}
                  onClear={() => setFileKeyword('')}
                  placeholder="搜索文件名"
                />
                <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                  {fileKeyword.trim()
                    ? `匹配 ${matchedFileCount} / ${totalFileCount} 个文件`
                    : `共 ${totalFileCount} 个文件`}
                </span>
                {pendingFileQueue.length > 0 && (
                  <button
                    onClick={() => setTab('ingest')}
                    className="pd-btn shrink-0 flex items-center gap-1 px-2 py-1 rounded-lg text-[10px]"
                    style={{
                      color: 'var(--status-warning)',
                      backgroundColor: 'var(--status-warning-bg)',
                      border: '1px solid var(--border)',
                    }}
                    title="这些文件已归档但还没整理出知识条目，仍在「待整理」队列中：不会计入「清理无引用」，点此去投喂页查看"
                  >
                    <AlertTriangle size={10} />
                    待整理 {pendingFileQueue.length} 个
                  </button>
                )}
                {canEdit && orphans.length > 0 && (
                  <button
                    onClick={() => void cleanupOrphans()}
                    className="pd-btn shrink-0 flex items-center gap-1 px-2 py-1 rounded-lg text-[10px]"
                    style={{
                      color: 'var(--status-danger)',
                      backgroundColor: 'var(--bg-field)',
                      border: '1px solid var(--border)',
                    }}
                    title="删除已无任何条目 / 任务引用的原件，回收磁盘（待整理中的投喂文件、仍被队列引用的文件不会计入）"
                  >
                    <Trash2 size={10} />
                    清理无引用（{orphans.length}）
                  </button>
                )}
                <div className="flex-1" />
                <span className="text-[10px] shrink-0 whitespace-nowrap" style={{ color: 'var(--text-tertiary)' }}>
                  任务交付物与投喂文件 · 打开即查看
                </span>
              </div>

              <div className="flex-1 overflow-y-auto">
                {files.length === 0 && (
                  <div
                    className="h-full flex flex-col items-center justify-center gap-2 text-xs"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    <Files size={18} />
                    还没有文件：任务完成的交付物、投喂的文件都会归档到这里
                  </div>
                )}
                {files.length > 0 && fileGroups.length === 0 && (
                  <div
                    className="flex flex-col items-center justify-center gap-2 py-10 text-xs"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    <Search size={18} />
                    没有匹配「{fileKeyword.trim()}」的文件
                  </div>
                )}

                {fileGroups.map((g) => {
                  // 搜索时强制展开，避免命中文件被折叠目录藏起来
                  const collapsed = !fileKeyword.trim() && collapsedGroups.includes(g.id);
                  return (
                    <div key={g.id}>
                      <button
                        onClick={() => toggleGroup(g.id)}
                        className="w-full px-4 py-1.5 flex items-center gap-1.5 text-left transition-colors"
                        style={{
                          borderBottom: '1px solid var(--border)',
                          backgroundColor: 'var(--bg-tertiary)',
                        }}
                        title={collapsed ? '展开这个目录' : '折叠这个目录'}
                      >
                        <ChevronRight
                          size={11}
                          className="shrink-0 transition-transform"
                          style={{
                            color: 'var(--text-tertiary)',
                            transform: collapsed ? 'none' : 'rotate(90deg)',
                          }}
                        />
                        <Folder size={11} className="shrink-0" style={{ color: 'var(--accent)' }} />
                        <span
                          className="text-[11px] font-medium truncate min-w-0"
                          style={{ color: 'var(--text-primary)' }}
                        >
                          {g.title}
                        </span>
                        <span className="shrink-0 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                          {new Set(g.files.map((f) => f.fileId)).size} 个文件
                        </span>
                      </button>

                      {!collapsed &&
                        g.files.map((f) => (
                          <div
                            key={`${g.id}:${f.fileId}`}
                            className="py-2 pr-4 flex items-center gap-3"
                            style={{
                              borderBottom: '1px solid var(--border)',
                              marginLeft: 22,
                              borderLeft: '1px solid var(--border)',
                              paddingLeft: 16,
                            }}
                          >
                            <FileText size={13} className="shrink-0" style={{ color: 'var(--accent)' }} />
                            <div className="flex-1 min-w-0">
                              <div
                                className="text-xs truncate"
                                style={{ color: 'var(--text-primary)' }}
                                title={f.fileName}
                              >
                                {f.fileName}
                              </div>
                              <div className="text-[10px] mt-0.5 truncate" style={{ color: 'var(--text-tertiary)' }}>
                                {fileGroupBy.kind !== 'entry' && (
                                  <>来源「{f.entryTitle || f.sourceRef || '未知'}」 · </>
                                )}
                                {f.fileSize > 0 && `${formatSize(f.fileSize)} · `}
                                {formatRelative(f.addedAt)}归档
                              </div>
                            </div>
                            <CacheHintIcon visible={myRole !== '' && myRole !== 'owner'} />
                            <button
                              onClick={() => void openFile(f.fileId)}
                              className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px] shrink-0"
                              style={{
                                color: 'var(--text-secondary)',
                                backgroundColor: 'var(--bg-field)',
                                border: '1px solid var(--border)',
                              }}
                              title="打开文件"
                            >
                              <FolderOpen size={11} />
                              打开
                            </button>
                            {canEdit && (
                              <button
                                onClick={() => void deleteAsset(f)}
                                className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] shrink-0"
                                style={{
                                  color: 'var(--status-danger)',
                                  backgroundColor: 'var(--bg-field)',
                                  border: '1px solid var(--border)',
                                }}
                                title="删除原件（仍有任务 / 条目引用时会被拒绝）"
                              >
                                <Trash2 size={11} />
                                删除
                              </button>
                            )}
                          </div>
                        ))}
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {/* 图谱：团队条目的关系图（条目 / 文件 / 共标签 / 附件），复用个人库的 3D 渲染 */}
          {tab === 'graph' && (
            <KnowledgeGraph base={graphBase} entries={NO_ENTRIES} source={graphSource} />
          )}

          {error && (
            <div className="shrink-0 px-3 py-1 text-[11px]" style={{ color: 'var(--status-danger)', borderTop: '1px solid var(--border)' }}>
              {error}
            </div>
          )}
          </div>
        </div>
      </div>

      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
