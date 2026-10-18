/**
 * TagEditor — 标签 chip 列表 + 输入框（回车 / 逗号提交，退格删最后一个）。
 *
 * 个人知识库与飞地团队知识库共用同一套展示与交互。
 * 用 `pd-field` 把聚焦反馈交给容器 —— 否则全局 `:focus-visible` 的描边会落在内部
 * `<input>` 上，出现「边框在外、焦点圈在内」的错位。
 */

import { useState } from 'react';
import { X } from 'lucide-react';

interface TagEditorProps {
  tags: string[];
  onChange: (tags: string[]) => void;
  placeholder?: string;
}

export function TagEditor({ tags, onChange, placeholder }: TagEditorProps) {
  const [input, setInput] = useState('');
  const add = (raw: string) => {
    const t = raw.trim().replace(/^#/, '').trim();
    if (t && !tags.includes(t)) onChange([...tags, t]);
    setInput('');
  };
  return (
    <div
      className="pd-field flex items-center gap-1.5 flex-wrap px-2 py-1.5 rounded-lg"
      style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
    >
      {tags.map((t) => (
        <span
          key={t}
          className="flex items-center gap-1 px-1.5 py-[1px] rounded text-[10px]"
          style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-secondary)' }}
        >
          #{t}
          <button
            onClick={() => onChange(tags.filter((x) => x !== t))}
            style={{ color: 'var(--text-tertiary)' }}
            title="移除这个标签"
          >
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
        placeholder={placeholder ?? (tags.length === 0 ? '加标签，回车确认' : '继续加…')}
        className="min-w-[110px] flex-1 bg-transparent outline-none text-[10px]"
        style={{ color: 'var(--text-primary)' }}
      />
    </div>
  );
}
