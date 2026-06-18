import { useState, useEffect } from 'react';
import { Brain, Search, Clock } from 'lucide-react';
import { AgentBadge } from '../common/AgentBadge';

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
        <h3 className="text-sm font-medium mb-2" style={{ color: 'var(--text-primary)' }}>
          Agent 记忆
        </h3>
        <div className="relative">
          <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索记忆..."
            className="search-input"
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
          <div className="flex flex-col gap-2 px-3 py-2">
            {filteredMemories.map((memory) => (
              <button
                key={memory.id}
                onClick={() => {
                  setSelectedMemory(memory);
                  onSelect?.(memory.content);
                }}
                className="w-full text-left px-3 py-2.5 transition-colors hover:scale-[1.01] flex flex-col rounded-xl"
                style={{
                  backgroundColor: 'var(--bg-secondary)',
                  border: '1px solid var(--border)',
                  alignItems: 'stretch',
                }}
              >
                {/* Header row: Agent tag + session title + date */}
                <div className="flex items-center gap-2 mb-1 min-w-0">
                  <AgentBadge agentType={memory.agentType as 'claude' | 'hermes' | 'codex' | 'api'} />
                  <span className="text-[10px] truncate min-w-0 font-semibold" style={{ color: 'var(--text-primary)' }}>
                    {memory.sessionTitle}
                  </span>
                  <span className="text-[10px] shrink-0 ml-auto" style={{ color: 'var(--text-tertiary)' }}>
                    {formatTime(memory.timestamp)}
                  </span>
                </div>
                {/* Content preview */}
                <p
                  className="text-xs leading-relaxed line-clamp-2 mb-1 break-words"
                  style={{ color: 'var(--text-secondary)' }}
                >
                  {memory.content.length > 120 ? memory.content.slice(0, 120) + '...' : memory.content}
                </p>

              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
