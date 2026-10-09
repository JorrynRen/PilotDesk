/**
 * 知识库字段控件：专属属性的「定义」与「取值」两个编辑器。
 *
 * 定义（FieldSchemaEditor）由用户在库定义里维护，一行一个字段；
 * 取值（MetaEditor）按「库 × 条目」各存一份 —— 多对多的必然结果：
 * 同一条知识属于两个库时，两个库的专属字段各不相同，值不能放在条目上。
 */

import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { Select } from '../common/Select';
import type { KnowledgeFieldDef, KnowledgeFieldType, KnowledgeMeta } from '../../types/knowledge';

const TYPE_LABEL: Record<KnowledgeFieldType, string> = {
  text: '文本',
  number: '数字',
  boolean: '是/否',
  select: '单选',
  date: '日期',
};

/** 字段类型下拉项（只在本文件的编辑器里用，不对外导出 —— 组件文件导出常量会破坏 fast refresh） */
const FIELD_TYPE_OPTIONS = (Object.keys(TYPE_LABEL) as KnowledgeFieldType[])
  .map((t) => ({ value: t, label: TYPE_LABEL[t] }));

const inputStyle = {
  backgroundColor: 'var(--bg-field)',
  color: 'var(--text-primary)',
  border: '1px solid var(--border)',
} as const;

/** 输入框基础样式（**不含宽度**）：宽度必须由调用方显式给，见下方说明 */
const inputBase = 'px-2 py-1 rounded-lg text-xs outline-none';

/**
 * 单选候选项拆分：中英文逗号、顿号、分号、竖线都算分隔符。
 * 与记忆标签的归一化口径（db.rs 的 normalize_tags）保持一致 ——
 * 否则用户打了全角逗号，整串会被当成一个选项。
 */
function splitOptions(raw: string): string[] {
  return raw
    .split(/[,，、;；|]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

/**
 * 单选候选项输入框。
 *
 * **不能**把 `value` 直接绑成 `options.join(',')`：那样每敲一个字符都会走一次
 * `split → join` 往返，尾部的分隔符被当场吃掉（`"后端,"` → `["后端"]` → `"后端"`），
 * 表现为 **分隔符根本打不进去**（顺带还会吃掉刚输入的空格）。
 *
 * 所以这里保留"用户原样的草稿文本"，只有在**草稿的解析结果与外部值不一致**时
 * （= 确实被外部改了，比如上方删掉一行导致下标错位）才用外部值回填草稿。
 */
function SelectOptionsInput({
  options,
  onOptionsChange,
  placeholder,
  className,
  style,
}: {
  options: string[];
  onOptionsChange: (options: string[]) => void;
  placeholder?: string;
  className?: string;
  style?: CSSProperties;
}) {
  const [draft, setDraft] = useState(() => options.join(','));
  const draftRef = useRef(draft);

  useEffect(() => {
    if (JSON.stringify(splitOptions(draftRef.current)) !== JSON.stringify(options)) {
      draftRef.current = options.join(',');
      setDraft(draftRef.current);
    }
  }, [options]);

  return (
    <input
      value={draft}
      onChange={(e) => {
        draftRef.current = e.target.value;
        setDraft(e.target.value);
        onOptionsChange(splitOptions(e.target.value));
      }}
      placeholder={placeholder}
      className={className}
      style={style}
    />
  );
}

/* ────────────── 取值编辑器 ────────────── */

interface MetaEditorProps {
  fields: KnowledgeFieldDef[];
  value: KnowledgeMeta;
  onChange: (meta: KnowledgeMeta) => void;
}

export function MetaEditor({ fields, value, onChange }: MetaEditorProps) {
  const set = (key: string, v: string | number | boolean | undefined) => {
    const next = { ...value };
    if (v === undefined || v === '') delete next[key];
    else next[key] = v;
    onChange(next);
  };

  if (fields.length === 0) {
    return (
      <div className="text-[11px] py-1" style={{ color: 'var(--text-tertiary)' }}>
        该知识库未定义专属属性（可在「编辑知识库」里补充）
      </div>
    );
  }

  return (
    <div className="grid grid-cols-2 gap-3">
      {fields.map((f) => {
        const v = value[f.key];
        return (
          <div key={f.key}>
            <label className="block text-[11px] mb-1" style={{ color: 'var(--text-secondary)' }}>
              {f.label}
              {f.required && <span style={{ color: '#EF4444' }}> *</span>}
            </label>
            {f.type === 'boolean' || f.type === 'select' ? (
              <Select
                value={v === undefined ? '' : String(v)}
                onChange={(nv) => set(f.key, f.type === 'boolean' ? (nv === '' ? undefined : nv === 'true') : nv)}
                options={[
                  { value: '', label: '未设置' },
                  ...(f.type === 'boolean'
                    ? [{ value: 'true', label: '是' }, { value: 'false', label: '否' }]
                    : (f.options ?? []).map((o) => ({ value: o, label: o }))),
                ]}
                size="sm"
                className="w-full"
                placeholder="未设置"
              />
            ) : (
              <input
                type={f.type === 'number' ? 'number' : f.type === 'date' ? 'date' : 'text'}
                value={v === undefined ? '' : String(v)}
                onChange={(e) => {
                  const raw = e.target.value;
                  if (f.type === 'number') set(f.key, raw === '' ? undefined : Number(raw));
                  else set(f.key, raw);
                }}
                placeholder="未设置"
                className={`${inputBase} w-full`}
                style={inputStyle}
              />
            )}
          </div>
        );
      })}
    </div>
  );
}

/* ────────────── 定义编辑器 ────────────── */

interface FieldSchemaEditorProps {
  fields: KnowledgeFieldDef[];
  onChange: (fields: KnowledgeFieldDef[]) => void;
}

export function FieldSchemaEditor({ fields, onChange }: FieldSchemaEditorProps) {
  const patch = (i: number, p: Partial<KnowledgeFieldDef>) =>
    onChange(fields.map((f, idx) => (idx === i ? { ...f, ...p } : f)));

  return (
    <div className="space-y-2">
      {fields.map((f, i) => (
        // flex-wrap 只是兜底：行内固定列加起来超过可用宽度时才换行，绝不允许溢出弹窗
        <div key={i} className="flex flex-wrap items-start gap-2 min-w-0">
          <input
            value={f.key}
            onChange={(e) => patch(i, { key: e.target.value })}
            placeholder="存储键"
            className={`${inputBase} w-28 shrink-0`}
            style={inputStyle}
            title="落在关联行 kb_meta 里的键名（短英文）"
          />
          <input
            value={f.label}
            onChange={(e) => patch(i, { label: e.target.value })}
            placeholder="展示名"
            className={`${inputBase} w-28 shrink-0`}
            style={inputStyle}
          />
          <div className="w-24 shrink-0">
            <Select
              value={f.type}
              onChange={(t) => patch(i, { type: t as KnowledgeFieldType })}
              options={FIELD_TYPE_OPTIONS}
              size="sm"
              className="w-full"
            />
          </div>
          {f.type === 'select' ? (
            <SelectOptionsInput
              options={f.options ?? []}
              onOptionsChange={(options) => patch(i, { options })}
              placeholder="候选项，逗号分隔（中英文逗号都行）"
              className={`${inputBase} flex-1 min-w-0`}
              style={inputStyle}
            />
          ) : (
            <div className="flex-1 min-w-0 text-[10px] py-1" style={{ color: 'var(--text-tertiary)' }}>
              {/* 每种类型都给一句说明：否则 text/date 会留一块空槽，看着像少了个控件 */}
              {f.type === 'number' ? '数值，可用于区间筛选'
                : f.type === 'boolean' ? '是 / 否，可参与同属性连边'
                  : f.type === 'date' ? '日期，可用于时间区间筛选'
                    : '自由文本'}
            </div>
          )}
          <label className="shrink-0 flex items-center gap-1 text-[10px] py-1 cursor-pointer" style={{ color: 'var(--text-secondary)' }}>
            <input type="checkbox" checked={!!f.required} onChange={(e) => patch(i, { required: e.target.checked })} />
            必填
          </label>
          <button
            onClick={() => onChange(fields.filter((_, idx) => idx !== i))}
            className="pd-btn p-1 rounded shrink-0 hover:opacity-80"
            style={{ color: 'var(--status-danger, #EF4444)' }}
            title="删除字段"
          >
            <Trash2 size={12} />
          </button>
        </div>
      ))}
      <button
        onClick={() => onChange([...fields, { key: '', label: '', type: 'text' }])}
        className="pd-btn flex items-center gap-1 px-2 py-1 rounded-lg text-[11px]"
        style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
      >
        <Plus size={11} />
        添加专属属性字段
      </button>
    </div>
  );
}
