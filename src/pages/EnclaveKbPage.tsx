import { useCallback, useEffect, useMemo, useState } from 'react';
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
  AlertTriangle,
  ArrowUpRight,
  ChevronRight,
  Files,
  Sparkles,
  Network,
  FolderOpen,
  Folder,
  Pencil,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { TitleBar, StatusBar } from '../components/layout';
import { TagEditor } from '../components/knowledge/TagEditor';
import { Select } from '../components/common/Select';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useEnclaveKbStore } from '../stores/enclaveKbStore';
import { useEnclaveTaskStore } from '../stores/enclaveTaskStore';
import { useAccountStore } from '../stores/accountStore';
import { EnclaveNavBar } from '../components/enclave/EnclaveNavBar';
import type { KbEntryView } from '../types/enclave';

/** 表单控件口径：与个人知识库一致（参见 KnowledgeBaseDialog） */
const FIELD_CLASS = 'w-full px-2.5 py-1.5 rounded-lg text-xs outline-none';
const FIELD_STYLE = {
  backgroundColor: 'var(--bg-tertiary)',
  color: 'var(--text-primary)',
  border: '1px solid var(--border)',
} as const;

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
  { key: 'ingest', label: '投喂', icon: Sparkles, enabled: false },
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
