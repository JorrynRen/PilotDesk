import { useState, useEffect } from 'react';
import { Cpu, Search, FolderOpen, Bot, ChevronDown, ChevronRight } from 'lucide-react';
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
  const [collapsedAgents, setCollapsedAgents] = useState<Set<string>>(new Set());

  const toggleAgent = (agent: string) => {
    setCollapsedAgents((prev) => {
      const next = new Set(prev);
      if (next.has(agent)) {
        next.delete(agent);
      } else {
        next.add(agent);
      }
      return next;
    });
  };

  const { requestSkills } = useAgentEvent({
    onSkills: (agentType, skills) => {
      setAgentSkills(agentType, skills);
    },
  });

  // Auto-fetch skills: detect installed agents first, then fetch skills for each
  useEffect(() => {
    const fetchAll = async () => {
      setLoading(true);
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const installed = await invoke<string[]>('agent_detect_installed');
        for (const agent of installed) {
          const cached = skillsByAgent[agent];
          if (!cached) {
            await requestSkills(agent);
          }
        }
      } catch {
        // Fallback: try known agents
        for (const agent of ['claude', 'hermes', 'codex']) {
          const cached = skillsByAgent[agent];
          if (!cached) {
            await requestSkills(agent);
          }
        }
      }
      setLoading(false);
    };
    fetchAll();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // 按分类聚合所有 agent 的技能（固定排序：hermes → claude → codex）
  const AGENT_ORDER = ['hermes', 'claude', 'codex'];
  const allAgentTypes = Object.keys(skillsByAgent).sort(
    (a, b) => AGENT_ORDER.indexOf(a) - AGENT_ORDER.indexOf(b)
  );
  const hasAny = allAgentTypes.length > 0;

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
        <h3 className="text-sm font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
          全部技能
        </h3>
        <div className="relative">
          <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索技能..."
            className="search-input"
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
            return (
              <div key={agt}>
                {/* Agent group header — click to toggle */}
                <div
                  className="flex items-center gap-2 px-4 py-2 cursor-pointer select-none"
                  style={{ backgroundColor: 'var(--bg-secondary)', borderBottom: '1px solid var(--border)' }}
                  onClick={() => toggleAgent(agt)}
                >
                  <Bot size={14} style={{ color: (AGENT_THEMES[agt] ?? AGENT_THEMES.claude).cssVar }} />
                  <span className="text-xs font-medium uppercase tracking-wide" style={{ color: 'var(--text-secondary)' }}>
                    {AGENT_LABELS[agt] || agt}
                  </span>
                  <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
                    ({filtered.length})
                  </span>
                  <span className="ml-auto shrink-0 flex items-center" style={{ color: 'var(--text-tertiary)' }}>
                    {collapsedAgents.has(agt) ? <ChevronRight size={16} /> : <ChevronDown size={16} />}
                  </span>
                </div>

                {/* Skills */}
                {!collapsedAgents.has(agt) && (
                  filtered.length > 0 ? filtered.map((skill) => (
                    <button
                      key={`${agt}:${skill.name}`}
                      onClick={() => {
                        setSelectedSkill({ agent: agt, ...skill });
                        onSkillSelect?.(skill.name);
                      }}
                      className="w-full flex items-center gap-2 px-4 py-2.5 text-left transition-colors hover:bg-[var(--bg-tertiary)]"
                      style={{
                        backgroundColor: selectedSkill?.agent === agt && selectedSkill?.name === skill.name
                          ? 'var(--bg-tertiary)'
                          : 'transparent',
                        borderBottom: '1px solid var(--border)',
                      }}
                    >
                      <div className="flex-1 min-w-0">
                        <div className="text-xs flex items-center gap-1.5" style={{ color: 'var(--text-primary)' }}>
                          <Cpu size={12} className="shrink-0" style={{ color: (AGENT_THEMES[agt] ?? AGENT_THEMES.claude).cssVar }} />
                          <span className="truncate">{skill.name}</span>
                        </div>
                        <div className="text-[10px] truncate mt-0.5" style={{ color: 'var(--text-secondary)' }}>
                          {skill.description}
                        </div>
                      </div>

                    </button>
                  )) : (
                    <div className="px-4 py-2 text-[10px]" style={{ color: 'var(--text-tertiary)', borderBottom: '1px solid var(--border)' }}>
                      暂无技能文件
                    </div>
                  )
                )}
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
