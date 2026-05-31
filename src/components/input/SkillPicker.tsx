import { useState, useEffect, useRef, useCallback } from 'react';
import { Search, X, Cpu } from 'lucide-react';

interface SkillItem {
  name: string;
  description: string;
}

interface SkillPickerProps {
  agentType: string;
  onSelect: (name: string, description: string) => void;
  onClose: () => void;
}

// Built-in skill definitions — works for all agent types without Tauri invoke
const BUILTIN_SKILLS: SkillItem[] = [
  { name: 'code-review', description: '代码审查与优化建议' },
  { name: 'translate', description: '多语言翻译' },
  { name: 'summarize', description: '文本摘要与总结' },
  { name: 'debug', description: '代码调试与错误诊断' },
  { name: 'refactor', description: '代码重构' },
  { name: 'test-gen', description: '单元测试生成' },
  { name: 'doc-gen', description: '文档生成' },
  { name: 'data-analysis', description: '数据分析与可视化' },
  { name: 'sql-helper', description: 'SQL 查询编写与优化' },
  { name: 'git-assist', description: 'Git 操作与版本管理辅助' },
  { name: 'api-design', description: 'API 接口设计与文档' },
  { name: 'perf-tune', description: '性能分析与调优建议' },
];

// Key used to persist user-added skills in localStorage
const CUSTOM_SKILLS_KEY = 'pd-custom-skills';

function loadCustomSkills(): SkillItem[] {
  try {
    const raw = localStorage.getItem(CUSTOM_SKILLS_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

export function SkillPicker({ agentType, onSelect, onClose }: SkillPickerProps) {
  const [skills, setSkills] = useState<SkillItem[]>([...BUILTIN_SKILLS, ...loadCustomSkills()]);
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
        {filteredSkills.length === 0 ? (
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
              <Cpu size={14} className="mt-0.5 shrink-0" style={{ color: 'var(--hermes-tag)' }} />
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
