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
  FileUp,
  Wand2,
  Scissors,
  Globe,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { TitleBar, StatusBar } from '../components/layout';
import { TagEditor } from '../components/knowledge/TagEditor';
import { Select } from '../components/common/Select';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useEnclaveKbStore } from '../stores/enclaveKbStore';
import { useEnclaveTaskStore } from '../stores/enclaveTaskStore';
import { useAccountStore } from '../stores/accountStore';
import { EnclaveNavBar } from '../components/enclave/EnclaveNavBar';
import { showToast } from '../utils/toast';
import type { EnclaveKbQueueItem, KbEntryView } from '../types/enclave';
import type { IngestCapability } from '../types/knowledge';

/** 表单控件口径：与个人知识库一致（参见 KnowledgeBaseDialog） */
const FIELD_CLASS = 'w-full px-2.5 py-1.5 rounded-lg text-xs outline-none';
const FIELD_STYLE = {
  backgroundColor: 'var(--bg-tertiary)',
  color: 'var(--text-primary)',
  border: '1px solid var(--border)',
} as const;

/** 待整理条目的展示名：从 payload 里取原始输入（不额外存冗余字段） */
function queueTitle(item: EnclaveKbQueueItem): string {
  try {
    const p = JSON.parse(item.payload) as Record<string, unknown>;
    if (item.kind === 'url') return String(p.url ?? item.sourceRef);
    const text = String(p.title ?? p.text ?? '');
    return text.slice(0, 40) || '片段';
  } catch {
    return item.sourceRef;
  }
}

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

const ORIGIN_LABEL: Record<string, string> = {
  task_output: '任务产出',
  manual: '手工添加',
};

type KbTab = 'entries' | 'files' | 'ingest' | 'graph';

/** 文件目录树的分组维度：全部基于现有数据，不需要额外录入属性 */
type FileGroupBy = 'entry' | 'assignee' | 'priority' | 'filetype' | 'month';

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

const TABS: { key: KbTab; label: string; icon: typeof Search; enabled: boolean }[] = [
  { key: 'entries', label: '知识', icon: Search, enabled: true },
  { key: 'files', label: '文件', icon: Files, enabled: true },
  { key: 'ingest', label: '投喂', icon: Sparkles, enabled: true },
  { key: 'graph', label: '图谱', icon: Network, enabled: false },
];

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
  const members = useEnclaveOrgStore((s) => s.detail?.members ?? []);

  const kbs = useEnclaveKbStore((s) => s.kbs);
  const activeKbId = useEnclaveKbStore((s) => s.activeKbId);
  const entries = useEnclaveKbStore((s) => s.entries);
  const activeEntry = useEnclaveKbStore((s) => s.activeEntry);
  const files = useEnclaveKbStore((s) => s.files);
  const query = useEnclaveKbStore((s) => s.query);
  const loading = useEnclaveKbStore((s) => s.loading);
  const error = useEnclaveKbStore((s) => s.error);

  const [tab, setTab] = useState<KbTab>('entries');
  const [keyword, setKeyword] = useState('');
  const [fileKeyword, setFileKeyword] = useState('');
  /** 文件目录树的分组维度 */
  const [fileGroupBy, setFileGroupBy] = useState<FileGroupBy>('entry');
  /** 文件目录树里被折叠的分组 */
  const [collapsedGroups, setCollapsedGroups] = useState<string[]>([]);
  // 知识库信息编辑（owner / admin）
  const [editingKb, setEditingKb] = useState(false);
  const [draftKbName, setDraftKbName] = useState('');
  const [draftKbDesc, setDraftKbDesc] = useState('');
  // 展开行的草稿：正文 / 标签 / 标题一起保存（与个人知识库一致）
  const [draftTitle, setDraftTitle] = useState('');
  const [draftContent, setDraftContent] = useState('');
  const [draftTags, setDraftTags] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  /* ── 投喂（人工入库）：内容照收，整理按加工能力降级 ── */
  const capability = useEnclaveKbStore((s) => s.capability);
  const queue = useEnclaveKbStore((s) => s.queue);
  const candidates = useEnclaveKbStore((s) => s.candidates);
  const [ingestMode, setIngestMode] = useState<'snippet' | 'file' | 'url' | 'ai'>('snippet');
  const [snipTitle, setSnipTitle] = useState('');
  const [snipText, setSnipText] = useState('');
  const [ingestUrl, setIngestUrl] = useState('');
  const [ingestUrlName, setIngestUrlName] = useState('');
  const [filePath, setFilePath] = useState('');
  const [topic, setTopic] = useState('');
  const [picking, setPicking] = useState(false);
  const [ingesting, setIngesting] = useState(false);
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
        ? Boolean(filePath)
        : ingestMode === 'url'
          ? Boolean(ingestUrl.trim())
          : Boolean(topic.trim()) && !aiBlocked;

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
              setFilePath(p.paths[0]);
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
      const out =
        ingestMode === 'snippet'
          ? await st.ingestSnippet(activeOrgId, activeKbId, snipTitle.trim(), snipText)
          : ingestMode === 'file'
            ? await st.ingestFile(activeOrgId, activeKbId, filePath)
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
      setFilePath('');
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
  /** 单一默认库：直接取列表里唯一的一项 */
  const kb = kbs[0] ?? null;

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
  }, [activeEntry]);

  // 切换组织 / 账号：退出信息编辑态
  useEffect(() => {
    setEditingKb(false);
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

  // 切换分组维度后旧的折叠状态不再适用
  useEffect(() => {
    setCollapsedGroups([]);
  }, [fileGroupBy]);

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
    try {
      const path = await invoke<string>('enclave_file_abs_path', { orgId: activeOrgId, fileId });
      await invoke('open_path', { path });
    } catch {
      // 单个文件打不开不影响列表；错误由 store 统一提示
    }
  };

  const startEditKb = () => {
    if (!kb) return;
    setDraftKbName(kb.name);
    setDraftKbDesc(kb.description);
    setEditingKb(true);
  };

  const saveKb = async () => {
    if (!activeOrgId || !kb) return;
    const ok = await useEnclaveKbStore
      .getState()
      .updateKb(activeOrgId, kb.id, draftKbName, draftKbDesc);
    if (ok) setEditingKb(false);
  };

  const cancelEditKb = () => {
    setDraftKbName(kb?.name ?? '');
    setDraftKbDesc(kb?.description ?? '');
    setEditingKb(false);
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
    }
    useEnclaveKbStore.getState().openEntry(null);
  };

  const removeEntry = async () => {
    if (!activeOrgId || !activeEntry) return;
    if (!window.confirm('从团队知识库移除该条目？任务中的交付物原件不会被删除。')) return;
    const kbId = useEnclaveKbStore.getState().activeKbId;
    await useEnclaveKbStore.getState().removeEntry(activeOrgId, kbId, activeEntry.memoryId);
  };

  /** 跳回来源任务：先加载任务列表再选中，避免选中态被列表刷新覆盖。 */
  const jumpToTask = async () => {
    const taskId = activeEntry?.sourceRef;
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
      joinTags(draftTags) !== joinTags(splitTags(activeEntry.tags)));

  /** 文件页目录树：按所选维度分组（默认按知识条目 / 来源任务） */
  const fileGroups = useMemo(() => {
    const q = fileKeyword.trim().toLowerCase();
    const map = new Map<
      string,
      { id: string; title: string; files: typeof files; latest: number }
    >();
    for (const f of files) {
      if (q && !f.fileName.toLowerCase().includes(q)) continue;
      let id: string;
      let title: string;
      switch (fileGroupBy) {
        case 'assignee':
          id = f.assigneeId || '__unassigned__';
          title = f.assigneeId ? memberName(f.assigneeId) : '未指派';
          break;
        case 'priority': {
          const p = f.priority || 'normal';
          id = p;
          title = `优先级：${PRIORITY_LABEL[p] ?? p}`;
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
      if (fileGroupBy === 'month') return b.id.localeCompare(a.id);
      if (fileGroupBy === 'priority') {
        return (PRIORITY_ORDER[a.id] ?? 9) - (PRIORITY_ORDER[b.id] ?? 9);
      }
      return b.latest - a.latest;
    });
    return groups;
  }, [files, fileKeyword, fileGroupBy, members]);

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
            {editingKb ? (
              <div className="flex flex-col gap-2">
                <div className="flex items-center gap-2">
                  <span className="w-14 shrink-0 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    名称
                  </span>
                  <input
                    className={FIELD_CLASS + ' flex-1'}
                    style={FIELD_STYLE}
                    value={draftKbName}
                    onChange={(e) => setDraftKbName(e.target.value)}
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
                    value={draftKbDesc}
                    onChange={(e) => setDraftKbDesc(e.target.value)}
                    placeholder="知识库描述（用途、收录范围等）"
                  />
                </div>
                <div className="flex items-center gap-2">
                  <span className="w-14 shrink-0" />
                  <button
                    onClick={() => void saveKb()}
                    className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                    style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
                  >
                    <Save size={11} />
                    保存
                  </button>
                  <button
                    onClick={cancelEditKb}
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
                  <span className="text-sm font-medium truncate min-w-0" style={{ color: 'var(--text-primary)' }}>
                    {kb?.name ?? '团队知识库'}
                  </span>
                  <span
                    className="text-[10px] shrink-0 whitespace-nowrap"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    {kb?.entryCount ?? entries.length} 条知识
                    {' · '}
                    {kb?.fileCount ?? files.length} 个文件
                    {kb && kb.lastEntryAt > 0 && ` · 最近更新 ${formatTime(kb.lastEntryAt)}`}
                  </span>
                  <div className="flex-1" />
                  {canEdit && (
                    <button
                      onClick={startEditKb}
                      className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] shrink-0"
                      style={{
                        color: 'var(--text-secondary)',
                        backgroundColor: 'var(--bg-field)',
                        border: '1px solid var(--border)',
                      }}
                      title="修改知识库名称与描述（owner / admin）"
                    >
                      <Pencil size={11} />
                      编辑信息
                    </button>
                  )}
                </div>
                <div
                  className="text-[11px] mt-1 truncate"
                  style={{ color: 'var(--text-secondary)' }}
                  title={kb?.description}
                >
                  {kb?.description || '任务产出自动沉淀；团队共享'}
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
                      主机本机直接读路径，成员设备经局域网上传（单文件有大小上限）。
                    </div>
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
                          ? dragCount > 1
                            ? `松开即投喂 ${dragCount} 个文件（一次取第一个）`
                            : '松开即投喂'
                          : filePath
                            ? filePath.split(/[\\/]/).pop()
                            : '把文件拖到这里，或点击选择'}
                      </div>
                      <button
                        onClick={async () => {
                          setPicking(true);
                          try {
                            const picked = await openDialog({
                              multiple: false,
                              title: '选择要投喂到团队知识库的文件',
                            });
                            if (typeof picked === 'string') setFilePath(picked);
                          } finally {
                            setPicking(false);
                          }
                        }}
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
                      <div
                        key={item.id}
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
                          {item.kind === 'snippet' ? '片段' : item.kind === 'url' ? '网址' : item.kind}
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
                        {item.kind === 'snippet' && (
                          <button
                            className="shrink-0 text-[10px]"
                            style={{ color: 'var(--text-secondary)' }}
                            title="不经模型，按原文直接写入组织库"
                            onClick={() => {
                              if (activeOrgId) {
                                void useEnclaveKbStore.getState().manualIngest(activeOrgId, item.id);
                              }
                            }}
                          >
                            手工入库
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
                  placeholder="搜索标题 / 正文 / 标签 / 交付物（回车）"
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
                              title={`来源：${ORIGIN_LABEL[e.origin] ?? e.origin}`}
                            >
                              {ORIGIN_LABEL[e.origin] ?? e.origin}
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
                            {e.files.length > 0 && (
                              <span
                                className="shrink-0 flex items-center gap-0.5 text-[9px]"
                                style={{ color: 'var(--text-tertiary)' }}
                                title={`${e.files.length} 个交付物`}
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

                          <div
                            className="text-[10px] flex items-center gap-3 flex-wrap"
                            style={{ color: 'var(--text-tertiary)' }}
                          >
                            <span>任务「{taskTitle || e.sourceRef}」</span>
                            {confirmedAt > 0 && (
                              <span>
                                确认人 {memberName(metaStr(e, 'confirmedBy')) || '-'} · 确认于 {formatTime(confirmedAt)}
                              </span>
                            )}
                            <span>第 {metaNum(e, 'revision') || 1} 次确认</span>
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

                          <div>
                            <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                              交付物（{e.files.length}）
                            </div>
                            <div
                              className="rounded-md p-2 flex flex-col gap-1"
                              style={{ border: '1px dashed var(--border)' }}
                            >
                              {e.files.length === 0 && (
                                <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                                  该条目没有交付物
                                </span>
                              )}
                              {e.files.map((f) => (
                                <button
                                  key={f.fileId}
                                  onClick={() => void openFile(f.fileId)}
                                  className="attachment-row w-full text-left flex items-center gap-2 px-1 py-1"
                                  title="打开文件"
                                >
                                  <FileText size={11} style={{ color: 'var(--accent)' }} />
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
                                </button>
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
                            {e.sourceRef && (
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

          {/* 文件：知识库全部交付物（参照个人知识库的文件页） */}
          {tab === 'files' && (
            <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
              <div
                className="shrink-0 px-4 py-1.5 flex items-center gap-2"
                style={{ borderBottom: '1px solid var(--border)' }}
              >
                <div className="w-[150px] shrink-0">
                  <Select
                    value={fileGroupBy}
                    onChange={(v) => setFileGroupBy(v as FileGroupBy)}
                    size="sm"
                    className="w-full"
                    options={[
                      { value: 'entry', label: '按来源任务' },
                      { value: 'assignee', label: '按负责人' },
                      { value: 'priority', label: '按优先级' },
                      { value: 'filetype', label: '按文件类型' },
                      { value: 'month', label: '按归档月份' },
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
                <div className="flex-1" />
                <span className="text-[10px] shrink-0 whitespace-nowrap" style={{ color: 'var(--text-tertiary)' }}>
                  与任务交付物是同一份原件 · 打开即查看
                </span>
              </div>

              <div className="flex-1 overflow-y-auto">
                {files.length === 0 && (
                  <div
                    className="h-full flex flex-col items-center justify-center gap-2 text-xs"
                    style={{ color: 'var(--text-tertiary)' }}
                  >
                    <Files size={18} />
                    还没有文件：任务汇报「确认完成」时上传的交付物会归档到这里
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
                                {fileGroupBy !== 'entry' && (
                                  <>来源任务「{f.entryTitle || f.sourceRef || '未知'}」 · </>
                                )}
                                {f.fileSize > 0 && `${formatSize(f.fileSize)} · `}
                                {formatRelative(f.addedAt)}归档
                              </div>
                            </div>
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
                          </div>
                        ))}
                    </div>
                  );
                })}
              </div>
            </div>
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
