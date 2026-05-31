import { useState, useEffect } from 'react';
import { Plus, Search, Archive } from 'lucide-react';
import { useSessionStore } from '../../stores/sessionStore';
import { SessionListItem } from './SessionListItem';

export function SessionList() {
  const {
    sessions,
    archivedSessions,
    currentSessionId,
    isLoadingSessions,
    showArchived,
    fetchSessions,
    selectSession,
    createSession,
    archiveSession,
    deleteSession,
    toggleArchived,
  } = useSessionStore();

  const [searchQuery, setSearchQuery] = useState('');
  const [showNewDialog, setShowNewDialog] = useState(false);

  useEffect(() => {
    fetchSessions();
  }, [fetchSessions]);

  const displayList = showArchived ? archivedSessions : sessions;
  const filteredList = displayList.filter((s) =>
    s.title.toLowerCase().includes(searchQuery.toLowerCase())
  );

  // Group by time
  const now = Date.now() / 1000;
  const todayStart = Math.floor(now / 86400) * 86400;
  const yesterdayStart = todayStart - 86400;

  const today = filteredList.filter((s) => s.updatedAt >= todayStart);
  const yesterday = filteredList.filter(
    (s) => s.updatedAt >= yesterdayStart && s.updatedAt < todayStart
  );
  const earlier = filteredList.filter((s) => s.updatedAt < yesterdayStart);

  const renderGroup = (label: string, items: typeof sessions) => {
    if (items.length === 0) return null;
    return (
      <div key={label}>
        <div className="px-3 py-1 text-[10px] font-medium" style={{ color: 'var(--text-secondary)' }}>
          {label}
        </div>
        {items.map((session) => (
          <SessionListItem
            key={session.id}
            session={session}
            isActive={session.id === currentSessionId}
            onSelect={() => selectSession(session.id)}
            onArchive={showArchived ? undefined : () => archiveSession(session.id)}
            onDelete={() => deleteSession(session.id)}
          />
        ))}
      </div>
    );
  };

  return (
    <aside
      className="w-[260px] flex flex-col shrink-0"
      style={{ borderRight: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
    >
      {/* Header */}
      <div className="p-3 flex items-center justify-between" style={{ borderBottom: '1px solid var(--border)' }}>
        <span className="text-xs font-semibold uppercase tracking-wider" style={{ color: 'var(--text-secondary)' }}>
          会话列表
        </span>
        <button
          className="p-1 rounded transition-colors hover:opacity-80"
          style={{ color: 'var(--accent)' }}
          title="新建会话"
          onClick={() => setShowNewDialog(true)}
        >
          <Plus size={16} />
        </button>
      </div>

      {/* Search */}
      <div className="px-3 py-2">
        <div
          className="flex items-center gap-2 px-2 py-1.5 rounded-md text-xs"
          style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        >
          <Search size={12} style={{ color: 'var(--text-secondary)' }} />
          <input
            type="text"
            placeholder="搜索会话..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="bg-transparent outline-none flex-1 text-xs"
            style={{ color: 'var(--text-primary)' }}
          />
        </div>
      </div>

      {/* Filter */}
      <div className="px-3 pb-2 flex gap-1">
        <button
          onClick={() => { if (showArchived) toggleArchived(); }}
          className="px-2 py-0.5 rounded text-xs transition-colors"
          style={{
            backgroundColor: !showArchived ? 'var(--accent)' : 'transparent',
            color: !showArchived ? '#fff' : 'var(--text-secondary)',
          }}
        >
          活跃
        </button>
        <button
          onClick={() => { if (!showArchived) toggleArchived(); }}
          className="px-2 py-0.5 rounded text-xs transition-colors flex items-center gap-1"
          style={{
            backgroundColor: showArchived ? 'var(--accent)' : 'transparent',
            color: showArchived ? '#fff' : 'var(--text-secondary)',
          }}
        >
          <Archive size={10} />
          归档
        </button>
      </div>

      {/* Session List */}
      <div className="flex-1 overflow-y-auto">
        {isLoadingSessions ? (
          <div className="flex items-center justify-center py-8">
            <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</p>
          </div>
        ) : filteredList.length === 0 ? (
          <div className="flex items-center justify-center py-8">
            <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '无匹配结果' : '暂无会话'}
            </p>
          </div>
        ) : (
          <>
            {showArchived ? (
              renderGroup('已归档', filteredList)
            ) : (
              <>
                {renderGroup('今天', today)}
                {renderGroup('昨天', yesterday)}
                {renderGroup('更早', earlier)}
              </>
            )}
          </>
        )}
      </div>

      {/* New Session Dialog */}
      {showNewDialog && (
        <div
          className="absolute inset-0 flex items-center justify-center z-10"
          style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
          onClick={() => setShowNewDialog(false)}
        >
          <div
            className="rounded-lg p-4 w-48"
            style={{ backgroundColor: 'var(--bg-panel)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <p className="text-xs font-medium mb-3">选择 Agent 类型</p>
            <div className="flex flex-col gap-2">
              <button
                onClick={async () => {
                  await createSession('claude');
                  setShowNewDialog(false);
                }}
                className="flex items-center gap-2 px-3 py-2 rounded text-xs transition-colors"
                style={{
                  backgroundColor: 'rgba(59,130,246,0.1)',
                  color: '#3B82F6',
                  border: '1px solid rgba(59,130,246,0.3)',
                }}
              >
                <span className="w-5 h-5 rounded bg-blue-500/20 text-blue-400 flex items-center justify-center text-[10px] font-bold">C</span>
                Claude Code
              </button>
              <button
                onClick={async () => {
                  await createSession('hermes');
                  setShowNewDialog(false);
                }}
                className="flex items-center gap-2 px-3 py-2 rounded text-xs transition-colors"
                style={{
                  backgroundColor: 'rgba(139,92,246,0.1)',
                  color: '#8B5CF6',
                  border: '1px solid rgba(139,92,246,0.3)',
                }}
              >
                <span className="w-5 h-5 rounded bg-purple-500/20 text-purple-400 flex items-center justify-center text-[10px] font-bold">H</span>
                Hermes Agent
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}
