import { useState, useEffect, useRef } from 'react';
import { X, Plus } from 'lucide-react';
import type { InspirationItem } from '../../stores/inspirationStore';

const EMOJI_OPTIONS = ['💡', '🔥', '🎯', '⚡', '🚀', '🌟', '📝', '🔧', '🎨', '🧠', '📊', '🛠️', '💻', '📱', '🌐', '🔬', '🎭', '🎵', '🏆', '💎', '🌈', '🔑', '📖', '🧩'];

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
  const [tagInput, setTagInput] = useState('');
  const [saving, setSaving] = useState(false);
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!initialData && !prefill) {
      titleRef.current?.focus();
    }
  }, [initialData, prefill]);

  const addTag = () => {
    const t = tagInput.trim();
    if (t && !tags.includes(t)) {
      setTags([...tags, t]);
      setTagInput('');
    }
  };

  const removeTag = (tag: string) => {
    setTags(tags.filter((t) => t !== tag));
  };

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
          <h3 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
            {initialData ? '编辑灵感' : '新建灵感'}
          </h3>
          <button onClick={onCancel} className="p-1 rounded" style={{ color: 'var(--text-secondary)' }}>
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

          {/* Tags */}
          <div>
            <div className="flex items-center gap-2 flex-wrap">
              {tags.map((tag) => (
                <span
                  key={tag}
                  className="flex items-center gap-1 px-2 py-0.5 rounded-full text-xs"
                  style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                >
                  #{tag}
                  <button onClick={() => removeTag(tag)}>
                    <X size={10} />
                  </button>
                </span>
              ))}
              <div className="flex items-center gap-1">
                <input
                  type="text"
                  value={tagInput}
                  onChange={(e) => setTagInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault();
                      addTag();
                    }
                  }}
                  placeholder="添加标签..."
                  className="px-2 py-0.5 rounded-full text-xs outline-none w-20"
                  style={{
                    backgroundColor: 'var(--bg-tertiary)',
                    color: 'var(--text-primary)',
                    border: '1px solid var(--border)',
                  }}
                />
                <button onClick={addTag} className="p-0.5" style={{ color: 'var(--text-secondary)' }}>
                  <Plus size={14} />
                </button>
              </div>
            </div>
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end gap-2 px-5 py-3" style={{ borderTop: '1px solid var(--border)' }}>
          <button
            onClick={onCancel}
            className="px-4 py-2 rounded-lg text-sm"
            style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)' }}
          >
            取消
          </button>
          <button
            onClick={handleSave}
            disabled={!title.trim() || saving}
            className="px-4 py-2 rounded-lg text-sm font-medium disabled:opacity-50"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            {saving ? '保存中...' : '保存'}
          </button>
        </div>
      </div>
    </div>
  );
}
