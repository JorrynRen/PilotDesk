import { useState, useEffect } from 'react';
import { Brain, Search, Clock, Tag } from 'lucide-react';
import { AGENT_THEMES } from '../../types';

interface MemoryItem {
  id: string;
  content: string;
  role: string;
  timestamp: number;
  sessionTitle: string;
  agentType: string;
  mode: string;
}

interface MemoryBrowserProps {
  agentType?: string;
  onSelect?: (content: string) => void;
}

export function MemoryBrowser({ agentType, onSelect }: MemoryBrowserProps) {
  const [memories, setMemories] = useState<MemoryItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedMemory, setSelectedMemory] = useState<MemoryItem | null>(null);

  useEffect(() => {
    loadMemories();
  }, [agentType]);

  const loadMemories = async () => {
    setLoading(true);
    // Load recent messages as "memories" (would use dedicated Tauri commands)
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      // Use existing get_session_messages for current sessions
      // In a full implementation, this would query a dedicated memories table
      const sessions = await invoke<Array<{ id: string; title: string; agentType: string }>>('list_sessions');
      const recentMemories: MemoryItem[] = [];

      for (const session of sessions.slice(0, 5)) {
        try {
          const msgs = await invoke<Array<{ id: string; role: string; content: string; timestamp: number; mode: string }>>('get_session_messages', { sessionId: session.id });
          for (const msg of msgs.slice(-3)) { // Last 3 messages per session
            if (msg.role === 'assistant') {
              recentMemories.push({
                id: msg.id,
                content: msg.content,
                role: msg.role,
                timestamp: msg.timestamp,
                sessionTitle: session.title,
                agentType: session.agentType,
                mode: msg.mode,
              });
            }
          }
        } catch {
          // Skip sessions with errors
        }
      }

      recentMemories.sort((a, b) => b.timestamp - a.timestamp);
      setMemories(recentMemories.slice(0, 20));
    } catch {
      // Fallback: show empty state
      setMemories([]);
    }
    setLoading(false);
  };

  const filteredMemories = searchQuery
    ? memories.filter((m) => m.content.toLowerCase().includes(searchQuery.toLowerCase()))
    : memories;

  const formatTime = (ts: number) => {
    const d = new Date(ts * 1000);
    return d.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-4 py-3" style={{ borderBottom: '1px solid var(--border)' }}>
        <h3 className="text-sm font-semibold mb-2" style={{ color: 'var(--text-primary)' }}>
          Agent 记忆
        </h3>
        <div className="relative">
          <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索记忆..."
            className="w-full pl-8 pr-3 py-1.5 rounded-lg text-xs outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
        </div>
      </div>

      {/* Memory list */}
      <div className="flex-1 overflow-y-auto">
        {loading ? (
          <div className="flex items-center justify-center h-24">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
          </div>
        ) : filteredMemories.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-24 gap-2">
            <Brain size={24} style={{ color: 'var(--text-tertiary)' }} />
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配记忆' : '暂无 Agent 记忆'}
            </span>
          </div>
        ) : (
          <div className="divide-y" style={{ borderColor: 'var(--border)' }}>
            {filteredMemories.map((memory) => (
              <button
                key={memory.id}
                onClick={() => {
                  setSelectedMemory(memory);
                  onSelect?.(memory.content);
                }}
                className="w-full text-left px-4 py-3 transition-colors hover:bg-[var(--bg-tertiary)]"
                style={{
                  backgroundColor: selectedMemory?.id === memory.id ? 'var(--bg-tertiary)' : 'transparent',
                }}
              >
                {/* Meta */}
                <div className="flex items-center gap-2 mb-1">
                  <span
                    className="text-[10px] px-1.5 py-0.5 rounded"
                    style={{
                      backgroundColor: (AGENT_THEMES[memory.agentType] ?? AGENT_THEMES.claude).cssVar,
                      color: '#fff',
                    }}
                  >
                    {(AGENT_THEMES[memory.agentType] ?? AGENT_THEMES.claude).label}
                  </span>
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    {memory.sessionTitle}
                  </span>
                </div>
                {/* Content preview */}
                <p
                  className="text-xs leading-relaxed line-clamp-2 mb-1"
                  style={{ color: 'var(--text-secondary)' }}
                >
                  {memory.content.length > 120 ? memory.content.slice(0, 120) + '...' : memory.content}
                </p>
                {/* Footer */}
                <div className="flex items-center gap-2">
                  <Clock size={10} style={{ color: 'var(--text-tertiary)' }} />
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    {formatTime(memory.timestamp)}
                  </span>
                  <Tag size={10} style={{ color: 'var(--text-tertiary)' }} />
                  <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                    {memory.mode}
                  </span>
                </div>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
