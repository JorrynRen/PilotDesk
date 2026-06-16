import { useState, useEffect } from 'react';
import { Cpu, Search, ChevronRight, FolderOpen, Bot } from 'lucide-react';
import { useSkillStore } from '../../stores/skillStore';
import { useAgentEvent } from '../../hooks/useAgentEvent';
import { AGENT_THEMES } from '../../types';
import type { SkillInfo } from '../../stores/skillStore';

interface SkillBrowserProps {
  agentType: string;
  onSkillSelect?: (name: string) => void;
}

const AGENT_LABELS: Record<string, string> = {
  claude: 'Claude Code',
  hermes: 'Hermes Agent',
  api: 'API 直连',
};

export function SkillBrowser({ agentType, onSkillSelect }: SkillBrowserProps) {
  const { skillsByAgent, isLoading, setAgentSkills, setLoading } = useSkillStore();
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedSkill, setSelectedSkill] = useState<{ agent: string; name: string; description: string; category?: string } | null>(null);

  const { requestAllSkills } = useAgentEvent();

  // Auto-fetch skills on mount
  useEffect(() => {
    const cached = skillsByAgent[agentType];
    if (!cached) {
      setLoading(true);
      requestAllSkills();
    }
  }, [agentType]); // eslint-disable-line react-hooks/exhaustive-deps

  // Subscribe to skillStore changes instead of polling
  useEffect(() => {
    // Check if data already exists
    const existing = useSkillStore.getState().skillsByAgent[agentType];
    if (existing && existing.length > 0) {
      setLoading(false);
      return;
    }

    // Subscribe to store changes
    const unsub = useSkillStore.subscribe((state) => {
      const agentSkills = state.skillsByAgent[agentType];
      if (agentSkills && agentSkills.length > 0) {
        setLoading(false);
      }
    });
    return () => unsub();
  }, [agentType]);

  // 按分类聚合所有 agent 的技能
  const allAgentTypes = Object.keys(skillsByAgent);
  const hasAny = allAgentTypes.length > 0 && allAgentTypes.some(a => (skillsByAgent[a] || []).length > 0);

  // 搜索过滤
  const getFiltered = (agentType: string) => {
    const skills = skillsByAgent[agentType] || [];
    if (!searchQuery) return skills;
    return skills.filter(
      (s) =>
        s.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
        s.description.toLowerCase().includes(searchQuery.toLowerCase())
    );
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-4 py-3" style={{ borderBottom: '1px solid var(--border)' }}>
        <h3 className="text-sm font-semibold mb-2" style={{ color: 'var(--text-primary)' }}>
          全部技能
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

      {/* Skills list by agent */}
      <div className="flex-1 overflow-y-auto">
        {isLoading ? (
          <div className="flex items-center justify-center h-24">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
          </div>
        ) : !hasAny ? (
          <div className="flex flex-col items-center justify-center h-24 gap-2">
            <FolderOpen size={24} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配技能' : '暂无已安装技能'}
            </span>
          </div>
        ) : (
          allAgentTypes.map((agt) => {
            const filtered = getFiltered(agt);
            if (filtered.length === 0) return null;

            return (
              <div key={agt}>
                {/* Agent group header */}
                <div
                  className="flex items-center gap-2 px-4 py-2"
                  style={{ backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}
                >
                  <Bot size={12} style={{ color: (AGENT_THEMES[agt] ?? AGENT_THEMES.claude).cssVar }} />
                  <span className="text-[10px] font-semibold uppercase tracking-wide" style={{ color: 'var(--text-secondary)' }}>
                    {AGENT_LABELS[agt] || agt}
                  </span>
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    ({filtered.length})
                  </span>
                </div>

                {/* Skills */}
                {filtered.map((skill) => (
                  <button
                    key={`${agt}:${skill.name}`}
                    onClick={() => {
                      setSelectedSkill({ agent: agt, ...skill });
                      onSkillSelect?.(skill.name);
                    }}
                    className="w-full flex items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-[var(--bg-tertiary)]"
                    style={{
                      backgroundColor: selectedSkill?.agent === agt && selectedSkill?.name === skill.name
                        ? 'var(--bg-tertiary)'
                        : 'transparent',
                      borderBottom: '1px solid var(--border)',
                    }}
                  >
                    <div className="w-8 h-8 rounded-lg flex items-center justify-center shrink-0" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
                      <Cpu size={16} style={{ color: (AGENT_THEMES[agt] ?? AGENT_THEMES.claude).cssVar }} />
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                        {skill.name}
                      </div>
                      <div className="text-[10px] mt-0.5" style={{ color: 'var(--text-secondary)' }}>
                        {skill.description}
                      </div>
                    </div>
                    {skill.category && (
                      <span
                        className="text-[10px] px-1.5 py-0.5 rounded"
                        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}
                      >
                        {skill.category}
                      </span>
                    )}
                    <ChevronRight size={14} style={{ color: 'var(--text-tertiary)' }} />
                  </button>
                ))}
              </div>
            );
          })
        )}
      </div>

      {/* Skill detail */}
      {selectedSkill && (
        <div className="px-4 py-3" style={{ borderTop: '1px solid var(--border)' }}>
          <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>
            <span style={{ color: 'var(--text-tertiary)' }}>来源:</span> {AGENT_LABELS[selectedSkill.agent] || selectedSkill.agent}
            {selectedSkill.category && (
              <span className="ml-2">
                <span style={{ color: 'var(--text-tertiary)' }}>分类:</span> {selectedSkill.category}
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
