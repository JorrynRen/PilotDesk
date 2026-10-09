/**
 * KnowledgeFileList — 文件知识列表
 *
 * 文件与条目是两层：原文（本地路径或网络地址）只存一份，正文抽取后切成条目进检索；
 * 多个库引用同一份文件，不重复拷贝 —— 所以这里显示「被几个库引用」，而不是归属某一个库。
 *
 * 文件之间还可以有关系（公文类：正文—附件；制度类：新版—旧版）：
 * 附件挂在正文下面显示，新旧版本互相标注 —— 但**只标注、不替换**，两版都留在库里、都参与检索。
 *
 * 支持批量导出（分享 / 资产拷贝）：复制到目录，或打包成一个 zip。
 * 导出**不覆盖**目标里已有的同名文件，重名加 `-1`/`-2` 后缀。
 */

import { useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import {
  FileText, Globe, FolderOpen, Loader2, AlertCircle, CheckCircle2, Layers, FileQuestion, Search,
  Download, ChevronDown, ChevronRight, ChevronsUp, ChevronsDown, Package, AlertTriangle, Link2, CornerDownRight, X, Trash2, Folder,
} from 'lucide-react';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import { confirmDialog } from '../../stores/confirmStore';
import { Select } from '../common/Select';
import type {
  KnowledgeBase,
  KnowledgeFile,
  KnowledgeFileAttr,
  KnowledgeFileRelation,
  KnowledgeFileRelationKind,
} from '../../types/knowledge';

const STATUS_META: Record<KnowledgeFile['extractStatus'], { label: string; color: string; icon: typeof FileText }> = {
  pending: { label: '待抽取', color: '#F59E0B', icon: Loader2 },
  extracting: { label: '抽取中', color: '#3B82F6', icon: Loader2 },
  done: { label: '已抽取', color: '#10B981', icon: CheckCircle2 },
  // 原文已登记，但该格式暂不支持抽正文（内容不进检索）——是明确的边界，不是失败
  unsupported: { label: '未抽取（格式暂不支持）', color: '#9CA3AF', icon: FileQuestion },
  failed: { label: '抽取失败', color: '#EF4444', icon: AlertCircle },
};

interface KbExportOutcome {
  dest: string;
  exported: number;
  renamed: number;
  missing: number;
  failed: string[];
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 相对时间（秒级时间戳）：AI 主导维护下，"属性是什么时候被刷的"必须看得见 */
function fmtAgo(sec: number): string {
  const diff = Math.floor(Date.now() / 1000) - sec;
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}

/** 文件关系的索引（一次算好给渲染用），以及「正文 → 附件」的展示顺序 */
interface FileTree {
  /** 附件 → 它的正文 */
  parentOf: Map<string, string>;
  /** 正文 → 它的附件 */
  childrenOf: Map<string, string[]>;
  /** 旧版 → 替代它的新版 */
  supersededBy: Map<string, string>;
  /** 新版 → 它替代的旧版（可以有多个：新版合并了几份旧制度） */
  superseded: Map<string, string[]>;
  /** 列表顺序：正文在前，它的附件缩进跟在其后 */
  rows: FileRow[];
}

/**
 * 列表的一行：要么是一个目录（按属性分组时的组头），要么是一个文件。
 *
 * 两种分组方式产出同一种行类型，渲染只写一遍：目录行自带缩进基准，文件行用 `depth` 缩进
 * （关系树里附件 = 1；属性目录下文件 = 1，都在目录下；根级散文件 = 0）。
 */
type FileRow =
  | { kind: 'group'; label: string; depth: number; count: number }
  | {
      kind: 'file';
      file: KnowledgeFile;
      depth: number;
      /**
       * **额外**缩进层数（同一 `depth` 里再往右几级）。
       *
       * 为什么不直接加到 `depth` 上：`depth` 在属性目录模式下是"在不在目录里"的开关
       * （折叠、左侧竖线、根级散文件的判断全用它，见渲染处），把附件的层级塞进去会连带
       * 把"根级散文件"和"目录下的附件"混成同一档。两个概念分开，各自只有一个含义。
       */
      indent: number;
      /** 该文件在这个属性上取值不唯一（会出现在多个目录 = 虚拟重复） */
      multi?: boolean;
      /** 所属目录名（按属性分组时才有）—— 多值时同一文件在多处出现，渲染的 key 要靠它区分 */
      groupLabel?: string;
    };

/**
 * 在一个"桶"里按文件关系排：正文在前、它的附件缩进跟在其后。
 *
 * **为什么分组之后还要排关系**：附件本来就是"挂在正文下面"的东西，按属性分目录会把它打平，
 * 从属关系就完全看不出来了（用户反馈过）。分组与关系是两件事，不该互相抹掉。
 *
 * **父不在本桶里时，附件就当根处理**：否则会出现"缩进在一个看不见的正文下面"。
 * 这与 `buildFileTree` 对"被筛剩的附件"的口径一致。
 */
function treeRows(
  bucket: KnowledgeFile[],
  parentOf: Map<string, string>,
  childrenOf: Map<string, string[]>,
  baseDepth: number,
  opts?: { groupLabel?: string; isMulti?: (f: KnowledgeFile) => boolean },
): FileRow[] {
  const byId = new Map(bucket.map((f) => [f.id, f]));
  const out: FileRow[] = [];
  const seen = new Set<string>();
  const walk = (f: KnowledgeFile, level: number) => {
    if (seen.has(f.id)) return;
    seen.add(f.id);
    out.push({
      kind: 'file',
      file: f,
      depth: baseDepth,
      indent: level,
      groupLabel: opts?.groupLabel,
      multi: opts?.isMulti?.(f),
    });
    for (const cid of childrenOf.get(f.id) ?? []) {
      const c = byId.get(cid);
      if (c) walk(c, level + 1);
    }
  };
  // 根 = 没有正文、或正文不在本桶里的；随后兜底扫一遍防止异常数据（成环）漏显
  for (const f of bucket) if (!parentOf.has(f.id) || !byId.has(parentOf.get(f.id) as string)) walk(f, 0);
  for (const f of bucket) walk(f, 0);
  return out;
}

/** 按某个专属属性把文件组织成目录树：目录在前、根级散文件在后（资源管理器习惯） */
function buildAttrGroups(
  files: KnowledgeFile[],
  field: string,
  fileAttrs: Record<string, KnowledgeFileAttr>,
  parentOf: Map<string, string>,
  childrenOf: Map<string, string[]>,
): FileRow[] {
  const byName = (a: KnowledgeFile, b: KnowledgeFile) => a.name.localeCompare(b.name, 'zh');
  const folders = new Map<string, KnowledgeFile[]>();
  /** 该属性一个取值都没有的文件 —— 没有子目录，直接留在根 */
  const loose: KnowledgeFile[] = [];
  /** 该文件在这个属性上是否多值（多值 = 会出现在多个目录里 = 虚拟重复） */
  const isMulti = (f: KnowledgeFile) => (fileAttrs[f.path]?.values[field]?.length ?? 0) > 1;

  for (const f of files) {
    const values = fileAttrs[f.path]?.values[field] ?? [];
    if (values.length === 0) {
      loose.push(f);
      continue;
    }
    // 有多个取值 → 每个取值对应的目录里都放一份（虚拟重复：同一份文件，不是 N 份）
    for (const v of values) folders.set(v, [...(folders.get(v) ?? []), f]);
  }

  const rows: FileRow[] = [];
  for (const label of [...folders.keys()].sort((a, b) => a.localeCompare(b, 'zh'))) {
    const items = (folders.get(label) ?? []).slice().sort(byName);
    rows.push({ kind: 'group', label, depth: 0, count: items.length });
    // 目录内的直属文件 depth 仍为 1（折叠、左侧竖线都按它判断），附件的额外层级走 `indent`
    rows.push(...treeRows(items, parentOf, childrenOf, 1, { groupLabel: label, isMulti }));
  }
  rows.push(...treeRows(loose.slice().sort(byName), parentOf, childrenOf, 0, { isMulti }));
  return rows;
}

/**
 * 各库折叠起来的目录（`{库id: [目录名]}`），落 localStorage。
 *
 * 单开一个 key 装全部库、而不是每库一个 key：切库时不需要重读（读不到的库就是"全展开"），
 * 也不会把上一个库的折叠集误用到新库上。
 */
const COLLAPSED_KEY = 'kb-file-groups-collapsed';

/** 各库选过的「分组方式」（`{库id: 字段key}`，空串 = 文件关系），同样落 localStorage */
const GROUP_KEY = 'kb-file-group-by';

/** 读一个 `{库id: 值}` 形状的偏好；坏数据一律当"没记过" */
function readPerBase<T>(key: string): Record<string, T> {
  try {
    const raw = localStorage.getItem(key);
    const parsed = raw ? (JSON.parse(raw) as unknown) : null;
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, T>) : {};
  } catch {
    return {}; // 读不出来就用默认（全展开 / 文件关系），不值得因此让文件页打不开
  }
}

/** 写 `{库id: 值}` 形状的偏好；写不进去只是记不住，不影响使用 */
function writePerBase<T>(key: string, value: Record<string, T>) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* 忽略：记不住偏好不影响使用 */
  }
}

function buildFileTree(files: KnowledgeFile[], relations: KnowledgeFileRelation[]): FileTree {
  const byId = new Map(files.map((f) => [f.id, f]));
  const parentOf = new Map<string, string>();
  const childrenOf = new Map<string, string[]>();
  const supersededBy = new Map<string, string>();
  const superseded = new Map<string, string[]>();

  for (const r of relations) {
    // 只认两端都在本库的关系：跨库那一侧由对面库的列表表达
    if (!byId.has(r.fromFileId) || !byId.has(r.toFileId)) continue;
    if (r.kind === 'attachment') {
      parentOf.set(r.toFileId, r.fromFileId);
      childrenOf.set(r.fromFileId, [...(childrenOf.get(r.fromFileId) ?? []), r.toFileId]);
    } else {
      supersededBy.set(r.toFileId, r.fromFileId);
      superseded.set(r.fromFileId, [...(superseded.get(r.fromFileId) ?? []), r.toFileId]);
    }
  }

  const rows: FileRow[] = [];
  const seen = new Set<string>();
  const walk = (id: string, depth: number) => {
    const file = byId.get(id);
    if (!file || seen.has(id)) return;
    seen.add(id);
    rows.push({ kind: 'file', file, depth, indent: 0 });
    for (const child of childrenOf.get(id) ?? []) walk(child, depth + 1);
  };
  // 顶层 = 没有正文的文件（正文自身、独立文件）；随后兜底扫一遍防止异常数据漏显
  for (const f of files) if (!parentOf.has(f.id)) walk(f.id, 0);
  for (const f of files) walk(f.id, 0);

  return { parentOf, childrenOf, supersededBy, superseded, rows };
}

interface KnowledgeFileListProps {
  base: KnowledgeBase;
  files: KnowledgeFile[];
  /** 文件级专属属性（按 `path` 索引）：「按属性分目录」用；缺省则只有文件关系一种分组 */
  fileAttrs: Record<string, KnowledgeFileAttr>;
}

export function KnowledgeFileList({ base, files, fileAttrs }: KnowledgeFileListProps) {
  /**
   * 分组方式（'' = 文件关系；否则是「按这个专属属性分目录」），**按库记住**。
   *
   * 为什么按库记而不是全局记一个字段名：可分目录的字段是**各库自己在库定义里定的**
   * （A 库的「部门」与 B 库的「部门」是两回事，B 库还未必有），所以"用哪个字段"天然属于库级偏好。
   * 与折叠状态同一套存法（一个 key 装全部库）。
   */
  const [groupByBase, setGroupByBase] = useState<Record<string, string>>(
    () => readPerBase<string>(GROUP_KEY),
  );
  const groupField = groupByBase[base.id] ?? '';
  const setGroupField = (field: string) =>
    setGroupByBase((prev) => {
      const next = { ...prev, [base.id]: field };
      writePerBase(GROUP_KEY, next);
      return next;
    });
  /** 各库折叠起来的目录（默认全展开，只有用户手动折叠过才记） */
  const [collapsedByBase, setCollapsedByBase] = useState<Record<string, string[]>>(
    () => readPerBase<string[]>(COLLAPSED_KEY),
  );
  const [selected, setSelected] = useState<string[]>([]);
  const [menuOpen, setMenuOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failures, setFailures] = useState<string[]>([]);
  // AI 整理（打标 + 填专属属性）：按文件逐个跑，进度由 store 统一维护
  const enriching = useKnowledgeStore((s) => s.enriching);
  const enrichFile = useKnowledgeStore((s) => s.enrichFile);
  const relations = useKnowledgeStore((s) => s.relations);
  const linkFiles = useKnowledgeStore((s) => s.linkFiles);
  const unlinkFiles = useKnowledgeStore((s) => s.unlinkFiles);
  const removeFiles = useKnowledgeStore((s) => s.removeFiles);
  /** 展开「关系」面板的文件 id（一次只开一个，面板就在该行下面） */
  const [relPanelId, setRelPanelId] = useState<string | null>(null);
  /** 文件搜索（本地过滤，不落库）：文件名 / 来源地址。按**内容**检索在「知识」页，那里按条目搜。 */
  const [query, setQuery] = useState('');

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return files;
    return files.filter((f) => f.name.toLowerCase().includes(q) || f.url.toLowerCase().includes(q));
  }, [files, query]);

  // 关系索引 + 展示顺序（正文在前、附件缩进跟在其后）。
  // 传 `filtered` 而不是 `files`：关系两端有一端被筛掉时 `buildFileTree` 会跳过那条关系，
  // 被筛剩的附件自然落到顶层 —— 不会出现"附件挂在一个看不见的正文下面"。
  const tree = useMemo(() => buildFileTree(filtered, relations), [filtered, relations]);

  /**
   * 可用来分目录的专属属性：只开放**取值有限**的字段（单选 / 是否）。
   * 文本、数字、日期会产生成百上千个目录，树反而比平铺更难用。
   */
  const groupableFields = useMemo(
    () => base.fields.filter((f) => f.type === 'select' || f.type === 'boolean'),
    [base.fields],
  );
  // 分组方式变了（或该字段被从库定义里删掉）就回落到"文件关系"，不留在空分组上
  const activeGroup = groupableFields.some((f) => f.key === groupField) ? groupField : '';
  /** 渲染用的行：按属性分目录，或按文件关系（默认）—— 两者产出同一种行类型 */
  const rows = useMemo(
    () =>
      activeGroup
        ? buildAttrGroups(filtered, activeGroup, fileAttrs, tree.parentOf, tree.childrenOf)
        : tree.rows,
    [activeGroup, filtered, fileAttrs, tree.parentOf, tree.childrenOf, tree.rows],
  );

  /** 折叠起来的目录（只对当前库生效；没记过就是空 = 全展开） */
  const collapsed = useMemo(
    () => new Set(collapsedByBase[base.id] ?? []),
    [collapsedByBase, base.id],
  );

  /** 折叠状态写入（同时落 localStorage；写不进去只是记不住，不影响使用） */
  const persistCollapsed = (updater: (cur: string[]) => string[]) =>
    setCollapsedByBase((prev) => {
      const next = { ...prev, [base.id]: updater(prev[base.id] ?? []) };
      writePerBase(COLLAPSED_KEY, next);
      return next;
    });

  const toggleCollapse = (label: string) =>
    persistCollapsed((cur) => (cur.includes(label) ? cur.filter((x) => x !== label) : [...cur, label]));

  /** 当前树上的全部目录名（「全部折叠/展开」按它来做；关系树模式下为空） */
  const folderLabels = useMemo(
    () => rows.flatMap((r) => (r.kind === 'group' ? [r.label] : [])),
    [rows],
  );
  /** 已经全折叠（一个个手动折叠完也算）—— 决定按钮是"折叠"还是"展开" */
  const allCollapsed = folderLabels.length > 0 && folderLabels.every((l) => collapsed.has(l));
  const toggleAllCollapsed = () => persistCollapsed(() => (allCollapsed ? [] : folderLabels));

  /**
   * 折叠后的可见行：目录行永远可见；它下面的文件行（depth = 1）在该目录被折叠时跳过。
   * 依赖 `buildAttrGroups` 的输出顺序 —— 目录行紧跟着自己的文件行（根级散文件 depth = 0，不受影响）。
   */
  const visibleRows = useMemo(() => {
    if (activeGroup === '' || collapsed.size === 0) return rows;
    const out: FileRow[] = [];
    let skipChildren = false;
    for (const row of rows) {
      if (row.kind === 'group') {
        skipChildren = collapsed.has(row.label);
        out.push(row);
        continue;
      }
      if (skipChildren && row.depth > 0) continue;
      out.push(row);
    }
    return out;
  }, [rows, collapsed, activeGroup]);

  const nameOf = (id: string) => files.find((f) => f.id === id)?.name ?? '（已不在本库）';

  const selectedSet = useMemo(() => new Set(selected), [selected]);
  /** 全选按**当前筛选结果**算，与「知识」页同一口径 */
  const allSelected = filtered.length > 0 && filtered.every((f) => selectedSet.has(f.id));
  const filteredOut = files.length - filtered.length;

  const toggle = (id: string) =>
    setSelected((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));

  const toggleAll = () => setSelected(allSelected ? [] : filtered.map((f) => f.id));

  /** 从库中移除选中的文件（**只解关联，不删库定义**） */
  const handleRemoveSelected = async () => {
    if (selected.length === 0) return;
    const ok = await confirmDialog({
      title: '从库中移除文件',
      message:
        `将从本库移除 ${selected.length} 个文件，并删掉它们在本库的条目。`
        + '不再被任何库引用的文件会连磁盘原文一起删除；仍被其它库引用的会保留。'
        + '库定义与其它内容不受影响。',
      confirmText: '移除',
    });
    if (!ok) return;
    const out = await removeFiles(base.id, selected);
    if (out) {
      setSelected([]);
      setRelPanelId(null);
    }
  };

  /** 导出：`ids` 为空 = 全部；`asZip` = 打包成一个压缩包 */
  const doExport = async (ids: string[], asZip: boolean) => {
    setMenuOpen(false);
    // 先问导出位置；取消或报错就直接结束，不往下走
    let dest: string;
    try {
      if (asZip) {
        const picked = await saveDialog({
          title: '导出为压缩包',
          defaultPath: `${base.name}.zip`,
          filters: [{ name: 'ZIP 压缩包', extensions: ['zip'] }],
        });
        if (typeof picked !== 'string' || !picked) return;
        dest = picked;
      } else {
        const picked = await openDialog({ directory: true, title: '选择导出目录' });
        if (typeof picked !== 'string' || !picked) return;
        dest = picked;
      }
    } catch (e) {
      showToast(`选择导出位置失败: ${errorMessage(e)}`, 'error');
      return;
    }

    setBusy(true);
    setFailures([]);
    try {
      const out = await invoke<KbExportOutcome>('kb_export_files', {
        kbId: base.id,
        fileIds: ids,
        dest,
        asZip,
      });
      const extra = [
        out.renamed > 0 ? `${out.renamed} 个重名已加后缀` : '',
        out.missing > 0 ? `${out.missing} 个原文缺失已跳过` : '',
      ].filter(Boolean);
      if (out.failed.length > 0) {
        setFailures(out.failed);
        showToast(`已导出 ${out.exported} 个，${out.failed.length} 个失败`, 'error');
      } else {
        showToast(
          `已导出 ${out.exported} 个文件${extra.length ? `（${extra.join('；')}）` : ''}`,
          'success',
        );
      }
    } catch (e) {
      showToast(`导出失败: ${errorMessage(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  if (files.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-2 text-xs" style={{ color: 'var(--text-tertiary)' }}>
        <Layers size={18} />
        这个库还没有文件知识：去「投喂」拖入文件或粘一个网址
      </div>
    );
  }

  const exportTargets = selected.length > 0 ? selected : [];

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* 工具栏：搜索 + 选择 + 批量导出 */}
      <div className="shrink-0 px-4 py-2 flex items-center gap-2" style={{ borderBottom: '1px solid var(--border)' }}>
        {/* `pd-field` 把聚焦反馈交给容器：全局 `:focus-visible` 的 1px 描边优先级高于 Tailwind 的
            `outline-none`，不加这个类的话描边会落在内层 input 上 —— 看着像"边框在外、焦点圈在内"。 */}
        <div
          className="pd-field flex items-center gap-1.5 px-2 py-1 rounded-lg flex-1 min-w-0"
          style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)', maxWidth: 260 }}
        >
          <Search size={12} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索文件名 / 地址"
            className="flex-1 min-w-0 bg-transparent outline-none text-xs"
            style={{ color: 'var(--text-primary)' }}
          />
          {query && (
            <button
              onClick={() => setQuery('')}
              className="shrink-0 flex items-center"
              style={{ color: 'var(--text-tertiary)' }}
              title="清空搜索"
            >
              <X size={11} />
            </button>
          )}
        </div>
        {groupableFields.length > 0 && (
          <div className="w-40 shrink-0" title="按专属属性把文件显示成目录结构（只列出取值有限的单选 / 是否字段）">
            <Select
              value={activeGroup}
              onChange={setGroupField}
              options={[
                { value: '', label: '分组：文件关系' },
                ...groupableFields.map((f) => ({ value: f.key, label: `按「${f.label}」分目录` })),
              ]}
              size="sm"
              className="w-full"
            />
          </div>
        )}
        {/* 目录树的全折叠 / 全展开（只有"按属性分目录"时才有目录可折） */}
        {folderLabels.length > 0 && (
          <button
            onClick={toggleAllCollapsed}
            className="pd-btn flex items-center justify-center shrink-0 rounded-lg"
            style={{
              width: 26,
              height: 26,
              color: 'var(--text-secondary)',
              backgroundColor: 'var(--bg-field)',
              border: '1px solid var(--border)',
            }}
            title={allCollapsed ? `全部展开（${folderLabels.length} 个目录）` : `全部折叠（${folderLabels.length} 个目录）`}
          >
            {allCollapsed ? <ChevronsDown size={13} /> : <ChevronsUp size={13} />}
          </button>
        )}
        <label
          className="flex items-center gap-1.5 text-[11px] cursor-pointer select-none shrink-0"
          style={{ color: 'var(--text-secondary)' }}
          title="全选当前搜索结果，用于批量导出 / 从库中移除"
        >
          <input type="checkbox" checked={allSelected} onChange={toggleAll} />
          全选
        </label>
        <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
          {selected.length > 0
            ? `已选 ${selected.length} / ${filtered.length}`
            : filteredOut > 0 ? `匹配 ${filtered.length} / ${files.length} 个文件` : `共 ${files.length} 个文件`}
        </span>
        {enriching && (
          <span className="flex items-center gap-1 text-[10px]" style={{ color: 'var(--accent)' }}>
            <Loader2 size={9} className="animate-spin" />
            AI 补整理中 {enriching.index}/{enriching.total}
            {enriching.name ? ` · ${enriching.name}` : ''}
          </span>
        )}
        <div className="flex-1" />
        <button
          onClick={() => void handleRemoveSelected()}
          disabled={selected.length === 0}
          className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
          style={{
            color: selected.length > 0 ? 'var(--status-danger, #EF4444)' : 'var(--text-tertiary)',
            backgroundColor: 'var(--bg-field)',
            border: '1px solid var(--border)',
            opacity: selected.length === 0 ? 0.5 : 1,
          }}
          title="把选中的文件从本库移除（只解关联，不会删除知识库定义）"
        >
          <Trash2 size={11} />
          从库中移除
        </button>
        <div className="relative">
          <button
            onClick={() => setMenuOpen((v) => !v)}
            disabled={busy}
            className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
            style={{
              color: 'var(--text-secondary)',
              backgroundColor: 'var(--bg-field)',
              border: '1px solid var(--border)',
              opacity: busy ? 0.5 : 1,
            }}
            title={selected.length > 0 ? `导出选中的 ${selected.length} 个文件` : '未选中则导出全部'}
          >
            {busy ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
            {selected.length > 0 ? `导出选中（${selected.length}）` : '导出全部'}
            <ChevronDown size={10} />
          </button>
          {menuOpen && (
            <div
              className="absolute right-0 top-full mt-1 py-1 rounded-lg z-20"
              style={{
                backgroundColor: 'var(--bg-secondary)',
                border: '1px solid var(--border)',
                boxShadow: '0 8px 24px rgba(0,0,0,0.22)',
                minWidth: 168,
              }}
            >
              <button
                onClick={() => void doExport(exportTargets, false)}
                className="flex items-center gap-2 w-full px-3 py-1.5 text-xs text-left transition-colors hover:opacity-80"
                style={{ color: 'var(--text-primary)' }}
              >
                <FolderOpen size={12} />
                复制到目录…
              </button>
              <button
                onClick={() => void doExport(exportTargets, true)}
                className="flex items-center gap-2 w-full px-3 py-1.5 text-xs text-left transition-colors hover:opacity-80"
                style={{ color: 'var(--text-primary)' }}
              >
                <Package size={12} />
                打包为 zip…
              </button>
            </div>
          )}
        </div>
      </div>

      <div className="px-4 py-2 text-[10px]" style={{ color: 'var(--text-tertiary)', borderBottom: '1px solid var(--border)' }}>
        原文只存一份，多库共享引用；正文抽取后按「AI 分组」进「知识」列表参与检索。导出不会覆盖目标目录里的同名文件；
        「从库中移除」只解除与本库的关联 —— 不再被任何库引用的文件才会连原文一起删除。
        按属性分目录时，目录名取自该文件所有分块的属性（文件本身没有属性）：
        没填过该属性的留在根目录，分块取值不一致的同时在多个目录出现（同一份文件，勾选/导出只算一次，
        标着「多值」；因此各目录的文件数之和可能大于文件总数）。
      </div>

      {failures.length > 0 && (
        <div
          className="shrink-0 mx-4 my-2 rounded-lg px-3 py-2 text-[11px] space-y-1"
          style={{ backgroundColor: 'rgba(239,68,68,0.08)', border: '1px solid rgba(239,68,68,0.3)', color: 'var(--text-secondary)' }}
        >
          <div className="flex items-center gap-1.5" style={{ color: 'var(--status-danger, #EF4444)' }}>
            <AlertTriangle size={12} />
            <span>以下文件导出失败：</span>
          </div>
          {failures.slice(0, 10).map((f) => (
            <div key={f} className="break-all" style={{ color: 'var(--text-tertiary)' }}>{f}</div>
          ))}
          {failures.length > 10 && <div style={{ color: 'var(--text-tertiary)' }}>…另有 {failures.length - 10} 项</div>}
        </div>
      )}

      <div className="flex-1 overflow-y-auto">
        {visibleRows.length === 0 && (
          // 库里有文件、只是都被搜掉了 —— 说清楚，别让用户以为文件丢了
          <div className="flex flex-col items-center justify-center gap-2 py-10 text-xs" style={{ color: 'var(--text-tertiary)' }}>
            <Search size={18} />
            没有匹配「{query.trim()}」的文件
          </div>
        )}
        {visibleRows.map((row) => {
          // 目录行：按属性分目录时的目录（点整行折叠/展开）
          if (row.kind === 'group') {
            const isCollapsed = collapsed.has(row.label);
            return (
              <button
                key={`group:${row.label}`}
                onClick={() => toggleCollapse(row.label)}
                className="w-full px-4 py-1.5 flex items-center gap-1.5 text-left transition-colors hover:opacity-90"
                style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-tertiary)' }}
                title={isCollapsed ? '展开这个目录' : '折叠这个目录'}
              >
                <ChevronRight
                  size={11}
                  className="shrink-0 transition-transform"
                  style={{ color: 'var(--text-tertiary)', transform: isCollapsed ? 'none' : 'rotate(90deg)' }}
                />
                <Folder size={11} className="shrink-0" style={{ color: 'var(--accent)' }} />
                <span className="text-[11px] font-medium truncate" style={{ color: 'var(--text-primary)' }}>
                  {row.label}
                </span>
                <span className="shrink-0 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{row.count} 个文件</span>
              </button>
            );
          }
          const { file: f, depth, indent, multi, groupLabel } = row;
          // 层级只由 `depth`（在不在目录里）+ `indent`（再往下几级）决定，不靠额外容器。
          // 属性目录下用「缩进 + 左侧竖线」画出树（像资源管理器），附件再按 `indent` 多缩一级；
          // 关系树沿用纯缩进（附件挂在正文下，已经有 CornerDownRight 指示，再画竖线是两套记号打架）。
          const inGroup = activeGroup && depth > 0;
          const rowIndent = {
            marginLeft: inGroup ? 22 : 0,
            borderLeft: inGroup ? '1px solid var(--border)' : undefined,
            paddingLeft: activeGroup ? 16 + indent * 22 : 16 + depth * 22,
          };
          /** 这行是"附件"（挂在一份正文下面）—— 两种分组方式下都要给出从属的记号 */
          const isAttachment = activeGroup ? indent > 0 : depth > 0;
          const st = STATUS_META[f.extractStatus];
          const StIcon = st.icon;
          const isUrl = Boolean(f.url);
          const checked = selectedSet.has(f.id);
          const parentId = tree.parentOf.get(f.id);
          const versionOf = tree.supersededBy.get(f.id);
          const replaces = tree.superseded.get(f.id) ?? [];
          return (
            // 多值时同一个文件会出现在多个目录里，key 必须带上目录名，否则会撞
            <div key={groupLabel ? `${f.id}@${groupLabel}` : f.id}>
            <div
              className="py-2.5 pr-4 flex items-start gap-3"
              style={{
                borderBottom: '1px solid var(--border)',
                backgroundColor: checked ? 'var(--bg-secondary)' : 'transparent',
                ...rowIndent,
              }}
            >
              {isAttachment && (
                <CornerDownRight size={11} className="shrink-0 mt-1" style={{ color: 'var(--text-tertiary)' }} />
              )}
              <input
                type="checkbox"
                checked={checked}
                onChange={() => toggle(f.id)}
                className="shrink-0 mt-0.5"
                title="选中后可批量导出"
              />
              <div className="shrink-0 mt-0.5" style={{ color: isUrl ? '#F59E0B' : 'var(--text-tertiary)' }}>
                {isUrl ? <Globe size={14} /> : <FileText size={14} />}
              </div>
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 min-w-0">
                  <span className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }}>{f.name}</span>
                  <span className="shrink-0 flex items-center gap-0.5 px-1.5 py-[1px] rounded text-[9px]" style={{ backgroundColor: `${st.color}1A`, color: st.color }}>
                    <StIcon size={9} className={f.extractStatus === 'extracting' ? 'animate-spin' : ''} />
                    {st.label}
                  </span>
                  {f.chunkCount > 0 && (
                    <span className="shrink-0 text-[9px]" style={{ color: 'var(--text-tertiary)' }}>{f.chunkCount} 个分块</span>
                  )}
                  {/* 多值：这个文件的分块在该属性上取值不一致，所以它同时出现在多个目录里。
                      不标出来的话，"这份文件整理得不彻底"这件事在文件视图里就彻底看不见了 */}
                  {multi && (
                    <span
                      className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                      style={{ backgroundColor: 'rgba(245,158,11,0.14)', color: '#F59E0B' }}
                      title="这个文件的分块在该属性上取值不一致，因此同时挂在多个目录下；可用行上的「AI 补整理」刷新"
                    >
                      多值
                    </span>
                  )}
                  {/* 整理状态：AI 预处理（打标 + 填专属属性）。没整理过就当场能发起，失败可重试 */}
                  {f.extractStatus === 'done' && f.chunkCount > 0 && (
                    f.enriched ? (
                      <span
                        className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                        style={{ backgroundColor: 'rgba(16,185,129,0.12)', color: '#10B981' }}
                        title={
                          '已由 AI 打标并填好专属属性（正文未被改写）'
                          + (f.enrichedAt ? ` · AI 整理于 ${fmtAgo(f.enrichedAt)}` : '')
                        }
                      >
                        已整理{f.enrichedAt ? ` · ${fmtAgo(f.enrichedAt)}` : ''}
                      </span>
                    ) : (
                      <button
                        onClick={() => void enrichFile(base.id, f.id)}
                        disabled={!!enriching}
                        className="pd-btn shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                        style={{
                          backgroundColor: f.enrichError ? 'rgba(239,68,68,0.12)' : 'var(--bg-field)',
                          color: f.enrichError ? '#EF4444' : 'var(--text-secondary)',
                          border: '1px solid var(--border)',
                          opacity: enriching ? 0.5 : 1,
                        }}
                        title={
                          f.enrichError
                            ? `上次补整理失败：${f.enrichError}（点击重试）`
                            : '让 AI 为这些条目重新打标与补专属属性（不改写正文）。投喂时已由 AI 分组，这里只是补救'
                        }
                      >
                        {f.enrichError ? '补整理失败 · 重试' : 'AI 补整理'}
                      </button>
                    )
                  )}
                  <span className="shrink-0 text-[9px]" style={{ color: 'var(--text-tertiary)' }}>{fmtSize(f.size)}</span>
                  {/* 关系标注：附件归属 / 历史版本 / 替代了哪些旧版 —— 都是标注，不改变检索范围 */}
                  {parentId && (
                    <span
                      className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                      style={{ backgroundColor: 'rgba(59,130,246,0.12)', color: '#3B82F6' }}
                      title={`属于正文《${nameOf(parentId)}》`}
                    >
                      附件 · 属于《{nameOf(parentId)}》
                    </span>
                  )}
                  {versionOf && (
                    <span
                      className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                      style={{ backgroundColor: 'rgba(245,158,11,0.14)', color: '#D97706' }}
                      title="不是现行版本；旧版仍完整保留、也仍会被检索命中"
                    >
                      历史版本 · 已被《{nameOf(versionOf)}》替代
                    </span>
                  )}
                  {replaces.length > 0 && (
                    <span
                      className="shrink-0 px-1.5 py-[1px] rounded text-[9px]"
                      style={{ backgroundColor: 'rgba(139,92,246,0.12)', color: '#8B5CF6' }}
                      title="本文件替代了这些旧版；旧版未被删除、仍可检索"
                    >
                      替代了《{replaces.map(nameOf).join('》《')}》
                    </span>
                  )}
                </div>
                <div className="text-[10px] mt-1 truncate" style={{ color: 'var(--text-tertiary)' }} title={f.path || f.url}>
                  {f.path || f.url}
                </div>
                <div className="flex items-center gap-2 mt-1 text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                  <span>被 {f.baseIds.length} 个库引用</span>
                  <span>· {f.mime || '未知类型'}</span>
                </div>
              </div>
              <button
                onClick={() => { void openOriginal(f.absPath); }}
                disabled={!f.absPath}
                className="pd-btn shrink-0 flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
                style={{
                  color: 'var(--text-secondary)',
                  backgroundColor: 'var(--bg-field)',
                  border: '1px solid var(--border)',
                  opacity: f.absPath ? 1 : 0.5,
                  cursor: f.absPath ? 'pointer' : 'not-allowed',
                }}
                title={f.absPath ? `打开原文：${f.absPath}` : '原文路径缺失'}
              >
                <FolderOpen size={11} />
                打开原文
              </button>
              <button
                onClick={() => setRelPanelId((v) => (v === f.id ? null : f.id))}
                className="pd-btn shrink-0 flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
                style={{
                  color: parentId || versionOf || replaces.length > 0 ? 'var(--accent)' : 'var(--text-secondary)',
                  backgroundColor: 'var(--bg-field)',
                  border: '1px solid var(--border)',
                }}
                title="设置与其它文件的关系：属于哪个正文、替代了哪一版"
              >
                <Link2 size={11} />
                关系
              </button>
            </div>
            {relPanelId === f.id && (
              // 面板的缩进必须与它所属那一行**完全一致** —— 否则它会从目录那一层起头，
              // 看上去像"这个目录的属性"而不是"这个文件的属性"（用户反馈过）。
              // 面板自带 `px-4`（= 行的基础 paddingLeft 16），所以这里只补差额。
              <div
                style={{
                  marginLeft: rowIndent.marginLeft,
                  borderLeft: rowIndent.borderLeft,
                  paddingLeft: activeGroup ? indent * 22 : depth * 22,
                }}
              >
                <FileRelationPanel
                  file={f}
                  files={files}
                  parentId={parentId}
                  versionOf={versionOf}
                  replaces={replaces}
                  onLink={linkFiles}
                  onUnlink={unlinkFiles}
                />
              </div>
            )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

interface FileRelationPanelProps {
  file: KnowledgeFile;
  files: KnowledgeFile[];
  parentId?: string;
  versionOf?: string;
  replaces: string[];
  onLink: (kind: KnowledgeFileRelationKind, fromFileId: string, toFileId: string) => Promise<boolean>;
  onUnlink: (kind: KnowledgeFileRelationKind, fromFileId: string, toFileId: string) => Promise<boolean>;
}

const SELECT_STYLE = { minWidth: 240, maxWidth: 340 } as const;

/**
 * 行内关系面板：设「所属正文」与「替代了哪一版」。
 * 只写标注，不动文件与条目 —— 旧版仍留在库里、仍参与检索。
 */
function FileRelationPanel({
  file,
  files,
  parentId,
  versionOf,
  replaces,
  onLink,
  onUnlink,
}: FileRelationPanelProps) {
  const others = files.filter((f) => f.id !== file.id);
  const nameOf = (id: string) => files.find((f) => f.id === id)?.name ?? '（已不在本库）';
  const [addingOld, setAddingOld] = useState('');

  /** 改「所属正文」：先解除旧关系再建新的（后端保证一个文件只能有一个正文） */
  const changeParent = async (next: string) => {
    if (parentId && !(await onUnlink('attachment', parentId, file.id))) return;
    if (next) await onLink('attachment', next, file.id);
  };

  return (
    <div
      className="px-4 py-2.5 flex flex-col gap-2 text-[11px]"
      style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-tertiary)' }}
    >
      <div className="flex items-center gap-2">
        <span className="shrink-0 w-20" style={{ color: 'var(--text-secondary)' }}>所属正文</span>
        <Select
          value={parentId ?? ''}
          onChange={(v) => void changeParent(v)}
          options={[
            { value: '', label: '不属于任何正文' },
            ...others.map((f) => ({ value: f.id, label: f.name })),
          ]}
          size="sm"
          style={SELECT_STYLE}
          placeholder="不属于任何正文"
          title="公文类：这份文件是哪个正文的附件（一个文件只能有一个正文）"
        />
        <span style={{ color: 'var(--text-tertiary)' }}>设为附件后，它会缩进显示在正文下面</span>
      </div>

      <div className="flex items-start gap-2 flex-wrap">
        <span className="shrink-0 w-20 mt-1" style={{ color: 'var(--text-secondary)' }}>版本替代</span>
        {versionOf ? (
          <span
            className="flex items-center gap-1 px-1.5 py-[2px] rounded"
            style={{ backgroundColor: 'rgba(245,158,11,0.14)', color: '#D97706' }}
          >
            已被《{nameOf(versionOf)}》替代（历史版本，仍可检索）
            <button
              onClick={() => void onUnlink('supersedes', versionOf, file.id)}
              className="pd-btn p-0.5"
              title="解除该标注"
            >
              <X size={10} />
            </button>
          </span>
        ) : (
          <span className="mt-0.5" style={{ color: 'var(--text-tertiary)' }}>当前是现行版本</span>
        )}
        {replaces.map((oldId) => (
          <span
            key={oldId}
            className="flex items-center gap-1 px-1.5 py-[2px] rounded"
            style={{ backgroundColor: 'rgba(139,92,246,0.12)', color: '#8B5CF6' }}
          >
            替代了《{nameOf(oldId)}》
            <button
              onClick={() => void onUnlink('supersedes', file.id, oldId)}
              className="pd-btn p-0.5"
              title="解除该标注"
            >
              <X size={10} />
            </button>
          </span>
        ))}
      </div>

      <div className="flex items-center gap-2">
        <span className="shrink-0 w-20" style={{ color: 'var(--text-secondary)' }}>新增替代</span>
        <Select
          value={addingOld}
          onChange={(v) => {
            setAddingOld('');
            if (v) void onLink('supersedes', file.id, v);
          }}
          options={[
            { value: '', label: '选择本文件替代的旧版…' },
            ...others.map((f) => ({ value: f.id, label: f.name })),
          ]}
          size="sm"
          style={SELECT_STYLE}
          placeholder="选择本文件替代的旧版…"
          title="只做标注：旧版不会被删除，也不会被排除在检索之外"
        />
      </div>

      <div style={{ color: 'var(--text-tertiary)' }}>
        关系只用于标注（列表缩进、检索命中时带出来源），不会删除或替换任何文件与条目；旧版本仍保留、仍可检索。
      </div>
    </div>
  );
}

/** 打开原文：文件在知识库目录里（网页投喂时存的是抓取到的正文 .md） */
async function openOriginal(absPath: string) {
  if (!absPath) return;
  try {
    await invoke('open_path', { path: absPath });
  } catch (e) {
    showToast(`打开原文失败: ${errorMessage(e)}`, 'error');
  }
}
