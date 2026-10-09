import { useState } from 'react';
import { Plus, X } from 'lucide-react';

/**
 * TagEditor — 灵感的标签**增删编辑**（受控组件）。
 *
 * 独立页的弹窗表单与会话侧栏的内联表单共用这一份：侧栏此前只能"原样保留"已有标签、
 * 改不了（它的内联表单压根没有标签输入），根因就是这块能力只写在弹窗表单里。
 *
 * 交互：输入后回车或点 + 添加；点标签上的 × 移除。空串忽略、重复忽略。
 */
interface TagEditorProps {
  tags: string[];
  onChange: (tags: string[]) => void;
  /** 紧凑档：更小尺寸，用于会话侧栏 */
  compact?: boolean;
}

export function TagEditor({ tags, onChange, compact }: TagEditorProps) {
  const [input, setInput] = useState('');

  const addTag = () => {
    const t = input.trim();
    if (!t) return;
    // 重复标签直接忽略（后端也是以 (灵感, 标签) 为主键，重复插入本就无效）
    if (!tags.includes(t)) onChange([...tags, t]);
    setInput('');
  };

  const removeTag = (tag: string) => onChange(tags.filter((t) => t !== tag));

  const chipCls = compact ? 'text-[10px] px-1.5 py-0.5 gap-0.5' : 'text-xs px-2 py-0.5 gap-1';
  const inputCls = compact
    ? 'px-1.5 py-0.5 rounded-full text-[10px] outline-none w-16'
    : 'px-2 py-0.5 rounded-full text-xs outline-none w-20';

  return (
    <div className="flex items-center gap-2 flex-wrap">
      {tags.map((tag) => (
        <span
          key={tag}
          className={`flex items-center rounded-full ${chipCls}`}
          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
        >
          #{tag}
          <button onClick={() => removeTag(tag)} title={`移除标签 ${tag}`}>
            <X size={compact ? 9 : 10} />
          </button>
        </span>
      ))}
      <div className="flex items-center gap-1">
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              addTag();
            }
          }}
          placeholder={compact ? '加标签...' : '添加标签...'}
          className={`rounded-full ${inputCls}`}
          style={{
            backgroundColor: 'var(--bg-field)',
            color: 'var(--text-primary)',
            border: '1px solid var(--border)',
          }}
        />
        <button
          onClick={addTag}
          disabled={!input.trim()}
          className="pd-btn p-0.5 disabled:opacity-40"
          style={{ color: 'var(--text-secondary)' }}
          title="添加标签"
        >
          <Plus size={compact ? 12 : 14} />
        </button>
      </div>
    </div>
  );
}
