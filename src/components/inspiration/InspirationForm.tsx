import { useState, useEffect, useRef } from 'react';
import { X } from 'lucide-react';
import type { InspirationItem } from '../../stores/inspirationStore';
import { TagEditor } from './TagEditor';

import { EMOJI_OPTIONS } from '../../constants';

interface InspirationFormProps {
  initialData?: InspirationItem | null;
  prefill?: string;
  sourceAgent?: string;
  onSave: (data: {
    icon?: string;
    title: string;
    content: string;
    sourceAgent?: string;
    tags?: string[];
  }) => Promise<void>;
  onUpdate?: (data: {
    id: string;
    icon?: string;
    title?: string;
    content?: string;
    sourceAgent?: string;
    tags?: string[];
  }) => Promise<void>;
  onCancel: () => void;
}

export function InspirationForm({ initialData, prefill, sourceAgent, onSave, onUpdate, onCancel }: InspirationFormProps) {
  const [icon, setIcon] = useState(initialData?.icon ?? '💡');
  const [title, setTitle] = useState(initialData?.title ?? '');
  const [content, setContent] = useState(initialData?.content ?? prefill ?? '');
  const [tags, setTags] = useState<string[]>(initialData?.tags ?? []);
  const [saving, setSaving] = useState(false);
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!initialData && !prefill) {
      titleRef.current?.focus();
    }
  }, [initialData, prefill]);

  const handleSave = async () => {
    if (!title.trim()) return;
    setSaving(true);
    try {
      if (initialData && onUpdate) {
        await onUpdate({
          id: initialData.id,
          icon: icon !== initialData.icon ? icon : undefined,
          title: title !== initialData.title ? title : undefined,
          content: content !== initialData.content ? content : undefined,
          tags: JSON.stringify(tags) !== JSON.stringify(initialData.tags) ? tags : undefined,
        });
      } else {
        await onSave({
          icon,
          title,
          content,
          sourceAgent: sourceAgent || initialData?.sourceAgent,
          tags,
        });
      }
    } catch (err) {
      console.error('Save inspiration failed:', err);
    }
    setSaving(false);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div
        className="w-[560px] max-h-[80vh] rounded-2xl overflow-hidden flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3" style={{ borderBottom: '1px solid var(--border)' }}>
          <h3 className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
            {initialData ? '编辑灵感' : '新建灵感'}
          </h3>
          <button onClick={onCancel} className="pd-btn p-1 rounded" style={{ color: 'var(--text-secondary)' }}>
            <X size={16} />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-4">
          {/* Icon + Title row */}
          <div className="flex items-center gap-3">
            <div className="relative">
              <button
                onClick={() => setShowEmojiPicker(!showEmojiPicker)}
                className="text-2xl w-10 h-10 flex items-center justify-center rounded-lg"
                style={{ backgroundColor: 'var(--bg-tertiary)' }}
              >
                {icon}
              </button>
              {showEmojiPicker && (
                <div
                  className="absolute top-12 left-0 z-10 grid grid-cols-12 gap-0.5 p-2 rounded-lg shadow-lg"
                  style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)', minWidth: '360px' }}
                >
                  {EMOJI_OPTIONS.map((e) => (
                    <button
                      type="button"
                      key={e}
                      onClick={() => {
                        setIcon(e);
                        setShowEmojiPicker(false);
                      }}
                      className="w-7 h-7 flex items-center justify-center rounded transition-colors hover:bg-[var(--bg-tertiary)]"
                      style={{
                        outline: icon === e ? '1.5px solid var(--accent)' : 'none',
                        outlineOffset: '-1px',
                      }}
                    >
                      {e}
                    </button>
                  ))}
                </div>
              )}
            </div>
            <input
              ref={titleRef}
              type="text"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="灵感标题..."
              className="flex-1 px-3 py-2 rounded-lg text-sm outline-none"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
              }}
            />
          </div>

          {/* Content */}
          <div>
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              placeholder="灵感prompt（支持 Markdown）..."
              rows={6}
              className="w-full px-3 py-2 rounded-lg text-sm outline-none resize-none"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
              }}
            />
          </div>

          {/* Tags：与侧栏内联表单共用 TagEditor */}
          <div>
            <TagEditor tags={tags} onChange={setTags} />
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end gap-2 px-5 py-3" style={{ borderTop: '1px solid var(--border)' }}>
          <button
            onClick={onCancel}
            className="pd-btn px-4 py-2 rounded-lg text-sm"
            style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)' }}
          >
            取消
          </button>
          <button
            onClick={handleSave}
            disabled={!title.trim() || saving}
            className="pd-btn px-4 py-2 rounded-lg text-sm  disabled:opacity-50"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            {saving ? '保存中...' : '保存'}
          </button>
        </div>
      </div>
    </div>
  );
}
