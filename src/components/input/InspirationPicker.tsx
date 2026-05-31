import { useState, useEffect, useRef, useCallback } from 'react';
import { Search, X } from 'lucide-react';
import { useInspirationStore, type InspirationItem } from '../../stores/inspirationStore';

interface InspirationPickerProps {
  onSelect: (content: string) => void;
  onClose: () => void;
}

export function InspirationPicker({ onSelect, onClose }: InspirationPickerProps) {
  // Only read inspirations, never mutate the global store from this component
  const inspirations = useInspirationStore((s) => s.inspirations);
  const loading = useInspirationStore((s) => s.loading);
  const [query, setQuery] = useState('');
  const [filtered, setFiltered] = useState<InspirationItem[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Focus input on mount
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Local filtering only — never calls searchInspirations/fetchInspirations
  // so the right panel's global state is never polluted
  useEffect(() => {
    setSelectedIndex(0);
    if (!query.trim()) {
      setFiltered(inspirations);
    } else {
      const q = query.toLowerCase();
      setFiltered(
        inspirations.filter(
          (insp) =>
            insp.title.toLowerCase().includes(q) ||
            insp.content.toLowerCase().includes(q) ||
            insp.icon.includes(q)
        )
      );
    }
  }, [query, inspirations]);

  const handleSearch = useCallback(
    (q: string) => {
      setQuery(q);
    },
    []
  );

  const handleSelect = useCallback(
    (content: string) => {
      onSelect(content);
    },
    [onSelect]
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (filtered[selectedIndex]) {
          handleSelect(filtered[selectedIndex].content);
        }
      }
    },
    [onClose, filtered, selectedIndex, handleSelect]
  );

  // Click outside to close
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [onClose]);

  return (
    <div
      ref={containerRef}
      className="absolute bottom-full left-0 mb-2 w-80 rounded-xl shadow-xl overflow-hidden z-50"
      style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
      onKeyDown={handleKeyDown}
    >
      {/* Search */}
      <div className="flex items-center gap-2 px-3 py-2" style={{ borderBottom: '1px solid var(--border)' }}>
        <Search size={14} style={{ color: 'var(--text-tertiary)' }} />
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => handleSearch(e.target.value)}
          placeholder="搜索灵感..."
          className="flex-1 text-xs outline-none bg-transparent"
          style={{ color: 'var(--text-primary)' }}
        />
        <button onClick={onClose} className="p-0.5" style={{ color: 'var(--text-tertiary)' }}>
          <X size={12} />
        </button>
      </div>

      {/* List */}
      <div className="max-h-64 overflow-y-auto">
        {loading && !filtered.length ? (
          <div className="px-3 py-4 text-center">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>搜索中...</span>
          </div>
        ) : filtered.length === 0 ? (
          <div className="px-3 py-4 text-center">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {query ? '未找到匹配灵感' : '暂无灵感'}
            </span>
          </div>
        ) : (
          filtered.slice(0, 10).map((insp, idx) => (
            <button
              key={insp.id}
              onClick={() => handleSelect(insp.content)}
              className="w-full flex items-start gap-2 px-3 py-2 text-left transition-colors"
              style={{
                backgroundColor: idx === selectedIndex ? 'var(--bg-tertiary)' : 'transparent',
              }}
            >
              <span className="text-base shrink-0 mt-0.5">{insp.icon}</span>
              <div className="flex-1 min-w-0">
                <div className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }}>
                  {insp.title}
                </div>
                <div className="text-[10px] truncate mt-0.5" style={{ color: 'var(--text-secondary)' }}>
                  {insp.content.slice(0, 80)}
                </div>
              </div>
            </button>
          ))
        )}
      </div>

      {/* Footer hint */}
      <div className="px-3 py-1.5" style={{ borderTop: '1px solid var(--border)' }}>
        <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          ↑↓ 导航 / Enter 选择 / Esc 关闭
        </span>
      </div>
    </div>
  );
}
