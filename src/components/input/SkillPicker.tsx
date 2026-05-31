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

export function SkillPicker({ agentType, onSelect, onClose }: SkillPickerProps) {
  const [skills, setSkills] = useState<SkillItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
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
    loadSkills();
  }, []);

  const loadSkills = async () => {
    if (agentType !== 'hermes') return;
    setLoading(true);
    setError(null);
    try {
      // Read skills from Hermes skills directory
      const skillsDir = await invoke<string>('get_hermes_skills_dir');
      const entries = await invoke<Array<{ name: string; description: string }>>('list_hermes_skills', {
        dir: skillsDir,
      });
      setSkills(entries);
    } catch {
      // Fallback: use hardcoded common skills for demo
      setSkills([
        { name: 'code-review', description: '代码审查与优化建议' },
        { name: 'translate', description: '多语言翻译' },
        { name: 'summarize', description: '文本摘要与总结' },
        { name: 'debug', description: '代码调试与错误诊断' },
        { name: 'refactor', description: '代码重构' },
        { name: 'test-gen', description: '单元测试生成' },
        { name: 'doc-gen', description: '文档生成' },
        { name: 'data-analysis', description: '数据分析与可视化' },
      ]);
    }
    setLoading(false);
  };

  // Stub invoke for type safety - actual commands would come from Tauri
  // These will be added in Task 12 (环境管理)
  async function invoke<T>(_cmd: string, _args?: Record<string, unknown>): Promise<T> {
    throw new Error('Command not yet implemented');
  }

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
        {loading ? (
          <div className="px-3 py-4 text-center">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载技能列表中...</span>
          </div>
        ) : error ? (
          <div className="px-3 py-4 text-center">
            <span className="text-xs" style={{ color: 'var(--danger)' }}>{error}</span>
          </div>
        ) : filteredSkills.length === 0 ? (
          <div className="px-3 py-4 text-center">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {agentType !== 'hermes' ? '技能列表仅支持 Hermes Agent' : query ? '未找到匹配技能' : '暂无已安装技能'}
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
