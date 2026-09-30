/**
 * KnowledgeEntryList — 某个知识库下的知识列表
 *
 * 筛选 = 通用属性（搜索 / 来源 / 重要）× 专属属性（按该库的字段定义动态渲染）。
 * 展开行即条目详情，且**正文 / 标签 / 专属属性都可就地编辑**（一次保存，一个事务）——
 * AI 整理与投喂都会出错，只读的详情等于把纠错成本推给"重新投喂一遍"。
 * 展开行还能把这一条**复制成 markdown** —— 知识被取出到别处用（贴进对话、写进文档）比留在库里更常见。
 */

import { useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { Search, Pin, RotateCcw, Link2Off, FileText, Globe, Hammer, Scissors, ChevronRight, Save, Wand2, X, Copy, Download, Cloud } from 'lucide-react';
import { Select } from '../common/Select';
import { MetaEditor } from './KnowledgeFieldInputs';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import {
  EMPTY_ENTRY_FILTERS,
  filterEntries,
  hasActiveEntryFilters,
  useKnowledgeStore,
} from '../../stores/knowledgeStore';
import {
  ORIGIN_COLOR,
  ORIGIN_LABEL,
  entryTitle,
  sourceFileName,
  type KnowledgeEntry,
  type KnowledgeMeta,
  type KnowledgeOrigin,
  type KnowledgeBase,
} from '../../types/knowledge';

const ORIGIN_ICON: Record<KnowledgeOrigin, typeof FileText> = {
  work: Hammer,
  snippet: Scissors,
  file: FileText,
  url: Globe,
  cloud: Cloud,
  ai: Wand2,
};

/** 保存时一起提交的三样东西 */
interface EntryPatch {
  value: string;
  tags: string[];
  meta: KnowledgeMeta;
}

/** `kb_export_entries` 的返回（与后端 KbEntryExportOutcome 对应） */
interface KbEntryExportOutcome {
  dest: string;
  exported: number;
  /** 库里已找不到的 key（条目被删除 / 解除关联） */
  missing: string[];
}

interface KnowledgeEntryListProps {
  base: KnowledgeBase;
  /** 已过滤为「属于本库」的条目 */
  entries: KnowledgeEntry[];
  /** 保存正文 / 标签 / 专属属性；返回是否成功（失败时保持展开、不丢草稿） */
  onSave: (key: string, patch: EntryPatch) => Promise<boolean>;
  onRemove: (key: string) => void;
  /** 批量解除/删除（多选后一次处理） */
  onRemoveMany: (keys: string[]) => void;
}

function fmtTime(sec: number): string {
  const diff = Math.floor(Date.now() / 1000) - sec;
  if (diff < 3600) return `${Math.max(1, Math.floor(diff / 60))} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}

/** 专属属性是否改动：忽略空值，键序无关 —— 直接 stringify 比较会因为键序把没改的判成改了 */
function sameMeta(a: KnowledgeMeta, b: KnowledgeMeta): boolean {
  const pick = (m: KnowledgeMeta) =>
    Object.keys(m).filter((k) => m[k] !== undefined && m[k] !== '').sort();
  const ka = pick(a);
  const kb = pick(b);
  return ka.length === kb.length && ka.every((k, i) => k === kb[i] && String(a[k]) === String(b[k]));
}

/**
 * 标签维护：chip 列表 + 输入框（回车 / 逗号提交，退格删最后一个）。
 *
 * 用 `pd-field` 把聚焦反馈交给容器 —— 没这个类的话全局 `:focus-visible` 的描边会落在内部的
 * `<input>` 上，出现"边框在外、焦点圈在内"的错位（见 globals.css 的说明）。
 */
function TagEditor({ tags, onChange }: { tags: string[]; onChange: (t: string[]) => void }) {
  const [input, setInput] = useState('');
  const add = (raw: string) => {
    const t = raw.trim().replace(/^#/, '').trim();
    if (t && !tags.includes(t)) onChange([...tags, t]);
    setInput('');
  };
  return (
    <div
      className="pd-field flex items-center gap-1.5 flex-wrap px-2 py-1.5 rounded-lg"
      style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
    >
      {tags.map((t) => (
        <span
          key={t}
          className="flex items-center gap-1 px-1.5 py-[1px] rounded text-[10px]"
          style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)' }}
        >
          #{t}
          <button onClick={() => onChange(tags.filter((x) => x !== t))} style={{ color: 'var(--text-tertiary)' }} title="移除这个标签">
            <X size={9} />
          </button>
        </span>
      ))}
      <input
        value={input}
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={(e) => {
          // 中文输入法组字期间的回车/逗号不能抢（抢了就会把半个拼音当标签）
          if (e.nativeEvent.isComposing) return;
          if (e.key === 'Enter' || e.key === ',' || e.key === '，') {
            e.preventDefault();
            add(input);
          } else if (e.key === 'Backspace' && !input && tags.length > 0) {
            onChange(tags.slice(0, -1));
          }
        }}
        onBlur={() => add(input)}
        placeholder={tags.length === 0 ? '加标签，回车确认' : '继续加…'}
        className="min-w-[110px] flex-1 bg-transparent outline-none text-[10px]"
        style={{ color: 'var(--text-primary)' }}
      />
    </div>
  );
}

export function KnowledgeEntryList({ base, entries, onSave, onRemove, onRemoveMany }: KnowledgeEntryListProps) {
  // 筛选条件放在 store 里：图谱页共用同一套口径，且图谱能据此限流
  const filters = useKnowledgeStore((s) => s.entryFilters);
  const setFilters = useKnowledgeStore((s) => s.setEntryFilters);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [draftMeta, setDraftMeta] = useState<KnowledgeMeta>({});
  /** 展开行的正文 / 标签草稿：与 meta 一起保存，任一项改动都算"有未保存的改动" */
  const [draftValue, setDraftValue] = useState('');
  const [draftTags, setDraftTags] = useState<string[]>([]);
  /** 保存中：防止连点造成并发写 */
  const [saving, setSaving] = useState(false);
  /** 多选（按条目 key）：用于批量解除/删除 */
  const [checkedKeys, setCheckedKeys] = useState<string[]>([]);

  const comparableFields = base.fields.filter((f) => f.type === 'select' || f.type === 'boolean');

  const filtered = useMemo(
    () =>
      filterEntries(entries, filters).sort(
        (a, b) => (Number(b.pin) - Number(a.pin)) || (b.updatedAt - a.updatedAt),
      ),
    [entries, filters],
  );

  const hasFilter = hasActiveEntryFilters(filters);

  const resetFilters = () => setFilters(EMPTY_ENTRY_FILTERS);

  const selectRow = (e: KnowledgeEntry) => {
    if (selectedKey === e.key) {
      setSelectedKey(null);
      return;
    }
    setSelectedKey(e.key);
    // 三项草稿一起初始化：只初始化 meta 的话，切换条目会把上一条的正文带过来
    setDraftMeta(e.meta);
    setDraftValue(e.value);
    setDraftTags(e.tags);
  };

  /** 保存展开行的全部改动（正文 + 标签 + 专属属性） */
  const handleSave = async (e: KnowledgeEntry) => {
    if (saving) return;
    setSaving(true);
    try {
      await onSave(e.key, { value: draftValue, tags: draftTags, meta: draftMeta });
    } finally {
      setSaving(false);
    }
  };

  const handleRemove = async (e: KnowledgeEntry) => {
    const onlyHere = e.baseIds.length === 1;
    const name = entryTitle(e.key);
    const ok = await confirmDialog({
      title: onlyHere ? '删除该条知识' : '解除关联',
      message: onlyHere
        ? `「${name}」只属于本库，解除后将被删除（与删库时清理孤儿条目同一口径）。`
        : `「${name}」还属于其它 ${e.baseIds.length - 1} 个库，将只解除与本库的关联，条目本身保留。`,
      confirmText: onlyHere ? '删除' : '解除关联',
    });
    if (!ok) return;
    onRemove(e.key);
    if (selectedKey === e.key) setSelectedKey(null);
    setCheckedKeys((prev) => prev.filter((k) => k !== e.key));
  };

  const checked = useMemo(() => new Set(checkedKeys), [checkedKeys]);
  const allChecked = filtered.length > 0 && filtered.every((e) => checked.has(e.key));
  const toggleChecked = (key: string) =>
    setCheckedKeys((prev) => (prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key]));
  const toggleAllChecked = () => setCheckedKeys(allChecked ? [] : filtered.map((e) => e.key));

  const handleRemoveMany = async () => {
    // 以「当前仍在库里的条目」为准：勾选过的条目可能已被删掉，直接用 checkedKeys 会算错分类
    const targets = entries.filter((e) => checked.has(e.key));
    if (targets.length === 0) return;
    const onlyHere = targets.filter((e) => e.baseIds.length === 1).length;
    const ok = await confirmDialog({
      title: '批量处理',
      message:
        `已选 ${targets.length} 条：其中 ${onlyHere} 条只属于本库，将被删除；`
        + `${targets.length - onlyHere} 条还属于其它库，只解除与本库的关联（条目本体保留）。`,
      confirmText: '确认处理',
    });
    if (!ok) return;
    onRemoveMany(targets.map((e) => e.key));
    setCheckedKeys([]);
    setSelectedKey(null);
  };

  /** 把选中的条目导出成**一个 markdown 文件**（片段 / AI 生成 / 工作沉淀唯一的交付出口） */
  const handleExport = async () => {
    const targets = entries.filter((e) => checked.has(e.key));
    if (targets.length === 0) return;
    let dest: string;
    try {
      const picked = await saveDialog({
        title: '导出选中的知识',
        defaultPath: `${base.name}-知识.md`,
        filters: [{ name: 'Markdown', extensions: ['md'] }],
      });
      if (typeof picked !== 'string' || !picked) return;
      dest = picked;
    } catch (e) {
      showToast(`选择导出位置失败：${errorMessage(e)}`, 'error');
      return;
    }
    try {
      const out = await invoke<KbEntryExportOutcome>('kb_export_entries', {
        kbId: base.id,
        keys: targets.map((e) => e.key),
        dest,
      });
      const extra = out.missing.length > 0 ? `（${out.missing.length} 条已不存在，已跳过）` : '';
      showToast(`已导出 ${out.exported} 条知识${extra}`, 'success');
    } catch (e) {
      showToast(`导出失败：${errorMessage(e)}`, 'error');
    }
  };

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* 筛选区：通用属性一行 + 专属属性一行 */}
      <div className="shrink-0 px-4 pt-3 pb-2 space-y-2" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="flex items-center gap-2">
          {/* `pd-field` 把聚焦反馈交给容器：全局 `:focus-visible` 的 1px 描边优先级高于 Tailwind 的
              `outline-none`，不加这个类的话描边会落在内层 input 上 —— 看着像"边框在外、焦点圈在内"。 */}
          <div
            className="pd-field flex items-center gap-1.5 px-2 py-1 rounded-lg flex-1 min-w-0"
            style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)', maxWidth: 320 }}
          >
            <Search size={12} style={{ color: 'var(--text-tertiary)', flexShrink: 0 }} />
            <input
              value={filters.query}
              onChange={(e) => setFilters({ query: e.target.value })}
              placeholder="搜索键 / 内容 / 标签"
              className="flex-1 min-w-0 bg-transparent outline-none text-xs"
              style={{ color: 'var(--text-primary)' }}
            />
          </div>
          <div className="w-32 shrink-0">
            <Select
              value={filters.origin}
              onChange={(v) => setFilters({ origin: v })}
              options={[
                { value: 'all', label: '全部来源' },
                ...(Object.keys(ORIGIN_LABEL) as KnowledgeOrigin[]).map((o) => ({ value: o, label: ORIGIN_LABEL[o] })),
              ]}
              size="sm"
              className="w-full"
            />
          </div>
          <button
            onClick={() => setFilters({ pinOnly: !filters.pinOnly })}
            className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] shrink-0"
            style={{
              color: filters.pinOnly ? '#F59E0B' : 'var(--text-secondary)',
              backgroundColor: filters.pinOnly ? 'rgba(245,158,11,0.12)' : 'var(--bg-tertiary)',
              border: '1px solid var(--border)',
            }}
            title="只看重要（pin）"
          >
            <Pin size={11} />
            重要
          </button>
          {hasFilter && (
            <button
              onClick={resetFilters}
              className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] shrink-0"
              style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
            >
              <RotateCcw size={11} />
              重置
            </button>
          )}
          <div className="flex-1" />
          <label
            className="flex items-center gap-1.5 text-[11px] shrink-0 cursor-pointer select-none"
            style={{ color: 'var(--text-secondary)' }}
            title="全选当前筛选结果，用于批量处理"
          >
            <input type="checkbox" checked={allChecked} onChange={toggleAllChecked} />
            全选
          </label>
          <span className="text-[11px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
            {filtered.length} / {entries.length} 条
          </span>
        </div>

        {/* 专属属性筛选：由库定义动态生成 */}
        {comparableFields.length > 0 && (
          <div className="flex items-center gap-2 flex-wrap">
            <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>专属属性</span>
            {comparableFields.map((f) => (
              <div key={f.key} className="w-32 shrink-0">
                <Select
                  value={filters.fields[f.key] ?? ''}
                  onChange={(v) => setFilters({ fields: { ...filters.fields, [f.key]: v } })}
                  options={[
                    { value: '', label: `${f.label}：不限` },
                    ...(f.type === 'boolean'
                      ? [{ value: 'true', label: `${f.label}：是` }, { value: 'false', label: `${f.label}：否` }]
                      : (f.options ?? []).map((o) => ({ value: o, label: `${f.label}：${o}` }))),
                  ]}
                  size="sm"
                  className="w-full"
                />
              </div>
            ))}
          </div>
        )}
      </div>

      {/* 批量操作栏：只有勾选了才出现，不占常态空间 */}
      {checkedKeys.length > 0 && (
        <div
          className="shrink-0 px-4 py-1.5 flex items-center gap-2 text-[11px]"
          style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-tertiary)' }}
        >
          <span style={{ color: 'var(--text-primary)' }}>已选 {checkedKeys.length} 条</span>
          <div className="flex-1" />
          <button
            onClick={() =>
              void copyText(
                entries
                  .filter((e) => checked.has(e.key))
                  .map((e) => toMarkdown(e))
                  .join('\n\n---\n\n'),
                `已复制 ${checkedKeys.length} 条（markdown）`,
              )
            }
            className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg"
            style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            title="把选中的条目按 markdown 复制到剪贴板（多条之间用 --- 分隔）"
          >
            <Copy size={11} />
            复制选中
          </button>
          <button
            onClick={() => void handleExport()}
            className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg"
            style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            title="把选中的条目导出成一个 markdown 文件（标题 + 正文 + 属性 + 标签 + 来源）"
          >
            <Download size={11} />
            导出为 .md
          </button>
          <button
            onClick={() => setCheckedKeys([])}
            className="pd-btn px-2 py-1 rounded-lg"
            style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
          >
            清空选择
          </button>
          <button
            onClick={() => void handleRemoveMany()}
            className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg"
            style={{ color: 'var(--status-danger, #EF4444)', backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            title="只属于本库的会被删除；还属于其它库的只解除关联"
          >
            <Link2Off size={11} />
            批量删除 / 解除关联
          </button>
        </div>
      )}

      {/* 列表 */}
      <div className="flex-1 overflow-y-auto">
        {filtered.length === 0 ? (
          <div className="h-full flex flex-col items-center justify-center gap-2 text-xs" style={{ color: 'var(--text-tertiary)' }}>
            <Search size={18} />
            {entries.length === 0 ? '这个库还没有知识：去「投喂」丢一段文本' : '没有符合筛选条件的知识'}
          </div>
        ) : (
          filtered.map((e) => {
            const OriginIcon = ORIGIN_ICON[e.origin];
            const open = selectedKey === e.key;
            const meta = e.meta;
            // 有未保存的改动：正文 / 标签顺序敏感（标签是列表，顺序变了也算改），专属属性比较忽略空值与键序
            const dirty = open && (
              draftValue !== e.value
              || draftTags.length !== e.tags.length
              || draftTags.some((t, i) => t !== e.tags[i])
              || !sameMeta(draftMeta, e.meta)
            );
            return (
              <div
                key={e.key}
                className="px-4 py-2.5 transition-colors"
                style={{ borderBottom: '1px solid var(--border)', backgroundColor: open ? 'var(--bg-secondary)' : 'transparent' }}
              >
                <div className="flex items-start gap-2 cursor-pointer" onClick={() => selectRow(e)}>
                  <input
                    type="checkbox"
                    checked={checked.has(e.key)}
                    onChange={() => toggleChecked(e.key)}
                    onClick={(ev) => ev.stopPropagation()}
                    className="shrink-0 mt-0.5"
                    title="勾选后可批量删除 / 解除关联"
                  />
                  <ChevronRight
                    size={12}
                    className="shrink-0 mt-0.5 transition-transform"
                    style={{ color: 'var(--text-tertiary)', transform: open ? 'rotate(90deg)' : 'none' }}
                  />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-1.5 min-w-0">
                      {e.pin && <Pin size={10} className="shrink-0" style={{ color: '#F59E0B' }} />}
                      <span className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }} title={entryTitle(e.key)}>
                        {entryTitle(e.key)}
                      </span>
                      <span
                        className="shrink-0 flex items-center gap-0.5 px-1.5 py-[1px] rounded text-[9px]"
                        style={{ backgroundColor: `${ORIGIN_COLOR[e.origin]}1A`, color: ORIGIN_COLOR[e.origin] }}
                        title={`来源：${ORIGIN_LABEL[e.origin]}${e.sourceRef ? ` · ${e.sourceRef}` : ''}`}
                      >
                        <OriginIcon size={9} />
                        {ORIGIN_LABEL[e.origin]}
                      </span>
                      {/* 来源原文：只显示名字，rel_path 里的 `<sha8>-` 前缀是去重用的内部标识。
                          不折叠到 tooltip 里 —— "这条是从哪份文件来的"是判断可信度的第一信息 */}
                      {e.sourceRef && (
                        <span
                          className="shrink-0 max-w-[180px] truncate text-[9px]"
                          style={{ color: 'var(--text-tertiary)' }}
                          title={e.sourceRef}
                        >
                          {sourceFileName(e.sourceRef)}
                        </span>
                      )}
                      {e.chunkIndex !== undefined && (
                        <span className="shrink-0 text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                          第 {e.chunkIndex}/{e.chunkTotal} 块
                        </span>
                      )}
                      {e.baseIds.length > 1 && (
                        <span className="shrink-0 text-[9px]" style={{ color: 'var(--text-tertiary)' }} title={`同时属于 ${e.baseIds.length} 个库`}>
                          ▣ {e.baseIds.length} 库
                        </span>
                      )}
                    </div>
                    {!open && (
                      <div className="text-[11px] mt-1 line-clamp-2" style={{ color: 'var(--text-secondary)' }}>{e.value}</div>
                    )}
                    {!open && (
                      <div className="flex items-center gap-1.5 mt-1.5 flex-wrap">
                        {base.fields.map((f) => {
                          const v = meta[f.key];
                          if (v === undefined || v === '') return null;
                          return (
                            <span
                              key={f.key}
                              className="px-1.5 py-[1px] rounded text-[9px]"
                              style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}
                            >
                              {f.label}：{typeof v === 'boolean' ? (v ? '是' : '否') : String(v)}
                            </span>
                          );
                        })}
                        {e.tags.map((t) => (
                          <span key={t} className="px-1.5 py-[1px] rounded text-[9px]" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
                            #{t}
                          </span>
                        ))}
                        <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                          命中 {e.accessCount} 次 · {fmtTime(e.updatedAt)}
                        </span>
                      </div>
                    )}
                  </div>
                </div>

                {/* 展开：正文 / 标签 / 专属属性都可就地改，一次保存 */}
                {open && (
                  <div className="mt-2 ml-5 space-y-3">
                    <div>
                      <div className="text-[11px] mb-2" style={{ color: 'var(--text-secondary)' }}>正文（可编辑）</div>
                      <textarea
                        value={draftValue}
                        onChange={(ev) => setDraftValue(ev.target.value)}
                        rows={Math.min(18, Math.max(6, draftValue.split('\n').length))}
                        className="w-full p-3 rounded-lg text-xs leading-relaxed outline-none resize-y"
                        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                      />
                    </div>

                    <div>
                      <div className="text-[11px] mb-2" style={{ color: 'var(--text-secondary)' }}>
                        标签（用于检索与图谱的「共标签」关系）
                      </div>
                      <TagEditor tags={draftTags} onChange={setDraftTags} />
                    </div>

                    <div className="flex items-center gap-3 text-[10px] flex-wrap" style={{ color: 'var(--text-tertiary)' }}>
                      <span>分类：知识（不占 600 条会话记忆配额）</span>
                      <span>热度 {e.accessCount}</span>
                      <span>创建 {fmtTime(e.createdAt)}</span>
                      {e.sourceRef && (
                        <span className="truncate" title={e.sourceRef}>
                          来源：{sourceFileName(e.sourceRef)}
                          {e.chunkIndex !== undefined ? ` · 第 ${e.chunkIndex}/${e.chunkTotal} 块` : ''}
                        </span>
                      )}
                    </div>

                    <div>
                      <div className="text-[11px] mb-2" style={{ color: 'var(--text-secondary)' }}>专属属性（仅本库）</div>
                      <MetaEditor
                        fields={base.fields}
                        value={draftMeta}
                        onChange={setDraftMeta}
                      />
                    </div>

                    <div className="flex items-center gap-2 flex-wrap">
                      <button
                        onClick={() => void handleSave(e)}
                        disabled={saving || !dirty}
                        className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                        style={{
                          backgroundColor: 'var(--accent)',
                          color: '#fff',
                          border: 'none',
                          opacity: saving || !dirty ? 0.5 : 1,
                          cursor: saving || !dirty ? 'not-allowed' : 'pointer',
                        }}
                        title={dirty ? '保存正文 / 标签 / 专属属性' : '没有改动'}
                      >
                        <Save size={11} />
                        {saving ? '保存中…' : dirty ? '保存' : '已保存'}
                      </button>
                      <button
                        onClick={() => void copyText(toMarkdown(e, { value: draftValue, tags: draftTags, meta: draftMeta }), '已复制到剪贴板（markdown）')}
                        className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                        style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                        title="复制这一条为 markdown（标题 + 正文 + 标签 + 来源）"
                      >
                        <Copy size={11} />
                        复制
                      </button>
                      <button
                        onClick={() => void handleRemove(e)}
                        className="pd-btn flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px]"
                        style={{ color: 'var(--status-danger, #EF4444)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
                        title={e.baseIds.length === 1 ? '删除该条知识' : '只解除与本库的关联'}
                      >
                        <Link2Off size={11} />
                        {e.baseIds.length === 1 ? '删除' : '解除关联'}
                      </button>
                    </div>
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

/* ────────────── 取出：把知识变成可以直接拿去用的文本 ────────────── */

/**
 * 一条知识 → markdown。传 `patch` 时用**当前草稿**（所见即所复制），
 * 不传则用已保存的值（批量复制走这条）。
 */
function toMarkdown(e: KnowledgeEntry, patch?: EntryPatch): string {
  const value = patch?.value ?? e.value;
  const tags = patch?.tags ?? e.tags;
  const parts = [`## ${entryTitle(e.key)}`, '', value.trim()];
  if (tags.length > 0) {
    parts.push('', tags.map((t) => `#${t}`).join(' '));
  }
  if (e.sourceRef) {
    parts.push('', `> 来源：${sourceFileName(e.sourceRef)}`);
  }
  return parts.join('\n');
}

async function copyText(text: string, okMsg: string) {
  try {
    await navigator.clipboard.writeText(text);
    showToast(okMsg, 'success');
  } catch (err) {
    showToast(`复制失败：${errorMessage(err)}`, 'error');
  }
}
