/**
 * 搜索命中呈现（个人库与团队库共用）。
 *
 * 查询词是用户输入的自由文本，这里只做大小写不敏感的**子串**匹配，
 * 不解析正则 / FTS 语法，避免特殊字符让渲染报错。
 */

import type { ReactNode } from 'react';

/** 命中高亮：把查询词命中的片段包一层强调底色；无命中时原样返回 */
export function highlightText(text: string, query: string): ReactNode {
  const q = query.trim();
  if (!q || !text) return text;
  const lower = text.toLowerCase();
  const needle = q.toLowerCase();
  let idx = lower.indexOf(needle);
  if (idx < 0) return text;
  const parts: ReactNode[] = [];
  let from = 0;
  let key = 0;
  while (idx >= 0) {
    if (idx > from) parts.push(text.slice(from, idx));
    parts.push(
      <mark
        key={key++}
        style={{
          backgroundColor: 'var(--accent-light)',
          color: 'var(--accent)',
          borderRadius: 2,
          padding: '0 1px',
        }}
      >
        {text.slice(idx, idx + needle.length)}
      </mark>,
    );
    from = idx + needle.length;
    idx = lower.indexOf(needle, from);
  }
  if (from < text.length) parts.push(text.slice(from));
  return parts;
}

/** 命中片段：以首个命中为中心截取，避免只看得到正文开头；未命中则取开头 */
export function matchSnippet(content: string, query: string, max = 160): string {
  const text = content.replace(/\s+/g, ' ').trim();
  const q = query.trim();
  if (!q) return text.slice(0, max);
  const idx = text.toLowerCase().indexOf(q.toLowerCase());
  if (idx < 0) return text.slice(0, max);
  const start = Math.max(0, idx - Math.floor(max / 3));
  const end = Math.min(text.length, start + max);
  return `${start > 0 ? '…' : ''}${text.slice(start, end)}${end < text.length ? '…' : ''}`;
}
