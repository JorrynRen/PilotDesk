/**
 * KnowledgeBaseDialog — 知识库定义弹窗
 *
 * 用户在这里只维护「基本信息」三样：名称、描述、专属属性字段。
 * 刻意不放服务范围与注入上限：知识库本质是 KV 记忆的一个集合，任何场景都可被使用，
 * 命中与注入由统一的 KV 记忆策略负责 —— 在库上再挂一套策略只会让人以为"一个库一套口径"。
 */

import { useState } from 'react';
import { X, Sparkles } from 'lucide-react';
import { FieldSchemaEditor } from './KnowledgeFieldInputs';
import type { KnowledgeBase, KnowledgeFieldDef } from '../../types/knowledge';

interface KnowledgeBaseDialogProps {
  /** null = 新建 */
  base: KnowledgeBase | null;
  onClose: () => void;
  onSubmit: (input: { name: string; description: string; fields: KnowledgeFieldDef[] }) => void;
}

const inputStyle = {
  backgroundColor: 'var(--bg-tertiary)',
  color: 'var(--text-primary)',
  border: '1px solid var(--border)',
} as const;

export function KnowledgeBaseDialog({ base, onClose, onSubmit }: KnowledgeBaseDialogProps) {
  const [name, setName] = useState(base?.name ?? '');
  const [description, setDescription] = useState(base?.description ?? '');
  const [fields, setFields] = useState<KnowledgeFieldDef[]>(base?.fields ?? []);
  const [error, setError] = useState('');

  const handleSubmit = () => {
    const trimmed = name.trim();
    if (!trimmed) {
      setError('请填写知识库名称');
      return;
    }
    const keys = fields.map((f) => f.key.trim());
    if (keys.some((k) => !k)) {
      setError('专属属性字段的「存储键」不能为空');
      return;
    }
    if (new Set(keys).size !== keys.length) {
      setError('专属属性字段的「存储键」不能重复');
      return;
    }
    const labels = fields.map((f) => f.label.trim());
    if (labels.some((l) => !l)) {
      setError('专属属性字段的「展示名」不能为空');
      return;
    }
    onSubmit({
      name: trimmed,
      description: description.trim(),
      fields: fields.map((f) => ({ ...f, key: f.key.trim(), label: f.label.trim() })),
    });
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[720px] max-w-[92vw] max-h-[86vh] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 px-5 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <Sparkles size={15} style={{ color: 'var(--accent)', flexShrink: 0 }} />
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
            {base ? `编辑知识库「${base.name}」` : '新建知识库'}
          </span>
          <div className="flex-1" />
          <button onClick={onClose} className="pd-btn p-1 rounded hover:opacity-80" style={{ color: 'var(--text-tertiary)' }} title="关闭">
            <X size={14} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-5">
          <div className="space-y-3">
            <div>
              <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>名称</label>
              <input
                value={name}
                onChange={(e) => { setName(e.target.value); setError(''); }}
                placeholder="例如：PilotDesk 开发"
                className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none"
                style={inputStyle}
                autoFocus
              />
            </div>
            <div>
              <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>描述</label>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="这个库收什么、什么场景该来查它（会用于自动命中的判断）"
                rows={2}
                className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none resize-none"
                style={inputStyle}
              />
            </div>
          </div>

          <div>
            <div className="flex items-baseline gap-2 mb-2">
              <label className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>专属属性字段</label>
              <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                标签 / 热度 / 重要由记忆表自带，无需在这里定义；单选与是/否字段还会自动参与关系连边
              </span>
            </div>
            <FieldSchemaEditor fields={fields} onChange={setFields} />
          </div>

          {error && (
            <div className="text-[11px] px-3 py-2 rounded" style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}>
              {error}
            </div>
          )}
        </div>

        <div className="px-5 py-3 shrink-0 flex items-center gap-2" style={{ borderTop: '1px solid var(--border)' }}>
          <span className="text-[10px] flex-1 leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            你只需定义这三样。服务范围不限定（任何场景都可被使用）；命中与注入条数跟随全局 KV 记忆策略。
          </span>
          <button
            onClick={onClose}
            className="pd-btn px-3 py-1.5 rounded-lg text-xs"
            style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
          >
            取消
          </button>
          <button
            onClick={handleSubmit}
            className="pd-btn px-3 py-1.5 rounded-lg text-xs"
            style={{ backgroundColor: 'var(--accent)', color: '#fff', border: 'none' }}
          >
            {base ? '保存' : '创建'}
          </button>
        </div>
      </div>
    </div>
  );
}
