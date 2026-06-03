import { useState, useEffect, useRef, useCallback } from 'react';
import { Search, X, Cpu, Loader2 } from 'lucide-react';
import { useSkillStore, type SkillInfo } from '../../stores/skillStore';
import { AGENT_THEMES } from '../../types';

interface SkillPickerProps {
  agentType: string;
  onSelect: (name: string, description: string) => void;
  onClose: () => void;
}

export function SkillPicker({ agentType, onSelect, onClose }: SkillPickerProps) {
  const { skillsByAgent, isLoading } = useSkillStore();
  const skills = skillsByAgent[agentType] || [];
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  const filteredSkills = query
    ? skills.filter(
        (s) =>
          s.name.toLowerCase().includes(query.toLowerCase()) ||
          s.description.toLowerCase().includes(query.toLowerCase())
      )
    : skills;

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleSelect = useCallback(
    (name: string, description: string) => {
      onSelect(name, description);
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
        setSelectedIndex((i) => Math.min(i + 1, filteredSkills.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (filteredSkills[selectedIndex]) {
          handleSelect(filteredSkills[selectedIndex].name, filteredSkills[selectedIndex].description);
        }
      }
    },
    [onClose, filteredSkills, selectedIndex, handleSelect]
  );

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
          onChange={(e) => {
            setQuery(e.target.value);
            setSelectedIndex(0);
          }}
          placeholder="搜索技能..."
          className="flex-1 text-xs outline-none bg-transparent"
          style={{ color: 'var(--text-primary)' }}
        />
        <button onClick={onClose} className="p-0.5" style={{ color: 'var(--text-tertiary)' }}>
          <X size={12} />
        </button>
      </div>

      {/* List */}
      <div className="max-h-64 overflow-y-auto">
        {isLoading ? (
          <div className="flex items-center justify-center gap-2 py-6">
            <Loader2 size={14} className="animate-spin" style={{ color: 'var(--text-secondary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载技能中...</span>
          </div>
        ) : filteredSkills.length === 0 ? (
          <div className="px-3 py-4 text-center">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {query ? '未找到匹配技能' : '暂无可用技能'}
            </span>
          </div>
        ) : (
          filteredSkills.slice(0, 10).map((skill, idx) => (
            <button
              key={skill.name}
              onClick={() => handleSelect(skill.name, skill.description)}
              className="w-full flex items-start gap-2 px-3 py-2 text-left transition-colors"
              style={{
                backgroundColor: idx === selectedIndex ? 'var(--bg-tertiary)' : 'transparent',
              }}
            >
              <Cpu size={14} className="mt-0.5 shrink-0" style={{ color: AGENT_THEMES.hermes.cssVar }} />
              <div className="flex-1 min-w-0">
                <div className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                  {skill.name}
                </div>
                <div className="text-[10px] truncate mt-0.5" style={{ color: 'var(--text-secondary)' }}>
                  {skill.description}
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
