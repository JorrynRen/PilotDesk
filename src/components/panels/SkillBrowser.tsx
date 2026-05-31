import { useState, useEffect } from 'react';
import { Cpu, Search, ChevronRight, FolderOpen } from 'lucide-react';

interface SkillEntry {
  name: string;
  description: string;
  path: string;
  isDir: boolean;
}

interface SkillBrowserProps {
  agentType: string;
  onSkillSelect?: (name: string) => void;
}

export function SkillBrowser({ agentType, onSkillSelect }: SkillBrowserProps) {
  const [skills, setSkills] = useState<SkillEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedSkill, setSelectedSkill] = useState<SkillEntry | null>(null);

  useEffect(() => {
    loadSkills();
  }, [agentType]);

  const loadSkills = async () => {
    setLoading(true);
    // For Claude Code: skills are managed differently (via MCP servers)
    // For Hermes: skills are in ~/.hermes/skills/
    if (agentType === 'hermes') {
      // Use fallback demo data until Task 12 adds Tauri commands
      setSkills([
        { name: 'code-review', description: '代码审查与优化建议', path: '~/.hermes/skills/code-review', isDir: true },
        { name: 'translate', description: '多语言翻译', path: '~/.hermes/skills/translate', isDir: true },
        { name: 'summarize', description: '文本摘要与总结', path: '~/.hermes/skills/summarize', isDir: true },
        { name: 'debug', description: '代码调试与错误诊断', path: '~/.hermes/skills/debug', isDir: true },
        { name: 'refactor', description: '代码重构', path: '~/.hermes/skills/refactor', isDir: true },
        { name: 'test-gen', description: '单元测试生成', path: '~/.hermes/skills/test-gen', isDir: true },
        { name: 'doc-gen', description: '文档生成', path: '~/.hermes/skills/doc-gen', isDir: true },
      ]);
    } else if (agentType === 'claude') {
      setSkills([
        { name: 'claude-code (native)', description: 'Claude Code 原生能力 - 内置的代码理解和生成', path: 'builtin', isDir: false },
        { name: 'MCP Servers', description: '通过 MCP 协议连接的外部工具服务器', path: 'mcp', isDir: true },
      ]);
    }
    setLoading(false);
  };

  const filteredSkills = searchQuery
    ? skills.filter(
        (s) =>
          s.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
          s.description.toLowerCase().includes(searchQuery.toLowerCase())
      )
    : skills;

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-4 py-3" style={{ borderBottom: '1px solid var(--border)' }}>
        <h3 className="text-sm font-semibold mb-2" style={{ color: 'var(--text-primary)' }}>
          {agentType === 'claude' ? 'Claude Code 技能' : 'Hermes Agent 技能'}
        </h3>
        <div className="relative">
          <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索技能..."
            className="w-full pl-8 pr-3 py-1.5 rounded-lg text-xs outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
        </div>
      </div>

      {/* Skills list */}
      <div className="flex-1 overflow-y-auto">
        {loading ? (
          <div className="flex items-center justify-center h-24">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
          </div>
        ) : filteredSkills.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-24 gap-2">
            <FolderOpen size={24} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配技能' : '暂无已安装技能'}
            </span>
          </div>
        ) : (
          <div className="divide-y" style={{ borderColor: 'var(--border)' }}>
            {filteredSkills.map((skill) => (
              <button
                key={skill.name}
                onClick={() => {
                  setSelectedSkill(skill);
                  onSkillSelect?.(skill.name);
                }}
                className="w-full flex items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-[var(--bg-tertiary)]"
                style={{
                  backgroundColor: selectedSkill?.name === skill.name ? 'var(--bg-tertiary)' : 'transparent',
                }}
              >
                <div className="w-8 h-8 rounded-lg flex items-center justify-center shrink-0" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                  <Cpu size={16} style={{ color: agentType === 'claude' ? 'var(--claude-tag)' : 'var(--hermes-tag)' }} />
                </div>
                <div className="flex-1 min-w-0">
                  <div className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                    {skill.name}
                  </div>
                  <div className="text-[10px] mt-0.5" style={{ color: 'var(--text-secondary)' }}>
                    {skill.description}
                  </div>
                </div>
                <ChevronRight size={14} style={{ color: 'var(--text-tertiary)' }} />
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Skill detail */}
      {selectedSkill && (
        <div className="px-4 py-3" style={{ borderTop: '1px solid var(--border)' }}>
          <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>
            <span style={{ color: 'var(--text-tertiary)' }}>路径:</span> {selectedSkill.path}
          </div>
        </div>
      )}
    </div>
  );
}
